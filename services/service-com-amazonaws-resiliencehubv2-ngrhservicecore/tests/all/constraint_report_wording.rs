// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request the framework refused before any handler ran.
//!
//! A value the modelled `@pattern` of the member it was sent for does not admit is
//! refused with this service's own constraint report — the bracketed list the
//! regular-expression layer in front of it states, one entry per violated member, naming
//! the expression the value had to match and the value that did not match it — and not
//! with the common AWS framework form that counts the violations and names the member
//! path.
//!
//! The suites are TABLES: a row is one input of the class the rule covers — the operation,
//! the member, the form of the expression, the form of the value, how many members broke
//! their pattern — and the recorded `CreateSystem` case is ONE of them. The last suite is
//! the other half of the rule: every OTHER constraint class keeps the framework's own
//! wording, so one rule is never reported in two voices.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch`), so the refusal
//! is rendered by the same seam a client reaches.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`resiliencehub`). Signature verification is off for these tests, so the value
/// only has to parse.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY00000000/20260101/us-east-1/resiliencehub/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// `EntityName`'s `@pattern`, transcribed from the model
/// (`smithy_query(query="operation",operation="CreateSystem")`, whose `name` targets it;
/// `CreateService$name` and `CreatePolicy$name` target the same shape).
const ENTITY_NAME_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$";

/// `EntityLabel`'s `@pattern`, from
/// `smithy_query(query="operation",operation="CreateServiceFunction")`. Close to
/// `EntityName`'s and NOT the same: this one admits a space inside the label.
const ENTITY_LABEL_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9 _\-]{1,59}$";

/// `ClientToken`'s `@pattern`, from the same two operations' `clientToken`.
const CLIENT_TOKEN_PATTERN: &str = r"^[A-Za-z0-9_.-]{0,63}$";

/// `AccountId`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListServices")`.
const ACCOUNT_ID_PATTERN: &str = r"^[0-9]{12}$";

/// `OuId`'s `@pattern`, from the same operation.
const OU_ID_PATTERN: &str = r"^ou-[a-z0-9]{4,32}-[a-z0-9]{8,32}$";

/// `Arn`'s `@pattern`, from the same operation (`ListServices$policyArn`); every member of
/// this model that carries an ARN targets this one shape.
const ARN_PATTERN: &str = r"^arn:(aws|aws-cn|aws-iso|aws-iso-[a-z]{1}|aws-us-gov):[A-Za-z0-9][A-Za-z0-9_/.-]{0,62}:([a-z]{2}-((iso[a-z]{0,1}-)|(gov-)){0,1}[a-z]+-[0-9]):[0-9]{12}:[A-Za-z0-9/][A-Za-z0-9:_/+.-]{0,1023}$";

/// An ARN-shaped value whose Region segment is not a Region, so it breaks `Arn`'s
/// `@pattern` while still clearing that shape's `@length` minimum of 31 — the refusal then
/// holds the pattern violation alone.
const MALFORMED_ARN: &str = "arn:aws:resiliencehub:not_a_region:000000000000:service/x";

