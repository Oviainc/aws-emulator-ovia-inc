// SPDX-License-Identifier: CC0-1.0

//! A request body this service cannot READ earns the same answer as one it read
//! and rejected, end to end over the wire (the shared harness is `crate::wire`).
//!
//! One rule, asserted over a table spanning its class: a body the front door will
//! not act on is refused with HTTP 400 and the error code `BadRequestException` in
//! both `x-amzn-errortype` and the body's `__type`, with a message that names no
//! member — whether a value breaks a modelled constraint
//! (`body_validation_refusals`' rows), a member carries a JSON type that is not
//! the type the model gives it, or the document is not well-formed JSON at all.
//! Nothing is written either way. A value bound to the request URI is NOT a body
//! rejection and is not answered in that channel.
//!
//! Each row's answer is compared against the answer to a body that READS cleanly
//! and breaks a modelled constraint, computed here rather than transcribed: what
//! the rule says is that the two classes are answered alike, so the readable
//! refusal is the reference every unreadable one is held to.

use crate::wire::{
    build_router, call, get_customizations, update_customizations, update_customizations_bytes,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The error code this service's front door names for a body it refuses. Not a
/// shape in this service's model
/// (`smithy_query(query="operation", operation="UpdateAccountCustomizations")`
/// declares `AccessDeniedException`, `InternalServerException`,
/// `ThrottlingException` and `ValidationException`): the refusal happens ahead of
/// the application layer that owns the modelled errors.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The body members a front-door refusal must NOT name, from
/// `smithy_query(query="operation", operation="UpdateAccountCustomizations")`.
const BODY_MEMBERS: &[&str] = &["accountColor", "visibleServices", "visibleRegions"];

/// A body that READS cleanly into the input shape and breaks a modelled
/// constraint: `accountColor` is an enum of ten values
/// (`smithy_query(query="shape", shape="AccountColor")`) and this is none of them.
fn readable_but_refused() -> Value {
    json!({"accountColor": "chartreuse"})
}

/// A record to prime state with before a refused request, so "nothing was
/// written" is an observable claim rather than the absence of one.
fn primed_record() -> Value {
    json!({
        "accountColor": "red",
        "visibleServices": ["s3", "ec2"],
        "visibleRegions": ["eu-west-1", "us-east-1"],
    })
}

/// The refusal's envelope as a row asserts it: `(status, error-type, `__type`,
/// `message`)`.
type Answer = (u16, String, String, String);

/// `(status, error-type, __type, message)` of one response.
fn answer(status: u16, error_type: &str, body: &Value) -> Answer {
    (
        status,
        error_type.to_string(),
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        body.get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// Prime the record, so every row below can show it survived.
async fn prime(router: &MultiServiceRouter) -> Value {
    let (status, _, body) = update_customizations(router, primed_record()).await;
    assert_eq!(
        status, 200,
        "priming the record must succeed, got {status} ({body})"
    );
    body
}

/// The answer this service gives a body that reads cleanly and breaks a modelled
/// constraint — the reference every unreadable body is held to.
async fn reference_answer() -> Answer {
    let router = build_router();
    let (status, error_type, body) = update_customizations(&router, readable_but_refused()).await;
    let reference = answer(status, &error_type, &body);
    assert_eq!(
        reference.0, 400,
        "a body breaking a modelled constraint must be refused with HTTP 400, got {body}"
    );
    assert_eq!(
        reference.1, BAD_REQUEST_EXCEPTION,
        "a body breaking a modelled constraint must name {BAD_REQUEST_EXCEPTION}, got {body}"
    );
    reference
}

/// A body whose member carries a JSON type the model does not give it never
/// becomes an input, and is refused exactly as a readable body that breaks a
/// constraint is.
///
/// The rows differ along which member carries the mismatch, the JSON type sent
/// (string, number, boolean, object, array, null-bearing array) and the DEPTH at
/// which it is wrong (the member's own value, or one element of its list) — which
/// is what the rule generalises over. The modelled types come from
/// `smithy_query(query="operation", operation="UpdateAccountCustomizations")`:
/// `accountColor` is an enum (a string on the wire), `visibleServices` and
/// `visibleRegions` are lists of strings.
#[tokio::test]
async fn a_member_of_the_wrong_json_type_is_refused_as_a_bad_request_body() {
    let reference = reference_answer().await;

    let cases: Vec<(&str, Value)> = vec![
        // A scalar where the model declares a list.
        ("a string where a list is modelled", json!({"visibleServices": "s3"})),
        ("an object where a list is modelled", json!({"visibleServices": {"a": 1}})),
        ("a number where a list is modelled", json!({"visibleRegions": 1})),
        ("a boolean where a list is modelled", json!({"visibleRegions": true})),
        // A non-string element inside a list of strings.
        ("numbers inside a list of strings", json!({"visibleRegions": [1, 2]})),
        ("a boolean inside a list of strings", json!({"visibleServices": [true]})),
        ("an object inside a list of strings", json!({"visibleServices": [{"name": "s3"}]})),
        ("a list inside a list of strings", json!({"visibleRegions": [["us-east-1"]]})),
        (
            "the mistyped element is not the first",
            json!({"visibleServices": ["s3", 7, "ec2"]}),
        ),
        // A non-string where the model declares the enum.
        ("a number where the enum is modelled", json!({"accountColor": 7})),
        ("a boolean where the enum is modelled", json!({"accountColor": true})),
        ("a list where the enum is modelled", json!({"accountColor": ["red"]})),
        ("an object where the enum is modelled", json!({"accountColor": {"value": "red"}})),
        // One mistyped member beside a value that would have been accepted.
        (
            "a mistyped member beside a good one",
            json!({"accountColor": 7, "visibleRegions": ["us-east-1"]}),
        ),
    ];

    for (case, body) in cases {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) = update_customizations(&router, body.clone()).await;
        let got = answer(status, &error_type, &answered);
        assert_eq!(
            got, reference,
            "case {case:?}: a body the modelled types cannot be read from must be answered \
             exactly as a readable body that breaks a constraint ({reference:?}), got {got:?}"
        );
        for member in BODY_MEMBERS {
            assert!(
                !got.3.contains(member),
                "case {case:?}: a front-door refusal names no member, but its message names \
                 {member:?}: {:?}",
                got.3
            );
        }

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
    }
}

/// A request document that is not well-formed JSON — or is well-formed and is not
/// a JSON object — is refused the same way: it is a body the service will not act
/// on, so the front door owns it.
///
/// The rows differ along HOW the document fails to be a JSON object the input
/// shape can be read from: truncated at several depths, a stray delimiter, a
/// trailing comma, an unquoted key, a bare scalar, an array, and text that is not
/// JSON at all.
#[tokio::test]
async fn a_document_that_is_not_a_readable_json_object_is_refused_as_a_bad_request_body() {
    let reference = reference_answer().await;

    let cases: [(&str, &[u8]); 10] = [
        ("truncated after a member name", br#"{"accountColor": "#),
        ("truncated after a member value", br#"{"accountColor": "red""#),
        ("truncated inside a list", br#"{"visibleRegions": ["us-east-1""#),
        ("a trailing comma", br#"{"accountColor": "red",}"#),
        ("a stray delimiter", b"{,}"),
        ("an unquoted member name", br#"{accountColor: "red"}"#),
        ("a bare string", br#""red""#),
        ("a bare number", b"17"),
        ("a JSON array", br#"["red"]"#),
        ("text that is not JSON", b"not json at all"),
    ];

    for (case, raw) in cases {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) = update_customizations_bytes(&router, raw).await;
        let got = answer(status, &error_type, &answered);
        assert_eq!(
            got, reference,
            "case {case:?} ({:?}): a body that is not a readable JSON object must be answered \
             exactly as a readable body that breaks a constraint ({reference:?}), got {got:?}",
            String::from_utf8_lossy(raw)
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched, primed,
            "case {case:?}: a refused body must leave the record untouched, got {fetched:?}"
        );
    }
}

/// The two bodies a well-formed empty request carries are NOT refusals: an empty
/// document and no body at all both update nothing and answer the record.
///
/// This is the other side of the bound above — the rule refuses a body that
/// cannot be READ, not every body that sets nothing — and it is what keeps the
/// refusal from swallowing the request an SDK sends for an update with no member.
#[tokio::test]
async fn an_empty_body_is_not_an_unreadable_one() {
    for (case, raw) in [("an empty JSON object", &b"{}"[..]), ("no body at all", &b""[..])] {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) = update_customizations_bytes(&router, raw).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a request that sets nothing must be accepted, got {status} \
             ({answered}) [{error_type}]"
        );
        assert_eq!(
            answered, primed,
            "case {case:?}: a request that sets nothing must answer the stored record, got \
             {answered:?}"
        );
    }
}

/// The contrast that makes the rule a rule about the BODY: a value bound to the
/// request URI that the codec cannot read is not a body rejection, so it is not
/// answered in the front door's channel.
///
/// `ListServices` binds both of its inputs with `@httpQuery`
/// (`smithy_query(query="operation", operation="ListServices")`) and carries
/// nothing in the body, so no refusal of that operation is ever a body refusal.
#[tokio::test]
async fn a_uri_bound_value_the_codec_cannot_read_is_not_a_body_refusal() {
    let router = build_router();
    // Rows differ along the JSON/text type sent for an `integer` and for a
    // `string` query parameter.
    let cases = [
        ("maxResults as a word", "maxResults=abc"),
        ("maxResults as a decimal", "maxResults=1.5"),
        ("maxResults as a boolean", "maxResults=true"),
        ("maxResults empty", "maxResults="),
    ];

    for (case, query) in cases {
        let (status, error_type, body) =
            call(&router, "GET", &format!("/v1/services?{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a URI-bound value the codec cannot read must be refused, got \
             {status} ({body})"
        );
        assert_ne!(
            error_type, BAD_REQUEST_EXCEPTION,
            "case {case:?}: a URI-bound value is not a body rejection, so it must not answer \
             the front door's code, got {body}"
        );
    }
}
