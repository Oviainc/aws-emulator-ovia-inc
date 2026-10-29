// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Kinesis::Stream` resources its template declares — in Amazon Kinesis Data Streams,
//! with every declared property applied to the stream.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns it created it, through that service's own public API and with every declared
//! property applied, so the handle the stack publishes addresses a resource that service
//! really holds — and deleting the stack releases it there.** For a data stream the handles
//! are its NAME ("Ref returns the stream name") and its ARN (`Fn::GetAtt Arn`), so from the
//! instant the stack first reports itself complete `DescribeStreamSummary` describes the
//! stream the template declared and `ListTagsForResource` answers with the tags it declared
//! — rather than Kinesis refusing both with `ResourceNotFoundException` for a stream its own
//! stack just reported complete. Two crates have to agree for that to hold (CloudFormation
//! makes the calls, Kinesis serves them), so the suite lives here: a crate-local test can
//! only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! stream or leaves the name to CloudFormation, whether it is written in JSON or YAML, the
//! capacity mode declared (none, so provisioned, or on-demand), how many shards a
//! provisioned stream asks for, how the declaration spells its `Tags` (the published list,
//! the map form, none at all), whether a retention window longer than the default or exactly
//! it is asked for, whether server-side encryption is asked for, and the Region the stack is
//! deployed in. A window SHORTER than the default is a row of the refusal table below rather
//! than of this one: 24 hours is also the shortest window Kinesis holds a stream to, so a
//! declaration below it is one that service refuses.
//!
//! The deletion half is the same rule read from the other side: once the stack is gone the
//! stream is gone with it, and Kinesis refuses the stack's own published ARN with the
//! message that error class carries.
//!
//! The awsQuery form encoder and the XML readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, opt_element, outputs_of, physical_ids_of};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How often the suite asks the stack (or Kinesis) whether what it is waiting for has
/// happened, and how long it goes on asking. The interval is short because the FIRST
/// observation is what each rule is stated about; the budget is long enough that a loaded
/// machine is slower than the spans these services choose without the rule being different.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The `StreamStatus` a stream serves reads and writes in.
const ACTIVE: &str = "ACTIVE";

/// The longest stream name Kinesis accepts (`StreamName`, `@length(min: 1, max: 128)` with
/// the pattern `^[a-zA-Z0-9_.-]+$` in that service's own model), which a name CloudFormation
/// GENERATES is held to.
const MAX_STREAM_NAME: usize = 128;

