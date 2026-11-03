// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteProfile`.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns. The profiles are CREATED through `CreateProfile`, so what
//! the delete removes is the record that create stored.
//!
//! RULE under test: `DeleteProfile` removes the addressed AS2 profile from the store, so
//! afterwards every reader of it (`DescribeProfile`, `ListProfiles`) answers as if it had
//! never been created and the store capacity it held is released, while every other
//! stored profile is left exactly as it was; a `ProfileId` the store does not hold —
//! never minted, or already deleted — is refused with this service's
//! `ResourceNotFoundException` and removes nothing.
//!
//! Each suite asserts that rule over a TABLE of inputs rather than one recorded call: the
//! rows differ along the `ProfileType` value, the form and length of `As2Id`, how many
//! `CertificateIds` and `Tags` the profile carries, which Region it was created in, which
//! position in the store's order it occupies, and how many profiles are deleted at once.

use serde_json::{json, Value};

use service_com_amazonaws_transfer_transferservice::state::MAX_STORED_PROFILES;

use crate::harness::{
    assert_members, drain_pages, is_modelled_profile_id, member_of, Emulator, CERTIFICATE_IDS,
    PROFILE_TYPES, REGION,
};

/// Syntactically valid `ProfileId`s (`len 19..19`, `^p-([0-9a-f]{17})$`) that no
/// `CreateProfile` ever minted: the low end of the id space, a mid value, and the high
/// end.
const NEVER_MINTED: [&str; 3] = [
    "p-00000000000000000",
    "p-0000000000000f00d",
    "p-fffffffffffffffff",
];

/// One row of the profile table: what it varies, the Region it is created in, and the
/// `CreateProfile` body.
struct Row {
    case: &'static str,
    region: &'static str,
    request: Value,
}

/// The table every suite below creates: the rows differ along the `ProfileType` value
/// (both members of the modelled set), the length and character content of `As2Id` (both
/// modelled bounds and the printable punctuation its pattern admits), the number of
/// `CertificateIds` (none, one, all three) and of `Tags` (none, one, two), and the Region
/// the create was signed for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "LOCAL with nothing optional",
            region: REGION,
            request: json!({"As2Id": "local-one", "ProfileType": PROFILE_TYPES[0]}),
        },
        Row {
            case: "PARTNER with nothing optional",
            region: REGION,
            request: json!({"As2Id": "partner-one", "ProfileType": PROFILE_TYPES[1]}),
        },
        Row {
            case: "LOCAL with the shortest legal As2Id and one certificate",
            region: REGION,
            request: json!({
                "As2Id": "l",
                "ProfileType": PROFILE_TYPES[0],
                "CertificateIds": [CERTIFICATE_IDS[1]],
            }),
        },
        Row {
            case: "PARTNER with the longest legal As2Id, three certificates and two tags",
            region: REGION,
            // `As2Id` is modelled `len 1..128`: this is its upper bound.
            request: json!({
                "As2Id": "a".repeat(128),
                "ProfileType": PROFILE_TYPES[1],
                "CertificateIds": CERTIFICATE_IDS,
                "Tags": [
                    {"Key": "team", "Value": "integration"},
                    {"Key": "env", "Value": "test"},
                ],
            }),
        },
        Row {
            case: "LOCAL whose As2Id carries the punctuation the pattern admits",
            region: REGION,
            request: json!({
                "As2Id": "AS2-From_v1.0+{test}/[x]",
                "ProfileType": PROFILE_TYPES[0],
                "Tags": [{"Key": "env", "Value": "prod"}],
            }),
        },
        Row {
            case: "PARTNER created in another Region",
            region: "ap-southeast-2",
            request: json!({"As2Id": "partner-far", "ProfileType": PROFILE_TYPES[1]}),
        },
    ]
}

/// `ProfileType` as a row's create named it.
fn profile_type_of(request: &Value) -> &str {
    member_of(request, "ProfileType")
        .as_str()
        .expect("every row names a ProfileType")
}

