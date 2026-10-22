// SPDX-License-Identifier: CC0-1.0

//! How `uxc` refuses an input it will not act on, end to end over the wire.
//!
//! One rule, asserted over a table spanning its class (the shared harness is
//! `crate::wire`): a request BODY that breaks a modelled shape constraint — a
//! value outside an enum, a string outside its `@pattern` or `@length`, a list
//! longer than its `@length` — is refused at the front door with HTTP 400 and the
//! error code `BadRequestException`, and nothing is written. A constraint on a
//! value bound to the request URI (`ListServices`' `@httpQuery` `maxResults` /
//! `nextToken`) is NOT a body rejection and keeps the modelled
//! `ValidationException`.
//!
//! The modelled constraints are the WHOLE admission rule for these inputs: the
//! complement of this rule — that a value they admit is accepted, whatever a
//! catalog happens to hold — is `visible_services_round_trip`'s.

use crate::wire::{
    build_router, call, encode_query_value, get_customizations, legal_region,
    update_customizations,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The error code a body-constraint refusal names. Not a shape in this service's
/// model (`smithy_query(query="operation", operation="UpdateAccountCustomizations")`
/// declares `AccessDeniedException`, `InternalServerException`,
/// `ThrottlingException` and `ValidationException`): the refusal happens at the
/// front door, before the application layer that owns the modelled errors.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The error code every refusal the application layer owns names — the one this
/// service's model declares for an input that fails its constraints.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A record to prime state with before a refused request, so "nothing was
/// written" is an observable claim rather than the absence of one. Its members
/// are identifiers and Region codes legal under their modelled constraints.
fn primed_record() -> Value {
    json!({
        "accountColor": "red",
        "visibleServices": ["s3", "ec2"],
        "visibleRegions": ["eu-west-1", "us-east-1"],
    })
}

/// Prime the record and return it as the state every assertion below compares
/// against.
async fn prime(router: &MultiServiceRouter) -> Value {
    let (status, _, body) = update_customizations(router, primed_record()).await;
    assert_eq!(
        status, 200,
        "priming the record must succeed, got {status} ({body})"
    );
    body
}

/// The `message` of an error body, for an assertion's failure text.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// A body that breaks a modelled shape constraint
// ---------------------------------------------------------------------------

/// Every class of shape violation reachable in this operation's body is refused
/// with 400 `BadRequestException`, and the previously persisted record survives
/// unchanged — then a valid update still succeeds.
///
/// The rows differ along which constraint is broken (enum value set, string
/// `@pattern`, string `@length` at both ends, list `@length`), which member
/// carries it, the JSON type of the value, and the position of the offending
/// element in the list — which is what the rule generalises over. Bounds come
/// from `smithy_query(query="shape", shape="AccountColor,ServiceList,RegionsList")`:
/// `AccountColor` is an enum of ten values; `Service` is `@length(1, 64)`
/// `@pattern ^[a-z0-9]+(-[a-z0-9]+)*$` inside a list of max 500; `Region` is
/// `@pattern ^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$` inside a list of max 100.
#[tokio::test]
async fn a_body_breaking_a_modelled_constraint_is_refused_as_bad_request() {
    let long_service = "a".repeat(65);
    let over_long_services: Vec<String> = (0..501)
        .map(|n| if n % 2 == 0 { "s3".to_string() } else { "ec2".to_string() })
        .collect();
    let over_long_regions: Vec<String> = (0..101).map(legal_region).collect();

    let cases: Vec<(&str, Value)> = vec![
        // accountColor outside the modelled value set.
        ("colour not in the enum", json!({"accountColor": "chartreuse"})),
        ("colour in the wrong case", json!({"accountColor": "Teal"})),
        ("empty colour", json!({"accountColor": ""})),
        ("colour with a trailing space", json!({"accountColor": "teal "})),
        ("colour that is an enum NAME, not a value", json!({"accountColor": "DARK_BLUE"})),
        // A service identifier outside the member @pattern.
        (
            "service with an underscore and a bang",
            json!({"visibleServices": ["Invalid_Service!"]}),
        ),
        ("upper-case service", json!({"visibleServices": ["S3"]})),
        ("service with a leading hyphen", json!({"visibleServices": ["-ec2"]})),
        ("service with a trailing hyphen", json!({"visibleServices": ["ec2-"]})),
        ("service with a doubled hyphen", json!({"visibleServices": ["ec2--x"]})),
        ("service with a dot", json!({"visibleServices": ["ec2.amazonaws.com"]})),
        (
            "the offending element is not the first",
            json!({"visibleServices": ["s3", "Invalid_Service!", "ec2"]}),
        ),
        // A service identifier outside the member @length(1, 64).
        ("empty service", json!({"visibleServices": [""]})),
        (
            "service one character past the 64-char maximum",
            json!({"visibleServices": [long_service]}),
        ),
        // A list one element past its own @length maximum.
        ("501 services", json!({"visibleServices": over_long_services})),
        ("101 Regions", json!({"visibleRegions": over_long_regions})),
        // A Region code outside the member @pattern.
        ("upper-case Region", json!({"visibleRegions": ["US-EAST-1"]})),
        ("Region without hyphens", json!({"visibleRegions": ["useast1"]})),
        ("Region ending in zero", json!({"visibleRegions": ["us-east-0"]})),
        (
            "one bad Region beside two good ones",
            json!({"visibleRegions": ["eu-west-1", "nope", "us-east-1"]}),
        ),
        // Several violated constraints in one body.
        (
            "a bad colour and a bad service together",
            json!({"accountColor": "chartreuse", "visibleServices": ["S3"]}),
        ),
    ];

    for (case, body) in cases {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) = update_customizations(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({answered})"
        );
        assert_eq!(
            error_type, BAD_REQUEST_EXCEPTION,
            "case {case:?}: expected error code {BAD_REQUEST_EXCEPTION}, got {error_type:?} (message {:?})",
            message(&answered)
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched, primed,
            "case {case:?}: a refused body must leave the record untouched, got {fetched:?}"
        );

        // And the account is not wedged: a valid update still succeeds.
        let (status, _, recovered) =
            update_customizations(&router, json!({"accountColor": "green"})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a valid update after a refusal must succeed, got {status} ({recovered})"
        );
        assert_eq!(
            recovered.get("accountColor").and_then(Value::as_str),
            Some("green"),
            "case {case:?}: the valid update must take effect, got {recovered:?}"
        );
    }
}

