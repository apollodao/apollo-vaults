use cw_dex::astroport::{AstroportPool, AstroportStaking};

use apollo_vault::msg::{
    ApolloExtensionExecuteMsg, ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg,
    StateResponse,
};
use apollo_vault::state::{ConfigUnchecked, ConfigUpdates};
use cosmwasm_std::{Coin, Empty, Uint128};
use cw_it::robot::TestRobot;
use cw_it::test_tube::{Account, Runner, SigningAccount};
use cw_utils::Duration;
use cw_vault_standard::extensions::force_unlock::ForceUnlockExecuteMsg;
use cw_vault_standard::extensions::lockup::{LockupExecuteMsg, LockupQueryMsg, UnlockingPosition};
use cw_vault_standard::msg::{
    VaultStandardExecuteMsg as ExecuteMsg, VaultStandardQueryMsg as QueryMsg,
};
use cw_vault_standard::VaultInfoResponse;
use cw_vault_token::osmosis::OsmosisDenom;

pub trait CwVaultStandardRobot<'a, R: Runner<'a> + 'a>: TestRobot<'a, R> {
    fn vault_addr(&self) -> String;

    fn query_vault_state(&self) -> StateResponse<AstroportStaking, AstroportPool, OsmosisDenom> {
        self.wasm()
            .query(
                &self.vault_addr(),
                &QueryMsg::VaultExtension(ExtensionQueryMsg::Apollo(
                    ApolloExtensionQueryMsg::State {},
                )),
            )
            .unwrap()
    }

    fn query_info(&self) -> VaultInfoResponse {
        self.wasm()
            .query(&self.vault_addr(), &QueryMsg::<Empty>::Info {})
            .unwrap()
    }

    fn deposit(
        &self,
        signer: &SigningAccount,
        amount: impl Into<Uint128>,
        recipient: Option<String>,
        funds: &[Coin],
    ) -> &Self {
        let amount: Uint128 = amount.into();
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::<Empty>::Deposit { amount, recipient },
                funds,
                signer,
            )
            .unwrap();
        self
    }

    fn deposit_all(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let base_token_denom = self.query_info().base_token;
        let amount = self.query_native_token_balance(&signer.address(), &base_token_denom);

        self.deposit(
            signer,
            amount,
            recipient,
            &[Coin::new(amount.u128(), base_token_denom)],
        )
    }

    fn assert_base_token_balance_eq(
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

    fn redeem(&self, signer: &SigningAccount, amount: Uint128, recipient: Option<String>) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::<Empty>::Redeem { amount, recipient },
                &[],
                signer,
            )
            .unwrap();
        self
    }

    fn redeem_all(&self, signer: &SigningAccount, recipient: Option<String>) -> &Self {
        let amount =
            self.query_native_token_balance(signer.address(), &self.query_info().vault_token);
        self.redeem(signer, amount, recipient)
    }

    fn update_config(&self, signer: &SigningAccount, updates: ConfigUpdates) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::UpdateConfig { updates },
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    fn assert_config(&self, config_unchecked: ConfigUnchecked) -> &Self {
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

    fn update_admin(&self, signer: &SigningAccount, new_admin: impl Into<String>) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
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

    fn accept_admin_transfer(&self, signer: &SigningAccount) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Apollo(
                    ApolloExtensionExecuteMsg::AcceptAdminTransfer {},
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    fn assert_admin(&self, expected: impl Into<String>) -> &Self {
        let admin = self.query_vault_state().admin;
        assert_eq!(admin.unwrap().to_string(), expected.into());

        self
    }

    fn drop_admin_transfer(&self, signer: &SigningAccount) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
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

pub trait LockedVaultRobot<'a, R: Runner<'a> + 'a>: CwVaultStandardRobot<'a, R> {
    fn unlock_with_funds(
        &self,
        amount: impl Into<Uint128>,
        signer: &SigningAccount,
        funds: &[Coin],
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::Lockup(
                    LockupExecuteMsg::Unlock {
                        amount: amount.into(),
                    },
                )),
                funds,
                signer,
            )
            .unwrap();
        self
    }

    fn unlock(&self, amount: impl Into<Uint128>, signer: &SigningAccount) -> &Self {
        let info = self.query_info();
        let amount: Uint128 = amount.into();
        self.unlock_with_funds(
            amount.clone(),
            signer,
            &[Coin {
                amount,
                denom: info.vault_token,
            }],
        )
    }

    fn withdraw_unlocked(
        &self,
        lockup_id: u64,
        recipient: Option<String>,
        signer: &SigningAccount,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
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

    fn query_unlocking_positions(
        &self,
        address: impl Into<String>,
        start_after: Option<u64>,
        limit: Option<u32>,
    ) -> Vec<UnlockingPosition> {
        self.wasm()
            .query(
                &self.vault_addr(),
                &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                    LockupQueryMsg::UnlockingPositions {
                        owner: address.into(),
                        start_after,
                        limit,
                    },
                )),
            )
            .unwrap()
    }

    fn query_unlocking_position(&self, lockup_id: u64) -> UnlockingPosition {
        self.wasm()
            .query(
                &self.vault_addr(),
                &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                    LockupQueryMsg::UnlockingPosition { lockup_id },
                )),
            )
            .unwrap()
    }

    fn query_lockup_duration(&self) -> Duration {
        self.wasm()
            .query(
                &self.vault_addr(),
                &QueryMsg::VaultExtension(ExtensionQueryMsg::Lockup(
                    LockupQueryMsg::LockupDuration {},
                )),
            )
            .unwrap()
    }

    fn assert_number_of_unlocking_positions(
        &self,
        address: impl Into<String>,
        expected: usize,
    ) -> &Self {
        let positions = self.query_unlocking_positions(address, None, None);
        assert_eq!(positions.len(), expected);

        self
    }
}

pub trait ForceUnlockVaultRobot<'a, R: Runner<'a> + 'a>: LockedVaultRobot<'a, R> {
    fn force_redeem(
        &self,
        amount: impl Into<Uint128>,
        recipient: Option<String>,
        signer: &SigningAccount,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
                    ForceUnlockExecuteMsg::ForceRedeem {
                        recipient,
                        amount: amount.into(),
                    },
                )),
                &[],
                signer,
            )
            .unwrap();
        self
    }

    fn force_withdraw_unlocking(
        &self,
        lockup_id: u64,
        amount: Option<impl Into<Uint128>>,
        recipient: Option<String>,
        signer: &SigningAccount,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
                &ExecuteMsg::VaultExtension(ExtensionExecuteMsg::ForceUnlock(
                    ForceUnlockExecuteMsg::ForceWithdrawUnlocking {
                        amount: amount.map(Into::into),
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

    fn update_force_withdraw_whitelist(
        &self,
        signer: &SigningAccount,
        add_addresses: Vec<String>,
        remove_addresses: Vec<String>,
    ) -> &Self {
        self.wasm()
            .execute(
                &self.vault_addr(),
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
}
