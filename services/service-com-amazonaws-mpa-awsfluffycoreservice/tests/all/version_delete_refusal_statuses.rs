// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATUS CLAIM every `DeleteInactiveApprovalTeamVersion`
//! refusal states.
//!
//! The rule under test: a `ConflictException` from this operation reports the status of
//! the VERSION the request named and names as deletable exactly the statuses this
//! operation deletes THAT version in — the version a team is RUNNING on while it has
//! never been activated (`PENDING`, the team draft) or is no longer active (`INACTIVE`),
//! and the version a team was ASKED TO APPROVE only once it is one the team will never
//! move to (`INACTIVE`). The two versions a team holds therefore earn two different
//! sentences, and the status a refusal reports is never among the statuses the same
//! sentence says would have been accepted.
//!
//! The first table spans the class the rule generalises over: which of the two versions
//! is asked for (the live one, the one the team's approvers are deciding) crossed with
//! the state the team is in when it is asked for (active; inside the pending deletion
//! window `StartActiveApprovalTeamDeletion` opens; awaiting its approvers' answer to a
//! change; carrying a change the approvers approved and the clock has not activated yet).
//!
//! The second table takes the refusals at their word: a version in a status the first
//! table's refusals name as deletable IS deleted, for the requested version (`INACTIVE`,
//! reached by the very remedy the refusal offers — cancelling the update session) and for
//! the live version of a team that has never been activated (`PENDING`). The live
//! version's `INACTIVE` half is the second row of
//! `inactive_team_version_deletion.rs`'s table and is not repeated here.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the `DELETE /approval-teams/{Arn}/{VersionId}`
//! route, the error envelope and the HTTP status are all exercised.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, text, Harness};
use crate::inactive_team_version_deletion::{
    create_body, delete_version, move_invitation_responses, stored_status,
};
use crate::pending_team_update::{postpone_approval, with_pending_update};
use crate::session_cancellation::cancel_session;

/// The two status claims a refusal makes, pulled out of the sentence that states them:
/// the status it reports for the version named, and the set of statuses it says this
/// operation deletes that version in.
fn status_claims(message: &str) -> (String, BTreeSet<String>) {
    let (reported, accepted) = message
        .split_once(" and not ")
        .unwrap_or_else(|| panic!("the refusal states no status claim: {message:?}"));
    let reported = reported
        .rsplit_once(" is ")
        .unwrap_or_else(|| panic!("the refusal reports no status: {message:?}"))
        .1
        .to_string();
    let accepted = accepted
        .split_once(", so ")
        .unwrap_or_else(|| panic!("the refusal names no accepted statuses: {message:?}"))
        .0
        .split(" or ")
        .map(str::to_string)
        .collect();
    (reported, accepted)
}

/// What the team holds, and in which state, when the delete is issued.
#[derive(Clone, Copy, Debug)]
enum Arrangement {
    /// An activated team, nothing pending.
    Active,
    /// A team inside the pending deletion window `StartActiveApprovalTeamDeletion` opens.
    Deleting,
    /// A team that has been asked to approve a change and has not answered.
    AwaitingApproval,
    /// A team whose approvers approved the change, before the clock activates it.
    ApprovedAwaitingActivation,
}

/// Which of the two versions a team holds the request names.
#[derive(Clone, Copy, Debug)]
enum Asked {
    /// The version the team is running on.
    Live,
    /// The version the team was asked to approve.
    Requested,
}

/// A stored team arranged for one row, and the ids of the versions it holds.
struct Arranged {
    arn: String,
    live_version: String,
    requested_version: Option<String>,
    session_arn: Option<String>,
}

impl Arranged {
    fn version(&self, asked: Asked) -> String {
        match asked {
            Asked::Live => self.live_version.clone(),
            Asked::Requested => self
                .requested_version
                .clone()
                .expect("an arrangement that holds a requested version"),
        }
    }
}

