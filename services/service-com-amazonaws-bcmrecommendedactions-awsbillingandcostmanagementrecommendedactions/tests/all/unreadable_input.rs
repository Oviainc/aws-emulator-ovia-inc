// SPDX-License-Identifier: CC0-1.0

//! Every refusal of `ListRecommendedActions`' input states the modelled
//! `ValidationException$reason`, including the refusals of an input this service
//! could not READ.
//!
//! `reason` is REQUIRED by the model and `ValidationExceptionReason` declares one
//! value for each class: `cannotParse` for a value the service could not read at
//! all, `fieldValidationFailed` for one it read and does not accept. So the class a
//! refusal states must follow from whether the value was readable — never from
//! whether the failure happened to have a member-level form the framework could
//! describe.
//!
//! The suite sends RAW request bytes, because the inputs it reasons about are
//! bodies a JSON value cannot express: a member present with the wrong JSON type,
//! and text that is not JSON at all.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

/// The awsJson1_0 target this service answers.
const TARGET: &str = "AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name — the routing signal the router reads. The signature is not
/// checked: request authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261028/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The media type a refusal of the REQUEST announces: the one the model's protocol
/// trait declares (see the `crate::service` module docs).
const REFUSAL_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The `ValidationExceptionReason` values the model publishes
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const PUBLISHED_REASONS: [&str; 4] = [
    "unknownOperation",
    "cannotParse",
    "fieldValidationFailed",
    "other",
];

/// One emulator: a shared engine, and a router with this service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator, holding the inventory the service computed for this
    /// account.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(engine));
        Self { router }
    }

    /// `ListRecommendedActions` with `body` as the payload BYTES, reporting the
    /// status, the announced media type and the parsed payload.
    async fn list_bytes(&self, body: &str) -> (u16, String, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", TARGET)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(body.as_bytes().to_vec()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, content_type, payload)
    }
}

