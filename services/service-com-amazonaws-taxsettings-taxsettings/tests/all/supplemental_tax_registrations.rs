// SPDX-License-Identifier: CC0-1.0

//! The supplemental tax registration (supplemental TRN) lifecycle, end to end
//! over the restJson1 wire.
//!
//! The rules under test:
//!
//! * `ListSupplementalTaxRegistrations` reports, from persisted state, every
//!   supplemental TRN the account has stored and nothing else, in the store's
//!   order, in pages of at most the server-decided size, issuing a resume cursor
//!   on every page but the last, and changing nothing a later caller sees.
//! * `PutSupplementalTaxRegistration` stores the submitted entry under an
//!   `authorityId` keyed on the TRN it names (its `registrationType` and
//!   `registrationId`), so writing the same TRN again updates that one record
//!   and a TRN the account does not hold yet gets its own id.
//! * only a jurisdiction this service supports supplemental registration in may
//!   hold one — the boundary is observed behaviour, not a model constraint, and
//!   a country code outside the member's own constraint takes the same refusal
//!   class — and an `authorityId` that names no live registration cannot be
//!   deleted.
//!
//! Every table below spans the class its rule generalises over rather than one
//! recorded request: several address shapes and country codes, the published
//! bounds of the constrained members plus one past them, several store sizes
//! against several page sizes, each form of a `nextToken` the service did not
//! issue, and several shapes of `authorityId`.
//!
//! The harness (`emulator`, `emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::ops::types::{Address, TaxRegistrationEntry};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, StoredTaxRegistration,
    ERROR_CODE_FIELD_VALIDATION_FAILED, ERROR_CODE_INVALID_TOKEN, ERROR_CODE_MALFORMED_TOKEN,
    STATUS_PENDING, STATUS_VERIFIED, SUPPLEMENTAL_COUNTRY_NOT_SUPPORTED,
    SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS, SUPPLEMENTAL_TRN_COUNTRIES, TRN_VALIDATION_DELAY,
};

use crate::declared_member_defaults::address_as_stored;
use crate::tax_registration_lifecycle::{call, emulator_with_engine, Answer};

/// Hold every stored supplemental TRN's validation open, so a listing reports
/// the status the write reported however long the test takes to get there.
pub(crate) fn hold_supplemental_validations_open(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    for stored in store.supplemental_registrations.values_mut() {
        stored.validation_due_at = Some(Instant::now() + Duration::from_secs(3_600));
    }
}

/// Make every stored supplemental TRN's validation due, the way the clock would
/// have: the next listing must report what the validation reached. Never a
/// sleep — the due instant is state.
pub(crate) fn make_supplemental_validations_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    for stored in store.supplemental_registrations.values_mut() {
        stored.validation_due_at = Some(Instant::now());
    }
}

/// Make the presented account hold a tax registration of its own whose
/// validation has already run — the account state a supplemental registration
/// SUPPLEMENTS
/// (`state::ComAmazonawsTaxsettingsTaxsettingsState::store_supplemental_registration`).
///
/// Seeded through the same store the handlers read rather than over the wire,
/// because what every case below is about is the supplemental write, not the
/// primary one; the account-state conflict itself is pinned in
/// `tests/all/supplemental_trn_conflict.rs`.
pub(crate) fn hold_a_settled_own_registration(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    let account_id = engine::emulator_account_id();
    let entry = TaxRegistrationEntry {
        registration_id: "DE136695976".to_string(),
        registration_type: "VAT".to_string(),
        legal_name: Some("Acme GmbH".to_string()),
        legal_address: Some(Address {
            postal_code: "10117".to_string(),
            country_code: "DE".to_string(),
            ..Address::default()
        }),
        ..TaxRegistrationEntry::default()
    };
    store.registrations.insert(
        account_id.to_string(),
        StoredTaxRegistration::from_entry(&entry, account_id, 1, Instant::now()),
    );
    // What the clock owed on that write is settled, so the account's own TRN is
    // not an outstanding one.
    store.settle_due(Instant::now() + TRN_VALIDATION_DELAY);
}

/// An emulator whose presented account already holds that settled registration.
pub(crate) fn emulator_supplementing() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
) {
    let (router, engine) = emulator_with_engine();
    hold_a_settled_own_registration(&engine);
    (router, engine)
}

/// A supplemental entry for `country_code` registering `registration_id`.
/// Everything else satisfies every constraint on
/// `SupplementalTaxRegistrationEntry`.
pub(crate) fn entry_for(country_code: &str, registration_id: &str, legal_name: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "VAT",
        "legalName": legal_name,
        "address": {
            "addressLine1": "1 Example Street",
            "city": "Example City",
            "postalCode": "8001",
            "countryCode": country_code,
        },
    })
}

