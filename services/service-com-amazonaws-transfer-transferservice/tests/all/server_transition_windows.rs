// SPDX-License-Identifier: CC0-1.0

//! The transition a server state change begins, and the window it is in flight for.
//!
//! Two rules, each asserted over a TABLE rather than over one call:
//!
//! 1. what a state change does is decided by the state the server is IN — it begins its
//!    transition from the state it moves the server out of and from its own failure
//!    state, it has no impact on a server already in or already on its way to the state
//!    it targets, and it is refused when the server is in flight the other way
//!    ([`the_state_a_server_is_in_decides_what_a_state_change_does`]);
//! 2. the window such a transition is in flight for lasts the span the service gives it
//!    — so a caller that acts straight after the call that began it, and keeps calling,
//!    still observes the intermediate state and the refusal that depends on it, and the
//!    terminal state arrives only once the span comes due
//!    ([`every_clock_owed_window_outlasts_a_caller_that_keeps_calling`]).
//!
//! Rule 1's rows are the whole matrix of stored state x state-changing call, pinned
//! through the store so each row asserts the SERVICE's decision and not a race with the
//! clock. Rule 2's rows are the three windows this service opens, each measured on its
//! own server.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::state as st;

use crate::harness::{member_of, Emulator, SETTLE_MARGIN};

/// How far out a pinned in-flight state's due instant is put: far enough that no delay
/// inside this test can settle it, so a rule-1 row asserts the decision the service
/// makes rather than what the clock did meanwhile.
const PINNED_WINDOW: Duration = Duration::from_secs(3_600);

/// How many calls a caller makes straight after the call that opened a window, before
/// it looks at the state again. More than one, because the window has to outlast a
/// client's whole next burst of requests, not just its very next one.
const CALLS_WHILE_IN_FLIGHT: usize = 16;

