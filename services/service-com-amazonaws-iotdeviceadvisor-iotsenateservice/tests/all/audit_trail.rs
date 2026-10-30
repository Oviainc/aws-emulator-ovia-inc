// SPDX-License-Identifier: CC0-1.0

//! The record a state-changing call leaves behind, over the wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the emulator
//! serves on a socket (the shared harness in [`super::suite_lifecycle`]), so what is
//! asserted is the service's own behaviour: routing, the handler, and the one record the
//! handler writes on the `emulator::audit` target.
//!
//! The rule under test: a mutating call leaves EXACTLY ONE record saying how it ended —
//! including a call the service refused, which is recorded with the wire error code the
//! caller was answered with and with the resource the request NAMED. The table spans the
//! class that rule generalises over: every mutating operation this service serves, and for
//! each one every way its call can end — completed, a no-op, and each refusal it answers
//! (a prose-rule violation, a reused idempotency token, a resource the store does not hold,
//! a value that is not one of this service's ARNs, a per-resource limit, a conflicting run).

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::endpoint::percent_encode;
use super::suite_lifecycle::{thing_arn, Answer, Emulator, ROLE_ARN};

/// The region every call below signs for, so the ARN a record names is predictable.
const REGION: &str = "us-east-1";

/// The access key id these calls present: this suite's own, so the records it reads back
/// are its own and a sibling suite driving the same routes in parallel cannot add to the
/// count a row demands.
const AUDIT_AKID: &str = "AKIDAUDITTRAIL";

/// An id of the modelled `UUID` form (`len 12..36`) that no call below creates.
const UNKNOWN_ID: &str = "11111111-2222-3333-4444-555555555555";

/// `TagMap` is `len 0..50` (source: `smithy_query(query="shape",shape="TagMap")`), so this
/// is how many tags one resource holds at once — the bound the per-resource limit row sits
/// on.
const MAX_TAGS: usize = 50;

/// The status a successful tag mutation answers with: 204 No Content, as AWS answers an
/// empty-output tag mutation — written out here rather than read from the service, so a
/// regression to the protocol default fails these rows.
const TAG_MUTATION_STATUS: u16 = 204;

/// One row of the table below: a call, the answer it must get, and the record it must
/// leave — `(case, method, path, body, status, error code, operation, the resource the
/// record names, the outcome it carries)`.
type Row = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    u16,
    Option<&'static str>,
    &'static str,
    String,
    String,
);

/// A legal suite definition configuration named `name`.
fn configuration(name: &str) -> Value {
    json!({
        "suiteDefinitionName": name,
        "rootGroup": "root-group",
        "devicePermissionRoleArn": ROLE_ARN,
        "devices": [{ "thingArn": thing_arn(1) }],
    })
}

/// A configuration that violates the one rule the model states in prose for `rootGroup`:
/// empty is legal only for a suite intended for qualification.
fn illegal_configuration(name: &str) -> Value {
    json!({
        "suiteDefinitionName": name,
        "rootGroup": "",
        "intendedForQualification": false,
        "devicePermissionRoleArn": ROLE_ARN,
    })
}

/// This service's ARN for a suite definition id.
fn definition_arn(id: &str) -> String {
    iotdeviceadvisor::state::arn(REGION, &format!("suitedefinition/{id}"))
}

/// This service's ARN for a suite run id under a definition.
fn run_arn(definition_id: &str, run_id: &str) -> String {
    iotdeviceadvisor::state::arn(REGION, &format!("suiterun/{definition_id}/{run_id}"))
}

/// The `/tags/{resourceArn}` URI for `arn`.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", percent_encode(arn))
}

// ---------------------------------------------------------------------------
// Reading the records back
// ---------------------------------------------------------------------------

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
///
/// Written against `tracing`'s own `Subscriber` trait rather than configuring
/// `tracing-subscriber`: what is asserted is one line per event carrying its target and
/// fields, and this keeps the capture independent of any formatter's layout. The same shape
/// of harness is used by
/// `services/service-com-amazonaws-accessanalyzer-accessanalyzer/tests/all/modeled_rejections.rs`;
/// it stays per-crate because an integration test target cannot import another crate's test
/// module.
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

