// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `ExecutionStatus` `GetSession` reports: how far the action a
//! session approves has got.
//!
//! The rule under test: a session reports the action it approves as `PENDING` for as long
//! as that action has not been carried out, and as `EXECUTED` from the instant this
//! service carried it out — every session here carries
//! `AUTO_COMPLETION_UPON_APPROVAL`, under which an approved action is executed on the
//! approval, so the approval alone is the session's `Status` and the execution is what the
//! action really taking effect records. An action that was never carried out leaves the
//! execution where it started.
//!
//! The tables span the class the rule generalises over: the stage the action has reached
//! (waiting on the approvers, approved but not yet carried out, carried out), the members
//! the carried-out action changes (description, strategy, approvers added, approvers
//! dropped, every member at once), how many approvers the team has (1, 2, 3 and the
//! `Approvers` `length(1..20)` maximum), how many approvals its strategy requires (M = 1,
//! M below N, M = N), and the ways a session ends with its action never carried out
//! (outstanding, cancelled, expired unanswered).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{
    active_team, expire_session, get_session, start_baseline, wait_for_the_clock, MAX_APPROVERS,
};
use crate::approval_team_lifecycle::{text, Harness};
use crate::pending_team_update::{
    approver_member, make_activation_due, make_approval_due, postpone_approval,
};
use crate::session_cancellation::cancel_session;

/// The `ExecutionStatus` a session reports, asserted to be a token the model declares.
async fn execution_status(harness: &Harness, session_arn: &str, label: &str) -> String {
    let (status, session) = get_session(harness, session_arn).await;
    assert_eq!(
        status, 200,
        "case {label}: GetSession returned {status} with {session:?}"
    );
    let reported = text(&session, "ExecutionStatus");
    assert!(
        model::SESSION_EXECUTION_STATUS_VALUES.contains(&reported.as_str()),
        "case {label}: expected a token SessionExecutionStatus declares, got {reported:?}"
    );
    reported
}

/// The `UpdateSessionArn` a team reports while a change is waiting on it.
async fn update_session_arn(harness: &Harness, team_arn: &str, label: &str) -> String {
    let (status, team) = harness.get(team_arn).await;
    assert_eq!(
        status, 200,
        "case {label}: GetApprovalTeam returned {status} with {team:?}"
    );
    let session_arn = text(&team, "UpdateSessionArn");
    assert!(
        !session_arn.is_empty(),
        "case {label}: the team must name the session that decides the change, got {team:?}"
    );
    session_arn
}

/// One row of the execution table: the team the change is asked of, the change itself, and
/// the description the team must be running on once the change has been carried out.
struct ExecutionCase {
    case: &'static str,
    team_approvers: usize,
    team_min_approvals: i64,
    patch: Value,
    description_after: String,
}

/// The rows differ along the members the action changes, the size of the team that
/// approves it and the threshold its strategy requires — not along one recorded shape.
fn execution_cases() -> Vec<ExecutionCase> {
    vec![
        ExecutionCase {
            case: "a description, one approver, M = N",
            team_approvers: 1,
            team_min_approvals: 1,
            patch: json!({ "Description": "carried out" }),
            description_after: "carried out".to_string(),
        },
        ExecutionCase {
            case: "a strategy that raises M to N, three approvers",
            team_approvers: 3,
            team_min_approvals: 1,
            patch: json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 3 } } }),
            // `approval_sessions::active_team` creates every team with this description.
            description_after: "session table".to_string(),
        },
        ExecutionCase {
            case: "an Approvers that adds one, two approvers, M = N",
            team_approvers: 2,
            team_min_approvals: 2,
            patch: json!({ "Approvers": (1..=3).map(approver_member).collect::<Vec<_>>() }),
            description_after: "session table".to_string(),
        },
        ExecutionCase {
            case: "an Approvers that drops one, three approvers, M below N",
            team_approvers: 3,
            team_min_approvals: 2,
            patch: json!({ "Approvers": (1..=2).map(approver_member).collect::<Vec<_>>() }),
            description_after: "session table".to_string(),
        },
        ExecutionCase {
            case: "every member at once, two approvers, M = N",
            patch: json!({
                "Description": "d",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": [approver_member(1)],
            }),
            team_approvers: 2,
            team_min_approvals: 2,
            // `Description` is `length(min: 1, max: 256)` — the minimum.
            description_after: "d".to_string(),
        },
        ExecutionCase {
            // `Approvers` is `length(min: 1, max: 20)` on create and on update alike.
            case: "the largest team the model allows",
            team_approvers: MAX_APPROVERS,
            team_min_approvals: MAX_APPROVERS as i64,
            patch: json!({ "Description": "x".repeat(256) }),
            // `Description` is `length(min: 1, max: 256)` — the maximum.
            description_after: "x".repeat(256),
        },
    ]
}

