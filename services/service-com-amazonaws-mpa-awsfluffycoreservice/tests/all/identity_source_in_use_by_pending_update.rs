// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IN-USE check `DeleteIdentitySource` makes, over BOTH approver
//! sets a stored approval team holds.
//!
//! The rule under test: an identity source that ANY approver record a stored approval team
//! holds authenticates through is in use, so the delete is refused with the modelled
//! `ConflictException` (409) and the source stays exactly as it was. A team holds two
//! approver sets and a reference in either one is a reference this store holds: the team's
//! LIVE approvers, and the approvers of the change the team has been asked to approve,
//! which `GetApprovalTeam` reports member-for-member in `PendingUpdate$Approvers` and
//! which activation installs as the live set. Once no set of any stored team references
//! the source, the same call deletes it.
//!
//! The table spans the class the rule generalises over: WHICH of the two sets holds the
//! reference (the live one only, the requested one only, both, neither), where in the
//! requested set it sits (first, middle, last), how many approvers each set has, and
//! whether the change names `Approvers` at all. The expectation of every row is read off
//! what `GetApprovalTeam` reports for the team, so a row cannot assert a refusal the
//! service's own report of the team does not call for.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the error envelope
//! and the HTTP status are all exercised.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, text, Harness};
use crate::identity_source_deletion::{create_source, delete_source, listed_status, stored};
use crate::pending_team_update::postpone_approval;

/// Slack over a clock-owed span before the transition it owes is expected to have been
/// materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// A policy ARN the seeded catalog holds, in the form `PolicyReference$PolicyArn`'s
/// modelled pattern requires.
pub(crate) const POLICY_ARN: &str = "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT";

/// One approver of the team: identity `index`, authenticated by `source_arn`.
pub(crate) fn approver_member(index: usize, source_arn: &str) -> Value {
    json!({
        "PrimaryIdentityId": format!("identity-{index:04}"),
        "PrimaryIdentitySourceArn": source_arn,
    })
}

/// The `PrimaryIdentitySourceArn` of every approver a team read reports, either off the
/// team itself (`member` is `None`) or off the change it has been asked to approve
/// (`Some("PendingUpdate")`).
pub(crate) fn reported_sources(body: &Value, member: Option<&str>) -> Vec<String> {
    let approvers = match member {
        None => &body["Approvers"],
        Some(member) => &body[member]["Approvers"],
    };
    approvers
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|approver| text(approver, "PrimaryIdentitySourceArn"))
        .collect()
}

/// One row of the in-use table.
struct Case {
    case: &'static str,
    /// One entry per approver the team is CREATED with: `true` names the source under
    /// test, `false` the bystander source.
    live: Vec<bool>,
    /// One entry per approver the requested change names, as `(identity index, names the
    /// source under test)`. An entry whose index is one of the live approvers' KEEPS that
    /// approver; any other index ADDS one. `None` names no `Approvers` at all, so the
    /// change carries the set the team already has.
    requested: Option<Vec<(usize, bool)>>,
    /// Whether the delete of the source under test must be refused.
    refused: bool,
}

/// Rows differ along which approver set holds the reference, where in that set it sits,
/// the size of each set, and whether the change names `Approvers`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "the change ADDS the only approver on the source",
            live: vec![false],
            requested: Some(vec![(0, false), (1, true)]),
            refused: true,
        },
        Case {
            case: "the added approver is the FIRST of three in the requested set",
            live: vec![false, false],
            requested: Some(vec![(2, true), (0, false), (1, false)]),
            refused: true,
        },
        Case {
            case: "the added approver is the LAST of three in the requested set",
            live: vec![false, false],
            requested: Some(vec![(0, false), (1, false), (2, true)]),
            refused: true,
        },
        Case {
            case: "the change REPLACES the approver on the source, so only the live set names it",
            live: vec![true],
            requested: Some(vec![(1, false)]),
            refused: true,
        },
        Case {
            case: "both sets name it: the change keeps the live approver on the source",
            live: vec![true, false],
            requested: Some(vec![(0, true), (2, false)]),
            refused: true,
        },
        Case {
            case: "the change names approvers, none of them on the source",
            live: vec![false],
            requested: Some(vec![(0, false), (1, false)]),
            refused: false,
        },
        Case {
            case: "the change names no Approvers and no live approver is on the source",
            live: vec![false, false],
            requested: None,
            refused: false,
        },
        Case {
            case: "the change names no Approvers and a live approver is on the source",
            live: vec![true],
            requested: None,
            refused: true,
        },
    ]
}

