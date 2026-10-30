// SPDX-License-Identifier: CC0-1.0

//! The access record a `PutTaxExemption` submission leaves, and the accounts it
//! reports.
//!
//! RULE: the operator-facing access record a state-changing call emits reports
//! the accounts the WRITE addressed — each named id once, as
//! `state::distinct_account_ids` defines the accounts an `accountIds` list
//! addresses — so the count it carries is the number of accounts the store then
//! holds the submitted exemption for, whatever number of times the request
//! repeated an id; and a submission the store refuses leaves no record claiming
//! it changed anything.
//!
//! The rows span what the rule generalises over: a list with no repeat, repeats
//! adjacent, separated and interleaved, a list filled by a single id, how many
//! times an id repeats, how many accounts the list names, and the published
//! `@length` maximum of `accountIds` (1..=550 —
//! `smithy_query(query="operation", operation="PutTaxExemption")`) reached both
//! by one repeated id and by 550 distinct ones.
//!
//! Each case installs a capturing `tracing` subscriber scoped to the calling
//! thread, drives the real router, and reads the record back beside the store the
//! call wrote. Nothing is written to disk and no global subscriber is installed.
//!
//! The harness (`emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs` and the submitted certificate in
//! `tax_exemptions.rs`; nothing here builds a second copy of either.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, MAX_STORED_EXEMPTIONS,
};

use crate::tax_exemptions::certificate;
use crate::tax_registration_lifecycle::{call, emulator_with_engine};

/// Five distinct 12-digit account ids the hand-written rows draw their lists
/// from.
const A: &str = "000000000001";
const B: &str = "000000000002";
const C: &str = "000000000003";
const D: &str = "000000000004";
const E: &str = "000000000005";

/// An entry of the catalog this service publishes, named by its `displayName`
/// (`tests/all/tax_exemption_types.rs` pins the catalog's wire form).
const EXEMPTION_TYPE: &str = "501(c)(3)";

/// A second published entry, for the submission that ADDS a record beside the
/// one an account already holds rather than replacing it.
const OTHER_EXEMPTION_TYPE: &str = "Reseller";

/// The most `accountIds` entries one submission may name — the published
/// `@length` maximum of `PutTaxExemption`'s list
/// (`smithy_query(query="operation", operation="PutTaxExemption")`).
const MAX_ACCOUNT_IDS: usize = 550;

/// One captured event: its fields, rendered as strings whatever type they were
/// recorded as.
#[derive(Clone, Debug)]
struct CapturedEvent {
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Collects an event's fields into strings.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps every event it is handed, so a case can read
/// the access records back.
#[derive(Clone, Default)]
struct Capture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Capture {
    /// Every captured record for `operation`, oldest first.
    fn records_for(&self, operation: &str) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture buffer")
            .iter()
            .filter(|event| event.field("operation") == Some(operation))
            .cloned()
            .collect()
    }

