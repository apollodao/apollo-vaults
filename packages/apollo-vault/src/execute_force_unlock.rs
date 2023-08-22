use std::collections::HashSet;

use apollo_utils::responses::merge_responses;
use cosmwasm_std::{attr, Addr, DepsMut, Env, Event, MessageInfo, Response, Uint128};
use cw_dex::traits::{ForceUnlock, Pool};
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::ContractError;
use crate::AutocompoundingVault;

impl<S, P, V> AutocompoundingVault<'_, S, P, V>
where
    S: ForceUnlock + Serialize + DeserializeOwned,
    P: Pool + Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Force withdrawal of a locked position. Called only by whitelisted
    /// addresses in the event of liquidation.
    pub fn execute_force_redeem(
        &self,
        mut deps: DepsMut,
        env: Env,
        info: MessageInfo,
        vault_token_amount: Uint128,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        let cfg = self.config.load(deps.storage)?;
        let vault_token = self.base_vault.vault_token.load(deps.storage)?;

        // Receive the vault token to the contract's balance, or validate that it was
        // already received
        vault_token.receive(deps.branch(), &env, &info, vault_token_amount)?;

        // Unwrap recipient or use caller's address
        let recipient =
            recipient.map_or(Ok(info.sender.clone()), |x| deps.api.addr_validate(&x))?;

        // Check ForceWithdraw whitelist
        let whitelist = cfg.force_withdraw_whitelist;
        if !whitelist.contains(&info.sender) {
            return Err(ContractError::Unauthorized {});
        }

        // Burn vault tokens and get the amount of base tokens to withdraw
        let (lp_tokens_to_unlock, burn_res) = self.base_vault.burn_vault_tokens_for_base_tokens(
            deps.branch(),
            &env,
            vault_token_amount,
        )?;

        // Call force withdraw on staked LP
        let staking = self.staking.load(deps.storage)?;
        let force_withdraw_res =
            staking.force_unlock(deps.as_ref(), &env, None, lp_tokens_to_unlock)?;

        // Send the unstaked tokens to the recipient
        let send_res = self
            .base_vault
            .send_base_tokens(deps, &recipient, lp_tokens_to_unlock)?;

        let event = Event::new("apollo/vaults/execute_force_unlock").add_attributes(vec![
            attr("action", "execute_force_redeem"),
            attr("recipient", recipient),
            attr("vault_token_amount", vault_token_amount),
            attr("redeem_amount", lp_tokens_to_unlock),
        ]);

        Ok(merge_responses(vec![burn_res, force_withdraw_res, send_res]).add_event(event))
    }

    /// Force withdrawal of an unlocking position. Can only be called only by
    /// whitelisted addresses.
    pub fn execute_force_withdraw_unlocking(
        &self,
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        lockup_id: u64,
        amount: Option<Uint128>,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        let cfg = self.config.load(deps.storage)?;

        // Unwrap recipient or use caller's address
        let recipient =
            recipient.map_or(Ok(info.sender.clone()), |x| deps.api.addr_validate(&x))?;

        // Check ForceWithdraw whitelist
        let whitelist = cfg.force_withdraw_whitelist;
        if !whitelist.contains(&info.sender) {
            return Err(ContractError::Unauthorized {});
        }

        // Check if the lockup is expired. We must do this before calling
        // force_claim, as it may delete the claim if all of the tokens are claimed.
        let is_expired = self
            .claims
            .query_claim_by_id(deps.as_ref(), lockup_id)?
            .release_at
            .is_expired(&env.block);

        // Get the claimed amount and update the claim in storage, deleting it if
        // all of the tokens are claimed, or updating it with the remaining amount.
        let claimed_amount = self
            .claims
            .force_claim(deps.storage, &info, lockup_id, amount)?;

        // If the lockup is not expired, call force withdraw to retrieve the
        // locked tokens.
        // If the lockup is already expired the tokens are already unlocked and
        // already sent to this contract.
        let force_withdraw_res = if !is_expired {
            let staking = self.staking.load(deps.storage)?;
            staking.force_unlock(deps.as_ref(), &env, Some(lockup_id), claimed_amount)?
        } else {
            Response::default()
        };

        // Send the unstaked tokens to the recipient
        let send_res = self
            .base_vault
            .send_base_tokens(deps, &recipient, claimed_amount)?;

        let event = Event::new("apollo/vaults/execute_force_unlock").add_attributes(vec![
            attr("action", "execute_force_withdraw_unlocking"),
            attr("recipient", recipient),
            attr("lockup_id", lockup_id.to_string()),
            attr("claimed_amount", claimed_amount),
        ]);

        Ok(merge_responses(vec![force_withdraw_res, send_res]).add_event(event))
    }

    /// Update the whitelist of addresses that can force withdraw from the
    /// vault.
    pub fn execute_update_force_withdraw_whitelist(
        &self,
        deps: DepsMut,
        info: MessageInfo,
        add_addresses: Vec<String>,
        remove_addresses: Vec<String>,
    ) -> Result<Response, ContractError> {
        self.admin.assert_admin(deps.as_ref(), &info.sender)?;

        let mut cfg = self.config.load(deps.storage)?;
        let whitelist = cfg.force_withdraw_whitelist;

        //Check if addresses are valid
        let add_addresses: Vec<Addr> = add_addresses
            .into_iter()
            .map(|x| deps.api.addr_validate(&x))
            .collect::<Result<Vec<Addr>, _>>()?;
        let remove_addresses: Vec<Addr> = remove_addresses
            .into_iter()
            .map(|x| deps.api.addr_validate(&x))
            .collect::<Result<Vec<Addr>, _>>()?;

        //Update whitelist and remove duplicates
        let new_whitelist: Vec<Addr> = whitelist
            .into_iter()
            .filter(|x| !remove_addresses.contains(x))
            .chain(add_addresses.into_iter())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        //Save new whitelist
        cfg.force_withdraw_whitelist = new_whitelist;
        self.config.save(deps.storage, &cfg)?;

        let event = Event::new("apollo/vaults/execute_force_unlock").add_attributes(vec![attr(
            "action",
            "execute_update_force_withdraw_whitelist",
        )]);

        Ok(Response::default().add_event(event))
    }
}
