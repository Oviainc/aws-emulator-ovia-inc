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
//! Each case drives the real router and reads the record back beside the store
//! the call wrote. Nothing is written to disk.
//!
//! The records are read through the ONE `tracing` capture this test binary
//! installs ([`access_records`]), which is GLOBAL rather than scoped to the
//! calling thread on purpose: `tracing` decides per CALLSITE whether any
//! subscriber is interested and caches that decision process-wide, and the test
//! harness runs this binary's suites in parallel, so a thread-scoped capture
//! (`subscriber::set_default`) sees nothing whenever a sibling test — holding no
//! subscriber of its own — reached the handler's callsite first. One global
//! capture plus an interest-cache rebuild removes that ordering dependency.
//!
//! What keeps one case's records out of another's assertions — and out of
//! another's drain, since every suite here runs in parallel over the same
//! operation — is the exemption type each case submits: a token no other case
//! and no other suite names ([`case_type`]).
//!
//! The harness (`emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs` and the submitted certificate in
//! `tax_exemptions.rs`; nothing here builds a second copy of either.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

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

/// The most `accountIds` entries one submission may name — the published
/// `@length` maximum of `PutTaxExemption`'s list
/// (`smithy_query(query="operation", operation="PutTaxExemption")`).
const MAX_ACCOUNT_IDS: usize = 550;

/// The exemption type one case of one test submits: a token naming them both.
///
/// `PutTaxExemptionRequest$exemptionType` is a free-form string the model
/// constrains only in length (1..200) and by a pattern admitting every character,
/// and it enumerates no legal values for it
/// (`smithy_query(query="operation", operation="PutTaxExemption")`), so such a
/// token is admitted and recorded exactly as a published entry's name is —
/// `tests/all/exemption_type_admission.rs` pins that rule, and nothing in the
/// rule under test here is about the type. Submitting a token no other case and
/// no other suite names is what makes the records a submission left its own in
/// the capture this whole test binary shares.
fn case_type(test: &str, case: &str) -> String {
    format!("{test} / {case}")
}

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
    /// The `PutTaxExemption` access records naming `exemption_type`, TAKEN from
    /// the buffer and leaving every other case's and every other suite's records
    /// in it.
    ///
    /// A case submits a type no other case and no other suite submits
    /// ([`case_type`]), so the records of a sibling running in parallel over the
    /// same operation are neither counted here nor discarded from under it —
    /// which is what lets an assertion demand EXACTLY one record per submission.
    fn drain_records_for(&self, exemption_type: &str) -> Vec<CapturedEvent> {
        let mut held = self.events.lock().expect("the capture buffer");
        let (mine, theirs): (Vec<CapturedEvent>, Vec<CapturedEvent>) =
            std::mem::take(&mut *held).into_iter().partition(|event| {
                event.field("operation") == Some("PutTaxExemption")
                    && event.field("exemption_type") == Some(exemption_type)
            });
        *held = theirs;
        mine
    }

    /// The one access record the submission of `exemption_type` left, or an
    /// explanation of what was captured instead.
    fn the_exemption_record(&self, case: &str, exemption_type: &str) -> CapturedEvent {
        let matching = self.drain_records_for(exemption_type);
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one access record for {exemption_type:?}, captured \
             {matching:?}"
        );
        matching.into_iter().next().expect("the one record")
    }
}

impl Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // The access records this suite reads are emitted at INFO; nothing finer
        // belongs to an assertion here.
        *metadata.level() <= tracing::Level::INFO
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

