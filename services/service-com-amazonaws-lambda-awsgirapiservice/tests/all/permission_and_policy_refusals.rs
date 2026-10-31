// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how the permission and policy surface REFUSES a call, and for what
//! a policy read states beside the document.
//!
//! The rules under test, each asserted over a set of inputs rather than on one recorded
//! call:
//!
//! * a request whose BODY is past the quota published for the operation it addresses is
//!   refused `413`, naming that quota and that operation — which is a different condition,
//!   and a different status, from a grant that would push the stored POLICY past its own
//!   quota, refused `400` and naming the size the policy would have reached and the limit it
//!   may not pass;
//! * a read of a function's policy states whether the policy it reports grants public
//!   access, computed from that document: a grant to a wildcard principal that no condition
//!   narrows is public access, and nothing else is;
//! * the resource-policy operations' revision precondition is answered with its message
//!   alone, while the same precondition on the permission operations names the class of its
//!   fault — and every other refusal of this surface names it;
//! * a read of a resource's policy that finds none states one sentence, whatever it was
//!   that it could not find;
//! * a document a caller puts is reported in the rendering this service stores it in, so
//!   two callers that submit the same policy written differently read the same text back.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction`, `AddPermission` and `GetPolicy` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the resource-policy operations are bound
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`).
const RESOURCE_POLICY_URI: &str = "/2026-07-09/resource-policy";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and what region the ARNs carry. Nothing in it is
/// verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The request-body quota AWS publishes for `AddPermission`, in bytes.
const ADD_PERMISSION_REQUEST_QUOTA: usize = 5_120;

/// The published quota on a resource-based policy, in bytes.
const MAX_POLICY_SIZE_BYTES: usize = 20_480;

/// The class every refusal of this surface names, except the ones recorded without it.
const USER_FAULT: &str = "User";

/// The sentence a resource-policy read states when it finds no policy.
const NO_POLICY_SENTENCE: &str = "The resource you requested does not exist.";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| self.header("x-amzn-errortype"))
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// One response header.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// A fresh router on its own engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    router_on(&engine)
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create a function.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `AddPermission` on a function.
async fn add_permission(router: &MultiServiceRouter, function: &str, body: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/policy"),
        body,
    )
    .await
}

/// `GetPolicy` on a function.
async fn get_policy(router: &MultiServiceRouter, function: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{function}/policy"),
        &Value::Null,
    )
    .await
}

/// `PutResourcePolicy` on the resource `arn` names.
async fn put_resource_policy(
    router: &MultiServiceRouter,
    arn: &str,
    policy: &str,
    revision_id: Option<&str>,
) -> Answer {
    let mut body = json!({ "Policy": policy });
    if let Some(revision) = revision_id {
        body["RevisionId"] = json!(revision);
    }
    call(router, "PUT", &format!("{RESOURCE_POLICY_URI}/{arn}"), &body).await
}

/// `GetResourcePolicy` on the resource `arn` names.
async fn get_resource_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{RESOURCE_POLICY_URI}/{arn}"),
        &Value::Null,
    )
    .await
}

/// `DeleteResourcePolicy` on the resource `arn` names.
async fn delete_resource_policy(
    router: &MultiServiceRouter,
    arn: &str,
    revision_id: Option<&str>,
) -> Answer {
    let query = revision_id.map_or_else(String::new, |revision| format!("?RevisionId={revision}"));
    call(
        router,
        "DELETE",
        &format!("{RESOURCE_POLICY_URI}/{arn}{query}"),
        &Value::Null,
    )
    .await
}

/// An `AddPermission` body whose serialized length is exactly `bytes`, padded in its
/// `SourceArn` — a member whose modelled maximum (10000) is well past the request quota, so
/// the padding is what the size of the request is varied along.
fn body_of_size(statement_id: &str, bytes: usize) -> Value {
    let request = |padding: usize| {
        json!({
            "StatementId": statement_id,
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
            "SourceArn": format!("arn:aws:s3:::{}", "b".repeat(padding)),
        })
    };
    let empty = serde_json::to_vec(&request(0))
        .expect("the request must serialize")
        .len();
    let padding = bytes.saturating_sub(empty);
    let built = request(padding);
    let length = serde_json::to_vec(&built)
        .expect("the request must serialize")
        .len();
    assert_eq!(
        length, bytes,
        "the test needs a request of exactly {bytes} bytes, got {length}"
    );
    built
}

/// A request past the quota published for the operation it addresses is refused `413`,
/// naming that quota and that operation — and one within the quota is served.
#[tokio::test]
async fn a_request_past_its_operations_quota_is_refused_as_too_large() {
    let router = router();
    create_function(&router, "quota").await;

    // The rows differ along the one thing the rule generalises over — the size of the
    // request — at and around the quota, so the boundary is asserted rather than one
    // recorded size.
    let cases: &[(&str, usize, bool)] = &[
        ("a small request", 200, false),
        (
            "one byte under the quota",
            ADD_PERMISSION_REQUEST_QUOTA - 1,
            false,
        ),
        ("exactly the quota", ADD_PERMISSION_REQUEST_QUOTA, false),
        (
            "one byte over the quota",
            ADD_PERMISSION_REQUEST_QUOTA + 1,
            true,
        ),
        (
            "far over the quota",
            ADD_PERMISSION_REQUEST_QUOTA * 2,
            true,
        ),
    ];
    for (index, (case, size, refused)) in cases.iter().enumerate() {
        let body = body_of_size(&format!("size-{index}"), *size);
        let answer = add_permission(&router, "quota", &body).await;
        if !refused {
            assert_eq!(
                answer.status,
                201,
                "case {case:?}: expected the grant to be served, got {} {}",
                answer.status,
                answer.text()
            );
            continue;
        }
        assert_eq!(
            answer.status,
            413,
            "case {case:?}: expected HTTP 413, got {} {}",
            answer.status,
            answer.text()
        );
        let expected = format!(
            "Request must be smaller than {ADD_PERMISSION_REQUEST_QUOTA} bytes for the \
             AddPermission operation"
        );
        assert_eq!(
            answer.string("Message"),
            Some(expected.clone()),
            "case {case:?}: expected {expected:?}, got {}",
            answer.text()
        );
        assert_eq!(
            answer.string("Type").as_deref(),
            Some(USER_FAULT),
            "case {case:?}: expected the fault class on the refusal, got {}",
            answer.text()
        );
        assert_eq!(
            answer.header("x-amzn-errortype").as_deref(),
            Some("RequestEntityTooLargeException"),
            "case {case:?}: expected the too-large error code, got {:?}",
            answer.header("x-amzn-errortype")
        );
    }
}

/// A grant that would push the stored policy past its own quota is refused `400`, stating
/// the size the policy would have reached — a computed value, which differs with the size of
/// the statements — and the limit it may not pass.
#[tokio::test]
async fn the_policy_quota_refusal_states_the_resulting_size_and_the_limit() {
    // Two rows, differing in the size of each statement, so the refusal happens after a
    // different number of grants and at a different resulting size: the stated size is
    // computed from the policy rather than fixed.
    let cases: &[(&str, usize)] = &[("statements of about 4.5 KB", 4_400), ("statements of about 2.5 KB", 2_400)];
    let mut reported_sizes = Vec::new();
    for (case, padding) in cases {
        let router = router();
        create_function(&router, "bulky").await;
        let body = |sid: &str| {
            json!({
                "StatementId": sid,
                "Action": "lambda:InvokeFunction",
                "Principal": "s3.amazonaws.com",
                "SourceArn": format!("arn:aws:s3:::{}", "b".repeat(*padding)),
            })
        };
        let mut granted = 0_usize;
        let mut refusal = None;
        for index in 0..24 {
            let answer = add_permission(&router, "bulky", &body(&format!("stmt-{index}"))).await;
            if answer.status == 201 {
                granted += 1;
                continue;
            }
            refusal = Some(answer);
            break;
        }
        let refused = refusal.unwrap_or_else(|| {
            panic!("case {case:?}: a policy past {MAX_POLICY_SIZE_BYTES} bytes must be refused")
        });
        assert!(
            granted > 1,
            "case {case:?}: expected several statements to fit, got {granted}"
        );
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status,
            refused.text()
        );
        assert!(
            refused.error_code().contains("PolicyLengthExceededException"),
            "case {case:?}: expected PolicyLengthExceededException, got {}",
            refused.error_code()
        );
        assert_eq!(
            refused.string("Type").as_deref(),
            Some(USER_FAULT),
            "case {case:?}: expected the fault class on the refusal, got {}",
            refused.text()
        );
        let stated = refused.string("message").unwrap_or_default();
        let size = stated
            .strip_prefix("The final policy size (")
            .and_then(|rest| rest.split_once(')'))
            .and_then(|(size, tail)| {
                (tail == format!(" is bigger than the limit ({MAX_POLICY_SIZE_BYTES})."))
                    .then(|| size.parse::<usize>().ok())
                    .flatten()
            });
        let size = size.unwrap_or_else(|| {
            panic!("case {case:?}: expected the final-policy-size refusal, got {stated:?}")
        });
        assert!(
            size > MAX_POLICY_SIZE_BYTES,
            "case {case:?}: expected a size past the {MAX_POLICY_SIZE_BYTES} byte quota, got \
             {size} in {stated:?}"
        );
        // The refused grant left the policy as it was, so what the read reports still fits.
        let stored = get_policy(&router, "bulky")
            .await
            .string("Policy")
            .unwrap_or_default();
        assert!(
            stored.len() <= MAX_POLICY_SIZE_BYTES,
            "case {case:?}: the refused grant was stored anyway: {} bytes",
            stored.len()
        );
        reported_sizes.push((case, size));
    }
    let [(first_case, first), (second_case, second)] = reported_sizes[..] else {
        panic!("expected one reported size per row, got {reported_sizes:?}");
    };
    assert_ne!(
        first, second,
        "case {first_case:?} vs {second_case:?}: the stated size must be computed from the \
         policy, but both rows reported {first}"
    );
}

/// A read of a function's policy states whether the document it reports grants public
/// access.
#[tokio::test]
async fn a_policy_read_states_whether_the_policy_allows_public_access() {
    let router = router();

    // One row per form of grant, differing in who the statement allows and whether a
    // condition narrows it — the dimensions the rule generalises over.
    let cases: &[(&str, &str, Value, bool)] = &[
        (
            "a grant to one service principal",
            "scoped-service",
            json!({
                "StatementId": "one",
                "Action": "lambda:InvokeFunction",
                "Principal": "s3.amazonaws.com",
                "SourceArn": "arn:aws:s3:::probe-bucket",
            }),
            false,
        ),
        (
            "a grant to one account",
            "scoped-account",
            json!({
                "StatementId": "one",
                "Action": "lambda:InvokeFunction",
                "Principal": "123456789012",
            }),
            false,
        ),
        (
            "a grant to every principal",
            "public",
            json!({
                "StatementId": "one",
                "Action": "lambda:InvokeFunction",
                "Principal": "*",
            }),
            true,
        ),
        (
            "a grant to every principal in one organization",
            "org-wide",
            json!({
                "StatementId": "one",
                "Action": "lambda:InvokeFunction",
                "Principal": "*",
                "PrincipalOrgID": "o-abcdefghij",
            }),
            false,
        ),
        (
            "a grant to every principal from one source account",
            "account-wide",
            json!({
                "StatementId": "one",
                "Action": "lambda:InvokeFunction",
                "Principal": "*",
                "SourceAccount": "123456789012",
            }),
            false,
        ),
    ];
    for (case, function, grant, public) in cases {
        create_function(&router, function).await;
        let granted = add_permission(&router, function, grant).await;
        assert_eq!(
            granted.status,
            201,
            "case {case:?}: expected the grant to be served, got {} {}",
            granted.status,
            granted.text()
        );
        let answer = get_policy(&router, function).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the policy to be reported, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.json().get("PublicAccessAllowed").and_then(Value::as_bool),
            Some(*public),
            "case {case:?}: expected public access {public}, got {}",
            answer.text()
        );
    }

    // A document a caller PUT is judged the same way, including the wildcard grant a
    // condition narrows, which only a document can express this way.
    create_function(&router, "put-policy").await;
    let documents: &[(&str, &str, bool)] = &[
        (
            "a put document granting everyone",
            r#"{"Version":"2012-10-17","Id":"default","Statement":[{"Sid":"one","Effect":"Allow","Principal":"*","Action":"lambda:InvokeFunction"}]}"#,
            true,
        ),
        (
            "a put document granting everyone from one organization",
            r#"{"Version":"2012-10-17","Id":"default","Statement":[{"Sid":"one","Effect":"Allow","Principal":{"AWS":"*"},"Action":"lambda:InvokeFunction","Condition":{"StringEquals":{"aws:PrincipalOrgID":"o-abcdefghij"}}}]}"#,
            false,
        ),
        (
            "a put document denying everyone",
            r#"{"Version":"2012-10-17","Id":"default","Statement":[{"Sid":"one","Effect":"Deny","Principal":"*","Action":"lambda:InvokeFunction"}]}"#,
            false,
        ),
    ];
    for (case, document, public) in documents {
        let put = put_resource_policy(&router, &function_arn("put-policy"), document, None).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected the policy to be stored, got {} {}",
            put.status,
            put.text()
        );
        let answer = get_policy(&router, "put-policy").await;
        assert_eq!(
            answer.json().get("PublicAccessAllowed").and_then(Value::as_bool),
            Some(*public),
            "case {case:?}: expected public access {public}, got {}",
            answer.text()
        );
        // The resource-policy read reports the document and its revision alone.
        let read = get_resource_policy(&router, &function_arn("put-policy")).await;
        assert!(
            read.json().get("PublicAccessAllowed").is_none(),
            "case {case:?}: the resource-policy read must not state the flag, got {}",
            read.text()
        );
    }
}

/// The resource-policy operations answer a revision precondition that did not hold with
/// their message alone, while every other refusal of this surface — including the same
/// precondition on the permission operations — names the class of its fault.
#[tokio::test]
async fn the_resource_policy_precondition_is_the_one_refusal_naming_no_fault_class() {
    let router = router();
    create_function(&router, "guarded").await;
    let arn = function_arn("guarded");
    let document = r#"{"Version":"2012-10-17","Id":"default","Statement":[{"Sid":"one","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"lambda:InvokeFunction"}]}"#;
    let stored = put_resource_policy(&router, &arn, document, None).await;
    assert_eq!(
        stored.status,
        200,
        "the policy must be stored, got {} {}",
        stored.status,
        stored.text()
    );
    // A revision that is not the stored policy's: well-formed, so the precondition is what
    // refuses it.
    let stale = "00000000-0000-4000-8000-000000000000";

    let refusals: &[(&str, Answer, bool)] = &[
        (
            "PutResourcePolicy with a stale revision",
            put_resource_policy(&router, &arn, document, Some(stale)).await,
            false,
        ),
        (
            "DeleteResourcePolicy with a stale revision",
            delete_resource_policy(&router, &arn, Some(stale)).await,
            false,
        ),
        (
            "AddPermission with a stale revision",
            add_permission(
                &router,
                "guarded",
                &json!({
                    "StatementId": "two",
                    "Action": "lambda:InvokeFunction",
                    "Principal": "s3.amazonaws.com",
                    "RevisionId": stale,
                }),
            )
            .await,
            true,
        ),
    ];
    for (case, answer, names_class) in refusals {
        assert_eq!(
            answer.status,
            412,
            "case {case:?}: expected HTTP 412, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("PreconditionFailedException"),
            "case {case:?}: expected PreconditionFailedException, got {}",
            answer.error_code()
        );
        let class = answer.string("Type");
        assert_eq!(
            class.is_some(),
            *names_class,
            "case {case:?}: expected the fault class named {names_class}, got {}",
            answer.text()
        );
        if *names_class {
            assert_eq!(
                class.as_deref(),
                Some(USER_FAULT),
                "case {case:?}: expected the user-fault class, got {class:?}"
            );
        }
        assert!(
            answer.string("message").is_some() || answer.string("Message").is_some(),
            "case {case:?}: expected the refusal to carry its message, got {}",
            answer.text()
        );
    }

    // Every other refusal of the same operations still names the class.
    let unknown = function_arn("no-such-function");
    for (case, answer) in [
        (
            "PutResourcePolicy on an unknown function",
            put_resource_policy(&router, &unknown, document, None).await,
        ),
        (
            "PutResourcePolicy with a document that is no policy",
            put_resource_policy(&router, &arn, "not a policy", None).await,
        ),
        (
            "GetResourcePolicy on an unknown function",
            get_resource_policy(&router, &unknown).await,
        ),
    ] {
        assert_eq!(
            answer.string("Type").as_deref(),
            Some(USER_FAULT),
            "case {case:?}: expected the fault class named, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// A read of a resource's policy that finds none states one sentence, whatever it was that
/// it could not find.
#[tokio::test]
async fn a_resource_policy_read_that_finds_nothing_states_one_sentence() {
    let router = router();
    create_function(&router, "bare").await;

    // The rows differ in WHAT is missing: the function, the version the ARN qualifies, and
    // the policy itself.
    let cases: &[(&str, String)] = &[
        ("a function that has no policy", function_arn("bare")),
        (
            "a function that does not exist",
            function_arn("no-such-function"),
        ),
        (
            "a version that does not exist",
            format!("{}:7", function_arn("bare")),
        ),
        (
            "an alias that does not exist",
            format!("{}:live", function_arn("bare")),
        ),
    ];
    for (case, arn) in cases {
        let answer = get_resource_policy(&router, arn).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected HTTP 404, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
        assert_eq!(
            answer.string("Message").as_deref(),
            Some(NO_POLICY_SENTENCE),
            "case {case:?}: expected {NO_POLICY_SENTENCE:?}, got {}",
            answer.text()
        );
    }
}

/// A document a caller puts is reported in the rendering this service stores it in, and
/// putting back what was read stores the same bytes again.
#[tokio::test]
async fn a_put_document_is_reported_in_one_rendering_whatever_layout_it_arrived_in() {
    let router = router();
    create_function(&router, "rendered").await;
    let arn = function_arn("rendered");

    // The rows differ in the LAYOUT the same policy is written in, and in the form a
    // one-value element takes — the dimensions the rendering generalises over.
    let reported = r#"{"Version":"2012-10-17","Id":"default","Statement":[{"Sid":"one","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"lambda:InvokeFunction","Resource":"arn:aws:lambda:us-east-1:000000000000:function:rendered"}]}"#;
    let cases: &[(&str, String)] = &[
        ("already in the reported form", reported.to_string()),
        (
            "a space after every separator between tokens",
            r#"{"Version": "2012-10-17", "Id": "default", "Statement": [{"Sid": "one", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::123456789012:root"}, "Action": "lambda:InvokeFunction", "Resource": "arn:aws:lambda:us-east-1:000000000000:function:rendered"}]}"#
                .to_string(),
        ),
        (
            "indented over several lines",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Id\": \"default\",\n  \"Statement\": [\n    {\n      \
             \"Sid\": \"one\",\n      \"Effect\": \"Allow\",\n      \"Principal\": {\n        \
             \"AWS\": \"arn:aws:iam::123456789012:root\"\n      },\n      \"Action\": \
             \"lambda:InvokeFunction\",\n      \"Resource\": \
             \"arn:aws:lambda:us-east-1:000000000000:function:rendered\"\n    }\n  ]\n}"
                .to_string(),
        ),
        (
            "a one-value Action list",
            reported.replace(
                r#""Action":"lambda:InvokeFunction""#,
                r#""Action":["lambda:InvokeFunction"]"#,
            ),
        ),
    ];
    for (case, document) in cases {
        let put = put_resource_policy(&router, &arn, document, None).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: expected the policy to be stored, got {} {}",
            put.status,
            put.text()
        );
        assert_eq!(
            put.string("Policy").as_deref(),
            Some(reported),
            "case {case:?}: expected the stored rendering, got {}",
            put.text()
        );
        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            read.string("Policy").as_deref(),
            Some(reported),
            "case {case:?}: expected the read to report the same rendering, got {}",
            read.text()
        );
        assert_eq!(
            read.string("RevisionId"),
            put.string("RevisionId"),
            "case {case:?}: expected the read to report the stored revision, got {}",
            read.text()
        );
    }

    // The members a caller wrote in an order of their own keep that order: the rendering
    // drops whitespace, it does not re-serialize the document.
    let caller_order = r#"{"Statement":[{"Action":"lambda:InvokeFunction","Effect":"Allow","Sid":"one","Principal":"*"}],"Id":"custom","Version":"2012-10-17"}"#;
    let put = put_resource_policy(&router, &arn, caller_order, None).await;
    assert_eq!(
        put.string("Policy").as_deref(),
        Some(caller_order),
        "case {:?}: expected the caller's member order to survive, got {}",
        "members in the caller's own order",
        put.text()
    );
}
