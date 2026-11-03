// SPDX-License-Identifier: CC0-1.0

//! How the spelling of a supplemental tax registration number bears on the
//! identity of the record it is held in, end to end over the restJson1 wire.
//!
//! RULE: `PutSupplementalTaxRegistration` is a `Put` whose record is identified
//! by the TRN the entry names (the entry carries no `authorityId`), and that
//! identity is the NUMBER, not the string one submission spelled it with: the
//! separators a holder groups it with and the whitespace a submission pads it
//! with are no part of the number, so a write whose `registrationId` differs
//! from a held record's only in those re-spells one TRN and REPLACES that
//! record under the `authorityId` it was first stored under — one record, one
//! id, deletable by that one id — while a number that differs in its characters
//! is another TRN, recorded beside the first.
//!
//! The first table spans the class along what the rule generalises over: the
//! number first written (the punctuated form its authority prints, the bare
//! form, one character — the low bound of the member's 1..=200 length — the
//! 200-character bound itself, a mixed-case number and one carrying non-ASCII
//! letters) crossed with the form it is re-spelled in (every separator the
//! normalisation treats as no part of a number, used to group the digits; every
//! separator removed; the number re-cased; and each whitespace padding form the
//! member's non-blank pattern admits). The second table is the rule's other
//! side: the differences that DO name a second TRN, with the re-spelling as its
//! control row.
//!
//! Every row reads the store back through a SECOND mount of the service on the
//! same engine, so what it asserts came from persisted state, and against the
//! records the store actually holds.
//!
//! The harness (`emulator_with_engine`, `mount_on`, `call`) lives in
//! `tax_registration_lifecycle.rs`, the entry builder, the write and the
//! listing reader in `supplemental_tax_registrations.rs`, and the supported
//! jurisdiction and the reader of a write's reported authority in
//! `shared_supplemental_lifecycle.rs`; nothing here builds a second copy of any
//! of them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::ComAmazonawsTaxsettingsTaxsettingsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::shared_supplemental_lifecycle::{reported_authority, supported_country};
use crate::supplemental_tax_registrations::{
    emulator_supplementing, entry_for, list_all, make_supplemental_validations_due, put,
};
use crate::tax_registration_lifecycle::{call, mount_on};

/// The published length of `SupplementalTaxRegistrationEntry$registrationId`:
/// `@length` 1..200 (`smithy_query(query="shape",
/// shape="SupplementalTaxRegistrationEntry")`). A re-spelling is still a
/// submission, so no row submits a number past it.
const REGISTRATION_ID_BOUND: usize = 200;

/// The characters a holder writes a registration number's groups apart with,
/// which are no part of the number — the set this service's number
/// normalisation removes (`src/state/registration_number.rs`).
const SEPARATORS: &[(&str, char)] = &[
    ("dots", '.'),
    ("hyphens", '-'),
    ("slashes", '/'),
    ("underscores", '_'),
    ("spaces", ' '),
];

/// The whitespace forms a submission may surround the number with — what the
/// member's non-blank pattern `^(?!\s*$)[\s\S]+$` admits around a non-blank
/// value.
const PADDINGS: &[(&str, &str, &str)] = &[
    ("a trailing space", "", " "),
    ("a leading space", " ", ""),
    ("spaces on both sides", "  ", "  "),
    ("a leading tab", "\t", ""),
    ("a trailing newline", "", "\n"),
    ("mixed whitespace on both sides", " \t", "\r\n "),
];

/// `number` with every separator and every space removed: the number with
/// nothing but its own characters left.
fn bare(number: &str) -> String {
    number
        .chars()
        .filter(|character| {
            !character.is_whitespace()
                && !SEPARATORS
                    .iter()
                    .any(|(_, separator)| character == separator)
        })
        .collect()
}

/// `number`'s characters written in groups of `size` separated by `separator`.
fn grouped(number: &str, separator: char, size: usize) -> String {
    let mut spelled = String::new();
    for (index, character) in number.chars().enumerate() {
        if index > 0 && index % size == 0 {
            spelled.push(separator);
        }
        spelled.push(character);
    }
    spelled
}

