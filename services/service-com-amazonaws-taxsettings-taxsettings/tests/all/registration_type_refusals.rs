// SPDX-License-Identifier: CC0-1.0

//! The two refusals of a registration TYPE, over both writers of a tax
//! registration.
//!
//! RULE 1: a `TaxRegistrationType` the service ties to ONE country and publishes
//! its own `ValidationExceptionErrorCode` value for may be set only for a
//! registration held in that country — the country being the
//! `legalAddress.countryCode` the entry carries, and only where that address is
//! the caller's to supply. A type whose address the service derives from the tax
//! number admits no submitted address at all, so such an entry is refused for
//! the member it may not carry and its `countryCode` is never read as evidence of
//! where the customer is; the precedence itself is
//! `tests/all/derived_address_precedence.rs`.
//!
//! RULE 2: a `TaxRegistrationType` the service publishes a published-pair code
//! for may not be set while the account's STORED registration is of the type
//! that blocks it; such a write is refused 400 `ValidationException` with that
//! `errorCode`, the stored record is left exactly as it was, and removing that
//! record lets the write through.
//!
//! Both rules are asserted over tables spanning what they generalise over: the
//! jurisdiction the entry's address names (seven countries, including countries
//! whose own per-country rules would otherwise refuse the entry, and the one
//! country the type belongs to in three spellings), the registration type already stored
//! (six of the model's `TaxRegistrationType` values, held in two countries), and
//! both writers — a 400 response for the single-account write, a row of the
//! batch response's `errors` list for the batch.
//!
//! The harness (`emulator_with_engine`, `call`, `field_list`,
//! `make_validations_due`) lives in `tax_registration_lifecycle.rs`, the writer
//! fan-out (`WRITERS`, `write`) in `country_rules.rs` and the batch error reader
//! in `pending_trn_conflict.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ERROR_CODE_FIELD_VALIDATION_FAILED, ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN,
    ERROR_CODE_NON_INDIA_CUSTOMER_CAN_NOT_SET_PAN, STATUS_PENDING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::country_rules::{write, WRITERS};
use crate::pending_trn_conflict::error_rows;
use crate::tax_registration_lifecycle::{
    call, emulator_with_engine, field_list, make_validations_due, Answer,
};

/// Every value of the model's `ValidationExceptionErrorCode` enum
/// (`smithy_query(query="shape", shape="ValidationExceptionErrorCode")`), which
/// is the closed set `ValidationException$errorCode` is reported from.
///
/// `pub(crate)` because `tests/all/batch_row_error_codes.rs` asserts that a
/// batch row's `code` is one of them: one catalog, so two suites cannot disagree
/// about which codes this service publishes.
pub(crate) const VALIDATION_EXCEPTION_ERROR_CODES: &[&str] = &[
    "MalformedToken",
    "ExpiredToken",
    "InvalidToken",
    "FieldValidationFailed",
    "MissingInput",
    "NonIndiaCustomerCanNotSetPAN",
    "GSTExistenceBlockSetPAN",
];

/// The registration type both rules are published for, and the one country it
/// belongs to.
const INDIA_TYPE: &str = "PAN";
/// See [`INDIA_TYPE`]: the `Address$countryCode` of that country.
const INDIA: &str = "IN";
/// See [`INDIA_TYPE`]: the country's name, as a refusal states it.
const INDIA_NAME: &str = "India";
/// The stored registration type that blocks a write of [`INDIA_TYPE`].
const BLOCKING_TYPE: &str = "GST";

/// The batch writer, whose per-account refusal channel is a row of `errors`
/// rather than a response status.
const BATCH_WRITER: &str = "BatchPutTaxRegistration";

/// An address held in `country_code`, with every member the model requires of
/// one. `stateOrRegion` is carried throughout because several countries require
/// it of an address held in them (`crate::state::COUNTRY_RULES`), and no rule
/// asserted here turns on it.
fn address_in(country_code: &str) -> Value {
    json!({
        "addressLine1": "1 Example Street",
        "city": "Example City",
        "stateOrRegion": "Example Region",
        "postalCode": "110001",
        "countryCode": country_code,
    })
}

/// An entry of `registration_type`, with the legal address of `country_code` or
/// with none at all.
///
/// `pub(crate)` because `tests/all/batch_row_error_codes.rs` stores and submits
/// the same entries: one builder, so the two suites cannot submit two different
/// entries for one type.
pub(crate) fn entry_of(registration_type: &str, country_code: Option<&str>) -> Value {
    let mut entry = json!({
        // A number its jurisdiction's authority issues where this service
        // publishes a structure for the type, so what each row below is judged
        // on is the registration TYPE and the address — never the number
        // (`crate::fidelity_rules::number_of_type`).
        "registrationId": crate::fidelity_rules::number_of_type(registration_type, country_code),
        "registrationType": registration_type,
        "legalName": "Example Holdings",
    });
    if let Some(country_code) = country_code {
        entry
            .as_object_mut()
            .expect("an object")
            .insert("legalAddress".to_string(), address_in(country_code));
    }
    entry
}

