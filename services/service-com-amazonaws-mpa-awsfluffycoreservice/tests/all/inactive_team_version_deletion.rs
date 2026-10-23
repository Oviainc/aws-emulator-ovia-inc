// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteInactiveApprovalTeamVersion`.
//!
//! The rule under test: the call removes the approval-team VERSION its
//! `(Arn, VersionId)` pair names when that version has never been activated (the team
//! DRAFT: `PENDING` / `PENDING_ACTIVATION`) or is no longer active (`INACTIVE`); a
//! version whose team is `ACTIVE`, or already inside the pending deletion window
//! `StartActiveApprovalTeamDeletion` opens (`DELETING`), is refused with
//! `ConflictException` (409); and a pair that names nothing stored — an unknown team
//! ARN, or a `VersionId` other than the stored team's current one — is a success that
//! changes nothing, the operation being `@idempotent`.
//!
//! The table below spans that class along the two things the rule keys on: the stored
//! version's status (all four `ApprovalTeamStatus` tokens this service reaches) and the
//! `VersionId` asked for (the team's own, a superseded one, one no team carries, a
//! non-numeric one, and one at `VersionId`'s modelled `length(max: 1000)` bound).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the `DELETE /approval-teams/{Arn}/{VersionId}`
//! route, both URI labels, the error envelope and the HTTP status are exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// `VersionId` is `length(max: 1000)` on both this operation's input and
/// `UpdateApprovalTeam`'s output (`smithy_query(query="operation",
/// operation="DeleteInactiveApprovalTeamVersion")`), so a request may legally carry an id
/// of exactly this many characters.
const MAX_VERSION_ID: usize = 1000;

/// A create body the modelled constraints accept: one approver, one policy, `MofN` with
/// M = 1. What this suite varies is the stored version's STATUS and the `VersionId`
/// asked for, so one valid body serves every row.
fn create_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "inactive version deletion",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                engine::emulator_account_id()
            ),
        }],
        "Policies": [{
            "PolicyArn": "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT"
        }],
    })
}

/// `DELETE /approval-teams/{Arn}/{VersionId}` — both labels are non-greedy, so both are
/// percent-encoded into one path segment each.
async fn delete_version(harness: &Harness, arn: &str, version_id: &str) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &format!(
                "/approval-teams/{}/{}",
                encode_label(arn),
                encode_label(version_id)
            ),
            None,
        )
        .await
}

/// Read-write access to one stored team, for the two arrangements no operation of this
/// model reaches (below).
fn with_team<R>(
    harness: &Harness,
    arn: &str,
    edit: impl FnOnce(&mut state::ApprovalTeam) -> R,
) -> R {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let team = guard.team_mut(arn).expect("a stored team to arrange");
    edit(team)
}

/// Move every approver's invitation response DUE INSTANT, so the team's activation is
/// owed either far in the future (`due_in`, keeping the draft `PENDING` for the length of
/// a test without sleeping) or already (a past instant, so the next call materialises the
/// activation). Only the due instant is moved; the transition itself is the service's,
/// exactly as `approval_team_lifecycle.rs` moves a deletion window's end.
fn move_invitation_responses(harness: &Harness, arn: &str, due_in: Duration, future: bool) {
    with_team(harness, arn, |team| {
        for approver in &mut team.approvers {
            approver.invitation_response_due = Some(if future {
                SystemTime::now() + due_in
            } else {
                SystemTime::now() - due_in
            });
        }
    });
}

/// The stored team's `(Status, VersionId)`, straight out of the store.
fn stored_status(harness: &Harness, arn: &str) -> Option<(&'static str, String)> {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard
        .team(arn)
        .map(|team| (team.status.as_wire(), team.version_id.clone()))
}

/// Whether `ListApprovalTeams` reports a team under `arn`.
async fn listed(harness: &Harness, arn: &str) -> bool {
    let (status, body) = harness.list("").await;
    assert_eq!(status, 200, "list returned {body:?}");
    body.get("ApprovalTeams")
        .and_then(Value::as_array)
        .is_some_and(|teams| teams.iter().any(|team| text(team, "Arn") == arn))
}

