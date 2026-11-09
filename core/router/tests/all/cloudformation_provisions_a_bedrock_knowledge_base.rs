// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Bedrock::KnowledgeBase` really creates it in
//! Amazon Bedrock Agent, over an Amazon S3 Vectors index of the same template, and the handle
//! the stack hands back is the identifier that service answers to.
//!
//! The rule under test: **for a resource type whose owning service MINTS the resource's
//! identifier, the stack creates the resource through that service and publishes the
//! identifier THAT service answered as the resource's physical id** — so `Ref`, the type's
//! identifier attribute, `DescribeStackResources` and a stack output all carry a value
//! `bedrock-agent GetKnowledgeBase` resolves to the knowledge base the template declared, and
//! a declaration that service REFUSES fails the stack rather than being reported as created.
//! Three crates have to agree for that to hold (CloudFormation makes the call, Bedrock Agent
//! serves it, and resolves the vector store in S3 Vectors), so the suite lives here rather
//! than in any one of them.
//!
//! The rows differ along what the rule generalises over: the verdict the owning service gives
//! — a knowledge base whose embedding model's vectors FIT the index the template declared, and
//! one whose vectors do not, which that service refuses — and, with it, what the stack then
//! reports. The crate-local half of the rule (a declared name and a generated one, one
//! knowledge base and two, the other vector stores, the update and the delete) is
//! `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/\
//! bedrock_knowledge_base_identity.rs`.
//!
//! Every expected value is computed here from what the row declares, except the minted
//! identifier itself, which is the owning service's to assign: what is checked about it is its
//! FORM (the modelled `^[0-9a-zA-Z]{10}$`), that every reader of the stack reports the same
//! one, and that the owning service resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's own clock-owed span and the knowledge base's activation to come
/// due: CloudFormation's `STATUS_TRANSITION_DELAY` is 500 ms and the owning service reports a
/// created knowledge base `CREATING` for its own span before `ACTIVE`, and the stack's create
/// is held open until it is.
const PAST_THE_SPANS: Duration = Duration::from_millis(1_200);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The Region these stacks deploy in.
const REGION: &str = "us-east-1";

/// The embedding model each row declares, and how many values its vectors carry: Titan Text
/// Embeddings G1 produces 1536, Titan Text Embeddings V2 1024. The index a knowledge base
/// writes into has to hold vectors of exactly that length, which is what the refusing row
/// turns on.
const TITAN_G1: &str = "amazon.titan-embed-text-v1";
const TITAN_G1_DIMENSION: u32 = 1536;
const TITAN_V2: &str = "amazon.titan-embed-text-v2:0";

/// How many characters a minted `knowledgeBaseId` carries: the modelled `Id` shape of
/// `GetKnowledgeBase`'s `knowledgeBaseId` (`@pattern ^[0-9a-zA-Z]{10}$`).
const KNOWLEDGE_BASE_ID_CHARACTERS: usize = 10;

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other suite
/// here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
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
        service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000000\
         000000000000000000000000"
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
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon Bedrock Agent operation over the `restJson1` wire its model declares, on the
/// method and URI that operation's own `@http` binding names, signed for `bedrock` — the signing
/// name every Bedrock-family model declares.
async fn bedrock_agent(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("bedrock"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// `GetKnowledgeBase` for one knowledge base: the status and the `knowledgeBase` it answers.
async fn get_knowledge_base(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let (status, document) = bedrock_agent(router, "GET", &format!("/knowledgebases/{id}")).await;
    let knowledge_base = document
        .get("knowledgeBase")
        .cloned()
        .unwrap_or(document.clone());
    (status, knowledge_base)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// A `DescribeStackResources` answer's resources, as the logical-id to physical-id map.
fn physical_ids_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "LogicalResourceId")
        .into_iter()
        .zip(elements(xml, "PhysicalResourceId"))
        .collect()
}

/// The ARN of the knowledge base `id` names, as `KnowledgeBaseArn`'s own pattern spells it.
fn knowledge_base_arn(id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:knowledge-base/{id}")
}

/// Whether `value` is an identifier of the form the owning service mints: exactly
/// [`KNOWLEDGE_BASE_ID_CHARACTERS`] alphanumeric characters, the whole of the modelled pattern.
fn is_knowledge_base_id(value: &str) -> bool {
    value.len() == KNOWLEDGE_BASE_ID_CHARACTERS
        && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// The recorded shape of such a template: a vector bucket, an index of it sized for the
/// embedding model, and the knowledge base bound to that index, with the identifier published
/// three ways.
fn template(model: &str, dimension: u32) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Vectors": {
                "Type": "AWS::S3Vectors::VectorBucket",
                "Properties": { "VectorBucketName": "kbcfn-router-vectors" }
            },
            "Embeddings": {
                "Type": "AWS::S3Vectors::Index",
                "Properties": {
                    "VectorBucketArn": { "Ref": "Vectors" },
                    "IndexName": "kbcfn-router-embeddings",
                    "DataType": "float32",
                    "Dimension": dimension,
                    "DistanceMetric": "cosine"
                }
            },
            "Escalation": {
                "Type": "AWS::Bedrock::KnowledgeBase",
                "Properties": {
                    "Name": "kbcfn-router-escalation",
                    "Description": "the escalation knowledge base",
                    "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/kbcfn-router"),
                    "KnowledgeBaseConfiguration": {
                        "Type": "VECTOR",
                        "VectorKnowledgeBaseConfiguration": {
                            "EmbeddingModelArn":
                                format!("arn:aws:bedrock:{REGION}::foundation-model/{model}")
                        }
                    },
                    "StorageConfiguration": {
                        "Type": "S3_VECTORS",
                        "S3VectorsConfiguration": { "IndexArn": { "Ref": "Embeddings" } }
                    },
                    "Tags": { "estate": "kbcfn" }
                }
            }
        },
        "Outputs": {
            "KnowledgeBaseRef": { "Value": { "Ref": "Escalation" } },
            "KnowledgeBaseId": {
                "Value": { "Fn::GetAtt": ["Escalation", "KnowledgeBaseId"] }
            },
            "KnowledgeBaseArn": {
                "Value": { "Fn::GetAtt": ["Escalation", "KnowledgeBaseArn"] }
            }
        }
    })
    .to_string()
}

