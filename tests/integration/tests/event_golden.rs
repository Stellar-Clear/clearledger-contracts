// Generated fixed-input cases for the published emitter schema. Do not regenerate
// golden bytes during tests: review schema changes as an explicit API change.
use soroban_sdk::{
    contract, contractimpl, testutils::Events as _, xdr::ToXdr, Address, BytesN, Env, String,
    Symbol,
};
#[contract]
pub struct EventHarness;
#[contractimpl]
impl EventHarness {
    pub fn noop(_env: Env) {}
}
fn hex(bytes: soroban_sdk::Bytes) -> std::string::String {
    bytes
        .iter()
        .map(|byte| std::format!("{byte:02x}"))
        .collect()
}
fn fixture(name: &str) -> (&'static str, &'static str) {
    let line = include_str!("../fixtures/event-xdr.tsv")
        .lines()
        .find(|line| line.split('\t').next() == Some(name))
        .expect("missing golden event");
    let mut fields = line.split('\t');
    fields.next();
    (fields.next().unwrap(), fields.next().unwrap())
}
#[path = "../../../contracts/invoice/src/events.rs"]
mod invoice_events;

#[path = "../../../contracts/registry/src/events.rs"]
mod registry_events;

#[path = "../../../contracts/pool/src/events.rs"]
mod pool_events;

#[path = "../../../contracts/escrow/src/events.rs"]
mod escrow_events;

#[path = "../../../contracts/pool_factory/src/events.rs"]
mod pool_factory_events;
#[test]
fn invoice_invoice_created_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let issuer = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let buyer = Address::from_string(&String::from_str(
        &env,
        "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3",
    ));
    let face_value = 18446744073709551637u128;
    let funding_asset = Address::from_string(&String::from_str(
        &env,
        "CACQKBIFAUCQKBIFAUCQKBIFAUCQKBIFAUCQKBIFAUCQKBIFAUCQLC2U",
    ));
    env.as_contract(&contract, || {
        invoice_events::invoice_created(
            &env,
            &invoice_id,
            &issuer,
            &buyer,
            face_value,
            &funding_asset,
        )
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_created");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_created"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_created"
    );
}

#[test]
fn invoice_invoice_listed_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let discount_bps = 102u32;
    env.as_contract(&contract, || {
        invoice_events::invoice_listed(&env, &invoice_id, discount_bps)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_listed");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_listed"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_listed"
    );
}

#[test]
fn invoice_invoice_cancelled_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || {
        invoice_events::invoice_cancelled(&env, &invoice_id)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_cancelled");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_cancelled"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_cancelled"
    );
}

#[test]
fn invoice_upgraded_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let wasm_hash = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || invoice_events::upgraded(&env, &wasm_hash));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.upgraded");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.upgraded"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.upgraded"
    );
}

#[test]
fn invoice_invoice_funded_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let funded_amount = 18446744073709551635u128;
    env.as_contract(&contract, || {
        invoice_events::invoice_funded(&env, &invoice_id, funded_amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_funded");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_funded"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_funded"
    );
}

#[test]
fn invoice_invoice_shipped_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || {
        invoice_events::invoice_shipped(&env, &invoice_id)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_shipped");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_shipped"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_shipped"
    );
}

#[test]
fn invoice_delivery_confirmed_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let confirmer = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        invoice_events::delivery_confirmed(&env, &invoice_id, &confirmer)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.delivery_confirmed");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.delivery_confirmed"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.delivery_confirmed"
    );
}

#[test]
fn invoice_both_confirmed_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || {
        invoice_events::both_confirmed(&env, &invoice_id)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.both_confirmed");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.both_confirmed"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.both_confirmed"
    );
}

#[test]
fn invoice_invoice_repaid_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let amount = 18446744073709551635u128;
    env.as_contract(&contract, || {
        invoice_events::invoice_repaid(&env, &invoice_id, amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_repaid");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_repaid"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_repaid"
    );
}

#[test]
fn invoice_partial_repayment_received_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let amount = 18446744073709551635u128;
    let remaining_balance = 18446744073709551636u128;
    env.as_contract(&contract, || {
        invoice_events::partial_repayment_received(&env, &invoice_id, amount, remaining_balance)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.partial_repayment_received");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.partial_repayment_received"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.partial_repayment_received"
    );
}

#[test]
fn invoice_invoice_defaulted_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || {
        invoice_events::invoice_defaulted(&env, &invoice_id)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_defaulted");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_defaulted"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_defaulted"
    );
}

