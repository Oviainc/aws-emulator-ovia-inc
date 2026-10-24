// SPDX-License-Identifier: CC0-1.0

//! m2's PRE-HANDLER refusals, driven over the wire.
//!
//! Three rules, each about the whole service rather than about one operation, and each
//! checked as a TABLE whose rows differ along what the rule generalises over:
//!
//! 1. **A caller-supplied `resourceArn` that is not an ARN is a bad request.** m2
//!    parses the resource ARN in its ARN/tagging layer, ahead of the modelled
//!    constraint walk, so a value that is not six ARN fields is refused with HTTP 400
//!    and the error code [`BAD_REQUEST`] — not with the modelled validation error.
//!    Recorded behaviour (corpus `simparity-wire-aws-v1`, fixture
//!    `m2-environment-not-found-errors`, step `task#4` `ListTagsForResource`:
//!    `GET /tags/not-a-valid-arn` -> 400, `x-amzn-errortype: BadRequestException`,
//!    message [`BAD_SYNTAX_MESSAGE`]). The rule is about the member, not about one
//!    operation, so every operation that binds `resourceArn` is a row — the three the
//!    model declares (`smithy_query(query="operation",operation="ListTagsForResource,
//!    TagResource,UntagResource")`: `GET`/`POST`/`DELETE /tags/{resourceArn}`).
//! 2. **A value that IS an ARN but falls outside the modelled `Arn` pattern keeps the
//!    modelled validation refusal**, which names the member and the constraint it
//!    failed, and so does every other `Arn`-typed member (`CreateApplication`'s
//!    `roleArn`): the ARN layer parsed those fine, so they are constraint violations
//!    rather than bad syntax.
//! 3. **A caller the service will not authorize is denied with the shape the model
//!    declares for it**: `AccessDeniedException`, HTTP 403, carrying its required
//!    `message` member — one of this service's common errors
//!    (`smithy_query(query="service_info")` -> `common_errors`:
//!    `AccessDeniedException`/`InternalServerException`/`ThrottlingException`), so
//!    every operation declares it. The framework's own wording (`AccessDenied`) is a
//!    code no m2 client branches on.
//!
//! Every assertion goes over the same front door a client uses:
//! [`MultiServiceRouter::dispatch`] with a buffered restJson1 request, so the URI
//! bindings, the refusal envelope and the `x-amzn-errortype` header are all exercised.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_m2_awssupernovacontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for.
const REGION: &str = "eu-west-1";

/// The error code m2's ARN layer refuses a value that is not an ARN with. Written out
/// here rather than read from the service's own constant so the value itself is under
/// test.
const BAD_REQUEST: &str = "BadRequestException";

/// What that refusal says, recorded with the code above.
const BAD_SYNTAX_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The error code the framework names a modelled-constraint refusal with on this
/// service's restJson1 envelope — what every refusal rule 1 does NOT claim keeps.
const VALIDATION: &str = "ValidationException";

/// The code and status m2 denies an unauthorized caller with: the modelled
/// `AccessDeniedException`, `@httpError(403)`.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// A SigV4 `Authorization` header naming m2 in [`REGION`]: it is what routes the
/// request to this service. Signatures are not checked unless a test turns
/// authentication on, so the signature bytes are filler.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTM2EXAMPLE/20260101/eu-west-1/m2/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One answer off the wire: the status, the `x-amzn-errortype` header restJson1
/// resolves an error shape from, and the parsed body.
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error code the body carries (`__type`), which must agree with the header.
    fn body_type(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
    }

    /// The `message` member, or a marker when the body carries none.
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
    }
}

/// A router serving ONE m2 service over ONE shared engine — the assembly
/// `core/router` performs, minus the other services. `authenticated` turns request
/// authentication on, which is what rule 3 needs.
fn router(authenticated: bool) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request. `authorization` is the header value to send, if any — absent
/// is itself a case for rule 3, and the request still reaches m2 because the `Host`
/// header names its endpoint.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
    authorization: Option<&str>,
) -> Answer {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://m2.{REGION}.amazonaws.com{path_and_query}"))
        .header("host", format!("m2.{REGION}.amazonaws.com"));
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let bytes = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("a test body always serializes"))
    });
    let response = router
        .dispatch(request.body(bytes).expect("a valid test request"))
        .await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("<no x-amzn-errortype>")
        .to_string();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body: parsed,
    }
}

/// The three operations the model binds `resourceArn` on, as (label, method, how the
/// request carries the rest of the input). Source:
/// `smithy_query(query="operation",operation="ListTagsForResource,TagResource,
/// UntagResource")` — `GET`, `POST` (body `tags`) and `DELETE` (query `tagKeys`) on
/// `/tags/{resourceArn}`.
struct TagOperation {
    label: &'static str,
    method: &'static str,
    query: &'static str,
    body: Option<fn() -> Value>,
}

