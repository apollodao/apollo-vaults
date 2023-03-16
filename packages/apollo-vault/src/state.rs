use apollo_cw_asset::{AssetInfo, AssetInfoBase};
use cosmwasm_schema::cw_serde;
use cosmwasm_std::{
    Addr, BlockInfo, Decimal, Deps, MessageInfo, Order, StdError, StdResult, Storage, Uint128,
};
use cw20::Expiration;
use cw_dex_router::helpers::CwDexRouterBase;
use cw_storage_plus::{Bound, Index, IndexList, IndexedMap, Item, MultiIndex};
use cw_vault_standard::extensions::lockup::UnlockingPosition;
use liquidity_helper::LiquidityHelperBase;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

//--------------------------------------------------------------------------------------------------
// Config
//--------------------------------------------------------------------------------------------------

/// Base config struct for the contract.
#[cw_serde]
#[derive(Builder)]
#[builder(derive(Serialize, Deserialize, Debug, PartialEq, JsonSchema))]
pub struct ConfigBase<T> {
    /// Percentage of profit to be charged as performance fee
    pub performance_fee: Decimal,
    /// Account to receive fee payments
    pub treasury: T,
    /// Router address
    pub router: CwDexRouterBase<T>,
    /// The assets that are given as liquidity mining rewards that the vault
    /// will compound into more of base_token.
    pub reward_assets: Vec<AssetInfoBase<T>>,
    /// The asset to which we should swap reward_assets into before providing
    /// liquidity. Should be one of the assets in the pool.
    pub reward_liquidation_target: AssetInfoBase<T>,
    /// Whitelisted addresses that can call ForceWithdraw and
    /// ForceWithdrawUnlocking
    pub force_withdraw_whitelist: Vec<T>,
    /// Helper for providing liquidity with unbalanced assets.
    pub liquidity_helper: LiquidityHelperBase<T>,
}

/// Config with non-validated addresses.
pub type ConfigUnchecked = ConfigBase<String>;
/// Config with validated addresses.
pub type Config = ConfigBase<Addr>;
/// Config updates struct containing same fields as Config, but all fields are
/// optional.
pub type ConfigUpdates = ConfigBaseBuilder<String>;

/// Merges the old config with a new partial config.
impl Config {
    /// Updates the existing config with the new config updates. If a field is
    /// `None` in the `updates` then the old config is kept, else it is updated
    /// to the new value.
    pub fn update(self, deps: Deps, updates: ConfigUpdates) -> StdResult<Config> {
        ConfigUnchecked {
            performance_fee: updates.performance_fee.unwrap_or(self.performance_fee),
            treasury: updates.treasury.unwrap_or_else(|| self.treasury.into()),
            router: updates.router.unwrap_or_else(|| self.router.into()),
            reward_assets: updates
                .reward_assets
                .unwrap_or_else(|| self.reward_assets.into_iter().map(Into::into).collect()),
            reward_liquidation_target: updates
                .reward_liquidation_target
                .unwrap_or_else(|| self.reward_liquidation_target.into()),
            force_withdraw_whitelist: updates.force_withdraw_whitelist.unwrap_or_else(|| {
                self.force_withdraw_whitelist
                    .into_iter()
                    .map(Into::into)
                    .collect()
            }),
            liquidity_helper: updates
                .liquidity_helper
                .unwrap_or_else(|| self.liquidity_helper.into()),
        }
        .check(deps)
    }
}

