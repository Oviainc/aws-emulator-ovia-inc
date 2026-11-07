// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OBJECTS an export task writes to its destination bucket.
//!
//! CloudWatch Logs and Amazon S3 are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so an object this suite reads back is one the object store itself
//! holds, written by the export task over the in-process callback and not by anything here.
//! S3 is driven the way a client drives it, path-style over the wire (`GET
//! /<bucket>?list-type=2`, `GET /<bucket>/<key>`).
//!
//! The rule under test: an export task writes the named log group's events whose timestamp
//! falls in its window to gzip objects under its `destinationPrefix` in its destination
//! bucket, and does so before it reports a terminal status — so a client that waits for
//! `COMPLETED` and reads the prefix finds the exported data. Events outside the window, and
//! streams the task's `logStreamNamePrefix` excludes, are not written.
//!
//! The table spans the classes the rule generalises over: where the window falls over the
//! events, how many streams the group has, whether a stream prefix narrows them, and what
//! prefix the destination names.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{now_ms, past_completion};

/// The Region every request here is signed for.
const REGION: &str = "eu-west-2";

/// The host a path-style S3 request names.
const HOST: &str = "s3.eu-west-2.amazonaws.com";

/// Milliseconds in one hour.
const HOUR: i64 = 60 * 60 * 1_000;

/// CloudWatch Logs and Amazon S3 on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// The authorization header naming `service`'s own credential scope.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        )
    }

    /// One CloudWatch Logs call: its status and body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .header("authorization", Self::authorization("logs"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One path-style Amazon S3 request: its status and body bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", Self::authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// `PUT /<bucket>` — S3's `CreateBucket`, naming the Region this suite signs for.
    async fn create_bucket(&self, case: &str, bucket: &str) {
        let (status, body) = self
            .s3(
                "PUT",
                &format!("/{bucket}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: create bucket {bucket}: got {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Every key under `prefix`, read through `ListObjectsV2`.
    async fn keys_under(&self, case: &str, bucket: &str, prefix: &str) -> Vec<String> {
        let (status, body) = self
            .s3(
                "GET",
                &format!("/{bucket}?list-type=2&prefix={prefix}"),
                b"",
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: list {bucket}/{prefix}: got {}",
            String::from_utf8_lossy(&body)
        );
        let document = String::from_utf8_lossy(&body).into_owned();
        document
            .split("<Key>")
            .skip(1)
            .filter_map(|tail| tail.split("</Key>").next())
            .map(str::to_string)
            .collect()
    }

    /// The bytes `bucket` holds at `key`, read through `GetObject`.
    async fn object(&self, case: &str, bucket: &str, key: &str) -> Vec<u8> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            status,
            200,
            "case {case:?}: get {bucket}/{key}: got {}",
            String::from_utf8_lossy(&body)
        );
        body.to_vec()
    }

    /// The exported text under `prefix`: every object there, gunzipped and concatenated.
    async fn exported_text(&self, case: &str, bucket: &str, prefix: &str) -> String {
        let mut text = String::new();
        for key in self.keys_under(case, bucket, prefix).await {
            let body = self.object(case, bucket, &key).await;
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(body.as_slice())
                .read_to_end(&mut decoded)
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: the object at {key} must be gzip: {error}")
                });
            text.push_str(&String::from_utf8_lossy(&decoded));
        }
        text
    }

    /// The status code `DescribeExportTasks` reports for `task_id`.
    async fn task_status(&self, case: &str, task_id: &str) -> String {
        let page = self
            .logs_ok(case, "DescribeExportTasks", &json!({"taskId": task_id}))
            .await;
        page["exportTasks"][0]["status"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an export task reports a status, got {page}"))
            .to_string()
    }
}

/// One row of the table.
struct Row {
    /// What the row is about, named in every assertion.
    case: &'static str,
    /// The destination prefix the create names, or `None` to take the documented default.
    prefix: Option<&'static str>,
    /// The `logStreamNamePrefix` the create names, when it narrows the streams.
    stream_prefix: Option<&'static str>,
    /// The window, as HALF hours either side of the clock, so a row can sit between two
    /// planted events.
    window: (i64, i64),
    /// The messages that must appear in the exported text.
    exported: &'static [&'static str],
    /// The messages that must NOT appear in it.
    withheld: &'static [&'static str],
}

