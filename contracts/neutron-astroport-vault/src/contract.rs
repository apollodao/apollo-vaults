use apollo_vault::error::ContractError;
use apollo_vault::msg::{
    ApolloExtensionExecuteMsg, ApolloExtensionQueryMsg, CallbackMsg, ExtensionExecuteMsg,
    ExtensionQueryMsg,
};
use apollo_vault::AutocompoundingVault;

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_binary, Binary, Deps, DepsMut, Env, Event, MessageInfo, Reply, Response, StdError,
    StdResult, SubMsgResponse, SubMsgResult,
};
use cw2::{get_contract_version, set_contract_version};
use cw_dex::astroport::{AstroportPool, AstroportStaking};
use cw_dex::traits::Pool;
use cw_vault_standard::msg::{VaultInfoResponse, VaultStandardInfoResponse};
use cw_vault_token::osmosis::OsmosisDenom;
use osmosis_std::types::osmosis::lockup::{MsgBeginUnlockingResponse, MsgLockTokensResponse};
use semver::Version;

use crate::msg::{ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:osmosis-vault";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Constants passed to VaultStandardInfo query
const VAULT_STANDARD_VERSION: u16 = 1;
const VAULT_STANDARD_EXTENSIONS: [&str; 2] = ["lockup", "force-unlock"];

pub type NeutronAstroportVaultContract<'a> =
    AutocompoundingVault<'a, AstroportStaking, AstroportPool, OsmosisDenom>;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    let contract = NeutronAstroportVaultContract::default();

    let admin_addr = deps.api.addr_validate(&msg.admin)?;
    let config = msg.config.check(deps.as_ref())?;

    // Validate that 10 osmo for vault token creation are sent
    let token_creation_fee_amount = info
        .funds
        .iter()
        .find(|coin| coin.denom == msg.token_creation_fee.denom)
        .map(|coin| coin.amount)
        .unwrap_or_default();
    if token_creation_fee_amount < msg.token_creation_fee.amount {
        return Err(ContractError::from(format!(
            "A minimum of {} must be sent to create the vault token",
            msg.token_creation_fee
        )));
    }

    // Create the pool object
    let pool = AstroportPool::new(deps.as_ref(), deps.api.addr_validate(&msg.pool_addr)?)?;

    let staking = AstroportStaking {
        lp_token_addr: pool.lp_token_addr.clone(),
        generator_addr: deps.api.addr_validate(&msg.generator_addr)?,
        astro_addr: deps.api.addr_validate(&msg.astro_token_addr)?,
    };

    let vault_token = OsmosisDenom::new(env.contract.address.to_string(), msg.vault_token_subdenom);

    contract.init(deps, admin_addr, pool, staking, config, vault_token, None)
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    let contract = NeutronAstroportVaultContract::default();

    match msg {
        ExecuteMsg::Deposit { amount, recipient } => {
            contract.execute_deposit(deps, env, &info, amount, recipient)
        }
        ExecuteMsg::Redeem {
            recipient: _,
            amount: _,
        } => Err(ContractError::from(
            "Redeem is not supported for locked vaults. Use Unlock and WithdrawUnlocked.",
        )),
        ExecuteMsg::VaultExtension(msg) => match msg {
            ExtensionExecuteMsg::Apollo(msg) => match msg {
                ApolloExtensionExecuteMsg::UpdateConfig { updates } => {
                    contract.execute_update_config(deps, info, updates)
                }
                ApolloExtensionExecuteMsg::UpdateAdmin { address } => {
                    contract.execute_update_admin(deps, info, address)
                }
                ApolloExtensionExecuteMsg::AcceptAdminTransfer {} => {
                    contract.execute_accept_admin_transfer(deps, info)
                }
                ApolloExtensionExecuteMsg::DropAdminTransfer {} => {
                    contract.execute_drop_admin_transfer(deps, info)
                }
            },
            ExtensionExecuteMsg::Callback(msg) => {
                // Assert that only the contract itself can call this
                if info.sender != env.contract.address {
                    return Err(ContractError::Unauthorized {});
                }

                match msg {
                    CallbackMsg::SellRewards {} => {
                        contract.execute_callback_sell_rewards(deps, env, info)
                    }
                    CallbackMsg::ProvideLiquidity {} => {
                        contract.execute_callback_provide_liquidity(deps, env, info)
                    }
                    CallbackMsg::Stake {
                        base_token_balance_before,
                    } => contract.execute_callback_stake(deps, env, base_token_balance_before),
                    CallbackMsg::MintVaultToken { amount, recipient } => {
                        contract.execute_callback_mint_vault_token(deps, env, amount, recipient)
                    }
                    CallbackMsg::Redeem { recipient, amount } => todo!(),
                }
            }
        },
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    let contract = NeutronAstroportVaultContract::default();
    let base_vault = &contract.base_vault;

    match msg {
        QueryMsg::VaultStandardInfo {} => to_binary(&VaultStandardInfoResponse {
            version: VAULT_STANDARD_VERSION,
            extensions: VAULT_STANDARD_EXTENSIONS
                .iter()
                .map(|&s| s.into())
                .collect(),
        }),
        QueryMsg::Info {} => {
            let vault_token = base_vault.vault_token.load(deps.storage)?;
            let base_token = base_vault.base_token.load(deps.storage)?;

            to_binary(&VaultInfoResponse {
                base_token: base_token.to_string(),
                vault_token: vault_token.to_string(),
            })
        }
        QueryMsg::PreviewDeposit { amount } => {
            to_binary(&base_vault.query_simulate_deposit(deps, amount)?)
        }
        QueryMsg::PreviewRedeem { amount } => {
            to_binary(&base_vault.query_simulate_withdraw(deps, amount)?)
        }
        QueryMsg::TotalAssets {} => to_binary(&base_vault.query_total_assets(deps)?),
        QueryMsg::TotalVaultTokenSupply {} => {
            to_binary(&base_vault.query_total_vault_token_supply(deps)?)
        }
        QueryMsg::ConvertToShares { amount } => {
            to_binary(&base_vault.query_simulate_deposit(deps, amount)?)
        }
        QueryMsg::ConvertToAssets { amount } => {
            to_binary(&base_vault.query_simulate_withdraw(deps, amount)?)
        }
        QueryMsg::VaultExtension(msg) => match msg {
            ExtensionQueryMsg::Apollo(msg) => match msg {
                ApolloExtensionQueryMsg::State {} => to_binary(&contract.query_state(deps, env)?),
            },
        },
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    let version: Version = CONTRACT_VERSION.parse()?;
    let storage_version: Version = get_contract_version(deps.storage)?.version.parse()?;

    // migrate only if newer
    if storage_version < version {
        set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // If state structure changed in any contract version in the way
        // migration is needed, it should occur here
    }
    Ok(Response::default())
}
