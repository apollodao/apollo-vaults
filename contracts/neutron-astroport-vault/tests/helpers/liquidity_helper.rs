use cosmwasm_std::Addr;
use cw_it::{
    cw_multi_test::ContractWrapper,
    test_tube::{Account, Module, SigningAccount, Wasm},
    traits::CwItRunner,
    ContractType, TestRunner,
};
use liquidity_helper::LiquidityHelper;

pub struct AstroportLiquidityHelperRobot<'a> {
    runner: &'a TestRunner<'a>,
    pub liquidity_helper: LiquidityHelper,
}

impl<'a> AstroportLiquidityHelperRobot<'a> {
    pub fn new(
        runner: &'a TestRunner<'a>,
        astroport_factory: String,
        signer: &SigningAccount,
    ) -> Self {
        let contract = match runner {
            TestRunner::MultiTest(_) => {
                ContractType::MultiTestContract(Box::new(ContractWrapper::new(
                    astroport_liquidity_helper::contract::execute,
                    astroport_liquidity_helper::contract::instantiate,
                    astroport_liquidity_helper::contract::query,
                )))
            }
            _ => panic!("Unsupported runner"),
        };

        let code_id = runner.store_code(contract, signer).unwrap();

        let wasm = Wasm::new(runner);
        let addr = wasm
            .instantiate(
                code_id,
                &astroport_liquidity_helper::msg::InstantiateMsg { astroport_factory },
                Some(&signer.address()),
                Some("astroport_liquidity_helper"),
                &[],
                signer,
            )
            .unwrap()
            .data
            .address;

        let liquidity_helper = LiquidityHelper::new(Addr::unchecked(addr));

        Self {
            runner,
            liquidity_helper,
        }
    }
}
