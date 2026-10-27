// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the deletion handshake of `StartActiveApprovalTeamDeletion`.
//!
//! The rule under test: deleting an ACTIVE approval team is a request its approvers have
//! to approve, so the call RECORDS the request and puts it to the team in an approval
//! session of its own instead of carrying it out — the team keeps everything it has and
//! reports `DELETING` with `DELETE_PENDING_APPROVAL` — and the pending window
//! `PendingWindowDays` names runs from the team's APPROVAL of that request, so the removal
//! falls due that many days after the approval and a deletion the team never approved
//! removes nothing at all.
//!
//! The tables span the class the rule generalises over: the window the request names
//! (absent, the shortest, a week, a month, a year, and the longest this service records),
//! how many approvers the team has (1, 2, 3 and the `Approvers` `length(1..20)` maximum),
//! how many approvals its strategy requires (M = 1, M below N, M = N) and the two ways an
//! approval session can end without an approval (cancelled, expired unanswered).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, expire_session, get_session, wait_for_the_clock};
use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness};

/// Seconds in one day, the unit `PendingWindowDays` counts in.
const SECONDS_PER_DAY: f64 = 24.0 * 60.0 * 60.0;

/// `PUT /sessions/{SessionArn}` — `CancelSession`.
async fn cancel_session(harness: &Harness, session_arn: &str) -> (u16, Value) {
    harness
        .call("PUT", &format!("/sessions/{}", encode_label(session_arn)), None)
        .await
}

/// `POST /approval-teams/{ApprovalTeamArn}/sessions/?List` — `ListSessions`.
async fn list_sessions(harness: &Harness, team_arn: &str) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/approval-teams/{}/sessions/?List", encode_label(team_arn)),
            Some(json!({})),
        )
        .await
}

/// The sessions `ListSessions` reports for a team, in the order it lists them.
async fn listed_sessions(harness: &Harness, team_arn: &str, label: &str) -> Vec<Value> {
    let (status, page) = list_sessions(harness, team_arn).await;
    assert_eq!(
        status, 200,
        "case {label}: ListSessions returned {status} with {page:?}"
    );
    page.get("Sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `SessionArn`s of a team's DELETION sessions, in listing order: the ones whose
/// `ActionName` is the operation that starts one.
async fn deletion_session_arns(harness: &Harness, team_arn: &str, label: &str) -> Vec<String> {
    listed_sessions(harness, team_arn, label)
        .await
        .iter()
        .filter(|entry| text(entry, "ActionName") == state::SessionKind::Deletion.action_name())
        .map(|entry| text(entry, "SessionArn"))
        .collect()
}

/// Pull the end of a recorded pending window into the past, leaving the approval exactly
/// where it stands, so the removal is owed only if the team has approved the deletion.
/// Only the recorded instant is moved; the removal itself is the service's own.
fn elapse_window(harness: &Harness, team_arn: &str) {
    harness.with_pending_deletion(team_arn, |deletion| {
        deletion.completion_time = SystemTime::now() - Duration::from_secs(1);
    });
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

/// One row of the deletion table: the team the deletion is asked of, the window the
/// request names, and the window the service must measure from the approval.
struct DeletionCase {
    case: &'static str,
    team_approvers: usize,
    min_approvals: i64,
    /// `PendingWindowDays` as the request names it; `None` leaves the member absent.
    requested_days: Option<i64>,
    expected_days: i64,
}

fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            case: "the window left absent, one approver, M = N",
            team_approvers: 1,
            min_approvals: 1,
            requested_days: None,
            expected_days: i64::from(state::DEFAULT_PENDING_WINDOW_DAYS),
        },
        DeletionCase {
            case: "the shortest window a day count names, two approvers, M below N",
            team_approvers: 2,
            min_approvals: 1,
            requested_days: Some(1),
            expected_days: 1,
        },
        DeletionCase {
            case: "a week named explicitly, three approvers, M = N",
            team_approvers: 3,
            min_approvals: 3,
            requested_days: Some(7),
            expected_days: 7,
        },
        DeletionCase {
            case: "a month, two approvers, M = N",
            team_approvers: 2,
            min_approvals: 2,
            requested_days: Some(30),
            expected_days: 30,
        },
        DeletionCase {
            case: "a year, one approver",
            team_approvers: 1,
            min_approvals: 1,
            requested_days: Some(365),
            expected_days: 365,
        },
        DeletionCase {
            // `Approvers` is `length(min: 1, max: 20)` on `CreateApprovalTeam`, so twenty
            // is the largest team a deletion can be put to; `MAX_PENDING_WINDOW_DAYS` is
            // the longest window this service records.
            case: "the longest window this service records, twenty approvers",
            team_approvers: 20,
            min_approvals: 1,
            requested_days: Some(i64::from(state::MAX_PENDING_WINDOW_DAYS)),
            expected_days: i64::from(state::MAX_PENDING_WINDOW_DAYS),
        },
    ]
}

