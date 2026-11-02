// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the component resource-policy slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. `PutComponentPolicy` carries its members in a
//! JSON body; `GetComponentPolicy` carries the ARN in the `componentArn` query parameter,
//! as the modelled `@httpQuery` puts it.
//!
//! The rule under test: a resource policy is attached to the resource its ARN names, so a
//! policy read reports the document the last successful write applied to that resource,
//! verbatim, and the unset-policy document `{}` when no write has applied one. An ARN the
//! calling Region holds no such resource under is reported as a missing required resource,
//! and a policy document that is not parseable JSON is refused as an invalid parameter
//! value, leaving the stored policy as it was.
//!
//! The rows span what the rule generalises over: the form, size, nesting, whitespace and
//! character set of the document, how many writes a resource has seen, which build version
//! of a component name the ARN names, which Region the endpoint serves, and the way a
//! document fails to be JSON.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The document a policy read reports for a resource no policy has been applied to.
const UNSET_POLICY: &str = "{}";

/// The modelled `ResourcePolicyDocument` length maximum
/// (`smithy_query(query="operation",operation="PutComponentPolicy")` reports
/// `len 1..30000`), which the largest document a case writes stays inside.
const MAX_POLICY_LENGTH: usize = 30_000;

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
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

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `GetComponentPolicy` for `arn`, in `region`.
async fn get_policy_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetComponentPolicy?componentArn={arn}"),
        None,
    )
    .await
}

/// `GetComponentPolicy` for `arn`, in [`REGION`].
async fn get_policy(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    get_policy_in(router, REGION, arn).await
}

/// `PutComponentPolicy` applying `document` to `arn`, in `region`.
async fn put_policy_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    document: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        "/PutComponentPolicy",
        Some(&json!({ "componentArn": arn, "policy": document })),
    )
    .await
}