/// One `SupplementalTaxRegistrationEntry` to store.
struct Case {
    name: &'static str,
    entry: Value,
}

/// The entries the round-trip table stores.
///
/// `registrationType` has exactly one value in the model
/// (`smithy_query(query="shape", shape="SupplementalTaxRegistrationType")`
/// answers `["VAT"]`), so the dimensions this table spans are the ones the model
/// leaves open: `registrationId` and `legalName` from one character to the
/// 1..=200 bound, and the `Address` from only its two required members
/// (`postalCode`, `countryCode`) to every optional line this jurisdiction accepts
/// filled in (`addressLine3` the API accepts only for Saudi Arabia — see
/// `crate::address_member_jurisdictions`), in both cases of the two-letter
/// country code.
///
/// Every row registers a Swiss address: which jurisdictions a supplemental TRN
/// may be registered for is a SEPARATE rule, with its own recording and its own
/// issue (`issues/.../taxsettings-supplemental-trn-and-batch-tax-registration-write-paths.md`,
/// cause 3), and `CH` is the country that recording shows clearing country
/// validation. Nothing here asserts anything about the others, so adding that
/// rule does not have to rewrite this table.
fn round_trip_cases() -> Vec<Case> {
    vec![
        Case {
            name: "only the address members the model requires",
            entry: json!({
                "registrationId": "CHE-123.456.789",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
        },
        Case {
            name: "every optional address member this jurisdiction accepts filled in",
            entry: json!({
                "registrationId": "CHE-987.654.321",
                "registrationType": "VAT",
                "legalName": "Acme Holding AG",
                "address": {
                    "addressLine1": "Bahnhofstrasse 1",
                    "addressLine2": "Floor 4",
                    "districtOrCounty": "Zürich",
                    "city": "Zürich",
                    "stateOrRegion": "ZH",
                    "postalCode": "8001",
                    "countryCode": "CH",
                },
            }),
        },
        Case {
            name: "registration id and legal name at the 200-character bound",
            entry: json!({
                "registrationId": "A".repeat(200),
                "registrationType": "VAT",
                "legalName": "N".repeat(200),
                "address": { "postalCode": "1211", "countryCode": "CH" },
            }),
        },
        Case {
            name: "registration id and legal name at the 1-character bound",
            entry: json!({
                "registrationId": "1",
                "registrationType": "VAT",
                "legalName": "A",
                "address": { "postalCode": "1", "countryCode": "CH" },
            }),
        },
        Case {
            name: "lower-case country code and non-ASCII legal name",
            entry: json!({
                "registrationId": "CHE-111.222.333",
                "registrationType": "VAT",
                "legalName": "Compañía Española S.à r.l.",
                "address": {
                    "addressLine1": "Rue du Rhône 1",
                    "city": "Genève",
                    "postalCode": "1204",
                    "countryCode": "ch",
                },
            }),
        },
    ]
}

/// Store `entry` and answer the whole call, asserting nothing: a case that
/// expects a refusal reads the same answer.
pub(crate) async fn put(
    router: &smithy_server_runtime::MultiServiceRouter,
    entry: &Value,
) -> Answer {
    call(
        router,
        "PutSupplementalTaxRegistration",
        &json!({ "taxRegistrationEntry": entry }),
    )
    .await
}

/// RULE: a supplemental TRN is stored only for a jurisdiction this service
/// supports supplemental registration in; an entry for any other country is
/// refused with the modelled `ValidationException` (HTTP 400), stating that the
/// country is not supported and naming the member it is about, and creates
/// nothing.
///
/// The jurisdiction boundary is observed behaviour, not a model constraint (see
/// `crate::state::SUPPLEMENTAL_TRN_COUNTRIES`), so both sides of it are pinned
/// here: a supported jurisdiction stores and reads back, and the refusal reaches
/// countries the model's two-letter `countryCode` admits and no constraint
/// excludes — including ones no observation has placed on either side of the
/// boundary.
#[tokio::test]
async fn only_a_supported_jurisdiction_can_hold_a_supplemental_registration() {
    assert!(
        !SUPPLEMENTAL_TRN_COUNTRIES.is_empty(),
        "the supported set must name at least one jurisdiction"
    );

    // The supported side: every row of the catalog, in both cases the model's
    // `CountryCode` pattern (`^[a-zA-Z]+$`) admits.
    for (country_code, country) in SUPPLEMENTAL_TRN_COUNTRIES {
        for spelling in [country_code.to_uppercase(), country_code.to_lowercase()] {
            let case = format!("{country} as {spelling:?}");
            let (router, _engine) = emulator_supplementing();
            let answer = put(&router, &entry_for(&spelling, "CHE123456789", "Acme AG")).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a supported jurisdiction must store, got {answer:?}"
            );
            let authority_id = answer
                .get("authorityId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !authority_id.is_empty() && authority_id.chars().count() <= 200,
                "case {case:?}: the write must report the authority it filed, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the write reports the status it stored, got {:?}",
                answer.body
            );
            // The record is readable back under the authority the write minted,
            // with the country code as submitted: the write stored it, it did
            // not merely answer 200.
            let listed = list_all(&router, None).await;
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: the registration must be readable back, got {listed:?}"
            );
            let record = &listed[0];
            assert_eq!(
                record.get("authorityId").and_then(Value::as_str),
                Some(authority_id.as_str()),
                "case {case:?}: the record is reported under the authority the write minted, got \
                 {record:?}"
            );
            assert_eq!(
                record
                    .get("address")
                    .and_then(|address| address.get("countryCode"))
                    .and_then(Value::as_str),
                Some(spelling.as_str()),
                "case {case:?}: the record reports the country code as submitted, got {record:?}"
            );
        }
    }

    // The unsupported side spans the class the rule generalises over rather than
    // one request: the two countries the recorded refusal cites (FR, GB), the
    // further EU jurisdictions a VAT number is held in (DE, ES, NL, ie), and
    // other continents (US, IN, JP, BR, ZA, au) — in both cases the member's
    // pattern admits. None is excluded by any model constraint, so each is
    // refused for its jurisdiction alone.
    for country_code in [
        "FR", "GB", "DE", "ES", "NL", "ie", "US", "IN", "JP", "BR", "ZA", "au",
    ] {
        if SUPPLEMENTAL_TRN_COUNTRIES
            .iter()
            .any(|(code, _)| code.eq_ignore_ascii_case(country_code))
        {
            continue;
        }
        let (router, _engine) = emulator_supplementing();
        let answer = put(
            &router,
            &entry_for(country_code, "XX12345678901", "Acme Overseas"),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {country_code:?}: an unsupported jurisdiction must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {country_code:?}: the refusal carries the field-validation code, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.get("message").as_str(),
            Some(SUPPLEMENTAL_COUNTRY_NOT_SUPPORTED),
            "case {country_code:?}: the refusal states that the country is not supported, got {:?}",
            answer.body
        );
        let fields: Vec<&str> = answer
            .get("fieldList")
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["COUNTRY_CODE"],
            "case {country_code:?}: the refusal names the member it is about, got {:?}",
            answer.body
        );
        assert!(
            list_all(&router, None).await.is_empty(),
            "case {country_code:?}: a refused write must create nothing"
        );
    }
}