/// A well-formed `Arn` for a resource nothing was ever stored under: enough to satisfy the
/// member's own constraints so a SIBLING member's pattern violation is the only one.
const WELL_FORMED_ARN: &str =
    "arn:aws:resiliencehub:us-east-1:000000000000:service/11111111-1111-4111-8111-111111111111";

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService::new(engine),
    );
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("a JSON body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", SIGNED)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// One entry of the report: the expression the value had to match and the value that did
/// not match it.
fn entry(pattern: &str, value: &str) -> String {
    format!("ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"")
}

/// The whole report: the entries inside ONE pair of brackets.
fn report(entries: &[String]) -> String {
    format!("[{}]", entries.join(", "))
}

/// One row of the one-member table: a request carrying one value outside the `@pattern`
/// of the member it was sent for.
struct PatternCase {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
    /// The expression that member's value had to satisfy.
    pattern: &'static str,
    /// The value that did not satisfy it.
    value: &'static str,
}

/// RULE: a value outside the modelled `@pattern` of the member it was sent for is refused
/// 400 `ValidationException` with this service's constraint report — `[ECMA 262 regex
/// "<the member's @pattern>" does not match input string "<the submitted value>"]` —
/// whichever operation and whichever member carried it.
#[tokio::test]
async fn a_value_outside_a_modelled_pattern_is_refused_with_the_constraint_report() {
    let router = emulator();

    let cases = vec![
        PatternCase {
            label: "CreateSystem name holding spaces and punctuation",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "bad name has spaces!"})),
            pattern: ENTITY_NAME_PATTERN,
            value: "bad name has spaces!",
        },
        PatternCase {
            label: "CreateSystem name whose first character is not alphanumeric",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "_leading-underscore"})),
            pattern: ENTITY_NAME_PATTERN,
            value: "_leading-underscore",
        },
        PatternCase {
            label: "CreateService name holding dots",
            method: "POST",
            uri: "/v2/create-service",
            body: Some(json!({
                "name": "name.with.dots",
                "regions": ["us-east-1"],
                "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
            })),
            pattern: ENTITY_NAME_PATTERN,
            value: "name.with.dots",
        },
        PatternCase {
            label: "CreatePolicy name holding a space",
            method: "POST",
            uri: "/v2/create-policy",
            body: Some(json!({"name": "policy name"})),
            pattern: ENTITY_NAME_PATTERN,
            value: "policy name",
        },
        PatternCase {
            // `EntityLabel` admits the space `EntityName` refuses, so the entry has to
            // state the expression of the member that was sent, not one of its siblings.
            label: "CreateServiceFunction name holding an asterisk",
            method: "POST",
            uri: "/v2/create-service-function",
            body: Some(json!({
                "name": "function*label",
                "serviceArn": WELL_FORMED_ARN,
                "criticality": "PRIMARY",
            })),
            pattern: ENTITY_LABEL_PATTERN,
            value: "function*label",
        },
        PatternCase {
            label: "CreateSystem clientToken holding a space",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a-well-formed-name", "clientToken": "bad token"})),
            pattern: CLIENT_TOKEN_PATTERN,
            value: "bad token",
        },
        PatternCase {
            label: "ListServices accountId of twelve non-digits",
            method: "GET",
            uri: "/v2/list-services?accountId=notanaccount",
            body: None,
            pattern: ACCOUNT_ID_PATTERN,
            value: "notanaccount",
        },
        PatternCase {
            label: "ListServices ouId in upper case",
            method: "GET",
            uri: "/v2/list-services?ouId=ou-ABCD-EFGHIJKL",
            body: None,
            pattern: OU_ID_PATTERN,
            value: "ou-ABCD-EFGHIJKL",
        },
        PatternCase {
            label: "ListServices policyArn whose Region segment is not a Region",
            method: "GET",
            uri: "/v2/list-services?policyArn=arn:aws:resiliencehub:not_a_region:000000000000:service/x",
            body: None,
            pattern: ARN_PATTERN,
            value: MALFORMED_ARN,
        },
        PatternCase {
            label: "GetSystem systemArn whose Region segment is not a Region",
            method: "GET",
            uri: "/v2/get-system?systemArn=arn:aws:resiliencehub:not_a_region:000000000000:service/x",
            body: None,
            pattern: ARN_PATTERN,
            value: MALFORMED_ARN,
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.method, case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} and {body:?}",
            case.label
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {:?}: expected ValidationException, got {code:?}",
            case.label
        );
        let expected = report(&[entry(case.pattern, case.value)]);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            case.label
        );
    }
}

/// Characters `EntityName`'s `@pattern` does not admit after its first character — read
/// off the expression itself, whose tail class is `[A-Za-z0-9_\-]`, so everything outside
/// that class is a row. None of them is the character the recorded case carried.
const CHARACTERS_OUTSIDE_THE_NAME_PATTERN: [char; 10] =
    ['.', '/', '@', '*', '#', ':', ',', '+', '=', '\t'];