impl ConfigUnchecked {
    /// Constructs a Config from the unchecked config, validating all addresses.
    pub fn check(&self, deps: Deps) -> StdResult<Config> {
        if self.performance_fee > Decimal::one() {
            return Err(StdError::generic_err(
                "Performance fee cannot be greater than 100%",
            ));
        }

        let reward_assets: Vec<AssetInfo> = self
            .reward_assets
            .iter()
            .map(|x| x.check(deps.api))
            .collect::<StdResult<_>>()?;
        let router = self.router.check(deps.api)?;
        let reward_liquidation_target = self.reward_liquidation_target.check(deps.api)?;

        // Check that the router can route between all reward assets and the
        // reward liquidation target. We discard the actual path because we
        // don't need it here. We just need to make sure the paths exist.
        for asset in &reward_assets {
            // We skip the reward liquidation target because we don't need to
            // route to it.
            if asset == &reward_liquidation_target {
                continue;
            }
            // We map the error here because the error coming from the router is
            // not passed along into the query error, and thus we will otherwise
            // just see "Querier contract error" and no more information.
            router
                .query_path_for_pair(&deps.querier, asset, &reward_liquidation_target)
                .map_err(|_| {
                    StdError::generic_err(format!(
                        "Could not read path in cw-dex-router for {:?} -> {:?}",
                        asset, reward_liquidation_target
                    ))
                })?;
        }

        Ok(Config {
            performance_fee: self.performance_fee,
            treasury: deps.api.addr_validate(&self.treasury)?,
            reward_assets,
            reward_liquidation_target,
            router,
            force_withdraw_whitelist: self
                .force_withdraw_whitelist
                .iter()
                .map(|x| deps.api.addr_validate(x))
                .collect::<StdResult<_>>()?,
            liquidity_helper: self.liquidity_helper.check(deps.api)?,
        })
    }
}

//--------------------------------------------------------------------------------------------------
// State
//--------------------------------------------------------------------------------------------------

// Settings for pagination
const DEFAULT_LIMIT: u32 = 10;

/// An unlockin position for a user that can be claimed once it has matured.
pub type Claim = UnlockingPosition;
/// A struct for handling the addition and removal of claims, as well as
/// querying and force unlocking of claims.
pub struct Claims<'a> {
    /// All currently unclaimed claims, both unlocking and matured. Once a claim
    /// is claimed by its owner after it has matured, it is removed from this
    /// map.
    claims: IndexedMap<'a, u64, Claim, ClaimIndexes<'a>>,
    /// The pending claim that is currently being created. When the claim is
    /// ready to be saved to the `claims` map [`self.commit_pending_claim()`]
    /// should be called.
    pending_claim: Item<'a, Claim>,
    // Counter of the number of claims. Used as a default value for the ID of a new
    // claim if the underlying staking contract doesn't issue their own IDs. This is monotonically
    // increasing and is not decremented when a claim is removed. It represents the number of
    // claims that have been created since creation of the `Claims` instance.
    next_claim_id: Item<'a, u64>,
}

/// Helper struct for indexing claims. Needed by the [`IndexedMap`]
/// implementation.
pub struct ClaimIndexes<'a> {
    /// Index mapping an address to all claims for that address.
    pub owner: MultiIndex<'a, Addr, Claim, u64>,
}

impl<'a> IndexList<Claim> for ClaimIndexes<'a> {
    fn get_indexes(&'_ self) -> Box<dyn Iterator<Item = &'_ dyn Index<Claim>> + '_> {
        let v: Vec<&dyn Index<Claim>> = vec![&self.owner];
        Box::new(v.into_iter())
    }
}

impl<'a> Claims<'a> {
    /// Create a new Claims instance
    ///
    /// ## Arguments
    /// * `claims_namespace` - The key to use for the the primary key (u64
    ///   lockup ID)
    /// * `num_claims_key` - The key to use for the index value (owner addr)
    pub fn new(
        claims_namespace: &'a str,
        claims_index_namespace: &'a str,
        pending_claims_key: &'a str,
        num_claims_key: &'a str,
    ) -> Self {
        let indexes = ClaimIndexes {
            owner: MultiIndex::new(
                |_pk, d| d.owner.clone(),
                claims_namespace,
                claims_index_namespace,
            ),
        };

        Self {
            claims: IndexedMap::new(claims_namespace, indexes),
            pending_claim: Item::new(pending_claims_key),
            next_claim_id: Item::new(num_claims_key),
        }
    }

