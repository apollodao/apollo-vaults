use base_vault::BaseVault;
use cosmwasm_std::{Addr, Binary, DepsMut, Event, MessageInfo, Response};
use cw_controllers::Admin;
use cw_dex::traits::Pool;
use cw_storage_plus::Item;
use cw_vault_token::VaultToken;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::ContractError;
use crate::state::{Claims, Config, ConfigUpdates};

/// AutocompoundingVault is a wrapper around BaseVault that implements
/// autocompounding functionality.
pub struct AutocompoundingVault<'a, S, P, V> {
    /// The base vault implementation
    pub base_vault: BaseVault<'a, V>,

    /// The pool that this vault compounds.
    pub pool: Item<'a, P>,

    /// The staking implementation for this vault
    pub staking: Item<'a, S>,

    /// Configuration for this vault
    pub config: Item<'a, Config>,

    /// The admin address that is allowed to update the config.
    pub admin: Admin<'a>,

    /// Temporary storage of an address that will become the new admin once
    /// they accept the transfer request.
    pub admin_transfer: Item<'a, Addr>,

    /// Stores claims of base_tokens for users who have burned their vault
    /// tokens via ExecuteMsg::Unlock.
    pub claims: Claims<'a>,
}

impl<'a, S, P, V> Default for AutocompoundingVault<'a, S, P, V> {
    fn default() -> Self {
        Self {
            base_vault: BaseVault::default(),
            pool: Item::new("pool"),
            staking: Item::new("staking"),
            config: Item::new("config"),
            claims: Claims::new("claims", "claims_index", "pending_claim", "num_claims"),
            admin: Admin::new("admin"),
            admin_transfer: Item::new("admin_transfer"),
        }
    }
}

impl<S, P, V> AutocompoundingVault<'_, S, P, V>
where
    S: Serialize + DeserializeOwned,
    P: Pool + Serialize + DeserializeOwned,
    V: VaultToken + Serialize + DeserializeOwned,
{
    /// Save values for all of the Items in the struct and instantiates
    /// `base_vault`.
    #[allow(clippy::too_many_arguments)]
    pub fn init(
        &self,
        mut deps: DepsMut,
        admin: Addr,
        pool: P,
        staking: S,
        config: Config,
        vault_token: V,
        init_info: Option<Binary>,
    ) -> Result<Response, ContractError> {
        // Validate that the reward_liquidation_target is part of the pool assets
        let pool_assets = pool.pool_assets(deps.as_ref())?;
        if !pool_assets.contains(&config.reward_liquidation_target) {
            return Err(ContractError::InvalidRewardLiquidationTarget {
                expected: pool_assets,
                actual: config.reward_liquidation_target,
            });
        }

        self.pool.save(deps.storage, &pool)?;
        self.staking.save(deps.storage, &staking)?;
        self.config.save(deps.storage, &config)?;
        self.admin.set(deps.branch(), Some(admin))?;

        Ok(self
            .base_vault
            .init(deps, pool.lp_token(), vault_token, init_info)?)
    }

    /// Update the admin address.
    pub fn execute_update_admin(
        &self,
        deps: DepsMut,
        info: MessageInfo,
        address: String,
    ) -> Result<Response, ContractError> {
        self.admin.assert_admin(deps.as_ref(), &info.sender)?;
        let admin_addr = deps.api.addr_validate(&address)?;
        self.admin_transfer.save(deps.storage, &admin_addr)?;
        let event = Event::new("apollo/vaults/autocompounding_vault").add_attributes(vec![
            ("action", "execute_update_admin"),
            (
                "previous_admin",
                self
                    .admin
                    .get(deps.as_ref())?
                    .unwrap_or_else(|| Addr::unchecked("")).as_ref(),
            ),
            ("new_admin", &address),
        ]);
        Ok(Response::new().add_event(event))
    }

    /// Accept the admin transfer request. This must be called by the new admin
    /// address for the transfer to complete.
    pub fn execute_accept_admin_transfer(
        &self,
        mut deps: DepsMut,
        info: MessageInfo,
    ) -> Result<Response, ContractError> {
        let new_admin = self.admin_transfer.load(deps.storage)?;
        if info.sender != new_admin {
            return Err(ContractError::Unauthorized {});
        }
        self.admin_transfer.remove(deps.storage);
        let event = Event::new("apollo/vaults/autocompounding_vault").add_attributes(vec![
            ("action", "execute_accept_admin_transfer"),
            (
                "previous_admin",
                self
                    .admin
                    .get(deps.as_ref())?
                    .unwrap_or_else(|| Addr::unchecked("")).as_ref(),
            ),
            ("new_admin", new_admin.as_ref()),
        ]);
        self.admin.set(deps.branch(), Some(new_admin))?;
        Ok(Response::new().add_event(event))
    }

    /// Removes the initiated admin transfer. This can only be called by the
    /// admin who initiated the admin transfer.
    pub fn execute_drop_admin_transfer(
        &self,
        deps: DepsMut,
        info: MessageInfo,
    ) -> Result<Response, ContractError> {
        self.admin.assert_admin(deps.as_ref(), &info.sender)?;
        self.admin_transfer.remove(deps.storage);
        let event = Event::new("apollo/vaults/autocompounding_vault")
            .add_attributes(vec![("action", "execute_drop_admin_transfer")]);
        Ok(Response::new().add_event(event))
    }

    /// Update the config.
    pub fn execute_update_config(
        &self,
        deps: DepsMut,
        info: MessageInfo,
        updates: ConfigUpdates,
    ) -> Result<Response, ContractError> {
        self.admin.assert_admin(deps.as_ref(), &info.sender)?;

        let new_config = self
            .config
            .load(deps.storage)?
            .update(deps.as_ref(), updates.clone())?;
        self.config.save(deps.storage, &new_config)?;

        let event = Event::new("apollo/vaults/autocompounding_vault").add_attributes(vec![
            ("action", "execute_update_config"),
            ("updates", &format!("{:?}", updates)),
        ]);

        Ok(Response::default().add_event(event))
    }
}
