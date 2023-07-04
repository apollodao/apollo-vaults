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
    /// Contract address of the under lying pool
    pub pair_addr: String,
    /// The lockup duration in seconds that this vault will use when staking
    /// LP tokens.
    pub lockup_duration: u64,
    /// Configurable parameters for the contract.
    pub config: ConfigUnchecked,
    /// The subdenom that will be used for the native vault token, e.g.
    /// the denom of the vault token will be:
    /// "factory/{vault_contract}/{vault_token_subdenom}".
    pub vault_token_subdenom: String,
    /// Fee required to create a new token with the token factory.
    pub token_creation_fee: Coin,
    /// Contract address of the astroport generator.
    pub astroport_generator: String,
    /// ASTRO token address.
    pub astro_token: String,
}

#[cw_serde]
pub struct MigrateMsg {}
