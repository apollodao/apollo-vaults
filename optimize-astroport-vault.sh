docker run --rm -v "$(pwd)":/code \
  --mount type=volume,source="astroport-vault_cache",target=/code/contracts/astroport-vault/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/rust-optimizer:0.12.10 ./contracts/astroport-vault
