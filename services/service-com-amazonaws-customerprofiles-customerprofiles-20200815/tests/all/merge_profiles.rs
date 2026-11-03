// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `MergeProfiles`.
//!
//! The rule under test: a merge names the profile taken and the profiles merged into it,
//! and those are DISJOINT — a request in which one profile id appears twice (the main
//! profile named inside `ProfileIdsToBeMerged`, or that list repeating an entry) is
//! refused with `BadRequestException` (400) before anything is written, so every profile
//! the request named is still readable afterwards. A merge of genuinely different
//! profiles succeeds: the standard fields are union-ed with the main profile winning a
//! conflict unless `FieldSourceProfileIds` names another source for that field — which
//! is then taken even when that profile carries no value for it — each merged profile is
//! deleted, and what the merge wrote is an ordinary profile a later call acts on.
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

/// One row of the merge table: the main profile's request, the requests of the profiles
/// merged into it, the `FieldSourceProfileIds` the merge carries (by INDEX into the
/// merged profiles, `None` meaning the main profile), the members the merged profile must
/// then report, and what the search index must and must not find it under.
///
/// `Default` fills the columns a row does not exercise, so each row states only what it
/// is about.
struct MergeRow {
    case: &'static str,
    main: Value,
    merged: Vec<Value>,
    /// Field name -> which participant the field is taken from: `None` is the main
    /// profile, `Some(i)` the i-th merged profile. A member absent from this list carries
    /// no source at all.
    field_sources: Vec<(&'static str, Option<usize>)>,
    /// `FieldSourceProfileIds$Attributes`: attribute key -> the same choice of
    /// participant.
    attribute_sources: Vec<(&'static str, Option<usize>)>,
    /// Members the merged profile must report, `null` meaning the member must be absent.
    expected: Value,
    /// Predefined search keys and values that must find the merged profile.
    index_finds: Vec<(&'static str, &'static str)>,
    /// Predefined search keys and values that must find nothing at all.
    index_drops: Vec<(&'static str, &'static str)>,
}

impl Default for MergeRow {
    fn default() -> Self {
        Self {
            case: "",
            main: json!({}),
            merged: Vec::new(),
            field_sources: Vec::new(),
            attribute_sources: Vec::new(),
            // An empty object, not `Value::Null`: a row that states no member
            // expectations still has every other column asserted for it.
            expected: json!({}),
            index_finds: Vec::new(),
            index_drops: Vec::new(),
        }
    }
}

/// The `FieldSourceProfileIds` one row carries: every named member and attribute key
/// resolved to the id of the participant the row names for it. `None` when the row names
/// no source, which is the request shape that omits the member entirely.
fn field_source_ids(row: &MergeRow, main_id: &str, merged_ids: &[String]) -> Option<Value> {
    if row.field_sources.is_empty() && row.attribute_sources.is_empty() {
        return None;
    }
    let id = |from: &Option<usize>| match from {
        None => main_id.to_string(),
        Some(position) => merged_ids
            .get(*position)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a source names merged profile {position}, but the row seeds {}",
                    row.case,
                    merged_ids.len()
                )
            })
            .clone(),
    };
    let mut sources: serde_json::Map<String, Value> = row
        .field_sources
        .iter()
        .map(|(field, from)| ((*field).to_string(), Value::String(id(from))))
        .collect();
    if !row.attribute_sources.is_empty() {
        let keys: serde_json::Map<String, Value> = row
            .attribute_sources
            .iter()
            .map(|(key, from)| ((*key).to_string(), Value::String(id(from))))
            .collect();
        sources.insert("Attributes".to_string(), Value::Object(keys));
    }
    Some(Value::Object(sources))
}