/// The retention window a stream carries when nothing asks for another one: "The default
/// value is 24" (`CreateStream`'s documented default retention period).
const DEFAULT_RETENTION_HOURS: i64 = 24;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon Kinesis Data Streams operation over its awsJson1_1 wire, signed for
/// `region`.
async fn kinesis(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kinesis.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Kinesis_20131202.{operation}"))
        .header("authorization", authorization("kinesis", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a stream of `name` carries in `region`: the same total function of the Region, the
/// account and the name Kinesis itself keys a stream by.
fn stream_arn(region: &str, name: &str) -> String {
    format!("arn:aws:kinesis:{region}:{ACCOUNT}:stream/{name}")
}

/// The error code an awsJson answer carries, without the namespace a `__type` may qualify it
/// with.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, region: &str, stack: &str) -> (String, String) {
    let (_, body) = cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it first reports `wanted`, and answer the last status it reported with
/// the `DescribeStacks` answer it was read from. The FIRST such observation is what the rules
/// below are stated about, so nothing is waited out past it.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = stack_status(router, region, stack).await;
    for _ in 0..POLLS {
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, region, stack).await;
    }
    last
}

/// `DescribeStreamSummary` for one stream, by the name the stack published.
async fn stream_summary(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, Value, Value) {
    let (status, body) = kinesis(
        router,
        region,
        "DescribeStreamSummary",
        &json!({ "StreamName": name }),
    )
    .await;
    let summary = body
        .get("StreamDescriptionSummary")
        .cloned()
        .unwrap_or(Value::Null);
    (status, summary, body)
}

/// Poll Kinesis until it no longer holds `name`, and answer the last
/// `DescribeStreamSummary`. A stream whose deletion that service has accepted is reported
/// `DELETING` until the removal falls due, so what the rule is about — the stream is gone —
/// is the refusal this waits for.
async fn wait_for_stream_gone(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, Value) {
    let (mut status, _, mut body) = stream_summary(router, region, name).await;
    for _ in 0..POLLS {
        if status != 200 {
            return (status, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        let read = stream_summary(router, region, name).await;
        status = read.0;
        body = read.2;
    }
    (status, body)
}

/// One row of the table: a template, and what the rule says the stream it declares must then
/// be in Amazon Kinesis Data Streams.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the stream is created in and the
    /// one its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn(&str) -> String,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// stream carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `StreamModeDetails.StreamMode` the summary must report.
    expected_mode: &'static str,
    /// The `OpenShardCount` it must report: the shards the declaration provisions, or the
    /// ones the service opens by itself for an on-demand stream.
    expected_open_shards: i64,
    /// The `RetentionPeriodHours` it must report.
    expected_retention_hours: i64,
    /// The `(EncryptionType, KeyId)` it must report, for a row that declares encryption.
    expected_encryption: Option<(&'static str, &'static str)>,
    /// The tags the stream's own declaration asks for.
    expected_tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named provisioned stream in JSON with one shard and two declared tags",
        region: "us-east-1",
        template: |_| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a Kinesis data stream with tags",
                "Resources": {
                    "Events": {
                        "Type": "AWS::Kinesis::Stream",
                        "Properties": {
                            "Name": "orders-events",
                            "ShardCount": 1,
                            "Tags": [
                                { "Key": "Owner", "Value": "orders" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "StreamName": { "Value": { "Ref": "Events" } },
                    "StreamArn": { "Value": { "Fn::GetAtt": ["Events", "Arn"] } }
                }
            })
            .to_string()
        },
        declared_name: Some("orders-events"),
        expected_mode: "PROVISIONED",
        expected_open_shards: 1,
        expected_retention_hours: DEFAULT_RETENTION_HOURS,
        expected_encryption: None,
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
    },
    Row {
        case: "a stream the template does not name, on-demand, with its Tags as a map and a \
               retention window longer than the default",
        region: "us-west-2",
        template: |_| {
            json!({
                "Resources": {
                    "Telemetry": {
                        "Type": "AWS::Kinesis::Stream",
                        "Properties": {
                            "StreamModeDetails": { "StreamMode": "ON_DEMAND" },
                            "RetentionPeriodHours": 48,
                            "Tags": { "Owner": "platform" }
                        }
                    }
                },
                "Outputs": {
                    "StreamName": { "Value": { "Ref": "Telemetry" } },
                    "StreamArn": { "Value": { "Fn::GetAtt": ["Telemetry", "Arn"] } }
                }
            })
            .to_string()
        },
        declared_name: None,
        expected_mode: "ON_DEMAND",
        // An on-demand stream's shard count is the service's to manage: "Kinesis Data
        // Streams automatically manages the shards", so the count is the one it opened.
        expected_open_shards: 4,
        expected_retention_hours: 48,
        expected_encryption: None,
        expected_tags: &[("Owner", "platform")],
    },
    Row {
        case: "a named provisioned stream in YAML with three shards, server-side encryption \
               and a retention window of exactly the default",
        region: "eu-west-1",
        template: |_| {
            [
                "Resources:",
                "  Ledger:",
                "    Type: AWS::Kinesis::Stream",
                "    Properties:",
                "      Name: ledger-events",
                "      ShardCount: 3",
                "      RetentionPeriodHours: 24",
                "      StreamEncryption:",
                "        EncryptionType: KMS",
                "        KeyId: alias/aws/kinesis",
                "      Tags:",
                "        - Key: Owner",
                "          Value: ledger",
                "Outputs:",
                "  StreamName:",
                "    Value: !Ref Ledger",
                "  StreamArn:",
                "    Value: !GetAtt Ledger.Arn",
                "",
            ]
            .join("\n")
        },
        declared_name: Some("ledger-events"),
        expected_mode: "PROVISIONED",
        expected_open_shards: 3,
        expected_retention_hours: DEFAULT_RETENTION_HOURS,
        expected_encryption: Some(("KMS", "alias/aws/kinesis")),
        expected_tags: &[("Owner", "ledger")],
    },
    Row {
        case: "a named provisioned stream in JSON declaring no tags of its own and a \
               retention window of a full week",
        region: "ap-northeast-1",
        template: |_| {
            json!({
                "Resources": {
                    "Audit": {
                        "Type": "AWS::Kinesis::Stream",
                        "Properties": {
                            "Name": "audit.trail-1",
                            "ShardCount": 2,
                            "RetentionPeriodHours": 168
                        }
                    }
                },
                "Outputs": {
                    "StreamName": { "Value": { "Ref": "Audit" } },
                    "StreamArn": { "Value": { "Fn::GetAtt": ["Audit", "Arn"] } }
                }
            })
            .to_string()
        },
        declared_name: Some("audit.trail-1"),
        expected_mode: "PROVISIONED",
        expected_open_shards: 2,
        expected_retention_hours: 168,
        expected_encryption: None,
        expected_tags: &[],
    },
];

/// Every row: the name the stack reports for its `AWS::Kinesis::Stream` is a stream Amazon
/// Kinesis Data Streams really holds, configured and tagged as the template declared it, and
/// reachable through the ARN the stack published — and deleting the stack releases it there,
/// after which that service refuses the same ARN with the message its not-found class
/// carries.
#[tokio::test]
async fn a_stack_provisions_the_kinesis_stream_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("kinesis-stream-stack-{index}");
        let template = (row.template)(region);

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("Tags.member.1.Key", "Stack"),
                ("Tags.member.1.Value", "kinesis-suite"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The stack holds the resource open while Kinesis still reports the stream coming
        // up, and reports itself complete only once that service reports it ready.
        let (status, described) =
            wait_for_stack_status(&router, region, &stack_name, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = physical_ids_of(&resources)
            .into_values()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: no physical id in {resources}"));

        // "Ref returns the stream name": the physical id IS the name the template asked for,
        // or the one CloudFormation generated for a template that named nothing — which is
        // held to the name Kinesis accepts.
        if let Some(declared) = row.declared_name {
            assert_eq!(
                physical_id, declared,
                "case {case:?}: expected the physical id to be the declared name {declared:?}, \
                 got {physical_id:?}"
            );
        }
        assert!(
            !physical_id.is_empty()
                && physical_id.chars().count() <= MAX_STREAM_NAME
                && physical_id.chars().all(|character| character
                    .is_ascii_alphanumeric()
                    || character == '_'
                    || character == '.'
                    || character == '-'),
            "case {case:?}: expected a name inside Kinesis's own StreamName rule, got \
             {physical_id:?}"
        );

        // The two handles the stack publishes for the resource: the name its `Ref` resolves
        // to, and the ARN `Fn::GetAtt Arn` reads — the one Kinesis keys the stream by.
        let outputs = outputs_of(&described);
        let arn = stream_arn(region, &physical_id);
        assert_eq!(
            outputs.get("StreamName").map(String::as_str),
            Some(physical_id.as_str()),
            "case {case:?}: expected the StreamName output to be the stream name \
             {physical_id:?}, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("StreamArn").map(String::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the StreamArn output to be {arn:?}, got {outputs:?}"
        );

        // Kinesis itself resolves that name to the stream the template declared, in the
        // state a client may read and write it in.
        let (status, summary, answer) = stream_summary(&router, region, &physical_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStreamSummary on the name the \
             stack reported, got {status} with body {answer}"
        );
        assert_eq!(
            summary.get("StreamStatus").and_then(Value::as_str),
            Some(ACTIVE),
            "case {case:?}: expected the stream to be {ACTIVE} once the stack reported itself \
             {CREATE_COMPLETE}, got {summary}"
        );
        assert_eq!(
            summary.get("StreamARN").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the stream's own ARN to be the one the stack published \
             ({arn:?}), got {summary}"
        );
        assert_eq!(
            summary
                .pointer("/StreamModeDetails/StreamMode")
                .and_then(Value::as_str),
            Some(row.expected_mode),
            "case {case:?}: expected the declared capacity mode {:?}, got {summary}",
            row.expected_mode
        );
        assert_eq!(
            summary.get("OpenShardCount").and_then(Value::as_i64),
            Some(row.expected_open_shards),
            "case {case:?}: expected {} open shards, got {summary}",
            row.expected_open_shards
        );
        assert_eq!(
            summary.get("RetentionPeriodHours").and_then(Value::as_i64),
            Some(row.expected_retention_hours),
            "case {case:?}: expected a retention window of {} hours, got {summary}",
            row.expected_retention_hours
        );
        match row.expected_encryption {
            Some((encryption_type, key_id)) => {
                assert_eq!(
                    summary.get("EncryptionType").and_then(Value::as_str),
                    Some(encryption_type),
                    "case {case:?}: expected the declared encryption type {encryption_type:?}, \
                     got {summary}"
                );
                assert_eq!(
                    summary.get("KeyId").and_then(Value::as_str),
                    Some(key_id),
                    "case {case:?}: expected the declared key {key_id:?}, got {summary}"
                );
            }
            None => assert_ne!(
                summary.get("EncryptionType").and_then(Value::as_str),
                Some("KMS"),
                "case {case:?}: expected no server-side encryption on a stream whose \
                 declaration asks for none, got {summary}"
            ),
        }

        // The tags the declaration asked for are on the stream, beside the stack's own, and
        // reachable through the ARN the stack published. The three `aws:cloudformation:*`
        // tags are NOT: Kinesis refuses that namespace to a caller, and CloudFormation must
        // not fail a stack over tags it adds itself.
        let (status, listed) = kinesis(
            &router,
            region,
            "ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource on the ARN the stack \
             published, got {status} with body {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'Tags' list in {listed}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("Key")?.as_str()?.to_string(),
                    tag.get("Value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ))
            })
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get("Stack").map(String::as_str),
            Some("kinesis-suite"),
            "case {case:?}: expected the stack's own tag to be propagated, got {reported:?}"
        );
        assert!(
            !reported.keys().any(|key| key.starts_with("aws:")),
            "case {case:?}: expected no reserved tag key to be sent to Kinesis, got \
             {reported:?}"
        );

        // Deleting the stack releases the stream Kinesis holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = wait_for_stream_gone(&router, region, &physical_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeStreamSummary once the stack \
             deleted the stream, got {status} with body {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException once the stream was deleted, \
             got {gone}"
        );

        // And the ARN the stack published is refused with the message that error class
        // carries: the stream name at fault and the caller's account.
        let (status, refused) = kinesis(
            &router,
            region,
            "ListTagsForResource",
            &json!({ "ResourceARN": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from ListTagsForResource once the stack was \
             deleted, got {status} with body {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for the deleted stream, got \
             {refused}"
        );
        let expected_message = format!("Stream {physical_id} under account {ACCOUNT} not found.");
        assert_eq!(
            refused.get("message").and_then(Value::as_str),
            Some(expected_message.as_str()),
            "case {case:?}: expected the message {expected_message:?}, got {refused}"
        );
    }
}

/// A declaration Amazon Kinesis Data Streams REFUSES does not report a resource nothing
/// created: the stack fails with that service's own refusal, and nothing half-created is left
/// behind.
///
/// The rows differ along WHAT is refused and by WHICH of the calls the type's arm makes: a
/// stream name a client already holds (refused by the create), a capacity mode whose shard
/// count the service manages itself (refused by the create too), a retention window past the
/// published ceiling and one SHORTER than the default — the two directions the second call
/// carries a declared window in, both refused after the stream was created, so each also
/// checks the stream that call was for is gone again — a property this emulator maps onto no
/// call at all, and a whole-number property whose declared value is no whole number: the last
/// two are refused rather than silently dropped.
#[tokio::test]
async fn a_stream_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    // The stream a client already holds, which the first row's template re-declares.
    let (status, created) = kinesis(
        &router,
        region,
        "CreateStream",
        &json!({ "StreamName": "already-there", "ShardCount": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own CreateStream to succeed, got {status} with body {created}"
    );

    // (case, the resource's declared properties, the stream name the declaration asks for —
    // which must hold no stream once the stack has failed, except where a client created it).
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a stream name a client already holds",
            json!({ "Name": "already-there", "ShardCount": 1 }),
            "already-there",
        ),
        (
            "an on-demand stream whose shard count the service manages itself",
            json!({
                "Name": "on-demand-with-shards",
                "ShardCount": 2,
                "StreamModeDetails": { "StreamMode": "ON_DEMAND" },
            }),
            "on-demand-with-shards",
        ),
        (
            "a retention window past the published ceiling",
            json!({ "Name": "retained-too-long", "ShardCount": 1, "RetentionPeriodHours": 9000 }),
            "retained-too-long",
        ),
        (
            "a retention window shorter than the one a created stream carries",
            json!({ "Name": "retained-too-briefly", "ShardCount": 1, "RetentionPeriodHours": 12 }),
            "retained-too-briefly",
        ),
        (
            "a property this emulator maps onto no call",
            json!({
                "Name": "with-desired-metrics",
                "ShardCount": 1,
                "DesiredShardLevelMetrics": ["IncomingBytes"],
            }),
            "with-desired-metrics",
        ),
        (
            "a shard count that is no whole number",
            json!({ "Name": "shards-not-a-number", "ShardCount": "two" }),
            "shards-not-a-number",
        ),
    ];

    for (index, (case, properties, stream)) in rows.into_iter().enumerate() {
        let stack_name = format!("kinesis-refusal-stack-{index}");
        let template = json!({
            "Resources": { "Stream": { "Type": "AWS::Kinesis::Stream", "Properties": properties } }
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (status, described) =
            wait_for_stack_status(&router, region, &stack_name, ROLLBACK_COMPLETE).await;
        assert_eq!(
            status, ROLLBACK_COMPLETE,
            "case {case:?}: expected the failed create to be rolled back, got {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("Stream"),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        if stream == "already-there" {
            // The stream the CLIENT created is still exactly as it was: the stack's failure
            // neither replaced nor removed it.
            let (status, read, answer) = stream_summary(&router, region, stream).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the client's stream to still resolve, got {status} \
                 with body {answer}"
            );
            assert_eq!(
                read.get("OpenShardCount").and_then(Value::as_i64),
                Some(1),
                "case {case:?}: expected the client's own stream untouched, got {read}"
            );
        } else {
            let (status, read) = wait_for_stream_gone(&router, region, stream).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected no stream to be left behind by the failed stack, got \
                 {status} with body {read}"
            );
            assert_eq!(
                error_code(&read),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for the failed resource, \
                 got {read}"
            );
        }
    }
}

/// A stack that declares no resource of this type at all creates no stream: the arm is
/// reached by the TYPE, not by anything in the request.
///
/// The control row of the rule above — a stack whose resource types this emulator leaves
/// declared-only must create nothing in Kinesis, and must still complete.
#[tokio::test]
async fn a_stack_declaring_no_stream_creates_none() {
    let router = build_router();
    let region = "eu-central-1";
    let stack = "no-kinesis-stream-stack";
    let template = json!({
        "Resources": {
            "Waiter": { "Type": "AWS::CloudFormation::WaitConditionHandle", "Properties": {} }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let (status, described) = wait_for_stack_status(&router, region, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "expected the stack to complete, got {described}"
    );

    let (status, listed) = kinesis(&router, region, "ListStreams", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListStreams, got {status} with body {listed}"
    );
    let names = listed
        .get("StreamNames")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        names.is_empty(),
        "expected no stream to exist in {region}, got {listed}"
    );
}
