// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetProfile`.
//!
//! The rule under test: a batch read answers each DISTINCT requested id out of the
//! domain's stored profiles — an id the domain holds is reported in `Profiles` exactly as
//! it was stored, an id it does not hold is reported as its own entry in `Errors` whose
//! `Code` is the status of the equivalent single-item failure (404), whose `ProfileId`
//! echoes the id that failed and whose `Message` is the service's fixed sentence for that
//! failure class, interpolating neither the requested id nor the domain name — so an
//! unknown id never fails the whole batch, and the batch is partial-success. Both lists
//! follow the order the request named the ids. Only
//! the domain named in the path is searched, and a domain that is not stored is
//! `ResourceNotFoundException` (404) while an input outside a modelled constraint is
//! `BadRequestException` (400).
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::domain_profile_lifecycle::{error_code, Emulator};

/// The per-item `Code` `BatchGetProfileError` carries for a requested profile the domain
/// does not hold.
const NOT_FOUND_CODE: &str = "404";

/// The per-item `Message` `BatchGetProfileError` carries for a requested profile the
/// domain does not hold: the service's fixed sentence for that failure class, the same
/// text for every requested id and every domain.
const NOT_FOUND_MESSAGE: &str = "Profile was not found in domain";

/// `BatchGetProfileIdList` is `len 1..20`, so 20 ids is the published maximum.
const MAX_PROFILE_IDS: usize = 20;

/// A `ProfileId` that satisfies the modelled pattern `^[a-f0-9]{32}$` but that no create
/// in these tests minted, so the store cannot hold it.
fn unknown_id(seed: u64) -> String {
    format!("{:032x}", 0xffff_0000_0000_0000u64 + seed)
}

/// `BatchGetProfile` over the wire: `POST /domains/{DomainName}/batch-get-profiles`.
async fn batch_get(emulator: &Emulator, domain: &str, ids: &[String]) -> (u16, Value) {
    emulator
        .post(
            &format!("/domains/{domain}/batch-get-profiles"),
            json!({ "ProfileIds": ids }),
        )
        .await
}

