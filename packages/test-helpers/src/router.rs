use apollo_cw_asset::AssetInfoUnchecked;
use cosmwasm_std::Addr;
use cw_dex_router::helpers::{CwDexRouter, CwDexRouterUnchecked};
use cw_dex_router::msg::InstantiateMsg;
use cw_dex_router::operations::SwapOperationsListUnchecked;
use cw_it::cw_multi_test::ContractWrapper;
use cw_it::robot::TestRobot;
use cw_it::test_tube::{Account, Module, SigningAccount, Wasm};
use cw_it::traits::CwItRunner;
use cw_it::{ContractType, TestRunner};

#[cfg(feature = "osmosis-test-tube")]
use cw_it::Artifact;

pub const CW_DEX_ROUTER_WASM_NAME: &str = "cw_dex_router_osmosis.wasm";

pub struct CwDexRouterRobot<'a> {
    pub runner: &'a TestRunner<'a>,
    pub cw_dex_router: CwDexRouter,
}

impl<'a> CwDexRouterRobot<'a> {
    /// Returns a `ContractType` representing the contract to use for the given
    /// `TestRunner`.
    pub fn contract(runner: &'a TestRunner<'a>, _artifacts_dir: &str) -> ContractType {
        match runner {
            #[cfg(feature = "osmosis-test-tube")]
            TestRunner::OsmosisTestApp(_) => ContractType::Artifact(Artifact::Local(format!(
                "{}/{}",
                _artifacts_dir, CW_DEX_ROUTER_WASM_NAME
            ))),
            TestRunner::MultiTest(_) => {
                ContractType::MultiTestContract(Box::new(ContractWrapper::new_with_empty(
                    cw_dex_router::contract::execute,
                    cw_dex_router::contract::instantiate,
                    cw_dex_router::contract::query,
                )))
            }
            _ => panic!("Unsupported runner"),
        }
    }

    pub fn new(
        runner: &'a TestRunner<'a>,
        contract: ContractType,
        signer: &SigningAccount,
    ) -> Self {
        let code_id = runner.store_code(contract, signer).unwrap();

        let wasm = Wasm::new(runner);
        let router_addr = wasm
            .instantiate(
                code_id,
                &InstantiateMsg {},
                Some(&signer.address()),
                Some("cw_dex_router"),
                &[],
                signer,
            )
            .unwrap()
            .data
            .address;

        let cw_dex_router = CwDexRouter::new(&Addr::unchecked(router_addr));

        Self {
            runner,
            cw_dex_router,
        }
    }

    pub fn set_path(
        &self,
        from: AssetInfoUnchecked,
        to: AssetInfoUnchecked,
        path: SwapOperationsListUnchecked,
        bidirectional: bool,
        signer: &SigningAccount,
    ) {
        self.wasm()
            .execute(
                self.cw_dex_router.0.as_ref(),
                &cw_dex_router::msg::ExecuteMsg::SetPath {
                    offer_asset: from,
                    ask_asset: to,
                    path,
                    bidirectional,
                },
                &[],
                signer,
            )
            .unwrap();
    }
}

impl<'a> From<CwDexRouterRobot<'a>> for CwDexRouter {
    fn from(value: CwDexRouterRobot) -> Self {
        value.cw_dex_router
    }
}

impl<'a> From<CwDexRouterRobot<'a>> for CwDexRouterUnchecked {
    fn from(value: CwDexRouterRobot) -> Self {
        value.cw_dex_router.into()
    }
}

impl<'a> TestRobot<'a, TestRunner<'a>> for CwDexRouterRobot<'a> {
    fn runner(&self) -> &'a TestRunner<'a> {
        self.runner
    }
}
