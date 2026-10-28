// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER in which this service materialises what the clock owes.
//!
//! The rule under test: a clock-owed change belongs to the instant it fell DUE, so where
//! two of them bear on the same record the earlier-due one is applied first and the later
//! one acts on its result — what a caller reads is a function of the current instant
//! alone, never of when the next request happened to arrive. The two changes here are the
//! approvers' answer the clock owes in a session (the approval of a requested team change,
//! the approval of a requested deletion) and that session's own expiration: an answer owed
//! while the session was still open is recorded however long after both instants the next
//! call comes, and only a session that ran out before any answer was owed in it ends
//! unanswered.
//!
//! The tables span the class the rule generalises over: which handshake the session
//! carries (a requested deletion, a requested change), how long the gap between the
//! request and the next call is (inside the session's life, just past its expiration, days
//! past it), what the earlier-due answer owes next (a pending window still running, one
//! that has run out, an activation), the shape of the change asked for, and the ORDER of
//! the two instants (the answer first, the two coinciding, the expiration first).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, get_session};
use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness};

/// Seconds in one day, the unit `PendingWindowDays` counts in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Tolerance when two instants this service computed from the same due instant are
/// compared, in seconds.
const INSTANT_SLACK: f64 = 0.05;

/// `POST /approval-teams/{ApprovalTeamArn}/sessions/?List` — `ListSessions`, filtered to
/// the sessions a given operation started.
async fn session_arns_for(harness: &Harness, team_arn: &str, action: &str, label: &str) -> Vec<String> {
    let (status, page) = harness
        .call(
            "POST",
            &format!("/approval-teams/{}/sessions/?List", encode_label(team_arn)),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label}: ListSessions returned {status} with {page:?}"
    );
    page.get("Sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|entry| text(entry, "ActionName") == action)
        .map(|entry| text(entry, "SessionArn"))
        .collect()
}

/// The one session a team's deletion was put to its approvers in.
async fn deletion_session_arn(harness: &Harness, team_arn: &str, label: &str) -> String {
    let arns = session_arns_for(
        harness,
        team_arn,
        state::SessionKind::Deletion.action_name(),
        label,
    )
    .await;
    assert_eq!(
        arns.len(),
        1,
        "case {label}: expected the one deletion session, got {arns:?}"
    );
    arns[0].clone()
}

/// How many of a session's approvers answered `APPROVED`.
fn approvals(session: &Value) -> usize {
    session["ApproverResponses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|response| text(response, "Response") == state::SessionResponse::Approved.as_wire())
        .count()
}

/// Rewind every instant the handshake a team is in was recorded with by `elapsed`, so the
/// store holds exactly what it would hold had the request been made `elapsed` ago with no
/// call since: the approvers' answer still falls due its own delay after the request, and
/// the session still runs out `SESSION_EXPIRATION` after it, both now behind us. Only the
/// recorded instants move; every transition stays the service's own, as
/// `approval_sessions::expire_session` moves one session's.
fn rewind_by(harness: &Harness, team_arn: &str, elapsed: Duration) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    for session in guard
        .sessions
        .values_mut()
        .filter(|session| session.approval_team_arn == team_arn)
    {
        session.initiation_time -= elapsed;
        session.expiration_time -= elapsed;
        session.activation_due = session.activation_due.map(|due| due - elapsed);
        session.completion_time = session.completion_time.map(|at| at - elapsed);
    }
    let team = guard.team_mut(team_arn).expect("a stored team to rewind");
    if let Some(deletion) = team.pending_deletion.as_mut() {
        deletion.start_time -= elapsed;
        deletion.approval_due = deletion.approval_due.map(|due| due - elapsed);
        deletion.approval_time = deletion.approval_time.map(|at| at - elapsed);
        deletion.completion_time -= elapsed;
    }
    if let Some(update) = team.pending_update.as_mut() {
        update.update_initiation_time -= elapsed;
        update.approval_due = update.approval_due.map(|due| due - elapsed);
        update.activation_due = update.activation_due.map(|due| due - elapsed);
    }
}

