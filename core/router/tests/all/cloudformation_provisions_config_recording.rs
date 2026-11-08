// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Config::*` resources its template declares — in AWS Config.
//!
//! The rule under test: **a stack creates every resource its `Resources` section declares
//! through the service that OWNS the type, so after the create completes that service's own
//! read answers for it with the configuration the template declared, and deleting the stack
//! deletes it there.** Here the owning calls are Config's own
//! `PutConfigurationRecorder`, `PutDeliveryChannel` and `PutConfigRule`, each documented to
//! create or update the resource it names, and the reads are
//! `DescribeConfigurationRecorders`, `DescribeDeliveryChannels` and `DescribeConfigRules` —
//! the last of which models `NoSuchConfigRuleException` only for a rule that does not
//! exist. Three crates have to agree for that to hold (CloudFormation makes the calls,
//! Config serves them, and the delivery channel is only taken while the bucket policy the
//! same stack declares admits Config), so the suite lives here: a crate-local test can only
//! ever see one side of the call.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES each
//! resource or leaves the name to CloudFormation, which recording strategy the recorder's
//! `RecordingGroup` asks for, whether the channel configures an `S3KeyPrefix` and a
//! snapshot frequency, and how the rule's scope is narrowed (by resource type, by one
//! resource, by a tag) — including a rule whose `InputParameters` are declared as a
//! template DOCUMENT where `ConfigRule$InputParameters` takes "a string, in JSON format".
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked; the one exception is a name the row leaves to CloudFormation,
//! which is read off the stack's own `Ref` and then checked for the form CloudFormation
//! generates and against the name Config holds the resource under.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): a suite that writes its own drifts from the others.
use super::query_wire::{elements, form_encode};

/// The Region every request here is signed for, and so the Region the stack belongs to.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's own clock-owed span to come due. None of the three Config
/// resources has a transient state of its own: each `Put…` stores it outright.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// Config's `X-Amz-Target` prefix, from its model's service shape (`StarlingDoveService`),
/// and the signing name its credential scope carries (`sigv4Name: config`).
const CONFIG_TARGET_PREFIX: &str = "StarlingDoveService";
/// See [`CONFIG_TARGET_PREFIX`].
const CONFIG_SIGNING_NAME: &str = "config";

/// The logical ids every row declares its resources under.
const BUCKET_LOGICAL_ID: &str = "DeliveryBucket";
/// See [`BUCKET_LOGICAL_ID`].
const POLICY_LOGICAL_ID: &str = "DeliveryBucketPolicy";
/// See [`BUCKET_LOGICAL_ID`].
const RECORDER_LOGICAL_ID: &str = "Recorder";
/// See [`BUCKET_LOGICAL_ID`].
const CHANNEL_LOGICAL_ID: &str = "Channel";
/// See [`BUCKET_LOGICAL_ID`].
const RULE_LOGICAL_ID: &str = "Rule";

/// The `ConfigRuleState` Config reports a rule it holds in.
const RULE_ACTIVE: &str = "ACTIVE";