/// Create a team, let the clock-owed invitation responses be due so it activates, and
/// then put it in the state the row needs. Only DUE INSTANTS are moved; every transition
/// stays the service's own, exactly as `inactive_team_version_deletion.rs` arranges its
/// rows.
async fn arrange(harness: &Harness, label: &str, arrangement: Arrangement) -> Arranged {
    let (status, created) = harness.create(&create_body("refusal-statuses")).await;
    assert_eq!(status, 200, "case {label:?}: create returned {created:?}");
    let arn = text(&created, "Arn");

    move_invitation_responses(harness, &arn, Duration::from_secs(1), false);
    let (status, team) = harness.get(&arn).await;
    assert_eq!(status, 200, "case {label:?}: get returned {team:?}");
    assert_eq!(
        text(&team, "Status"),
        state::TeamStatus::Active.as_wire(),
        "case {label:?}: the team did not activate: {team:?}"
    );
    let live_version = text(&team, "VersionId");

    let mut arranged = Arranged {
        arn,
        live_version,
        requested_version: None,
        session_arn: None,
    };

    match arrangement {
        Arrangement::Active => {}
        Arrangement::Deleting => {
            let (status, started) = harness.start_deletion(&arranged.arn, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: start deletion returned {started:?}"
            );
            let (_, team) = harness.get(&arranged.arn).await;
            assert_eq!(
                text(&team, "Status"),
                state::TeamStatus::Deleting.as_wire(),
                "case {label:?}: the team is not inside its deletion window: {team:?}"
            );
        }
        Arrangement::AwaitingApproval | Arrangement::ApprovedAwaitingActivation => {
            let (status, updated) = harness
                .update(&arranged.arn, &json!({ "Description": "a change" }))
                .await;
            assert_eq!(status, 200, "case {label:?}: update returned {updated:?}");
            arranged.requested_version = Some(text(&updated, "VersionId"));
            // The approvers must not answer while the row is being arranged.
            postpone_approval(harness, &arranged.arn);
            if matches!(arrangement, Arrangement::ApprovedAwaitingActivation) {
                // The answer comes due, and the next call materialises it; the activation
                // it owes is measured from that instant, so the row reads the state in
                // between.
                with_pending_update(harness, &arranged.arn, |update| {
                    update.approval_due = Some(SystemTime::now());
                });
            }
            let (status, team) = harness.get(&arranged.arn).await;
            assert_eq!(status, 200, "case {label:?}: get returned {team:?}");
            arranged.session_arn = Some(text(&team, "UpdateSessionArn"));
            let expected_code = match arrangement {
                Arrangement::ApprovedAwaitingActivation => {
                    state::TeamStatusCode::UpdatePendingActivation
                }
                _ => state::TeamStatusCode::UpdatePendingApproval,
            };
            assert_eq!(
                text(&team["PendingUpdate"], "StatusCode"),
                expected_code.as_wire(),
                "case {label:?}: the requested version is not in the arranged stage: {team:?}"
            );
        }
    }

    arranged
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    arrangement: Arrangement,
    asked: Asked,
    /// The status the refusal must report for the version asked for.
    reported: &'static str,
    /// The statuses the refusal must name as the ones this operation deletes that version
    /// in.
    accepted: &'static [&'static str],
}

/// The statuses this operation deletes each of a team's two versions in. Every token is
/// one `ApprovalTeamStatus` declares (`smithy_query(query="shape",
/// shape="ApprovalTeamStatus")`: `ACTIVE`, `INACTIVE`, `DELETING`, `PENDING`), which
/// `the_statuses_the_refusals_name_are_ones_the_model_declares` below pins; the operation
/// documents the two it deletes in as an inactive team and a team draft
/// (`smithy_query(query="operation", operation="DeleteInactiveApprovalTeamVersion")`).
const LIVE_ACCEPTED: &[&str] = &["INACTIVE", "PENDING"];
/// A version the team was asked to approve is deleted only once it is one the team will
/// never move to, which `PendingUpdate$Status` reports as `INACTIVE`.
const REQUESTED_ACCEPTED: &[&str] = &["INACTIVE"];

fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "the version an active team is running on",
            arrangement: Arrangement::Active,
            asked: Asked::Live,
            reported: "ACTIVE",
            accepted: LIVE_ACCEPTED,
        },
        RefusalCase {
            case: "the version of a team inside its pending deletion window",
            arrangement: Arrangement::Deleting,
            asked: Asked::Live,
            reported: "DELETING",
            accepted: LIVE_ACCEPTED,
        },
        RefusalCase {
            // The TEAM reads PENDING for as long as the handshake runs, but the version it
            // is running on is the activated one its approvers already approved, which is
            // the status this refusal is about.
            case: "the version a team runs on while a change awaits its approval",
            arrangement: Arrangement::AwaitingApproval,
            asked: Asked::Live,
            reported: "ACTIVE",
            accepted: LIVE_ACCEPTED,
        },
        RefusalCase {
            case: "the version a team's approvers are still deciding",
            arrangement: Arrangement::AwaitingApproval,
            asked: Asked::Requested,
            reported: "PENDING",
            accepted: REQUESTED_ACCEPTED,
        },
        RefusalCase {
            case: "the version the approvers approved and the clock has not activated",
            arrangement: Arrangement::ApprovedAwaitingActivation,
            asked: Asked::Requested,
            reported: "PENDING",
            accepted: REQUESTED_ACCEPTED,
        },
    ]
}

/// The rule, over the whole table.
#[tokio::test]
async fn a_refusal_never_names_the_status_it_reports_among_the_ones_it_accepts() {
    for case in refusal_cases() {
        let label = case.case;
        let harness = Harness::new();
        let arranged = arrange(&harness, label, case.arrangement).await;
        let asked_for = arranged.version(case.asked);

        let before = stored_status(&harness, &arranged.arn);
        let (status, body) = delete_version(&harness, &arranged.arn, &asked_for).await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected HTTP 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label:?}: expected error code ConflictException, got {body:?}"
        );

        let message = text(&body, "Message");
        assert!(
            message.contains(&arranged.arn) && message.contains(&asked_for),
            "case {label:?}: the refusal names neither the team nor the version: {message:?}"
        );
        let (reported, accepted) = status_claims(&message);
        assert_eq!(
            reported, case.reported,
            "case {label:?}: expected the refusal to report {:?}, got {reported:?} in \
             {message:?}",
            case.reported
        );
        let expected: BTreeSet<String> = case.accepted.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            accepted, expected,
            "case {label:?}: expected the refusal to accept {expected:?}, got {accepted:?} in \
             {message:?}"
        );
        assert!(
            !accepted.contains(&reported),
            "case {label:?}: the refusal reports {reported:?} and calls it deletable in the same \
             sentence: {message:?}"
        );

        // The version's status, as the service itself reports it to a reader.
        let (status, team) = harness.get(&arranged.arn).await;
        assert_eq!(status, 200, "case {label:?}: get returned {team:?}");
        if matches!(case.asked, Asked::Requested) {
            assert_eq!(
                text(&team["PendingUpdate"], "Status"),
                reported,
                "case {label:?}: the refusal reports a status the read does not: {team:?}"
            );
        }

        // A refused delete is a refusal: nothing moved.
        assert_eq!(
            stored_status(&harness, &arranged.arn),
            before,
            "case {label:?}: the stored team changed under a call that must change nothing"
        );
        assert_eq!(
            text(&team, "VersionId"),
            arranged.live_version,
            "case {label:?}: the team's live version moved under a refusal: {team:?}"
        );
    }
}

/// Which version the follow-through row drives into an accepted status, and how.
#[derive(Clone, Copy, Debug)]
enum Accepted {
    /// The version a team was asked to approve, after the update session that decides it
    /// is cancelled — the remedy the refusal above offers, which leaves the requested
    /// version `INACTIVE`.
    RequestedAfterCancellation,
    /// The version a team that has never been activated is running on: the team draft,
    /// `PENDING`.
    LiveDraft,
}

