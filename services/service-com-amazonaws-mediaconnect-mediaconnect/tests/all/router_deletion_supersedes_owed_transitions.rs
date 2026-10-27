// SPDX-License-Identifier: CC0-1.0

//! What an accepted router delete does to the state change the clock still owed, end to end
//! over HTTP.
//!
//! The rule the cases below pin: once a delete of a resource that is removed asynchronously
//! has been accepted, that acceptance supersedes any state change the clock still owed the
//! record — so the record reports the `DELETING` state the delete answered
//! (`DeleteRouterInputResponse$State`, "the current state of the deleted router input,
//! indicating where it is in the deletion process",
//! `smithy_query(query="doc",shape="DeleteRouterInputResponse",member="State")`, and the same
//! member on the router output and router network interface responses) for every later read
//! until the record stops existing, never a running state an earlier call had owed it — and
//! the accepted delete is itself a change to the record, so it moves the record's
//! `UpdatedAt`.
//!
//! The rows differ along what the rule generalises over: which of the three router resources
//! whose delete is asynchronous is addressed (router input, router output, router network
//! interface, each read back through that resource's own implemented reader), which transient
//! state the owed change had left the record in and which state it was owed to settle into,
//! and whether the clock passes that owed instant before or after the delete is accepted —
//! the window the second case drives, where the owed instant falls BETWEEN the delete and the
//! read.
//!
//! The owed change is written onto the record at an instant the case chooses — the shape
//! `StartRouterInput` / `RestartRouterInput` / `UpdateRouterInput` and their router output
//! and interface counterparts leave behind
//! (`mc::StoredRouterInput::begin_transition` writes exactly this, at
//! `now + mc::TRANSITION_SPAN`) — the way `tests/all/get_router_input.rs` owes one, so a case
//! is about what the DELETE does with an owed change rather than about racing a 500 ms span.
//! Every call under test travels the whole request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`].

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_deletion::seed_router_input;
use crate::router_network_interfaces::{
    batch_get_network_interfaces, create_network_interface, public_configuration,
};
use crate::router_outputs::{create_router_output, minted_network_interface, standard};

/// Far enough ahead that an owed change is NOT due at any point during a case: what the
/// delete must supersede rather than wait out.
const NOT_DUE_FOR_A_WHILE: Duration = Duration::from_secs(600);

/// An owed instant that falls INSIDE the window the delete opens: short enough that the
/// clock passes it while the record is still `DELETING` (the removal is owed one
/// [`mc::TRANSITION_SPAN`] after the delete).
fn owed_soon() -> Duration {
    mc::TRANSITION_SPAN / 8
}

/// Long enough for [`owed_soon`] to have passed, and short enough that the removal the
/// accepted delete owes has not.
fn past_the_owed_instant() -> Duration {
    mc::TRANSITION_SPAN / 2
}

/// The three router resources whose delete is accepted before it completes: each carries a
/// `State`, can have a state change the clock still owes it, and stops existing one
/// [`mc::TRANSITION_SPAN`] after its delete is accepted — so the rule has to hold for all
/// three.
#[derive(Clone, Copy, Debug)]
enum Router {
    Input,
    Output,
    Interface,
}

impl Router {
    /// The `DELETING` value of this resource's own state enum — the state its delete reports
    /// and, by the rule under test, the state every later read answers.
    fn deleting(self) -> &'static str {
        match self {
            Router::Input => mc::ROUTER_INPUT_STATE_DELETING,
            Router::Output => mc::ROUTER_OUTPUT_STATE_DELETING,
            Router::Interface => mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING,
        }
    }
}

/// One row of the tables below: which resource the delete addresses, the transient state the
/// owed change left the record in, and the state that change was owed to settle it into.
struct Row {
    case: &'static str,
    kind: Router,
    transient: &'static str,
    settled: &'static str,
}

/// The rows span the class the rule generalises over: all three asynchronously deleted router
/// resources, and the owed changes their own operations leave behind — a start, a restart, a
/// stop, a reconfiguration and a creation still in flight — settling into both of the running
/// states such a change can be owed to reach.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a router input whose start is still in flight",
            kind: Router::Input,
            transient: mc::ROUTER_INPUT_STATE_STARTING,
            settled: mc::ROUTER_INPUT_STATE_ACTIVE,
        },
        Row {
            case: "a router input whose restart is still in flight",
            kind: Router::Input,
            transient: mc::ROUTER_INPUT_STATE_RECOVERING,
            settled: mc::ROUTER_INPUT_STATE_ACTIVE,
        },
        Row {
            case: "a router input whose reconfiguration is still in flight",
            kind: Router::Input,
            transient: mc::ROUTER_INPUT_STATE_UPDATING,
            settled: mc::ROUTER_INPUT_STATE_STANDBY,
        },
        Row {
            case: "a router input that is still being created",
            kind: Router::Input,
            transient: mc::ROUTER_INPUT_STATE_CREATING,
            settled: mc::ROUTER_INPUT_STATE_STANDBY,
        },
        Row {
            case: "a router output whose start is still in flight",
            kind: Router::Output,
            transient: mc::ROUTER_OUTPUT_STATE_STARTING,
            settled: mc::ROUTER_OUTPUT_STATE_ACTIVE,
        },
        Row {
            case: "a router output whose stop is still in flight",
            kind: Router::Output,
            transient: mc::ROUTER_OUTPUT_STATE_STOPPING,
            settled: mc::ROUTER_OUTPUT_STATE_STANDBY,
        },
        Row {
            case: "a router network interface whose reconfiguration is still in flight",
            kind: Router::Interface,
            transient: mc::ROUTER_NETWORK_INTERFACE_STATE_UPDATING,
            settled: mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE,
        },
        Row {
            case: "a router network interface that is still being created",
            kind: Router::Interface,
            transient: mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING,
            settled: mc::ROUTER_NETWORK_INTERFACE_STATE_ACTIVE,
        },
    ]
}

