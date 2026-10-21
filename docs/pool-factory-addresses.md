# Deterministic pool addresses

`pool_factory::register_asset` derives each pool instance address from the factory contract and the asset address. The salt is constructed by `asset_salt` in `contracts/pool_factory/src/lib.rs`; it is network and factory specific, so the same asset on another network has a different address.

Indexers should treat `pool_instance_created` as the canonical mapping event and verify the returned address with `get_pool_for_asset`. SDK callers can independently verify a deployment by reading the factory mapping and comparing the returned pool address before using it. The factory admin and pool Wasm hash must come from the configured Testnet deployment; this procedure does not imply Mainnet deployment.
