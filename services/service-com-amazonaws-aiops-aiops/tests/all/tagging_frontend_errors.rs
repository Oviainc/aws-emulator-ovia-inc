// SPDX-License-Identifier: CC0-1.0

//! Which error SHAPE each of this service's two surfaces reports an absent resource
//! with, over the whole set of identifiers that resolve nothing.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization (including the `@httpLabel` `resourceArn` and the
//! `@httpQuery` `tagKeys` list), the modelled-constraint walk and the handlers — so
//! what is asserted is what a signed AWS SDK request observes, header included.
//!
//! The rule under test: the operations bound directly on the service — the shared
//! TAGGING surface, `ListTagsForResource`, `TagResource` and `UntagResource`, the only
//! three in the service shape's own `operations` list
//! (`smithy_query(query="digest")`) — report a `resourceArn` that resolves to no
//! resource as HTTP 404 with the tagging frontend's own error code
//! `NotFoundException`, in the `x-amzn-errortype` header a restJson1 client resolves
//! the error shape from AND in the body's `__type`, while the operations bound on the
//! `InvestigationGroup` resource keep this service's modelled
//! `ResourceNotFoundException` (404) for the same condition. Both messages carry
//! nothing the request sent.
//!
//! The split is the tagging frontend's, not this operation's: the same frontend
//! already answers an oversized tag set with its own `BadRequestException` rather than
//! this service's modelled `ValidationException`
//! (`state::tag_capacity_rejection`), and neither shape is in this service's model.
//!
//! Rows differ along the operation, along the FORM of the unresolvable identifier
//! (every form the modelled `InvestigationGroupIdentifier` pattern admits, and each
//! single ARN field changed in turn), along the request body or query a tagging call
//! carries, and along whether the resource never existed or was deleted.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state::TAGGING_RESOURCE_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// An account other than the emulator's own.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The error code the TAGGING surface reports an absent resource with.
const TAGGING_NOT_FOUND: &str = "NotFoundException";

/// The error code the RESOURCE surface reports an absent resource with: this service's
/// modelled shape (`smithy_query(query="shape",shape="ResourceNotFoundException")`:
/// `@error client`, `@httpError 404`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in [`REGION`], the signal the router routes on.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// What one call answered: the HTTP status, the `x-amzn-errortype` header (empty when
/// the response carries none), the body's `__type` and the body's `message`.
#[derive(Debug)]
struct Answer {
    status: u16,
    header_code: String,
    body_code: String,
    message: String,
    body: Value,
}

/// Dispatch one request and report it. The header and the body code are kept APART:
/// restJson1 resolves the error shape from the header, so a body that names one shape
/// and a header that names another is exactly the defect under test.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        // The header may carry a namespace (`com.amazonaws.aiops#Foo`) or a trailing
        // locator (`Foo:http://…`); neither is part of the code.
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    let body_code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        header_code,
        body_code,
        message,
        body,
    }
}

/// Percent-encode everything a URI component may not carry literally — what an SDK
/// does for an `@httpLabel` path segment and for an `@httpQuery` value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create a group that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN, "tags": { "Team": "ops" } })),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "creating {name:?}: expected 201, got {answer:?}"
    );
    answer
        .body
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("creating {name:?}: response carried no arn, got {answer:?}"))
        .to_string()
}

/// One request against the tagging surface, as `(operation, method, uri, body)`, for
/// the `resourceArn` `identifier`.
///
/// The rows differ along the request each operation carries too: a read with no body
/// or query, a write whose `tags` map the call would store, and a removal naming
/// several `tagKeys` — the modelled `@httpQuery` list, whose length the model bounds at
/// 50 (`smithy_query(query="operation",operation="UntagResource")`).
fn tagging_calls(identifier: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let label = encode(identifier);
    vec![
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{label}"),
            None,
        ),
        (
            "TagResource",
            "POST",
            format!("/tags/{label}"),
            Some(json!({ "tags": { "Team": "ops" } })),
        ),
        (
            "TagResource with several tags",
            "POST",
            format!("/tags/{label}"),
            Some(json!({ "tags": { "Team": "ops", "Environment": "prod" } })),
        ),
        (
            "TagResource with an empty tags map",
            "POST",
            format!("/tags/{label}"),
            Some(json!({ "tags": {} })),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{label}?tagKeys=Team"),
            None,
        ),
        (
            "UntagResource with several keys",
            "DELETE",
            format!("/tags/{label}?tagKeys=Team&tagKeys=Environment&tagKeys=NeverSet"),
            None,
        ),
    ]
}

