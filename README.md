# Apollo Vault Contracts

This repo contains contracts implementing tokenized autocompounding LP vaults for liquidity provider positions on Osmosis, Astroport. Each of the vaults in this repo implements the [CosmWasm Vault Standard](https://github.com/apollodao/cosmwasm-vault-standard/). It is recommended to read the README and/or documentation in that repo before continuing to read about the architecture of thsese contracts below.

## Architectural Overview

This repo contains two packages in the `packages` folder:

- [base-vault](packages/base-vault)
- [apollo-vault](packages/apollo-vaults)

As well as two contracts in the `contracts` folder:

- [osmosis-vault](contracts/osmosis-vault)
- [astroport-vault](contracts/astroport-vault)

### Base Vault

The `base-vault` package contains a [`BaseVault`](https://github.com/apollodao/apollo-vaults/tree/9fbc5cc143d35648644b1316f35f1ce29df626af/packages/base-vault/src/base_vault.rs#L9) struct which takes in a generic parameter `V` which is a vault token implementation. The type used for this parameter must implement the [`VaultToken`](https://github.com/apollodao/cw-vault-token/tree/d04ff1d6f4088b9d734f4190fb7023e22e72a8d8/src/traits.rs#L8) trait from the `cw-vault-token` repo. Currently there are two implementations of this trait used in this repo, namely [`OsmosisDenom`](https://github.com/apollodao/cw-vault-token/tree/d04ff1d6f4088b9d734f4190fb7023e22e72a8d8/src/implementations/osmosis.rs#L24) which uses a Cosmos native token on the Osmosis blockchain minted through the [Token Factory module](https://github.com/CosmWasm/token-factory), as well as [`Cw4626`](https://github.com/apollodao/cw-vault-token/tree/d04ff1d6f4088b9d734f4190fb7023e22e72a8d8/src/implementations/cw4626.rs#L29) which represents functionality for using the vault contract itself as a CW20 vault token, similar to the [ERC-4626 Standard](https://ethereum.org/en/developers/docs/standards/tokens/erc-4626/) on Ethereum.

The `BaseVault` struct contains methods for calculating between numbers of base tokens and numbers of vault tokens, as well as helper methods for sending base tokens and burning vault tokens. See the doc comments in the package for more information. The suggested usage of the `BaseVault` struct is to compose it into a struct implementing a fully complete vault, such as is done in the [`apollo-vault`](https://github.com/apollodao/apollo-vaults/tree/9fbc5cc143d35648644b1316f35f1ce29df626af/packages/apollo-vault/src/autocompounding_vault.rs#L17) package.

### Apollo Vault

The `apollo-vault` package contains a [`AutocomooundingVault`](https://github.com/apollodao/apollo-vaults/tree/9fbc5cc143d35648644b1316f35f1ce29df626af/packages/apollo-vault/src/autocompounding_vault.rs#L15) struct which contains a `BaseVault` struct and other fields storing configurable variables and state of the vault. The struct has three generic parameters: `S`, `P`, and `V`, where `V` is the vault token implementation, the same as in `BaseVault` and is simply passed down to the `BaseVault` struct. `S` and `P` indicate Staking and Pool implementations from the [`cw-dex`](https://github.com/apollodao/cw-dex) repo.

The type passed in to the `P` parameter should implement the [`Pool`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/pool.rs#L9) trait which contains methods for interacting with a dex pool, such as `provide_liquidity`, `swap`, etc.

The type passed in to the `S` parameter should implement some of the traits defined in the [`staking.rs`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/staking.rs) file in the `cw-dex` repo. At a minimum the type must implement the [`Stake`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/staking.rs#L32) trait and by extension the [`Rewards`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/staking.rs#L10) trait. On the `AutoCompoundingVault` struct there exists method implementations with trait bounds for the various traits in the `staking.rs` file of `cw-dex`. This means that depending on which of these traits the type passed in to the `S` generic parameter implements, different methods on the struct will be available. For example, some staking implementations such as [`OsmosisStaking`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/implementations/osmosis/staking.rs#L24) implement the [`Unlock`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/staking.rs#L60) and [`LockedStaking`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/staking.rs#L78) traits, indicating that there is a lockup period associated with staking your tokens in the underlying staking module, while other implementations such as [`AstroportStaking`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/implementations/astroport/staking.rs#L21) instead implement the [`Unstake`](https://github.com/apollodao/cw-dex/tree/de7394fdbc74a3401f4227f81389413991b309e3/src/traits/staking.rs#L46) trait, meaning that tokens can be unstaked directly without lockup.

### Vault Contracts

The contracts in the `contracts` folder each import the `apollo-vault` package with different feature flags depending on what kind of staking implementation the vault is uses. These features enable variants on the [`CallbackMsg`](https://github.com/apollodao/apollo-vaults/blob/9fbc5cc143d35648644b1316f35f1ce29df626af/packages/apollo-vault/src/msg.rs#L34), [`ExtensionExecuteMsg`](https://github.com/apollodao/apollo-vaults/blob/9fbc5cc143d35648644b1316f35f1ce29df626af/packages/apollo-vault/src/msg.rs#L19), and [`ExtensionQueryMsg`](https://github.com/apollodao/apollo-vaults/blob/9fbc5cc143d35648644b1316f35f1ce29df626af/packages/apollo-vault/src/msg.rs#L127) enums. This is so that we don't have to make copies of the enums in each of the contracts with those variants that are needed, which could lead to bugs if we forget to update one of the copies. This is also the reason that this repo is not a Cargo workspace, since two packages in the same workspace cannot import a third package with different features.

In each of the `contract.rs` files for the vaults we import the `AutocompoundingVault` struct from the `apollo-vault` package and give the three generic type parameters concrete types, for example in the Osmosis vault:

```rust
pub type OsmosisVaultContract<'a> =
    AutocompoundingVault<'a, OsmosisStaking, OsmosisPool, OsmosisDenom>;
```

We can then in the `instantiate`, `execute`, and `query` entrypoints call the methods on the struct which will be available depending on which traits the `S` type implements.
