// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER in which this service applies the changes the clock
//! owes a stored record, over the team and change shapes the handshakes take.
//!
//! The rule under test: when two clock-owed changes to the same record have both come
//! due, they are applied in the order they fell DUE, so what a call reports never depends
//! on how long the caller waited before making it. For a session that was put to a team's
//! approvers that means the answer the clock owed before the session's expiration is what
//! decides the session — the expiration only ends a session nothing answered before it —
//! so a change the approvers were owed to approve is not discarded by a read taken after
//! the session's deadline had also passed.
//!
//! `clock_owed_order.rs` is the suite that spans the two instants themselves (the gap
//! between them, what the earlier-due answer owes next, and the two coinciding); this one
//! holds the order fixed per row and spans the RESOURCE the rule is applied to instead.
//! The rows differ along which of the two instants fell due first, how far apart they fell
//! (a second, and a whole [`state::SESSION_EXPIRATION`] span), which request the session
//! carries the answer to (a requested update, a requested deletion), how many approvers
//! the team has and how many approvals its strategy requires, and which members the change
//! names.
//!
//! Only the DUE INSTANTS are arranged, exactly as `approval_sessions::expire_session`
//! moves one; every status, timestamp and version the service then reports is its own, and
//! each row is read twice because a materialised outcome must not move between reads.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, expire_session, get_session};
use crate::approval_team_lifecycle::{instant, text, Harness};

/// Which of the two instants the clock owes a session fell due FIRST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FellDueFirst {
    /// The answer the approvers were owed for the request the session carries.
    TheAnswer,
    /// The session's own expiration.
    TheExpiry,
}

/// One row: which instant fell due first, how far back each of them sits, and the team
/// and change the row asks about.
struct OrderCase {
    case: &'static str,
    first: FellDueFirst,
    /// How long ago the EARLIER of the two instants fell due.
    earlier: Duration,
    /// How long ago the LATER of the two fell due; both are behind us, so only their
    /// order can decide the outcome.
    later: Duration,
    team_approvers: usize,
    min_approvals: i64,
}

/// Epoch seconds of a `SystemTime`, the unit `instant` reads a reported timestamp in.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

/// The `(answer, expiry)` instants a row arranges, in the order it puts them.
fn due_instants(case: &OrderCase) -> (SystemTime, SystemTime) {
    let now = SystemTime::now();
    let (answer_ago, expiry_ago) = match case.first {
        FellDueFirst::TheAnswer => (case.earlier, case.later),
        FellDueFirst::TheExpiry => (case.later, case.earlier),
    };
    (now - answer_ago, now - expiry_ago)
}

/// Move the instants a stored session was STARTED with, so a session whose expiration has
/// been placed in the past reads as one started before it rather than after it. Only
/// recorded instants move; this is `approval_sessions::expire_session`'s other end.
fn rewind_session_start(harness: &Harness, session_arn: &str, started_at: SystemTime) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let session = guard
        .session_mut(session_arn)
        .expect("a stored session to arrange");
    session.initiation_time = started_at;
    if session.activation_due.is_some() {
        session.activation_due = Some(started_at + state::SESSION_ACTIVATION_DELAY);
    }
}

/// Place the two instants the clock owes the UPDATE session a requested change was put
/// in: the instant the team's answer falls due, and the instant the session expires. The
/// request's own instants move with its answer, so the store reads as it would have if the
/// change had been requested that long ago.
fn arrange_update_clock(
    harness: &Harness,
    team_arn: &str,
    answer_at: SystemTime,
    expires_at: SystemTime,
) -> String {
    let started_at = answer_at - state::UPDATE_APPROVAL_DELAY;
    let session_arn = {
        let store = state::service_state(&*harness.engine).expect("the service store");
        let mut guard = store.lock();
        let team = guard.team_mut(team_arn).expect("a stored team to arrange");
        let update = team
            .pending_update
            .as_mut()
            .expect("a requested change to arrange");
        update.update_initiation_time = started_at;
        update.approval_due = Some(answer_at);
        update.update_session_arn.clone()
    };
    expire_session(harness, &session_arn, expires_at);
    rewind_session_start(harness, &session_arn, started_at);
    session_arn
}

/// The same for the DELETION session a requested deletion was put in. The pending window
/// the request named runs from the approval, so it moves with the answer and keeps the
/// length the request asked for.
fn arrange_deletion_clock(
    harness: &Harness,
    team_arn: &str,
    answer_at: SystemTime,
    expires_at: SystemTime,
) -> String {
    let started_at = answer_at - state::DELETION_APPROVAL_DELAY;
    let mut session_arn = String::new();
    harness.with_pending_deletion(team_arn, |deletion| {
        let window = deletion
            .completion_time
            .duration_since(deletion.approval_due.expect("an unanswered deletion"))
            .expect("a window that ends after the approval");
        deletion.start_time = started_at;
        deletion.approval_due = Some(answer_at);
        deletion.completion_time = answer_at + window;
        session_arn = deletion.session_arn.clone();
    });
    expire_session(harness, &session_arn, expires_at);
    rewind_session_start(harness, &session_arn, started_at);
    session_arn
}

