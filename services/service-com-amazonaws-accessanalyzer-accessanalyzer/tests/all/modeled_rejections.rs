// SPDX-License-Identifier: CC0-1.0

//! `CreateServiceLinkedAnalyzer`'s authorization gate, and the access record every
//! handler in this service leaves behind.
//!
//! Every case goes through the real router (the shared wire harness in
//! [`super::harness`]), so routing, boundary constraint validation, the handler and the
//! response serialization all run as they do in the emulator binary.
//!
//! The first table spans the class its rule generalises over — every `Type` the model
//! accepts, and the optional members a create may carry — with the recorded
//! `type=ACCOUNT` call as one row. The second pins that every call these handlers answer
//! leaves one structured, attributable record on the `emulator::audit` target, carrying
//! no caller data.
//!
//! `CheckAccessNotGranted` has its own suite in `tests/all/policy_access_check.rs`,
//! `UpdateAnalyzer` in `tests/all/update_analyzer.rs`, and `CreateAnalyzer` in
//! `tests/all/analyzer_lifecycle.rs`; this suite does not duplicate them.

use serde_json::{json, Value};

use super::analyzer_lifecycle::ANALYZER_TYPES;
use super::harness::{Emulator, Reply};

/// An `unusedAccess` configuration, the variant an `*_UNUSED_ACCESS` analyzer carries:
/// the shape the recorded `CreateServiceLinkedAnalyzer` request carried.
fn unused_access_configuration() -> Value {
    json!({"unusedAccess": {"analysisRule": {"exclusions": [{"resourceTags": [{"k": "v"}]}]}}})
}

/// `PUT /service-linked-analyzer`.
async fn create_service_linked_analyzer(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("PUT", "/service-linked-analyzer", Some(body))
        .await
}

/// The rule: `CreateServiceLinkedAnalyzer` is invokable only by the Amazon Web Services
/// service that manages the analyzer, so a call made with a caller's own credentials is
/// refused with HTTP 403 `AccessDeniedException` and creates nothing.
///
/// Rows differ along every `Type` the model accepts and along the optional members the
/// request may carry (`configuration`, `archiveRules`, `clientToken`); the recorded
/// `type=ACCOUNT` call is the first row.
#[tokio::test]
async fn create_service_linked_analyzer_is_denied_to_a_customer_caller() {
    let emulator = Emulator::new();

    let mut cases: Vec<(String, Value)> = ANALYZER_TYPES
        .iter()
        .map(|analyzer_type| {
            (
                format!("type only: {analyzer_type}"),
                json!({"type": analyzer_type}),
            )
        })
        .collect();
    cases.push((
        "with an unused-access configuration".to_string(),
        json!({"type": "ACCOUNT_UNUSED_ACCESS", "configuration": unused_access_configuration()}),
    ));
    cases.push((
        "with inline archive rules".to_string(),
        json!({
            "type": "ORGANIZATION_UNUSED_ACCESS",
            "archiveRules": [{"ruleName": "r1", "filter": {"resourceType": {"eq": ["AWS::S3::Bucket"]}}}]
        }),
    ));
    cases.push((
        "with a client token".to_string(),
        json!({"type": "ACCOUNT_UNUSED_ACCESS", "clientToken": "token-1"}),
    ));

    for (case, request) in &cases {
        let reply = create_service_linked_analyzer(&emulator, request.clone()).await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: a customer invocation must be refused with HTTP 403, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "AccessDeniedException",
            "case {case:?}: the refusal must be the modelled AccessDeniedException, got {:?}",
            reply.code()
        );
        assert!(
            reply
                .body
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: AccessDeniedException's required message must say that the \
             caller is not authorized, got {:?}",
            reply.body
        );
        assert!(
            reply.member("arn").is_none(),
            "case {case:?}: a refused create returns no analyzer ARN, got {:?}",
            reply.member("arn")
        );
    }

    // Nothing was created: not one service-linked analyzer, under any name.
    let listed = emulator.list_analyzers("").await;
    let analyzers = listed
        .member("analyzers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    assert!(
        analyzers.is_empty(),
        "after {} refused calls no analyzer may exist, got {analyzers:?}",
        cases.len()
    );
}

// ---------------------------------------------------------------------------
// Every call these handlers answer leaves a structured, attributable record
// ---------------------------------------------------------------------------

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than configuring
/// `tracing-subscriber`: what this asserts is one line per event carrying its target and
/// fields, which is a dozen lines of `Visit` here against a custom `MakeWriter` plus a
/// format layer there, and it keeps the capture independent of any formatter's layout.
/// The same shape of harness is used by
/// `services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs`;
/// it stays per-crate because an integration test target cannot import another crate's
/// test module.
#[derive(Clone, Default)]
struct CapturedRecords(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// Every record captured since the last drain, clearing the buffer.
    fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the audit capture lock"))
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling tests run in
/// parallel on other threads — so a thread-local subscriber is raced away as soon as
/// another thread re-evaluates a handler's callsite with no subscriber of its own. A
/// global default is always interested, and each assertion below picks its own records
/// out by the request target it alone uses.
///
/// This accessor is therefore the ONE place this test binary installs a global
/// subscriber: a suite that wants captured records calls it rather than installing
/// another, which `set_global_default` would refuse.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// The captured records on the `emulator::audit` target naming `request_target`.
///
/// The target carries the request path, so a row that updates an analyzer only this test
/// names picks out exactly its own records from a process-wide log sibling tests also
/// write to. The names this test uses are distinct and none is a prefix of another, so
/// matching the field's value is unambiguous wherever the subscriber renders it.
/// The captured records on the `emulator::audit` target that THIS test's calls wrote.
///
/// Every call below signs with [`AUDIT_AKID`], which no other suite uses, so the records
/// of a sibling suite running in parallel on the same routes are not counted here — which
/// is what lets each assertion demand EXACTLY one record per call.
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with("emulator::audit ")
                && record.contains(&format!("access_key_id={AUDIT_AKID}"))
        })
        .cloned()
        .collect()
}

