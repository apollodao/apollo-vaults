use apollo_cw_asset::AssetInfoUnchecked;
use apollo_vault::msg::{ApolloExtensionExecuteMsg, ExtensionExecuteMsg};
use apollo_vault::state::ConfigUpdates;
use cosmwasm_std::{Coin, Decimal, Timestamp, Uint128};

use cw_dex_router::helpers::CwDexRouterUnchecked;

use cw_it::robot::TestRobot;
use cw_it::TestRunner;

use cw_utils::Expiration;
use helpers::NeutronAstroportVaultRobot;
use liquidity_helper::LiquidityHelperUnchecked;
use neutron_astroport_vault::msg::ExecuteMsg;

use base_vault::DEFAULT_VAULT_TOKENS_PER_STAKED_BASE_TOKEN;

use test_case::test_case;

mod helpers;

const WASM_FILE_PATH: &str = "target/wasm32-unknown-unknown/release/osmosis_vault.wasm";
const UOSMO: &str = "uosmo";

const DEFAULT_POOL: ConstAstroportTestPool = ConstAstroportTestPool::new(
    &[
        ConstCoin::new(1000000000000, "uosmo"),
        ConstCoin::new(1000000000000, "uatom"),
    ],
    ,
);

const TWO_WEEKS_IN_SECONDS: u32 = 60 * 60 * 24 * 14;
const TWO_WEEKS_IN_NANOS: u64 = TWO_WEEKS_IN_SECONDS as u64 * 1_000_000_000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Funds {
    Correct,
    Insufficient,
    Excess,
    TooManyCoins,
}

// TODO: Tests for compounding

#[test_case(false, Funds::Correct ; "normal deposit")]
#[test_case(true, Funds::Correct ; "deposit to different recipient")]
#[test_case(false, Funds::Insufficient => panics ; "insufficient funds")]
#[test_case(false, Funds::Excess => panics ; "excess funds")]
#[test_case(false, Funds::TooManyCoins => panics ; "too many coins in funds")]
fn deposit(different_recipient: bool, funds: Funds) {
    let app = OsmosisTestApp::new();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let recipient = if different_recipient {
        Some(app.init_account(&[]).unwrap().address())
    } else {
        None
    };

    let vault_token_denom = robot.query_info().vault_token;
    let base_token_denom = robot.query_info().base_token;
    let deposit_amount = Uint128::new(1_000_000_000_000_000u128);
    let funds = match funds {
        Funds::Correct => vec![Coin::new(deposit_amount.u128(), &base_token_denom)],
        Funds::Insufficient => vec![Coin::new(deposit_amount.u128() - 1000, &base_token_denom)],
        Funds::Excess => vec![Coin::new(deposit_amount.u128() + 1000, &base_token_denom)],
        Funds::TooManyCoins => vec![
            Coin::new(deposit_amount.u128(), &base_token_denom),
            Coin::new(1000u128, UOSMO),
        ],
    };

    robot
        .deposit(&admin, deposit_amount, recipient.clone(), &funds)
        .assert_native_token_balance_eq(
            recipient.unwrap_or(admin.address()),
            &vault_token_denom,
            deposit_amount * DEFAULT_VAULT_TOKENS_PER_STAKED_BASE_TOKEN,
        );
}

#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::Correct ; "correct funds")]
#[test_case(Uint128::zero(), Funds::Correct => panics ; "zero amount correct funds")]
#[test_case(Uint128::zero(), Funds::Excess => panics ; "zero amount excess funds")]
#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::Insufficient => panics ; "insufficient funds")]
#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::Excess => panics ; "excess funds")]
#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::TooManyCoins => panics ; "too many coins in funds")]
fn redeem(unlock_amount: Uint128, funds_type: Funds) {
    let app = TestRunner::from_env_var().unwrap();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        NeutronAstroportVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let vault_token_denom = robot.query_info().vault_token;
    let funds = match funds_type {
        Funds::Correct => vec![Coin::new(unlock_amount.u128(), &vault_token_denom)],
        Funds::Insufficient => vec![Coin::new(unlock_amount.u128() - 1000, &vault_token_denom)],
        Funds::Excess => vec![Coin::new(unlock_amount.u128() + 1000, &vault_token_denom)],
        Funds::TooManyCoins => vec![
            Coin::new(unlock_amount.u128(), &vault_token_denom),
            Coin::new(1000u128, UOSMO),
        ],
    };

    robot
        .deposit_all(&admin, None)
        .redeem_all(&admin, None);

    // These assertions are only valid if the funds are correct. Otherwise, the
    // transaction should fail above.
    match funds_type {
        Funds::Correct => {
            let unlocking_pos = robot
                .assert_number_of_unlocking_position(admin.address(), 1)
                .query_unlocking_positions(admin.address())[0]
                .clone();

            let unlock_time = robot.app.get_block_time_nanos() + TWO_WEEKS_IN_NANOS as i64;

            assert_eq!(
                // No compounding has occured so the ration vault tokens to base tokens should
                // not have changed
                unlocking_pos.base_token_amount,
                unlock_amount
                    .multiply_ratio(1u128, DEFAULT_VAULT_TOKENS_PER_STAKED_BASE_TOKEN.u128())
            );
            assert_eq!(unlocking_pos.owner.to_string(), admin.address());
            assert_eq!(
                unlocking_pos.release_at,
                Expiration::AtTime(Timestamp::from_nanos(unlock_time as u64))
            );
        }
        _ => {}
    }
}


