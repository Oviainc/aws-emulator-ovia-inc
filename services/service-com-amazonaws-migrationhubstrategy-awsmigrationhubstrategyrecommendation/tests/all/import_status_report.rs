// SPDX-License-Identifier: CC0-1.0

//! The status report an import file task writes, end to end over HTTP.
//!
//! The rule under test: when an import is started with a report bucket, the import writes
//! its status report object into that bucket and the completed task reports WHERE it is —
//! `statusReportS3Bucket` AND `statusReportS3Key` — so a caller can follow the pair and
//! read the per-record outcomes (the companion of `numberOfRecordsFailed`); a task that
//! named no report bucket, or whose report could not be written, reports no key, so no
//! reported key ever names an object a caller cannot fetch. `GetImportFileTask` and
//! `ListImportFileTask` report the same pair, because both read the one stored task.
//!
//! The emulator mounted here therefore holds the S3 service beside Strategy
//! Recommendations on ONE engine, exactly as the assembled emulator does: the import
//! writes its report by calling S3 in process, and each case fetches the object back
//! through S3's own front door rather than trusting the handler's word for it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::COMPLETION_SPAN;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, get_import, start_import, Answer};

/// The terminal `ImportFileTaskStatus` the clock owes a started task, spelled out here
/// rather than imported from the crate so a wrong status in `src/state.rs` fails here.
const IMPORT_SUCCESS: &str = "ImportSuccess";

/// `ImportFileTaskStatus`'s non-terminal import status, spelled out for the same reason.
const IMPORT_IN_PROGRESS: &str = "ImportInProgress";

/// `importS3Key`'s `@length(max: 1024)`, from
/// `smithy_query(query="operation",operation="GetImportFileTask")` — the bound the
/// reported `statusReportS3Key` has to stay within.
const MAX_IMPORT_S3_KEY: usize = 1024;

/// A SigV4 credential scope naming S3 in the SAME region the Strategy Recommendations
/// requests are signed for (`crate::import_and_assess`), so the object the import wrote
/// is read back from where it was written.
const SIGNED_FOR_S3_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// One emulator holding the S3 service beside Strategy Recommendations over one shared
/// engine — what the assembled emulator does.
fn emulator_with_object_storage() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// Send one path-style request to the emulator's S3 front door, returning the HTTP status
/// and the raw body.
///
/// S3 is a REST service: the bucket and key live in the URI, and the credential scope
/// names `s3`, which is what routes the request there.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str) -> (u16, Vec<u8>) {
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "s3.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_S3_EU_WEST_1)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body().as_ref().to_vec())
}