/// The ways one number may be re-spelled without naming another number: its
/// groups written apart with each separator, its separators removed, the number
/// re-cased, and each padding form around it.
///
/// A spelling past the member's published length is not submittable, so it is
/// left out rather than asserted on.
fn respellings(number: &str) -> Vec<(String, String)> {
    let bare = bare(number);
    let mut forms = vec![
        ("as first written".to_string(), number.to_string()),
        ("every separator removed".to_string(), bare.clone()),
    ];
    // Case is folded for the letters the comparison folds; a number carrying
    // non-ASCII letters is re-spelled only in its separators and its padding.
    if number.is_ascii() {
        forms.push(("upper-cased".to_string(), number.to_uppercase()));
        forms.push(("lower-cased".to_string(), number.to_lowercase()));
        forms.push((
            "the bare number lower-cased".to_string(),
            bare.to_lowercase(),
        ));
    }
    for (name, separator) in SEPARATORS {
        forms.push((
            format!("grouped in threes by {name}"),
            grouped(&bare, *separator, 3),
        ));
        forms.push((
            format!("every character written apart by {name}"),
            grouped(&bare, *separator, 1),
        ));
    }
    for (padding, before, after) in PADDINGS {
        forms.push((
            format!("{padding} around the number as first written"),
            format!("{before}{number}{after}"),
        ));
        forms.push((
            format!("{padding} around the bare number"),
            format!("{before}{bare}{after}"),
        ));
    }
    forms
        .into_iter()
        .filter(|(_, spelled)| {
            spelled.chars().count() <= REGISTRATION_ID_BOUND && !spelled.trim().is_empty()
        })
        .collect()
}

/// The numbers the identity table writes first.
fn first_numbers() -> Vec<(&'static str, String)> {
    vec![
        (
            "the punctuated form its authority prints",
            "CHE-123.456.789".to_string(),
        ),
        ("the bare form", "CHE123456789".to_string()),
        ("one character, the low bound", "7".to_string()),
        (
            "a number at the 200-character bound",
            "C".repeat(REGISTRATION_ID_BOUND),
        ),
        (
            "a mixed-case number",
            "che-123.456.789".to_string(),
        ),
        (
            "a number carrying non-ASCII letters",
            "ÖNR-12.345".to_string(),
        ),
    ]
}

/// How many supplemental records the store holds — the records a caller's
/// listing is projected from.
fn resident_records(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsTaxsettingsTaxsettingsState>()
        .lock()
        .supplemental_registrations
        .len()
}

/// The supplemental TRNs a SECOND mount of the service on the same engine
/// reports, so what a row asserts came from persisted state.
async fn held(engine: &Arc<engine::InMemoryEngine>, case: &str) -> Vec<Value> {
    let listed = list_all(&mount_on(engine), None).await;
    assert_eq!(
        listed.len(),
        resident_records(engine),
        "case {case:?}: the listing reports the records the store holds, got {listed:?}"
    );
    listed
}

/// Write `registration_id` under `legal_name`, asserting the write was applied.
async fn write(
    router: &MultiServiceRouter,
    registration_id: &str,
    legal_name: &str,
    case: &str,
) -> String {
    let answer = put(
        router,
        &entry_for(supported_country(), registration_id, legal_name),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (200, ""),
        "case {case:?}: expected the write of {registration_id:?} to be applied, got {answer:?}"
    );
    let authority_id = reported_authority(&answer);
    assert!(
        !authority_id.is_empty() && authority_id.chars().count() <= REGISTRATION_ID_BOUND,
        "case {case:?}: the write reports the authority it filed under, got {authority_id:?}"
    );
    authority_id
}

