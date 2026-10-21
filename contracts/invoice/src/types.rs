use soroban_sdk::{contracttype, Address, BytesN, Symbol};

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InvoiceStatus {
    Created,
    Listed,
    Funded,
    Active,
    Confirmed,
    Repaid,
    Defaulted,
    Expired,
    Cancelled,
}

/// Represents a single invoice tracked by the invoice contract.
///
/// # Invariants
///
/// - `funded_amount <= face_value` — funding must never exceed the face value
///   of the invoice.
/// - `discount_bps` is expressed in basis points (1 bp = 0.01%).
/// - All amounts are denominated in USDC stroops (7 decimals).
#[contracttype]
#[derive(Clone, Debug)]
pub struct Invoice {
    /// Unique 32-byte identifier for this invoice. Used as the primary key
    /// in persistent storage (see [`DataKey::Invoice`]).
    pub id: BytesN<32>,
    /// Address of the party that issued (sold) the invoice and will be paid
    /// the discounted proceeds when funded.
    pub issuer: Address,
    /// Address of the party obligated to repay the invoice at maturity
    /// (typically the buyer of the underlying goods/services).
    pub buyer: Address,
    /// Full face value of the invoice, denominated in USDC stroops
    /// (7 decimals). This is the amount the buyer owes at `due_date`.
    pub face_value: u128,
    /// Discount applied when the invoice is funded, in basis points
    /// (1 bp = 0.01%). Determines the yield to funders.
    pub discount_bps: u32,
    /// Amount that has been funded against this invoice, in USDC stroops.
    ///
    /// Invariant: `funded_amount <= face_value`.
    pub funded_amount: u128,
    /// Unix timestamp (seconds) at which the invoice matures and repayment
    /// is due.
    pub due_date: u64,
    /// Current lifecycle state of the invoice. See [`InvoiceStatus`].
    pub status: InvoiceStatus,
    /// Unix timestamp (seconds) recording when the invoice was created
    /// on-chain.
    pub created_at: u64,
    /// Unix timestamp (seconds) recording when the invoice was listed for
    /// funding. `None` if it has not yet been listed.
    pub listed_at: Option<u64>,
    /// Unix timestamp (seconds) recording when the invoice was funded.
    /// `None` if it has not yet been funded.
    pub funded_at: Option<u64>,
    /// Unix timestamp (seconds) recording when the underlying goods were
    /// shipped by the issuer. `None` until shipment is reported.
    pub shipped_at: Option<u64>,
    /// Dual-confirmation flag: `true` once the issuer has confirmed delivery
    /// / completion. Both `issuer_confirmed` and `buyer_confirmed` must be
    /// `true` to advance the invoice to [`InvoiceStatus::Confirmed`].
    pub issuer_confirmed: bool,
    /// Dual-confirmation flag: `true` once the buyer has confirmed receipt
    /// / acceptance. Both `issuer_confirmed` and `buyer_confirmed` must be
    /// `true` to advance the invoice to [`InvoiceStatus::Confirmed`].
    pub buyer_confirmed: bool,
    /// Unix timestamp (seconds) recording when the invoice was repaid.
    /// `None` until repayment is settled.
    pub repaid_at: Option<u64>,
    /// Address of the asset (token contract) used to fund and repay this
    /// invoice — typically the USDC token contract.
    pub funding_asset: Address,
    /// Address of the pool contract that funded this invoice, when funding
    /// came from a pool rather than a direct funder. `None` for
    /// direct-funded invoices.
    pub funding_pool: Option<Address>,
    /// Cumulative amount repaid so far by the buyer, in USDC stroops.
    pub repaid_amount: u128,
    /// Remaining balance left to be repaid, in USDC stroops.
    pub remaining_balance: u128,
}