/// One request against the resource surface, for the same `identifier`.
fn resource_calls(identifier: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let label = encode(identifier);
    vec![
        (
            "GetInvestigationGroup",
            "GET",
            format!("/investigationGroups/{label}"),
            None,
        ),
        (
            "UpdateInvestigationGroup",
            "PATCH",
            format!("/investigationGroups/{label}"),
            Some(json!({ "isCloudTrailEventHistoryEnabled": false })),
        ),
        (
            "DeleteInvestigationGroup",
            "DELETE",
            format!("/investigationGroups/{label}"),
            None,
        ),
        (
            "GetInvestigationGroupPolicy",
            "GET",
            format!("/investigationGroups/{label}/policy"),
            None,
        ),
    ]
}

/// Identifiers that resolve no investigation group, every one of them inside the
/// modelled `InvestigationGroupIdentifier` pattern so the boundary walk admits it and
/// the handler is what answers: `(case, identifier)`.
///
/// The pattern is `(?:[\-_A-Za-z0-9]{1,512}|arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):aiops:[a-zA-Z0-9-]*:[0-9]{12}:investigation-group/[A-Za-z0-9]{16})`
/// — `smithy_query(query="shape",shape="InvestigationGroupIdentifier")`. The ARN rows
/// change ONE field each, so what fails to resolve is that field and nothing else.
fn unresolvable(id: &str) -> Vec<(String, String)> {
    let arn = |region: &str, account: &str, partition: &str, id: &str| {
        format!("arn:{partition}:aiops:{region}:{account}:investigation-group/{id}")
    };
    vec![
        ("a bare name".to_string(), "no-such-group".to_string()),
        ("a bare id".to_string(), "A".repeat(16)),
        (
            "a one-character label, the pattern's shortest".to_string(),
            "x".to_string(),
        ),
        (
            "a 512-character label, the pattern's longest".to_string(),
            "z".repeat(512),
        ),
        (
            "an ARN with an id no group carries".to_string(),
            arn(REGION, ACCOUNT, "aws", &"B".repeat(16)),
        ),
        (
            "the ARN of this group in another region".to_string(),
            arn("eu-west-1", ACCOUNT, "aws", id),
        ),
        (
            "the ARN of this group in another account".to_string(),
            arn(REGION, FOREIGN_ACCOUNT, "aws", id),
        ),
        (
            "the ARN of this group in another partition".to_string(),
            arn(REGION, ACCOUNT, "aws-cn", id),
        ),
        (
            "an ARN in a GovCloud partition and region".to_string(),
            arn("us-gov-west-1", ACCOUNT, "aws-us-gov", &"C".repeat(16)),
        ),
    ]
}

/// Whether "the message does not echo this identifier" is a meaningful assertion: a
/// one- or two-character label occurs inside ordinary English words, so finding it in
/// a message is no evidence the message echoed the request.
fn echoable(identifier: &str) -> bool {
    identifier.chars().count() > 4
}

