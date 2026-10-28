// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares a queue's access policy really applies it — in SQS.
//!
//! The rule under test: **a resource type whose configuration IS an attribute of another
//! service's resource is deployed by writing that attribute, so a stack that reports
//! `CREATE_COMPLETE` has left the owning service holding it.** A queue's access policy is
//! its `Policy` attribute, and the two resource types that carry one — `AWS::SQS::QueuePolicy`
//! for a list of queues and `AWS::SQS::QueueInlinePolicy` for a single queue — therefore
//! deploy as a `SetQueueAttributes`: after the create waiter returns, `GetQueueAttributes`
//! reports the rendered document, the principal it grants can really use the queue, and a
//! stack update that stops granting it REVOKES that access. Three crates have to agree for
//! that to hold (CloudFormation makes the call, SQS serves and then enforces it, IAM and STS
//! vend the principal), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: which of the two types carries the
//! document and therefore which property names the queue, how many queues one policy
//! resource applies to, and whether the stack's next revision keeps the grant, states a
//! different one or drops the policy resource altogether.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the stack's queues, their
/// ARNs and the role belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// The access key id the stack owner's calls present: the account's own published
/// credential, which holds no IAM policies and keeps full control of its own queues.
const OWNER_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// Long enough for the stack's clock-owed transition (500 ms) to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly
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
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` — which is what
/// routes the request — and carries the access key id the called service resolves the caller
/// from. The signature is never checked: request authentication is off.
fn authorization(access_key_id: &str, service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` value the way an AWS SDK does, so
/// a template body full of `{`, `"` and `&` survives the body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One awsQuery call, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation operation, with the owner's own credential.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, String)],
) -> (u16, String) {
    query(
        router,
        OWNER_KEY,
        "cloudformation",
        "2010-05-15",
        action,
        members,
    )
    .await
}

/// One Amazon SQS operation over its awsJson1_0 wire, presenting `access_key_id`.
async fn sqs(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization(access_key_id, "sqs"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    found
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// The queue url of the queue called `name`, as this emulator mints it.
fn queue_url(name: &str) -> String {
    format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/{name}")
}

/// The ARN of the queue called `name`.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// The stored `Policy` attribute of the queue at `url`, as the document it is, read with the
/// owner's own credential. `None` when the queue carries no policy at all.
async fn stored_policy(router: &MultiServiceRouter, case: &str, url: &str) -> Option<Value> {
    let (status, reported) = sqs(
        router,
        OWNER_KEY,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["Policy"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetQueueAttributes, got {status} with {reported}"
    );
    let document = reported
        .get("Attributes")
        .and_then(|attributes| attributes.get("Policy"))
        .and_then(Value::as_str)?;
    Some(
        serde_json::from_str(document)
            .unwrap_or_else(|_| panic!("case {case:?}: the stored policy must be a document")),
    )
}

/// Create a stack and wait past its transition, asserting the create was accepted.
async fn create_and_settle(router: &MultiServiceRouter, case: &str, stack: &str, template: &Value) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack.to_string()),
            ("TemplateBody", template.to_string()),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    assert_eq!(
        stack_status(router, case, stack).await,
        "CREATE_COMPLETE",
        "case {case:?}: the stack must have created every resource it declares"
    );
}

/// Update a stack and wait past its transition, asserting the update was accepted.
async fn update_and_settle(router: &MultiServiceRouter, case: &str, stack: &str, template: &Value) {
    let (status, body) = cloudformation(
        router,
        "UpdateStack",
        &[
            ("StackName", stack.to_string()),
            ("TemplateBody", template.to_string()),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    assert_eq!(
        stack_status(router, case, stack).await,
        "UPDATE_COMPLETE",
        "case {case:?}: the stack must have applied the change it declares"
    );
}

/// The stack's reported status.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack.to_string())]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
    );
    xml_text(&body, "StackStatus")
        .unwrap_or_else(|| panic!("case {case:?}: DescribeStacks answered no StackStatus: {body}"))
}

/// Assume the role the stack created and answer the access key id of the session — the
/// credential the principal the policy grants then signs its SQS calls with.
async fn assumed_role_key(router: &MultiServiceRouter, case: &str, role_name: &str) -> String {
    let (status, body) = query(
        router,
        OWNER_KEY,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            (
                "RoleArn",
                format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}"),
            ),
            ("RoleSessionName", "probe".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AssumeRole, got {status} with {body}"
    );
    xml_text(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no AccessKeyId"))
}

/// The trust policy that lets this account assume the role the stack declares.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The policy document a `QueuePolicy`/`QueueInlinePolicy` resource declares: one statement
/// granting `sqs:SendMessage` on each of `queues` to the principal `principal`, which the
/// template writes as an intrinsic so the deployed document carries the stack's own role.
fn grant(principal: Value, queues: &[&str]) -> Value {
    let resources: Vec<Value> = queues
        .iter()
        .map(|queue| json!({"Fn::GetAtt": [queue, "Arn"]}))
        .collect();
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "GrantTheStacksRole",
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": "sqs:SendMessage",
            "Resource": resources,
        }],
    })
}

