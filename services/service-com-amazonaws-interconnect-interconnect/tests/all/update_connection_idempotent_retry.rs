// SPDX-License-Identifier: CC0-1.0

//! What a REPEATED `UpdateConnection` does — driven over the wire through the shared
//! `crate::emulator` harness.
//!
//! The operation is modelled `@idempotent` with an `@idempotencyToken` member
//! (`smithy_query(query="operation",operation="UpdateConnection")`: `"idempotent":
//! true`, `clientToken` `"idempotencyToken": true`), so a client or an SDK that repeats
//! a request it has already made must be given that request's own answer again instead
//! of a refusal.
//!
//! Each test asserts that RULE over a TABLE whose rows differ along what it generalises
//! over: which optional members the request supplies, the bandwidth it asks for, the
//! environment (and so the provider kind and the `ConnectionId` prefix), whether a
//! `clientToken` is sent and whether the repeats share it, and how many times the
//! request is repeated.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, text, Emulator,
};

/// The envelope this service opens every refusal message it words itself with.
const ENVELOPE: &str = "Unable to process request: ";

/// `UpdateConnection`'s refusal on a connection whose state does not permit the change
/// the request asks for, which states the permitted-state rule itself.
fn connection_not_updatable(identifier: &str) -> String {
    format!(
        "{ENVELOPE}Cannot update connection {identifier}: only connections in 'requested' or \
         'available' state can be updated"
    )
}

/// The `ConnectionState` values a connection whose circuit is still there reports: the
/// two an update is permitted from, the two legs of the provisioning progression, the
/// one a provisioned circuit that is not passing traffic reports, and the one an
/// in-flight change reports. (`ConnectionState`'s values, per
/// `smithy_query(query="shape",shape="ConnectionState")`, minus the ones a connection
/// that is gone or broken reports: `deleting`, `deleted`, `failed`.)
const LIVE_STATES: [&str; 5] = ["requested", "pending", "available", "down", "updating"];

/// Long enough to cover the handler's own provisioning span (400 ms) with margin; the
/// transition is due at a stored instant, so any wait past the span settles it.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// Whether, and how, a row's repeats carry a `clientToken`. The operation is idempotent
/// by construction — a repeat asks for what the record already holds — so the answer is
/// the same however the token is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    /// No `clientToken` at all, as a hand-written client may send it.
    Absent,
    /// The SAME token on every call, as an SDK retry of one request sends it.
    Shared,
    /// A fresh token per call, as a client that re-issues the request sends it.
    PerCall,
}

/// One row of the repeat table.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    /// Whether the request asks for a bandwidth — the environment's next available one,
    /// which is a CHANGE on the first call and already applied on every repeat.
    asks_bandwidth: bool,
    /// The description the request asks for, or `None` when it supplies none.
    asks_description: Option<&'static str>,
    token: Token,
    /// How many times the identical request is repeated after the first call.
    repeats: usize,
}

/// The rows, spanning the members a request may carry, the token policy and the repeat
/// count. A row that asks for a bandwidth is the one whose first call hands the
/// connection to the clock in the transient `updating` state.
const ROWS: [Row; 6] = [
    Row {
        label: "bandwidth only, shared token, retried once",
        asks_bandwidth: true,
        asks_description: None,
        token: Token::Shared,
        repeats: 1,
    },
    Row {
        label: "bandwidth only, no token, retried three times",
        asks_bandwidth: true,
        asks_description: None,
        token: Token::Absent,
        repeats: 3,
    },
    Row {
        label: "bandwidth only, fresh token per call",
        asks_bandwidth: true,
        asks_description: None,
        token: Token::PerCall,
        repeats: 2,
    },
    Row {
        label: "bandwidth and description, shared token",
        asks_bandwidth: true,
        asks_description: Some("retried link"),
        token: Token::Shared,
        repeats: 2,
    },
    Row {
        label: "description only, shared token",
        asks_bandwidth: false,
        asks_description: Some("renamed link"),
        token: Token::Shared,
        repeats: 2,
    },
    Row {
        label: "neither member, shared token",
        asks_bandwidth: false,
        asks_description: None,
        token: Token::Shared,
        repeats: 2,
    },
];

/// The members of a `Connection` an update owns or must leave alone, which is what a
/// repeat has to report unchanged.
const CARRIED_MEMBERS: [&str; 7] = [
    "id",
    "arn",
    "environmentId",
    "attachPoint",
    "bandwidth",
    "description",
    "billingTier",
];

