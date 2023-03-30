use crate::BaseVault;
use cosmwasm_std::Uint128;
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use cosmwasm_std::Deps;
use cosmwasm_std::StdResult;

impl<V> BaseVault<'_, V>
where
    V: VaultToken + Serialize + DeserializeOwned,
{
    pub fn query_total_vault_token_supply(&self, deps: Deps) -> StdResult<Uint128> {
        let vault_token = self.vault_token.load(deps.storage)?;
        Ok(vault_token.query_total_supply(deps)?)
    }

    pub fn query_vault_token_balance(&self, deps: Deps, address: String) -> StdResult<Uint128> {
        let vault_token = self.vault_token.load(deps.storage)?;
        Ok(vault_token.query_balance(deps, address)?)
    }

    /// Calculate the number of shares minted from a deposit of `assets` base
    /// tokens.
    pub fn query_simulate_deposit(&self, deps: Deps, amount: Uint128) -> StdResult<Uint128> {
        let vault_token_supply = self
            .vault_token
            .load(deps.storage)?
            .query_total_supply(deps)?;
        let total_staked_amount = self.total_staked_base_tokens.load(deps.storage)?;
        self.calculate_vault_tokens(amount, total_staked_amount, vault_token_supply)
            .map_err(Into::into)
    }

    /// Calculate the number of base tokens returned when burning `shares` vault
    /// tokens.
    pub fn query_simulate_withdraw(&self, deps: Deps, amount: Uint128) -> StdResult<Uint128> {
        let vault_token_supply = self
            .vault_token
            .load(deps.storage)?
            .query_total_supply(deps)?;
        let total_staked_amount = self.total_staked_base_tokens.load(deps.storage)?;
        self.calculate_base_tokens(amount, total_staked_amount, vault_token_supply)
            .map_err(Into::into)
    }

    pub fn query_total_assets(&self, deps: Deps) -> StdResult<Uint128> {
        self.total_staked_base_tokens.load(deps.storage)
    }
}
