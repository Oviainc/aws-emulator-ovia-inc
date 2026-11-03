// SPDX-License-Identifier: CC0-1.0

//! Tax exemptions end to end over the restJson1 wire: what `PutTaxExemption`
//! records, and what `BatchGetTaxExemptions` and `ListTaxExemptions` report.
//!
//! The rules under test:
//!
//! * the reads report, for each account, the exemptions this service holds for
//!   it — every stored record, member for member — and an account it holds none
//!   for simply has no entry in the reported map;
//! * an exemption READ naming an account this service holds no tax settings for
//!   and that is not the caller's own reports that account on the batch read's
//!   own per-account failure channel (`failedAccounts`) on a 200, and still
//!   reports every account named beside it;
//! * a submission, by contrast, screens no account: it records the exemption for
//!   every account id it names, whatever this service already holds for that
//!   account, and answers one support case id;
//! * an exemption whose expiration date has passed is reported `Expired`,
//!   because lapsing is a transition the clock owes; every other recorded status
//!   is reported as recorded;
//! * a submission records the exemption its `exemptionType` and `authority`
//!   identify for EVERY account it names, under one support case id, and that
//!   pair is the exemption's identity — submitting it again replaces the
//!   account's record rather than adding one beside it;
//! * `ListTaxExemptions` pages over the accounts holding an exemption in
//!   account-id order, the page size being the server's decision, and a cursor it
//!   did not issue is refused;
//! * reading is a report: it changes neither the exemptions nor anything beside
//!   them;
//! * a submission's `exemptionType` and `authority` are admitted on the
//!   constraints the model states for them rather than against the collection
//!   `GetTaxExemptionTypes` publishes, whose wording is this emulator's own: a
//!   token that names a published entry is recorded as that entry, any other
//!   token as submitted, and the only refusal left is the blank `Authority$state`
//!   the model's pattern forbids (`exemption_type_admission.rs` spans that class
//!   to the modelled bounds).
//!
//! The tables below span the classes those rules generalise over — the kinds of
//! evidence an account is in the family (the caller's own, a stored TRN, stored
//! exemptions), what a submission's accounts already hold (nothing at all, a tax
//! registration, another exemption) against how many it names (one, several, the
//! model's `@length` bound of 550, and duplicates), the number of accounts a read
//! names (one, its own `@length` bound of five, one past it, and duplicates), the
//! catalog entry an exemption was granted
//! under and whether it is named by its description or its display name, the
//! authority's shape (a country, a country and a state, either re-cased or
//! padded), which dates a record carries, every value of
//! `EntityExemptionAccountStatus` crossed with an expiration in the past, in the
//! future and absent, several page sizes against several store sizes, and each
//! published bound of `maxResults` plus one past it.
//!
//! Where a table needs a store rather than a wire write it seeds the shared
//! engine's state the way `tax_registration_lifecycle.rs` moves a clock-owed
//! transition: through the same store the handlers read.
//!
//! The harness (`emulator`, `emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::ops::types::{Authority, TaxExemptionType};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    find_exemption_type, tax_exemption_type_catalog, ComAmazonawsTaxsettingsTaxsettingsState,
    StoredTaxExemption, EXEMPTION_STATUS_EXPIRED, EXEMPTION_STATUS_NONE, EXEMPTION_STATUS_PENDING,
    EXEMPTION_STATUS_VALID, MAX_STORED_EXEMPTIONS, TAX_EXEMPTIONS_PAGE_BOUNDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_exemption_types::wire_form;
use crate::tax_registration_lifecycle::{call, emulator, emulator_with_engine, Answer};

/// Record `exemptions` as `account_id`'s tax exemptions in the shared engine's
/// store — the store the handlers read — for a table that is about what a read
/// reports rather than about the write that recorded it.
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

