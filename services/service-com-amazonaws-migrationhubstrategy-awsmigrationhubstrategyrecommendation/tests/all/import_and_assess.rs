// SPDX-License-Identifier: CC0-1.0

//! The Strategy Recommendations import-then-assess loop, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted record and serialization — the same path
//! a client takes. The rule under test is the one the service states for its two primary
//! resources: an import file task and an assessment are whatever the start call stored,
//! every reporting operation computes its answer from that record, and an id that names
//! no record is refused with the modelled error rather than answered with empty fields.

use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::COMPLETION_SPAN;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/migrationhub-strategy/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Every value `Condition` admits, from `smithy_query(query="shape",shape="Condition")`.
/// Spelled out here so a case can span the whole catalog rather than the one condition
/// the issue cited (`EQUALS`).
const CONDITION_VALUES: &[&str] = &["CONTAINS", "EQUALS", "NOT_CONTAINS", "NOT_EQUALS"];

/// Every value `AssessmentDataSourceType` admits, from
/// `smithy_query(query="shape",shape="AssessmentDataSourceType")`.
const ASSESSMENT_DATA_SOURCE_TYPES: &[&str] = &[
    "ApplicationDiscoveryService",
    "ManualImport",
    "StrategyRecommendationsApplicationDataCollector",
];

/// Every value `DataSourceType` admits, from
/// `smithy_query(query="shape",shape="DataSourceType")`.
const DATA_SOURCE_TYPES: &[&str] = &[
    "ApplicationDiscoveryService",
    "Import",
    "MPA",
    "StrategyRecommendationsApplicationDataCollector",
];

/// `ImportFileTaskStatus`'s non-terminal import status, and the terminal one the clock
/// owes a task. Spelled out independently of the crate's constants so a wrong status in
/// `src/state.rs` fails here.
const IMPORT_IN_PROGRESS: &str = "ImportInProgress";
const IMPORT_SUCCESS: &str = "ImportSuccess";

/// The `AssessmentStatus` values this slice moves an assessment between, spelled out for
/// the same reason.
const ASSESSMENT_IN_PROGRESS: &str = "IN_PROGRESS";
const ASSESSMENT_COMPLETE: &str = "COMPLETE";
const ASSESSMENT_STOPPED: &str = "STOPPED";

/// `AsyncTaskId`'s `[0-9a-z-:]+` within its `@length(max: 52)`, checked here rather than
/// through any crate predicate so a wrong id shape cannot make this test pass.
fn matches_async_task_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 52
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-' || c == ':')
}

/// One emulator: a router carrying only this service, over one shared engine.
///
/// `pub(crate)` so the suites beside this one drive the same emulator through the same
/// harness rather than standing up a second copy of it.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    pub(crate) fn string(&self, pointer: &str) -> String {
        at(&self.body, pointer)
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "migrationhub-strategy.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

pub(crate) async fn start_import(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/start-import-file-task", Some(request)).await
}

pub(crate) async fn get_import(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-import-file-task/{id}"), None).await
}

pub(crate) async fn start_assessment(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/start-assessment", Some(request)).await
}

pub(crate) async fn get_assessment(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-assessment/{id}"), None).await
}

pub(crate) async fn get_latest_assessment_id(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/get-latest-assessment-id", None).await
}

pub(crate) async fn stop_assessment(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "POST",
        "/stop-assessment",
        Some(json!({"assessmentId": id})),
    )
    .await
}

/// Where a `StartImportFileTask` input member lands in the `GetImportFileTask` answer,
/// as a JSON pointer.
///
/// The table is the crate's `OPERATIONS.md` StartImportFileTask input list read against
/// `GetImportFileTaskResponse`; `None` marks an input member that response does not
/// carry. An unmapped member panics rather than being skipped, so a case cannot quietly
/// assert nothing.
fn reported_as(member: &str) -> Option<&'static str> {
    Some(match member {
        "name" => "/importName",
        "S3Bucket" => "/inputS3Bucket",
        "s3key" => "/inputS3Key",
        "s3bucketForReportData" => "/statusReportS3Bucket",
        // `GetImportFileTaskResponse` carries neither: the source type and the import
        // groups are reported by ListServers / ListApplicationComponents, both stubs.
        "dataSourceType" | "groupId" => return None,
        other => {
            panic!("this test names a StartImportFileTask member with no mapping: {other:?}")
        }
    })
}