/// Put the two instants the rule keys on where a row needs them: the instant the session
/// ran out, and the instant the approvers' answer was owed in it. Only DUE INSTANTS are
/// moved; the outcome each one owes stays the service's own.
fn arrange_due_instants(
    harness: &Harness,
    team_arn: &str,
    session_arn: &str,
    ran_out: SystemTime,
    answer_due: SystemTime,
) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    guard
        .session_mut(session_arn)
        .expect("a stored session to arrange")
        .expiration_time = ran_out;
    let team = guard.team_mut(team_arn).expect("a stored team to arrange");
    if let Some(deletion) = team.pending_deletion.as_mut() {
        deletion.approval_due = Some(answer_due);
    }
    if let Some(update) = team.pending_update.as_mut() {
        update.approval_due = Some(answer_due);
    }
}

/// What the approved deletion owes by the time the next request arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The pending window the approval started is still running, so the team is still
    /// `DELETING`.
    InsideTheWindow,
    /// That window has run out too, so the removal it owes has happened.
    Removed,
}

/// One row of the deletion-ordering table.
struct DeletionCase {
    case: &'static str,
    /// `PendingWindowDays` as the request names it; `None` leaves the member absent.
    requested_days: Option<u64>,
    expected_days: u64,
    /// Gap between the request and the next call.
    elapsed: Duration,
    approvers: usize,
    min_approvals: i64,
    outcome: Outcome,
}

