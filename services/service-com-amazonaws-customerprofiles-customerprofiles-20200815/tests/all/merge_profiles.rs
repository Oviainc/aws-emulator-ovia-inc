// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `MergeProfiles`.
//!
//! The rule under test: a merge names the profile taken and the profiles merged into it,
//! and those are DISJOINT — a request in which one profile id appears twice (the main
//! profile named inside `ProfileIdsToBeMerged`, or that list repeating an entry) is
//! refused with `BadRequestException` (400) before anything is written, so every profile
//! the request named is still readable afterwards. A merge of genuinely different
//! profiles succeeds: the standard fields are union-ed with the main profile winning a
//! conflict unless `FieldSourceProfileIds` names another source for that field, and each
//! merged profile is deleted.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, item_ids, Emulator};

/// The fixed text AWS answers a merge that names one profile twice with.
const DUPLICATE_MESSAGE: &str = "Request contains duplicated profile ID";

/// `ProfileIdToBeMergedList` is `len 1..20`, so 20 ids is the published maximum.
const MAX_IDS_TO_BE_MERGED: usize = 20;

/// A `ProfileId` that satisfies the modelled pattern `^[a-f0-9]{32}$` but that no create
/// in these tests minted, so the store cannot hold it.
fn unknown_id(seed: u64) -> String {
    format!("{:032x}", 0xeeee_0000_0000_0000u64 + seed)
}

/// `MergeProfiles` over the wire: `POST /domains/{DomainName}/profiles/objects/merge`.
async fn merge(emulator: &Emulator, domain: &str, body: Value) -> (u16, Value) {
    emulator
        .post(&format!("/domains/{domain}/profiles/objects/merge"), body)
        .await
}

/// The profile `BatchGetProfile` reports for `id`, or `None` when it reports the id as an
/// error instead — which is how a deleted profile shows up.
async fn read_profile(emulator: &Emulator, domain: &str, id: &str) -> Option<Value> {
    let (status, body) = emulator
        .post(
            &format!("/domains/{domain}/batch-get-profiles"),
            json!({ "ProfileIds": [id] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {id:?}: expected BatchGetProfile 200, got {status} {body:?}"
    );
    body.get("Profiles")
        .and_then(Value::as_array)
        .and_then(|profiles| profiles.first())
        .cloned()
}

/// The ids `SearchProfiles` finds for one predefined key and value.
async fn search(emulator: &Emulator, domain: &str, key: &str, value: &str) -> Vec<String> {
    let (status, body) = emulator
        .search(domain, json!({ "KeyName": key, "Values": [value] }), "")
        .await;
    assert_eq!(
        status, 200,
        "case {key:?}={value:?}: expected SearchProfiles 200, got {status} {body:?}"
    );
    item_ids(&body)
}

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A profile per request body, in the order given.
async fn seed(emulator: &Emulator, domain: &str, requests: &[Value]) -> Vec<String> {
    let mut ids = Vec::new();
    for request in requests {
        ids.push(emulator.create_profile(domain, request.clone()).await);
    }
    ids
}

/// The refusal, over a table of request shapes that all say the same thing: a profile id
/// appears twice. The rows differ in WHICH id repeats, WHERE in the list it repeats and
/// HOW MANY ids the list carries — and every one of them must leave the store as it was.
#[tokio::test]
async fn a_merge_that_names_one_profile_twice_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    emulator.create_domain("dupdom", 30).await;
    let stored = seed(
        &emulator,
        "dupdom",
        &[
            json!({ "FirstName": "Solo", "EmailAddress": "solo@example.com" }),
            json!({ "FirstName": "Pair", "PhoneNumber": "+15550101" }),
            json!({ "AccountNumber": "acct-3" }),
            json!({ "LastName": "Fourth" }),
        ],
    )
    .await;
    let (main, second, third, fourth) = (
        stored[0].clone(),
        stored[1].clone(),
        stored[2].clone(),
        stored[3].clone(),
    );

    // Twenty ids — the modelled maximum — whose last entry repeats the first.
    let mut at_maximum: Vec<String> = vec![second.clone()];
    at_maximum.extend((0..MAX_IDS_TO_BE_MERGED as u64 - 2).map(unknown_id));
    at_maximum.push(second.clone());

    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "the main profile is the only id to be merged",
            main.clone(),
            vec![main.clone()],
        ),
        (
            "the main profile is the first of two ids",
            main.clone(),
            vec![main.clone(), second.clone()],
        ),
        (
            "the main profile is the last of three ids",
            main.clone(),
            vec![second.clone(), third.clone(), main.clone()],
        ),
        (
            "another profile is named twice in a row",
            main.clone(),
            vec![second.clone(), second.clone()],
        ),
        (
            "another profile is named twice, far apart",
            main.clone(),
            vec![second.clone(), third.clone(), fourth.clone(), second.clone()],
        ),
        (
            "a list at the modelled maximum length repeats an id",
            main.clone(),
            at_maximum.clone(),
        ),
    ];

    for (case, main_profile_id, ids) in rows {
        let (status, body) = merge(
            &emulator,
            "dupdom",
            json!({ "MainProfileId": main_profile_id, "ProfileIdsToBeMerged": ids }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body:?}"
        );
        assert_eq!(
            error_message(&body),
            DUPLICATE_MESSAGE,
            "case {case:?}: expected the message {DUPLICATE_MESSAGE:?}, got {body:?}"
        );

        // Nothing was merged and nothing was deleted: every profile the request named is
        // still readable, with the attributes its create stored.
        for id in [&main, &second, &third, &fourth] {
            let profile = read_profile(&emulator, "dupdom", id).await;
            assert!(
                profile.is_some(),
                "case {case:?}: profile {id} must still be readable after the refusal, \
                 got {profile:?}"
            );
        }
        assert_eq!(
            search(&emulator, "dupdom", "_email", "solo@example.com").await,
            vec![main.clone()],
            "case {case:?}: the refused merge must leave the search index alone"
        );
    }
}