/// The knowledge base the stack declares is created in Amazon Bedrock Agent, and the
/// identifier every reader of the stack publishes for it is the one that service minted.
#[tokio::test]
async fn a_stack_publishes_the_knowledge_base_id_bedrock_agent_minted() {
    let case = "a knowledge base over an S3 Vectors index of the same template";
    let router = build_router();
    let stack = "kbcfn-router";
    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(TITAN_G1, TITAN_G1_DIMENSION)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );

    tokio::time::sleep(PAST_THE_SPANS).await;
    let (status, described) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {described}"
    );
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
    );

    let (status, resources) =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
         {resources}"
    );
    let reported = physical_ids_of(&resources);
    let id = reported
        .get("Escalation")
        .unwrap_or_else(|| panic!("case {case:?}: no physical id for the knowledge base in {resources}"))
        .clone();
    assert!(
        is_knowledge_base_id(&id),
        "case {case:?}: expected the physical id to be an identifier of the modelled \
         ^[0-9a-zA-Z]{{10}}$ form, got {id:?}"
    );

    // Every value the stack publishes for the resource is that one identifier (or the ARN
    // built on it), so a client takes any of them straight to the owning service.
    let outputs = outputs_of(&described);
    for (key, expected) in [
        ("KnowledgeBaseRef", id.clone()),
        ("KnowledgeBaseId", id.clone()),
        ("KnowledgeBaseArn", knowledge_base_arn(&id)),
    ] {
        assert_eq!(
            outputs.get(key),
            Some(&expected),
            "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
            outputs.get(key)
        );
    }

    // ...and Amazon Bedrock Agent resolves it to the knowledge base the template declared.
    let (status, knowledge_base) = get_knowledge_base(&router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetKnowledgeBase on the id the stack \
         published, got {status} with {knowledge_base}"
    );
    for (member, expected) in [
        ("knowledgeBaseId", id.clone()),
        ("knowledgeBaseArn", knowledge_base_arn(&id)),
        ("name", "kbcfn-router-escalation".to_string()),
        ("description", "the escalation knowledge base".to_string()),
        ("status", "ACTIVE".to_string()),
    ] {
        let answered = knowledge_base.get(member).and_then(Value::as_str);
        assert_eq!(
            answered,
            Some(expected.as_str()),
            "case {case:?}: expected the knowledge base's {member:?} to be {expected:?}, got \
             {answered:?} in {knowledge_base}"
        );
    }
    let bound = knowledge_base
        .pointer("/storageConfiguration/s3VectorsConfiguration/indexArn")
        .and_then(Value::as_str);
    let index_arn =
        format!("arn:aws:s3vectors:{REGION}:{ACCOUNT}:bucket/kbcfn-router-vectors/index/kbcfn-router-embeddings");
    assert_eq!(
        bound,
        Some(index_arn.as_str()),
        "case {case:?}: expected the knowledge base to be bound to the index the stack \
         created, got {bound:?} in {knowledge_base}"
    );
}

/// The other verdict: the owning service REFUSES a knowledge base whose embedding model's
/// vectors the declared index cannot hold, so the stack fails instead of publishing an
/// identifier for a knowledge base nothing created.
#[tokio::test]
async fn a_knowledge_base_the_owning_service_refuses_fails_the_stack() {
    let case = "an embedding model whose vectors the declared index cannot hold";
    let router = build_router();
    let stack = "kbcfn-router-refused";
    // The index holds 1536-value vectors; the knowledge base reads a model that produces
    // 1024.
    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(TITAN_V2, TITAN_G1_DIMENSION)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );

    tokio::time::sleep(PAST_THE_SPANS).await;
    let (status, described) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {described}"
    );
    let reported = opt_element(&described, "StackStatus");
    assert_eq!(
        reported.as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the refused create to be rolled back, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Escalation"),
        "case {case:?}: expected the failure to name the resource the owning service \
         refused, got {reason:?}"
    );
}
