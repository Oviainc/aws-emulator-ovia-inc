// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IN-USE rule at the instant a scheduled identity-source
//! removal comes DUE, rather than at the instant it was asked for.
//!
//! The rule under test: an identity source an approver record of a stored approval team
//! authenticates through is in use and is not removed, so the clock pass that carries a
//! scheduled removal out decides it against the store as it stands when the removal comes
//! due — not against the store the accepted `DeleteIdentitySource` read. A reference
//! recorded after that call was accepted therefore keeps the source: it stays `DELETING`
//! with its removal still owed, a later `DeleteIdentitySource` for it earns the same
//! modelled `ConflictException` (409), and the removal is carried out at the first
//! instant no stored team names the source any more. No read ever reports an approver
//! authenticated by an identity source this store no longer holds.
//!
//! The table spans the class the rule generalises over: WHICH write path records the
//! reference while the removal is owed (`CreateApprovalTeam`'s live set,
//! `UpdateApprovalTeam`'s requested set), WHERE in the approver set the reference sits
//! (only, first of three, last of three), how many teams the store holds and which of
//! them holds it, the status the source was in when the delete was accepted (`CREATING`
//! and `ACTIVE`, the two statuses a created source is in), and the control rows where no
//! stored team names the source at all — those, and only those, are removed.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the error envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, text, Harness};
use crate::identity_source_deletion::{create_source, delete_source, listed_status, stored};
use crate::identity_source_detail::get_source;
use crate::identity_source_in_use_by_pending_update::{
    approver_member, reported_sources, POLICY_ARN,
};
use crate::pending_team_update::postpone_approval;

/// Slack over a clock-owed span before the transition it owes is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// Hold a scheduled removal open for the rest of a row, so the reference below is really
/// recorded while the removal is owed and not yet due. Only the DUE INSTANT is moved, the
/// way `pending_team_update.rs` moves a requested change's approval; the removal itself is
/// the service's own.
fn postpone_removal(harness: &Harness, arn: &str) {
    with_stored_source(harness, arn, |source| {
        source.deletion_due = Some(SystemTime::now() + Duration::from_secs(3600));
    });
}

/// Pull that same instant into the past, so the scheduled removal is due on the next call.
fn make_removal_due(harness: &Harness, arn: &str) {
    with_stored_source(harness, arn, |source| {
        source.deletion_due = Some(SystemTime::now() - Duration::from_secs(1));
    });
}

fn with_stored_source(
    harness: &Harness,
    arn: &str,
    edit: impl FnOnce(&mut state::IdentitySource),
) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let source = guard
        .identity_source_mut(arn)
        .expect("a stored identity source to arrange");
    edit(source);
}

/// How the in-use reference is recorded while the removal is owed.
enum Reference {
    /// No approval team is created at all, so nothing names the source.
    NoTeam,
    /// Teams are CREATED while the removal is owed: one entry per team, each listing its
    /// approvers as `true` for the source under test and `false` for the bystander.
    Created(Vec<Vec<bool>>),
    /// A team created and activated BEFORE the delete is asked to approve a change naming
    /// the source, so only the REQUESTED approver set names it.
    Requested,
}

/// One row of the table.
struct Case {
    case: &'static str,
    reference: Reference,
    /// Whether the source's clock-owed activation is awaited first, so the delete is
    /// accepted against an `ACTIVE` source rather than a `CREATING` one — the two
    /// statuses a created source is in.
    activate_first: bool,
    /// Whether the scheduled removal must be carried out when it comes due.
    removed: bool,
}

/// Rows differ along the write path that records the reference, where in the approver set
/// it sits, how many teams the store holds and which one holds it, the status the source
/// was in when the delete was accepted, and whether any team names the source at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no team at all, the source still CREATING when the delete was accepted",
            reference: Reference::NoTeam,
            activate_first: false,
            removed: true,
        },
        Case {
            case: "no team at all, the source ACTIVE when the delete was accepted",
            reference: Reference::NoTeam,
            activate_first: true,
            removed: true,
        },
        Case {
            case: "one team created in the window, its only approver on the source",
            reference: Reference::Created(vec![vec![true]]),
            activate_first: false,
            removed: false,
        },
        Case {
            case: "one team created in the window, the source FIRST of three approvers",
            reference: Reference::Created(vec![vec![true, false, false]]),
            activate_first: true,
            removed: false,
        },
        Case {
            case: "one team created in the window, the source LAST of three approvers",
            reference: Reference::Created(vec![vec![false, false, true]]),
            activate_first: false,
            removed: false,
        },
        Case {
            case: "three teams created in the window, only the LAST names the source",
            reference: Reference::Created(vec![vec![false], vec![false, false], vec![true]]),
            activate_first: true,
            removed: false,
        },
        Case {
            case: "three teams created in the window, only the FIRST names the source",
            reference: Reference::Created(vec![vec![true, false], vec![false], vec![false]]),
            activate_first: false,
            removed: false,
        },
        Case {
            case: "two teams created in the window, neither on the source",
            reference: Reference::Created(vec![vec![false, false], vec![false]]),
            activate_first: true,
            removed: true,
        },
        Case {
            case: "a change requested in the window names the source, the live set does not",
            reference: Reference::Requested,
            activate_first: true,
            removed: false,
        },
    ]
}

