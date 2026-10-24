// SPDX-License-Identifier: CC0-1.0

//! `BatchGetTaxExemptions`: the tax exemptions this service reports for the
//! accounts a caller names, end to end over the restJson1 wire.
//!
//! The rules under test:
//!
//! * the call reports, for each account it names, the exemptions this service
//!   holds for that account — every stored record, member for member — and an
//!   account it holds none for simply has no entry in the reported map;
//! * an account this service holds no tax settings for and that is not the
//!   caller's own is outside the caller's consolidated billing family, so the
//!   CALL is refused with `ResourceNotFoundException` (404) rather than reported
//!   on;
//! * an exemption whose expiration date has passed is reported `Expired`,
//!   because lapsing is a transition the clock owes; every other recorded status
//!   is reported as recorded;
//! * reading is a report: it changes neither the exemptions nor anything beside
//!   them.
//!
//! The tables below span the classes those rules generalise over — the kinds of
//! evidence an account is in the family (the caller's own, a stored TRN, stored
//! exemptions), the number of accounts named (one, the model's `@length` bound of
//! five, one past it, and duplicates), the catalog entry an exemption was granted
//! under, the authority's shape (a country, a country and a state), which dates a
//! record carries, and every value of `EntityExemptionAccountStatus` crossed with
//! an expiration in the past, in the future and absent.
//!
//! `PutTaxExemption` — the writer AWS models for these records — is still a
//! scaffold stub with its own issue, so the tables seed the store the way
//! `tax_registration_lifecycle.rs` moves a clock-owed transition: through the
//! shared engine's state, the same store the handler reads.
//!
//! The harness (`emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::ops::types::{Authority, TaxExemptionType};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    tax_exemption_type_catalog, ComAmazonawsTaxsettingsTaxsettingsState, StoredTaxExemption,
    EXEMPTION_STATUS_EXPIRED, EXEMPTION_STATUS_NONE, EXEMPTION_STATUS_PENDING,
    EXEMPTION_STATUS_VALID,
};

use crate::tax_exemption_types::wire_form;
use crate::tax_registration_lifecycle::{call, emulator_with_engine, Answer};

/// Record `exemptions` as `account_id`'s tax exemptions in the shared engine's
/// store — the store the handler reads — standing in for the write
/// `PutTaxExemption` will perform once it is implemented.
fn seed_exemptions(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    exemptions: Vec<StoredTaxExemption>,
) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    state
        .lock()
        .exemptions
        .insert(account_id.to_string(), exemptions);
}

/// The exemptions the store holds for `account_id`, as records rather than as
/// wire JSON: what a read must leave exactly as it found.
fn stored_exemptions(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
) -> Vec<StoredTaxExemption> {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let store = state.lock();
    store.exemptions.get(account_id).cloned().unwrap_or_default()
}

/// The catalog entry at `index` of the collection `GetTaxExemptionTypes`
/// publishes (`state::tax_exemption_type_catalog`), which is the only set of
/// types an exemption can be granted under.
fn catalog_entry(index: usize) -> TaxExemptionType {
    let catalog = tax_exemption_type_catalog();
    catalog
        .get(index)
        .cloned()
        .unwrap_or_else(|| panic!("the published catalog has an entry {index}"))
}

/// One stored exemption: the catalog entry at `type_index`, held in `authority`,
/// recorded with `status`, effective `effective` and expiring `expiration`.
fn exemption(
    type_index: usize,
    authority: Authority,
    status: &str,
    effective: Option<SystemTime>,
    expiration: Option<SystemTime>,
) -> StoredTaxExemption {
    StoredTaxExemption {
        authority,
        exemption_type: catalog_entry(type_index),
        effective_date: effective,
        expiration_date: expiration,
        system_effective_date: effective,
        status: status.to_string(),
    }
}

/// An authority: a country, and a state where the exemption is a state one.
fn authority(country: &str, state: Option<&str>) -> Authority {
    Authority {
        country: country.to_string(),
        state: state.map(std::string::ToString::to_string),
    }
}

/// `now` shifted by `seconds` — negative for the past.
fn at(seconds: i64) -> SystemTime {
    let shift = Duration::from_secs(seconds.unsigned_abs());
    if seconds < 0 {
        SystemTime::now() - shift
    } else {
        SystemTime::now() + shift
    }
}

