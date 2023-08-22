use std::collections::HashSet;
use std::str::FromStr;

use apollo_cw_asset::{AssetInfo, AssetInfoUnchecked};
use apollo_vault::msg::{
    ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg, StateResponse,
};
use apollo_vault::state::ConfigUnchecked;
use base_vault::DEFAULT_VAULT_TOKENS_PER_STAKED_BASE_TOKEN;
use cosmwasm_std::{Coin, Decimal, Empty, Uint128};
use cw_dex::osmosis::{OsmosisPool, OsmosisStaking};
use cw_dex::traits::Pool as PoolTrait;
use cw_dex::Pool;
use cw_dex_router::helpers::CwDexRouterUnchecked;
use cw_dex_router::operations::{SwapOperation, SwapOperationsList};
use cw_it::config::TestConfig;
use cw_it::helpers::{
    bank_send, instantiate_contract, instantiate_contract_with_funds, upload_wasm_files,
};
use cw_it::mock_api::OsmosisMockApi;
use cw_vault_standard::extensions::force_unlock::ForceUnlockExecuteMsg;
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg, UnlockingPosition};
use osmosis_std::types::osmosis::lockup::{
    MsgBeginUnlocking, MsgBeginUnlockingResponse, MsgLockTokens, MsgLockTokensResponse,
};
use osmosis_vault::msg::ExecuteMsg;

use cw_vault_token::osmosis::OsmosisDenom;
use liquidity_helper::LiquidityHelperUnchecked;
use osmosis_testing::cosmrs::proto::cosmos::bank::v1beta1::{MsgSend, QueryBalanceRequest};
use osmosis_testing::cosmrs::proto::cosmos::base::v1beta1::Coin as ProtoCoin;
use osmosis_testing::Bank;
use osmosis_testing::{
    cosmrs::proto::cosmwasm::wasm::v1::MsgExecuteContractResponse, Account, Gamm, Module,
    OsmosisTestApp as BindingsRunner, Runner, SigningAccount, Wasm,
};
use osmosis_vault::msg::{InstantiateMsg, QueryMsg};
use test_case::test_case;

const TEST_CONFIG_PATH: &str = "tests/configs/osmosis.yaml";
const UATOM: &str = "uatom";
const UOSMO: &str = "uosmo";
const UION: &str = "uion";
const STAKE: &str = "stake";
const PERFORMANCE_FEE: Decimal = Decimal::raw(5 * 10u128.pow(16)); // 5%