/// The rule, over the whole table: the delete is refused exactly when one of the team's
/// two approver sets names the source, and a refused delete leaves the stored source
/// alone.
#[tokio::test]
async fn an_identity_source_either_approver_set_of_a_team_names_is_refused() {
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

        let live: Vec<Value> = case
            .live
            .iter()
            .enumerate()
            .map(|(index, &on_target)| approver_member(index, &source_of(on_target)))
            .collect();
        let (status, created) = harness
            .create(&json!({
                "Name": format!("in-use-team-{row}"),
                "Description": "a team whose approvers authenticate through the sources",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": live,
                "Policies": [{ "PolicyArn": POLICY_ARN }],
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: creating the team returned {status} with {created:?}"
        );
        let team_arn = text(&created, "Arn");

        // The change is put to the approvers of an ACTIVE team, so the invitations the
        // clock owes are waited out first.
        tokio::time::sleep(state::INVITATION_RESPONSE_DELAY + CLOCK_SLACK).await;
        let (status, read) = harness.get(&team_arn).await;
        assert_eq!(
            text(&read, "Status"),
            "ACTIVE",
            "case {label:?}: expected an ACTIVE team before the change, got {status} with {read:?}"
        );

        let mut patch = json!({ "Description": "the change the team is asked to approve" });
        if let Some(requested) = case.requested.as_ref() {
            patch["Approvers"] = Value::Array(
                requested
                    .iter()
                    .map(|&(index, on_target)| approver_member(index, &source_of(on_target)))
                    .collect(),
            );
        }
        let (status, updated) = harness.update(&team_arn, &patch).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the update returned {status} with {updated:?}"
        );
        // The change stays waiting on the team's approvers for the rest of the row, so
        // the delete below is decided against a team that really holds two sets.
        postpone_approval(&harness, &team_arn);

        // What the service itself reports for the team is what the row's expectation is
        // read off: both sets, exactly as `GetApprovalTeam` serves them.
        let (status, read) = harness.get(&team_arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: reading the team back returned {status} with {read:?}"
        );
        let live_sources = reported_sources(&read, None);
        let pending_sources = reported_sources(&read, Some("PendingUpdate"));
        assert!(
            !pending_sources.is_empty(),
            "case {label:?}: the team read reports no PendingUpdate approvers: {read:?}"
        );
        let named = live_sources.contains(&target) || pending_sources.contains(&target);
        assert_eq!(
            named, case.refused,
            "case {label:?}: the team read names the source under test in live \
             {live_sources:?} / pending {pending_sources:?}, expected named = {}",
            case.refused
        );

        let before = stored(&harness, &target);
        let (status, body) = delete_source(&harness, &target).await;
        if case.refused {
            assert_eq!(
                status, 409,
                "case {label:?}: expected HTTP 409, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {label:?}: expected error code ConflictException, got {body:?}"
            );
            // The refusal names the source asked for and the team the reference sits on,
            // both read out of the request and the store.
            let message = text(&body, "Message");
            assert!(
                message.contains(&target) && message.contains(&team_arn),
                "case {label:?}: the refusal names the source and the team in neither \
                 part: {message:?}"
            );
            assert_eq!(
                stored(&harness, &target),
                before,
                "case {label:?}: the refused delete changed the stored source, \
                 {before:?} became {:?}",
                stored(&harness, &target)
            );
            assert_eq!(
                listed_status(&harness, &target).await.as_deref(),
                before.map(|(status, _)| status),
                "case {label:?}: the refused delete moved the source's reported status"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: expected HTTP 200, got {status} with {body:?}"
            );
            assert_eq!(
                listed_status(&harness, &target).await.as_deref(),
                Some(state::IdentitySourceStatus::Deleting.as_wire()),
                "case {label:?}: the accepted delete did not put the source into DELETING"
            );
        }
        assert!(
            listed_status(&harness, &bystander).await.is_some(),
            "case {label:?}: the call touched the bystander identity source"
        );
    }
}

