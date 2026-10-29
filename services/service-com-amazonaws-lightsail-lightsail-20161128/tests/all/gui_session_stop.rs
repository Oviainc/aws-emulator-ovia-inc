// SPDX-License-Identifier: CC0-1.0

//! `StopGUISession` — terminating the Amazon DCV graphical user interface session
//! of a virtual computer, driven over the wire through the same front door a
//! client uses.
//!
//! The rule under test: a terminate resolves `resourceName` against the ONE
//! stored instance record and closes the session that computer holds — the
//! session is `stopping` until the clock owes it `stopped`, its URLs stop being
//! reported from the instant the close was asked for, and nothing resumes it, so
//! only a later start brings a session back on that computer, under a new token.
//! The close is the work the call started, so the one `StopGUISession` entry it
//! appends to the computer's operation log is non-terminal until that instant and
//! settled by the clock. A terminate is idempotent: a second one neither restarts
//! the close already under way nor pushes its completion out, and a computer that
//! holds no session at all is served with nothing written, because the end state
//! the request asks for already holds. A name the store holds no computer under
//! is refused with the service's `DoesNotExist` envelope.
//!
//! Each wire case is a TABLE: the computers of `gui_session_start.rs` — which
//! differ in blueprint platform, bundle size, Region and IP address type, none of
//! which the answer depends on — crossed with the phase the session is in when it
//! is terminated, which is what the rule generalises over. The clock case is a
//! table over the instants of a close, each bound of the span and one instant
//! either side of it, for a session terminated at each phase of its own life.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has, and the
//! session helpers are the ones `gui_session_start.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::STATUS_VALUES;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    GuiSessionStop, StoredGuiSession, GUI_SESSION_ACTIVE_MILLIS, GUI_SESSION_STATUS_NOT_STARTED,
    GUI_SESSION_STATUS_STARTED, GUI_SESSION_STATUS_STOPPED, GUI_SESSION_STATUS_STOPPING,
    STATE_TRANSITION_MILLIS,
};

use crate::gui_session_start::{
    access_details, create_body, logged_types_for, reported_token, start_session, stored_session,
    wait_until_operation_terminal, wait_until_session_started, Row, HOST, ROWS,
};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// The operation under test.
const OPERATION: &str = "StopGUISession";

/// The `operationType` every log entry this suite is about carries, the modelled
/// `OperationType` enum's own `StopGUISession` member.
const OPERATION_TYPE: &str = "StopGUISession";

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist, and the wire error code that carries it.
const DOES_NOT_EXIST: &str = "DoesNotExist";
/// See [`DOES_NOT_EXIST`].
const NOT_FOUND: &str = "NotFoundException";

/// Names nothing is created under, differing along the length, the case, the
/// digits and the separators. Every one satisfies the modelled `ResourceName`
/// pattern `^\w[\w\-]*\w$`, so the centralized walk passes it to the handler.
const MISSING_NAMES: &[&str] = &[
    "qz",
    "no_such_virtual_computer",
    "Terminate-Probe-42",
    "another_name_with_many_underscore_separated_words_and_digits_98765",
];

/// Whether the session is still coming up or already up when it is terminated:
/// the phase the rule generalises over, because the close is about the session
/// however far it had got.
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Terminated while the session is still coming up.
    ComingUp,
    /// Terminated once the session is up.
    Up,
}

impl Phase {
    /// Every phase a wire case runs.
    const ALL: &'static [Phase] = &[Phase::ComingUp, Phase::Up];

    /// The suffix a phase's computers are named with, which is also what names
    /// the phase in an assertion message.
    fn label(self) -> &'static str {
        match self {
            Phase::ComingUp => "coming-up",
            Phase::Up => "up",
        }
    }
}

/// Terminate the GUI session of `name` in `region`, requiring success.
async fn stop_session(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            Some(HOST),
            Some(region),
            OPERATION,
            json!({"resourceName": name}),
        )
        .await
}