/// Drive one row end to end in its own domain: seed its profiles, merge them, and assert
/// every column the row states.
async fn run_merge(emulator: &Emulator, domain: &str, row: &MergeRow) {
    let case = row.case;
    emulator.create_domain(domain, 30).await;
    let main_id = emulator.create_profile(domain, row.main.clone()).await;
    let merged_ids = seed(emulator, domain, &row.merged).await;

    let mut request = json!({
        "MainProfileId": main_id,
        "ProfileIdsToBeMerged": merged_ids,
    });
    if let Some(sources) = field_source_ids(row, &main_id, &merged_ids) {
        request["FieldSourceProfileIds"] = sources;
    }

    let (status, body) = merge(emulator, domain, request.clone()).await;
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

    // The main profile now reports what the merge resolved, read back through
    // BatchGetProfile.
    let merged_profile = read_profile(emulator, domain, &main_id)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the main profile must still be readable"));
    for (member, expected) in row.expected.as_object().expect("an object expectation") {
        let actual = merged_profile.get(member);
        if expected.is_null() {
            assert!(
                actual.is_none_or(Value::is_null),
                "case {case:?}: expected {member} to be absent, got {actual:?}"
            );
        } else {
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: expected {member} = {expected:?}, got {actual:?}"
            );
        }
    }

    // "All the ProfileToBeMerged will be deleted at the end."
    for id in &merged_ids {
        let gone = read_profile(emulator, domain, id).await;
        assert!(
            gone.is_none(),
            "case {case:?}: the merged profile {id} must be deleted, got {gone:?}"
        );
        assert_eq!(
            search(emulator, domain, "_profileId", id).await,
            Vec::<String>::new(),
            "case {case:?}: the merged profile {id} must leave the search index"
        );
    }

    // The merged profile was re-indexed from the fields it ended up with: a value it
    // kept or inherited finds it, and one a named source cleared finds nothing.
    for (key, value) in &row.index_finds {
        assert_eq!(
            search(emulator, domain, key, value).await,
            vec![main_id.clone()],
            "case {case:?}: expected {key}={value:?} to find the merged profile"
        );
    }
    for (key, value) in &row.index_drops {
        assert_eq!(
            search(emulator, domain, key, value).await,
            Vec::<String>::new(),
            "case {case:?}: expected {key}={value:?} to find nothing"
        );
    }
}

/// The success path, over a table spanning the number of profiles merged, which profile
/// carries a field, and whether `FieldSourceProfileIds` overrides the winner.
#[tokio::test]
async fn a_merge_unions_the_standard_fields_and_deletes_the_merged_profiles() {
    let emulator = Emulator::new();

    let rows = [
        MergeRow {
            case: "one profile merged, disjoint fields are union-ed",
            main: json!({ "FirstName": "Ada", "EmailAddress": "ada@example.com" }),
            merged: vec![json!({ "LastName": "Lovelace", "PhoneNumber": "+15550100" })],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Lovelace",
                "EmailAddress": "ada@example.com",
                "PhoneNumber": "+15550100",
            }),
            // The main profile is findable under a phone number it INHERITED, so the
            // merged profile really was re-indexed.
            index_finds: vec![("_phone", "+15550100")],
            ..MergeRow::default()
        },
        MergeRow {
            case: "a conflicting field falls to the main profile",
            main: json!({ "FirstName": "Ada", "Attributes": { "tier": "gold" } }),
            merged: vec![json!({
                "FirstName": "Grace",
                "LastName": "Hopper",
                "Attributes": { "tier": "silver", "region": "eu" },
            })],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Hopper",
                "Attributes": { "tier": "gold", "region": "eu" },
            }),
            ..MergeRow::default()
        },
        MergeRow {
            case: "FieldSourceProfileIds takes a field from the merged profile",
            main: json!({ "FirstName": "Ada", "BusinessName": "Analytical" }),
            merged: vec![json!({ "FirstName": "Grace", "BusinessName": "Harvard" })],
            field_sources: vec![("FirstName", Some(0))],
            expected: json!({ "FirstName": "Grace", "BusinessName": "Analytical" }),
            ..MergeRow::default()
        },
        MergeRow {
            case: "FieldSourceProfileIds naming the main profile keeps its value",
            main: json!({ "LastName": "Lovelace" }),
            merged: vec![json!({ "LastName": "Hopper" })],
            field_sources: vec![("LastName", None)],
            expected: json!({ "LastName": "Lovelace" }),
            ..MergeRow::default()
        },
        MergeRow {
            case: "three profiles merged, the first carrier of a field wins",
            main: json!({ "FirstName": "Ada" }),
            merged: vec![
                json!({ "LastName": "Hopper", "AccountNumber": "acct-2" }),
                json!({ "LastName": "Johnson", "AccountNumber": "acct-3", "Gender": "FEMALE" }),
            ],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Hopper",
                "AccountNumber": "acct-2",
                "Gender": "FEMALE",
            }),
            ..MergeRow::default()
        },
        MergeRow {
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
            ..MergeRow::default()
        },
        MergeRow {
            case: "a structure field is union-ed whole",
            main: json!({ "FirstName": "Ada" }),
            merged: vec![json!({
                "MailingAddress": { "Address1": "1 Main St", "City": "Springfield" },
            })],
            expected: json!({
                "FirstName": "Ada",
                "MailingAddress": { "Address1": "1 Main St", "City": "Springfield" },
            }),
            ..MergeRow::default()
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        run_merge(&emulator, &format!("uniondom{index}"), row).await;
    }
}

