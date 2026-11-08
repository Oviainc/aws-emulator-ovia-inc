// SPDX-License-Identifier: CC0-1.0

//! A CloudWatch Logs subscription filter whose destination is a Firehose delivery stream
//! lands its log events in that stream's Amazon S3 destination.
//!
//! The rule under test: a log group's subscription filter forwards every event its
//! `filterPattern` selects to the resource `destinationArn` names, and a Firehose delivery
//! stream is one of those resources ("An Amazon Kinesis Data Firehose delivery stream that
//! belongs to the same account as the subscription filter" — `PutSubscriptionFilter`), so
//! the stream TAKES what CloudWatch Logs hands it — gzip-framed CloudWatch Logs envelopes —
//! and delivers its buffer to the Amazon S3 location its destination configuration names.
//! What the delivered object HOLDS is what that destination's enabled data processing leaves
//! behind: nothing at all configured, or disabled, leaves the gzip frames as they arrived;
//! `Decompression` inflates each frame; and `CloudWatchLogProcessing` with
//! `DataMessageExtraction` reduces each inflated `DATA_MESSAGE` envelope to its events'
//! `message` values, one per line, and drops a `CONTROL_MESSAGE` envelope, which carries no
//! log event of the account.
//!
//! Only a cross-service suite can pin this: the flow spans four services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — CloudWatch Logs for the group and the
//! filter, IAM for the role both the forwarding and the delivery are made under, Firehose
//! for the stream, and Amazon S3 for the object the delivery writes.
//!
//! The rows differ along what the rule generalises over: the processing configuration
//! (absent, present but disabled, decompression alone, the whole pipeline), and how many
//! events the batch carries.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode};

/// The Region every call below is signed for, and the one every ARN here names.
const REGION: &str = "us-west-2";

/// The bucket every stream here delivers into.
const BUCKET: &str = "cwl-firehose-delivery";

/// The role the forwarding and the delivery are both made under.
const ROLE: &str = "cwl-firehose-delivery-role";

/// The log stream every row writes its events to.
const LOG_STREAM: &str = "instance-one";

/// The subscription filter every row configures.
const FILTER: &str = "to-firehose";

/// The trust policy the delivery role carries: CloudWatch Logs forwards under it and
/// Firehose delivers under it, so both service principals may assume it.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"#,
    r#"["logs.amazonaws.com","firehose.amazonaws.com"]},"Action":"sts:AssumeRole"}]}"#
);

/// The role's permissions: the record write CloudWatch Logs makes on the delivery stream
/// (`PutSubscriptionFilter$roleArn`, "grants CloudWatch Logs permissions to deliver ingested
/// log events to the destination stream") and the object write the delivery makes.
const ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["firehose:PutRecord","firehose:PutRecordBatch","s3:PutObject"],"Resource":"*"}]}"#
);

/// Long enough to cover the Firehose emulator's chosen `CREATING` -> `ACTIVE` span, so the
/// forwarding below reaches a stream that can take a record.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(3_400);

/// How long a delivery is waited for before a row fails: far longer than the emulator's own
/// delivery span, so a loaded machine does not fail a row the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination prefix is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What a delivered object holds once the destination's processing has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Payload {
    /// The gzip frames CloudWatch Logs delivered, concatenated as they arrived.
    GzipFrames,
    /// Those frames inflated: the envelope documents themselves.
    Envelopes,
    /// Only the `message` of each forwarded log event, one per line.
    Messages,
}

/// One row: a log group, a stream, and the processing its destination configures.
struct Row {
    case: &'static str,
    /// Names this row's log group, stream and destination prefix, so rows cannot collide.
    name: &'static str,
    /// The destination's `ProcessingConfiguration`, or `None` for a destination that
    /// configures none.
    processing: Option<Value>,
    /// How many events the row's `PutLogEvents` carries.
    events: usize,
    expect: Payload,
}

/// The `Decompression` processor, with the compression format CloudWatch Logs frames its
/// payload in. Source: `smithy_query(query="shape", shape="ProcessorType,
/// ProcessorParameterName")`.
fn decompression() -> Value {
    json!({
        "Type": "Decompression",
        "Parameters": [{ "ParameterName": "CompressionFormat", "ParameterValue": "GZIP" }],
    })
}

