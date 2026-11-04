// SPDX-License-Identifier: CC0-1.0

//! Wire-shape probes for `ApiGatewayManagementApi` on restJson1: the exact bytes,
//! status codes and JSON keys a real AWS SDK client parses.
//!
//! The lifecycle suite proves each response is computed from the connection registry.
//! This one pins the ENVELOPE that registry is reported in, where the model — not the
//! stored value — decides the form:
//!
//! * `ConnectedAt` / `LastActiveAt` target `__timestampIso8601`, which the model marks
//!   `@timestampFormat("date-time")`, so they render as RFC 3339 date-time STRINGS, not
//!   as the restJson1 default epoch-seconds number.
//! * the `@jsonName`s the model declares (`connectedAt`, `identity`, `lastActiveAt`,
//!   `sourceIp`, `userAgent`) are the keys on the wire.
//! * a modelled error renders as `{"__type":"<Shape>", …}` with that shape's
//!   `@httpError` status.
//!
//! The router, request builder and response readers live in `crate::support`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use engine::InMemoryEngine;

use service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi as mgmt;

use crate::support::{call, register};

/// Registration instants and the RFC 3339 text a `@timestampFormat("date-time")` member
/// renders them as. Written out by hand (the format is the model's, not this crate's)
/// so a fallback to restJson1's default epoch-seconds encoding fails here instead of
/// silently shipping a number where a string belongs. Rows span the epoch itself, the
/// second after it, and three widely separated later instants.
const TIMESTAMP_TABLE: [(u64, &str); 5] = [
    (0, "1970-01-01T00:00:00Z"),
    (1, "1970-01-01T00:00:01Z"),
    (1_000_000, "1970-01-12T13:46:40Z"),
    (1_600_000_000, "2020-09-13T12:26:40Z"),
    (1_700_000_000, "2023-11-14T22:13:20Z"),
];

fn register_at(engine: &Arc<InMemoryEngine>, connection_id: &str, connected_at_secs: u64) {
    register(
        engine,
        connection_id,
        "192.0.2.80",
        "probe/1",
        SystemTime::UNIX_EPOCH + Duration::from_secs(connected_at_secs),
    );
}

#[tokio::test]
async fn get_connection_renders_iso8601_date_times_and_the_modeled_json_names() {
    for (secs, expected) in TIMESTAMP_TABLE {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let connection_id = "probe-ts";
        register_at(&engine, connection_id, secs);

        let (status, body) = call(&engine, "GET", connection_id, Vec::new()).await;
        assert_eq!(
            status, 200,
            "case {secs}: GetConnection must answer 200, got {status}"
        );
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("case {secs}: body must be JSON ({e}): {body:?}"));

        // Nothing was delivered, so both instants are the registration instant and both
        // must render in the modelled format.
        for member in ["connectedAt", "lastActiveAt"] {
            let rendered = value.get(member);
            assert_eq!(
                rendered.and_then(serde_json::Value::as_str),
                Some(expected),
                "case {secs}: {member} must be the ISO 8601 date-time {expected:?}, got {rendered:?}"
            );
        }

        assert_eq!(
            value.pointer("/identity/sourceIp").and_then(serde_json::Value::as_str),
            Some("192.0.2.80"),
            "case {secs}: identity.sourceIp must use the modelled @jsonName keys, got {value}"
        );
        assert_eq!(
            value.pointer("/identity/userAgent").and_then(serde_json::Value::as_str),
            Some("probe/1"),
            "case {secs}: identity.userAgent must use the modelled @jsonName keys, got {value}"
        );
    }
}

