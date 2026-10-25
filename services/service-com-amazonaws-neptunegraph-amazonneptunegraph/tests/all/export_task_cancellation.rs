// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelExportTask`: the operation that stops an export this
//! service accepted, driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A cancellation is accepted only while a task's export is still in flight, and it is
//!   ACCEPTED rather than completed: the task reports the modelled `CANCELLING` status
//!   ("The status is CANCELLING when the export task is cancelled"), the response carries
//!   the record that was stored, and the clock owes the task the move to `CANCELLED`
//!   carrying a reason that names the cancellation.
//! * A task that has already stopped, or is already winding down, has no work left to
//!   cancel and is refused `ConflictException` (409), and a refused cancellation persists
//!   nothing.
//! * A `taskIdentifier` this Region holds no task under is refused
//!   `ResourceNotFoundException` (404); one outside the modelled `ExportTaskId` pattern
//!   `^t-[a-z0-9]{10}$` is refused `ValidationException` (400) before the handler runs.
//! * Cancelling an export does not touch the graph it reads: an export READS a graph, and
//!   the model puts no export status on `GraphStatus`.
//!
//! So each table below spans what its rule generalises over — the export format and
//! parquet type, whether a filter is supplied, the destination's form, the status the
//! task is in when the cancellation arrives, and the form of the identifier it names —
//! rather than re-running one request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    CANCELLATION_SPAN, EXPORT_TASK_SPAN, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// Another Region, so a row can ask what a task of one Region looks like from another.
const OTHER_REGION: &str = "eu-west-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneExportRole";

/// The same pattern's `role/service-role` form, so a row spans both role paths.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/NeptuneExport";

/// An emulator holding only this service, on its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request signed for `region` and return its status and parsed
/// JSON body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// [`call_in`] for the Region these tests work in.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, path, body).await
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `GET /exporttasks/{taskIdentifier}`.
async fn get_export(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/exporttasks/{task_id}"), None).await
}

/// `DELETE /exporttasks/{taskIdentifier}` — the wire form of `CancelExportTask`.
async fn cancel_export(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/exporttasks/{task_id}"), None).await
}

/// Poll a graph until it reports `AVAILABLE`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, graph_id: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            status, 200,
            "case graph {graph_id}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case graph {graph_id}: never became AVAILABLE");
}

/// Poll an export task until it reports `status`.
async fn wait_for_task_status(
    router: &MultiServiceRouter,
    case: &str,
    task_id: &str,
    status: &str,
) -> Value {
    for _ in 0..40 {
        let (code, body) = get_export(router, task_id).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected 200 from GetExportTask, got {code}: {body}"
        );
        if body["status"] == json!(status) {
            return body;
        }
        tokio::time::sleep(CANCELLATION_SPAN.min(EXPORT_TASK_SPAN) / 4).await;
    }
    let (_, body) = get_export(router, task_id).await;
    panic!(
        "case {case:?}: task {task_id} never reached {status}, last status {}",
        body["status"]
    );
}

/// Create a graph, wait it into service and return its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "graphName": name,
        "provisionedMemory": MIN_PROVISIONED_MEMORY,
    });
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string();
    wait_until_available(router, &id).await;
    id
}

/// Start an export of `graph_id` from `request` and return the accepted task's id and the
/// record `StartExportTask` reported.
async fn export_in_flight(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    request: Value,
) -> (String, Value) {
    let mut body = json!({ "graphIdentifier": graph_id });
    let (Value::Object(target), Value::Object(extra)) = (&mut body, request) else {
        panic!("case {case:?}: the request table carries JSON objects");
    };
    target.extend(extra);
    let (status, started) = call(router, "POST", "/exporttasks", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from StartExportTask for {body}, got {status}: {started}"
    );
    assert_eq!(
        started["status"],
        json!("INITIALIZING"),
        "case {case:?}: an accepted export starts INITIALIZING, got {}",
        started["status"]
    );
    let task_id = started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
        .to_string();
    (task_id, started)
}

/// The members `CancelExportTaskOutput` shares with the record `StartExportTask` stored:
/// cancelling stops the export, it does not re-write what the task is exporting.
const STORED_MEMBERS: [&str; 7] = [
    "taskId",
    "graphId",
    "roleArn",
    "format",
    "destination",
    "kmsKeyIdentifier",
    "parquetType",
];

/// One row of the accepted-cancellation table.
struct CancelCase {
    case: &'static str,
    /// The `StartExportTask` members this row exports with, beside `graphIdentifier`.
    request: Value,
}

