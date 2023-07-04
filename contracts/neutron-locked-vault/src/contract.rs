use std::ops::Deref;

use apollo_utils::responses::merge_responses;
use apollo_vault::error::ContractError;
use apollo_vault::msg::{
    ApolloExtensionExecuteMsg, ApolloExtensionQueryMsg, CallbackMsg, ExtensionExecuteMsg,
    ExtensionQueryMsg,
};
use apollo_vault::AutocompoundingVault;

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    attr, to_binary, Binary, Deps, DepsMut, Env, Event, MessageInfo, Response, StdResult, Uint128,
};
use cw2::{get_contract_version, set_contract_version};
use cw_dex::astroport::{AstroportPool, AstroportStaking};
use cw_dex::traits::Unstake;
use cw_storage_plus::Item;
use cw_utils::{Duration, Expiration};
use cw_vault_standard::extensions::force_unlock::ForceUnlockExecuteMsg;
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg, UnlockingPosition};
use cw_vault_standard::msg::{VaultInfoResponse, VaultStandardInfoResponse};
use cw_vault_token::osmosis::OsmosisDenom;
use cw_vault_token::Receive;
use semver::Version;

use crate::msg::{ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:osmosis-vault";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Constants passed to VaultStandardInfo query
const VAULT_STANDARD_VERSION: u16 = 1;
const VAULT_STANDARD_EXTENSIONS: [&str; 2] = ["lockup", "force-unlock"];

const LOCKUP_DURATION: Item<Duration> = Item::new("lockup_duration");

pub struct LockedNeutronVaultContract<'a>(
    AutocompoundingVault<'a, AstroportStaking, AstroportPool, OsmosisDenom>,
);

impl Default for LockedNeutronVaultContract<'_> {
    fn default() -> Self {
        Self(AutocompoundingVault::default())
    }
}