/// The invariant the refusal protects: activation never installs an approver set naming a
/// source the store no longer holds. While the change waits, the live set still names the
/// source, so the delete is refused; once the change is activated and no set names it any
/// more, the same call deletes it — and every source the activated team's approvers name
/// is still stored.
#[tokio::test]
async fn activation_never_installs_an_approver_naming_a_removed_identity_source() {
    let harness = Harness::new();
    let bystander = create_source(&harness, 1, 0).await;
    let target = create_source(&harness, 2, 0).await;

    let (status, created) = harness
        .create(&json!({
            "Name": "activation-team",
            "Description": "a team one of whose approvers is dropped by the change",
            "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
            "Approvers": [
                approver_member(0, &target),
                approver_member(1, &bystander),
            ],
            "Policies": [{ "PolicyArn": POLICY_ARN }],
        }))
        .await;
    assert_eq!(status, 200, "creating the team returned {created:?}");
    let team_arn = text(&created, "Arn");
    tokio::time::sleep(state::INVITATION_RESPONSE_DELAY + CLOCK_SLACK).await;

    // The change DROPS the approver on the source under test, so the requested set does
    // not name it while the live set still does.
    let (status, updated) = harness
        .update(
            &team_arn,
            &json!({ "Approvers": [approver_member(1, &bystander)] }),
        )
        .await;
    assert_eq!(status, 200, "the update returned {status} with {updated:?}");

    let (status, refused) = delete_source(&harness, &target).await;
    assert_eq!(
        status, 409,
        "the live set still names the source, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "expected error code ConflictException, got {refused:?}"
    );

    // The approval and then the activation of the change are both clock-owed: once they
    // are materialised the requested set is the live one.
    tokio::time::sleep(
        state::UPDATE_APPROVAL_DELAY + state::UPDATE_ACTIVATION_DELAY + CLOCK_SLACK,
    )
    .await;
    let (status, read) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "reading the activated team returned {read:?}");
    assert_eq!(
        text(&read, "Status"),
        "ACTIVE",
        "the team did not become ACTIVE again: {read:?}"
    );
    assert_eq!(
        reported_sources(&read, None),
        vec![bystander.clone()],
        "the activated team's approvers are not the ones the change asked for: {read:?}"
    );
    assert!(
        read.get("PendingUpdate").is_none_or(Value::is_null),
        "the activated team still reports a requested change: {read:?}"
    );

    // No set of any stored team names the source any more, so the same call is accepted.
    let (status, body) = delete_source(&harness, &target).await;
    assert_eq!(
        status, 200,
        "no approver set names the source any more, got {status} with {body:?}"
    );
    tokio::time::sleep(state::IDENTITY_SOURCE_DELETION_DELAY + CLOCK_SLACK).await;
    assert_eq!(
        listed_status(&harness, &target).await,
        None,
        "the deleted source is still listed"
    );
    assert_eq!(
        stored(&harness, &target),
        None,
        "the store still holds the deleted source"
    );

    // The invariant: every identity source an approver of the team names is one this
    // store still holds.
    let (status, read) = harness.get(&team_arn).await;
    assert_eq!(status, 200, "reading the team back returned {read:?}");
    for source_arn in reported_sources(&read, None) {
        assert!(
            listed_status(&harness, &source_arn).await.is_some(),
            "the team's approvers name {source_arn:?}, which this store no longer holds"
        );
    }
}