    /// Create a pending claim that can be saved to the claims by calling
    /// `save_pending_claim`.
    ///
    /// For Osmosis implementation this ID is changed in the reply handling
    /// since we need to use the same ID as osmosis does (to be able to
    /// force unlock by ID). The pending claim is not saved to the claims
    /// map until `execute_callback_save_claim` is called.
    ///
    /// ## Arguments
    /// * `owner` - The owner of the claim
    /// * `lock_id` - The optional lockup ID. If `None`, the next default ID
    ///   will be used.
    /// * `amount` - The amount the claim represents
    /// * `expiration` - The time or block height at which the claim can be
    ///   released
    pub fn create_pending_claim(
        &self,
        storage: &mut dyn Storage,
        owner: &Addr,
        base_token_amount: Uint128,
        expiration: Expiration,
        lock_id: Option<u64>,
    ) -> StdResult<()> {
        // Set lock_id to number of claims and increment the num_claims counter
        let lock_id =
            lock_id.unwrap_or_else(|| self.next_claim_id.load(storage).unwrap_or_default());

        match self.pending_claim.may_load(storage)? {
            Some(_) => Err(StdError::generic_err("Pending claim already exists")),
            None => {
                let lockup = UnlockingPosition {
                    owner: owner.clone(),
                    id: lock_id,
                    release_at: expiration,
                    base_token_amount,
                };
                self.pending_claim.save(storage, &lockup)
            }
        }
    }

    /// Sets the pending claim. This will overwrite any existing pending claim.
    pub fn set_pending_claim(&self, storage: &mut dyn Storage, claim: &Claim) -> StdResult<()> {
        self.pending_claim.save(storage, claim)
    }

    /// Get the pending claim. Returns an error if there is no pending claim.
    pub fn get_pending_claim(&self, storage: &dyn Storage) -> StdResult<Claim> {
        self.pending_claim.load(storage)
    }

    /// Save the pending claim to the claims map and removes it from the
    /// `pending_claim` item. This should be called after
    /// `create_pending_claim`. The id of the `pending_claim` MUST be unique, or
    /// an error will be returned.
    pub fn commit_pending_claim(&self, storage: &mut dyn Storage) -> StdResult<()> {
        let pending_claim = self.pending_claim.load(storage)?;

        // Set num_claims to the pending_claim.id + 1 if num_claims is greater than or
        // equal to the current num_claims value
        if pending_claim.id >= self.next_claim_id.load(storage).unwrap_or_default() {
            self.next_claim_id.save(storage, &(pending_claim.id + 1))?;
        }

        // Save the pending claim to the claims map if a claim with the same ID does not
        // already exist
        match self.claims.may_load(storage, pending_claim.id)? {
            Some(claim) => Err(StdError::generic_err(format!(
                "Claim with id {} already exists",
                claim.id
            ))),
            None => {
                self.pending_claim.remove(storage);
                self.claims.save(storage, pending_claim.id, &pending_claim)
            }
        }
    }

    /// Redeem claim for the underlying tokens
    ///
    /// ## Arguments
    /// * `lock_id` - The id of the claim
    ///
    /// ## Returns
    /// Returns the amount of tokens redeemed if `info.sender` is the `owner` of
    /// the claim and the `release_at` time has passed, else returns an
    /// error. Also returns an error if a claim with the given `lock_id` does
    /// not exist.
    pub fn claim_tokens(
        &self,
        storage: &mut dyn Storage,
        block: &BlockInfo,
        info: &MessageInfo,
        lock_id: u64,
    ) -> StdResult<Uint128> {
        let claim = self.claims.load(storage, lock_id)?;

        // Ensure the claim is owned by the sender
        if claim.owner != info.sender {
            return Err(StdError::generic_err("Claim not owned by sender"));
        }

        // Check if the claim is expired
        if !claim.release_at.is_expired(block) {
            return Err(StdError::generic_err("Claim has not yet matured."));
        }

        // Remove the claim from the map
        self.claims.remove(storage, lock_id)?;

        Ok(claim.base_token_amount)
    }

