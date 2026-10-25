// SPDX-License-Identifier: CC0-1.0

//! How a Connection the caller creates WALKS the documented `ConnectionState`
//! progression, driven over the wire through the shared `crate::emulator` harness.
//!
//! Each test asserts a RULE over a TABLE whose rows differ along what the rule
//! generalises over — the environment (and so the provider kind and the
//! `ConnectionId` prefix), the bandwidth, the attach point, and whether the optional
//! request members are present. The rule itself is the progression the model
//! documents for `Connection$state`, not any one row's timing.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::emulator::{available_bandwidths, placements, remote_account_for, text, Emulator};

/// The states a connection being brought up walks, in order, as
/// `smithy_query(query="doc",shape="Connection",member="state")` documents them:
/// "requested: the initial state of a connection" -> "pending: the connection has
/// been accepted and is being provisioned between AWS and the Partner" ->
/// "available: the connection has been fully provisioned between AWS and the
/// Partner".
const PROGRESSION: [&str; 3] = ["requested", "pending", "available"];

/// How long a poll waits for the progression to complete before the case fails.
///
/// Well past the spans the service owes, so a connection is proved to converge
/// PROMPTLY rather than eventually.
const POLL_BUDGET: Duration = Duration::from_secs(5);

/// How often a poll re-reads while it waits — short enough to observe a leg of the
/// progression rather than only its end.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Long enough to cover BOTH legs of the progression (250 ms each) with margin, so a
/// read after this wait observes the state the clock owed with nobody polling.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// Where `state` sits on the documented progression, or `None` for a state that is
/// not part of bringing a connection up.
fn position(state: &str) -> Option<usize> {
    PROGRESSION.iter().position(|known| *known == state)
}

/// The gateways the rows below attach to: an attach point IS a Direct Connect gateway
/// of the account, so the suite creates its own and the rows differ in which one they
/// use, in each gateway's name and in its `amazonSideAsn`.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] = [
    ("ic-progression-a", None),
    ("ic-progression-b", Some(64_512)),
];

/// Create a connection whose request carries only the members the model marks
/// required, plus the identifying information the named Environment itself asks for —
/// so a row can differ in whether the OPTIONAL ones are present.
async fn create_minimal(
    emulator: &Emulator,
    environment_id: &str,
    bandwidth: &str,
    attach_point: &str,
) -> Value {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(environment_id));
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": attach_point }),
    );
    if let Some(remote_account) = remote_account_for(&emulator.environment(environment_id).await) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    let created = emulator
        .ic_ok("CreateConnection", Value::Object(request))
        .await;
    created["connection"].clone()
}