/// A start request carrying only the three required members.
///
/// Every bucket name here begins with `migrationhub-strategy-`, the prefix each bucket
/// member's documentation requires; the refusal of a name that does not is a table of
/// its own in `assessment_error_paths.rs`.
pub(crate) fn minimal_import(name: &str) -> Value {
    json!({"name": name, "S3Bucket": "migrationhub-strategy-import", "s3key": "servers.csv"})
}

/// The `StartImportFileTask` requests every import case runs, differing along what the
/// rule generalises over: which required values, whether each optional member is present
/// at all, how many import groups, and which `dataSourceType`.
fn import_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("only the required members", minimal_import("import-1")),
        (
            "a report bucket and a nested key",
            json!({
                "name": "import_2",
                "S3Bucket": "migrationhub-strategy-my-bucket",
                "s3key": "inventory/2026/01/servers.csv",
                "s3bucketForReportData": "migrationhub-strategy-reports",
            }),
        ),
        (
            // `dataSourceType` entry the issue did not cite, from the catalog above.
            "a data source type and one import group",
            json!({
                "name": "a",
                "S3Bucket": "migrationhub-strategy-bucket.with.dots",
                "s3key": "x",
                "dataSourceType": DATA_SOURCE_TYPES[2],
                "groupId": [{"name": "ExternalId", "value": "wave-1"}],
            }),
        ),
        (
            "every optional member, with two import groups",
            json!({
                "name": "IMPORT-4_mixed-case_and-underscores",
                "S3Bucket": "migrationhub-strategy-ab",
                "s3key": "a/".repeat(400) + "servers.csv",
                "s3bucketForReportData": "migrationhub-strategy-r",
                "dataSourceType": DATA_SOURCE_TYPES[1],
                "groupId": [
                    {"name": "ExternalId", "value": "wave-2"},
                    {"name": "ExternalSourceType", "value": "MPA"},
                ],
            }),
        ),
    ]
}

/// `StartImportFileTask` persists what it was given and `GetImportFileTask` reports that
/// record: for every input member the response carries, the reported value is the
/// started one, and the id is fresh for every call.
#[tokio::test]
async fn an_import_file_task_is_reported_from_what_was_stored() {
    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();

    for (case, request) in import_cases() {
        let started_at = Instant::now();
        let started = start_import(&router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartImportFileTask, got {} with {:?}",
            started.status, started.body
        );

        let id = started.string("/id");
        assert!(
            !id.trim().is_empty() && id.len() <= 1024,
            "case {case:?}: expected a non-blank id within 1024 characters, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: every import file task gets a fresh id, but {id:?} was already \
             minted ({seen_ids:?})"
        );
        seen_ids.push(id.clone());

        let reported = get_import(&router, &id).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: expected HTTP 200 from GetImportFileTask, got {} with {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            at(&reported.body, "/id"),
            json!(id),
            "case {case:?}: GetImportFileTask must report the id it was asked about, got {:?}",
            at(&reported.body, "/id")
        );

        for (member, expected) in request.as_object().expect("each case is a JSON object") {
            let Some(pointer) = reported_as(member) else {
                continue;
            };
            let observed = at(&reported.body, pointer);
            assert_eq!(
                &observed, expected,
                "case {case:?}: member {member:?} was stored as {expected:?} but {pointer} \
                 reports {observed:?}"
            );
        }

        // No report bucket was named, so there is none to report.
        if request.get("s3bucketForReportData").is_none() {
            assert_eq!(
                at(&reported.body, "/statusReportS3Bucket"),
                Value::Null,
                "case {case:?}: a task started without s3bucketForReportData reports no \
                 statusReportS3Bucket, got {:?}",
                at(&reported.body, "/statusReportS3Bucket")
            );
        }
        assert!(
            at(&reported.body, "/startTime").is_number(),
            "case {case:?}: every task reports the start time it was stored with, got {:?}",
            at(&reported.body, "/startTime")
        );

        // The completion is owed by the clock: inside the span the task is still
        // running, and its completion members are absent until it is not.
        if started_at.elapsed() < COMPLETION_SPAN {
            assert_eq!(
                at(&reported.body, "/status"),
                json!(IMPORT_IN_PROGRESS),
                "case {case:?}: a task read inside the completion span is still \
                 {IMPORT_IN_PROGRESS}, got {:?}",
                at(&reported.body, "/status")
            );
            for member in ["/completionTime", "/numberOfRecordsSuccess", "/numberOfRecordsFailed"] {
                assert_eq!(
                    at(&reported.body, member),
                    Value::Null,
                    "case {case:?}: a running task reports no {member}, got {:?}",
                    at(&reported.body, member)
                );
            }
        }
    }
}