/// The rule, over the whole table: the request is put to the team in a deletion session,
/// and the pending window is measured from the approval that session records.
#[tokio::test]
async fn an_active_team_is_deleted_only_after_the_team_approves_it() {
    for (index, case) in deletion_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let arn = active_team(
            &harness,
            &format!("deletion-approval-{index}"),
            case.team_approvers,
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
        let requested_at = instant(&started, "DeletionStartTime");
        let completion = instant(&started, "DeletionCompletionTime");

        // Nothing is deleted yet: the team keeps everything it has and reports that it is
        // waiting on its approvers.
        let (status, waiting) = harness.get(&arn).await;
        assert_eq!(
            status, 200,
            "case {label}: a team awaiting its approvers must still be readable, got \
             {status} with {waiting:?}"
        );
        assert_eq!(
            text(&waiting, "Status"),
            state::TeamStatus::Deleting.as_wire(),
            "case {label}: expected a DELETING team, got {waiting:?}"
        );
        assert_eq!(
            text(&waiting, "StatusCode"),
            state::TeamStatusCode::DeletePendingApproval.as_wire(),
            "case {label}: expected the deletion to be pending the team's approval, got \
             {waiting:?}"
        );

        // The request was put to the team in a session of its own, naming the operation
        // that started it and the approvers whose approval it needs.
        let sessions = listed_sessions(&harness, &arn, label).await;
        assert_eq!(
            sessions.len(),
            1,
            "case {label}: the deletion must be put to the team in exactly one session, \
             got {sessions:?}"
        );
        let entry = &sessions[0];
        assert_eq!(
            text(entry, "ActionName"),
            state::SessionKind::Deletion.action_name(),
            "case {label}: the session must name the operation that started it, got \
             {entry:?}"
        );
        assert_eq!(
            text(entry, "ApprovalTeamArn"),
            arn,
            "case {label}: the session must ask the team being deleted, got {entry:?}"
        );
        let session_arn = text(entry, "SessionArn");

        // The team's answer comes due, and it is that answer the window runs from.
        wait_for_the_clock(state::DELETION_APPROVAL_DELAY);
        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(
            status, 200,
            "case {label}: get session returned {status} with {session:?}"
        );
        assert_eq!(
            text(&session, "Status"),
            state::SessionStatus::Approved.as_wire(),
            "case {label}: the deletion session must carry the team's approval, got \
             {session:?}"
        );
        assert_eq!(
            session["NumberOfApprovers"].as_u64(),
            Some(case.team_approvers as u64),
            "case {label}: the session must ask the approvers the team has, got {session:?}"
        );
        assert_eq!(
            approvals(&session),
            usize::try_from(case.min_approvals).expect("a positive threshold"),
            "case {label}: the session records the approvals its strategy requires, no \
             more: {session:?}"
        );
        let approved_at = instant(&session, "CompletionTime");
        assert!(
            approved_at > requested_at,
            "case {label}: the approval must come after the request: approved at \
             {approved_at}, requested at {requested_at}"
        );
        let expected_seconds = case.expected_days as f64 * SECONDS_PER_DAY;
        assert!(
            (completion - approved_at - expected_seconds).abs() < 0.05,
            "case {label}: the pending window must run from the team's approval: expected \
             {expected_seconds}s between the approval and DeletionCompletionTime, got {}s",
            completion - approved_at
        );

        // With the approval recorded nothing is pending it, and `ApprovalTeamStatusCode`
        // carries no token for an approved deletion inside its window.
        let (status, approved) = harness.get(&arn).await;
        assert_eq!(
            status, 200,
            "case {label}: an approved deletion inside its window must still read, got \
             {status} with {approved:?}"
        );
        assert_eq!(
            text(&approved, "Status"),
            state::TeamStatus::Deleting.as_wire(),
            "case {label}: expected a DELETING team inside its window, got {approved:?}"
        );
        assert!(
            approved.get("StatusCode").is_none_or(Value::is_null),
            "case {label}: an approved deletion is pending no approval, got {approved:?}"
        );

        // The removal the approved window owes: the team is gone from both readers, and
        // its sessions cannot outlive it.
        elapse_window(&harness, &arn);
        let (status, gone) = harness.get(&arn).await;
        assert_eq!(
            status, 404,
            "case {label}: the team must be gone once the approved window elapsed, got \
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
            "case {label}: a session of a removed team must answer 404, got {status} with \
             {orphan:?}"
        );
    }
}

