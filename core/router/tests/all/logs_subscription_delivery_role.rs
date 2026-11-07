// SPDX-License-Identifier: CC0-1.0

//! CloudWatch Logs delivers a subscription filter's log events only under a role IAM allows
//! the destination's write to.
//!
//! The rule under test: CloudWatch Logs writes to a subscription destination UNDER the IAM
//! role the filter's configuration names (`PutSubscriptionFilter$roleArn`, "an IAM role that
//! grants CloudWatch Logs permissions to deliver ingested log events to the destination
//! stream"), so a record reaches that destination only where the permissions in force on the
//! role allow the destination service's own write action — `kinesis:PutRecord` for a data
//! stream — on that destination resource. A role those permissions allow nothing writes
//! nothing: neither the health-check record the put makes nor the envelope the next batch
//! owes. Where IAM gives no answer at all — this account holds no role under that name, a
//! delivery role being routinely provisioned out of band — nothing is withheld. Either way
//! the batch is ingested and stays readable, because delivery is best effort and the put is
//! accepted.
//!
//! This suite needs THREE services on one engine — CloudWatch Logs reaches IAM and Kinesis
//! through the in-process callback, so a single-service router has neither to ask — which is
//! why it lives here rather than in the CloudWatch Logs crate. The rows span what the rule
//! generalises over: whether the role carries any identity policy at all, whether the grant is
//! inline or comes from an attached managed policy, whether its action and resource elements
//! cover this call (the exact action, a wildcard action on every resource, the sibling batch
//! action, another stream's ARN), whether an explicit deny stands beside the grant, and
//! whether IAM holds the named role at all. The half of the rule that needs no IAM — every
//! form of undecidable role withholding nothing — is the CloudWatch Logs crate's own
//! `tests/all/subscription_delivery_authorization.rs`.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_id, ACTIVATION_DELAY, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every request below is signed for, and the one the destination ARNs name.
const REGION: &str = "us-west-2";

/// Long enough to cover the Kinesis emulator's `CREATING` -> `ACTIVE` span, so the puts below
/// land on streams that can take a record.
const PAST_ACTIVATION: std::time::Duration =
    ACTIVATION_DELAY.saturating_add(std::time::Duration::from_millis(600));

/// The trust policy every role here carries: CloudWatch Logs may assume it. Being assumable is
/// not what this suite grades — what the role is ALLOWED to do once assumed is.
const TRUST_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
     \"Principal\":{\"Service\":\"logs.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}";

/// The action the destination's service authorizes one delivered record under, and the sibling
/// action that does not cover it: Kinesis `PutRecord` writes one record, `PutRecords` is the
/// batch operation. Source: `PutDestination$roleArn` ("grants CloudWatch Logs permissions to
/// call the Amazon Kinesis PutRecord operation on the destination stream").
const PUT_RECORD: &str = "kinesis:PutRecord";
const PUT_RECORDS: &str = "kinesis:PutRecords";

/// The line every row writes, which an empty filter pattern selects.
const LINE: &str = "ERROR disk full";

/// The filter every row configures, and the log stream it writes to.
const FILTER: &str = "to-kinesis";
const LOG_STREAM: &str = "instance-one";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The signature
/// is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Send one request and answer its status with the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    content_type: &str,
    target: Option<&str>,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action that must succeed: `POST /` with the parameters in the form
/// body. Every value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
fn iam(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    let (status, answered) = send(
        router,
        runtime,
        "iam",
        "application/x-www-form-urlencoded",
        None,
        body,
    );
    assert_eq!(
        status, 200,
        "case {case:?}: IAM {action} must succeed, got {status} {answered}"
    );
}

/// Drive one `awsJson1_1` operation of the service `signing_name` serves, answering its status
/// and parsed body.
fn call(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    prefix: &str,
    operation: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, text) = send(
        router,
        runtime,
        signing_name,
        "application/x-amz-json-1.1",
        Some(&format!("{prefix}.{operation}")),
        body.to_string(),
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// One CloudWatch Logs operation that must succeed.
fn logs(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    operation: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let (status, parsed) = call(router, runtime, "logs", "Logs_20140328", operation, body);
    assert_eq!(
        status, 200,
        "case {case:?}: logs {operation} must succeed, got {status} {parsed}"
    );
    parsed
}

/// One Kinesis operation that must succeed.
fn kinesis(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    operation: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let (status, parsed) = call(
        router,
        runtime,
        "kinesis",
        "Kinesis_20131202",
        operation,
        body,
    );
    assert_eq!(
        status, 200,
        "case {case:?}: kinesis {operation} must succeed, got {status} {parsed}"
    );
    parsed
}

/// The PARTITION KEY of every record the one-shard stream `stream` holds, from its oldest
/// retained one.
///
/// The key is what tells the two writes of one row apart without decompressing anything: a
/// `PutSubscriptionFilter`'s health check is keyed on the log group it validates the filter
/// for, and an envelope of log data grouped by log stream on the log stream it came from
/// ("By default, log data is grouped by log stream").
fn delivered_keys(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    stream: &str,
) -> Vec<String> {
    let iterator = kinesis(
        router,
        runtime,
        case,
        "GetShardIterator",
        &serde_json::json!({
            "StreamName": stream,
            "ShardId": shard_id(0),
            "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
        }),
    )["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetShardIterator reports an iterator"))
        .to_string();
    let answer = kinesis(
        router,
        runtime,
        case,
        "GetRecords",
        &serde_json::json!({"ShardIterator": iterator, "Limit": 100}),
    );
    answer["Records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            record["PartitionKey"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a record carries a PartitionKey"))
                .to_string()
        })
        .collect()
}