/// The completion an import file task carries is the one the clock owed it: the status
/// moves to the terminal one once the span has passed, the record counts are reported
/// then (and only then), and the completion time is the same on every later read.
#[tokio::test]
async fn an_import_file_task_completes_when_the_clock_owes_it() {
    let router = emulator();
    for (case, request) in import_cases() {
        let id = start_import(&router, request).await.string("/id");

        // Nothing is done here but wait: no call drives the transition.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

        let settled = get_import(&router, &id).await;
        assert_eq!(
            at(&settled.body, "/status"),
            json!(IMPORT_SUCCESS),
            "case {case:?}: once the completion span has passed the task is {IMPORT_SUCCESS}, \
             got {:?}",
            at(&settled.body, "/status")
        );
        assert!(
            at(&settled.body, "/completionTime").is_number(),
            "case {case:?}: a completed task reports a completion time, got {:?}",
            at(&settled.body, "/completionTime")
        );
        // This emulator serves no S3 data plane, so no import file is read and no
        // records are imported: the counts report zero rather than an invented count.
        assert_eq!(
            at(&settled.body, "/numberOfRecordsSuccess"),
            json!(0),
            "case {case:?}: no import file was read, so no record was imported, got {:?}",
            at(&settled.body, "/numberOfRecordsSuccess")
        );
        assert_eq!(
            at(&settled.body, "/numberOfRecordsFailed"),
            json!(0),
            "case {case:?}: no import file was read, so no record failed, got {:?}",
            at(&settled.body, "/numberOfRecordsFailed")
        );

        let again = get_import(&router, &id).await;
        assert_eq!(
            at(&again.body, "/completionTime"),
            at(&settled.body, "/completionTime"),
            "case {case:?}: the completion time must be stable across reads, got {:?} then {:?}",
            at(&settled.body, "/completionTime"),
            at(&again.body, "/completionTime")
        );
        assert_eq!(
            again.body, settled.body,
            "case {case:?}: reading a task twice must report the same record, got {:?} then {:?}",
            settled.body, again.body
        );
    }
}

/// An import file task id that names no task is a `ResourceNotFoundException` (HTTP
/// 404), never a 200 with empty fields. The rows span the ways an id can miss: unknown
/// shapes, a minted-looking id nothing minted, and a known id in the wrong case.
#[tokio::test]
async fn an_unknown_import_file_task_id_is_refused_with_resource_not_found() {
    let router = emulator();
    let known = start_import(&router, minimal_import("import-1"))
        .await
        .string("/id");

    let cases = vec![
        ("the issue's own missing id", "does-not-exist".to_string()),
        ("a single character", "a".to_string()),
        (
            "an id of the minted shape that was never minted",
            "import-file-task-ffffffffffff".to_string(),
        ),
        ("a known id upper-cased", known.to_uppercase()),
        ("a known id with a suffix", format!("{known}-1")),
    ];

    for (case, id) in cases {
        let answer = get_import(&router, &id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: GetImportFileTask on {id:?} must answer HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {id:?}, got {:?}",
            answer.body
        );
    }

    // The known id is unaffected by every miss above.
    assert_eq!(
        get_import(&router, &known).await.status,
        200,
        "the task that was started is still reported after the refusals"
    );
}

/// The smallest `StartAssessment` request the service accepts: the analysis-data bucket
/// it cannot start without, named with the prefix its documentation requires.
pub(crate) fn minimal_assessment() -> Value {
    json!({"s3bucketForAnalysisData": "migrationhub-strategy-analysis"})
}