/// The rule on a requested DELETION: the approvers' answer fell due
/// [`state::DELETION_APPROVAL_DELAY`] after the request and the session only ran out
/// [`state::SESSION_EXPIRATION`] later, so the answer is recorded first whatever the gap
/// before the next call — and the `DeletionCompletionTime` the request was answered with
/// is the instant the team is actually removed at.
#[tokio::test]
async fn a_deletion_approval_owed_before_its_session_ran_out_is_recorded_first() {
    // Rows differ along the gap before the next call (inside the session's life, just past
    // its expiration, days past it), the window the request names (absent, the shortest a
    // day count names, a week, a month) and the team the deletion is asked of.
    let cases = vec![
        DeletionCase {
            case: "the next call a second after the request, the session still open",
            requested_days: Some(7),
            expected_days: 7,
            elapsed: Duration::from_secs(1),
            approvers: 2,
            min_approvals: 1,
            outcome: Outcome::InsideTheWindow,
        },
        DeletionCase {
            case: "the next call a second past the session's expiration",
            requested_days: Some(7),
            expected_days: 7,
            elapsed: state::SESSION_EXPIRATION + Duration::from_secs(1),
            approvers: 3,
            min_approvals: 2,
            outcome: Outcome::InsideTheWindow,
        },
        DeletionCase {
            case: "the window left absent, the next call a day past the expiration",
            requested_days: None,
            expected_days: u64::try_from(state::DEFAULT_PENDING_WINDOW_DAYS)
                .expect("a positive default window"),
            elapsed: state::SESSION_EXPIRATION + Duration::from_secs(SECONDS_PER_DAY),
            approvers: 1,
            min_approvals: 1,
            outcome: Outcome::InsideTheWindow,
        },
        DeletionCase {
            case: "a month-long window, the next call a week past the expiration",
            requested_days: Some(30),
            expected_days: 30,
            elapsed: state::SESSION_EXPIRATION + Duration::from_secs(7 * SECONDS_PER_DAY),
            approvers: 2,
            min_approvals: 2,
            outcome: Outcome::InsideTheWindow,
        },
        DeletionCase {
            case: "the shortest window, the next call past both the expiration and the window",
            requested_days: Some(1),
            expected_days: 1,
            elapsed: state::SESSION_EXPIRATION + Duration::from_secs(1),
            approvers: 2,
            min_approvals: 1,
            outcome: Outcome::Removed,
        },
        DeletionCase {
            case: "a week-long window, the next call a month after the request",
            requested_days: Some(7),
            expected_days: 7,
            elapsed: Duration::from_secs(30 * SECONDS_PER_DAY),
            approvers: 3,
            min_approvals: 3,
            outcome: Outcome::Removed,
        },
    ];

    for (index, case) in cases.into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let arn = active_team(
            &harness,
            &format!("owed-approval-{index}"),
            case.approvers,
            case.min_approvals,
        )
        .await;

        let body = case
            .requested_days
            .map(|days| json!({ "PendingWindowDays": days }));
        let (status, started) = harness.start_deletion(&arn, body).await;
        assert_eq!(
            status, 200,
            "case {label}: start deletion returned {status} with {started:?}"
        );
        let promised_start = instant(&started, "DeletionStartTime");
        let promised_completion = instant(&started, "DeletionCompletionTime");
        let session_arn = deletion_session_arn(&harness, &arn, label).await;

        rewind_by(&harness, &arn, case.elapsed);
        let elapsed_seconds = case.elapsed.as_secs_f64();

        match case.outcome {
            Outcome::InsideTheWindow => {
                // Twice, because a materialised value does not move between reads.
                for read in 1..=2 {
                    let (status, team) = harness.get(&arn).await;
                    assert_eq!(
                        status, 200,
                        "case {label}, read {read}: the team must still be readable inside \
                         its approved window, got {status} with {team:?}"
                    );
                    assert_eq!(
                        text(&team, "Status"),
                        state::TeamStatus::Deleting.as_wire(),
                        "case {label}, read {read}: the approved deletion must stand, got \
                         {team:?}"
                    );
                    assert!(
                        team.get("StatusCode").is_none_or(Value::is_null),
                        "case {label}, read {read}: an approved deletion is pending no \
                         approval, got {team:?}"
                    );
                }

                let (status, session) = get_session(&harness, &session_arn).await;
                assert_eq!(
                    status, 200,
                    "case {label}: get session returned {status} with {session:?}"
                );
                assert_eq!(
                    text(&session, "Status"),
                    state::SessionStatus::Approved.as_wire(),
                    "case {label}: the session must carry the answer owed before it ran \
                     out, got {session:?}"
                );
                assert!(
                    session.get("StatusCode").is_none_or(Value::is_null),
                    "case {label}: an answered session did not expire, got {session:?}"
                );
                assert_eq!(
                    approvals(&session),
                    usize::try_from(case.min_approvals).expect("a positive threshold"),
                    "case {label}: the session records the approvals its strategy requires, \
                     no more: {session:?}"
                );

                // The answer is stamped with the instant it was DUE, and the window the
                // caller was promised runs from exactly that instant.
                let approved_at = instant(&session, "CompletionTime");
                let expected_approval = promised_start
                    + state::DELETION_APPROVAL_DELAY.as_secs_f64()
                    - elapsed_seconds;
                assert!(
                    (approved_at - expected_approval).abs() < INSTANT_SLACK,
                    "case {label}: the answer must be stamped with the instant it was due: \
                     expected {expected_approval}, got {approved_at}"
                );
                let window_seconds = (case.expected_days * SECONDS_PER_DAY) as f64;
                assert!(
                    (promised_completion - elapsed_seconds - approved_at - window_seconds).abs()
                        < INSTANT_SLACK,
                    "case {label}: the promised completion instant must be the approval plus \
                     the window: expected {}, got {}",
                    approved_at + window_seconds,
                    promised_completion - elapsed_seconds
                );
            }
            Outcome::Removed => {
                let (status, gone) = harness.get(&arn).await;
                assert_eq!(
                    status, 404,
                    "case {label}: the approved deletion must have removed the team, got \
                     {status} with {gone:?}"
                );
                assert_eq!(
                    error_code(&gone),
                    "ResourceNotFoundException",
                    "case {label}: wrong error code for a removed team: {gone:?}"
                );
                let (status, orphan) = get_session(&harness, &session_arn).await;
                assert_eq!(
                    status, 404,
                    "case {label}: a session of a removed team must answer 404, got {status} \
                     with {orphan:?}"
                );
            }
        }
    }
}

/// One row of the update-ordering table.
struct UpdateCase {
    case: &'static str,
    /// Gap between the request and the next call.
    elapsed: Duration,
    /// The change `UpdateApprovalTeam` is asked for.
    patch: Value,
    /// `Description` the approved change makes live, where the row names one.
    description: Option<&'static str>,
    /// `MinApprovalsRequired` the approved change makes live, where the row names one.
    min_approvals: Option<i64>,
}

