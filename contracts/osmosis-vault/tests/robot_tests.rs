use std::collections::HashSet;

use apollo_cw_asset::{AssetInfo, AssetInfoUnchecked};
use apollo_vault::{
    msg::{ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg, StateResponse},
    state::{Config, ConfigUnchecked},
};
use cosmwasm_std::{Coin, Decimal, Uint128};
use cw_dex::osmosis::{OsmosisPool, OsmosisStaking};
use cw_dex_router::helpers::CwDexRouterUnchecked;
use cw_it::{
    helpers::upload_wasm_file,
    osmosis::{robot::OsmosisTestRobot, OsmosisTestPool},
    osmosis_test_tube::{Account, Module, OsmosisTestApp, Runner, SigningAccount, Wasm},
    robot::TestRobot,
};
use cw_vault_standard::{
    extensions::{
        force_unlock::ForceUnlockExecuteMsg,
        lockup::{LockupExecuteMsg, LockupQueryMsg, UnlockingPosition},
    },
    VaultInfoResponse,
};
use cw_vault_token::osmosis::OsmosisDenom;
use liquidity_helper::LiquidityHelperUnchecked;
use osmosis_vault::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};

#[derive(Debug, Clone)]
pub struct OsmosisVaultRobot<'a, R: Runner<'a>> {
    pub app: &'a R,
    pub vault_addr: String,
    pub base_pool: OsmosisPool,
}

impl<'a, R: Runner<'a>> TestRobot<'a, R> for OsmosisVaultRobot<'a, R> {
    fn app(&self) -> &'a R {
        self.app
    }
}

impl<'a> OsmosisTestRobot<'a> for OsmosisVaultRobot<'a, OsmosisTestApp> {}

fn max_of_all_coins(coins: &[Vec<Coin>]) -> Vec<Coin> {
    coins
        .iter()
        .flatten()
        .map(|c| c.denom.clone())
        .collect::<HashSet<String>>()
        .iter()
        .map(|d| Coin::new(u128::MAX, d.clone()))
        .collect::<Vec<_>>()
}

impl<'a> OsmosisVaultRobot<'a, OsmosisTestApp> {
    // pub fn new(app: &'a OsmosisTestApp, vault_addr: String) -> Self {
    //     Self { app, vault_addr }
    // }

    // pub fn without_rewards(
    //     app: &'a OsmosisTestApp,
    //     base_pool: OsmosisTestPool,
    //     wasm_file_path: &str,
    // ) -> (Self, &SigningAccount, &SigningAccount, &SigningAccount) {
    //     Self::with_single_rewards(app, base_pool.clone(), base_pool, wasm_file_path)
    // }

    // TODO: set up router and liquidity helper
    pub fn with_single_rewards(
        app: &'a OsmosisTestApp,
        base_pool: OsmosisTestPool,
        reward_pool: OsmosisTestPool,
        wasm_file_path: &str,
    ) -> (Self, SigningAccount, SigningAccount, SigningAccount) {
        let admin = app
            .init_account(&max_of_all_coins(&[
                base_pool.liquidity.clone(),
                reward_pool.liquidity.clone(),
            ]))
            .unwrap();
        let fwa_admin = app
            .init_account(&max_of_all_coins(&[
                base_pool.liquidity.clone(),
                reward_pool.liquidity.clone(),
            ]))
            .unwrap();
        let treasury = app.init_account(&[]).unwrap();
        let base_pool_id = base_pool.create(app, &admin);
        let reward_pool_id = reward_pool.create(app, &admin);

        let code_id = upload_wasm_file(app, &admin, wasm_file_path).unwrap();

        let config = ConfigUnchecked {
            performance_fee: Decimal::percent(3), //TODO: variable performance fee
            treasury: treasury.address(),
            // TODO: Setup router
            router: CwDexRouterUnchecked::new(app.init_account(&[]).unwrap().address()),
            reward_assets: vec![AssetInfoUnchecked::native(
                reward_pool.liquidity[0].denom.clone(),
            )],
            reward_liquidation_target: AssetInfoUnchecked::native(
                base_pool.liquidity[0].denom.clone(),
            ),
            force_withdraw_whitelist: vec![fwa_admin.address()],
            // TODO: Setup liquidity helper
            liquidity_helper: LiquidityHelperUnchecked::new(
                app.init_account(&[]).unwrap().address(),
            ),
        };

        let init_msg = InstantiateMsg {
            admin: admin.address(),
            pool_id: base_pool_id,
            lockup_duration: 60 * 60 * 24 * 14, // TODO: dont hard code
            config,
            vault_token_subdenom: format!("apVault/{base_pool_id}"),
        };

        let wasm = Wasm::new(app);
        let vault_addr = wasm
            .instantiate(
                code_id,
                &init_msg,
                Some(&admin.address()),
                Some("Apollo Vault"),
                &[Coin::new(10_000_000u128, "uosmo")],
                &admin,
            )
            .unwrap()
            .data
            .address;

        (
            Self {
                app,
                vault_addr,
                base_pool: OsmosisPool::unchecked(base_pool_id),
            },
            admin,
            fwa_admin,
            treasury,
        )
    }