/// What a request that set a registration type must be answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// The address is not the caller's to supply for this type (the refusal
    /// `tests/all/fidelity_rules.rs` pins) — so this row asserts that the
    /// jurisdiction rule did NOT claim it, whatever country the address names.
    AddressNotSubmittable,
    /// The entry is stored, held in this `Address$countryCode`.
    Stored(&'static str),
}

/// What `GetTaxRegistration` reports for `account_id`.
async fn read(router: &MultiServiceRouter, account_id: &str) -> Answer {
    call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await
}

/// The `registrationId` of the record `account_id` holds, or `None` when it
/// holds none.
async fn stored_registration_id(router: &MultiServiceRouter, account_id: &str) -> Option<String> {
    let answer = read(router, account_id).await;
    answer
        .body
        .pointer("/taxRegistration/registrationId")
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)
}

/// Assert that `answer` is the 400 `ValidationException` that carries
/// `error_code` and names `parameter`, with `message` as its sentence.
fn assert_refused(case: &str, answer: &Answer, error_code: &str, parameter: &str, message: &str) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.get("errorCode").as_str(),
        Some(error_code),
        "case {case:?}: expected the {error_code} code, got {:?}",
        answer.body
    );
    assert_eq!(
        field_list(answer),
        vec![parameter.to_string()],
        "case {case:?}: expected the refusal to name {parameter}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.get("message").as_str(),
        Some(message),
        "case {case:?}: expected the rule stated in words, got {:?}",
        answer.body
    );
}