/// RULE: the entry quotes the value the request carried, VERBATIM — the rule generalises
/// over the value, so one row per character the expression does not admit, each inside an
/// otherwise well-formed name.
#[tokio::test]
async fn the_report_quotes_the_value_the_request_carried() {
    let router = emulator();

    for character in CHARACTERS_OUTSIDE_THE_NAME_PATTERN {
        let name = format!("ab{character}cd");
        let (status, body) = call(
            &router,
            "POST",
            "/v2/create-system",
            Some(json!({"name": name})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected 400, got {status} and {body:?}"
        );
        let expected = report(&[entry(ENTITY_NAME_PATTERN, &name)]);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case name={name:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: two members outside their own patterns are reported as two entries inside the ONE
/// pair of brackets, each naming its own member's expression and value.
#[tokio::test]
async fn two_members_outside_their_patterns_are_reported_as_one_bracketed_list() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "GET",
        "/v2/list-services?accountId=notanaccount&ouId=ou-ABCD-EFGHIJKL",
        None,
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status} and {body:?}");
    let actual = message(&body);
    let account = entry(ACCOUNT_ID_PATTERN, "notanaccount");
    let ou = entry(OU_ID_PATTERN, "ou-ABCD-EFGHIJKL");
    // The entries come out in the order the validating walk found the members, which is
    // the model's own member order; both orderings are one bracketed list of the same two
    // findings.
    let orderings = [
        report(&[account.clone(), ou.clone()]),
        report(&[ou, account]),
    ];
    assert!(
        orderings.contains(&actual),
        "case two members outside their patterns: expected one of {orderings:?}, got {actual:?}"
    );
}

/// One row of the declining table: a request breaking a constraint of some other class.
struct OtherClassCase {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Option<Value>,
}

/// RULE: only the `@pattern` class is reported that way. A request breaking any other
/// modelled constraint — a `@length` bound, a `@range` bound, an enumerated value set, an
/// absent `@required` member — keeps the framework's own wording, which is what this
/// service's handler-side refusals state too, so one rule is never reported in two voices.
///
/// A refusal MIXING a pattern violation with another class keeps that wording whole as
/// well: a message half in one voice and half in the other is worse than one consistent
/// voice.
#[tokio::test]
async fn a_violation_of_another_constraint_keeps_the_framework_wording() {
    let router = emulator();
    // One character past `EntityDescription`'s `@length` maximum of 500, and holding
    // nothing the name pattern would also object to.
    let too_long_description = "d".repeat(501);

    let cases = vec![
        OtherClassCase {
            label: "a name shorter than its modelled length minimum (also outside its pattern)",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a"})),
        },
        OtherClassCase {
            label: "a description past its modelled length maximum",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"name": "a-well-formed-name", "description": too_long_description})),
        },
        OtherClassCase {
            label: "a required member the request left out",
            method: "POST",
            uri: "/v2/create-system",
            body: Some(json!({"description": "no name at all"})),
        },
        OtherClassCase {
            label: "maxResults below its modelled range",
            method: "GET",
            uri: "/v2/list-services?maxResults=0",
            body: None,
        },
        OtherClassCase {
            label: "maxResults above its modelled range",
            method: "GET",
            uri: "/v2/list-services?maxResults=101",
            body: None,
        },
        OtherClassCase {
            label: "assessmentStatus outside its modelled value set",
            method: "GET",
            uri: "/v2/list-services?assessmentStatus=NOT_A_STATUS",
            body: None,
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.method, case.uri, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} and {body:?}",
            case.label
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case {:?}: expected ValidationException, got {code:?}",
            case.label
        );
        let actual = message(&body);
        assert!(
            actual.contains("validation error") && !actual.starts_with('['),
            "case {:?}: expected the framework's validation wording, got {actual:?}",
            case.label
        );
    }
}

/// RULE: wording a refusal changes nothing about the store — a create refused for a value
/// outside its member's pattern leaves no resource behind, so a later read sees exactly
/// what it saw before.
#[tokio::test]
async fn a_refused_create_leaves_the_store_untouched() {
    let router = emulator();

    // A service that WAS accepted, so the read after the refusals has something to still
    // see: a count that was zero both times would also hold for a store nothing reaches.
    let (status, created) = call(
        &router,
        "POST",
        "/v2/create-service",
        Some(json!({
            "name": "a-well-formed-name",
            "regions": ["us-east-1"],
            "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
        })),
    )
    .await;
    assert_eq!(status, 200, "creating the service to read back: got {created:?}");
    let (status, before) = call(&router, "GET", "/v2/list-services", None).await;
    assert_eq!(status, 200, "listing before the refusals: got {before:?}");
    let names_before = summary_names(&before);
    assert_eq!(
        names_before,
        vec!["a-well-formed-name".to_string()],
        "listing before the refusals: expected the one created service, got {names_before:?}"
    );

    for name in ["bad name has spaces!", "_leading-underscore", "name.with.dots"] {
        let (status, body) = call(
            &router,
            "POST",
            "/v2/create-service",
            Some(json!({
                "name": name,
                "regions": ["us-east-1"],
                "permissionModel": {"invokerRoleName": "ResilienceHubRole"},
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected 400, got {status} and {body:?}"
        );
    }

    let (status, after) = call(&router, "GET", "/v2/list-services", None).await;
    assert_eq!(status, 200, "listing after the refusals: got {after:?}");
    let names_after = summary_names(&after);
    assert_eq!(
        names_after, names_before,
        "case a refused create: expected {names_before:?} still stored, got {names_after:?}"
    );
}

/// The `name` of every service `ListServices` reported, in the order it reported them.
fn summary_names(body: &Value) -> Vec<String> {
    body["serviceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no serviceSummaries array in {body:?}"))
        .iter()
        .map(|summary| {
            summary["name"]
                .as_str()
                .unwrap_or_else(|| panic!("no name in {summary:?}"))
                .to_string()
        })
        .collect()
}
