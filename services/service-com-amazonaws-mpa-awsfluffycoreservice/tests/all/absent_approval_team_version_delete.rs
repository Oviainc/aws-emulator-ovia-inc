// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an ABSENT approval-team version earns from
//! `DeleteInactiveApprovalTeamVersion`, and for the contrast that tells that answer
//! apart from a resource-lookup failure.
//!
//! The rule under test: a delete addressed by a `(Arn, VersionId)` pair whose target is
//! not stored is a SUCCESS that changes nothing — the operation is `@idempotent`
//! (`smithy_query(query="operation",operation="DeleteInactiveApprovalTeamVersion")`
//! reports `idempotent: true`), so a caller asking to be rid of a version that is
//! already gone is left in exactly the state it asked for — while the ARN-addressed
//! REPORTING operations on that same address answer 404 `ResourceNotFoundException`
//! carrying this service's one not-found message
//! ([`state::RESOURCE_NOT_FOUND_MESSAGE`]). The modelled 404 of this operation is
//! therefore not what an absent target earns: the recorded unit
//! `mpa.delete-inactive-approval-team-version-nonexistent-is-noop` is where the real
//! service was observed answering this very call — a delete against an approval-team ARN
//! nothing had been created under — with success, and failing only on the contrasting
//! `GetApprovalTeam` for that same ARN.
//!
//! The table spans the ways an address can name nothing stored, along both of its
//! labels: the `Arn` (a team name never created, one at `ApprovalTeamName`'s length
//! bound, a punctuated one, one differing from a stored team's only by case) and the
//! `VersionId` (an id no team carries, one the stored team has moved past, a
//! non-numeric one, and one at `VersionId`'s modelled length bound). The sibling suite
//! `inactive_team_version_deletion.rs` spans the stored version's STATUS, which is what
//! decides the 200/409 answers for a version that IS stored; the tail of the first test
//! here is the control showing the same route still computes removal from the store.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the `DELETE /approval-teams/{Arn}/{VersionId}`
//! route, both URI labels, the error envelope and the HTTP status are exercised.

use std::time::Duration;

use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{error_code, expected_arn, text, Harness};
use crate::inactive_team_version_deletion::{
    create_body, delete_version, move_invitation_responses, stored_status, MAX_VERSION_ID,
};

/// `ApprovalTeamName` is `length(max: 64)` with pattern `^[a-zA-Z0-9._-]+$`
/// (`smithy_query(query="shape",shape="ApprovalTeamName")`).
const MAX_TEAM_NAME: usize = 64;

/// The name of the team these tables keep stored, so a row that varies only the
/// `VersionId` is still addressing a team the store holds.
const STORED_TEAM: &str = "absent-draft";

/// How many times an idempotent call is repeated: an absent target must earn the same
/// answer every time, not only the first.
const ATTEMPTS: usize = 3;

/// How many teams the store holds, so a call that must change nothing can be shown not
/// to have recorded or removed one.
fn team_count(harness: &Harness) -> usize {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    guard.approval_teams.len()
}

/// One address that names nothing stored.
struct Absent {
    case: &'static str,
    /// The team name the request's `Arn` carries, or `None` to address the stored team
    /// itself (so only the `VersionId` is absent).
    team: Option<String>,
    version: String,
}

/// The stored team, the version it carries, and the addresses that name nothing.
struct Arranged {
    arn: String,
    current_version: String,
    rows: Vec<Absent>,
}

/// Create the team these tables keep stored, record a newer version for it (so one of
/// its version ids is superseded) and hold its approvers' invitation responses well past
/// the end of the test, so the draft is still there when each row runs.
async fn arrange(harness: &Harness) -> Arranged {
    let (status, created) = harness.create(&create_body(STORED_TEAM)).await;
    assert_eq!(status, 200, "create returned {status} with {created:?}");
    let arn = text(&created, "Arn");
    let first_version = text(&created, "VersionId");
    assert!(
        !first_version.is_empty(),
        "create returned no VersionId: {created:?}"
    );

    let (status, updated) = harness
        .update(&arn, &serde_json::json!({ "Description": "a newer version" }))
        .await;
    assert_eq!(status, 200, "update returned {status} with {updated:?}");
    let current_version = text(&updated, "VersionId");
    assert_ne!(
        current_version, first_version,
        "the update did not record a newer VersionId: {updated:?}"
    );

    move_invitation_responses(harness, &arn, Duration::from_secs(3600), true);

    let rows = vec![
        Absent {
            case: "a team name never created",
            team: Some("never-created".to_string()),
            version: "1".to_string(),
        },
        Absent {
            case: "a team name at ApprovalTeamName's length bound",
            team: Some("n".repeat(MAX_TEAM_NAME)),
            version: "1".to_string(),
        },
        Absent {
            case: "a punctuated team name never created",
            team: Some("a.b_c-d".to_string()),
            version: "1".to_string(),
        },
        Absent {
            case: "a team name differing from a stored team only by case",
            team: Some("Absent-Draft".to_string()),
            version: "1".to_string(),
        },
        Absent {
            case: "the stored team, a numeric version id it does not carry",
            team: None,
            version: "4294967296".to_string(),
        },
        Absent {
            case: "the stored team, the version id it has moved past",
            team: None,
            version: first_version,
        },
        Absent {
            case: "the stored team, a non-numeric version id",
            team: None,
            version: "DRAFT".to_string(),
        },
        Absent {
            case: "the stored team, a version id at VersionId's length bound",
            team: None,
            version: "9".repeat(MAX_VERSION_ID),
        },
    ];

    Arranged {
        arn,
        current_version,
        rows,
    }
}