/// A wall-clock instant as the restJson1 wire carries a `Timestamp`: epoch
/// seconds.
fn epoch_secs(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test instants are after the epoch")
        .as_secs_f64()
}

/// A plain valid exemption, for a case that is about something other than the
/// record's own members.
fn a_valid_exemption() -> StoredTaxExemption {
    exemption(
        0,
        authority("US", None),
        EXEMPTION_STATUS_VALID,
        Some(at(-3_600)),
        None,
    )
}

/// The `taxExemptionDetailsMap` a call reported.
fn details_map(answer: &Answer) -> Value {
    answer.get("taxExemptionDetailsMap").clone()
}

/// The account ids the reported map carries, sorted.
fn reported_accounts(answer: &Answer) -> Vec<String> {
    let mut accounts: Vec<String> = details_map(answer)
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    accounts.sort();
    accounts
}

/// The `taxExemptions` the reported map carries for one account.
fn reported_exemptions(answer: &Answer, account_id: &str) -> Vec<Value> {
    details_map(answer)
        .get(account_id)
        .and_then(|details| details.get("taxExemptions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The account this emulator presents as the caller's own.
fn own_account() -> &'static str {
    engine::emulator_account_id()
}

/// A `TaxRegistrationEntry` to make an account one this service holds tax
/// settings for.
fn a_registration() -> Value {
    json!({
        "registrationId": "DE123456789",
        "registrationType": "VAT",
        "legalName": "Acme GmbH",
        "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
    })
}

/// One row of the visibility table: the accounts a call names, and what the call
/// must answer.
struct Visibility {
    case: &'static str,
    accounts: Vec<&'static str>,
    status: u16,
    error_code: &'static str,
    /// The accounts the reported map must carry, sorted.
    reported: Vec<&'static str>,
}

/// RULE: the call reports the exemptions this service holds for each account it
/// names, and refuses the whole call with `ResourceNotFoundException` (404) when
/// one of them names an account this service holds no tax settings for and that
/// is not the caller's own — an account outside the caller's consolidated
/// billing family.
#[tokio::test]
async fn only_accounts_this_service_holds_tax_settings_for_are_reported() {
    let (router, engine) = emulator_with_engine();

    // Four kinds of account, so the rows differ along the EVIDENCE that an
    // account is one this service can answer for rather than along its id: the
    // caller's own account, an account a TRN was stored for through this
    // service, an account exemptions were recorded for, and an account the store
    // holds an (empty) exemption record list for.
    let with_registration = "000000000001";
    let with_exemption = "000000000002";
    let with_empty_record = "000000000003";
    let foreign = "999999999999";

    let stored = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": with_registration, "taxRegistrationEntry": a_registration() }),
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "case \"setup\": the registration must store, got {stored:?}"
    );
    seed_exemptions(&engine, with_exemption, vec![a_valid_exemption()]);
    seed_exemptions(&engine, with_empty_record, Vec::new());

    let rows = vec![
        Visibility {
            case: "the caller's own account, which holds no exemption",
            accounts: vec![own_account()],
            status: 200,
            error_code: "",
            reported: vec![],
        },
        Visibility {
            case: "an account with a stored TRN and no exemption",
            accounts: vec![with_registration],
            status: 200,
            error_code: "",
            reported: vec![],
        },
        Visibility {
            case: "an account with a recorded exemption",
            accounts: vec![with_exemption],
            status: 200,
            error_code: "",
            reported: vec![with_exemption],
        },
        Visibility {
            case: "an account whose exemption record list is empty",
            accounts: vec![with_empty_record],
            status: 200,
            error_code: "",
            reported: vec![],
        },
        Visibility {
            case: "several accounts, only one of which holds an exemption",
            accounts: vec![own_account(), with_registration, with_exemption],
            status: 200,
            error_code: "",
            reported: vec![with_exemption],
        },
        Visibility {
            // `accountIds` is @length 1..=5: five ids is the published bound,
            // and ids repeated inside it name the same accounts once.
            case: "five accounts, two of them repeats",
            accounts: vec![
                own_account(),
                with_registration,
                with_exemption,
                with_exemption,
                own_account(),
            ],
            status: 200,
            error_code: "",
            reported: vec![with_exemption],
        },
        Visibility {
            case: "an account outside the caller's billing family",
            accounts: vec![foreign],
            status: 404,
            error_code: "ResourceNotFoundException",
            reported: vec![],
        },
        Visibility {
            case: "a foreign account named beside accounts that would report",
            accounts: vec![with_exemption, foreign],
            status: 404,
            error_code: "ResourceNotFoundException",
            reported: vec![],
        },
        Visibility {
            case: "a foreign account named first",
            accounts: vec![foreign, with_exemption],
            status: 404,
            error_code: "ResourceNotFoundException",
            reported: vec![],
        },
        Visibility {
            // One past the published `@length` bound: the boundary validator
            // refuses it before the handler runs.
            case: "six accounts, one past the length bound",
            accounts: vec![
                own_account(),
                with_registration,
                with_exemption,
                with_empty_record,
                "000000000004",
                "000000000005",
            ],
            status: 400,
            error_code: "ValidationException",
            reported: vec![],
        },
    ];

    for row in &rows {
        let answer = call(
            &router,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": row.accounts }),
        )
        .await;
        let case = row.case;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (row.status, row.error_code),
            "case {case:?}: expected {} {:?}, got {answer:?}",
            row.status,
            row.error_code
        );

        let mut expected: Vec<String> =
            row.reported.iter().map(|account| (*account).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported_accounts(&answer),
            expected,
            "case {case:?}: expected the map to carry {expected:?}, got {:?}",
            details_map(&answer)
        );

        if row.status == 200 {
            // Nothing can fail on its own: the accounts a reporting call names
            // are all read from one store under one lock.
            assert_eq!(
                answer.get("failedAccounts"),
                &json!([]),
                "case {case:?}: expected no failed account, got {:?}",
                answer.body
            );
        } else {
            assert!(
                answer
                    .get("message")
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected a refusal carrying a message, got {:?}",
                answer.body
            );
        }
    }
}