    pub fn query_vault_state(&self) -> StateResponse<OsmosisStaking, OsmosisPool, OsmosisDenom> {
        self.wasm()
            .query(
                &self.vault_addr,
                &QueryMsg::VaultExtension(ExtensionQueryMsg::Apollo(
                    ApolloExtensionQueryMsg::State {},
                )),
            )
            .unwrap()
    }

    pub fn query_info(&self) -> VaultInfoResponse {
        self.wasm()
            .query(&self.vault_addr, &QueryMsg::Info {})
            .unwrap()
    }

    pub fn deposit(
        &self,
        signer: &SigningAccount,
        amount: impl Into<Uint128>,
        recipient: Option<String>,
    ) -> &Self {
        let base_token_denom = self.query_info().base_token;
        let amount: Uint128 = amount.into();
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::Deposit { amount, recipient },
                &[Coin::new(amount.u128(), base_token_denom)],
                signer,
            )
            .unwrap();
        self
    }

    pub fn deposit_all(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let base_token_denom = self.query_info().base_token;
        let amount = self.query_native_token_balance(&signer.address(), &base_token_denom);

        self.deposit(signer, amount, recipient)
    }

    pub fn unlock(&self, signer: &SigningAccount, amount: impl Into<Uint128>) -> &Self {
        let vault_token_denom = self.query_info().vault_token;
        let amount: Uint128 = amount.into();
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
                    LockupExecuteMsg::Unlock { amount },
                )),
                &[Coin::new(amount.u128(), vault_token_denom)],
                signer,
            )
            .unwrap();
        self
    }

    pub fn unlock_all(&self, signer: &SigningAccount) -> &Self {
        let vault_token_denom = self.query_info().vault_token;
        let amount = self.query_native_token_balance(&signer.address(), &vault_token_denom);

        self.unlock(signer, amount)
    }

    pub fn assert_number_of_unlocking_position(
        &self,
        owner: impl Into<String>,
        num: usize,
    ) -> &Self {
        let unlocking_positions = self.query_unlocking_positions(owner);
        assert_eq!(unlocking_positions.len(), num);
        self
    }

    pub fn assert_unlocking_position_exists(
        &self,
        owner: impl Into<String>,
        lockup_id: u64,
    ) -> &Self {
        let unlocking_positions = self.query_unlocking_positions(owner);

        assert!(unlocking_positions.iter().any(|pos| pos.id == lockup_id));
        self
    }

    pub fn query_unlocking_positions(&self, owner: impl Into<String>) -> Vec<UnlockingPosition> {
        self.wasm()
            .query(
                &self.vault_addr,
                &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                    LockupQueryMsg::UnlockingPositions {
                        owner: owner.into(),
                        start_after: None,
                        limit: None,
                    },
                )),
            )
            .unwrap()
    }

    pub fn force_withdraw_unlocking(
        &self,
        signer: &SigningAccount,
        lockup_id: u64,
        amount: Option<Uint128>,
        recipient: Option<String>,
    ) -> &Self {
        let vault_token_denom = self.query_info().vault_token;
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
                    ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
                        amount,
                        recipient,
                        lockup_id,
                    },
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }
}

