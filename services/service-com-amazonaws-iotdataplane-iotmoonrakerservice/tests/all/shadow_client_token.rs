// SPDX-License-Identifier: CC0-1.0

//! The correlation token an update carries, end to end over the router this service
//! mounts in production.
//!
//! The rule these cases assert: **the accepted reply to `UpdateThingShadow` carries back
//! the `clientToken` the request's document named, and the STORED document never holds
//! it** — a token is the caller's key for matching an acknowledgement to the request that
//! produced it, not shadow state, so `GetThingShadow` (and the reply to any later update
//! that names no token) reports the merged state, metadata, version and timestamp and
//! nothing else.
//!
//! The rows vary what the rule generalises over: the token's JSON form and length, the
//! sections the update writes, whether the shadow is named or classic, and whether the
//! update is a shadow's first or a later one. A request that names NO token is a row too:
//! its reply has no `clientToken` member at all, so the member's absence still means
//! "you sent none".

use serde_json::{json, Value};

use crate::shadow_and_retained_message::Harness;

/// The members the accepted reply carries beside the correlation token: the merged
/// document a reader of the shadow sees.
const DOCUMENT_MEMBERS: [&str; 4] = ["metadata", "state", "timestamp", "version"];

/// The sorted member names of a JSON object body.
fn member_names(case: &str, document: &Value) -> Vec<String> {
    let members = document
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {document}"));
    let mut names: Vec<String> = members.keys().cloned().collect();
    names.sort();
    names
}

/// One row: the document an update sends, and the token the reply must echo.
struct Row {
    case: &'static str,
    /// The shadow the row addresses, or `None` for the unnamed (classic) shadow.
    shadow: Option<&'static str>,
    /// The token the request's document names, or `None` for a document that names none.
    token: Option<Value>,
    /// The state the update writes.
    state: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a short string token on a classic shadow's desired state",
            shadow: None,
            token: Some(json!("tok-1")),
            state: json!({"desired": {"a": 1}}),
        },
        Row {
            case: "a UUID-shaped token on a named shadow's reported state",
            shadow: Some("cfg"),
            token: Some(json!("7b4c0f52-0f0e-4e2e-9a1a-6d4a8c2b1e55")),
            state: json!({"reported": {"firmware": "1.4.2"}}),
        },
        Row {
            case: "a 64-character token writing both sections",
            shadow: None,
            token: Some(json!("t".repeat(64))),
            state: json!({"desired": {"mode": "auto"}, "reported": {"mode": "manual"}}),
        },
        Row {
            case: "a token that is a number",
            shadow: Some("counter"),
            token: Some(json!(42)),
            state: json!({"reported": {"count": 7}}),
        },
        Row {
            case: "an empty-string token",
            shadow: None,
            token: Some(json!("")),
            state: json!({"reported": {"b": true}}),
        },
        Row {
            case: "no token at all",
            shadow: Some("plain"),
            token: None,
            state: json!({"desired": {"nested": {"deep": [1, 2]}}}),
        },
    ]
}

/// The document an update sends for one row.
fn update_document(row: &Row) -> Vec<u8> {
    let mut document = json!({"state": row.state.clone()});
    if let Some(token) = &row.token {
        document
            .as_object_mut()
            .expect("a JSON object document")
            .insert("clientToken".to_string(), token.clone());
    }
    serde_json::to_vec(&document).expect("a serializable update document")
}

#[tokio::test]
async fn an_update_echoes_its_client_token_and_never_stores_it() {
    let harness = Harness::new();

    for (index, row) in rows().iter().enumerate() {
        let thing = format!("tokenThing{index}");

        // Two updates per row, so the rule holds on a shadow's first update and on a
        // later one: both carry the row's token.
        for pass in 1..=2 {
            let answer = harness
                .update(&thing, row.shadow, update_document(row))
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {:?} pass {pass}: expected the update to be accepted, got {} with body {}",
                row.case,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let accepted = answer.json();
            assert_eq!(
                accepted.get("clientToken"),
                row.token.as_ref(),
                "case {:?} pass {pass}: expected the request's clientToken echoed, got {:?}",
                row.case,
                accepted.get("clientToken")
            );
            // The reply is the merged document plus the token, and nothing else.
            let mut expected: Vec<String> =
                DOCUMENT_MEMBERS.iter().map(|name| name.to_string()).collect();
            if row.token.is_some() {
                expected.push("clientToken".to_string());
                expected.sort();
            }
            assert_eq!(
                member_names(row.case, &accepted),
                expected,
                "case {:?} pass {pass}: expected the document members plus the token, got {accepted}",
                row.case
            );
            assert_eq!(
                accepted.get("version").and_then(Value::as_i64),
                Some(pass),
                "case {:?} pass {pass}: expected the version to count the accepted updates, \
                 got {accepted}",
                row.case
            );
        }

        // The token is the caller's, not state: what was stored carries the merged
        // document alone, as every later reader reports it.
        let read = harness.get_shadow(&thing, row.shadow).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the stored shadow to be readable, got {} with body {}",
            row.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let stored = read.json();
        assert_eq!(
            stored.get("clientToken"),
            None,
            "case {:?}: the stored document must not carry a clientToken, got {stored}",
            row.case
        );
        assert_eq!(
            member_names(row.case, &stored),
            DOCUMENT_MEMBERS
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the stored document's own members, got {stored}",
            row.case
        );

        // An update that names NO token acknowledges without one, whatever the updates
        // before it carried: the echo comes from the request, never from the record.
        let untokened = harness
            .update(
                &thing,
                row.shadow,
                serde_json::to_vec(&json!({"state": {"reported": {"heartbeat": 1}}}))
                    .expect("a serializable update document"),
            )
            .await;
        assert_eq!(
            untokened.status,
            200,
            "case {:?}: expected the untokened update to be accepted, got {} with body {}",
            row.case,
            untokened.status,
            String::from_utf8_lossy(&untokened.body)
        );
        assert_eq!(
            untokened.json().get("clientToken"),
            None,
            "case {:?}: a request with no clientToken must get a reply with none, got {}",
            row.case,
            untokened.json()
        );
    }
}

/// A refused update echoes nothing: the failure answers in the data plane's
/// `{message, traceId}` envelope, which carries no document and so no token.
#[tokio::test]
async fn a_refused_update_answers_the_failure_envelope_without_the_token() {
    let harness = Harness::new();

    for (case, payload) in [
        (
            "a document with no state node",
            json!({"clientToken": "tok-refused"}),
        ),
        (
            "a document whose state node is a scalar",
            json!({"clientToken": "tok-refused", "state": 7}),
        ),
        (
            "a document stating a version nothing stored",
            json!({"clientToken": "tok-refused", "version": 9, "state": {"reported": {"a": 1}}}),
        ),
    ] {
        let answer = harness
            .update(
                "refusedTokenThing",
                None,
                serde_json::to_vec(&payload).expect("a serializable document"),
            )
            .await;
        assert!(
            (400..500).contains(&answer.status),
            "case {case:?}: expected the update to be refused, got {} with body {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.json().get("clientToken"),
            None,
            "case {case:?}: a refusal carries the error envelope, not the token, got {}",
            answer.json()
        );
    }
}
