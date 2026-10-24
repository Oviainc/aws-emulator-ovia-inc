// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the export-task resource: `StartExportTask` and the
//! `GetExportTask` read it round-trips through, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * `StartExportTask` mints an export task OF a stored graph, persists it, and reports
//!   the stored record — a minted `^t-[a-z0-9]{10}$` task id, the source graph's id,
//!   every export parameter the request chose, and an `INITIALIZING` status — and
//!   `GetExportTask` answers that same record.
//! * The clock owes an accepted task a terminal status one span later. This emulator
//!   runs no export (it holds no graph data plane and makes no connection to Amazon S3),
//!   so that status is the modelled `FAILED` carrying a `statusReason` which says so,
//!   never `SUCCEEDED`, and no exported-element count is ever reported.
//! * `destination` is the Amazon S3 URI the data is written to, so a value carrying the
//!   `s3://` scheme and a bucket is accepted and one that does not is refused
//!   `ValidationException` (400).
//! * An identifier naming no stored resource is refused `ResourceNotFoundException`
//!   (404), and a graph that is not `AVAILABLE` cannot be exported (409).
//!
//! So each table below spans what its rule generalises over — the export format and
//! parquet type, whether a filter is supplied and what it filters on, the destination's
//! form and its length at the published bound and one past it, the tag entry count, the
//! IAM role's path form, and which non-`AVAILABLE` status the source graph is in —
//! rather than re-running one recorded request.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ARN_NAMESPACE, EXPORT_NOT_PERFORMED_REASON, EXPORT_TASK_SPAN, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs the service mints
/// are predictable.
const REGION: &str = "us-east-1";

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

/// The same pattern's `role/service-role` form, so the table spans both role paths.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/NeptuneExport";

/// The longest `destination` the model allows: `StartExportTaskInput$destination` is
/// `@length(1..1024)` (`smithy_query(query="operation",operation="StartExportTask")`).
const MAX_DESTINATION_CHARS: usize = 1024;

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
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

/// Percent-encode one path segment.
///
/// `resourceArn` is bound to a NON-greedy `{resourceArn}` label
/// (`/tags/{resourceArn}`), so the `/` inside an ARN has to arrive encoded or it would
/// split the URI into another path segment. Only the RFC 3986 unreserved set is left
/// as-is.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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

