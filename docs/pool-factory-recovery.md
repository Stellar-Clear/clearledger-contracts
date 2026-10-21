# Recovering an already deployed pool

If deployment succeeds but a client loses the response before registration is observed, use `register_existing_pool` with the asset and deployed pool address. The factory requires admin authorization, rejects an already registered asset, validates that the address is a contract, and checks that the pool reports the same funding asset. These checks make recovery safe to retry: a successful registration cannot be replaced by a second pool.

Confirm the pool address and asset on Stellar Testnet before submitting the recovery transaction. A failed validation leaves the factory mapping unchanged.