/// Set up the contracts and pools needed to run the tests
/// Returns (String,String) of (vault_addr, base_token_addr)
pub fn setup_test<'a, R>(
    runner: &'a R,
    base_pool_liquidity: Vec<Coin>,
    reward_token_denoms: &Vec<String>,
    reward1_pool_liquidity: Vec<Coin>,
    reward2_pool_liquidity: Option<Vec<Coin>>,
    reward_liquidation_target: String,
    accs: &[SigningAccount],
    test_config: &TestConfig,
) -> (String, String)
where
    R: Runner<'a>,
{
    let gamm = Gamm::new(runner);
    let api = OsmosisMockApi::new();

    let admin = &accs[0];
    let force_withdraw_admin = &accs[1];
    let treasury = &accs[2];
    let user1 = &accs[3];
    let user2 = &accs[4];

    // Create base pool (the pool this vault will compound)
    let pool_id = gamm
        .create_basic_pool(&base_pool_liquidity, user1)
        .unwrap()
        .data
        .pool_id;
    println!("Pool ID: {}", pool_id);
    let base_pool = OsmosisPool::unchecked(pool_id);
    let base_token = base_pool.lp_token();

    // Create pool for first reward token
    let pool_id = gamm
        .create_basic_pool(&reward1_pool_liquidity, admin)
        .unwrap()
        .data
        .pool_id;
    let reward1_pool = OsmosisPool::unchecked(pool_id);
    let reward1_token = reward1_pool_liquidity
        .iter()
        .find(|x| x.denom != reward_liquidation_target)
        .unwrap()
        .denom
        .clone();

    // Lock reward LP tokens to increase global lock ID count.
    // Since test-tube starts with a clean state, we do this to ensure that the
    // lock ID of the vault is not 1.
    // This is to test a bug we found where the correct lock ID was not being used.
    let stake_msg = MsgLockTokens {
        owner: admin.address().clone(),
        duration: Some(osmosis_std::shim::Duration {
            seconds: 1,
            nanos: 0,
        }),
        coins: vec![Coin::new(1000000000u128, reward1_pool.lp_token().to_string()).into()],
    };
    let res = runner
        .execute::<_, MsgLockTokensResponse>(stake_msg, "/osmosis.lockup.MsgLockTokens", admin)
        .unwrap();
    let lock_id = res.data.id;
    // Unlock position to remove lock
    let unstake_msg = MsgBeginUnlocking {
        owner: admin.address().clone(),
        id: lock_id,
        coins: vec![Coin::new(1000000000u128, reward1_pool.lp_token().to_string()).into()],
    };
    runner
        .execute::<_, MsgBeginUnlockingResponse>(
            unstake_msg,
            "/osmosis.lockup.MsgBeginUnlocking",
            admin,
        )
        .unwrap();

    // Create pool for second reward token (if set)
    let reward2_pool = reward2_pool_liquidity.clone().map(|liquidity| {
        let pool_id = gamm
            .create_basic_pool(&liquidity, admin)
            .unwrap()
            .data
            .pool_id;
        OsmosisPool::unchecked(pool_id)
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
    let code_ids = upload_wasm_files(runner, admin, test_config.clone()).unwrap();

    // Instantiate Osmosis Liquidity Helper
    let osmosis_liquidity_helper = instantiate_contract::<_, _, LiquidityHelperUnchecked>(
        runner,
        admin,
        code_ids["osmosis_liquidity_helper"],
        &Empty {},
    )
    .unwrap();

    // Instantiate CwDexRouter
    let cw_dex_router = instantiate_contract::<_, _, CwDexRouterUnchecked>(
        runner,
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
        runner
            .execute_cosmos_msgs::<MsgExecuteContractResponse>(&[msg], admin)
            .unwrap();
    };
    update_path_for_reward_pool(reward1_token.clone(), Pool::Osmosis(reward1_pool));
    if let Some(reward2_token) = &reward2_token {
        update_path_for_reward_pool(reward2_token.clone(), Pool::Osmosis(reward2_pool.unwrap()));
    }

    // Create vault config
    let reward_assets = reward_token_denoms
        .iter()
        .map(|x| AssetInfoUnchecked::Native(x.clone()))
        .collect::<Vec<_>>();
    let config = ConfigUnchecked {
        force_withdraw_whitelist: vec![force_withdraw_admin.address().clone()],
        performance_fee: PERFORMANCE_FEE,
        reward_assets,
        reward_liquidation_target: AssetInfoUnchecked::Native(reward_liquidation_target),
        treasury: treasury.address().clone(),
        liquidity_helper: osmosis_liquidity_helper.clone(),
        router: cw_dex_router.clone().into(),
    };

    // Instantiate osmosis vault contract
    let vault_addr: String = instantiate_contract_with_funds(
        runner,
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
            // 10 OSMO needed to create vault token
            denom: UOSMO.to_string(),
            amount: Uint128::from(10_000_000u128),
        }],
    )
    .unwrap();

    // WARNING!!! This is a hack
    // Send 1B base token to allow contract to create new locks on ExecuteMsg::Unlock
    bank_send(
        runner,
        user1,
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
    println!("user1: {}", user1.address());
    println!("user2: {}", user2.address());

    println!(" ------ Contracts -------");
    println!("Vault: {}", vault_addr);
    println!("Liquidity helper: {:?}", osmosis_liquidity_helper);
    println!("CwDexRouter: {}", cw_dex_router.clone().addr().to_string());
    println!("-----------------------------------");

    (vault_addr, base_token.to_string())
}

#[test_case(
    vec![Coin::new(1_000_000_000_000, UATOM),Coin::new(1_000_000_000_000, UOSMO)],
    vec![String::from(UION)],
    vec![Coin::new(1_000_000_000_000, UION),Coin::new(1_000_000_000_000, UOSMO)],
    None,
    UOSMO.to_string();
    "uatom-osmo pool with ion rewards and uosmo liquidation target")]
#[test_case(
    vec![Coin::new(1_000_000_000_000, UATOM), Coin::new(1_000_000_000_000, UOSMO)],
    vec![String::from(UION), String::from(STAKE)],
    vec![Coin::new(1_000_000_000_000, UION),Coin::new(1_000_000_000_000, UOSMO)],
    Some(vec![Coin::new(1_000_000_000_000, STAKE), Coin::new(1_000_000_000_000, UOSMO)]),
    UOSMO.to_string();
    "uatom-osmo pool with uion and stake rewards and uosmo liquidation target")]
#[test_case(
    vec![Coin::new(1_000_000_000_000, UATOM), Coin::new(1_000_000_000_000, UOSMO)],
    vec![String::from(UATOM)],
    vec![Coin::new(1_000_000_000_000, UATOM),Coin::new(1_000_000_000_000, UOSMO)],
    None,
    UOSMO.to_string();
    "uatom-osmo pool with atom rewards and osmo liquidation target")]
#[test_case(
    vec![Coin::new(1_000_000_000_000, UATOM), Coin::new(1_000_000_000_000, UOSMO)],
    vec![String::from(UOSMO)],
    vec![Coin::new(1_000_000_000_000, UATOM),Coin::new(1_000_000_000_000, UOSMO)],
    None,
    UOSMO.to_string();
    "uatom-osmo pool with osmo rewards and osmo liquidation target")]
#[test_case(
    vec![Coin::new(1_000, UATOM),Coin::new(1_000, UOSMO)],
    vec![String::from(UION)],
    vec![Coin::new(1_000, UION),Coin::new(1_000, UOSMO)],
    None,
    UOSMO.to_string();
    "uatom-osmo pool with ion rewards and uosmo liquidation target, low reward and low pool liquidity")]