#[contracttype]
pub enum DataKey {
    Admin,
    RegistryContract,
    PoolContract,
    Counter,
    Invoice(BytesN<32>),
    IssuerIndexCount(Address),
    BuyerIndexCount(Address),
    StatusIndexCount(u32),
    StatusCount(u32),
    IssuerIndexEntry(Address, u32),
    BuyerIndexEntry(Address, u32),
    StatusIndexEntry(u32, u32),
    ExpiryWindow,
    SupportedAsset(Address),
    SupportedAssetCount,
    // EscrowContract intentionally last to avoid changing enum discriminants for
    // already-deployed contract storage keys. New variants must keep being
    // appended after it, in the same spirit, rather than inserted earlier.
    EscrowContract,
    // Address of the agent-registry contract (deployed separately from the
    // underwrite-contract repo) that `submit_attestation` consults to check
    // an attesting agent's signing key.
    AgentRegistryContract,
    // Attestation recorded against a given invoice, keyed by invoice id.
    Attestation(BytesN<32>),
    // O(1) status-membership marker: `(status, short invoice id)` -> `true`
    // while the invoice is a member of that status index. Written when an
    // invoice enters a status and removed when it transitions out, replacing
    // the full `StatusIndexEntry` scans previously used for duplicate and
    // idempotency checks (issues #831 / #835). Appended after every existing
    // variant so already-deployed key discriminants stay stable.
    //
    // Kept as a *legacy* read path only: new writes use the collision-free
    // `StatusPosition` map below, and the bounded migration clears these rows.
    StatusMembership(InvoiceStatus, u64),
    // Position of an invoice inside its status's compacted slot array:
    // `(status, full invoice id)` -> `u32` slot. Presence is membership and
    // the value is the exact `StatusIndexEntry` slot, so leaving a status is
    // O(1) (swap-and-pop with the last slot) instead of a linear scan.
    // Appended after `StatusMembership` so existing discriminants stay stable.
    StatusPosition(InvoiceStatus, BytesN<32>),
    // Cursor (source slot) for the bounded, paged status-index migration.
    StatusMigrationCursor(InvoiceStatus),
    // Number of live members written by the migration so far.
    StatusMigrationKept(InvoiceStatus),
}

/// A risk attestation recorded against an invoice by a registered
/// Underwrite agent, gating `list_for_financing`.
///
/// Written once by [`InvoiceContract::submit_attestation`] and never
/// updated — the presence check itself is the replay guard, so an invoice
/// can only ever carry a single attestation.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Attestation {
    /// Identifier of the attesting agent, as registered in the
    /// agent-registry contract.
    pub agent_id: Symbol,
    /// Risk score in basis points (0-10000). Never a float — same
    /// fixed-point convention as [`Invoice::discount_bps`].
    pub risk_score: u32,
    /// Hash of the off-chain evidence backing this attestation (e.g. the
    /// underwriting report), for later off-chain verification.
    pub evidence_hash: BytesN<32>,
    /// Unix timestamp (seconds) at which the attestation was submitted.
    pub submitted_at: u64,
}

/// The signed payload an Underwrite agent produces off-chain and submits
/// via [`InvoiceContract::submit_attestation`].
///
/// Decoded from the raw `payload: Bytes` argument via
/// [`soroban_sdk::xdr::FromXdr`], and the *same* raw bytes are what get
/// keccak256-hashed and checked against `signature`. The field order here
/// therefore doubles as the wire format agents must sign over.
#[contracttype]
#[derive(Clone, Debug)]
pub struct AttestationPayload {
    /// Domain separator binding this signature to ClearLedger attestations
    /// specifically, so a signature can't be replayed against an unrelated
    /// contract or message scheme.
    pub domain_separator: BytesN<32>,
    /// The invoice this attestation is for. Must match the `invoice_id`
    /// argument passed to `submit_attestation`.
    pub invoice_id: BytesN<32>,
    pub risk_score: u32,
    pub evidence_hash: BytesN<32>,
    pub agent_id: Symbol,
    /// Caller-chosen nonce. Not separately tracked on-chain: the
    /// one-attestation-per-invoice storage check is the replay guard.
    pub nonce: u64,
}

/// Local mirror of the `Agent` record exposed by the agent-registry
/// contract (from the separate `underwrite-contract` repo). This contract
/// has no crate dependency on that repo, so this shape must be kept in
/// sync with its `get_agent` return type by hand.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Agent {
    pub active: bool,
    pub pubkey: BytesN<65>,
}

impl InvoiceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvoiceStatus::Created => "Created",
            InvoiceStatus::Listed => "Listed",
            InvoiceStatus::Funded => "Funded",
            InvoiceStatus::Active => "Active",
            InvoiceStatus::Confirmed => "Confirmed",
            InvoiceStatus::Repaid => "Repaid",
            InvoiceStatus::Defaulted => "Defaulted",
            InvoiceStatus::Expired => "Expired",
            InvoiceStatus::Cancelled => "Cancelled",
        }
    }
}