#[test]
fn invoice_invoice_expired_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || {
        invoice_events::invoice_expired(&env, &invoice_id)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.invoice_expired");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.invoice_expired"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.invoice_expired"
    );
}

#[test]
fn invoice_expiry_window_set_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let window = 101u64;
    env.as_contract(&contract, || {
        invoice_events::expiry_window_set(&env, window)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.expiry_window_set");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.expiry_window_set"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.expiry_window_set"
    );
}

#[test]
fn invoice_batch_invoices_created_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let created = 101u32;
    let failed = 102u32;
    env.as_contract(&contract, || {
        invoice_events::batch_invoices_created(&env, created, failed)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.batch_invoices_created");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.batch_invoices_created"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.batch_invoices_created"
    );
}

#[test]
fn invoice_batch_invoices_listed_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let listed = 101u32;
    let failed = 102u32;
    env.as_contract(&contract, || {
        invoice_events::batch_invoices_listed(&env, listed, failed)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.batch_invoices_listed");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.batch_invoices_listed"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.batch_invoices_listed"
    );
}

#[test]
fn invoice_ownership_transferred_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let from = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let to = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        invoice_events::ownership_transferred(&env, &from, &to)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.ownership_transferred");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.ownership_transferred"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.ownership_transferred"
    );
}

#[test]
fn invoice_paused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || invoice_events::paused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.paused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.paused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.paused"
    );
}

#[test]
fn invoice_unpaused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || invoice_events::unpaused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.unpaused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.unpaused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.unpaused"
    );
}

#[test]
fn invoice_pool_contract_updated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        invoice_events::pool_contract_updated(&env, &old, &new)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.pool_contract_updated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.pool_contract_updated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.pool_contract_updated"
    );
}

#[test]
fn invoice_attestation_submitted_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let agent_id = Symbol::new(&env, "agent_1");
    let risk_score = 103u32;
    env.as_contract(&contract, || {
        invoice_events::attestation_submitted(&env, &invoice_id, &agent_id, risk_score)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.attestation_submitted");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.attestation_submitted"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.attestation_submitted"
    );
}

#[test]
fn invoice_agent_registry_contract_updated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        invoice_events::agent_registry_contract_updated(&env, &old, &new)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.agent_registry_contract_updated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.agent_registry_contract_updated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.agent_registry_contract_updated"
    );
}

#[test]
fn invoice_contract_initialized_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let registry_contract = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        invoice_events::contract_initialized(&env, &admin, &registry_contract)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.contract_initialized");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.contract_initialized"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.contract_initialized"
    );
}

#[test]
fn invoice_supported_asset_added_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let asset = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        invoice_events::supported_asset_added(&env, &asset)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.supported_asset_added");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.supported_asset_added"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.supported_asset_added"
    );
}

#[test]
fn invoice_supported_asset_removed_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let asset = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        invoice_events::supported_asset_removed(&env, &asset)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.supported_asset_removed");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.supported_asset_removed"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.supported_asset_removed"
    );
}

#[test]
fn invoice_escrow_contract_updated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        invoice_events::escrow_contract_updated(&env, &old, &new)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("invoice.escrow_contract_updated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: invoice.escrow_contract_updated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: invoice.escrow_contract_updated"
    );
}

#[test]
fn registry_contract_initialized_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        registry_events::contract_initialized(&env, &admin)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.contract_initialized");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.contract_initialized"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.contract_initialized"
    );
}

#[test]
fn registry_issuer_registered_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let address = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        registry_events::issuer_registered(&env, &address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.issuer_registered");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.issuer_registered"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.issuer_registered"
    );
}

#[test]
fn registry_buyer_registered_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let address = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        registry_events::buyer_registered(&env, &address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.buyer_registered");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.buyer_registered"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.buyer_registered"
    );
}

#[test]
fn registry_address_revoked_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let address = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        registry_events::address_revoked(&env, &address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.address_revoked");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.address_revoked"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.address_revoked"
    );
}

#[test]
fn registry_batch_registered_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let registered = 101u32;
    let skipped = 102u32;
    env.as_contract(&contract, || {
        registry_events::batch_registered(&env, registered, skipped)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.batch_registered");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.batch_registered"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.batch_registered"
    );
}

#[test]
fn registry_profile_verified_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let address = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let status = true;
    env.as_contract(&contract, || {
        registry_events::profile_verified(&env, &address, status)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.profile_verified");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.profile_verified"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.profile_verified"
    );
}

