use cw_dex::astroport::{AstroportPool, AstroportStaking};
use cw_it::astroport::robot::AstroportTestRobot;
use cw_it::astroport::utils::AstroportContracts;
use cw_it::cw_multi_test::ContractWrapper;
use cw_it::traits::CwItRunner;
use cw_it::{ContractMap, ContractType, TestRunner};

const DEFAULT_TEST_RUNNER: &str = "multi-test";

use std::collections::HashSet;

use super::liquidity_helper::AstroportLiquidityHelperRobot;
use super::pool::AstroportTestPool;
use super::router::CwDexRouterRobot;
use apollo_cw_asset::AssetInfoUnchecked;
use apollo_vault::msg::{
    ApolloExtensionExecuteMsg, ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg,
    StateResponse,
};
use apollo_vault::state::{ConfigUnchecked, ConfigUpdates};
use cosmwasm_std::{coin, Coin, Decimal, Uint128};
use cw_it::robot::TestRobot;
use cw_it::test_tube::{Account, Module, Runner, SigningAccount, Wasm};
use cw_vault_standard::VaultInfoResponse;
use cw_vault_token::osmosis::OsmosisDenom;
use neutron_astroport_vault::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};

#[derive(Debug, Clone)]
pub struct NeutronAstroportVaultRobot<'a, R: Runner<'a>> {
    pub runner: &'a R,
    pub vault_addr: String,
    pub astroport_contracts: AstroportContracts,
}

fn get_astroport_contracts<'a>(runner: &TestRunner<'a>) -> ContractMap {
    match runner {
        TestRunner::MultiTest(_) => cw_it::astroport::utils::get_astroport_multitest_contracts(),
        TestRunner::OsmosisTestApp(_) => todo!(),
        _ => panic!("unsupported test runner"),
    }
}

impl<'a, R: Runner<'a>> TestRobot<'a, R> for NeutronAstroportVaultRobot<'a, R> {
    fn runner(&self) -> &'a R {
        self.runner
    }
}

impl<'a> AstroportTestRobot<'a, TestRunner<'a>> for NeutronAstroportVaultRobot<'a, TestRunner<'a>> {
    fn astroport_contracts(&self) -> &AstroportContracts {
        &self.astroport_contracts
    }
}

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

impl<'a> NeutronAstroportVaultRobot<'a, TestRunner<'a>> {
    /// Creates a new instance of a NeutronAstroportVaultRobot. If
    pub fn new(
        runner: &'a TestRunner<'a>,
        astroport_contracts: ContractMap,
        signer: &SigningAccount,
        base_pool: &AstroportTestPool,
        reward_pools: Vec<&AstroportTestPool>,
        performance_fee: Decimal,
        treasury: &SigningAccount,
        reward_assets: Vec<AssetInfoUnchecked>,
        reward_liquidation_target: AssetInfoUnchecked,
    ) -> Self {
        let astroport_contracts = get_astroport_contracts(runner);

        let astroport_contracts =
            Self::upload_and_init_astroport_contracts(runner, astroport_contracts, signer);

        let mut robot = Self {
            runner,
            vault_addr: "".to_string(),
            astroport_contracts,
        };

        let (base_pool_addr, base_pool_lp) = base_pool.create(&robot, &signer);

        for pool in reward_pools {
            let (reward_pool_addr, reward_pool_lp) = pool.create(&robot, &signer);
            pool.create(&robot, signer);
        }

        // Instantiate cw dex router
        let router_robot = CwDexRouterRobot::new(runner, signer);

        // Instantiate liquidity helper
        let liquidity_helper_robot =
            AstroportLiquidityHelperRobot::new(runner, astroport_contracts.factory.address, signer);

        // Get vault contract
        let contract = match runner {
            TestRunner::MultiTest(_) => {
                ContractType::MultiTestContract(Box::new(ContractWrapper::new(
                    neutron_astroport_vault::contract::execute,
                    neutron_astroport_vault::contract::instantiate,
                    neutron_astroport_vault::contract::query,
                )))
            }
            _ => panic!("unsupported test runner"),
        };
        let code_id = runner.store_code(contract, signer).unwrap();

        // Instantiate vault
        let init_msg = InstantiateMsg {
            admin: signer.address(),
            pool_addr: base_pool_addr.clone(),
            config: ConfigUnchecked {
                performance_fee,
                treasury: treasury.address(),
                router: router_robot.cw_dex_router.into(),
                reward_assets,
                reward_liquidation_target,
                force_withdraw_whitelist: vec![],
                liquidity_helper: liquidity_helper_robot.liquidity_helper.into(),
            },
            vault_token_subdenom: format!(
                "apVault/{}{}",
                base_pool.liquidity[0].denom, base_pool.liquidity[1].denom
            ),
            token_creation_fee: coin(10_000_000, "untrn"),
            astro_token_addr: astroport_contracts.astro_token.address,
            generator_addr: astroport_contracts.generator.address,
        };

        let wasm = Wasm::new(runner);
        let vault_addr = wasm
            .instantiate(
                code_id,
                &init_msg,
                Some(&signer.address()),
                Some("astroport_vault"),
                &[],
                signer,
            )
            .unwrap()
            .data
            .address;

        robot.vault_addr = vault_addr;

        robot
    }

    pub fn query_vault_state(
        &self,
    ) -> StateResponse<AstroportStaking, AstroportPool, OsmosisDenom> {
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