/// The `StartAssessment` requests every assessment case runs, differing along what the
/// rule generalises over: how many targets, which condition, how many values per target,
/// which data source type, and whether the optional report bucket is named at all.
fn assessment_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("only the bucket the service requires", minimal_assessment()),
        (
            "the issue's single EQUALS target",
            json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
                "assessmentTargets": [
                    {"condition": "EQUALS", "name": "OS_NAME", "values": ["Linux"]},
                ],
            }),
        ),
        (
            "an empty target list",
            json!({
                "s3bucketForAnalysisData": "migrationhub-strategy-a",
                "assessmentTargets": [],
                "assessmentDataSourceType": ASSESSMENT_DATA_SOURCE_TYPES[1],
            }),
        ),
        (
            // A condition the issue did not cite, from the `Condition` catalog above.
            "one NOT_CONTAINS target with three values",
            json!({
                "assessmentTargets": [
                    {"condition": CONDITION_VALUES[2], "name": "STRATEGY",
                     "values": ["Rehost", "Replatform", "Refactor"]},
                ],
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
            }),
        ),
        (
            "every condition, one target each, with both buckets",
            json!({
                "assessmentTargets": CONDITION_VALUES
                    .iter()
                    .map(|condition| json!({
                        "condition": condition,
                        "name": "SERVER_ID",
                        "values": [format!("server-{condition}")],
                    }))
                    .collect::<Vec<Value>>(),
                "assessmentDataSourceType": ASSESSMENT_DATA_SOURCE_TYPES[2],
                "s3bucketForAnalysisData": "migrationhub-strategy-analysis-data.1",
                "s3bucketForReportData": "migrationhub-strategy-reports",
            }),
        ),
    ]
}

/// `StartAssessment` persists what it was given, `GetLatestAssessmentId` names the most
/// recently started assessment and `GetAssessment` reports that record: the targets come
/// back exactly as supplied and `dataCollectionDetails` is the stored status, counters
/// and timestamps.
#[tokio::test]
async fn an_assessment_is_reported_from_what_was_stored() {
    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();

    for (case, request) in assessment_cases() {
        let started_at = Instant::now();
        let started = start_assessment(&router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartAssessment, got {} with {:?}",
            started.status, started.body
        );

        let id = started.string("/assessmentId");
        assert!(
            matches_async_task_id(&id),
            "case {case:?}: expected an assessmentId matching [0-9a-z-:]+ within 52 \
             characters, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: every assessment gets a fresh id, but {id:?} was already minted \
             ({seen_ids:?})"
        );
        seen_ids.push(id.clone());

        let latest = get_latest_assessment_id(&router).await;
        assert_eq!(
            latest.status, 200,
            "case {case:?}: expected HTTP 200 from GetLatestAssessmentId, got {} with {:?}",
            latest.status, latest.body
        );
        assert_eq!(
            latest.string("/id"),
            id,
            "case {case:?}: GetLatestAssessmentId must name the assessment just started, got {:?}",
            latest.body
        );

        let reported = get_assessment(&router, &id).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: expected HTTP 200 from GetAssessment, got {} with {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            at(&reported.body, "/id"),
            json!(id),
            "case {case:?}: GetAssessment must report the id it was asked about, got {:?}",
            at(&reported.body, "/id")
        );
        assert_eq!(
            at(&reported.body, "/assessmentTargets"),
            request
                .get("assessmentTargets")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: the assessment targets must come back exactly as supplied, got {:?}",
            at(&reported.body, "/assessmentTargets")
        );
        assert!(
            at(&reported.body, "/dataCollectionDetails/startTime").is_number(),
            "case {case:?}: dataCollectionDetails reports the stored start time, got {:?}",
            at(&reported.body, "/dataCollectionDetails/startTime")
        );

        if started_at.elapsed() < COMPLETION_SPAN {
            assert_eq!(
                at(&reported.body, "/dataCollectionDetails/status"),
                json!(ASSESSMENT_IN_PROGRESS),
                "case {case:?}: an assessment read inside the completion span is still \
                 {ASSESSMENT_IN_PROGRESS}, got {:?}",
                at(&reported.body, "/dataCollectionDetails/status")
            );
            assert_eq!(
                at(&reported.body, "/dataCollectionDetails/completionTime"),
                Value::Null,
                "case {case:?}: a running assessment has not completed, got {:?}",
                at(&reported.body, "/dataCollectionDetails/completionTime")
            );
        }
        assert_counters_add_up(case, &reported.body);
    }
}

