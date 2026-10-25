// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpgradeProfileVersion`.
//!
//! The rule these pin, over a TABLE of requests rather than one recorded call: "Upgrade a
//! profile" (`smithy_query(query="operation", operation="UpgradeProfileVersion")`) moves the
//! named workload's association of the named profile to the version that PROFILE is at —
//! the association is `Workload$Profiles`, whose `ProfileVersion` is the version the
//! workload is reviewed against — and records a milestone of the workload as it stood BEFORE
//! that move when the request names one; a workload that is not stored and a `ProfileArn`
//! that names no profile associated with it are each a modelled 404
//! `ResourceNotFoundException` that writes nothing, and one `ClientRequestToken` performs
//! the upgrade once.
//!
//! Every workload, profile and association here is made over HTTP (`CreateWorkload`,
//! `CreateProfile`, `AssociateProfiles`). One half of the starting situation has no HTTP
//! path yet: a profile REVISION, which is what puts a profile ahead of the version an
//! association pinned — associating pins the version the profile is at, and minting a
//! profile's next version belongs to `UpdateProfile`, still a stub with its own issue — so
//! that revision is seeded through the shared state by the harness's
//! `Client::revise_profile_version`, exactly as this crate's suites seed a stored answer
//! whose writer is a stub. Everything asserted crosses the HTTP boundary: requests go
//! through the real `MultiServiceRouter`, so routing, restJson1 (de)serialization, boundary
//! constraint validation and the handler all run as they do in production.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::workload_harness::{
    age_workload, assert_not_found, created_id, created_profile, encode_path_segment, member,
    minimal_create, profiles_of, updated_at_of, Answer, Client,
};

/// The Region the shared harness signs every request for, and so the Region an ARN built
/// here is in.
const REGION: &str = "us-west-2";

/// The modelled `ResourceType` of a workload, of a profile and of a milestone
/// (`crate::state::WORKLOAD_RESOURCE_TYPE` / `PROFILE_RESOURCE_TYPE` /
/// `MILESTONE_RESOURCE_TYPE`), as the wire carries them on a failure about one.
const WORKLOAD_TYPE: &str = "WORKLOAD";
const PROFILE_TYPE: &str = "PROFILE";
const MILESTONE_TYPE: &str = "MILESTONE";

/// A `WorkloadId`-shaped value (`^[0-9a-f]{32}$`) that no stored workload carries, so a
/// refusal it draws is the lookup's and not the boundary validator's.
const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";

/// How far into the past a workload's `UpdatedAt` is moved before the request under test, so
/// a write to it has to ADVANCE the value rather than merely not go backwards.
const AGE: Duration = Duration::from_secs(3_600);

/// The most milestones one workload holds, from the emulator's single source of truth
/// rather than a literal.
const MAX_MILESTONES_PER_WORKLOAD: i32 = service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_MILESTONES_PER_WORKLOAD;

/// `UpgradeProfileVersion`: `PUT /workloads/{WorkloadId}/profiles/{ProfileArn}/upgrade`.
/// The ARN is a non-greedy `@httpLabel`, so it travels percent-encoded in one path segment,
/// exactly as an SDK sends it.
async fn upgrade(client: &Client, workload_id: &str, profile_arn: &str, body: Value) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/workloads/{workload_id}/profiles/{}/upgrade",
                encode_path_segment(profile_arn)
            ),
            Some(body),
        )
        .await
}

/// The `MilestoneNumber`s `ListMilestones` reports for a workload, in the order it reports
/// them.
async fn milestone_numbers(client: &Client, workload_id: &str, case: &str) -> Vec<i64> {
    let answer = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestonesSummaries"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListMilestones 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("MilestoneNumber").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

/// The `Milestone` `GetMilestone` reports for one number of one workload.
async fn stored_milestone(client: &Client, workload_id: &str, number: i64, case: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/{number}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetMilestone 200, got {answer:?}"
    );
    answer.body.get("Milestone").cloned().unwrap_or(Value::Null)
}

