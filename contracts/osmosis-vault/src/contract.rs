use apollo_vault::error::ContractError;
use apollo_vault::msg::{
    ApolloExtensionExecuteMsg, ApolloExtensionQueryMsg, CallbackMsg, ExtensionExecuteMsg,
    ExtensionQueryMsg,
};
use apollo_vault::AutocompoundingVault;

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_binary, Binary, Deps, DepsMut, Env, Event, MessageInfo, Reply, Response, StdResult,
    SubMsgResponse, SubMsgResult, Uint128,
};
use cw2::{get_contract_version, set_contract_version};
use cw_dex::osmosis::{
    OsmosisPool, OsmosisStaking, OSMOSIS_LOCK_TOKENS_REPLY_ID, OSMOSIS_UNLOCK_TOKENS_REPLY_ID,
};
use cw_dex::traits::{LockedStaking, Pool};
use cw_vault_standard::extensions::force_unlock::ForceUnlockExecuteMsg;
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg};
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

pub type OsmosisVaultContract<'a> =
    AutocompoundingVault<'a, OsmosisStaking, OsmosisPool, OsmosisDenom>;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    let contract = OsmosisVaultContract::default();

    let admin_addr = deps.api.addr_validate(&msg.admin)?;
    let config = msg.config.check(deps.as_ref())?;

    // Validate that 10 osmo for vault token creation are sent
    let osmo_amount = info
        .funds
        .iter()
        .find(|coin| coin.denom == "uosmo")
        .map(|coin| coin.amount)
        .unwrap_or_default();
    if osmo_amount < Uint128::new(10_000_000) {
        return Err(ContractError::from(
            "A minimum of 10_000_000 uosmo must be sent to create the vault token",
        ));
    }

    // Create the pool object
    let pool = OsmosisPool::new(msg.pool_id, deps.as_ref())?;

    let staking = OsmosisStaking::new(msg.lockup_duration, None, pool.lp_token().to_string())?;

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
    let contract = OsmosisVaultContract::default();

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
            ExtensionExecuteMsg::Lockup(msg) => match msg {
                LockupExecuteMsg::WithdrawUnlocked {
                    recipient,
                    lockup_id,
                } => contract.execute_withdraw_unlocked(deps, env, &info, lockup_id, recipient),
                LockupExecuteMsg::Unlock { amount } => {
                    contract.execute_unlock(deps, env, &info, amount)
                }
            },
            ExtensionExecuteMsg::ForceUnlock(msg) => match msg {
                ForceUnlockExecuteMsg::ForceRedeem { recipient, amount } => {
                    contract.execute_force_redeem(deps, env, info, amount, recipient)
                }
                ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
                    lockup_id,
                    amount,
                    recipient,
                } => contract.execute_force_withdraw_unlocking(
                    deps, env, info, lockup_id, amount, recipient,
                ),
                ForceUnlockExecuteMsg::UpdateForceWithdrawWhitelist {
                    add_addresses,
                    remove_addresses,
                } => contract.execute_update_force_withdraw_whitelist(
                    deps,
                    info,
                    add_addresses,
                    remove_addresses,
                ),
            },
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
                    CallbackMsg::Unlock {
                        owner,
                        vault_token_amount,
                    } => {
                        contract.execute_callback_unlock(deps, env, info, owner, vault_token_amount)
                    }
                    CallbackMsg::SaveClaim {} => contract.execute_callback_save_claim(deps),
                }
            }
        },
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    let contract = OsmosisVaultContract::default();
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
            ExtensionQueryMsg::Lockup(msg) => match msg {
                LockupQueryMsg::UnlockingPositions {
                    owner,
                    start_after,
                    limit,
                } => to_binary(&contract.query_unlocking_positions(
                    deps,
                    owner,
                    start_after,
                    limit,
                )?),
                LockupQueryMsg::UnlockingPosition { lockup_id } => {
                    to_binary(&contract.claims.query_claim_by_id(deps, lockup_id)?)
                }
                LockupQueryMsg::LockupDuration {} => to_binary(
                    &contract
                        .staking
                        .load(deps.storage)?
                        .get_lockup_duration(deps)?,
                ),
            },
            ExtensionQueryMsg::Apollo(msg) => match msg {
                ApolloExtensionQueryMsg::State {} => to_binary(&contract.query_state(deps, env)?),
            },
        },
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut, _env: Env, reply: Reply) -> Result<Response, ContractError> {
    let contract = OsmosisVaultContract::default();

    if let SubMsgResult::Ok(SubMsgResponse {
        data: Some(b),
        events: _,
    }) = reply.result
    {
        match reply.id {
            OSMOSIS_LOCK_TOKENS_REPLY_ID => {
                // If `lock_tokens` event exists. Save the lockup_id. This only happens when the
                // contract does not currently have an active lock, i.e. either
                // before the first Deposit or after all the locked coins have
                // started unlocking and another user calls Deposit. If a lock
                // already exists an "add_tokens_to_lock" event will be emitted instead.
                let res: MsgLockTokensResponse = b.try_into().map_err(ContractError::Std)?;

                let mut staking = contract.staking.load(deps.storage)?;
                staking.lock_id = Some(res.id);
                contract.staking.save(deps.storage, &staking)?;

                let event = Event::new("apollo/vault/lock/reply")
                    .add_attribute("vault_type", "osmosis")
                    .add_attribute("lock_id", res.id.to_string());
                Ok(Response::default().add_event(event))
            }
            OSMOSIS_UNLOCK_TOKENS_REPLY_ID => {
                let res: MsgBeginUnlockingResponse = b.try_into().map_err(ContractError::Std)?;

                let mut pending_claim = contract.claims.get_pending_claim(deps.storage)?;
                pending_claim.id = res.unlocking_lock_id;
                contract
                    .claims
                    .set_pending_claim(deps.storage, &pending_claim)?;

                let event = Event::new("apollo/vault/unlock/reply")
                    .add_attribute("vault_type", "osmosis")
                    .add_attribute("lock_id", res.unlocking_lock_id.to_string());
                let data = to_binary(&res.unlocking_lock_id)?;
                Ok(Response::default().add_event(event).set_data(data))
            }
            id => Err(ContractError::UnknownReplyId(id)),
        }
    } else {
        Err(ContractError::NoDataInSubMsgResponse {})
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
