# Registry Metadata Privacy and Data Minimization

> **Updated:** 2026-10-20
> **Applies to:** `contracts/registry` (the ClearLedger registry contract)
> **Status:** Stellar Testnet only — no Mainnet privacy or availability guarantee

This document describes what registry profile data becomes public when a
profile is registered or updated on Stellar, which values must never be placed
on-chain, and how clients should store references to personal data that stays
off-chain. It is engineering guidance, not legal advice.

The short version: **treat every registry field — especially every metadata key
and value — as public, permanent, and readable by anyone.** The contract has no
confidentiality layer and no erasure function.

---

## Table of Contents

- [Scope](#scope)
- [On-Chain Visibility and Permanence](#on-chain-visibility-and-permanence)
- [Public Profile Data](#public-profile-data)
- [Public Views and Enumeration](#public-views-and-enumeration)
- [Events](#events)
- [Metadata Constraints](#metadata-constraints)
- [Prohibited Data](#prohibited-data)
- [Recommended On-Chain References](#recommended-on-chain-references)
- [Client Storage Guidance](#client-storage-guidance)
- [Deletion and Correction Limits](#deletion-and-correction-limits)
- [Testnet-Only Framing](#testnet-only-framing)
- [Integrator Checklist](#integrator-checklist)
- [Related Documentation](#related-documentation)

---

## Scope

This document covers the registry's `Profile` record, its `metadata` map, the
registration/update entry points, the profile enumeration index, and the events
published by `contracts/registry/src/events.rs`.

It does not define the privacy properties of invoice, escrow, pool, or
pool-factory data. Those contracts have their own public state and event
payloads; see the [event catalog](EVENTS.md) and [storage schema](STORAGE.md).

---

## On-Chain Visibility and Permanence

Stellar is a public ledger. Soroban contract data, transaction invocation
arguments, and contract events are not confidential. The registry adds no
encryption, access control, or off-chain gate to any of them:

- `get_profile` is a permissionless read-only view. It returns the full
  `Profile`, including the complete metadata map, to any caller.
- Contract storage entries are part of public ledger state and can be read
  through Stellar RPC or ledger tooling without calling the contract at all.
- `register_issuer`, `register_buyer`, `update_profile`, and
  `update_metadata` receive the metadata map as a transaction argument. The
  submitted map is therefore recorded in transaction history and public
  archives, even where the contract later discards or overwrites it.
- Registration and update events are public, and indexers may retain them
  indefinitely.
- Anyone can enumerate registered addresses by role through
  `get_profile_count` and `list_profiles`.

On-chain writes should be assumed to be **practically permanent**. A later
update can change what the contract returns for the *current* profile, but it
does not remove the earlier transaction arguments, ledger history, or copies
held by indexers and archives. See
[Deletion and Correction Limits](#deletion-and-correction-limits).

Do not submit a value that you would not be comfortable publishing on the
public internet forever.

---

## Public Profile Data

The registry stores the following fields under `DataKey::Profile(address)` in
persistent storage. All of them are public.

| Field | Type | Public meaning |
| --- | --- | --- |
| Address (storage key) | `Address` | The registered Stellar account. Also appears in the enumeration index and registration events. |
| `packed_flags` | `u32` | Bit 0 is the role (`Issuer` or `Buyer`), bit 1 is verified, bit 2 is revoked. |
| `registered_at` | `u64` | Ledger timestamp of registration. Publicly reveals registration timing. |
| `metadata` | `Map<String, String>` | Arbitrary key-value data supplied by the registrant or admin. Returned verbatim by `get_profile`. |

The following keys are also public:

| DataKey | Type | Public meaning |
| --- | --- | --- |
| `DataKey::Admin` | `Address` | Contract admin, readable through `get_admin` and emitted at initialization. |
| `DataKey::ProfileIndex(Role, u32)` | `Address` | Append-only list of registered addresses for each role, in registration order. |
| `DataKey::ProfileCount(Role)` | `u32` | Number of registered addresses for each role. |

Registration itself is therefore a disclosure: the address, its role, its
registration time, its verification/revocation state, and its position in the
enumeration index are all public. Revocation does not hide a profile or remove
it from the index.

A Stellar address is pseudonymous, not anonymous. If an address is linked to a
real-world identity anywhere off-chain, that link can be used to associate the
public registry record with that identity. Do not assume that omitting a name
from metadata makes a profile anonymous.

---

## Public Views and Enumeration

None of these functions require authorization:

| Function | What it exposes |
| --- | --- |
| `get_profile(address)` | The full `Profile`, including all metadata key-value pairs. |
| `is_verified(address)` | Whether the profile exists and is currently verified. |
| `get_verification_status(address)` | `Unregistered`, `Pending`, `Verified`, or `Revoked`. |
| `get_profile_count(role)` | Number of registered addresses for a role. |
| `list_profiles(role, start, limit)` | Up to 50 registered addresses per call, in registration order. |
| `get_admin()` | The contract admin address. |

`list_profiles` is capped at 50 addresses per call, but none of the data is
access-controlled. An indexer can walk the full registry in bounded pages.

---

## Events

The registry currently emits the following events. Events contain addresses and
status values, but **not metadata key-value pairs**.

| Event | Topics | Data | Privacy note |
| --- | --- | --- | --- |
| `contract_initialized` | `contract_initialized`, admin | `()` | Publishes the admin address. |
| `issuer_registered` | `issuer_registered`, issuer | `()` | Publishes the issuer address and the fact/timing of registration. |
| `buyer_registered` | `buyer_registered`, buyer | `()` | Publishes the buyer address and the fact/timing of registration. |
| `address_revoked` | `address_revoked`, address | `()` | Publishes that a profile was revoked. |
| `address_reinstated` | `address_reinstated`, address | `()` | Publishes that a profile was reinstated. |
| `batch_registered` | `batch_registered` | `(registered: u32, skipped: u32)` | This summary event carries counts only, but the batch path also emits one `issuer_registered` / `buyer_registered` event per successfully registered address. |
| `profile_verified` | `profile_verified`, address | `bool` | Publishes the verification state change. |
| `profile_updated` | `profile_updated`, address | `()` | Publishes that a profile's metadata was updated and when. |
| `ownership_transferred` | `ownership_transferred`, old admin | `Address` (new admin) | Publishes both admin addresses. |
| `admin_transferred` | `admin_transferred`, old admin | `Address` (new admin) | Publishes both admin addresses. |
| `paused` / `unpaused` | event name, admin | `()` | Publishes which admin operated the circuit breaker. |

**Metadata not appearing in an event is not a confidentiality guarantee.**
Metadata is still returned by `get_profile`, is present in the transaction
arguments of the call that submitted it, and may be copied into indexer
databases or archival systems. Integrators should assume metadata is public and
durable regardless of event payloads.

The authoritative event descriptions and source locations are in
[docs/EVENTS.md](EVENTS.md#registry-contract).

---

## Metadata Constraints

The registry validates only size and emptiness. It does not validate meaning,
schema, encoding, or content, and it cannot detect personal data.

| Constraint | Value | Enforcement |
| --- | --- | --- |
| Maximum entries per metadata map | 20 | `MAX_METADATA_SIZE`; larger maps panic with `InvalidMetadata` (`#6`). |
| Maximum key length | 64 bytes | `MAX_METADATA_KEY_LEN`; measured by Soroban `String::len`. |
| Maximum value length | 512 bytes | `MAX_METADATA_VALUE_LEN`; measured by Soroban `String::len`. |
| Empty key or empty value | Rejected | `InvalidMetadata` (`#6`). An empty map is allowed. |
| Maximum batch entries | 50 | `BatchSizeExceeded` (`#5`) on `batch_register_issuers` / `batch_register_buyers`. |
| Maximum `list_profiles` page | 50 | `PageSizeExceeded` (`#8`). |

Notes:

- `String` is a byte array in Soroban, so the contract does not enforce UTF-8
  or any other character encoding. Applications must agree on an encoding and
  treat arbitrary bytes defensively.
- The 20-entry cap bounds raw metadata to roughly 11.5 KB
  (`20 × (64 + 512)` bytes) plus container/XDR overhead. The cap exists to
  bound storage and resource use; it is **not** a privacy control.
- `update_profile` and `update_metadata` replace the entire metadata map. They
  do not merge keys. A client that sends only changed keys silently drops every
  previously stored key.
- `batch_register_issuers` validates all supplied metadata before writing, but
  currently creates issuer profiles with an **empty** metadata map; the supplied
  values are discarded. `batch_register_buyers` stores the supplied metadata.
  Do not rely on either behavior for confidentiality — both still receive the
  metadata as public transaction arguments. Use the single-profile
  `register_issuer` path if issuer metadata must be persisted.

The relevant source is
[`contracts/registry/src/lib.rs`](../contracts/registry/src/lib.rs) and
[`contracts/registry/src/types.rs`](../contracts/registry/src/types.rs).

---

## Prohibited Data

Never put the following in metadata keys, metadata values, or transaction
memos associated with registration:

- **Direct identifiers:** legal or trading names of individuals, personal
  names, email addresses, phone numbers, postal addresses, dates of birth, and
  government, tax, or national identification numbers.
- **Financial identifiers:** bank account numbers, IBANs, card numbers, payment
  credentials, and account statements.
- **KYC/AML material:** passport or driver's-licence images, proof of address,
  beneficial-ownership records, sanctions-screening results, and identity
  documents.
- **Authentication secrets:** private keys, seed phrases, API keys, passwords,
  bearer tokens, session tokens, and signed or pre-signed URLs.
- **Sensitive personal data:** health, biometric, genetic, religious, ethnic,
  political, sexual-orientation, or criminal-record information.
- **Confidential commercial data:** invoice text, contract terms, customer or
  supplier lists, pricing, bank statements, and free-text case notes.
- **Unkeyed hashes of low-entropy identifiers:** an unsalted SHA-256 of an email
  address, phone number, or national ID is not anonymization; it can be
  brute-forced or matched against another dataset.

Also avoid:

- Personal data in metadata **keys**. Keys are just as public as values.
- Deterministic references reused across profiles, which make correlation
  trivial.
- Free-text notes that may accidentally contain any of the above.
- Assuming that reversal or revocation removes a value. It does not.

If a workflow requires any of this information, it belongs in an
access-controlled off-chain system, referenced from the registry only by an
opaque identifier or a keyed commitment.

---

## Recommended On-Chain References

Store the minimum reference needed to join the on-chain record to an off-chain
record. The reference should be useless to someone who does not already have
access to the off-chain system.

Preferred, in order:

1. **Random opaque identifier.** Use a per-profile random value such as a
   128-bit random ID (for example, a UUIDv4-style value) generated independently
   of the personal data. Store the mapping from that ID to the PII in an
   access-controlled off-chain database. Do not derive the ID from an email,
   phone number, or account number.
2. **Keyed commitment.** If an on-chain commitment is required, use a
   domain-separated HMAC (for example, HMAC-SHA-256) with a secret key held in
   an off-chain secret manager, and document the algorithm and key version.
   Keep the key out of the registry and out of transaction metadata.
3. **Salted hash of a high-entropy document.** For integrity checks over
   documents that already have high entropy, a random-salted hash can be used.
   Store the salt and the document off-chain. A salt does not make a
   low-entropy identifier safe from brute force.
4. **Status or schema labels only.** Non-personal values such as a coarse
   jurisdiction tier, a product code, or a schema version may be stored
   directly if they are intentionally public and cannot be combined to
   re-identify a person.

A minimal, non-personal metadata map might look like:

```json
{
  "schema": "clearledger.profile.v1",
  "kyc_ref": "4f4c0a2e-7a0d-4f7f-9c2b-2d6a5e8b1c33",
  "doc_ref": "8b1d6a0c-2e4f-4a7b-9d10-5c3e2f1a6b90"
}
```

The example values are random identifiers, not personal data. The off-chain
system is responsible for resolving them, enforcing access control, and
deleting the mapping when required.

Additional guidance:

- Keep keys generic and versioned (for example, `schema`, `kyc_ref`,
  `doc_ref`) rather than embedding identities in key names.
- Do not store access tokens, presigned URLs, or credentials as "references."
  A reference should not grant access by itself.
- Treat a hash as a permanent correlation handle. If the same low-entropy value
  is hashed elsewhere, or the off-chain data leaks, the hash may become
  linkable.
- Avoid putting personal data in the Stellar transaction memo as a workaround.
  Memos are public too.

---

## Client Storage Guidance

Clients integrating the registry should follow these rules:

- **Split public from private.** Put only opaque references on-chain; keep
  names, contact details, KYC documents, and commercial terms in an
  access-controlled off-chain store.
- **Encrypt and minimize off-chain data.** Encrypt at rest, restrict access by
  role, log access, and retain only what the business and legal basis require.
- **Design for deletion off-chain.** Keep the ID-to-PII mapping deletable.
  Deleting the mapping (or the encryption key protecting it) makes the
  on-chain reference unresolvable, but it does not erase the reference, the
  transaction history, or any copies already exported. Do not describe this as
  on-chain erasure.
- **Do not rely on metadata secrecy.** Do not use metadata as a covert channel,
  a password store, or an access-control decision source. Anyone can read it.
- **Warn users before submission.** Registration is public and effectively
  permanent. Obtain informed consent for the on-chain reference and explain
  that the reference cannot be erased from ledger history.
- **Validate before signing.** Enforce your own allowlist of metadata keys,
  reject PII patterns, and cap the map before calling the contract. The
  registry's size checks are not a privacy review.
- **Treat indexers and caches as public copies.** If you index registry data,
  store it as public data, keep PII out of the index, apply retention limits,
  and do not enrich on-chain profiles with private data in a shared store.
- **Use separate environments.** Never use production personal data on
  Testnet, and never reuse production secrets or identifiers in test
  deployments.

---

## Deletion and Correction Limits

There is no on-chain erasure function in the registry.

- **No delete or deregister.** A profile cannot be deleted. `revoke` only
  flips verification/revocation flags; it leaves the profile, its metadata, and
  its enumeration slot in place. The index is append-only.
- **Updates replace, not erase.** `update_profile` and `update_metadata`
  overwrite the *current* metadata map. The previous values remain in
  transaction arguments, ledger history, and any archives or indexer copies.
- **Clearing current metadata is possible.** A profile owner can submit an
  empty map through `update_profile` or `update_metadata`. This clears what
  `get_profile` returns going forward, but it does not erase historical values.
  Only the profile owner can do this; the admin cannot update metadata on the
  owner's behalf.
- **Revocation does not clear metadata.** Revoked profiles remain readable and
  enumerated.
- **TTL expiry is not erasure.** Soroban state archival may move expired
  entries out of live state, but entries can be restored, and transaction
  history and external copies are unaffected. Testnet resets are not a
  deletion guarantee.
- **No on-chain correction guarantee.** A correction changes the current
  record only. Consumers that cached or indexed the old value may continue to
  hold it.

For any data that may need to be corrected or deleted, keep the authoritative
copy off-chain and put only an opaque reference on-chain. If a reference must
become unusable, delete the off-chain mapping and document that the historical
on-chain reference remains.

---

## Testnet-Only Framing

These contracts are currently deployed and used on **Stellar Testnet**. This
document describes that implementation and makes **no Mainnet guarantee**:

- There is no Mainnet deployment, audit, or production privacy commitment
  described here.
- Testnet state may be reset or become unavailable. That is not a data
  deletion mechanism.
- Do not submit real personal data, real KYC documents, production secrets, or
  live financial identifiers to Testnet contracts.
- A future Mainnet deployment would require a separate privacy, security, and
  compliance review. Do not infer Mainnet behavior from this document.

See [Limitations](LIMITATIONS.md) and [SECURITY.md](../SECURITY.md) for the
broader Testnet and audit caveats.

---

## Integrator Checklist

Before writing a registry profile:

- [ ] Does the metadata contain only opaque references or non-personal values?
- [ ] Are all keys schema labels rather than identifiers?
- [ ] Is the map within 20 entries, with keys at most 64 bytes and values at
      most 512 bytes?
- [ ] Are empty keys and values avoided? (An entirely empty map is allowed.)
- [ ] Is the off-chain PII store access-controlled, encrypted, and deletable?
- [ ] Are references random or keyed, rather than unsalted hashes of
      low-entropy identifiers?
- [ ] Has the user been told that the record is public and effectively
      permanent?
- [ ] Is the same data absent from transaction memos and logs?
- [ ] If using a batch entry point, is the metadata behavior understood
      (`batch_register_issuers` currently discards metadata;
      `batch_register_buyers` persists it)?

---

## Related Documentation

- [Event catalog](EVENTS.md) — every registry event and its payload.
- [Storage schema](STORAGE.md) — registry keys, profile structure, and TTLs.
- [Threat model](THREAT_MODEL.md) — trust assumptions and authentication gates.
- [Limitations](LIMITATIONS.md) — Testnet constraints and known gaps.
- [Registry contract source](../contracts/registry/src/lib.rs) — validation,
  registration, update, revocation, and views.
- [Registry types](../contracts/registry/src/types.rs) — `Profile`, `Role`,
  `VerificationStatus`, and `DataKey`.