/// The `taxExemptions` the reported map carries for one account. One reader for
/// every suite that asserts on a recorded exemption, so none of them reads the
/// map its own way.
pub(crate) fn reported_exemptions(answer: &Answer, account_id: &str) -> Vec<Value> {
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

/// The accounts a listing reports once this service holds exemptions for
/// `accounts`: those accounts and the caller's own, which the exemption reads
/// answer for whether or not it holds an exemption, in account-id order.
fn listed_family<I: IntoIterator<Item = String>>(accounts: I) -> Vec<String> {
    let mut family: Vec<String> = accounts
        .into_iter()
        .chain(std::iter::once(own_account().to_string()))
        .collect();
    family.sort();
    family.dedup();
    family
}

/// A `TaxRegistrationEntry` to make an account one this service holds tax
/// settings for.
fn a_registration() -> Value {
    json!({
        "registrationId": "DE136695976",
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
    /// The accounts `failedAccounts` must carry, in the order the request named
    /// them for the first time.
    failed: Vec<&'static str>,
}

/// RULE: the call reports a `TaxExemptionDetails` for each account it names that
/// this service can answer for — the exemptions it holds for that account where
/// there are any, and the inheritance indicator either way — and an account it
/// holds no tax settings for and that is not the caller's own — an account
/// outside the caller's consolidated billing family — is reported on the
/// response's own per-account failure channel (`failedAccounts`) on a 200, the
/// accounts named beside it still being reported in the map.
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
            reported: vec![own_account()],
            failed: vec![],
        },
        Visibility {
            case: "an account with a stored TRN and no exemption",
            accounts: vec![with_registration],
            status: 200,
            error_code: "",
            reported: vec![with_registration],
            failed: vec![],
        },
        Visibility {
            case: "an account with a recorded exemption",
            accounts: vec![with_exemption],
            status: 200,
            error_code: "",
            reported: vec![with_exemption],
            failed: vec![],
        },
        Visibility {
            case: "an account whose exemption record list is empty",
            accounts: vec![with_empty_record],
            status: 200,
            error_code: "",
            reported: vec![with_empty_record],
            failed: vec![],
        },
        Visibility {
            case: "several accounts, only one of which holds an exemption",
            accounts: vec![own_account(), with_registration, with_exemption],
            status: 200,
            error_code: "",
            reported: vec![own_account(), with_registration, with_exemption],
            failed: vec![],
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
            reported: vec![own_account(), with_registration, with_exemption],
            failed: vec![],
        },
        Visibility {
            case: "an account outside the caller's billing family",
            accounts: vec![foreign],
            status: 200,
            error_code: "",
            reported: vec![],
            failed: vec![foreign],
        },
        Visibility {
            case: "a foreign account named beside accounts that report",
            accounts: vec![with_exemption, foreign],
            status: 200,
            error_code: "",
            reported: vec![with_exemption],
            failed: vec![foreign],
        },
        Visibility {
            case: "a foreign account named first",
            accounts: vec![foreign, with_exemption],
            status: 200,
            error_code: "",
            reported: vec![with_exemption],
            failed: vec![foreign],
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
            failed: vec![],
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
            // The per-account failure channel carries exactly the accounts this
            // service could not report on, and is present-and-empty when there
            // are none.
            assert_eq!(
                answer.get("failedAccounts"),
                &json!(row.failed),
                "case {case:?}: expected the failed accounts {:?}, got {:?}",
                row.failed,
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

        // Both readers project one record the same way, so each is asserted on
        // the same row rather than one of them being trusted to agree.
        for (reader, request) in [
            (
                "BatchGetTaxExemptions",
                json!({ "accountIds": [account_id] }),
            ),
            ("ListTaxExemptions", json!({})),
        ] {
            let case = format!("{case} through {reader}");
            let answer = call(&router, reader, &request).await;
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
        }

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
}

// ---------------------------------------------------------------------------
// `PutTaxExemption` and `ListTaxExemptions`: the write, and the listing that
// pages over what it recorded.
// ---------------------------------------------------------------------------

/// A certificate file to submit: `documentFile` is a blob, which restJson1
/// carries base64-encoded.
pub(crate) fn certificate(document_name: &str) -> Value {
    json!({
        "documentName": document_name,
        "documentFile": ::aws_smithy_types::base64::encode("%PDF-1.4 exemption certificate"),
    })
}

/// One `PutTaxExemption` submission and what the listing must then report for it.
struct Submission {
    case: &'static str,
    accounts: &'static [&'static str],
    /// The `exemptionType` the request names: the `description` or the
    /// `displayName` of an entry the catalog publishes
    /// (`smithy_query(query="doc", shape="PutTaxExemptionRequest",
    /// member="exemptionType")`: "Use the supported tax exemption type
    /// description").
    exemption_type: &'static str,
    country: &'static str,
    state: Option<&'static str>,
    /// `ExemptionCertificate$documentName`, whose modelled pattern admits a
    /// `.pdf`, `.jpg` or `.png` suffix in either case
    /// (`smithy_query(query="shape", shape="ExemptionCertificate")`).
    document_name: &'static str,
    expected_display_name: &'static str,
    expected_description: &'static str,
    expected_country: &'static str,
    expected_state: Option<&'static str>,
}

impl Submission {
    /// The request body this submission sends.
    fn body(&self) -> Value {
        let mut authority = json!({ "country": self.country });
        if let Some(state) = self.state {
            authority["state"] = json!(state);
        }
        json!({
            "accountIds": self.accounts,
            "authority": authority,
            "exemptionType": self.exemption_type,
            "exemptionCertificate": certificate(self.document_name),
        })
    }

    /// The `taxExemptionType` the listing must report for it: the catalog's
    /// entry, as `tax_exemption_types.rs` pins that collection literally.
    fn expected_type(&self) -> Value {
        crate::tax_exemption_types::pinned_entry(self.expected_display_name)
    }

    /// The `authority` the listing must report for it.
    fn expected_authority(&self) -> Value {
        let mut authority = json!({ "country": self.expected_country });
        if let Some(state) = self.expected_state {
            authority["state"] = json!(state);
        }
        authority
    }
}

/// The submissions the round-trip table makes.
///
/// The rows differ along what the rule generalises over: how many accounts one
/// submission names, which published entry the type is (four of the seven, none
/// of them cited by any issue), whether it is named by its `description` or its
/// `displayName` and in which case, whether the jurisdiction carries a state, and
/// which of the three file extensions the certificate uses.
const SUBMISSIONS: &[Submission] = &[
    Submission {
        case: "one account, the type named by its description, no state",
        accounts: &["000000000001"],
        exemption_type: "Purchases for resale, held under a resale certificate",
        country: "US",
        state: None,
        document_name: "resale-certificate.pdf",
        expected_display_name: "Reseller",
        expected_description: "Purchases for resale, held under a resale certificate",
        expected_country: "US",
        expected_state: None,
    },
    Submission {
        case: "three accounts, the type named by its display name, with a state",
        accounts: &["000000000002", "000000000003", "000000000004"],
        exemption_type: "501C",
        country: "US",
        state: Some("NY"),
        document_name: "charity.JPG",
        expected_display_name: "501C",
        expected_description: "501C",
        expected_country: "US",
        expected_state: Some("NY"),
    },
    Submission {
        case: "a lower-cased country code and a padded state",
        accounts: &["000000000005"],
        exemption_type: "educational institution",
        country: "us",
        state: Some("  TX  "),
        document_name: "school.png",
        expected_display_name: "Educational Institution",
        expected_description: "Accredited school, college or university exempt from sales and use \
                               tax",
        expected_country: "US",
        expected_state: Some("TX"),
    },
    Submission {
        case: "a tribal government exemption for two accounts",
        accounts: &["000000000006", "000000000007"],
        exemption_type: "Federally recognized tribal government body exempt from sales and use \
                         tax",
        country: "US",
        state: Some("AZ"),
        document_name: "tribal.pdf",
        expected_display_name: "Tribal Government",
        expected_description: "Federally recognized tribal government body exempt from sales and \
                               use tax",
        expected_country: "US",
        expected_state: Some("AZ"),
    },
];

/// RULE: a submission records the exemption it identifies for every account it
/// names, and the listing reports each of those accounts with that exemption:
/// the published type, the submitted jurisdiction, the status a submitted
/// exemption carries while its certificate is under review, and the instant this
/// service recorded it.
#[tokio::test]
async fn every_submitted_exemption_is_reported_for_every_account_it_names() {
    for submission in SUBMISSIONS {
        let case = submission.case;
        let router = emulator();

        let before = epoch_secs(SystemTime::now());
        let stored = call(&router, "PutTaxExemption", &submission.body()).await;
        assert_eq!(
            (stored.status, stored.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected 200, got {stored:?}"
        );
        let case_id = stored
            .get("caseId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a caseId, got {stored:?}"))
            .to_string();
        assert!(
            (1..=200).contains(&case_id.chars().count()),
            "case {case:?}: caseId is a 1..200 string, got {case_id:?}"
        );
        let after = epoch_secs(SystemTime::now());

        let listed = call(&router, "ListTaxExemptions", &json!({})).await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected 200, got {listed:?}"
        );
        let expected_accounts: Vec<String> =
            listed_family(submission.accounts.iter().map(|id| (*id).to_string()));
        assert_eq!(
            reported_accounts(&listed),
            expected_accounts,
            "case {case:?}: expected the accounts the submission named, beside the caller's own, \
             got {:?}",
            details_map(&listed)
        );

        for account in submission.accounts {
            let case = format!("{case} ({account})");
            let details = details_map(&listed)
                .get(*account)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected {account}, got {listed:?}"));
            // Nothing this service records is inherited from a consolidated
            // billing family's management account, so the indicator is reported
            // false and the members describing an inheritance stay absent.
            assert_eq!(
                details.get("heritageObtainedDetails"),
                Some(&json!(false)),
                "case {case:?}: the inheritance indicator is always reported, got {details}"
            );
            for member in [
                "heritageObtainedParentEntity",
                "heritageObtainedReason",
            ] {
                assert!(
                    details.get(member).is_none(),
                    "case {case:?}: nothing was inherited, so {member} is not reported, got \
                     {details}"
                );
            }

            let held = reported_exemptions(&listed, account);
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: the account holds the one submitted exemption, got {held:?}"
            );
            let exemption = &held[0];
            assert_eq!(
                exemption.get("authority"),
                Some(&submission.expected_authority()),
                "case {case:?}: expected {}, got {exemption}",
                submission.expected_authority()
            );
            assert_eq!(
                exemption.get("taxExemptionType"),
                Some(&submission.expected_type()),
                "case {case:?}: expected {}, got {exemption}",
                submission.expected_type()
            );
            // The type's own identifying member, stated by this row rather than
            // read from the collection: the token a later submission names this
            // type by.
            assert_eq!(
                exemption
                    .get("taxExemptionType")
                    .and_then(|entry| entry.get("description"))
                    .and_then(Value::as_str),
                Some(submission.expected_description),
                "case {case:?}: expected the type's published description, got {exemption}"
            );
            assert_eq!(
                exemption.get("status").and_then(Value::as_str),
                Some(EXEMPTION_STATUS_PENDING),
                "case {case:?}: a submitted certificate is under review, got {exemption}"
            );
            let recorded = exemption
                .get("systemEffectiveDate")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a systemEffectiveDate, got {exemption}")
                });
            assert!(
                (before - 1.0..=after + 1.0).contains(&recorded),
                "case {case:?}: the recording time is when the submission was stored \
                 ({before}..{after}), got {recorded}"
            );
            // Both dates are facts about the certificate this emulator does not
            // read, so neither reaches the wire.
            for member in ["effectiveDate", "expirationDate"] {
                assert!(
                    exemption.get(member).is_none(),
                    "case {case:?}: no certificate date is reported, got {exemption}"
                );
            }
        }
    }
}

