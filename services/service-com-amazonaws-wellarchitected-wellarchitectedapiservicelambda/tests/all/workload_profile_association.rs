// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `AssociateProfiles`.
//!
//! The rule these pin, over a TABLE of requests rather than one recorded call:
//! "Associate a profile with a workload" (`smithy_query(query="operation",
//! operation="AssociateProfiles")`) records, on the workload the request names, one
//! association per distinct profile its `ProfileArns` name, pinned at the `ProfileVersion`
//! that profile is at — the member the workload reports them by is `Workload$Profiles`,
//! which a lens review of the workload reports too — and resolves BOTH identifiers before
//! writing anything, so a workload that is not stored and an ARN that names no stored
//! profile are each a modelled 404 `ResourceNotFoundException` that changes nothing.
//! `WorkloadProfiles` is `@length(max: 1)` (`smithy_query(query="shape",
//! shape="WorkloadProfiles")`), so a request that would leave the workload reviewed against
//! more profiles than that is refused instead — before any ARN is resolved, since no such
//! request can be served whatever the store holds: with the modelled 409 `ConflictException`
//! when the workload already holds a different profile, and with the modelled 400
//! `ValidationException` when one request names several. The list's own LENGTH is held at
//! the service's per-request quota first of all, because `ProfileArns` publishes no
//! modelled maximum.
//!
//! The same rule holds for a `CreateWorkload` that names a `ProfileArn`, so the created
//! workload reports the association too — that is the last test here.
//!
//! Every profile and workload is minted over HTTP and everything asserted crosses the HTTP
//! boundary: requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production.

use std::time::Duration;

use serde_json::{json, Value};

use super::workload_harness::{
    age_workload, assert_not_found, created_id, created_profile, lens_review_profiles, member,
    minimal_create, profiles_of, updated_at_of, Answer, Client,
};

/// The Region the shared harness signs every request for, and so the Region an ARN built
/// here is in.
const REGION: &str = "us-west-2";

/// The modelled `ResourceType` of a workload and of a profile
/// (`crate::state::WORKLOAD_RESOURCE_TYPE` / `PROFILE_RESOURCE_TYPE`), as the wire carries
/// them on a failure about one.
const WORKLOAD_TYPE: &str = "WORKLOAD";
const PROFILE_TYPE: &str = "PROFILE";

/// `AssociateProfiles`: `PATCH /workloads/{WorkloadId}/associateProfiles`.
async fn associate_profiles(client: &Client, workload_id: &str, body: Value) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateProfiles"),
            Some(body),
        )
        .await
}

/// A workload created over HTTP from `lenses`, with nothing associated with it yet.
async fn created_workload(client: &Client, name: &str, lenses: &[&str]) -> String {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, name)
}

/// One row of the association table: the lenses the workload under test is reviewed
/// against, how many times the request names the profile's ARN, and how many profiles are
/// minted (only the first is named, so an unnamed one must stay unassociated).
struct AssociationCase {
    case: &'static str,
    name: &'static str,
    lenses: &'static [&'static str],
    names: usize,
}