/// The `ProfileVersion` `GetProfile` reports the profile itself is at: this operation moves
/// the WORKLOAD's association, so the profile must read back unchanged.
async fn profile_version_of(client: &Client, profile_arn: &str, case: &str) -> String {
    let answer = client.get_profile(profile_arn, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetProfile 200, got {answer:?}"
    );
    answer
        .body
        .get("Profile")
        .and_then(|profile| profile.get("ProfileVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A workload and a profile, both created over HTTP, with the profile associated with the
/// workload over HTTP — which pins the association at the version the profile is at — and
/// the profile then revised to `latest` when the row asks for a revision.
///
/// Answers `(WorkloadId, ProfileArn, the version the association is pinned at, the version
/// the profile is now at)`.
async fn fixture(
    client: &Client,
    name: &str,
    latest: Option<&str>,
) -> (String, String, String, String) {
    let workload_id = created_id(
        &client
            .create(minimal_create(name, &format!("tok-{name}")))
            .await,
        name,
    );
    let (profile_arn, pinned) = created_profile(client, name).await;
    let associated = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateProfiles"),
            Some(json!({ "ProfileArns": [profile_arn] })),
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case {name:?}: expected AssociateProfiles 200, got {associated:?}"
    );
    assert_eq!(
        profiles_of(client, &workload_id, name).await,
        vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": pinned })],
        "case {name:?}: associating pins the association at the version the profile is at"
    );
    if let Some(latest) = latest {
        client.revise_profile_version(&profile_arn, latest);
    }
    let current = latest.unwrap_or(&pinned).to_string();
    assert_eq!(
        profile_version_of(client, &profile_arn, name).await,
        current,
        "case {name:?}: the profile must be at the version the fixture put it at"
    );
    (workload_id, profile_arn, pinned, current)
}

/// One row of the upgrade table: the version the profile has been revised to (`None` leaves
/// it at the version `CreateProfile` minted, so the workload is already current) and the
/// milestone name the request carries.
struct UpgradeCase {
    case: &'static str,
    name: &'static str,
    latest: Option<&'static str>,
    milestone: Option<&'static str>,
}

/// `UpgradeProfileVersion` re-pins the association at the version the profile is at,
/// whatever that version's spelling, and records a milestone of the PRE-upgrade workload
/// exactly when the request names one. A workload already at that version is served without
/// a write.
#[tokio::test]
async fn upgrade_repins_the_association_at_the_profile_version() {
    // A `ProfileVersion` is `@length(min: 1, max: 32)`, `^[A-Za-z0-9-]+$`
    // (`smithy_query(query="shape", shape="ProfileVersion")`), so the versions the rows move
    // TO span that form — the next revision, a later one, a dashed spelling and the longest
    // the model allows — rather than one spelling of it, and the rows differ along whether
    // the workload is behind the profile at all and whether a milestone is asked for.
    let rows = [
        UpgradeCase {
            case: "an association one revision behind",
            name: "upgrade-behind",
            latest: Some("2"),
            milestone: None,
        },
        UpgradeCase {
            case: "a milestone of the review before the move",
            name: "upgrade-milestone",
            latest: Some("3"),
            milestone: Some("before the upgrade"),
        },
        UpgradeCase {
            case: "an association already at the profile's version",
            name: "upgrade-current",
            latest: None,
            milestone: None,
        },
        UpgradeCase {
            case: "already current, with a milestone asked for",
            name: "upgrade-current-milestone",
            latest: None,
            milestone: Some("current review"),
        },
        UpgradeCase {
            case: "a dashed revision of the profile",
            name: "upgrade-dashed",
            latest: Some("2024-11-02-B"),
            milestone: Some("dashed revision"),
        },
        UpgradeCase {
            case: "the longest version the model allows",
            name: "upgrade-long-version",
            latest: Some("A234567890123456789012345678901b"),
            milestone: None,
        },
    ];

    for row in rows {
        let client = Client::new();
        let (workload_id, profile_arn, pinned, current) =
            fixture(&client, row.name, row.latest).await;

        age_workload(&client, &workload_id, AGE);
        let before = updated_at_of(&client, &workload_id, row.case).await;

        let mut body = json!({ "ClientRequestToken": format!("tok-upgrade-{}", row.name) });
        if let Some(milestone) = row.milestone {
            body["MilestoneName"] = json!(milestone);
        }
        let answer = upgrade(&client, &workload_id, &profile_arn, body).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected UpgradeProfileVersion 200, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.body,
            Value::Null,
            "case {:?}: UpgradeProfileVersion answers no body, got {answer:?}",
            row.case
        );

        // The association now reports the version the PROFILE is at, which for a workload
        // that was already there is the version it already held.
        assert_eq!(
            profiles_of(&client, &workload_id, row.case).await,
            vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": current })],
            "case {:?}: the association must be pinned at the profile's version",
            row.case
        );
        // The PROFILE is untouched: this operation moves the workload's association, not
        // the resource it points at.
        assert_eq!(
            profile_version_of(&client, &profile_arn, row.case).await,
            current,
            "case {:?}: the profile itself must still be at its own version",
            row.case
        );

        let after = updated_at_of(&client, &workload_id, row.case).await;
        if pinned == current {
            assert_eq!(
                after, before,
                "case {:?}: a workload already at the profile's version is changed by \
                 nothing, so its UpdatedAt must not move",
                row.case
            );
        } else {
            assert!(
                after > before,
                "case {:?}: moving the association changes the workload, so its UpdatedAt \
                 must advance, got {after} after {before}",
                row.case
            );
        }

        match row.milestone {
            None => assert!(
                milestone_numbers(&client, &workload_id, row.case)
                    .await
                    .is_empty(),
                "case {:?}: a request naming no milestone records none",
                row.case
            ),
            Some(milestone) => {
                assert_eq!(
                    milestone_numbers(&client, &workload_id, row.case).await,
                    vec![1],
                    "case {:?}: a request naming a milestone records exactly one",
                    row.case
                );
                let recorded = stored_milestone(&client, &workload_id, 1, row.case).await;
                assert_eq!(
                    recorded.get("MilestoneName").and_then(Value::as_str),
                    Some(milestone),
                    "case {:?}: the milestone must carry the name the request sent, got \
                     {recorded:?}",
                    row.case
                );
                // The milestone froze the review as it stood BEFORE the move: it reports
                // the version the association was pinned at, not the upgraded one.
                assert_eq!(
                    recorded
                        .get("Workload")
                        .and_then(|workload| workload.get("Profiles"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                    vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": pinned })],
                    "case {:?}: the milestone must report the pre-upgrade association, got \
                     {recorded:?}",
                    row.case
                );
            }
        }
    }
}

/// One row of the refusal table: whether the request names the stored workload, which ARN it
/// names, and the resource the 404 must report as missing.
struct RefusalCase {
    case: &'static str,
    /// `None` names the stored workload under test; `Some(id)` a `WorkloadId` of the right
    /// shape that names no stored workload.
    workload: Option<&'static str>,
    /// Which of the fixture's ARNs the row sends, by key.
    arn: &'static str,
    resource_type: &'static str,
}

/// A request naming a workload that is not stored, or an ARN that names no profile
/// associated with it, is a modelled 404 that leaves the association, the workload's
/// `UpdatedAt` and the workload's milestones exactly as they stood — including when the
/// request also asks for a milestone.
#[tokio::test]
async fn upgrade_refuses_what_it_cannot_resolve_and_writes_nothing() {
    let client = Client::new();
    let (workload_id, own, pinned, current) =
        fixture(&client, "upgrade-refusal-a", Some("2")).await;
    let (other_id, other, other_pinned, _) =
        fixture(&client, "upgrade-refusal-b", Some("2")).await;
    // A profile that EXISTS and is associated with no workload: an association is what this
    // operation moves, so a stored profile nobody associated is as absent here as an ARN
    // naming no profile at all.
    let (unassociated, _) = created_profile(&client, "upgrade-refusal-loose").await;
    let absent = format!(
        "arn:aws:wellarchitected:{REGION}:{}:profile/noprofile9",
        engine::emulator_account_id()
    );
    // The association's ARN in ANOTHER account: a `ProfileArn` identifies the profile on its
    // own, so an ARN agreeing on the resource part and disagreeing on the account names a
    // different profile.
    let another_account = own.replace(engine::emulator_account_id(), "210987654321");
    assert_ne!(
        another_account, own,
        "the other-account ARN must differ from the association's"
    );

    let arn_of = |key: &str| match key {
        "own" => own.clone(),
        "other" => other.clone(),
        "unassociated" => unassociated.clone(),
        "absent" => absent.clone(),
        "another-account" => another_account.clone(),
        unknown => panic!("unknown fixture key {unknown:?}"),
    };

    let rows = [
        RefusalCase {
            case: "a workload that is not stored",
            workload: Some(ABSENT_WORKLOAD_ID),
            arn: "own",
            resource_type: WORKLOAD_TYPE,
        },
        RefusalCase {
            case: "a stored profile that is associated with no workload",
            workload: None,
            arn: "unassociated",
            resource_type: PROFILE_TYPE,
        },
        RefusalCase {
            case: "an ARN naming no stored profile at all",
            workload: None,
            arn: "absent",
            resource_type: PROFILE_TYPE,
        },
        RefusalCase {
            case: "a profile associated with another workload",
            workload: None,
            arn: "other",
            resource_type: PROFILE_TYPE,
        },
        RefusalCase {
            case: "an ARN differing from the association only in the account",
            workload: None,
            arn: "another-account",
            resource_type: PROFILE_TYPE,
        },
    ];

    for row in rows {
        let target = row.workload.unwrap_or(&workload_id).to_string();
        age_workload(&client, &workload_id, AGE);
        let before = updated_at_of(&client, &workload_id, row.case).await;
        let answer = upgrade(
            &client,
            &target,
            &arn_of(row.arn),
            json!({
                "MilestoneName": "a milestone no refused request records",
                "ClientRequestToken": format!("tok-refused-{}", row.arn),
            }),
        )
        .await;

        if row.resource_type == WORKLOAD_TYPE {
            assert_not_found(
                row.case,
                &answer,
                WORKLOAD_TYPE,
                ABSENT_WORKLOAD_ID,
                &format!("workload with ID {ABSENT_WORKLOAD_ID}"),
            );
        } else {
            let arn = arn_of(row.arn);
            assert_not_found(
                row.case,
                &answer,
                PROFILE_TYPE,
                &arn,
                &format!("profile with ARN {arn} associated with a workload with ID {target}"),
            );
        }

        // Nothing the refused request would have written landed: the association is still
        // behind, the workload's `UpdatedAt` has not moved and no milestone was recorded —
        // in either workload.
        assert_eq!(
            profiles_of(&client, &workload_id, row.case).await,
            vec![json!({ "ProfileArn": own, "ProfileVersion": pinned })],
            "case {:?}: a refused request must leave the association where it was",
            row.case
        );
        assert_eq!(
            profiles_of(&client, &other_id, row.case).await,
            vec![json!({ "ProfileArn": other, "ProfileVersion": other_pinned })],
            "case {:?}: a refused request must leave another workload's association alone",
            row.case
        );
        assert_eq!(
            updated_at_of(&client, &workload_id, row.case).await,
            before,
            "case {:?}: a refused request writes nothing, so UpdatedAt must not move",
            row.case
        );
        for (id, which) in [(&workload_id, "the named"), (&other_id, "another")] {
            assert!(
                milestone_numbers(&client, id, row.case).await.is_empty(),
                "case {:?}: a refused request records no milestone in {which} workload",
                row.case
            );
        }
    }

    // …and the upgrade the rows above could not resolve still works for the identifiers that
    // do resolve, so the refusals were about the lookup and not about the operation.
    let served = upgrade(
        &client,
        &workload_id,
        &own,
        json!({ "ClientRequestToken": "tok-refusal-tail" }),
    )
    .await;
    assert_eq!(
        served.status, 200,
        "the association that does resolve must still upgrade, got {served:?}"
    );
    assert_eq!(
        profiles_of(&client, &workload_id, "after the refusals").await,
        vec![json!({ "ProfileArn": own, "ProfileVersion": current })],
        "the resolved upgrade must pin the association at the profile's version"
    );
}

/// One `ClientRequestToken` performs the upgrade ONCE: a retry carrying it with the same
/// parameters replays the original empty success without recording a second milestone, and
/// the same token with different parameters is the modelled 409. A request carrying no token
/// is served on its own, so repeating its milestone name collides with the milestone the
/// first one recorded.
#[tokio::test]
async fn upgrade_performs_one_token_once() {
    let client = Client::new();
    let case = "idempotent upgrade";
    let (workload_id, profile_arn, _, current) =
        fixture(&client, "upgrade-idempotent", Some("4")).await;

    let first = upgrade(
        &client,
        &workload_id,
        &profile_arn,
        json!({ "MilestoneName": "first upgrade", "ClientRequestToken": "tok-once" }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected the first upgrade to answer 200, got {first:?}"
    );
    assert_eq!(
        milestone_numbers(&client, &workload_id, case).await,
        vec![1],
        "case {case:?}: the first upgrade records one milestone"
    );
    let after_first = updated_at_of(&client, &workload_id, case).await;

    // The replay: same token, same parameters. The original result was an empty body, so
    // that is what comes back — and nothing is written a second time.
    let replay = upgrade(
        &client,
        &workload_id,
        &profile_arn,
        json!({ "MilestoneName": "first upgrade", "ClientRequestToken": "tok-once" }),
    )
    .await;
    assert_eq!(
        replay.status, 200,
        "case {case:?}: a replay of the same token and parameters answers 200, got {replay:?}"
    );
    assert_eq!(
        milestone_numbers(&client, &workload_id, case).await,
        vec![1],
        "case {case:?}: a replay records no second milestone"
    );
    assert_eq!(
        updated_at_of(&client, &workload_id, case).await,
        after_first,
        "case {case:?}: a replay writes nothing, so UpdatedAt must not move"
    );

    // The same token with DIFFERENT parameters is not the same request.
    let conflicting = upgrade(
        &client,
        &workload_id,
        &profile_arn,
        json!({ "MilestoneName": "another milestone", "ClientRequestToken": "tok-once" }),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case {case:?}: expected HTTP 409, got {conflicting:?}"
    );
    assert_eq!(
        conflicting.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {conflicting:?}"
    );
    assert_eq!(
        member(&conflicting, "ResourceType"),
        PROFILE_TYPE,
        "case {case:?}: the conflict names the profile it already upgraded, got {conflicting:?}"
    );
    assert_eq!(
        member(&conflicting, "ResourceId"),
        profile_arn,
        "case {case:?}: ResourceId must be that profile's ARN, got {conflicting:?}"
    );
    assert_eq!(
        member(&conflicting, "Message"),
        "[Conflict] A request with ClientRequestToken tok-once was already submitted with \
         different parameters.",
        "case {case:?}: unexpected Message, got {conflicting:?}"
    );
    assert_eq!(
        milestone_numbers(&client, &workload_id, case).await,
        vec![1],
        "case {case:?}: the refused token records no milestone"
    );

    // A request carrying NO token is not replayable, so it is served on its own: the
    // milestone name the first upgrade recorded is taken — compared the way this service
    // compares a milestone name, which ignores capitalization and whitespace — and a free
    // one is recorded.
    let taken = upgrade(
        &client,
        &workload_id,
        &profile_arn,
        json!({ "MilestoneName": "FIRST   upgrade" }),
    )
    .await;
    assert_eq!(
        taken.status, 409,
        "case {case:?}: a milestone name already recorded is a conflict, got {taken:?}"
    );
    assert_eq!(
        member(&taken, "ResourceType"),
        MILESTONE_TYPE,
        "case {case:?}: that conflict names the milestone in the way, got {taken:?}"
    );
    let free = upgrade(
        &client,
        &workload_id,
        &profile_arn,
        json!({ "MilestoneName": "a second milestone" }),
    )
    .await;
    assert_eq!(
        free.status, 200,
        "case {case:?}: a request with a free milestone name is served, got {free:?}"
    );
    assert_eq!(
        milestone_numbers(&client, &workload_id, case).await,
        vec![1, 2],
        "case {case:?}: the served request records the next milestone"
    );
    // Every one of those requests left the association at the profile's version, which the
    // first upgrade moved it to.
    assert_eq!(
        profiles_of(&client, &workload_id, case).await,
        vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": current })],
        "case {case:?}: the association must stay at the profile's version"
    );
}

/// The milestone a request asks for is metered by the workload's milestone ceiling, which
/// this operation answers with the modelled 402 `ServiceQuotaExceededException` — and a
/// refused request upgrades nothing.
#[tokio::test]
async fn upgrade_refuses_a_milestone_past_the_workload_ceiling() {
    let client = Client::new();
    let case = "the milestone ceiling";
    let (workload_id, profile_arn, pinned, current) =
        fixture(&client, "upgrade-milestone-quota", Some("5")).await;
    // The ceiling is reached by seeding, as this crate's suites reach a quota the HTTP
    // boundary would take a hundred requests to reach; the request under test still crosses
    // it.
    {
        let state = client.store();
        let mut store = state.lock();
        let recorded = store
            .workloads
            .get(&workload_id)
            .cloned()
            .expect("the workload under test must exist");
        let milestones = store.milestones.entry(workload_id.clone()).or_default();
        for number in 1..=MAX_MILESTONES_PER_WORKLOAD {
            milestones.insert(
                number,
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MilestoneRecord {
                    milestone_number: number,
                    milestone_name: format!("seeded-{number}"),
                    recorded_at: SystemTime::now(),
                    workload: recorded.clone(),
                },
            );
        }
    }

    let answer = upgrade(
        &client,
        &workload_id,
        &profile_arn,
        json!({ "MilestoneName": "one milestone too many" }),
    )
    .await;
    assert_eq!(
        answer.status, 402,
        "case {case:?}: expected HTTP 402, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ServiceQuotaExceededException",
        "case {case:?}: expected ServiceQuotaExceededException, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "QuotaCode"),
        "MilestonesPerWorkload",
        "case {case:?}: the refusal names the quota it metered, got {answer:?}"
    );
    assert_eq!(
        profiles_of(&client, &workload_id, case).await,
        vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": pinned })],
        "case {case:?}: a refused request upgrades nothing"
    );

    // Asking for no milestone at all is not metered, so the upgrade is served.
    let served = upgrade(&client, &workload_id, &profile_arn, json!({})).await;
    assert_eq!(
        served.status, 200,
        "case {case:?}: an upgrade asking for no milestone is served, got {served:?}"
    );
    assert_eq!(
        profiles_of(&client, &workload_id, case).await,
        vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": current })],
        "case {case:?}: that upgrade pins the association at the profile's version"
    );
}

/// The modelled constraints on this operation's members are refused at the boundary with a
/// 400 `ValidationException`, never with the 404 of a lookup or with a 200: `MilestoneName`
/// is `@length(min: 3, max: 100)` and `ClientRequestToken` `@length(min: 1, max: 2048)`
/// (`smithy_query(query="operation", operation="UpgradeProfileVersion")`).
#[tokio::test]
async fn upgrade_refuses_a_request_the_model_does_not_allow() {
    let client = Client::new();
    let (workload_id, profile_arn, pinned, _) =
        fixture(&client, "upgrade-validation", Some("6")).await;

    let rows = [
        (
            "a MilestoneName one character short of the minimum",
            json!({ "MilestoneName": "ab" }),
        ),
        (
            "a MilestoneName one character past the maximum",
            json!({ "MilestoneName": "m".repeat(101) }),
        ),
        (
            "an empty ClientRequestToken",
            json!({ "MilestoneName": "a milestone", "ClientRequestToken": "" }),
        ),
    ];

    for (case, body) in rows {
        let answer = upgrade(&client, &workload_id, &profile_arn, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            milestone_numbers(&client, &workload_id, case)
                .await
                .is_empty(),
            "case {case:?}: a refused request records no milestone"
        );
        assert_eq!(
            profiles_of(&client, &workload_id, case).await,
            vec![json!({ "ProfileArn": profile_arn, "ProfileVersion": pinned })],
            "case {case:?}: a refused request upgrades nothing"
        );
    }

    // Each published bound is tested one step past it above and ON the bound here, where the
    // same request is served.
    for (case, name) in [
        ("a MilestoneName at the minimum length", "abc".to_string()),
        ("a MilestoneName at the maximum length", "m".repeat(100)),
    ] {
        let answer = upgrade(
            &client,
            &workload_id,
            &profile_arn,
            json!({ "MilestoneName": name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a name on the published bound is served, got {answer:?}"
        );
    }
    assert_eq!(
        milestone_numbers(&client, &workload_id, "after the bounds").await,
        vec![1, 2],
        "the two requests on the published bounds each recorded a milestone"
    );
}
