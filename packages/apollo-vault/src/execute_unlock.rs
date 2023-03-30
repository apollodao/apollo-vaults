use crate::error::ContractError;
use crate::msg::CallbackMsg;
use crate::AutocompoundingVault;
use apollo_utils::responses::merge_responses;
use cosmwasm_std::{
    attr, Addr, Deps, DepsMut, Env, Event, MessageInfo, Response, StdResult, Uint128,
};
use cw_dex::traits::{LockedStaking, Pool};
use cw_vault_standard::extensions::lockup::{
    UnlockingPosition, UNLOCKING_POSITION_ATTR_KEY, UNLOCKING_POSITION_CREATED_EVENT_TYPE,
};
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// ExecuteMsg handlers related to vaults that have a lockup. Here we have the
/// trait bound Unlock on the S generic.
impl<S, P, V> AutocompoundingVault<'_, S, P, V>
where
    S: LockedStaking + Serialize + DeserializeOwned,
    P: Pool + Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Withdraw the base tokens from a locked position that has finished
    /// unlocking.
    ///
    /// ## Arguments
    /// - lockup_id: ID of the lockup position to withdraw from.
    /// - recipient: Optional address to receive the withdrawn base tokens. If
    ///   `None` is provided `info.sender` will be used instead.
    pub fn execute_withdraw_unlocked(
        &self,
        deps: DepsMut,
        env: Env,
        info: &MessageInfo,
        lockup_id: u64,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        // Unwrap recipient or use caller's address
        let recipient =
            recipient.map_or(Ok(info.sender.clone()), |x| deps.api.addr_validate(&x))?;

        let sum_to_claim = self
            .claims
            .claim_tokens(deps.storage, &env.block, info, lockup_id)?;

        let res = self.staking.load(deps.storage)?.withdraw_unlocked(
            deps.as_ref(),
            &env,
            sum_to_claim,
        )?;

        let event = Event::new("apollo/vaults/execute_unlock").add_attributes(vec![
            attr("action", "execute_withdraw_unlocked"),
            attr("recipient", recipient.clone()),
            attr("lockup_id", lockup_id.to_string()),
            attr("amount", sum_to_claim),
        ]);

        Ok(merge_responses(vec![
            res,
            self.base_vault
                .send_base_tokens(deps, &recipient, sum_to_claim)?,
        ])
        .add_event(event))
    }

    /// Burn `vault_token_amount` vault tokens and start the unlocking process.
    /// If the vault token is a native token it must be sent in the `info.funds`
    /// field.
    pub fn execute_unlock(
        &self,
        mut deps: DepsMut,
        env: Env,
        info: &MessageInfo,
        vault_token_amount: Uint128,
    ) -> Result<Response, ContractError> {
        let vault_token = self.base_vault.vault_token.load(deps.storage)?;

        // Receive the vault token to the contract's balance, or validate that it was
        // already received
        vault_token.receive(deps.branch(), &env, info, vault_token_amount)?;

        // First compound the vault
        let compound_res = self.compound(deps, &env, Uint128::zero())?;

        // Continue with the unlock after compounding
        let unlock_msg = CallbackMsg::Unlock {
            owner: info.sender.clone(),
            vault_token_amount,
        }
        .into_cosmos_msg(&env)?;

        // Store the claim for base_tokens
        let store_claim_msg = CallbackMsg::SaveClaim {}.into_cosmos_msg(&env)?;

        let event = Event::new("apollo/vaults/execute_unlock").add_attributes(vec![
            attr("action", "execute_unlock"),
            attr("owner", info.sender.to_string()),
            attr("amount", vault_token_amount),
        ]);

        Ok(compound_res
            .add_message(unlock_msg)
            .add_message(store_claim_msg)
            .add_event(event))
    }

    /// Transfer vault tokens to the vault to start unlocking a locked position.
    pub fn execute_callback_unlock(
        &self,
        mut deps: DepsMut,
        env: Env,
        info: MessageInfo,
        owner: Addr,
        vault_token_amount: Uint128,
    ) -> Result<Response, ContractError> {
        let staking = self.staking.load(deps.storage)?;

        // Burn vault tokens and get the amount of base tokens to withdraw
        let (lp_tokens_to_unlock, burn_res) = self.base_vault.burn_vault_tokens_for_base_tokens(
            deps.branch(),
            &env,
            vault_token_amount,
        )?;

        let expiration = self
            .staking
            .load(deps.storage)?
            .get_lockup_duration(deps.as_ref())?
            .after(&env.block);

        // Create a pending claim for using the default ID.
        self.claims.create_pending_claim(
            deps.storage,
            &owner,
            lp_tokens_to_unlock,
            expiration,
            None,
        )?;

        // Unstake response
        let unlock_res = staking.unlock(deps.as_ref(), &env, lp_tokens_to_unlock)?;

        // Event containing the lockup id and claim
        let event = Event::new("apollo/vaults/execute_unlock").add_attributes(vec![
            ("action", "execute_callback_unlock"),
            ("sender", info.sender.as_ref()),
            ("owner", owner.as_ref()),
            ("vault_token_amount", &vault_token_amount.to_string()),
            ("lp_tokens_to_unlock", &lp_tokens_to_unlock.to_string()),
        ]);

        // Create response.
        // We also send the lockup_id back in the data field so that the caller
        // can read it easily in a SubMsg reply.
        Ok(merge_responses(vec![burn_res, unlock_res]).add_event(event))
    }

    /// Callback function to save a pending claim to the claims store.
    pub fn execute_callback_save_claim(&self, deps: DepsMut) -> Result<Response, ContractError> {
        let claim = self.claims.get_pending_claim(deps.storage)?;

        // Commit the pending claim
        self.claims.commit_pending_claim(deps.storage)?;

        let event = Event::new(UNLOCKING_POSITION_CREATED_EVENT_TYPE)
            .add_attribute("action", "execute_callback_save_claim")
            .add_attribute("unlock_amount", claim.base_token_amount.to_string())
            .add_attribute("owner", claim.owner)
            .add_attribute("release_at", claim.release_at.to_string())
            .add_attribute(UNLOCKING_POSITION_ATTR_KEY, claim.id.to_string());

        Ok(Response::default().add_event(event))
    }

    /// Query unlocking positions for `owner`. Optional arguments `start_after`
    /// and `limit` can be used for pagination.
    ///
    /// ## Arguments
    /// - owner: Address of the owner of the lockup positions.
    /// - start_after: Optional ID of the lockup position to start the query
    /// - limit: Optional maximum number of lockup positions to return.
    pub fn query_unlocking_positions(
        &self,
        deps: Deps,
        owner: String,
        start_after: Option<u64>,
        limit: Option<u32>,
    ) -> StdResult<Vec<UnlockingPosition>> {
        let owner = deps.api.addr_validate(&owner)?;
        let claims = self
            .claims
            .query_claims_for_owner(deps, &owner, start_after, limit)?;
        Ok(claims.into_iter().map(|(_, lockup)| lockup).collect())
    }
}