/// `AssociateProfiles` records the association the request names, whatever lenses the
/// workload carries and however many times the list names the one profile, and the workload
/// and its lens reviews both report it at the version the profile is at.
#[tokio::test]
async fn associate_profiles_records_the_named_association() {
    // The rows differ along the workload's lens set — every lens review of the workload
    // reports "the profiles associated with the workload", so the association has to reach
    // each of them — and along how many times the request names the one ARN, which is what
    // the distinctness of the list is read over.
    let rows = [
        AssociationCase {
            case: "one lens, the ARN named once",
            name: "profile-associate-single",
            lenses: &["wellarchitected"],
            names: 1,
        },
        AssociationCase {
            case: "two lenses, the ARN named once",
            name: "profile-associate-two-lenses",
            lenses: &["wellarchitected", "serverless"],
            names: 1,
        },
        AssociationCase {
            case: "the same ARN named twice is one association",
            name: "profile-associate-duplicate",
            lenses: &["wellarchitected"],
            names: 2,
        },
        AssociationCase {
            case: "the same ARN named five times is one association",
            name: "profile-associate-duplicate-five",
            lenses: &["wellarchitected"],
            names: 5,
        },
    ];

    for row in rows {
        let client = Client::new();
        let workload_id = created_workload(&client, row.name, row.lenses).await;
        let (arn, version) = created_profile(&client, row.name).await;
        // A second profile nobody names: associating one profile must not associate
        // another the account happens to hold.
        let (unnamed, _) = created_profile(&client, &format!("{}-unnamed", row.name)).await;

        assert!(
            profiles_of(&client, &workload_id, row.case).await.is_empty(),
            "case {:?}: a created workload that names no profile must report none",
            row.case
        );
        // The workload's `UpdatedAt` is moved an hour into the past first, so the handler's
        // write to it has to ADVANCE the value rather than merely not go backwards.
        age_workload(&client, &workload_id, Duration::from_secs(3_600));
        let before = updated_at_of(&client, &workload_id, row.case).await;

        let answer = associate_profiles(
            &client,
            &workload_id,
            json!({ "ProfileArns": vec![arn.clone(); row.names] }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected AssociateProfiles 200, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.body,
            Value::Null,
            "case {:?}: AssociateProfiles answers no body, got {answer:?}",
            row.case
        );

        // Read back through `GetWorkload`: the association is recorded once, under the ARN
        // as sent and at the version `CreateProfile` put the profile at.
        assert_eq!(
            profiles_of(&client, &workload_id, row.case).await,
            vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
            "case {:?}: the workload must report exactly the association it was given",
            row.case
        );
        // …and through every lens review of the workload, which reports the same list.
        for lens in row.lenses {
            assert_eq!(
                lens_review_profiles(&client, &workload_id, lens, row.case).await,
                vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
                "case {:?}: the review of lens {lens:?} must report the workload's \
                 association",
                row.case
            );
        }
        // `ListLensReviews` reports "the profiles associated with the workload" on each
        // summary it pages over, so a listed review has to agree with the read of it.
        let listed = client
            .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected ListLensReviews 200, got {listed:?}",
            row.case
        );
        let summaries = listed
            .body
            .get("LensReviewSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            row.lenses.len(),
            "case {:?}: the workload must list one review per lens, got {listed:?}",
            row.case
        );
        for summary in &summaries {
            assert_eq!(
                summary.get("Profiles"),
                Some(&json!([{ "ProfileArn": arn, "ProfileVersion": version }])),
                "case {:?}: a listed review must report the workload's association, got \
                 {summary:?}",
                row.case
            );
        }
        let after = updated_at_of(&client, &workload_id, row.case).await;
        assert!(
            after > before,
            "case {:?}: associating a profile changes the workload, so its UpdatedAt must \
             advance, got {after} after {before}",
            row.case
        );
        // Associating a profile does not change the PROFILE: it is a resource of its own,
        // still readable at the version the association pinned.
        let profile = client.get_profile(&arn, Some(&version)).await;
        assert_eq!(
            profile.status, 200,
            "case {:?}: the profile must still be readable at the pinned version, got \
             {profile:?}",
            row.case
        );
        // And a profile the request did not name is associated with nothing: a second
        // workload sees no association of it.
        let other_id = created_workload(
            &client,
            &format!("{}-other", row.name),
            &["wellarchitected"],
        )
        .await;
        assert!(
            profiles_of(&client, &other_id, row.case).await.is_empty(),
            "case {:?}: another workload must report no association, got the one of \
             {unnamed}",
            row.case
        );
    }
}