/// RULE: a `countryCode` outside its modelled constraint (exactly two letters)
/// takes the same 400 `ValidationException` class as an unsupported
/// jurisdiction, so the refusal does not depend on WHY the country cannot be
/// registered for, and neither kind of refused write stores anything.
#[tokio::test]
async fn a_country_code_outside_its_modelled_constraint_is_refused_too() {
    // `Address$countryCode` is `@required` with `@length 2..2` and pattern
    // `^[a-zA-Z]+$` (`smithy_query(query="shape",
    // shape="com.amazonaws.taxsettings#Address")`): one character either side of
    // the published bound, a non-letter inside it, and the member omitted.
    // `None` is the omission. The body of a boundary refusal is the request
    // walk's, which states the constraint that failed rather than this
    // service's own field-validation code, so the class and a stated reason are
    // what is asserted.
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("one letter, below the length bound", Some(json!("C"))),
        ("three letters, above the length bound", Some(json!("CHE"))),
        ("empty", Some(json!(""))),
        ("a letter and a digit", Some(json!("C1"))),
        ("two digits", Some(json!("41"))),
        ("punctuation", Some(json!("C-"))),
        ("the required member omitted", None),
    ];
    for (case, country_code) in &cases {
        let (router, _engine) = emulator_supplementing();
        let mut entry = entry_for("CH", "CHE123456789", "Acme AG");
        let address = entry
            .get_mut("address")
            .and_then(Value::as_object_mut)
            .expect("the entry's address is an object");
        match country_code {
            Some(value) => {
                address.insert("countryCode".to_string(), value.clone());
            }
            None => {
                address.remove("countryCode");
            }
        }
        let answer = put(&router, &entry).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a country code outside its constraint must be refused, got {answer:?}"
        );
        assert!(
            answer
                .get("message")
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: a refusal states what it refused, got {:?}",
            answer.body
        );
        assert!(
            list_all(&router, None).await.is_empty(),
            "case {case:?}: a refused write must create nothing"
        );
    }
}

