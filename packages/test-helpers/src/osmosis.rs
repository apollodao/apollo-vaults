use std::collections::HashSet;

use apollo_cw_asset::AssetInfoUnchecked;
use apollo_vault::msg::{
    ApolloExtensionExecuteMsg, ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg,
    StateResponse,
};
use apollo_vault::state::{ConfigUnchecked, ConfigUpdates};
use cosmwasm_std::{Addr, Coin, Decimal, Empty, Uint128};
use cw_dex::osmosis::{OsmosisPool, OsmosisStaking};
use cw_it::helpers::upload_wasm_file;
use cw_it::osmosis::robot::{OsmosisTestAppRobot, OsmosisTestRobot};
use cw_it::osmosis::OsmosisTestPool;
use cw_it::osmosis_test_tube::{Account, Module, OsmosisTestApp, Runner, SigningAccount, Wasm};
use cw_it::robot::TestRobot;
use cw_it::traits::CwItRunner;
use cw_it::{Artifact, ContractType, TestRunner};
use cw_vault_standard::extensions::force_unlock::ForceUnlockExecuteMsg;
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg, UnlockingPosition};
use cw_vault_standard::VaultInfoResponse;
use cw_vault_token::osmosis::OsmosisDenom;
use liquidity_helper::LiquidityHelper;
use osmosis_vault::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};

use crate::router::CwDexRouterRobot;

pub const LIQUIDITY_HELPER_WASM_NAME: &str = "osmosis_liquidity_helper.wasm";

/// Contracts that are required for the Vault contract to function.
pub struct OsmosisVaultDependencies<'a> {
    pub cw_dex_router_robot: CwDexRouterRobot<'a>,
    pub liquidity_helper: LiquidityHelper,
}

#[derive(Debug, Clone)]
pub struct OsmosisVaultRobot<'a, R: Runner<'a>> {
    pub app: &'a R,
    pub vault_addr: String,
    pub base_pool: OsmosisPool,
}

impl<'a, R: Runner<'a>> TestRobot<'a, R> for OsmosisVaultRobot<'a, R> {
    fn runner(&self) -> &'a R {
        self.app
    }
}

impl<'a> OsmosisTestAppRobot<'a> for OsmosisVaultRobot<'a, OsmosisTestApp> {}

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
    pub fn setup(&self, _admin: &SigningAccount) -> &Self {
        self.whitelist_address_for_force_unlock(&self.vault_addr)
    }
}

impl<'a> OsmosisVaultRobot<'a, TestRunner<'a>> {
    // Uploads and instantiates dependencies for the LockedAstroportVaultRobot.
    pub fn instantiate_deps(
        runner: &'a TestRunner<'a>,
        signer: &SigningAccount,
        artifacts_dir: &str,
    ) -> OsmosisVaultDependencies<'a> {
        // Create CwDexRouterRobot
        let cw_dex_router_robot = CwDexRouterRobot::new(
            runner,
            CwDexRouterRobot::contract(runner, artifacts_dir),
            signer,
        );

        // Upload and instantiate liquidity helper
        let liquidity_helper_contract = match runner {
            #[cfg(feature = "osmosis-test-tube")]
            TestRunner::OsmosisTestApp(_) => ContractType::Artifact(Artifact::Local(format!(
                "{}/{}",
                artifacts_dir, LIQUIDITY_HELPER_WASM_NAME
            ))),
            // TestRunner::MultiTest(_) => {
            //     ContractType::MultiTestContract(Box::new(ContractWrapper::new_with_empty(
            //         osmosis_liquidity_helper::contract::execute,
            //         osmosis_liquidity_helper::contract::instantiate,
            //         osmosis_liquidity_helper::contract::query,
            //     )))
            // }
            _ => panic!("Unsupported runner"),
        };
        let wasm = Wasm::new(runner);
        let code_id = runner
            .store_code(liquidity_helper_contract, signer)
            .unwrap();
        let liquidity_helper_addr = wasm
            .instantiate(
                code_id,
                &Empty {},
                Some(&signer.address()),
                Some("astroport_liquidity_helper"),
                &[],
                signer,
            )
            .unwrap()
            .data
            .address;

        OsmosisVaultDependencies {
            cw_dex_router_robot,
            liquidity_helper: LiquidityHelper::new(Addr::unchecked(liquidity_helper_addr)),
        }
    }
}