/// The rule, over the whole table: when the scheduled removal comes due the source is
/// removed exactly when no approver set of any stored team names it, and a source still
/// named keeps its record, its `DELETING` status and its owed removal.
#[tokio::test]
async fn a_scheduled_removal_reads_the_in_use_rule_when_it_comes_due() {
    for (row, case) in cases().into_iter().enumerate() {
        let label = case.case;
        // Creating an identity source and a team is restricted to the organization's
        // management account, so the table runs as it; that gate is tested in
        // `authorization_gate.rs`.
        let harness = Harness::new();
        let bystander = create_source(&harness, 1, 0).await;
        let target = create_source(&harness, 2, 0).await;
        let source_of = |on_target: bool| {
            if on_target {
                target.clone()
            } else {
                bystander.clone()
            }
        };
        let approvers = |flags: &[bool]| {
            Value::Array(
                flags
                    .iter()
                    .enumerate()
                    .map(|(index, &on_target)| approver_member(index, &source_of(on_target)))
                    .collect(),
            )
        };

        // The team the requested-change row asks to approve a change exists BEFORE the
        // delete, with its live approvers on the bystander source only, and has to be
        // ACTIVE for the change to be put to its approvers.
        let mut using_team: Option<String> = None;
        let existing = if matches!(case.reference, Reference::Requested) {
            let (status, created) = harness
                .create(&json!({
                    "Name": format!("pre-existing-team-{row}"),
                    "Description": "a team whose change is asked for inside the window",
                    "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                    "Approvers": approvers(&[false]),
                    "Policies": [{ "PolicyArn": POLICY_ARN }],
                }))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: creating the pre-existing team returned {status} with {created:?}"
            );
            let team_arn = text(&created, "Arn");
            tokio::time::sleep(state::INVITATION_RESPONSE_DELAY + CLOCK_SLACK).await;
            let (status, read) = harness.get(&team_arn).await;
            assert_eq!(
                text(&read, "Status"),
                "ACTIVE",
                "case {label:?}: expected an ACTIVE team before the change, got {status} \
                 with {read:?}"
            );
            Some(team_arn)
        } else {
            None
        };

        if case.activate_first {
            tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
            assert_eq!(
                listed_status(&harness, &target).await.as_deref(),
                Some(state::IdentitySourceStatus::Active.as_wire()),
                "case {label:?}: the source did not become ACTIVE before the delete"
            );
        }

        // The delete is accepted while nothing names the source: it records `DELETING`
        // and the instant the removal comes due.
        let (status, body) = delete_source(&harness, &target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for the accepted delete, got {status} \
             with {body:?}"
        );
        let deleting = state::IdentitySourceStatus::Deleting.as_wire();
        assert_eq!(
            stored(&harness, &target).map(|(status, _)| status),
            Some(deleting),
            "case {label:?}: the accepted delete did not record {deleting:?}"
        );
        // The window is held open for the rest of the row, so the reference below is
        // recorded while the removal is owed and the row does not race the clock.
        postpone_removal(&harness, &target);

        // The reference the accepted delete could not have seen.
        match &case.reference {
            Reference::NoTeam => {}
            Reference::Created(teams) => {
                for (team, flags) in teams.iter().enumerate() {
                    let (status, created) = harness
                        .create(&json!({
                            "Name": format!("in-window-team-{row}-{team}"),
                            "Description": "a team created while the removal is owed",
                            "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                            "Approvers": approvers(flags),
                            "Policies": [{ "PolicyArn": POLICY_ARN }],
                        }))
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {label:?}: creating team {team} returned {status} with {created:?}"
                    );
                    if flags.contains(&true) {
                        using_team = Some(text(&created, "Arn"));
                    }
                }
            }
            Reference::Requested => {
                let team_arn = existing.clone().expect("the pre-existing team");
                let (status, updated) = harness
                    .update(
                        &team_arn,
                        &json!({ "Approvers": approvers(&[false, true]) }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the update returned {status} with {updated:?}"
                );
                // The change stays waiting on the team's approvers for the rest of the
                // row, so the reference really sits in the REQUESTED set only.
                postpone_approval(&harness, &team_arn);
                let (status, read) = harness.get(&team_arn).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: reading the team back returned {status} with {read:?}"
                );
                assert!(
                    !reported_sources(&read, None).contains(&target),
                    "case {label:?}: the live approver set already names the source: {read:?}"
                );
                assert!(
                    reported_sources(&read, Some("PendingUpdate")).contains(&target),
                    "case {label:?}: the requested approver set does not name the \
                     source: {read:?}"
                );
                using_team = Some(team_arn);
            }
        }

        // The removal comes due: whether it is carried out is read off the store as it
        // stands now, not off the request that scheduled it.
        make_removal_due(&harness, &target);
        let listed = listed_status(&harness, &target).await;
        let (detail_status, detail) = get_source(&harness, &target).await;

        if case.removed {
            assert_eq!(
                listed, None,
                "case {label:?}: the source no team names is still listed as {listed:?}"
            );
            assert_eq!(
                stored(&harness, &target),
                None,
                "case {label:?}: the store still holds the source no team names"
            );
            assert_eq!(
                detail_status, 404,
                "case {label:?}: expected HTTP 404 for the removed source, got \
                 {detail_status} with {detail:?}"
            );
            assert_eq!(
                error_code(&detail),
                "ResourceNotFoundException",
                "case {label:?}: expected error code ResourceNotFoundException, got {detail:?}"
            );
            // A removed source is no longer in use by anybody, so the idempotent delete
            // of it is the success an absent source earns.
            let (status, body) = delete_source(&harness, &target).await;
            assert_eq!(
                status, 200,
                "case {label:?}: deleting the absent source returned {status} with {body:?}"
            );
        } else {
            let team_arn = using_team.clone().expect("the team naming the source");
            assert_eq!(
                listed.as_deref(),
                Some(deleting),
                "case {label:?}: expected the in-use source still reported {deleting:?}, \
                 got {listed:?}"
            );
            let after = stored(&harness, &target);
            assert_eq!(
                after.map(|(status, _)| status),
                Some(deleting),
                "case {label:?}: the in-use source's stored record is {after:?}"
            );
            assert!(
                after.and_then(|(_, due)| due).is_some(),
                "case {label:?}: the in-use source's removal is owed at no instant: {after:?}"
            );
            assert_eq!(
                detail_status, 200,
                "case {label:?}: expected HTTP 200 for the in-use source, got \
                 {detail_status} with {detail:?}"
            );
            assert_eq!(
                text(&detail, "Status"),
                deleting,
                "case {label:?}: GetIdentitySource reports {detail:?}"
            );
            // The record is still there, so the refusal the in-use rule exists for is
            // reachable for it — and names the source asked for and the team the
            // reference sits on.
            let (status, refused) = delete_source(&harness, &target).await;
            assert_eq!(
                status, 409,
                "case {label:?}: expected HTTP 409 for the in-use source, got {status} \
                 with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {label:?}: expected error code ConflictException, got {refused:?}"
            );
            let message = text(&refused, "Message");
            assert!(
                message.contains(&target) && message.contains(&team_arn),
                "case {label:?}: the refusal names neither the source nor the team: \
                 {message:?}"
            );
        }

        // Whatever the row, every identity source an approver of a stored team names is
        // one this store still holds.
        let (status, teams) = harness.list("").await;
        assert_eq!(
            status, 200,
            "case {label:?}: listing the teams returned {status} with {teams:?}"
        );
        let team_arns: Vec<String> = teams["ApprovalTeams"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|team| text(team, "Arn"))
            .collect();
        for team_arn in team_arns {
            let (status, read) = harness.get(&team_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: reading team {team_arn:?} returned {status} with {read:?}"
            );
            let mut named = reported_sources(&read, None);
            if read.get("PendingUpdate").is_some_and(|value| !value.is_null()) {
                named.extend(reported_sources(&read, Some("PendingUpdate")));
            }
            for source_arn in named {
                assert!(
                    listed_status(&harness, &source_arn).await.is_some(),
                    "case {label:?}: team {team_arn:?} names {source_arn:?}, which this \
                     store no longer holds"
                );
            }
        }
        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {label:?}: the row took the bystander identity source with it"
        );
    }
}

