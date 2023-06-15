use apollo_vault::msg::{ExtensionExecuteMsg, ExtensionQueryMsg};
use apollo_vault::state::ConfigUnchecked;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::Coin;
use cw_vault_standard::{VaultStandardExecuteMsg, VaultStandardQueryMsg};

/// ExecuteMsg for an Autocompounding Vault.
pub type ExecuteMsg = VaultStandardExecuteMsg<ExtensionExecuteMsg>;

/// QueryMsg for an Autocompounding Vault.
pub type QueryMsg = VaultStandardQueryMsg<ExtensionQueryMsg>;

#[cw_serde]
pub struct InstantiateMsg {
    /// Address that is allowed to update config.
    pub admin: String,
    /// The addr of the pool that this vault will autocompound.
    pub pool_addr: String,
    /// Configurable parameters for the contract.
    pub config: ConfigUnchecked,
    /// The subdenom that will be used for the native vault token, e.g.
    /// the denom of the vault token will be:
    /// "factory/{vault_contract}/{vault_token_subdenom}".
    pub vault_token_subdenom: String,
    /// Token creation fee
    pub token_creation_fee: Coin,
    /// ASTRO token address
    pub astro_token_addr: String,
    /// Astroport generator address
    pub generator_addr: String,
}

#[cw_serde]
pub struct MigrateMsg {}