/// RULE: a `registrationId` that differs from a held record's only in the
/// separators it is grouped with or the whitespace around it names the SAME TRN,
/// so the write replaces that one record under the `authorityId` it was first
/// stored under, leaves one record holding the members the last write submitted,
/// and that one id deletes it.
#[tokio::test]
async fn re_spelling_a_supplemental_number_updates_the_one_record_it_names() {
    for (number_case, number) in first_numbers() {
        for (spelling, respelled) in respellings(&number) {
            let case = format!("{number_case}, re-spelled {spelling}");
            let (router, engine) = emulator_supplementing();

            let first_id = write(&router, &number, "Acme AG", &case).await;
            // A write aimed at a record whose own validation is still
            // outstanding conflicts with the current state of that record, so the
            // clock settles what it owed before the second write: what this table
            // is about is which record a re-spelled number names.
            make_supplemental_validations_due(&engine);
            let second_id = write(&router, &respelled, "Acme Holding AG", &case).await;
            assert_eq!(
                second_id, first_id,
                "case {case:?}: a write of {respelled:?} re-spells {number:?}, so it must answer \
                 the authorityId that TRN was first stored under, got {second_id:?} after \
                 {first_id:?}"
            );

            let records = held(&engine, &case).await;
            assert_eq!(
                records.len(),
                1,
                "case {case:?}: expected the re-spelled write to replace the one record of \
                 {number:?}, got {records:?}"
            );
            let record = &records[0];
            assert_eq!(
                record.get("authorityId").and_then(Value::as_str),
                Some(first_id.as_str()),
                "case {case:?}: the record is reported under the id it was first stored under, \
                 got {record:?}"
            );
            // The identity is the number; the stored VALUE is what the last
            // write submitted, spelling included.
            assert_eq!(
                record.get("registrationId").and_then(Value::as_str),
                Some(respelled.as_str()),
                "case {case:?}: expected the last write's number as submitted, got {record:?}"
            );
            assert_eq!(
                record.get("legalName").and_then(Value::as_str),
                Some("Acme Holding AG"),
                "case {case:?}: expected the last write's legal name, got {record:?}"
            );

            // One record under one id: deleting that id leaves nothing behind.
            let removed = call(
                &router,
                "DeleteSupplementalTaxRegistration",
                &json!({ "authorityId": first_id }),
            )
            .await;
            assert_eq!(
                (removed.status, removed.error_code.as_str()),
                (200, ""),
                "case {case:?}: expected the record to delete under {first_id:?}, got {removed:?}"
            );
            let remaining = held(&engine, &case).await;
            assert!(
                remaining.is_empty(),
                "case {case:?}: deleting the TRN's one id leaves no record of it, got {remaining:?}"
            );
        }
    }
}

/// One row of the table of differences that DO make a second supplemental TRN.
struct Distinct {
    case: &'static str,
    first: String,
    second: String,
    expected: usize,
}

/// RULE's other side: a number whose own characters differ from a held record's
/// — a changed, added, removed or reordered character — is another TRN, recorded
/// beside the first under its own `authorityId`.
#[tokio::test]
async fn a_number_that_differs_in_its_characters_is_another_supplemental_trn() {
    let number = "CHE-123.456.789";
    let rows = vec![
        Distinct {
            case: "one digit changed",
            first: number.to_string(),
            second: "CHE-123.456.780".to_string(),
            expected: 2,
        },
        Distinct {
            case: "a digit appended",
            first: number.to_string(),
            second: "CHE-123.456.7890".to_string(),
            expected: 2,
        },
        Distinct {
            case: "a digit dropped",
            first: number.to_string(),
            second: "CHE-123.456.78".to_string(),
            expected: 2,
        },
        Distinct {
            case: "the digits reordered",
            first: number.to_string(),
            second: "CHE-123.465.789".to_string(),
            expected: 2,
        },
        Distinct {
            case: "the authority prefix dropped",
            first: number.to_string(),
            second: "123.456.789".to_string(),
            expected: 2,
        },
        Distinct {
            case: "another number entirely",
            first: number.to_string(),
            second: "CHE-987.654.321".to_string(),
            expected: 2,
        },
        // The control: the same number, re-grouped and padded — the one
        // difference the identity does not keep.
        Distinct {
            case: "the same number re-grouped and padded",
            first: number.to_string(),
            second: "  che/123/456/789 ".to_string(),
            expected: 1,
        },
    ];

    for row in rows {
        let case = row.case;
        let expected = row.expected;
        let (router, engine) = emulator_supplementing();
        let first_id = write(&router, &row.first, "Acme AG", case).await;
        // See the sibling suite above: the first record's outstanding validation
        // is settled so that what the second write meets is the identity rule.
        make_supplemental_validations_due(&engine);
        let second_id = write(&router, &row.second, "Acme Holding AG", case).await;
        if expected == 1 {
            assert_eq!(
                second_id, first_id,
                "case {case:?}: {:?} names the TRN {:?} is held under, got {second_id:?} after \
                 {first_id:?}",
                row.second, row.first
            );
        } else {
            assert_ne!(
                second_id, first_id,
                "case {case:?}: {:?} is another TRN than {:?}, so it must get an authorityId of \
                 its own, got {second_id:?}",
                row.second, row.first
            );
        }

        let records = held(&engine, case).await;
        assert_eq!(
            records.len(),
            expected,
            "case {case:?}: expected {expected} record(s) for {:?} and {:?}, got {records:?}",
            row.first,
            row.second
        );
        assert!(
            records
                .iter()
                .any(|record| record.get("registrationId").and_then(Value::as_str)
                    == Some(row.second.as_str())),
            "case {case:?}: the last write's number must read back as submitted, got {records:?}"
        );
    }
}
