use cosmwasm_std::{
    testing::{MockApi, MockStorage},
    Deps, Empty, Querier, QuerierWrapper,
};

pub struct RunnerMockDeps<'a, Q: Querier> {
    pub storage: MockStorage,
    pub api: MockApi,
    pub querier: &'a Q,
}

impl<'a, Q: Querier> RunnerMockDeps<'a, Q> {
    pub fn new(querier: &'a Q) -> Self {
        Self {
            storage: MockStorage::default(),
            api: MockApi::default(),
            querier,
        }
    }
    pub fn as_ref(&'_ self) -> Deps<'_, Empty> {
        Deps {
            storage: &self.storage,
            api: &self.api,
            querier: QuerierWrapper::new(self.querier),
        }
    }
}
