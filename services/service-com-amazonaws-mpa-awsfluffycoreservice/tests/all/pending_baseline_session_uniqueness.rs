// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE pending baseline session an approver may have.
//!
//! The rules under test:
//!
//! * an approver is asked by at most ONE outstanding baseline session of a team —
//!   `GetApprovalTeamResponseApprover$PendingBaselineSessionArn` is a single `SessionArn`,
//!   documented as "the pending baseline session" of that approver — so a
//!   `StartApprovalTeamBaseline` whose approver set overlaps an outstanding baseline
//!   session's is refused with the modelled `ValidationException` (400), naming the
//!   approver and the session already waiting on them, while a start whose approvers
//!   overlap none of them is accepted.
//! * a refused start records NOTHING: the store holds the sessions it held, and the
//!   initiation-order id the next accepted start carries is the one that follows the last
//!   session really stored.
//! * the session an approver waits on is only pending while it is OUTSTANDING: once it
//!   ends — cancelled, or expired by the clock — that approver can be asked again, and the
//!   team read then reports the new session for them.
//!
//! The tables span the class the first rule generalises over (how the two starts overlap:
//! the same single approver, a superset, a partial overlap, `ApproverIds` absent on either
//! side, and the disjoint sets that are accepted) and the way an outstanding session ends,
//! not one shape.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::ops::Range;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{
    active_team, expire_session, get_session, get_team, message, start_baseline,
    stored_approver_ids,
};
use crate::approval_team_lifecycle::{error_code, text, Harness};
use crate::session_cancellation::cancel_session;

/// One row of the overlap table.
struct OverlapCase {
    case: &'static str,
    team_name: &'static str,
    approvers: usize,
    /// The team approvers (by position in team order) the FIRST start selects; `None`
    /// leaves `ApproverIds` absent, which asks the whole team.
    first: Option<Range<usize>>,
    /// The team approvers the SECOND start selects; `None` leaves `ApproverIds` absent.
    second: Option<Range<usize>>,
    /// The position of the approver the refusal must name, or `None` when the second start
    /// overlaps nobody and is accepted.
    refused_on: Option<usize>,
}

/// The overlap table: rows differ along how the second start's approver set meets the
/// first's (identical, a superset, a partial overlap, the whole team, disjoint), which
/// side leaves `ApproverIds` absent, and the team's approver count.
fn overlap_cases() -> Vec<OverlapCase> {
    vec![
        OverlapCase {
            case: "the same single approver named twice",
            team_name: "overlap-same-approver",
            approvers: 2,
            first: Some(0..1),
            second: Some(0..1),
            refused_on: Some(0),
        },
        OverlapCase {
            case: "ApproverIds absent, then that one approver named",
            team_name: "Overlap.Absent_1",
            approvers: 1,
            first: None,
            second: Some(0..1),
            refused_on: Some(0),
        },
        OverlapCase {
            case: "one approver, then a superset naming them",
            team_name: "o",
            approvers: 3,
            first: Some(0..1),
            second: Some(0..2),
            refused_on: Some(0),
        },
        OverlapCase {
            case: "two approvers, then a set overlapping the second of them",
            team_name: "overlap-partial",
            approvers: 3,
            first: Some(0..2),
            second: Some(1..3),
            refused_on: Some(1),
        },
        OverlapCase {
            case: "one approver, then ApproverIds absent so the whole team is asked",
            team_name: "overlap-whole-team",
            approvers: 3,
            first: Some(1..2),
            second: None,
            refused_on: Some(1),
        },
        OverlapCase {
            case: "one approver, then a disjoint one",
            team_name: "overlap-disjoint",
            approvers: 3,
            first: Some(0..1),
            second: Some(1..3),
            refused_on: None,
        },
        OverlapCase {
            case: "half the team, then the half it left out",
            team_name: "overlap-split-team",
            approvers: 4,
            first: Some(0..2),
            second: Some(2..4),
            refused_on: None,
        },
    ]
}

/// The ARNs of the sessions the store holds for a team, in initiation order, with the
/// initiation-order id each one carries.
fn stored_sessions(harness: &Harness, team_arn: &str) -> Vec<(String, u64)> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .team_sessions_in_initiation_order(team_arn)
        .into_iter()
        .map(|session| (session.arn.clone(), session.seq))
        .collect()
}

