// SPDX-License-Identifier: CC0-1.0

//! How the three `@connections` operations refuse a `ConnectionId` that identifies no
//! connection of this service, driven over HTTP through the same `MultiServiceRouter` the
//! production emulator serves with.
//!
//! The rule these cases assert, one level above the recorded case (one `PostToConnection`
//! to a padded base64-shaped id nothing had issued):
//!
//! > A connection id is not an arbitrary string: API Gateway issues it when a client's
//! > handshake completes, and the three `@connections` operations address a connection BY
//! > it. So a `ConnectionId` this service never issued identifies no connection at all and
//! > is refused — before any connection is looked up, and without touching anything —
//! > with HTTP 400 `BadRequestException` and the message `Invalid connectionId: ` plus the
//! > `ConnectionId` the request sent. `GoneException` (HTTP 410) is reserved for an id it
//! > DID issue whose connection has since closed: "The connection with the provided id no
//! > longer exists" (`smithy_query(query="shape", shape="GoneException")`).
//!
//! WHY the two are not interchangeable: a caller that pushes to a connection prunes its
//! own record of that connection on the 410, which is the right thing to do for a socket
//! that has closed and the wrong thing to do for an id it got wrong — answering 410 there
//! makes it delete a record it should keep, and a 410 carries no message (the modelled
//! shape has no members), so nothing says what was wrong with the request.
//!
//! The rows span what the rule generalises over: the form of the `ConnectionId` (the
//! padded base64 shape AWS mints, an all-`A` one of that same shape, unpadded, dashed,
//! very short, very long, percent-encoded), the operation (all three), and the connection
//! id's history (never issued, open, closed).
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{call, error_code, register};

/// The status and code API Gateway refuses a `ConnectionId` that identifies no connection
/// with, and the fixed text of its message.
const INVALID_STATUS: u16 = 400;
const INVALID_TYPE: &str = "BadRequestException";
const INVALID_MESSAGE: &str = "Invalid connectionId: ";

/// The modelled refusal for an issued id whose connection has closed
/// (`@httpError(410)`).
const GONE_STATUS: u16 = 410;
const GONE_TYPE: &str = "GoneException";

/// `ConnectionId` forms. The model declares no syntax for one — `__string` with
/// `@httpLabel` and `@required`, no `@pattern` and no `@length`
/// (`smithy_query(query="shape", shape="PostToConnectionRequest")`) — so what decides the
/// answer is whether this service issued the id, which is what these rows vary against.
const ID_FORMS: [(&str, &str); 7] = [
    ("the padded base64 shape AWS mints", "Lx9ZKcvPmjQCJhA="),
    ("an all-A id of that same shape", "AAAAAAAAAAAAAAA="),
    ("an unpadded hex-shaped id", "0123456789abcdef"),
    ("a dashed id", "cid-with-dashes"),
    ("a single character", "x"),
    (
        "a long underscore-separated id",
        "ZZZ_0000000000000000000000000000",
    ),
    ("an id of base64url characters", "a-b_c9DEF0123456"),
];

/// The three operations, by the method each is reached with, and the payload each sends.
fn operations() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("GET", Vec::new()),
        ("POST", b"payload".to_vec()),
        ("DELETE", Vec::new()),
    ]
}

/// A registration instant in the past, so an instant the service writes later is strictly
/// greater.
fn connected_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// The message a JSON error body carries.
fn message(body: &bytes::Bytes) -> String {
    let value: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

#[tokio::test]
async fn an_id_this_service_never_issued_is_refused_as_an_invalid_connection_id() {
    for (id_form, connection_id) in ID_FORMS {
        for (method, payload) in operations() {
            // One OTHER connection is open, so each refusal below is about the id that
            // was asked for and not about an empty registry.
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            register(&engine, "issued", "192.0.2.1", "other/1", connected_at());

            let case = format!("{method} on {id_form} ({connection_id})");
            let (status, body) = call(&engine, method, connection_id, payload).await;
            assert_eq!(
                status, INVALID_STATUS,
                "case {case:?}: expected HTTP {INVALID_STATUS}, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_TYPE,
                "case {case:?}: expected {INVALID_TYPE}, got {body:?}"
            );
            assert_eq!(
                message(&body),
                format!("{INVALID_MESSAGE}{connection_id}"),
                "case {case:?}: expected the message to name the id the request sent, \
                 got {body:?}"
            );

            // The refusal changed nothing: no connection was registered for the id, and
            // the open one is untouched.
            assert!(
                mgmt::state::connection(&*engine, connection_id).is_none(),
                "case {case:?}: a refused call must not register the id it refused"
            );
            let other = mgmt::state::connection(&*engine, "issued")
                .unwrap_or_else(|| panic!("case {case:?}: the open connection must still exist"));
            assert_eq!(
                other.delivered_count, 0,
                "case {case:?}: a refused call must deliver nothing, got {} deliveries",
                other.delivered_count
            );
        }
    }
}

#[tokio::test]
async fn an_issued_id_is_served_while_its_connection_is_open_and_gone_once_it_closes() {
    for (id_form, connection_id) in ID_FORMS {
        // Open: the id this service issued names a live connection, so each operation
        // serves it.
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        register(&engine, connection_id, "198.51.100.7", "sender/1", connected_at());

        let case = format!("{id_form} ({connection_id})");
        let (status, body) = call(&engine, "GET", connection_id, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetConnection on the open connection must answer 200, got \
             {status} {body:?}"
        );
        let (status, body) = call(&engine, "POST", connection_id, b"payload".to_vec()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PostToConnection on the open connection must answer 200, got \
             {status} {body:?}"
        );
        let (status, body) = call(&engine, "DELETE", connection_id, Vec::new()).await;
        assert_eq!(
            status, 204,
            "case {case:?}: DeleteConnection must answer the modelled 204, got {status} \
             {body:?}"
        );

        // Closed: the id is still one this service issued, so every later call is the
        // modelled Gone and not the invalid-id refusal.
        for (method, payload) in operations() {
            let (status, body) = call(&engine, method, connection_id, payload).await;
            assert_eq!(
                status, GONE_STATUS,
                "case {case:?}: {method} after the delete must answer {GONE_STATUS}, got \
                 {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                GONE_TYPE,
                "case {case:?}: {method} after the delete must answer {GONE_TYPE}, got \
                 {body:?}"
            );
        }
    }
}