fn tag_operations() -> [TagOperation; 3] {
    [
        TagOperation {
            label: "ListTagsForResource",
            method: "GET",
            query: "",
            body: None,
        },
        TagOperation {
            label: "TagResource",
            method: "POST",
            query: "",
            body: Some(|| json!({"tags": {"env": "dev"}})),
        },
        TagOperation {
            label: "UntagResource",
            method: "DELETE",
            query: "?tagKeys=env",
            body: None,
        },
    ]
}

/// Rule 1: a `resourceArn` that is not an ARN at all is refused by the ARN layer as a
/// bad request — on every operation that takes one, for every way a value can fail to
/// be an ARN.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_a_bad_request() {
    // (case, the resourceArn as it travels in the URI). The forms a value can fail
    // the ARN parse by: not an ARN at all, the wrong first field, too few fields, an
    // empty partition, an empty vendor, an empty resource. `%2F` is a slash inside the
    // label, which an SDK percent-encodes.
    let values: &[(&str, &str)] = &[
        ("the recorded value", "not-a-valid-arn"),
        ("a single character", "-"),
        ("the first field alone", "arn"),
        ("an ARN that stops before its region", "arn:aws:m2"),
        (
            "an ARN that stops before its resource",
            "arn:aws:m2:eu-west-1:000000000000",
        ),
        (
            "a first field that is not arn",
            "urn:aws:m2:eu-west-1:000000000000:app%2Fa1b2c3",
        ),
        (
            "an empty partition",
            "arn::m2:eu-west-1:000000000000:app%2Fa1b2c3",
        ),
        (
            "an empty vendor",
            "arn:aws::eu-west-1:000000000000:app%2Fa1b2c3",
        ),
        (
            "an empty resource",
            "arn:aws:m2:eu-west-1:000000000000:",
        ),
        ("an ARN-looking prefix only", "arn:aws:m2:eu-west-1"),
    ];

    let router = router(false);
    for operation in tag_operations() {
        for (case, value) in values {
            let path = format!("/tags/{value}{}", operation.query);
            let answer = call(
                &router,
                operation.method,
                &path,
                operation.body.map(|build| build()),
                Some(AUTHORIZATION),
            )
            .await;

            assert_eq!(
                answer.status, 400,
                "case {case:?} on {}: expected HTTP 400, got {} {:?}",
                operation.label, answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, BAD_REQUEST,
                "case {case:?} on {}: expected x-amzn-errortype {BAD_REQUEST}, got {:?}",
                operation.label, answer.error_type
            );
            assert_eq!(
                answer.body_type(),
                BAD_REQUEST,
                "case {case:?} on {}: expected __type {BAD_REQUEST}, got {:?}",
                operation.label,
                answer.body_type()
            );
            assert_eq!(
                answer.message(),
                BAD_SYNTAX_MESSAGE,
                "case {case:?} on {}: expected message {BAD_SYNTAX_MESSAGE:?}, got {:?}",
                operation.label,
                answer.message()
            );
        }
    }
}

/// Rule 2: a value the ARN layer parses fine, but the model's `Arn` pattern rules out,
/// keeps the modelled validation refusal — and so does every other `Arn`-typed member.
#[tokio::test]
async fn an_arn_outside_the_modelled_pattern_keeps_the_validation_refusal() {
    // (case, the resourceArn as it travels in the URI). Each is six ARN fields with a
    // non-empty partition, vendor and resource — so it IS a well-formed ARN —
    // but outside the modelled `Arn` pattern
    // `^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):…:[0-9]{12}:…$`.
    let values: &[(&str, &str)] = &[
        (
            "a partition the pattern does not list",
            "arn:aws-example:m2:eu-west-1:000000000000:app%2Fa1b2c3",
        ),
        (
            "an account id shorter than 12 digits",
            "arn:aws:m2:eu-west-1:12345:app%2Fa1b2c3",
        ),
        (
            "a region segment that is not shaped like a region",
            "arn:aws:m2:not_a_region:000000000000:app%2Fa1b2c3",
        ),
    ];

    let router = router(false);
    for operation in tag_operations() {
        for (case, value) in values {
            let path = format!("/tags/{value}{}", operation.query);
            let answer = call(
                &router,
                operation.method,
                &path,
                operation.body.map(|build| build()),
                Some(AUTHORIZATION),
            )
            .await;

            assert_eq!(
                answer.status, 400,
                "case {case:?} on {}: expected HTTP 400, got {} {:?}",
                operation.label, answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, VALIDATION,
                "case {case:?} on {}: expected x-amzn-errortype {VALIDATION}, got {:?}",
                operation.label, answer.error_type
            );
            assert!(
                answer.message().contains("failed to satisfy constraint"),
                "case {case:?} on {}: expected the constraint clause, got {:?}",
                operation.label,
                answer.message()
            );
        }
    }

    // The rule is scoped to the resource the call ADDRESSES: an `Arn`-typed member
    // that is an attribute of the resource being written (`CreateApplication`'s
    // `roleArn`) is refused the same modelled way, whatever its syntax.
    for (case, role_arn) in [
        ("a roleArn that is not an ARN", "not-an-arn"),
        (
            "a roleArn with an account id shorter than 12 digits",
            "arn:aws:iam::1:role/m2",
        ),
    ] {
        let answer = call(
            &router,
            "POST",
            "/applications",
            Some(json!({
                "name": "RoleArnCase",
                "engineType": "bluage",
                "definition": {"content": "{\"template-version\":\"2.0\"}"},
                "roleArn": role_arn,
            })),
            Some(AUTHORIZATION),
        )
        .await;

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, VALIDATION,
            "case {case:?}: expected x-amzn-errortype {VALIDATION}, got {:?}",
            answer.error_type
        );
        assert!(
            answer.message().contains("roleArn"),
            "case {case:?}: expected the message to name roleArn, got {:?}",
            answer.message()
        );
    }
}