/// What the store holds when the delete is issued.
#[derive(Clone, Copy, Debug)]
enum Stored {
    /// A version that has never been activated: `PENDING` with `PENDING_ACTIVATION`,
    /// the state `CreateApprovalTeam` leaves a team in until its approvers answer.
    PendingDraft,
    /// A version that is no longer active. No operation this model declares moves a team
    /// to `INACTIVE` (a failed activation does, which this emulator does not produce), so
    /// the status is recorded directly — the delete path under test is the service's.
    Inactive,
    /// An activated team.
    Active,
    /// A team inside the pending deletion window `StartActiveApprovalTeamDeletion` opens.
    Deleting,
}

/// Which `VersionId` the request names.
#[derive(Clone, Debug)]
enum Version {
    /// The version the stored team currently carries.
    Current,
    /// A version the team has moved past, because an update recorded a newer one.
    Superseded,
    /// An id of the caller's own choosing that no stored version carries.
    Other(String),
}

/// One row: what is stored, which version is asked for, and what the service must answer.
struct Case {
    case: &'static str,
    stored: Stored,
    version: Version,
    expected_status: u16,
    /// Whether the call must have removed the team's only version, and with it the team.
    removed: bool,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a draft version, named by its own id",
            stored: Stored::PendingDraft,
            version: Version::Current,
            expected_status: 200,
            removed: true,
        },
        Case {
            case: "an inactive version, named by its own id",
            stored: Stored::Inactive,
            version: Version::Current,
            expected_status: 200,
            removed: true,
        },
        Case {
            case: "an active team's version",
            stored: Stored::Active,
            version: Version::Current,
            expected_status: 409,
            removed: false,
        },
        Case {
            case: "a team already inside its pending deletion window",
            stored: Stored::Deleting,
            version: Version::Current,
            expected_status: 409,
            removed: false,
        },
        Case {
            case: "a draft team, named by a superseded version id",
            stored: Stored::PendingDraft,
            version: Version::Superseded,
            expected_status: 200,
            removed: false,
        },
        Case {
            case: "an active team, named by a superseded version id",
            stored: Stored::Active,
            version: Version::Superseded,
            expected_status: 200,
            removed: false,
        },
        Case {
            case: "a draft team, named by a version id no team carries",
            stored: Stored::PendingDraft,
            version: Version::Other("4294967296".to_string()),
            expected_status: 200,
            removed: false,
        },
        Case {
            case: "a draft team, named by a non-numeric version id",
            stored: Stored::PendingDraft,
            version: Version::Other("DRAFT".to_string()),
            expected_status: 200,
            removed: false,
        },
        Case {
            case: "a draft team, named by a version id at the modelled length bound",
            stored: Stored::PendingDraft,
            version: Version::Other("9".repeat(MAX_VERSION_ID)),
            expected_status: 200,
            removed: false,
        },
    ]
}

/// A stored team arranged for one row.
struct Arranged {
    arn: String,
    /// The version the stored team carries when the delete is issued.
    current_version: String,
    /// The `VersionId` the request names.
    asked_for: String,
}