/// Create the bucket `name` through S3's own front door, as the account's owner would
/// before naming it to an import.
async fn create_bucket(case: &str, router: &MultiServiceRouter, name: &str) {
    let (status, body) = s3(router, "PUT", &format!("/{name}")).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: creating the report bucket {name:?} must succeed, got {status} with {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `GET /list-import-file-task`'s entry for `id`, or `Value::Null` when the listing does
/// not carry it.
async fn listed(router: &MultiServiceRouter, id: &str) -> Value {
    let answer: Answer = call(router, "GET", "/list-import-file-task", None).await;
    assert_eq!(
        answer.status, 200,
        "ListImportFileTask must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    at(&answer.body, "/taskInfos")
        .as_array()
        .and_then(|infos| {
            infos
                .iter()
                .find(|info| info.get("id").and_then(Value::as_str) == Some(id))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// One row of the table: a `StartImportFileTask` request, and whether the report bucket
/// it names is one the account actually holds.
struct Case {
    name: &'static str,
    request: Value,
    /// The bucket to create before starting the import; `None` leaves the named report
    /// bucket (if any) absent from the account.
    create: Option<&'static str>,
    /// Whether the import can write its report, and so whether a key is reported.
    expect_key: bool,
}

/// The requests every case runs, differing along what the rule generalises over: whether
/// a report bucket is named at all, whether the account holds it, the shape of the bucket
/// name and of the input key, how many import groups come with the import, and which
/// `dataSourceType` it carries.
///
/// Every bucket name begins with `migrationhub-strategy-`, the prefix each bucket
/// member's documentation requires.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no report bucket at all",
            request: json!({
                "name": "import-1",
                "S3Bucket": "migrationhub-strategy-import",
                "s3key": "servers.csv",
            }),
            create: None,
            expect_key: false,
        },
        Case {
            name: "a report bucket the account holds, with a nested input key",
            request: json!({
                "name": "import_2",
                "S3Bucket": "migrationhub-strategy-my-bucket",
                "s3key": "inventory/2026/01/servers.csv",
                "s3bucketForReportData": "migrationhub-strategy-reports",
            }),
            create: Some("migrationhub-strategy-reports"),
            expect_key: true,
        },
        Case {
            name: "a dotted report bucket name, a data source type and one import group",
            request: json!({
                "name": "IMPORT-3_mixed-case",
                "S3Bucket": "migrationhub-strategy-in",
                "s3key": "x",
                "s3bucketForReportData": "migrationhub-strategy-rep.dotted.name",
                "dataSourceType": "Import",
                "groupId": [{"name": "ExternalId", "value": "wave-1"}],
            }),
            create: Some("migrationhub-strategy-rep.dotted.name"),
            expect_key: true,
        },
        Case {
            name: "the shortest conforming report bucket name, with two import groups",
            request: json!({
                "name": "import-4",
                "S3Bucket": "migrationhub-strategy-ab",
                "s3key": "a/".repeat(100) + "servers.csv",
                "s3bucketForReportData": "migrationhub-strategy-r",
                "dataSourceType": "MPA",
                "groupId": [
                    {"name": "ExternalId", "value": "wave-2"},
                    {"name": "ExternalSourceType", "value": "MPA"},
                ],
            }),
            create: Some("migrationhub-strategy-r"),
            expect_key: true,
        },
        Case {
            // Nothing could be written, so nothing is named: the bucket the request
            // carried is still reported, the key is not.
            name: "a report bucket the account does not hold",
            request: json!({
                "name": "import-5",
                "S3Bucket": "migrationhub-strategy-in",
                "s3key": "servers.csv",
                "s3bucketForReportData": "migrationhub-strategy-no-such-bucket",
            }),
            create: None,
            expect_key: false,
        },
    ]
}

/// A completed import file task reports the status report it wrote — bucket and key — and
/// the object that key names is there to read, with the import's own record in it.
#[tokio::test]
async fn a_completed_import_reports_the_status_report_it_wrote() {
    let router = emulator_with_object_storage();
    let mut keys_seen: Vec<String> = Vec::new();

    for case in cases() {
        let Case {
            name: case,
            request,
            create,
            expect_key,
        } = case;
        if let Some(bucket) = create {
            create_bucket(case, &router, bucket).await;
        }

        let started = start_import(&router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartImportFileTask, got {} with {:?}",
            started.status, started.body
        );
        let id = started.string("/id");

        // Nothing but waiting drives the completion the clock owes the task.
        tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

        let reported = get_import(&router, &id).await;
        assert_eq!(
            at(&reported.body, "/status"),
            json!(IMPORT_SUCCESS),
            "case {case:?}: the task must have reached {IMPORT_SUCCESS}, got {:?}",
            at(&reported.body, "/status")
        );
        assert_eq!(
            at(&reported.body, "/statusReportS3Bucket"),
            request
                .get("s3bucketForReportData")
                .cloned()
                .unwrap_or(Value::Null),
            "case {case:?}: statusReportS3Bucket is the bucket the start call named, got {:?}",
            at(&reported.body, "/statusReportS3Bucket")
        );

        let reported_key = at(&reported.body, "/statusReportS3Key");
        if !expect_key {
            assert_eq!(
                reported_key,
                Value::Null,
                "case {case:?}: no status report object was written, so no \
                 statusReportS3Key may be reported, got {reported_key:?}"
            );
            // The listing agrees with the single read, because both read one record.
            let entry = listed(&router, &id).await;
            assert_eq!(
                at(&entry, "/statusReportS3Key"),
                Value::Null,
                "case {case:?}: the taskInfos entry must report no statusReportS3Key \
                 either, got {:?}",
                at(&entry, "/statusReportS3Key")
            );
            continue;
        }

        let key = reported_key.as_str().unwrap_or_default().to_string();
        assert!(
            !key.trim().is_empty() && key.len() <= MAX_IMPORT_S3_KEY,
            "case {case:?}: statusReportS3Key must be a non-blank key within \
             {MAX_IMPORT_S3_KEY} characters, got {reported_key:?}"
        );
        assert!(
            !keys_seen.contains(&key),
            "case {case:?}: each import writes its own status report, but {key:?} was \
             already reported ({keys_seen:?})"
        );
        keys_seen.push(key.clone());

        // The pair is followable: the object the key names is in the bucket reported
        // beside it.
        let bucket = at(&reported.body, "/statusReportS3Bucket")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, body) = s3(&router, "GET", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the status report at s3://{bucket}/{key} must be readable, \
             got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );

        let document: Value = serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the status report is a JSON document ({error}): {:?}",
                String::from_utf8_lossy(&body)
            )
        });
        // Every value in the report comes from the task it reports on.
        for (pointer, expected) in [
            ("/importTaskId", json!(id)),
            ("/importName", request["name"].clone()),
            ("/importFileS3Bucket", request["S3Bucket"].clone()),
            ("/importFileS3Key", request["s3key"].clone()),
            (
                "/numberOfRecordsSuccess",
                at(&reported.body, "/numberOfRecordsSuccess"),
            ),
            (
                "/numberOfRecordsFailed",
                at(&reported.body, "/numberOfRecordsFailed"),
            ),
        ] {
            assert_eq!(
                at(&document, pointer),
                expected,
                "case {case:?}: the status report's {pointer} must be the task's own \
                 value {expected:?}, got {:?}",
                at(&document, pointer)
            );
        }
        assert!(
            at(&document, "/records").is_array(),
            "case {case:?}: the status report lists one entry per imported record, got \
             {:?}",
            at(&document, "/records")
        );

        // The list read reports the same pair as the single read.
        let entry = listed(&router, &id).await;
        for member in ["/statusReportS3Bucket", "/statusReportS3Key"] {
            assert_eq!(
                at(&entry, member),
                at(&reported.body, member),
                "case {case:?}: ListImportFileTask must report {member} exactly as \
                 GetImportFileTask does ({:?}), got {:?}",
                at(&reported.body, member),
                at(&entry, member)
            );
        }

        // Reading twice reports the same location: the report is not re-minted per read.
        let again = get_import(&router, &id).await;
        assert_eq!(
            again.body, reported.body,
            "case {case:?}: reading the task twice must report the same record, got {:?} \
             then {:?}",
            reported.body, again.body
        );
    }

    assert_eq!(
        keys_seen.len(),
        cases().iter().filter(|case| case.expect_key).count(),
        "every case that named a report bucket the account holds reports a key, got \
         {keys_seen:?}"
    );
}

