// SPDX-License-Identifier: CC0-1.0

//! How the ARN-addressed TAGGING surface refuses an ARN that names no resource.
//!
//! The rule under test: on `TagResource`, `ListTagsForResource` and `UntagResource` —
//! the operations that address a resource by `resourceArn` — an ARN this service holds
//! no resource for is refused with HTTP **400** `ResourceNotFoundException`, a body
//! whose `message` is one fixed, resource-agnostic string, and a `resourceArn` member
//! echoing the request's ARN verbatim. That BODY is not the one an operation addressing
//! an environment by `environmentId` answers: that one keeps the modelled shape
//! (`resourceId` + `resourceType`, and a message naming the requested id), on the same
//! 400 — which the last test here pins.
//!
//! Every test drives the service the way a client does — `POST /` with
//! `X-Amz-Target: AmazonElasticVMwareService.<Operation>` through the same
//! [`MultiServiceRouter`] the production router mounts — so what it asserts is the wire
//! behaviour.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test client signs for, so the ARNs under test carry it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The fixed, resource-agnostic text the tagging surface answers a missing resource
/// with (`crate::scaffolded::TAGGING_RESOURCE_NOT_FOUND_MESSAGE`). The ARN travels in
/// the `resourceArn` member instead, never in this string.
const NOT_FOUND_MESSAGE: &str = "Resource cannot be found.";

/// The VPC and the subnet inside it that the creates below name. `CreateEnvironment`
/// checks the network a request names against the account's own Amazon EC2 store, so
/// [`emulator`] registers these there.
const VPC_ID: &str = "vpc-0123456789abcdef0";
/// See [`VPC_ID`].
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// The emulator a test drives: one shared engine, this service mounted on it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise behaviour, not signing: the credentials below are unsigned.
    router.set_request_authentication(false);
    crate::environment_lifecycle::register_network(&engine, VPC_ID, SUBNET_ID);
    // An environment is created only for an account holding a Premium Support
    // subscription (`tests/all/premium_support_entitlement.rs` owns that rule).
    crate::premium_support_entitlement::grant_premium_support(&engine);
    router.register_service(
        service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("https://evs.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AmazonElasticVMwareService.{operation}"),
        )
        // The SigV4 credential scope is what routes the request to `evs` and what
        // names the Region the service builds ARNs in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/evs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(::bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Ten non-overlapping VLAN subnet CIDR blocks, one per `InitialVlans` member.
fn vlans() -> Value {
    let cidr = |index: u8| json!({ "cidr": format!("10.0.{}.0/28", index * 2) });
    json!({
        "vmkManagement": cidr(0),
        "vmManagement": cidr(1),
        "vMotion": cidr(2),
        "vSan": cidr(3),
        "vTep": cidr(4),
        "edgeVTep": cidr(5),
        "nsxUplink": cidr(6),
        "hcx": cidr(7),
        "expansionVlan1": cidr(8),
        "expansionVlan2": cidr(9),
    })
}

/// Create one environment carrying `tags` and return its ARN.
async fn create_tagged(router: &MultiServiceRouter, name: &str, tags: Value) -> String {
    let request = json!({
        "environmentName": name,
        "vpcId": VPC_ID,
        "serviceAccessSubnetId": SUBNET_ID,
        "vcfVersion": "VCF-5.2.1",
        "termsAccepted": true,
        "initialVlans": vlans(),
        "tags": tags,
    });
    let (status, body) = call(router, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "create {name:?}: expected 200, got {status} {body}"
    );
    body["environment"]["environmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name:?}: no environmentArn in {body}"))
        .to_string()
}

/// The `__type` an error body carries, for an assertion message.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or("<none>").to_string()
}

/// An environment ARN for `environment_id` in `region`, as the modelled `Arn` pattern
/// `^arn:aws:evs:[a-z]{2}-[a-z]+-[0-9]:[0-9]{12}:environment/[a-zA-Z0-9_-]+$` spells it.
fn arn(region: &str, account: &str, environment_id: &str) -> String {
    format!("arn:aws:evs:{region}:{account}:environment/{environment_id}")
}

/// The three ARN-addressed tagging operations, each with the rest of its request body —
/// the operations the model binds `resourceArn` to
/// (`smithy_query(query="operation",operation="TagResource,ListTagsForResource,UntagResource")`).
fn tagging_operations() -> Vec<(&'static str, Value)> {
    vec![
        ("TagResource", json!({ "tags": { "team": "platform" } })),
        ("ListTagsForResource", json!({})),
        ("UntagResource", json!({ "tagKeys": ["team"] })),
    ]
}

/// One tagging request: the operation, the ARN it names, and the rest of its body.
fn tagging_request(resource_arn: &str, extra: &Value) -> Value {
    let mut request = json!({ "resourceArn": resource_arn });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            request[name] = value.clone();
        }
    }
    request
}