/// Put one record of `kind` in the store and answer the ARN it is addressed by.
///
/// The router output and the router network interface are minted over the wire through the
/// creates their own suites own, so what a case acts on is a record a client could really
/// have received; the router input is seeded through the shared
/// [`seed_router_input`], the way the router input suites seed a record whose state a create
/// never answers.
async fn mint(emulator: &Emulator, kind: Router, name: &str) -> String {
    match kind {
        Router::Input => seed_router_input(emulator, name, mc::ROUTER_INPUT_STATE_STANDBY),
        Router::Output => {
            let interface_arn = minted_network_interface(emulator, &format!("{name}-rni")).await;
            let body = json!({
                "name": name,
                "configuration": standard(
                    &interface_arn,
                    json!({"Rtp": {"destinationAddress": "198.51.100.10",
                        "destinationPort": 5004}}),
                ),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            });
            let (status, code, created) = create_router_output(emulator, body).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "CreateRouterOutput expected 201 for {name:?}, got {status} {code:?}: {created}"
            );
            created["routerOutput"]["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("the minted router output carries its ARN: {created}"))
                .to_string()
        }
        Router::Interface => {
            let body = json!({ "name": name, "configuration": public_configuration(1) });
            let (status, code, created) = create_network_interface(emulator, body).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "CreateRouterNetworkInterface expected 201 for {name:?}, got {status} {code:?}: \
                 {created}"
            );
            created["routerNetworkInterface"]["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("the minted interface carries its ARN: {created}"))
                .to_string()
        }
    }
}

/// Owe the stored record `arn` the state change `settled`, entering `transient` now and due
/// at `due_at` — the record a call that starts a transition leaves behind.
fn owe_transition(
    emulator: &Emulator,
    kind: Router,
    arn: &str,
    transient: &str,
    settled: &str,
    due_at: SystemTime,
) {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = store.lock();
    let pending = Some(mc::PendingStatus {
        status: settled.to_string(),
        due_at,
    });
    match kind {
        Router::Input => {
            let record = store
                .router_inputs
                .get_mut(arn)
                .expect("the store holds the router input under test");
            record.state = transient.to_string();
            record.pending = pending;
        }
        Router::Output => {
            let record = store
                .router_outputs
                .get_mut(arn)
                .expect("the store holds the router output under test");
            record.state = transient.to_string();
            record.pending = pending;
        }
        Router::Interface => {
            let record = store
                .router_network_interfaces
                .get_mut(arn)
                .expect("the store holds the router network interface under test");
            record.state = transient.to_string();
            record.pending = pending;
        }
    }
}

/// Whether the store still owes the record `arn` a state change.
fn transition_is_owed(emulator: &Emulator, kind: Router, arn: &str) -> bool {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    match kind {
        Router::Input => store
            .router_inputs
            .get(arn)
            .is_some_and(|record| record.pending.is_some()),
        Router::Output => store
            .router_outputs
            .get(arn)
            .is_some_and(|record| record.pending.is_some()),
        Router::Interface => store
            .router_network_interfaces
            .get(arn)
            .is_some_and(|record| record.pending.is_some()),
    }
}

/// Whether the store owes the record `arn` its removal: what an accepted delete records.
fn removal_is_owed(emulator: &Emulator, kind: Router, arn: &str) -> bool {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    match kind {
        Router::Input => store
            .router_inputs
            .get(arn)
            .is_some_and(|record| record.is_deleting()),
        Router::Output => store
            .router_outputs
            .get(arn)
            .is_some_and(|record| record.is_deleting()),
        Router::Interface => store
            .router_network_interfaces
            .get(arn)
            .is_some_and(|record| record.is_deleting()),
    }
}

/// Send one delete of `arn` over the wire: `DELETE /v1/routerInput/{Arn}`,
/// `DELETE /v1/routerOutput/{Arn}` or `DELETE /v1/routerNetworkInterface/{Arn}`, each with
/// `Arn` bound as an `@httpLabel`.
async fn delete(emulator: &Emulator, kind: Router, arn: &str) -> (u16, Option<String>, Value) {
    let uri = match kind {
        Router::Input => format!("/v1/routerInput/{arn}"),
        Router::Output => format!("/v1/routerOutput/{arn}"),
        Router::Interface => format!("/v1/routerNetworkInterface/{arn}"),
    };
    emulator.send("DELETE", &uri, None).await
}