/// One row of the projection table: an account, the exemptions recorded for it,
/// and the statuses the read must report for them.
struct Projection {
    case: &'static str,
    account_id: &'static str,
    exemptions: Vec<StoredTaxExemption>,
}

/// RULE: every exemption this service holds for an account is reported, member
/// for member — its authority, the catalog entry it was granted under, each date
/// it carries and its status — and accounts are reported side by side in one
/// map.
#[tokio::test]
async fn every_stored_exemption_is_reported_member_for_member() {
    let (router, engine) = emulator_with_engine();

    // The rows differ along the authority's shape, the catalog entry the
    // exemption was granted under, which dates the record carries, and how many
    // exemptions one account holds — not along the account id.
    let rows = vec![
        Projection {
            case: "one country-wide exemption with both dates",
            account_id: "000000000001",
            exemptions: vec![exemption(
                0,
                authority("US", None),
                EXEMPTION_STATUS_VALID,
                Some(at(-86_400)),
                Some(at(86_400)),
            )],
        },
        Projection {
            case: "an exemption held in one state, with no expiration",
            account_id: "000000000002",
            exemptions: vec![exemption(
                4,
                authority("US", Some("Washington")),
                EXEMPTION_STATUS_VALID,
                Some(at(-7_200)),
                None,
            )],
        },
        Projection {
            case: "an exemption carrying no date at all",
            account_id: "000000000003",
            exemptions: vec![exemption(
                2,
                authority("US", None),
                EXEMPTION_STATUS_PENDING,
                None,
                None,
            )],
        },
        Projection {
            case: "three exemptions of three types on one account",
            account_id: "000000000004",
            exemptions: vec![
                exemption(
                    1,
                    authority("US", None),
                    EXEMPTION_STATUS_VALID,
                    Some(at(-600)),
                    Some(at(600)),
                ),
                exemption(
                    3,
                    authority("US", Some("Texas")),
                    EXEMPTION_STATUS_PENDING,
                    Some(at(-60)),
                    None,
                ),
                exemption(
                    5,
                    authority("US", Some("New York")),
                    EXEMPTION_STATUS_NONE,
                    None,
                    None,
                ),
            ],
        },
    ];

    for row in &rows {
        seed_exemptions(&engine, row.account_id, row.exemptions.clone());
    }

    // One call naming every account at once (four ids, inside the @length bound
    // of five): the map carries each account's own records.
    let accounts: Vec<&str> = rows.iter().map(|row| row.account_id).collect();
    let answer = call(
        &router,
        "BatchGetTaxExemptions",
        &json!({ "accountIds": accounts }),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (200, ""),
        "case \"every seeded account at once\": expected 200, got {answer:?}"
    );
    let mut expected_accounts: Vec<String> =
        accounts.iter().map(|account| (*account).to_string()).collect();
    expected_accounts.sort();
    assert_eq!(
        reported_accounts(&answer),
        expected_accounts,
        "case \"every seeded account at once\": expected {expected_accounts:?}, got {:?}",
        details_map(&answer)
    );

    for row in &rows {
        let case = row.case;
        let reported = reported_exemptions(&answer, row.account_id);
        assert_eq!(
            reported.len(),
            row.exemptions.len(),
            "case {case:?}: expected {} exemption(s), got {reported:?}",
            row.exemptions.len()
        );

        for (index, stored) in row.exemptions.iter().enumerate() {
            let case = format!("{case} (exemption {index})");
            let entry = &reported[index];

            let mut expected_authority = json!({ "country": stored.authority.country });
            if let Some(state) = &stored.authority.state {
                expected_authority["state"] = json!(state);
            }
            assert_eq!(
                entry.get("authority"),
                Some(&expected_authority),
                "case {case:?}: expected the recorded authority {expected_authority}, got {entry}"
            );
            assert_eq!(
                entry.get("taxExemptionType"),
                Some(&wire_form(&stored.exemption_type)),
                "case {case:?}: expected the catalog entry {}, got {entry}",
                wire_form(&stored.exemption_type)
            );
            assert_eq!(
                entry.get("status").and_then(Value::as_str),
                Some(stored.status.as_str()),
                "case {case:?}: expected the recorded status {:?}, got {entry}",
                stored.status
            );

            for (member, recorded) in [
                ("effectiveDate", stored.effective_date),
                ("expirationDate", stored.expiration_date),
                ("systemEffectiveDate", stored.system_effective_date),
            ] {
                match recorded {
                    Some(instant) => {
                        let reported_secs = entry
                            .get(member)
                            .and_then(Value::as_f64)
                            .unwrap_or_else(|| panic!("case {case:?}: expected {member}, got {entry}"));
                        assert!(
                            (reported_secs - epoch_secs(instant)).abs() < 1.0,
                            "case {case:?}: expected {member} at {}, got {reported_secs}",
                            epoch_secs(instant)
                        );
                    }
                    None => assert!(
                        entry.get(member).is_none(),
                        "case {case:?}: a record with no {member} must report none, got {entry}"
                    ),
                }
            }
        }
    }
}

