// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DisassociateProfiles`.
//!
//! The rule these pin, over a TABLE of requests rather than one recorded call:
//! "Disassociate a profile from a workload" (`smithy_query(query="operation",
//! operation="DisassociateProfiles")`) removes from the workload the request names exactly
//! the profile associations its `ProfileArns` name — the member the workload reports them
//! by is `Workload$Profiles` — and resolves BOTH identifiers before writing anything, so a
//! workload that is not stored and an ARN that names no profile associated with it are each
//! a modelled 404 `ResourceNotFoundException` that leaves every association as it stood.
//!
//! The profiles here are minted over HTTP by `CreateProfile`, so every ARN a row sends is
//! one the service handed out, and the association each row starts from is made over HTTP by
//! `AssociateProfiles` wherever that operation can produce it. An association pinned at a
//! version OTHER than the one the profile is at is not reachable over HTTP — associating
//! pins the profile's current version, and `UpgradeProfileVersion` moves an association to
//! the version its profile is at rather than to an arbitrary one — so those rows seed it
//! through the shared state, exactly as this crate's suites seed a stored answer or a
//! workload share whose writer is still a stub.
//!
//! Everything asserted crosses the HTTP boundary: requests go through the real
//! `MultiServiceRouter`, so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run as they do in production.

use std::time::Duration;

use serde_json::{json, Value};

use super::workload_harness::{
    age_workload, assert_not_found, created_id, created_profile, lens_review_profiles,
    minimal_create, profiles_of, updated_at_of, Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state as state_crate;

/// The Region the shared harness signs every request for, and so the Region an ARN built
/// here is in.
const REGION: &str = "us-west-2";

/// A workload created over HTTP, with one profile — also created over HTTP — associated
/// with it, pinned at `version` or at the version the created profile is at, and that
/// association asserted to be what the workload reports.
async fn workload_with_profile(
    client: &Client,
    name: &str,
    version: Option<&str>,
) -> (String, String, String) {
    let workload_id = created_id(
        &client
            .create(minimal_create(name, &format!("tok-{name}")))
            .await,
        name,
    );
    let (arn, minted_version) = created_profile(client, name).await;
    let version = version.unwrap_or(&minted_version).to_string();
    associate_profile(client, &workload_id, &arn, &version, &minted_version).await;
    assert_eq!(
        profiles_of(client, &workload_id, name).await,
        vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
        "case {name:?}: the workload must report the association it holds"
    );
    (workload_id, arn, version)
}

/// `DisassociateProfiles`: `PATCH /workloads/{WorkloadId}/disassociateProfiles`.
async fn disassociate_profiles(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/disassociateProfiles"),
            Some(body),
        )
        .await
}

