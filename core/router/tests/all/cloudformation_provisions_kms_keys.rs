// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::KMS::Key` really creates that key in KMS,
//! and the identifiers it publishes for it resolve there — including on the DATA plane of
//! the service the stack hands them to.
//!
//! The rule under test: **a stack's physical resource id, and every attribute it publishes,
//! are the values the service that owns the resource assigned — so `Ref` and `Fn::GetAtt`
//! resolve to a key that service's own reads resolve, and a resource of another service
//! configured with one of them (a queue's `KmsMasterKeyId`) reaches the same key.** For a
//! KMS key that means `Ref` is the key id `CreateKey` minted and `Fn::GetAtt ['Key','Arn']`
//! is the ARN KMS built from it, never a CloudFormation `<stack>-<logical id>-<suffix>`
//! physical id in a key ARN's key-id position — which no key answers to, so every producer
//! on the queue it keys would be refused `KMS.NotFoundException`.
//!
//! Three crates have to agree for that to hold (CloudFormation makes the calls, KMS mints
//! the key and serves the reads, Amazon SQS resolves the queue's key on every send and
//! receive), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: which properties the key declares
//! (none at all, a description, an explicit key policy, an explicit spec and usage, tags,
//! a disabled key, a multi-Region key, automatic rotation with a period), which intrinsic a
//! reference reads the key with (`Ref`, `Fn::GetAtt … KeyId`, `Fn::GetAtt … Arn`), which
//! identifier form KMS is then asked about the key by (the key id and the key ARN), whether
//! the queue is keyed at CREATE or by a later UPDATE, and whether the key's own policy
//! permits the data-plane action the queue needs.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the key's identity, which is KMS's to mint: what is
//! checked about it is that ONE value appears everywhere the stack reports it, that it is
//! not an identifier CloudFormation computed, and that KMS's own `DescribeKey` resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, elements, form_encode, opt_element};

/// The region every request here is signed for, and so the region the stack's key and its
/// ARN belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due. A key has no transient status of its own: `CreateKey` answers with a key that
/// is usable at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
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

/// Call one KMS operation over its awsJson1_1 wire.
async fn kms(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header("authorization", authorization("kms"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One KMS call that must succeed.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = kms(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from KMS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Call one Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One SQS call that must succeed.
async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = sqs(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of an awsJson refusal, which both services carry as the body's
/// `__type`.
fn error_code(body: &Value) -> String {
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

/// One output of a settled stack.
fn output(outputs: &BTreeMap<String, String>, case: &str, key: &str) -> String {
    outputs
        .get(key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {outputs:?}"))
        .clone()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
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
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    body
}

/// Update a stack with a new template, wait past its transition, and answer the
/// `DescribeStacks` body.
async fn update_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );
    body
}

/// A key policy giving the account root every KMS action, which is the statement the
/// canonical SSE-KMS template writes and the one that hands the decision to IAM.
fn root_key_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "EnableRootPermissions",
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
            "Action": "kms:*",
            "Resource": "*",
        }],
    })
}

/// A template declaring ONE `AWS::KMS::Key` carrying `properties`, publishing the three
/// ways a stack can report the key's identity: `Ref`, and each of the type's two published
/// `Fn::GetAtt` attributes.
fn one_key_template(properties: &Value) -> String {
    json!({
        "Resources": {"Key": {"Type": "AWS::KMS::Key", "Properties": properties}},
        "Outputs": {
            "KeyRef": {"Value": {"Ref": "Key"}},
            "KeyIdAttribute": {"Value": {"Fn::GetAtt": ["Key", "KeyId"]}},
            "KeyArn": {"Value": {"Fn::GetAtt": ["Key", "Arn"]}},
        },
    })
    .to_string()
}