/// A named source wins even when the profile it names carries no value for the field:
/// "if a SourceProfileIds entry is specified, the specified profileId is always taken,
/// even if it is a NULL value". The rows span the kinds of member a source can name — a
/// string, an enum, a structure, and one key of the attribute map — and a source whose
/// profile DOES carry a value, which is the same rule with the other outcome.
#[tokio::test]
async fn a_named_field_source_is_taken_even_when_that_profile_carries_no_value() {
    let emulator = Emulator::new();

    let rows = [
        MergeRow {
            case: "a string member sourced from a profile that has none is cleared",
            main: json!({ "FirstName": "Ada", "EmailAddress": "ada@example.com" }),
            merged: vec![json!({ "LastName": "Lovelace" })],
            field_sources: vec![("EmailAddress", Some(0))],
            expected: json!({
                "FirstName": "Ada",
                "LastName": "Lovelace",
                "EmailAddress": Value::Null,
            }),
            index_finds: vec![("_fullName", "Ada Lovelace")],
            index_drops: vec![("_email", "ada@example.com")],
            ..MergeRow::default()
        },
        MergeRow {
            case: "an enum member sourced from a profile that has none is cleared",
            main: json!({ "FirstName": "Grace", "Gender": "FEMALE" }),
            merged: vec![json!({ "LastName": "Hopper" })],
            field_sources: vec![("Gender", Some(0))],
            expected: json!({ "FirstName": "Grace", "Gender": Value::Null }),
            index_finds: vec![("_fullName", "Grace Hopper")],
            ..MergeRow::default()
        },
        MergeRow {
            case: "a structure member sourced from a profile that has none is cleared",
            main: json!({
                "FirstName": "Katherine",
                "MailingAddress": { "Address1": "1 Main St", "City": "Hampton" },
            }),
            merged: vec![json!({ "LastName": "Johnson" })],
            field_sources: vec![("MailingAddress", Some(0))],
            expected: json!({ "FirstName": "Katherine", "MailingAddress": Value::Null }),
            index_finds: vec![("_fullName", "Katherine Johnson")],
            ..MergeRow::default()
        },
        MergeRow {
            case: "one attribute key sourced from a profile that lacks it is dropped",
            main: json!({
                "FirstName": "Mary",
                "Attributes": { "tier": "gold", "region": "eu" },
            }),
            merged: vec![json!({ "Attributes": { "region": "us", "loyalty": "silver" } })],
            attribute_sources: vec![("tier", Some(0)), ("loyalty", None)],
            expected: json!({
                "FirstName": "Mary",
                "Attributes": { "region": "eu" },
            }),
            index_finds: vec![("_fullName", "Mary")],
            ..MergeRow::default()
        },
        MergeRow {
            case: "a source that does carry a value replaces the main profile's",
            main: json!({ "FirstName": "Annie", "PhoneNumber": "+15550001" }),
            merged: vec![json!({ "PhoneNumber": "+15550002", "AccountNumber": "acct-9" })],
            field_sources: vec![("PhoneNumber", Some(0))],
            expected: json!({
                "FirstName": "Annie",
                "PhoneNumber": "+15550002",
                "AccountNumber": "acct-9",
            }),
            index_finds: vec![("_phone", "+15550002"), ("_account", "acct-9")],
            index_drops: vec![("_phone", "+15550001")],
            ..MergeRow::default()
        },
        MergeRow {
            case: "a source naming the main profile, which has none, clears the field",
            main: json!({ "FirstName": "Dorothy" }),
            merged: vec![json!({ "LastName": "Vaughan", "BusinessName": "Langley" })],
            field_sources: vec![("BusinessName", None)],
            expected: json!({
                "FirstName": "Dorothy",
                "LastName": "Vaughan",
                "BusinessName": Value::Null,
            }),
            index_finds: vec![("_fullName", "Dorothy Vaughan")],
            ..MergeRow::default()
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        run_merge(&emulator, &format!("sourcedom{index}"), row).await;
    }
}

/// What the merge WROTE is an ordinary stored profile and what it deleted is ordinarily
/// gone: a second merge folds another profile into the result, and every later operation
/// — a merge or a delete — answers 404 for the ids the first merge consumed.
#[tokio::test]
async fn the_merged_profile_is_ordinary_state_that_later_calls_act_on() {
    let emulator = Emulator::new();
    emulator.create_domain("chaindom", 30).await;
    let stored = seed(
        &emulator,
        "chaindom",
        &[
            json!({ "FirstName": "Ada" }),
            json!({ "LastName": "Lovelace", "EmailAddress": "ada@example.com" }),
            json!({ "AccountNumber": "acct-chain", "PhoneNumber": "+15550303" }),
            json!({ "FirstName": "Bystander", "EmailAddress": "by@example.com" }),
        ],
    )
    .await;
    let (main, second, third, bystander) = (
        stored[0].clone(),
        stored[1].clone(),
        stored[2].clone(),
        stored[3].clone(),
    );

    // Two merges in a row: the second one's main profile is the FIRST one's result, so
    // the union it starts from is the union the first merge persisted.
    for (case, ids) in [
        ("the first merge", vec![second.clone()]),
        ("the second merge", vec![third.clone()]),
    ] {
        let (status, body) = merge(
            &emulator,
            "chaindom",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": ids }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
    }

    let profile = read_profile(&emulator, "chaindom", &main)
        .await
        .expect("case \"after two merges\": the main profile must be readable");
    for (member, expected) in [
        ("FirstName", "Ada"),
        ("LastName", "Lovelace"),
        ("EmailAddress", "ada@example.com"),
        ("AccountNumber", "acct-chain"),
        ("PhoneNumber", "+15550303"),
    ] {
        assert_eq!(
            profile.get(member).and_then(Value::as_str),
            Some(expected),
            "case \"after two merges\": expected {member} = {expected:?}, got {profile:?}"
        );
    }

    // Every id the merges consumed is simply not a profile any more, so the operations
    // that take a profile id answer 404 for it.
    for (case, body) in [
        (
            "a merge naming an already-merged profile",
            json!({ "MainProfileId": main, "ProfileIdsToBeMerged": [second.clone()] }),
        ),
        (
            "a merge whose main profile was already merged away",
            json!({ "MainProfileId": third, "ProfileIdsToBeMerged": [main.clone()] }),
        ),
    ] {
        let (status, response) = merge(&emulator, "chaindom", body).await;
        assert_eq!(
            (status, error_code(&response)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {response:?}"
        );
    }

    let (status, response) = emulator.delete_profile("chaindom", &second).await;
    assert_eq!(
        (status, error_code(&response)),
        (404, "ResourceNotFoundException"),
        "case \"deleting an already-merged profile\": expected 404, got {status} {response:?}"
    );

    // The profile the merges built is deletable like any other, and taking it away
    // leaves the bystander — which no merge named — exactly as its create stored it.
    let (status, response) = emulator.delete_profile("chaindom", &main).await;
    assert_eq!(
        status, 200,
        "case \"deleting the merged profile\": expected HTTP 200, got {status} {response:?}"
    );
    let gone = read_profile(&emulator, "chaindom", &main).await;
    assert!(
        gone.is_none(),
        "case \"deleting the merged profile\": expected it to be gone, got {gone:?}"
    );
    assert_eq!(
        search(&emulator, "chaindom", "_email", "ada@example.com").await,
        Vec::<String>::new(),
        "case \"deleting the merged profile\": expected its index entries to go with it"
    );
    assert!(
        read_profile(&emulator, "chaindom", &bystander)
            .await
            .is_some(),
        "case \"the bystander\": a profile no merge named must still be readable"
    );
    assert_eq!(
        search(&emulator, "chaindom", "_email", "by@example.com").await,
        vec![bystander.clone()],
        "case \"the bystander\": its index entries must be untouched"
    );
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