/// Over the whole table: the update session reports its action `PENDING` while the change
/// is waiting and while it is approved but not yet live, and `EXECUTED` from the instant
/// the change became the team's live configuration — which `GetApprovalTeam` proves.
#[tokio::test]
async fn an_update_session_reports_its_action_executed_once_the_change_is_carried_out() {
    for (index, case) in execution_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let team_arn = active_team(
            &harness,
            &format!("session-execution-{index}"),
            case.team_approvers,
            case.team_min_approvals,
        )
        .await;
        let (_, before) = harness.get(&team_arn).await;
        let live_version = text(&before, "VersionId");

        let (status, updated) = harness.update(&team_arn, &case.patch).await;
        assert_eq!(
            status, 200,
            "case {label}: UpdateApprovalTeam returned {status} with {updated:?}"
        );
        let requested_version = text(&updated, "VersionId");
        // The approvers must not answer while this row reads the stage before their answer.
        postpone_approval(&harness, &team_arn);
        let session_arn = update_session_arn(&harness, &team_arn, label).await;

        // Waiting on the approvers: the action has not run.
        let waiting = execution_status(&harness, &session_arn, label).await;
        assert_eq!(
            waiting,
            state::SessionExecutionStatus::Pending.as_wire(),
            "case {label}: an action still waiting on the approvers has not been executed, \
             got {waiting:?}"
        );

        // Approved, and so owed its activation: the action still has not run, and the team
        // is still on the configuration it was running on.
        make_approval_due(&harness, &team_arn);
        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: GetSession returned {session:?}");
        assert_eq!(
            text(&session, "Status"),
            state::SessionStatus::Approved.as_wire(),
            "case {label}: the session must carry the approval by now, got {session:?}"
        );
        assert_eq!(
            text(&session, "ActionCompletionStrategy"),
            state::ActionCompletionStrategy::AutoCompletionUponApproval.as_wire(),
            "case {label}: wrong ActionCompletionStrategy, got {session:?}"
        );
        assert_eq!(
            text(&session, "ActionName"),
            state::SessionKind::Update.action_name(),
            "case {label}: wrong ActionName on an update session, got {session:?}"
        );
        assert_eq!(
            text(&session, "ExecutionStatus"),
            state::SessionExecutionStatus::Pending.as_wire(),
            "case {label}: an approved action that has not taken effect yet is not \
             executed, got {session:?}"
        );
        let (_, approved_team) = harness.get(&team_arn).await;
        assert_eq!(
            text(&approved_team, "VersionId"),
            live_version,
            "case {label}: the live version must not move on the approval alone, got \
             {approved_team:?}"
        );

        // Carried out: the requested version is the team's live one, and the session
        // reports the action as executed.
        make_activation_due(&harness, &team_arn);
        let (status, live) = harness.get(&team_arn).await;
        assert_eq!(status, 200, "case {label}: GetApprovalTeam returned {live:?}");
        assert_eq!(
            text(&live, "VersionId"),
            requested_version,
            "case {label}: the approved version must be the live one, got {live:?}"
        );
        assert_eq!(
            text(&live, "Description"),
            case.description_after,
            "case {label}: the approved configuration must be the live one, got {live:?}"
        );
        let executed = execution_status(&harness, &session_arn, label).await;
        assert_eq!(
            executed,
            state::SessionExecutionStatus::Executed.as_wire(),
            "case {label}: the action this session approves has been carried out, got \
             {executed:?}"
        );

        // A read does not move it: two reads of the same session agree, member for
        // member.
        let (_, first) = get_session(&harness, &session_arn).await;
        let (_, second) = get_session(&harness, &session_arn).await;
        assert_eq!(
            first, second,
            "case {label}: a read moved the session: {first:?} then {second:?}"
        );
        assert_eq!(
            text(&first, "Status"),
            state::SessionStatus::Approved.as_wire(),
            "case {label}: an executed action leaves the session APPROVED, got {first:?}"
        );
    }
}