/// The address the row names, given the stored team's ARN.
fn address(row: &Absent, stored: &str) -> String {
    match &row.team {
        Some(name) => expected_arn(name),
        None => stored.to_string(),
    }
}

/// Rule, the delete side: an address naming nothing stored is answered 200 with an empty
/// body, as many times as it is asked, and the store is exactly as it was.
#[tokio::test]
async fn an_absent_approval_team_version_is_an_idempotent_success_that_changes_nothing() {
    let harness = Harness::new();
    let Arranged {
        arn,
        current_version,
        rows,
    } = arrange(&harness).await;

    for row in &rows {
        let case = row.case;
        let target = address(row, &arn);
        let before_teams = team_count(&harness);
        let before_stored = stored_status(&harness, &arn);

        for attempt in 1..=ATTEMPTS {
            let (status, body) = delete_version(&harness, &target, &row.version).await;
            assert_eq!(
                status, 200,
                "case {case:?}: attempt {attempt} expected HTTP 200, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "",
                "case {case:?}: attempt {attempt} answered an error envelope: {body:?}"
            );
        }

        assert_eq!(
            team_count(&harness),
            before_teams,
            "case {case:?}: the call changed how many teams the store holds, expected \
             {before_teams}"
        );
        assert_eq!(
            stored_status(&harness, &arn),
            before_stored,
            "case {case:?}: the stored team changed under a call that must change \
             nothing, expected {before_stored:?}"
        );
        if row.team.is_some() {
            assert_eq!(
                stored_status(&harness, &target),
                None,
                "case {case:?}: the call recorded a team under an ARN that held none"
            );
        }

        let (status, team) = harness.get(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored team is no longer reported: {team:?}"
        );
        assert_eq!(
            text(&team, "VersionId"),
            current_version,
            "case {case:?}: the stored team's VersionId moved, expected \
             {current_version:?}: {team:?}"
        );
    }

    // The control: the same route is not a constant success. Asked for the version the
    // team IS carrying, it removes it — so the 200s above are decided from the store,
    // not from the request.
    let (status, body) = delete_version(&harness, &arn, &current_version).await;
    assert_eq!(
        status, 200,
        "deleting the stored draft's own version returned {status} with {body:?}"
    );
    assert_eq!(
        stored_status(&harness, &arn),
        None,
        "the store still holds the version that was deleted"
    );
    assert_eq!(
        team_count(&harness),
        0,
        "the removed version left a team behind"
    );
    let (status, team) = harness.get(&arn).await;
    assert_eq!(
        status, 404,
        "the deleted team is still reported: {status} with {team:?}"
    );
    // And now that the version is gone, the very same call is the no-op success above.
    let (status, body) = delete_version(&harness, &arn, &current_version).await;
    assert_eq!(
        status, 200,
        "repeating the delete of a version now gone returned {status} with {body:?}"
    );
}

/// Rule, the contrast: for an ARN this store holds no team under, the idempotent delete
/// succeeds while the ARN-addressed reporting operations answer 404
/// `ResourceNotFoundException` with the service's one not-found message — so a caller
/// learns the resource does not exist from the reads, not from the delete.
#[tokio::test]
async fn absence_is_reported_by_the_arn_addressed_reads_not_by_the_delete() {
    let harness = Harness::new();
    let Arranged { arn, rows, .. } = arrange(&harness).await;

    for row in &rows {
        let case = row.case;
        let target = address(row, &arn);

        let (status, body) = delete_version(&harness, &target, &row.version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the idempotent delete expected HTTP 200, got {status} with \
             {body:?}"
        );

        if row.team.is_none() {
            // The `VersionId` was the absent part, so the TEAM is stored and the reads
            // answer it — which is what makes the 200 above a statement about the
            // version and not about the team. The concrete answers: the read reports the
            // team, and starting its deletion is refused as a conflict, the stored team
            // being PENDING rather than ACTIVE, so neither call can change what the next
            // row sees.
            let before = stored_status(&harness, &arn);
            let (status, team) = harness.get(&target).await;
            assert_eq!(
                status, 200,
                "case {case:?} / GetApprovalTeam: expected HTTP 200, got {status} with \
                 {team:?}"
            );
            let (status, refused) = harness.start_deletion(&target, None).await;
            assert_eq!(
                status, 409,
                "case {case:?} / StartActiveApprovalTeamDeletion: expected HTTP 409, got \
                 {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ConflictException",
                "case {case:?} / StartActiveApprovalTeamDeletion: wrong error code in \
                 {refused:?}"
            );
            assert_eq!(
                stored_status(&harness, &arn),
                before,
                "case {case:?}: the stored team changed under calls that must change \
                 nothing, expected {before:?}"
            );
            continue;
        }

        let answers = [
            ("GetApprovalTeam", harness.get(&target).await),
            (
                "StartActiveApprovalTeamDeletion",
                harness.start_deletion(&target, None).await,
            ),
        ];
        for (operation, (status, body)) in answers {
            assert_eq!(
                status, 404,
                "case {case:?} / {operation}: expected HTTP 404, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} / {operation}: wrong error code in {body:?}"
            );
            // The not-found class is ONE service-wide sentence, naming nothing the
            // request carried.
            assert_eq!(
                text(&body, "Message"),
                state::RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?} / {operation}: wrong not-found message in {body:?}"
            );
        }
    }
}