/// `PutComponentPolicy` applying `document` to `arn`, in [`REGION`].
async fn put_policy(router: &MultiServiceRouter, arn: &str, document: &str) -> (u16, Value) {
    put_policy_in(router, REGION, arn, document).await
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status`, the error code and the message of a refusal.
fn assert_refused(
    case: &str,
    status: u16,
    body: &Value,
    expect_status: u16,
    expect_code: &str,
    expect_message: &str,
) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
    let message = text(case, body, "message");
    assert_eq!(
        message, expect_message,
        "case {case:?}: expected the message {expect_message:?}, got {message:?}"
    );
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// A build component document whose single `build` phase echoes `greeting`, so two builds
/// of one component name can differ in content the way a new build version must.
fn build_document(greeting: &str) -> String {
    format!(
        "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: \
         Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - \
         echo {greeting}\n"
    )
}

/// Create one build version of the component `name` carries in `region`, with `greeting`
/// as its document's content, and answer its build version ARN.
async fn create_component_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    greeting: &str,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert(
        "clientToken".to_string(),
        json!(format!("token-{region}-{name}-{greeting}")),
    );
    body.insert("data".to_string(), json!(build_document(greeting)));
    let case = format!("create the component {name} in {region}");
    let (status, answer) =
        call_in(router, region, "PUT", "/CreateComponent", Some(&Value::Object(body))).await;
    let answer = assert_ok(&case, status, answer);
    text(&case, &answer, "componentBuildVersionArn")
}

/// Create one build version of the component `name` carries in [`REGION`].
async fn create_component(router: &MultiServiceRouter, name: &str, greeting: &str) -> String {
    create_component_in(router, REGION, name, greeting).await
}

/// The message a call naming a resource this Region holds nothing under answers with.
fn not_found_message(arn: &str) -> String {
    format!("The following required resource 'Component' cannot be found: '{arn}'.")
}

/// The message a policy document that is not JSON is refused with.
fn malformed_policy_message() -> String {
    "The value supplied for parameter 'policy' is not valid. Policy is malformed and cannot be \
     parsed"
        .to_string()
}

/// One policy document a write applies and a read must report back verbatim.
#[derive(Debug)]
struct PolicyRow {
    /// What makes this document different from the others.
    case: &'static str,
    /// The document itself.
    document: String,
}

/// The documents a write stores, spanning the form, size, nesting, whitespace and
/// character set the rule generalises over. The documents are JSON; nothing else about
/// them is constrained by the model, whose `policy` member is text of length 1..30000.
fn policy_rows() -> Vec<PolicyRow> {
    let long_value = "a".repeat(MAX_POLICY_LENGTH / 2);
    vec![
        PolicyRow {
            case: "the unset-policy document written explicitly",
            document: UNSET_POLICY.to_string(),
        },
        PolicyRow {
            case: "a one-statement share policy",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": ["111111111111"] },
                    "Action": ["imagebuilder:GetComponent"],
                    "Resource": "*",
                }],
            })
            .to_string(),
        },
        PolicyRow {
            case: "a two-statement policy with nested arrays and conditions",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "AWS": ["111111111111", "222222222222"] },
                        "Action": ["imagebuilder:GetComponent", "imagebuilder:ListComponents"],
                        "Resource": "*",
                        "Condition": { "StringEquals": { "aws:PrincipalOrgID": ["o-abc123"] } },
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "imagebuilder:DeleteComponent",
                        "Resource": "*",
                    },
                ],
            })
            .to_string(),
        },
        PolicyRow {
            case: "a document carrying newlines and indentation",
            document: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": []\n}".to_string(),
        },
        PolicyRow {
            case: "a document whose values carry non-ASCII text",
            document: json!({ "Version": "2012-10-17", "Sid": "accès-partagé-\u{1f512}" })
                .to_string(),
        },
        PolicyRow {
            case: "a document half the modelled maximum length",
            document: json!({ "Version": "2012-10-17", "Sid": long_value }).to_string(),
        },
    ]
}

/// The ways a policy document fails to be JSON. Each is refused as an invalid value for
/// the `policy` member, whatever the component it names.
fn malformed_rows() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an object that is never closed", "{this is not valid json"),
        ("text that is not JSON at all", "not json at all"),
        (
            "an object with a trailing comma",
            "{\"Version\": \"2012-10-17\",}",
        ),
        (
            "an object quoted the way a shell would",
            "{'Version': '2012-10-17'}",
        ),
        ("a bare closing brace", "}"),
        (
            "two documents concatenated",
            "{\"Version\": \"2012-10-17\"}{\"Version\": \"2012-10-17\"}",
        ),
    ]
}

/// The document a read reports for `arn`, asserted to be a successful read.
async fn reported_policy(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let (status, body) = get_policy(router, arn).await;
    let body = assert_ok(case, status, body);
    let request_id = text(case, &body, "requestId");
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected a non-empty requestId, got {request_id:?}"
    );
    text(case, &body, "policy")
}

#[tokio::test]
async fn the_policy_a_write_applies_is_the_document_the_read_reports() {
    let router = emulator();

    for (index, row) in policy_rows().into_iter().enumerate() {
        let arn = create_component(&router, &format!("policy-row-{index}"), "hello").await;

        let (status, body) = put_policy(&router, &arn, &row.document).await;
        let body = assert_ok(row.case, status, body);
        let written = text(row.case, &body, "componentArn");
        assert_eq!(
            written, arn,
            "case {:?}: expected the write to name the component {arn:?}, got {written:?}",
            row.case
        );

        let reported = reported_policy(&router, row.case, &arn).await;
        assert_eq!(
            reported, row.document,
            "case {:?}: expected the stored document back verbatim, got {reported:?}",
            row.case
        );

        // A read changes nothing a later caller sees: the same read answers the same
        // document.
        let again = reported_policy(&router, row.case, &arn).await;
        assert_eq!(
            again, row.document,
            "case {:?}: expected a second read to report the same document, got {again:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_write_replaces_the_document_the_component_carried() {
    let router = emulator();
    let arn = create_component(&router, "replaced-policy", "hello").await;

    let mut previous = UNSET_POLICY.to_string();
    for row in policy_rows() {
        let before = reported_policy(&router, row.case, &arn).await;
        assert_eq!(
            before, previous,
            "case {:?}: expected the previously stored document, got {before:?}",
            row.case
        );

        let (status, body) = put_policy(&router, &arn, &row.document).await;
        assert_ok(row.case, status, body);

        let after = reported_policy(&router, row.case, &arn).await;
        assert_eq!(
            after, row.document,
            "case {:?}: expected the newly written document, got {after:?}",
            row.case
        );
        previous = row.document;
    }
}

#[tokio::test]
async fn a_component_no_policy_was_applied_to_reports_the_unset_policy_document() {
    let router = emulator();

    // Three components no write ever named: one on its own, two build versions of one
    // name, and one reached through another Region's endpoint.
    let alone = create_component(&router, "unshared-component", "hello").await;
    let first_build = create_component(&router, "two-build-component", "hello").await;
    let second_build = create_component(&router, "two-build-component", "goodbye").await;
    let elsewhere =
        create_component_in(&router, OTHER_REGION, "unshared-component", "hello").await;
    assert_ne!(
        first_build, second_build,
        "expected two build versions to carry different ARNs, got {first_build:?} twice"
    );

    for (case, arn) in [
        ("a component nothing was ever applied to", &alone),
        ("the first build version of a name", &first_build),
        ("the second build version of a name", &second_build),
    ] {
        let reported = reported_policy(&router, case, arn).await;
        assert_eq!(
            reported, UNSET_POLICY,
            "case {case:?}: expected the unset-policy document, got {reported:?}"
        );
    }

    let case = "a component of another Region, read through that Region's endpoint";
    let (status, body) = get_policy_in(&router, OTHER_REGION, &elsewhere).await;
    let body = assert_ok(case, status, body);
    let reported = text(case, &body, "policy");
    assert_eq!(
        reported, UNSET_POLICY,
        "case {case:?}: expected the unset-policy document, got {reported:?}"
    );

    // A policy applied to one build version is that build version's alone.
    let shared = json!({ "Version": "2012-10-17", "Statement": [] }).to_string();
    let (status, body) = put_policy(&router, &first_build, &shared).await;
    assert_ok("apply a policy to the first build version", status, body);
    for (case, arn, expected) in [
        ("the build version the write named", &first_build, &shared),
        (
            "the sibling build version of the same name",
            &second_build,
            &UNSET_POLICY.to_string(),
        ),
        ("a component of another name", &alone, &UNSET_POLICY.to_string()),
    ] {
        let reported = reported_policy(&router, case, arn).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_policy_that_is_not_json_is_refused_and_leaves_the_stored_policy_alone() {
    let router = emulator();
    let arn = create_component(&router, "malformed-policy", "hello").await;
    let stored = json!({ "Version": "2012-10-17", "Statement": [] }).to_string();

    // Before any write, the refusal must leave the component with no policy at all.
    for (case, document) in malformed_rows() {
        let (status, body) = put_policy(&router, &arn, document).await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "InvalidParameterValueException",
            &malformed_policy_message(),
        );
        let reported = reported_policy(&router, case, &arn).await;
        assert_eq!(
            reported, UNSET_POLICY,
            "case {case:?}: expected the refusal to apply no policy, got {reported:?}"
        );
    }

    // And once a document is stored, a refused write must not disturb it.
    let (status, body) = put_policy(&router, &arn, &stored).await;
    assert_ok("apply a well-formed policy", status, body);
    for (case, document) in malformed_rows() {
        let (status, body) = put_policy(&router, &arn, document).await;
        assert_refused(
            case,
            status,
            &body,
            400,
            "InvalidParameterValueException",
            &malformed_policy_message(),
        );
        let reported = reported_policy(&router, case, &arn).await;
        assert_eq!(
            reported, stored,
            "case {case:?}: expected the stored document to survive the refusal, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_this_region_holds_no_component_under_is_reported_missing() {
    let router = emulator();
    let existing = create_component(&router, "present-component", "hello").await;
    let elsewhere =
        create_component_in(&router, OTHER_REGION, "other-region-component", "hello").await;
    let document = json!({ "Version": "2012-10-17", "Statement": [] }).to_string();

    let never_created = existing.replace("present-component", "absent-component");
    let other_build = format!("{}2", existing.trim_end_matches('1'));
    let other_version = existing.replace("/1.0.0/", "/9.9.9/");
    assert_ne!(
        never_created, existing,
        "expected the absent-name ARN to differ from {existing:?}"
    );

    let rows: Vec<(&str, &String)> = vec![
        ("a name nothing was created under", &never_created),
        ("a build version that was never built", &other_build),
        ("a semantic version that was never created", &other_version),
        (
            "a component of another Region, through this Region's endpoint",
            &elsewhere,
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = get_policy(&router, arn).await;
        assert_refused(
            case,
            status,
            &body,
            404,
            "ResourceNotFoundException",
            &not_found_message(arn),
        );

        let (status, body) = put_policy(&router, arn, &document).await;
        assert_refused(
            case,
            status,
            &body,
            404,
            "ResourceNotFoundException",
            &not_found_message(arn),
        );
    }

    // The component that does exist is unaffected by every refusal above.
    let reported = reported_policy(&router, "the component that does exist", &existing).await;
    assert_eq!(
        reported, UNSET_POLICY,
        "expected the existing component to carry no policy, got {reported:?}"
    );
}