impl<'a, R> OsmosisVaultRobot<'a, R>
where
    R: CwItRunner<'a>,
{
    pub fn new_admin(
        app: &R,
        base_pool: OsmosisTestPool,
        reward_pool: OsmosisTestPool,
    ) -> SigningAccount {
        let admin = app
            .init_account(&max_of_all_coins(&[
                base_pool.liquidity.clone(),
                reward_pool.liquidity.clone(),
            ]))
            .unwrap();
        admin
    }

    // TODO: set up router and liquidity helper using robots
    pub fn with_single_rewards(
        app: &'a R,
        base_pool: OsmosisTestPool,
        reward_pool: OsmosisTestPool,
        dependencies: &'a OsmosisVaultDependencies<'a>,
        wasm_file_path: &str,
        admin: &SigningAccount,
    ) -> (Self, SigningAccount, SigningAccount) {
        let fwa_admin = app
            .init_account(&max_of_all_coins(&[
                base_pool.liquidity.clone(),
                reward_pool.liquidity.clone(),
            ]))
            .unwrap();
        let treasury = app.init_account(&[]).unwrap();
        let base_pool_id = base_pool.create(app, &admin);

        let contract = ContractType::Artifact(cw_it::Artifact::Local(wasm_file_path.to_string()));

        let code_id = upload_wasm_file(app, &admin, contract).unwrap();

        let config = ConfigUnchecked {
            performance_fee: Decimal::percent(3), //TODO: variable performance fee
            treasury: treasury.address(),
            router: dependencies
                .cw_dex_router_robot
                .cw_dex_router
                .clone()
                .into(),
            reward_assets: vec![AssetInfoUnchecked::native(
                reward_pool.liquidity[0].denom.clone(),
            )],
            reward_liquidation_target: AssetInfoUnchecked::native(
                base_pool.liquidity[0].denom.clone(),
            ),
            force_withdraw_whitelist: vec![fwa_admin.address()],
            liquidity_helper: dependencies.liquidity_helper.clone().into(),
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
                &[],
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
        funds: &[Coin],
    ) -> &Self {
        let amount: Uint128 = amount.into();
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::Deposit { amount, recipient },
                funds,
                signer,
            )
            .unwrap();
        self
    }

    pub fn deposit_all(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let base_token_denom = self.query_info().base_token;
        let amount = self.query_native_token_balance(&signer.address(), &base_token_denom);

        self.deposit(
            signer,
            amount,
            recipient,
            &[Coin::new(amount.u128(), base_token_denom)],
        )
    }

    pub fn unlock(&self, signer: &SigningAccount, amount: impl Into<Uint128>) -> &Self {
        let vault_token_denom = self.query_info().vault_token;
        let amount: Uint128 = amount.into();
        self.unlock_with_funds(
            signer,
            amount,
            &[Coin::new(amount.u128(), vault_token_denom)],
        );

        self
    }

    pub fn unlock_with_funds(
        &self,
        signer: &SigningAccount,
        amount: impl Into<Uint128>,
        funds: &[Coin],
    ) -> &Self {
        let amount: Uint128 = amount.into();
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
                    LockupExecuteMsg::Unlock { amount },
                )),
                funds,
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

    pub fn withdraw_unlocked(
        &self,
        signer: &SigningAccount,
        lockup_id: u64,
        recipient: Option<String>,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
                    LockupExecuteMsg::WithdrawUnlocked {
                        lockup_id,
                        recipient,
                    },
                )),
                &[],
                signer,
            )
            .unwrap();

        self
    }

    pub fn withdraw_first_unlocked(
        &self,
        signer: &SigningAccount,
        recipient: Option<String>,
    ) -> &Self {
        let unlocking_positions = self.query_unlocking_positions(signer.address());
        let first_lockup_id = unlocking_positions[0].id;

        self.withdraw_unlocked(signer, first_lockup_id, recipient)
    }

    pub fn assert_base_token_balance_eq(
        &self,
        address: impl Into<String>,
        amount: impl Into<Uint128>,
    ) -> &Self {
        let base_token_denom = self.query_info().base_token;
        let amount: Uint128 = amount.into();
        let balance = self.query_native_token_balance(address, &base_token_denom);
        assert_eq!(balance, amount);
        self
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

    pub fn force_redeem(
        &self,
        signer: &SigningAccount,
        amount: Uint128,
        recipient: Option<String>,
        funds: &[Coin],
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
                    ForceUnlockExecuteMsg::ForceRedeem { amount, recipient },
                )),
                funds,
                signer,
            )
            .unwrap();
        self
    }

    pub fn update_force_withdraw_whitelist(
        &self,
        signer: &SigningAccount,
        add_addresses: Vec<String>,
        remove_addresses: Vec<String>,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
                    ForceUnlockExecuteMsg::UpdateForceWithdrawWhitelist {
                        add_addresses,
                        remove_addresses,
                    },
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    pub fn query_whitelist(&self) -> Vec<Addr> {
        self.query_vault_state().config.force_withdraw_whitelist
    }

    pub fn assert_whitelist_contains(&self, account: impl Into<String>) -> &Self {
        let whitelist = self.query_whitelist();
        assert!(whitelist.contains(&Addr::unchecked(account.into())));

        self
    }

    pub fn assert_whitelist_not_contains(&self, account: impl Into<String>) -> &Self {
        let whitelist = self.query_whitelist();
        assert!(!whitelist.contains(&Addr::unchecked(account.into())));

        self
    }

    pub fn redeem(
        &self,
        signer: &SigningAccount,
        amount: Uint128,
        recipient: Option<String>,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::Redeem { amount, recipient },
                &[],
                signer,
            )
            .unwrap();
        self
    }

    pub fn redeem_all(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let amount =
            self.query_native_token_balance(signer.address(), &self.query_info().vault_token);
        self.redeem(signer, amount, recipient)
    }

    pub fn update_config(&self, signer: &SigningAccount, updates: ConfigUpdates) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::UpdateConfig { updates },
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    pub fn assert_config(&self, config_unchecked: ConfigUnchecked) -> &Self {
        let config = self.query_vault_state().config;
        assert_eq!(
            config
                .force_withdraw_whitelist
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>(),
            config_unchecked.force_withdraw_whitelist
        );
        assert_eq!(config.performance_fee, config_unchecked.performance_fee);
        assert_eq!(
            config.liquidity_helper.0.to_string(),
            config_unchecked.liquidity_helper.0
        );
        assert_eq!(config.router.0.to_string(), config_unchecked.router.0);
        assert_eq!(
            config.reward_liquidation_target.to_string(),
            config_unchecked.reward_liquidation_target.to_string()
        );
        assert_eq!(
            config
                .reward_assets
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>(),
            config_unchecked
                .reward_assets
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
        );

        self
    }

    pub fn update_admin(&self, signer: &SigningAccount, new_admin: impl Into<String>) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::UpdateAdmin {
                        address: new_admin.into(),
                    },
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    pub fn accept_admin_transfer(&self, signer: &SigningAccount) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::AcceptAdminTransfer {},
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    pub fn assert_admin(&self, expected: impl Into<String>) -> &Self {
        let admin = self.query_vault_state().admin;
        assert_eq!(admin.unwrap().to_string(), expected.into());

        self
    }

    pub fn drop_admin_transfer(&self, signer: &SigningAccount) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::DropAdminTransfer {},
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }
}