/// The two ways an approved change can be carried out are the same change: the CLOCK
/// alone, with nothing arranged, carries it through its approval and its activation, and
/// the session reports the action executed exactly once the team is live on it.
#[tokio::test]
async fn the_clock_alone_carries_an_approved_action_through_to_executed() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "clock-execution", 2, 1).await;
    let (_, before) = harness.get(&team_arn).await;
    let live_version = text(&before, "VersionId");

    let (status, updated) = harness
        .update(&team_arn, &json!({ "Description": "carried by the clock" }))
        .await;
    assert_eq!(status, 200, "UpdateApprovalTeam returned {updated:?}");
    let requested_version = text(&updated, "VersionId");
    let session_arn = update_session_arn(&harness, &team_arn, "the clock alone").await;

    // Straight after the call the change is still waiting: the span is never zero.
    let waiting = execution_status(&harness, &session_arn, "the clock alone").await;
    assert_eq!(
        waiting,
        state::SessionExecutionStatus::Pending.as_wire(),
        "straight after the call nothing has been carried out, got {waiting:?}"
    );

    wait_for_the_clock(state::UPDATE_APPROVAL_DELAY);
    let (status, approved) = get_session(&harness, &session_arn).await;
    assert_eq!(status, 200, "GetSession returned {approved:?}");
    assert_eq!(
        text(&approved, "Status"),
        state::SessionStatus::Approved.as_wire(),
        "once the approval is due the session carries it: {approved:?}"
    );
    assert_eq!(
        text(&approved, "ExecutionStatus"),
        state::SessionExecutionStatus::Pending.as_wire(),
        "the approval alone carries nothing out: {approved:?}"
    );
    let (_, team) = harness.get(&team_arn).await;
    assert_eq!(
        text(&team, "VersionId"),
        live_version,
        "the live version does not move on the approval alone: {team:?}"
    );

    wait_for_the_clock(state::UPDATE_ACTIVATION_DELAY);
    let (status, live) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "GetApprovalTeam returned {live:?}");
    assert_eq!(
        text(&live, "VersionId"),
        requested_version,
        "once the activation is due the approved version is the live one: {live:?}"
    );
    let executed = execution_status(&harness, &session_arn, "the clock alone").await;
    assert_eq!(
        executed,
        state::SessionExecutionStatus::Executed.as_wire(),
        "the action the session approves has been carried out, got {executed:?}"
    );
}

/// How a session whose action was never carried out ends, per row: nothing moves its
/// execution.
#[derive(Clone, Copy)]
enum Unexecuted {
    /// A baseline session nobody has answered, which no clock-owed answer ever approves.
    OutstandingBaseline,
    /// A baseline session `CancelSession` ended.
    CancelledBaseline,
    /// An update session cancelled before the approvers' answer came due.
    CancelledUpdate,
    /// An update session that ran out with no answer.
    ExpiredUpdate,
}

/// A session whose action was never carried out reports it `PENDING`, however the session
/// itself ended — across every ending this service records, and under a second read.
#[tokio::test]
async fn a_session_whose_action_was_never_carried_out_reports_it_pending() {
    let rows: &[(&str, Unexecuted)] = &[
        ("an outstanding baseline session", Unexecuted::OutstandingBaseline),
        ("a cancelled baseline session", Unexecuted::CancelledBaseline),
        ("an update session cancelled unanswered", Unexecuted::CancelledUpdate),
        ("an update session expired unanswered", Unexecuted::ExpiredUpdate),
    ];

    for (index, &(label, ending)) in rows.iter().enumerate() {
        let harness = Harness::new();
        let team_arn = active_team(&harness, &format!("unexecuted-{index}"), 3, 2).await;

        let session_arn = match ending {
            Unexecuted::OutstandingBaseline | Unexecuted::CancelledBaseline => {
                let (status, started) = start_baseline(&harness, &team_arn, None).await;
                assert_eq!(
                    status, 200,
                    "case {label}: StartApprovalTeamBaseline returned {status} with \
                     {started:?}"
                );
                text(&started, "BaselineSessionArn")
            }
            Unexecuted::CancelledUpdate | Unexecuted::ExpiredUpdate => {
                let (status, updated) = harness
                    .update(&team_arn, &json!({ "Description": "never carried out" }))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {label}: UpdateApprovalTeam returned {status} with {updated:?}"
                );
                // The approvers must not answer while this row arranges the ending.
                postpone_approval(&harness, &team_arn);
                update_session_arn(&harness, &team_arn, label).await
            }
        };

        match ending {
            Unexecuted::OutstandingBaseline => {}
            Unexecuted::CancelledBaseline | Unexecuted::CancelledUpdate => {
                let (status, cancelled) = cancel_session(&harness, &session_arn).await;
                assert_eq!(
                    status, 200,
                    "case {label}: CancelSession returned {status} with {cancelled:?}"
                );
            }
            Unexecuted::ExpiredUpdate => {
                expire_session(
                    &harness,
                    &session_arn,
                    SystemTime::now() - Duration::from_secs(1),
                );
            }
        }

        // Twice, because a read must not move the execution either.
        for read in 1..=2 {
            let reported = execution_status(&harness, &session_arn, label).await;
            assert_eq!(
                reported,
                state::SessionExecutionStatus::Pending.as_wire(),
                "case {label}, read {read}: nothing was carried out, so the execution is \
                 still waiting, got {reported:?}"
            );
        }
    }
}