/// `dataCollectionDetails` accounts for every server the assessment covers: the three
/// per-server counters always sum to `servers`, whatever status the assessment is in.
fn assert_counters_add_up(case: &str, body: &Value) {
    let count = |member: &str| -> i64 {
        at(body, &format!("/dataCollectionDetails/{member}"))
            .as_i64()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: dataCollectionDetails must report {member}, got {:?}",
                    at(body, "/dataCollectionDetails")
                )
            })
    };
    let (servers, success, failed, in_progress) = (
        count("servers"),
        count("success"),
        count("failed"),
        count("inProgress"),
    );
    assert_eq!(
        success + failed + in_progress,
        servers,
        "case {case:?}: success({success}) + failed({failed}) + inProgress({in_progress}) must \
         account for all {servers} servers"
    );
}

/// `dataCollectionDetails.servers` is computed from state, not canned: it is the number
/// of servers the completed imports brought into this service, which is the only way a
/// server enters it. The rows differ in how many imports completed first.
#[tokio::test]
async fn an_assessments_server_count_comes_from_the_imports_in_state() {
    for imports in [0usize, 1, 3] {
        let case = format!("{imports} completed import(s)");
        let router = emulator();

        let mut ids = Vec::new();
        for index in 0..imports {
            ids.push(
                start_import(&router, minimal_import(&format!("import-{index}")))
                    .await
                    .string("/id"),
            );
        }
        if imports > 0 {
            std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));
        }

        // What the imports themselves report they brought in.
        let mut expected_servers = 0i64;
        for id in &ids {
            let task = get_import(&router, id).await;
            expected_servers += at(&task.body, "/numberOfRecordsSuccess")
                .as_i64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a completed import reports its record count, got {:?}",
                        task.body)
                });
        }

        let id = start_assessment(&router, minimal_assessment())
            .await
            .string("/assessmentId");
        let reported = get_assessment(&router, &id).await;
        assert_eq!(
            at(&reported.body, "/dataCollectionDetails/servers"),
            json!(expected_servers),
            "case {case:?}: the assessment covers the {expected_servers} server(s) the imports \
             brought in, got {:?}",
            at(&reported.body, "/dataCollectionDetails/servers")
        );
        assert_counters_add_up(&case, &reported.body);
    }
}

/// `StopAssessment` mutates the stored assessment, so the following `GetAssessment`
/// observes a terminal status and nothing left collecting — and the stop is idempotent:
/// a second one leaves the record exactly as the first left it.
#[tokio::test]
async fn stopping_an_assessment_is_observed_by_the_next_read() {
    for (case, request) in assessment_cases() {
        // A fresh emulator per row, so each row's assessment is the latest one.
        let router = emulator();
        let started_at = Instant::now();
        let id = start_assessment(&router, request.clone())
            .await
            .string("/assessmentId");

        let before = get_assessment(&router, &id).await;
        if started_at.elapsed() >= COMPLETION_SPAN {
            // The clock already completed this assessment, so there is nothing to stop;
            // the row proves nothing about the stop and is skipped rather than asserted
            // wrongly.
            continue;
        }
        assert_eq!(
            at(&before.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_IN_PROGRESS),
            "case {case:?}: the assessment is {ASSESSMENT_IN_PROGRESS} before the stop, got {:?}",
            at(&before.body, "/dataCollectionDetails/status")
        );

        let stopped = stop_assessment(&router, &id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected HTTP 200 from StopAssessment, got {} with {:?}",
            stopped.status, stopped.body
        );

        let after = get_assessment(&router, &id).await;
        assert_eq!(
            at(&after.body, "/dataCollectionDetails/status"),
            json!(ASSESSMENT_STOPPED),
            "case {case:?}: the stop must persist, so the next read reports \
             {ASSESSMENT_STOPPED}, got {:?}",
            at(&after.body, "/dataCollectionDetails/status")
        );
        assert_eq!(
            at(&after.body, "/dataCollectionDetails/inProgress"),
            json!(0),
            "case {case:?}: a stopped assessment is collecting nothing, got {:?}",
            at(&after.body, "/dataCollectionDetails/inProgress")
        );
        assert!(
            at(&after.body, "/dataCollectionDetails/completionTime").is_number(),
            "case {case:?}: a stopped assessment reports when it stopped, got {:?}",
            at(&after.body, "/dataCollectionDetails/completionTime")
        );
        assert_eq!(
            at(&after.body, "/assessmentTargets"),
            at(&before.body, "/assessmentTargets"),
            "case {case:?}: stopping an assessment must not change its targets, got {:?}",
            at(&after.body, "/assessmentTargets")
        );
        assert_counters_add_up(case, &after.body);

        // `STOPPED` is terminal: the clock owes the assessment nothing more, so neither
        // waiting nor stopping again moves it.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));
        let twice = stop_assessment(&router, &id).await;
        assert_eq!(
            twice.status, 200,
            "case {case:?}: stopping a stopped assessment is not an error, got {} with {:?}",
            twice.status, twice.body
        );
        assert_eq!(
            get_assessment(&router, &id).await.body,
            after.body,
            "case {case:?}: a terminal assessment must not move again, got {:?}",
            get_assessment(&router, &id).await.body
        );
    }
}