/// Associating the profile a workload already holds asks for the state the workload is
/// already in: it is a 200 that changes nothing — neither the pinned version nor
/// `UpdatedAt`.
#[tokio::test]
async fn associating_the_same_profile_twice_changes_nothing() {
    let client = Client::new();
    let case = "repeated association";
    let workload_id = created_workload(&client, "profile-repeat-associate", &["wellarchitected"]).await;
    let (arn, version) = created_profile(&client, "profile-repeat-associate").await;

    let first = associate_profiles(&client, &workload_id, json!({ "ProfileArns": [arn.clone()] })).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected the first AssociateProfiles to answer 200, got {first:?}"
    );
    let association = vec![json!({ "ProfileArn": arn, "ProfileVersion": version })];
    assert_eq!(
        profiles_of(&client, &workload_id, case).await,
        association,
        "case {case:?}: the workload must report the association the first call made"
    );

    age_workload(&client, &workload_id, Duration::from_secs(3_600));
    let before = updated_at_of(&client, &workload_id, case).await;
    // The second call names the same ARN, twice over, to check the de-duplication and the
    // idempotence together.
    let second = associate_profiles(
        &client,
        &workload_id,
        json!({ "ProfileArns": [arn.clone(), arn.clone()] }),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "case {case:?}: re-associating the profile a workload holds is not a failure, got \
         {second:?}"
    );
    assert_eq!(
        profiles_of(&client, &workload_id, case).await,
        association,
        "case {case:?}: the workload must still report exactly one association"
    );
    assert_eq!(
        updated_at_of(&client, &workload_id, case).await,
        before,
        "case {case:?}: a request that asks for no change must not move the workload's \
         UpdatedAt"
    );
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