/// The TAGGING surface reports an unresolvable `resourceArn` as a 404
/// `NotFoundException` — header and body alike — for every operation and every
/// identifier form, and the answer carries nothing of the request.
#[tokio::test]
async fn the_tagging_surface_reports_an_absent_resource_as_not_found_exception() {
    let router = emulator();
    let arn = create_ok(&router, "tagging-error-probe").await;
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string();

    for (form, identifier) in unresolvable(&id) {
        for (operation, method, uri, body) in tagging_calls(&identifier) {
            let case = format!("{operation} addressed by {form}");
            let answer = call(&router, method, &uri, body.as_ref()).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {answer:?}"
            );
            // restJson1 resolves the error SHAPE from the header, so this is the
            // assertion a client's not-found path actually depends on.
            assert_eq!(
                answer.header_code, TAGGING_NOT_FOUND,
                "case {case:?}: expected x-amzn-errortype {TAGGING_NOT_FOUND}, got {answer:?}"
            );
            assert_eq!(
                answer.body_code, TAGGING_NOT_FOUND,
                "case {case:?}: expected __type {TAGGING_NOT_FOUND}, got {answer:?}"
            );
            assert_eq!(
                answer.message, TAGGING_RESOURCE_NOT_FOUND,
                "case {case:?}: expected {TAGGING_RESOURCE_NOT_FOUND:?}, got {answer:?}"
            );
            assert!(
                !echoable(&identifier) || !answer.message.contains(&identifier),
                "case {case:?}: the answer must not echo the identifier {identifier:?}, \
                 got {answer:?}"
            );
        }
    }

    // The ARN still resolves, so every 404 above is about the identifier and not about
    // the group having gone missing — and no refused call touched the stored set.
    let answer = call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
    assert_eq!(
        answer.status, 200,
        "the ARN itself must still resolve, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("tags"),
        Some(&json!({ "Team": "ops" })),
        "a refused tagging call must leave the stored set alone, got {answer:?}"
    );
}

/// The two surfaces are DISTINGUISHABLE: for the same unresolvable identifier, the
/// resource operations answer this service's modelled `ResourceNotFoundException` while
/// the tagging operations answer the frontend's `NotFoundException`, so a client reads
/// one code per surface rather than one code for both.
#[tokio::test]
async fn the_resource_surface_keeps_the_modelled_resource_not_found_exception() {
    let router = emulator();
    let arn = create_ok(&router, "resource-error-probe").await;
    let id = arn
        .rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string();
    assert_ne!(
        TAGGING_NOT_FOUND, RESOURCE_NOT_FOUND,
        "the two surfaces must report two distinguishable codes"
    );

    for (form, identifier) in unresolvable(&id) {
        for (operation, method, uri, body) in resource_calls(&identifier) {
            let case = format!("{operation} addressed by {form}");
            let answer = call(&router, method, &uri, body.as_ref()).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected 404, got {answer:?}"
            );
            assert_eq!(
                answer.header_code, RESOURCE_NOT_FOUND,
                "case {case:?}: expected x-amzn-errortype {RESOURCE_NOT_FOUND}, got {answer:?}"
            );
            assert_eq!(
                answer.body_code, RESOURCE_NOT_FOUND,
                "case {case:?}: expected __type {RESOURCE_NOT_FOUND}, got {answer:?}"
            );
            assert!(
                !echoable(&identifier) || !answer.message.contains(&identifier),
                "case {case:?}: the answer must not echo the identifier {identifier:?}, \
                 got {answer:?}"
            );
        }
    }
}

/// Which code a tagging call answers with is read off the STORE, not wired to the
/// request: the same ARN that succeeds while the group exists is the frontend's 404
/// once the group is deleted, on all three operations.
#[tokio::test]
async fn a_deleted_resource_turns_a_succeeding_tagging_call_into_the_frontend_404() {
    let router = emulator();
    let arn = create_ok(&router, "deleted-while-tagging").await;

    for (operation, method, uri, body) in tagging_calls(&arn) {
        let answer = call(&router, method, &uri, body.as_ref()).await;
        assert_eq!(
            answer.status, 200,
            "case {operation:?} while the group exists: expected 200, got {answer:?}"
        );
    }

    let answer = call(
        &router,
        "DELETE",
        &format!("/investigationGroups/{}", encode(&arn)),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "deleting the group: expected 200, got {answer:?}"
    );

    for (operation, method, uri, body) in tagging_calls(&arn) {
        let case = format!("{operation} after the group is deleted");
        let answer = call(&router, method, &uri, body.as_ref()).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.header_code, TAGGING_NOT_FOUND,
            "case {case:?}: expected x-amzn-errortype {TAGGING_NOT_FOUND}, got {answer:?}"
        );
        assert_eq!(
            answer.body_code, TAGGING_NOT_FOUND,
            "case {case:?}: expected __type {TAGGING_NOT_FOUND}, got {answer:?}"
        );
    }
}
