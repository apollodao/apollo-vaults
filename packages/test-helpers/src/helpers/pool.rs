use cosmwasm_std::{Binary, Coin};
use cw_it::{
    astroport::{
        astroport::{asset::AssetInfo, factory::PairType},
        robot::AstroportTestRobot,
    },
    test_tube::SigningAccount,
    traits::CwItRunner,
};

#[derive(Clone)]
pub struct AstroportTestPool {
    pub pair_type: PairType,
    pub liquidity: [Coin; 2],
    pub init_params: Option<Binary>,
}

impl AstroportTestPool {
    pub fn create<'a, T: AstroportTestRobot<'a, R>, R: CwItRunner<'a> + 'a>(
        &self,
        robot: &'a T,
        signer: &SigningAccount,
    ) -> (String, String) {
        robot.create_astroport_pair(
            self.pair_type.clone(),
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
