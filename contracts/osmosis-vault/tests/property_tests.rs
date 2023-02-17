use cosmwasm_std::testing::{MockApi, MockStorage};
use cosmwasm_std::{Coin, Decimal, Deps, Empty, Querier, QuerierWrapper, Uint128};

use std::time::Duration;

use osmosis_testing::{Account, OsmosisTestApp};

mod test_helpers;

use test_helpers::robot::OsmosisVaultRobot;

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

use proptest::prelude::*;

use crate::test_helpers::constants::{
    INITIAL_BALANCE, STAKE, TEST_CONFIG_PATH, TWO_WEEKS_IN_SECONDS, UATOM, UION, UOSMO,
};

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