/// What `GetApprovalTeam` reports as each approver's pending baseline session, in team
/// order.
async fn reported_pending(harness: &Harness, team_arn: &str) -> Vec<Option<String>> {
    let (status, team) = get_team(harness, team_arn).await;
    assert_eq!(status, 200, "GetApprovalTeam returned {status} with {team:?}");
    team["Approvers"]
        .as_array()
        .expect("Approvers is an array")
        .iter()
        .map(|entry| {
            entry
                .get("PendingBaselineSessionArn")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The sentence a start that would ask an approver already waiting on a baseline session
/// is refused with.
fn expected_refusal(approver_id: &str, team_arn: &str, pending: &str) -> String {
    format!(
        "Approver '{approver_id}' of approval team '{team_arn}' already has baseline session \
         '{pending}' pending, so another baseline session cannot be started for them."
    )
}

/// Over the whole overlap table: a start that would ask an approver who already has an
/// outstanding baseline session is refused and records nothing, and a start that overlaps
/// nobody is accepted.
#[tokio::test]
async fn a_start_overlapping_an_outstanding_baseline_session_is_refused() {
    let harness = Harness::new();

    for case in overlap_cases() {
        let label = case.case;
        let team_arn = active_team(&harness, case.team_name, case.approvers, 1).await;
        let team_approver_ids = stored_approver_ids(&harness, &team_arn);

        let first_selection: Option<Vec<String>> = case
            .first
            .clone()
            .map(|range| team_approver_ids[range].to_vec());
        let (status, started) = start_baseline(&harness, &team_arn, first_selection.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {label}: the first start returned {status} with {started:?}"
        );
        let first_arn = text(&started, "BaselineSessionArn");
        let before = stored_sessions(&harness, &team_arn);

        let second_selection: Option<Vec<String>> = case
            .second
            .clone()
            .map(|range| team_approver_ids[range].to_vec());
        let (status, body) =
            start_baseline(&harness, &team_arn, second_selection.as_deref()).await;

        match case.refused_on {
            Some(position) => {
                assert_eq!(
                    status, 400,
                    "case {label}: expected 400 for an overlapping start, got {status} with \
                     {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {label}: wrong error code in {body:?}"
                );
                assert_eq!(
                    message(&body),
                    expected_refusal(&team_approver_ids[position], &team_arn, &first_arn),
                    "case {label}: wrong refusal sentence in {body:?}"
                );
                assert_eq!(
                    stored_sessions(&harness, &team_arn),
                    before,
                    "case {label}: a refused start must record nothing"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {label}: expected 200 for a start overlapping nobody, got {status} \
                     with {body:?}"
                );
                let second_arn = text(&body, "BaselineSessionArn");
                assert_ne!(
                    second_arn, first_arn,
                    "case {label}: an accepted start mints its own session, got {second_arn:?}"
                );
                let after = stored_sessions(&harness, &team_arn);
                assert_eq!(
                    after.len(),
                    before.len() + 1,
                    "case {label}: expected one more stored session, got {after:?}"
                );
            }
        }

        // Whichever way the second start was answered, every approver the team read
        // reports a pending session for reports the session `GetSession` agrees is
        // outstanding and asks them.
        for (position, reported) in reported_pending(&harness, &team_arn)
            .await
            .into_iter()
            .enumerate()
        {
            let Some(session_arn) = reported else {
                continue;
            };
            let (status, session) = get_session(&harness, &session_arn).await;
            assert_eq!(
                status, 200,
                "case {label}, approver {position}: GetSession({session_arn}) returned {status} \
                 with {session:?}"
            );
            let asked: Vec<String> = session["ApproverResponses"]
                .as_array()
                .expect("ApproverResponses is an array")
                .iter()
                .map(|entry| text(entry, "ApproverId"))
                .collect();
            assert!(
                asked.contains(&team_approver_ids[position]),
                "case {label}, approver {position}: the session reported as pending for them \
                 must ask them, got {asked:?}"
            );
            assert!(
                ["CREATING", "PENDING"].contains(&text(&session, "Status").as_str()),
                "case {label}, approver {position}: only an outstanding session is pending, got \
                 {session:?}"
            );
        }
    }
}

/// A refused start consumes no initiation-order id: the next session the store really
/// records carries the id that follows the last one it holds.
#[tokio::test]
async fn a_refused_start_consumes_no_session_id() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "refused-start-ids", 3, 1).await;
    let approvers = stored_approver_ids(&harness, &team_arn);

    let (status, started) = start_baseline(&harness, &team_arn, Some(&approvers[0..1])).await;
    assert_eq!(status, 200, "the first start returned {started:?}");
    let first_arn = text(&started, "BaselineSessionArn");

    // Two refused starts, so the sequence cannot creep forward by one per refusal either.
    for attempt in 0..2 {
        let (status, body) = start_baseline(&harness, &team_arn, Some(&approvers[0..1])).await;
        assert_eq!(
            status, 400,
            "attempt {attempt}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            expected_refusal(&approvers[0], &team_arn, &first_arn),
            "attempt {attempt}: wrong refusal sentence in {body:?}"
        );
    }

    let (status, started) = start_baseline(&harness, &team_arn, Some(&approvers[1..2])).await;
    assert_eq!(status, 200, "the accepted start returned {started:?}");
    let second_arn = text(&started, "BaselineSessionArn");

    let stored = stored_sessions(&harness, &team_arn);
    assert_eq!(
        stored.iter().map(|(arn, _)| arn.clone()).collect::<Vec<_>>(),
        vec![first_arn, second_arn],
        "expected exactly the two accepted sessions, got {stored:?}"
    );
    assert_eq!(
        stored[1].1,
        stored[0].1 + 1,
        "expected consecutive initiation-order ids, got {stored:?}"
    );
}

/// How the row's first session stops being outstanding.
enum Ending {
    /// Ended by its caller, through `CancelSession`.
    Cancelled,
    /// Ended by the clock, its expiration having passed unanswered.
    Expired,
}

/// One row of the table of ways an outstanding session ends.
struct EndedCase {
    case: &'static str,
    team_name: &'static str,
    ending: Ending,
}

/// Once the session an approver waits on is OVER, that approver can be asked again — and
/// the team read then reports the new session for them. The rows differ along how the
/// first session ended: cancelled by its caller, or expired by the clock.
#[tokio::test]
async fn an_approver_whose_baseline_session_is_over_can_be_asked_again() {
    let cases: [EndedCase; 2] = [
        EndedCase {
            case: "the first session was cancelled",
            team_name: "ended-by-cancel",
            ending: Ending::Cancelled,
        },
        EndedCase {
            case: "the first session expired unanswered",
            team_name: "ended-by-expiry",
            ending: Ending::Expired,
        },
    ];

    for case in cases {
        let label = case.case;
        let harness = Harness::new();
        let team_arn = active_team(&harness, case.team_name, 2, 1).await;
        let approvers = stored_approver_ids(&harness, &team_arn);

        let (status, started) = start_baseline(&harness, &team_arn, None).await;
        assert_eq!(status, 200, "case {label}: the first start returned {started:?}");
        let first_arn = text(&started, "BaselineSessionArn");

        match case.ending {
            Ending::Cancelled => {
                let (status, body) = cancel_session(&harness, &first_arn).await;
                assert_eq!(
                    status, 200,
                    "case {label}: CancelSession returned {status} with {body:?}"
                );
            }
            Ending::Expired => expire_session(
                &harness,
                &first_arn,
                SystemTime::now() - Duration::from_secs(1),
            ),
        }

        let (status, started) = start_baseline(&harness, &team_arn, None).await;
        assert_eq!(
            status, 200,
            "case {label}: a start for approvers whose session is over returned {status} with \
             {started:?}"
        );
        let second_arn = text(&started, "BaselineSessionArn");
        assert_ne!(
            second_arn, first_arn,
            "case {label}: the second start mints its own session"
        );

        assert_eq!(
            reported_pending(&harness, &team_arn).await,
            vec![Some(second_arn.clone()); approvers.len()],
            "case {label}: every approver's pending session is the one still outstanding"
        );

        // And the approvers are now waiting on that session, so a third start is refused
        // naming it rather than the session that ended.
        let (status, body) = start_baseline(&harness, &team_arn, Some(&approvers[0..1])).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400 for a third start, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            expected_refusal(&approvers[0], &team_arn, &second_arn),
            "case {label}: the refusal must name the session still outstanding, got {body:?}"
        );
    }
}