/// A request naming a workload that is not stored, or an ARN that names no stored profile,
/// is a modelled 404 and changes NOTHING — including when the same list also names a profile
/// that does exist.
#[tokio::test]
async fn associate_profiles_refuses_what_it_cannot_resolve_and_writes_nothing() {
    let client = Client::new();
    let workload_id = created_workload(&client, "profile-associate-refusal", &["wellarchitected"]).await;
    let (stored, _) = created_profile(&client, "profile-associate-refusal").await;

    // A `WorkloadId` is `^[0-9a-f]{32}$` and a `ProfileArn` carries the modelled pattern
    // (`smithy_query(query="shape", shape="WorkloadId,ProfileArn")`), so the identifiers
    // that name nothing still satisfy the modelled forms: the refusal under test is the
    // lookup's, not the boundary validator's.
    const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";
    let absent_profile = format!(
        "arn:aws:wellarchitected:{REGION}:{}:profile/noprofile9",
        engine::emulator_account_id()
    );
    // The stored profile's ARN in ANOTHER account: a `ProfileArn` identifies the profile on
    // its own, so an ARN agreeing on the resource part and disagreeing on the account names
    // a different profile, which this account does not hold.
    let another_account = stored.replace(engine::emulator_account_id(), "210987654321");
    assert_ne!(
        another_account, stored,
        "the other-account ARN must differ from the stored profile's"
    );
    // The same resource part in another Region, for the same reason.
    let another_region = stored.replace(REGION, "eu-west-1");
    assert_ne!(
        another_region, stored,
        "the other-Region ARN must differ from the stored profile's"
    );

    let arn_of = |key: &str| match key {
        "stored" => stored.clone(),
        "absent" => absent_profile.clone(),
        "another-account" => another_account.clone(),
        "another-region" => another_region.clone(),
        unknown => panic!("unknown fixture key {unknown:?}"),
    };

    let rows = [
        RefusalCase {
            case: "a workload that is not stored",
            workload: Some(ABSENT_WORKLOAD_ID),
            arns: &["stored"],
            resource_type: WORKLOAD_TYPE,
            resource_id: ABSENT_WORKLOAD_ID,
        },
        RefusalCase {
            case: "an ARN naming no stored profile at all",
            workload: None,
            arns: &["absent"],
            resource_type: PROFILE_TYPE,
            resource_id: "absent",
        },
        RefusalCase {
            case: "an ARN differing from a stored profile only in the account",
            workload: None,
            arns: &["another-account"],
            resource_type: PROFILE_TYPE,
            resource_id: "another-account",
        },
        RefusalCase {
            case: "an ARN differing from a stored profile only in the Region",
            workload: None,
            arns: &["another-region"],
            resource_type: PROFILE_TYPE,
            resource_id: "another-region",
        },
    ];

    for row in rows {
        let target = row.workload.unwrap_or(&workload_id).to_string();
        let sent: Vec<String> = row.arns.iter().map(|key| arn_of(key)).collect();
        // Moved into the past so that a write to `UpdatedAt` — which a refused request must
        // not make — would show up as an advance.
        age_workload(&client, &workload_id, Duration::from_secs(3_600));
        let before = updated_at_of(&client, &workload_id, row.case).await;

        let answer = associate_profiles(&client, &target, json!({ "ProfileArns": sent })).await;

        let (resource_id, subject) = if row.resource_type == WORKLOAD_TYPE {
            (
                row.resource_id.to_string(),
                format!("workload with ID {}", row.resource_id),
            )
        } else {
            let arn = arn_of(row.resource_id);
            (arn.clone(), format!("profile with ARN {arn}"))
        };
        assert_not_found(row.case, &answer, row.resource_type, &resource_id, &subject);

        // Nothing the refused request named was recorded: the workload still reports no
        // association, so the walk resolved every ARN before writing any.
        assert!(
            profiles_of(&client, &workload_id, row.case).await.is_empty(),
            "case {:?}: a refused request must leave the workload with no association",
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

/// A workload is reviewed against at most `MAX_PROFILES_PER_WORKLOAD` profiles, which is
/// what the modelled `WorkloadProfiles` `@length(max: 1)` allows it to report: a request
/// that would leave it holding more is refused — as a 409 `ConflictException` naming the
/// association in the way when the workload already holds a different profile, and as a 400
/// `ValidationException` naming `ProfileArns` when one request names several — and the
/// workload keeps exactly what it held.
#[tokio::test]
async fn a_workload_is_associated_with_at_most_the_modelled_number_of_profiles() {
    // The service's own ceiling, so the test cannot drift from the handler.
    const MAX: usize = service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_PROFILES_PER_WORKLOAD;

    // Rows differ along HOW the request would exceed the bound: by naming more distinct
    // profiles than a workload can hold in one request (one past the bound, and further
    // past it), and by naming one more than the workload already holds.
    for extra in [MAX + 1, MAX + 2, MAX + 4] {
        let client = Client::new();
        let case = format!("{extra} distinct profiles in one request");
        let workload_id = created_workload(&client, &format!("profile-bound-{extra}"), &["wellarchitected"]).await;
        let mut arns = Vec::new();
        for index in 0..extra {
            let (arn, _) = created_profile(&client, &format!("profile-bound-{extra}-{index}")).await;
            arns.push(arn);
        }

        age_workload(&client, &workload_id, Duration::from_secs(3_600));
        let before = updated_at_of(&client, &workload_id, &case).await;
        let answer = associate_profiles(&client, &workload_id, json!({ "ProfileArns": arns })).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {case:?}: a refused member value is a field validation, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .map(|fields| fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect::<Vec<_>>()),
            Some(vec!["ProfileArns"]),
            "case {case:?}: Fields must name the member that carried the ARNs, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains(&format!("at most {MAX} profile")),
            "case {case:?}: the Message must state the bound, got {answer:?}"
        );
        assert!(
            profiles_of(&client, &workload_id, &case).await.is_empty(),
            "case {case:?}: a refused request must leave the workload with no association"
        );
        assert_eq!(
            updated_at_of(&client, &workload_id, &case).await,
            before,
            "case {case:?}: a refused request must not move the workload's UpdatedAt"
        );
    }

    // The bound is decided before any ARN is resolved: a request naming more distinct
    // profiles than a workload can hold cannot be served whatever the store holds, so it is
    // refused the same way when one of those ARNs names no profile at all.
    {
        let client = Client::new();
        let case = "one stored profile beside an ARN naming none";
        let workload_id =
            created_workload(&client, "profile-bound-unresolvable", &["wellarchitected"]).await;
        let (stored, _) = created_profile(&client, "profile-bound-unresolvable").await;
        let absent = format!(
            "arn:aws:wellarchitected:{REGION}:{}:profile/noprofile9",
            engine::emulator_account_id()
        );
        let answer = associate_profiles(
            &client,
            &workload_id,
            json!({ "ProfileArns": [stored, absent] }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            profiles_of(&client, &workload_id, case).await.is_empty(),
            "case {case:?}: a refused request must leave the workload with no association"
        );
    }

    // A workload already holding its one profile: associating a DIFFERENT one collides with
    // the association it holds.
    let client = Client::new();
    let case = "a second profile for a workload that holds one";
    let workload_id = created_workload(&client, "profile-bound-conflict", &["wellarchitected"]).await;
    let (held, held_version) = created_profile(&client, "profile-bound-conflict-held").await;
    let (second, _) = created_profile(&client, "profile-bound-conflict-second").await;
    let first = associate_profiles(&client, &workload_id, json!({ "ProfileArns": [held.clone()] })).await;
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected the first AssociateProfiles to answer 200, got {first:?}"
    );

    // Both spellings of the over-full request: the second profile alone, and the second
    // beside the one the workload already holds.
    for (label, sent) in [
        ("the second profile alone", vec![second.clone()]),
        (
            "the second profile beside the held one",
            vec![held.clone(), second.clone()],
        ),
    ] {
        age_workload(&client, &workload_id, Duration::from_secs(3_600));
        let before = updated_at_of(&client, &workload_id, case).await;
        let answer = associate_profiles(&client, &workload_id, json!({ "ProfileArns": sent })).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?} ({label}): expected HTTP 409, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ConflictException",
            "case {case:?} ({label}): expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            WORKLOAD_TYPE,
            "case {case:?} ({label}): the collision is inside the workload, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            workload_id,
            "case {case:?} ({label}): ResourceId must name the workload, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            format!(
                "[Conflict] The workload with ID {workload_id} is already associated with the \
                 profile with ARN {held}."
            ),
            "case {case:?} ({label}): the Message must name the association in the way, got \
             {answer:?}"
        );
        assert_eq!(
            profiles_of(&client, &workload_id, case).await,
            vec![json!({ "ProfileArn": held, "ProfileVersion": held_version })],
            "case {case:?} ({label}): the workload must keep exactly the association it held"
        );
        assert_eq!(
            updated_at_of(&client, &workload_id, case).await,
            before,
            "case {case:?} ({label}): a refused request must not move the workload's UpdatedAt"
        );
    }

    // …and once the held profile is disassociated, the second one associates.
    let removed = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/disassociateProfiles"),
            Some(json!({ "ProfileArns": [held.clone()] })),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: expected DisassociateProfiles 200, got {removed:?}"
    );
    let answer = associate_profiles(&client, &workload_id, json!({ "ProfileArns": [second.clone()] })).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the freed slot must take the second profile, got {answer:?}"
    );
    assert_eq!(
        profiles_of(&client, &workload_id, case)
            .await
            .iter()
            .filter_map(|profile| profile.get("ProfileArn").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        vec![second.as_str()],
        "case {case:?}: the workload must now report the second profile"
    );
}