async fn arrange(harness: &Harness, name: &str, case: &Case) -> Arranged {
    let (status, created) = harness.create(&create_body(name)).await;
    assert_eq!(status, 200, "case {:?}: create returned {created:?}", case.case);
    let arn = text(&created, "Arn");
    let first_version = text(&created, "VersionId");
    assert!(
        !first_version.is_empty(),
        "case {:?}: create returned no VersionId: {created:?}",
        case.case
    );

    // A superseded id needs a newer version to supersede it, so the update comes first:
    // `UpdateApprovalTeam` records a new version and the team moves past the old one.
    let (current_version, asked_for) = match &case.version {
        Version::Current => (first_version.clone(), first_version),
        Version::Other(id) => (first_version, id.clone()),
        Version::Superseded => {
            let (status, updated) = harness
                .update(&arn, &json!({ "Description": "a newer version" }))
                .await;
            assert_eq!(status, 200, "case {:?}: update returned {updated:?}", case.case);
            let newer = text(&updated, "VersionId");
            assert_ne!(
                newer, first_version,
                "case {:?}: the update did not record a newer VersionId",
                case.case
            );
            (newer, first_version)
        }
    };

    match case.stored {
        Stored::PendingDraft => {
            // Hold the approvers' responses well past the end of this test, so the draft
            // is still PENDING when the delete is issued.
            move_invitation_responses(harness, &arn, Duration::from_secs(3600), true);
        }
        Stored::Inactive => {
            with_team(harness, &arn, |team| {
                team.status = state::TeamStatus::Inactive;
                team.status_code = None;
            });
        }
        Stored::Active | Stored::Deleting => {
            move_invitation_responses(harness, &arn, Duration::from_secs(1), false);
            let (status, team) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {:?}: get returned {team:?}", case.case);
            assert_eq!(
                text(&team, "Status"),
                state::TeamStatus::Active.as_wire(),
                "case {:?}: the team did not activate: {team:?}",
                case.case
            );
            if matches!(case.stored, Stored::Deleting) {
                let (status, started) = harness.start_deletion(&arn, None).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: start deletion returned {started:?}",
                    case.case
                );
            }
        }
    }

    Arranged {
        arn,
        current_version,
        asked_for,
    }
}

/// The rule, over the whole table.
#[tokio::test]
async fn a_version_is_deleted_only_when_it_is_a_draft_or_inactive() {
    for case in cases() {
        let label = case.case;
        let harness = Harness::new();
        let Arranged {
            arn,
            current_version,
            asked_for,
        } = arrange(&harness, "version-delete", &case).await;

        let status_before = stored_status(&harness, &arn);
        let (status, body) = delete_version(&harness, &arn, &asked_for).await;
        assert_eq!(
            status, case.expected_status,
            "case {label:?}: expected HTTP {}, got {status} with {body:?}",
            case.expected_status
        );
        if case.expected_status != 200 {
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {label:?}: expected error code ConflictException, got {body:?}"
            );
            let message = text(&body, "Message");
            assert!(
                message.contains(&arn) && message.contains(&asked_for),
                "case {label:?}: the refusal names neither the team nor the version: \
                 {message:?}"
            );
        }

        let after = stored_status(&harness, &arn);
        let (get_status, team) = harness.get(&arn).await;
        if case.removed {
            assert_eq!(
                after, None,
                "case {label:?}: the store still holds the deleted version"
            );
            assert_eq!(
                get_status, 404,
                "case {label:?}: get still reports the deleted team: {team:?}"
            );
            assert_eq!(
                error_code(&team),
                "ResourceNotFoundException",
                "case {label:?}: expected ResourceNotFoundException, got {team:?}"
            );
            assert!(
                !listed(&harness, &arn).await,
                "case {label:?}: list still reports the deleted team"
            );
            // Repeating the call is a success that changes nothing: the version the
            // caller asks to be rid of is already gone.
            let (repeat, repeated) = delete_version(&harness, &arn, &asked_for).await;
            assert_eq!(
                repeat, 200,
                "case {label:?}: the repeated delete returned {repeat} with {repeated:?}"
            );
            // The removal freed the team's name, so the name is available again.
            let (recreated, body) = harness.create(&create_body("version-delete")).await;
            assert_eq!(
                recreated, 200,
                "case {label:?}: the deleted team's name is still taken: {body:?}"
            );
            continue;
        }

        assert_eq!(
            after, status_before,
            "case {label:?}: the stored team changed under a call that must change nothing"
        );
        assert_eq!(
            get_status, 200,
            "case {label:?}: get no longer reports the team: {team:?}"
        );
        assert_eq!(
            text(&team, "VersionId"),
            current_version,
            "case {label:?}: the team's VersionId moved: {team:?}"
        );
        assert!(
            listed(&harness, &arn).await,
            "case {label:?}: list no longer reports the team"
        );
    }
}