/// A send to the queue at `url` by `access_key_id`: its HTTP status, so a row can assert
/// either outcome.
async fn send(router: &MultiServiceRouter, access_key_id: &str, url: &str) -> (u16, Value) {
    sqs(
        router,
        access_key_id,
        "SendMessage",
        &json!({"QueueUrl": url, "MessageBody": "through the stack's grant"}),
    )
    .await
}

/// A deployed `AWS::SQS::QueuePolicy` lands on every queue it names: the document is readable
/// as the `Policy` attribute, the role it grants can send to those queues, and a revision
/// that stops granting the role revokes that access.
#[tokio::test]
async fn a_deployed_queue_policy_grants_the_role_and_an_update_revokes_it() {
    let router = build_router();
    let stack = "queue-policy-stack";
    let role_name = "queue-policy-stack-role";
    let first = "queue-policy-stack-a";
    let second = "queue-policy-stack-b";
    let role_arn = json!({"Fn::GetAtt": ["Role", "Arn"]});
    let template = |document: Option<Value>| {
        let mut resources = json!({
            "QueueA": {"Type": "AWS::SQS::Queue", "Properties": {"QueueName": first}},
            "QueueB": {"Type": "AWS::SQS::Queue", "Properties": {"QueueName": second}},
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role_name,
                    "AssumeRolePolicyDocument": trust_policy(),
                },
            },
        });
        if let Some(document) = document {
            resources["Policy"] = json!({
                "Type": "AWS::SQS::QueuePolicy",
                "Properties": {
                    "Queues": [{"Ref": "QueueA"}, {"Ref": "QueueB"}],
                    "PolicyDocument": document,
                },
            });
        }
        json!({"Resources": resources})
    };

    let case = "a policy granting the stack's own role";
    create_and_settle(
        &router,
        case,
        stack,
        &template(Some(grant(role_arn.clone(), &["QueueA", "QueueB"]))),
    )
    .await;

    // The rendered document is on BOTH queues, with the role's real ARN in it rather than
    // the intrinsic the template wrote.
    let expected_principal = format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}");
    for queue in [first, second] {
        let document = stored_policy(&router, case, &queue_url(queue))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: {queue} carries no Policy at all"));
        let statement = &document["Statement"][0];
        assert_eq!(
            statement["Principal"]["AWS"],
            json!(expected_principal),
            "case {case:?}: the deployed document must name the role, got {document}"
        );
        assert!(
            statement["Resource"]
                .as_array()
                .is_some_and(|resources| resources.contains(&json!(queue_arn(queue)))),
            "case {case:?}: the deployed document must name {queue}, got {document}"
        );
    }

    // And the grant is real: the role the stack created can send to the queues it named.
    let session = assumed_role_key(&router, case, role_name).await;
    for queue in [first, second] {
        let (status, body) = send(&router, &session, &queue_url(queue)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the granted role's send to {queue} must be allowed, got {status} \
             with {body}"
        );
    }

    // A revision that grants somebody else revokes the role's access, and the document on
    // the queues is the new one.
    let case = "a revision that grants another account instead";
    let elsewhere = json!("arn:aws:iam::210987654321:root".to_string());
    update_and_settle(
        &router,
        case,
        stack,
        &template(Some(grant(elsewhere, &["QueueA", "QueueB"]))),
    )
    .await;
    for queue in [first, second] {
        let (status, body) = send(&router, &session, &queue_url(queue)).await;
        assert_eq!(
            (status, body["__type"].as_str()),
            (403, Some("AccessDenied")),
            "case {case:?}: the revoked role's send to {queue} must be refused, got {status} \
             with {body}"
        );
    }

    // A revision that drops the policy resource takes the document off the queues entirely.
    let case = "a revision that drops the policy resource";
    update_and_settle(&router, case, stack, &template(None)).await;
    for queue in [first, second] {
        assert!(
            stored_policy(&router, case, &queue_url(queue)).await.is_none(),
            "case {case:?}: {queue} must carry no Policy once the resource is gone"
        );
    }
}