/// One row of the union table: the main profile's request, the requests of the profiles
/// merged into it, the `FieldSourceProfileIds` the merge carries (by INDEX into the
/// merged profiles, `None` meaning the main profile), and the members the merged profile
/// must report.
struct Union {
    case: &'static str,
    main: Value,
    merged: Vec<Value>,
    /// Field name -> which participant the field is taken from: `None` is the main
    /// profile, `Some(i)` the i-th merged profile.
    field_sources: Vec<(&'static str, Option<usize>)>,
    expected: Value,
}

/// The success path, over a table spanning the number of profiles merged, which profile
/// carries a field, and whether `FieldSourceProfileIds` overrides the winner.
#[tokio::test]
async fn a_merge_unions_the_standard_fields_and_deletes_the_merged_profiles() {
    let emulator = Emulator::new();

    let rows = [
        Union {
            case: "one profile merged, disjoint fields are union-ed",
            main: json!({ "FirstName": "Ada", "EmailAddress": "ada@example.com" }),
            merged: vec![json!({ "LastName": "Lovelace", "PhoneNumber": "+15550100" })],
            field_sources: vec![],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Lovelace",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+15550100",
            }),
        },
        Union {
            case: "a conflicting field falls to the main profile",
            main: json!({ "FirstName": "Ada", "Attributes": { "tier": "gold" } }),
            merged: vec![json!({
                "FirstName": "Grace",
                "LastName": "Hopper",
                "Attributes": { "tier": "silver", "region": "eu" },
            })],
            field_sources: vec![],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Hopper",
                "Attributes": { "tier": "gold", "region": "eu" },
            }),
        },
        Union {
            case: "FieldSourceProfileIds takes a field from the merged profile",
            main: json!({ "FirstName": "Ada", "BusinessName": "Analytical" }),
            merged: vec![json!({ "FirstName": "Grace", "BusinessName": "Harvard" })],
            field_sources: vec![("FirstName", Some(0))],
            expected: json!({ "FirstName": "Grace", "BusinessName": "Analytical" }),
        },
        Union {
            case: "FieldSourceProfileIds naming the main profile keeps its value",
            main: json!({ "LastName": "Lovelace" }),
            merged: vec![json!({ "LastName": "Hopper" })],
            field_sources: vec![("LastName", None)],
            expected: json!({ "LastName": "Lovelace" }),
        },
        Union {
            case: "three profiles merged, the first carrier of a field wins",
            main: json!({ "FirstName": "Ada" }),
            merged: vec![
                json!({ "LastName": "Hopper", "AccountNumber": "acct-2" }),
                json!({ "LastName": "Johnson", "AccountNumber": "acct-3", "Gender": "FEMALE" }),
            ],
            field_sources: vec![],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Hopper",
                "AccountNumber": "acct-2",
                "Gender": "FEMALE",
            }),
        },
        Union {
            case: "three profiles merged with a source named for the last one",
            main: json!({ "FirstName": "Ada" }),
            merged: vec![
                json!({ "LastName": "Hopper" }),
                json!({ "LastName": "Johnson", "MiddleName": "Goble" }),
            ],
            field_sources: vec![("LastName", Some(1))],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Johnson",
                "MiddleName": "Goble",
            }),
        },
        Union {
            case: "a structure field is union-ed whole",
            main: json!({ "FirstName": "Ada" }),
            merged: vec![json!({
                "MailingAddress": { "Address1": "1 Main St", "City": "Springfield" },
            })],
            field_sources: vec![],
            expected: json!({
                "FirstName": "Ada",
                "MailingAddress": { "Address1": "1 Main St", "City": "Springfield" },
            }),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let domain = format!("uniondom{index}");
        emulator.create_domain(&domain, 30).await;

        let main_id = emulator.create_profile(&domain, row.main.clone()).await;
        let merged_ids = seed(&emulator, &domain, &row.merged).await;

        let mut request = json!({
            "MainProfileId": main_id,
            "ProfileIdsToBeMerged": merged_ids,
        });
        if !row.field_sources.is_empty() {
            let sources: serde_json::Map<String, Value> = row
                .field_sources
                .iter()
                .map(|(field, from)| {
                    let id = match from {
                        None => main_id.clone(),
                        Some(position) => merged_ids[*position].clone(),
                    };
                    ((*field).to_string(), Value::String(id))
                })
                .collect();
            request["FieldSourceProfileIds"] = Value::Object(sources);
        }

        let (status, body) = merge(&emulator, &domain, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {request:?}, got {status} {body:?}"
        );
        assert!(
            body.get("Message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a Message reporting the merge is complete, got {body:?}"
        );

        // The main profile now reports the union, read back through BatchGetProfile.
        let merged_profile = read_profile(&emulator, &domain, &main_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the main profile must still be readable"));
        for (member, expected) in row.expected.as_object().expect("an object expectation") {
            assert_eq!(
                merged_profile.get(member),
                Some(expected),
                "case {case:?}: expected {member} = {expected:?}, got {:?}",
                merged_profile.get(member)
            );
        }

        // "All the ProfileToBeMerged will be deleted at the end."
        for id in &merged_ids {
            let gone = read_profile(&emulator, &domain, id).await;
            assert!(
                gone.is_none(),
                "case {case:?}: the merged profile {id} must be deleted, got {gone:?}"
            );
            assert_eq!(
                search(&emulator, &domain, "_profileId", id).await,
                Vec::<String>::new(),
                "case {case:?}: the merged profile {id} must leave the search index"
            );
        }

        // A field the main profile inherited is searchable under its predefined key, so
        // the merged profile really was re-indexed.
        if let Some(phone) = row.expected.get("PhoneNumber").and_then(Value::as_str) {
            assert_eq!(
                search(&emulator, &domain, "_phone", phone).await,
                vec![main_id.clone()],
                "case {case:?}: expected the merged profile to be found by its phone"
            );
        }
    }
}

/// The refusals that are not the duplicate: a domain or profile the merge names that is
/// not stored is 404, a `FieldSourceProfileIds` entry naming a profile outside the merge
/// is 400, and an input outside a modelled constraint is 400 — the id list on both
/// published bounds and one past each.
#[tokio::test]
async fn a_merge_refuses_an_absent_participant_and_input_outside_a_modelled_constraint() {
    let emulator = Emulator::new();
    emulator.create_domain("refusedom", 30).await;
    let stored = seed(
        &emulator,
        "refusedom",
        &[
            json!({ "FirstName": "Main" }),
            json!({ "FirstName": "Other" }),
            json!({ "FirstName": "Bystander" }),
        ],
    )
    .await;
    let (main, other, bystander) = (stored[0].clone(), stored[1].clone(), stored[2].clone());

    let not_found: Vec<(&str, &str, Value)> = vec![
        (
            "a domain that is not stored",
            "nosuchdomain",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [other.clone()] }),
        ),
        (
            "a main profile that is not stored",
            "refusedom",
            json!({ "MainProfileId": unknown_id(1), "ProfileIdsToBeMerged": [other.clone()] }),
        ),
        (
            "a profile to be merged that is not stored",
            "refusedom",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [unknown_id(2)] }),
        ),
        (
            "one stored and one absent profile to be merged",
            "refusedom",
            json!({
                "MainProfileId": main,
                "ProfileIdsToBeMerged": [other.clone(), unknown_id(3)],
            }),
        ),
    ];

    for (case, domain, body) in not_found {
        let (status, response) = merge(&emulator, domain, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }

    // A refused merge wrote nothing, so both profiles are still there to merge.
    for id in [&main, &other, &bystander] {
        assert!(
            read_profile(&emulator, "refusedom", id).await.is_some(),
            "case \"after the refusals\": profile {id} must still be readable"
        );
    }

    // `FieldSourceProfileIds` can only name a profile the merge itself carries.
    let (status, body) = merge(
        &emulator,
        "refusedom",
        json!({
            "MainProfileId": main,
            "ProfileIdsToBeMerged": [other.clone()],
            "FieldSourceProfileIds": { "FirstName": bystander },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case \"a field source outside the merge\": expected 400 BadRequestException, \
         got {status} {body:?}"
    );

    let at_maximum: Vec<String> = (0..MAX_IDS_TO_BE_MERGED as u64).map(unknown_id).collect();
    let past_maximum: Vec<String> = (0..MAX_IDS_TO_BE_MERGED as u64 + 1).map(unknown_id).collect();

    // Each row is refused at the boundary, before the handler runs: the ids need not be
    // stored for the modelled constraint to decide.
    let constraints: Vec<(&str, Value, u16)> = vec![
        (
            "one id at the minimum list length",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [unknown_id(9)] }),
            404,
        ),
        (
            "no id below the minimum list length",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [] }),
            400,
        ),
        (
            "twenty ids at the maximum list length",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": at_maximum }),
            404,
        ),
        (
            "twenty-one ids above the maximum list length",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": past_maximum }),
            400,
        ),
        (
            "the id list absent",
            json!({ "MainProfileId": main }),
            400,
        ),
        (
            "the main profile id absent",
            json!({ "ProfileIdsToBeMerged": [other.clone()] }),
            400,
        ),
        (
            "a main profile id outside the modelled pattern",
            json!({ "MainProfileId": "not-a-uuid", "ProfileIdsToBeMerged": [other.clone()] }),
            400,
        ),
        (
            "an id to be merged in upper-case hex, outside the modelled pattern",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": ["A".repeat(32)] }),
            400,
        ),
    ];

    for (case, body, expected_status) in constraints {
        let (status, response) = merge(&emulator, "refusedom", body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {response:?}"
            );
        }
    }
}
