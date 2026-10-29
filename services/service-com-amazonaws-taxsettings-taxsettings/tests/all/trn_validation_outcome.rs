// SPDX-License-Identifier: CC0-1.0

//! What the TRN validation an accepted write starts reaches, over the restJson1
//! wire.
//!
//! ONE rule: the validation judges the submitted NUMBER against the structure
//! its jurisdiction publishes for that registration type — the layout and, where
//! the authority publishes one, the check digit — so a stored registration whose
//! number satisfies that structure settles `Verified` and one whose number does
//! not settles `Rejected`, while a jurisdiction and type this service holds no
//! published structure for is not judged at all. The outcome is fixed by what the
//! write stored, so every read path reports one record the same way, however
//! often it is read.
//!
//! The table spans the class the rule generalises over: each jurisdiction whose
//! structure this service holds, with a number that satisfies it and a number
//! that does not, a jurisdiction it holds none for, and both read paths. The
//! `Verified` side of the same rule, across the dimensions the outcome is NOT a
//! function of, is in `tests/all/accepted_registration_verifies.rs`.
//!
//! The harness (`emulator_with_engine`, `call`, the two helpers that move a
//! clock-owed TRN validation, and the status reader) lives in
//! `tax_registration_lifecycle.rs` and `fidelity_rules.rs`; nothing here builds a
//! second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    STATUS_PENDING, STATUS_REJECTED, STATUS_VERIFIED,
};

use crate::fidelity_rules::read_status;
use crate::tax_registration_lifecycle::{
    call, emulator_with_engine, hold_validations_open, make_validations_due,
};

/// The read paths that report the status of an account's own TRN.
const TRN_READERS: &[&str] = &["GetTaxRegistration", "ListTaxRegistrations"];

/// One stored registration and the terminal status its number earns.
struct Row {
    case: &'static str,
    /// The `taxRegistrationEntry` to store. Every row is an entry this service
    /// accepts: the per-country write rules are about the entry's MEMBERS, not
    /// about the number, so a number that will fail its validation is still
    /// stored with a 200.
    entry: Value,
    /// The status the record settles to once the clock has made its validation
    /// due.
    settles_to: &'static str,
}

/// A German VAT entry carrying `registration_id`.
fn german_vat(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "VAT",
        "legalName": "Beispiel GmbH",
        "legalAddress": {
            "addressLine1": "Unter den Linden 1",
            "city": "Berlin",
            "postalCode": "10117",
            "countryCode": "DE",
        },
    })
}

/// A French VAT entry carrying `registration_id`. France's documented write rule
/// is the `sirenNumber` block (`crate::state::COUNTRY_RULES`), which every row
/// carries so the entry is accepted whatever its number.
fn french_vat(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "VAT",
        "legalName": "Exemple SARL",
        "legalAddress": {
            "addressLine1": "1 Rue de la Paix",
            "city": "Paris",
            "postalCode": "75002",
            "countryCode": "FR",
        },
        "additionalTaxInformation": {
            "franceAdditionalInfo": { "sirenNumber": "345678901" },
        },
    })
}

/// A Brazilian CNPJ entry, whose legal name and address the service fills in.
fn brazilian_cnpj(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "CNPJ",
    })
}

/// A Brazilian CPF entry. Brazil's CPF rows of the write rules require the
/// address's `stateOrRegion`, so every row carries it.
fn brazilian_cpf(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "CPF",
        "legalName": "Exemplo da Silva",
        "legalAddress": {
            "addressLine1": "Avenida Paulista 1",
            "city": "Sao Paulo",
            "stateOrRegion": "SP",
            "postalCode": "01310-000",
            "countryCode": "BR",
        },
    })
}

/// An Indian PAN entry, whose legal address the service fills in from the number.
fn indian_pan(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "PAN",
        "legalName": "Example India Private Limited",
    })
}

/// An Indian GST entry. India's write rule requires the address's
/// `stateOrRegion`.
fn indian_gst(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "GST",
        "legalName": "Example India Private Limited",
        "legalAddress": {
            "stateOrRegion": "Karnataka",
            "city": "Bengaluru",
            "postalCode": "560025",
            "countryCode": "IN",
        },
    })
}

/// A Kenyan TIN entry. Kenya's write rule requires the `personType`, and a
/// `Business` one needs no certificate document.
fn kenyan_tin(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "TIN",
        "legalName": "Example Kenya Limited",
        "legalAddress": {
            "addressLine1": "1 Kenyatta Avenue",
            "city": "Nairobi",
            "postalCode": "00100",
            "countryCode": "KE",
        },
        "additionalTaxInformation": {
            "kenyaAdditionalInfo": { "personType": "Business" },
        },
    })
}

/// A Malaysian NRIC entry — a jurisdiction and type this service holds no
/// published structure for. Malaysia's write rules pair `NRIC` with the
/// `Individual` sector.
fn malaysian_nric(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "NRIC",
        "legalName": "Siti Example",
        "sector": "Individual",
        "legalAddress": {
            "addressLine1": "1 Jalan Example",
            "city": "Kuala Lumpur",
            "postalCode": "50000",
            "countryCode": "MY",
        },
    })
}

