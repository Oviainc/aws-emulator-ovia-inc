// SPDX-License-Identifier: CC0-1.0

//! What a suite EXPECTS, in one place: how a list setting reads back, and how a
//! body this service refuses at its front door is answered.
//!
//! The harness that drives requests is `crate::wire`; this module holds only the
//! expectation helpers the suites share, so none of them re-derives the rule it is
//! asserting in its own words.

use crate::wire::{build_router, update_customizations};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// What a list setting (`visibleServices` / `visibleRegions`) reads back as after
/// a request supplied `values`: the SAME MEMBERS, in the service's one canonical
/// sequence — ascending — whatever sequence the request used.
///
/// The model fixes no order (`ServiceList`/`RegionsList` are plain lists, no
/// `@uniqueItems`, no sort trait —
/// `smithy_query(query="shape", shape="ServiceList,RegionsList")`), so this is
/// recorded behaviour: the corpus reports these settings ascending. Nothing is
/// de-duplicated, because neither list declares `@uniqueItems`, so a member named
/// twice comes back twice.
///
/// Computed here with a sort written out by hand rather than by calling the
/// emulator's own `state::canonical_members`, so a suite asserts the rule and not
/// the implementation. One helper, so a suite cannot quietly assert a different
/// expectation than its neighbour.
pub fn reported(values: &[&str]) -> Value {
    let mut members: Vec<&str> = values.to_vec();
    // Insertion sort over the supplied members, comparing the identifiers' bytes:
    // a hand-written ascending order, independent of the service's own.
    for position in 1..members.len() {
        let mut at = position;
        while at > 0 && members[at - 1].as_bytes() > members[at].as_bytes() {
            members.swap(at - 1, at);
            at -= 1;
        }
    }
    Value::Array(members.into_iter().map(Value::from).collect())
}

/// [`reported`] for members a case built as owned strings (a generated list, or
/// one read back from `ListServices`).
pub fn reported_owned(values: &[String]) -> Value {
    reported(&values.iter().map(String::as_str).collect::<Vec<&str>>())
}

/// The members of a reported list setting, for an assertion about the ARRAY rather
/// than about one expected document: panics if `body` has no such array.
pub fn members(body: &Value, member: &str) -> Vec<String> {
    body.get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an array at {member} in {body}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("expected string members at {member} in {body}"))
                .to_string()
        })
        .collect()
}

/// The error code this service's front door names for a body it refuses. NOT a
/// shape in this service's model
/// (`smithy_query(query="operation", operation="UpdateAccountCustomizations")`
/// declares `AccessDeniedException`, `InternalServerException`,
/// `ThrottlingException` and `ValidationException`): the refusal happens ahead of
/// the application layer that owns the modelled errors.
pub const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// The body members a front-door refusal must NOT name, from
/// `smithy_query(query="operation", operation="UpdateAccountCustomizations")`.
pub const BODY_MEMBERS: &[&str] = &["accountColor", "visibleServices", "visibleRegions"];

/// A record to prime state with before a refused request, so "nothing was
/// written" is an observable claim rather than the absence of one.
pub fn primed_record() -> Value {
    json!({
        "accountColor": "red",
        "visibleServices": ["s3", "ec2"],
        "visibleRegions": ["eu-west-1", "us-east-1"],
    })
}

/// A refusal's envelope as a suite asserts it: `(status, error-type, `__type`,
/// `message`)`.
pub type Answer = (u16, String, String, String);

/// `(status, error-type, __type, message)` of one response. `error-type` is the
/// `x-amzn-errortype` header restJson1 resolves an error shape from.
pub fn answer(status: u16, error_type: &str, body: &Value) -> Answer {
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

/// Prime [`primed_record`], and answer the record the service reported — what a
/// later read must still report when a request was refused.
pub async fn prime(router: &MultiServiceRouter) -> Value {
    let (status, _, body) = update_customizations(router, primed_record()).await;
    assert_eq!(
        status, 200,
        "priming the record must succeed, got {status} ({body})"
    );
    body
}

/// The answer this service gives a body that READS cleanly into the input shape
/// and breaks a modelled constraint — the reference every body it could not read
/// is held to, computed rather than transcribed, so what a suite asserts is that
/// the two classes are answered alike.
///
/// The body is one `accountColor` the modelled enum does not hold: it is an enum
/// of ten values (`smithy_query(query="shape", shape="AccountColor")`) and this is
/// none of them.
pub async fn reference_answer() -> Answer {
    let router = build_router();
    let (status, error_type, body) =
        update_customizations(&router, json!({"accountColor": "chartreuse"})).await;
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