/// RULE: the exemption's identity is its type and its jurisdiction, so one
/// account can hold several exemptions, and a second submission of the same pair
/// REPLACES its record instead of adding one beside it — case and padding in
/// either making no second exemption.
#[tokio::test]
async fn an_exemption_is_identified_by_its_type_and_its_jurisdiction() {
    let account = "000000000011";
    // The rows differ along the member that makes a submission a different
    // exemption: the type, the state of the jurisdiction, and neither (a repeat,
    // spelled several ways).
    let submissions: &[(&str, Value, usize)] = &[
        (
            "the first exemption",
            json!({ "exemptionType": "Reseller", "authority": { "country": "US" } }),
            1,
        ),
        (
            "a second type in the same jurisdiction",
            json!({ "exemptionType": "Direct Pay Permit", "authority": { "country": "US" } }),
            2,
        ),
        (
            "the first type in another state",
            json!({
                "exemptionType": "Reseller",
                "authority": { "country": "US", "state": "OH" },
            }),
            3,
        ),
        (
            "the same type in the same state again",
            json!({
                "exemptionType": "Reseller",
                "authority": { "country": "US", "state": "OH" },
            }),
            3,
        ),
        (
            "the same state spelled in another case",
            json!({
                "exemptionType": "Reseller",
                "authority": { "country": "us", "state": "oh" },
            }),
            3,
        ),
        (
            "the same type named by its description instead",
            json!({
                "exemptionType": "Purchases for resale, held under a resale certificate",
                "authority": { "country": "US", "state": " OH " },
            }),
            3,
        ),
    ];

    let router = emulator();
    let mut case_ids: Vec<String> = Vec::new();
    for (case, submission, expected_held) in submissions {
        let body = json!({
            "accountIds": [account],
            "authority": submission["authority"],
            "exemptionType": submission["exemptionType"],
            "exemptionCertificate": certificate("certificate.pdf"),
        });
        let stored = call(&router, "PutTaxExemption", &body).await;
        assert_eq!(
            (stored.status, stored.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected 200, got {stored:?}"
        );
        let case_id = stored
            .get("caseId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a caseId, got {stored:?}"))
            .to_string();
        assert!(
            !case_ids.contains(&case_id),
            "case {case:?}: every submission opens its own case, got {case_id:?} twice"
        );
        case_ids.push(case_id);

        let listed = call(&router, "ListTaxExemptions", &json!({})).await;
        let held = reported_exemptions(&listed, account);
        assert_eq!(
            held.len(),
            *expected_held,
            "case {case:?}: expected {expected_held} exemption(s), got {held:?}"
        );
    }
}

