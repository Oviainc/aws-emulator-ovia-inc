// SPDX-License-Identifier: CC0-1.0

//! The JSON TYPE of a request document decides whether this service will act on
//! it, end to end over the wire (the shared harness is `crate::wire`).
//!
//! One rule, asserted over a table spanning its class: an operation that carries
//! members in the request body is read from a JSON OBJECT, so a well-formed
//! document of any other JSON type declares no member of that input and is
//! refused with HTTP 400 and the error code `BadRequestException` — the same
//! answer a document that is not well-formed JSON earns
//! (`unreadable_body_refusal`) — with nothing written. A document that IS an
//! object, the empty object included, and a request with no body at all are the
//! other side of that bound: they set nothing, which is a request this service
//! serves.
//!
//! The refused rows differ along the JSON type sent (null, boolean, number,
//! string, array), along the VALUE within a type (integer, fraction, exponent,
//! negative; empty and non-empty string; empty and populated array), and along
//! whether the document is surrounded by insignificant whitespace — which is what
//! the rule generalises over. `null` is the row the codec itself accepts, reading
//! it as a structure with every member absent, so it is the one the front door has
//! to place on its own.
//!
//! Each refused row's answer is compared against the answer to a body that READS
//! cleanly and breaks a modelled constraint, computed rather than transcribed
//! (`crate::support`'s `reference_answer`, the one every suite asserting this
//! service's front-door rule is held to): the rule says a body the service will
//! not act on is answered in one channel.

use crate::support::{answer, prime, reference_answer, BODY_MEMBERS};
use crate::wire::{
    build_router, call_bytes, get_customizations, update_customizations,
    update_customizations_bytes,
};
use serde_json::{json, Value};

/// A well-formed JSON document whose type is not an object is refused as a body
/// this service will not act on, and writes nothing.
#[tokio::test]
async fn a_well_formed_document_that_is_not_an_object_is_refused_as_a_bad_request_body() {
    let reference = reference_answer().await;

    let cases: Vec<(&str, &[u8])> = vec![
        // The one document the JSON codec reads rather than failing on.
        ("the literal null", b"null"),
        ("the literal null inside whitespace", b"  null\n"),
        // Booleans.
        ("the literal true", b"true"),
        ("the literal false", b"false"),
        // Numbers, across the forms JSON writes them in.
        ("a positive integer", b"17"),
        ("zero", b"0"),
        ("a negative integer", b"-1"),
        ("a fraction", b"1.5"),
        ("an exponent", b"1e3"),
        // Strings, empty and not.
        ("the empty string", br#""""#),
        ("a non-empty string", br#""red""#),
        // Arrays, including ones whose elements are the objects this input is
        // read from — the TOP-LEVEL type is what the rule is about.
        ("the empty array", b"[]"),
        ("an array of strings", br#"["red"]"#),
        ("an array of objects", br#"[{"accountColor":"red"}]"#),
        ("an array inside whitespace", b" [ ] "),
    ];

    for (case, raw) in cases {
        let router = build_router();
        let primed = prime(&router).await;

        let (status, error_type, answered) = update_customizations_bytes(&router, raw).await;
        let got = answer(status, &error_type, &answered);
        assert_eq!(
            got, reference,
            "case {case:?} ({:?}): a document that is not a JSON object must be answered exactly \
             as a readable body that breaks a constraint ({reference:?}), got {got:?}",
            String::from_utf8_lossy(raw)
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

/// The other side of the bound: a document that IS a JSON object, and a request
/// with no body, are served — the refusal is about the document's TYPE, not about
/// a request that sets nothing — and what the object said is what the record then
/// holds, read back through `GetAccountCustomizations`.
///
/// The rows differ along how little the object carries (no member, a member with a
/// value, a member carried as `null`) and along whitespace around it. Each row's
/// expectation is derived from the record the service itself reported after
/// priming, so what is asserted is the DELTA the object asked for.
#[tokio::test]
async fn a_json_object_and_an_absent_body_are_served() {
    /// The record a row expects afterwards, from the one the service reported
    /// after priming.
    type Expected = fn(&Value) -> Value;

    fn unchanged(primed: &Value) -> Value {
        primed.clone()
    }

    fn recoloured(primed: &Value) -> Value {
        let mut expected = primed.clone();
        expected["accountColor"] = json!("orange");
        expected
    }

    /// A list carried as `null` returns to unconfigured, which
    /// `GetAccountCustomizations` reports as an ABSENT member
    /// (`smithy_query(query="doc", shape="GetAccountCustomizations")`).
    fn services_reset(primed: &Value) -> Value {
        let mut expected = primed.clone();
        expected
            .as_object_mut()
            .expect("the record is a JSON object")
            .remove("visibleServices");
        expected
    }

    let cases: Vec<(&str, &[u8], Expected)> = vec![
        ("the empty object", b"{}", unchanged),
        ("the empty object inside whitespace", b" {}\n", unchanged),
        (
            "an object naming a setting",
            br#"{"accountColor":"orange"}"#,
            recoloured,
        ),
        (
            "an object carrying a member as null",
            br#"{"visibleServices":null}"#,
            services_reset,
        ),
        ("no body at all", b"", unchanged),
    ];

    for (case, raw, expected) in cases {
        let router = build_router();
        let primed = prime(&router).await;
        let want = expected(&primed);

        let (status, error_type, answered) = update_customizations_bytes(&router, raw).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a JSON object body must be served, got {status} ({answered}) \
             [{error_type}]"
        );
        assert_eq!(
            answered, want,
            "case {case:?}: the update must answer the record the object asked for ({want}), got \
             {answered}"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched, want,
            "case {case:?}: a later read must report what the object stored ({want}), got \
             {fetched}"
        );
    }
}

/// The rule is about the document the INPUT SHAPE is read from, so it reaches only
/// the operations that carry a member in the request body.
///
/// `GetAccountCustomizations` declares no input member and `ListServices` binds
/// both of its inputs with `@httpQuery`
/// (`smithy_query(query="operation", operation="GetAccountCustomizations,ListServices")`),
/// so neither reads a body at all and neither has a body document to refuse.
#[tokio::test]
async fn an_operation_that_reads_no_body_does_not_refuse_one() {
    let router = build_router();
    prime(&router).await;

    let cases = [
        ("GetAccountCustomizations", "/v1/account-customizations"),
        ("ListServices", "/v1/services"),
    ];

    for (case, uri) in cases {
        let (status, error_type, body) = call_bytes(&router, "GET", uri, b"null").await;
        assert_eq!(
            status, 200,
            "case {case:?}: an operation that carries no member in the body has no body document \
             to refuse, got {status} ({body}) [{error_type}]"
        );
    }
}
