// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a `@idempotencyToken` replay of a create ANSWERS WITH.
//!
//! The rule under test: a create a client token identifies as a retry repeats the answer
//! the first call returned — recorded when that call answered — instead of re-rendering
//! one out of the resource as it stands now, so a member a later call moves (a created
//! resource's version id) never drifts between two answers to the same request.
//!
//! The table spans the class the rule generalises over: how far the team's live version
//! has moved on between the create and the replay, and by which path it moved (no update
//! at all; one, two and three changes applied in place to a team no approver has
//! activated; a change an ACTIVE team's approvers approved and the clock then activated),
//! which configuration members the change names, and how many approvers the team has.
//! The create is answered the same way in every row; what differs is the state the
//! replay reads.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::wait_for_the_clock;
use crate::approval_team_lifecycle::{expected_arn, instant, text, Harness, REGION};

/// An approver: `(PrimaryIdentityId, PrimaryIdentitySourceArn)`. `IdentityId` is
/// `length(1..100)` with no pattern; `PrimaryIdentitySourceArn` carries an identity
/// source's ARN.
fn approver(n: usize) -> (String, String) {
    (
        format!("identity-{n:04}"),
        format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ids-{n:012}",
            engine::emulator_account_id()
        ),
    )
}

/// The policy a fresh store seeds, named through `PolicyReference$PolicyArn`'s modelled
/// pattern — an attachment the catalog cannot resolve is refused.
fn policy_arn() -> String {
    "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT".to_string()
}

/// `CreateApprovalTeam`'s body, carrying `ClientToken`.
fn create_body(name: &str, approvers: usize, min_approvals: i64, token: &str) -> Value {
    json!({
        "Name": name,
        "Description": "idempotent create",
        "ClientToken": token,
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": min_approvals } },
        "Approvers": (1..=approvers)
            .map(approver)
            .map(|(id, source)| json!({
                "PrimaryIdentityId": id,
                "PrimaryIdentitySourceArn": source,
            }))
            .collect::<Vec<_>>(),
        "Policies": [{ "PolicyArn": policy_arn() }],
    })
}

/// Push the instant every invited approver's answer comes due well past the end of a
/// test, so the team stays `PENDING` — the state in which `UpdateApprovalTeam` applies a
/// change IN PLACE — for as long as the row needs it to. Only the DUE INSTANTS are moved;
/// every transition stays the service's own, exactly as `approval_team_lifecycle.rs`
/// moves a deletion window's end.
fn postpone_invitation_responses(harness: &Harness, team_arn: &str) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let team = guard.team_mut(team_arn).expect("a stored team to arrange");
    for approver in team.approver_records_mut() {
        if approver.invitation_response_due.is_some() {
            approver.invitation_response_due = Some(SystemTime::now() + Duration::from_secs(3600));
        }
    }
}

/// How the team's live version moves on between the create and the replay of it.
enum Movement {
    /// Nothing updates the team.
    None,
    /// Changes applied IN PLACE to a team no approver has activated yet, one per patch.
    InPlace(Vec<Value>),
    /// A change an ACTIVE team's approvers approved, which the clock then activates.
    ActivatedHandshake(Value),
}

/// One row of the replay table.
struct ReplayCase {
    case: &'static str,
    name: &'static str,
    approvers: usize,
    min_approvals: i64,
    /// `Token` is `length(max: 4096)` with no pattern, so the rows differ in token form
    /// as well: short, punctuated, and the published maximum.
    token: String,
    movement: Movement,
}