/// RULE: an exemption that was in force and whose expiration date has passed is
/// reported `Expired` — lapsing is a transition the clock owes, due at the
/// instant the expiration names whether or not anybody calls — and every other
/// recorded status is reported exactly as recorded, because no other transition
/// of an exemption is the clock's to make.
#[tokio::test]
async fn an_exemption_is_reported_expired_once_its_expiration_has_passed() {
    // Every value of `EntityExemptionAccountStatus`
    // (`smithy_query(query="shape", shape="EntityExemptionAccountStatus")` —
    // `None | Valid | Expired | Pending`) crossed with an expiration in the
    // past, in the future and absent.
    let rows: Vec<(&str, &str, Option<i64>, &str)> = vec![
        (
            "in force, expiring in an hour",
            EXEMPTION_STATUS_VALID,
            Some(3_600),
            EXEMPTION_STATUS_VALID,
        ),
        (
            "in force with no expiration",
            EXEMPTION_STATUS_VALID,
            None,
            EXEMPTION_STATUS_VALID,
        ),
        (
            "in force, expired an hour ago",
            EXEMPTION_STATUS_VALID,
            Some(-3_600),
            EXEMPTION_STATUS_EXPIRED,
        ),
        (
            "in force, expired a second ago",
            EXEMPTION_STATUS_VALID,
            Some(-1),
            EXEMPTION_STATUS_EXPIRED,
        ),
        (
            "under review, expiring in an hour",
            EXEMPTION_STATUS_PENDING,
            Some(3_600),
            EXEMPTION_STATUS_PENDING,
        ),
        (
            // The clock cannot review a certificate, so a pending exemption
            // stays pending however old it is.
            "under review, with a date an hour past",
            EXEMPTION_STATUS_PENDING,
            Some(-3_600),
            EXEMPTION_STATUS_PENDING,
        ),
        (
            "already expired, with no expiration date",
            EXEMPTION_STATUS_EXPIRED,
            None,
            EXEMPTION_STATUS_EXPIRED,
        ),
        (
            "no exemption on the account, with a date an hour past",
            EXEMPTION_STATUS_NONE,
            Some(-3_600),
            EXEMPTION_STATUS_NONE,
        ),
    ];

    for (case, recorded, expiration, expected) in rows {
        let (router, engine) = emulator_with_engine();
        let account_id = "000000000007";
        seed_exemptions(
            &engine,
            account_id,
            vec![exemption(
                0,
                authority("US", None),
                recorded,
                Some(at(-86_400)),
                expiration.map(at),
            )],
        );

        let answer = call(
            &router,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": [account_id] }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected 200, got {answer:?}"
        );
        let reported = reported_exemptions(&answer, account_id);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one recorded exemption, got {reported:?}"
        );
        assert_eq!(
            reported[0].get("status").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected status {expected:?}, got {:?}",
            reported[0]
        );

        // The derivation is a projection of the record and the clock: the stored
        // status is left exactly as it was, so a later caller sees the same.
        let still_stored = stored_exemptions(&engine, account_id);
        assert_eq!(
            still_stored.len(),
            1,
            "case {case:?}: the read must store nothing, got {still_stored:?}"
        );
        assert_eq!(
            still_stored[0].status, recorded,
            "case {case:?}: expected the recorded status {recorded:?} to stand, got {:?}",
            still_stored[0].status
        );
    }
}