/// The wire error code of a failure response (`__type`, bare or namespaced).
fn error_code(case: &str, body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: error body carries no __type: {body}"));
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// The `reason` a refusal states, asserted to be one the model publishes.
fn reason(case: &str, body: &Value) -> String {
    let stated = body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must state a reason, got {body}"));
    assert!(
        PUBLISHED_REASONS.contains(&stated),
        "case {case:?}: expected a modelled reason out of {PUBLISHED_REASONS:?}, got \
         {stated:?} in {body}"
    );
    stated.to_string()
}

/// The member names a refusal's `fieldList` carries, in ascending order so a case
/// does not depend on the order the members were written in.
fn field_names(case: &str, body: &Value) -> Vec<String> {
    let mut names: Vec<String> = body
        .get("fieldList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList array, got {body}"))
        .iter()
        .map(|field| {
            field
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a fieldList entry must name its member, got {body}")
                })
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// Assert the shared shape of a refusal: a 400 `ValidationException` in the media
/// type a request refusal announces, carrying a non-empty message and reporting no
/// page.
fn assert_refusal(case: &str, status: u16, media_type: &str, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        media_type, REFUSAL_MEDIA_TYPE,
        "case {case:?}: expected media type {REFUSAL_MEDIA_TYPE:?}, got {media_type:?} with \
         {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected a ValidationException, got {code:?} in {body}"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a refusal must state a message, got {body}"));
    assert!(
        !message.is_empty(),
        "case {case:?}: expected a non-empty message, got {message:?} in {body}"
    );
    assert!(
        body.get("recommendedActions").is_none(),
        "case {case:?}: a refused request must not report a page, got {body}"
    );
}

/// One unreadable-input case: the raw body, and the input members whose values the
/// refusal must name.
struct UnreadableCase {
    case: &'static str,
    body: &'static str,
    members: Vec<&'static str>,
}

/// A member present with a value of a JSON type the member cannot hold, for each
/// member of the input shape (`smithy_query(query="operation",
/// operation="ListRecommendedActions")`: `filter`, `maxResults`, `nextToken`) and
/// for the shapes nested under `filter`. The rows differ along the member, the JSON
/// type sent, the depth the unreadable value sits at, and how many members are
/// unreadable at once.
fn unreadable_cases() -> Vec<UnreadableCase> {
    vec![
        UnreadableCase {
            case: "maxResults as a string",
            body: r#"{"maxResults": "ten"}"#,
            members: vec!["maxResults"],
        },
        UnreadableCase {
            case: "maxResults as a boolean",
            body: r#"{"maxResults": true}"#,
            members: vec!["maxResults"],
        },
        UnreadableCase {
            case: "maxResults as an array",
            body: r#"{"maxResults": [1]}"#,
            members: vec!["maxResults"],
        },
        UnreadableCase {
            case: "maxResults as a fraction",
            body: r#"{"maxResults": 1.5}"#,
            members: vec!["maxResults"],
        },
        UnreadableCase {
            case: "nextToken as a number",
            body: r#"{"nextToken": 7}"#,
            members: vec!["nextToken"],
        },
        UnreadableCase {
            case: "nextToken as an object",
            body: r#"{"nextToken": {"offset": 1}}"#,
            members: vec!["nextToken"],
        },
        UnreadableCase {
            case: "filter as an array",
            body: r#"{"filter": []}"#,
            members: vec!["filter"],
        },
        UnreadableCase {
            case: "filter.actions as a number",
            body: r#"{"filter": {"actions": 7}}"#,
            members: vec!["filter"],
        },
        UnreadableCase {
            case: "an action's values as a string",
            body: r#"{"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": "BUDGETS"}
            ]}}"#,
            members: vec!["filter"],
        },
        UnreadableCase {
            case: "an action's key as a number",
            body: r#"{"filter": {"actions": [
                {"key": 1, "matchOption": "EQUALS", "values": ["BUDGETS"]}
            ]}}"#,
            members: vec!["filter"],
        },
        UnreadableCase {
            case: "a filter value as a number",
            body: r#"{"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": [7]}
            ]}}"#,
            members: vec!["filter"],
        },
        UnreadableCase {
            case: "two unreadable members at once",
            body: r#"{"maxResults": "ten", "nextToken": 7}"#,
            members: vec!["maxResults", "nextToken"],
        },
        UnreadableCase {
            case: "three unreadable members at once",
            body: r#"{"filter": 1, "maxResults": "ten", "nextToken": 7}"#,
            members: vec!["filter", "maxResults", "nextToken"],
        },
        UnreadableCase {
            case: "an unreadable member beside a readable one",
            body: r#"{"maxResults": "ten", "nextToken": "0123"}"#,
            members: vec!["maxResults"],
        },
        UnreadableCase {
            case: "an unreadable member beside a key the model does not declare",
            body: r#"{"notAModelledMember": [1, 2], "maxResults": "ten"}"#,
            members: vec!["maxResults"],
        },
    ]
}

#[tokio::test]
async fn a_member_the_service_cannot_read_is_refused_as_cannot_parse_naming_that_member() {
    let emulator = Emulator::new();

    for UnreadableCase {
        case,
        body,
        members,
    } in unreadable_cases()
    {
        let (status, media_type, payload) = emulator.list_bytes(body).await;
        assert_refusal(case, status, &media_type, &payload);

        let stated = reason(case, &payload);
        assert_eq!(
            stated, "cannotParse",
            "case {case:?}: expected reason cannotParse for a value the service could not \
             read, got {stated:?} in {payload}"
        );

        let mut expected: Vec<String> = members.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let named = field_names(case, &payload);
        assert_eq!(
            named, expected,
            "case {case:?}: expected the refusal to name {expected:?}, got {named:?} in \
             {payload}"
        );
        for field in payload
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            let message = field.get("message").and_then(Value::as_str).unwrap_or("");
            assert!(
                !message.is_empty(),
                "case {case:?}: each named member must say why it failed, got {field}"
            );
        }
    }
}