/// Every supplemental TRN the listing reports, following its cursor to the end.
pub(crate) async fn list_all(
    router: &smithy_server_runtime::MultiServiceRouter,
    page_size: Option<i64>,
) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = serde_json::Map::new();
        if let Some(size) = page_size {
            request.insert("maxResults".to_string(), json!(size));
        }
        if let Some(ref cursor) = token {
            request.insert("nextToken".to_string(), json!(cursor));
        }
        let page = call(
            router,
            "ListSupplementalTaxRegistrations",
            &Value::Object(request),
        )
        .await;
        assert_eq!(
            page.status, 200,
            "listing expected 200, got {} {:?}",
            page.status, page.body
        );
        let items = page
            .get("taxRegistrations")
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("taxRegistrations is required, got {:?}", page.body));
        all.extend(items);
        token = page
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// RULE: a stored supplemental TRN reads back through the listing with every
/// member the write submitted, under the `authorityId` the write reported, in
/// the status the write reported; distinct TRNs get distinct ids and the listing
/// reports exactly the stored set.
#[tokio::test]
async fn a_stored_supplemental_trn_reads_back_through_the_listing() {
    let (router, engine) = emulator_supplementing();
    let cases = round_trip_cases();
    let mut authority_ids: Vec<String> = Vec::new();

    for case in &cases {
        let stored = put(&router, &case.entry).await;
        assert_eq!(
            stored.status, 200,
            "case {:?}: expected HTTP 200 from PutSupplementalTaxRegistration, got {} {:?}",
            case.name, stored.status, stored.body
        );
        let authority_id = stored
            .get("authorityId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !authority_id.is_empty() && authority_id.chars().count() <= 200,
            "case {:?}: authorityId is a required GenericString of 1..=200 characters, got {:?}",
            case.name,
            stored.get("authorityId")
        );
        assert_eq!(
            stored.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {:?}: expected the write to report {STATUS_PENDING:?}, got {:?}",
            case.name,
            stored.get("status")
        );
        authority_ids.push(authority_id);
    }

    let unique: BTreeSet<&String> = authority_ids.iter().collect();
    assert_eq!(
        unique.len(),
        authority_ids.len(),
        "each supplemental TRN must get its own authorityId, got {authority_ids:?}"
    );

    // A read settles the validation the clock has made due, which is a change
    // the clock owed rather than one the read made: hold them open so what is
    // left to observe is only what the write stored.
    hold_supplemental_validations_open(&engine);
    let listed = list_all(&router, None).await;
    assert_eq!(
        listed.len(),
        cases.len(),
        "the listing must report exactly the stored set, got {listed:?}"
    );

    for (case, authority_id) in cases.iter().zip(&authority_ids) {
        let reported = listed
            .iter()
            .find(|item| item.get("authorityId").and_then(Value::as_str) == Some(authority_id))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the listing reports no record under {authority_id:?}: {listed:?}",
                    case.name
                )
            });
        let submitted = case.entry.as_object().expect("the entry is an object");
        for (member, value) in submitted {
            // `address` is reported with the defaults the model declares for
            // `addressLine1` and `city` materialised; every other member is
            // reported verbatim.
            let expected = if member == "address" {
                address_as_stored(value)
            } else {
                value.clone()
            };
            assert_eq!(
                reported.get(member),
                Some(&expected),
                "case {:?}: the listing must report {member:?} as {expected:?}, got {:?}",
                case.name,
                reported.get(member)
            );
        }
        assert_eq!(
            reported.get("status").and_then(Value::as_str),
            Some(STATUS_PENDING),
            "case {:?}: expected the listing to report {STATUS_PENDING:?} while the validation is \
             outstanding, got {:?}",
            case.name,
            reported.get("status")
        );
    }
}

