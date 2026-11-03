// SPDX-License-Identifier: CC0-1.0

//! What the validation of a STORED tax registration reaches, over the wire.
//!
//! ONE rule: the validation a write starts judges the submitted NUMBER against
//! the structure its jurisdiction publishes for it, so a stored registration
//! whose number satisfies that structure is reported `Verified` by every read
//! path once the clock has made the validation due, and by none of them as a
//! validation that failed. The tables span the dimensions the outcome is not a
//! function of — the registration type, the jurisdiction, the sector, whether
//! documents were submitted, whether the entry carried its own legal address,
//! and whether the record is the account's TRN or a supplemental one — rather
//! than the one entry that happened to be recorded. The outcome for a number
//! that does NOT satisfy its published structure is pinned in
//! `tests/all/trn_validation_outcome.rs`.
//!
//! The harness (`emulator_with_engine`, `call`, the two helpers that move a
//! clock-owed TRN validation and the one that reads a status back) lives in
//! `tax_registration_lifecycle.rs`, `fidelity_rules.rs` and
//! `supplemental_tax_registrations.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    STATUS_DELETED, STATUS_PENDING, STATUS_REJECTED, STATUS_VERIFIED,
};

use crate::fidelity_rules::read_status;
use crate::supplemental_tax_registrations::{
    emulator_supplementing, entry_for, hold_supplemental_validations_open, list_all,
    make_supplemental_validations_due, put as put_supplemental,
};
use crate::tax_registration_lifecycle::{
    call, emulator_with_engine, hold_validations_open, make_validations_due,
};

/// The `TaxRegistrationStatus` values that report a validation which did NOT
/// succeed, from the model's enum
/// (`smithy_query(query="shape", shape="TaxRegistrationStatus")` answers
/// `Deleted | Pending | Rejected | Verified`): a stored registration whose
/// validation has run reports neither.
const UNSUCCESSFUL_STATUSES: &[&str] = &[STATUS_REJECTED, STATUS_DELETED];

/// The read paths that report the status of an account's own TRN.
const TRN_READERS: &[&str] = &["GetTaxRegistration", "ListTaxRegistrations"];

/// The entries the table stores: one per dimension the validation outcome is not
/// a function of. Every row is an entry this service accepts — a submission it
/// does not accept is refused by the write instead of stored (see
/// `tests/all/country_rules.rs` and `tests/all/registration_type_refusals.rs`) —
/// and every row's number satisfies the structure its jurisdiction publishes for
/// it.
fn accepted_entries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a VAT registration in Germany",
            json!({
                // Nine digits closed by the check digit Germany's rule computes
                // for the eight before it.
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": {
                    "addressLine1": "Unter den Linden 1",
                    "city": "Berlin",
                    "postalCode": "10117",
                    "countryCode": "DE",
                },
            }),
        ),
        (
            "a GST registration in India declaring a sector",
            json!({
                "registrationId": "29ABCDE1234F1Z5",
                "registrationType": "GST",
                "legalName": "Acme India Private Limited",
                "sector": "Business",
                "legalAddress": {
                    "stateOrRegion": "Karnataka",
                    "postalCode": "560025",
                    "countryCode": "IN",
                },
            }),
        ),
        (
            "a TIN registration in Spain carrying a certificate document",
            json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
                // Spain's documented rules: the registrationType must be
                // specified, and a `Local` one needs the certificate document.
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Local" },
                },
                "verificationDetails": {
                    "taxRegistrationDocuments": [
                        { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
                    ],
                },
            }),
        ),
        (
            "a TIN registration in Kenya for a business",
            json!({
                "registrationId": "P051234567M",
                "registrationType": "TIN",
                "legalName": "Acme Kenya Limited",
                "legalAddress": {
                    "addressLine1": "1 Kenyatta Avenue",
                    "city": "Nairobi",
                    "postalCode": "00100",
                    "countryCode": "KE",
                },
                // Kenya's documented rule: the personType must be specified.
                "additionalTaxInformation": {
                    "kenyaAdditionalInfo": { "personType": "Business" },
                },
            }),
        ),
        (
            "an NRIC registration in Malaysia for an individual",
            json!({
                "registrationId": "800101011234",
                "registrationType": "NRIC",
                "legalName": "Siti Example",
                "sector": "Individual",
                "legalAddress": {
                    "addressLine1": "1 Jalan Example",
                    "city": "Kuala Lumpur",
                    "postalCode": "50000",
                    "countryCode": "MY",
                },
            }),
        ),
        (
            "a PAN registration in India, whose legal address the service fills in",
            json!({
                "registrationId": "ABCDE1234F",
                "registrationType": "PAN",
                "legalName": "Acme India Private Limited",
            }),
        ),
        (
            "a CNPJ registration in Brazil, which needs no legal address",
            json!({
                "registrationId": "12345678000195",
                "registrationType": "CNPJ",
                "legalName": "Acme Brasil Ltda",
            }),
        ),
    ]
}