/// What `DescribeProfile` must report for the profile a row created: the create's own
/// members, where a list the create did not name reads back as the EMPTY list —
/// `CertificateIds` and `Tags` are collections the profile always holds.
fn expected_members(request: &Value) -> Value {
    json!({
        "As2Id": member_of(request, "As2Id"),
        "ProfileType": profile_type_of(request),
        "CertificateIds": requested_list(request, "CertificateIds"),
        "Tags": requested_list(request, "Tags"),
    })
}

/// A list member of a create request as the profile's reader reports it back: the list
/// the request named, or the EMPTY list when it named none.
fn requested_list(request: &Value, member: &str) -> Value {
    match member_of(request, member) {
        Value::Null => json!([]),
        supplied => supplied.clone(),
    }
}

/// Create one profile from `request`, signed for `region`, and return its minted id.
async fn create_profile_in(emulator: &Emulator, region: &str, case: &str, request: &Value) -> String {
    let answer = emulator.call_in(region, "CreateProfile", request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateProfile 200, got {} with body {}",
        answer.status, answer.body
    );
    let profile_id = member_of(&answer.body, "ProfileId")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateProfile returned no ProfileId: {}",
                answer.body
            )
        })
        .to_string();
    assert!(
        is_modelled_profile_id(&profile_id),
        "case {case:?}: expected a ProfileId matching ^p-([0-9a-f]{{17}})$, got {profile_id:?}"
    );
    profile_id
}

/// Delete one profile and assert the delete succeeded with the modelled EMPTY output
/// (`DeleteProfile`'s output shape is `Unit`, so it carries no member at all).
async fn delete_profile(emulator: &Emulator, case: &str, profile_id: &str) {
    let answer = emulator
        .call("DeleteProfile", json!({"ProfileId": profile_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteProfile 200, got {} with body {}",
        answer.status, answer.body
    );
    let members = answer.body.as_object().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a JSON object body, got {}",
            answer.body
        )
    });
    assert!(
        members.is_empty(),
        "case {case:?}: DeleteProfile models an empty output, got the members {members:?}"
    );
}

/// Every `ProfileId` `ListProfiles` reports for `base`, through the shared paging drain
/// (it bounds the page COUNT, so a listing that never terminates fails instead of
/// hanging).
async fn listed_profile_ids(emulator: &Emulator, base: &Value) -> Vec<String> {
    let (_pages, ids) = drain_pages(emulator, "ListProfiles", base, "Profiles", "ProfileId", None).await;
    ids
}

