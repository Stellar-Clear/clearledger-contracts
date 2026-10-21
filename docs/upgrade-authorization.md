# Contract upgrade authorization

Every contract upgrade entry point requires the stored admin address to authorize the call and accepts a 32-byte Wasm hash. Operators should verify the hash against the artifact uploaded to the selected Stellar network before signing. Invalid hashes and unauthorized callers are rejected by the contract; upgrades do not bypass pause or storage migration policy. The behavior is network-specific and does not imply Mainnet readiness.