/// The ARN of the Kinesis data stream `stream`, in the Region every call here is signed for.
fn stream_arn(stream: &str) -> String {
    format!(
        "arn:aws:kinesis:{REGION}:{}:stream/{stream}",
        engine::emulator_account_id()
    )
}

/// The IAM role ARN of this account that `name` names. An IAM ARN carries no Region.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// The ARN of the customer-managed policy `name`, the form `AttachRolePolicy$PolicyArn` names
/// one by.
fn policy_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:policy/{name}", engine::emulator_account_id())
}

/// One identity-based policy document over the statements it is given, each granting or
/// denying one action on one resource. Written with no `&`, `=`, `+` or `%`, so it travels in
/// an `awsQuery` form body as written.
fn document(statements: &[(&str, &str, String)]) -> String {
    let written: Vec<String> = statements
        .iter()
        .map(|(effect, action, resource)| {
            format!(
                "{{\"Effect\":\"{effect}\",\"Action\":\"{action}\",\"Resource\":\"{resource}\"}}"
            )
        })
        .collect();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        written.join(",")
    )
}

/// One row: the role the filter is configured with, the documents IAM holds for it, and
/// whether the destination must end up holding the writes.
struct Row {
    case: &'static str,
    /// The row's own suffix: its role, log group and Kinesis stream are all named from it, so
    /// every row is isolated from every other.
    name: &'static str,
    /// Whether IAM holds a role under that name at all.
    held: bool,
    /// The inline policy document embedded in the role, if any.
    inline: Option<String>,
    /// The document of a customer-managed policy created and ATTACHED to the role, if any.
    attached: Option<String>,
    /// Whether the delivery and the put's health check must reach the destination.
    delivers: bool,
}

fn rows() -> Vec<Row> {
    let own_stream = |name: &str| stream_arn(&format!("stream-{name}"));
    vec![
        // The recorded case: the role is assumable by CloudWatch Logs and allowed nothing.
        Row {
            case: "a role carrying no identity policy at all",
            name: "no-policy",
            held: true,
            inline: None,
            attached: None,
            delivers: false,
        },
        Row {
            case: "an inline policy allowing the write on the destination",
            name: "inline-allow",
            held: true,
            inline: Some(document(&[(
                "Allow",
                PUT_RECORD,
                own_stream("inline-allow"),
            )])),
            attached: None,
            delivers: true,
        },
        Row {
            case: "an inline policy allowing the write on ANOTHER stream",
            name: "other-resource",
            held: true,
            inline: Some(document(&[(
                "Allow",
                PUT_RECORD,
                stream_arn("stream-elsewhere"),
            )])),
            attached: None,
            delivers: false,
        },
        Row {
            case: "an inline policy allowing every action of the service on every resource",
            name: "wildcard-allow",
            held: true,
            inline: Some(document(&[("Allow", "kinesis:*", "*".to_string())])),
            attached: None,
            delivers: true,
        },
        Row {
            case: "an inline policy allowing only the batch action",
            name: "sibling-action",
            held: true,
            inline: Some(document(&[(
                "Allow",
                PUT_RECORDS,
                own_stream("sibling-action"),
            )])),
            attached: None,
            delivers: false,
        },
        Row {
            case: "an attached managed policy allowing the write",
            name: "managed-allow",
            held: true,
            inline: None,
            attached: Some(document(&[(
                "Allow",
                PUT_RECORD,
                own_stream("managed-allow"),
            )])),
            delivers: true,
        },
        Row {
            case: "a grant and an explicit deny of the same write",
            name: "allow-and-deny",
            held: true,
            inline: Some(document(&[
                ("Allow", PUT_RECORD, own_stream("allow-and-deny")),
                ("Deny", PUT_RECORD, own_stream("allow-and-deny")),
            ])),
            attached: None,
            delivers: false,
        },
        // Nothing is known about a role this store holds nothing under, so nothing is withheld
        // on its account.
        Row {
            case: "a role IAM holds nothing under",
            name: "absent-from-iam",
            held: false,
            inline: None,
            attached: None,
            delivers: true,
        },
    ]
}

