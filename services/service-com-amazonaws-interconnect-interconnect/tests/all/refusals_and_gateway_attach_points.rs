// SPDX-License-Identifier: CC0-1.0

//! How Interconnect CLASSIFIES and WORDS its refusals, and how a deleted Connection
//! converges — driven over the wire through the shared `crate::emulator` harness.
//!
//! Each test asserts a RULE over a TABLE whose rows differ along what the rule
//! generalises over — the shape of the absent identifier, the operation that resolves
//! it, the provider kind (and so the `ConnectionId` prefix), the connection state, the
//! environment and the bandwidth. The recorded case is one row, not the rule.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, text, Emulator,
};

/// The envelope this service opens every refusal message it words itself with
/// ("Unable to process request: This environment model with id … could not be found",
/// "Unable to process request: Connection does not exist for …").
const ENVELOPE: &str = "Unable to process request: ";

/// A `DirectConnectGatewayAttachPoint` the caller may send: 36 characters in the UUID
/// groups the member's pattern requires.
const ANY_GATEWAY_UUID: &str = "12345678-1234-1234-1234-123456789012";

/// How long a poll waits for a clock-owed transition before the case fails.
///
/// Well under the budget a behavioural check allows, so a lifecycle this suite drives
/// is proved to converge PROMPTLY rather than eventually.
const POLL_BUDGET: Duration = Duration::from_secs(5);

/// How often a poll re-reads while it waits.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Long enough to cover a connection's provisioning span (the handler's own 400 ms)
/// with margin; the transition is due at a stored instant, so any wait past the span
/// settles it.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

// ---------------------------------------------------------------------------
// The refusal messages this service is recorded wording, written out here rather
// than imported, so the assertions below do not restate the implementation.
// ---------------------------------------------------------------------------

/// The refusal for an `environmentId` (or `GetEnvironment$id`) that resolves to no
/// Environment.
fn environment_not_found(environment_id: &str) -> String {
    format!("{ENVELOPE}This environment model with id {environment_id} could not be found")
}

/// `UpdateConnection`'s refusal for an identifier that names no connection.
fn connection_does_not_exist(identifier: &str) -> String {
    format!("{ENVELOPE}Connection does not exist for {identifier}")
}

/// The connection READS' refusal for an identifier that names no connection — a
/// DIFFERENT construction from the update path's.
fn connection_not_retrievable(identifier: &str) -> String {
    format!("{ENVELOPE}Unable to retrieve connection {identifier}, as it does not exist")
}

/// `UpdateConnection`'s refusal on a connection whose state does not permit an update,
/// which states the permitted-state rule itself.
fn connection_not_updatable(identifier: &str) -> String {
    format!(
        "{ENVELOPE}Cannot update connection {identifier}: only connections in 'requested' or \
         'available' state can be updated"
    )
}

/// The `ConnectionState` values an update is permitted from, per the rule the refusal
/// above states.
const UPDATABLE_STATES: [&str; 2] = ["requested", "available"];

// ---------------------------------------------------------------------------
// An unresolvable environmentId
// ---------------------------------------------------------------------------