/// `MinApprovalsRequired` as a response reports it for the live configuration.
fn min_approvals(body: &Value) -> Option<i64> {
    body["ApprovalStrategy"]["MofN"]["MinApprovalsRequired"].as_i64()
}

fn order_cases() -> Vec<OrderCase> {
    vec![
        OrderCase {
            case: "the answer fell due a second before the expiry",
            first: FellDueFirst::TheAnswer,
            earlier: Duration::from_secs(2),
            later: Duration::from_secs(1),
            team_approvers: 1,
            min_approvals: 1,
        },
        OrderCase {
            // The gap a caller whose next request comes a whole session lifetime after the
            // one that started the session leaves behind.
            case: "the answer fell due a whole expiration span before the expiry",
            first: FellDueFirst::TheAnswer,
            earlier: state::SESSION_EXPIRATION + Duration::from_secs(1),
            later: Duration::from_secs(1),
            team_approvers: 3,
            min_approvals: 2,
        },
        OrderCase {
            case: "the expiry fell due a second before the answer",
            first: FellDueFirst::TheExpiry,
            earlier: Duration::from_secs(2),
            later: Duration::from_secs(1),
            team_approvers: 2,
            min_approvals: 2,
        },
        OrderCase {
            case: "the expiry fell due a whole expiration span before the answer",
            first: FellDueFirst::TheExpiry,
            earlier: state::SESSION_EXPIRATION + Duration::from_secs(1),
            later: Duration::from_secs(1),
            team_approvers: 4,
            min_approvals: 1,
        },
    ]
}

/// A change an active team was asked to approve: the answer owed before the update
/// session's expiration carries the change through, and an expiration that fell due
/// before any answer was owed ends the change unapproved — whichever read materialises
/// both.
#[tokio::test]
async fn the_earlier_due_instant_decides_a_requested_update() {
    for (index, case) in order_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let arn = active_team(
            &harness,
            &format!("due-order-update-{index}"),
            case.team_approvers,
            case.min_approvals,
        )
        .await;

        let (status, before) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {before:?}");
        let live_version = text(&before, "VersionId");
        let live_description = text(&before, "Description");
        let live_min_approvals = min_approvals(&before);

        // Rows differ in the change they ask for too: a description, and a description
        // beside a strategy that lowers M.
        let requested_description = format!("requested by case {index}");
        let patch = if index.is_multiple_of(2) {
            json!({ "Description": requested_description })
        } else {
            json!({
                "Description": requested_description,
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
            })
        };
        let (status, updated) = harness.update(&arn, &patch).await;
        assert_eq!(status, 200, "case {label}: update returned {updated:?}");
        let requested_version = text(&updated, "VersionId");

        let (answer_at, expires_at) = due_instants(&case);
        let session_arn = arrange_update_clock(&harness, &arn, answer_at, expires_at);

        // Twice: a materialised outcome does not move between reads.
        for read in 1..=2 {
            let (status, team) = harness.get(&arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: get returned {status} with {team:?}"
            );
            let (status, session) = get_session(&harness, &session_arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: get session returned {status} with {session:?}"
            );

            match case.first {
                FellDueFirst::TheAnswer => {
                    assert_eq!(
                        text(&session, "Status"),
                        state::SessionStatus::Approved.as_wire(),
                        "case {label}, read {read}: the answer owed before the expiry \
                         decides the session, got {session:?}"
                    );
                    let completed = instant(&session, "CompletionTime");
                    assert!(
                        (completed - epoch_seconds(answer_at)).abs() < 1.0,
                        "case {label}, read {read}: the answer is stamped with the instant \
                         it was due ({}), got {completed}",
                        epoch_seconds(answer_at)
                    );
                    assert_eq!(
                        text(&team, "VersionId"),
                        requested_version,
                        "case {label}, read {read}: the approved version must be the live \
                         one, got {team:?}"
                    );
                    assert_eq!(
                        text(&team, "Description"),
                        requested_description,
                        "case {label}, read {read}: the approved configuration must be the \
                         live one, got {team:?}"
                    );
                    assert_eq!(
                        text(&team, "Status"),
                        state::TeamStatus::Active.as_wire(),
                        "case {label}, read {read}: the team is ACTIVE on its new version, \
                         got {team:?}"
                    );
                    assert!(
                        team.get("StatusCode").is_none_or(Value::is_null),
                        "case {label}, read {read}: an approved and activated change \
                         reports no outcome, got {team:?}"
                    );
                    assert!(
                        team.get("PendingUpdate").is_none()
                            && team.get("UpdateSessionArn").is_none(),
                        "case {label}, read {read}: nothing is pending once the change is \
                         live, got {team:?}"
                    );
                }
                FellDueFirst::TheExpiry => {
                    assert_eq!(
                        text(&session, "Status"),
                        state::SessionStatus::Failed.as_wire(),
                        "case {label}, read {read}: a session that expired before any \
                         answer was owed FAILED, got {session:?}"
                    );
                    assert_eq!(
                        text(&session, "StatusCode"),
                        state::SessionStatusCode::Expired.as_wire(),
                        "case {label}, read {read}: wrong StatusCode for an expired \
                         session, got {session:?}"
                    );
                    let completed = instant(&session, "CompletionTime");
                    assert!(
                        (completed - epoch_seconds(expires_at)).abs() < 1.0,
                        "case {label}, read {read}: the expiry is stamped with the instant \
                         it was due ({}), got {completed}",
                        epoch_seconds(expires_at)
                    );
                    assert_eq!(
                        text(&team, "VersionId"),
                        live_version,
                        "case {label}, read {read}: the live version must not move for a \
                         change the team never approved, got {team:?}"
                    );
                    assert_eq!(
                        text(&team, "Description"),
                        live_description,
                        "case {label}, read {read}: the live configuration must not move, \
                         got {team:?}"
                    );
                    assert_eq!(
                        min_approvals(&team),
                        live_min_approvals,
                        "case {label}, read {read}: the live strategy must not move, got \
                         {team:?}"
                    );
                    assert_eq!(
                        text(&team, "Status"),
                        state::TeamStatus::Active.as_wire(),
                        "case {label}, read {read}: a team whose change was not approved \
                         is active again, got {team:?}"
                    );
                    assert_eq!(
                        text(&team, "StatusCode"),
                        state::TeamStatusCode::UpdateFailedApproval.as_wire(),
                        "case {label}, read {read}: wrong StatusCode after a change the \
                         team never approved, got {team:?}"
                    );
                    assert_eq!(
                        text(&team["PendingUpdate"], "Status"),
                        state::TeamStatus::Inactive.as_wire(),
                        "case {label}, read {read}: the version the team never approved is \
                         inactive, got {team:?}"
                    );
                }
            }
        }
    }
}