    /// Bypass expiration and claim `claim_amount`. Should only be called if the
    /// caller is whitelisted. Will return an error if the claim does not exist
    /// or if the caller is not the owner of the claim.
    /// TODO: Move whitelist logic into Claims struct? That way we won't need to
    /// have a separate ForceUnlock message.
    pub fn force_claim(
        &self,
        storage: &mut dyn Storage,
        info: &MessageInfo,
        lock_id: u64,
        claim_amount: Option<Uint128>,
    ) -> StdResult<Uint128> {
        let mut lockup = self.claims.load(storage, lock_id)?;

        // Ensure the claim is owned by the sender
        if lockup.owner != info.sender {
            return Err(StdError::generic_err("Claim not owned by sender"));
        }

        let claimable_amount = lockup.base_token_amount;

        let claimed = claim_amount.unwrap_or(claimable_amount);

        let left_after_claim = claimable_amount.checked_sub(claimed).map_err(|x| {
            StdError::generic_err(format!(
                "Claim amount is greater than the claimable amount: {}",
                x
            ))
        })?;

        if left_after_claim > Uint128::zero() {
            lockup.base_token_amount = left_after_claim;
            self.claims.save(storage, lock_id, &lockup)?;
        } else {
            self.claims.remove(storage, lock_id)?;
        }

        Ok(claimed)
    }

    // ========== Query functions ==========

    /// Query lockup by id
    pub fn query_claim_by_id(&self, deps: Deps, lockup_id: u64) -> StdResult<UnlockingPosition> {
        self.claims.load(deps.storage, lockup_id)
    }