/// The events every row plants, as `(stream, hours before the clock, message)`.
const PLANTED: &[(&str, i64, &str)] = &[
    ("app/one", 4, "OUTSIDE-BEFORE"),
    ("app/one", 3, "INSIDE-ONE"),
    ("app/one", 2, "INSIDE-TWO"),
    ("app/one", 1, "OUTSIDE-AFTER"),
    ("sys/two", 3, "INSIDE-OTHER-STREAM"),
];

/// An export task writes the window's events under the destination prefix, and has written
/// them by the time it reports a terminal status.
#[tokio::test]
async fn an_export_task_writes_the_windows_events_under_the_destination_prefix() {
    let rows: [Row; 4] = [
        Row {
            case: "a window over the middle of the events",
            prefix: Some("exported"),
            stream_prefix: None,
            // [-3.5h, -1.5h): the two middle events of each stream.
            window: (-7, -3),
            exported: &["INSIDE-ONE", "INSIDE-TWO", "INSIDE-OTHER-STREAM"],
            withheld: &["OUTSIDE-BEFORE", "OUTSIDE-AFTER"],
        },
        Row {
            case: "a window over every event",
            prefix: Some("deep/prefix/segments"),
            stream_prefix: None,
            window: (-10, 1),
            exported: &[
                "OUTSIDE-BEFORE",
                "INSIDE-ONE",
                "INSIDE-TWO",
                "OUTSIDE-AFTER",
                "INSIDE-OTHER-STREAM",
            ],
            withheld: &[],
        },
        Row {
            case: "a stream prefix narrowing the export to one stream",
            prefix: Some("one-stream"),
            stream_prefix: Some("app/"),
            window: (-10, 1),
            exported: &["INSIDE-ONE", "INSIDE-TWO"],
            withheld: &["INSIDE-OTHER-STREAM"],
        },
        Row {
            case: "no destination prefix named, so the documented default is used",
            prefix: None,
            stream_prefix: None,
            window: (-7, -3),
            exported: &["INSIDE-ONE"],
            withheld: &["OUTSIDE-BEFORE"],
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let group = "export/group";
        let bucket = "export-destination";
        emulator.create_bucket(row.case, bucket).await;
        emulator
            .logs_ok(row.case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;

        let clock = now_ms();
        for stream in ["app/one", "sys/two"] {
            emulator
                .logs_ok(
                    row.case,
                    "CreateLogStream",
                    &json!({"logGroupName": group, "logStreamName": stream}),
                )
                .await;
            let events: Vec<Value> = PLANTED
                .iter()
                .filter(|(planted, _, _)| *planted == stream)
                .map(|(_, hours_before, message)| {
                    json!({"timestamp": clock - hours_before * HOUR, "message": message})
                })
                .collect();
            emulator
                .logs_ok(
                    row.case,
                    "PutLogEvents",
                    &json!({
                        "logGroupName": group,
                        "logStreamName": stream,
                        "logEvents": Value::Array(events),
                    }),
                )
                .await;
        }

        // The window is given in half hours either side of the clock, so a row can sit
        // between two planted events.
        let (from_half_hours, to_half_hours) = row.window;
        let mut create = json!({
            "logGroupName": group,
            "from": clock + from_half_hours * HOUR / 2,
            "to": clock + to_half_hours * HOUR / 2,
            "destination": bucket,
        });
        if let Some(prefix) = row.prefix {
            create["destinationPrefix"] = json!(prefix);
        }
        if let Some(prefix) = row.stream_prefix {
            create["logStreamNamePrefix"] = json!(prefix);
        }
        let started = emulator
            .logs_ok(row.case, "CreateExportTask", &create)
            .await;
        let task_id = started["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a started export reports a taskId", row.case))
            .to_string();

        // Wait past both spans the create armed: the task is terminal by then, so this is
        // exactly what a client that waits for the terminal status observes.
        tokio::time::sleep(past_completion()).await;
        let status = emulator.task_status(row.case, &task_id).await;
        assert_eq!(
            status, "COMPLETED",
            "case {:?}: the export must complete, got {status}",
            row.case
        );

        // "If you don't specify a value, the default is exportedlogs".
        let prefix = row.prefix.unwrap_or("exportedlogs");
        let keys = emulator.keys_under(row.case, bucket, prefix).await;
        assert!(
            !keys.is_empty(),
            "case {:?}: the export must write an object under {prefix}/, got {keys:?}",
            row.case
        );
        for key in &keys {
            assert!(
                key.starts_with(&format!("{prefix}/")) && key.ends_with(".gz"),
                "case {:?}: every exported object is a gzip object under {prefix}/, got {key}",
                row.case
            );
            assert!(
                key.contains(&task_id),
                "case {:?}: an exported object is keyed by the task that wrote it, got {key}",
                row.case
            );
        }

        let text = emulator.exported_text(row.case, bucket, prefix).await;
        for message in row.exported {
            assert!(
                text.contains(message),
                "case {:?}: the exported text must carry {message}, got {text:?}",
                row.case
            );
        }
        for message in row.withheld {
            assert!(
                !text.contains(message),
                "case {:?}: the exported text must not carry {message}, got {text:?}",
                row.case
            );
        }
        // Every exported line carries its event's own timestamp beside the message, so the
        // exported data is readable as the events it came from.
        for line in text.lines() {
            assert!(
                line.split_once(' ').is_some_and(|(stamp, rest)| {
                    stamp.ends_with('Z') && !rest.is_empty()
                }),
                "case {:?}: an exported line is a timestamp and a message, got {line:?}",
                row.case
            );
        }
    }
}

/// The window's own bounds: an event stamped exactly at `from` is exported and one stamped
/// exactly at `to` is not, so two adjacent windows neither duplicate an event nor drop it.
#[tokio::test]
async fn the_window_includes_its_start_and_excludes_its_end() {
    let case = "an event on each bound of the window";
    let emulator = Emulator::new();
    let group = "export/bounds";
    let bucket = "export-bounds";
    emulator.create_bucket(case, bucket).await;
    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs_ok(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": "stream"}),
        )
        .await;
    let clock = now_ms();
    let from = clock - 2 * HOUR;
    let to = clock - HOUR;
    emulator
        .logs_ok(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": "stream",
                "logEvents": [
                    {"timestamp": from, "message": "AT-FROM"},
                    {"timestamp": to, "message": "AT-TO"},
                ],
            }),
        )
        .await;

    let started = emulator
        .logs_ok(
            case,
            "CreateExportTask",
            &json!({
                "logGroupName": group,
                "from": from,
                "to": to,
                "destination": bucket,
                "destinationPrefix": "bounds",
            }),
        )
        .await;
    let task_id = started["taskId"]
        .as_str()
        .expect("a started export reports a taskId")
        .to_string();

    tokio::time::sleep(past_completion()).await;
    assert_eq!(
        emulator.task_status(case, &task_id).await,
        "COMPLETED",
        "case {case:?}: the export must complete"
    );
    let text = emulator.exported_text(case, bucket, "bounds").await;
    assert!(
        text.contains("AT-FROM"),
        "case {case:?}: the event on the window's start is exported, got {text:?}"
    );
    assert!(
        !text.contains("AT-TO"),
        "case {case:?}: the event on the window's end belongs to the next window, got {text:?}"
    );
}