/// Create the computer `name` of `row`, wait until it is serving, and start its
/// GUI session, leaving it in `phase`.
async fn computer_with_session(emulator: &Emulator, row: &Row, name: &str, phase: Phase) {
    emulator
        .ok_at(
            Some(HOST),
            Some(row.region),
            "CreateInstances",
            create_body(row, name),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
    start_session(emulator, row.region, name).await;
    if matches!(phase, Phase::Up) {
        wait_until_session_started(emulator, row.region, name).await;
    }
}

/// Assert `operations` is exactly one `StopGUISession` log entry against `name`
/// in `region`, terminal exactly when `terminal` says so, and answer its id.
fn assert_stop_operation(
    case: &str,
    operations: &Value,
    name: &str,
    region: &str,
    terminal: bool,
) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: the call acts on one virtual computer, so it logs one operation, \
         got {operations:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected operationType {OPERATION_TYPE}, got {entry:?}"
    );
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(name),
        "case {case}: expected resourceName {name}, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some("Instance"),
        "case {case}: expected resourceType Instance, got {entry:?}"
    );
    assert_eq!(
        entry["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: the entry is located where the computer is, got {entry:?}"
    );
    let want = if terminal {
        (Some("Succeeded"), Some(true))
    } else {
        (Some("Started"), Some(false))
    };
    assert_eq!(
        (entry["status"].as_str(), entry["isTerminal"].as_bool()),
        want,
        "case {case}: expected a {} entry, got {entry:?}",
        if terminal {
            "terminal Succeeded"
        } else {
            "non-terminal Started"
        }
    );
    let id = entry["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a minted operation id, got {entry:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id.to_string()
}

/// Assert the access details of `name` report a session that is closing or
/// closed, and never a URL: a terminated session's addresses reach nothing.
async fn assert_closing_or_closed(case: &str, emulator: &Emulator, region: &str, name: &str) {
    let body = access_details(emulator, region, name).await;
    let status = body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a status, got {body:?}"));
    assert!(
        STATUS_VALUES.contains(&status),
        "case {case}: expected a modelled Status member, got {status:?}"
    );
    assert!(
        status == GUI_SESSION_STATUS_STOPPING || status == GUI_SESSION_STATUS_STOPPED,
        "case {case}: a terminated session is closing or closed, got {status:?}"
    );
    assert!(
        body["sessions"].is_null(),
        "case {case}: a terminated session's URLs reach nothing, so none is reported, \
         got {body:?}"
    );
}

/// Poll the access details of `name` until they report `stopped`, answering the
/// body they reported it in.
async fn wait_until_session_stopped(emulator: &Emulator, region: &str, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = access_details(emulator, region, name).await;
        if body["status"].as_str() == Some(GUI_SESSION_STATUS_STOPPED) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the GUI session of {name} to close: the clock never settled it, \
             last {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The close the store holds for the session of the computer `name`, read through
/// the same public state API a handler uses — so a case can assert the close was
/// not restarted, which no reported member carries.
fn stored_stop(emulator: &Emulator, name: &str) -> Option<GuiSessionStop> {
    stored_session(emulator, name).and_then(|session| session.stop)
}

// ---------------------------------------------------------------------------
// Terminating a session
// ---------------------------------------------------------------------------

/// RULE: a terminate closes the session the named computer holds, however far
/// that session had got — it is `stopping` for the transition span and `stopped`
/// after it, its URLs are no longer reported, and the one `StopGUISession` entry
/// the call logs against the computer waits on the clock exactly as the close
/// does.
#[tokio::test]
async fn terminating_a_session_closes_it_and_logs_the_termination() {
    let emulator = Emulator::new();

    for row in ROWS {
        for phase in Phase::ALL {
            let case = format!("{} ({})", row.case, phase.label());
            let name = format!("{}-stop-{}", row.name, phase.label());
            computer_with_session(&emulator, row, &name, *phase).await;
            let before = stored_session(&emulator, &name).unwrap_or_else(|| {
                panic!("case {case}: the store must hold the session the start created")
            });
            assert!(
                before.stop.is_none(),
                "case {case}: nothing has terminated the session yet, got {before:?}"
            );

            let answered = stop_session(&emulator, row.region, &name).await;
            let id = assert_stop_operation(&case, &answered["operations"], &name, row.region, false);

            // The close the store recorded is the transition span the service
            // takes, measured from the call that asked for it.
            let stop = stored_stop(&emulator, &name)
                .unwrap_or_else(|| panic!("case {case}: the store must hold the close"));
            assert_eq!(
                stop.at_ms,
                stop.requested_at_ms + STATE_TRANSITION_MILLIS,
                "case {case}: the close is due one transition span after it was asked for, \
                 got {stop:?}"
            );
            // The session is the same one the start created — a close does not
            // mint anything — and it is no longer reported as reachable.
            assert_eq!(
                stored_session(&emulator, &name).map(|session| session.token),
                Some(before.token.clone()),
                "case {case}: the close acts on the session the computer held"
            );
            assert_closing_or_closed(&case, &emulator, row.region, &name).await;

            // The clock, not the poll, is what finishes the close and settles the
            // entry that recorded it.
            wait_until_session_stopped(&emulator, row.region, &name).await;
            assert_closing_or_closed(&case, &emulator, row.region, &name).await;
            let settled = wait_until_operation_terminal(&emulator, &id).await;
            assert_eq!(
                (
                    settled["operationType"].as_str(),
                    settled["resourceName"].as_str(),
                    settled["status"].as_str(),
                ),
                (Some(OPERATION_TYPE), Some(name.as_str()), Some("Succeeded")),
                "case {case}: GetOperation must report the settled entry the terminate logged, \
                 got {settled:?}"
            );
        }
    }
}

/// RULE: a terminate is idempotent — a second one on a session that is already
/// closing leaves the close exactly where it is, neither restarting it nor
/// pushing its completion out, and one on a session that is already closed has
/// nothing left to do, so the entry recording it is terminal as the call returns.
/// Each accepted call still appends its own entry: the call was served each time.
#[tokio::test]
async fn a_second_terminate_keeps_the_close_already_under_way() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-stop-again", row.name);
        computer_with_session(&emulator, row, &name, Phase::Up).await;

        stop_session(&emulator, row.region, &name).await;
        let first = stored_stop(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must hold the close"));

        let answered = stop_session(&emulator, row.region, &name).await;
        assert_stop_operation(case, &answered["operations"], &name, row.region, false);
        let again = stored_stop(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must still hold the close"));
        assert_eq!(
            (again.requested_at_ms, again.at_ms),
            (first.requested_at_ms, first.at_ms),
            "case {case}: the close already under way is not restarted by a second terminate, \
             got {again:?} after {first:?}"
        );

        // Once the close is through there is nothing left to do, so the entry a
        // further terminate logs is complete as the call returns.
        wait_until_session_stopped(&emulator, row.region, &name).await;
        let answered = stop_session(&emulator, row.region, &name).await;
        assert_stop_operation(case, &answered["operations"], &name, row.region, true);
        let after = stored_stop(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must still hold the close"));
        assert_eq!(
            (after.requested_at_ms, after.at_ms),
            (first.requested_at_ms, first.at_ms),
            "case {case}: a terminate of a closed session changes nothing, got {after:?}"
        );

        let logged = logged_types_for(&emulator, row.region, &name).await;
        assert_eq!(
            logged
                .iter()
                .filter(|logged| logged.as_str() == OPERATION_TYPE)
                .count(),
            3,
            "case {case}: each accepted terminate appends one entry, got {logged:?}"
        );
    }
}

/// RULE: a computer that holds no session has nothing to close, so the end state
/// the request asks for already holds: the call is served with a terminal entry
/// and writes nothing — no session is invented on a computer that never ran one.
/// The rows span a computer whose session was never started and one that is not
/// running at all, whose session the clock released when it stopped serving.
#[tokio::test]
async fn terminating_a_computer_that_holds_no_session_writes_nothing() {
    let emulator = Emulator::new();

    for row in ROWS {
        for variant in ["never started a session", "stopped, so its session is gone"] {
            let case = format!("{} ({variant})", row.case);
            let suffix = if variant.starts_with("never") {
                "no-session"
            } else {
                "computer-stopped"
            };
            let name = format!("{}-{suffix}", row.name);
            emulator
                .ok_at(
                    Some(HOST),
                    Some(row.region),
                    "CreateInstances",
                    create_body(row, &name),
                )
                .await;
            wait_for_state(&emulator, &name, "running").await;
            if suffix == "computer-stopped" {
                // A session the clock releases when the computer stops serving:
                // started first, so what is gone is a session that existed.
                start_session(&emulator, row.region, &name).await;
                wait_until_session_started(&emulator, row.region, &name).await;
                emulator
                    .ok_at(
                        Some(HOST),
                        Some(row.region),
                        "StopInstance",
                        json!({"instanceName": name}),
                    )
                    .await;
                wait_for_state(&emulator, &name, "stopped").await;
            }
            assert!(
                stored_session(&emulator, &name).is_none(),
                "case {case}: the computer must hold no session, got {:?}",
                stored_session(&emulator, &name)
            );

            let answered = stop_session(&emulator, row.region, &name).await;
            assert_stop_operation(&case, &answered["operations"], &name, row.region, true);
            assert!(
                stored_session(&emulator, &name).is_none(),
                "case {case}: a terminate invents no session, got {:?}",
                stored_session(&emulator, &name)
            );
            let logged = logged_types_for(&emulator, row.region, &name).await;
            assert_eq!(
                logged
                    .iter()
                    .filter(|logged| logged.as_str() == OPERATION_TYPE)
                    .count(),
                1,
                "case {case}: the served terminate appends exactly one entry, got {logged:?}"
            );
        }
    }
}

/// RULE: a terminated session is not resumed by reading its access details —
/// bringing a session back on the computer is `StartGUISession`'s business — and
/// the session that start creates is a NEW one: a token the store minted for it
/// and a fresh active window, because the session the caller closed is what the
/// old address belonged to.
#[tokio::test]
async fn a_start_after_a_terminate_resumes_the_session_under_a_new_token() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = format!("{}-stop-resume", row.name);
        computer_with_session(&emulator, row, &name, Phase::Up).await;
        let started = access_details(&emulator, row.region, &name).await;
        let first = reported_token(case, &started)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {started:?}"));

        stop_session(&emulator, row.region, &name).await;
        wait_until_session_stopped(&emulator, row.region, &name).await;
        // Reading the access details of a closed session reports the close, it
        // does not silently bring a session back.
        assert_closing_or_closed(case, &emulator, row.region, &name).await;

        start_session(&emulator, row.region, &name).await;
        let resumed = wait_until_session_started(&emulator, row.region, &name).await;
        let second = reported_token(case, &resumed)
            .unwrap_or_else(|| panic!("case {case}: expected a session token, got {resumed:?}"));
        assert_ne!(
            second, first,
            "case {case}: the session the caller closed is what the old address belonged to, so \
             the resumed session is addressed by a new token, got {resumed:?}"
        );
        let after = stored_session(&emulator, &name)
            .unwrap_or_else(|| panic!("case {case}: the store must hold the resumed session"));
        assert!(
            after.stop.is_none()
                && after.expires_at_ms() == after.starts_at_ms + GUI_SESSION_ACTIVE_MILLIS,
            "case {case}: the resumed session carries no close and a fresh window of the \
             documented length, got {after:?}"
        );
        assert_eq!(
            access_details(&emulator, row.region, &name).await["status"].as_str(),
            Some(GUI_SESSION_STATUS_STARTED),
            "case {case}: the resumed session is the one reported after the close"
        );
    }
}

/// RULE: a name no virtual computer is stored under is refused with Lightsail's
/// `DoesNotExist` envelope — HTTP 400, the wire error `NotFoundException`, the
/// body code `DoesNotExist` and a message naming the resource type and the
/// identifier the request carried.
#[tokio::test]
async fn an_unresolvable_name_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();

    for name in MISSING_NAMES {
        let case = format!("{OPERATION} on {name:?}");
        let (status, body) = emulator
            .call(OPERATION, json!({"resourceName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case}: expected the wire error {NOT_FOUND}, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Instance does not exist: {name}").as_str()),
            "case {case}: expected a message naming the resource type and the name, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// One phase a session can be terminated in, as an offset from the instant the
/// session's own start was asked for.
#[derive(Debug)]
struct StopCase {
    case: &'static str,
    stopped_at_offset_ms: i64,
}

/// RULE: from the instant a close is asked for, the close is what the session's
/// status and progress are about — `stopping` until the transition span is
/// through and `stopped` after it, whatever phase of its own life the session was
/// in — and a session a caller closed is never live again, so a start resumes the
/// computer's session rather than handing the closed one back. The rows terminate
/// a session while it is coming up, at each bound of its active window and past
/// that window, and each row walks every bound of the close's own span.
#[test]
fn a_terminated_sessions_status_and_progress_follow_the_clock() {
    // An arbitrary instant to start a session at: the rule is about spans, not
    // about dates.
    const STARTED_AT_MS: i64 = 1_700_000_000_000;

    let cases = [
        StopCase {
            case: "terminated while the session is still coming up",
            stopped_at_offset_ms: 0,
        },
        StopCase {
            case: "terminated the instant before the session is up",
            stopped_at_offset_ms: STATE_TRANSITION_MILLIS - 1,
        },
        StopCase {
            case: "terminated the instant the session is up",
            stopped_at_offset_ms: STATE_TRANSITION_MILLIS,
        },
        StopCase {
            case: "terminated halfway through the documented active hour",
            stopped_at_offset_ms: STATE_TRANSITION_MILLIS + GUI_SESSION_ACTIVE_MILLIS / 2,
        },
        StopCase {
            case: "terminated the last instant of the documented active hour",
            stopped_at_offset_ms: STATE_TRANSITION_MILLIS + GUI_SESSION_ACTIVE_MILLIS - 1,
        },
        StopCase {
            case: "terminated once the documented active hour has run out",
            stopped_at_offset_ms: STATE_TRANSITION_MILLIS + GUI_SESSION_ACTIVE_MILLIS,
        },
    ];

    for case in &cases {
        let label = case.case;
        let mut session = StoredGuiSession::starting(
            "fedcba98-7654-3210-fedc-ba9876543210".to_string(),
            STARTED_AT_MS,
        );
        let stopped_at_ms = STARTED_AT_MS.saturating_add(case.stopped_at_offset_ms);
        let stop = GuiSessionStop::after(stopped_at_ms);
        assert_eq!(
            stop.at_ms,
            stopped_at_ms + STATE_TRANSITION_MILLIS,
            "case {label}: a close is due one transition span after it was asked for, got {stop:?}"
        );
        session.stop = Some(stop);

        // Each bound of the close's span, and one instant either side of it.
        for (at_ms, want_status, want_percentage) in [
            (stopped_at_ms, GUI_SESSION_STATUS_STOPPING, 0),
            (
                stopped_at_ms + STATE_TRANSITION_MILLIS / 2,
                GUI_SESSION_STATUS_STOPPING,
                50,
            ),
            (stop.at_ms - 1, GUI_SESSION_STATUS_STOPPING, 99),
            (stop.at_ms, GUI_SESSION_STATUS_STOPPED, 100),
            (stop.at_ms + 1, GUI_SESSION_STATUS_STOPPED, 100),
            (
                stop.at_ms + GUI_SESSION_ACTIVE_MILLIS,
                GUI_SESSION_STATUS_STOPPED,
                100,
            ),
        ] {
            let got = session.status_at(at_ms);
            assert!(
                STATUS_VALUES.contains(&got),
                "case {label}: expected a modelled Status member at {at_ms}, got {got:?}"
            );
            assert_eq!(
                got, want_status,
                "case {label}: expected {want_status:?} at {at_ms}, got {got:?}"
            );
            assert_eq!(
                session.percentage_complete_at(at_ms),
                want_percentage,
                "case {label}: expected the close to be {want_percentage}% through at {at_ms}, \
                 got {session:?}"
            );
            assert!(
                !session.is_live_at(at_ms),
                "case {label}: a terminated session is never live again, got {session:?} \
                 at {at_ms}"
            );
        }
    }
}

/// RULE: `notStarted` is what a computer holding no session reports, so a
/// terminate of such a computer reports the state the computer is already in
/// rather than inventing a session — the value the handler answers for it is a
/// modelled `Status` member.
#[test]
fn a_computer_holding_no_session_has_a_modelled_status_to_report() {
    assert!(
        STATUS_VALUES.contains(&GUI_SESSION_STATUS_NOT_STARTED),
        "case the status of a computer with no session: expected a modelled Status member, \
         got {GUI_SESSION_STATUS_NOT_STARTED:?}"
    );
    for status in [GUI_SESSION_STATUS_STOPPING, GUI_SESSION_STATUS_STOPPED] {
        assert!(
            STATUS_VALUES.contains(&status),
            "case the status of a terminated session: expected a modelled Status member, \
             got {status:?}"
        );
    }
}
