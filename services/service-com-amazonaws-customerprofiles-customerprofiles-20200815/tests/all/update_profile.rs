// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateProfile`.
//!
//! The rule under test: an update is applied to the STORED profile member by member — a
//! member the request does not carry keeps the value already there, a string member
//! carrying the empty string has its value removed, and any other value replaces it —
//! and the profile is re-indexed, so a later search answers from the values the update
//! left behind. A domain or a profile the request names but the store does not hold is
//! `ResourceNotFoundException` (404) and nothing is written.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing and the create helpers are this crate's one test
//! harness, in `tests/all/domain_profile_lifecycle.rs`.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, item_ids, Emulator};

/// The profile every row of the table starts from: members of each updatable class — a
/// plain string, an enum, an address with several of its own members, and an attribute
/// map with several entries — so one row can touch one class and the rest prove they
/// were kept.
fn base_profile() -> Value {
    json!({
        "FirstName": "Ada",
        "MiddleName": "Byron",
        "LastName": "Lovelace",
        "AccountNumber": "acct-1",
        "EmailAddress": "ada@example.com",
        "PhoneNumber": "+15550100",
        "PartyType": "INDIVIDUAL",
        "GenderString": "unspecified",
        "BirthDate": "1815-12-10",
        "MailingAddress": { "Address1": "12 Baker St", "City": "London", "PostalCode": "NW1 6XE" },
        "Attributes": { "tier": "gold", "segment": "beta" },
    })
}

/// A `ProfileId` that satisfies the modelled pattern `^[a-f0-9]{32}$` but that no create
/// in these tests minted, so the store cannot hold it.
fn unknown_id(seed: u64) -> String {
    format!("{:032x}", 0xabcd_0000_0000_0000u64 + seed)
}

/// `UpdateProfile` over the wire: `PUT /domains/{DomainName}/profiles`, with the id of
/// the profile being updated put into the body the row supplied.
async fn update(emulator: &Emulator, domain: &str, profile_id: &str, patch: Value) -> (u16, Value) {
    let mut body = patch;
    body.as_object_mut()
        .expect("an object request body")
        .insert("ProfileId".to_string(), json!(profile_id));
    emulator
        .send("PUT", &format!("/domains/{domain}/profiles"), Some(body))
        .await
}

/// The profile `BatchGetProfile` reports for `id`, or `None` when it reports the id as an
/// error instead.
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

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The profile a row expects after its update: the profile as it was stored, with the
/// members the row names set to a new value or removed outright.
fn with_changes(before: &Value, changes: &[(&str, Option<Value>)]) -> Value {
    let mut members = before
        .as_object()
        .cloned()
        .expect("the stored profile is an object");
    for (member, value) in changes {
        match value {
            Some(value) => {
                members.insert((*member).to_string(), value.clone());
            }
            None => {
                members.remove(*member);
            }
        }
    }
    Value::Object(members)
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

/// One member an update must have changed, and what it must have become: `None` where
/// the member must be gone from the profile altogether.
type MemberChange<'a> = (&'a str, Option<Value>);

/// One row of the member-by-member table: the case, the request body minus `ProfileId`,
/// and the members that must have changed.
type UpdateRow<'a> = (&'a str, Value, Vec<MemberChange<'a>>);

/// One row of the re-indexing table: the case, the profile to create, the update to
/// apply, the predefined search key, the value it was indexed under before, and the
/// value it must be indexed under after (`None` where the update removed it).
type SearchRow<'a> = (&'a str, Value, Value, &'a str, &'a str, Option<&'a str>);

