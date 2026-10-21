# Pool configuration schema v1

`get_config_v1() -> PoolConfigV1` reads funding asset, minimum initial deposit,
utilization cap, batch size/fee bounds, effective protocol fee, treasury,
linked invoice/escrow/registry contracts and the pool's pause flag in one call.
The response's `version` is `1`. Future incompatible changes should use a new
method/type rather than silently changing this struct's wire format.

The call needs no authorization, remains usable while paused, and rejects an
uninitialized pool. Like existing queries it may renew the instance TTL. It
performs no cross-contract calls and does not change configuration.

`protocol_fee_bps` is the effective settlement fee from `ProtocolFeeBps`, default
zero for legacy state. `initialization_fee_bps` separately exposes the legacy
`FeeBps` key: clients must not mistake it for the current settlement fee.
Treasury and minimum initial deposit use existing scalar getter fallbacks.
`paused` is this instance's flag, not another linked contract's pause state.

This is an additive ABI change; all existing getters, events and storage keys
remain compatible. Regenerate SDK bindings from the newly built contract spec
to use the new struct. Focused tests compare the scalar getters, admin updates,
paused reads and uninitialized failure.