/// The same rule on a requested CHANGE: the approval fell due
/// [`state::UPDATE_APPROVAL_DELAY`] after the request and the update session only ran out
/// [`state::SESSION_EXPIRATION`] later, so the approval — and the activation it owes — are
/// applied whatever the gap before the next call, and the requested version becomes the
/// live one.
#[tokio::test]
async fn an_update_approval_owed_before_its_session_ran_out_is_recorded_first() {
    // Rows differ along the gap before the next call and the members the change names.
    let cases = vec![
        UpdateCase {
            case: "a description, the next call a second after the request",
            elapsed: Duration::from_secs(1),
            patch: json!({ "Description": "owed before the expiration" }),
            description: Some("owed before the expiration"),
            min_approvals: None,
        },
        UpdateCase {
            case: "a description, the next call a second past the session's expiration",
            elapsed: state::SESSION_EXPIRATION + Duration::from_secs(1),
            patch: json!({ "Description": "owed a day before the expiration" }),
            description: Some("owed a day before the expiration"),
            min_approvals: None,
        },
        UpdateCase {
            case: "a strategy, the next call a week past the session's expiration",
            elapsed: state::SESSION_EXPIRATION + Duration::from_secs(7 * SECONDS_PER_DAY),
            patch: json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 2 } } }),
            description: None,
            min_approvals: Some(2),
        },
        UpdateCase {
            case: "a description and a strategy, the next call a month after the request",
            elapsed: Duration::from_secs(30 * SECONDS_PER_DAY),
            patch: json!({
                "Description": "owed long ago",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 3 } },
            }),
            description: Some("owed long ago"),
            min_approvals: Some(3),
        },
    ];

    for (index, case) in cases.into_iter().enumerate() {
        let UpdateCase {
            case: label,
            elapsed,
            patch,
            description,
            min_approvals,
        } = case;
        let harness = Harness::new();
        let arn = active_team(&harness, &format!("owed-update-{index}"), 3, 1).await;

        let (status, updated) = harness.update(&arn, &patch).await;
        assert_eq!(
            status, 200,
            "case {label}: update returned {status} with {updated:?}"
        );
        let requested_version = text(&updated, "VersionId");
        let (status, waiting) = harness.get(&arn).await;
        assert_eq!(
            status, 200,
            "case {label}: get returned {status} with {waiting:?}"
        );
        let session_arn = text(&waiting, "UpdateSessionArn");
        assert!(
            !session_arn.is_empty(),
            "case {label}: the change must name the session that decides it, got {waiting:?}"
        );

        rewind_by(&harness, &arn, elapsed);

        // Twice, because a materialised value does not move between reads.
        for read in 1..=2 {
            let (status, live) = harness.get(&arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: get returned {status} with {live:?}"
            );
            assert_eq!(
                text(&live, "Status"),
                state::TeamStatus::Active.as_wire(),
                "case {label}, read {read}: the approved change must have been activated, \
                 got {live:?}"
            );
            assert_eq!(
                text(&live, "VersionId"),
                requested_version,
                "case {label}, read {read}: the approved version must be the live one, got \
                 {live:?}"
            );
            if let Some(expected) = description {
                assert_eq!(
                    text(&live, "Description"),
                    expected,
                    "case {label}, read {read}: the approved configuration must be the live \
                     one, got {live:?}"
                );
            }
            if let Some(expected) = min_approvals {
                assert_eq!(
                    live["ApprovalStrategy"]["MofN"]["MinApprovalsRequired"].as_i64(),
                    Some(expected),
                    "case {label}, read {read}: the approved strategy must be the live one, \
                     got {live:?}"
                );
            }
            assert!(
                live.get("PendingUpdate").is_none_or(Value::is_null),
                "case {label}, read {read}: nothing is pending once the change is live, got \
                 {live:?}"
            );
        }

        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: get session returned {status} with {session:?}"
        );
        assert_eq!(
            text(&session, "Status"),
            state::SessionStatus::Approved.as_wire(),
            "case {label}: the session must carry the answer owed before it ran out, got \
             {session:?}"
        );
    }
}

