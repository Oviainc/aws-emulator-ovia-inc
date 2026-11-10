// SPDX-License-Identifier: CC0-1.0

//! An EventBridge rule whose target is an Amazon Data Firehose delivery stream puts the
//! event into that stream, and the stream delivers it to its Amazon S3 destination.
//!
//! The rule under test: for every target of a rule whose `EventPattern` matched,
//! EventBridge makes the API call that target's service is invoked by — `firehose:PutRecord`
//! for a delivery stream, carrying what the target receives as the record's data — and it
//! makes that call ACTING AS the target's `RoleArn` ("for EC2 instances, Kinesis data
//! streams, Step Functions state machines and API Gateway APIs, EventBridge relies on IAM
//! roles that you specify in the `RoleARN` argument in `PutTargets`"), a delivery stream
//! being one of the kinds no resource-based policy governs. So a role this service can
//! assume and whose policies allow that call delivers the event, while a role it cannot
//! assume, and one whose policies withhold the call, delivers nothing at all — and what the
//! stream then writes to its destination is what the TARGET received, the whole envelope or
//! the constant `Input` the target carries instead.
//!
//! Only a cross-service suite can pin this: the flow spans four services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — EventBridge for the rule and the
//! publication, IAM for the roles both calls are made under, Firehose for the stream, and
//! Amazon S3 for the object the stream's delivery writes.
//!
//! The rows differ along what the rule generalises over: the target role (one that may make
//! the call, one that may not, one that cannot be assumed) and what the target receives (the
//! published envelope, or its own constant input).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{elements, form_encode};

/// The Region every call below is signed for, and the one every ARN here names.
const REGION: &str = "us-west-2";

/// The bucket every stream here delivers into.
const BUCKET: &str = "events-firehose-delivery";

/// The role every STREAM delivers to that bucket under: Firehose has to be able to assume
/// it, and it has to allow the object write the delivery makes.
const DELIVERY_ROLE: &str = "firehose-delivery-role";

/// The source and detail type every rule below matches on, and the detail each published
/// event carries.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Created";
const DETAIL: &str = r#"{"orderId":"A-1"}"#;

/// The constant one row's target receives instead of the envelope
/// (`Target$Input`: "valid JSON text passed to the target").
const CONSTANT_INPUT: &str = r#"{"fixed":"constant-input"}"#;

/// The action EventBridge's call on a delivery stream performs, which the target's role has
/// to be allowed to make.
const PUT_RECORD_ACTION: &str = "firehose:PutRecord";

/// The service principal EventBridge assumes a target's role as, and one that is not this
/// service: a role trusting only the latter cannot be assumed here.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const OTHER_PRINCIPAL: &str = "ec2.amazonaws.com";

/// Long enough to cover the Firehose emulator's chosen `CREATING` -> `ACTIVE` span, so the
/// publication below reaches a stream that can take a record.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(3_400);

/// How long a delivery is waited for before a row fails: far longer than the emulator's own
/// delivery span, so a loaded machine does not fail a row the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination prefix is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What a row expects of its destination prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The whole event envelope reached the stream and was delivered.
    Envelope,
    /// The target's own constant input reached the stream and was delivered.
    Constant,
    /// Nothing reached the stream, so the destination holds nothing.
    Nothing,
}

/// One row: the target's role, what the target receives, and what its destination holds.
struct Row {
    case: &'static str,
    /// Names this row's stream, role, rule and destination prefix, so rows cannot collide.
    name: &'static str,
    /// The service principal the target's role trusts.
    trusts: &'static str,
    /// The action that role is allowed, or `None` for a role carrying no identity policy.
    allows: Option<&'static str>,
    /// The target's own `Input`, or `None` for a target taking the event as published.
    input: Option<&'static str>,
    expect: Expect,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a role this service can assume that allows the record write",
            name: "delivered",
            trusts: EVENTS_PRINCIPAL,
            allows: Some(PUT_RECORD_ACTION),
            input: None,
            expect: Expect::Envelope,
        },
        Row {
            case: "a target carrying its own constant input",
            name: "constant",
            trusts: EVENTS_PRINCIPAL,
            allows: Some(PUT_RECORD_ACTION),
            input: Some(CONSTANT_INPUT),
            expect: Expect::Constant,
        },
        Row {
            case: "a role this service can assume whose policy allows another action",
            name: "unpermitted",
            trusts: EVENTS_PRINCIPAL,
            allows: Some("firehose:DescribeDeliveryStream"),
            input: None,
            expect: Expect::Nothing,
        },
        Row {
            case: "a role this service cannot assume",
            name: "untrusted",
            trusts: OTHER_PRINCIPAL,
            allows: Some(PUT_RECORD_ACTION),
            input: None,
            expect: Expect::Nothing,
        },
    ]
}

/// A trust policy with one statement, letting `principal` assume the role.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing `action` on every resource.
fn identity_policy(action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": "*" }],
    })
    .to_string()
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