/// The `ProfileId`s of a response's `Profiles`, in the order the response listed them.
fn returned_ids(body: &Value) -> Vec<String> {
    body.get("Profiles")
        .and_then(Value::as_array)
        .map(|profiles| {
            profiles
                .iter()
                .map(|profile| {
                    profile
                        .get("ProfileId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A response's `Errors`, in the order the response listed them, as
/// `(ProfileId, Code, Message)` — the order is part of the rule, so it is asserted
/// rather than collapsed into a set.
fn returned_errors(body: &Value) -> Vec<(String, String, String)> {
    body.get("Errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .map(|entry| {
                    let member = |name: &str| {
                        entry
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("ProfileId"), member("Code"), member("Message"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Three profiles in one domain and one in another, so a case can mix stored ids, ids of
/// another domain's profiles and ids nothing stored. Returns the ids of the three
/// profiles of `domain`, and the id of the profile of `other`, beside the request body
/// each id was created from.
async fn seed(
    emulator: &Emulator,
    domain: &str,
    other: &str,
) -> (Vec<String>, String, BTreeMap<String, Value>) {
    emulator.create_domain(domain, 90).await;
    emulator.create_domain(other, 90).await;

    let requests = [
        json!({
            "FirstName": "Ada",
            "LastName": "Lovelace",
            "EmailAddress": "ada@example.com",
            "Attributes": { "tier": "gold" },
        }),
        json!({ "FirstName": "Grace", "PhoneNumber": "+15550100" }),
        json!({ "AccountNumber": "acct-3", "PartyType": "INDIVIDUAL" }),
    ];

    let mut created = BTreeMap::new();
    let mut ids = Vec::new();
    for request in requests {
        let id = emulator.create_profile(domain, request.clone()).await;
        created.insert(id.clone(), request);
        ids.push(id);
    }

    let elsewhere = json!({ "FirstName": "Alan", "EmailAddress": "alan@example.com" });
    let other_id = emulator.create_profile(other, elsewhere.clone()).await;
    created.insert(other_id.clone(), elsewhere);

    (ids, other_id, created)
}

/// One row of the table below: the ids one request carries, and the answer the rule owes
/// it — which of them belong in `Profiles`, and which in `Errors`.
struct Case {
    name: &'static str,
    ids: Vec<String>,
    found: Vec<String>,
    missing: Vec<String>,
}

/// The rule over a table of request shapes: which ids are stored, how many, repeated,
/// and stored in ANOTHER domain. Each row states the ids it expects in `Profiles` and the
/// ids it expects in `Errors`.
#[tokio::test]
async fn a_batch_answers_each_distinct_id_out_of_the_domains_stored_profiles() {
    let emulator = Emulator::new();
    let (stored, other_id, created) = seed(&emulator, "batchdom", "otherdom").await;
    let (first, second, third) = (
        stored[0].clone(),
        stored[1].clone(),
        stored[2].clone(),
    );

    // Twenty ids — the modelled maximum — of which three are stored.
    let at_maximum: Vec<String> = stored
        .iter()
        .cloned()
        .chain((0..MAX_PROFILE_IDS - stored.len()).map(|seed| unknown_id(seed as u64)))
        .collect();

    let cases = vec![
        Case {
            name: "one-stored-id",
            ids: vec![first.clone()],
            found: vec![first.clone()],
            missing: vec![],
        },
        Case {
            name: "every-id-stored",
            ids: vec![first.clone(), second.clone(), third.clone()],
            found: vec![first.clone(), second.clone(), third.clone()],
            missing: vec![],
        },
        Case {
            name: "stored-and-unknown-mixed",
            ids: vec![first.clone(), unknown_id(101), second.clone()],
            found: vec![first.clone(), second.clone()],
            missing: vec![unknown_id(101)],
        },
        Case {
            name: "no-id-stored",
            ids: vec![unknown_id(102), unknown_id(103)],
            found: vec![],
            missing: vec![unknown_id(102), unknown_id(103)],
        },
        Case {
            name: "stored-id-repeated",
            ids: vec![first.clone(), first.clone()],
            found: vec![first.clone()],
            missing: vec![],
        },
        Case {
            name: "unknown-id-repeated",
            ids: vec![unknown_id(104), unknown_id(104)],
            found: vec![],
            missing: vec![unknown_id(104)],
        },
        Case {
            name: "id-stored-in-another-domain",
            ids: vec![other_id.clone()],
            found: vec![],
            missing: vec![other_id.clone()],
        },
        Case {
            name: "own-id-beside-another-domains-id",
            ids: vec![third.clone(), other_id.clone()],
            found: vec![third.clone()],
            missing: vec![other_id.clone()],
        },
        Case {
            name: "twenty-ids-at-the-modelled-maximum",
            ids: at_maximum.clone(),
            found: stored.clone(),
            missing: at_maximum[stored.len()..].to_vec(),
        },
    ];

    for Case { name: case, ids, found: expected_found, missing: expected_missing } in cases {
        let (status, body) = batch_get(&emulator, "batchdom", &ids).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            returned_ids(&body),
            expected_found,
            "case {case:?}: expected Profiles for {expected_found:?}, got {body:?}"
        );

        let errors = returned_errors(&body);
        assert_eq!(
            errors
                .iter()
                .map(|(profile_id, _, _)| profile_id.clone())
                .collect::<Vec<String>>(),
            expected_missing,
            "case {case:?}: expected Errors for {expected_missing:?}, in that order, got {body:?}"
        );
        for (profile_id, code, message) in &errors {
            assert_eq!(
                code, NOT_FOUND_CODE,
                "case {case:?}: expected Code {NOT_FOUND_CODE} for {profile_id}, got {code:?}"
            );
            assert_eq!(
                message, NOT_FOUND_MESSAGE,
                "case {case:?}: expected the fixed Message {NOT_FOUND_MESSAGE:?} for {profile_id}, got {message:?}"
            );
        }

        // A returned profile carries what the create stored, not a shape built here.
        for returned in body
            .get("Profiles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let profile_id = returned
                .get("ProfileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let request = created
                .get(&profile_id)
                .unwrap_or_else(|| panic!("case {case:?}: unexpected ProfileId {profile_id:?}"));
            for (member, expected) in request.as_object().expect("an object request") {
                assert_eq!(
                    returned.get(member),
                    Some(expected),
                    "case {case:?}: expected {member} = {expected:?} as CreateProfile stored it, got {:?}",
                    returned.get(member)
                );
            }
        }
    }
}

/// The batch is a READ: repeating it answers the same, and an id it reported as failed is
/// not created by having been asked for.
#[tokio::test]
async fn a_batch_read_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let (stored, _other_id, _created) = seed(&emulator, "readdom", "elsewhere").await;
    let ids = vec![stored[0].clone(), unknown_id(7)];

    let (first_status, first_body) = batch_get(&emulator, "readdom", &ids).await;
    let (second_status, second_body) = batch_get(&emulator, "readdom", &ids).await;
    assert_eq!(
        (first_status, &first_body),
        (second_status, &second_body),
        "case \"repeated-batch\": a read must answer the same twice, got {first_body:?} then {second_body:?}"
    );

    // The id the batch failed on is still absent, and the profiles it found are still
    // found by a different reader.
    let (status, searched) = emulator
        .post(
            "/domains/readdom/profiles/search",
            json!({ "KeyName": "_profileId", "Values": [unknown_id(7)] }),
        )
        .await;
    assert_eq!(
        (status, searched.get("Items")),
        (200, Some(&json!([]))),
        "case \"failed-id-not-created\": expected an empty search, got {status} {searched:?}"
    );

    let (status, searched) = emulator
        .post(
            "/domains/readdom/profiles/search",
            json!({ "KeyName": "_profileId", "Values": [stored[0].clone()] }),
        )
        .await;
    let found = searched
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        (status, found),
        (200, 1),
        "case \"found-id-still-stored\": expected the profile to still be stored, got {status} {searched:?}"
    );
}

/// The per-item failure reason is the service's FIXED sentence: the same `Message` for
/// every unknown id and in every domain, with nothing interpolated into it.
///
/// The table varies what a synthesized sentence would have leaked — the domain name (its
/// length, its case and its separators, up to `name`'s modelled maximum of 64) and the
/// shape of the requested id inside the modelled pattern `^[a-f0-9]{32}$` (all-zero,
/// all-`f`, mixed hex), as well as how many ids fail and where in the batch they sit.
#[tokio::test]
async fn an_unresolvable_id_reports_the_services_fixed_failure_message() {
    let emulator = Emulator::new();

    // `DomainName` targets `name`: len 1..64, pattern ^[a-zA-Z0-9_-]+$ (smithy_query
    // shape=name / BatchGetProfileRequest$DomainName).
    const MAX_DOMAIN_NAME: usize = 64;
    let long_domain = format!("msg-{}", "x".repeat(MAX_DOMAIN_NAME - 4));
    let domains = [
        "msgdom",
        "msg_Dom-9",
        long_domain.as_str(),
    ];

    // Ids inside the modelled `ProfileId` pattern that nothing created.
    let absent_ids = [
        ("all-zero-hex", "0".repeat(32)),
        ("all-f-hex", "f".repeat(32)),
        ("mixed-hex", "0a1b2c3d4e5f60718293a4b5c6d7e8f9".to_string()),
    ];

    for domain in domains {
        emulator.create_domain(domain, 90).await;
        let stored = emulator
            .create_profile(domain, json!({ "FirstName": "Ida" }))
            .await;

        for (id_case, absent) in &absent_ids {
            // The unknown id alone, and the same id beside a stored one, so the entry is
            // asserted both in an all-failed batch and in a partial-success batch.
            let batches: [(&str, Vec<String>); 3] = [
                ("alone", vec![absent.clone()]),
                ("after-a-stored-id", vec![stored.clone(), absent.clone()]),
                ("before-a-stored-id", vec![absent.clone(), stored.clone()]),
            ];
            for (position, ids) in batches {
                let case = format!("{domain}/{id_case}/{position}");
                let (status, body) = batch_get(&emulator, domain, &ids).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body:?}"
                );

                let errors = returned_errors(&body);
                assert_eq!(
                    errors.len(),
                    1,
                    "case {case:?}: expected exactly one Errors entry, got {body:?}"
                );
                let (profile_id, code, message) = &errors[0];
                assert_eq!(
                    profile_id, absent,
                    "case {case:?}: expected ProfileId {absent:?}, got {profile_id:?}"
                );
                assert_eq!(
                    code, NOT_FOUND_CODE,
                    "case {case:?}: expected Code {NOT_FOUND_CODE:?}, got {code:?}"
                );
                assert_eq!(
                    message, NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the fixed Message {NOT_FOUND_MESSAGE:?}, got {message:?}"
                );
                assert!(
                    !message.contains(absent.as_str()) && !message.contains(domain),
                    "case {case:?}: expected a Message interpolating neither the id nor the domain, got {message:?}"
                );
                // `BatchGetProfileError$Message` targets string1To1000: @length 1..1000.
                assert!(
                    (1..=1000).contains(&message.chars().count()),
                    "case {case:?}: expected a Message within the modelled length 1..1000, got {} characters in {message:?}",
                    message.chars().count()
                );
            }
        }
    }
}

/// The refusals: a domain that is not stored is 404, and an input outside a modelled
/// constraint is 400 — the id list on both published bounds and one past each, and an id
/// outside the modelled `ProfileId` pattern.
#[tokio::test]
async fn a_batch_refuses_an_absent_domain_and_input_outside_a_modelled_constraint() {
    let emulator = Emulator::new();
    let (stored, _other_id, _created) = seed(&emulator, "bounds", "otherbounds").await;

    let (status, body) = batch_get(&emulator, "nosuchdomain", &[stored[0].clone()]).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"absent-domain\": expected 404 ResourceNotFoundException, got {status} {body:?}"
    );

    let at_maximum: Vec<String> = (0..MAX_PROFILE_IDS as u64).map(unknown_id).collect();
    let past_maximum: Vec<String> = (0..MAX_PROFILE_IDS as u64 + 1).map(unknown_id).collect();

    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "one-id-at-the-minimum-length",
            json!({ "ProfileIds": [stored[0].clone()] }),
            200,
        ),
        ("no-id-below-the-minimum-length", json!({ "ProfileIds": [] }), 400),
        (
            "twenty-ids-at-the-maximum-length",
            json!({ "ProfileIds": at_maximum }),
            200,
        ),
        (
            "twenty-one-ids-above-the-maximum-length",
            json!({ "ProfileIds": past_maximum }),
            400,
        ),
        ("the-id-list-absent", json!({}), 400),
        (
            "id-outside-the-modelled-pattern",
            json!({ "ProfileIds": ["not-a-uuid"] }),
            400,
        ),
        (
            "id-shorter-than-the-modelled-pattern",
            json!({ "ProfileIds": ["a".repeat(31)] }),
            400,
        ),
        (
            "id-in-upper-case-hex-outside-the-modelled-pattern",
            json!({ "ProfileIds": ["A".repeat(32)] }),
            400,
        ),
    ];

    for (case, body, expected_status) in cases {
        let (status, response) = emulator.post("/domains/bounds/batch-get-profiles", body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {response:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {case:?}: expected the error code BadRequestException, got {response:?}"
            );
        }
    }
}