/// The request a row sends: the identifier, the members the row asks for, and the token
/// the policy dictates for call number `call` (0 being the first call).
fn request(row: &Row, identifier: &str, bandwidth: &str, call: usize) -> Value {
    let mut body = Map::new();
    body.insert("identifier".to_string(), json!(identifier));
    if row.asks_bandwidth {
        body.insert("bandwidth".to_string(), json!(bandwidth));
    }
    if let Some(description) = row.asks_description {
        body.insert("description".to_string(), json!(description));
    }
    match row.token {
        Token::Absent => {}
        Token::Shared => {
            body.insert("clientToken".to_string(), json!(format!("{identifier}-retry")));
        }
        Token::PerCall => {
            body.insert(
                "clientToken".to_string(),
                json!(format!("{identifier}-retry-{call}")),
            );
        }
    }
    Value::Object(body)
}

// ---------------------------------------------------------------------------
// A repeat of a request already applied
// ---------------------------------------------------------------------------

/// RULE: a repeat of an `UpdateConnection` whose every supplied member the connection
/// already holds asks for no change, so it is answered with that connection — HTTP 200
/// and the same record the call that applied it returned — rather than refused, and it
/// neither re-arms nor extends the transient transition the first call handed the
/// connection to the clock in.
#[tokio::test]
async fn a_repeat_of_an_applied_update_is_answered_with_the_record_it_already_produced() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator.attach_point().await;
    let environments = creatable_environments(&catalog);
    assert!(
        !environments.is_empty(),
        "case \"catalog\": expected at least one environment that takes connections, got {environments:?}"
    );

    for environment in &environments {
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let created_with = bandwidths[0].clone();
        // The bandwidth a row asks for: the environment's next available one, so the
        // first call is a real change. An environment offering only one bandwidth has
        // no second one to ask for, so its rows ask for the one it holds — still a
        // request whose every member the record already satisfies.
        let asked_for = bandwidths
            .iter()
            .find(|candidate| **candidate != created_with)
            .cloned()
            .unwrap_or_else(|| created_with.clone());

        for row in ROWS {
            let case = format!("{environment_id} / {}", row.label);
            let created = emulator
                .create_connection(&environment_id, &created_with, &gateway, "retry subject")
                .await;
            let id = text(&created, "id");

            let first = emulator
                .ic_ok("UpdateConnection", request(&row, &id, &asked_for, 0))
                .await;
            let first = first["connection"].clone();
            let expected_bandwidth = if row.asks_bandwidth {
                asked_for.clone()
            } else {
                created_with.clone()
            };
            assert_eq!(
                text(&first, "bandwidth"),
                expected_bandwidth,
                "case {case:?}: expected the first call to apply the bandwidth, got {:?}",
                first["bandwidth"]
            );

            // Every repeat is the byte-identical request an SDK retry resends (bar the
            // token the policy varies), and every one of them is answered.
            for call in 1..=row.repeats {
                let (status, body) = emulator
                    .ic("UpdateConnection", request(&row, &id, &asked_for, call))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected repeat {call} of an applied update to be answered with HTTP 200, got {status} with {body:?}"
                );
                let repeated = body["connection"].clone();
                for member in CARRIED_MEMBERS {
                    assert_eq!(
                        repeated[member], first[member],
                        "case {case:?}: expected repeat {call} to report {member} unchanged, got {:?} (was {:?})",
                        repeated[member], first[member]
                    );
                }
                let state = text(&repeated, "state");
                assert!(
                    LIVE_STATES.contains(&state.as_str()),
                    "case {case:?}: expected repeat {call} to report a live state, got {state:?}"
                );
            }

            // The record a read observes is the one the first call produced: a repeat
            // wrote nothing of its own.
            let fetched = emulator.get_connection(&id).await.1;
            for member in CARRIED_MEMBERS {
                assert_eq!(
                    fetched["connection"][member], first[member],
                    "case {case:?}: expected GetConnection to observe {member} as the first call left it, got {:?}",
                    fetched["connection"][member]
                );
            }

            // Where the first call armed the transient transition, the repeats did not
            // re-arm it: the connection is out of that state one provisioning span
            // after the FIRST call, not one span after the last repeat.
            if text(&first, "state") == "updating" {
                tokio::time::sleep(SETTLE_WAIT).await;
                let settled = emulator.get_connection(&id).await.1;
                let settled_state = text(&settled["connection"], "state");
                assert_ne!(
                    settled_state, "updating",
                    "case {case:?}: expected the connection to have settled out of the transient state, got {settled_state:?}"
                );
                assert_eq!(
                    text(&settled["connection"], "bandwidth"),
                    expected_bandwidth,
                    "case {case:?}: expected the applied bandwidth to survive settling, got {:?}",
                    settled["connection"]["bandwidth"]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The other side of the rule: a repeat that asks for something else
// ---------------------------------------------------------------------------

/// RULE: the permitted-state rule still refuses a request that asks for a CHANGE the
/// connection's state cannot take — a second, different update sent while the circuit
/// is being re-provisioned is `InterconnectClientException` (400) stating that rule,
/// and it changes nothing.
#[tokio::test]
async fn a_different_update_is_still_refused_while_the_circuit_is_in_flight() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator.attach_point().await;

    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let bandwidths = available_bandwidths(&environment);
        let created_with = bandwidths[0].clone();
        let Some(other) = bandwidths
            .iter()
            .find(|candidate| **candidate != created_with)
            .cloned()
        else {
            continue;
        };

        // Rows differ in WHAT the second request asks for: another description, and the
        // bandwidth the connection is being moved off — each a change the record does
        // not hold, so each is refused while the change is in flight.
        for (label, asked) in [
            (
                "another description",
                json!({ "description": "changed mid flight" }),
            ),
            (
                "the previous bandwidth",
                json!({ "bandwidth": created_with.clone() }),
            ),
        ] {
            let case = format!("{environment_id} / {label}");
            let created = emulator
                .create_connection(&environment_id, &created_with, &gateway, "in flight subject")
                .await;
            let id = text(&created, "id");
            let in_flight = emulator
                .ic_ok(
                    "UpdateConnection",
                    json!({ "identifier": id, "bandwidth": other, "clientToken": "flight" }),
                )
                .await;
            assert_eq!(
                text(&in_flight["connection"], "state"),
                "updating",
                "case {case:?}: expected a bandwidth change to report the transient state, got {:?}",
                in_flight["connection"]["state"]
            );

            let mut body = asked.as_object().cloned().unwrap_or_default();
            body.insert("identifier".to_string(), json!(id));
            let (status, refused) = emulator.ic("UpdateConnection", Value::Object(body)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a change the in-flight state cannot take, got {status} with {refused:?}"
            );
            assert!(
                code(&refused).contains("InterconnectClientException"),
                "case {case:?}: expected InterconnectClientException, got {:?}",
                code(&refused)
            );
            assert_eq!(
                message(&refused),
                connection_not_updatable(&id),
                "case {case:?}: expected the permitted-state rule in the message, got {:?}",
                message(&refused)
            );

            let fetched = emulator.get_connection(&id).await.1;
            for member in ["bandwidth", "description"] {
                assert_eq!(
                    fetched["connection"][member], in_flight["connection"][member],
                    "case {case:?}: expected the refused change to leave {member} alone, got {:?} (was {:?})",
                    fetched["connection"][member], in_flight["connection"][member]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Where the connection is gone, a repeat has no record to report
// ---------------------------------------------------------------------------

/// RULE: a connection that is being deleted, deleted or failed has no record an update
/// reports at all — even a request asking for exactly what that record holds is refused
/// with the permitted-state message, while the very same request is answered on a live
/// connection.
#[tokio::test]
async fn a_no_change_update_is_refused_once_the_connection_is_gone() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateway = emulator.attach_point().await;

    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let bandwidth = available_bandwidths(&environment)[0].clone();
        let description = "already what it holds";
        let created = emulator
            .create_connection(&environment_id, &bandwidth, &gateway, description)
            .await;
        let id = text(&created, "id");
        // The request asks for exactly what the connection holds, so nothing but the
        // connection's own state decides the answer.
        let no_change = json!({
            "identifier": id,
            "description": description,
            "bandwidth": bandwidth,
            "clientToken": "no-change",
        });

        // Positive control: on a live connection it is answered.
        let answered = emulator.ic_ok("UpdateConnection", no_change.clone()).await;
        assert_eq!(
            text(&answered["connection"], "id"),
            id,
            "case {environment_id:?}: expected the no-change update to report the connection, got {:?}",
            answered["connection"]
        );

        emulator
            .ic_ok("DeleteConnection", json!({ "identifier": id }))
            .await;
        // Rows: the connection on its way out, and the connection gone.
        for stage in ["being deleted", "deleted"] {
            if stage == "deleted" {
                tokio::time::sleep(SETTLE_WAIT).await;
            }
            let case = format!("{environment_id} / {stage}");
            let (status, body) = emulator.ic("UpdateConnection", no_change.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for an update of a connection that is gone, got {status} with {body:?}"
            );
            assert_eq!(
                message(&body),
                connection_not_updatable(&id),
                "case {case:?}: expected the permitted-state rule in the message, got {:?}",
                message(&body)
            );
        }
    }
}