/// Each failure this service answers renders in the restJson1 error envelope with its
/// own status: one row per error it can answer from a request. The two refusals of a
/// `ConnectionId` differ — an id the service never issued is the 400
/// `BadRequestException`, an id it issued whose connection is closed is the modelled 410
/// `GoneException` — so both are pinned here.
#[tokio::test]
async fn modeled_errors_render_with_their_own_status_and_type() {
    /// Which connection id a row addresses.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Target {
        /// An id this service never issued.
        NeverIssued,
        /// An id it issued whose connection the row deletes first.
        Deleted,
        /// A registered, open connection.
        Open,
    }

    // (case, method, request body length, target, expected status, expected __type)
    let table: [(&str, &str, usize, Target, u16, &str); 7] = [
        (
            "get on an id this service never issued",
            "GET",
            0,
            Target::NeverIssued,
            400,
            "BadRequestException",
        ),
        (
            "post to an id this service never issued",
            "POST",
            4,
            Target::NeverIssued,
            400,
            "BadRequestException",
        ),
        (
            "delete an id this service never issued",
            "DELETE",
            0,
            Target::NeverIssued,
            400,
            "BadRequestException",
        ),
        (
            "get on a closed connection",
            "GET",
            0,
            Target::Deleted,
            410,
            "GoneException",
        ),
        (
            "post to a closed connection",
            "POST",
            4,
            Target::Deleted,
            410,
            "GoneException",
        ),
        (
            "delete a closed connection",
            "DELETE",
            0,
            Target::Deleted,
            410,
            "GoneException",
        ),
        (
            "post one byte past the modelled maximum",
            "POST",
            mgmt::ops::post_to_connection::MAX_DATA_BYTES + 1,
            Target::Open,
            413,
            "PayloadTooLargeException",
        ),
    ];

    for (case, method, body_len, target, expected_status, expected_type) in table {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        // The oversized row needs a registered connection, so that its refusal is
        // about the payload and not about the id.
        register_at(&engine, "probe-err", 1_700_000_000);

        let connection_id = match target {
            Target::NeverIssued => "not-registered",
            Target::Open => "probe-err",
            Target::Deleted => {
                register_at(&engine, "probe-gone", 1_700_000_000);
                let (status, _) = call(&engine, "DELETE", "probe-gone", Vec::new()).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: the delete that closes the connection must answer 204, \
                     got {status}"
                );
                "probe-gone"
            }
        };
        let (status, body) = call(&engine, method, connection_id, vec![b'x'; body_len]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body:?}"
        );
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("case {case:?}: error body must be JSON ({e}): {body:?}"));
        assert_eq!(
            value.get("__type").and_then(serde_json::Value::as_str),
            Some(expected_type),
            "case {case:?}: expected __type {expected_type:?}, got {value}"
        );
    }
}

/// `PayloadTooLargeException` models a `message` member (`@jsonName("message")`), and
/// the text names the size that was sent and the bound it broke, so the row that is
/// one byte over and the row that is far over do not read the same.
#[tokio::test]
async fn the_payload_too_large_message_states_the_size_and_the_bound() {
    let max = mgmt::ops::post_to_connection::MAX_DATA_BYTES;
    for size in [max + 1, max + 1024, max * 3] {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        register_at(&engine, "probe-big", 1_700_000_000);

        let (status, body) = call(&engine, "POST", "probe-big", vec![b'x'; size]).await;
        assert_eq!(status, 413, "case {size}: expected HTTP 413, got {status}");
        let value: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("case {size}: error body must be JSON ({e}): {body:?}"));
        let message = value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(&size.to_string()) && message.contains(&max.to_string()),
            "case {size}: the message must name the payload size and the maximum, got {message:?}"
        );
    }
}

/// `DeleteConnection` models no output members and a 204 success status, so the
/// response carries no body at all — not an empty JSON object.
#[tokio::test]
async fn delete_connection_answers_204_with_an_empty_body() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    register_at(&engine, "probe-del", 1_600_000_000);

    let (status, body) = call(&engine, "DELETE", "probe-del", Vec::new()).await;
    assert_eq!(status, 204, "expected HTTP 204, got {status}");
    assert!(body.is_empty(), "a 204 carries no body, got {body:?}");
}

/// `PostToConnection` models no output members and a 200 success status.
#[tokio::test]
async fn post_to_connection_answers_200_with_an_empty_body() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    register_at(&engine, "probe-post", 1_600_000_000);

    let (status, body) = call(&engine, "POST", "probe-post", b"payload".to_vec()).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}");
    assert!(body.is_empty(), "the success response carries no body, got {body:?}");
}