/// What this service already holds for an account a submission names. None of it
/// is a precondition for the submission, so the table spans it.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// Nothing at all: a billing-family member account that has never called
    /// this service.
    Nothing,
    /// A tax registration, stored through `PutTaxRegistration` — a resource that
    /// has nothing to do with exemptions.
    ARegistration,
    /// Another exemption, recorded by an earlier submission.
    AnotherExemption,
}

/// Make this service hold `held` for each of `accounts`, through the wire.
async fn make_service_hold(router: &MultiServiceRouter, held: Held, accounts: &[String]) {
    for account in accounts {
        let answer = match held {
            Held::Nothing => continue,
            Held::ARegistration => {
                call(
                    router,
                    "PutTaxRegistration",
                    &json!({ "accountId": account, "taxRegistrationEntry": a_registration() }),
                )
                .await
            }
            Held::AnotherExemption => {
                call(
                    router,
                    "PutTaxExemption",
                    &json!({
                        "accountIds": [account],
                        "authority": { "country": "US" },
                        "exemptionType": "Direct Pay Permit",
                        "exemptionCertificate": certificate("permit.pdf"),
                    }),
                )
                .await
            }
        };
        assert_eq!(
            answer.status, 200,
            "case \"setup {held:?} for {account}\": expected 200, got {answer:?}"
        );
    }
}

/// Every account the listing reports, across its pages, sorted — the whole of
/// what a submission recorded, however many accounts it named.
async fn all_listed_accounts(router: &MultiServiceRouter) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        // The largest page `maxResults` admits, so the biggest table row still
        // takes few pages (`smithy_query(query="operation",
        // operation="ListTaxExemptions")`: 1..=1000).
        let mut body = json!({ "maxResults": TAX_EXEMPTIONS_PAGE_BOUNDS.max_size });
        if let Some(cursor) = &token {
            body["nextToken"] = json!(cursor);
        }
        let page = call(router, "ListTaxExemptions", &body).await;
        assert_eq!(
            (page.status, page.error_code.as_str()),
            (200, ""),
            "case \"listing what a submission recorded\": expected 200, got {page:?}"
        );
        seen.extend(reported_accounts(&page));
        token = page
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    seen.sort();
    seen
}