    /// The one access record `PutTaxExemption` left, or an explanation of what
    /// was captured instead.
    fn the_exemption_record(&self, case: &str) -> CapturedEvent {
        let matching = self.records_for("PutTaxExemption");
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one access record, captured {matching:?}"
        );
        matching.into_iter().next().expect("the one record")
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // These cases assert on events, not spans; one id for every span is
        // enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        self.events
            .lock()
            .expect("the capture buffer")
            .push(CapturedEvent { fields });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Capture this thread's `tracing` events for the life of the returned guard.
fn capture_events() -> (Capture, tracing::subscriber::DefaultGuard) {
    let capture = Capture::default();
    let guard = tracing::subscriber::set_default(capture.clone());
    (capture, guard)
}

/// One row: the list a submission names, and how many accounts it addresses —
/// each named id once.
struct Row {
    case: String,
    named: Vec<String>,
    addressed: usize,
}

/// `count` distinct 12-digit account ids, each repeated `repeats` times, named
/// highest id first so a walk of a sorted set rather than of the request is
/// visible.
fn generated(case: &str, count: usize, repeats: usize) -> Row {
    let mut named = Vec::with_capacity(count * repeats);
    for index in (0..count).rev() {
        for _ in 0..repeats {
            named.push(format!("{:012}", index + 1));
        }
    }
    Row {
        case: case.to_string(),
        named,
        addressed: count,
    }
}

/// A row naming `named`, which addresses `addressed` accounts.
fn row(case: &str, named: &[&str], addressed: usize) -> Row {
    Row {
        case: case.to_string(),
        named: named.iter().map(|id| (*id).to_string()).collect(),
        addressed,
    }
}

/// The rows: the hand-written shapes a repeat can take, and generated lists that
/// vary how many accounts a submission names and how often it repeats one, up to
/// the published maximum of 550 entries.
fn rows() -> Vec<Row> {
    vec![
        row("a single id, nothing repeated", &[C], 1),
        row("distinct ids, named out of sorted order", &[C, A, E, B], 4),
        row("an adjacent repeat ahead of a smaller id", &[B, B, A], 2),
        row("a repeat separated by another account", &[D, A, D], 2),
        row("one id filling a five-entry list", &[E, E, E, E, E], 1),
        row("two ids interleaved ahead of a third", &[D, B, D, B, A], 3),
        row("repeats at both ends of the list", &[C, A, A, A, C], 2),
        generated("sixty accounts, each named once", 60, 1),
        generated("sixty accounts, each named three times", 60, 3),
        generated(
            "the published maximum of entries, one account",
            1,
            MAX_ACCOUNT_IDS,
        ),
        generated(
            "the published maximum of entries, all distinct",
            MAX_ACCOUNT_IDS,
            1,
        ),
    ]
}

/// The body of a submission naming `named` with `exemption_type`.
fn submission(named: &[String], exemption_type: &str) -> Value {
    json!({
        "accountIds": named,
        "authority": { "country": "US" },
        "exemptionType": exemption_type,
        "exemptionCertificate": certificate("exemption.pdf"),
    })
}

/// The accounts this service holds exemption records for, and how many records
/// each holds.
fn held(engine: &Arc<engine::InMemoryEngine>) -> Vec<(String, usize)> {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let store = state.lock();
    store
        .exemptions
        .iter()
        .map(|(account_id, records)| (account_id.clone(), records.len()))
        .collect()
}

/// RULE: the access record reports the accounts the write addressed — the count
/// is the number of accounts the store then holds a record for, never the number
/// of entries the request listed.
#[tokio::test]
async fn the_access_record_counts_the_accounts_the_write_addressed() {
    for row in rows() {
        let case = row.case.as_str();
        let (router, engine) = emulator_with_engine();
        let (capture, guard) = capture_events();
        let submitted = call(
            &router,
            "PutTaxExemption",
            &submission(&row.named, EXEMPTION_TYPE),
        )
        .await;
        drop(guard);

        assert_eq!(
            (submitted.status, submitted.error_code.as_str()),
            (200, ""),
            "case {case:?}: the submission must succeed, got {submitted:?}"
        );
        let record = capture.the_exemption_record(case);
        assert_eq!(
            record.field("outcome"),
            Some("recorded"),
            "case {case:?}: expected a recorded outcome, got {record:?}"
        );
        assert_eq!(
            record.field("accounts"),
            Some(row.addressed.to_string().as_str()),
            "case {case:?}: expected the record to report the {} accounts the submission \
             addressed out of {} entries, got {record:?}",
            row.addressed,
            row.named.len()
        );
        assert_eq!(
            record.field("case_id"),
            submitted.get("caseId").as_str(),
            "case {case:?}: expected the record to name the case the submission opened ({:?}), \
             got {record:?}",
            submitted.get("caseId")
        );
        assert_eq!(
            record.field("exemption_type"),
            Some(EXEMPTION_TYPE),
            "case {case:?}: expected the record to name the submitted type, got {record:?}"
        );

        // The store the record describes: one exemption for each addressed
        // account, so the count the record carries is the store's own.
        let stored = held(&engine);
        assert_eq!(
            stored.len(),
            row.addressed,
            "case {case:?}: expected one addressed account per record, got {stored:?}"
        );
        assert!(
            stored.iter().all(|(_, records)| *records == 1),
            "case {case:?}: expected exactly one exemption record per addressed account, got \
             {stored:?}"
        );
    }
}

/// RULE, on the writes that REPLACE and that ADD: every submission's record
/// reports the accounts that submission addressed, and the store holds what the
/// records say it does.
#[tokio::test]
async fn every_submission_records_the_accounts_it_addressed() {
    for row in rows() {
        let case = row.case.as_str();
        let (router, engine) = emulator_with_engine();

        // The same exemption submitted twice REPLACES each addressed account's
        // record: the second record reports the same accounts, and the store
        // still holds one record each.
        for attempt in 1..=2 {
            let (capture, guard) = capture_events();
            let submitted = call(
                &router,
                "PutTaxExemption",
                &submission(&row.named, EXEMPTION_TYPE),
            )
            .await;
            drop(guard);
            assert_eq!(
                (submitted.status, submitted.error_code.as_str()),
                (200, ""),
                "case {case:?}: submission {attempt} must succeed, got {submitted:?}"
            );
            let record = capture.the_exemption_record(case);
            assert_eq!(
                record.field("accounts"),
                Some(row.addressed.to_string().as_str()),
                "case {case:?}: submission {attempt} addressed {} accounts, got {record:?}",
                row.addressed
            );
        }
        let stored = held(&engine);
        assert!(
            stored.len() == row.addressed && stored.iter().all(|(_, records)| *records == 1),
            "case {case:?}: a replacing submission adds no record, got {stored:?}"
        );

        // A DIFFERENT exemption for the same accounts ADDS a record to each:
        // the count the record reports is still the accounts addressed, not the
        // records written. A row addressing more than half the records this
        // emulator keeps resident cannot hold a second exemption for every one
        // of its accounts, so it is left at the replacement above.
        if row.addressed * 2 > MAX_STORED_EXEMPTIONS {
            continue;
        }
        let (capture, guard) = capture_events();
        let other = call(
            &router,
            "PutTaxExemption",
            &submission(&row.named, OTHER_EXEMPTION_TYPE),
        )
        .await;
        drop(guard);
        assert_eq!(
            (other.status, other.error_code.as_str()),
            (200, ""),
            "case {case:?}: the second exemption must succeed, got {other:?}"
        );
        let record = capture.the_exemption_record(case);
        assert_eq!(
            record.field("accounts"),
            Some(row.addressed.to_string().as_str()),
            "case {case:?}: the second exemption addressed {} accounts, got {record:?}",
            row.addressed
        );
        let stored = held(&engine);
        assert!(
            stored.len() == row.addressed && stored.iter().all(|(_, records)| *records == 2),
            "case {case:?}: expected two exemption records per addressed account, got {stored:?}"
        );
    }
}

/// RULE, on the refusal path: a submission the store refuses writes nothing, so
/// it leaves no access record claiming it changed accounts.
#[tokio::test]
async fn a_refused_submission_leaves_no_record_of_a_change() {
    let (router, engine) = emulator_with_engine();

    // Fill the store to just under the bound this emulator keeps resident, one
    // exemption per account — in submissions of at most the published maximum
    // number of entries each.
    let resident: Vec<String> = (0..MAX_STORED_EXEMPTIONS - 1)
        .map(|index| format!("{:012}", index + 1))
        .collect();
    for chunk in resident.chunks(MAX_ACCOUNT_IDS) {
        let filled = call(&router, "PutTaxExemption", &submission(chunk, EXEMPTION_TYPE)).await;
        assert_eq!(
            (filled.status, filled.error_code.as_str()),
            (200, ""),
            "setup: filling the store must succeed, got {filled:?}"
        );
    }

    // Two further accounts: the write would take the store one record past the
    // bound, so it is refused whole.
    let overflowing: Vec<String> = (0..2)
        .map(|index| format!("{:012}", MAX_STORED_EXEMPTIONS + index + 1))
        .collect();
    let (capture, guard) = capture_events();
    let refused = call(
        &router,
        "PutTaxExemption",
        &submission(&overflowing, EXEMPTION_TYPE),
    )
    .await;
    drop(guard);
    assert_eq!(
        refused.status, 413,
        "case \"a submission past the resident bound\": expected the modelled refusal, got \
         {refused:?}"
    );
    assert!(
        refused
            .error_code
            .contains("CaseCreationLimitExceededException"),
        "case \"a submission past the resident bound\": expected \
         CaseCreationLimitExceededException, got {refused:?}"
    );
    let records = capture.records_for("PutTaxExemption");
    assert!(
        records.is_empty(),
        "case \"a submission past the resident bound\": a refused submission changed nothing, so \
         it may leave no record, got {records:?}"
    );
    let stored = held(&engine);
    assert_eq!(
        stored.len(),
        MAX_STORED_EXEMPTIONS - 1,
        "case \"a submission past the resident bound\": a refused submission stores nothing, got \
         {} accounts",
        stored.len()
    );
}