/// RULE 1: a registration type the service ties to one country, and publishes an
/// `errorCode` for, is judged on the country of an address the caller was
/// allowed to supply — so a type whose address the service derives is refused
/// for that member instead, and a type the service ties to no country of its own
/// takes the address it was given.
#[tokio::test]
async fn a_type_tied_to_one_country_is_judged_only_on_an_address_it_admits() {
    // The two codes this suite asserts are values of the enum the model
    // publishes, not names this emulator invented.
    for code in [
        ERROR_CODE_NON_INDIA_CUSTOMER_CAN_NOT_SET_PAN,
        ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN,
    ] {
        assert!(
            VALIDATION_EXCEPTION_ERROR_CODES.contains(&code),
            "the code answered must be one the model publishes, got {code:?}"
        );
    }

    let account_id = "000000000001";
    let not_submittable_message =
        format!("Legal address should not be provided for {INDIA_NAME} {INDIA_TYPE} Tax \
                 Registration.");

    // Rows differ in the country the submitted address names — countries on
    // three continents, two of which (Malaysia, Türkiye) have per-country rules
    // of their own that this refusal precedes, and the type's own country in
    // three spellings, `Address$countryCode` being a code whose case carries no
    // meaning — and in the registration type, so that the rule is pinned to the
    // types whose address the service derives: a type that admits a submitted
    // address takes the address it was given.
    let rows: &[(&str, Value, Expected)] = &[
        (
            "PAN submitting an address in Germany",
            entry_of(INDIA_TYPE, Some("DE")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in the United States",
            entry_of(INDIA_TYPE, Some("US")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in Brazil",
            entry_of(INDIA_TYPE, Some("BR")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in Malaysia",
            entry_of(INDIA_TYPE, Some("MY")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in Türkiye",
            entry_of(INDIA_TYPE, Some("TR")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in the United Kingdom",
            entry_of(INDIA_TYPE, Some("GB")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in India",
            entry_of(INDIA_TYPE, Some(INDIA)),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in India, lower-cased country code",
            entry_of(INDIA_TYPE, Some("in")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting an address in India, mixed-case country code",
            entry_of(INDIA_TYPE, Some("In")),
            Expected::AddressNotSubmittable,
        ),
        (
            "PAN submitting no address at all",
            entry_of(INDIA_TYPE, None),
            Expected::Stored(INDIA),
        ),
        (
            "CNPJ held in Germany",
            entry_of("CNPJ", Some("DE")),
            Expected::Stored("DE"),
        ),
        (
            "NRIC held in Germany",
            entry_of("NRIC", Some("DE")),
            Expected::Stored("DE"),
        ),
        (
            "VAT held in Germany",
            entry_of("VAT", Some("DE")),
            Expected::Stored("DE"),
        ),
    ];

    for (row, entry, expected) in rows {
        for writer in WRITERS {
            let case = format!("{row}, written via {writer}");
            let (router, _engine) = emulator_with_engine();
            let answer = write(&router, writer, account_id, entry).await;
            match expected {
                Expected::AddressNotSubmittable => {
                    assert_refused(
                        &case,
                        &answer,
                        ERROR_CODE_FIELD_VALIDATION_FAILED,
                        "LEGAL_ADDRESS",
                        &not_submittable_message,
                    );
                }
                Expected::Stored(country_code) => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the entry to be stored, got {answer:?}"
                    );
                    let got = read(&router, account_id).await;
                    assert_eq!(
                        got.body
                            .pointer("/taxRegistration/legalAddress/countryCode")
                            .and_then(Value::as_str),
                        Some(*country_code),
                        "case {case:?}: expected the record held in {country_code}, got {:?}",
                        got.body
                    );
                    continue;
                }
            }
            // A refused entry is a refused write: the account holds nothing,
            // whichever writer refused it.
            assert_eq!(
                stored_registration_id(&router, account_id).await,
                None,
                "case {case:?}: a refused write must store nothing"
            );
        }
    }
}

/// RULE 2: the registration type a stored registration blocks may not be set
/// while that record exists; the record is left untouched, and removing it lets
/// the write through.
#[tokio::test]
async fn a_stored_registration_blocks_the_type_it_is_published_to_block() {
    let account_id = "000000000001";
    let blocked_message = format!(
        "You can't set a {INDIA_TYPE} tax registration while there's a {BLOCKING_TYPE} tax \
         registration for this account. Delete the {BLOCKING_TYPE} tax registration first."
    );
    let submitted = entry_of(INDIA_TYPE, None);

    // Rows differ in the type the account already holds — six of the model's
    // nine `TaxRegistrationType` values
    // (`smithy_query(query="shape", shape="TaxRegistrationType")`) — and in the
    // country that record is held in, which the rule does not turn on: the
    // blocking type blocks the write wherever the blocked registration is held,
    // and no other stored type blocks it anywhere.
    let rows: &[(&str, Value, bool)] = &[
        (
            "a GST registration held in India",
            entry_of(BLOCKING_TYPE, Some(INDIA)),
            true,
        ),
        (
            "a GST registration held in Germany",
            entry_of(BLOCKING_TYPE, Some("DE")),
            true,
        ),
        (
            "a VAT registration held in Germany",
            entry_of("VAT", Some("DE")),
            false,
        ),
        (
            "a TIN registration held in Germany",
            entry_of("TIN", Some("DE")),
            false,
        ),
        (
            "a NIP registration held in Germany",
            entry_of("NIP", Some("DE")),
            false,
        ),
        (
            "a PAN registration held in India",
            entry_of(INDIA_TYPE, None),
            false,
        ),
    ];

    for (row, stored_entry, blocked) in rows {
        for writer in WRITERS {
            let case = format!("{row}, then a PAN written via {writer}");
            let (router, engine) = emulator_with_engine();
            let setup = write(&router, "PutTaxRegistration", account_id, stored_entry).await;
            assert_eq!(
                setup.status, 200,
                "case {case:?}: the setup write must succeed, got {setup:?}"
            );
            // The setup TRN's validation is what the clock owes; letting it fall
            // due keeps the pending-TRN conflict out of this rule's way.
            make_validations_due(&engine);
            let held = stored_registration_id(&router, account_id).await;

            let answer = write(&router, writer, account_id, &submitted).await;
            if !blocked {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a stored type that blocks nothing must be replaceable, got \
                     {answer:?}"
                );
                assert_eq!(
                    stored_registration_id(&router, account_id).await,
                    submitted
                        .get("registrationId")
                        .and_then(Value::as_str)
                        .map(std::string::ToString::to_string),
                    "case {case:?}: the account must hold the entry the write submitted"
                );
                continue;
            }

            if writer == &BATCH_WRITER {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: the batch call itself succeeds, got {answer:?}"
                );
                let rows = error_rows(&answer);
                assert_eq!(
                    rows,
                    vec![(
                        account_id.to_string(),
                        ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN.to_string(),
                        blocked_message.clone(),
                    )],
                    "case {case:?}: the batch must report the blocked account with the code the \
                     write failed with, got {rows:?}"
                );
                assert!(
                    answer.body.get("status").is_none(),
                    "case {case:?}: a batch that stored nothing has no stored status, got {:?}",
                    answer.body
                );
            } else {
                assert_refused(
                    &case,
                    &answer,
                    ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN,
                    "REGISTRATION_TYPE",
                    &blocked_message,
                );
            }
            assert_eq!(
                stored_registration_id(&router, account_id).await,
                held,
                "case {case:?}: the refused write must leave the record it blocked on untouched"
            );

            // Removing the blocking registration is what the refusal tells the
            // caller to do, so it must be what lets the write through.
            let deleted = call(
                &router,
                "DeleteTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: the blocking record must be deletable, got {deleted:?}"
            );
            let accepted = write(&router, writer, account_id, &submitted).await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: the write must go through once nothing blocks it, got {accepted:?}"
            );
            assert_eq!(
                accepted.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the write reports the status it stored, got {:?}",
                accepted.body
            );
            let got = read(&router, account_id).await;
            assert_eq!(
                got.body
                    .pointer("/taxRegistration/registrationType")
                    .and_then(Value::as_str),
                Some(INDIA_TYPE),
                "case {case:?}: the account must hold the registration it was refused, got {:?}",
                got.body
            );
        }
    }
}

/// RULE 2, on the batch's own channel: a batch write reports every account whose
/// stored registration blocks the submitted type, naming the account and the
/// code, while every other account it names is stored.
#[tokio::test]
async fn a_batch_reports_each_blocked_account_and_stores_the_rest() {
    let blocked_message = format!(
        "You can't set a {INDIA_TYPE} tax registration while there's a {BLOCKING_TYPE} tax \
         registration for this account. Delete the {BLOCKING_TYPE} tax registration first."
    );
    let submitted = entry_of(INDIA_TYPE, None);

    // Rows differ in how many accounts of each kind the one request names: a
    // blocked account alone, mixes of the three kinds, and a batch where every
    // account is blocked.
    let rows: &[(&str, usize, usize, usize)] = &[
        ("one blocked account alone", 1, 0, 0),
        ("one blocked, one holding a replaceable TRN", 1, 1, 0),
        ("two blocked, one unregistered", 2, 0, 1),
        ("one of each kind", 1, 1, 1),
        ("three blocked accounts", 3, 0, 0),
    ];

    for (case, blocked, replaceable, unregistered) in rows.iter().copied() {
        let (router, engine) = emulator_with_engine();
        // Interleave the kinds, so the order the response reports failures in is
        // asserted against the order the REQUEST named them.
        let mut named: Vec<(String, Option<Value>)> = Vec::new();
        for index in 0..blocked.max(replaceable).max(unregistered) {
            if index < blocked {
                named.push((
                    format!("{:012}", 1 + index),
                    Some(entry_of(BLOCKING_TYPE, Some(INDIA))),
                ));
            }
            if index < replaceable {
                named.push((
                    format!("{:012}", 101 + index),
                    Some(entry_of("VAT", Some("DE"))),
                ));
            }
            if index < unregistered {
                named.push((format!("{:012}", 201 + index), None));
            }
        }

        for (account_id, seed) in &named {
            if let Some(entry) = seed {
                let setup = write(&router, "PutTaxRegistration", account_id, entry).await;
                assert_eq!(
                    setup.status, 200,
                    "case {case:?}: seeding {account_id} failed, got {setup:?}"
                );
            }
        }
        make_validations_due(&engine);

        let ids: Vec<&str> = named
            .iter()
            .map(|(account_id, _)| account_id.as_str())
            .collect();
        let answer = call(
            &router,
            BATCH_WRITER,
            &json!({ "accountIds": &ids, "taxRegistrationEntry": submitted }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the batch call itself succeeds, got {answer:?}"
        );

        let expected: Vec<(String, String, String)> = named
            .iter()
            .filter(|(_, seed)| {
                seed.as_ref().and_then(|entry| {
                    entry.get("registrationType").and_then(Value::as_str)
                }) == Some(BLOCKING_TYPE)
            })
            .map(|(account_id, _)| {
                (
                    account_id.clone(),
                    ERROR_CODE_GST_EXISTENCE_BLOCK_SET_PAN.to_string(),
                    blocked_message.clone(),
                )
            })
            .collect();
        assert_eq!(
            error_rows(&answer),
            expected,
            "case {case:?}: exactly the blocked accounts must be reported, in the order the \
             request named them, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.get("status").as_str(),
            if expected.len() == named.len() {
                None
            } else {
                Some(STATUS_PENDING)
            },
            "case {case:?}: the batch reports a stored status only when it stored something, got \
             {:?}",
            answer.body
        );

        for (account_id, seed) in &named {
            let blocked_here = seed
                .as_ref()
                .and_then(|entry| entry.get("registrationType").and_then(Value::as_str))
                == Some(BLOCKING_TYPE);
            let got = read(&router, account_id).await;
            let expected_type = if blocked_here {
                BLOCKING_TYPE
            } else {
                INDIA_TYPE
            };
            assert_eq!(
                got.body
                    .pointer("/taxRegistration/registrationType")
                    .and_then(Value::as_str),
                Some(expected_type),
                "case {case:?}: {account_id} must hold a {expected_type} registration, got {:?}",
                got.body
            );
        }
    }
}