#[tokio::test]
async fn a_body_that_names_no_member_is_still_refused_as_cannot_parse() {
    let emulator = Emulator::new();

    // Bodies with nothing member-level to name: a JSON value that is not this
    // input's object, and text that is not JSON at all. `fieldList` is optional in
    // the model, so a refusal with no member to name carries none — but `reason` is
    // required, so it is stated all the same.
    let cases: Vec<(&str, &str)> = vec![
        ("an array", "[1, 2]"),
        ("a string", "\"ten\""),
        ("a number", "7"),
        ("a boolean", "true"),
        ("an unterminated object", "{\"maxResults\": 1"),
        ("a truncated member", "{\"maxResults\":"),
        ("a bare brace", "{"),
        ("text that is not JSON", "not json at all"),
        ("an object key that is not a string", "{1: 2}"),
    ];

    for (case, body) in cases {
        let (status, media_type, payload) = emulator.list_bytes(body).await;
        assert_refusal(case, status, &media_type, &payload);

        let stated = reason(case, &payload);
        assert_eq!(
            stated, "cannotParse",
            "case {case:?}: expected reason cannotParse for a body the service could not \
             read, got {stated:?} in {payload}"
        );
        assert!(
            payload.get("fieldList").is_none(),
            "case {case:?}: a refusal that names no member must carry no fieldList, got \
             {payload}"
        );
    }
}

#[tokio::test]
async fn a_value_the_service_reads_and_refuses_still_states_field_validation_failed() {
    let emulator = Emulator::new();

    // The other side of the rule: these values ARE readable — the service read them
    // and refused what they say — so they keep the member-level reason, which is what
    // tells a client the difference between a value to re-type and a value to change.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "maxResults below the published range",
            r#"{"maxResults": 0}"#,
            vec!["maxResults"],
        ),
        (
            "maxResults above the published range",
            r#"{"maxResults": 1001}"#,
            vec!["maxResults"],
        ),
        (
            "an action key outside the published set",
            r#"{"filter": {"actions": [
                {"key": "NOT_A_REAL_FILTER_KEY", "matchOption": "EQUALS", "values": ["BUDGETS"]}
            ]}}"#,
            vec!["filter.actions.1.member.key"],
        ),
        (
            "an empty values list",
            r#"{"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": []}
            ]}}"#,
            vec!["filter.actions.1.member.values"],
        ),
    ];

    for (case, body, members) in cases {
        let (status, media_type, payload) = emulator.list_bytes(body).await;
        assert_refusal(case, status, &media_type, &payload);

        let stated = reason(case, &payload);
        assert_eq!(
            stated, "fieldValidationFailed",
            "case {case:?}: expected reason fieldValidationFailed for a value the service \
             read, got {stated:?} in {payload}"
        );
        let mut expected: Vec<String> = members.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let named = field_names(case, &payload);
        assert_eq!(
            named, expected,
            "case {case:?}: expected the refusal to name {expected:?}, got {named:?} in \
             {payload}"
        );
    }
}

#[tokio::test]
async fn a_readable_input_is_still_served() {
    let emulator = Emulator::new();

    // The refusal paths must not claim a request they have nothing to refuse: every
    // row below is readable and within every modelled constraint, including the
    // bounds of `maxResults` and a body that names no member at all.
    let cases: Vec<(&str, &str)> = vec![
        ("an empty object", "{}"),
        ("an empty body", ""),
        ("the lowest page size", r#"{"maxResults": 1}"#),
        ("the highest page size", r#"{"maxResults": 1000}"#),
        (
            "a filter",
            r#"{"filter": {"actions": [
                {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["CRITICAL"]}
            ]}}"#,
        ),
        (
            "a key the model does not declare",
            r#"{"notAModelledMember": [1, 2]}"#,
        ),
    ];

    for (case, body) in cases {
        let (status, _, payload) = emulator.list_bytes(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {payload}"
        );
        assert!(
            payload.get("recommendedActions").is_some(),
            "case {case:?}: expected a reported page, got {payload}"
        );
    }
}
