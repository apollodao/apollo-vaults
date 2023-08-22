use apollo_utils::responses::merge_responses;
use cosmwasm_std::{attr, Addr, DepsMut, Env, Event, MessageInfo, Response, Uint128};

use cw_dex::traits::{Pool, Stake, Unstake};

use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::msg::CallbackMsg;
use crate::AutocompoundingVault;

use crate::error::ContractError;

/// ExecuteMsg handlers for vaults that are able to be unstaked without a
/// lockup. Has the Unstake trait bound on the S generic.
impl<S, P, V> AutocompoundingVault<'_, S, P, V>
where
    S: Stake + Unstake + Serialize + DeserializeOwned,
    P: Pool + Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Redeem vault tokens for base tokens. This will first compound the
    /// pending rewards, then the vault tokens will be burned and the base
    /// tokens will be sent to the recipient. If the vault token is a native
    /// token, the tokens must be sent in the `info.funds` field.
    ///
    /// ## Arguments
    /// - `vault_token_amount`: Amount of vault tokens to redeem.
    /// - `recipient`: Optional address to receive the base tokens. If None, the
    ///   `info.sender` will be used instead.
    pub fn execute_redeem(
        &self,
        mut deps: DepsMut,
        env: Env,
        info: &MessageInfo,
        vault_token_amount: Uint128,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        let vault_token = self.base_vault.vault_token.load(deps.storage)?;

        // Receive the vault token to the contract's balance, or validate that it was
        // already received
        vault_token.receive(deps.branch(), &env, info, vault_token_amount)?;

        // Unwrap recipient or use caller's address
        let recipient =
            recipient.map_or(Ok(info.sender.clone()), |x| deps.api.addr_validate(&x))?;

        let event = Event::new("apollo/vaults/execute_redeem").add_attributes(vec![
            attr("action", "redeem"),
            attr("recipient", recipient.clone()),
            attr("amount", vault_token_amount),
        ]);

        // Compound then redeem
        Ok(self
            .compound(deps, &env, Uint128::zero())?
            .add_message(
                CallbackMsg::Redeem {
                    amount: vault_token_amount,
                    recipient,
                }
                .into_cosmos_msg(&env)?,
            )
            .add_event(event))
    }

    /// Callback function to redeem `amount` of vault tokens for base tokens and
    /// send the base tokens to `recipient`. Called from the `execute_redeem`
    /// function.
    pub fn execute_callback_redeem(
        &self,
        mut deps: DepsMut,
        env: Env,
        vault_token_amount: Uint128,
        recipient: Addr,
    ) -> Result<Response, ContractError> {
        let staking = self.staking.load(deps.storage)?;

        // Burn vault tokens and get the amount of base tokens to withdraw
        let (lp_tokens_to_unstake, burn_res) = self.base_vault.burn_vault_tokens_for_base_tokens(
            deps.branch(),
            &env,
            vault_token_amount,
        )?;

        // Unstakes base tokens
        let unstake_res = staking.unstake(deps.as_ref(), &env, lp_tokens_to_unstake)?;

        // Send unstaked base tokes to recipient
        let send_res = self
            .base_vault
            .send_base_tokens(deps, &recipient, lp_tokens_to_unstake)?;

        let event = Event::new("apollo/vaults/execute_redeem").add_attributes(vec![
            attr("action", "execute_callback_redeem"),
            attr("recipient", recipient),
            attr("vault_token_amount", vault_token_amount),
            attr("lp_tokens_to_unstake", lp_tokens_to_unstake),
        ]);

        Ok(merge_responses(vec![burn_res, unstake_res, send_res]).add_event(event))
    }
}