/// The same rule end-to-end on the REAL clock, with nothing arranged in the store: a team
/// created inside the span `DeleteIdentitySource` schedules the removal over keeps the
/// source — the owed removal is held, not cancelled — and the removal is carried out at
/// the first instant no stored team names the source any more, which here is the
/// activation of a change that drops the approver holding the reference.
#[tokio::test]
async fn a_reference_recorded_inside_the_real_delay_window_holds_the_removal() {
    let harness = Harness::new();
    let bystander = create_source(&harness, 1, 0).await;
    let target = create_source(&harness, 2, 0).await;

    // Nothing names the source, so the delete is accepted: `DELETING`, with the removal
    // owed one `IDENTITY_SOURCE_DELETION_DELAY` from now.
    let (status, body) = delete_source(&harness, &target).await;
    assert_eq!(status, 200, "the delete returned {status} with {body:?}");

    // Inside that span, a team is created whose approvers authenticate through the
    // source — the reference the accepted delete could not have seen.
    let (status, created) = harness
        .create(&json!({
            "Name": "held-removal-team",
            "Description": "a team created inside the deletion window",
            "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
            "Approvers": [
                approver_member(0, &bystander),
                approver_member(1, &target),
            ],
            "Policies": [{ "PolicyArn": POLICY_ARN }],
        }))
        .await;
    assert_eq!(status, 200, "creating the team returned {created:?}");
    let team_arn = text(&created, "Arn");
    let deleting = state::IdentitySourceStatus::Deleting.as_wire();
    assert_eq!(
        stored(&harness, &target).map(|(status, _)| status),
        Some(deleting),
        "the team was not created while the removal was still owed"
    );

    // Past the span: the source a stored team's approvers name is not removed.
    tokio::time::sleep(state::IDENTITY_SOURCE_DELETION_DELAY + CLOCK_SLACK).await;
    assert_eq!(
        listed_status(&harness, &target).await.as_deref(),
        Some(deleting),
        "the source the team's approvers name is no longer reported {deleting:?}"
    );
    let (status, refused) = delete_source(&harness, &target).await;
    assert_eq!(
        status, 409,
        "expected HTTP 409 for the in-use source, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "expected error code ConflictException, got {refused:?}"
    );

    // The team is ACTIVE once the invitations the clock owes are answered, which is what
    // lets a change be put to its approvers.
    tokio::time::sleep(state::INVITATION_RESPONSE_DELAY + CLOCK_SLACK).await;
    let (status, read) = harness.get(&team_arn).await;
    assert_eq!(
        text(&read, "Status"),
        "ACTIVE",
        "expected an ACTIVE team before the change, got {status} with {read:?}"
    );

    // The change DROPS the approver on the source. While it waits on the team's
    // approvers the live set still names the source, so the owed removal is still held.
    let (status, updated) = harness
        .update(
            &team_arn,
            &json!({ "Approvers": [approver_member(0, &bystander)] }),
        )
        .await;
    assert_eq!(status, 200, "the update returned {status} with {updated:?}");
    assert_eq!(
        listed_status(&harness, &target).await.as_deref(),
        Some(deleting),
        "the live approver set still names the source, so it is not removed yet"
    );

    // Its approval and then its activation are both clock-owed: once the change is the
    // live configuration no approver set of any stored team names the source, so the
    // removal that was owed all along is carried out.
    tokio::time::sleep(
        state::UPDATE_APPROVAL_DELAY + state::UPDATE_ACTIVATION_DELAY + CLOCK_SLACK,
    )
    .await;
    let (status, read) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "reading the activated team returned {read:?}");
    assert_eq!(
        reported_sources(&read, None),
        vec![bystander.clone()],
        "the activated team's approvers are not the ones the change asked for: {read:?}"
    );
    assert_eq!(
        listed_status(&harness, &target).await,
        None,
        "the source no stored team names any more is still listed"
    );
    assert_eq!(
        stored(&harness, &target),
        None,
        "the store still holds the source no stored team names any more"
    );
    let (status, detail) = get_source(&harness, &target).await;
    assert_eq!(
        status, 404,
        "expected HTTP 404 for the removed source, got {status} with {detail:?}"
    );

    // And the invariant the rule protects, over the whole run: every source the team's
    // approvers name is one this store still holds.
    for source_arn in reported_sources(&read, None) {
        assert!(
            listed_status(&harness, &source_arn).await.is_some(),
            "the team's approvers name {source_arn:?}, which this store no longer holds"
        );
    }
}
