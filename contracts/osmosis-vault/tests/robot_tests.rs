use apollo_cw_asset::AssetInfoUnchecked;
use apollo_vault::msg::{ApolloExtensionExecuteMsg, ExtensionExecuteMsg};
use apollo_vault::state::ConfigUpdates;
use apollo_vault_test_helpers::osmosis::OsmosisVaultRobot;
use cosmwasm_std::{Coin, Decimal, Timestamp, Uint128};

use cw_dex_router::helpers::CwDexRouterUnchecked;

use cw_it::osmosis::robot::OsmosisTestRobot;
use cw_it::osmosis::OsmosisTestPool;
use cw_it::osmosis_test_tube::{
    Account, FeeSetting, Module, OsmosisTestApp, Runner, SigningAccount, Wasm,
};
use cw_it::robot::TestRobot;

use cw_utils::Expiration;
use liquidity_helper::LiquidityHelperUnchecked;
use osmosis_vault::msg::ExecuteMsg;

use base_vault::DEFAULT_VAULT_TOKENS_PER_STAKED_BASE_TOKEN;

use cw_it::const_coin::ConstCoin;
use cw_it::osmosis::{ConstOsmosisTestPool, OsmosisPoolType};

use test_case::test_case;

const WASM_FILE_PATH: &str = "target/wasm32-unknown-unknown/release/osmosis_vault.wasm";
const UOSMO: &str = "uosmo";