/// Each jurisdiction whose structure this service holds, with a number that
/// satisfies it and one that does not, plus a jurisdiction it holds none for.
///
/// Every admitted number below carries the check digits or layout its own
/// authority publishes, and each refused one differs from an admitted number only
/// in that check digit or in that layout — so no row passes because the number
/// looks odd.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a German VAT number whose check digit is the published one",
            entry: german_vat("DE136695976"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "the same German number without the country prefix",
            entry: german_vat("136695976"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a German VAT number whose check digit is not the published one",
            entry: german_vat("DE123456789"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a German VAT number of the wrong length",
            entry: german_vat("DE13669597"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a French VAT number whose key matches its SIREN",
            entry: french_vat("FR15345678901"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a French VAT number whose key does not match its SIREN",
            entry: french_vat("FR12345678901"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a CNPJ written with the separators the register prints",
            entry: brazilian_cnpj("12.345.678/0001-95"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "the same CNPJ written as bare digits",
            entry: brazilian_cnpj("12345678000195"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a CNPJ whose second check digit is wrong",
            entry: brazilian_cnpj("12345678000196"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a CPF whose check digits are the published ones",
            entry: brazilian_cpf("111.444.777-35"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a CPF whose check digits are not",
            entry: brazilian_cpf("111.444.777-36"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a PAN in the published layout",
            entry: indian_pan("ABCDE1234F"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a PAN whose letters and digits are in the wrong places",
            entry: indian_pan("ABCD12345F"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a GSTIN in the published layout",
            entry: indian_gst("29ABCDE1234F1Z5"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a GSTIN missing the fixed letter of the layout",
            entry: indian_gst("29ABCDE1234F1X5"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a Kenyan PIN in the published layout",
            entry: kenyan_tin("P051234567M"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a Kenyan PIN whose taxpayer-class letter is not one published",
            entry: kenyan_tin("X051234567M"),
            settles_to: STATUS_REJECTED,
        },
        Row {
            case: "a Malaysian NRIC, whose structure this service holds none of",
            entry: malaysian_nric("800101011234"),
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a Malaysian number nothing resembles, judged by no structure",
            entry: malaysian_nric("not-a-number-at-all"),
            settles_to: STATUS_VERIFIED,
        },
    ]
}

/// RULE: the terminal status of a stored TRN is the outcome of validating the
/// submitted number against the structure its jurisdiction publishes — `Verified`
/// where the number satisfies it, `Rejected` where it does not — reported the
/// same way by every read path and by every read.
#[tokio::test]
async fn the_terminal_status_of_a_stored_trn_is_what_its_number_earns() {
    for row in rows() {
        let (router, engine) = emulator_with_engine();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": row.entry }),
        )
        .await;
        // Acceptance and the outcome of validating the number are separate
        // facts: the write stores the entry and reports the outstanding status,
        // whichever status the validation will reach.
        assert_eq!(
            put.status, 200,
            "case {:?}: the entry must be accepted, got {put:?}",
            row.case
        );
        assert_eq!(
            put.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {:?}: the write must report the outstanding status, got {:?}",
            row.case,
            put.body
        );

        // While the validation is outstanding no reader reports an outcome.
        hold_validations_open(&engine);
        for reader in TRN_READERS {
            let status = read_status(&router, reader, account_id).await;
            assert_eq!(
                status.as_deref(),
                Some(STATUS_PENDING),
                "case {:?}: {reader} must report the outstanding status, got {status:?}",
                row.case
            );
        }

        // Once the clock has made it due, every reader reports the outcome the
        // number earned — and keeps reporting it.
        make_validations_due(&engine);
        for reader in TRN_READERS.iter().chain(TRN_READERS.iter()) {
            let status = read_status(&router, reader, account_id).await;
            assert_eq!(
                status.as_deref(),
                Some(row.settles_to),
                "case {:?}: {reader} must report {:?}, got {status:?}",
                row.case,
                row.settles_to
            );
        }
    }
}

/// RULE: both outcomes are reachable — the state machine of a stored
/// registration has more than one terminal value, so a client that polls for one
/// of them can see it.
#[tokio::test]
async fn both_validation_outcomes_are_reachable() {
    let mut reached = Vec::new();
    for row in rows() {
        let (router, engine) = emulator_with_engine();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": row.entry }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: the entry must be accepted, got {put:?}",
            row.case
        );
        make_validations_due(&engine);
        if let Some(status) = read_status(&router, "GetTaxRegistration", account_id).await {
            if !reached.contains(&status) {
                reached.push(status);
            }
        }
    }
    reached.sort();
    assert_eq!(
        reached,
        vec![STATUS_REJECTED.to_string(), STATUS_VERIFIED.to_string()],
        "the stored registrations above must reach both terminal outcomes, got {reached:?}"
    );
}

/// RULE: a validation that failed changes nothing but the status — the record the
/// write stored is still the record every read reports, so a client that polls to
/// a `Rejected` status can see what it submitted.
#[tokio::test]
async fn a_failed_validation_leaves_the_stored_record_as_it_was() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000002";
    let registration_id = "DE123456789";
    let put = call(
        &router,
        "PutTaxRegistration",
        &json!({
            "accountId": account_id,
            "taxRegistrationEntry": german_vat(registration_id),
        }),
    )
    .await;
    assert_eq!(put.status, 200, "the entry must be accepted, got {put:?}");

    make_validations_due(&engine);
    let read = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(read.status, 200, "GetTaxRegistration expected 200, got {read:?}");
    let reported = read.get("taxRegistration").clone();
    assert_eq!(
        reported.get("status").and_then(Value::as_str),
        Some(STATUS_REJECTED),
        "the record must report the outcome its number earned, got {reported:?}"
    );
    assert_eq!(
        reported.get("registrationId").and_then(Value::as_str),
        Some(registration_id),
        "the record must still carry the submitted number, got {reported:?}"
    );
    assert_eq!(
        reported.get("legalName").and_then(Value::as_str),
        Some("Beispiel GmbH"),
        "the record must still carry the submitted legal name, got {reported:?}"
    );
}
