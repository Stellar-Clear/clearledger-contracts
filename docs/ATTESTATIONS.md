# Underwrite Attestations: Signed Preimage and Trust Boundary

> **Applies to:** `InvoiceContract::submit_attestation` in
> `contracts/invoice/src/lib.rs`
> **Related issue:** #117 (signature domain separation for cross-network
> replay resistance)

This document specifies exactly what an Underwrite agent signs, how the
invoice contract verifies it, and which replay paths are blocked. It is the
ABI/security reference for off-chain signers and relayers.

---

## Wire format

`submit_attestation(invoice_id, payload, signature)` takes the signed payload
as raw bytes. `payload` must be the XDR `ScVal` encoding of
[`AttestationPayload`](../contracts/invoice/src/types.rs):

```text
AttestationPayload {
    domain_separator: BytesN<32>,   // effective domain separator, see below
    invoice_id:       BytesN<32>,   // invoice being attested
    risk_score:       u32,          // basis points
    evidence_hash:    BytesN<32>,   // hash of off-chain evidence
    agent_id:         Symbol,       // registered agent identity
    nonce:            u64,          // caller-chosen, signed
}
```

The signed preimage is **exactly these bytes**, in this field order. The
digest is:

```text
digest = keccak256(payload_bytes)
```

and `signature` is the 65-byte secp256k1 recoverable signature
`r || s || recovery_id` over `digest`. The contract does not re-encode the
struct before hashing: the bytes supplied by the caller are the bytes that
were signed.

---

## Effective domain separator

`AttestationPayload.domain_separator` must equal the value returned by the
view function:

```text
InvoiceContract::get_attestation_domain_separator()
```

which is computed as:

```text
separator = SHA-256(
    ATTESTATION_DOMAIN_SEPARATOR   // "ClearLedger.InvoiceAttest.v1____" (32 bytes)
    || ledger.network_id()         // 32 bytes: SHA-256 of the Stellar network passphrase
    || contract_address.to_xdr()   // XDR encoding of this invoice contract's Address
)
```

Why each component is present:

| Component | Binds the signature to |
|-----------|------------------------|
| `ATTESTATION_DOMAIN_SEPARATOR` | The ClearLedger invoice-attestation scheme (v1), separating it from other protocols and message formats. |
| `ledger.network_id()` | The Stellar network. `network_id` is `SHA-256(passphrase)`, e.g. `SHA-256("Test SDF Network ; September 2015")` on testnet or `SHA-256("Public Global Stellar Network ; September 2015")` on mainnet. An attestation signed for testnet cannot be accepted by a mainnet deployment. |
| `contract_address.to_xdr()` | This exact invoice-contract deployment. A signature for one deployed invoice contract cannot be replayed against another deployment, even one with the same Wasm and the same invoice id. Soroban contract ids are themselves derived with the network id, so this also provides defence in depth for network separation. |

Agents **must not** hard-code the raw 32-byte tag or a previously observed
separator. Call the view on the exact contract and network they intend to
attest to, then place the returned value in the payload.

### Migration note

Earlier revisions accepted the raw fixed constant as the payload's
`domain_separator`. That value is no longer accepted because it did not bind
the network or the contract deployment. Off-chain signers must switch to
`get_attestation_domain_separator()` (or reproduce the formula above). This is
a signing-time change only:

- `AttestationPayload`'s field names, types, and order are unchanged.
- `submit_attestation`'s signature is unchanged.
- Already-stored `Attestation` records are unchanged.
- One new read-only method, `get_attestation_domain_separator`, is added to
  the contract interface.

---

## Verification and trust boundary

Submission is **permissionless**: there is no caller `require_auth()`. The
relayer cannot change the attested facts, because trust comes from the
signature and the agent registry, not from the caller.

For a submission to succeed:

1. The invoice exists (`get_invoice`).
2. The invoice has no stored attestation yet. This one-attestation-per-invoice
   guard is the on-chain replay guard (`AlreadyAttested`).
3. `payload` decodes as `AttestationPayload`.
4. `domain_separator` equals the effective separator for this network and this
   contract (`InvalidAmount` on mismatch).
5. `invoice_id` inside the payload equals the `invoice_id` argument
   (`InvalidAmount` on mismatch).
6. `secp256k1_recover(keccak256(payload), r, s, recovery_id)` succeeds.
7. The configured agent registry returns an **active** agent for
   `payload.agent_id`, and that agent's registered 65-byte pubkey equals the
   recovered pubkey (`UntrustedSigner` otherwise).

The resulting `Attestation` stores `agent_id`, `risk_score`, `evidence_hash`,
and `submitted_at`. It does not store the nonce or the signature.

### What each signed field prevents

| Change after signing | Result |
|----------------------|--------|
| Different `domain_separator` (other scheme, network, or contract) | `InvalidAmount` before signature recovery. |
| Different `invoice_id` than the call argument | `InvalidAmount`. |
| Different `invoice_id` whose value is also passed as the argument | The other invoice has no stored attestation, so the signature is verified against the modified payload and recovers a different key; `UntrustedSigner`. Signature bytes cannot be reused across invoices. |
| Different `risk_score` or `evidence_hash` | Signature no longer recovers the registered key; `UntrustedSigner`. |
| Different `agent_id` | Payload digest changes; recovered key does not match that agent's registered pubkey (`UntrustedSigner`). |
| Different `nonce` | Payload digest changes; signature no longer verifies (`UntrustedSigner`). |
| Same payload and signature replayed for the same invoice | Stored attestation already exists; `AlreadyAttested`. |

### Nonce semantics

The nonce is part of the signed preimage, so mutating it invalidates a
signature. It is **not** a per-agent monotonic nonce and there is no nonce
ledger. Because an invoice can carry at most one attestation, distinct nonces
cannot authorize a second attestation for the same invoice. Agents should
still use unique nonces off-chain so two attestations intended for different
invoices never share a preimage.

---

## Resource budget

`submit_attestation` performs:

- one persistent invoice read and one persistent attestation-existence check;
- one SHA-256 over the ~100-byte domain preimage (tag + network id + address
  XDR);
- one keccak256 over the caller's payload;
- one secp256k1 public-key recovery;
- one cross-contract `get_agent` call to the configured agent registry;
- one persistent attestation write plus TTL extensions.

It is roughly a **medium** relative cost: higher than a plain state write
because of the two hashes, signature recovery, and cross-contract read, but
lower than token-transfer paths. See the budget table in
[LIMITATIONS.md](./LIMITATIONS.md#soroban-resource-model). The
`test_gas_benchmark_submit_attestation` test in
`contracts/invoice/src/test.rs` prints the measured CPU-instruction and memory
deltas for the current host so changes that add work to this path are visible
in test output.

---

## Out of scope / residual assumptions

- The agent registry is trusted to report the correct active pubkey for an
  `agent_id`. This repo only mirrors its `get_agent` return type.
- The off-chain signer is responsible for obtaining the separator from the
  intended network/contract and protecting its secp256k1 key.
- Stellar transaction-level replay protection (network passphrase in the
  transaction signature base) is separate from, and additional to, this
  attestation-level binding.
- Contract upgrades are admin-authorized. An upgrade can change this scheme;
  the v1 tag and the formula above identify the current one.