/// RULE: a tax registration the service stored whose number satisfies the
/// structure its jurisdiction publishes passed its validation, so once the clock
/// has made that validation due every read path reports it `Verified`, repeatedly
/// and whoever calls — never the status of a validation that failed.
#[tokio::test]
async fn an_accepted_tax_registration_settles_verified_on_every_read_path() {
    for (case, entry) in accepted_entries() {
        let (router, engine) = emulator_with_engine();
        let account_id = "000000000001";
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the entry must be accepted, got {put:?}"
        );
        assert_eq!(
            put.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {case:?}: the write reports the outstanding status it stored, got {:?}",
            put.body
        );

        // While the validation is outstanding every reader reports `Pending`:
        // the successful outcome is not reported before the validation is due.
        hold_validations_open(&engine);
        for reader in TRN_READERS {
            let status = read_status(&router, reader, account_id).await;
            assert_eq!(
                status.as_deref(),
                Some(STATUS_PENDING),
                "case {case:?}: {reader} must report the outstanding status, got {status:?}"
            );
        }

        // Once it is due, every reader reports the successful outcome, and keeps
        // reporting it: settling what the clock owed is not a change a read made.
        make_validations_due(&engine);
        for reader in TRN_READERS.iter().chain(TRN_READERS.iter()) {
            let status = read_status(&router, reader, account_id).await;
            assert_eq!(
                status.as_deref(),
                Some(STATUS_VERIFIED),
                "case {case:?}: {reader} must report the accepted registration verified, got \
                 {status:?}"
            );
            for unsuccessful in UNSUCCESSFUL_STATUSES {
                assert_ne!(
                    status.as_deref(),
                    Some(*unsuccessful),
                    "case {case:?}: {reader} must not report {unsuccessful} for a registration \
                     the service accepted, got {status:?}"
                );
            }
        }
    }
}

/// RULE: the same for a supplemental registration — the record the listing
/// reports for an accepted supplemental TRN is `Pending` while its validation is
/// outstanding and, where no published structure contradicts its number,
/// `Verified` once the clock has made it due.
#[tokio::test]
async fn an_accepted_supplemental_registration_settles_verified_in_the_listing() {
    // The rows differ in the registration id and in how much of the address the
    // entry filled in; the jurisdiction a supplemental TRN may be held in is a
    // separate rule with its own suite, so every row uses a supported one.
    let rows: &[(&str, &str, &str)] = &[
        ("a Swiss supplemental VAT registration", "CHE-123.456.789", "Acme AG"),
        ("another Swiss supplemental VAT registration", "CHE-987.654.321", "Acme Holding AG"),
        ("a supplemental registration at the 1-character bound", "1", "A"),
    ];

    for (case, registration_id, legal_name) in rows.iter().copied() {
        // A supplemental registration supplements the account's own, so the
        // account holds a settled registration before the write under test.
        let (router, engine) = emulator_supplementing();
        let written = put_supplemental(&router, &entry_for("CH", registration_id, legal_name)).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the entry must be accepted, got {written:?}"
        );
        assert_eq!(
            written.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {case:?}: the write reports the outstanding status it stored, got {:?}",
            written.body
        );

        hold_supplemental_validations_open(&engine);
        let status = listed_status(&router, registration_id).await;
        assert_eq!(
            status.as_deref(),
            Some(STATUS_PENDING),
            "case {case:?}: the listing must report the outstanding status, got {status:?}"
        );

        make_supplemental_validations_due(&engine);
        for attempt in 0..2 {
            let status = listed_status(&router, registration_id).await;
            assert_eq!(
                status.as_deref(),
                Some(STATUS_VERIFIED),
                "case {case:?}: read {attempt} must report the accepted registration verified, \
                 got {status:?}"
            );
        }
    }
}

/// The status the supplemental listing reports for `registration_id`, or `None`
/// when it reports no such registration.
async fn listed_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    registration_id: &str,
) -> Option<String> {
    list_all(router, None)
        .await
        .into_iter()
        .find(|listed| listed.get("registrationId").and_then(Value::as_str) == Some(registration_id))
        .and_then(|listed| {
            listed
                .get("status")
                .and_then(Value::as_str)
                .map(std::string::ToString::to_string)
        })
}
