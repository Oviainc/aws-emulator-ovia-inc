// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the framework refused at the boundary — a value
//! outside a constraint the model declares, or a required member that is absent — end to
//! end over HTTP through the router this service mounts in production.
//!
//! The rule these cases assert: a refusal carries an error code this service MODELS, and
//! the one it models for a bad request is `InvalidRequestException` (HTTP 400) — the
//! client-input error its model binds to every operation
//! (`smithy_query(query="digest")`: `common_errors` is InternalFailureException /
//! InvalidRequestException / ThrottlingException). `ValidationException` is not a shape
//! this service returns at all, so it must appear in no refusal: a client's
//! `except InvalidRequestException`, and an SDK's modelled-error dispatch, would never
//! match it. The MESSAGE is this data plane's one terse front-door sentence
//! (`state::BAD_REQUEST`), the same for every member of every operation and carrying
//! nothing of the request: not the member at fault, not the bound it crossed, and not the
//! value the caller sent.
//!
//! The table spans what the rule generalises over — which operation, which member, how
//! the member is bound (path label, query parameter), and which kind of constraint
//! (length, pattern, range) — with the accepted row at each published bound beside the
//! refused one past it. The recorded case that filed the issue (a 65-character
//! `shadowName`) is one row among them.

use crate::shadow_and_retained_message::{Answer, Harness};
use serde_json::{json, Value};

/// A valid shadow state document, so no row is refused for its body.
fn state_document() -> Vec<u8> {
    serde_json::to_vec(&json!({"state": {"reported": {"ok": true}}})).expect("serializable")
}

/// The `message` of a response body.
fn message(answer: &Answer) -> String {
    answer
        .json()
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One request that breaks a constraint the model declares.
struct RefusedCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Vec<u8>,
    /// The member at fault, which the refusal must NOT name — the front-door sentence
    /// reflects nothing of the request back.
    member: &'static str,
    /// The value the request carried for that member, which the refusal must not echo
    /// either; `None` for a row whose value is too short to tell from ordinary text.
    value: Option<String>,
}

/// One request that is INSIDE every constraint, at the bound the refused row crosses.
struct AcceptedCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Vec<u8>,
}

fn refused_cases() -> Vec<RefusedCase> {
    vec![
        RefusedCase {
            // The recorded case: one character past `shadowName`'s modelled maximum.
            name: "a shadowName one character past its modelled maximum",
            method: "POST",
            uri: format!("/things/refusalThing/shadow?name={}", "s".repeat(65)),
            body: state_document(),
            member: "shadowName",
            value: Some("s".repeat(65)),
        },
        RefusedCase {
            name: "a shadowName far past its modelled maximum",
            method: "POST",
            uri: format!("/things/refusalThing/shadow?name={}", "s".repeat(400)),
            body: state_document(),
            member: "shadowName",
            value: Some("s".repeat(400)),
        },
        RefusedCase {
            name: "a shadowName outside its modelled pattern",
            method: "POST",
            uri: "/things/refusalThing/shadow?name=not%20a%20shadow%21".to_string(),
            body: state_document(),
            member: "shadowName",
            value: Some("not a shadow!".to_string()),
        },
        RefusedCase {
            name: "a shadowName on a READ, outside its modelled pattern",
            method: "GET",
            uri: "/things/refusalThing/shadow?name=not%20a%20shadow%21".to_string(),
            body: Vec::new(),
            member: "shadowName",
            value: Some("not a shadow!".to_string()),
        },
        RefusedCase {
            name: "a thingName one character past its modelled maximum",
            method: "POST",
            uri: format!("/things/{}/shadow", "t".repeat(129)),
            body: state_document(),
            member: "thingName",
            value: Some("t".repeat(129)),
        },
        RefusedCase {
            name: "a qos above its modelled range",
            method: "POST",
            uri: "/topics/refusal-topic?qos=2&retain=false".to_string(),
            body: b"x".to_vec(),
            member: "qos",
            // A one-character value is not distinguishable from ordinary text, so only
            // the member name is asserted absent for this row.
            value: None,
        },
        RefusedCase {
            name: "a qos below its modelled range",
            method: "POST",
            uri: "/topics/refusal-topic?qos=-1&retain=false".to_string(),
            body: b"x".to_vec(),
            member: "qos",
            value: None,
        },
        RefusedCase {
            name: "a maxResults above its modelled range",
            method: "GET",
            uri: "/retainedMessage?maxResults=201".to_string(),
            body: Vec::new(),
            member: "maxResults",
            value: Some("201".to_string()),
        },
        RefusedCase {
            name: "a pageSize above its modelled range",
            method: "GET",
            uri: "/api/things/shadow/ListNamedShadowsForThing/refusalThing?pageSize=101"
                .to_string(),
            body: Vec::new(),
            member: "pageSize",
            value: Some("101".to_string()),
        },
        RefusedCase {
            name: "a pageSize below its modelled range",
            method: "GET",
            uri: "/api/things/shadow/ListNamedShadowsForThing/refusalThing?pageSize=0".to_string(),
            body: Vec::new(),
            member: "pageSize",
            value: None,
        },
        RefusedCase {
            // The boundary refusal is decided before the handler, so this answers 400
            // and not the 404 an unknown client id would otherwise get.
            name: "a maxResults above its modelled range on a client id with no session",
            method: "GET",
            uri: "/connections/refusal-client/subscriptions?maxResults=201".to_string(),
            body: Vec::new(),
            member: "maxResults",
            value: Some("201".to_string()),
        },
        RefusedCase {
            name: "a clientId one character past its modelled maximum",
            method: "GET",
            uri: format!("/connections/{}", "c".repeat(129)),
            body: Vec::new(),
            member: "clientId",
            value: Some("c".repeat(129)),
        },
        RefusedCase {
            name: "a clientId outside its modelled pattern",
            method: "GET",
            uri: "/connections/%24reserved-client".to_string(),
            body: Vec::new(),
            member: "clientId",
            value: Some("$reserved-client".to_string()),
        },
    ]
}