/// RULE: a submission names the accounts it applies to rather than looking them
/// up — it records the exemption for EVERY well-formed account id it names,
/// whatever this service already holds for that account, and answers one support
/// case id. Holding a tax registration is no precondition: the model has no
/// operation that enrols an account in a consolidated billing family, so a
/// well-formed id this service holds nothing for is a member account like any
/// other.
#[tokio::test]
async fn a_submission_records_the_exemption_for_every_account_it_names() {
    // `accountIds` is @length 1..=550 of 12-digit ids
    // (`smithy_query(query="operation", operation="PutTaxExemption")`), so one row
    // names as many accounts as a submission may — none of them known here — while
    // the others differ along how many accounts are named, where in the list an
    // unknown one sits, whether an id repeats, and what the service already holds.
    let the_maximum: Vec<String> = (1..=550).map(|index| format!("7{index:011}")).collect();
    let cases: Vec<(&str, Vec<String>, Held)> = vec![
        (
            "one account this service holds nothing for",
            vec!["111111111111".to_string()],
            Held::Nothing,
        ),
        (
            "two accounts this service holds nothing for",
            vec!["111111111111".to_string(), "222222222222".to_string()],
            Held::Nothing,
        ),
        (
            "the caller's own account",
            vec![own_account().to_string()],
            Held::Nothing,
        ),
        (
            "an account holding a tax registration",
            vec!["000000000031".to_string()],
            Held::ARegistration,
        ),
        (
            "an account already holding another exemption",
            vec!["000000000032".to_string()],
            Held::AnotherExemption,
        ),
        (
            "an unknown account named after the caller's own",
            vec![own_account().to_string(), "222222222222".to_string()],
            Held::Nothing,
        ),
        (
            "an unknown account named before the caller's own",
            vec!["333333333333".to_string(), own_account().to_string()],
            Held::Nothing,
        ),
        (
            "ids from across the 12-digit space",
            vec![
                "000000000001".to_string(),
                "123456789012".to_string(),
                "999999999999".to_string(),
            ],
            Held::Nothing,
        ),
        (
            "the same unknown account named twice",
            vec!["444444444444".to_string(), "444444444444".to_string()],
            Held::Nothing,
        ),
        (
            "the 550 accounts the model's bound admits, none of them known",
            the_maximum,
            Held::Nothing,
        ),
    ];

    for (case, accounts, held) in cases {
        let router = emulator();
        make_service_hold(&router, held, &accounts).await;

        let answer = call(
            &router,
            "PutTaxExemption",
            &json!({
                "accountIds": accounts,
                "authority": { "country": "US" },
                "exemptionType": "Reseller",
                "exemptionCertificate": certificate("resale.pdf"),
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the submission to be applied, got {answer:?}"
        );
        let case_id = answer
            .get("caseId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a caseId, got {answer:?}"))
            .to_string();
        assert!(
            (1..=200).contains(&case_id.chars().count()),
            "case {case:?}: caseId is a 1..200 string, got {case_id:?}"
        );

        let mut expected: Vec<String> = accounts.clone();
        expected.sort();
        expected.dedup();
        let recorded = all_listed_accounts(&router).await;
        assert_eq!(
            recorded,
            listed_family(expected.clone()),
            "case {case:?}: expected the exemption recorded for every account named, beside the \
             caller's own, got {recorded:?}"
        );

        // Read the records back through `BatchGetTaxExemptions` — the first, a
        // middle and the last of the accounts named, within that read's @length
        // bound of five.
        let mut sampled: Vec<String> = Vec::new();
        for index in [0, expected.len() / 2, expected.len() - 1] {
            let account = expected[index].clone();
            if !sampled.contains(&account) {
                sampled.push(account);
            }
        }
        let read = call(
            &router,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": sampled }),
        )
        .await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the recorded accounts to be readable, got {read:?}"
        );
        for account in &sampled {
            let records = reported_exemptions(&read, account);
            let expected_count = match held {
                Held::AnotherExemption => 2,
                Held::Nothing | Held::ARegistration => 1,
            };
            assert_eq!(
                records.len(),
                expected_count,
                "case {case:?} ({account}): expected {expected_count} exemption(s), got {records:?}"
            );
            let submitted = records
                .iter()
                .find(|record| {
                    record
                        .get("taxExemptionType")
                        .and_then(|exemption_type| exemption_type.get("displayName"))
                        .and_then(Value::as_str)
                        == Some("Reseller")
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?} ({account}): expected the submitted type, got {records:?}")
                });
            assert_eq!(
                submitted.get("authority"),
                Some(&json!({ "country": "US" })),
                "case {case:?} ({account}): expected the submitted jurisdiction, got {submitted}"
            );
            assert_eq!(
                submitted.get("status").and_then(Value::as_str),
                Some(EXEMPTION_STATUS_PENDING),
                "case {case:?} ({account}): a submitted certificate is under review, got \
                 {submitted}"
            );
        }
    }
}

/// Seed one exemption for each of `count` accounts, answering the account ids in
/// store order.
///
/// The caller's own account is the first of them: the exemption reads answer for
/// that account whether or not it holds an exemption, so seeding it keeps the
/// accounts the store holds and the accounts a listing reports one and the same
/// set.
pub(crate) fn seed_many(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<String> {
    let accounts: Vec<String> = std::iter::once(own_account().to_string())
        .chain((1..count).map(|index| format!("{index:012}")))
        .take(count)
        .collect();
    for account in &accounts {
        seed_exemptions(engine, account, vec![a_valid_exemption()]);
    }
    accounts
}

/// RULE: the listing pages over the accounts holding an exemption in account-id
/// order, every account appearing exactly once across the pages, and only the
/// last page carries no cursor.
#[tokio::test]
async fn the_listing_pages_over_every_account() {
    // The rows differ along the page size against the store size: one at a time,
    // a size that does not divide the store, exactly the store, and more than it.
    let page_sizes: &[Option<i64>] = &[Some(1), Some(2), Some(3), Some(7), Some(1_000), None];
    let store_size = 7;

    for page_size in page_sizes {
        let case = format!("maxResults {page_size:?} over {store_size} accounts");
        let (router, engine) = emulator_with_engine();
        let expected = listed_family(seed_many(&engine, store_size));

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["maxResults"] = json!(size);
            }
            if let Some(cursor) = &token {
                body["nextToken"] = json!(cursor);
            }
            let answer = call(&router, "ListTaxExemptions", &body).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (200, ""),
                "case {case:?}: expected 200, got {answer:?}"
            );
            let page = reported_accounts(&answer);
            let limit = page_size.map_or(TAX_EXEMPTIONS_PAGE_BOUNDS.default_size, |size| {
                size.unsigned_abs() as usize
            });
            assert!(
                page.len() <= limit,
                "case {case:?}: a page holds at most {limit} accounts, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= store_size + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {seen:?}"
            );
            token = answer
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every account exactly once, got {seen:?}"
        );
    }
}