/// Assert `profile_id` is gone from every reader of a profile: `DescribeProfile` refuses
/// it with this service's not-found refusal, and `ListProfiles` does not list it —
/// unfiltered or under either `ProfileType` filter.
async fn assert_absent(emulator: &Emulator, case: &str, profile_id: &str) {
    let answer = emulator
        .call("DescribeProfile", json!({"ProfileId": profile_id}))
        .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected DescribeProfile of a deleted profile to answer HTTP 400, \
         got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException after the delete, got {}",
        answer.body
    );

    let mut listings = vec![json!({})];
    for profile_type in PROFILE_TYPES {
        listings.push(json!({"ProfileType": profile_type}));
    }
    for base in &listings {
        let listed = listed_profile_ids(emulator, base).await;
        assert!(
            !listed.contains(&profile_id.to_string()),
            "case {case:?}: expected ListProfiles {base} not to list the deleted \
             {profile_id:?}, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A delete removes the addressed record and nothing else.
// ---------------------------------------------------------------------------

/// RULE: `DeleteProfile` removes exactly the addressed profile — afterwards no reader
/// answers for it — and leaves every other stored profile readable, listed and unchanged
/// member for member.
///
/// The rows span the create class (see [`rows`]) and are deleted in an order that is not
/// their stored (`ProfileId`) order, so a handler that removed the first or the last
/// record instead of the addressed one fails.
#[tokio::test]
async fn deleting_a_profile_removes_only_that_profile_from_every_reader() {
    let emulator = Emulator::new();
    let table = rows();

    let mut live: Vec<(&Row, String)> = Vec::new();
    for row in &table {
        let profile_id = create_profile_in(&emulator, row.region, row.case, &row.request).await;
        live.push((row, profile_id));
    }

    // Delete order: a permutation of every row index DERIVED from the table's length —
    // the odd positions front-to-back, then the even ones back-to-front — so it is
    // never the stored (`ProfileId`) order, never starts at the first record, and still
    // covers every row when the table grows.
    let delete_order: Vec<usize> = (0..table.len())
        .filter(|index| index % 2 == 1)
        .chain((0..table.len()).filter(|index| index % 2 == 0).rev())
        .collect();
    assert_eq!(
        delete_order.len(),
        table.len(),
        "the delete order must visit every row of the table once, got {delete_order:?}"
    );
    let mut deleted: Vec<String> = Vec::new();

    for index in delete_order {
        let (row, profile_id) = (live[index].0, live[index].1.clone());
        delete_profile(&emulator, row.case, &profile_id).await;
        deleted.push(profile_id.clone());
        assert_absent(&emulator, row.case, &profile_id).await;

        // Every profile not yet deleted is still there, unchanged member for member.
        let listed = listed_profile_ids(&emulator, &json!({})).await;
        for (other_row, other) in &live {
            if deleted.contains(other) {
                continue;
            }
            let answer = emulator
                .call("DescribeProfile", json!({"ProfileId": other}))
                .await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected the surviving profile of {:?} to still describe, \
                 got {} with body {}",
                row.case, other_row.case, answer.status, answer.body
            );
            let described = answer.get("Profile");
            assert_members(other_row.case, described, &expected_members(&other_row.request));
            let reported = member_of(described, "ProfileId").as_str().unwrap_or_default();
            assert_eq!(
                reported, other,
                "case {:?}: expected the surviving profile of {:?} to report {other:?}, \
                 got {reported:?}",
                row.case, other_row.case
            );
            assert!(
                listed.contains(other),
                "case {:?}: expected ListProfiles to still list {other:?} of {:?}, got {listed:?}",
                row.case,
                other_row.case
            );
        }
    }

    // With every row deleted the store is empty again.
    let listed = listed_profile_ids(&emulator, &json!({})).await;
    assert!(
        listed.is_empty(),
        "case \"after deleting every profile\": expected an empty listing, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A profile the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: `DeleteProfile` for a `ProfileId` the store does not hold — one never minted, or
/// one already deleted — answers this service's not-found refusal (HTTP 400
/// `ResourceNotFoundException`, the capitalized `ResourceType`, the requested identifier
/// in `Resource` alone and a fixed generic `Message` carrying no identifier) and removes
/// nothing.
///
/// The rows are the ends and the middle of the id space that was never minted, plus a
/// profile that WAS created and then deleted (twice over, so a repeat of the refusal is
/// covered too), so the refusal does not depend on how the id came to be absent.
#[tokio::test]
async fn deleting_a_profile_the_store_does_not_hold_answers_the_generic_not_found() {
    let emulator = Emulator::new();

    // A store that DOES hold a profile, so the refusal is about the id, not emptiness.
    let survivor_row = &rows()[0];
    let survivor =
        create_profile_in(&emulator, survivor_row.region, survivor_row.case, &survivor_row.request)
            .await;

    let doomed_row = &rows()[3];
    let once_created =
        create_profile_in(&emulator, doomed_row.region, doomed_row.case, &doomed_row.request).await;
    delete_profile(&emulator, doomed_row.case, &once_created).await;

    let mut absent: Vec<String> = NEVER_MINTED.iter().map(|id| (*id).to_string()).collect();
    absent.push(once_created.clone());
    // The same already-deleted id a second time: the refusal is not a one-shot.
    absent.push(once_created);

    let mut messages: Vec<String> = Vec::new();

    for profile_id in &absent {
        assert!(
            is_modelled_profile_id(profile_id),
            "the probe id {profile_id:?} must satisfy ProfileId's modelled pattern"
        );
        let answer = emulator
            .call("DeleteProfile", json!({"ProfileId": profile_id}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {profile_id:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {profile_id:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_members(
            profile_id,
            &answer.body,
            &json!({"Resource": profile_id, "ResourceType": "Profile"}),
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty() && !message.contains(profile_id.as_str()),
            "case {profile_id:?}: expected a non-empty Message carrying no identifier, \
             got {message:?}"
        );
        messages.push(message);
    }

    for message in &messages {
        assert_eq!(
            message, &messages[0],
            "expected one fixed Profile not-found message, got {message:?} beside {:?}",
            messages[0]
        );
    }

    // None of the refusals removed the profile the store does hold.
    let listed = listed_profile_ids(&emulator, &json!({})).await;
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "case \"after the refused deletes\": expected only {survivor:?} to remain, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// A delete releases the store's capacity.
// ---------------------------------------------------------------------------

/// RULE: the entry a deleted profile held is RELEASED, so a store at its bound takes
/// exactly as many further creates as were deleted — no more. The bound itself
/// ([`MAX_STORED_PROFILES`]) is read from the crate, so the test cannot drift from it.
///
/// The rows differ along how many profiles are deleted before creating again (one, then
/// several), because the released capacity is a count rather than a single slot.
#[tokio::test]
async fn deleting_a_profile_releases_the_store_capacity_it_held() {
    let emulator = Emulator::new();

    /// One create, distinct in `As2Id` per slot so no row is a repeat of another.
    fn request(slot: usize) -> Value {
        json!({
            "As2Id": format!("party-{slot:04}"),
            "ProfileType": PROFILE_TYPES[slot % PROFILE_TYPES.len()],
        })
    }

    let mut stored: Vec<String> = Vec::with_capacity(MAX_STORED_PROFILES);
    for slot in 0..MAX_STORED_PROFILES {
        let case = format!("profile {slot} of the bound");
        stored.push(create_profile_in(&emulator, REGION, &case, &request(slot)).await);
    }

    let mut next_slot = MAX_STORED_PROFILES;

    for released in [1usize, 3] {
        let case = format!("after releasing {released} of the bound");

        // At the bound, a create is refused with the modelled server-side refusal.
        let refused = emulator
            .call("CreateProfile", request(next_slot))
            .await;
        assert_eq!(
            refused.status, 503,
            "case {case:?}: expected a create at the bound to answer HTTP 503, got {} with \
             body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ServiceUnavailableException",
            "case {case:?}: expected ServiceUnavailableException, got {}",
            refused.body
        );

        for _ in 0..released {
            let victim = stored.remove(0);
            delete_profile(&emulator, &case, &victim).await;
            assert_absent(&emulator, &case, &victim).await;
        }

        // Exactly `released` creates fit, and the next one is refused again.
        for _ in 0..released {
            let case = format!("{case}: create into the released capacity");
            stored.push(create_profile_in(&emulator, REGION, &case, &request(next_slot)).await);
            next_slot += 1;
        }
        let refused = emulator.call("CreateProfile", request(next_slot)).await;
        assert_eq!(
            refused.status, 503,
            "case {case:?}: expected the create past the refilled bound to answer HTTP 503, \
             got {} with body {}",
            refused.status, refused.body
        );
    }

    // The store is back at its bound, and every record in it is still listed once.
    let listed = listed_profile_ids(&emulator, &json!({})).await;
    assert_eq!(
        listed.len(),
        MAX_STORED_PROFILES,
        "case \"the refilled store\": expected {MAX_STORED_PROFILES} profiles, got {}",
        listed.len()
    );
    let mut unique = listed.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        listed.len(),
        "case \"the refilled store\": expected every listed ProfileId to be distinct, \
         got {listed:?}"
    );
}