/// The other order, which the rule decides the same way: a session that ran out BEFORE any
/// answer was owed in it ends unanswered, and the request it carried is one the team never
/// approved — so a deletion removes nothing and a change never becomes live.
#[tokio::test]
async fn a_session_that_ran_out_before_the_answer_was_owed_ends_unanswered() {
    /// Which request the session carries.
    #[derive(Debug, Clone, Copy)]
    enum Handshake {
        Deletion,
        Update,
    }

    // Rows differ along the handshake and the gap between the instant the session ran out
    // and the instant the answer would have been owed — including the two coinciding,
    // where the session is already over at the instant its expiration falls due.
    let gaps: &[(&str, Duration)] = &[
        ("the answer owed at the very instant the session ran out", Duration::ZERO),
        ("the session ran out a second before the answer was owed", Duration::from_secs(1)),
        (
            "the session ran out a day before the answer was owed",
            Duration::from_secs(SECONDS_PER_DAY),
        ),
    ];

    for (index, handshake) in [Handshake::Deletion, Handshake::Update]
        .into_iter()
        .enumerate()
    {
        for (gap_index, &(gap_label, gap)) in gaps.iter().enumerate() {
            let label = format!("{handshake:?}, {gap_label}");
            let harness = Harness::new();
            let arn = active_team(
                &harness,
                &format!("ran-out-{index}-{gap_index}"),
                3,
                2,
            )
            .await;
            let (_, before) = harness.get(&arn).await;
            let live_version = text(&before, "VersionId");

            let session_arn = match handshake {
                Handshake::Deletion => {
                    let (status, started) = harness
                        .start_deletion(&arn, Some(json!({ "PendingWindowDays": 1 })))
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {label}: start deletion returned {status} with {started:?}"
                    );
                    deletion_session_arn(&harness, &arn, &label).await
                }
                Handshake::Update => {
                    let (status, updated) = harness
                        .update(&arn, &json!({ "Description": "never approved" }))
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {label}: update returned {status} with {updated:?}"
                    );
                    let (_, waiting) = harness.get(&arn).await;
                    text(&waiting, "UpdateSessionArn")
                }
            };

            // The answer was owed `gap` after the session had already run out, and both
            // instants are behind us.
            let answer_due = SystemTime::now() - Duration::from_secs(2);
            arrange_due_instants(&harness, &arn, &session_arn, answer_due - gap, answer_due);

            // Twice, because the refusal is recorded once and does not move under a read.
            for read in 1..=2 {
                let (status, team) = harness.get(&arn).await;
                assert_eq!(
                    status, 200,
                    "case {label}, read {read}: a request the team never approved leaves it \
                     readable, got {status} with {team:?}"
                );
                assert_eq!(
                    text(&team, "Status"),
                    state::TeamStatus::Active.as_wire(),
                    "case {label}, read {read}: the team must be active again, got {team:?}"
                );
                let expected_code = match handshake {
                    Handshake::Deletion => state::TeamStatusCode::DeleteFailedApproval,
                    Handshake::Update => state::TeamStatusCode::UpdateFailedApproval,
                };
                assert_eq!(
                    text(&team, "StatusCode"),
                    expected_code.as_wire(),
                    "case {label}, read {read}: wrong StatusCode for a request the team \
                     never approved, got {team:?}"
                );
                assert_eq!(
                    text(&team, "VersionId"),
                    live_version,
                    "case {label}, read {read}: the live version must not move, got {team:?}"
                );
            }

            let (status, session) = get_session(&harness, &session_arn).await;
            assert_eq!(
                status, 200,
                "case {label}: get session returned {status} with {session:?}"
            );
            assert_eq!(
                text(&session, "Status"),
                state::SessionStatus::Failed.as_wire(),
                "case {label}: a session that ran out unanswered FAILED, got {session:?}"
            );
            assert_eq!(
                text(&session, "StatusCode"),
                state::SessionStatusCode::Expired.as_wire(),
                "case {label}: wrong status code for a session that ran out, got {session:?}"
            );
            assert_eq!(
                approvals(&session),
                0,
                "case {label}: nobody answered a session that ran out, got {session:?}"
            );
        }
    }
}
