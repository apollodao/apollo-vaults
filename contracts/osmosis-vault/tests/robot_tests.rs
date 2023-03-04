use apollo_vault::msg::{
    ApolloExtensionQueryMsg, ExtensionExecuteMsg, ExtensionQueryMsg, StateResponse,
};
use cosmwasm_std::{Coin, Uint128};
use cw_dex::osmosis::{OsmosisPool, OsmosisStaking};
use cw_it::{
    osmosis::robot::OsmosisTestRobot,
    osmosis_test_tube::{OsmosisTestApp, Runner, SigningAccount},
    robot::TestRobot,
};
use cw_vault_standard::{extensions::lockup::LockupExecuteMsg, VaultInfoResponse};
use cw_vault_token::osmosis::OsmosisDenom;
use osmosis_vault::msg::{ExecuteMsg, QueryMsg};

pub struct OsmosisVaultRobot<'a, R: Runner<'a>> {
    pub app: &'a R,
    pub vault_addr: String,
}

impl<'a, R: Runner<'a>> TestRobot<'a, R> for OsmosisVaultRobot<'a, R> {
    fn app(&self) -> &'a R {
        self.app
    }
}

impl<'a> OsmosisTestRobot<'a> for OsmosisVaultRobot<'a, OsmosisTestApp> {}

impl<'a> OsmosisVaultRobot<'a, OsmosisTestApp> {
    pub fn new(app: &'a OsmosisTestApp, vault_addr: String) -> Self {
        Self { app, vault_addr }
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
}