#[cfg(test)]
mod tests {
    use cw_it::osmosis::OsmosisPoolType;

    use super::*;

    use test_case::test_case;

    const WASM_FILE_PATH: &str = "target/wasm32-unknown-unknown/release/osmosis_vault.wasm";
    const UOSMO: &str = "uosmo";

    #[test_case(false, false, None, false => panics ; "caller not whitelisted")]
    #[test_case(true, false, None, false ; "lock not expired amount is None recipient is None")]
    #[test_case(true, false, None, true ; "lock not expired amount is None recipient is Some")]
    #[test_case(true, false, Some(Decimal::zero()), false => panics ; "lock not expired amount is Some(0) recipient is none")]
    #[test_case(true, false, Some(Decimal::percent(50)), false ; "lock not expired amount is Some(50%) recipient is none")]
    #[test_case(true, false, Some(Decimal::percent(100)), false ; "lock not expired amount is Some(100%) recipient is none")]
    #[test_case(true, false, Some(Decimal::percent(150)), false => panics ; "lock not expired amount is Some(150%) recipient is none")]
    #[test_case(true, true, None, false => ; "lock is expired amount is None recipient is None")]
    fn force_withdraw_unlocking(
        whitlisted: bool,
        expired: bool,
        force_unlock_amount: Option<Decimal>,
        different_recipient: bool,
    ) {
        let app = OsmosisTestApp::new();
        let pool = OsmosisTestPool::new(
            vec![
                Coin::new(1000000000000, "uosmo"),
                Coin::new(1000000000000, "uatom"),
            ],
            OsmosisPoolType::Basic,
        );
        let other_user = app.init_account(&[Coin::new(1000000, "uosmo")]).unwrap();

        let (robot, admin, mut fwa_admin, _treasury) =
            OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);

        // Parse args into message params
        if !whitlisted {
            fwa_admin = app
                .init_account(&[Coin::new(1000000000000000, UOSMO)])
                .unwrap();
        }
        let recipient = match different_recipient {
            true => Some(other_user.address()),
            false => None,
        };
        let increase_time_by = if expired { 3600 * 24 * 15 } else { 0 };

        println!("Whitelisting address: {}", fwa_admin.address());

        let unlocking_pos = &robot
            .send_native_tokens(
                // LP tokens to vault to allow it to create new Locks on unlock
                // TODO: Remove this after mainnet chain upgrade
                &admin,
                &robot.vault_addr,
                1000000u32,
                robot.query_info().base_token,
            )
            .whitelist_address_for_force_unlock(&robot.vault_addr)
            .join_pool_swap_extern_amount_in(
                &fwa_admin,
                robot.base_pool.pool_id(),
                Coin::new(1_000_000_000u128, UOSMO),
                None,
            )
            .deposit_all(&fwa_admin, None)
            .unlock_all(&fwa_admin)
            .assert_number_of_unlocking_position(fwa_admin.address(), 1)
            .query_unlocking_positions(fwa_admin.address())[0];

        // Calculate amount to force unlock
        let force_unlock_amount = force_unlock_amount.map(|x| x * unlocking_pos.base_token_amount);

        println!("Unlocking position: {:?}", unlocking_pos);
        robot
            .increase_time(increase_time_by)
            .force_withdraw_unlocking(
                &fwa_admin,
                unlocking_pos.id,
                force_unlock_amount,
                recipient.clone(),
            )
            .assert_native_token_balance_eq(
                recipient.unwrap_or(fwa_admin.address()),
                robot.query_info().base_token,
                force_unlock_amount.unwrap_or(unlocking_pos.base_token_amount),
            );

        // If entire amount is unlocked, there should be no more unlocking positions
        if force_unlock_amount.is_none()
            || (force_unlock_amount.is_some()
                && force_unlock_amount.unwrap() == unlocking_pos.base_token_amount)
        {
            robot.assert_number_of_unlocking_position(fwa_admin.address(), 0);
        } else {
            robot.assert_number_of_unlocking_position(fwa_admin.address(), 1);
        }
    }
}
