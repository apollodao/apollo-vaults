# Astroport autocompounding vault

This contract is an autocompounding vault for Astroport. It uses [cw-dex](https://github.com/apollodao/cw-dex/tree/master/src/implementations/astroport) for interfacing with Astroport and is only compatible with Astroport pairs built from the commits that the `cw-dex` version used is compatible with. There are two types of Astroport pairs supported by this vault, constant product pairs and stable swap pairs with two liquid assets each.
