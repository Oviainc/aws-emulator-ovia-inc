// SPDX-License-Identifier: CC0-1.0

//! CloudFormation creates an `AWS::OpenSearchServerless::*` resource in OpenSearch
//! Serverless, and `Ref` resolves to the identifier that service assigned.
//!
//! The rule under test: a stack's resource is only real if the service that OWNS it
//! created it, so a template declaring an `AWS::OpenSearchServerless::SecurityPolicy` and
//! an `AWS::OpenSearchServerless::Collection` leaves a policy `GetSecurityPolicy` reports
//! and a collection `BatchGetCollection` reports — addressed by the server-minted
//! `CollectionId` that `{"Ref": <collection>}` returns, not by the name the template
//! declared — and deleting the stack takes both away. A resource the owning service
//! REFUSES is not reported as created: the failed create is rolled back and the stack
//! settles into `ROLLBACK_COMPLETE` carrying that refusal.
//!
//! This suite needs TWO services on one engine — CloudFormation reaches OpenSearch
//! Serverless through the in-process callback, so a single-service router has no
//! OpenSearch Serverless to ask — which is why it lives here rather than in either
//! service's own crate.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::Value;

// Percent-encoding one form value — so a template body carrying `{`, `"` or `&` survives
// the body — is the same job this crate's `transfer_user_with_iam_role` suite already
// publishes, so that one is reused rather than copied.
use crate::transfer_user_with_iam_role::form_encode;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// Long enough for a stack's status transition (500 ms) and a collection's own
/// (`CREATING -> ACTIVE`, and the purge of a `DELETING` collection, 500 ms each) to fall
/// due. Nothing sleeps for either inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// Drive one CloudFormation action over its awsQuery wire, answering the status and the
/// XML body.
fn cloudformation(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
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
        .uri(format!("https://cloudformation.{REGION}.amazonaws.com/"))
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// Drive one OpenSearch Serverless operation over its awsJson1_0 wire, answering the
/// status, the error code (empty on success) and the parsed body.
fn aoss(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("aoss.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("OpenSearchServerless.{operation}"))
        .header("authorization", authorization("aoss"))
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

/// The first `<tag>` text of `xml`, or the empty string. The scraping itself is this
/// crate's `cloudformation_provisions_ecr_public_repository` helper, reused rather than
/// copied; only the "absent reads as empty" convenience is local.
fn xml_text(xml: &str, tag: &str) -> String {
    crate::cloudformation_provisions_ecr_public_repository::xml_text(xml, tag).unwrap_or_default()
}

/// The `OutputValue` a `DescribeStacks` response carries for one `OutputKey`, read member
/// by member so a key stays paired with its own value.
fn output_value(xml: &str, key: &str) -> String {
    let mut rest = xml;
    while let Some(open) = rest.find("<member>") {
        let after_open = &rest[open + "<member>".len()..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        let entry = &after_open[..close];
        if xml_text(entry, "OutputKey") == key {
            return xml_text(entry, "OutputValue");
        }
        rest = &after_open[close + "</member>".len()..];
    }
    String::new()
}

/// True when `id` satisfies `CollectionId`'s modelled `^[a-z0-9]{3,40}$`, which is what a
/// `Ref` to a collection has to return for `BatchGetCollection` to accept it.
fn is_collection_id(id: &str) -> bool {
    (3..=40).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The encryption policy document a stack-created policy carries, as the template spells
/// it: one rule covering the collection the same stack declares.
fn policy_document(collection: &str) -> String {
    format!(
        "{{\"Rules\":[{{\"ResourceType\":\"collection\",\"Resource\":[\"collection/{collection}\"]}}],\
         \"AWSOwnedKey\":true}}"
    )
}

/// A template declaring an encryption policy and a collection that relies on it, with the
/// identifiers of both as outputs.
fn template(policy: &str, collection: &str) -> String {
    format!(
        "{{\"Resources\":{{\
           \"Policy\":{{\"Type\":\"AWS::OpenSearchServerless::SecurityPolicy\",\
             \"Properties\":{{\"Name\":\"{policy}\",\"Type\":\"encryption\",\
               \"Policy\":{},\"Description\":\"the stack's encryption policy\"}}}},\
           \"Collection\":{{\"Type\":\"AWS::OpenSearchServerless::Collection\",\
             \"DependsOn\":\"Policy\",\
             \"Properties\":{{\"Name\":\"{collection}\",\"Type\":\"SEARCH\",\
               \"Description\":\"deployed by a stack\"}}}}}},\
         \"Outputs\":{{\
           \"CollectionId\":{{\"Value\":{{\"Ref\":\"Collection\"}}}},\
           \"CollectionArn\":{{\"Value\":{{\"Fn::GetAtt\":[\"Collection\",\"Arn\"]}}}},\
           \"PolicyName\":{{\"Value\":{{\"Ref\":\"Policy\"}}}}}}}}",
        serde_json::Value::String(policy_document(collection)),
    )
}

/// A template declaring a collection and NO encryption policy for it, which OpenSearch
/// Serverless refuses.
fn template_without_a_policy(collection: &str) -> String {
    format!(
        "{{\"Resources\":{{\
           \"Collection\":{{\"Type\":\"AWS::OpenSearchServerless::Collection\",\
             \"Properties\":{{\"Name\":\"{collection}\"}}}}}},\
         \"Outputs\":{{\"CollectionId\":{{\"Value\":{{\"Ref\":\"Collection\"}}}}}}}}"
    )
}

/// An encryption policy document covering EVERY collection whose name starts with
/// `prefix` — the trailing-wildcard form AWS's encryption-policy documentation gives for a
/// `Resource` entry — so a re-deploy may rename the collection while the policy's own
/// document stays exactly as it was.
fn wildcard_policy_document(prefix: &str) -> String {
    format!(
        "{{\"Rules\":[{{\"ResourceType\":\"collection\",\"Resource\":[\"collection/{prefix}*\"]}}],\
         \"AWSOwnedKey\":true}}"
    )
}

/// The same two resources under a wildcard policy, so the only thing a re-deploy of it
/// changes is the collection's declared `Name`. `marker` is an extra OUTPUT, which makes a
/// re-deploy a real update (CloudFormation refuses one whose template is identical with
/// "No updates are to be performed") without touching either resource.
fn template_under_a_wildcard_policy(
    policy: &str,
    prefix: &str,
    collection: &str,
    marker: &str,
) -> String {
    format!(
        "{{\"Resources\":{{\
           \"Policy\":{{\"Type\":\"AWS::OpenSearchServerless::SecurityPolicy\",\
             \"Properties\":{{\"Name\":\"{policy}\",\"Type\":\"encryption\",\
               \"Policy\":{}}}}},\
           \"Collection\":{{\"Type\":\"AWS::OpenSearchServerless::Collection\",\
             \"DependsOn\":\"Policy\",\
             \"Properties\":{{\"Name\":\"{collection}\",\"Type\":\"SEARCH\"}}}}}},\
         \"Outputs\":{{\"CollectionId\":{{\"Value\":{{\"Ref\":\"Collection\"}}}},\
           \"Marker\":{{\"Value\":\"{marker}\"}}}}}}",
        serde_json::Value::String(wildcard_policy_document(prefix)),
    )
}

/// The one `collectionDetails` entry `BatchGetCollection` reports for `id`, or `None` when
/// it reports the id as not found.
fn read_collection(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    id: &str,
) -> Option<Value> {
    let (status, code, read) = aoss(
        router,
        runtime,
        "BatchGetCollection",
        serde_json::json!({ "ids": [id] }),
    );
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {case}: expected 200 from BatchGetCollection({id}), got {status} {code} with {read}"
    );
    read.get("collectionDetails")
        .and_then(Value::as_array)
        .and_then(|details| details.first())
        .cloned()
}

/// A re-deploy of a resource whose identifier the OWNING SERVICE assigns keeps that
/// identifier while the declaration still asks for the same resource, and REPLACES the
/// resource when the declaration renames it — the identity of such a resource is its
/// logical id, its type and the name it declares, because the template never carries the
/// minted identifier itself.
#[test]
fn redeploying_a_collection_keeps_its_id_unless_the_template_renames_it() {
    std::thread::Builder::new()
        .name("aoss-cloudformation-redeploy".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            let policy_name = "sim-secpol-redeploy";
            let prefix = "sim-redeploy";
            let first = "sim-redeploy-one";
            let renamed = "sim-redeploy-two";
            let stack_name = "sim-aoss-redeploy";

            let (status, body) = cloudformation(
                &router,
                &runtime,
                "CreateStack",
                &[
                    ("StackName", stack_name),
                    (
                        "TemplateBody",
                        &template_under_a_wildcard_policy(policy_name, prefix, first, "created"),
                    ),
                ],
            );
            assert_eq!(
                status, 200,
                "case CreateStack: expected 200, got {status} with {body}"
            );
            // The stack's own transition, then the collection's CREATING -> ACTIVE.
            std::thread::sleep(SETTLE_WAIT);
            std::thread::sleep(SETTLE_WAIT);

            let (_, described) = cloudformation(
                &router,
                &runtime,
                "DescribeStacks",
                &[("StackName", stack_name)],
            );
            let created_id = output_value(&described, "CollectionId");
            assert!(
                is_collection_id(&created_id),
                "case CreateStack: Ref must be a CollectionId, got {created_id:?} in {described}"
            );

            // Each row re-deploys the stack and says what the collection's identity owes:
            // the SAME declaration keeps the minted id, a RENAMED one replaces it. Each
            // row's marker output is what makes its deployment an update at all.
            let rows = [
                ("the same declaration", first, "redeployed", true),
                ("a renamed collection", renamed, "renamed", false),
            ];
            let mut previous_id = created_id.clone();
            for (row, collection, marker, keeps_its_id) in rows {
                let (status, body) = cloudformation(
                    &router,
                    &runtime,
                    "UpdateStack",
                    &[
                        ("StackName", stack_name),
                        (
                            "TemplateBody",
                            &template_under_a_wildcard_policy(
                                policy_name,
                                prefix,
                                collection,
                                marker,
                            ),
                        ),
                    ],
                );
                assert_eq!(
                    status, 200,
                    "case {row}: expected 200 from UpdateStack, got {status} with {body}"
                );
                // The stack's transition, then the collection the update created or deleted.
                std::thread::sleep(SETTLE_WAIT);
                std::thread::sleep(SETTLE_WAIT);

                let (_, described) = cloudformation(
                    &router,
                    &runtime,
                    "DescribeStacks",
                    &[("StackName", stack_name)],
                );
                assert_eq!(
                    xml_text(&described, "StackStatus"),
                    "UPDATE_COMPLETE",
                    "case {row}: the update must complete, got {described}"
                );
                let id = output_value(&described, "CollectionId");
                assert!(
                    is_collection_id(&id),
                    "case {row}: Ref must still be a CollectionId, got {id:?} in {described}"
                );
                if keeps_its_id {
                    assert_eq!(
                        id, previous_id,
                        "case {row}: the minted id must carry forward, got {id:?} in {described}"
                    );
                } else {
                    assert_ne!(
                        id, previous_id,
                        "case {row}: a rename replaces the collection, so its id must change, \
                         got {id:?} in {described}"
                    );
                    // The resource the re-deploy replaced is deleted, not left behind.
                    assert!(
                        read_collection(&router, &runtime, row, &previous_id).is_none(),
                        "case {row}: the replaced collection {previous_id:?} must be gone"
                    );
                }
                let detail = read_collection(&router, &runtime, row, &id).unwrap_or_else(|| {
                    panic!("case {row}: the collection {id:?} must be readable")
                });
                assert_eq!(
                    detail.get("name").and_then(Value::as_str),
                    Some(collection),
                    "case {row}: the collection carries the declared name, got {detail}"
                );
                assert_eq!(
                    detail.get("status").and_then(Value::as_str),
                    Some("ACTIVE"),
                    "case {row}: the collection must be ACTIVE, got {detail}"
                );
                previous_id = id;
            }
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the test thread completes");
}

#[test]
fn a_stack_creates_its_opensearch_serverless_resources_and_ref_returns_the_collection_id() {
    std::thread::Builder::new()
        .name("aoss-cloudformation-resources".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            let policy_name = "sim-secpol-one";
            let collection_name = "sim-cfncoll-one";
            let stack_name = "sim-aoss-stack";

            let (status, body) = cloudformation(
                &router,
                &runtime,
                "CreateStack",
                &[
                    ("StackName", stack_name),
                    ("TemplateBody", &template(policy_name, collection_name)),
                ],
            );
            assert_eq!(
                status, 200,
                "case CreateStack: expected 200, got {status} with {body}"
            );

            // The stack settles on its own; the collection it created settles with it.
            std::thread::sleep(SETTLE_WAIT);
            let (status, described) = cloudformation(
                &router,
                &runtime,
                "DescribeStacks",
                &[("StackName", stack_name)],
            );
            assert_eq!(
                status, 200,
                "case DescribeStacks: expected 200, got {status} with {described}"
            );
            assert_eq!(
                xml_text(&described, "StackStatus"),
                "CREATE_COMPLETE",
                "case DescribeStacks: the stack must have completed, got {described}"
            );

            // `Ref` on an AWS::OpenSearchServerless::Collection returns the collection's
            // Id, which is the identifier BatchGetCollection addresses it by.
            let collection_id = output_value(&described, "CollectionId");
            assert!(
                is_collection_id(&collection_id),
                "case Ref Collection: must be a CollectionId (^[a-z0-9]{{3,40}}$), got {collection_id:?} in {described}"
            );
            assert_ne!(
                collection_id, collection_name,
                "case Ref Collection: the Ref must be the collection's id, not its name"
            );

            let (status, code, read) = aoss(
                &router,
                &runtime,
                "BatchGetCollection",
                serde_json::json!({ "ids": [collection_id.clone()] }),
            );
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case BatchGetCollection(Ref Collection): expected 200, got {status} {code} with {read}"
            );
            let details = read
                .get("collectionDetails")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                details.len(),
                1,
                "case BatchGetCollection(Ref Collection): expected exactly one detail, got {read}"
            );
            let detail = &details[0];
            assert_eq!(
                detail.get("name").and_then(Value::as_str),
                Some(collection_name),
                "case BatchGetCollection(Ref Collection): the collection carries the declared name, got {detail}"
            );
            assert_eq!(
                detail.get("status").and_then(Value::as_str),
                Some("ACTIVE"),
                "case BatchGetCollection(Ref Collection): the collection must be ACTIVE, got {detail}"
            );
            // `Fn::GetAtt Collection.Arn` is the ARN the owning service minted for it.
            assert_eq!(
                output_value(&described, "CollectionArn"),
                detail
                    .get("arn")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "case GetAtt Collection.Arn: must be the stored ARN, got {described}"
            );

            // And the policy the stack declared exists in the service, with the template's
            // own document.
            assert_eq!(
                output_value(&described, "PolicyName"),
                policy_name,
                "case Ref Policy: must be the policy's name, got {described}"
            );
            let (status, code, policy) = aoss(
                &router,
                &runtime,
                "GetSecurityPolicy",
                serde_json::json!({ "type": "encryption", "name": policy_name }),
            );
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case GetSecurityPolicy(stack-created): expected 200, got {status} {code} with {policy}"
            );
            let expected: Value = serde_json::from_str(&policy_document(collection_name))
                .expect("the template's policy document is JSON");
            assert_eq!(
                policy.get("securityPolicyDetail").and_then(|d| d.get("policy")),
                Some(&expected),
                "case GetSecurityPolicy(stack-created): the document must be the template's, got {policy}"
            );

            // Deleting the stack takes both resources with it.
            let (status, body) = cloudformation(
                &router,
                &runtime,
                "DeleteStack",
                &[("StackName", stack_name)],
            );
            assert_eq!(
                status, 200,
                "case DeleteStack: expected 200, got {status} with {body}"
            );
            // The stack's own transition, then the collection's DELETING window.
            std::thread::sleep(SETTLE_WAIT);
            std::thread::sleep(SETTLE_WAIT);

            let (status, code, policy) = aoss(
                &router,
                &runtime,
                "GetSecurityPolicy",
                serde_json::json!({ "type": "encryption", "name": policy_name }),
            );
            // This service answers every modelled client error with the one client-error
            // status its own suite pins (`CLIENT_ERROR` in the crate's
            // `tests/all/client_error_status.rs`), so the refusal is read off the CODE.
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFoundException"),
                "case GetSecurityPolicy(after DeleteStack): expected the not-found refusal, got {status} {code} with {policy}"
            );
            let (status, code, read) = aoss(
                &router,
                &runtime,
                "BatchGetCollection",
                serde_json::json!({ "ids": [collection_id] }),
            );
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case BatchGetCollection(after DeleteStack): expected 200, got {status} {code} with {read}"
            );
            assert_eq!(
                read.get("collectionDetails"),
                Some(&serde_json::json!([])),
                "case BatchGetCollection(after DeleteStack): the collection is gone, got {read}"
            );
            assert_eq!(
                read.get("collectionErrorDetails")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(1),
                "case BatchGetCollection(after DeleteStack): it is reported NOT_FOUND, got {read}"
            );
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the test thread completes");
}

#[test]
fn a_stack_whose_resource_the_owning_service_refuses_fails_instead_of_reporting_success() {
    std::thread::Builder::new()
        .name("aoss-cloudformation-refusal".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            let collection_name = "sim-unencrypted";
            let stack_name = "sim-aoss-failing-stack";

            let (status, body) = cloudformation(
                &router,
                &runtime,
                "CreateStack",
                &[
                    ("StackName", stack_name),
                    ("TemplateBody", &template_without_a_policy(collection_name)),
                ],
            );
            assert_eq!(
                status, 200,
                "case CreateStack(no encryption policy): expected 200, got {status} with {body}"
            );

            std::thread::sleep(SETTLE_WAIT);
            let (status, described) = cloudformation(
                &router,
                &runtime,
                "DescribeStacks",
                &[("StackName", stack_name)],
            );
            assert_eq!(
                status, 200,
                "case DescribeStacks(no encryption policy): expected 200, got {status} with {described}"
            );
            assert_eq!(
                xml_text(&described, "StackStatus"),
                "ROLLBACK_COMPLETE",
                "case DescribeStacks(no encryption policy): the failed create must have been rolled back, got {described}"
            );
            let reason = xml_text(&described, "StackStatusReason");
            assert!(
                reason.contains("Collection") && reason.contains("encryption"),
                "case DescribeStacks(no encryption policy): the reason must name the resource and the service's refusal, got {reason:?}"
            );
            // Nothing was created, so no collection carries the declared name.
            let (status, code, read) = aoss(
                &router,
                &runtime,
                "BatchGetCollection",
                serde_json::json!({ "names": [collection_name] }),
            );
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case BatchGetCollection(by name): expected 200, got {status} {code} with {read}"
            );
            assert_eq!(
                read.get("collectionDetails"),
                Some(&serde_json::json!([])),
                "case BatchGetCollection(by name): the refused collection does not exist, got {read}"
            );
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the test thread completes");
}
