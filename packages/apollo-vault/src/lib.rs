#![warn(rust_2021_compatibility, future_incompatible, nonstandard_style)]
#![forbid(unsafe_code)]
#![deny(bare_trait_objects, unused_doc_comments, unused_import_braces)]
#![warn(missing_docs)]

//! # Apollo Autocompounding Vault
//!
//! ## Description
//!
//! This package contains functions and messages to implement an Autocompounding
//! Vault following the [CosmWasm Vault Standard](https://crates.io/crates/cosmwasm-vault-standard)
//! specification.
//!
//! Any contract using this package MUST import the [`crate::msg::CallbackMsg`],
//! and implement the variant `Callback` as an extension as described in the
//! [specification](https://docs.rs/cosmwasm-vault-standard/0.1.0/cosmwasm_vault_standard/#how-to-use-extensions)
//! The internal implementations of the Autocompounding Vault depend on this
//! extension being properly implemented. All variants of
//! [`crate::msg::CallbackMsg`] MUST be implemented.

#[macro_use]
extern crate derive_builder;

/// Autocompoundning vault
pub mod autocompounding_vault;
/// Error types
pub mod error;
/// Logic related to compounding.
pub mod execute_compound;
/// Logic related to force unlocking.
#[cfg(feature = "force-unlock")]
pub mod execute_force_unlock;
/// Implementations related to redeeming and withdrawing
/// for non-lockup vaults.
#[cfg(feature = "redeem")]
pub mod execute_redeem;
/// Logic related to staking.
pub mod execute_staking;
/// Logic related to unlocking of locked positions.
#[cfg(feature = "lockup")]
pub mod execute_unlock;
/// Messages for the Autocompounding Vault.
pub mod msg;
/// Query functions for the Autocompounding Vault.
pub mod query;
/// Logic for state management.
pub mod state;

pub use crate::autocompounding_vault::AutocompoundingVault;