/// The `CloudWatchLogProcessing` processor asking for message extraction.
fn message_extraction() -> Value {
    json!({
        "Type": "CloudWatchLogProcessing",
        "Parameters": [{ "ParameterName": "DataMessageExtraction", "ParameterValue": "true" }],
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a destination configuring no data processing at all",
            name: "untouched",
            processing: None,
            events: 12,
            expect: Payload::GzipFrames,
        },
        Row {
            case: "the whole pipeline configured but disabled",
            name: "disabled",
            processing: Some(json!({
                "Enabled": false,
                "Processors": [decompression(), message_extraction()],
            })),
            events: 3,
            expect: Payload::GzipFrames,
        },
        Row {
            case: "decompression alone",
            name: "inflated",
            processing: Some(json!({ "Enabled": true, "Processors": [decompression()] })),
            events: 1,
            expect: Payload::Envelopes,
        },
        Row {
            case: "decompression with message extraction",
            name: "extracted",
            processing: Some(json!({
                "Enabled": true,
                "Processors": [decompression(), message_extraction()],
            })),
            events: 12,
            expect: Payload::Messages,
        },
    ]
}

/// The message of event `index` of the row `name` writes: distinct per row and per event, so
/// an assertion names exactly what it did or did not find.
fn message(name: &str, index: usize) -> String {
    format!("ERROR {name} event {index}")
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The signature
/// is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One request to the service `signing_name` serves; answers its status and body bytes.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.body().to_vec())
}

/// One `awsJson1_1` operation that must succeed.
async fn json_call(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    target: &str,
    body: Value,
) -> Value {
    let (status, answered) = send(
        router,
        signing_name,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: {target} must succeed, got {status} {text}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One `awsQuery` IAM action that must succeed.
async fn iam(router: &MultiServiceRouter, case: &str, action: &str, parameters: &[(&str, &str)]) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = send(
        router,
        "iam",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: IAM {action} must succeed, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// Every key the bucket holds under `prefix`, read through `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
        Vec::new(),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: listing {prefix:?} must succeed, got {status} {text}"
    );
    let mut keys = elements(&text, "Key");
    keys.sort();
    keys
}

/// The bytes the bucket holds at `key`, read through `GetObject`.
async fn object(router: &MultiServiceRouter, case: &str, key: &str) -> Vec<u8> {
    let (status, body) = send(router, "s3", "GET", &format!("/{BUCKET}/{key}"), &[], Vec::new()).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: reading {key:?} must succeed, got {status} {}",
        String::from_utf8_lossy(&body)
    );
    body
}

