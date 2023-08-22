use apollo_vault::msg::{ExtensionExecuteMsg, ExtensionQueryMsg};
use apollo_vault::state::ConfigUnchecked;
use cosmwasm_schema::cw_serde;
use cw_vault_standard::extensions::cw4626::{Cw4626ExecuteMsg, Cw4626QueryMsg};
use cw_vault_token::cw4626::Cw4626InstantiateMsg;

pub type ExecuteMsg = Cw4626ExecuteMsg<ExtensionExecuteMsg>;

pub type QueryMsg = Cw4626QueryMsg<ExtensionQueryMsg>;
#[cw_serde]
pub struct InstantiateMsg {
    /// Address that is allowed to update config.
    pub admin: String,
    /// The address of the pool that this vault will autocompound.
    pub pool: String,
    /// Configurable parameters for the contract.
    pub config: ConfigUnchecked,
    /// Cw20 instantiate parameters for the vault token.
    pub init_info: Cw4626InstantiateMsg,
    /// Astroport Generator contract address for the base token
    pub generator: String,
    /// Astro token contract address
    pub astro_token: String,
}