#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::Correct ; "correct funds")]
#[test_case(Uint128::zero(), Funds::Correct => panics ; "zero amount correct funds")]
#[test_case(Uint128::zero(), Funds::Excess => panics ; "zero amount excess funds")]
#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::Insufficient => panics ; "insufficient funds")]
#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::Excess => panics ; "excess funds")]
#[test_case(Uint128::new(1_000_000_000_000_000u128), Funds::TooManyCoins => panics ; "too many coins in funds")]
fn redeem(recipient: Option<String>, funds_type: Funds) -> String {
    let app = OsmosisTestApp::new();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let recipient = recipient.map(|_| fwa_admin.address());
    robot.deposit_all(&admin, None);

    let funds = match funds_type {
        Funds::Correct => vec![Coin::new(unlock_amount.u128(), &vault_token_denom)],
        Funds::Insufficient => vec![Coin::new(unlock_amount.u128() - 1000, &vault_token_denom)],
        Funds::Excess => vec![Coin::new(unlock_amount.u128() + 1000, &vault_token_denom)],
        Funds::TooManyCoins => vec![
            Coin::new(unlock_amount.u128(), &vault_token_denom),
            Coin::new(1000u128, UOSMO),
        ],
    };

    let amount = Uint128::new(1000000000u128);
    let vault_token_denom = robot.query_info().vault_token;
    robot
        .wasm()
        .execute(
            &robot.vault_addr,
            &ExecuteMsg::Redeem { amount, recipient },
            &[Coin::new(amount.u128(), vault_token_denom)],
            &admin,
        )
        .unwrap_err()
        .to_string()
}

#[test_case(false => panics ; "caller is not admin")]
#[test_case(true ; "caller is admin")]
fn update_config(is_admin: bool) {
    let app = OsmosisTestApp::new();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let accs = app
        .init_accounts(&[Coin::new(1000000000u128, UOSMO)], 5)
        .unwrap();

    let caller = if is_admin { &admin } else { &accs[3] };

    let mut config_updates = ConfigUpdates::default();
    config_updates
        .performance_fee(Decimal::percent(50))
        .treasury(accs[0].address())
        .router(CwDexRouterUnchecked::new(accs[1].address()))
        .reward_assets(vec![AssetInfoUnchecked::native(
            "new_reward_token".to_string(),
        )])
        .reward_liquidation_target(AssetInfoUnchecked::native("new_reward_token".to_string()))
        .force_withdraw_whitelist(vec![])
        .liquidity_helper(LiquidityHelperUnchecked::new(accs[2].address()));

    robot.update_config(caller, config_updates.clone());

    // Assertion is only valid if the caller is the admin. Otherwise, the
    // transaction should fail above.
    if is_admin {
        robot.assert_config(config_updates.build().unwrap());
    }
}

#[test_case(true, true ; "caller is admin and new admin is a valid address")]
#[test_case(true, false => panics ; "caller is admin but new admin is invalid address")]
#[test_case(false, true => panics ; "caller is not admin")]
#[test_case(false, false => panics ; "caller is not admin and new admin is invalid address")]
fn update_admin(caller_is_admin: bool, new_admin_is_valid_address: bool) {
    let app = OsmosisTestApp::new();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let accs = app
        .init_accounts(&[Coin::new(1000000000u128, UOSMO)], 2)
        .unwrap();

    let caller = if caller_is_admin { &admin } else { &accs[0] };
    let new_admin = if new_admin_is_valid_address {
        accs[1].address()
    } else {
        "invalid_addr".to_string()
    };

    robot.update_admin(caller, &new_admin);
}

#[test_case(true ; "caller is new admin")]
#[test_case(false => panics ; "caller is not new admin")]
fn accept_admin_transfer(caller_is_new_admin: bool) {
    let app = OsmosisTestApp::new();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    let new_admin = app
        .init_account(&[Coin::new(1000000000u128, UOSMO)])
        .unwrap();
    let user = app
        .init_account(&[Coin::new(1000000000u128, UOSMO)])
        .unwrap();
    let caller = if caller_is_new_admin {
        &new_admin
    } else {
        &user
    };

    robot
        .setup(&admin)
        .update_admin(&admin, new_admin.address())
        .assert_admin(admin.address())
        .accept_admin_transfer(caller)
        .assert_admin(new_admin.address());
}

#[test_case(true ; "caller is admin")]
#[test_case(false => panics ; "caller is not admin")]
fn drop_admin_transfer(caller_is_admin: bool) {
    let app = OsmosisTestApp::new();
    let pool: AstroportTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    let new_admin = app
        .init_account(&[Coin::new(1000000000u128, UOSMO)])
        .unwrap();
    let user = app
        .init_account(&[Coin::new(1000000000u128, UOSMO)])
        .unwrap();
    let caller = if caller_is_admin { &admin } else { &user };

    robot
        .setup(&admin)
        .update_admin(&admin, new_admin.address())
        .assert_admin(admin.address())
        .drop_admin_transfer(caller);

    // If admin transfer is dropped, the admin should still be the original admin.
    // And AcceptAdminTransfer should fail.
    if caller_is_admin {
        robot
            .wasm()
            .execute(
                &robot.vault_addr,
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::AcceptAdminTransfer {},
                )),
                &[],
                &new_admin,
            )
            .unwrap_err();

        robot.assert_admin(admin.address());
    }
}
