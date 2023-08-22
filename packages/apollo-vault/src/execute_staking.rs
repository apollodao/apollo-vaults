use apollo_utils::assets::receive_asset;
use apollo_utils::responses::merge_responses;
use cosmwasm_std::{attr, Addr, Coin, DepsMut, Env, Event, MessageInfo, Response, Uint128};

use cw_dex::traits::{Pool, Stake};

use apollo_cw_asset::{Asset, AssetInfo};
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::msg::CallbackMsg;
use crate::AutocompoundingVault;

use crate::error::ContractError;

/// ExecuteMsg handlers for vault thats that are able to stake the base token.
/// This has a trait bound Stake on the S generic.
impl<S, P, V> AutocompoundingVault<'_, S, P, V>
where
    S: Stake + Serialize + DeserializeOwned,
    P: Pool + Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Deposit base tokens into the vault. This will first compound the pending
    /// rewards, then the deposited tokens will be staked and vault tokens
    /// will be minted to the `info.sender`.
    ///
    /// ## Arguments
    /// - amount: Amount of base tokens to deposit.
    /// - recipient: Optional address to receive the minted vault tokens. If
    ///   None, the `info.sender` will be used instead.
    pub fn execute_deposit(
        &self,
        deps: DepsMut,
        env: Env,
        info: &MessageInfo,
        amount: Uint128,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        // Unwrap recipient or use caller's address
        let recipient =
            recipient.map_or(Ok(info.sender.clone()), |x| deps.api.addr_validate(&x))?;

        // Receive the assets to the contract
        let receive_res = receive_asset(
            info,
            &env,
            &Asset::new(self.base_vault.base_token.load(deps.storage)?, amount),
        )?;

        // Check that only the expected amount of base token was sent
        if info.funds.len() > 1 {
            return Err(ContractError::UnexpectedFunds {
                expected: vec![Coin {
                    denom: self.base_vault.base_token.load(deps.storage)?.to_string(),
                    amount,
                }],
                actual: info.funds.clone(),
            });
        }

        // If base token is a native token it was sent in the `info.funds` and is
        // already part of the contract balance. That is not the case for a cw20 token,
        // which will be received when the above `receive_res` is handled.
        let user_deposit_amount = match self.base_vault.base_token.load(deps.storage)? {
            AssetInfo::Cw20(_) => Uint128::zero(),
            AssetInfo::Native(_) => amount,
        };

        // Compound. Also stakes the users deposit
        let compound_res = self.compound(deps, &env, user_deposit_amount)?;

        // Mint vault tokens to recipient
        let mint_res = Response::new().add_message(
            CallbackMsg::MintVaultToken {
                amount,
                recipient: recipient.clone(),
            }
            .into_cosmos_msg(&env)?,
        );

        let event = Event::new("apollo/vaults/execute_staking").add_attributes(vec![
            attr("action", "deposit"),
            attr("recipient", recipient),
            attr("amount", amount),
        ]);

        // Merge responses and add message to mint vault token
        Ok(merge_responses(vec![receive_res, compound_res, mint_res]).add_event(event))
    }

    /// Callback function to mint `amount` of vault tokens to
    /// `vault_token_recipient`. Called from the `execute_deposit` function.
    pub fn execute_callback_mint_vault_token(
        &self,
        deps: DepsMut,
        env: Env,
        amount: Uint128,
        vault_token_recipient: Addr,
    ) -> Result<Response, ContractError> {
        // Load state
        let vault_token = self.base_vault.vault_token.load(deps.storage)?;
        let total_staked_amount = self
            .base_vault
            .total_staked_base_tokens
            .load(deps.storage)?;
        let vault_token_supply = vault_token.query_total_supply(deps.as_ref())?;

        // Calculate how many base tokens the given amount of vault tokens represents
        // Here we must subtract the deposited amount from `total_staked_amount` because
        // it was already incremented in `execute_callback_stake` during the compound.
        let vault_tokens = self.base_vault.calculate_vault_tokens(
            amount,
            total_staked_amount.checked_sub(amount)?,
            vault_token_supply,
        )?;

        let event = Event::new("apollo/vaults/execute_staking").add_attributes(vec![
            attr("action", "execute_callback_mint_vault_token"),
            attr("recipient", vault_token_recipient.to_string()),
            attr("mint_amount", vault_tokens),
        ]);

        // Return Response with message to mint vault tokens
        Ok(vault_token
            .mint(deps, &env, &vault_token_recipient, vault_tokens)?
            .add_event(event))
    }
}