fn accepted_cases() -> Vec<AcceptedCase> {
    vec![
        AcceptedCase {
            name: "a shadowName exactly at its modelled maximum",
            method: "POST",
            uri: format!("/things/refusalThing/shadow?name={}", "s".repeat(64)),
            body: state_document(),
        },
        AcceptedCase {
            name: "a thingName exactly at its modelled maximum",
            method: "POST",
            uri: format!("/things/{}/shadow", "t".repeat(128)),
            body: state_document(),
        },
        AcceptedCase {
            name: "a qos at the top of its modelled range",
            method: "POST",
            uri: "/topics/refusal-topic?qos=1&retain=false".to_string(),
            body: b"x".to_vec(),
        },
        AcceptedCase {
            name: "a maxResults at the top of its modelled range",
            method: "GET",
            uri: "/retainedMessage?maxResults=200".to_string(),
            body: Vec::new(),
        },
        AcceptedCase {
            name: "a pageSize at the top of its modelled range",
            method: "GET",
            uri: "/api/things/shadow/ListNamedShadowsForThing/refusalThing?pageSize=100"
                .to_string(),
            body: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_constraint_refusal_carries_this_services_own_error_code() {
    let harness = Harness::new();
    // A thing with a shadow, so the accepted rows that read one are not 404s for a
    // reason that has nothing to do with the constraint.
    harness
        .update("refusalThing", None, state_document())
        .await;

    for case in refused_cases() {
        let answer = harness
            .send(case.method, &case.uri, &[], case.body.clone())
            .await;
        assert_eq!(
            answer.status,
            400,
            "case {:?}: expected 400, got {} with body {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {:?}: expected this service's own client-input error, got {}",
            case.name,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            !String::from_utf8_lossy(&answer.body).contains("ValidationException"),
            "case {:?}: ValidationException is not a shape this service returns, got {}",
            case.name,
            String::from_utf8_lossy(&answer.body)
        );
        let stated = message(&answer);
        assert_eq!(
            // The sentence is written out here, not taken from the crate, so the wording
            // is pinned independently of the renderer under test.
            stated, "Bad Request",
            "case {:?}: expected this data plane's fixed front-door refusal, got {stated:?}",
            case.name
        );
        assert!(
            !stated.contains(case.member),
            "case {:?}: the front-door refusal must not name the member {:?}, got {stated:?}",
            case.name,
            case.member
        );
        if let Some(value) = &case.value {
            assert!(
                !String::from_utf8_lossy(&answer.body).contains(value.as_str()),
                "case {:?}: the front-door refusal must not echo the value the request \
                 carried, got {}",
                case.name,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

#[tokio::test]
async fn a_request_at_each_modelled_bound_is_not_refused() {
    let harness = Harness::new();
    harness
        .update("refusalThing", None, state_document())
        .await;

    for case in accepted_cases() {
        let answer = harness
            .send(case.method, &case.uri, &[], case.body.clone())
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected the bound itself to be accepted, got {} with body {}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}
