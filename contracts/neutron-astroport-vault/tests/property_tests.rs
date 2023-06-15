use apollo_cw_asset::{AssetInfo, AssetInfoUnchecked};
use apollo_vault::msg::{
    ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg, StateResponse,
};
use apollo_vault::state::ConfigUnchecked;
use cosmwasm_std::testing::{MockApi, MockStorage};
use cosmwasm_std::{Coin, Decimal, Deps, Empty, Querier, QuerierWrapper, StdResult, Uint128};
use cw_dex::osmosis::{OsmosisPool, OsmosisStaking};
use cw_dex::traits::Pool as PoolTrait;
use cw_dex::Pool;
use cw_dex_router::helpers::CwDexRouterUnchecked;
use cw_dex_router::operations::{SwapOperation, SwapOperationsList};
use cw_it::config::TestConfig;
use cw_it::helpers::{
    bank_balance_query, bank_send, instantiate_contract, instantiate_contract_with_funds,
    upload_wasm_files,
};
use cw_it::mock_api::OsmosisMockApi;
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg, UnlockingPosition};
use osmosis_vault::msg::ExecuteMsg;
use std::time::Duration;

use cw_vault_token::osmosis::OsmosisDenom;
use liquidity_helper::LiquidityHelperUnchecked;
use osmosis_testing::cosmrs::proto::cosmwasm::wasm::v1::MsgExecuteContractResponse;
use osmosis_testing::{Account, Gamm, Module, OsmosisTestApp, Runner, SigningAccount, Wasm};
use osmosis_vault::msg::{InstantiateMsg, QueryMsg};

const UOSMO: &str = "uosmo";
const UATOM: &str = "uatom";
const UION: &str = "uion";
const STAKE: &str = "stake";
const INITIAL_BALANCE: u128 = 100_000_000_000_000;
const TEST_CONFIG_PATH: &str = "tests/configs/osmosis.yaml";
const TWO_WEEKS_IN_SECONDS: u64 = 60 * 60 * 24 * 14;

pub struct RunnerMockDeps<'a, Q: Querier> {
    pub storage: MockStorage,
    pub api: MockApi,
    pub querier: &'a Q,
}

