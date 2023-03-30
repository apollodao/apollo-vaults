use crate::AutocompoundingVault;
use cosmwasm_std::Env;
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::msg::StateResponse;
use cosmwasm_std::{Deps, StdResult};

impl<'a, S, P, V> AutocompoundingVault<'a, S, P, V>
where
    S: Serialize + DeserializeOwned,
    P: Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Returns the current state of the contract.
    pub fn query_state(&self, deps: Deps, _env: Env) -> StdResult<StateResponse<S, P, V>> {
        let admin = self.admin.get(deps)?;
        let total_staked_base_tokens = self
            .base_vault
            .total_staked_base_tokens
            .load(deps.storage)?;

        let vault_token = self.base_vault.vault_token.load(deps.storage)?;
        let vault_token_supply = vault_token.query_total_supply(deps)?;

        let config = self.config.load(deps.storage)?;
        let staking = self.staking.load(deps.storage)?;
        let pool = self.pool.load(deps.storage)?;

        Ok(StateResponse {
            admin,
            total_staked_base_tokens,
            vault_token,
            vault_token_supply,
            config,
            staking,
            pool,
        })
    }
}