#[tokio::test]
async fn an_export_whose_window_selects_no_event_writes_no_object() {
    let case = "a window before every event";
    let emulator = Emulator::new();
    let group = "export/empty";
    let bucket = "export-empty";
    emulator.create_bucket(case, bucket).await;
    emulator
        .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs_ok(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": "stream"}),
        )
        .await;
    let clock = now_ms();
    emulator
        .logs_ok(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": "stream",
                "logEvents": [{"timestamp": clock - HOUR, "message": "ONLY-EVENT"}],
            }),
        )
        .await;

    let started = emulator
        .logs_ok(
            case,
            "CreateExportTask",
            &json!({
                "logGroupName": group,
                "from": clock - 6 * HOUR,
                "to": clock - 5 * HOUR,
                "destination": bucket,
                "destinationPrefix": "nothing",
            }),
        )
        .await;
    let task_id = started["taskId"]
        .as_str()
        .expect("a started export reports a taskId")
        .to_string();

    tokio::time::sleep(past_completion()).await;
    assert_eq!(
        emulator.task_status(case, &task_id).await,
        "COMPLETED",
        "case {case:?}: the export still completes"
    );
    assert_eq!(
        emulator.keys_under(case, bucket, "nothing").await,
        Vec::<String>::new(),
        "case {case:?}: no event falls in the window, so no object is written"
    );
}