#[test]
fn a_subscription_filter_delivers_only_what_its_role_is_allowed_to_write() {
    std::thread::Builder::new()
        .name("logs-subscription-delivery-role".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let rows = rows();

            // Every row's role, destination stream and log group, before anything is put: the
            // streams are created together so one wait covers the span their creates armed.
            for row in &rows {
                if row.held {
                    iam(
                        &router,
                        &runtime,
                        row.case,
                        "CreateRole",
                        &[
                            ("RoleName", row.name),
                            ("AssumeRolePolicyDocument", TRUST_POLICY),
                        ],
                    );
                    if let Some(inline) = &row.inline {
                        iam(
                            &router,
                            &runtime,
                            row.case,
                            "PutRolePolicy",
                            &[
                                ("RoleName", row.name),
                                ("PolicyName", "delivery"),
                                ("PolicyDocument", inline.as_str()),
                            ],
                        );
                    }
                    if let Some(attached) = &row.attached {
                        iam(
                            &router,
                            &runtime,
                            row.case,
                            "CreatePolicy",
                            &[
                                ("PolicyName", row.name),
                                ("PolicyDocument", attached.as_str()),
                            ],
                        );
                        iam(
                            &router,
                            &runtime,
                            row.case,
                            "AttachRolePolicy",
                            &[
                                ("RoleName", row.name),
                                ("PolicyArn", policy_arn(row.name).as_str()),
                            ],
                        );
                    }
                }
                kinesis(
                    &router,
                    &runtime,
                    row.case,
                    "CreateStream",
                    &serde_json::json!({
                        "StreamName": format!("stream-{}", row.name),
                        "ShardCount": 1,
                    }),
                );
                let group = format!("/aws/emulator/{}", row.name);
                logs(
                    &router,
                    &runtime,
                    row.case,
                    "CreateLogGroup",
                    &serde_json::json!({"logGroupName": group}),
                );
                logs(
                    &router,
                    &runtime,
                    row.case,
                    "CreateLogStream",
                    &serde_json::json!({
                        "logGroupName": group,
                        "logStreamName": LOG_STREAM,
                    }),
                );
            }
            std::thread::sleep(PAST_ACTIVATION);

            for row in &rows {
                let case = row.case;
                let stream = format!("stream-{}", row.name);
                let group = format!("/aws/emulator/{}", row.name);
                // The put is accepted whatever the role allows: what the role decides is what
                // reaches the destination, not whether the filter is configured.
                logs(
                    &router,
                    &runtime,
                    case,
                    "PutSubscriptionFilter",
                    &serde_json::json!({
                        "logGroupName": group,
                        "filterName": FILTER,
                        "filterPattern": "",
                        "destinationArn": stream_arn(&stream),
                        "roleArn": role_arn(row.name),
                    }),
                );
                // One in-window event: the ingestion window is 14 days back, so an hour ago is
                // comfortably inside it.
                let timestamp = i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("the clock is after the Unix epoch")
                        .as_millis(),
                )
                .expect("the epoch milliseconds fit in an i64")
                    - 60 * 60 * 1_000;
                logs(
                    &router,
                    &runtime,
                    case,
                    "PutLogEvents",
                    &serde_json::json!({
                        "logGroupName": group,
                        "logStreamName": LOG_STREAM,
                        "logEvents": [{"timestamp": timestamp, "message": LINE}],
                    }),
                );

                let keys = delivered_keys(&router, &runtime, case, &stream);
                let expected: Vec<String> = if row.delivers {
                    // The health check the put wrote, keyed on the log group, then the
                    // envelope of the batch, keyed on the log stream.
                    vec![group.clone(), LOG_STREAM.to_string()]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    keys, expected,
                    "case {case:?}: expected the destination to hold {expected:?}, got {keys:?}"
                );

                // The batch is ingested whatever the delivery did: `PutLogEvents` answered it,
                // and whether a destination took the envelope is not part of that answer.
                let page = logs(
                    &router,
                    &runtime,
                    case,
                    "GetLogEvents",
                    &serde_json::json!({
                        "logGroupName": group,
                        "logStreamName": LOG_STREAM,
                    }),
                );
                let messages: Vec<String> = page["events"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|event| {
                        event["message"]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: a stored event carries a message")
                            })
                            .to_string()
                    })
                    .collect();
                assert_eq!(
                    messages,
                    vec![LINE.to_string()],
                    "case {case:?}: the ingested line stays readable, got {messages:?}"
                );
            }
        })
        .expect("spawn the delivery-role thread")
        .join()
        .expect("every delivery must be authorized against the role it is made under");
}
