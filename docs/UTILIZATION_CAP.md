# Pool utilization cap

`set_max_utilization` accepts a cap from 1 through 10,000 basis points. The
lower bound is intentional: `0` would make every positive `fund_invoice`
operation fail, effectively freezing the pool. The contract rejects that
configuration with `PoolError::InvalidConfiguration` before changing storage,
so an invalid administrative call cannot alter the active cap.

The upper bound remains 10,000 bps (100%). Funding still checks the post-funding
utilization and returns `PoolError::UtilizationCapExceeded` when a valid cap
would be exceeded. Clients should treat an administrative cap update as an
operation that can fail and surface the typed error to operators.