/// Every delivered object under `prefix`, concatenated in key order, once at least one is
/// there — or a failure naming the row once [`DELIVERY_DEADLINE`] has passed.
async fn delivered(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<u8> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = keys_under(router, case, prefix).await;
        if !keys.is_empty() {
            let mut body = Vec::new();
            for key in &keys {
                body.extend_from_slice(&object(router, case, key).await);
            }
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least one delivered object under {prefix:?}, got none"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The emulator the suite drives: the four services the flow spans, on ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

#[tokio::test]
async fn a_subscription_filters_firehose_destination_delivers_its_events_to_s3() {
    let router = emulator();
    let account = engine::emulator_account_id();
    let role_arn = format!("arn:aws:iam::{account}:role/{ROLE}");
    let rows = rows();

    // The role both writes are made under, and the bucket every stream delivers into.
    iam(
        &router,
        "setup",
        "CreateRole",
        &[("RoleName", ROLE), ("AssumeRolePolicyDocument", TRUST_POLICY)],
    )
    .await;
    iam(
        &router,
        "setup",
        "PutRolePolicy",
        &[
            ("RoleName", ROLE),
            ("PolicyName", "delivery"),
            ("PolicyDocument", ROLE_POLICY),
        ],
    )
    .await;
    let (status, body) = send(
        &router,
        "s3",
        "PUT",
        &format!("/{BUCKET}"),
        &[],
        format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}\
             </LocationConstraint></CreateBucketConfiguration>"
        )
        .into_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "setup: the destination bucket must be created, got {status} {}",
        String::from_utf8_lossy(&body)
    );

    // Every row's stream, log group and log stream, created together so one wait covers the
    // span the creates armed.
    for row in &rows {
        let mut destination = json!({
            "RoleARN": role_arn,
            "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
            "Prefix": format!("{}/", row.name),
            // The shortest buffering the hints admit, so a row waits for the emulator's own
            // delivery span and nothing longer.
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        });
        if let Some(processing) = &row.processing {
            destination["ProcessingConfiguration"] = processing.clone();
        }
        json_call(
            &router,
            row.case,
            "firehose",
            "Firehose_20150804.CreateDeliveryStream",
            json!({
                "DeliveryStreamName": row.name,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": destination,
            }),
        )
        .await;
        json_call(
            &router,
            row.case,
            "logs",
            "Logs_20140328.CreateLogGroup",
            json!({ "logGroupName": format!("/aws/emulator/{}", row.name) }),
        )
        .await;
        json_call(
            &router,
            row.case,
            "logs",
            "Logs_20140328.CreateLogStream",
            json!({
                "logGroupName": format!("/aws/emulator/{}", row.name),
                "logStreamName": LOG_STREAM,
            }),
        )
        .await;
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for row in &rows {
        let case = row.case;
        let group = format!("/aws/emulator/{}", row.name);
        json_call(
            &router,
            case,
            "logs",
            "Logs_20140328.PutSubscriptionFilter",
            json!({
                "logGroupName": group,
                "filterName": FILTER,
                "filterPattern": "",
                "destinationArn":
                    format!("arn:aws:firehose:{REGION}:{account}:deliverystream/{}", row.name),
                "roleArn": role_arn,
            }),
        )
        .await;
        // One ingestion instant for the whole batch, inside the ingestion window.
        let timestamp = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the Unix epoch")
                .as_millis(),
        )
        .expect("the epoch milliseconds fit in an i64")
            - 60 * 60 * 1_000;
        let events: Vec<Value> = (0..row.events)
            .map(|index| json!({ "timestamp": timestamp, "message": message(row.name, index) }))
            .collect();
        json_call(
            &router,
            case,
            "logs",
            "Logs_20140328.PutLogEvents",
            json!({
                "logGroupName": group,
                "logStreamName": LOG_STREAM,
                "logEvents": events,
            }),
        )
        .await;
    }

    for row in &rows {
        let case = row.case;
        let body = delivered(&router, case, &format!("{}/", row.name)).await;
        let text = String::from_utf8_lossy(&body).to_string();
        let messages: Vec<String> = (0..row.events)
            .map(|index| message(row.name, index))
            .collect();
        match row.expect {
            Payload::GzipFrames => {
                assert_eq!(
                    body.get(..2),
                    Some([0x1f, 0x8b].as_slice()),
                    "case {case:?}: an untransformed record keeps the gzip framing CloudWatch \
                     Logs delivers, got {:?}",
                    body.get(..2)
                );
                assert!(
                    !text.contains("\"messageType\""),
                    "case {case:?}: an untransformed record is not readable as the envelope \
                     document, got {text:?}"
                );
            }
            Payload::Envelopes => {
                assert!(
                    text.contains("\"messageType\":\"DATA_MESSAGE\"")
                        && text.contains(&format!("\"logGroup\":\"/aws/emulator/{}\"", row.name)),
                    "case {case:?}: an inflated record is the envelope document, got {text:?}"
                );
                for message in &messages {
                    assert!(
                        text.contains(message),
                        "case {case:?}: the inflated envelope carries {message:?}, got {text:?}"
                    );
                }
            }
            Payload::Messages => {
                let expected: String = messages
                    .iter()
                    .map(|message| format!("{message}\n"))
                    .collect();
                assert_eq!(
                    text, expected,
                    "case {case:?}: expected only the extracted messages {expected:?}, got \
                     {text:?}"
                );
            }
        }
    }
}