#[test_case(
    vec![Coin::new(1_000_000_000_000, UATOM),Coin::new(1_000_000_000_000, UOSMO)],
    vec![String::from(UION)],
    vec![Coin::new(1_000, UION),Coin::new(1_000, UOSMO)],
    None,
    UOSMO.to_string();
    "uatom-osmo pool with ion rewards and uosmo liquidation target, low reward liquidity")]
pub fn test_osmosis_vault_functionality(
    base_pool_liquidity: Vec<Coin>,
    reward_token_denoms: Vec<String>,
    reward1_pool_liquidity: Vec<Coin>,
    reward2_pool_liquidity: Option<Vec<Coin>>,
    reward_liquidation_target: String,
) {
    let test_config = TestConfig::from_yaml(TEST_CONFIG_PATH);

    // Run with Bindings
    // We currently can't run these tests against LocalOsmosis since we have no
    // way to increase the time of the chain to bypass the 2 week unbonding period,
    // nor do we have a way to set the force withdraw whitelisted addresses.
    let runner = BindingsRunner::default();
    let accs = runner
        .init_accounts(
            &[
                Coin::new(1_000_000_000_000_000_000_000_000, UATOM),
                Coin::new(1_000_000_000_000_000_000_000_000, UOSMO),
                Coin::new(1_000_000_000_000_000_000_000_000, UION),
                Coin::new(1_000_000_000_000_000_000_000_000, STAKE),
            ],
            10,
        )
        .unwrap();

    let admin = &accs[0];
    let force_withdraw_admin = &accs[1];
    let treasury = &accs[2];
    let user1 = &accs[3];
    let user2 = &accs[4];

    let wasm = Wasm::new(&runner);

    // Setup test
    let (vault_addr, base_token) = setup_test(
        &runner,
        base_pool_liquidity,
        &reward_token_denoms,
        reward1_pool_liquidity,
        reward2_pool_liquidity,
        reward_liquidation_target,
        &accs,
        &test_config,
    );

    // Query vault state
    let state: StateResponse<OsmosisStaking, OsmosisPool, OsmosisDenom> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Apollo(ApolloExtensionQueryMsg::State {})),
        )
        .unwrap();
    let vault_token_denom = state.vault_token.to_string();
    println!("Vault token denom: {}", vault_token_denom);

    // Query user1 base token balance
    let base_token_balance = query_token_balance(&runner, &user1.address(), &base_token);
    println!("User1 base token balance: {}", base_token_balance);

    // Track how much user 1 deposits (different depending on number of reward tokens)
    let mut user1_total_deposit_amount = Uint128::zero();

    // Deposit into vault
    // Use 1/2 of base token balance to deposit
    let deposit_amount = base_token_balance / Uint128::from(2u128);
    user1_total_deposit_amount += deposit_amount;
    let deposit_msg = ExecuteMsg::Deposit {
        amount: deposit_amount,
        recipient: None,
    };
    let _res = wasm
        .execute(
            &vault_addr,
            &deposit_msg,
            &[Coin {
                amount: deposit_amount,
                denom: base_token.to_string(),
            }],
            user1,
        )
        .unwrap();

    // Query user1 vault token balance
    let vault_token_balance = query_token_balance(&runner, &user1.address(), &vault_token_denom);
    println!("User1 vault token balance: {}", vault_token_balance);
    assert_ne!(vault_token_balance, Uint128::zero());

    // Assert total staked amount and vault token supply is correct
    let state = query_vault_state(&runner, &vault_addr);
    let total_staked_amount = state.total_staked_base_tokens;
    let vault_token_supply = state.vault_token_supply;
    assert_eq!(total_staked_amount, deposit_amount);
    assert_eq!(vault_token_supply, vault_token_balance);
    assert_eq!(
        vault_token_supply,
        total_staked_amount * DEFAULT_VAULT_TOKENS_PER_STAKED_BASE_TOKEN
    );

    // Send some reward tokens to vault to simulate reward accruing
    let reward_amount = Uint128::from(100_000_000u128);
    send_native_coins(
        &runner,
        admin,
        &vault_addr,
        &reward_token_denoms[0],
        reward_amount,
    );

    // Query treasury reward token balance
    let treasury_reward_token_balance_before =
        query_token_balance(&runner, &treasury.address(), &reward_token_denoms[0]);

    // Query vault state
    let state = query_vault_state(&runner, &vault_addr);
    let total_staked_amount_before_compound_deposit = state.total_staked_base_tokens;

    // Deposit some more base token to vault to trigger compounding
    let deposit_amount = Uint128::from(100_000u128);
    user1_total_deposit_amount += deposit_amount;
    let deposit_msg = ExecuteMsg::Deposit {
        amount: deposit_amount,
        recipient: None,
    };
    wasm.execute(
        &vault_addr,
        &deposit_msg,
        &[Coin {
            amount: deposit_amount,
            denom: base_token.to_string(),
        }],
        user1,
    )
    .unwrap();

    // Query vault state
    let state = query_vault_state(&runner, &vault_addr);
    let total_staked_amount = state.total_staked_base_tokens;
    let total_staked_amount_diff_after_compounding_reward1 =
        total_staked_amount - total_staked_amount_before_compound_deposit;
    // Should have increased more than the deposit due to the compounded rewards
    assert!(total_staked_amount_diff_after_compounding_reward1 > deposit_amount);

    // Query treasury reward token balance
    let treasury_reward_token_balance_after =
        query_token_balance(&runner, &treasury.address(), &reward_token_denoms[0]);
    assert_eq!(
        treasury_reward_token_balance_after,
        treasury_reward_token_balance_before + reward_amount * PERFORMANCE_FEE
    );

    // Send base_token to user2 to deposit from second user
    let user2_deposit_amount = Uint128::from(100_000_000u128);
    send_native_coins(
        &runner,
        user1,
        &user2.address(),
        &base_token,
        user2_deposit_amount,
    );

    // Query vault state
    let state_before_user2_deposit = query_vault_state(&runner, &vault_addr);

    // Deposit from user 2
    println!("Deposit from user2");
    let deposit_msg = ExecuteMsg::Deposit {
        amount: user2_deposit_amount,
        recipient: None,
    };
    let _res = wasm
        .execute(
            &vault_addr,
            &deposit_msg,
            &[Coin {
                amount: user2_deposit_amount,
                denom: base_token.to_string(),
            }],
            user2,
        )
        .unwrap();
    let user2_vault_token_balance =
        query_token_balance(&runner, &user2.address(), &vault_token_denom);
    assert_ne!(user2_vault_token_balance, Uint128::zero());
    let user2_base_token_balance = query_token_balance(&runner, &user2.address(), &base_token);
    assert!(user2_base_token_balance.is_zero());

    // Query user 1 vault token balance
    let user1_vault_token_balance =
        query_token_balance(&runner, &user1.address(), &vault_token_denom);
    println!("User1 vault token balance: {}", user1_vault_token_balance);

    // Check that total supply of vault tokens is correct
    let state = query_vault_state(&runner, &vault_addr);
    let vault_token_supply = state.vault_token_supply;
    assert_eq!(
        user1_vault_token_balance + user2_vault_token_balance,
        vault_token_supply
    );

    // Assert that user2's share of the vault was correctly calculated
    println!("User2 vault token balance: {}", user2_vault_token_balance);
    println!("vault token supply: {}", vault_token_supply);
    println!("user2_deposit_amount: {}", user2_deposit_amount);
    println!(
        "total_staked_base_tokens_before_user2_deposit: {}",
        state_before_user2_deposit.total_staked_base_tokens
    );
    let user2_vault_token_share =
        Decimal::from_ratio(user2_vault_token_balance, vault_token_supply);
    let expected_share = Decimal::from_ratio(
        user2_deposit_amount,
        state_before_user2_deposit.total_staked_base_tokens,
    );
    println!("user2_vault_token_share: {}", user2_vault_token_share);
    println!("expected_share: {}", expected_share);
    assert_eq!(user2_vault_token_share, expected_share);

    // If second reward token is set, try donating both reward tokens to
    // contract to simulate rewards accruing
    if let Some(reward2_denom) = reward_token_denoms.get(1) {
        let reward1_denom = &reward_token_denoms[0];
        // Send some reward tokens to vault to simulate reward accruing
        let reward_amount = Uint128::from(100_000_000u128);
        send_native_coins(&runner, admin, &vault_addr, &reward1_denom, reward_amount);
        send_native_coins(&runner, admin, &vault_addr, &reward2_denom, reward_amount);

        // Query treasury reward token balance
        let treasury_reward1_token_balance_before =
            query_token_balance(&runner, &treasury.address(), reward1_denom);
        let treasury_reward2_token_balance_before =
            query_token_balance(&runner, &treasury.address(), reward2_denom);

        // Query vault state
        let state = query_vault_state(&runner, &vault_addr);
        let total_staked_amount_before_compound_deposit = state.total_staked_base_tokens;

        // Deposit some more base token to vault to trigger compounding
        let deposit_amount = Uint128::from(100_000u128);
        user1_total_deposit_amount += deposit_amount;
        let deposit_msg = ExecuteMsg::Deposit {
            amount: deposit_amount,
            recipient: None,
        };
        wasm.execute(
            &vault_addr,
            &deposit_msg,
            &[Coin {
                amount: deposit_amount,
                denom: base_token.to_string(),
            }],
            user1,
        )
        .unwrap();

        // Query vault state
        let state = query_vault_state(&runner, &vault_addr);
        let total_staked_amount = state.total_staked_base_tokens;
        let total_staked_amount_diff =
            total_staked_amount - total_staked_amount_before_compound_deposit;
        // Should have increased more than the deposit due to the compounded rewards
        assert!(total_staked_amount_diff > deposit_amount);
        // Should have increased more than when we just compounded one reward token
        assert!(total_staked_amount_diff > total_staked_amount_diff_after_compounding_reward1);

        // Query treasury reward token balance
        let treasury_reward1_token_balance_after =
            query_token_balance(&runner, &treasury.address(), &reward1_denom);
        assert_eq!(
            treasury_reward1_token_balance_after,
            treasury_reward1_token_balance_before + reward_amount * PERFORMANCE_FEE
        );
        let treasury_reward2_token_balance_after =
            query_token_balance(&runner, &treasury.address(), &reward2_denom);
        assert_eq!(
            treasury_reward2_token_balance_after,
            treasury_reward2_token_balance_before + reward_amount * PERFORMANCE_FEE
        );
    }

    // Query user 1 vault token balance
    let user1_vault_token_balance =
        query_token_balance(&runner, &user1.address(), &vault_token_denom);

    // Query how many base tokens user 1's vault tokens represents
    let msg = QueryMsg::ConvertToAssets {
        amount: user1_vault_token_balance,
    };
    let user1_base_token_balance_in_vault: Uint128 = wasm.query(&vault_addr, &msg).unwrap();
    // Assert that user 1's vault tokens represents more than the amount they
    // deposited (due to compounding)
    assert!(user1_base_token_balance_in_vault > user1_total_deposit_amount);

    // Begin Unlocking all user 1 vault tokens
    println!("Begin Unlocking all user 1 vault tokens");
    let user1_withdraw_amount = user1_vault_token_balance;
    let state = query_vault_state(&runner, &vault_addr);
    let vault_token_supply_before_withdraw = state.vault_token_supply;
    let withdraw_msg =
        ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(LockupExecuteMsg::Unlock {
            amount: user1_withdraw_amount,
        }));
    let _res = wasm
        .execute(
            &vault_addr,
            &withdraw_msg,
            &[Coin {
                amount: user1_withdraw_amount,
                denom: vault_token_denom.clone(),
            }],
            user1,
        )
        .unwrap();

    // Query user 1 unlocking position
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: user1.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.len() == 1);
    let position = unlocking_positions[0].clone();

    // Withdraw unlocked
    println!("Withdrawing unlocked, should fail");
    let withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
        LockupExecuteMsg::WithdrawUnlocked {
            lockup_id: position.id,
            recipient: None,
        },
    ));
    let res = wasm
        .execute(&vault_addr, &withdraw_msg, &[], user1)
        .unwrap_err(); // Should error because not unlocked yet
    println!("Expected error: {}", res);

    println!("Increasing blockchain time by 1 day");
    runner.increase_time(86400);

    // Query user 1 base token balance
    let base_token_balance_before = query_token_balance(&runner, &user1.address(), &base_token);
    println!(
        "User1 base token balance before: {}",
        base_token_balance_before
    );

    // Withdraw unlocked
    println!("Withdrawing unlocked");
    let _res = wasm
        .execute(&vault_addr, &withdraw_msg, &[], user1)
        .unwrap();

    // Query user 1 base token balance
    let base_token_balance_after = query_token_balance(&runner, &user1.address(), &base_token);
    println!(
        "User1 base token balance after withdrawal: {}",
        base_token_balance_after
    );
    assert!(base_token_balance_after > base_token_balance_before);
    let base_token_balance_increase = base_token_balance_after - base_token_balance_before;
    // Assert that all the base tokens were withdrawn
    assert_eq!(
        base_token_balance_increase,
        user1_base_token_balance_in_vault
    );

    // Query vault token supply
    let vault_token_supply: Uint128 = wasm
        .query(&vault_addr, &QueryMsg::TotalVaultTokenSupply {})
        .unwrap();
    println!("Vault token supply: {}", vault_token_supply);
    assert_eq!(
        vault_token_supply_before_withdraw - vault_token_supply,
        user1_withdraw_amount
    );

    // Try force redeem from non-admin wallet
    println!("Force redeem, should fail as sender not whitelisted in contract");
    let force_withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceRedeem {
            amount: Uint128::from(1000000u128),
            recipient: None,
        },
    ));
    let res = wasm
        .execute(
            &vault_addr,
            &force_withdraw_msg,
            &[Coin::new(1000000, &vault_token_denom)],
            user2,
        )
        .unwrap_err(); // Should error because not unlocked yet
    assert!(res.to_string().contains("Unauthorized"));

    // Send 3M vault tokens to force_withdraw_admin
    send_native_coins(
        &runner,
        &user2,
        &force_withdraw_admin.address(),
        &vault_token_denom,
        "3000000000000",
    );

    // Query vault token and base_token balance of force_withdraw_admin
    let fwa_vt_balance_before =
        query_token_balance(&runner, &force_withdraw_admin.address(), &vault_token_denom);
    println!("Force withdraw admin vt balance: {}", fwa_vt_balance_before);
    let fwa_bt_balance_before =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    println!("Force withdraw admin bt balance: {}", fwa_bt_balance_before);

    let force_redeem_amount = Uint128::from(1000000u128);

    // Try force redeem from admin wallet, but not whitelisted at Osmosis yet
    println!("Force redeem, should fail as contract not whitelisted in Osmosis");
    let res = wasm
        .execute(
            &vault_addr,
            &force_withdraw_msg,
            &[Coin::new(force_redeem_amount.u128(), &vault_token_denom)],
            force_withdraw_admin,
        )
        .unwrap_err(); // Should error because not unlocked yet
    assert!(res.to_string().contains("not allowed to force unlock"));

    // Whitelist force_withdraw_admin in Osmosis
    println!("Whitelisting contract for force withdrawals in Osmosis");
    runner.whitelist_address_for_force_unlock(&vault_addr);

    // Try force redeem from admin wallet, should work now
    println!("Force redeem, should work now");
    let _res = wasm
        .execute(
            &vault_addr,
            &force_withdraw_msg,
            &[Coin::new(force_redeem_amount.u128(), &vault_token_denom)],
            force_withdraw_admin,
        )
        .unwrap();

    // Query vault token and base_token balance of force_withdraw_admin
    let fwa_vt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &vault_token_denom);
    println!("Force withdraw admin vt balance: {}", fwa_vt_balance_after);
    let fwa_bt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    println!("Force withdraw admin bt balance: {}", fwa_bt_balance_after);

    // Check difference in balances
    assert_eq!(
        fwa_vt_balance_before - fwa_vt_balance_after, // Vault tokens should be burned
        force_redeem_amount
    );
    // Base tokens should be received, i.e. increase in balance should not be zero
    assert_ne!(
        fwa_bt_balance_after - fwa_bt_balance_before,
        Uint128::zero()
    );

    // Try force withdraw unlocking with old id, should fail
    println!("Force withdraw unlocking, should fail as no position already withdrawn");
    let force_withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            amount: None,
            recipient: None,
            lockup_id: 0,
        },
    ));
    let _res = wasm
        .execute(&vault_addr, &force_withdraw_msg, &[], force_withdraw_admin)
        .unwrap_err(); // Should error because lock already claimed

    // Define amount to unlock in vault tokens and convert to base tokens
    let unlock_amount = Uint128::from(2000000000000u128);
    let msg = QueryMsg::ConvertToAssets {
        amount: unlock_amount,
    };
    let unlock_amount_base_tokens: Uint128 = wasm.query(&vault_addr, &msg).unwrap();
    println!(
        "Unlock amount in base tokens: {}",
        unlock_amount_base_tokens
    );
    // Force withdraw unlocking amounts are denominated in base tokens.
    // We will first try to withdraw half, but can't just divide by 2 and use this
    // amount for both force withdraws, because the amount is rounded down, and if
    // the amount is uneven then the two amounts will not sum to the full amount.
    let first_unlock_amount_base_tokens = unlock_amount_base_tokens / Uint128::from(2u128);
    let second_unlock_amount_base_tokens =
        unlock_amount_base_tokens - first_unlock_amount_base_tokens;

    // Initiate unlocking from force withdraw admin
    println!("Initiate unlocking from force withdraw admin");
    let unlock_msg =
        ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(LockupExecuteMsg::Unlock {
            amount: unlock_amount,
        }));
    let _res = wasm
        .execute(
            &vault_addr,
            &unlock_msg,
            &[Coin {
                amount: unlock_amount,
                denom: vault_token_denom.clone(),
            }],
            force_withdraw_admin,
        )
        .unwrap();

    // Query unlocking positions
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.len() == 1);
    let position = unlocking_positions[0].clone();
    assert_eq!(position.base_token_amount, unlock_amount_base_tokens);
    let lock_id = position.id;

    // Try force withdraw unlocking from non-admin wallet, should fail
    println!("Force withdraw unlocking, should fail as not admin");
    let force_withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            amount: Some(first_unlock_amount_base_tokens),
            recipient: None,
            lockup_id: lock_id,
        },
    ));
    let res = wasm
        .execute(&vault_addr, &force_withdraw_msg, &[], user2)
        .unwrap_err(); // Should error because not admin
    assert!(res.to_string().contains("Unauthorized"));

    // Try force withdraw unlocking from whitelisted admin wallet, should work
    println!("Force withdraw unlocking, should work as admin");
    let _res = wasm
        .execute(&vault_addr, &force_withdraw_msg, &[], force_withdraw_admin)
        .unwrap();

    // Check force withdraw admin balance
    let fwa_bt_balance_before = fwa_bt_balance_after; // Old balance
    let fwa_bt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    println!("Force withdraw admin bt balance: {}", fwa_bt_balance_after);
    assert_eq!(
        fwa_bt_balance_after - fwa_bt_balance_before, // Base token balance should have increased by requested amount
        first_unlock_amount_base_tokens
    );

    // Query unlocking position again, should have updated
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.len() == 1);
    let position = unlocking_positions[0].clone();
    // Amount left in unlocking position should be original minus what we withdrew
    assert_eq!(
        position.base_token_amount,
        unlock_amount_base_tokens - first_unlock_amount_base_tokens
    );

    // Try force withdraw unlocking remaining amount, should work
    println!("Force withdraw unlocking remaining amount, should work");
    let force_withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            amount: None,
            recipient: None,
            lockup_id: lock_id,
        },
    ));
    let _res = wasm
        .execute(&vault_addr, &force_withdraw_msg, &[], force_withdraw_admin)
        .unwrap();

    // Check force withdraw admin balance
    let fwa_bt_balance_before = fwa_bt_balance_after; // Old balance
    let fwa_bt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    println!("Force withdraw admin bt balance: {}", fwa_bt_balance_after);
    assert_eq!(
        fwa_bt_balance_after - fwa_bt_balance_before,
        second_unlock_amount_base_tokens
    );

    // Query unlocking position again, should be empty
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.len() == 0);

    // Set unlock amount
    let unlock_amount = Uint128::from(999999000000u128);
    let msg = QueryMsg::ConvertToAssets {
        amount: unlock_amount,
    };
    let unlock_amount_base_tokens: Uint128 = wasm.query(&vault_addr, &msg).unwrap();
    println!(
        "Unlock amount in base tokens: {}",
        unlock_amount_base_tokens
    );

    // Create new unlocking position
    println!("Initiate unlocking from force withdraw admin");
    let unlock_msg =
        ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(LockupExecuteMsg::Unlock {
            amount: unlock_amount,
        }));
    let _res = wasm
        .execute(
            &vault_addr,
            &unlock_msg,
            &[Coin {
                amount: unlock_amount,
                denom: vault_token_denom.clone(),
            }],
            force_withdraw_admin,
        )
        .unwrap();

    // Query unlocking positions
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.len() == 1);
    let position = unlocking_positions[0].clone();
    assert_eq!(position.base_token_amount, unlock_amount_base_tokens);
    let lock_id = position.id;

    // Increment block time to mature unlocking position
    println!("Incrementing block time to mature unlocking position");
    runner.increase_time(TWO_WEEKS_IN_SECS);

    // Try force withdraw unlocking from non-admin wallet, should fail
    println!("Force withdraw unlocking, should fail as not admin");
    let force_withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            amount: None,
            recipient: None,
            lockup_id: lock_id,
        },
    ));
    wasm.execute(&vault_addr, &force_withdraw_msg, &[], user2)
        .unwrap_err(); // Should error because not admin

    // Try force withdraw unlocking from whitelisted admin wallet, should work
    println!("Force withdraw unlocking, should work as admin");
    let _res = wasm
        .execute(&vault_addr, &force_withdraw_msg, &[], force_withdraw_admin)
        .unwrap();

    // Check force withdraw admin balance
    let fwa_bt_balance_before = fwa_bt_balance_after; // Old balance
    let fwa_bt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    println!("Force withdraw admin bt balance: {}", fwa_bt_balance_after);
    assert_eq!(
        fwa_bt_balance_after - fwa_bt_balance_before,
        position.base_token_amount
    );

    println!("=========== Test multiple simultaneous unlocking positions... ===========");
    println!("Deposit into vault again...");
    // Deposit into vault
    let deposit_amount = Uint128::from(1000000u128);
    let deposit_msg = ExecuteMsg::Deposit {
        amount: deposit_amount,
        recipient: None,
    };
    wasm.execute(
        &vault_addr,
        &deposit_msg,
        &[Coin {
            amount: deposit_amount,
            denom: base_token.to_string(),
        }],
        force_withdraw_admin,
    )
    .unwrap();

    // Query force withdraw admin balance before unlocking
    let fwa_bt_balance_before =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);

    // Query vault state
    let state = query_vault_state(&runner, &vault_addr);
    println!("Vault state: {:?}", state);

    // Unlock 4M vault tokens multiple times from force_withdraw_admin
    let num_unlocking_positions = 4;
    let unlock_amount = Uint128::from(4000000u128);
    let msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(LockupExecuteMsg::Unlock {
        amount: unlock_amount,
    }));
    for i in 0..num_unlocking_positions {
        println!("Unlocking position nr {i}");
        wasm.execute(
            &vault_addr,
            &msg,
            &[Coin::new(unlock_amount.u128(), &vault_token_denom)],
            force_withdraw_admin,
        )
        .unwrap();
    }

    // Query unlocking positions for force_withdraw_admin
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);

    assert!(unlocking_positions.len() == num_unlocking_positions);
    assert!(
        // Assert that all position ids are unique
        unlocking_positions
            .iter()
            .map(|p| p.id)
            .collect::<HashSet<_>>()
            .len()
            == num_unlocking_positions
    );

    // Force unlock first position
    println!("Force unlock first position");
    let first_position = unlocking_positions[0].clone();
    let force_unlock_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            lockup_id: first_position.id,
            amount: None,
            recipient: None,
        },
    ));
    wasm.execute(&vault_addr, &force_unlock_msg, &[], force_withdraw_admin)
        .unwrap();

    // Force unlock second position
    println!("Force unlock second position");
    let second_position = unlocking_positions[1].clone();
    let force_unlock_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            lockup_id: second_position.id,
            amount: None,
            recipient: None,
        },
    ));
    wasm.execute(&vault_addr, &force_unlock_msg, &[], force_withdraw_admin)
        .unwrap();

    // Increment chain time
    runner.increase_time(TWO_WEEKS_IN_SECS);

    // Force unlock third position
    println!("Force unlock third position");
    let third_position = unlocking_positions[2].clone();
    let force_unlock_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            lockup_id: third_position.id,
            amount: None,
            recipient: None,
        },
    ));
    wasm.execute(&vault_addr, &force_unlock_msg, &[], force_withdraw_admin)
        .unwrap();

    // Assert the force withdraw admin balance
    let fwa_bt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    assert_eq!(
        // Should have increased by the sum of the three positions
        fwa_bt_balance_after - fwa_bt_balance_before,
        first_position.base_token_amount
            + second_position.base_token_amount
            + third_position.base_token_amount
    );

    // Withdraw fourth position from force_withdraw_admin
    let fourth_position = unlocking_positions[3].clone();
    let withdraw_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
        LockupExecuteMsg::WithdrawUnlocked {
            lockup_id: fourth_position.id,
            recipient: None,
        },
    ));
    wasm.execute(&vault_addr, &withdraw_msg, &[], force_withdraw_admin)
        .unwrap();

    println!(
        "=========== Test force unlock entire unlocking position using Some(amount) ==========="
    );
    let fwa_bt_balance_before =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    // Unlock from force_withdraw_admin
    let unlock_amount = Uint128::from(4204206969u128);
    let msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(LockupExecuteMsg::Unlock {
        amount: unlock_amount,
    }));
    wasm.execute(
        &vault_addr,
        &msg,
        &[Coin::new(unlock_amount.u128(), &vault_token_denom)],
        force_withdraw_admin,
    )
    .unwrap();

    // Query unlocking positions for force_withdraw_admin
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.len() == 1);

    // Force unlock entire position
    println!("Force unlock entire position");
    let position = unlocking_positions[0].clone();
    let force_unlock_msg = ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
        ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
            lockup_id: position.id,
            amount: Some(position.base_token_amount),
            recipient: None,
        },
    ));
    wasm.execute(&vault_addr, &force_unlock_msg, &[], force_withdraw_admin)
        .unwrap();

    // Assert the force withdraw admin balance
    let fwa_bt_balance_after =
        query_token_balance(&runner, &force_withdraw_admin.address(), &base_token);
    assert_eq!(
        fwa_bt_balance_after - fwa_bt_balance_before,
        position.base_token_amount
    );

    // Query unlocking positions for force_withdraw_admin
    // Assert that the position has been removed from the contract storage
    let unlocking_positions: Vec<UnlockingPosition> = wasm
        .query(
            &vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                LockupQueryMsg::UnlockingPositions {
                    owner: force_withdraw_admin.address().clone(),
                    limit: None,
                    start_after: None,
                },
            )),
        )
        .unwrap();
    println!("Unlocking positions: {:?}", unlocking_positions);
    assert!(unlocking_positions.is_empty());
}

