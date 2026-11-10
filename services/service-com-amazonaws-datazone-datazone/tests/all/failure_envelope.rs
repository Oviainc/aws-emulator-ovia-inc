// SPDX-License-Identifier: CC0-1.0

//! What EVERY failing DataZone answer carries in its BODY, whichever renderer wrote it.
//!
//! RULE: this service speaks `restJson1` (`smithy_query(query="service_info")` reports
//! protocols `[restJson1]`), and a `restJson1` failure names its error SHAPE in the
//! `x-amzn-errortype` response header, so the body of a failure carries the error shape's
//! own members and nothing else — no `__type` discriminator (that member belongs to the
//! AWS JSON protocol family, which conveys the shape in the body because it has no such
//! header), and no member written as the empty string (on the wire that is an absent
//! member, not a present empty one).
//!
//! The rows below differ along the one thing that decides WHICH renderer writes the body:
//! a failure a HANDLER raised, serialized by this service's own protocol, and a failure
//! the request never reached a handler for because it named no modelled operation,
//! rendered by the routing seam. The rule is one rule, so one table asserts it over both,
//! and it holds for the error classes either seam can answer with.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::Emulator;
use serde_json::{json, Value};

/// The member a JSON error envelope of the AWS JSON protocol family names its error shape
/// in — the one `restJson1` names in a header instead, so no body here may carry it.
const SHAPE_DISCRIMINATOR: &str = "__type";

/// A domain identifier no domain in the store carries, satisfying `DomainId`'s modelled
/// pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") so that the
/// request reaches a handler rather than being refused before one runs.
const UNSERVED_DOMAIN: &str = "dzd_noenvelopedomain0";

/// One failing call: how it is addressed, and the body members its answer must carry.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    members: &'static [&'static str],
}

/// The body members of an answer, sorted, so an assertion names exactly what went on the
/// wire.
fn members(body: &Value) -> Vec<String> {
    match body.as_object() {
        Some(object) => {
            let mut names: Vec<String> = object.keys().cloned().collect();
            names.sort();
            names
        }
        None => vec![format!("<not an object: {body}>")],
    }
}

#[tokio::test]
async fn every_failure_body_carries_the_error_shapes_own_members_whichever_renderer_wrote_it() {
    let dz = Emulator::new();

    let rows = vec![
        // Rendered by the ROUTING seam: the request reached this service, in its
        // protocol, but named no modelled operation, so no handler ran.
        Row {
            case: "a path no operation is bound to",
            method: "GET",
            uri: "/v2/not-a-datazone-resource".to_string(),
            body: None,
            members: &["message"],
        },
        Row {
            case: "a method no operation binds on a bound path",
            method: "PUT",
            uri: "/v2/domains".to_string(),
            body: Some(json!({ "name": "envelope domain" })),
            members: &["message"],
        },
        Row {
            case: "a path one segment longer than a bound one",
            method: "GET",
            uri: format!("/v2/domains/{UNSERVED_DOMAIN}/projects/p0/deeper"),
            body: None,
            members: &["message"],
        },
        Row {
            case: "the root path",
            method: "GET",
            uri: "/".to_string(),
            body: None,
            members: &["message"],
        },
        // Serialized by this service's own protocol: a handler ran and raised a
        // modelled error.
        Row {
            case: "a miss a handler raised, naming the domain it could not resolve",
            method: "POST",
            uri: format!("/v2/domains/{UNSERVED_DOMAIN}/projects"),
            body: Some(json!({ "name": "project in no domain" })),
            members: &["message"],
        },
        Row {
            case: "a denial a handler raised, whose own identifier IS the domain",
            method: "DELETE",
            uri: format!("/v2/domains/{UNSERVED_DOMAIN}"),
            body: None,
            members: &[],
        },
    ];

    for Row {
        case,
        method,
        uri,
        body,
        members: expected,
    } in rows
    {
        let answer = dz.call(method, &uri, body).await;
        assert!(
            answer.status >= 400,
            "case {case:?}: expected a failure, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.body.get(SHAPE_DISCRIMINATOR).is_none(),
            "case {case:?}: the error shape travels in the header, got {:?}",
            answer.body
        );
        // `error_code` is read from `x-amzn-errortype`, falling back to the body's
        // discriminator — which the assertion above proved absent — so a code here came
        // from the header this protocol resolves the shape from.
        assert!(
            !answer.error_code.is_empty(),
            "case {case:?}: expected the error shape in the header, got {:?} {:?}",
            answer.error_code,
            answer.body
        );
        let on_the_wire = members(&answer.body);
        assert_eq!(
            on_the_wire,
            expected
                .iter()
                .map(|member| (*member).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly these members, got {:?}",
            answer.body
        );
        for member in &on_the_wire {
            let value = answer.body.get(member).and_then(Value::as_str);
            assert!(
                value.is_none_or(|text| !text.is_empty()),
                "case {case:?}: {member:?} must be absent rather than empty, got {:?}",
                answer.body
            );
        }
    }

    // A SUCCESS is not an error envelope at all: it keeps every member the model gives
    // it, including one the rule above would have dropped from a failure.
    let created = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "envelope" })))
        .await;
    assert_eq!(
        created.status, 201,
        "CreateDomain: expected 201, got {} {:?}",
        created.status, created.body
    );
    assert!(
        created.body.get("id").is_some(),
        "a success must carry its own members, got {:?}",
        created.body
    );
}
