// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE a body refusal arrives in, which a client reads before anything
//! else: the error code in the `x-amzn-errortype` header and the SAME code in
//! the body's `__type`, with a message that names no member.
//!
//! Which bodies are refused, which constraint classes those are, and the
//! contrast with a URI-bound value are the subject of `body_validation_refusals`;
//! this suite asserts only that the parts of one answer agree, on a couple of
//! rows from different constraint classes. The harness is `crate::wire`.

use crate::wire::{build_router, legal_region, update_customizations};
use serde_json::{json, Value};

/// The error code a body-constraint refusal names. The front door refuses the
/// body as a whole, so the message names no member — a per-member clause belongs
/// to the refusals the application layer owns.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The body members a refusal must NOT name, from
/// `smithy_query(query="operation", operation="UpdateAccountCustomizations")`.
const BODY_MEMBERS: &[&str] = &["accountColor", "visibleServices", "visibleRegions"];

/// One row per refusal path into the front door — a member value outside its
/// constraint, and a list outside its own `@length` — each answering with the
/// header code, the same code in the body, and a member-free message.
#[tokio::test]
async fn a_body_refusal_carries_one_code_in_the_header_and_the_body() {
    let over_bound_regions: Vec<String> = (0..101).map(legal_region).collect();
    let cases: Vec<(&str, Value)> = vec![
        ("colour outside the enum", json!({"accountColor": "chartreuse"})),
        (
            "service outside the member pattern",
            json!({"visibleServices": ["Invalid_Service!"]}),
        ),
        (
            "Region list past its own length maximum",
            json!({"visibleRegions": over_bound_regions}),
        ),
    ];

    for (case, body) in cases {
        let router = build_router();
        let (status, error_type, answered) = update_customizations(&router, body).await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({answered})"
        );
        assert_eq!(
            error_type, BAD_REQUEST_EXCEPTION,
            "case {case:?}: expected the {BAD_REQUEST_EXCEPTION} error code in the header, got {error_type:?} ({answered})"
        );
        assert_eq!(
            answered.get("__type").and_then(Value::as_str),
            Some(BAD_REQUEST_EXCEPTION),
            "case {case:?}: the body's __type must carry the same code as the header, got {answered:?}"
        );

        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no message in {answered}"));
        assert!(
            !message.is_empty(),
            "case {case:?}: a refusal must say something, got {answered:?}"
        );
        for member in BODY_MEMBERS {
            assert!(
                !message.contains(member),
                "case {case:?}: a front-door refusal names no member, but its message names {member:?}: {message:?}"
            );
        }
    }
}