/// The modelled constraints on `ProfileArns` are refused at the boundary with a 400
/// `ValidationException`, never with the 404 of a lookup or with a 200: `ProfileArns` is
/// `@length(min: 1)` and each entry is a `ProfileArn` carrying the modelled pattern
/// (`smithy_query(query="shape", shape="ProfileArns")`).
#[tokio::test]
async fn associate_profiles_refuses_a_request_the_model_does_not_allow() {
    let client = Client::new();
    let workload_id = created_workload(&client, "profile-associate-validation", &["wellarchitected"]).await;

    let rows = [
        ("an empty ProfileArns list", json!({ "ProfileArns": [] })),
        ("no ProfileArns member at all", json!({})),
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
        let answer = associate_profiles(&client, &workload_id, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            profiles_of(&client, &workload_id, case).await.is_empty(),
            "case {case:?}: a refused request must leave the workload with no association"
        );
    }
}

/// A `ProfileArns` longer than the per-request quota is refused on its LENGTH alone, with
/// the modelled 400 `ValidationException` naming the member: the member publishes no
/// modelled maximum (`smithy_query(query="shape", shape="ProfileArns")`), so the quota is
/// the service's, and it is checked before any of the ARNs is looked at. Both operations
/// that carry the member apply it, and the list is accepted ON the quota.
#[tokio::test]
async fn a_profile_arns_list_past_the_per_request_quota_is_refused_on_its_length() {
    // The service's own ceiling, so the test cannot drift from the handler.
    const QUOTA: usize = service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_PROFILE_ARNS_PER_REQUEST;

    let client = Client::new();
    let workload_id = created_workload(&client, "profile-arns-quota", &["wellarchitected"]).await;
    let (arn, version) = created_profile(&client, "profile-arns-quota").await;

    // ON the quota: the same ARN repeated up to the quota is one association, so the
    // request is served.
    let on_quota = associate_profiles(
        &client,
        &workload_id,
        json!({ "ProfileArns": vec![arn.clone(); QUOTA] }),
    )
    .await;
    assert_eq!(
        on_quota.status, 200,
        "case \"{QUOTA} ARNs\": a list on the quota must be accepted, got {on_quota:?}"
    );
    assert_eq!(
        profiles_of(&client, &workload_id, "on the quota").await,
        vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
        "case \"{QUOTA} ARNs\": the workload must report the one association"
    );

    // One past it, and far past it, on BOTH operations that carry the member.
    for named in [QUOTA + 1, QUOTA * 10] {
        for path in ["associateProfiles", "disassociateProfiles"] {
            let case = format!("{named} ARNs to {path}");
            let answer = client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload_id}/{path}"),
                    Some(json!({ "ProfileArns": vec![arn.clone(); named] })),
                )
                .await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );
            assert_eq!(
                answer
                    .body
                    .get("Fields")
                    .and_then(Value::as_array)
                    .map(|fields| fields
                        .iter()
                        .filter_map(|field| field.get("Name").and_then(Value::as_str))
                        .collect::<Vec<_>>()),
                Some(vec!["ProfileArns"]),
                "case {case:?}: Fields must name the member that carried the ARNs, got \
                 {answer:?}"
            );
            assert!(
                member(&answer, "Message").contains(&format!("at most {QUOTA} profile ARNs")),
                "case {case:?}: the Message must state the quota, got {answer:?}"
            );
        }
    }

    // Nothing the refused requests named was added or removed.
    assert_eq!(
        profiles_of(&client, &workload_id, "after the refusals").await,
        vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
        "a refused request must leave the workload's association exactly as it stood"
    );
}