/// The access key id this test signs with: its own, so its records are identifiable.
const AUDIT_AKID: &str = "AKIDAUDITCASE";

/// The rule: every call these handlers answer — above all the state-changing ones —
/// emits ONE structured record on the `emulator::audit` target naming the operation, the
/// identity the request presented, the request target and the outcome, and nothing of
/// the caller's own data.
///
/// A TABLE over the outcomes `UpdateAnalyzer` can report, then the service-linked denial
/// and an evaluated policy check. Every row asserts exactly ONE record for its call: the
/// calls are signed with [`AUDIT_AKID`], so a sibling suite driving the same routes in
/// parallel cannot add to the count.
#[tokio::test]
async fn every_call_emits_a_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_AKID, "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let emulator = Emulator::new();
    let configuration = unused_access_configuration();
    for (case, analyzer_type, name, body, outcome) in [
        (
            "the configuration was replaced",
            "ACCOUNT_UNUSED_ACCESS",
            "audit_updated",
            json!({"configuration": configuration}),
            "analyzer-updated",
        ),
        (
            "the call carried no configuration",
            "ACCOUNT_UNUSED_ACCESS",
            "audit_unchanged",
            json!({}),
            "no-change",
        ),
        (
            "the analyzer's type carries no configuration",
            "ACCOUNT",
            "audit_refused",
            json!({"configuration": unused_access_configuration()}),
            "validation-refused",
        ),
    ] {
        let created = emulator
            .send_as(
                AUDIT_AKID,
                "PUT",
                "/analyzer",
                Some(json!({"analyzerName": name, "type": analyzer_type})),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the analyzer under test must be created, got {created:?}"
        );
        let _ = captured.drain();

        let reply = emulator
            .send_as(AUDIT_AKID, "PUT", &format!("/analyzer/{name}"), Some(body))
            .await;
        let records = audit_records(&captured.drain());
        // One call, one record: a second would mean a branch audited twice.
        assert_eq!(
            records.len(),
            1,
            "case {case:?} (HTTP {}): expected exactly one audit record, got {records:?}",
            reply.status
        );
        let record = &records[0];
        for field in [
            "service=access-analyzer",
            "operation=UpdateAnalyzer",
            &format!("access_key_id={AUDIT_AKID}"),
            &format!("request_target=PUT /analyzer/{name}"),
            &format!("outcome={outcome}"),
        ] {
            assert!(
                record.contains(field),
                "case {case:?}: the audit record must carry {field:?}, got {record:?}"
            );
        }
        // Metadata only: the caller's own configuration never reaches the log.
        assert!(
            !record.contains("unusedAccess") && !record.contains("resourceTags"),
            "case {case:?}: the audit record must stay metadata-only, got {record:?}"
        );
    }

    // The refusal of a service-principal-only operation is attributable too.
    let _ = captured.drain();
    let denied = emulator
        .send_as(
            AUDIT_AKID,
            "PUT",
            "/service-linked-analyzer",
            Some(json!({"type": "ACCOUNT"})),
        )
        .await;
    assert_eq!(
        denied.status, 403,
        "the denial under test must be the modelled 403, got {denied:?}"
    );
    let records = audit_records(&captured.drain());
    // ONE call, ONE record, here as much as on the update rows above.
    assert_eq!(
        records.len(),
        1,
        "case \"the denial is recorded\": expected exactly one audit record, got {records:?}"
    );
    for field in [
        "service=access-analyzer",
        "operation=CreateServiceLinkedAnalyzer",
        &format!("access_key_id={AUDIT_AKID}"),
        "request_target=PUT /service-linked-analyzer",
        "outcome=access-denied",
    ] {
        assert!(
            records[0].contains(field),
            "case \"the denial is recorded\": the record must carry {field:?}, got {:?}",
            records[0]
        );
    }

    // So is an evaluated policy check.
    let _ = captured.drain();
    let check = emulator
        .send_as(
            AUDIT_AKID,
            "POST",
            "/policy/check-access-not-granted",
            Some(json!({
                "policyDocument": r#"{"Statement": [{"Effect": "Allow", "Action": "s3:GetObject",
                                                      "Resource": "*"}]}"#,
                "access": [{"actions": ["s3:DeleteObject"]}],
                "policyType": "IDENTITY_POLICY"
            })),
        )
        .await;
    assert_eq!(
        check.status, 200,
        "the check under test must be evaluated, got {check:?}"
    );
    let records = audit_records(&captured.drain());
    assert_eq!(
        records.len(),
        1,
        "case \"the check is recorded\": expected exactly one audit record, got {records:?}"
    );
    for field in [
        "service=access-analyzer",
        "operation=CheckAccessNotGranted",
        &format!("access_key_id={AUDIT_AKID}"),
        "request_target=POST /policy/check-access-not-granted",
        "outcome=check-evaluated",
    ] {
        assert!(
            records[0].contains(field),
            "case \"the check is recorded\": the record must carry {field:?}, got {:?}",
            records[0]
        );
    }
    // The submitted policy document is sensitive input and never reaches the log.
    assert!(
        !records[0].contains("s3:GetObject"),
        "the audit record must not carry the policy document, got {:?}",
        records[0]
    );
}