/// One request a rule is checked on: which operation it names and what it carries.
struct Call {
    label: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
}

/// Rule 3: a caller request authentication refuses is denied with the modelled
/// `AccessDeniedException` — 403, the error code a client branches on, and the shape's
/// required `message` member — for every way the refusal can arise, and whatever
/// operation was called.
#[tokio::test]
async fn an_unauthorized_caller_is_denied_with_the_modelled_shape() {
    // (case, the Authorization header the request carries)
    let callers: &[(&str, Option<&str>)] = &[
        ("no credentials at all", None),
        ("a signature that does not match", Some(AUTHORIZATION)),
        (
            "a scheme no authenticator recognizes",
            Some("Bearer not-a-sigv4-credential"),
        ),
    ];
    // One read and one write, so the denial is shown to precede the handler whatever
    // the operation does.
    let operations: &[Call] = &[
        Call {
            label: "GetSignedBluinsightsUrl",
            method: "GET",
            path: "/signed-bi-url",
            body: None,
        },
        Call {
            label: "ListApplications",
            method: "GET",
            path: "/applications",
            body: None,
        },
        Call {
            label: "CreateApplication",
            method: "POST",
            path: "/applications",
            body: Some(|| {
                json!({
                    "name": "DeniedApp",
                    "engineType": "bluage",
                    "definition": {"content": "{\"template-version\":\"2.0\"}"},
                })
            }),
        },
    ];

    let router = router(true);
    for (case, authorization) in callers {
        for operation in operations {
            let Call {
                label,
                method,
                path,
                body,
            } = operation;
            let answer = call(
                &router,
                method,
                path,
                body.map(|build| build()),
                *authorization,
            )
            .await;

            assert_eq!(
                answer.status, 403,
                "case {case:?} on {label}: expected HTTP 403, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, ACCESS_DENIED,
                "case {case:?} on {label}: expected x-amzn-errortype {ACCESS_DENIED}, got {:?}",
                answer.error_type
            );
            assert_eq!(
                answer.body_type(),
                ACCESS_DENIED,
                "case {case:?} on {label}: expected __type {ACCESS_DENIED}, got {:?}",
                answer.body_type()
            );
            assert!(
                !answer.message().is_empty() && answer.message() != "<no message>",
                "case {case:?} on {label}: AccessDeniedException requires a message, got {:?}",
                answer.body
            );
        }
    }
}

/// A denied write leaves no resource behind: the refusal is pre-handler, so the
/// application the denied `CreateApplication` named is in no listing afterwards.
///
/// Both routers serve the SAME engine — authentication is a property of the router, so
/// this is how the state a denied call could have written is read back.
#[tokio::test]
async fn a_denied_create_persists_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut authenticated = MultiServiceRouter::new();
    authenticated.set_request_authentication(true);
    authenticated.register_service(EmulatedService::new(engine.clone()));
    let mut open = MultiServiceRouter::new();
    open.set_request_authentication(false);
    open.register_service(EmulatedService::new(engine));

    let denied = call(
        &authenticated,
        "POST",
        "/applications",
        Some(json!({
            "name": "NeverCreated",
            "engineType": "bluage",
            "definition": {"content": "{\"template-version\":\"2.0\"}"},
        })),
        Some(AUTHORIZATION),
    )
    .await;
    assert_eq!(
        denied.status, 403,
        "expected the create to be denied, got {} {:?}",
        denied.status, denied.body
    );

    let listing = call(&open, "GET", "/applications", None, Some(AUTHORIZATION)).await;
    assert_eq!(
        listing.status, 200,
        "expected the listing to be served, got {} {:?}",
        listing.status, listing.body
    );
    let applications = listing
        .body
        .get("applications")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        applications, 0,
        "a denied create must persist nothing, got {:?}",
        listing.body
    );
}
