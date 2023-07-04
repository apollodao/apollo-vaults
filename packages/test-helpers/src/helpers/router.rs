use apollo_cw_asset::AssetInfoUnchecked;
use cosmwasm_std::Addr;
use cw_dex_router::{
    helpers::CwDexRouter, msg::InstantiateMsg, operations::SwapOperationsListUnchecked,
};
use cw_it::{
    cw_multi_test::ContractWrapper,
    test_tube::{Account, Module, SigningAccount, Wasm},
    traits::CwItRunner,
    ContractType, TestRunner,
};

pub struct CwDexRouterRobot<'a> {
    pub runner: &'a TestRunner<'a>,
    pub cw_dex_router: CwDexRouter,
}

impl<'a> CwDexRouterRobot<'a> {
    pub fn new(runner: &'a TestRunner<'a>, signer: &SigningAccount) -> Self {
        let contract = match runner {
            TestRunner::MultiTest(_) => {
                ContractType::MultiTestContract(Box::new(ContractWrapper::new(
                    cw_dex_router::contract::execute,
                    cw_dex_router::contract::instantiate,
                    cw_dex_router::contract::query,
                )))
            }
            _ => panic!("Unsupported runner"),
        };

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
        from: &str,
        to: &str,
        path: SwapOperationsListUnchecked,
        bidirectional: bool,
        signer: &SigningAccount,
    ) {
        let wasm = Wasm::new(self.runner);
        wasm.execute(
            &self.cw_dex_router.0.to_string(),
            &cw_dex_router::msg::ExecuteMsg::SetPath {
                offer_asset: AssetInfoUnchecked::Native(from.to_string()),
                ask_asset: AssetInfoUnchecked::Native(to.to_string()),
                path,
                bidirectional,
            },
            &[],
            signer,
        )
        .unwrap();
    }
}
