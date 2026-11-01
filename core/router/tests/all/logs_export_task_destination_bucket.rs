// SPDX-License-Identifier: CC0-1.0

//! CloudWatch Logs resolves an export task's destination bucket against Amazon S3 before it
//! mints a task id.
//!
//! The rule under test: a create whose input names a resource of ANOTHER service resolves
//! that resource against the owning service before it persists anything, and answers its own
//! modelled client error — here HTTP 400 `InvalidParameterException`, which
//! `CreateExportTask` binds (smithy_query: operation CreateExportTask, errors include
//! InvalidParameterException httpError 400) — when the owner reports it absent, leaving no
//! export task behind. The requirement is the model's own: `CreateExportTask$destination` is
//! "the name of S3 bucket for the exported log data", and an export to a bucket that is not
//! there could never write a byte, so a task id for it would be an id a client polls forever.
//! Only EXISTENCE is resolved: that is the half the object store can answer for and the half
//! this error channel can express.
//!
//! This suite needs TWO services on one engine — CloudWatch Logs reaches S3 through the
//! in-process callback, so a single-service router has no S3 to ask and the bucket a row
//! names has to be creatable — which is why it lives here rather than in either service's own
//! crate, exactly as `s3files_create_file_system_bucket.rs` does for S3 Files. The logs
//! crate's own suites pin the other half: with no S3 to ask, the create is admitted rather
//! than refused on an assumption.
//!
//! The rows span what the rule generalises over: whether the bucket was ever created, the
//! bucket name's length (S3 documents 3..=63 characters) and form (hyphens, dots, digits), a
//! name that merely HAS an existing bucket's name as a prefix — so the resolution is
//! exact-name, not prefix, matching — and whether the request also names a `destinationPrefix`
//! and a `taskName`. The recorded single absent-bucket case is one row.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "eu-west-2";

/// CloudWatch Logs' `X-Amz-Target` prefix and the signing name its credential scope carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const LOGS_SIGNING_NAME: &str = "logs";

/// Amazon S3's signing name, whose REST calls carry no `X-Amz-Target`.
const S3_SIGNING_NAME: &str = "s3";

/// The log group every export below reads from.
const GROUP: &str = "/export/destination-gate";

/// The wording the refusal carries, written out here rather than read from the service, so a
/// change to the implementation's own constant makes this assertion fail.
const BUCKET_MISSING_MESSAGE: &str =
    "The given bucket does not exist. Please make sure the bucket is valid.";

/// The modelled code an incorrectly specified parameter is refused under.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so requests are unauthenticated; the credential scope
    // is still carried, because it is what names the Region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// The SigV4 header a request carries, naming the signing name the router routes on.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
    )
}

