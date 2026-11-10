// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the wording of the profile-not-found refusal.
//!
//! The rule under test: every operation that addresses one profile by `ProfileId` refuses
//! an id the named domain does not hold with `ResourceNotFoundException` (404) whose
//! `Message` is the service's FIXED profile-not-found sentence, whose only variable part
//! is the `ProfileId` the request carried — the domain the id was looked for in is not
//! interpolated, even though every one of these operations carries it in its path.
//!
//! The operations covered are the ones whose input takes a `ProfileId`/`MainProfileId`
//! and looks it up in one domain: `UpdateProfile` (PUT /domains/{DomainName}/profiles),
//! `DeleteProfile` (POST /domains/{DomainName}/profiles/delete) and `MergeProfiles`
//! (POST /domains/{DomainName}/profiles/objects/merge), per
//! `smithy_query(query="operation", operation="UpdateProfile,DeleteProfile,MergeProfiles")`.
//! `BatchGetProfile` is not among them: it reports a missing id as a per-item
//! `BatchGetProfileError`, with its own fixed sentence, and is covered by
//! `tests/all/batch_get_profile.rs`.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, Emulator};
use crate::profile_object_types::error_message;

/// The domain the refusals are asked for. Its name is distinctive so an assertion can
/// check the refusal does NOT carry it.
const DOMAIN: &str = "wordingdom";

/// Another domain, so one row can use an id that was really minted but belongs elsewhere.
const OTHER_DOMAIN: &str = "elsewheredom";

/// The service's fixed profile-not-found sentence, with the request's `ProfileId` as its
/// one variable part.
fn expected_message(profile_id: &str) -> String {
    format!("Profile with profileId {profile_id} is not found in the domain")
}

/// Every per-id profile lookup, for one id in one domain: the operation's name and the
/// `(status, body)` it answered. `anchor` is a profile the domain really holds, so a
/// `MergeProfiles` row can put the unknown id on either side of the merge.
async fn lookups(
    emulator: &Emulator,
    domain: &str,
    profile_id: &str,
    anchor: &str,
) -> Vec<(&'static str, u16, Value)> {
    let (update_status, update_body) = emulator
        .send(
            "PUT",
            &format!("/domains/{domain}/profiles"),
            Some(json!({ "ProfileId": profile_id, "FirstName": "Overwritten" })),
        )
        .await;
    let (delete_status, delete_body) = emulator.delete_profile(domain, profile_id).await;
    let (main_status, main_body) = emulator
        .post(
            &format!("/domains/{domain}/profiles/objects/merge"),
            json!({ "MainProfileId": profile_id, "ProfileIdsToBeMerged": [anchor] }),
        )
        .await;
    let (merged_status, merged_body) = emulator
        .post(
            &format!("/domains/{domain}/profiles/objects/merge"),
            json!({ "MainProfileId": anchor, "ProfileIdsToBeMerged": [profile_id] }),
        )
        .await;
    vec![
        ("UpdateProfile", update_status, update_body),
        ("DeleteProfile", delete_status, delete_body),
        ("MergeProfiles/MainProfileId", main_status, main_body),
        (
            "MergeProfiles/ProfileIdsToBeMerged",
            merged_status,
            merged_body,
        ),
    ]
}

/// The rule, over a table whose rows differ in WHERE the unknown id comes from and what
/// it looks like — hand-written ids at both ends of the modelled `^[a-f0-9]{32}$` pattern
/// (`UpdateProfileRequest$ProfileId`, a `uuid`), an id the service itself minted and that
/// was then deleted, and an id the service minted in a different domain — crossed with
/// every operation that looks a profile up by id.
#[tokio::test]
async fn a_profile_the_domain_does_not_hold_is_refused_with_the_fixed_sentence() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;
    emulator.create_domain(OTHER_DOMAIN, 90).await;

    // A profile the domain really holds, so the merge rows refuse for the unknown id and
    // not for the other side of the merge.
    let anchor = emulator
        .create_profile(DOMAIN, json!({ "FirstName": "Ada" }))
        .await;

    // An id the service minted in this domain and that was then deleted: its own delete
    // succeeded, so every later lookup of it is the same absence as a made-up id.
    let deleted = emulator
        .create_profile(DOMAIN, json!({ "FirstName": "Grace" }))
        .await;
    let (status, body) = emulator.delete_profile(DOMAIN, &deleted).await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteProfile 200 for {deleted}, got {status} {body:?}"
    );

    // An id the service minted, but in the other domain, so it is a real profile id that
    // this domain does not hold.
    let foreign = emulator
        .create_profile(OTHER_DOMAIN, json!({ "FirstName": "Alan" }))
        .await;

    // (case, the id the request carries)
    let rows: Vec<(&str, String)> = vec![
        ("the lowest id the pattern allows", "0".repeat(32)),
        ("the highest id the pattern allows", "f".repeat(32)),
        (
            "an id mixing digits and hex letters",
            "a1b2c3d4e5f60718293a4b5c6d7e8f90".to_string(),
        ),
        ("an id this domain minted and then deleted", deleted.clone()),
        ("an id minted in another domain", foreign.clone()),
    ];

    for (case, profile_id) in rows {
        for (operation, status, body) in lookups(&emulator, DOMAIN, &profile_id, &anchor).await {
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {case:?} on {operation}: expected 404 ResourceNotFoundException, \
                 got {status} {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                expected_message(&profile_id),
                "case {case:?} on {operation}: expected the fixed profile-not-found \
                 sentence, got {message:?}"
            );
            assert!(
                !message.contains(DOMAIN),
                "case {case:?} on {operation}: expected the refusal not to name the \
                 domain {DOMAIN:?}, got {message:?}"
            );
        }
    }

    // None of the refusals touched the profile the domain does hold.
    let (status, read) = emulator
        .post(
            &format!("/domains/{DOMAIN}/batch-get-profiles"),
            json!({ "ProfileIds": [anchor.clone()] }),
        )
        .await;
    let profiles = read
        .get("Profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        (status, profiles.len()),
        (200, 1),
        "after the refusals: expected the anchor profile to still be readable, \
         got {status} {read:?}"
    );
}

/// The domain's own absence is a DIFFERENT refusal: when the domain is not stored either,
/// the operation reports the domain it could not find and never reaches the profile
/// lookup, so the profile-not-found sentence is not what comes back.
#[tokio::test]
async fn an_absent_domain_is_refused_for_the_domain_not_for_the_profile() {
    let emulator = Emulator::new();
    emulator.create_domain(DOMAIN, 90).await;
    let anchor = emulator
        .create_profile(DOMAIN, json!({ "FirstName": "Ada" }))
        .await;

    let profile_id = "b".repeat(32);
    for (operation, status, body) in lookups(&emulator, "nosuchdomain", &profile_id, &anchor).await
    {
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case \"absent domain\" on {operation}: expected 404 ResourceNotFoundException, \
             got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, "Domain nosuchdomain is not found",
            "case \"absent domain\" on {operation}: expected the domain-not-found \
             sentence, got {message:?}"
        );
    }
}