/// The error code Config answers for a rule it does not hold.
const NO_SUCH_RULE: &str = "NoSuchConfigRuleException";
/// The error code Config answers for a configuration recorder it does not hold.
const NO_SUCH_RECORDER: &str = "NoSuchConfigurationRecorderException";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
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
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`],
/// which is what routes the request and what each handler reads its Region from. The
/// signature is never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Config operation over its awsJson1_1 wire.
async fn config(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CONFIG_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{CONFIG_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization(CONFIG_SIGNING_NAME))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code of a Config refusal: awsJson1_1 carries it as the body's `__type`.
fn config_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The bucket policy AWS documents for a Config delivery channel's bucket, which the stack
/// declares beside the channel: one `Allow` for the Config service principal per S3 action
/// a delivery performs.
fn delivery_bucket_policy(bucket_logical_id: &str) -> Value {
    let bucket = json!({ "Fn::Sub": format!("arn:aws:s3:::${{{bucket_logical_id}}}") });
    let objects = json!({ "Fn::Sub": format!("arn:aws:s3:::${{{bucket_logical_id}}}/*") });
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AWSConfigBucketPermissionsCheck",
                "Effect": "Allow",
                "Principal": { "Service": "config.amazonaws.com" },
                "Action": ["s3:GetBucketAcl", "s3:ListBucket"],
                "Resource": bucket,
                "Condition": { "StringEquals": { "AWS:SourceAccount": ACCOUNT_ID } },
            },
            {
                "Sid": "AWSConfigBucketDelivery",
                "Effect": "Allow",
                "Principal": { "Service": "config.amazonaws.com" },
                "Action": "s3:PutObject",
                "Resource": objects,
                "Condition": {
                    "StringEquals": {
                        "s3:x-amz-acl": "bucket-owner-full-control",
                        "AWS:SourceAccount": ACCOUNT_ID,
                    },
                },
            },
        ],
    })
}

/// The template a row deploys: the delivery bucket and its policy, the configuration
/// recorder, the delivery channel that delivers into that bucket, and the Config rule — each
/// declared with the row's own properties, with the dependencies a real template states
/// (Config takes no delivery channel before there is a recorder, and none for a bucket whose
/// policy does not yet admit it).
fn template(bucket: &str, recorder: &Value, channel: &Value, rule: &Value) -> String {
    let mut channel_properties = channel.clone();
    channel_properties["S3BucketName"] = json!({ "Ref": BUCKET_LOGICAL_ID });
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "AWS Config recording, delivery and one rule",
        "Resources": {
            BUCKET_LOGICAL_ID: {
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": bucket },
            },
            POLICY_LOGICAL_ID: {
                "Type": "AWS::S3::BucketPolicy",
                "Properties": {
                    "Bucket": { "Ref": BUCKET_LOGICAL_ID },
                    "PolicyDocument": delivery_bucket_policy(BUCKET_LOGICAL_ID),
                },
            },
            RECORDER_LOGICAL_ID: {
                "Type": "AWS::Config::ConfigurationRecorder",
                "Properties": recorder,
            },
            CHANNEL_LOGICAL_ID: {
                "Type": "AWS::Config::DeliveryChannel",
                "DependsOn": [RECORDER_LOGICAL_ID, POLICY_LOGICAL_ID],
                "Properties": channel_properties,
            },
            RULE_LOGICAL_ID: {
                "Type": "AWS::Config::ConfigRule",
                "DependsOn": [RECORDER_LOGICAL_ID],
                "Properties": rule,
            },
        },
        "Outputs": {
            "RecorderName": { "Value": { "Ref": RECORDER_LOGICAL_ID } },
            "ChannelName": { "Value": { "Ref": CHANNEL_LOGICAL_ID } },
            "RuleName": { "Value": { "Ref": RULE_LOGICAL_ID } },
        },
    })
    .to_string()
}

/// Create a stack, wait past its clock-owed span, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The ONE recorder Config reports under `name`.
async fn recorder_of(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = config(
        router,
        "DescribeConfigurationRecorders",
        &json!({ "ConfigurationRecorderNames": [name] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the recorder {name:?}, got {status} with \
         {body}"
    );
    body["ConfigurationRecorders"][0].clone()
}

/// The ONE delivery channel Config reports under `name`.
async fn channel_of(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = config(
        router,
        "DescribeDeliveryChannels",
        &json!({ "DeliveryChannelNames": [name] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the channel {name:?}, got {status} with \
         {body}"
    );
    body["DeliveryChannels"][0].clone()
}

/// The ONE rule Config reports under `name`.
async fn rule_of(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = config(
        router,
        "DescribeConfigRules",
        &json!({ "ConfigRuleNames": [name] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the rule {name:?}, got {status} with {body}"
    );
    body["ConfigRules"][0].clone()
}

/// One row of the table: the properties each resource is declared with, and what the rule
/// says Config must then hold.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    bucket: &'static str,
    /// The `Properties` of the recorder, the channel and the rule the template declares.
    recorder: fn() -> Value,
    channel: fn() -> Value,
    rule: fn() -> Value,
    /// The names the template declares, or `None` where CloudFormation generates one.
    recorder_name: Option<&'static str>,
    channel_name: Option<&'static str>,
    rule_name: Option<&'static str>,
    /// The `s3KeyPrefix` Config must report for the channel.
    expected_prefix: Option<&'static str>,
    /// The `InputParameters` TEXT Config must report for the rule, for a row that declares
    /// them.
    expected_input_parameters: Option<fn() -> String>,
}

/// The role every row's recorder assumes.
fn recorder_role() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/config-recorder")
}

/// The parameters the third row declares as a template DOCUMENT, and the text Config has to
/// report them as.
fn declared_input_parameters() -> Value {
    json!({ "maxAccessKeyAge": "90" })
}

/// Assert that Config reports `reported` for a recorder member the declaration STATES. A
/// member the declaration leaves out is Config's own default for it — the recording strategy
/// it derives from `allSupported`, say — and says nothing about what the stack carried, so
/// nothing is asserted about it.
fn declared_member_agrees(case: &str, member: &str, declared: &Value, reported: &Value) {
    if declared.is_null() {
        return;
    }
    assert_eq!(
        reported, declared,
        "case {case:?}: {member} is the declared one, got {reported}"
    );
}

/// Every form of declaration round-trips: the stack creates the recorder, the channel and
/// the rule in Config under the names its `Ref`s resolve to, carrying the configuration the
/// template declared, and deleting the stack deletes all three there.
#[tokio::test]
async fn every_declared_config_resource_is_created_in_config_and_deleted_with_the_stack() {
    let rows: Vec<Row> = vec![
        Row {
            case: "named resources, all supported types, a rule scoped by resource type",
            stack_name: "config-named",
            bucket: "config-named-delivery",
            recorder: || {
                json!({
                    "Name": "named-recorder",
                    "RoleARN": recorder_role(),
                    "RecordingGroup": { "AllSupported": true, "IncludeGlobalResourceTypes": false },
                })
            },
            channel: || json!({ "Name": "named-channel" }),
            rule: || {
                json!({
                    "ConfigRuleName": "named-rule",
                    "Description": "buckets must have versioning on",
                    "Source": {
                        "Owner": "AWS",
                        "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED",
                    },
                    "Scope": { "ComplianceResourceTypes": ["AWS::S3::Bucket"] },
                })
            },
            recorder_name: Some("named-recorder"),
            channel_name: Some("named-channel"),
            rule_name: Some("named-rule"),
            expected_prefix: None,
            expected_input_parameters: None,
        },
        Row {
            case: "generated names, recording by inclusion, a rule scoped to one resource",
            stack_name: "config-generated",
            bucket: "config-generated-delivery",
            recorder: || {
                json!({
                    "RoleARN": recorder_role(),
                    "RecordingGroup": {
                        "AllSupported": false,
                        "ResourceTypes": ["AWS::S3::Bucket"],
                        "RecordingStrategy": { "UseOnly": "INCLUSION_BY_RESOURCE_TYPES" },
                    },
                })
            },
            channel: || {
                json!({
                    "S3KeyPrefix": "audit",
                    "ConfigSnapshotDeliveryProperties": { "DeliveryFrequency": "Six_Hours" },
                })
            },
            rule: || {
                json!({
                    "Source": {
                        "Owner": "AWS",
                        "SourceIdentifier": "S3_BUCKET_SERVER_SIDE_ENCRYPTION_ENABLED",
                    },
                    "Scope": {
                        "ComplianceResourceTypes": ["AWS::S3::Bucket"],
                        "ComplianceResourceId": "config-generated-delivery",
                    },
                })
            },
            recorder_name: None,
            channel_name: None,
            rule_name: None,
            expected_prefix: Some("audit"),
            expected_input_parameters: None,
        },
        Row {
            case: "recording by exclusion, a rule scoped by tag with declared parameters",
            stack_name: "config-excluded",
            bucket: "config-excluded-delivery",
            recorder: || {
                json!({
                    "Name": "excluding-recorder",
                    "RoleARN": recorder_role(),
                    "RecordingGroup": {
                        "AllSupported": false,
                        "ExclusionByResourceTypes": { "ResourceTypes": ["AWS::EC2::Volume"] },
                        "RecordingStrategy": { "UseOnly": "EXCLUSION_BY_RESOURCE_TYPES" },
                    },
                    "RecordingMode": { "RecordingFrequency": "CONTINUOUS" },
                })
            },
            channel: || json!({ "Name": "excluding-channel", "S3KeyPrefix": "logs/" }),
            rule: || {
                json!({
                    "ConfigRuleName": "tagged-rule",
                    "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                    "Scope": { "TagKey": "stage", "TagValue": "prod" },
                    "InputParameters": declared_input_parameters(),
                    "MaximumExecutionFrequency": "TwentyFour_Hours",
                })
            },
            recorder_name: Some("excluding-recorder"),
            channel_name: Some("excluding-channel"),
            rule_name: Some("tagged-rule"),
            expected_prefix: Some("logs/"),
            expected_input_parameters: Some(|| declared_input_parameters().to_string()),
        },
    ];

    for row in rows {
        let case = row.case;
        let router = build_router();
        let declared_recorder = (row.recorder)();
        let declared_channel = (row.channel)();
        let declared_rule = (row.rule)();
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &template(
                row.bucket,
                &declared_recorder,
                &declared_channel,
                &declared_rule,
            ),
        )
        .await;
        assert!(
            body.contains("<StackStatus>CREATE_COMPLETE</StackStatus>"),
            "case {case:?}: the stack must reach CREATE_COMPLETE, got {body}"
        );

        // The names the stack reports for the three resources: the declared one where the
        // row declares it, and otherwise the one CloudFormation generated, which has the
        // documented `<stack>-<logical id>-<unique>` form.
        let outputs = outputs_of(&body);
        let names: BTreeMap<&str, String> = [
            ("RecorderName", row.recorder_name, RECORDER_LOGICAL_ID),
            ("ChannelName", row.channel_name, CHANNEL_LOGICAL_ID),
            ("RuleName", row.rule_name, RULE_LOGICAL_ID),
        ]
        .into_iter()
        .map(|(output, declared, logical_id)| {
            let reported = outputs
                .get(output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the stack reports {output}, got {outputs:?}")
                })
                .clone();
            match declared {
                Some(declared) => assert_eq!(
                    reported, declared,
                    "case {case:?}: {output} is the name the template declared"
                ),
                None => {
                    let prefix = format!("{}-{logical_id}-", row.stack_name);
                    assert!(
                        reported.starts_with(&prefix),
                        "case {case:?}: a generated {output} starts with {prefix:?}, got \
                         {reported:?}"
                    );
                }
            }
            (output, reported)
        })
        .collect();

        // The recorder Config holds: under the name the stack reports, with the role and the
        // recording group the template declared.
        let recorder_name = names["RecorderName"].clone();
        let recorder = recorder_of(&router, case, &recorder_name).await;
        assert_eq!(
            recorder["name"],
            json!(recorder_name),
            "case {case:?}: Config holds the recorder under the name the stack reports, got \
             {recorder}"
        );
        assert_eq!(
            recorder["roleARN"],
            json!(recorder_role()),
            "case {case:?}: the recorder assumes the declared role, got {recorder}"
        );
        declared_member_agrees(
            case,
            "the recorder's allSupported",
            &declared_recorder["RecordingGroup"]["AllSupported"],
            &recorder["recordingGroup"]["allSupported"],
        );
        declared_member_agrees(
            case,
            "the recorder's resource types",
            &declared_recorder["RecordingGroup"]["ResourceTypes"],
            &recorder["recordingGroup"]["resourceTypes"],
        );
        declared_member_agrees(
            case,
            "the recorder's recording strategy",
            &declared_recorder["RecordingGroup"]["RecordingStrategy"]["UseOnly"],
            &recorder["recordingGroup"]["recordingStrategy"]["useOnly"],
        );
        declared_member_agrees(
            case,
            "the resource types the recorder excludes",
            &declared_recorder["RecordingGroup"]["ExclusionByResourceTypes"]["ResourceTypes"],
            &recorder["recordingGroup"]["exclusionByResourceTypes"]["resourceTypes"],
        );
        declared_member_agrees(
            case,
            "the recorder's recording frequency",
            &declared_recorder["RecordingMode"]["RecordingFrequency"],
            &recorder["recordingMode"]["recordingFrequency"],
        );

        // The channel Config holds: under the name the stack reports, delivering into the
        // bucket the stack created, under the prefix the template declared.
        let channel_name = names["ChannelName"].clone();
        let channel = channel_of(&router, case, &channel_name).await;
        assert_eq!(
            (
                channel["name"].as_str(),
                channel["s3BucketName"].as_str(),
                channel["s3KeyPrefix"].as_str(),
            ),
            (
                Some(channel_name.as_str()),
                Some(row.bucket),
                row.expected_prefix,
            ),
            "case {case:?}: the channel names the bucket and prefix the template declared, \
             got {channel}"
        );

        // The rule Config holds: under the name the stack reports, with the source and the
        // scope the template declared, ACTIVE, and carrying an ARN of Config's own minting.
        let rule_name = names["RuleName"].clone();
        let rule = rule_of(&router, case, &rule_name).await;
        assert_eq!(
            rule["ConfigRuleName"],
            json!(rule_name),
            "case {case:?}: Config holds the rule under the name the stack reports, got {rule}"
        );
        assert_eq!(
            rule["Source"], declared_rule["Source"],
            "case {case:?}: the rule's source is the declared one, got {rule}"
        );
        assert_eq!(
            rule["Scope"], declared_rule["Scope"],
            "case {case:?}: the rule's scope is the declared one, got {rule}"
        );
        assert_eq!(
            rule["ConfigRuleState"],
            json!(RULE_ACTIVE),
            "case {case:?}: a rule the stack created is active, got {rule}"
        );
        let expected_arn_prefix = format!("arn:aws:config:{REGION}:{ACCOUNT_ID}:config-rule/");
        assert!(
            rule["ConfigRuleArn"]
                .as_str()
                .is_some_and(|arn| arn.starts_with(&expected_arn_prefix)),
            "case {case:?}: the rule carries the ARN Config minted for it, got {rule}"
        );
        match row.expected_input_parameters {
            Some(expected) => assert_eq!(
                rule["InputParameters"],
                json!(expected()),
                "case {case:?}: the declared parameters reach Config as their JSON text, got \
                 {rule}"
            ),
            None => assert!(
                rule.get("InputParameters").is_none(),
                "case {case:?}: a rule declaring no parameters carries none, got {rule}"
            ),
        }

        // Deleting the stack deletes all three in Config.
        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (status, body) = config(
            &router,
            "DescribeConfigRules",
            &json!({ "ConfigRuleNames": [rule_name] }),
        )
        .await;
        assert_eq!(
            (status, config_error_code(&body).as_str()),
            (400, NO_SUCH_RULE),
            "case {case:?}: the rule is gone from Config, got {status} with {body}"
        );
        let (status, body) = config(
            &router,
            "DescribeConfigurationRecorders",
            &json!({ "ConfigurationRecorderNames": [recorder_name] }),
        )
        .await;
        assert_eq!(
            (status, config_error_code(&body).as_str()),
            (400, NO_SUCH_RECORDER),
            "case {case:?}: the recorder is gone from Config, got {status} with {body}"
        );
        let (status, body) = config(&router, "DescribeDeliveryChannels", &json!({})).await;
        assert_eq!(
            (status, body["DeliveryChannels"].as_array().map(Vec::len)),
            (200, Some(0)),
            "case {case:?}: the channel is gone from Config, got {status} with {body}"
        );
    }
}