/// Whether `value` matches the modelled `ExportTaskId` pattern `^t-[a-z0-9]{10}$`.
fn is_export_task_id(value: &str) -> bool {
    value.strip_prefix("t-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// `POST /exporttasks` with `body`.
async fn start_export(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/exporttasks", Some(body)).await
}

/// `GET /exporttasks/{taskIdentifier}`.
async fn get_export(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/exporttasks/{task_id}"), None).await
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Poll a graph until it reports `status`.
async fn wait_for_graph_status(router: &MultiServiceRouter, id: &str, status: &str) -> Value {
    for _ in 0..40 {
        let (code, body) = call(router, "GET", &format!("/graphs/{id}"), None).await;
        assert_eq!(
            code, 200,
            "case graph {id} -> {status}: expected 200 from GetGraph, got {code}: {body}"
        );
        if body["status"] == json!(status) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case graph {id}: never reached {status}");
}

/// Poll an export task until it has left `INITIALIZING` — the terminal status the clock
/// owes it. Nothing in the service sleeps for that span; only this test does.
async fn wait_until_terminal(router: &MultiServiceRouter, case: &str, task_id: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = get_export(router, task_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetExportTask, got {status}: {body}"
        );
        if body["status"] != json!("INITIALIZING") {
            return body;
        }
        tokio::time::sleep(EXPORT_TASK_SPAN / 4).await;
    }
    panic!("case {case:?}: task {task_id} never left INITIALIZING");
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let id = graph_created(router, name).await;
    wait_until_available(router, &format!("/graphs/{id}")).await;
    id
}

/// Create a graph and return its id, without waiting for it to come into service.
async fn graph_created(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "graphName": name,
        "provisionedMemory": MIN_PROVISIONED_MEMORY,
    });
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string()
}

/// A `destination` of exactly `chars` characters, spelled as a well-formed S3 URI, so a
/// length-bound row differs from the others only in its length.
fn destination_of_length(chars: usize) -> String {
    let prefix = "s3://export-bucket/";
    format!("{prefix}{}", "k".repeat(chars - prefix.len()))
}

/// Assert that exporting from the graph `graph_id` names is refused as the conflict it
/// is, because the graph is not `AVAILABLE`.
async fn refuse_export(router: &MultiServiceRouter, case: &str, graph_id: &str) {
    let (status, body) = start_export(
        router,
        json!({
            "graphIdentifier": graph_id,
            "roleArn": ROLE_ARN,
            "format": "CSV",
            "destination": "s3://export-bucket",
            "kmsKeyIdentifier": KMS_KEY_ARN,
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case {case:?}: unexpected error code in {body}"
    );
}

/// One row of the create-and-read-back table.
struct ExportCase {
    case: &'static str,
    role_arn: &'static str,
    format: &'static str,
    parquet_type: Option<&'static str>,
    destination: String,
    /// The `exportFilter` the request carries, with every optional member spelled out so
    /// the echoed value is compared exactly (`multiValueHandling` has a modelled
    /// default, which a server may fill in for an absent member).
    export_filter: Option<Value>,
    tags: BTreeMap<String, String>,
}

/// The export parameters a request carries are persisted on the task and reported back
/// by `GetExportTask`, with a minted task id and the `INITIALIZING` status.
#[tokio::test]
async fn start_export_task_persists_the_record_and_reads_it_back() {
    let router = emulator();

    let cases = vec![
        ExportCase {
            case: "CSV to a bucket root, no filter, no tags",
            role_arn: ROLE_ARN,
            format: "CSV",
            parquet_type: None,
            destination: "s3://export-bucket".to_string(),
            export_filter: None,
            tags: BTreeMap::new(),
        },
        ExportCase {
            case: "CSV under a key prefix, a vertex filter, one tag, a service role",
            role_arn: SERVICE_ROLE_ARN,
            format: "CSV",
            parquet_type: None,
            destination: "s3://export-bucket/exports/2026-10-24".to_string(),
            export_filter: Some(json!({
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
            })),
            tags: [("env".to_string(), "dev".to_string())]
                .into_iter()
                .collect(),
        },
        ExportCase {
            case: "PARQUET with its columnar type, an edge filter, several tags",
            role_arn: ROLE_ARN,
            format: "PARQUET",
            parquet_type: Some("COLUMNAR"),
            destination: "s3://export.bucket.with.dots/prefix".to_string(),
            export_filter: Some(json!({
                "edgeFilter": {
                    "knows": {
                        "properties": {
                            "since": {
                                "outputType": "Long",
                                "sourcePropertyName": "since",
                                "multiValueHandling": "TO_LIST",
                            },
                        },
                    },
                },
            })),
            tags: [
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "graph".to_string()),
                ("cost-centre".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
        },
        ExportCase {
            case: "PARQUET without a parquet type, an empty filter",
            role_arn: ROLE_ARN,
            format: "PARQUET",
            parquet_type: None,
            destination: "s3://export-bucket/empty-filter".to_string(),
            // "If the ExportFilter is provided but is an empty object, then no data
            // will be exported" (smithy_query(query="shape",shape="ExportFilter")).
            export_filter: Some(json!({})),
            tags: BTreeMap::new(),
        },
        ExportCase {
            case: "a destination at the published length bound",
            role_arn: ROLE_ARN,
            format: "CSV",
            parquet_type: None,
            destination: destination_of_length(MAX_DESTINATION_CHARS),
            export_filter: None,
            tags: BTreeMap::new(),
        },
    ];

    let mut minted = BTreeSet::new();
    for (index, row) in cases.into_iter().enumerate() {
        let ExportCase {
            case,
            role_arn,
            format,
            parquet_type,
            destination,
            export_filter,
            tags,
        } = row;
        let graph_id = graph_in_service(&router, &format!("source-graph-{index}")).await;

        let mut request = json!({
            "graphIdentifier": graph_id,
            "roleArn": role_arn,
            "format": format,
            "destination": destination,
            "kmsKeyIdentifier": KMS_KEY_ARN,
            "tags": tags,
        });
        if let Some(parquet) = parquet_type {
            request["parquetType"] = json!(parquet);
        }
        if let Some(filter) = export_filter.clone() {
            request["exportFilter"] = filter;
        }

        let (status, started) = start_export(&router, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {started}"
        );

        let task_id = started["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
            .to_string();
        assert!(
            is_export_task_id(&task_id),
            "case {case:?}: expected a taskId matching ^t-[a-z0-9]{{10}}$, got {task_id:?}"
        );
        assert!(
            minted.insert(task_id.clone()),
            "case {case:?}: taskId {task_id:?} was minted twice"
        );
        assert_eq!(
            started["graphId"],
            json!(graph_id),
            "case {case:?}: expected graphId {graph_id:?}, got {}",
            started["graphId"]
        );
        assert_eq!(
            started["status"],
            json!("INITIALIZING"),
            "case {case:?}: no data has moved when the call returns, expected \
             INITIALIZING, got {}",
            started["status"]
        );
        assert_eq!(
            started["statusReason"],
            Value::Null,
            "case {case:?}: an accepted task has no reason to report yet, got {}",
            started["statusReason"]
        );
        for (member, expected) in [
            ("roleArn", json!(role_arn)),
            ("format", json!(format)),
            ("destination", json!(destination)),
            ("kmsKeyIdentifier", json!(KMS_KEY_ARN)),
            (
                "parquetType",
                parquet_type.map_or(Value::Null, |value| json!(value)),
            ),
            (
                "exportFilter",
                export_filter.clone().unwrap_or(Value::Null),
            ),
        ] {
            assert_eq!(
                started[member], expected,
                "case {case:?}: expected {member} {expected}, got {}",
                started[member]
            );
        }

        // The round trip: an independent read reports the record that was stored, not
        // the response that was rendered.
        let (read_status, read) = get_export(&router, &task_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetExportTask, got {read_status}: {read}"
        );
        for member in [
            "taskId",
            "graphId",
            "roleArn",
            "format",
            "destination",
            "kmsKeyIdentifier",
            "parquetType",
            "exportFilter",
        ] {
            assert_eq!(
                read[member], started[member],
                "case {case:?}: GetExportTask's {member} must be the stored value {}, got {}",
                started[member], read[member]
            );
        }

        // The tags supplied at start time are stored ON the task, so the tagging
        // surface reads them back through the task's own ARN.
        let expected_arn =
            engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("export-task/{task_id}"));
        let (tags_status, tags_body) =
            call(&router, "GET", &format!("/tags/{}", encode(&expected_arn)), None).await;
        assert_eq!(
            tags_status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {tags_status}: \
             {tags_body}"
        );
        assert_eq!(
            tags_body["tags"],
            json!(tags),
            "case {case:?}: expected the supplied tags {}, got {}",
            json!(tags),
            tags_body["tags"]
        );
    }
}

/// The terminal status the clock owes an accepted task is `FAILED` with a reason: this
/// emulator runs no export, so no object was written and nothing is claimed otherwise.
#[tokio::test]
async fn an_accepted_task_ends_failed_because_no_export_ran() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "export-source").await;

    // (case, format, destination) — the terminal status does not depend on either.
    let cases = [
        ("CSV to a bucket root", "CSV", "s3://export-bucket"),
        (
            "PARQUET under a key prefix",
            "PARQUET",
            "s3://export-bucket/parquet",
        ),
    ];

    for (case, format, destination) in cases {
        let (status, started) = start_export(
            &router,
            json!({
                "graphIdentifier": graph_id,
                "roleArn": ROLE_ARN,
                "format": format,
                "destination": destination,
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {started}"
        );
        let task_id = started["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
            .to_string();

        // Read straight after the start call: the transient status is observable, so a
        // caller never sees the task skip it.
        let (early_status, early) = get_export(&router, &task_id).await;
        assert_eq!(
            early_status, 200,
            "case {case:?}: expected 200 from GetExportTask, got {early_status}: {early}"
        );
        assert_eq!(
            early["status"],
            json!("INITIALIZING"),
            "case {case:?}: a task read straight after the start call is still \
             INITIALIZING, got {}",
            early["status"]
        );

        let settled = wait_until_terminal(&router, case, &task_id).await;
        assert_eq!(
            settled["status"],
            json!("FAILED"),
            "case {case:?}: nothing exported the graph, so the clock owes FAILED, got {}",
            settled["status"]
        );
        assert_eq!(
            settled["statusReason"],
            json!(EXPORT_NOT_PERFORMED_REASON),
            "case {case:?}: expected the reason {EXPORT_NOT_PERFORMED_REASON:?}, got {}",
            settled["statusReason"]
        );

        // The details are derived from the record and the clock: a real start time, a
        // real elapsed span, no progress and no written-element count — a count would
        // claim a run that did not happen.
        let details = &settled["exportTaskDetails"];
        assert!(
            details["startTime"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: expected a startTime, got {}",
            details["startTime"]
        );
        assert!(
            details["timeElapsedSeconds"]
                .as_i64()
                .is_some_and(|seconds| seconds >= 0),
            "case {case:?}: expected a timeElapsedSeconds, got {}",
            details["timeElapsedSeconds"]
        );
        assert_eq!(
            details["progressPercentage"],
            json!(0),
            "case {case:?}: no data was exported, so no progress was made, got {}",
            details["progressPercentage"]
        );
        for member in ["numVerticesWritten", "numEdgesWritten"] {
            assert_eq!(
                details[member],
                Value::Null,
                "case {case:?}: nothing was written, so {member} is not reported, got {}",
                details[member]
            );
        }

        // A read reports; it does not move the record on. A second read answers the
        // same terminal status and reason.
        let (again_status, again) = get_export(&router, &task_id).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: expected 200 from the second read, got {again_status}: {again}"
        );
        for member in ["status", "statusReason", "taskId", "graphId", "destination"] {
            assert_eq!(
                again[member], settled[member],
                "case {case:?}: a read must not change {member}, expected {}, got {}",
                settled[member], again[member]
            );
        }
    }

    // The source graph is READ by an export, so its own status is untouched by one.
    let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
    assert_eq!(
        graph_status, 200,
        "case the source graph after two exports: expected 200, got {graph_status}: {graph}"
    );
    assert_eq!(
        graph["status"],
        json!("AVAILABLE"),
        "case the source graph after two exports: exporting reads the graph, so it stays \
         AVAILABLE, got {}",
        graph["status"]
    );
}

/// `destination` names the Amazon S3 URI the data is written to: one carrying the
/// `s3://` scheme and a bucket is accepted, and one that does not is refused.
#[tokio::test]
async fn a_destination_that_is_not_an_s3_uri_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "destination-graph").await;

    // (case, destination, expected status)
    let cases: Vec<(&str, String, u16)> = vec![
        ("a bucket root", "s3://export-bucket".to_string(), 201),
        (
            "a bucket and a key prefix",
            "s3://export-bucket/a/b/c".to_string(),
            201,
        ),
        (
            "the published length bound",
            destination_of_length(MAX_DESTINATION_CHARS),
            201,
        ),
        (
            "one character past the published length bound",
            destination_of_length(MAX_DESTINATION_CHARS + 1),
            400,
        ),
        ("an empty destination", String::new(), 400),
        ("no scheme at all", "export-bucket/key".to_string(), 400),
        (
            "an HTTP URL rather than an S3 URI",
            "https://export-bucket.s3.amazonaws.com/key".to_string(),
            400,
        ),
        ("the scheme with no bucket", "s3://".to_string(), 400),
        ("an empty bucket segment", "s3:///key".to_string(), 400),
        (
            // The scheme of an Amazon S3 URI is spelled in lowercase.
            "an upper-case scheme",
            "S3://export-bucket".to_string(),
            400,
        ),
    ];

    for (case, destination, expected) in cases {
        let (status, body) = start_export(
            &router,
            json!({
                "graphIdentifier": graph_id,
                "roleArn": ROLE_ARN,
                "format": "CSV",
                "destination": destination,
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("destination")),
                "case {case:?}: the refusal must name the member it is about, got {}",
                body["message"]
            );
        }
    }
}

/// An identifier naming no stored resource is a 404, and a graph that is not
/// `AVAILABLE` cannot be exported.
#[tokio::test]
async fn an_unknown_identifier_or_a_graph_that_is_not_available_is_refused() {
    let router = emulator();

    // A graph id the service never minted.
    let (status, body) = start_export(
        &router,
        json!({
            "graphIdentifier": "g-abcdefghij",
            "roleArn": ROLE_ARN,
            "format": "CSV",
            "destination": "s3://export-bucket",
            "kmsKeyIdentifier": KMS_KEY_ARN,
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case a graph id that was never minted: expected 404, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case a graph id that was never minted: unexpected error code in {body}"
    );

    // Each non-AVAILABLE status a graph can be read in: the graph's data is in flight,
    // so it cannot be exported. Each row is driven straight after the setup that puts
    // the graph in its status, so the clock cannot settle it in between.
    let stopped = graph_in_service(&router, "stopped-graph").await;
    let (stop_status, stop_body) =
        call(&router, "POST", &format!("/graphs/{stopped}/stop"), None).await;
    assert_eq!(
        stop_status, 200,
        "case stopping the source graph: expected 200, got {stop_status}: {stop_body}"
    );
    wait_for_graph_status(&router, &stopped, "STOPPED").await;
    refuse_export(&router, "a graph that is STOPPED", &stopped).await;

    let creating = graph_created(&router, "creating-graph").await;
    refuse_export(&router, "a graph that is still CREATING", &creating).await;

    // The read side: a task id the service never minted names nothing, and an
    // identifier outside the modelled pattern is refused before any lookup.
    for (case, task_identifier, expected_status, expected_code) in [
        (
            "a task id that was never minted",
            "t-abcdefghij",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identifier outside the modelled pattern",
            "not-a-task-id",
            400,
            "ValidationException",
        ),
        (
            "a graph id where a task id belongs",
            "g-abcdefghij",
            400,
            "ValidationException",
        ),
    ] {
        let (status, body) = get_export(&router, task_identifier).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
}