fn replay_cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            case: "no update at all",
            name: "replay-untouched",
            approvers: 1,
            min_approvals: 1,
            token: "t-untouched".to_string(),
            movement: Movement::None,
        },
        ReplayCase {
            case: "one description change applied in place",
            name: "replay-one-change",
            approvers: 2,
            min_approvals: 1,
            token: "a-much-longer-client-token-value".to_string(),
            movement: Movement::InPlace(vec![json!({ "Description": "changed" })]),
        },
        ReplayCase {
            case: "a description and a strategy change applied in place",
            name: "Replay.Two_Changes",
            approvers: 3,
            min_approvals: 3,
            token: "t-two".to_string(),
            movement: Movement::InPlace(vec![
                json!({ "Description": "changed once" }),
                json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 2 } } }),
            ]),
        },
        ReplayCase {
            case: "three changes applied in place, the last one shrinking the approvers",
            name: "r",
            // `Approvers` is `length(1..20)` — the maximum.
            approvers: 20,
            min_approvals: 20,
            // `Token` is `length(max: 4096)` with no pattern — the maximum.
            token: "z".repeat(4096),
            movement: Movement::InPlace(vec![
                json!({ "Description": "first" }),
                json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } } }),
                json!({
                    "Approvers": [{
                        "PrimaryIdentityId": "identity-0001",
                        "PrimaryIdentitySourceArn": format!(
                            "arn:aws:mpa:{REGION}:{}:identity-source/ids-{:012}",
                            engine::emulator_account_id(),
                            1
                        ),
                    }],
                }),
            ]),
        },
        ReplayCase {
            case: "a change the team approved and the clock activated",
            name: "replay-activated",
            approvers: 1,
            min_approvals: 1,
            token: "t-activated".to_string(),
            movement: Movement::ActivatedHandshake(json!({ "Description": "approved change" })),
        },
    ]
}

/// A replayed `ClientToken` answers with the ANSWER the first create gave — its `Arn`,
/// `Name`, `CreationTime` and the `VersionId` that call returned — however far the team's
/// live version has moved on since, and mints no second team.
#[tokio::test]
async fn a_client_token_replay_answers_with_the_first_call_s_answer() {
    for case in replay_cases() {
        let label = case.case;
        let harness = Harness::new();
        let body = create_body(case.name, case.approvers, case.min_approvals, &case.token);

        let (status, first) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: create returned {first:?}");
        let arn = text(&first, "Arn");
        assert_eq!(
            arn,
            expected_arn(case.name),
            "case {label}: create minted the wrong Arn, got {first:?}"
        );
        let first_version = text(&first, "VersionId");
        assert!(
            !first_version.is_empty(),
            "case {label}: create returned no VersionId: {first:?}"
        );

        // Move the team's live version on, by the path this row names.
        match &case.movement {
            Movement::None => {
                postpone_invitation_responses(&harness, &arn);
            }
            Movement::InPlace(patches) => {
                postpone_invitation_responses(&harness, &arn);
                for (nth, patch) in patches.iter().enumerate() {
                    let (status, updated) = harness.update(&arn, patch).await;
                    assert_eq!(
                        status, 200,
                        "case {label}: in-place update {nth} returned {updated:?}"
                    );
                }
            }
            Movement::ActivatedHandshake(patch) => {
                // The team is ACTIVE once its approvers have answered their invitations;
                // only then is a configuration change put to them rather than applied.
                wait_for_the_clock(state::INVITATION_RESPONSE_DELAY);
                let (status, team) = harness.get(&arn).await;
                assert_eq!(status, 200, "case {label}: get returned {team:?}");
                assert_eq!(
                    text(&team, "Status"),
                    state::TeamStatus::Active.as_wire(),
                    "case {label}: expected an ACTIVE team before the handshake, got {team:?}"
                );
                let (status, updated) = harness.update(&arn, patch).await;
                assert_eq!(status, 200, "case {label}: update returned {updated:?}");
                // The approval the clock owes, and then the activation that approval owes.
                wait_for_the_clock(state::UPDATE_APPROVAL_DELAY + state::UPDATE_ACTIVATION_DELAY);
                let (status, team) = harness.get(&arn).await;
                assert_eq!(status, 200, "case {label}: get returned {team:?}");
                assert_eq!(
                    text(&team, "Status"),
                    state::TeamStatus::Active.as_wire(),
                    "case {label}: the activated team must be ACTIVE again, got {team:?}"
                );
            }
        }

        // What the team carries NOW, read through the operation that reports it.
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        let live_version = text(&team, "VersionId");
        let moved = !matches!(case.movement, Movement::None);
        if moved {
            assert_ne!(
                live_version, first_version,
                "case {label}: the row must move the team's live VersionId past the one \
                 create returned"
            );
        } else {
            assert_eq!(
                live_version, first_version,
                "case {label}: an untouched team keeps the VersionId create returned"
            );
        }

        // The replay: the same request, byte for byte, with the same token.
        let (status, replay) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: the replay returned {replay:?}");
        assert_eq!(
            text(&replay, "VersionId"),
            first_version,
            "case {label}: the replay must answer with the VersionId the first create \
             returned, not the team's live {live_version:?}: got {replay:?}"
        );
        assert_eq!(
            text(&replay, "Arn"),
            arn,
            "case {label}: the replay must answer with the first call's Arn, got {replay:?}"
        );
        assert_eq!(
            text(&replay, "Name"),
            case.name,
            "case {label}: the replay must answer with the first call's Name, got {replay:?}"
        );
        assert_eq!(
            instant(&replay, "CreationTime"),
            instant(&first, "CreationTime"),
            "case {label}: the replay must answer with the first call's CreationTime, got \
             {replay:?}"
        );

        // Idempotent: a further replay answers exactly the same, and no replay minted a
        // second team.
        let (status, again) = harness.create(&body).await;
        assert_eq!(status, 200, "case {label}: the second replay returned {again:?}");
        assert_eq!(
            again, replay,
            "case {label}: two replays of one request must answer identically"
        );
        let (status, listed) = harness.list("MaxResults=20").await;
        assert_eq!(status, 200, "case {label}: list returned {listed:?}");
        let names: Vec<String> = listed["ApprovalTeams"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|team| text(team, "Name"))
            .collect();
        assert_eq!(
            names,
            vec![case.name.to_string()],
            "case {label}: a replay must mint no second team, got {listed:?}"
        );

        // The team itself is untouched by the replay: it still reports the version it had
        // moved to, so answering the retry changed nothing a later caller reads.
        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        assert_eq!(
            text(&team, "VersionId"),
            live_version,
            "case {label}: the replay must not move the team's live VersionId, got {team:?}"
        );
    }
}