/// RULE: the page size is the SERVER's decision — a request that names none
/// still gets a bounded page and a cursor to continue from.
#[tokio::test]
async fn a_request_naming_no_page_size_gets_the_servers_default() {
    let (router, engine) = emulator_with_engine();
    let store_size = TAX_EXEMPTIONS_PAGE_BOUNDS.default_size + 5;
    let expected = listed_family(seed_many(&engine, store_size));

    let first = call(&router, "ListTaxExemptions", &json!({})).await;
    let page = reported_accounts(&first);
    assert_eq!(
        page.len(),
        TAX_EXEMPTIONS_PAGE_BOUNDS.default_size,
        "case \"no maxResults\": expected the server's default page, got {} accounts",
        page.len()
    );
    let token = first
        .get("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case \"no maxResults\": expected a cursor, got {first:?}"))
        .to_string();

    let second = call(&router, "ListTaxExemptions", &json!({ "nextToken": token })).await;
    let rest = reported_accounts(&second);
    assert_eq!(
        rest.len(),
        expected.len() - TAX_EXEMPTIONS_PAGE_BOUNDS.default_size,
        "case \"the cursor's page\": expected the remainder, got {rest:?}"
    );
    assert!(
        second.get("nextToken").is_null(),
        "case \"the cursor's page\": the last page carries no cursor, got {second:?}"
    );
    let mut seen = page;
    seen.extend(rest);
    seen.sort();
    assert_eq!(
        seen, expected,
        "case \"the two pages\": expected every account exactly once, got {seen:?}"
    );
}