impl<'a> Deref for LockedNeutronVaultContract<'a> {
    type Target = AutocompoundingVault<'a, AstroportStaking, AstroportPool, OsmosisDenom>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> LockedNeutronVaultContract<'a> {
    pub fn execute_unlock(
        &self,
        mut deps: DepsMut,
        env: &Env,
        info: &MessageInfo,
        amount: Uint128,
    ) -> Result<Response, ContractError> {
        // Receive vault tokens
        let vault_token = self.base_vault.vault_token.load(deps.storage)?;
        vault_token.receive(deps.branch(), &env, &info, amount)?;

        // Burn vault tokens and get the amount of base tokens to unlock
        let (lp_amount, burn_res) =
            self.base_vault
                .burn_vault_tokens_for_base_tokens(deps.branch(), &env, amount)?;

        // Read lockup duration from storage
        let lockup_duration = LOCKUP_DURATION.load(deps.storage)?;

        // Create a the claim
        self.claims.create_pending_claim(
            deps.storage,
            &info.sender,
            lp_amount,
            (Expiration::AtTime(env.block.time) + lockup_duration)?,
            None,
        )?;
        self.claims.commit_pending_claim(deps.storage)?;

        Ok(burn_res)
    }

    pub fn execute_withdraw_unlocked(
        &self,
        deps: DepsMut,
        env: &Env,
        info: &MessageInfo,
        lock_id: u64,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        let recipient = recipient
            .map(|x| deps.api.addr_validate(&x))
            .transpose()?
            .unwrap_or(info.sender.clone());

        let amount = self
            .claims
            .claim_tokens(deps.storage, &env.block, &info, lock_id)?;

        let staking = self.staking.load(deps.storage)?;
        let unstake_res = staking.unstake(deps.as_ref(), &env, amount)?;

        // Send the unstaked tokens to the recipient
        let send_res = self.base_vault.send_base_tokens(deps, &recipient, amount)?;

        let event = Event::new("apollo/vaults/execute_withdraw_unlocked").add_attributes(vec![
            attr("action", "execute_withdraw_unlocked"),
            attr("recipient", recipient),
            attr("base_token_amount_withdraw_amount", amount),
        ]);

        Ok(merge_responses(vec![unstake_res, send_res]).add_event(event))
    }

    pub fn execute_force_redeem(
        &self,
        mut deps: DepsMut,
        env: Env,
        info: MessageInfo,
        vault_token_amount: Uint128,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        // Check ForceWithdraw whitelist
        let cfg = self.config.load(deps.storage)?;
        if !cfg.force_withdraw_whitelist.contains(&info.sender) {
            return Err(ContractError::Unauthorized {});
        }

        // Burn vault tokens and get the amount of base tokens to unlock
        let (lp_amount, burn_res) = self.base_vault.burn_vault_tokens_for_base_tokens(
            deps.branch(),
            &env,
            vault_token_amount,
        )?;

        // Unstake the LP tokens
        let staking = self.staking.load(deps.storage)?;
        let unstake_res = staking.unstake(deps.as_ref(), &env, lp_amount)?;

        // Send the unstaked tokens to the recipient
        let recipient = recipient
            .map(|x| deps.api.addr_validate(&x))
            .transpose()?
            .unwrap_or(info.sender);
        let send_res = self
            .base_vault
            .send_base_tokens(deps, &recipient, lp_amount)?;

        let event = Event::new("apollo/vaults/execute_force_unlock").add_attributes(vec![
            attr("action", "execute_force_redeem"),
            attr("recipient", recipient),
            attr("base_token_withdraw_amount", lp_amount),
            attr("vault_token_burned_amount", vault_token_amount),
        ]);

        Ok(merge_responses(vec![burn_res, unstake_res, send_res]).add_event(event))
    }

    pub fn execute_force_withdraw_unlocking(
        &self,
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        lockup_id: u64,
        amount: Option<Uint128>,
        recipient: Option<String>,
    ) -> Result<Response, ContractError> {
        // Check ForceWithdraw whitelist
        let cfg = self.config.load(deps.storage)?;
        if !cfg.force_withdraw_whitelist.contains(&info.sender) {
            return Err(ContractError::Unauthorized {});
        }

        // Force claim amount from the claim. Will delete the claim if all is claimed and if not
        // will update the claim with the new amount.
        let claim_amount = self
            .claims
            .force_claim(deps.storage, &info, lockup_id, amount)?;

        // Unstake the LP tokens
        let staking = self.staking.load(deps.storage)?;
        let unstake_res = staking.unstake(deps.as_ref(), &env, claim_amount)?;

        // Send the unstaked tokens to the recipient
        let recipient = recipient
            .map(|x| deps.api.addr_validate(&x))
            .transpose()?
            .unwrap_or(info.sender);
        let send_res = self
            .base_vault
            .send_base_tokens(deps, &recipient, claim_amount)?;

        Ok(merge_responses(vec![unstake_res, send_res]))
    }

    pub fn execute_update_force_withdraw_whitelist(
        &self,
        deps: DepsMut,
        info: &MessageInfo,
        add_addresses: Vec<String>,
        remove_addresses: Vec<String>,
    ) -> Result<Response, ContractError> {
        // Only admin can update the whitelist
        self.admin.assert_admin(deps.as_ref(), &info.sender)?;

        // Read the config
        let mut cfg = self.config.load(deps.storage)?;

        // Add addresses to the whitelist
        for address in add_addresses.clone() {
            let addr = deps.api.addr_validate(&address)?;
            cfg.force_withdraw_whitelist.push(addr);
        }

        // Remove addresses from the whitelist
        for address in remove_addresses.clone() {
            let addr = deps.api.addr_validate(&address)?;
            cfg.force_withdraw_whitelist.retain(|x| x != &addr);
        }

        self.config.save(deps.storage, &cfg)?;

        let event = Event::new("apollo/vaults/execute_update_force_withdraw_whitelist")
            .add_attributes(vec![
                attr("action", "execute_update_force_withdraw_whitelist"),
                attr("add_addresses", add_addresses.join(",")),
                attr("remove_addresses", remove_addresses.join(",")),
            ]);

        Ok(Response::new().add_event(event))
    }

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

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    let contract = LockedNeutronVaultContract::default();

    let admin_addr = deps.api.addr_validate(&msg.admin)?;
    let config = msg.config.check(deps.as_ref())?;

    // Validate that 10 osmo for vault token creation are sent
    let fee_amount = info
        .funds
        .iter()
        .find(|coin| coin.denom == msg.token_creation_fee.denom)
        .map(|coin| coin.amount)
        .unwrap_or_default();
    if fee_amount < msg.token_creation_fee.amount {
        return Err(ContractError::from(format!(
            "A minimum of {} must be sent to create the vault token",
            msg.token_creation_fee
        )));
    }

    // Create the pool object
    let pool = AstroportPool::new(deps.as_ref(), deps.api.addr_validate(&msg.pair_addr)?)?;

    let staking = AstroportStaking {
        lp_token_addr: pool.lp_token_addr.clone(),
        generator_addr: deps.api.addr_validate(&msg.astroport_generator)?,
        astro_addr: deps.api.addr_validate(&msg.astro_token)?,
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
    let contract = LockedNeutronVaultContract::default();

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
                } => contract.execute_withdraw_unlocked(deps, &env, &info, lockup_id, recipient),
                LockupExecuteMsg::Unlock { amount } => {
                    contract.execute_unlock(deps, &env, &info, amount)
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
                    &info,
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
                    CallbackMsg::Unlock { .. } => {
                        Err(ContractError::from("Unlock is not yet implemented."))
                    }
                    CallbackMsg::SaveClaim {} => {
                        Err(ContractError::from("SaveClaim is not yet implemented."))
                    }
                }
            }
        },
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    let contract = LockedNeutronVaultContract::default();
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
                LockupQueryMsg::LockupDuration {} => {
                    to_binary(&LOCKUP_DURATION.load(deps.storage)?)
                }
            },
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