impl<'a, Q: Querier> RunnerMockDeps<'a, Q> {
    pub fn new(querier: &'a Q) -> Self {
        Self {
            storage: MockStorage::default(),
            api: MockApi::default(),
            querier,
        }
    }
    pub fn as_ref(&'_ self) -> Deps<'_, Empty> {
        Deps {
            storage: &self.storage,
            api: &self.api,
            querier: QuerierWrapper::new(self.querier),
        }
    }
}

struct OsmosisVaultRobot<'a, R: Runner<'a>> {
    app: &'a R,
    vault_addr: String,
    base_pool: OsmosisPool,
}

impl<'a, R: Runner<'a> + Querier> OsmosisVaultRobot<'a, R> {
    fn new(
        app: &'a R,
        admin: &SigningAccount,
        force_withdraw_admin: &SigningAccount,
        treasury: &SigningAccount,
        base_pool_liquidity: Vec<Coin>,
        reward_token_denoms: &Vec<String>,
        reward1_pool_liquidity: Vec<Coin>,
        reward2_pool_liquidity: Option<Vec<Coin>>,
        reward_liquidation_target: String,
        performance_fee: Decimal,
        test_config_path: &str,
    ) -> Self {
        let gamm = Gamm::new(app);
        let api = OsmosisMockApi::new();

        let test_config = TestConfig::from_yaml(test_config_path);

        println!("base_pool_liquidity: {:?}", base_pool_liquidity);

        // Create base pool (the pool this vault will compound)
        let base_pool_id = gamm
            .create_basic_pool(&base_pool_liquidity, admin)
            .unwrap()
            .data
            .pool_id;
        println!("Pool ID: {}", base_pool_id);
        let base_pool = OsmosisPool::unchecked(base_pool_id);

        // Create pool for first reward token
        let reward1_pool_id = gamm
            .create_basic_pool(&reward1_pool_liquidity, admin)
            .unwrap()
            .data
            .pool_id;
        let reward1_pool = OsmosisPool::unchecked(reward1_pool_id);
        let reward1_token = reward1_pool_liquidity
            .iter()
            .find(|x| x.denom != reward_liquidation_target)
            .unwrap()
            .denom
            .clone();

        // Create pool for second reward token (if set)
        let reward2_pool = reward2_pool_liquidity.clone().map(|liquidity| {
            let rewards2_pool_id = gamm
                .create_basic_pool(&liquidity, admin)
                .unwrap()
                .data
                .pool_id;
            OsmosisPool::unchecked(rewards2_pool_id)
        });
        let reward2_token = reward2_pool_liquidity.clone().map(|liquidity| {
            liquidity
                .iter()
                .find(|x| x.denom != reward_liquidation_target)
                .unwrap()
                .denom
                .clone()
        });

        // Upload wasm files
        let code_ids = upload_wasm_files(app, admin, test_config.clone()).unwrap();

        // Instantiate Osmosis Liquidity Helper
        let osmosis_liquidity_helper = instantiate_contract::<_, _, LiquidityHelperUnchecked>(
            app,
            admin,
            code_ids["osmosis_liquidity_helper"],
            &Empty {},
        )
        .unwrap();

        // Instantiate CwDexRouter
        let cw_dex_router = instantiate_contract::<_, _, CwDexRouterUnchecked>(
            app,
            admin,
            code_ids["cw_dex_router"],
            &Empty {},
        )
        .unwrap()
        .check(&api)
        .unwrap();

        // Update paths for CwDexRouter
        let update_path_for_reward_pool = |reward_token: String, pool: Pool| {
            let msg = cw_dex_router
                .set_path_msg(
                    AssetInfo::Native(reward_token.clone()),
                    AssetInfo::Native(reward_liquidation_target.clone()),
                    &SwapOperationsList::new(vec![SwapOperation {
                        offer_asset_info: AssetInfo::Native(reward_token),
                        ask_asset_info: AssetInfo::Native(reward_liquidation_target.clone()),
                        pool,
                    }]),
                    false,
                )
                .unwrap();
            app.execute_cosmos_msgs::<MsgExecuteContractResponse>(&[msg], admin)
                .unwrap();
        };
        update_path_for_reward_pool(reward1_token.clone(), Pool::Osmosis(reward1_pool));
        if let Some(reward2_token) = &reward2_token {
            update_path_for_reward_pool(
                reward2_token.clone(),
                Pool::Osmosis(reward2_pool.unwrap()),
            );
        }

        // Create vault config
        let reward_assets = reward_token_denoms
            .iter()
            .map(|x| AssetInfoUnchecked::Native(x.clone()))
            .collect::<Vec<_>>();
        let config = ConfigUnchecked {
            force_withdraw_whitelist: vec![force_withdraw_admin.address().clone()],
            performance_fee,
            reward_assets,
            reward_liquidation_target: AssetInfoUnchecked::Native(reward_liquidation_target),
            treasury: treasury.address().clone(),
            liquidity_helper: osmosis_liquidity_helper.clone(),
            router: cw_dex_router.clone().into(),
        };

        // Instantiate osmosis vault contract
        let vault_addr: String = instantiate_contract_with_funds(
            app,
            admin,
            code_ids["osmosis_vault"],
            &InstantiateMsg {
                admin: admin.address().clone(),
                lockup_duration: 86400u64,
                pool_id: base_pool.pool_id(),
                vault_token_subdenom: "osmosis-vault".to_string(),
                config,
            },
            &[Coin {
                denom: UOSMO.to_string(),
                amount: Uint128::from(10_000_000u128), // 10 OSMO needed to create vault token
            }],
        )
        .unwrap();

        // WARNING!!! This is a hack
        // Send 1B base token to allow contract to create new locks on ExecuteMsg::Unlock
        bank_send(
            app,
            admin,
            &vault_addr,
            vec![Coin {
                denom: format!("gamm/pool/{}", base_pool.pool_id()),
                amount: Uint128::from(1_000_000_000u128),
            }],
        )
        .unwrap();

        println!(" ------ Addresses -------");
        println!("admin: {}", admin.address());
        println!("force_withdraw_admin: {}", force_withdraw_admin.address());
        println!("treasury: {}", treasury.address());

        println!(" ------ Contracts -------");
        println!("Vault: {}", vault_addr);
        println!("Liquidity helper: {:?}", osmosis_liquidity_helper);
        println!("CwDexRouter: {}", cw_dex_router.clone().addr().to_string());
        println!("-----------------------------------");

        Self {
            app,
            vault_addr,
            base_pool,
        }
    }