/// An assessment that is left alone reaches the terminal status the clock owes it, and
/// its counters move with it: nothing is collecting any more.
#[tokio::test]
async fn an_assessment_completes_when_the_clock_owes_it() {
    let router = emulator();
    let id = start_assessment(
        &router,
        json!({
            "s3bucketForAnalysisData": "migrationhub-strategy-analysis",
            "assessmentTargets": [
                {"condition": "EQUALS", "name": "OS_NAME", "values": ["Linux"]},
            ],
        }),
    )
    .await
    .string("/assessmentId");

    std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

    let settled = get_assessment(&router, &id).await;
    assert_eq!(
        at(&settled.body, "/dataCollectionDetails/status"),
        json!(ASSESSMENT_COMPLETE),
        "once the completion span has passed the assessment is {ASSESSMENT_COMPLETE}, got {:?}",
        at(&settled.body, "/dataCollectionDetails/status")
    );
    assert_eq!(
        at(&settled.body, "/dataCollectionDetails/inProgress"),
        json!(0),
        "a completed assessment is collecting nothing, got {:?}",
        at(&settled.body, "/dataCollectionDetails/inProgress")
    );
    assert_counters_add_up("a completed assessment", &settled.body);
    assert_eq!(
        get_assessment(&router, &id).await.body,
        settled.body,
        "a completed assessment reports the same record on every read"
    );
}

// The refusals an assessment id that names no assessment earns are tabled in
// `assessment_error_paths.rs`, over both operations that take one.

/// `GetLatestAssessmentId` reports the most recently started assessment, and before any
/// assessment exists it reports the absent resource rather than an id nothing minted.
/// Which status and error code each error path answers is tabled in
/// `assessment_error_paths.rs`; this case only pins that the empty state is an error and
/// that the happy path follows the last start.
#[tokio::test]
async fn the_latest_assessment_id_follows_the_most_recent_start() {
    let router = emulator();

    let empty = get_latest_assessment_id(&router).await;
    assert_eq!(
        empty.status, 404,
        "with no assessment started there is no latest id to report, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException before any assessment exists, got {:?}",
        empty.body
    );

    for (case, request) in assessment_cases() {
        let id = start_assessment(&router, request)
            .await
            .string("/assessmentId");
        let latest = get_latest_assessment_id(&router).await;
        assert_eq!(
            latest.string("/id"),
            id,
            "case {case:?}: the latest id must be the one just started, got {:?}",
            latest.body
        );
        // Reporting the latest id must not change what the next caller sees.
        assert_eq!(
            get_latest_assessment_id(&router).await.string("/id"),
            id,
            "case {case:?}: reading the latest id twice must answer the same id"
        );
    }
}
