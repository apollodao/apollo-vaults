use apollo_vault::msg::{ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg};
use apollo_vault::AutocompoundingVault;
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{to_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdResult};
use cw2::{get_contract_version, set_contract_version};
use cw20_base::allowances::{
    execute_decrease_allowance, execute_increase_allowance, execute_send_from,
    execute_transfer_from, query_allowance,
};
use cw20_base::contract::{
    execute_send, execute_transfer, execute_update_marketing, execute_upload_logo,
};
use cw20_base::enumerable::{query_all_accounts, query_owner_allowances};
use cw20_base::msg::MigrateMsg;
use cw20_base::state::{LOGO, MARKETING_INFO, TOKEN_INFO};
use cw_dex::astroport::{AstroportPool, AstroportStaking};
use cw_dex::traits::Pool;
use cw_vault_standard::msg::{VaultInfoResponse, VaultStandardInfoResponse};
use cw_vault_token::cw4626::Cw4626;
use semver::Version;

use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use apollo_vault::error::ContractError;
use apollo_vault::msg::{ApolloExtensionExecuteMsg, CallbackMsg};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:astroport-vault";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const VAULT_STANDARD_VERSION: u16 = 1;
const VAULT_STANDARD_EXTENSIONS: [&str; 2] = ["cw4626", "cw20"];

pub type AstroportVaultContract<'a> =
    AutocompoundingVault<'a, AstroportStaking, AstroportPool, Cw4626>;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let contract = AstroportVaultContract::default();

    let admin_addr = deps.api.addr_validate(&msg.admin)?;
    let config = msg.config.check(deps.as_ref())?;

    // Create the pool object
    let pool = AstroportPool::new(deps.as_ref(), deps.api.addr_validate(&msg.pool)?)?;

    let staking = AstroportStaking {
        lp_token_addr: deps.api.addr_validate(&pool.lp_token().to_string())?,
        generator_addr: deps.api.addr_validate(&msg.generator)?,
        astro_addr: deps.api.addr_validate(&msg.astro_token)?,
    };
    let vault_token = Cw4626::new(&env);

    contract.init(
        deps,
        admin_addr,
        pool,
        staking,
        config,
        vault_token,
        Some(to_binary(&msg.init_info)?),
    )
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    let contract = AstroportVaultContract::default();

    match msg {
        ExecuteMsg::Transfer { recipient, amount } => {
            Ok(execute_transfer(deps, env, info, recipient, amount)?)
        }
        ExecuteMsg::Send {
            contract,
            amount,
            msg,
        } => Ok(execute_send(deps, env, info, contract, amount, msg)?),
        ExecuteMsg::IncreaseAllowance {
            spender,
            amount,
            expires,
        } => Ok(execute_increase_allowance(
            deps, env, info, spender, amount, expires,
        )?),
        ExecuteMsg::DecreaseAllowance {
            spender,
            amount,
            expires,
        } => Ok(execute_decrease_allowance(
            deps, env, info, spender, amount, expires,
        )?),
        ExecuteMsg::TransferFrom {
            owner,
            recipient,
            amount,
        } => Ok(execute_transfer_from(
            deps, env, info, owner, recipient, amount,
        )?),
        ExecuteMsg::SendFrom {
            owner,
            contract,
            amount,
            msg,
        } => Ok(execute_send_from(
            deps, env, info, owner, contract, amount, msg,
        )?),
        ExecuteMsg::UpdateMarketing {
            project,
            description,
            marketing,
        } => Ok(execute_update_marketing(
            deps,
            env,
            info,
            project,
            description,
            marketing,
        )?),
        ExecuteMsg::UploadLogo(logo) => Ok(execute_upload_logo(deps, env, info, logo)?),
        ExecuteMsg::Deposit { amount, recipient } => {
            contract.execute_deposit(deps, env, &info, amount, recipient)
        }
        ExecuteMsg::Redeem { recipient, amount } => {
            contract.execute_redeem(deps, env, &info, amount, recipient)
        }
        ExecuteMsg::VaultExtension(msg) => match msg {
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
                    CallbackMsg::Redeem { amount, recipient } => {
                        contract.execute_callback_redeem(deps, env, amount, recipient)
                    }
                }
            }
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
        },
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    let contract = AstroportVaultContract::default();
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
            ExtensionQueryMsg::Apollo(apollo_msg) => match apollo_msg {
                ApolloExtensionQueryMsg::State {} => to_binary(&contract.query_state(deps, env)?),
            },
        },
        QueryMsg::Balance { address } => {
            to_binary(&base_vault.query_vault_token_balance(deps, address)?)
        }
        QueryMsg::TokenInfo {} => to_binary(&TOKEN_INFO.load(deps.storage)?),
        QueryMsg::Allowance { owner, spender } => {
            to_binary(&query_allowance(deps, owner, spender)?)
        }
        QueryMsg::MarketingInfo {} => to_binary(&MARKETING_INFO.load(deps.storage)?),
        QueryMsg::DownloadLogo {} => to_binary(&LOGO.load(deps.storage)?),
        QueryMsg::AllAllowances {
            owner,
            start_after,
            limit,
        } => to_binary(&query_owner_allowances(deps, owner, start_after, limit)?),
        QueryMsg::AllAccounts { start_after, limit } => {
            to_binary(&query_all_accounts(deps, start_after, limit)?)
        }
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, ContractError> {
    let version: Version = CONTRACT_VERSION.parse()?;
    let storage_version: Version = get_contract_version(deps.storage)?.version.parse()?;

    // migrate only if newer
    if storage_version < version {
        set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        // If state structure changed in any contract version in the way migration is needed, it
        // should occur here
    }
    Ok(Response::default())
}