/// `CreateWorkload` associates the profile its `ProfileArns` names with the workload it
/// creates — the same association `AssociateProfiles` makes, pinned the same way — and a
/// `ProfileArn` naming no stored profile is the same 404 as it is there.
#[tokio::test]
async fn creating_a_workload_with_a_profile_associates_it() {
    let client = Client::new();
    let (arn, version) = created_profile(&client, "profile-at-create").await;

    // Rows differ along WHICH profile the create names: one the service minted, and one
    // that names nothing.
    let absent = format!(
        "arn:aws:wellarchitected:{REGION}:{}:profile/noprofile9",
        engine::emulator_account_id()
    );
    for (case, named, expected_status) in [
        ("a stored profile", arn.clone(), 200),
        ("an ARN naming no stored profile", absent.clone(), 404),
    ] {
        let mut request = minimal_create(
            &format!("workload-with-{}", case.replace(' ', "-")),
            &format!("tok-create-{}", case.replace(' ', "-")),
        );
        request["ProfileArns"] = json!([named]);
        let answer = client.create(request).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_status != 200 {
            assert_not_found(
                case,
                &answer,
                PROFILE_TYPE,
                &named,
                &format!("profile with ARN {named}"),
            );
            continue;
        }
        let workload_id = created_id(&answer, case);
        assert_eq!(
            profiles_of(&client, &workload_id, case).await,
            vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
            "case {case:?}: a created workload must report the association it was given"
        );
        assert_eq!(
            lens_review_profiles(&client, &workload_id, "wellarchitected", case).await,
            vec![json!({ "ProfileArn": arn, "ProfileVersion": version })],
            "case {case:?}: its lens review must report the same association"
        );
        // The association survives a disassociation of something else being impossible:
        // what it does take is the profile's own ARN, which `DisassociateProfiles` removes.
        let removed = client
            .send(
                "PATCH",
                &format!("/workloads/{workload_id}/disassociateProfiles"),
                Some(json!({ "ProfileArns": [arn.clone()] })),
            )
            .await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected DisassociateProfiles 200, got {removed:?}"
        );
        assert!(
            profiles_of(&client, &workload_id, case).await.is_empty(),
            "case {case:?}: the association a create made is removable like any other"
        );
    }
}
