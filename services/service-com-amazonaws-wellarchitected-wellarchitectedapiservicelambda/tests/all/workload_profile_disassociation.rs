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
//! one the service handed out. ASSOCIATING a profile has no HTTP path yet —
//! `AssociateProfiles` is still a stub with its own issue — so the association each row
//! starts from is seeded through the shared state by this suite's own
//! [`associate_profile`], exactly as this crate's suites seed a stored answer or a workload
//! share whose writer is still a stub.
//!
//! Everything asserted crosses the HTTP boundary: requests go through the real
//! `MultiServiceRouter`, so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run as they do in production.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state as state_crate;

/// The Region the shared harness signs every request for, and so the Region an ARN built
/// here is in.
const REGION: &str = "us-west-2";

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A minimal valid `CreateProfile` request: `ProfileName`, `ProfileDescription`,
/// `ProfileQuestions` and `ClientRequestToken` are all `@required`
/// (`smithy_query(query="operation", operation="CreateProfile")`).
fn minimal_profile(name: &str) -> Value {
    json!({
        "ProfileName": name,
        "ProfileDescription": "a profile under test",
        "ProfileQuestions": [{
            "QuestionId": "priority",
            "SelectedChoiceIds": ["priority_cost"],
        }],
        "ClientRequestToken": format!("tok-{name}"),
    })
}

/// A profile minted over HTTP: its `ProfileArn` and the `ProfileVersion` the service put
/// it at, both checked against the modelled forms
/// (`^arn:aws[-a-z]*:wellarchitected:[a-z]{2}(-gov)?-[a-z]+-\d:\d{12}:profile/[a-z0-9]+$`
/// and `@length(1, 32)` `^[A-Za-z0-9-]+$`).
async fn created_profile(client: &Client, name: &str) -> (String, String) {
    let answer = client.create_profile(minimal_profile(name)).await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateProfile 200, got {answer:?}"
    );
    let arn = member(&answer, "ProfileArn").to_string();
    let version = member(&answer, "ProfileVersion").to_string();
    assert!(
        arn.starts_with(&format!(
            "arn:aws:wellarchitected:{REGION}:{}:profile/",
            engine::emulator_account_id()
        )),
        "case {name:?}: unexpected ProfileArn {arn:?}"
    );
    assert!(
        (1..=32).contains(&version.len())
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {name:?}: ProfileVersion must match ^[A-Za-z0-9-]{{1,32}}$, got {version:?}"
    );
    (arn, version)
}

/// The `Workload` a `GetWorkload` reports, as a JSON object.
async fn stored_workload(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    answer.body.get("Workload").cloned().unwrap_or(Value::Null)
}

/// The `Workload$Profiles` a workload reports, empty for a workload that reports none.
async fn profiles_of(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    stored_workload(client, workload_id, case)
        .await
        .get("Profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Workload$UpdatedAt` a workload reports, as the epoch seconds restJson1 carries it.
async fn updated_at_of(client: &Client, workload_id: &str, case: &str) -> f64 {
    stored_workload(client, workload_id, case)
        .await
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the workload must report UpdatedAt"))
}

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
    associate_profile(client, &workload_id, &arn, &version);
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

/// Associate `profile_arn`, at `profile_version`, with `workload_id` straight through the
/// shared state.
///
/// Associating a profile has no HTTP path yet: `AssociateProfiles` is still a stub with its
/// own issue (`20261024T015923Z-wellarchitected-associateprofiles-stub`), so the association
/// this suite has to UNDO is seeded exactly as this crate's suites seed a stored answer or a
/// workload share whose writer is a stub. The PROFILE itself is minted over HTTP by
/// `CreateProfile`, and the operation under test and the `GetWorkload` that reads the result
/// back both cross the HTTP boundary.
fn associate_profile(client: &Client, workload_id: &str, profile_arn: &str, version: &str) {
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

/// The `LensReview$Profiles` the workload's review of the framework lens reports: "the
/// profiles associated with the workload" (`smithy_query(query="doc", shape="LensReview",
/// member="Profiles")`), so a review has to agree with `GetWorkload` about them.
async fn lens_review_profiles(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    let answer = client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/lensReviews/wellarchitected"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .and_then(|review| review.get("Profiles"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Move the stored workload's `UpdatedAt` back by `age`, so a write the handler makes to
/// it is observable whatever the clock's granularity between two in-process requests.
/// Only the SEED reaches state directly; the write under test is the handler's.
fn age_workload(client: &Client, workload_id: &str, age: Duration) {
    let state = client.store();
    let mut store = state.lock();
    let workload = store
        .workloads
        .get_mut(workload_id)
        .expect("the workload under test must exist");
    workload.updated_at = SystemTime::now() - age;
}

/// The refusal a lookup that resolved nothing gets: 404 `ResourceNotFoundException`, with
/// the modelled `ResourceType` of the resource that was missing and the identifier AS SENT.
fn assert_not_found(
    case: &str,
    answer: &Answer,
    resource_type: &str,
    resource_id: &str,
    subject: &str,
) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        resource_type,
        "case {case:?}: expected ResourceType {resource_type}, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must be the identifier as sent, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[ResourceNotFound] No {subject} was found in account {}.",
            engine::emulator_account_id()
        ),
        "case {case:?}: unexpected Message, got {answer:?}"
    );
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
    // A version other than the minted one is reachable only by seeding, because
    // `UpgradeProfileVersion` is still a stub with its own issue.
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
            lens_review_profiles(&client, &workload_id, row.case).await,
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
            lens_review_profiles(&client, &workload_id, row.case)
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