/// An accepted cancellation reports the stored record with the modelled `CANCELLING`
/// status, persists that status, and the clock then settles the task to `CANCELLED` with
/// a reason that names the cancellation — whatever the task was exporting.
#[tokio::test]
async fn cancel_export_task_accepts_an_in_flight_export_and_the_clock_completes_it() {
    let router = emulator();

    let cases = vec![
        CancelCase {
            case: "CSV to a bucket root, no filter",
            request: json!({
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": "s3://export-bucket",
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
        },
        CancelCase {
            case: "PARQUET with its columnar type, under a key prefix, a service role",
            request: json!({
                "roleArn": SERVICE_ROLE_ARN,
                "format": "PARQUET",
                "parquetType": "COLUMNAR",
                "destination": "s3://export-bucket/exports/nightly",
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
        },
        CancelCase {
            case: "CSV with a vertex filter",
            request: json!({
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": "s3://export-bucket/filtered/",
                "kmsKeyIdentifier": KMS_KEY_ARN,
                "exportFilter": {
                    "vertexFilter": {
                        "person": {
                            "properties": {
                                "name": {
                                    "outputType": "String",
                                    "sourcePropertyName": "name",
                                    "multiValueHandling": "PICK_FIRST",
                                },
                            },
                        },
                    },
                },
            }),
        },
    ];

    for (index, CancelCase { case, request }) in cases.into_iter().enumerate() {
        // A graph per row, so a row is about the TASK and not about a transition another
        // row left outstanding on a shared graph.
        let graph_id = graph_in_service(&router, &format!("cancel-target-{index}")).await;
        let (task_id, started) = export_in_flight(&router, case, &graph_id, request).await;

        let (status, cancelled) = cancel_export(&router, &task_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CancelExportTask, got {status}: {cancelled}"
        );
        // Accepted, not completed: the modelled status of a cancelled export task is
        // CANCELLING.
        assert_eq!(
            cancelled["status"],
            json!("CANCELLING"),
            "case {case:?}: expected CANCELLING, got {}",
            cancelled["status"]
        );
        for member in STORED_MEMBERS {
            assert_eq!(
                cancelled[member], started[member],
                "case {case:?}: cancelling must report the stored {member}, expected {}, got {}",
                started[member], cancelled[member]
            );
        }
        // `CancelExportTaskOutput` models no `exportFilter`, so none is reported even for
        // a task that carries one.
        assert!(
            cancelled["exportFilter"].is_null(),
            "case {case:?}: CancelExportTaskOutput models no exportFilter, got {}",
            cancelled["exportFilter"]
        );

        // The cancellation is PERSISTED: an independent read sees the task winding down,
        // with everything else about the record where it was.
        let (read_status, read) = get_export(&router, &task_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetExportTask, got {read_status}: {read}"
        );
        assert_eq!(
            read["status"],
            json!("CANCELLING"),
            "case {case:?}: expected CANCELLING from the read, got {}",
            read["status"]
        );
        assert_eq!(
            read["exportFilter"], started["exportFilter"],
            "case {case:?}: cancelling must not change the stored exportFilter, expected {}, \
             got {}",
            started["exportFilter"], read["exportFilter"]
        );

        // The clock settles the task into CANCELLED — not the FAILED outcome the
        // uncancelled task would have reached — with a reason naming the cancellation.
        let stopped = wait_for_task_status(&router, case, &task_id, "CANCELLED").await;
        let reason = stopped["statusReason"].as_str().unwrap_or_default();
        assert!(
            reason.to_ascii_lowercase().contains("cancel"),
            "case {case:?}: expected a reason naming the cancellation, got {}",
            stopped["statusReason"]
        );
        for member in STORED_MEMBERS {
            assert_eq!(
                stopped[member], started[member],
                "case {case:?}: settling must not change {member}, expected {}, got {}",
                started[member], stopped[member]
            );
        }

        // An export READS the graph, so cancelling one leaves the graph where it was.
        let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            graph_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {graph_status}: {graph}"
        );
        assert_eq!(
            graph["status"],
            json!("AVAILABLE"),
            "case {case:?}: cancelling an export must not move the graph, got {}",
            graph["status"]
        );
    }
}

/// A cancellation is accepted only while the export is in flight: one row per status the
/// task can be in when it arrives.
#[tokio::test]
async fn cancellation_is_accepted_only_while_the_export_is_in_flight() {
    let router = emulator();

    // (case, the status the task is in when the cancellation arrives, expected status,
    // expected error code)
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        ("an in-flight task", "INITIALIZING", 200, ""),
        (
            "a task already cancelling",
            "CANCELLING",
            409,
            "ConflictException",
        ),
        (
            "a task that stopped cancelled",
            "CANCELLED",
            409,
            "ConflictException",
        ),
        (
            "a task that stopped failed",
            "FAILED",
            409,
            "ConflictException",
        ),
    ];
    for (index, (case, when, expected, expected_code)) in cases.into_iter().enumerate() {
        let graph_id = graph_in_service(&router, &format!("cancel-case-{index}")).await;
        let (task_id, _) = export_in_flight(
            &router,
            case,
            &graph_id,
            json!({
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": "s3://export-bucket/case/",
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
        )
        .await;

        // Bring the task to the status this row is about.
        match when {
            "INITIALIZING" => {}
            "CANCELLING" | "CANCELLED" => {
                let (status, body) = cancel_export(&router, &task_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the first cancellation must be accepted, got {status}: {body}"
                );
                if when == "CANCELLED" {
                    wait_for_task_status(&router, case, &task_id, "CANCELLED").await;
                }
            }
            other => {
                assert_eq!(other, "FAILED", "case {case:?}: unknown starting status");
                wait_for_task_status(&router, case, &task_id, "FAILED").await;
            }
        }

        let (status, body) = cancel_export(&router, &task_id).await;
        assert_eq!(
            status, expected,
            "case {case:?} (task in {when}): expected {expected}, got {status}: {body}"
        );
        if expected_code.is_empty() {
            assert_eq!(
                body["status"],
                json!("CANCELLING"),
                "case {case:?}: expected CANCELLING, got {}",
                body["status"]
            );
        } else {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} (task in {when}): expected {expected_code}, got {code:?}: {body}"
            );
            // A refused cancellation persists NOTHING: the task still reports the status
            // it was in, or the one the clock had already made due for it.
            let (read_status, read) = get_export(&router, &task_id).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected 200 from GetExportTask, got {read_status}: {read}"
            );
            let settled: [&str; 2] = if when == "CANCELLING" {
                // The first cancellation's own transition may have come due by now.
                ["CANCELLING", "CANCELLED"]
            } else {
                [when, when]
            };
            let actual = read["status"].as_str().unwrap_or_default();
            assert!(
                settled.contains(&actual),
                "case {case:?}: expected the task to still be one of {settled:?}, got {actual:?}"
            );
        }
    }
}

/// A `taskIdentifier` naming no task this Region holds is refused, and one outside the
/// modelled `ExportTaskId` pattern never reaches the handler.
#[tokio::test]
async fn a_task_identifier_naming_no_export_task_of_this_region_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "identifier-target").await;
    let (task_id, _) = export_in_flight(
        &router,
        "the stored task",
        &graph_id,
        json!({
            "roleArn": ROLE_ARN,
            "format": "CSV",
            "destination": "s3://export-bucket/identifier/",
            "kmsKeyIdentifier": KMS_KEY_ARN,
        }),
    )
    .await;

    // (case, the taskIdentifier the request carries, expected status, expected code)
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a well-formed task id that was never minted",
            "t-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        (
            "another well-formed absent task id",
            "t-0000000000",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a graph id where a task id belongs",
            "g-abcdefghij",
            400,
            "ValidationException",
        ),
        (
            "an identifier outside the modelled pattern",
            "nope",
            400,
            "ValidationException",
        ),
        (
            "an upper-cased task id, which the pattern excludes",
            "t-ABCDEFGHIJ",
            400,
            "ValidationException",
        ),
    ];
    for (case, identifier, expected, expected_code) in cases {
        let (status, body) = cancel_export(&router, identifier).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}: {body}"
        );
    }

    // The task's own Region is part of the lookup: the same id names no task from
    // another Region, and the task there is untouched.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "DELETE",
        &format!("/exporttasks/{task_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case a task of another Region: expected 404, got {status}: {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case a task of another Region: expected ResourceNotFoundException, got {code:?}: {body}"
    );
    let (read_status, read) = get_export(&router, &task_id).await;
    assert_eq!(
        read_status, 200,
        "case the stored task: expected 200 from GetExportTask, got {read_status}: {read}"
    );
    let actual = read["status"].as_str().unwrap_or_default();
    assert!(
        ["INITIALIZING", "FAILED"].contains(&actual),
        "case the stored task: a refused cancellation must leave it where the clock put \
         it, got {actual:?}"
    );
}