/// An `AWS::SQS::QueueInlinePolicy` is the same rule for the type that belongs to ONE queue:
/// the template may declare it, and the document it carries lands on the queue its `Queue`
/// property names.
#[tokio::test]
async fn a_deployed_queue_inline_policy_lands_on_its_own_queue() {
    let router = build_router();
    let stack = "queue-inline-policy-stack";
    let role_name = "queue-inline-policy-stack-role";
    let name = "queue-inline-policy-stack-queue";
    let case = "an inline policy on a KMS-encrypted queue";
    let template = json!({
        "Resources": {
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": {"QueueName": name, "KmsMasterKeyId": "alias/aws/sqs"},
            },
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role_name,
                    "AssumeRolePolicyDocument": trust_policy(),
                },
            },
            "Policy": {
                "Type": "AWS::SQS::QueueInlinePolicy",
                "Properties": {
                    "Queue": {"Ref": "Queue"},
                    "PolicyDocument": grant(json!({"Fn::GetAtt": ["Role", "Arn"]}), &["Queue"]),
                },
            },
        },
    });
    create_and_settle(&router, case, stack, &template).await;

    let document = stored_policy(&router, case, &queue_url(name))
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the queue carries no Policy at all"));
    assert_eq!(
        document["Statement"][0]["Principal"]["AWS"],
        json!(format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}")),
        "case {case:?}: the deployed document must name the role, got {document}"
    );
    let session = assumed_role_key(&router, case, role_name).await;
    let (status, body) = send(&router, &session, &queue_url(name)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the granted role's send must be allowed, got {status} with {body}"
    );
    // The queue the policy belongs to keeps the rest of its declaration.
    let (status, reported) = sqs(
        &router,
        OWNER_KEY,
        "GetQueueAttributes",
        &json!({"QueueUrl": queue_url(name), "AttributeNames": ["All"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetQueueAttributes, got {status} with {reported}"
    );
    let attributes: BTreeMap<String, String> = reported["Attributes"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        attributes.get("KmsMasterKeyId").map(String::as_str),
        Some("alias/aws/sqs"),
        "case {case:?}: the queue must keep the key its declaration names, got {attributes:?}"
    );
    // A queue carrying a key of the account's is not using Amazon SQS's own encryption
    // keys, which is what `SqsManagedSseEnabled` reports.
    assert_eq!(
        attributes.get("SqsManagedSseEnabled").map(String::as_str),
        Some("false"),
        "case {case:?}: a queue encrypted with a named key is not using SSE-SQS, got \
         {attributes:?}"
    );
}