/// Every value AT a modelled bound is accepted, so the refusals above are the
/// bound and not an off-by-one: each colour the enum declares, a one-character
/// service identifier, one of exactly 64 characters, and the shortest legal
/// Region code.
///
/// An identifier inside `Service`'s `@length(1, 64)` and `@pattern` is accepted
/// and read back whatever else it resembles: the modelled constraints are the
/// whole admission rule for this member.
#[tokio::test]
async fn values_at_a_modelled_bound_pass_the_body_check() {
    // The ten values of `AccountColor`
    // (`smithy_query(query="shape", shape="AccountColor")`).
    let colors = [
        "none", "pink", "purple", "darkBlue", "lightBlue", "teal", "green", "yellow", "orange",
        "red",
    ];
    for color in colors {
        let router = build_router();
        let (status, error_type, body) =
            update_customizations(&router, json!({"accountColor": color})).await;
        assert_eq!(
            status, 200,
            "case {color:?}: a modelled colour must be accepted, got {status} ({body}) [{error_type}]"
        );
        assert_eq!(
            body.get("accountColor").and_then(Value::as_str),
            Some(color),
            "case {color:?}: the accepted colour must be answered back, got {body:?}"
        );
    }

    // A one-character and a 64-character identifier are both inside
    // `Service`'s `@length(1, 64)`, so both are accepted and read back.
    let at_length_bound = ["a".to_string(), "a".repeat(64)];
    for service in at_length_bound {
        let router = build_router();
        let (status, error_type, body) =
            update_customizations(&router, json!({"visibleServices": [service.clone()]})).await;
        assert_eq!(
            status, 200,
            "case {} chars: an identifier inside @length(1, 64) must be accepted, got {status} ({body}) [{error_type}]",
            service.len()
        );
        assert_eq!(
            body.get("visibleServices"),
            Some(&json!([service.clone()])),
            "case {} chars: the accepted identifier must be read back, got {body:?}",
            service.len()
        );
    }

    // The shortest legal Region code under
    // `^[a-z]{2}(-[a-z]{1,10}){1,2}-[1-9]$`, and the longest segment it admits.
    let router = build_router();
    let (status, _, body) = update_customizations(
        &router,
        json!({"visibleRegions": ["us-a-1", "us-abcdefghij-1", "us-gov-west-1"]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "Region codes at the pattern's bounds must be accepted, got {status} ({body})"
    );
}

/// The contrast that makes the rule above a rule about the BODY: a constraint
/// broken by a value bound to the request URI keeps the modelled
/// `ValidationException`.
///
/// `ListServices` binds both of its inputs with `@httpQuery`
/// (`smithy_query(query="operation", operation="ListServices")`), so no refusal
/// of that operation is ever a body refusal.
#[tokio::test]
async fn a_uri_bound_value_outside_its_constraint_keeps_the_modelled_code() {
    let router = build_router();
    let cases: Vec<(&str, String)> = vec![
        ("maxResults below its @range", "maxResults=0".to_string()),
        ("maxResults above its @range", "maxResults=1001".to_string()),
        ("negative maxResults", "maxResults=-1".to_string()),
        (
            "nextToken shorter than its @length minimum",
            "nextToken=abc".to_string(),
        ),
        (
            "nextToken outside its @pattern",
            format!("nextToken={}", encode_query_value(&"!".repeat(40))),
        ),
    ];

    for (case, query) in cases {
        let (status, error_type, body) =
            call(&router, "GET", &format!("/v1/services?{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({body})"
        );
        assert_eq!(
            error_type, VALIDATION_EXCEPTION,
            "case {case:?}: a URI-bound refusal must keep {VALIDATION_EXCEPTION}, got {error_type:?} (message {:?})",
            message(&body)
        );
    }
}

// ---------------------------------------------------------------------------
// Resetting a list setting
// ---------------------------------------------------------------------------

/// Resetting `visibleServices` is refused by nothing: an empty list and an
/// omitted member both go through, and an empty list is a configured, empty
/// setting rather than an unconfigured one.
#[tokio::test]
async fn resetting_the_service_list_is_not_refused() {
    let router = build_router();
    prime(&router).await;

    let (status, _, body) = update_customizations(&router, json!({"visibleServices": []})).await;
    assert_eq!(
        status, 200,
        "an empty visibleServices must be accepted, got {status} ({body})"
    );
    assert_eq!(
        body.get("visibleServices"),
        Some(&json!([])),
        "an empty visibleServices must be stored as empty, got {body:?}"
    );

    let (status, _, body) = update_customizations(&router, json!({"accountColor": "pink"})).await;
    assert_eq!(
        status, 200,
        "an update naming no service list must be accepted, got {status} ({body})"
    );
}