/// RULE: an `environmentId` that resolves to no Environment is refused by EVERY
/// operation that resolves one, as `InterconnectClientException` (HTTP 400) — the
/// service's own client fault, not the connection-scoped 404 — with the message built
/// from the id the request carried.
#[tokio::test]
async fn an_unresolvable_environment_id_is_a_client_refusal_everywhere_it_is_resolved() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let known = text(&catalog[0], "environmentId");

    // Rows differ in the SHAPE of the absent id: unrelated, a prefix of a real one, a
    // real one with a suffix, a case variant, the shortest and the longest the member
    // allows (`EnvironmentId` is `@length(min: 1, max: 64)`).
    let mut absent: Vec<String> = vec![
        "env-does-not-exist".to_string(),
        "sim2real-behav-no-env-0001".to_string(),
        known[..known.len() - 1].to_string(),
        format!("{known}-extra"),
        known.to_ascii_uppercase(),
        "e".to_string(),
        "e".repeat(64),
    ];
    absent.retain(|candidate| *candidate != known);

    // Rows differ in the OPERATION that resolves the id, and in the member carrying it.
    for id in &absent {
        for (operation, request) in [
            ("ListAttachPoints", json!({ "environmentId": id })),
            ("GetEnvironment", json!({ "id": id })),
            (
                "CreateConnection",
                json!({
                    "environmentId": id,
                    "bandwidth": "1Gbps",
                    "attachPoint": { "directConnectGateway": ANY_GATEWAY_UUID },
                }),
            ),
        ] {
            let case = format!("{operation} / {id}");
            let (status, body) = emulator.ic(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an environment that does not exist, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains("InterconnectClientException"),
                "case {case:?}: expected InterconnectClientException, got {:?}",
                code(&body)
            );
            assert_eq!(
                message(&body),
                environment_not_found(id),
                "case {case:?}: expected the request-derived message, got {:?}",
                message(&body)
            );
        }
    }

    // Positive control: every environment the catalog DOES hold resolves on all three
    // paths, so what is refused above is the absent id and not the request.
    for environment in creatable_environments(&catalog) {
        let id = text(&environment, "environmentId");
        let bandwidth = available_bandwidths(&environment)[0].clone();
        emulator.ic_ok("GetEnvironment", json!({ "id": id })).await;
        emulator
            .ic_ok("ListAttachPoints", json!({ "environmentId": id }))
            .await;
        let created = emulator
            .create_connection(&id, &bandwidth, ANY_GATEWAY_UUID, "positive control")
            .await;
        assert_eq!(
            text(&created, "environmentId"),
            id,
            "case {id:?}: expected the connection to be created on the named environment, got {created:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A connection identifier that names nothing
// ---------------------------------------------------------------------------

/// RULE: an identifier that names no stored connection is `ResourceNotFoundException`
/// (404) with a message built from the request — `UpdateConnection` stating that the
/// connection does not exist for the identifier it was asked to modify, the connection
/// READS stating that the retrieval could not be served.
#[tokio::test]
async fn a_connection_identifier_that_names_nothing_is_worded_from_the_request() {
    let emulator = Emulator::new();

    // Rows differ in the prefix the `ConnectionId` pattern allows
    // (`^(mcc|lmcc)-[a-z0-9]{8}$`) and in the character class of the suffix: all
    // digits, all letters, mixed, and the two extremes of the alphabet.
    for identifier in [
        "mcc-ffffffff",
        "lmcc-ffffffff",
        "mcc-00000000",
        "lmcc-00000000",
        "mcc-0a1b2c3d",
        "lmcc-zzzzzzzz",
    ] {
        let (status, body) = emulator
            .ic(
                "UpdateConnection",
                json!({ "identifier": identifier, "description": "does not matter" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: expected HTTP 404 for an unknown connection, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {identifier:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            connection_does_not_exist(identifier),
            "case {identifier:?}: expected UpdateConnection's request-derived message, got {:?}",
            message(&body)
        );

        // The reads word the same absence their own way, and still as a 404.
        for operation in ["GetConnection", "DeleteConnection"] {
            let case = format!("{operation} / {identifier}");
            let (status, body) = emulator
                .ic(operation, json!({ "identifier": identifier }))
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404 for an unknown connection, got {status} with {body:?}"
            );
            assert_eq!(
                message(&body),
                connection_not_retrievable(identifier),
                "case {case:?}: expected the retrieval wording, got {:?}",
                message(&body)
            );
            assert_ne!(
                message(&body),
                connection_does_not_exist(identifier),
                "case {case:?}: expected the reads' wording to differ from UpdateConnection's, got {:?}",
                message(&body)
            );
        }

        // A refused update creates nothing.
        let (status, body) = emulator.get_connection(identifier).await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: expected the refused update to have created nothing, got {status} with {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Which states an update is permitted from
// ---------------------------------------------------------------------------

/// RULE: an update is permitted only FROM the states the service permits it from
/// (`requested` or `available`); from any other state it is refused with
/// `InterconnectClientException` (400), a message naming the requested identifier and
/// stating that rule, and the record left exactly as it was.
#[tokio::test]
async fn an_update_is_permitted_only_from_the_states_the_service_names() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let creatable = creatable_environments(&catalog);
    assert!(
        creatable.len() >= 2,
        "case \"catalog\": expected at least two environments that take connections, got {}",
        creatable.len()
    );

    // Rows span the provider kinds (so both `ConnectionId` prefixes are covered) and,
    // within each, the states a connection can be driven into.
    for environment in &creatable {
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let bandwidth = bandwidths[0].clone();

        // A freshly created connection is `requested`, which the rule permits.
        let fresh = emulator
            .create_connection(&environment_id, &bandwidth, ANY_GATEWAY_UUID, "fresh link")
            .await;
        let fresh_id = text(&fresh, "id");
        assert!(
            UPDATABLE_STATES.contains(&text(&fresh, "state").as_str()),
            "case {fresh_id:?}: expected a new connection to be in an updatable state, got {:?}",
            fresh["state"]
        );
        let updated = emulator
            .ic_ok(
                "UpdateConnection",
                json!({ "identifier": fresh_id, "description": "renamed link" }),
            )
            .await;
        assert_eq!(
            text(&updated["connection"], "description"),
            "renamed link",
            "case {fresh_id:?}: expected the permitted update to persist, got {:?}",
            updated["connection"]["description"]
        );

        // A bandwidth change hands the connection to the clock as `updating`. Whether
        // the next call lands before or after the clock settles it, the SAME rule
        // decides: refused while the state is not updatable, permitted once it is —
        // and the state the permitted call reports is one the rule permits.
        if let Some(other) = bandwidths.iter().find(|candidate| **candidate != bandwidth) {
            let in_flight = emulator
                .create_connection(
                    &environment_id,
                    &bandwidth,
                    ANY_GATEWAY_UUID,
                    "bandwidth change",
                )
                .await;
            let in_flight_id = text(&in_flight, "id");
            let changed = emulator
                .ic_ok(
                    "UpdateConnection",
                    json!({ "identifier": in_flight_id, "bandwidth": other }),
                )
                .await;
            assert_eq!(
                text(&changed["connection"], "state"),
                "updating",
                "case {in_flight_id:?}: expected a bandwidth change to report 'updating', got {:?}",
                changed["connection"]["state"]
            );
            let (status, body) = emulator
                .ic(
                    "UpdateConnection",
                    json!({ "identifier": in_flight_id, "description": "while in flight" }),
                )
                .await;
            if status == 200 {
                assert!(
                    UPDATABLE_STATES.contains(&text(&body["connection"], "state").as_str()),
                    "case {in_flight_id:?}: expected a permitted update to report an updatable state, got {:?}",
                    body["connection"]["state"]
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {in_flight_id:?}: expected HTTP 400 while the circuit is in flight, got {status} with {body:?}"
                );
                assert!(
                    code(&body).contains("InterconnectClientException"),
                    "case {in_flight_id:?}: expected InterconnectClientException, got {:?}",
                    code(&body)
                );
                assert_eq!(
                    message(&body),
                    connection_not_updatable(&in_flight_id),
                    "case {in_flight_id:?}: expected the permitted-state rule in the message, got {:?}",
                    message(&body)
                );
            }
        }

        // A connection on its way out, and one already gone, are both outside the
        // permitted set: the refusal is the same and the record does not change.
        let doomed = emulator
            .create_connection(&environment_id, &bandwidth, ANY_GATEWAY_UUID, "doomed link")
            .await;
        let doomed_id = text(&doomed, "id");
        emulator
            .ic_ok("DeleteConnection", json!({ "identifier": doomed_id }))
            .await;
        for stage in ["on its way out", "already gone"] {
            if stage == "already gone" {
                tokio::time::sleep(SETTLE_WAIT).await;
            }
            let case = format!("{doomed_id} {stage}");
            let before = emulator.get_connection(&doomed_id).await.1;
            let (status, body) = emulator
                .ic(
                    "UpdateConnection",
                    json!({ "identifier": doomed_id, "description": "post delete update" }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an update of a connection outside the permitted states, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains("InterconnectClientException"),
                "case {case:?}: expected InterconnectClientException, got {:?}",
                code(&body)
            );
            assert_eq!(
                message(&body),
                connection_not_updatable(&doomed_id),
                "case {case:?}: expected the permitted-state rule in the message, got {:?}",
                message(&body)
            );
            let after = emulator.get_connection(&doomed_id).await.1;
            assert_eq!(
                text(&after["connection"], "description"),
                text(&before["connection"], "description"),
                "case {case:?}: expected the refused update to leave the description alone, got {:?}",
                after["connection"]["description"]
            );
        }
        let terminal = emulator.get_connection(&doomed_id).await.1;
        assert_eq!(
            text(&terminal["connection"], "state"),
            "deleted",
            "case {doomed_id:?}: expected the connection to stay in its terminal state, got {:?}",
            terminal["connection"]["state"]
        );
    }
}

// ---------------------------------------------------------------------------
// The delete lifecycle, end to end
// ---------------------------------------------------------------------------

/// RULE: a `DeleteConnection` is accepted, echoes the Connection whose id is the one
/// requested, and the connection converges to the terminal `deleted` state (or becomes
/// not-found) observably through `GetConnection` within a bounded time — with no call
/// in the sequence blocking, so a caller polling after the delete completes instead of
/// hanging.
#[tokio::test]
async fn a_deleted_connection_converges_to_a_terminal_state_promptly() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;

    // Rows differ in the environment (and so in the provider kind, the location and
    // the `ConnectionId` prefix), in the bandwidth the connection is created with, and
    // in the attach point it is created on — one of the Environment's own listed attach
    // points, so the delete path is exercised on a connection a client could have made.
    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let attach_points = emulator.attach_points(&environment_id).await;
        assert!(
            !attach_points.is_empty(),
            "case {environment_id:?}: expected the environment to offer an attach point, got {attach_points:?}"
        );
        for (index, bandwidth) in available_bandwidths(&environment).iter().enumerate() {
            let attach_point = text(&attach_points[index % attach_points.len()], "identifier");
            let case = format!("{environment_id} / {bandwidth} / {attach_point}");

            let created = emulator
                .create_connection(&environment_id, bandwidth, &attach_point, "to be deleted")
                .await;
            let connection_id = text(&created, "id");

            let deleted = emulator
                .ic_ok("DeleteConnection", json!({ "identifier": connection_id }))
                .await;
            assert_eq!(
                text(&deleted["connection"], "id"),
                connection_id,
                "case {case:?}: expected the delete to echo the connection it was asked for, got {:?}",
                deleted["connection"]["id"]
            );
            assert!(
                ["deleting", "deleted"].contains(&text(&deleted["connection"], "state").as_str()),
                "case {case:?}: expected the delete to report a tear-down state, got {:?}",
                deleted["connection"]["state"]
            );

            // Poll the way an SDK waiter does: a terminal state, or a not-found.
            let started = Instant::now();
            let terminal = loop {
                let (status, body) = emulator.get_connection(&connection_id).await;
                if status == 404 {
                    break "not found".to_string();
                }
                let state = text(&body["connection"], "state");
                if state == "deleted" {
                    break state;
                }
                assert!(
                    started.elapsed() < POLL_BUDGET,
                    "case {case:?}: expected the connection to reach a terminal state within {POLL_BUDGET:?}, it is still {state:?}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            };
            assert!(
                terminal == "deleted" || terminal == "not found",
                "case {case:?}: expected a terminal outcome, got {terminal:?}"
            );

            // A repeated delete replays the terminal record rather than failing or
            // restarting the tear-down, and the record stays readable.
            let repeat = emulator.ic("DeleteConnection", json!({ "identifier": connection_id })).await;
            assert!(
                repeat.0 == 200 || repeat.0 == 404,
                "case {case:?}: expected a repeated delete to replay the record or report it gone, got {} with {:?}",
                repeat.0,
                repeat.1
            );
        }
    }
}