/// The capture, installed ONCE as this test binary's global `tracing` subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling suites run in parallel
/// on other threads — so a thread-local subscriber is raced away as soon as another thread
/// re-evaluates a handler's callsite with no subscriber of its own. The records this suite
/// reads are picked out by the access key id only it presents.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// The captured records on the `emulator::audit` target that THIS suite's calls wrote.
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with("emulator::audit ")
                && record.contains(&format!("access_key_id={AUDIT_AKID:?}"))
        })
        .cloned()
        .collect()
}

/// Make one call and hand back its answer together with the ONE record it wrote.
///
/// Asserting the count here is half the rule: a call that wrote none is the gap under test,
/// and a call that wrote two would mean a path audited twice.
async fn audited_call(
    emulator: &Emulator,
    captured: &CapturedRecords,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (Answer, String) {
    let _ = captured.drain();
    let answer = emulator
        .call_as(AUDIT_AKID, method, path, REGION, body)
        .await;
    let records = audit_records(&captured.drain());
    assert_eq!(
        records.len(),
        1,
        "case {case:?} (HTTP {}): expected exactly one audit record, got {records:?}",
        answer.status
    );
    let record = records
        .into_iter()
        .next()
        .expect("the length was just asserted");
    (answer, record)
}

/// What the record of a call must carry: who called, where, which resource, and how the
/// call ended.
fn assert_record(case: &str, record: &str, operation: &str, resource: &str, outcome: &str) {
    for field in [
        format!("operation={operation}"),
        format!("access_key_id={AUDIT_AKID:?}"),
        format!("region={REGION:?}"),
        format!("resource={resource:?}"),
        format!("outcome={outcome}"),
    ] {
        assert!(
            record.contains(&field),
            "case {case:?}: the audit record must carry {field:?}, got {record:?}"
        );
    }
}

/// Put one in-progress run of `definition_id` into the store, its next clock-owed
/// transition far enough out that nothing is due during the rows that use it.
///
/// A FIXTURE, not a behaviour: what the rows below assert is the record a stop and a
/// blocked start leave, and a run minted over the wire moves on the clock
/// (`state::STATUS_TRANSITION`), so the run is seeded in a fixed status rather than racing
/// that span. The real mint is exercised by this suite's own `StartSuiteRun` row.
fn seed_in_progress_run(emulator: &Emulator, definition_id: &str) -> (String, String) {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let mut store = state.lock();
    let id = iotdeviceadvisor::state::new_uuid();
    let arn = run_arn(definition_id, &id);
    let now = std::time::SystemTime::now();
    store.runs.insert(
        id.clone(),
        iotdeviceadvisor::state::SuiteRun {
            id: id.clone(),
            arn: arn.clone(),
            suite_definition_id: definition_id.to_string(),
            suite_definition_version: "v1".to_string(),
            configuration: Default::default(),
            status: "PENDING".to_string(),
            created_at: now,
            started_at: None,
            end_at: None,
            error_reason: None,
            endpoint: iotdeviceadvisor::state::endpoint_for_region(REGION),
            next_transition_at: Some(now + std::time::Duration::from_secs(3600)),
        },
    );
    (id, arn)
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// Every mutating call — the ones the service REFUSES as much as the ones it carries out —
/// leaves exactly one audit record naming the operation, the caller's access key id, the
/// region, the resource the request named and how the call ended.
///
/// One test function rather than several, because the capture is one process-wide buffer: a
/// sibling test draining it in parallel would take another's records.
#[tokio::test]
async fn every_mutating_call_leaves_one_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a missing record below can never be mistaken for a capture that
    // receives nothing: the capture must see an event emitted right here, rendered the way
    // the service renders its own request-derived fields.
    tracing::info!(target: "emulator::audit", access_key_id = ?AUDIT_AKID, "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let emulator = Emulator::new();
    let collection = iotdeviceadvisor::state::definition_collection_arn(REGION);
    let token = "audit-trail-token";
    let create_body = json!({
        "suiteDefinitionConfiguration": configuration("audit-subject"),
        "clientToken": token,
    });

    // CreateSuiteDefinition: the call that mints the definition every row below names.
    let (created, record) = audited_call(
        &emulator,
        captured,
        "a definition is created",
        "POST",
        "/suiteDefinitions",
        Some(create_body.clone()),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"a definition is created\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    let definition_id = created.string("suiteDefinitionId");
    let definition = created.string("suiteDefinitionArn");
    assert_record(
        "a definition is created",
        &record,
        "CreateSuiteDefinition",
        &definition,
        "created",
    );
    // Metadata only: nothing the caller sent as content reaches the record.
    assert!(
        !record.contains("audit-subject") && !record.contains(ROLE_ARN),
        "case \"a definition is created\": the record must stay metadata-only, got {record:?}"
    );

    // (case, method, path, body, expected status, expected error code, operation, resource,
    //  outcome) — one row per way a mutating call of this service can end.
    let rows: Vec<Row> = vec![
        (
            "the same create is retried with its token",
            "POST",
            "/suiteDefinitions".to_string(),
            Some(create_body.clone()),
            200,
            None,
            "CreateSuiteDefinition",
            definition.clone(),
            "idempotent-replay".to_string(),
        ),
        (
            "the token is reused for different parameters",
            "POST",
            "/suiteDefinitions".to_string(),
            Some(json!({
                "suiteDefinitionConfiguration": configuration("audit-subject"),
                "clientToken": token,
                "tags": { "owner": "audit" },
            })),
            400,
            Some("ValidationException"),
            "CreateSuiteDefinition",
            // A create names no definition of its own, so the attempt names the
            // collection it would have created into.
            collection.clone(),
            "refused-ValidationException".to_string(),
        ),
        (
            "a create whose configuration breaks the rootGroup rule",
            "POST",
            "/suiteDefinitions".to_string(),
            Some(json!({
                "suiteDefinitionConfiguration": illegal_configuration("audit-illegal"),
            })),
            400,
            Some("ValidationException"),
            "CreateSuiteDefinition",
            collection.clone(),
            "refused-ValidationException".to_string(),
        ),
        (
            "the definition's configuration is replaced",
            "PATCH",
            format!("/suiteDefinitions/{definition_id}"),
            Some(json!({
                "suiteDefinitionConfiguration": configuration("audit-subject-v2"),
            })),
            200,
            None,
            "UpdateSuiteDefinition",
            definition.clone(),
            "updated".to_string(),
        ),
        (
            "an update whose configuration breaks the rootGroup rule",
            "PATCH",
            format!("/suiteDefinitions/{definition_id}"),
            Some(json!({
                "suiteDefinitionConfiguration": illegal_configuration("audit-illegal"),
            })),
            400,
            Some("ValidationException"),
            "UpdateSuiteDefinition",
            definition.clone(),
            "refused-ValidationException".to_string(),
        ),
        (
            "an update of a definition the store does not hold",
            "PATCH",
            format!("/suiteDefinitions/{UNKNOWN_ID}"),
            Some(json!({
                "suiteDefinitionConfiguration": configuration("audit-absent"),
            })),
            404,
            Some("ResourceNotFoundException"),
            "UpdateSuiteDefinition",
            definition_arn(UNKNOWN_ID),
            "refused-ResourceNotFoundException".to_string(),
        ),
        (
            "a delete of a definition the store does not hold",
            "DELETE",
            format!("/suiteDefinitions/{UNKNOWN_ID}"),
            None,
            200,
            None,
            "DeleteSuiteDefinition",
            definition_arn(UNKNOWN_ID),
            "absent".to_string(),
        ),
        (
            "a tag merge onto the definition",
            "POST",
            tags_uri(&definition),
            Some(json!({ "tags": { "owner": "audit" } })),
            TAG_MUTATION_STATUS,
            None,
            "TagResource",
            definition.clone(),
            "tagged".to_string(),
        ),
        (
            "a removal naming one key the resource carries and one it does not",
            "DELETE",
            format!("{}?tagKeys=owner&tagKeys=absent", tags_uri(&definition)),
            None,
            TAG_MUTATION_STATUS,
            None,
            "UntagResource",
            definition.clone(),
            "untagged 1 of 2 key(s)".to_string(),
        ),
        (
            "a tag merge onto a resource the store does not hold",
            "POST",
            tags_uri(&definition_arn(UNKNOWN_ID)),
            Some(json!({ "tags": { "owner": "audit" } })),
            404,
            Some("ResourceNotFoundException"),
            "TagResource",
            definition_arn(UNKNOWN_ID),
            "refused-ResourceNotFoundException".to_string(),
        ),
        (
            "a tag merge onto a value that is not one of this service's ARNs",
            "POST",
            tags_uri("this-value-is-not-an-arn-at-all"),
            Some(json!({ "tags": { "owner": "audit" } })),
            400,
            Some("ValidationException"),
            "TagResource",
            "this-value-is-not-an-arn-at-all".to_string(),
            "refused-ValidationException".to_string(),
        ),
        (
            "a removal from a resource the store does not hold",
            "DELETE",
            format!("{}?tagKeys=owner", tags_uri(&definition_arn(UNKNOWN_ID))),
            None,
            404,
            Some("ResourceNotFoundException"),
            "UntagResource",
            definition_arn(UNKNOWN_ID),
            "refused-ResourceNotFoundException".to_string(),
        ),
        (
            "a removal from a value that is not one of this service's ARNs",
            "DELETE",
            format!("{}?tagKeys=owner", tags_uri("another-value-that-is-no-arn")),
            None,
            400,
            Some("ValidationException"),
            "UntagResource",
            "another-value-that-is-no-arn".to_string(),
            "refused-ValidationException".to_string(),
        ),
        (
            "a start whose primary device names no device",
            "POST",
            format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            Some(json!({ "suiteRunConfiguration": { "primaryDevice": {} } })),
            400,
            Some("ValidationException"),
            "StartSuiteRun",
            definition.clone(),
            "refused-ValidationException".to_string(),
        ),
        (
            "a start naming a version the definition does not hold",
            "POST",
            format!("/suiteDefinitions/{definition_id}/suiteRuns"),
            Some(json!({
                "suiteDefinitionVersion": "v99",
                "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
            })),
            400,
            Some("ValidationException"),
            "StartSuiteRun",
            definition.clone(),
            "refused-ValidationException".to_string(),
        ),
        (
            "a start on a definition the store does not hold",
            "POST",
            format!("/suiteDefinitions/{UNKNOWN_ID}/suiteRuns"),
            Some(json!({
                "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
            })),
            400,
            Some("ValidationException"),
            "StartSuiteRun",
            definition_arn(UNKNOWN_ID),
            "refused-ValidationException".to_string(),
        ),
        (
            "a stop of a run the store does not hold",
            "POST",
            format!("/suiteDefinitions/{definition_id}/suiteRuns/{UNKNOWN_ID}/stop"),
            None,
            404,
            Some("ResourceNotFoundException"),
            "StopSuiteRun",
            run_arn(&definition_id, UNKNOWN_ID),
            "refused-ResourceNotFoundException".to_string(),
        ),
    ];

    for (case, method, path, body, status, code, operation, resource, outcome) in rows {
        let (answer, record) =
            audited_call(&emulator, captured, case, method, &path, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code:?}, got {} {}",
            answer.status,
            answer.body
        );
        assert_record(case, &record, operation, &resource, &outcome);
    }

    // The per-resource tag limit, on the bound itself: a merge that fits is carried out and
    // recorded as such, and the one key past it is refused — and recorded.
    let full: serde_json::Map<String, Value> = (0..MAX_TAGS)
        .map(|index| (format!("key-{index}"), json!("v")))
        .collect();
    let (answer, record) = audited_call(
        &emulator,
        captured,
        "a merge up to the per-resource tag limit",
        "POST",
        &tags_uri(&definition),
        Some(json!({ "tags": full })),
    )
    .await;
    assert_eq!(
        answer.status, TAG_MUTATION_STATUS,
        "case \"a merge up to the per-resource tag limit\": expected HTTP \
         {TAG_MUTATION_STATUS}, got {} {}",
        answer.status, answer.body
    );
    assert_record(
        "a merge up to the per-resource tag limit",
        &record,
        "TagResource",
        &definition,
        "tagged",
    );
    let (answer, record) = audited_call(
        &emulator,
        captured,
        "a merge one key past the per-resource tag limit",
        "POST",
        &tags_uri(&definition),
        Some(json!({ "tags": { "one-too-many": "v" } })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"a merge one key past the per-resource tag limit\": expected HTTP 400, got {} {}",
        answer.status,
        answer.body
    );
    assert_record(
        "a merge one key past the per-resource tag limit",
        &record,
        "TagResource",
        &definition,
        "refused-ValidationException",
    );

    // StartSuiteRun's own mint, and the conflict a run already in progress answers the next
    // start with.
    let (started, record) = audited_call(
        &emulator,
        captured,
        "a run is started",
        "POST",
        &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
        Some(json!({
            "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"a run is started\": expected HTTP 200, got {} {}",
        started.status, started.body
    );
    assert_record(
        "a run is started",
        &record,
        "StartSuiteRun",
        &started.string("suiteRunArn"),
        "started",
    );

    // A second definition, holding a run that stays in progress, for the rows a run's
    // status decides.
    let busy = emulator
        .call_as(
            AUDIT_AKID,
            "POST",
            "/suiteDefinitions",
            REGION,
            Some(json!({ "suiteDefinitionConfiguration": configuration("audit-busy") })),
        )
        .await;
    assert_eq!(
        busy.status, 200,
        "the second definition must be created, got {} {}",
        busy.status, busy.body
    );
    let busy_id = busy.string("suiteDefinitionId");
    let busy_arn = busy.string("suiteDefinitionArn");
    let (seeded_run_id, seeded_run_arn) = seed_in_progress_run(&emulator, &busy_id);

    let (answer, record) = audited_call(
        &emulator,
        captured,
        "a start while a run of the definition is in progress",
        "POST",
        &format!("/suiteDefinitions/{busy_id}/suiteRuns"),
        Some(json!({
            "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
        })),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        // `ConflictException`'s own `httpError` is 400
        // (`smithy_query(query="shape",shape="ConflictException")`).
        (400, Some("ConflictException")),
        "case \"a start while a run of the definition is in progress\": expected HTTP 400, \
         got {} {}",
        answer.status,
        answer.body
    );
    assert_record(
        "a start while a run of the definition is in progress",
        &record,
        "StartSuiteRun",
        &busy_arn,
        "refused-ConflictException",
    );

    for (case, outcome) in [
        ("the in-progress run is stopped", "stopping"),
        (
            "the same run is stopped again",
            "unchanged-already-stopping-or-finished",
        ),
    ] {
        let (answer, record) = audited_call(
            &emulator,
            captured,
            case,
            "POST",
            &format!("/suiteDefinitions/{busy_id}/suiteRuns/{seeded_run_id}/stop"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        assert_record(case, &record, "StopSuiteRun", &seeded_run_arn, outcome);
    }

    // The destructive call last, so the definition every row above names outlives them.
    let (answer, record) = audited_call(
        &emulator,
        captured,
        "the definition is deleted",
        "DELETE",
        &format!("/suiteDefinitions/{definition_id}"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"the definition is deleted\": expected HTTP 200, got {} {}",
        answer.status, answer.body
    );
    assert_record(
        "the definition is deleted",
        &record,
        "DeleteSuiteDefinition",
        &definition,
        "deleted",
    );
}
