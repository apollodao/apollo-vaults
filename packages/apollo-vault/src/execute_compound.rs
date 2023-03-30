use apollo_cw_asset::{Asset, AssetList};
use cosmwasm_std::{
    attr, to_binary, Decimal, DepsMut, Env, Event, MessageInfo, Response, StdError, StdResult,
    Uint128,
};
use cw_dex::traits::{Pool, Stake};
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::ContractError;
use crate::msg::CallbackMsg;
use crate::AutocompoundingVault;

impl<S, P, V> AutocompoundingVault<'_, S, P, V>
where
    S: Stake + Serialize + DeserializeOwned,
    P: Pool + Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Claim rewards and compound them back into the base token. This will
    /// compound the pending rewards into base tokens and stake them plus the
    /// `user_deposit_amount`.
    ///
    /// # Arguments
    /// - `user_deposit_amount` - Amount of base tokens in the contract that
    ///   come from the user deposit. If this is called as part of a withdrawal
    ///   this should be 0.
    pub fn compound(
        &self,
        deps: DepsMut,
        env: &Env,
        user_deposit_amount: Uint128,
    ) -> Result<Response, ContractError> {
        let staking = self.staking.load(deps.storage)?;

        // Claim pending rewards
        let claim_rewards_res = staking.claim_rewards(deps.as_ref(), env)?;

        // Sell rewards
        let sell_rewards = CallbackMsg::SellRewards {}.into_cosmos_msg(env)?;

        // Provide liquidity
        let provide_liquidity = CallbackMsg::ProvideLiquidity {}.into_cosmos_msg(env)?;

        // Get the base token balance
        let base_token_balance = self
            .base_vault
            .base_token
            .load(deps.storage)?
            .query_balance(&deps.querier, &env.contract.address)?;

        // Stake LP tokens. Base token balance before is the contract balance before
        // user deposit.
        let stake = CallbackMsg::Stake {
            base_token_balance_before: base_token_balance.checked_sub(user_deposit_amount)?,
        }
        .into_cosmos_msg(env)?;

        let event = Event::new("apollo/vaults/execute_compound").add_attributes(vec![
            attr("action", "compound"),
            attr("user_deposit_amount", user_deposit_amount),
            attr("base_token_balance", base_token_balance),
        ]);

        Ok(claim_rewards_res
            .add_message(sell_rewards)
            .add_message(provide_liquidity)
            .add_message(stake)
            .add_event(event))
    }

    /// Sells all the reward tokens in the contract for the underlying tokens of
    /// the pool in proportion to the current balance of the pool.
    pub fn execute_callback_sell_rewards(
        &self,
        deps: DepsMut,
        env: Env,
        _info: MessageInfo,
    ) -> Result<Response, ContractError> {
        let cfg = self.config.load(deps.storage)?;
        let reward_assets = cfg.reward_assets;
        let pool_assets = self.pool.load(deps.storage)?.pool_assets(deps.as_ref())?;
        let treasury = cfg.treasury;
        let performance_fee = cfg.performance_fee;
        let base_token = &self.base_vault.base_token.load(deps.storage)?;

        // AssetList of reward tokens collected from performance fees
        let mut reward_asset_balances_to_treasury = AssetList::new();

        let reward_assets_to_sell: AssetList = reward_assets
            .into_iter()
            .map(|x| {
                // Take performance fee from each reward asset
                let balance = x.query_balance(&deps.querier, env.contract.address.clone())?;
                let balance_after_fee = balance * (Decimal::one() - performance_fee);
                let balance_sent_to_treasury = balance.checked_sub(balance_after_fee)?;
                reward_asset_balances_to_treasury
                    .add(&Asset::new(x.clone(), balance_sent_to_treasury))?;
                Ok(Asset::new(x, balance_after_fee))
            })
            .collect::<StdResult<Vec<_>>>()?
            .into_iter()
            .filter(|x| x.amount != Uint128::zero()) // Filter out assets with 0 balance
            //We only want to swap the reward assets that are not in the pair
            //and that are not the base_token (although that is unlikely)
            .filter(|x| !pool_assets.contains(&x.info) && &x.info != base_token)
            .collect::<Vec<_>>()
            .into();

        // Send performance fees to treasury
        let mut msgs = reward_asset_balances_to_treasury
            .into_iter()
            .filter(|x| x.amount != Uint128::zero()) // Filter out assets with 0 balance
            .map(|x| x.transfer_msg(treasury.to_string()))
            .collect::<StdResult<Vec<_>>>()?;

        let mut event = Event::new("apollo/vaults/execute_compound")
            .add_attribute("action", "execute_callback_sell_rewards");
        if reward_asset_balances_to_treasury.len() > 0 {
            event = event.add_attribute(
                "reward_asset_balances_to_treasury",
                reward_asset_balances_to_treasury.to_string(),
            );
        }

        // Swap all other reward assets
        if reward_assets_to_sell.len() > 0 {
            let mut swap_msgs = cfg.router.basket_liquidate_msgs(
                reward_assets_to_sell.clone(),
                &cfg.reward_liquidation_target,
                None,
                None,
            )?;
            msgs.append(&mut swap_msgs);
            event = event.add_attribute("reward_assets_to_sell", reward_assets_to_sell.to_string());
        }

        Ok(Response::new().add_messages(msgs).add_event(event))
    }

    /// Provides liquidity to the pool with all the underlying tokens in the
    /// contract.
    pub fn execute_callback_provide_liquidity(
        &self,
        deps: DepsMut,
        env: Env,
        _info: MessageInfo,
    ) -> Result<Response, ContractError> {
        let cfg = self.config.load(deps.storage)?;
        let pool = self.pool.load(deps.storage)?;

        let contract_assets: AssetList = pool
            .pool_assets(deps.as_ref())?
            .into_iter()
            .map(|a| {
                Ok(Asset {
                    info: a.clone(),
                    amount: a.query_balance(&deps.querier, env.contract.address.clone())?,
                })
            })
            .collect::<StdResult<Vec<_>>>()?
            .into_iter()
            .filter(|x| x.amount != Uint128::zero()) // Filter out assets with 0 balance
            .collect::<Vec<_>>()
            .into();

        // No assets to provide liquidity with
        if contract_assets.len() == 0 {
            return Ok(Response::default());
        }

        let provide_liquidity_msgs = cfg.liquidity_helper.balancing_provide_liquidity(
            contract_assets.clone(),
            Uint128::zero(),
            to_binary(&pool)?,
            None,
        )?;

        let event = Event::new("apollo/vaults/execute_compound").add_attributes(vec![
            attr("action", "execute_callback_provide_liquidity"),
            attr("contract_assets", contract_assets.to_string()),
        ]);

        Ok(Response::new()
            .add_messages(provide_liquidity_msgs)
            .add_event(event))
    }

    /// Callback function to stake the LP tokens in the contract. Stakes the
    /// entire balance of base tokens in the contract.
    ///
    /// This is called after compounding. Since we do not know how many base
    /// tokens we receive from the liquidity provision we call this as a
    /// callback to ensure that we stake the entire balance.
    pub fn execute_callback_stake(
        &self,
        deps: DepsMut,
        env: Env,
        base_token_balance_before: Uint128,
    ) -> Result<Response, ContractError> {
        let base_token_balance = self
            .base_vault
            .base_token
            .load(deps.storage)?
            .query_balance(&deps.querier, env.contract.address.clone())?;

        // Calculate amount to stake
        let amount_to_stake = base_token_balance
            .checked_sub(base_token_balance_before)
            .unwrap_or_default();

        // No base tokens to stake
        if amount_to_stake.is_zero() {
            return Ok(Response::default());
        }

        // Update total_staked_base_tokens with amount from compound
        self.base_vault
            .total_staked_base_tokens
            .update(deps.storage, |old_value| {
                old_value
                    .checked_add(amount_to_stake)
                    .map_err(StdError::overflow)
            })?;

        // We stake the entire base_token_balance, which means we don't have to
        // issue this call again in execute_callback_deposit.
        let res = self
            .staking
            .load(deps.storage)?
            .stake(deps.as_ref(), &env, amount_to_stake)?;

        let event = Event::new("apollo/vaults/execute_compound").add_attributes(vec![
            attr("action", "execute_callback_stake"),
            attr("amount_to_stake", amount_to_stake.to_string()),
            attr("base_token_balance", base_token_balance.to_string()),
            attr(
                "base_token_balance_before",
                base_token_balance_before.to_string(),
            ),
        ]);

        Ok(res.add_event(event))
    }
}
