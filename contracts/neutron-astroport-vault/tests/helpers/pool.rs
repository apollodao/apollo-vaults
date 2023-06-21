use cosmwasm_std::{Binary, Coin};
use cw_it::{
    astroport::{
        astroport::{asset::AssetInfo, factory::PairType},
        robot::AstroportTestRobot,
    },
    test_tube::SigningAccount,
    TestRunner,
};

use super::vault::NeutronAstroportVaultRobot;

pub struct AstroportTestPool {
    pub pair_type: PairType,
    pub liquidity: [Coin; 2],
    pub init_params: Option<Binary>,
}

impl<'a> AstroportTestPool {
    pub fn create(
        &self,
        robot: &NeutronAstroportVaultRobot<'a, TestRunner<'a>>,
        signer: &SigningAccount,
    ) -> (String, String) {
        robot.create_astroport_pair(
            self.pair_type,
            [
                AssetInfo::NativeToken {
                    denom: self.liquidity[0].denom.clone(),
                },
                AssetInfo::NativeToken {
                    denom: self.liquidity[1].denom.clone(),
                },
            ],
            None,
            signer,
            Some([self.liquidity[0].amount, self.liquidity[1].amount]),
        )
    }
}