/// One server's `State` as both reporting operations report it.
///
/// A transition is reported by whatever reads the server, so every row is asserted
/// through `DescribeServer` AND through `ListServers` rather than through one of them.
async fn reported_states(emulator: &Emulator, case: &str, server_id: &str) -> (String, String) {
    let described = emulator
        .call("DescribeServer", json!({"ServerId": server_id}))
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected DescribeServer 200, got {} with body {}",
        described.status, described.body
    );
    let listed = emulator.call("ListServers", json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected ListServers 200, got {} with body {}",
        listed.status, listed.body
    );
    let in_listing = listed
        .get("Servers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|server| server.get("ServerId").and_then(Value::as_str) == Some(server_id))
        .and_then(|server| server.get("State").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    (
        member_of(described.get("Server"), "State")
            .as_str()
            .unwrap_or_default()
            .to_string(),
        in_listing,
    )
}

/// The transition the stored server owes the clock, as the store holds it.
fn pending_transition(emulator: &Emulator, server_id: &str) -> Option<(Instant, String)> {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .servers
        .get(server_id)
        .expect("the test's own server is in the store")
        .state_due
        .clone()
}

/// Put `state` into the stored server, with the transition a server in that state owes
/// still pending, and hand back the due instant it was given.
///
/// Written through the store the handlers themselves read, because these states cannot
/// be reached over the wire: nothing in this service writes `START_FAILED` or
/// `STOP_FAILED` (the shared rule names them so a retry from one begins a transition),
/// and an in-flight state cannot be held still by any call.
fn pin_state(emulator: &Emulator, server_id: &str, state: &str) -> Option<(Instant, String)> {
    let owed = match state {
        st::SERVER_STATE_STARTING => Some(st::SERVER_STATE_ONLINE),
        st::SERVER_STATE_STOPPING => Some(st::SERVER_STATE_OFFLINE),
        _ => None,
    };
    let pending = owed.map(|next| (Instant::now() + PINNED_WINDOW, next.to_string()));
    let store = emulator.store();
    let mut guard = store.lock();
    let server = guard
        .servers
        .get_mut(server_id)
        .expect("the test's own server is in the store");
    server.state = state.to_string();
    server.state_due = pending.clone();
    pending
}

/// One row of the transition matrix.
struct Row {
    /// The `State` the server is in when the call arrives.
    state: &'static str,
    /// The state-changing call made on it.
    op: &'static str,
    /// The HTTP status it answers.
    status: u16,
    /// The `__type` of the refusal, or the empty string for a call that succeeds.
    error: &'static str,
    /// The `State` every reader reports once the call has been answered.
    after: &'static str,
}

/// RULE: a state-changing call decides from the state the server is IN. `StopServer`
/// begins a stop from `ONLINE` and from `STOP_FAILED`, has no impact on a server that is
/// already `OFFLINE` or already `STOPPING`, and is refused with the modelled
/// `InvalidRequestException` on a server in flight the other way; `StartServer` is the
/// same rule with the directions exchanged. A call that has no impact, and a call that
/// is refused, leave the stored state and the transition it owes exactly as they were.
///
/// The rows are the whole matrix: every `State` the rule classifies, against both calls.
#[tokio::test]
async fn the_state_a_server_is_in_decides_what_a_state_change_does() {
    let emulator = Emulator::new();

    // `State`'s values come from the service's own constants, so a row cannot name a
    // state the emulator does not use.
    let starting = st::SERVER_STATE_STARTING;
    let online = st::SERVER_STATE_ONLINE;
    let stopping = st::SERVER_STATE_STOPPING;
    let offline = st::SERVER_STATE_OFFLINE;
    let start_failed = st::SERVER_STATE_START_FAILED;
    let stop_failed = st::SERVER_STATE_STOP_FAILED;
    let refused = "InvalidRequestException";

    let rows = [
        // StopServer: the stop begins from ONLINE and from the failure of a stop.
        Row { state: online, op: "StopServer", status: 200, error: "", after: stopping },
        Row { state: stop_failed, op: "StopServer", status: 200, error: "", after: stopping },
        // It has no impact where there is nothing left to do.
        Row { state: offline, op: "StopServer", status: 200, error: "", after: offline },
        Row { state: stopping, op: "StopServer", status: 200, error: "", after: stopping },
        // And no stop to make on a server on its way online.
        Row { state: starting, op: "StopServer", status: 400, error: refused, after: starting },
        Row {
            state: start_failed,
            op: "StopServer",
            status: 400,
            error: refused,
            after: start_failed,
        },
        // StartServer: the same rule, the other way round.
        Row { state: offline, op: "StartServer", status: 200, error: "", after: starting },
        Row { state: start_failed, op: "StartServer", status: 200, error: "", after: starting },
        Row { state: online, op: "StartServer", status: 200, error: "", after: online },
        Row { state: starting, op: "StartServer", status: 200, error: "", after: starting },
        Row { state: stopping, op: "StartServer", status: 400, error: refused, after: stopping },
        Row {
            state: stop_failed,
            op: "StartServer",
            status: 400,
            error: refused,
            after: stop_failed,
        },
    ];

    for row in rows {
        let case = format!("{} on a {} server", row.op, row.state);
        // One server per row, so no row observes another's transition.
        let server_id = emulator.create_server(json!({})).await;
        let pinned = pin_state(&emulator, &server_id, row.state);

        let answer = emulator
            .call(row.op, json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            row.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            row.error,
            "case {case:?}: expected error code {:?}, got {}",
            row.error,
            answer.body
        );

        let (described, listed) = reported_states(&emulator, &case, &server_id).await;
        assert_eq!(
            (described.as_str(), listed.as_str()),
            (row.after, row.after),
            "case {case:?}: expected {} from both readers, got described {described:?} and \
             listed {listed:?}",
            row.after
        );

        let pending = pending_transition(&emulator, &server_id);
        if row.after == row.state {
            // Nothing was begun, so nothing was owed that was not owed before.
            assert_eq!(
                pending.as_ref().map(|(at, next)| (*at, next.clone())),
                pinned.as_ref().map(|(at, next)| (*at, next.clone())),
                "case {case:?}: expected the pending transition left alone, got {:?} where \
                 {:?} was stored",
                pending.as_ref().map(|(_, next)| next),
                pinned.as_ref().map(|(_, next)| next)
            );
        } else {
            // A transition WAS begun, so it owes its terminal state within this call's
            // own span — not within whatever was pending before.
            let span = if row.op == "StopServer" {
                st::SERVER_STOP_SPAN
            } else {
                st::SERVER_START_SPAN
            };
            let (at, next) = pending
                .unwrap_or_else(|| panic!("case {case:?}: expected a transition to be owed"));
            assert!(
                at.saturating_duration_since(Instant::now()) <= span,
                "case {case:?}: expected the owed transition due within {span:?}, got {:?}",
                at.saturating_duration_since(Instant::now())
            );
            let terminal = if row.op == "StopServer" { offline } else { online };
            assert_eq!(
                next, terminal,
                "case {case:?}: expected {terminal} to be owed, got {next:?}"
            );
        }
    }
}

/// One clock-owed window: the call that opens it, the state the server is in while the
/// transition is in flight, the state the clock owes it, the call that has no such
/// transition to make meanwhile, and the span the window lasts.
struct Window {
    case: &'static str,
    /// The state the server is put in before the window is opened; `None` leaves it as
    /// `CreateServer` left it, because the create is what opens that window.
    from: Option<&'static str>,
    /// The call that opens the window, or `None` when `CreateServer` opened it.
    begin: Option<&'static str>,
    in_flight: &'static str,
    terminal: &'static str,
    opposite: &'static str,
    span: Duration,
}

/// Open one window and assert it outlasts a caller that keeps calling, then that the
/// clock alone closes it.
async fn window_holds(emulator: &Emulator, window: &Window) {
    let case = window.case;
    let server_id = emulator.create_server(json!({})).await;
    if let Some(from) = window.from {
        pin_state(emulator, &server_id, from);
    }

    let opened = Instant::now();
    if let Some(begin) = window.begin {
        let answer = emulator.call(begin, json!({"ServerId": &server_id})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {begin} 200, got {} with body {}",
            answer.status, answer.body
        );
    }

    // A caller that acts straight after the call that opened the window, and keeps
    // calling, still finds the transition in flight.
    for call in 0..CALLS_WHILE_IN_FLIGHT {
        let (described, listed) = reported_states(emulator, case, &server_id).await;
        assert_eq!(
            (described.as_str(), listed.as_str()),
            (window.in_flight, window.in_flight),
            "case {case:?}: expected {} on call {call} of the window ({:?} of the {:?} span \
             elapsed), got described {described:?} and listed {listed:?}",
            window.in_flight,
            opened.elapsed(),
            window.span
        );
    }

    // And the refusal that depends on the window is still the answer: the opposite call
    // has no transition to make while this one is in flight.
    let opposite = emulator
        .call(window.opposite, json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        (opposite.status, opposite.error_code().as_str()),
        (400, "InvalidRequestException"),
        "case {case:?}: expected {} inside the window to be refused 400 \
         InvalidRequestException, got {} with body {}",
        window.opposite,
        opposite.status,
        opposite.body
    );

    let elapsed = opened.elapsed();
    assert!(
        elapsed < window.span,
        "case {case:?}: expected the {:?} window to outlast the {CALLS_WHILE_IN_FLIGHT} calls \
         a caller makes straight after the call that opened it, but {elapsed:?} had already \
         elapsed",
        window.span
    );

    // The terminal state comes due on the CLOCK: the reads above made none of it
    // happen, and this one makes nothing happen either.
    tokio::time::sleep(window.span + SETTLE_MARGIN).await;
    let (described, listed) = reported_states(emulator, case, &server_id).await;
    assert_eq!(
        (described.as_str(), listed.as_str()),
        (window.terminal, window.terminal),
        "case {case:?}: expected {} once the {:?} span came due, got described {described:?} \
         and listed {listed:?}",
        window.terminal,
        window.span
    );
}

/// RULE: a transition the clock owes a server is in flight for the span the service
/// gives it, measured from the call that began it — long enough that a caller acting
/// straight after that call still observes the intermediate state and the refusal that
/// depends on it, and the terminal state appears only once the span has come due, with
/// no call making it so.
///
/// The rows are every window this service opens: the one `CreateServer` begins, the one
/// `StopServer` begins and the one `StartServer` begins.
#[tokio::test]
async fn every_clock_owed_window_outlasts_a_caller_that_keeps_calling() {
    let emulator = Emulator::new();

    let created = Window {
        case: "the window CreateServer opens",
        from: None,
        begin: None,
        in_flight: st::SERVER_STATE_STARTING,
        terminal: st::SERVER_STATE_ONLINE,
        opposite: "StopServer",
        span: st::SERVER_START_SPAN,
    };
    let stopping = Window {
        case: "the window StopServer opens",
        from: Some(st::SERVER_STATE_ONLINE),
        begin: Some("StopServer"),
        in_flight: st::SERVER_STATE_STOPPING,
        terminal: st::SERVER_STATE_OFFLINE,
        opposite: "StartServer",
        span: st::SERVER_STOP_SPAN,
    };
    let starting = Window {
        case: "the window StartServer opens",
        from: Some(st::SERVER_STATE_OFFLINE),
        begin: Some("StartServer"),
        in_flight: st::SERVER_STATE_STARTING,
        terminal: st::SERVER_STATE_ONLINE,
        opposite: "StopServer",
        span: st::SERVER_START_SPAN,
    };

    // The rows run CONCURRENTLY on their own servers, so the spans they wait out overlap
    // instead of adding up.
    tokio::join!(
        window_holds(&emulator, &created),
        window_holds(&emulator, &stopping),
        window_holds(&emulator, &starting),
    );
}