/// RULE: `maxResults` is accepted on each published bound of its 1..=1000 range
/// (`smithy_query(query="operation", operation="ListTaxExemptions")`) and refused
/// one past either.
#[tokio::test]
async fn max_results_is_accepted_inside_its_published_range() {
    let cases: &[(&str, i64, u16)] = &[
        ("one below the floor", 0, 400),
        ("the floor", 1, 200),
        ("the ceiling", 1_000, 200),
        ("one past the ceiling", 1_001, 400),
    ];
    let (router, engine) = emulator_with_engine();
    seed_many(&engine, 3);

    for (case, max_results, expected_status) in cases {
        let answer = call(
            &router,
            "ListTaxExemptions",
            &json!({ "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if *expected_status == 400 {
            assert!(
                answer.error_code.contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {answer:?}"
            );
        }
    }
}

/// RULE: a cursor this service did not issue names nothing to resume from, so it
/// is refused with a `ValidationException` carrying the token error code.
#[tokio::test]
async fn a_cursor_this_service_did_not_issue_is_refused() {
    // The rows differ along how the token fails: it does not decode at all, it
    // decodes to something that is no account id, or it decodes to an account id
    // of the wrong length.
    let cases: &[(&str, String, &str)] = &[
        ("not base64 at all", "A".to_string(), "MalformedToken"),
        (
            "base64 of a word",
            ::aws_smithy_types::base64::encode("hello"),
            "InvalidToken",
        ),
        (
            "base64 of eleven digits",
            ::aws_smithy_types::base64::encode("00000000001"),
            "InvalidToken",
        ),
        (
            "base64 of thirteen digits",
            ::aws_smithy_types::base64::encode("0000000000001"),
            "InvalidToken",
        ),
    ];

    let (router, engine) = emulator_with_engine();
    let family = listed_family(seed_many(&engine, 2));

    for (case, token, expected_code) in cases {
        let answer = call(
            &router,
            "ListTaxExemptions",
            &json!({ "nextToken": token }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert!(
            answer.error_code.contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.get("errorCode").as_str(),
            Some(*expected_code),
            "case {case:?}: expected errorCode {expected_code:?}, got {answer:?}"
        );
    }

    // And a cursor this service DID issue resumes the listing.
    let first = call(&router, "ListTaxExemptions", &json!({ "maxResults": 1 })).await;
    let token = first
        .get("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case \"an issued cursor\": expected a cursor, got {first:?}"))
        .to_string();
    let second = call(&router, "ListTaxExemptions", &json!({ "nextToken": token })).await;
    assert_eq!(
        (second.status, reported_accounts(&second)),
        (200, family[1..].to_vec()),
        "case \"an issued cursor\": expected the accounts after the first, got {second:?}"
    );
}

/// RULE: the listing is a report — it changes nothing a later caller sees,
/// neither the records it reports nor anything beside them.
#[tokio::test]
async fn listing_the_exemptions_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    let submitted = call(
        &router,
        "PutTaxExemption",
        &json!({
            "accountIds": ["000000000021", own_account()],
            "authority": { "country": "US" },
            "exemptionType": "Reseller",
            "exemptionCertificate": certificate("resale.pdf"),
        }),
    )
    .await;
    assert_eq!(
        (submitted.status, submitted.error_code.as_str()),
        (200, ""),
        "case \"setup\": the submission must store, got {submitted:?}"
    );
    seed_exemptions(&engine, "000000000022", vec![a_valid_exemption()]);

    let first = call(&router, "ListTaxExemptions", &json!({})).await;
    let second = call(&router, "ListTaxExemptions", &json!({})).await;
    assert_eq!(
        second.body, first.body,
        "case \"a second read\": expected {:?}, got {:?}",
        first.body, second.body
    );

    // Paging through the store is a read too: the accounts and their exemptions
    // are the same afterwards.
    let mut token = call(&router, "ListTaxExemptions", &json!({ "maxResults": 1 }))
        .await
        .get("nextToken")
        .as_str()
        .map(std::string::ToString::to_string);
    assert!(
        token.is_some(),
        "case \"setup\": a store of three accounts pages"
    );
    while let Some(cursor) = token {
        let page = call(
            &router,
            "ListTaxExemptions",
            &json!({ "maxResults": 1, "nextToken": cursor }),
        )
        .await;
        token = page
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
    }
    let after_paging = call(&router, "ListTaxExemptions", &json!({})).await;
    assert_eq!(
        after_paging.body, first.body,
        "case \"after paging through the store\": expected {:?}, got {:?}",
        first.body, after_paging.body
    );

    // Nothing beside the exemptions moved either: the inheritance election is
    // still the default, and no further support case was opened — a read mints
    // none.
    let inheritance = call(&router, "GetTaxInheritance", &json!({})).await;
    assert_eq!(
        (inheritance.status, inheritance.get("heritageStatus").clone()),
        (200, json!("OptOut")),
        "case \"an inheritance read across a listing\": expected the unchanged default, got \
         {inheritance:?}"
    );
    let minted = engine
        .state::<ComAmazonawsTaxsettingsTaxsettingsState>()
        .lock()
        .minted_exemption_cases;
    assert_eq!(
        minted, 1,
        "case \"the cases a listing opens\": expected the one submission's case, got {minted}"
    );
}

/// RULE: an `exemptionType` is admitted on the constraints the model states for
/// the member, not against the wording the published collection happens to
/// carry, so every token the model admits is recorded; a token that names a
/// published entry is recorded as that entry, whatever its case or padding, and
/// any other is recorded as submitted.
#[tokio::test]
async fn every_exemption_type_the_model_admits_is_recorded() {
    // The rows differ along how the value relates to the published collection: a
    // display name, a description, either re-cased or padded, and tokens that
    // name no entry of it at all.
    let cases: &[(&str, &str)] = &[
        ("a display name", "Reseller"),
        (
            "a description",
            "Purchases for resale, held under a resale certificate",
        ),
        ("a padded, re-cased display name", "  dIRECT pAY pERMIT  "),
        ("a type the collection does not publish", "Diplomatic Mission"),
        ("a prefix of a published description", "Purchases for resale"),
        ("a shape name rather than a type", "TaxExemptionType"),
    ];

    for (case, exemption_type) in cases {
        let router = emulator();
        let answer = call(
            &router,
            "PutTaxExemption",
            &json!({
                "accountIds": [own_account()],
                "authority": { "country": "US" },
                "exemptionType": exemption_type,
                "exemptionCertificate": certificate("certificate.pdf"),
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the submission to be applied, got {answer:?}"
        );

        let listed = call(&router, "ListTaxExemptions", &json!({})).await;
        assert_eq!(
            reported_accounts(&listed),
            vec![own_account().to_string()],
            "case {case:?}: expected the account to hold the exemption, got {listed:?}"
        );
        // The type the record is held under: the published entry the token
        // names, or the token itself for one that names none.
        let expected_description = find_exemption_type(exemption_type)
            .map(|(_, description, _)| (*description).to_string())
            .unwrap_or_else(|| (*exemption_type).to_string());
        let held = reported_exemptions(&listed, own_account());
        let reported: Vec<Value> = held
            .iter()
            .filter_map(|exemption| {
                exemption
                    .get("taxExemptionType")
                    .and_then(|entry| entry.get("description"))
                    .cloned()
            })
            .collect();
        assert_eq!(
            reported,
            vec![json!(expected_description)],
            "case {case:?}: expected the exemption held under {expected_description:?}, got \
             {held:?}"
        );
    }
}

/// RULE: for a type the published collection does NOT carry, an `authority` is
/// admitted on the constraints the model states for `Authority` — a two-letter
/// country code, and a non-blank state where one is submitted — because the
/// collection says nothing about where such a type applies, so every
/// jurisdiction the model admits is recorded and reported as the store holds it;
/// a blank state is refused, naming that parameter, and stores nothing. Where
/// the type IS one the collection publishes, its own `applicableJurisdictions`
/// decide instead (`tests/all/exemption_authority_support.rs`).
#[tokio::test]
async fn an_authority_the_model_admits_is_recorded() {
    // The token names no entry of the published collection, so what each row is
    // judged on is the jurisdiction's own form.
    let unpublished_type = "Exemption certificate this collection does not publish";
    // The rows differ along the jurisdiction: a country with and without a
    // state, the country re-cased, a state nothing enumerates, countries outside
    // the published collection's own, and a state that is all whitespace (the
    // model's non-blank pattern).
    let cases: &[(&str, Value, u16, &str)] = &[
        ("a country only", json!({ "country": "US" }), 200, ""),
        (
            "a country with a state",
            json!({ "country": "US", "state": "California" }),
            200,
            "",
        ),
        ("the country lower-cased", json!({ "country": "us" }), 200, ""),
        (
            "a state nothing enumerates",
            json!({ "country": "US", "state": "Guam" }),
            200,
            "",
        ),
        (
            // The jurisdictions the published collection carries speak only for
            // the types it publishes, so they decide no submission of another
            // type: a country the model admits is recorded.
            "another country",
            json!({ "country": "DE" }),
            200,
            "",
        ),
        (
            "another country with a state",
            json!({ "country": "CA", "state": "Ontario" }),
            200,
            "",
        ),
        (
            "a blank state",
            json!({ "country": "US", "state": "   " }),
            400,
            "STATE",
        ),
    ];

    for (case, authority, expected_status, expected_field) in cases {
        let router = emulator();
        let answer = call(
            &router,
            "PutTaxExemption",
            &json!({
                "accountIds": [own_account()],
                "authority": authority,
                "exemptionType": unpublished_type,
                "exemptionCertificate": certificate("resale.pdf"),
            }),
        )
        .await;
        assert_eq!(
            answer.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        let listed = call(&router, "ListTaxExemptions", &json!({})).await;
        if *expected_status == 200 {
            assert_eq!(
                reported_accounts(&listed),
                vec![own_account().to_string()],
                "case {case:?}: expected the account to hold the exemption, got {listed:?}"
            );
            // The jurisdiction is reported as the store holds it: the country
            // upper-cased, the state trimmed.
            let mut expected = json!({
                "country": authority["country"].as_str().unwrap_or_default().to_uppercase(),
            });
            if let Some(state) = authority["state"].as_str() {
                expected["state"] = json!(state.trim());
            }
            let held = reported_exemptions(&listed, own_account());
            let reported: Vec<Value> = held
                .iter()
                .filter_map(|exemption| exemption.get("authority").cloned())
                .collect();
            assert_eq!(
                reported,
                vec![expected.clone()],
                "case {case:?}: expected the exemption held in {expected}, got {held:?}"
            );
        } else {
            assert!(
                answer.error_code.contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {answer:?}"
            );
            assert_eq!(
                answer.get("fieldList").clone(),
                json!([{ "name": expected_field }]),
                "case {case:?}: expected the refused parameter to be named, got {answer:?}"
            );
            assert!(
                reported_exemptions(&listed, own_account()).is_empty(),
                "case {case:?}: a refused submission stores nothing, got {listed:?}"
            );
        }
    }
}

/// RULE: a submission that would take the store past the exemptions this
/// emulator keeps resident is refused whole with the modelled
/// `CaseCreationLimitExceededException`, opening no case, while replacing an
/// exemption an account already holds is always allowed, because it adds nothing.
#[tokio::test]
async fn a_submission_past_the_resident_bound_is_refused() {
    let (router, engine) = emulator_with_engine();
    // Fill the store to its bound directly, so the bound is reached without
    // sending a thousand requests. Each seeded account holds an exemption, which
    // is also what puts it in the caller's billing family.
    let seeded = seed_many(&engine, MAX_STORED_EXEMPTIONS);

    let body = |account: &str, exemption_type: &str| {
        json!({
            "accountIds": [account],
            "authority": { "country": "US" },
            "exemptionType": exemption_type,
            "exemptionCertificate": certificate("resale.pdf"),
        })
    };

    // The caller's own account is inside the family and holds nothing, so this
    // submission would ADD the thousand-and-first record.
    let refused = call(&router, "PutTaxExemption", &body(own_account(), "Reseller")).await;
    assert_eq!(
        refused.status, 413,
        "case \"one account past the bound\": expected HTTP 413, got {refused:?}"
    );
    assert!(
        refused
            .error_code
            .contains("CaseCreationLimitExceededException"),
        "case \"one account past the bound\": expected the case-limit refusal, got {refused:?}"
    );

    // A refused submission opens no support case either.
    let minted = engine
        .state::<ComAmazonawsTaxsettingsTaxsettingsState>()
        .lock()
        .minted_exemption_cases;
    assert_eq!(
        minted, 0,
        "case \"the cases a refused submission opens\": expected none, got {minted}"
    );

    // The seeded records were granted under the catalog's first entry in the US,
    // so submitting that same exemption for a seeded account replaces its record
    // instead of adding one.
    let first_type = catalog_entry(0)
        .description
        .expect("every published entry carries a description");
    let replaced = call(&router, "PutTaxExemption", &body(&seeded[0], &first_type)).await;
    assert_eq!(
        (replaced.status, replaced.error_code.as_str()),
        (200, ""),
        "case \"replacing an exemption the account holds\": expected 200, got {replaced:?}"
    );

    let held = engine
        .state::<ComAmazonawsTaxsettingsTaxsettingsState>()
        .lock()
        .stored_exemption_count();
    assert_eq!(
        held, MAX_STORED_EXEMPTIONS,
        "case \"the store at its bound\": expected {MAX_STORED_EXEMPTIONS} records, got {held}"
    );
}