const DEFAULT_POOL: ConstOsmosisTestPool = ConstOsmosisTestPool::new(
    &[
        ConstCoin::new(1000000000000, "uosmo"),
        ConstCoin::new(1000000000000, "uatom"),
    ],
    OsmosisPoolType::Basic,
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

#[test_case(false, false, None, false, false => panics ; "caller not whitelisted")]
#[test_case(true, false, None, false, false ; "lock not expired amount is None recipient is None")]
#[test_case(true, false, None, true, false ; "lock not expired amount is None recipient is Some")]
#[test_case(true, false, Some(Decimal::zero()), false, false => panics ; "lock not expired amount is Some(0) recipient is none")]
#[test_case(true, false, Some(Decimal::percent(50)), false, false ; "lock not expired amount is Some(50%) recipient is none")]
#[test_case(true, false, Some(Decimal::percent(100)), false, false ; "lock not expired amount is Some(100%) recipient is none")]
#[test_case(true, false, Some(Decimal::percent(150)), false, false => panics ; "lock not expired amount is Some(150%) recipient is none")]
#[test_case(true, true, None, false, false => ; "lock is expired amount is None recipient is None")]
#[test_case(true, true, None, false, true => ; "lock is expired amount is None recipient is None multiple unlocking positions")]
fn force_withdraw_unlocking(
    whitlisted: bool,
    expired: bool,
    force_unlock_amount: Option<Decimal>,
    different_recipient: bool,
    multiple_unlocking_positions: bool,
) {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

    let (robot, admin, mut fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);

impl<'a> OsmosisVaultRobot<'a, OsmosisTestApp> {
    // pub fn new(app: &'a OsmosisTestApp, vault_addr: String) -> Self {
    //     Self { app, vault_addr }
    // }

    // pub fn without_rewards(
    //     app: &'a OsmosisTestApp,
    //     base_pool: OsmosisTestPool,
    //     wasm_file_path: &str,
    // ) -> (Self, &SigningAccount, &SigningAccount, &SigningAccount) {
    //     Self::with_single_rewards(app, base_pool.clone(), base_pool,
    // wasm_file_path) }

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
            .unwrap()
            .with_fee_setting(FeeSetting::Auto {
                gas_price: Coin::new(100, "uosmo"),
                gas_adjustment: 1.5,
            });
        println!("admin fee setting: {:?}", admin.fee_setting());
        let fwa_admin = app
            .init_account(&max_of_all_coins(&[
                base_pool.liquidity.clone(),
                reward_pool.liquidity.clone(),
            ]))
            .unwrap()
            .with_fee_setting(FeeSetting::Auto {
                gas_price: Coin::new(100, "uosmo"),
                gas_adjustment: 1.5,
            });
        let treasury = app.init_account(&[]).unwrap();
        let base_pool_id = base_pool.create(app, &admin);

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

    let recipient = match different_recipient {
        true => Some(app.init_account(&[]).unwrap().address()),
        false => None,
    };
    let increase_time_by = if expired { 3600 * 24 * 15 } else { 0 };

    println!("Whitelisting address: {}", fwa_admin.address());

    robot
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
        .unlock_all(&fwa_admin);

    if multiple_unlocking_positions {
        robot.deposit_all(&admin, None).unlock_all(&admin);
    }

    let unlocking_pos = &robot
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

#[test_case(false, Decimal::percent(100), false, Funds::Correct => panics ; "caller not whitelisted")]
#[test_case(true, Decimal::percent(50), false, Funds::Correct ; "caller whitelisted withdraw half")]
#[test_case(true, Decimal::percent(100), false, Funds::Correct ; "caller whitelisted withdraw all")]
#[test_case(true, Decimal::percent(150), false, Funds::Correct => panics ; "caller whitelisted withdraw too much")]
#[test_case(true, Decimal::percent(100), true, Funds::Correct ; "caller whitelisted withdraw all to different recipient")]
#[test_case(true, Decimal::percent(100), false, Funds::Insufficient => panics ; "caller whitelisted withdraw all insufficient funds")]
#[test_case(true, Decimal::percent(100), false, Funds::Excess => panics ; "caller whitelisted withdraw all excess funds")]
#[test_case(true, Decimal::percent(100), false, Funds::TooManyCoins => panics ; "caller whitelisted withdraw all too many coins in funds")]
fn force_redeem(
    whitlisted: bool,
    withdraw_percent: Decimal,
    different_recipient: bool,
    funds_type: Funds,
) {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

    let (robot, admin, mut fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);

    if !whitlisted {
        fwa_admin = app
            .init_account(&[Coin::new(1000000000000000, UOSMO)])
            .unwrap();
    }

    let recipient = if different_recipient {
        Some(app.init_account(&[]).unwrap().address())
    } else {
        None
    };

    let base_token_denom = robot.query_info().base_token;
    let vault_token_denom = robot.query_info().vault_token;

    let initial_base_token_balance = robot
        .setup(&admin)
        .join_pool_swap_extern_amount_in(
            &fwa_admin,
            robot.base_pool.pool_id(),
            Coin::new(1000000000, UOSMO),
            None,
        )
        .query_native_token_balance(fwa_admin.address(), &base_token_denom);

    let vault_token_balance = robot
        .deposit_all(&fwa_admin, None)
        .query_native_token_balance(fwa_admin.address(), &vault_token_denom);

    let redeem_amount = withdraw_percent * vault_token_balance;
    let recipient_addr = recipient.clone().unwrap_or(fwa_admin.address());
    let funds = match funds_type {
        Funds::Correct => vec![Coin::new(redeem_amount.u128(), &vault_token_denom)],
        Funds::Insufficient => vec![Coin::new(1000u128, &vault_token_denom)],
        Funds::TooManyCoins => vec![
            Coin::new(redeem_amount.u128(), &vault_token_denom),
            Coin::new(1000u128, UOSMO),
        ],
        Funds::Excess => vec![Coin::new(redeem_amount.u128() + 1000, &vault_token_denom)],
    };

    robot.force_redeem(&fwa_admin, redeem_amount, recipient, &funds);

    // These assertions are only valid if the funds are correct. Otherwise,
    // the transaction should fail above.
    match funds_type {
        Funds::Correct => {
            robot
                .assert_native_token_balance_eq(
                    &recipient_addr,
                    &base_token_denom,
                    // Since no compounding is done, the amount withdrawn should be
                    // exactly withdraw_percent of the initial deposit
                    withdraw_percent * initial_base_token_balance,
                )
                .assert_native_token_balance_eq(
                    &recipient_addr,
                    &vault_token_denom,
                    vault_token_balance - redeem_amount,
                );
        }
        _ => {}
    }
}

#[test_case(false, Funds::Correct ; "normal deposit")]
#[test_case(true, Funds::Correct ; "deposit to different recipient")]
#[test_case(false, Funds::Insufficient => panics ; "insufficient funds")]
#[test_case(false, Funds::Excess => panics ; "excess funds")]
#[test_case(false, Funds::TooManyCoins => panics ; "too many coins in funds")]
fn deposit(different_recipient: bool, funds: Funds) {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

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
fn unlock(unlock_amount: Uint128, funds_type: Funds) {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
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
        .unlock_with_funds(&admin, unlock_amount, &funds);

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

//TODO: Multiple different users unlocking at the same time
#[test_case(false, false, false => panics ; "not owner withdraws to self lock not expired")]
#[test_case(false, false, true => panics ; "not owner withdraws to self lock expired")]
#[test_case(false, true, false => panics ; "not owner withdraws to different recipient lock not expired")]
#[test_case(false, true, true => panics ; "not owner withdraws to different recipient lock expired")]
#[test_case(true, false, false => panics ; "owner withdraws to self lock not expired")]
#[test_case(true, false, true ; "owner withdraws to self lock expired")]
#[test_case(true, true, false => panics ; "owner withdraws to different recipient lock not expired")]
#[test_case(true, true, true ; "owner withdraws to different recipient lock expired")]
fn withdraw_unlocked(is_owner: bool, different_recipient: bool, expired: bool) {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

    let (robot, admin, fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let base_token_denom = robot.query_info().base_token;
    let base_token_balance = robot.query_native_token_balance(admin.address(), base_token_denom);

    let withdrawer = if is_owner { &admin } else { &fwa_admin };
    let recipient = if different_recipient {
        Some(app.init_account(&[]).unwrap().address())
    } else {
        None
    };

    let increase_time_by = if expired { TWO_WEEKS_IN_SECONDS } else { 0 };

    robot
        .deposit_all(&admin, None)
        .unlock_all(&admin)
        .increase_time(increase_time_by as u64)
        .withdraw_first_unlocked(withdrawer, recipient.clone());

    // These assertions are only valid if the withdrawer is the owner. Otherwise,
    // the transaction should fail above.
    if is_owner {
        robot
            .assert_number_of_unlocking_position(admin.address(), 0)
            .assert_base_token_balance_eq(recipient.unwrap_or(admin.address()), base_token_balance);
    }
}

#[test_case(false => panics ; "caller is not admin")]
#[test_case(true ; "caller is admin")]
fn update_force_withdraw_whitelist(is_admin: bool) {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

    let (robot, admin, _fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);
    let user = app
        .init_account(&[Coin::new(1000000000u128, UOSMO)])
        .unwrap();

    let caller = if is_admin { &admin } else { &user };

    robot.update_force_withdraw_whitelist(caller, vec![admin.address(), user.address()], vec![]);

    if is_admin {
        robot
            .assert_whitelist_contains(admin.address())
            .assert_whitelist_contains(user.address())
            .update_force_withdraw_whitelist(caller, vec![], vec![admin.address(), user.address()])
            .assert_whitelist_not_contains(admin.address())
            .assert_whitelist_not_contains(user.address());
    }
}

#[test_case(Some("recipient".to_string()) => "execute error: failed to execute message; message index: 0: Redeem is not supported for locked vaults. Use Unlock and WithdrawUnlocked.: execute wasm contract failed"
     ; "recipient is Some")]
#[test_case(None => "execute error: failed to execute message; message index: 0: Redeem is not supported for locked vaults. Use Unlock and WithdrawUnlocked.: execute wasm contract failed"
     ; "recipient is None")]
fn redeem(recipient: Option<String>) -> String {
    let app = OsmosisTestApp::new();
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

    let (robot, admin, fwa_admin, _treasury) =
        OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
    robot.setup(&admin);

    let recipient = recipient.map(|_| fwa_admin.address());
    robot.deposit_all(&admin, None);

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
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

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
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

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
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

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
    let pool: OsmosisTestPool = DEFAULT_POOL.into();

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

    #[test_case(false => panics ; "caller is not admin")]
    #[test_case(true ; "caller is admin")]
    fn update_config(is_admin: bool) {
        let app = OsmosisTestApp::new();
        let pool: OsmosisTestPool = DEFAULT_POOL.into();

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

        robot.update_config(&caller, config_updates.clone());

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
        let pool: OsmosisTestPool = DEFAULT_POOL.into();

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

        robot.update_admin(&caller, &new_admin);
    }

    #[test_case(true ; "caller is new admin")]
    #[test_case(false => panics ; "caller is not new admin")]
    fn accept_admin_transfer(caller_is_new_admin: bool) {
        let app = OsmosisTestApp::new();
        let pool: OsmosisTestPool = DEFAULT_POOL.into();

        let (robot, admin, _fwa_admin, _treasury) =
            OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
        let new_admin = app
            .init_account(&[Coin::new(1000000000u128, UOSMO)])
            .unwrap()
            .with_fee_setting(FeeSetting::Auto {
                gas_price: Coin::new(100, "uosmo"),
                gas_adjustment: 1.5,
            });
        let user = app
            .init_account(&[Coin::new(1000000000u128, UOSMO)])
            .unwrap()
            .with_fee_setting(FeeSetting::Auto {
                gas_price: Coin::new(100, "uosmo"),
                gas_adjustment: 1.5,
            });
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
        let pool: OsmosisTestPool = DEFAULT_POOL.into();

        let (robot, admin, _fwa_admin, _treasury) =
            OsmosisVaultRobot::with_single_rewards(&app, pool.clone(), pool, WASM_FILE_PATH);
        let new_admin = app
            .init_account(&[Coin::new(1000000000u128, UOSMO)])
            .unwrap()
            .with_fee_setting(FeeSetting::Auto {
                gas_price: Coin::new(100, "uosmo"),
                gas_adjustment: 1.5,
            });
        let user = app
            .init_account(&[Coin::new(1000000000u128, UOSMO)])
            .unwrap()
            .with_fee_setting(FeeSetting::Auto {
                gas_price: Coin::new(100, "uosmo"),
                gas_adjustment: 1.5,
            });
        let caller = if caller_is_admin { &admin } else { &user };

        robot
            .setup(&admin)
            .update_admin(&admin, new_admin.address())
            .assert_admin(admin.address())
            .drop_admin_transfer(&caller);

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
}