/// The status report is the artifact of a FINISHED import: a task still importing reports
/// the bucket it will be written to and no key, and the key appears once the clock has
/// owed the task its terminal status.
#[tokio::test]
async fn a_running_import_reports_no_status_report_key_yet() {
    let router = emulator_with_object_storage();
    let bucket = "migrationhub-strategy-progress-reports";
    create_bucket("the running case", &router, bucket).await;
    // How many rows were read while their import was still running. The completion span
    // is not zero, so a read issued straight after the start call observes the transient
    // state; asserting that at least one row did is what stops this half of the rule from
    // quietly going unchecked.
    let mut observed_running = 0usize;

    for (case, mut request) in [
        (
            "only the required members beside the report bucket",
            json!({
                "name": "running-1",
                "S3Bucket": "migrationhub-strategy-in",
                "s3key": "servers.csv",
            }),
        ),
        (
            "every optional member",
            json!({
                "name": "running-2",
                "S3Bucket": "migrationhub-strategy-in",
                "s3key": "deep/prefix/servers.csv",
                "dataSourceType": "ApplicationDiscoveryService",
                "groupId": [{"name": "ExternalId", "value": "wave-9"}],
            }),
        ),
    ] {
        request["s3bucketForReportData"] = json!(bucket);
        let id = start_import(&router, request).await.string("/id");

        // The transient half is asserted off the STATUS the task reports rather than off
        // the host clock: whenever a read finds the import still running, that read must
        // name the report bucket and no key.
        let running = get_import(&router, &id).await;
        if at(&running.body, "/status") == json!(IMPORT_IN_PROGRESS) {
            assert_eq!(
                at(&running.body, "/statusReportS3Bucket"),
                json!(bucket),
                "case {case:?}: a running task still reports the report bucket it was \
                 given, got {:?}",
                at(&running.body, "/statusReportS3Bucket")
            );
            assert_eq!(
                at(&running.body, "/statusReportS3Key"),
                Value::Null,
                "case {case:?}: a task that is still importing has no final status \
                 report to name, got {:?}",
                at(&running.body, "/statusReportS3Key")
            );
            observed_running += 1;
        }

        tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;

        let settled = get_import(&router, &id).await;
        assert_eq!(
            at(&settled.body, "/status"),
            json!(IMPORT_SUCCESS),
            "case {case:?}: the task must have reached {IMPORT_SUCCESS}, got {:?}",
            at(&settled.body, "/status")
        );
        let key = at(&settled.body, "/statusReportS3Key");
        let key = key.as_str().unwrap_or_default();
        assert!(
            !key.trim().is_empty(),
            "case {case:?}: a completed import reports the key of the status report it \
             wrote, got {:?}",
            at(&settled.body, "/statusReportS3Key")
        );
        let (status, body) = s3(&router, "GET", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the reported key must name a readable object, got {status} \
             with {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    assert!(
        observed_running > 0,
        "no row was read while its import was still running, so the no-key-while-running \
         half of the rule was never checked"
    );
}