    fn query_state(&self) -> StateResponse<OsmosisStaking, OsmosisPool, OsmosisDenom> {
        let wasm = Wasm::new(self.app);
        wasm.query(
            &self.vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Apollo(ApolloExtensionQueryMsg::State {})),
        )
        .unwrap()
    }

    fn query_vault_token_balance(&self, address: &str) -> StdResult<Uint128> {
        let state = self.query_state();
        let vault_token_denom = state.vault_token.to_string();
        Ok(bank_balance_query(self.app, address.to_string(), vault_token_denom).unwrap())
    }

    fn query_base_token_balance(&self, address: &str) -> Uint128 {
        bank_balance_query(
            self.app,
            address.to_string(),
            self.base_pool.lp_token().to_string(),
        )
        .unwrap()
    }

    fn assert_vault_token_balance(&self, address: &str, expected: Uint128) -> &Self {
        assert_eq!(self.query_vault_token_balance(address).unwrap(), expected);

        self
    }

    fn assert_base_token_balance_eq(&self, address: &str, expected: Uint128) -> &Self {
        assert_eq!(self.query_base_token_balance(address), expected);

        self
    }

    fn assert_base_token_balance_gt(&self, address: &str, expected: Uint128) -> &Self {
        assert!(
            self.query_base_token_balance(address) > expected,
            "Expected {} to be greater than {}",
            self.query_base_token_balance(address),
            expected
        );

        self
    }

    fn send_base_tokens(&self, from: &SigningAccount, to: &str, amount: Uint128) -> &Self {
        bank_send(
            self.app,
            from,
            to,
            vec![Coin::new(amount.u128(), &self.base_token())],
        )
        .unwrap();

        self
    }

    fn deposit(
        &self,
        signer: &SigningAccount,
        recipient: Option<String>,
        amount: Uint128,
    ) -> &Self {
        let deposit_msg = ExecuteMsg::Deposit { amount, recipient };

        let wasm = Wasm::new(self.app);
        wasm.execute(
            &self.vault_addr,
            &deposit_msg,
            &[Coin {
                amount,
                denom: self.base_token(),
            }],
            signer,
        )
        .unwrap();

        self
    }

    fn deposit_all(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let balance = self.query_base_token_balance(&signer.address());
        self.deposit(signer, recipient, balance)
    }

    fn unlock_all(&self, signer: &SigningAccount) -> &Self {
        let balance = self.query_vault_token_balance(&signer.address()).unwrap();
        self.unlock(signer, balance)
    }

    fn vault_token(&self) -> String {
        self.query_state().vault_token.to_string()
    }

    fn unlock(&self, signer: &SigningAccount, amount: Uint128) -> &Self {
        let unlock_msg =
            ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(LockupExecuteMsg::Unlock {
                amount,
            }));

        let wasm = Wasm::new(self.app);
        wasm.execute(
            &self.vault_addr,
            &unlock_msg,
            &[Coin {
                amount,
                denom: self.vault_token(),
            }],
            signer,
        )
        .unwrap();

        self
    }

    fn query_unlocking_positions(&self, address: &str) -> Vec<UnlockingPosition> {
        let wasm = Wasm::new(self.app);
        wasm.query(
            &self.vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: address.to_string(),
                    start_after: None,
                    limit: None,
                },
            )),
        )
        .unwrap()
    }

    fn withdraw_unlocked(
        &self,
        signer: &SigningAccount,
        recipient: Option<String>,
        lockup_id: u64,
    ) -> &Self {
        let withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
            LockupExecuteMsg::WithdrawUnlocked {
                recipient,
                lockup_id,
            },
        ));

        let user1_base_token_balance_before = self.query_base_token_balance(&signer.address());

        let wasm = Wasm::new(self.app);
        wasm.execute(&self.vault_addr, &withdraw_msg, &[], signer)
            .unwrap();

        let user1_base_token_balance_after = self.query_base_token_balance(&signer.address());

        assert!(user1_base_token_balance_after > user1_base_token_balance_before);

        self
    }

    fn withdraw_first_unlocked(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let lockup_id = self.query_unlocking_positions(&signer.address())[0].id;

        self.withdraw_unlocked(signer, recipient, lockup_id)
    }

    fn base_token(&self) -> String {
        self.base_pool.lp_token().to_string()
    }

    fn assert_total_staked_base_tokens(&self, expected: Uint128) -> &Self {
        let state = self.query_state();
        assert_eq!(state.total_staked_base_tokens, expected);

        self
    }

    fn assert_vault_token_supply(&self, expected: Uint128) -> &Self {
        let state = self.query_state();
        assert_eq!(state.vault_token_supply, expected);

        self
    }

    fn assert_vault_token_share(&self, address: &str, expected: Decimal) -> &Self {
        let vault_token_supply = self.query_state().vault_token_supply;
        let vault_token_balance = self.query_vault_token_balance(address).unwrap();

        assert_eq!(
            Decimal::from_ratio(vault_token_balance, vault_token_supply),
            expected
        );

        self
    }

    fn simulate_reward_accrual(
        &self,
        signer: &SigningAccount,
        reward_denom: &str,
        amount: Uint128,
    ) -> &Self {
        if amount > Uint128::zero() {
            bank_send(
                self.app,
                &signer,
                &self.vault_addr,
                vec![Coin::new(amount.u128(), reward_denom)],
            )
            .unwrap();
        }

        self
    }
}