/// Associate `profile_arn`, at `profile_version`, with `workload_id`.
///
/// Over HTTP through `AssociateProfiles` when the version asked for is the one the profile
/// is at, which is the version associating pins (`super::workload_profile_association`
/// covers that operation itself). An association pinned at any OTHER version is reachable
/// only by seeding the shared state — `UpgradeProfileVersion` moves a workload to the
/// version its profile is at, not to an arbitrary one — exactly as this crate's suites seed
/// a stored answer whose writer is a stub. The operation under test and the `GetWorkload`
/// that reads the result back both cross the HTTP boundary either way.
async fn associate_profile(
    client: &Client,
    workload_id: &str,
    profile_arn: &str,
    version: &str,
    minted_version: &str,
) {
    if version == minted_version {
        let answer = client
            .send(
                "PATCH",
                &format!("/workloads/{workload_id}/associateProfiles"),
                Some(json!({ "ProfileArns": [profile_arn] })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "expected AssociateProfiles 200 for {profile_arn}, got {answer:?}"
        );
        return;
    }
    let state = client.store();
    let mut store = state.lock();
    let workload = store
        .workloads
        .get_mut(workload_id)
        .expect("the workload under test must exist");
    workload.profiles.push(state_crate::WorkloadProfileRecord {
        profile_arn: profile_arn.to_string(),
        profile_version: version.to_string(),
    });
}

/// One row of the removal table: the version the association is pinned at (`None` for the
/// version the created profile is at) and how many times the request names its ARN.
struct RemovalCase {
    case: &'static str,
    name: &'static str,
    version: Option<&'static str>,
    names: usize,
}

/// `DisassociateProfiles` removes exactly the associations the request names, whatever the
/// version the association is pinned at and however many times the list names it.
#[tokio::test]
async fn disassociate_profiles_removes_every_named_association() {
    // A `ProfileVersion` is `@length(min: 1, max: 32)`, `^[A-Za-z0-9-]+$`
    // (`smithy_query(query="shape", shape="ProfileVersion")`), so the versions the rows
    // pin an association at span that form — the version the service itself minted, a
    // dashed spelling and the longest the model allows — rather than one spelling of it.
    // A version other than the minted one is pinned by seeding here: `UpgradeProfileVersion`
    // moves an association to the version its PROFILE is at, which is not how a row reaches
    // an arbitrary one.
    let rows = [
        RemovalCase {
            case: "one ARN, the version CreateProfile put the profile at",
            name: "profile-removal-minted",
            version: None,
            names: 1,
        },
        RemovalCase {
            case: "one ARN, an association pinned at a dashed version",
            name: "profile-removal-dashed",
            version: Some("2024-10-01-A"),
            names: 1,
        },
        RemovalCase {
            case: "the same ARN named twice drops it once",
            name: "profile-removal-duplicate",
            version: None,
            names: 2,
        },
        RemovalCase {
            case: "an association pinned at the longest version the model allows",
            name: "profile-removal-long-version",
            version: Some("A234567890123456789012345678901b"),
            names: 1,
        },
    ];

    for row in rows {
        let client = Client::new();
        let (workload_id, arn, version) = workload_with_profile(&client, row.name, row.version).await;
        // The workload's review of its lens reports the same associations as the workload.
        assert_eq!(
            lens_review_profiles(&client, &workload_id, "wellarchitected", row.case).await,
            vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
            "case {:?}: a lens review must report the workload's association",
            row.case
        );
        // The workload's `UpdatedAt` is moved an hour into the past first, so the
        // handler's write to it has to ADVANCE the value rather than merely not go
        // backwards.
        age_workload(&client, &workload_id, Duration::from_secs(3_600));
        let before = updated_at_of(&client, &workload_id, row.case).await;

        let answer = disassociate_profiles(
            &client,
            &workload_id,
            json!({ "ProfileArns": vec![arn.clone(); row.names] }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected DisassociateProfiles 200, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.body,
            Value::Null,
            "case {:?}: DisassociateProfiles answers no body, got {answer:?}",
            row.case
        );

        assert!(
            profiles_of(&client, &workload_id, row.case).await.is_empty(),
            "case {:?}: the workload must report no profile once the only one it held is \
             disassociated",
            row.case
        );
        assert!(
            lens_review_profiles(&client, &workload_id, "wellarchitected", row.case)
                .await
                .is_empty(),
            "case {:?}: a lens review of the workload must report no profile either",
            row.case
        );
        let after = updated_at_of(&client, &workload_id, row.case).await;
        assert!(
            after > before,
            "case {:?}: disassociating a profile changes the workload, so its UpdatedAt must \
             advance, got {after} after {before}",
            row.case
        );
        // Disassociating a profile does not DELETE it: the profile is a resource of its
        // own, and only `DeleteProfile` removes one.
        let profile = client.get_profile(&arn, None).await;
        assert_eq!(
            profile.status, 200,
            "case {:?}: the profile must still exist once it is disassociated, got \
             {profile:?}",
            row.case
        );
    }
}

/// One row of the refusal table: whether the request names the stored workload, which ARNs
/// it names, and the resource the 404 must report as missing.
struct RefusalCase {
    case: &'static str,
    /// `None` names the stored workload under test; `Some(id)` a `WorkloadId` of the right
    /// shape that names no stored workload.
    workload: Option<&'static str>,
    /// Which of the fixture's ARNs the row sends, by key.
    arns: &'static [&'static str],
    resource_type: &'static str,
    /// The fixture key, or the literal id, the 404's `ResourceId` must carry.
    resource_id: &'static str,
}

/// A request naming a workload that is not stored, or an ARN that names no profile
/// associated with it, is a modelled 404 and changes NOTHING — including when the same list
/// also names an association the workload does hold.
#[tokio::test]
async fn disassociate_profiles_refuses_what_it_cannot_resolve_and_writes_nothing() {
    let client = Client::new();
    let (workload_id, own, own_version) =
        workload_with_profile(&client, "profile-refusal-a", None).await;
    let (other_id, other, other_version) =
        workload_with_profile(&client, "profile-refusal-b", None).await;
    // A profile that EXISTS and is associated with no workload: an association is what
    // this operation resolves, so a stored profile nobody associated is as absent here as
    // an ARN naming no profile at all.
    let (unassociated, _) = created_profile(&client, "profile-refusal-loose").await;

    // A `WorkloadId` is `^[0-9a-f]{32}$` and a `ProfileArn` carries the pattern above
    // (`smithy_query(query="shape", shape="WorkloadId,ProfileArn")`), so the identifiers
    // that name nothing still satisfy the modelled forms: the refusal under test is the
    // lookup's, not the boundary validator's.
    const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";
    let absent_profile = format!(
        "arn:aws:wellarchitected:{REGION}:{}:profile/noprofile9",
        engine::emulator_account_id()
    );
    // The association's ARN in ANOTHER account: a `ProfileArn` identifies the profile on
    // its own, so an ARN agreeing on the resource part and disagreeing on the account names
    // a different profile.
    let another_account = own.replace(engine::emulator_account_id(), "210987654321");
    assert_ne!(
        another_account, own,
        "the other-account ARN must differ from the association's"
    );

    let arn_of = |key: &str| match key {
        "own" => own.clone(),
        "other" => other.clone(),
        "unassociated" => unassociated.clone(),
        "absent" => absent_profile.clone(),
        "another-account" => another_account.clone(),
        unknown => panic!("unknown fixture key {unknown:?}"),
    };

    let rows = [
        RefusalCase {
            case: "a workload that is not stored",
            workload: Some(ABSENT_WORKLOAD_ID),
            arns: &["own"],
            resource_type: "WORKLOAD",
            resource_id: ABSENT_WORKLOAD_ID,
        },
        RefusalCase {
            case: "a stored profile that is associated with no workload",
            workload: None,
            arns: &["unassociated"],
            resource_type: "PROFILE",
            resource_id: "unassociated",
        },
        RefusalCase {
            case: "an ARN naming no stored profile at all",
            workload: None,
            arns: &["absent"],
            resource_type: "PROFILE",
            resource_id: "absent",
        },
        RefusalCase {
            case: "a profile associated with another workload",
            workload: None,
            arns: &["other"],
            resource_type: "PROFILE",
            resource_id: "other",
        },
        RefusalCase {
            case: "an ARN differing from the association only in the account",
            workload: None,
            arns: &["another-account"],
            resource_type: "PROFILE",
            resource_id: "another-account",
        },
        RefusalCase {
            case: "one association the workload holds beside one it does not",
            workload: None,
            arns: &["own", "absent"],
            resource_type: "PROFILE",
            resource_id: "absent",
        },
    ];

    for row in rows {
        let target = row.workload.unwrap_or(&workload_id).to_string();
        let sent: Vec<String> = row.arns.iter().map(|key| arn_of(key)).collect();
        // Moved into the past so that a write to `UpdatedAt` — which a refused request must
        // not make — would show up as an advance.
        age_workload(&client, &workload_id, Duration::from_secs(3_600));
        let before = updated_at_of(&client, &workload_id, row.case).await;
        let answer =
            disassociate_profiles(&client, &target, json!({ "ProfileArns": sent })).await;

        let (resource_id, subject) = if row.resource_type == "WORKLOAD" {
            (
                row.resource_id.to_string(),
                format!("workload with ID {}", row.resource_id),
            )
        } else {
            let arn = arn_of(row.resource_id);
            (
                arn.clone(),
                format!("profile with ARN {arn} associated with a workload with ID {target}"),
            )
        };
        assert_not_found(row.case, &answer, row.resource_type, &resource_id, &subject);

        // Nothing the refused request named was removed: both workloads still report the
        // association they held, so the walk resolved every ARN before writing any.
        assert_eq!(
            profiles_of(&client, &workload_id, row.case).await,
            vec![json!({ "ProfileArn": own, "ProfileVersion": own_version })],
            "case {:?}: a refused request must leave the workload's association in place",
            row.case
        );
        assert_eq!(
            profiles_of(&client, &other_id, row.case).await,
            vec![json!({ "ProfileArn": other, "ProfileVersion": other_version })],
            "case {:?}: a refused request must leave another workload's association in place",
            row.case
        );
        assert_eq!(
            updated_at_of(&client, &workload_id, row.case).await,
            before,
            "case {:?}: a refused request writes nothing, so the workload's UpdatedAt must \
             not move",
            row.case
        );
    }
}

/// A second disassociation of the same profile has nothing left to resolve, so it is the
/// same 404 as a profile that was never associated: the first call removed the association
/// the second one names.
#[tokio::test]
async fn disassociating_the_same_profile_twice_is_not_found_the_second_time() {
    let client = Client::new();
    let case = "repeated disassociation";
    let (workload_id, arn, _) = workload_with_profile(&client, "profile-repeat", None).await;

    let first = disassociate_profiles(
        &client,
        &workload_id,
        json!({ "ProfileArns": [arn.clone()] }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected the first DisassociateProfiles to answer 200, got {first:?}"
    );

    let second = disassociate_profiles(
        &client,
        &workload_id,
        json!({ "ProfileArns": [arn.clone()] }),
    )
    .await;
    assert_not_found(
        case,
        &second,
        "PROFILE",
        &arn,
        &format!("profile with ARN {arn} associated with a workload with ID {workload_id}"),
    );
}

/// The modelled constraints on `ProfileArns` are refused at the boundary with a 400
/// `ValidationException`, never with the 404 of a lookup or with a 200: `ProfileArns` is
/// `@length(min: 1)` and each entry is a `ProfileArn` carrying the modelled pattern
/// (`smithy_query(query="shape", shape="ProfileArns")`).
#[tokio::test]
async fn disassociate_profiles_refuses_a_request_the_model_does_not_allow() {
    let client = Client::new();
    let (workload_id, _, _) = workload_with_profile(&client, "profile-validation", None).await;

    let rows = [
        ("an empty ProfileArns list", json!({ "ProfileArns": [] })),
        (
            "a ProfileArns entry that is not an ARN",
            json!({ "ProfileArns": ["not-an-arn"] }),
        ),
        (
            "a ProfileArns entry naming another resource kind",
            json!({
                "ProfileArns": [format!(
                    "arn:aws:wellarchitected:{REGION}:{}:review-template/abc",
                    engine::emulator_account_id()
                )]
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = disassociate_profiles(&client, &workload_id, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // …and the association the workload holds survived every refusal.
    assert_eq!(
        profiles_of(&client, &workload_id, "after the refusals")
            .await
            .len(),
        1,
        "a refused request must leave the workload's association in place"
    );
}