/// The capture for the access records, installed ONCE as this test binary's
/// global `tracing` subscriber.
///
/// A test binary admits exactly one global default, so this module owns it: a
/// suite that wants captured records calls this rather than installing another,
/// which `set_global_default` would refuse.
fn access_records() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone())
            .expect("this module is the only installer of a global tracing subscriber");
        // Recompute interest for callsites a test running in parallel may
        // already have reached — and cached as uninteresting — before this
        // subscriber existed.
        tracing::callsite::rebuild_interest_cache();
        capture
    })
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
    let capture = access_records();
    for row in rows() {
        let case = row.case.as_str();
        let exemption_type = case_type(
            "the_access_record_counts_the_accounts_the_write_addressed",
            case,
        );
        let (router, engine) = emulator_with_engine();
        let submitted = call(
            &router,
            "PutTaxExemption",
            &submission(&row.named, &exemption_type),
        )
        .await;

        assert_eq!(
            (submitted.status, submitted.error_code.as_str()),
            (200, ""),
            "case {case:?}: the submission must succeed, got {submitted:?}"
        );
        let record = capture.the_exemption_record(case, &exemption_type);
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
            Some(exemption_type.as_str()),
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
    let capture = access_records();
    for row in rows() {
        let case = row.case.as_str();
        let exemption_type = case_type("every_submission_records_the_accounts_it_addressed", case);
        let (router, engine) = emulator_with_engine();

        // The same exemption submitted twice REPLACES each addressed account's
        // record: the second record reports the same accounts, and the store
        // still holds one record each.
        for attempt in 1..=2 {
            let submitted = call(
                &router,
                "PutTaxExemption",
                &submission(&row.named, &exemption_type),
            )
            .await;
            assert_eq!(
                (submitted.status, submitted.error_code.as_str()),
                (200, ""),
                "case {case:?}: submission {attempt} must succeed, got {submitted:?}"
            );
            let record = capture.the_exemption_record(case, &exemption_type);
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
        let other_type = format!("{exemption_type} / a second exemption");
        let other = call(
            &router,
            "PutTaxExemption",
            &submission(&row.named, &other_type),
        )
        .await;
        assert_eq!(
            (other.status, other.error_code.as_str()),
            (200, ""),
            "case {case:?}: the second exemption must succeed, got {other:?}"
        );
        let record = capture.the_exemption_record(case, &other_type);
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
    const TEST: &str = "a_refused_submission_leaves_no_record_of_a_change";
    let capture = access_records();
    let (router, engine) = emulator_with_engine();

    // Fill the store to just under the bound this emulator keeps resident, one
    // exemption per account — in submissions of at most the published maximum
    // number of entries each. Each applied submission leaves its one record,
    // which is also what shows the capture these assertions read is live: the
    // refusal below is read through the same one.
    let fill_type = case_type(TEST, "filling the store");
    let resident: Vec<String> = (0..MAX_STORED_EXEMPTIONS - 1)
        .map(|index| format!("{:012}", index + 1))
        .collect();
    let mut applied = 0usize;
    let mut recorded = 0usize;
    for chunk in resident.chunks(MAX_ACCOUNT_IDS) {
        let filled = call(&router, "PutTaxExemption", &submission(chunk, &fill_type)).await;
        assert_eq!(
            (filled.status, filled.error_code.as_str()),
            (200, ""),
            "setup: filling the store must succeed, got {filled:?}"
        );
        applied += 1;
        recorded += capture.drain_records_for(&fill_type).len();
    }
    assert_eq!(
        recorded, applied,
        "setup: each of the {applied} applied submissions leaves one access record, captured \
         {recorded}"
    );

    // Two further accounts: the write would take the store one record past the
    // bound, so it is refused whole.
    let refused_type = case_type(TEST, "a submission past the resident bound");
    let overflowing: Vec<String> = (0..2)
        .map(|index| format!("{:012}", MAX_STORED_EXEMPTIONS + index + 1))
        .collect();
    let refused = call(
        &router,
        "PutTaxExemption",
        &submission(&overflowing, &refused_type),
    )
    .await;
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
    let records = capture.drain_records_for(&refused_type);
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

/// RULE, on the harness every case above reads its records through: the record a
/// state-changing call leaves is captured whatever thread the call ran on, so
/// what a case reads back does not depend on which test in this binary reached
/// the handler's callsite first — every suite here runs in parallel over the same
/// operation.
#[tokio::test]
async fn an_access_record_is_captured_from_a_call_on_another_thread() {
    const CASE: &str = "a submission driven from another thread";
    let capture = access_records();
    let exemption_type = case_type(
        "an_access_record_is_captured_from_a_call_on_another_thread",
        CASE,
    );
    // Three entries naming two accounts, so the record reports the accounts the
    // write addressed on this path too.
    let named = vec![A.to_string(), B.to_string(), A.to_string()];

    // A thread of its own, driving its own runtime: nothing of this capture is
    // installed there, and the record it leaves is read back here.
    let submitting = std::thread::spawn({
        let exemption_type = exemption_type.clone();
        move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a runtime for the submitting thread");
            runtime.block_on(async move {
                let (router, _engine) = emulator_with_engine();
                call(
                    &router,
                    "PutTaxExemption",
                    &submission(&named, &exemption_type),
                )
                .await
            })
        }
    });
    let submitted = submitting.join().expect("the submitting thread");
    assert_eq!(
        (submitted.status, submitted.error_code.as_str()),
        (200, ""),
        "case {CASE:?}: the submission must succeed, got {submitted:?}"
    );

    let record = capture.the_exemption_record(CASE, &exemption_type);
    assert_eq!(
        record.field("accounts"),
        Some("2"),
        "case {CASE:?}: expected the record to report the two accounts the three entries \
         addressed, got {record:?}"
    );
    assert_eq!(
        record.field("case_id"),
        submitted.get("caseId").as_str(),
        "case {CASE:?}: expected the record to name the case the submission opened ({:?}), got \
         {record:?}",
        submitted.get("caseId")
    );
}