/// The rule, over a table whose rows differ in WHICH member class the update touches
/// (plain string, enum, address member, attribute entry), in WHAT the update says about
/// it (a value, the empty string, nothing at all), in whether the profile already carries
/// a value for it, and in HOW MANY members one request touches. Each row is checked
/// against the WHOLE stored profile, so a member the row did not name proves it was kept.
#[tokio::test]
async fn an_update_replaces_named_members_removes_empty_ones_and_keeps_the_rest() {
    let emulator = Emulator::new();
    emulator.create_domain("updom", 90).await;

    // (case, the request body minus ProfileId, the members that must have changed)
    let rows: Vec<UpdateRow> = vec![
        (
            "a value replaces the value stored for that member",
            json!({ "FirstName": "Augusta" }),
            vec![("FirstName", Some(json!("Augusta")))],
        ),
        (
            "the empty string removes the value stored for that member",
            json!({ "MiddleName": "" }),
            vec![("MiddleName", None)],
        ),
        (
            "a value is set for a member the profile carries none for",
            json!({ "BusinessName": "Analytical Engines" }),
            vec![("BusinessName", Some(json!("Analytical Engines")))],
        ),
        (
            "the empty string for a member carrying no value changes nothing",
            json!({ "BusinessPhoneNumber": "" }),
            vec![],
        ),
        (
            "an enum member is replaced",
            json!({ "PartyType": "BUSINESS" }),
            vec![("PartyType", Some(json!("BUSINESS")))],
        ),
        (
            "one address member is replaced and the others kept",
            json!({ "MailingAddress": { "City": "Oxford" } }),
            vec![(
                "MailingAddress",
                Some(json!({
                    "Address1": "12 Baker St",
                    "City": "Oxford",
                    "PostalCode": "NW1 6XE",
                })),
            )],
        ),
        (
            "one address member is removed and the others kept",
            json!({ "MailingAddress": { "PostalCode": "" } }),
            vec![(
                "MailingAddress",
                Some(json!({ "Address1": "12 Baker St", "City": "London" })),
            )],
        ),
        (
            "an address member is added to an address already stored",
            json!({ "MailingAddress": { "Country": "GB" } }),
            vec![(
                "MailingAddress",
                Some(json!({
                    "Address1": "12 Baker St",
                    "City": "London",
                    "Country": "GB",
                    "PostalCode": "NW1 6XE",
                })),
            )],
        ),
        (
            "removing every member of an address leaves no address",
            json!({
                "MailingAddress": { "Address1": "", "City": "", "PostalCode": "" },
            }),
            vec![("MailingAddress", None)],
        ),
        (
            "an address is stored for a profile that carried none",
            json!({ "ShippingAddress": { "City": "Paris", "Country": "FR" } }),
            vec![(
                "ShippingAddress",
                Some(json!({ "City": "Paris", "Country": "FR" })),
            )],
        ),
        (
            "an attribute is set, another removed, and a third left alone",
            json!({ "Attributes": { "tier": "", "channel": "email" } }),
            vec![(
                "Attributes",
                Some(json!({ "segment": "beta", "channel": "email" })),
            )],
        ),
        (
            "removing every attribute leaves none",
            json!({ "Attributes": { "tier": "", "segment": "" } }),
            vec![("Attributes", None)],
        ),
        (
            "attributes are stored for a profile that carried none",
            json!({ "AdditionalInformation": "migrated", "Attributes": { "tier": "gold" } }),
            vec![("AdditionalInformation", Some(json!("migrated")))],
        ),
        (
            "several members at once, mixing a replacement, a removal and an addition",
            json!({
                "FirstName": "Augusta",
                "LastName": "",
                "BirthDate": "1815-12-11",
                "HomePhoneNumber": "+15550199",
                "MailingAddress": { "City": "Oxford", "PostalCode": "" },
                "Attributes": { "segment": "ga" },
            }),
            vec![
                ("FirstName", Some(json!("Augusta"))),
                ("LastName", None),
                ("BirthDate", Some(json!("1815-12-11"))),
                ("HomePhoneNumber", Some(json!("+15550199"))),
                (
                    "MailingAddress",
                    Some(json!({ "Address1": "12 Baker St", "City": "Oxford" })),
                ),
                (
                    "Attributes",
                    Some(json!({ "tier": "gold", "segment": "ga" })),
                ),
            ],
        ),
        (
            "an update that names no member at all leaves the profile as it was",
            json!({}),
            vec![],
        ),
    ];

    for (case, patch, changes) in rows {
        let profile_id = emulator.create_profile("updom", base_profile()).await;
        let before = read_profile(&emulator, "updom", &profile_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created profile must be readable"));

        let (status, body) = update(&emulator, "updom", &profile_id, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateProfile 200, got {status} {body:?}"
        );
        assert_eq!(
            body.get("ProfileId").and_then(Value::as_str),
            Some(profile_id.as_str()),
            "case {case:?}: expected the updated ProfileId {profile_id:?}, got {body:?}"
        );

        let after = read_profile(&emulator, "updom", &profile_id)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the updated profile must still be readable"));
        let expected = with_changes(&before, &changes);
        assert_eq!(
            after, expected,
            "case {case:?}: expected {expected:?}, got {after:?}"
        );
    }
}

/// An enum member is not a free-form string: the empty string is not in the value set
/// the model declares for it, so the boundary refuses the request rather than the member
/// being cleared, and the profile keeps the value it had. The free-form alternatives the
/// model offers for two of them (`PartyTypeString`, `GenderString`) are cleared by the
/// empty string like any other string, which the table above covers.
#[tokio::test]
async fn the_empty_string_does_not_clear_an_enum_member() {
    let emulator = Emulator::new();
    emulator.create_domain("enumdom", 90).await;

    // One row per enum-typed member of the request: `PartyType`, `Gender`, `ProfileType`.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("PartyType", "INDIVIDUAL", "BUSINESS"),
        ("Gender", "FEMALE", "UNSPECIFIED"),
        ("ProfileType", "PROFILE", "ACCOUNT_PROFILE"),
    ];

    for (member, stored_value, other_value) in rows {
        let profile_id = emulator
            .create_profile("enumdom", json!({ member: stored_value }))
            .await;

        let (status, body) = update(
            &emulator,
            "enumdom",
            &profile_id,
            json!({ member: "" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {member:?}: expected the empty string to be refused with 400 BadRequestException, got {status} {body:?}"
        );

        let after = read_profile(&emulator, "enumdom", &profile_id)
            .await
            .unwrap_or_else(|| panic!("case {member:?}: the profile must still be readable"));
        assert_eq!(
            after.get(member).and_then(Value::as_str),
            Some(stored_value),
            "case {member:?}: a refused update keeps the stored value, got {after:?}"
        );

        // A value the enum does declare replaces it, so the member is updatable.
        let (status, body) = update(
            &emulator,
            "enumdom",
            &profile_id,
            json!({ member: other_value }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {member:?}: expected UpdateProfile 200 for {other_value:?}, got {status} {body:?}"
        );
        let after = read_profile(&emulator, "enumdom", &profile_id)
            .await
            .unwrap_or_else(|| panic!("case {member:?}: the profile must still be readable"));
        assert_eq!(
            after.get(member).and_then(Value::as_str),
            Some(other_value),
            "case {member:?}: expected {other_value:?} to be stored, got {after:?}"
        );
    }
}

/// An update is not a create: it re-indexes the profile it rewrote, so a search by a
/// predefined key answers from the value the update left and no longer from the one it
/// replaced. The rows are the predefined keys a profile's OWN members populate, which
/// `SearchProfiles$KeyName` lists as `_account`, `_email`, `_phone`, `_fullName` and
/// `_profileId` — one row per key whose value an update can change, plus a row for a
/// value removed outright.
#[tokio::test]
async fn an_update_reindexes_the_profile_for_search() {
    let emulator = Emulator::new();
    emulator.create_domain("searchdom", 90).await;

    // (case, the created profile, the update, the key, the value before, the value after)
    let rows: Vec<SearchRow> = vec![
        (
            "an email address replaced",
            json!({ "EmailAddress": "old-mail@example.com" }),
            json!({ "EmailAddress": "new-mail@example.com" }),
            "_email",
            "old-mail@example.com",
            Some("new-mail@example.com"),
        ),
        (
            "a phone number replaced",
            json!({ "PhoneNumber": "+15550301" }),
            json!({ "PhoneNumber": "+15550302" }),
            "_phone",
            "+15550301",
            Some("+15550302"),
        ),
        (
            "a full name changed through one of its parts",
            json!({ "FirstName": "Grace", "LastName": "Hopper" }),
            json!({ "LastName": "Murray" }),
            "_fullName",
            "Grace Hopper",
            Some("Grace Murray"),
        ),
        (
            "an account number replaced",
            json!({ "AccountNumber": "acct-old" }),
            json!({ "AccountNumber": "acct-new" }),
            "_account",
            "acct-old",
            Some("acct-new"),
        ),
        (
            "a business email address removed",
            json!({ "BusinessEmailAddress": "retired@example.com" }),
            json!({ "BusinessEmailAddress": "" }),
            "_email",
            "retired@example.com",
            None,
        ),
    ];

    for (case, created, patch, key, before_value, after_value) in rows {
        let profile_id = emulator.create_profile("searchdom", created.clone()).await;
        assert_eq!(
            search(&emulator, "searchdom", key, before_value).await,
            vec![profile_id.clone()],
            "case {case:?}: the created profile must be found by {key} = {before_value:?}"
        );

        let (status, body) = update(&emulator, "searchdom", &profile_id, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateProfile 200, got {status} {body:?}"
        );

        let stale = search(&emulator, "searchdom", key, before_value).await;
        assert!(
            stale.is_empty(),
            "case {case:?}: expected {key} = {before_value:?} to find nothing after the update, got {stale:?}"
        );
        if let Some(after_value) = after_value {
            assert_eq!(
                search(&emulator, "searchdom", key, after_value).await,
                vec![profile_id.clone()],
                "case {case:?}: expected {key} = {after_value:?} to find the updated profile"
            );
        }
        assert_eq!(
            search(&emulator, "searchdom", "_profileId", &profile_id).await,
            vec![profile_id.clone()],
            "case {case:?}: the updated profile is still stored under its own id"
        );
    }
}

/// A domain or a profile the request names but the store does not hold is
/// `ResourceNotFoundException` (404), naming the resource it could not find, and the
/// refused call writes nothing: the profile that does exist is unchanged afterwards.
#[tokio::test]
async fn an_update_of_a_resource_that_is_not_stored_is_not_found_and_writes_nothing() {
    let emulator = Emulator::new();
    emulator.create_domain("here", 90).await;
    emulator.create_domain("elsewhere", 90).await;
    let profile_id = emulator.create_profile("here", base_profile()).await;
    let stored = read_profile(&emulator, "here", &profile_id)
        .await
        .expect("the created profile must be readable");

    // (case, domain, profile id, the identifier the refusal must name)
    let rows: Vec<(&str, String, String, String)> = vec![
        (
            "a domain that was never created",
            "nosuchdomain".to_string(),
            profile_id.clone(),
            "nosuchdomain".to_string(),
        ),
        (
            "a profile id nothing minted, in a domain that exists",
            "here".to_string(),
            unknown_id(1),
            unknown_id(1),
        ),
        (
            "a profile id stored in another domain",
            "elsewhere".to_string(),
            profile_id.clone(),
            profile_id.clone(),
        ),
        (
            "neither the domain nor the id is stored",
            "gonedomain".to_string(),
            unknown_id(2),
            "gonedomain".to_string(),
        ),
    ];

    for (case, domain, id, named) in rows {
        let (status, body) = update(
            &emulator,
            &domain,
            &id,
            json!({ "FirstName": "Overwritten" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body:?}"
        );
        assert!(
            error_message(&body).contains(&named),
            "case {case:?}: expected the refusal to name {named:?}, got {body:?}"
        );
    }

    let after = read_profile(&emulator, "here", &profile_id)
        .await
        .expect("the stored profile must still be readable");
    assert_eq!(
        after, stored,
        "a refused update must write nothing: expected {stored:?}, got {after:?}"
    );
}

/// The attributes an update carries are merged into the ones stored, so what the store
/// has to bound is the MERGED map, not one request's: an update whose result stays at the
/// cap is stored, an update whose result would pass it is refused `BadRequestException`
/// (400) naming the limit and writes nothing, and an update that only replaces keys
/// already stored is still accepted at the cap. Both the bound and one past it are rows,
/// reached across requests as well as within one.
#[tokio::test]
async fn attributes_accumulate_only_up_to_the_per_profile_cap() {
    let emulator = Emulator::new();
    emulator.create_domain("capdom", 90).await;

    /// `count` attributes, named from `first` onwards so two ranges can be disjoint.
    fn attributes(first: usize, count: usize) -> Value {
        let entries: serde_json::Map<String, Value> = (first..first + count)
            .map(|index| (format!("attr{index}"), json!("v")))
            .collect();
        Value::Object(entries)
    }

    let cap = service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::MAX_ATTRIBUTES_PER_PROFILE;

    // One request that lands exactly on the cap.
    let profile_id = emulator
        .create_profile("capdom", json!({ "FirstName": "Ada" }))
        .await;
    let (status, body) = update(
        &emulator,
        "capdom",
        &profile_id,
        json!({ "Attributes": attributes(0, cap) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"at-the-cap\": expected UpdateProfile 200, got {status} {body:?}"
    );
    let at_the_cap = read_profile(&emulator, "capdom", &profile_id)
        .await
        .expect("the updated profile must be readable");
    assert_eq!(
        attribute_count(&at_the_cap),
        Some(cap),
        "case \"at-the-cap\": expected {cap} attributes stored, got {:?}",
        attribute_count(&at_the_cap)
    );

    // Replacing keys already stored does not grow the map, so it is still accepted.
    let (status, body) = update(
        &emulator,
        "capdom",
        &profile_id,
        json!({ "Attributes": { "attr0": "replaced" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"replacing-a-key-at-the-cap\": expected UpdateProfile 200, got {status} {body:?}"
    );
    let replaced = read_profile(&emulator, "capdom", &profile_id)
        .await
        .expect("the updated profile must be readable");
    assert_eq!(
        (attribute_count(&replaced), attribute(&replaced, "attr0")),
        (Some(cap), Some("replaced")),
        "case \"replacing-a-key-at-the-cap\": expected {cap} attributes with attr0 replaced, got {:?}",
        attribute_count(&replaced)
    );

    // One fresh key past the cap, reached by ACCUMULATION across requests.
    let (status, body) = update(
        &emulator,
        "capdom",
        &profile_id,
        json!({ "Attributes": { "one-too-many": "v" } }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case \"one-past-the-cap-across-requests\": expected 400 BadRequestException, got {status} {body:?}"
    );
    assert!(
        error_message(&body).contains(&cap.to_string()),
        "case \"one-past-the-cap-across-requests\": the refusal must name the limit, got {body:?}"
    );
    let unchanged = read_profile(&emulator, "capdom", &profile_id)
        .await
        .expect("the stored profile must still be readable");
    assert_eq!(
        (
            attribute_count(&unchanged),
            attribute(&unchanged, "one-too-many")
        ),
        (Some(cap), None),
        "case \"one-past-the-cap-across-requests\": a refused update must write nothing, got {:?} attributes",
        attribute_count(&unchanged)
    );

    // One request that lands one past the cap on its own.
    let fresh = emulator
        .create_profile("capdom", json!({ "FirstName": "Grace" }))
        .await;
    let (status, body) = update(
        &emulator,
        "capdom",
        &fresh,
        json!({ "Attributes": attributes(0, cap + 1) }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case \"one-past-the-cap-in-one-request\": expected 400 BadRequestException, got {status} {body:?}"
    );
    let still_bare = read_profile(&emulator, "capdom", &fresh)
        .await
        .expect("the stored profile must still be readable");
    assert_eq!(
        attribute_count(&still_bare),
        None,
        "case \"one-past-the-cap-in-one-request\": a refused update must write nothing, got {:?} attributes",
        attribute_count(&still_bare)
    );
}

/// How many attributes a profile read back carries, or `None` when it carries no
/// `Attributes` member at all.
fn attribute_count(profile: &Value) -> Option<usize> {
    profile
        .get("Attributes")
        .and_then(Value::as_object)
        .map(serde_json::Map::len)
}

/// The value a profile read back carries for one attribute key.
fn attribute<'a>(profile: &'a Value, key: &str) -> Option<&'a str> {
    profile
        .get("Attributes")
        .and_then(|attributes| attributes.get(key))
        .and_then(Value::as_str)
}