/// A deletion the team has not approved removes nothing, even once the window the request
/// named has run out: the window the removal is owed at runs from the APPROVAL, so until
/// there is one nothing is owed at any instant — and reading the team does not move it.
#[tokio::test]
async fn an_unapproved_deletion_removes_nothing_when_its_window_has_elapsed() {
    // Rows differ along the window the request named and the team it was asked of.
    let rows: &[(&str, i64, usize, i64)] = &[
        ("the shortest window, one approver", 1, 1, 1),
        (
            "the default window named explicitly, two approvers, M below N",
            i64::from(state::DEFAULT_PENDING_WINDOW_DAYS),
            2,
            1,
        ),
        ("a year, three approvers, M = N", 365, 3, 3),
    ];

    for (index, &(label, days, approvers, min_approvals)) in rows.iter().enumerate() {
        let harness = Harness::new();
        let arn = active_team(
            &harness,
            &format!("unapproved-deletion-{index}"),
            approvers,
            min_approvals,
        )
        .await;
        let (status, started) = harness
            .start_deletion(&arn, Some(json!({ "PendingWindowDays": days })))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: start deletion returned {status} with {started:?}"
        );

        // The approvers must not answer while this row holds the request unapproved, and
        // the window the request named is behind us.
        harness.postpone_deletion_approval(&arn);
        elapse_window(&harness, &arn);

        // Twice, because a read must not move the team either.
        for read in 1..=2 {
            let (status, team) = harness.get(&arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: an unapproved deletion must not remove the \
                 team, got {status} with {team:?}"
            );
            assert_eq!(
                text(&team, "Status"),
                state::TeamStatus::Deleting.as_wire(),
                "case {label}, read {read}: expected a DELETING team, got {team:?}"
            );
            assert_eq!(
                text(&team, "StatusCode"),
                state::TeamStatusCode::DeletePendingApproval.as_wire(),
                "case {label}, read {read}: the deletion is still pending the team's \
                 approval, got {team:?}"
            );
        }

        let (status, listed) = harness.list("MaxResults=20").await;
        assert_eq!(
            status, 200,
            "case {label}: list returned {status} with {listed:?}"
        );
        let names: Vec<String> = listed["ApprovalTeams"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|team| text(team, "Arn"))
            .collect();
        assert!(
            names.contains(&arn),
            "case {label}: a team whose deletion nobody approved must still be listed, \
             got {names:?}"
        );

        // The session is still there for the team to answer.
        let deletion_sessions = deletion_session_arns(&harness, &arn, label).await;
        assert_eq!(
            deletion_sessions.len(),
            1,
            "case {label}: the deletion the team has to answer must still be a session, \
             got {deletion_sessions:?}"
        );
    }
}

/// A deletion whose session ended without an approval is one the team refused: the team is
/// not deleted, it is active again and reports the failed approval — and it can be asked
/// to be deleted again.
#[tokio::test]
async fn a_deletion_the_team_never_approved_leaves_the_team_active() {
    // The two ways a session this service records ends without an approval.
    #[derive(Clone, Copy)]
    enum Ending {
        Cancelled,
        Expired,
    }

    let rows: &[(&str, Ending)] = &[
        ("the session cancelled before the answer came due", Ending::Cancelled),
        ("the session expired unanswered", Ending::Expired),
    ];

    for (index, &(label, ending)) in rows.iter().enumerate() {
        let harness = Harness::new();
        let arn = active_team(&harness, &format!("refused-deletion-{index}"), 3, 2).await;
        let (status, started) = harness
            .start_deletion(&arn, Some(json!({ "PendingWindowDays": 1 })))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: start deletion returned {status} with {started:?}"
        );
        // The approvers must not answer while this row arranges the ending under test.
        harness.postpone_deletion_approval(&arn);
        let sessions = deletion_session_arns(&harness, &arn, label).await;
        assert_eq!(
            sessions.len(),
            1,
            "case {label}: expected the one deletion session, got {sessions:?}"
        );
        let session_arn = sessions[0].clone();

        match ending {
            Ending::Cancelled => {
                let (status, cancelled) = cancel_session(&harness, &session_arn).await;
                assert_eq!(
                    status, 200,
                    "case {label}: cancel session returned {status} with {cancelled:?}"
                );
            }
            Ending::Expired => {
                expire_session(
                    &harness,
                    &session_arn,
                    SystemTime::now() - Duration::from_secs(1),
                );
            }
        }

        // Twice, because the refusal is recorded once and does not move under a read.
        for read in 1..=2 {
            let (status, team) = harness.get(&arn).await;
            assert_eq!(
                status, 200,
                "case {label}, read {read}: a team that refused its deletion must still \
                 read, got {status} with {team:?}"
            );
            assert_eq!(
                text(&team, "Status"),
                state::TeamStatus::Active.as_wire(),
                "case {label}, read {read}: a team whose deletion was not approved is \
                 active again, got {team:?}"
            );
            assert_eq!(
                text(&team, "StatusCode"),
                state::TeamStatusCode::DeleteFailedApproval.as_wire(),
                "case {label}, read {read}: wrong StatusCode after a deletion the team \
                 did not approve, got {team:?}"
            );
        }

        // An active team can be asked to be deleted again, which puts a SECOND request to
        // its approvers.
        let (status, again) = harness.start_deletion(&arn, None).await;
        assert_eq!(
            status, 200,
            "case {label}: a refused deletion must be requestable again, got {status} with \
             {again:?}"
        );
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {status} with {team:?}");
        assert_eq!(
            text(&team, "StatusCode"),
            state::TeamStatusCode::DeletePendingApproval.as_wire(),
            "case {label}: the second request waits on the team's approval too, got \
             {team:?}"
        );
        let sessions = deletion_session_arns(&harness, &arn, label).await;
        assert_eq!(
            sessions.len(),
            2,
            "case {label}: each request is put to the team in its own session, got \
             {sessions:?}"
        );
    }
}