/// RULE: the TRN an entry names is the identity of the record, so writing the
/// same `registrationType` and `registrationId` again updates that record under
/// the `authorityId` it already has, and writing a different one adds a record.
#[tokio::test]
async fn rewriting_a_supplemental_trn_updates_it_under_the_same_authority_id() {
    // (what differs in the second write, the second entry, whether the second
    // write addresses the SAME record).
    let first = json!({
        "registrationId": "CHE-123.456.789",
        "registrationType": "VAT",
        "legalName": "Acme AG",
        "address": { "postalCode": "8001", "countryCode": "CH" },
    });
    let rewrites: &[(&str, Value, bool)] = &[
        ("the same entry again", first.clone(), true),
        (
            "a different legal name",
            json!({
                "registrationId": "CHE-123.456.789",
                "registrationType": "VAT",
                "legalName": "Acme Holding AG",
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
            true,
        ),
        (
            "a different address",
            json!({
                "registrationId": "CHE-123.456.789",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": {
                    "addressLine1": "Bahnhofstrasse 2",
                    "city": "Zürich",
                    "postalCode": "8002",
                    "countryCode": "CH",
                },
            }),
            true,
        ),
        (
            "a different registration id",
            json!({
                "registrationId": "CHE-987.654.321",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
            false,
        ),
    ];

    for (case, second, same_record) in rewrites {
        let (router, engine) = emulator_supplementing();
        let stored = put(&router, &first).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the first write failed: {:?}",
            stored.body
        );
        let first_id = stored
            .get("authorityId")
            .as_str()
            .unwrap_or_default()
            .to_string();

        // A write aimed at a record whose own validation is still outstanding
        // conflicts with the current state of that record (pinned in
        // `tests/all/supplemental_trn_conflict.rs`), so let the clock settle what
        // it owed before the second write: what this table is about is which
        // record the second write addresses.
        make_supplemental_validations_due(&engine);

        let rewritten = put(&router, second).await;
        assert_eq!(
            rewritten.status, 200,
            "case {case:?}: the second write failed: {:?}",
            rewritten.body
        );
        let second_id = rewritten
            .get("authorityId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if *same_record {
            assert_eq!(
                second_id, first_id,
                "case {case:?}: a write of the same TRN must answer the authorityId it was first \
                 stored under, got {second_id:?} after {first_id:?}"
            );
        } else {
            assert_ne!(
                second_id, first_id,
                "case {case:?}: a write of a TRN the account does not hold must mint its own \
                 authorityId, got {second_id:?}"
            );
        }

        hold_supplemental_validations_open(&engine);
        let listed = list_all(&router, None).await;
        let expected_len = if *same_record { 1 } else { 2 };
        assert_eq!(
            listed.len(),
            expected_len,
            "case {case:?}: expected {expected_len} stored record(s), got {listed:?}"
        );
        let reported = listed
            .iter()
            .find(|item| item.get("authorityId").and_then(Value::as_str) == Some(&second_id))
            .unwrap_or_else(|| panic!("case {case:?}: no record under {second_id:?}: {listed:?}"));
        for (member, value) in second.as_object().expect("the entry is an object") {
            let expected = if member == "address" {
                address_as_stored(value)
            } else {
                value.clone()
            };
            assert_eq!(
                reported.get(member),
                Some(&expected),
                "case {case:?}: the listing must report the last write's {member:?} as \
                 {expected:?}, got {:?}",
                reported.get(member)
            );
        }
    }
}

/// Store `count` supplemental TRNs and answer their `authorityId`s in the order
/// the writes minted them.
async fn seed(
    router: &smithy_server_runtime::MultiServiceRouter,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let stored = put(
            router,
            &json!({
                "registrationId": format!("VAT-{index:05}"),
                "registrationType": "VAT",
                "legalName": format!("Account {index} AG"),
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "seeding record {index} failed: {:?}",
            stored.body
        );
        ids.push(
            stored
                .get("authorityId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    ids
}

/// RULE: paging the listing emits every stored supplemental TRN exactly once, in
/// the store's order, in pages of at most the SERVER-decided page size, and
/// issues a resume cursor on every page but the last.
///
/// The server-decided size is `maxResults` when the request names one (its
/// modelled range is 1..=1000, so 1 and 1000 are its published bounds), and this
/// operation's own default when it does not
/// ([`SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS`]) — never the whole store.
#[tokio::test]
async fn paging_emits_every_supplemental_registration_exactly_once_in_order() {
    // (stored registrations, requested page size).
    let shapes: &[(usize, Option<i64>)] = &[
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (5, Some(1)),
        (5, Some(2)),
        (5, Some(4)),
        (5, Some(5)),
        (5, Some(6)),
        (5, Some(1000)),
        (7, Some(3)),
        (7, None),
        // More records than the default page size, so the clamp on a request
        // that names none is observable: it must still page rather than return
        // everything at once.
        (SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS.default_size + 5, None),
        (
            SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS.default_size + 5,
            Some(1000),
        ),
    ];
    for (count, page_size) in shapes.iter().copied() {
        let case = format!("{count} supplemental registrations, page size {page_size:?}");
        let (router, _engine) = emulator_supplementing();
        let expected = seed(&router, count).await;

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        let bound = SUPPLEMENTAL_TAX_REGISTRATIONS_PAGE_BOUNDS
            .page_size(page_size.and_then(|size| i32::try_from(size).ok()));
        loop {
            let mut request = serde_json::Map::new();
            if let Some(size) = page_size {
                request.insert("maxResults".to_string(), json!(size));
            }
            if let Some(ref cursor) = token {
                request.insert("nextToken".to_string(), json!(cursor));
            }
            let page = call(
                &router,
                "ListSupplementalTaxRegistrations",
                &Value::Object(request),
            )
            .await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: page {pages} expected 200, got {} {:?}",
                page.status, page.body
            );
            let items = page
                .get("taxRegistrations")
                .as_array()
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: taxRegistrations is required, got {:?}",
                        page.body
                    )
                });
            assert!(
                items.len() <= bound,
                "case {case:?}: page {pages} returned {} items, more than the {bound} the server \
                 allows",
                items.len()
            );
            for item in &items {
                seen.push(
                    item.get("authorityId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            token = page
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= count + 2,
                "case {case:?}: the listing kept issuing cursors after {pages} pages"
            );
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every stored record exactly once in the store's order"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: the listing must enumerate in the store's (authorityId) order"
        );
    }
}

/// RULE: a listing changes nothing a later caller sees — only the validation the
/// clock had already made due, which was owed whether or not anybody called.
#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = emulator_supplementing();
    seed(&router, 3).await;
    hold_supplemental_validations_open(&engine);

    let mut reads = Vec::new();
    for _ in 0..3 {
        reads.push(list_all(&router, Some(2)).await);
    }
    assert!(
        reads.windows(2).all(|pair| pair[0] == pair[1]),
        "a listing changed what the next listing saw: {reads:?}"
    );

    // The clock-owed transition, materialised by the clock and not by the read:
    // once the validation is due, every later listing reports what it reached.
    make_supplemental_validations_due(&engine);
    let settled = list_all(&router, None).await;
    for item in &settled {
        assert_eq!(
            item.get("status").and_then(Value::as_str),
            Some(STATUS_VERIFIED),
            "once the validation is due the listing must report the outcome it reached, got \
             {item:?}"
        );
    }
    let again = list_all(&router, None).await;
    assert_eq!(
        settled, again,
        "settling is owed once: a later listing must report the same records"
    );
}

/// RULE: a request outside what the model admits, or carrying a cursor this
/// service did not issue, is refused with `ValidationException` (HTTP 400) and
/// nothing is stored; a request at the published bounds is served.
#[tokio::test]
async fn requests_outside_the_model_are_refused() {
    // A token that decodes to a key longer than the 1..=200 `GenericString` an
    // `authorityId` is, so it names nothing the listing could resume from.
    let overlong_key_token = aws_smithy_types::base64::encode("k".repeat(201).as_str());

    // (case, body, expected status, expected `errorCode` when refused).
    let listings: &[(&str, Value, u16, &str)] = &[
        // `maxResults`: range 1..=1000 — each published bound, and one past each.
        ("maxResults below the range", json!({ "maxResults": 0 }), 400, ""),
        ("maxResults at the low bound", json!({ "maxResults": 1 }), 200, ""),
        (
            "maxResults at the high bound",
            json!({ "maxResults": 1000 }),
            200,
            "",
        ),
        (
            "maxResults above the range",
            json!({ "maxResults": 1001 }),
            400,
            "",
        ),
        ("negative maxResults", json!({ "maxResults": -1 }), 400, ""),
        // `nextToken`: length 1..2000, pattern ^[-A-Za-z0-9_+\=\/]+$, and the
        // cursor must be one this service issued.
        (
            "nextToken outside the modelled pattern",
            json!({ "nextToken": "not a token!" }),
            400,
            "",
        ),
        (
            "nextToken that is not decodable",
            json!({ "nextToken": "A" }),
            400,
            ERROR_CODE_MALFORMED_TOKEN,
        ),
        (
            "nextToken naming a key no authorityId could be",
            json!({ "nextToken": overlong_key_token.clone() }),
            400,
            ERROR_CODE_INVALID_TOKEN,
        ),
        // A well-formed cursor past every stored key is not a refusal: it names
        // a position in the store, and there is simply nothing after it.
        (
            "nextToken past every stored record",
            json!({ "nextToken": aws_smithy_types::base64::encode("zzzzzzzz") }),
            200,
            "",
        ),
    ];

    for (case, body, status, error_code) in listings {
        let (router, _engine) = emulator_supplementing();
        seed(&router, 2).await;
        let answer = call(&router, "ListSupplementalTaxRegistrations", body).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            answer.status, answer.body
        );
        if *status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected a ValidationException, got {:?} {:?}",
                answer.error_code, answer.body
            );
        }
        if !error_code.is_empty() {
            assert_eq!(
                answer.get("errorCode").as_str(),
                Some(*error_code),
                "case {case:?}: expected errorCode {error_code:?}, got {:?}",
                answer.body
            );
        }
    }

    // The write's own refusals: the model's non-blank pattern
    // `^(?!\s*$)[\s\S]+$` on `registrationId`, `legalName` and the address
    // lines, and the members the model marks @required.
    let valid = json!({
        "registrationId": "CHE-123.456.789",
        "registrationType": "VAT",
        "legalName": "Acme AG",
        "address": { "postalCode": "8001", "countryCode": "CH" },
    });
    // (case, member of the entry to replace, the value to replace it with,
    // expected status, expected `errorCode` when refused).
    let writes: &[(&str, &str, Value, u16, &str)] = &[
        (
            "blank registrationId",
            "registrationId",
            json!("   "),
            400,
            ERROR_CODE_FIELD_VALIDATION_FAILED,
        ),
        (
            "blank legalName",
            "legalName",
            json!("\t"),
            400,
            ERROR_CODE_FIELD_VALIDATION_FAILED,
        ),
        (
            "blank address line",
            "address",
            json!({ "addressLine1": " ", "postalCode": "8001", "countryCode": "CH" }),
            400,
            ERROR_CODE_FIELD_VALIDATION_FAILED,
        ),
        (
            "registrationId one past the 200-character bound",
            "registrationId",
            json!("A".repeat(201)),
            400,
            "",
        ),
        (
            "registrationType outside the SupplementalTaxRegistrationType enum",
            "registrationType",
            json!("GST"),
            400,
            "",
        ),
        (
            "countryCode of one letter",
            "address",
            json!({ "postalCode": "8001", "countryCode": "C" }),
            400,
            "",
        ),
        (
            "registrationId at the 200-character bound",
            "registrationId",
            json!("A".repeat(200)),
            200,
            "",
        ),
    ];

    for (case, member, value, status, error_code) in writes {
        let (router, _engine) = emulator_supplementing();
        let mut entry = valid.clone();
        entry
            .as_object_mut()
            .expect("the entry is an object")
            .insert((*member).to_string(), value.clone());
        let answer = put(&router, &entry).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status} from PutSupplementalTaxRegistration, got {} \
             {:?}",
            answer.status, answer.body
        );
        if *status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected a ValidationException, got {:?} {:?}",
                answer.error_code, answer.body
            );
            if !error_code.is_empty() {
                assert_eq!(
                    answer.get("errorCode").as_str(),
                    Some(*error_code),
                    "case {case:?}: expected errorCode {error_code:?}, got {:?}",
                    answer.body
                );
            }
            // A refused write persists nothing.
            let listed = list_all(&router, None).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused write must store nothing, got {listed:?}"
            );
        }
    }
}