    /// Reads all claims for an owner. The optional arguments `start_after` and
    /// `limit` can be used for pagination if there are too many claims to
    /// return in one query.
    ///
    /// # Arguments
    /// - `owner` - The owner of the claims
    /// - `start_after` - Optional id of the claim to start the query after
    /// - `limit` - Optional maximum number of claims to return
    pub fn query_claims_for_owner(
        &self,
        deps: Deps,
        owner: &Addr,
        start_after: Option<u64>,
        limit: Option<u32>,
    ) -> StdResult<Vec<(u64, Claim)>> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT) as usize;
        let start: Option<Bound<u64>> = start_after.map(Bound::exclusive);

        self.claims
            .idx
            .owner
            .prefix(owner.clone())
            .range(deps.storage, start, None, Order::Ascending)
            .take(limit)
            .collect::<StdResult<Vec<_>>>()
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::testing::{
        mock_dependencies, mock_env, mock_info, MockApi, MockQuerier, MockStorage,
    };
    use cosmwasm_std::{Addr, OwnedDeps, Uint128};
    use cw_utils::Expiration;

    use test_case::test_case;

    use super::*;

    const OWNER: &str = "owner";
    const NOT_OWNER: &str = "not_owner";

    const CLAIMS: &str = "claims";
    const CLAIMS_INDEX: &str = "claims_index";
    const PENDING_CLAIMS: &str = "pending_claims";
    const NUM_CLAIMS: &str = "num_claims";
    const BASE_TOKEN_AMOUNT: Uint128 = Uint128::new(100);
    const EXPIRATION: Expiration = Expiration::AtHeight(100);

    fn setup_pending_claim(
        lock_id: Option<u64>,
    ) -> (
        OwnedDeps<MockStorage, MockApi, MockQuerier>,
        Claims<'static>,
    ) {
        let mut deps = mock_dependencies();

        let claims = Claims::new(CLAIMS, CLAIMS_INDEX, PENDING_CLAIMS, NUM_CLAIMS);

        // Create pending claim without specifying lock_id
        claims
            .create_pending_claim(
                &mut deps.storage,
                &Addr::unchecked(OWNER),
                BASE_TOKEN_AMOUNT,
                EXPIRATION,
                lock_id,
            )
            .unwrap();

        (deps, claims)
    }

    #[test]
    fn test_create_pending_claim_without_id() {
        let (mut deps, claims) = setup_pending_claim(None);

        // Check that the pending claim was created
        let pending_claim = claims.pending_claim.load(&deps.storage).unwrap();

        // Assert that the pending claim has the correct values
        assert_eq!(
            pending_claim,
            Claim {
                id: 0,
                owner: Addr::unchecked(OWNER),
                base_token_amount: BASE_TOKEN_AMOUNT,
                release_at: EXPIRATION,
            }
        );

        // Check that pending claim is errors when trying to create another pending
        // claim before commiting the current pending claim to storage
        let err = claims
            .create_pending_claim(
                &mut deps.storage,
                &Addr::unchecked(OWNER),
                BASE_TOKEN_AMOUNT,
                EXPIRATION,
                None,
            )
            .unwrap_err();
        assert_eq!(err, StdError::generic_err("Pending claim already exists"));
    }

    #[test]
    fn test_create_pending_claim_with_id() {
        let lock_id = 1;
        let (deps, claims) = setup_pending_claim(Some(lock_id));

        // Get pending claim
        let pending_claim = claims.pending_claim.load(&deps.storage).unwrap();

        // Assert that the pending claim has the correct values
        assert_eq!(
            pending_claim,
            Claim {
                id: lock_id,
                owner: Addr::unchecked(OWNER),
                base_token_amount: BASE_TOKEN_AMOUNT,
                release_at: EXPIRATION,
            }
        );

        // Assert that num_claims was not incremented
        claims.next_claim_id.load(&deps.storage).unwrap_err();
    }

    #[test]
    fn test_set_pending_claim() {
        let (mut deps, claims) = setup_pending_claim(None);

        // Set a new pending claim
        let expiration = Expiration::AtHeight(200);
        let base_token_amount = Uint128::new(200);
        let owner = Addr::unchecked(NOT_OWNER);
        let id = 1;
        claims
            .set_pending_claim(
                &mut deps.storage,
                &Claim {
                    id,
                    owner: owner.clone(),
                    release_at: expiration,
                    base_token_amount,
                },
            )
            .unwrap();

        // Get pending claim
        let pending_claim = claims.pending_claim.load(&deps.storage).unwrap();

        // Assert that the pending claim is the new one
        assert_eq!(
            pending_claim,
            Claim {
                id,
                owner,
                base_token_amount,
                release_at: expiration,
            }
        );
    }

    #[test]
    fn test_get_pending_claim() {
        let (deps, claims) = setup_pending_claim(None);

        // Get pending claim
        let pending_claim = claims.get_pending_claim(&deps.storage).unwrap();

        // Assert that the pending claim has the correct values
        assert_eq!(
            pending_claim,
            Claim {
                id: 0,
                owner: Addr::unchecked(OWNER),
                base_token_amount: BASE_TOKEN_AMOUNT,
                release_at: EXPIRATION,
            }
        );
    }

    #[test]
    pub fn test_commit_pending_claim() {
        let (mut deps, claims) = setup_pending_claim(None);

        // Commit pending claim
        claims.commit_pending_claim(&mut deps.storage).unwrap();

        // Assert that the pending claim is deleted
        assert!(claims.pending_claim.load(&deps.storage).is_err());

        // Assert that the claim was commited to the claims map
        let claim = claims.claims.load(&deps.storage, 0).unwrap();
        assert_eq!(
            claim,
            Claim {
                id: 0,
                owner: Addr::unchecked(OWNER),
                base_token_amount: BASE_TOKEN_AMOUNT,
                release_at: EXPIRATION,
            }
        );

        // Assert that num claims was incremented
        let num_claims = claims.next_claim_id.load(&deps.storage).unwrap();
        assert_eq!(num_claims, 1);

        // Create another pending claim
        claims
            .create_pending_claim(
                &mut deps.storage,
                &Addr::unchecked(OWNER),
                BASE_TOKEN_AMOUNT,
                EXPIRATION,
                None,
            )
            .unwrap();

        // Assert that claim id was incremented
        let pending_claim = claims.pending_claim.load(&deps.storage).unwrap();
        assert_eq!(pending_claim.id, 1);
    }

    #[test_case(100, NOT_OWNER => Err(StdError::generic_err("Claim not owned by sender")); "claim not owned by sender")]
    #[test_case(100, OWNER => Ok(BASE_TOKEN_AMOUNT) ; "claim owned by sender")]
    #[test_case(99, OWNER => Err(StdError::generic_err("Claim has not yet matured.")); "claim not yet matured")]
    fn test_claim_tokens(block_height: u64, sender: &str) -> StdResult<Uint128> {
        let mut env = mock_env();
        env.block.height = block_height;
        let info = mock_info(sender, &[]);

        let (mut deps, claims) = setup_pending_claim(None);

        // Commit pending claim
        claims.commit_pending_claim(&mut deps.storage).unwrap();

        match claims.claim_tokens(&mut deps.storage, &env.block, &info, 0) {
            Ok(amount) => {
                // Assert that the claim was deleted
                assert!(claims.claims.load(&deps.storage, 0).is_err());
                Ok(amount)
            }
            Err(err) => {
                // Assert that the claim was not deleted
                assert!(claims.claims.load(&deps.storage, 0).is_ok());
                Err(err)
            }
        }
    }

    #[test_case(None, OWNER => Ok(BASE_TOKEN_AMOUNT); "sender is owner")]
    #[test_case(None, NOT_OWNER => Err(StdError::generic_err("Claim not owned by sender")); "sender is not owner")]
    #[test_case(Some(Uint128::new(99u128)), OWNER => Ok(Uint128::new(99u128)); "sender is owner and amount is less than base token amount")]
    fn test_force_unlock(claim_amount: Option<Uint128>, sender: &str) -> StdResult<Uint128> {
        let info = mock_info(sender, &[]);

        let (mut deps, claims) = setup_pending_claim(None);

        // Commit pending claim
        claims.commit_pending_claim(&mut deps.storage).unwrap();

        match claims.force_claim(&mut deps.storage, &info, 0, claim_amount) {
            Ok(amount) => {
                // Assert that the claim was deleted if entire amount was unlocked
                if amount == BASE_TOKEN_AMOUNT {
                    assert!(claims.claims.load(&deps.storage, 0).is_err());
                } else {
                    assert_eq!(
                        claims
                            .claims
                            .load(&deps.storage, 0)
                            .unwrap()
                            .base_token_amount,
                        BASE_TOKEN_AMOUNT - amount
                    );
                }
                Ok(amount)
            }
            Err(err) => {
                // Assert that the claim was not deleted
                assert!(claims.claims.load(&deps.storage, 0).is_ok());
                Err(err)
            }
        }
    }

    #[test_case(0 => Ok(Claim {id: 0, owner: Addr::unchecked(OWNER), base_token_amount: BASE_TOKEN_AMOUNT, release_at: EXPIRATION}); "claim exists")]
    #[test_case(1 => matches Err(_); "claim does not exist")]
    fn test_query_claim_by_id(id: u64) -> StdResult<Claim> {
        let (mut deps, claims) = setup_pending_claim(None);

        // Commit pending claim
        claims.commit_pending_claim(&mut deps.storage).unwrap();

        // Query the claim
        claims.query_claim_by_id(deps.as_ref(), id)
    }

    fn claims(start_id: u64, n: u32) -> Vec<Claim> {
        let mut claims = Vec::new();
        for i in start_id..(start_id + n as u64) {
            claims.push(Claim {
                id: i,
                owner: Addr::unchecked(OWNER),
                base_token_amount: BASE_TOKEN_AMOUNT,
                release_at: EXPIRATION,
            });
        }
        claims
    }

    #[test_case(OWNER, None, None => Ok(claims(0, DEFAULT_LIMIT)); "default pagination")]
    #[test_case(OWNER, None, Some(31) => Ok(claims(0, 31)); "pagination with limit")]
    #[test_case(OWNER, Some(1), None => Ok(claims(2, DEFAULT_LIMIT)); "pagination with start id")]
    #[test_case(OWNER, Some(1), Some(31) => Ok(claims(2, 31)); "pagination with start id and limit")]
    fn test_query_claims_for_owner(
        owner: &str,
        start_after: Option<u64>,
        limit: Option<u32>,
    ) -> StdResult<Vec<Claim>> {
        let mut deps = mock_dependencies();

        // Create 100 claims for owner
        let claims = Claims::new(CLAIMS, CLAIMS_INDEX, PENDING_CLAIMS, NUM_CLAIMS);
        let owner = Addr::unchecked(owner);
        for _ in 0..100 {
            claims
                .create_pending_claim(
                    &mut deps.storage,
                    &owner,
                    BASE_TOKEN_AMOUNT,
                    EXPIRATION,
                    None,
                )
                .unwrap();
            claims.commit_pending_claim(&mut deps.storage).unwrap();
        }

        // Query the claims without using pagination arguments
        claims
            .query_claims_for_owner(deps.as_ref(), &owner, start_after, limit)
            .map(|claims| claims.iter().map(|c| c.1.clone()).collect())
    }
}