/// Read `arn` back through this resource's own implemented reader and answer the HTTP status
/// with the resource the reader reported (`Value::Null` when it reported none).
///
/// GetRouterInput and GetRouterOutput read one resource each; GetRouterNetworkInterface is
/// still scaffolded, so an interface is read back through BatchGetRouterNetworkInterface, the
/// way `tests/all/router_network_interface_deletion.rs` reads one.
async fn read_back(emulator: &Emulator, kind: Router, arn: &str) -> (u16, Value) {
    match kind {
        Router::Input => {
            let (status, _, body) = emulator
                .send("GET", &format!("/v1/routerInput/{arn}"), None)
                .await;
            (status, body["routerInput"].clone())
        }
        Router::Output => {
            let (status, _, body) = emulator
                .send("GET", &format!("/v1/routerOutput/{arn}"), None)
                .await;
            (status, body["routerOutput"].clone())
        }
        Router::Interface => {
            let (status, _, body) = batch_get_network_interfaces(emulator, &[arn]).await;
            (status, body["routerNetworkInterfaces"][0].clone())
        }
    }
}

/// The `State` the reader reported for a resource.
fn reported_state(resource: &Value) -> Option<&str> {
    resource.get("state").and_then(Value::as_str)
}

/// The `UpdatedAt` the reader reported for a resource — the member an accepted delete has to
/// move, read as the epoch seconds this emulator's restJson1 codec renders every timestamp
/// in.
fn reported_updated_at(case: &str, resource: &Value) -> f64 {
    resource
        .get("updatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports UpdatedAt: {resource}"))
}

/// An accepted delete supersedes the state change the clock still owed the record: the store
/// owes that change no longer, the record reports `DELETING`, and the delete moved
/// `UpdatedAt`.
#[tokio::test]
async fn an_accepted_delete_supersedes_the_state_change_the_clock_still_owed() {
    let emulator = Emulator::new();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let arn = mint(&emulator, row.kind, &format!("superseded-{index}")).await;
        let (status, before) = read_back(&emulator, row.kind, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the minted record is readable before the delete, got {status}: \
             {before}"
        );
        let updated_before = reported_updated_at(case, &before);

        // The owed change is not due at any point in this case, so nothing but the delete can
        // account for the record no longer owing it.
        owe_transition(
            &emulator,
            row.kind,
            &arn,
            row.transient,
            row.settled,
            SystemTime::now() + NOT_DUE_FOR_A_WHILE,
        );
        assert!(
            transition_is_owed(&emulator, row.kind, &arn),
            "case {case:?}: the store owes the record a change before the delete"
        );

        let (status, code, deleted) = delete(&emulator, row.kind, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the delete is served, got {status} {code:?}: {deleted}"
        );
        assert_eq!(
            deleted["state"].as_str(),
            Some(row.kind.deleting()),
            "case {case:?}: the delete reports the deletion in progress, got {deleted}"
        );

        assert!(
            !transition_is_owed(&emulator, row.kind, &arn),
            "case {case:?}: the accepted delete supersedes the owed change, so the store owes \
             it no longer"
        );
        assert!(
            removal_is_owed(&emulator, row.kind, &arn),
            "case {case:?}: the accepted delete owes the record's removal instead"
        );

        let (status, after) = read_back(&emulator, row.kind, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a record that is still DELETING is still readable, got {status}: \
             {after}"
        );
        assert_eq!(
            reported_state(&after),
            Some(row.kind.deleting()),
            "case {case:?}: expected the reader to report {:?}, got {after}",
            row.kind.deleting()
        );
        let updated_after = reported_updated_at(case, &after);
        assert!(
            updated_after > updated_before,
            "case {case:?}: the accepted delete moves UpdatedAt, expected a value past \
             {updated_before}, got {updated_after}"
        );
    }
}

/// The window the rule is about: the owed instant falls BETWEEN the delete and the read, so a
/// record whose owed change the delete did not supersede would report the running state it
/// was owed. It reports the `DELETING` the delete answered instead, for every one of the three
/// resources.
#[tokio::test]
async fn an_owed_change_coming_due_after_the_delete_does_not_revive_the_record() {
    let emulator = Emulator::new();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let arn = mint(&emulator, row.kind, &format!("in-the-window-{index}")).await;
        // Due inside the window the delete below opens: the clock passes this instant while
        // the record is still DELETING.
        owe_transition(
            &emulator,
            row.kind,
            &arn,
            row.transient,
            row.settled,
            SystemTime::now() + owed_soon(),
        );

        let (status, code, deleted) = delete(&emulator, row.kind, &arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: the delete is served, got {status} {code:?}: {deleted}"
        );

        tokio::time::sleep(past_the_owed_instant()).await;

        let (status, after) = read_back(&emulator, row.kind, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read inside the deletion window is served, got {status}: {after}"
        );
        assert_eq!(
            reported_state(&after),
            Some(row.kind.deleting()),
            "case {case:?}: expected {:?} past the owed instant and inside the deletion \
             window, not the {:?} the superseded change was owed, got {after}",
            row.kind.deleting(),
            row.settled
        );
    }
}