/// The role `name` trusting `principal`, carrying an identity policy allowing `allowed` when
/// one is named; answers the ARN that role carries.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    principal: &str,
    allowed: Option<&str>,
) -> String {
    iam(
        router,
        case,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trust_policy(principal)),
        ],
    )
    .await;
    if let Some(action) = allowed {
        iam(
            router,
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "delivery"),
                ("PolicyDocument", &identity_policy(action)),
            ],
        )
        .await;
    }
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
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
    let (status, body) =
        send(router, "s3", "GET", &format!("/{BUCKET}/{key}"), &[], Vec::new()).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: reading {key:?} must succeed, got {status} {}",
        String::from_utf8_lossy(&body)
    );
    body
}

/// Everything the bucket holds under `prefix`, concatenated in key order, once at least one
/// object is there — or a failure naming the row once [`DELIVERY_DEADLINE`] has passed.
async fn delivered(router: &MultiServiceRouter, case: &str, prefix: &str) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = keys_under(router, case, prefix).await;
        if !keys.is_empty() {
            let mut body = Vec::new();
            for key in &keys {
                body.extend_from_slice(&object(router, case, key).await);
            }
            return String::from_utf8_lossy(&body).to_string();
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
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
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
async fn an_events_rules_firehose_target_delivers_its_events_to_s3() {
    let router = emulator();
    let account = engine::emulator_account_id();
    let rows = rows();

    // The bucket every stream delivers into, and the role every stream delivers under.
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
    let delivery_role = given_role(
        &router,
        "setup",
        DELIVERY_ROLE,
        "firehose.amazonaws.com",
        Some("s3:PutObject"),
    )
    .await;

    // Every row's stream, created together so one wait covers the span the creates armed.
    for row in &rows {
        json_call(
            &router,
            row.case,
            "firehose",
            "Firehose_20150804.CreateDeliveryStream",
            json!({
                "DeliveryStreamName": row.name,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": delivery_role,
                    "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
                    "Prefix": format!("{}/", row.name),
                    // The shortest buffering the hints admit, so a row waits for the
                    // emulator's own delivery span and nothing longer.
                    "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                },
            }),
        )
        .await;
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // Every row's rule, its target naming that row's stream under that row's role.
    for row in &rows {
        let case = row.case;
        let role_arn = given_role(&router, case, row.name, row.trusts, row.allows).await;
        let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
        json_call(
            &router,
            case,
            "events",
            "AWSEvents.PutRule",
            json!({ "Name": row.name, "EventPattern": pattern, "State": "ENABLED" }),
        )
        .await;
        let mut target = json!({
            "Id": "t1",
            "Arn": format!("arn:aws:firehose:{REGION}:{account}:deliverystream/{}", row.name),
            "RoleArn": role_arn,
        });
        if let Some(input) = row.input {
            target["Input"] = json!(input);
        }
        let answered = json_call(
            &router,
            case,
            "events",
            "AWSEvents.PutTargets",
            json!({ "Rule": row.name, "Targets": [target] }),
        )
        .await;
        assert_eq!(
            answered["FailedEntryCount"],
            json!(0),
            "case {case:?}: PutTargets reported a failed entry: {answered}"
        );
    }

    // ONE publication, which every row's rule matches: what each target's role admits is the
    // only thing that differs between them.
    let published = json_call(
        &router,
        "the publication",
        "events",
        "AWSEvents.PutEvents",
        json!({
            "Entries": [{ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL }],
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "the publication must be accepted whole: {published}"
    );

    // The rows that deliver, FIRST: their delivered object is the barrier the rows that
    // deliver nothing are judged against, so an empty prefix is an absence rather than a
    // delivery still in flight.
    for row in rows.iter().filter(|row| row.expect != Expect::Nothing) {
        let case = row.case;
        let body = delivered(&router, case, &format!("{}/", row.name)).await;
        match row.expect {
            Expect::Envelope => {
                for expected in [SOURCE, DETAIL_TYPE, r#""orderId":"A-1""#] {
                    assert!(
                        body.contains(expected),
                        "case {case:?}: the delivered object carries the published envelope, \
                         expected {expected:?} in {body:?}"
                    );
                }
            }
            Expect::Constant => {
                assert_eq!(
                    body, CONSTANT_INPUT,
                    "case {case:?}: expected the target's own input {CONSTANT_INPUT:?}, got \
                     {body:?}"
                );
            }
            Expect::Nothing => unreachable!("filtered out above"),
        }
    }
    for row in rows.iter().filter(|row| row.expect == Expect::Nothing) {
        let case = row.case;
        let keys = keys_under(&router, case, &format!("{}/", row.name)).await;
        assert!(
            keys.is_empty(),
            "case {case:?}: a delivery the target's role bars reaches the stream, so its \
             destination holds nothing; got {keys:?}"
        );
    }
}