/// An ARN in the caller's own account under which NOTHING is stored: the version is
/// already absent, so the idempotent delete succeeds and writes nothing — whatever the
/// ARN and the version id look like.
#[tokio::test]
async fn deleting_a_version_that_was_never_stored_is_a_success_that_changes_nothing() {
    let harness = Harness::new();

    // A team of the caller's, so a no-op delete can be shown to leave the store alone.
    let (status, created) = harness.create(&create_body("bystander")).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let bystander = text(&created, "Arn");

    // (case, team name in the ARN, version id)
    let rows: [(&str, String, String); 5] = [
        ("a team name never created", "never-created".to_string(), "1".to_string()),
        (
            // `ApprovalTeamName` is `length(max: 64)`, pattern `^[a-zA-Z0-9._-]+$`.
            "a team name at the modelled length bound",
            "n".repeat(64),
            "1".to_string(),
        ),
        ("a name differing by case from a stored team", "Bystander".to_string(), "1".to_string()),
        (
            "a version id at the modelled length bound",
            "never-created".to_string(),
            "9".repeat(MAX_VERSION_ID),
        ),
        ("a non-numeric version id", "never-created".to_string(), "DRAFT".to_string()),
    ];

    for (case, name, version_id) in rows {
        let arn = format!(
            "arn:aws:mpa:{REGION}:{}:approval-team/{name}",
            engine::emulator_account_id()
        );
        let (status, body) = delete_version(&harness, &arn, &version_id).await;
        assert_eq!(status, 200, "case {case:?}: expected HTTP 200, got {status} with {body:?}");
        assert_eq!(
            stored_status(&harness, &arn),
            None,
            "case {case:?}: the call recorded a team under an ARN that held none"
        );
        let (status, team) = harness.get(&bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call removed an unrelated team: {team:?}"
        );
    }

    let store = state::service_state(&*harness.engine).expect("the service store");
    let stored = store.lock().approval_teams.len();
    assert_eq!(stored, 1, "the store holds {stored} team(s), expected only the bystander");
}

/// The account boundary is decided before the version is resolved: an ARN outside the
/// caller's account is `AccessDeniedException` (403), not the no-op success an absent
/// version earns.
#[tokio::test]
async fn a_version_outside_the_callers_account_is_denied_before_resolution() {
    let harness = Harness::new();

    let (status, created) = harness.create(&create_body("owned-team")).await;
    assert_eq!(status, 200, "create returned {created:?}");
    let owned = text(&created, "Arn");
    let owned_version = text(&created, "VersionId");

    // (case, account, team name)
    let rows: [(&str, &str, &str); 3] = [
        ("another account", "999999999999", "probe-team"),
        ("an account one digit from the caller's", "000000000001", "probe-team"),
        // The denial is decided before the version is resolved, so an ARN whose team
        // name the caller DOES own is refused just the same.
        ("another account, a team name the caller owns", "999999999999", "owned-team"),
    ];

    for (case, account, name) in rows {
        let arn = format!("arn:aws:mpa:{REGION}:{account}:approval-team/{name}");
        let (status, body) = delete_version(&harness, &arn, &owned_version).await;
        assert_eq!(status, 403, "case {case:?}: expected HTTP 403, got {status} with {body:?}");
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected error code AccessDeniedException, got {body:?}"
        );
    }

    // A refused request is a refusal: the caller's own team is exactly as it was.
    let (status, team) = harness.get(&owned).await;
    assert_eq!(status, 200, "the caller's own team is gone: {team:?}");
    assert_eq!(
        text(&team, "VersionId"),
        owned_version,
        "a denied cross-account delete changed the caller's own team: {team:?}"
    );
}