/// One CloudWatch Logs awsJson1_1 call: its HTTP status and parsed body.
async fn logs_call(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{LOGS_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization(LOGS_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch Logs call that must succeed.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) -> Value {
    let (status, parsed) = logs_call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: logs {operation} {request} must succeed, got {status} with {parsed}"
    );
    parsed
}

/// `PUT /<bucket>` — S3's `CreateBucket`, naming the Region this suite is signed for as its
/// location constraint.
///
/// A create sent to a REGIONAL endpoint has to name one: the unspecified constraint means
/// us-east-1, so S3 refuses a create that leaves it out anywhere else.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let http_request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}"))
        .header("host", format!("{S3_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization(S3_SIGNING_NAME))
        .body(Bytes::from(format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        )))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// The export tasks `DescribeExportTasks` reports, as `(taskId, destination)` pairs.
async fn export_tasks(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let page = logs(router, "describe", "DescribeExportTasks", &json!({})).await;
    page["exportTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeExportTasks reports a list, got {page}"))
        .iter()
        .map(|task| {
            (
                task["taskId"].as_str().unwrap_or_default().to_string(),
                task["destination"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One row: the bucket the row names, whether that bucket is created first, whether the
/// create must be admitted, and the optional members the request also carries.
struct Row {
    case: &'static str,
    create: bool,
    bucket: String,
    prefix: Option<&'static str>,
    task_name: Option<&'static str>,
    accepted: bool,
}

/// A 63-character bucket name — the longest S3 documents ("Bucket names must be between 3
/// (min) and 63 (max) characters long", S3 bucket naming rules).
fn longest_bucket_name() -> String {
    let name = format!("logs-export-gate-{}", "a".repeat(46));
    assert_eq!(name.len(), 63, "the longest-name row must be 63 characters");
    name
}

fn rows() -> Vec<Row> {
    let existing = "logs-export-gate-plain";
    vec![
        Row {
            case: "a bucket that exists",
            create: true,
            bucket: existing.to_string(),
            prefix: None,
            task_name: None,
            accepted: true,
        },
        Row {
            case: "a bucket that was never created",
            create: false,
            bucket: "logs-export-gate-ghost".to_string(),
            prefix: None,
            task_name: None,
            accepted: false,
        },
        Row {
            // The shortest name S3's naming rules allow.
            case: "the shortest bucket name, created",
            create: true,
            bucket: "lec".to_string(),
            prefix: None,
            task_name: None,
            accepted: true,
        },
        Row {
            case: "the shortest bucket name, never created",
            create: false,
            bucket: "led".to_string(),
            prefix: None,
            task_name: None,
            accepted: false,
        },
        Row {
            case: "the longest bucket name, created",
            create: true,
            bucket: longest_bucket_name(),
            prefix: Some("exported/"),
            task_name: None,
            accepted: true,
        },
        Row {
            case: "a dotted, digit-bearing bucket name, created",
            create: true,
            bucket: "logs.export.7.dots".to_string(),
            prefix: None,
            task_name: Some("dotted-export"),
            accepted: true,
        },
        Row {
            case: "a dotted bucket name that was never created",
            create: false,
            bucket: "logs.export.9.dots".to_string(),
            prefix: None,
            task_name: Some("dotted-ghost"),
            accepted: false,
        },
        Row {
            // Resolution is exact-name: a name that merely STARTS WITH an existing bucket's
            // name is a different bucket.
            case: "a name that only has an existing bucket's name as its prefix",
            create: false,
            bucket: format!("{existing}-extra"),
            prefix: None,
            task_name: None,
            accepted: false,
        },
        Row {
            // The prefix and the task name say where the objects go and what the task is
            // called; neither changes which bucket is resolved.
            case: "a prefix and a task name on a bucket that does not exist",
            create: false,
            bucket: "logs-export-gate-prefixed-ghost".to_string(),
            prefix: Some("data/"),
            task_name: Some("prefixed-ghost"),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn an_export_task_is_created_only_for_a_bucket_s3_holds() {
    let router = build_emulator();
    logs(&router, "setup", "CreateLogGroup", &json!({"logGroupName": GROUP})).await;
    logs(
        &router,
        "setup",
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": "stream"}),
    )
    .await;
    // The window the exports are asked for, in the epoch milliseconds the member documents.
    let to = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_millis(),
    )
    .expect("the epoch milliseconds fit in an i64");

    let mut minted: Vec<(String, String)> = Vec::new();
    for row in rows() {
        if row.create {
            create_bucket(&router, &row.bucket).await;
        }
        let mut request = json!({
            "logGroupName": GROUP,
            "destination": row.bucket,
            "from": 0,
            "to": to,
        });
        if let Some(prefix) = row.prefix {
            request["destinationPrefix"] = json!(prefix);
        }
        if let Some(task_name) = row.task_name {
            request["taskName"] = json!(task_name);
        }

        let (status, body) = logs_call(&router, "CreateExportTask", &request).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: a bucket S3 holds must be accepted, got {status} with {body}",
                row.case
            );
            let task_id = body["taskId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {:?}: an accepted create answers a taskId, got {body}", row.case)
                })
                .to_string();

            // Round trip: the task is readable back through DescribeExportTasks, which
            // answers from the same stored record, carrying the bucket it was created for.
            let reported = export_tasks(&router).await;
            assert!(
                reported.contains(&(task_id.clone(), row.bucket.clone())),
                "case {:?}: the created task must be reported against its bucket, got \
                 {reported:?}",
                row.case
            );
            minted.push((task_id.clone(), row.bucket.clone()));

            // "Each account can only have one active (RUNNING or PENDING) export task at a
            // time": the accepted task is cancelled so the next row's create is judged on
            // its bucket rather than on the account's one active slot.
            logs(
                &router,
                row.case,
                "CancelExportTask",
                &json!({"taskId": task_id}),
            )
            .await;
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, INVALID_PARAMETER),
                "case {:?}: a bucket S3 does not hold must be refused 400 \
                 {INVALID_PARAMETER}, got {status} with {body}",
                row.case
            );
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                BUCKET_MISSING_MESSAGE,
                "case {:?}: expected the absent-bucket refusal, got {body}",
                row.case
            );
            // A refused create persists nothing: no task names that bucket, and the tasks
            // the accepted rows minted are all there still is.
            let mut reported = export_tasks(&router).await;
            assert!(
                reported.iter().all(|(_, bucket)| *bucket != row.bucket),
                "case {:?}: a refused create must mint no task, got {reported:?}",
                row.case
            );
            reported.sort();
            let mut expected = minted.clone();
            expected.sort();
            assert_eq!(
                reported, expected,
                "case {:?}: a refused create leaves the store as it was, got {reported:?}",
                row.case
            );
        }
    }

    // The identical request, sent twice around one bucket's creation: refused first,
    // accepted after — so what decides is the bucket's existence and nothing about the
    // request itself.
    let bucket = "logs-export-gate-created-later";
    let request = json!({
        "logGroupName": GROUP,
        "destination": bucket,
        "from": 0,
        "to": to,
    });
    let (status, body) = logs_call(&router, "CreateExportTask", &request).await;
    assert_eq!(
        (status, body["message"].as_str().unwrap_or_default()),
        (400, BUCKET_MISSING_MESSAGE),
        "before the bucket exists the create must be refused, got {status} with {body}"
    );

    create_bucket(&router, bucket).await;

    let (status, body) = logs_call(&router, "CreateExportTask", &request).await;
    assert_eq!(
        status, 200,
        "once the bucket exists the identical create must succeed, got {status} with {body}"
    );
    let task_id = body["taskId"].as_str().unwrap_or_default().to_string();
    assert!(
        export_tasks(&router)
            .await
            .contains(&(task_id, bucket.to_string())),
        "the accepted create must be reported against the bucket it names"
    );
}

/// The awsJson error code of a refusal body (`__type`, which may carry a namespace).
fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or_default();
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}