/// The refusals are taken at their word: a version in a status they name as deletable is
/// deleted, with the 200 an accepted delete answers.
#[tokio::test]
async fn a_version_in_a_status_a_refusal_accepts_is_deleted() {
    for row in [Accepted::RequestedAfterCancellation, Accepted::LiveDraft] {
        let label = format!("{row:?}");
        let harness = Harness::new();

        let (arn, asked_for, expected_status, accepted, team_survives) = match row {
            Accepted::RequestedAfterCancellation => {
                let arranged = arrange(&harness, &label, Arrangement::AwaitingApproval).await;
                let session_arn = arranged
                    .session_arn
                    .clone()
                    .expect("the session the change was put in");
                let (status, cancelled) = cancel_session(&harness, &session_arn).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: cancel session returned {cancelled:?}"
                );
                (
                    arranged.arn.clone(),
                    arranged.version(Asked::Requested),
                    state::TeamStatus::Inactive,
                    REQUESTED_ACCEPTED,
                    true,
                )
            }
            Accepted::LiveDraft => {
                let (status, created) = harness.create(&create_body("accepted-status")).await;
                assert_eq!(status, 200, "case {label:?}: create returned {created:?}");
                let arn = text(&created, "Arn");
                // Hold the approvers' responses past the end of this test, so the version
                // is still the never-activated draft when the delete is issued.
                move_invitation_responses(&harness, &arn, Duration::from_secs(3600), true);
                let asked_for = text(&created, "VersionId");
                (
                    arn,
                    asked_for,
                    state::TeamStatus::Pending,
                    LIVE_ACCEPTED,
                    false,
                )
            }
        };

        // The status the refusals name as deletable is the one the read reports.
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label:?}: get returned {team:?}");
        let reported = match row {
            Accepted::RequestedAfterCancellation => text(&team["PendingUpdate"], "Status"),
            Accepted::LiveDraft => text(&team, "Status"),
        };
        assert_eq!(
            reported,
            expected_status.as_wire(),
            "case {label:?}: expected the version to read {}, got {reported:?} in {team:?}",
            expected_status.as_wire()
        );
        assert!(
            accepted.contains(&reported.as_str()),
            "case {label:?}: {reported:?} is not a status this operation's refusals accept for \
             this version: {accepted:?}"
        );

        let (status, body) = delete_version(&harness, &arn, &asked_for).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {body:?}"
        );

        let (status, team) = harness.get(&arn).await;
        if team_survives {
            assert_eq!(
                status, 200,
                "case {label:?}: the team must survive the delete: {team:?}"
            );
            assert!(
                team.get("PendingUpdate").is_none(),
                "case {label:?}: the deleted version is still reported: {team:?}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {label:?}: the deleted team is still reported: {team:?}"
            );
            assert_eq!(
                stored_status(&harness, &arn),
                None,
                "case {label:?}: the store still holds the deleted version"
            );
        }
    }
}

/// Every status token these refusals name is one the model declares, read off this
/// service's own `ApprovalTeamStatus` rendering rather than written out again.
#[test]
fn the_statuses_the_refusals_name_are_ones_the_model_declares() {
    let declared: BTreeSet<&'static str> = [
        state::TeamStatus::Active,
        state::TeamStatus::Inactive,
        state::TeamStatus::Deleting,
        state::TeamStatus::Pending,
    ]
    .into_iter()
    .map(state::TeamStatus::as_wire)
    .collect();
    for status in LIVE_ACCEPTED
        .iter()
        .chain(REQUESTED_ACCEPTED)
        .chain(["ACTIVE", "DELETING"].iter())
    {
        assert!(
            declared.contains(status),
            "{status:?} is not a status this service's ApprovalTeamStatus declares: {declared:?}"
        );
    }
}
