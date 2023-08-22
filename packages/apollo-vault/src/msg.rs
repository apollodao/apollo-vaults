use cosmwasm_schema::cw_serde;
use cosmwasm_std::{to_binary, Addr, CosmosMsg, Env, StdResult, Uint128, WasmMsg};
#[cfg(feature = "force-unlock")]
use cw_vault_standard::extensions::force_unlock::ForceUnlockExecuteMsg;
#[cfg(feature = "lockup")]
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg};
use cw_vault_standard::msg::{VaultStandardExecuteMsg, VaultStandardQueryMsg};

use crate::state::{Config, ConfigUpdates};

/// ExecuteMsg for an Autocompounding Vault.
pub type ExecuteMsg = VaultStandardExecuteMsg<ExtensionExecuteMsg>;

/// QueryMsg for an Autocompounding Vault.
pub type QueryMsg = VaultStandardQueryMsg<ExtensionQueryMsg>;

/// Extension execute messages for an apollo autocompounding vault
#[cw_serde]
pub enum ExtensionExecuteMsg {
    /// Execute a callback message.
    Callback(CallbackMsg),
    /// Execute a an Apollo vault specific message.
    Apollo(ApolloExtensionExecuteMsg),
    /// Execute a message from the lockup extension.
    #[cfg(feature = "lockup")]
    Lockup(LockupExecuteMsg),
    /// Execute a message from the force unlock extension.
    #[cfg(feature = "force-unlock")]
    ForceUnlock(ForceUnlockExecuteMsg),
}

/// Callback messages for the autocompounding vault `Callback` extension
#[cw_serde]
pub enum CallbackMsg {
    /// Sell all the rewards in the contract to the underlying tokens of the
    /// pool.
    SellRewards {},
    /// Provide liquidity with all the underlying tokens of the pool currently
    /// in the contract.
    ProvideLiquidity {},
    /// Stake all base tokens in the contract.
    Stake {
        /// Contract base token balance before this transaction started. E.g. if
        /// funds were sent to the contract as part of the `info.funds` or
        /// received as cw20s in a previous message they must be deducted from
        /// the current contract balance.
        base_token_balance_before: Uint128,
    },
    /// Mint vault tokens
    MintVaultToken {
        /// The amount of base tokens to deposit.
        amount: Uint128,
        /// The recipient of the vault token.
        recipient: Addr,
    },
    /// Redeem vault tokens for base tokens.
    #[cfg(feature = "redeem")]
    Redeem {
        /// The address which should receive the withdrawn base tokens.
        recipient: Addr,
        /// The amount of vault tokens sent to the contract. In the case that
        /// the vault token is a Cosmos native denom, we of course have this
        /// information in the info.funds, but if the vault implements the
        /// Cw4626 API, then we need this argument. We figured it's
        /// better to have one API for both types of vaults, so we
        /// require this argument.
        amount: Uint128,
    },
    /// Burn vault tokens and start the unlocking process.
    #[cfg(feature = "lockup")]
    Unlock {
        /// The address that will be the owner of the unlocking position.
        owner: Addr,
        /// The amount of vault tokens to burn.
        vault_token_amount: Uint128,
    },
    /// Save the currently pending claim to the `claims` storage.
    #[cfg(feature = "lockup")]
    SaveClaim {},
}

impl CallbackMsg {
    /// Convert the callback message to a [`CosmosMsg`]. The message will be
    /// formatted as a `Callback` extension in a [`VaultStandardExecuteMsg`],
    /// accordning to the
    /// [CosmWasm Vault Standard](https://docs.rs/cosmwasm-vault-standard/0.1.0/cosmwasm_vault_standard/#how-to-use-extensions).
    pub fn into_cosmos_msg(&self, env: &Env) -> StdResult<CosmosMsg> {
        Ok(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: env.contract.address.to_string(),
            msg: to_binary(&VaultStandardExecuteMsg::VaultExtension(
                ExtensionExecuteMsg::Callback(self.clone()),
            ))?,
            funds: vec![],
        }))
    }
}

/// Apollo extension messages define functionality that is part of all apollo
/// vaults, but not part of the standard.
#[cw_serde]
pub enum ApolloExtensionExecuteMsg {
    /// Update the configuration of the vault.
    UpdateConfig {
        /// The config updates.
        updates: ConfigUpdates,
    },
    /// Update the vault admin.
    UpdateAdmin {
        /// The new admin address.
        address: String,
    },
    /// Accept the admin transfer. This must be called by the new admin to
    /// finalize the transfer.
    AcceptAdminTransfer {},
    /// Removes the initiated admin transfer. This can only be called by the
    /// admin who initiated the admin transfer.
    DropAdminTransfer {},
}

/// Apollo extension queries define functionality that is part of all apollo
/// vaults, but not part of the standard.
#[cw_serde]
pub enum ApolloExtensionQueryMsg {
    /// Query the current state of the vault.
    State {},
}

/// Extension query messages for an apollo autocompounding vault
#[cw_serde]
pub enum ExtensionQueryMsg {
    /// Queries related to the lockup extension.
    #[cfg(feature = "lockup")]
    Lockup(LockupQueryMsg),
    /// Apollo extension queries.
    Apollo(ApolloExtensionQueryMsg),
}

/// Response struct containing information about the current state of the vault.
/// Returned by the `AutocompoundingVault::query_state`.
#[cw_serde]
pub struct StateResponse<S, P, V> {
    /// The admin address. `None` if the admin is not set.
    pub admin: Option<Addr>,
    /// The config of the vault.
    pub config: Config,
    /// The amount of base tokens staked by the vault.
    pub total_staked_base_tokens: Uint128,
    /// The staking struct. This must implement [`cw_dex::traits::Staking`].
    pub staking: S,
    /// The pool struct. This must implement [`cw_dex::traits::Pool`].
    pub pool: P,
    /// The vault struct. This must implement
    /// [`cw_vault_token::traits::VaultToken`].
    pub vault_token: V,
    /// The total supply of the vault token.
    pub vault_token_supply: Uint128,
}