/// Poll `identifier` until it reports `available`, returning the distinct states
/// observed in the order they appeared.
///
/// Asserts as it goes that every state is one of the documented progression and that
/// the connection only ever moves FORWARD along it.
async fn walk_until_available(emulator: &Emulator, identifier: &str, case: &str) -> Vec<String> {
    let started = Instant::now();
    let mut observed: Vec<String> = Vec::new();
    loop {
        let (status, body) = emulator.get_connection(identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the connection to stay readable while it provisions, got {status} with {body:?}"
        );
        let state = text(&body["connection"], "state");
        let Some(reached) = position(&state) else {
            panic!(
                "case {case:?}: expected a state on the documented progression {PROGRESSION:?}, got {state:?}"
            );
        };
        if observed.last().is_none_or(|last| *last != state) {
            if let Some(last) = observed.last() {
                let previous = position(last).expect("an observed state is on the progression");
                assert!(
                    reached > previous,
                    "case {case:?}: expected the state to advance along {PROGRESSION:?}, got {state:?} after {last:?}"
                );
            }
            observed.push(state.clone());
        }
        if state == "available" {
            return observed;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: expected the connection to reach 'available' within {POLL_BUDGET:?}, it is still {state:?} after observing {observed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// The progression a created connection walks
// ---------------------------------------------------------------------------

/// RULE: a connection is returned in the first state of the documented progression
/// and is then carried along it by the clock — `requested` -> `pending` ->
/// `available` — reaching the fully provisioned state within a bounded time, never
/// moving backwards and never reporting a state outside the progression.
#[tokio::test]
async fn a_created_connection_walks_the_documented_progression_to_available() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;

    // Rows differ in the environment (and so the provider kind, the location and the
    // `ConnectionId` prefix), in the bandwidth, in the attach point, and in whether
    // the optional request members are present. Each row is walked as soon as it is
    // created, so the walk starts where the create left it.
    let mut rows = 0usize;
    let mut rows_that_saw_pending: Vec<String> = Vec::new();
    for (seed, (environment_id, bandwidth)) in placements.iter().enumerate() {
        let attach_point = gateways[seed % gateways.len()].clone();
        for shape in ["every optional member", "required members only"] {
            let case = format!("{environment_id} / {bandwidth} / {shape}");
            let connection = if shape == "required members only" {
                create_minimal(&emulator, environment_id, bandwidth, &attach_point).await
            } else {
                emulator
                    .create_connection(environment_id, bandwidth, &attach_point, "progression row")
                    .await
            };
            rows += 1;

            // The row STARTS at the documented initial state...
            assert_eq!(
                text(&connection, "state"),
                PROGRESSION[0],
                "case {case:?}: expected a created connection to report the initial state {:?}, got {:?}",
                PROGRESSION[0],
                connection["state"]
            );

            // ...and ENDS at the fully provisioned one, having walked forward only.
            let identifier = text(&connection, "id");
            let observed = walk_until_available(&emulator, &identifier, &case).await;
            assert_eq!(
                observed.first().map(String::as_str),
                Some(PROGRESSION[0]),
                "case {case:?}: expected the walk to start at {:?}, got {observed:?}",
                PROGRESSION[0]
            );
            assert_eq!(
                observed.last().map(String::as_str),
                Some("available"),
                "case {case:?}: expected the walk to end at 'available', got {observed:?}"
            );
            if observed.iter().any(|state| state == "pending") {
                rows_that_saw_pending.push(case);
            }
        }
    }
    assert!(
        rows >= 4,
        "expected several rows to walk, got {rows}"
    );

    // The middle leg is REACHABLE: a poll that keeps up with the clock sees the
    // accepted-and-provisioning state, not just the two ends of the progression. One
    // row is enough to prove the leg exists — a poll that is descheduled for longer
    // than the leg lasts legitimately misses it.
    assert!(
        !rows_that_saw_pending.is_empty(),
        "expected at least one of the {rows} rows to be observed in 'pending' while it provisioned, none was"
    );
}

/// RULE: the progression is owed by the CLOCK, so it completes whether or not anybody
/// polls, and `available` is where it STOPS: a connection that has been read as
/// available stays available.
#[tokio::test]
async fn the_progression_completes_with_nobody_polling_and_stops_at_available() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;

    // Created first, read only after the whole progression was due: no call in
    // between could have advanced anything.
    let mut created: Vec<(String, String)> = Vec::new();
    for (seed, (environment_id, bandwidth)) in placements.iter().enumerate() {
        let connection = emulator
            .create_connection(
                environment_id,
                bandwidth,
                &gateways[seed % gateways.len()],
                "settles with nobody polling",
            )
            .await;
        created.push((
            format!("{environment_id} / {bandwidth}"),
            text(&connection, "id"),
        ));
    }

    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, identifier) in &created {
        let fetched = emulator
            .ic_ok("GetConnection", json!({ "identifier": identifier }))
            .await;
        assert_eq!(
            text(&fetched["connection"], "state"),
            "available",
            "case {case:?}: expected the clock to have provisioned the connection with nobody polling, got {:?}",
            fetched["connection"]["state"]
        );
    }

    // `available` is the end of the progression: a second wait moves nothing.
    tokio::time::sleep(SETTLE_WAIT).await;
    for (case, identifier) in &created {
        let fetched = emulator
            .ic_ok("GetConnection", json!({ "identifier": identifier }))
            .await;
        assert_eq!(
            text(&fetched["connection"], "state"),
            "available",
            "case {case:?}: expected 'available' to be where the progression stops, got {:?}",
            fetched["connection"]["state"]
        );
    }

    // A listing reports the SAME state the read does, and its `state` filter
    // separates the stages the lifecycle actually reaches.
    let identifiers: Vec<&String> = created.iter().map(|(_, id)| id).collect();
    for (wanted, should_contain) in [
        ("available", true),
        ("requested", false),
        ("pending", false),
        ("updating", false),
        ("deleting", false),
        ("deleted", false),
        ("down", false),
        ("failed", false),
    ] {
        let listed = emulator
            .ic_ok("ListConnections", json!({ "state": wanted }))
            .await;
        let summaries = listed["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for identifier in &identifiers {
            let present = summaries
                .iter()
                .any(|summary| summary["id"] == json!(identifier));
            assert_eq!(
                present, should_contain,
                "case {identifier:?}/state={wanted}: expected present={should_contain}, got {summaries:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the end of the progression unlocks
// ---------------------------------------------------------------------------

/// RULE: `available` is one of the states an update is permitted from, so an update
/// of a fully provisioned connection is accepted, is persisted, and a bandwidth
/// change runs it through `updating` back to `available` — not back to the state it
/// was created in.
#[tokio::test]
async fn an_available_connection_can_be_updated_and_settles_back_to_available() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    // Rows differ in the environment and in the pair of bandwidths the change moves
    // between; only an environment offering two available bandwidths can have one
    // changed.
    let mut rows = 0usize;
    for (seed, environment) in catalog.iter().enumerate() {
        let bandwidths = available_bandwidths(environment);
        if bandwidths.len() < 2 {
            continue;
        }
        rows += 1;
        let environment_id = text(environment, "environmentId");
        let case = format!("{environment_id} / {} -> {}", bandwidths[0], bandwidths[1]);

        let connection = emulator
            .create_connection(
                &environment_id,
                &bandwidths[0],
                &gateways[seed % gateways.len()],
                "updated once available",
            )
            .await;
        let identifier = text(&connection, "id");
        let observed = walk_until_available(&emulator, &identifier, &case).await;
        assert_eq!(
            observed.last().map(String::as_str),
            Some("available"),
            "case {case:?}: expected the connection to be available before the update, got {observed:?}"
        );

        // A description-only change touches no state: an available connection stays
        // available.
        let renamed = emulator
            .ic_ok(
                "UpdateConnection",
                json!({ "identifier": identifier, "description": "renamed while available" }),
            )
            .await;
        assert_eq!(
            text(&renamed["connection"], "description"),
            "renamed while available",
            "case {case:?}: expected the update of an available connection to persist, got {:?}",
            renamed["connection"]["description"]
        );
        assert_eq!(
            text(&renamed["connection"], "state"),
            "available",
            "case {case:?}: expected a description-only change to leave the state alone, got {:?}",
            renamed["connection"]["state"]
        );

        // A bandwidth change re-provisions the circuit and the clock settles it back
        // into the state it came from, which is now `available`.
        let changed = emulator
            .ic_ok(
                "UpdateConnection",
                json!({ "identifier": identifier, "bandwidth": bandwidths[1] }),
            )
            .await;
        assert_eq!(
            text(&changed["connection"], "state"),
            "updating",
            "case {case:?}: expected a bandwidth change to report 'updating', got {:?}",
            changed["connection"]["state"]
        );

        let started = Instant::now();
        let settled = loop {
            let fetched = emulator
                .ic_ok("GetConnection", json!({ "identifier": identifier }))
                .await;
            let state = text(&fetched["connection"], "state");
            if state != "updating" {
                break fetched["connection"].clone();
            }
            assert!(
                started.elapsed() < POLL_BUDGET,
                "case {case:?}: expected the update to settle within {POLL_BUDGET:?}, it is still {state:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            text(&settled, "state"),
            "available",
            "case {case:?}: expected the update to settle back into 'available', got {:?}",
            settled["state"]
        );
        // The round trip: the read reports the change the update made.
        assert_eq!(
            text(&settled, "bandwidth"),
            bandwidths[1],
            "case {case:?}: expected the new bandwidth to survive settling, got {:?}",
            settled["bandwidth"]
        );
        assert_eq!(
            text(&settled, "description"),
            "renamed while available",
            "case {case:?}: expected the description to survive settling, got {:?}",
            settled["description"]
        );
    }

    assert!(
        rows > 0,
        "expected at least one environment offering two available bandwidths to change between, got none"
    );
}