/// RULE: reading exemptions is a report — two reads answer the same thing, the
/// store is left as it was, and nothing beside the exemptions moves.
#[tokio::test]
async fn reading_exemptions_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000008";

    let stored = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": account_id, "taxRegistrationEntry": a_registration() }),
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "case \"setup\": the registration must store, got {stored:?}"
    );
    seed_exemptions(
        &engine,
        account_id,
        vec![
            a_valid_exemption(),
            exemption(
                2,
                authority("US", Some("Oregon")),
                EXEMPTION_STATUS_PENDING,
                Some(at(-120)),
                None,
            ),
        ],
    );
    let before_store = stored_exemptions(&engine, account_id);

    let request = json!({ "accountIds": [account_id] });
    let first = call(&router, "BatchGetTaxExemptions", &request).await;
    let second = call(&router, "BatchGetTaxExemptions", &request).await;
    assert_eq!(
        (first.status, first.error_code.as_str()),
        (200, ""),
        "case \"a first read\": expected 200, got {first:?}"
    );
    assert_eq!(
        second.body, first.body,
        "case \"a second read\": expected the same report, got {:?}",
        second.body
    );

    let after_store = stored_exemptions(&engine, account_id);
    assert_eq!(
        after_store.len(),
        before_store.len(),
        "case \"the store across two reads\": expected {} record(s), got {after_store:?}",
        before_store.len()
    );
    for (index, (after, before)) in after_store.iter().zip(&before_store).enumerate() {
        assert_eq!(
            (after.status.as_str(), after.expiration_date),
            (before.status.as_str(), before.expiration_date),
            "case \"record {index} across two reads\": expected {before:?}, got {after:?}"
        );
    }

    // And nothing beside the exemptions moved: the account's registration and
    // its inheritance election answer exactly as they did.
    let registration = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(
        registration.status, 200,
        "case \"a registration read after an exemption read\": expected 200, got {registration:?}"
    );
    let inheritance = call(&router, "GetTaxInheritance", &json!({})).await;
    assert_eq!(
        (inheritance.status, inheritance.get("heritageStatus").clone()),
        (200, json!("OptOut")),
        "case \"an inheritance read after an exemption read\": expected the unchanged default, \
         got {inheritance:?}"
    );

    // A read of an account that holds no exemption must not create one, so the
    // same account still reports nothing on the next call.
    let empty = call(
        &router,
        "BatchGetTaxExemptions",
        &json!({ "accountIds": [own_account()] }),
    )
    .await;
    assert_eq!(
        (empty.status, reported_accounts(&empty)),
        (200, Vec::new()),
        "case \"the caller's own account, which holds no exemption\": expected an empty map, got \
         {empty:?}"
    );
    assert!(
        stored_exemptions(&engine, own_account()).is_empty(),
        "case \"the caller's own account\": a read must record nothing, got {:?}",
        stored_exemptions(&engine, own_account())
    );
}