/// RULE: deleting a supplemental registration removes it, and an `authorityId`
/// that names no live registration is refused with the modelled
/// `ValidationException` (HTTP 400) — whatever its shape, and however many
/// callers ask at once.
#[tokio::test]
async fn deleting_by_an_authority_that_names_nothing_is_refused() {
    let supported = SUPPLEMENTAL_TRN_COUNTRIES[0].0;

    // Rows span the shapes an authority id can take: junk, a well-formed UUID,
    // one character, the longest string the model admits, and punctuation the
    // member's `^[\s\S]*$` pattern allows.
    let unknown: Vec<(&str, String)> = vec![
        ("a junk string", "no-such-authority-7f3".to_string()),
        (
            "a well-formed UUID",
            "123e4567-e89b-12d3-a456-426614174000".to_string(),
        ),
        ("one character", "x".to_string()),
        ("the longest id the model admits", "a".repeat(200)),
        ("punctuation", "../../etc/passwd".to_string()),
    ];
    for (case, authority_id) in &unknown {
        let (router, _engine) = emulator_supplementing();
        // An account holding a registration still refuses an authority that is
        // not that registration's.
        let stored = put(&router, &entry_for(supported, "CHE400000001", "Acme AG")).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: setup write failed: {stored:?}"
        );
        let answer = call(
            &router,
            "DeleteSupplementalTaxRegistration",
            &json!({ "authorityId": authority_id }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: an unknown authority must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {case:?}: the refusal carries the field-validation code, got {:?}",
            answer.body
        );
        assert_eq!(
            list_all(&router, None).await.len(),
            1,
            "case {case:?}: a refused delete must remove nothing"
        );
    }

    // The live authority deletes, and deleting it again is refused the same way.
    let (router, _engine) = emulator_supplementing();
    let stored = put(&router, &entry_for(supported, "CHE400000002", "Acme AG")).await;
    let authority_id = stored
        .get("authorityId")
        .as_str()
        .expect("an authority")
        .to_string();
    let deleted = call(
        &router,
        "DeleteSupplementalTaxRegistration",
        &json!({ "authorityId": authority_id }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "a live authority must delete, got {deleted:?}"
    );
    assert!(
        list_all(&router, None).await.is_empty(),
        "the deleted registration must be gone"
    );
    let again = call(
        &router,
        "DeleteSupplementalTaxRegistration",
        &json!({ "authorityId": authority_id }),
    )
    .await;
    assert_eq!(
        (again.status, again.error_code.as_str()),
        (400, "ValidationException"),
        "a second delete of one authority must be refused, got {again:?}"
    );
}

/// RULE: concurrent callers see ONE consistent outcome — eight identical deletes
/// of an authority that names nothing all get the 400, and eight of a live one
/// delete it exactly once.
#[tokio::test]
async fn concurrent_deletes_of_one_authority_agree() {
    let supported = SUPPLEMENTAL_TRN_COUNTRIES[0].0;

    // An authority that names nothing: every caller is refused.
    let (router, _engine) = emulator_supplementing();
    let body = json!({ "authorityId": "no-such-authority-concurrent" });
    let mut codes = Vec::new();
    for answer in eight_deletes(&router, &body).await {
        codes.push((answer.status, answer.error_code.clone()));
    }
    assert!(
        codes
            .iter()
            .all(|(status, code)| *status == 400 && code == "ValidationException"),
        "case \"unknown authority\": every caller must see the same refusal, got {codes:?}"
    );

    // A live authority: exactly one caller deletes it, the rest are refused, and
    // the registration is gone.
    let (router, _engine) = emulator_supplementing();
    let stored = put(&router, &entry_for(supported, "CHE500000001", "Acme AG")).await;
    let authority_id = stored
        .get("authorityId")
        .as_str()
        .expect("an authority")
        .to_string();
    let body = json!({ "authorityId": authority_id });
    let answers = eight_deletes(&router, &body).await;
    let deleted = answers.iter().filter(|answer| answer.status == 200).count();
    let refused = answers
        .iter()
        .filter(|answer| answer.status == 400 && answer.error_code == "ValidationException")
        .count();
    assert_eq!(
        (deleted, refused),
        (1, 7),
        "case \"live authority\": exactly one delete succeeds, got {answers:?}"
    );
    assert!(
        list_all(&router, None).await.is_empty(),
        "case \"live authority\": the registration must be gone"
    );
}

/// Eight `DeleteSupplementalTaxRegistration` calls in flight at once.
async fn eight_deletes(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: &Value,
) -> Vec<Answer> {
    let one = || call(router, "DeleteSupplementalTaxRegistration", body);
    let (a, b, c, d, e, f, g, h) =
        tokio::join!(one(), one(), one(), one(), one(), one(), one(), one());
    vec![a, b, c, d, e, f, g, h]
}

/// RULE: a required member the request omits is refused, not stored.
#[tokio::test]
async fn an_entry_missing_a_required_member_is_refused() {
    let valid = json!({
        "registrationId": "CHE-123.456.789",
        "registrationType": "VAT",
        "legalName": "Acme AG",
        "address": { "postalCode": "8001", "countryCode": "CH" },
    });
    // Every @required member of `SupplementalTaxRegistrationEntry`
    // (`smithy_query(query="shape", shape="SupplementalTaxRegistrationEntry")`),
    // plus each @required member of the `Address` it carries.
    for member in ["registrationId", "registrationType", "legalName", "address"] {
        let (router, _engine) = emulator_supplementing();
        let mut entry = valid.clone();
        entry
            .as_object_mut()
            .expect("the entry is an object")
            .remove(member);
        let answer = put(&router, &entry).await;
        assert_eq!(
            answer.status, 400,
            "case {member:?}: expected HTTP 400 for an entry missing a required member, got {} \
             {:?}",
            answer.status, answer.body
        );
        let listed = list_all(&router, None).await;
        assert!(
            listed.is_empty(),
            "case {member:?}: a refused write must store nothing, got {listed:?}"
        );
    }

    for member in ["postalCode", "countryCode"] {
        let (router, _engine) = emulator_supplementing();
        let mut entry = valid.clone();
        entry["address"]
            .as_object_mut()
            .expect("the address is an object")
            .remove(member);
        let answer = put(&router, &entry).await;
        assert_eq!(
            answer.status, 400,
            "case {member:?}: expected HTTP 400 for an address missing a required member, got {} \
             {:?}",
            answer.status, answer.body
        );
    }
}