const TWO_WEEKS_IN_SECS: u64 = 60 * 60 * 24 * 14;

fn query_vault_state<'a, R>(
    runner: &'a R,
    vault_addr: &str,
) -> StateResponse<OsmosisStaking, OsmosisPool, OsmosisDenom>
where
    R: Runner<'a>,
{
    let wasm = Wasm::new(runner);
    let state: StateResponse<OsmosisStaking, OsmosisPool, OsmosisDenom> = wasm
        .query(
            vault_addr,
            &QueryMsg::VaultExtension(ExtensionQueryMsg::Apollo(ApolloExtensionQueryMsg::State {})),
        )
        .unwrap();
    state
}

fn query_token_balance<'a, R>(runner: &'a R, address: &str, denom: &str) -> Uint128
where
    R: Runner<'a>,
{
    let bank = Bank::new(runner);
    let balance = bank
        .query_balance(&QueryBalanceRequest {
            address: address.to_string(),
            denom: denom.to_string(),
        })
        .unwrap()
        .balance
        .unwrap_or_default()
        .amount;
    Uint128::from_str(&balance).unwrap()
}

fn send_native_coins<'a, R>(
    runner: &'a R,
    from: &SigningAccount,
    to: &str,
    denom: &str,
    amount: impl Into<String>,
) where
    R: Runner<'a>,
{
    let bank = Bank::new(runner);
    bank.send(
        MsgSend {
            amount: vec![ProtoCoin {
                denom: denom.to_string(),
                amount: amount.into(),
            }],
            from_address: from.address(),
            to_address: to.to_string(),
        },
        from,
    )
    .unwrap();
}