/// The deletion of an active team is the same handshake, so the same order decides it: an
/// answer owed before the deletion session's expiration is the team's approval, and an
/// expiration that fell due first is a deletion the team never approved.
#[tokio::test]
async fn the_earlier_due_instant_decides_a_requested_deletion() {
    // The window a row names, long enough that the removal it starts is still ahead of
    // every row's arranged approval. `PendingWindowDays` is what names it.
    const WINDOW_DAYS: i64 = 30;

    for (index, case) in order_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let arn = active_team(
            &harness,
            &format!("due-order-deletion-{index}"),
            case.team_approvers,
            case.min_approvals,
        )
        .await;

        let (status, started) = harness
            .start_deletion(&arn, Some(json!({ "PendingWindowDays": WINDOW_DAYS })))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: start deletion returned {status} with {started:?}"
        );

        let (answer_at, expires_at) = due_instants(&case);
        let session_arn = arrange_deletion_clock(&harness, &arn, answer_at, expires_at);

        // Twice: a materialised outcome does not move between reads.
        for read in 1..=2 {
            let (status, team) = harness.get(&arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: a team inside its window must still read, got \
                 {status} with {team:?}"
            );
            let (status, session) = get_session(&harness, &session_arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: get session returned {status} with {session:?}"
            );

            match case.first {
                FellDueFirst::TheAnswer => {
                    assert_eq!(
                        text(&session, "Status"),
                        state::SessionStatus::Approved.as_wire(),
                        "case {label}, read {read}: the answer owed before the expiry \
                         decides the session, got {session:?}"
                    );
                    let completed = instant(&session, "CompletionTime");
                    assert!(
                        (completed - epoch_seconds(answer_at)).abs() < 1.0,
                        "case {label}, read {read}: the approval is stamped with the \
                         instant it was due ({}), got {completed}",
                        epoch_seconds(answer_at)
                    );
                    assert_eq!(
                        text(&team, "Status"),
                        state::TeamStatus::Deleting.as_wire(),
                        "case {label}, read {read}: an approved deletion inside its window \
                         leaves the team DELETING, got {team:?}"
                    );
                    assert!(
                        team.get("StatusCode").is_none_or(Value::is_null),
                        "case {label}, read {read}: an approved deletion is pending no \
                         approval, got {team:?}"
                    );
                }
                FellDueFirst::TheExpiry => {
                    assert_eq!(
                        text(&session, "Status"),
                        state::SessionStatus::Failed.as_wire(),
                        "case {label}, read {read}: a session that expired before any \
                         answer was owed FAILED, got {session:?}"
                    );
                    assert_eq!(
                        text(&session, "StatusCode"),
                        state::SessionStatusCode::Expired.as_wire(),
                        "case {label}, read {read}: wrong StatusCode for an expired \
                         session, got {session:?}"
                    );
                    assert_eq!(
                        text(&team, "Status"),
                        state::TeamStatus::Active.as_wire(),
                        "case {label}, read {read}: a team whose deletion was not approved \
                         is active again, got {team:?}"
                    );
                    assert_eq!(
                        text(&team, "StatusCode"),
                        state::TeamStatusCode::DeleteFailedApproval.as_wire(),
                        "case {label}, read {read}: wrong StatusCode after a deletion the \
                         team never approved, got {team:?}"
                    );
                }
            }
        }
    }
}