#[tokio::test]
async fn every_tagging_operation_refuses_a_missing_arn_with_400_and_echoes_it() {
    let router = emulator();
    // One real, tagged environment, so a miss is a miss on the lookup and not an
    // empty store — and so the same operations can be shown to succeed below.
    let present = create_tagged(&router, "present", json!({ "team": "platform" })).await;
    let present_id = present
        .rsplit('/')
        .next()
        .expect("the ARN names an environment")
        .to_string();

    // Each case: why the ARN names no resource this service holds. Every one of them
    // satisfies the modelled `Arn` pattern, so the boundary validator passes it and the
    // refusal under test is the LOOKUP's, not a constraint violation.
    let missing: Vec<(&str, String)> = vec![
        (
            "an environment id this service never minted",
            arn(REGION, ACCOUNT, "env-0000000000"),
        ),
        (
            "a real environment's id, at another Region",
            arn("eu-central-1", ACCOUNT, &present_id),
        ),
        (
            "a real environment's id, under another account",
            arn(REGION, "123456789012", &present_id),
        ),
        (
            "a real environment's id with one character appended",
            arn(REGION, ACCOUNT, &format!("{present_id}0")),
        ),
        (
            "an id of the shortest form the pattern allows",
            arn(REGION, ACCOUNT, "e"),
        ),
        (
            "an id carrying the underscores and dashes the pattern allows",
            arn(REGION, ACCOUNT, "Env_0-ABC_z"),
        ),
    ];

    for (operation, extra) in tagging_operations() {
        for (why, resource_arn) in &missing {
            let case = format!("{operation} with {why}");
            let request = tagging_request(resource_arn, &extra);
            let (status, body) = call(&router, operation, &request).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {}",
                error_type(&body)
            );
            assert_eq!(
                body["message"],
                json!(NOT_FOUND_MESSAGE),
                "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {}",
                body["message"]
            );
            assert_eq!(
                body["resourceArn"],
                json!(resource_arn),
                "case {case:?}: expected resourceArn {resource_arn:?}, got {}",
                body["resourceArn"]
            );
            // The message is resource-agnostic by construction: the ARN appears in the
            // `resourceArn` member and nowhere else.
            assert!(
                !body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(resource_arn.as_str()),
                "case {case:?}: the message must not carry the ARN, got {}",
                body["message"]
            );
            // The modelled `ResourceNotFoundException` members belong to the
            // environmentId-addressed lookup error, not to this one.
            assert!(
                body["resourceId"].is_null() && body["resourceType"].is_null(),
                "case {case:?}: the tagging error carries no resourceId/resourceType, got {body}"
            );
        }
    }

    // The refusals above are the lookup's answer, not a blanket one: the SAME three
    // operations on the ARN that does exist all succeed, and nothing the refused calls
    // named changed the stored tags.
    for (operation, extra) in tagging_operations() {
        let (status, body) = call(&router, operation, &tagging_request(&present, &extra)).await;
        assert_eq!(
            status, 200,
            "case {operation:?} on the present ARN: expected 200, got {status} {body}"
        );
    }
    // `UntagResource` above removed `team`, and nothing else was ever applied: the
    // refused `TagResource` calls must have left no tag behind.
    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({ "resourceArn": present }),
    )
    .await;
    assert_eq!(status, 200, "reading the tags back: got {status} {body}");
    assert!(
        body["tags"].as_object().is_none_or(|tags| tags.is_empty()),
        "a refused TagResource must not write a tag, got {}",
        body["tags"]
    );
}

#[tokio::test]
async fn an_environment_id_lookup_answers_the_modelled_body_on_the_same_400() {
    // The BODY rule is scoped to the ARN-addressed tagging surface. An operation that
    // resolves an environment BY ID answers the modelled `ResourceNotFoundException`
    // with its `resourceId` / `resourceType` members
    // (`smithy_query(query="shape",shape="ResourceNotFoundException")`) and a message
    // naming the requested id. The STATUS is the same 400 for both surfaces: every
    // modeled client error of this awsJson1_0 service goes on the wire as a 400,
    // whatever exact code its `@httpError` trait names (see
    // `tests/all/error_wire_status.rs`).
    let router = emulator();
    let absent = "env-0000000000";

    for operation in ["GetEnvironment", "DeleteEnvironment", "ListEnvironmentVlans"] {
        let (status, body) = call(&router, operation, &json!({ "environmentId": absent })).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {operation:?}: expected a ResourceNotFoundException, got {}",
            error_type(&body)
        );
        assert_eq!(
            body["message"],
            json!(format!("Environment {absent} was not found")),
            "case {operation:?}: the message must name the requested id, got {}",
            body["message"]
        );
        assert_eq!(
            body["resourceId"],
            json!(absent),
            "case {operation:?}: expected resourceId {absent:?}, got {}",
            body["resourceId"]
        );
        assert_eq!(
            body["resourceType"],
            json!("environment"),
            "case {operation:?}: expected resourceType \"environment\", got {}",
            body["resourceType"]
        );
    }
}