impl<'a> OsmosisVaultRobot<'a, OsmosisTestApp> {
    fn increase_time(&self, duration: Duration) -> &Self {
        self.app.increase_time(duration.as_secs());

        self
    }
}

use proptest::prelude::*;

const SIXTY_FOUR_BITS: u128 = 18446744073709551616u128;
const HUNDRED_BITS: u128 = 1267650600228229401496703205376u128;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        .. ProptestConfig::default()
    })]

    #[test]
    fn deposit_and_withdraw_locked_vault(deposit_ppm in 1..1_000_000u64, withdraw_percent in 1..100u64,
                            // We need to make sure that the base pool has enough liquidity to cover the compounding. For some
                            // reason inputing more liquidity than exists in the pool fails on OsmosisTestApp but apparently not
                            // on testnet.
                            base_pool_liquidity in (SIXTY_FOUR_BITS..HUNDRED_BITS, SIXTY_FOUR_BITS..HUNDRED_BITS),
                            reward_pool1_liquidity in (1000000..u64::MAX, 1000000..u64::MAX),
                            reward_pool2_liquidity in (1000000..u64::MAX, 1000000..u64::MAX),
                            performance_permille in 0..500u64,
                            reward1_amount in 10000000..u64::MAX,
                            reward2_amount in 10000000..u64::MAX) {
        let app = OsmosisTestApp::new();
        let admin = &app.init_account(&[
            Coin::new(u128::MAX, UATOM),
            Coin::new(u128::MAX, UOSMO),
            Coin::new(u128::MAX, UION),
            Coin::new(u128::MAX, STAKE),
        ]).unwrap();
        let accs = app.init_accounts(
            &[
                Coin::new(INITIAL_BALANCE, UATOM),
                Coin::new(INITIAL_BALANCE, UOSMO),
                Coin::new(INITIAL_BALANCE, UION),
                Coin::new(INITIAL_BALANCE, STAKE),
            ],
        3,
        )
        .unwrap();
        let user1 = &accs[0];
        let force_withdraw_admin = &accs[1];
        let treasury = &accs[2];

        let base_pool_liquidity = vec![Coin::new(base_pool_liquidity.0, UATOM), Coin::new(base_pool_liquidity.1, UOSMO)];
        let reward1_denom = UION.to_string();
        let reward2_denom = STAKE.to_string();
        let reward1_pool_liquidity = vec![Coin::new(reward_pool1_liquidity.0 as u128, reward1_denom.clone()), Coin::new(reward_pool1_liquidity.1 as u128, UOSMO)];
        let reward2_pool_liquidity = if reward2_amount > 0 {
            Some(vec![Coin::new(reward_pool2_liquidity.0 as u128, reward2_denom.clone()), Coin::new(reward_pool2_liquidity.1 as u128, UOSMO)])
        } else {
            None
        };
        let reward_token_denoms = vec![UION.to_string()];
        let performance_fee = Decimal::from_ratio(performance_permille, 1000u128);

        let robot = OsmosisVaultRobot::new(&app, admin, force_withdraw_admin, treasury, base_pool_liquidity,
            &reward_token_denoms, reward1_pool_liquidity, reward2_pool_liquidity, UOSMO.to_string(), performance_fee, TEST_CONFIG_PATH);

        let admin_base_token_balance = robot.query_base_token_balance(&admin.address());
        let deposit_amount = admin_base_token_balance.multiply_ratio(deposit_ppm, 1_000_000u128);
        let withdraw_percent = Decimal::percent(withdraw_percent);
        let reward_amount = Uint128::from(reward1_amount as u128);

        // Send base tokens to user1, deposit and assert that values are correct and calculate withdraw amount
        let user_vault_token_balance = robot.send_base_tokens(admin, &user1.address(), deposit_amount)
            .deposit(&user1, None, deposit_amount)
            .query_vault_token_balance(&user1.address()).unwrap();


        // Send reward tokens to user1, assert that reward token balance is correct
        let withdraw_amount = user_vault_token_balance * withdraw_percent;
        let unlocking_position_amount = robot.assert_vault_token_share(&user1.address(), Decimal::one())
            .assert_total_staked_base_tokens(deposit_amount)
            .assert_vault_token_supply(user_vault_token_balance)
            .unlock(&user1, withdraw_amount)
            .assert_vault_token_balance(&user1.address(), user_vault_token_balance - withdraw_amount)
            .query_unlocking_positions(&user1.address()).first().unwrap().base_token_amount;

        // Withdraw unlocked position and assert base token balance
        robot.increase_time(Duration::from_secs(TWO_WEEKS_IN_SECONDS))
            .withdraw_first_unlocked(&user1, None)
            .assert_base_token_balance_eq(&user1.address(), unlocking_position_amount);


        // Deposit all unlocked tokend back into vault, simulate reward accrual,
        // unlock all tokens, and assert that total base tokens is larger than deposit_amount
        robot.deposit_all(&user1, None)
            .simulate_reward_accrual(admin, &reward1_denom, reward_amount)
            .simulate_reward_accrual(admin, &reward2_denom, reward_amount)
            .deposit(admin, None, Uint128::one()) // Deposit 1 to trigger compounding
            .unlock_all(&user1)
            .increase_time(Duration::from_secs(TWO_WEEKS_IN_SECONDS))
            .withdraw_first_unlocked(&user1, None)
            .assert_base_token_balance_gt(&user1.address(), deposit_amount);
    }
}