#[test]
fn registry_profile_updated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let address = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        registry_events::profile_updated(&env, &address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.profile_updated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.profile_updated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.profile_updated"
    );
}

#[test]
fn registry_address_reinstated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let address = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        registry_events::address_reinstated(&env, &address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.address_reinstated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.address_reinstated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.address_reinstated"
    );
}

#[test]
fn registry_ownership_transferred_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old_admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new_admin = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        registry_events::ownership_transferred(&env, &old_admin, &new_admin)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.ownership_transferred");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.ownership_transferred"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.ownership_transferred"
    );
}

#[test]
fn registry_admin_transferred_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old_admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new_admin = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        registry_events::admin_transferred(&env, &old_admin, &new_admin)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.admin_transferred");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.admin_transferred"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.admin_transferred"
    );
}

#[test]
fn registry_paused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || registry_events::paused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.paused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.paused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.paused"
    );
}

#[test]
fn registry_unpaused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || registry_events::unpaused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("registry.unpaused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: registry.unpaused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: registry.unpaused"
    );
}

#[test]
fn pool_pool_initialized_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let invoice_contract = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let escrow_contract = Address::from_string(&String::from_str(
        &env,
        "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3",
    ));
    let usdc_asset = Address::from_string(&String::from_str(
        &env,
        "CACAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAINCW",
    ));
    env.as_contract(&contract, || {
        pool_events::pool_initialized(
            &env,
            &admin,
            &invoice_contract,
            &escrow_contract,
            &usdc_asset,
        )
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.pool_initialized");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.pool_initialized"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.pool_initialized"
    );
}

#[test]
fn pool_lp_deposited_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let lp = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let usdc_amount = 18446744073709551635u128;
    let shares_issued = 18446744073709551636u128;
    env.as_contract(&contract, || {
        pool_events::lp_deposited(&env, &lp, usdc_amount, shares_issued)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.lp_deposited");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.lp_deposited"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.lp_deposited"
    );
}

#[test]
fn pool_lp_withdrawn_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let lp = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let usdc_amount = 18446744073709551635u128;
    let shares_burned = 18446744073709551636u128;
    env.as_contract(&contract, || {
        pool_events::lp_withdrawn(&env, &lp, usdc_amount, shares_burned)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.lp_withdrawn");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.lp_withdrawn"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.lp_withdrawn"
    );
}

#[test]
fn pool_shares_transferred_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let from = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let to = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let amount = 18446744073709551636i128;
    env.as_contract(&contract, || {
        pool_events::shares_transferred(&env, &from, &to, amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.shares_transferred");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.shares_transferred"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.shares_transferred"
    );
}

#[test]
fn pool_invoice_funded_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let funded_amount = 18446744073709551635u128;
    env.as_contract(&contract, || {
        pool_events::invoice_funded(&env, &invoice_id, funded_amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.invoice_funded");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.invoice_funded"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.invoice_funded"
    );
}

#[test]
fn pool_repayment_received_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let amount = 18446744073709551635u128;
    let lp_yield = 18446744073709551636u128;
    let protocol_cut = 18446744073709551637u128;
    env.as_contract(&contract, || {
        pool_events::repayment_received(&env, &invoice_id, amount, lp_yield, protocol_cut)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.repayment_received");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.repayment_received"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.repayment_received"
    );
}

#[test]
fn pool_invoice_defaulted_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let loss_amount = 18446744073709551635u128;
    env.as_contract(&contract, || {
        pool_events::invoice_defaulted(&env, &invoice_id, loss_amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.invoice_defaulted");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.invoice_defaulted"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.invoice_defaulted"
    );
}

#[test]
fn pool_max_utilization_updated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old_cap_bps = 101u32;
    let new_cap_bps = 102u32;
    env.as_contract(&contract, || {
        pool_events::max_utilization_updated(&env, old_cap_bps, new_cap_bps)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.max_utilization_updated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.max_utilization_updated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.max_utilization_updated"
    );
}

#[test]
fn pool_ownership_transferred_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old_admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new_admin = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        pool_events::ownership_transferred(&env, &old_admin, &new_admin)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.ownership_transferred");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.ownership_transferred"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.ownership_transferred"
    );
}

#[test]
fn pool_paused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || pool_events::paused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.paused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.paused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.paused"
    );
}

#[test]
fn pool_unpaused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || pool_events::unpaused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.unpaused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.unpaused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.unpaused"
    );
}