/// The same rule for the other `@idempotencyToken` create of this service: a replay of
/// `CreateIdentitySource` repeats the answer the first call gave. Every member that answer
/// carries is fixed when the source is minted, so the answer is stable across the one
/// transition the clock owes a new source (`CREATING` -> `ACTIVE`).
#[tokio::test]
async fn an_identity_source_replay_answers_with_the_first_call_s_answer() {
    // Rows differ along token form (short, punctuated, the published maximum
    // `Token` `length(max: 4096)`), the instance-ARN form `IdcInstanceArn`'s pattern
    // `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$` allows, and the instance's
    // own region.
    for (label, token, instance_arn, instance_region) in [
        (
            "short token",
            "t1".to_string(),
            "arn:aws:sso:::instance/ins-1234567890abcdef",
            REGION,
        ),
        (
            "uuid-shaped token",
            "9f1c7b24-0000-4000-8000-000000000001".to_string(),
            "arn:aws:sso:::instance/ssoins-abcdef1234567890",
            "us-east-1",
        ),
        (
            "maximum-length token",
            "t".repeat(4096),
            "arn:aws-us-gov:sso:::instance/ins-a.b-c.d-12345678",
            "us-gov-west-1",
        ),
    ] {
        let harness = Harness::new();
        let body = json!({
            "ClientToken": token,
            "IdentitySourceParameters": {
                "IamIdentityCenter": {
                    "InstanceArn": instance_arn,
                    "Region": instance_region,
                },
            },
        });
        let (status, first) = harness
            .call("POST", "/identity-sources", Some(body.clone()))
            .await;
        assert_eq!(status, 200, "case {label}: create returned {first:?}");

        // Past the one transition the clock owes a new source, so the replay reads a
        // record that has moved on from the one the first call answered out of.
        wait_for_the_clock(state::IDENTITY_SOURCE_ACTIVATION_DELAY);

        let (status, replay) = harness
            .call("POST", "/identity-sources", Some(body))
            .await;
        assert_eq!(status, 200, "case {label}: the replay returned {replay:?}");
        assert_eq!(
            replay, first,
            "case {label}: the replay must repeat the first call's answer"
        );
    }
}