/// One row of the key-creation table: the properties the key declares, and what KMS must
/// then report about the key the stack published.
struct KeyRow {
    case: &'static str,
    stack_name: &'static str,
    properties: Value,
    /// `KeyMetadata` members `DescribeKey` must report, each with the value the declaration
    /// implies.
    metadata: Vec<(&'static str, Value)>,
    /// The key policy `GetKeyPolicy` must report, for a row that declares one.
    policy: Option<Value>,
    /// `(whether rotation is on, the period it reports)` for a row that declares rotation.
    rotation: Option<(bool, Option<i64>)>,
    /// The tags `ListResourceTags` must report.
    tags: Vec<(&'static str, &'static str)>,
}

/// A stack that completes has really created its declared key in KMS, and the identity it
/// publishes for it is the one KMS minted and resolves.
#[tokio::test]
async fn a_stack_creates_its_declared_kms_key_in_kms_under_the_identity_kms_minted() {
    let router = build_router();
    let rows = vec![
        KeyRow {
            case: "a key declaring no properties at all",
            stack_name: "kms-bare-stack",
            properties: json!({}),
            // "Enabled ... Default: true" for the type, so a key that declares nothing is
            // enabled and usable.
            metadata: vec![("Enabled", json!(true)), ("KeyState", json!("Enabled"))],
            policy: None,
            rotation: None,
            tags: Vec::new(),
        },
        KeyRow {
            case: "a key with a description and an explicit key policy",
            stack_name: "kms-described-stack",
            properties: json!({
                "Description": "the stack's own key",
                "KeyPolicy": root_key_policy(),
            }),
            metadata: vec![
                ("Description", json!("the stack's own key")),
                ("Enabled", json!(true)),
            ],
            policy: Some(root_key_policy()),
            rotation: None,
            tags: Vec::new(),
        },
        KeyRow {
            case: "a key declared disabled, which no cryptographic operation may use",
            stack_name: "kms-disabled-stack",
            properties: json!({"Enabled": false, "KeyPolicy": root_key_policy()}),
            metadata: vec![("Enabled", json!(false)), ("KeyState", json!("Disabled"))],
            policy: Some(root_key_policy()),
            rotation: None,
            tags: Vec::new(),
        },
        KeyRow {
            // The spelling a JSON template gives a boolean, which CloudFormation reads
            // case-insensitively like every other template boolean.
            case: "a key declared disabled by the TEXT of a boolean",
            stack_name: "kms-text-disabled-stack",
            properties: json!({"Enabled": "False", "KeyPolicy": root_key_policy()}),
            metadata: vec![("Enabled", json!(false)), ("KeyState", json!("Disabled"))],
            policy: Some(root_key_policy()),
            rotation: None,
            tags: Vec::new(),
        },
        KeyRow {
            case: "a key whose spec and usage the template states explicitly",
            stack_name: "kms-spec-stack",
            properties: json!({
                "KeySpec": "SYMMETRIC_DEFAULT",
                "KeyUsage": "ENCRYPT_DECRYPT",
                "Origin": "AWS_KMS",
            }),
            metadata: vec![
                ("KeySpec", json!("SYMMETRIC_DEFAULT")),
                ("KeyUsage", json!("ENCRYPT_DECRYPT")),
                ("Origin", json!("AWS_KMS")),
            ],
            policy: None,
            rotation: None,
            tags: Vec::new(),
        },
        KeyRow {
            case: "a multi-Region key",
            stack_name: "kms-multiregion-stack",
            properties: json!({"MultiRegion": true}),
            metadata: vec![("MultiRegion", json!(true))],
            policy: None,
            rotation: None,
            tags: Vec::new(),
        },
        KeyRow {
            case: "a key with automatic rotation on and a period of its own",
            stack_name: "kms-rotated-stack",
            properties: json!({"EnableKeyRotation": true, "RotationPeriodInDays": 180}),
            metadata: vec![("Enabled", json!(true))],
            policy: None,
            rotation: Some((true, Some(180))),
            tags: Vec::new(),
        },
        KeyRow {
            case: "a key carrying tags of its own",
            stack_name: "kms-tagged-stack",
            properties: json!({
                "Tags": [
                    {"Key": "owner", "Value": "platform"},
                    {"Key": "purpose", "Value": "sse"},
                ],
            }),
            metadata: vec![("Enabled", json!(true))],
            policy: None,
            rotation: None,
            tags: vec![("owner", "platform"), ("purpose", "sse")],
        },
        KeyRow {
            case: "a key whose rotation the template turns off explicitly",
            stack_name: "kms-unrotated-stack",
            properties: json!({"EnableKeyRotation": false}),
            metadata: vec![("Enabled", json!(true))],
            policy: None,
            rotation: Some((false, None)),
            tags: Vec::new(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &one_key_template(&row.properties),
        )
        .await;
        let outputs = outputs_of(&body);
        let key_id = output(&outputs, case, "KeyRef");
        let reported_id = output(&outputs, case, "KeyIdAttribute");
        let key_arn = output(&outputs, case, "KeyArn");

        // One identity, however the stack is asked for it.
        assert_eq!(
            reported_id, key_id,
            "case {case:?}: the KeyId attribute must be the same identity Ref returns, got \
             {reported_id:?} against {key_id:?}"
        );
        assert_eq!(
            key_arn,
            format!(
                "arn:aws:kms:{REGION}:{}:key/{key_id}",
                engine::emulator_account_id()
            ),
            "case {case:?}: expected the key ARN to carry the published key id, got {key_arn:?}"
        );
        // The identity is KMS's, not one CloudFormation computed from the stack and the
        // logical id: a physical resource id in the key-id position is an ARN no key
        // answers to.
        assert!(
            !key_id.contains(row.stack_name) && !key_id.contains("Key"),
            "case {case:?}: expected the key id KMS minted, got the CloudFormation \
             physical-id shape {key_id:?}"
        );

        // KMS resolves the stack's handle, by either identifier form, to one key.
        for (form, identifier) in [("the key id", &key_id), ("the key ARN", &key_arn)] {
            let described =
                kms_ok(&router, case, "DescribeKey", json!({"KeyId": identifier})).await;
            let metadata = &described["KeyMetadata"];
            assert_eq!(
                (
                    metadata["KeyId"].as_str(),
                    metadata["Arn"].as_str(),
                    metadata["KeyManager"].as_str()
                ),
                (
                    Some(key_id.as_str()),
                    Some(key_arn.as_str()),
                    Some("CUSTOMER")
                ),
                "case {case:?}: asked by {form}, KMS had to resolve the stack's key, got \
                 {described}"
            );
            for (member, expected) in &row.metadata {
                assert_eq!(
                    &metadata[*member],
                    expected,
                    "case {case:?}: asked by {form}, expected {member} to be {expected}, got \
                     {described}"
                );
            }
        }

        if let Some(expected) = &row.policy {
            let reported = kms_ok(
                &router,
                case,
                "GetKeyPolicy",
                json!({"KeyId": key_arn, "PolicyName": "default"}),
            )
            .await;
            let document: Value = reported["Policy"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            assert_eq!(
                &document, expected,
                "case {case:?}: expected the declared key policy on the key, got {reported}"
            );
        }

        if let Some((enabled, period)) = &row.rotation {
            let reported = kms_ok(
                &router,
                case,
                "GetKeyRotationStatus",
                json!({"KeyId": key_arn}),
            )
            .await;
            assert_eq!(
                reported["KeyRotationEnabled"].as_bool(),
                Some(*enabled),
                "case {case:?}: expected the declared rotation state, got {reported}"
            );
            assert_eq!(
                reported["RotationPeriodInDays"].as_i64(),
                *period,
                "case {case:?}: expected the declared rotation period, got {reported}"
            );
        }

        let reported = kms_ok(&router, case, "ListResourceTags", json!({"KeyId": key_arn})).await;
        let carried: BTreeMap<String, String> = reported["Tags"]
            .as_array()
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| {
                        Some((
                            tag["TagKey"].as_str()?.to_string(),
                            tag["TagValue"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected: BTreeMap<String, String> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            carried, expected,
            "case {case:?}: expected the declared tags on the key, got {reported}"
        );
    }
}

/// A template declaring a key and a queue keyed by it, the canonical SSE-KMS pair: the
/// queue's `KmsMasterKeyId` is the key's own `Fn::GetAtt … Arn`.
fn key_and_queue_template(queue_name: &str, policy: &Value) -> String {
    json!({
        "Resources": {
            "Key": {"Type": "AWS::KMS::Key", "Properties": {"KeyPolicy": policy}},
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": queue_name,
                    "SqsManagedSseEnabled": false,
                    "KmsMasterKeyId": {"Fn::GetAtt": ["Key", "Arn"]},
                },
            },
        },
        "Outputs": {
            "QueueUrl": {"Value": {"Ref": "Queue"}},
            "KeyArn": {"Value": {"Fn::GetAtt": ["Key", "Arn"]}},
        },
    })
    .to_string()
}

/// A key policy allowing every principal `allowed` and denying it `denied`, so a row can
/// state exactly what the queue's key permits the caller.
fn key_policy(allowed: &str, denied: Option<&[&str]>) -> Value {
    let mut statements = vec![json!({
        "Sid": "grant",
        "Effect": "Allow",
        "Principal": "*",
        "Action": allowed,
        "Resource": "*",
    })];
    if let Some(denied) = denied {
        statements.push(json!({
            "Sid": "refuse",
            "Effect": "Deny",
            "Principal": "*",
            "Action": denied,
            "Resource": "*",
        }));
    }
    json!({"Version": "2012-10-17", "Statement": statements})
}

/// The queue a stack keys with its OWN key sends and receives: the `KmsMasterKeyId` the
/// stack put on it resolves in KMS, so a producer is served rather than refused for a key
/// that does not exist — and it is refused with the KMS error of the refusal's class when
/// the key's own policy does not permit the action.
///
/// The rows differ along what the key's policy says about the two data-plane actions.
#[tokio::test]
async fn a_queue_keyed_by_the_stacks_own_kms_key_serves_the_message_plane() {
    let router = build_router();
    // (case, stack name, queue name, the key's policy, the code a send must be refused with)
    let rows: Vec<(&str, &str, &str, Value, Option<&str>)> = vec![
        (
            "a key policy giving the account root every action",
            "sse-kms-root-stack",
            "sse-kms-root-queue",
            root_key_policy(),
            None,
        ),
        (
            "a key policy granting the data key generation and the decryption outright",
            "sse-kms-granted-stack",
            "sse-kms-granted-queue",
            key_policy("kms:*", None),
            None,
        ),
        (
            "a key policy denying the data key generation, which quarantines the key",
            "sse-kms-denied-stack",
            "sse-kms-denied-queue",
            key_policy("kms:*", Some(&["kms:GenerateDataKey", "kms:Decrypt"])),
            Some("KmsAccessDenied"),
        ),
    ];

    for (case, stack_name, queue_name, policy, refused_with) in &rows {
        let body = create_and_settle(
            &router,
            case,
            stack_name,
            &key_and_queue_template(queue_name, policy),
        )
        .await;
        let outputs = outputs_of(&body);
        let queue_url = output(&outputs, case, "QueueUrl");
        let key_arn = output(&outputs, case, "KeyArn");

        // The queue really carries the key the stack created, and that identifier resolves
        // in KMS.
        let reported = sqs_ok(
            &router,
            case,
            "GetQueueAttributes",
            json!({"QueueUrl": queue_url, "AttributeNames": ["All"]}),
        )
        .await;
        assert_eq!(
            reported["Attributes"]["KmsMasterKeyId"].as_str(),
            Some(key_arn.as_str()),
            "case {case:?}: expected the queue on the stack's own key, got {reported}"
        );
        kms_ok(&router, case, "DescribeKey", json!({"KeyId": key_arn})).await;

        let body_text = format!("sealed under {queue_name}");
        let (status, sent) = sqs(
            &router,
            "SendMessage",
            &json!({"QueueUrl": queue_url, "MessageBody": body_text}),
        )
        .await;
        match refused_with {
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&sent).as_str()),
                    (400, *expected),
                    "case {case:?}: expected the send to be refused {expected}, got {sent}"
                );
                let reported = sqs_ok(
                    &router,
                    case,
                    "GetQueueAttributes",
                    json!({
                        "QueueUrl": queue_url,
                        "AttributeNames": ["ApproximateNumberOfMessages"],
                    }),
                )
                .await;
                assert_eq!(
                    reported["Attributes"]["ApproximateNumberOfMessages"].as_str(),
                    Some("0"),
                    "case {case:?}: a refused send must store nothing, got {reported}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the send to be served by the stack's key, got \
                     {status} with {sent}"
                );
                let received = sqs_ok(
                    &router,
                    case,
                    "ReceiveMessage",
                    json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
                )
                .await;
                let bodies: Vec<String> = received["Messages"]
                    .as_array()
                    .map(|messages| {
                        messages
                            .iter()
                            .filter_map(|message| {
                                message["Body"].as_str().map(ToString::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    bodies.as_slice(),
                    &[body_text.clone()][..],
                    "case {case:?}: expected the body back byte for byte, got {received}"
                );
            }
        }
    }
}

/// A stack that moves its queue from the Amazon SQS owned key onto a key the SAME
/// deployment declares keeps every message the queue already held readable, and takes new
/// ones: the key the update creates is one KMS resolves, so neither the messages written
/// before the update nor the ones written after it are lost to a key that does not exist.
///
/// The move back is the same rule in the other direction: a deployment that no longer names
/// a key takes the queue off it and schedules the key it stopped declaring for deletion,
/// with everything the queue holds still readable.
#[tokio::test]
async fn an_update_onto_the_stacks_own_kms_key_keeps_earlier_messages_readable() {
    let router = build_router();
    let case = "an SSE-SQS queue updated onto a key the same stack declares";
    let stack_name = "sse-update-stack";
    let queue_name = "sse-update-queue";
    let before = json!({
        "Resources": {
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": {"QueueName": queue_name, "SqsManagedSseEnabled": true},
            },
        },
        "Outputs": {"QueueUrl": {"Value": {"Ref": "Queue"}}},
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &before).await;
    let queue_url = output(&outputs_of(&body), case, "QueueUrl");
    sqs_ok(
        &router,
        case,
        "SendMessage",
        json!({"QueueUrl": queue_url, "MessageBody": "written in the SSE-SQS era"}),
    )
    .await;

    // The update declares the key and puts it on the queue, which is the one template the
    // canonical SSE-KMS deployment writes.
    let body = update_and_settle(
        &router,
        case,
        stack_name,
        &key_and_queue_template(queue_name, &root_key_policy()),
    )
    .await;
    let outputs = outputs_of(&body);
    let key_arn = output(&outputs, case, "KeyArn");
    assert_eq!(
        output(&outputs, case, "QueueUrl"),
        queue_url,
        "case {case:?}: the update must keep the queue it already had"
    );
    kms_ok(&router, case, "DescribeKey", json!({"KeyId": key_arn})).await;
    let reported = sqs_ok(
        &router,
        case,
        "GetQueueAttributes",
        json!({"QueueUrl": queue_url, "AttributeNames": ["All"]}),
    )
    .await;
    assert_eq!(
        reported["Attributes"]["KmsMasterKeyId"].as_str(),
        Some(key_arn.as_str()),
        "case {case:?}: expected the queue moved onto the stack's own key, got {reported}"
    );

    sqs_ok(
        &router,
        case,
        "SendMessage",
        json!({"QueueUrl": queue_url, "MessageBody": "written in the SSE-KMS era"}),
    )
    .await;

    // And back: a deployment that no longer names a key takes the queue off the one it is
    // on — an empty `KmsMasterKeyId` is how Amazon SQS's own API asks for that — and the
    // key the stack stops declaring is scheduled for deletion. Everything the queue holds
    // stays readable across that move too.
    let body = update_and_settle(&router, case, stack_name, &before).await;
    assert_eq!(
        output(&outputs_of(&body), case, "QueueUrl"),
        queue_url,
        "case {case:?}: the second update must keep the queue it already had"
    );
    let reported = sqs_ok(
        &router,
        case,
        "GetQueueAttributes",
        json!({"QueueUrl": queue_url, "AttributeNames": ["All"]}),
    )
    .await;
    assert_eq!(
        (
            reported["Attributes"]["KmsMasterKeyId"].as_str(),
            reported["Attributes"]["SqsManagedSseEnabled"].as_str(),
        ),
        (None, Some("true")),
        "case {case:?}: expected the queue back on the Amazon SQS owned key, got {reported}"
    );
    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": key_arn})).await;
    assert_eq!(
        described["KeyMetadata"]["KeyState"].as_str(),
        Some("PendingDeletion"),
        "case {case:?}: the key the stack stopped declaring had to be scheduled for \
         deletion, got {described}"
    );
    sqs_ok(
        &router,
        case,
        "SendMessage",
        json!({"QueueUrl": queue_url, "MessageBody": "written after the key was dropped"}),
    )
    .await;

    let received = sqs_ok(
        &router,
        case,
        "ReceiveMessage",
        json!({"QueueUrl": queue_url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let mut bodies: Vec<String> = received["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message["Body"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();
    bodies.sort();
    assert_eq!(
        bodies.as_slice(),
        &[
            "written after the key was dropped".to_string(),
            "written in the SSE-KMS era".to_string(),
            "written in the SSE-SQS era".to_string(),
        ][..],
        "case {case:?}: every era's bodies had to come over byte for byte, got {received}"
    );
}

/// An update applies a changed declaration to the key the stack already has, through the
/// KMS call that owns each property — never by replacing it: the key keeps the identity
/// everything that references it resolves through.
///
/// The rows differ along which property changes and which KMS call that takes: the
/// description, the key policy, the enabled switch, the rotation switch, and the tags
/// CloudFormation gives the key (one added, one re-valued, one dropped).
#[tokio::test]
async fn an_update_applies_each_changed_key_property_through_the_kms_call_that_owns_it() {
    let router = build_router();
    let case = "a key whose every changeable property the update changes";
    let stack_name = "kms-updated-stack";
    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &one_key_template(&json!({
            "Description": "before the update",
            "KeyPolicy": root_key_policy(),
            "EnableKeyRotation": true,
            "RotationPeriodInDays": 120,
            "Tags": [
                {"Key": "kept", "Value": "as it was"},
                {"Key": "revalued", "Value": "before"},
                {"Key": "dropped", "Value": "before"},
            ],
        })),
    )
    .await;
    let outputs = outputs_of(&body);
    let key_id = output(&outputs, case, "KeyRef");
    let key_arn = output(&outputs, case, "KeyArn");

    let narrowed = key_policy("kms:DescribeKey", None);
    let body = update_and_settle(
        &router,
        case,
        stack_name,
        &one_key_template(&json!({
            "Description": "after the update",
            "KeyPolicy": narrowed,
            "Enabled": false,
            "EnableKeyRotation": false,
            "Tags": [
                {"Key": "kept", "Value": "as it was"},
                {"Key": "revalued", "Value": "after"},
                {"Key": "added", "Value": "after"},
            ],
        })),
    )
    .await;
    let outputs = outputs_of(&body);
    assert_eq!(
        (
            output(&outputs, case, "KeyRef"),
            output(&outputs, case, "KeyArn")
        ),
        (key_id.clone(), key_arn.clone()),
        "case {case:?}: an in-place update must not change the key's identity"
    );

    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": key_arn})).await;
    let metadata = &described["KeyMetadata"];
    assert_eq!(
        (
            metadata["Description"].as_str(),
            metadata["Enabled"].as_bool(),
            metadata["KeyState"].as_str(),
        ),
        (Some("after the update"), Some(false), Some("Disabled")),
        "case {case:?}: expected the declared description and enabled state, got {described}"
    );
    let reported = kms_ok(
        &router,
        case,
        "GetKeyPolicy",
        json!({"KeyId": key_arn, "PolicyName": "default"}),
    )
    .await;
    let document: Value = reported["Policy"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        document, narrowed,
        "case {case:?}: expected the key policy the update declares, got {reported}"
    );
    let rotation = kms_ok(
        &router,
        case,
        "GetKeyRotationStatus",
        json!({"KeyId": key_arn}),
    )
    .await;
    assert_eq!(
        rotation["KeyRotationEnabled"].as_bool(),
        Some(false),
        "case {case:?}: expected rotation turned off by the update, got {rotation}"
    );
    let reported = kms_ok(&router, case, "ListResourceTags", json!({"KeyId": key_arn})).await;
    let carried: BTreeMap<String, String> = reported["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    Some((
                        tag["TagKey"].as_str()?.to_string(),
                        tag["TagValue"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let expected: BTreeMap<String, String> = [
        ("added", "after"),
        ("kept", "as it was"),
        ("revalued", "after"),
    ]
    .iter()
    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
    .collect();
    assert_eq!(
        carried, expected,
        "case {case:?}: expected the tag set the update declares (one added, one \
         re-valued, one dropped), got {reported}"
    );
}

/// Deleting the stack deletes the key it created, which for a KMS key is scheduling its
/// deletion: the key is the stack's own rather than a leftover, and the waiting period is
/// the one the template asked for.
#[tokio::test]
async fn deleting_the_stack_schedules_its_kms_key_for_deletion() {
    let router = build_router();
    let case = "a key with a declared waiting period";
    let stack_name = "kms-deleted-stack";
    let declared_window = 7;
    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &one_key_template(&json!({
            "KeyPolicy": root_key_policy(),
            "PendingWindowInDays": declared_window,
        })),
    )
    .await;
    let key_arn = output(&outputs_of(&body), case, "KeyArn");

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let described = kms_ok(&router, case, "DescribeKey", json!({"KeyId": key_arn})).await;
    let metadata = &described["KeyMetadata"];
    assert_eq!(
        metadata["KeyState"].as_str(),
        Some("PendingDeletion"),
        "case {case:?}: expected the stack's deletion to schedule the key's, got {described}"
    );
    assert!(
        metadata["DeletionDate"].as_f64().is_some(),
        "case {case:?}: expected the instant KMS will delete the key, got {described}"
    );
    // The stack itself records the deletion it carried out.
    let (status, body) = cloudformation(
        &router,
        "DescribeStacks",
        &[("StackName", stack_name)],
    )
    .await;
    assert!(
        status != 200 || element(&body, "StackStatus") == "DELETE_COMPLETE",
        "case {case:?}: expected the stack to report its deletion, got {status} with {body}"
    );
}