#[test]
fn pool_protocol_fee_updated_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old_fee_bps = 101u32;
    let new_fee_bps = 102u32;
    let treasury = Address::from_string(&String::from_str(
        &env,
        "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3",
    ));
    env.as_contract(&contract, || {
        pool_events::protocol_fee_updated(&env, old_fee_bps, new_fee_bps, &treasury)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.protocol_fee_updated");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.protocol_fee_updated"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.protocol_fee_updated"
    );
}

#[test]
fn pool_allowance_approved_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let from = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let spender = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let amount = 18446744073709551636i128;
    let expiration_ledger = 104u32;
    env.as_contract(&contract, || {
        pool_events::allowance_approved(&env, &from, &spender, amount, expiration_ledger)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool.allowance_approved");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool.allowance_approved"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool.allowance_approved"
    );
}

#[test]
fn escrow_upgraded_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let wasm_hash = BytesN::<32>::from_array(&env, &[17; 32]);
    env.as_contract(&contract, || escrow_events::upgraded(&env, &wasm_hash));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.upgraded");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.upgraded"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.upgraded"
    );
}

#[test]
fn escrow_funds_locked_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let issuer = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let amount = 18446744073709551636u128;
    env.as_contract(&contract, || {
        escrow_events::funds_locked(&env, &invoice_id, &issuer, amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.funds_locked");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.funds_locked"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.funds_locked"
    );
}

#[test]
fn escrow_released_to_issuer_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let issuer = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let amount = 18446744073709551636u128;
    env.as_contract(&contract, || {
        escrow_events::released_to_issuer(&env, &invoice_id, &issuer, amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.released_to_issuer");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.released_to_issuer"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.released_to_issuer"
    );
}

#[test]
fn escrow_released_to_pool_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let pool = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let amount = 18446744073709551636u128;
    env.as_contract(&contract, || {
        escrow_events::released_to_pool(&env, &invoice_id, &pool, amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.released_to_pool");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.released_to_pool"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.released_to_pool"
    );
}

#[test]
fn escrow_default_resolved_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let invoice_id = BytesN::<32>::from_array(&env, &[17; 32]);
    let pool = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    let caller = Address::from_string(&String::from_str(
        &env,
        "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3",
    ));
    let amount = 18446744073709551637u128;
    env.as_contract(&contract, || {
        escrow_events::default_resolved(&env, &invoice_id, &pool, &caller, amount)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.default_resolved");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.default_resolved"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.default_resolved"
    );
}

#[test]
fn escrow_ownership_transferred_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let old_admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let new_admin = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        escrow_events::ownership_transferred(&env, &old_admin, &new_admin)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.ownership_transferred");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.ownership_transferred"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.ownership_transferred"
    );
}

#[test]
fn escrow_paused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || escrow_events::paused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.paused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.paused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.paused"
    );
}

#[test]
fn escrow_unpaused_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || escrow_events::unpaused(&env, &admin));
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("escrow.unpaused");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: escrow.unpaused"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: escrow.unpaused"
    );
}

#[test]
fn pool_factory_contract_initialized_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let admin = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    env.as_contract(&contract, || {
        pool_factory_events::contract_initialized(&env, &admin)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool_factory.contract_initialized");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool_factory.contract_initialized"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool_factory.contract_initialized"
    );
}

#[test]
fn pool_factory_pool_instance_created_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let asset = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let pool_address = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        pool_factory_events::pool_instance_created(&env, &asset, &pool_address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool_factory.pool_instance_created");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool_factory.pool_instance_created"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool_factory.pool_instance_created"
    );
}

#[test]
fn pool_factory_existing_pool_registered_golden_xdr() {
    let env = Env::default();
    let contract = env.register_contract(None, EventHarness);
    let asset = Address::from_string(&String::from_str(
        &env,
        "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526",
    ));
    let pool_address = Address::from_string(&String::from_str(
        &env,
        "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ",
    ));
    env.as_contract(&contract, || {
        pool_factory_events::existing_pool_registered(&env, &asset, &pool_address)
    });
    let events = env.events().all();
    assert_eq!(events.len(), 1);
    let (emitter, topics, data) = events.get(0).unwrap();
    assert_eq!(emitter, contract);
    let (golden_topics, golden_data) = fixture("pool_factory.existing_pool_registered");
    assert_eq!(
        hex(topics.to_xdr(&env)),
        golden_topics,
        "topic schema changed: pool_factory.existing_pool_registered"
    );
    assert_eq!(
        hex(data.to_xdr(&env)),
        golden_data,
        "data schema changed: pool_factory.existing_pool_registered"
    );
}
