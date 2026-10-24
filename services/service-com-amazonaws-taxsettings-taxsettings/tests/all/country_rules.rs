// SPDX-License-Identifier: CC0-1.0

//! One rule set, both writers: the country rules `PutTaxRegistration` is judged
//! by judge a `BatchPutTaxRegistration` entry identically.
//!
//! `tests/all/country_requirements.rs` asserts WHAT the per-country rules are,
//! through the single-account writer. This suite asserts that the batch writer
//! reaches the SAME rule set rather than a copy of it: every row below is driven
//! through both operations and their answers are compared to each other, so a
//! rule added to `crate::state::COUNTRY_RULES` for one writer cannot be missing
//! from the other.
//!
//! The rows are taken from `crate::state::COUNTRY_RULES` — one per class of
//! documented requirement (a missing `additionalTaxInformation` member, a missing
//! entry member, a missing address member, a value outside the documented set) —
//! plus, for each, the entry that satisfies that country's rules.
//!
//! The harness (`emulator`, `call`, `Answer`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{COUNTRY_RULES, STATUS_PENDING};

use crate::tax_registration_lifecycle::{call, emulator, Answer};

/// Both operations that store a `TaxRegistrationEntry`. The model gives them the
/// same entry shape and the same country-specific documentation.
const WRITERS: &[&str] = &["PutTaxRegistration", "BatchPutTaxRegistration"];

/// Store `entry` for `account_id` through `writer`, whichever of the two it is.
async fn write(
    router: &smithy_server_runtime::MultiServiceRouter,
    writer: &str,
    account_id: &str,
    entry: &Value,
) -> Answer {
    let body = if writer == "BatchPutTaxRegistration" {
        json!({ "accountIds": [account_id], "taxRegistrationEntry": entry })
    } else {
        json!({ "accountId": account_id, "taxRegistrationEntry": entry })
    };
    call(router, writer, &body).await
}

/// The `errorCode` and the `fieldList` names a refusal carries — the parts of an
/// answer that must be the same whichever writer refused.
fn refusal(answer: &Answer) -> (u16, String, Option<String>, Vec<String>) {
    let names = answer
        .get("fieldList")
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    (
        answer.status,
        answer.error_code.clone(),
        answer
            .get("errorCode")
            .as_str()
            .map(std::string::ToString::to_string),
        names,
    )
}

/// A certificate document, as `VerificationDetails` carries one.
fn document() -> Value {
    json!({
        "taxRegistrationDocuments": [
            { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
        ],
    })
}

/// The rows: for one country, an entry its documented rules REFUSE and an entry
/// they ADMIT. One row per class of requirement in
/// `crate::state::COUNTRY_RULES`.
fn rows() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            // Kenya: "You must specify the personType in the kenyaAdditionalInfo
            // field of the additionalTaxInformation object." (the recorded case)
            "KE",
            json!({
                "registrationId": "P123456789K",
                "registrationType": "VAT",
                "legalName": "Acme Kenya Limited",
                "legalAddress": {
                    "addressLine1": "Kenyatta Ave 1",
                    "city": "Nairobi",
                    "postalCode": "00100",
                    "countryCode": "KE",
                },
            }),
            json!({
                "registrationId": "P123456789K",
                "registrationType": "VAT",
                "legalName": "Acme Kenya Limited",
                "legalAddress": {
                    "addressLine1": "Kenyatta Ave 1",
                    "city": "Nairobi",
                    "postalCode": "00100",
                    "countryCode": "KE",
                },
                "additionalTaxInformation": {
                    "kenyaAdditionalInfo": { "personType": "Legal Person" },
                },
            }),
        ),
        (
            // Turkey: "You must specify the sector in the taxRegistrationEntry
            // object" (an entry member rather than an additional-info one), plus
            // "For address, you must specify districtOrCounty".
            "TR",
            json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Anonim Sirketi",
                "legalAddress": {
                    "addressLine1": "Ataturk Cad 1",
                    "districtOrCounty": "Kadikoy",
                    "city": "Istanbul",
                    "postalCode": "34710",
                    "countryCode": "TR",
                },
            }),
            json!({
                "registrationId": "1234567890",
                "registrationType": "VAT",
                "legalName": "Ornek Anonim Sirketi",
                "sector": "Individual",
                "legalAddress": {
                    "addressLine1": "Ataturk Cad 1",
                    "districtOrCounty": "Kadikoy",
                    "city": "Istanbul",
                    "postalCode": "34710",
                    "countryCode": "TR",
                },
            }),
        ),
        (
            // Saudi Arabia: "For address, you must specify addressLine3" (an
            // address member).
            "SA",
            json!({
                "registrationId": "300000000000003",
                "registrationType": "VAT",
                "legalName": "Example Trading Company",
                "legalAddress": {
                    "addressLine1": "King Fahd Road 1",
                    "city": "Riyadh",
                    "postalCode": "11564",
                    "countryCode": "SA",
                },
            }),
            json!({
                "registrationId": "300000000000003",
                "registrationType": "VAT",
                "legalName": "Example Trading Company",
                "legalAddress": {
                    "addressLine1": "King Fahd Road 1",
                    "addressLine3": "1234",
                    "city": "Riyadh",
                    "postalCode": "11564",
                    "countryCode": "SA",
                },
            }),
        ),
        (
            // Nepal: "The sector valid values are Business and Individual" (a
            // value outside the documented set, rather than a missing member).
            "NP",
            json!({
                "registrationId": "600000001",
                "registrationType": "VAT",
                "legalName": "Example Nepal Pvt Ltd",
                "sector": "Government",
                "legalAddress": {
                    "addressLine1": "Durbar Marg 1",
                    "city": "Kathmandu",
                    "postalCode": "44600",
                    "countryCode": "NP",
                },
            }),
            json!({
                "registrationId": "600000001",
                "registrationType": "VAT",
                "legalName": "Example Nepal Pvt Ltd",
                "sector": "Business",
                "legalAddress": {
                    "addressLine1": "Durbar Marg 1",
                    "city": "Kathmandu",
                    "postalCode": "44600",
                    "countryCode": "NP",
                },
            }),
        ),
        (
            // Bangladesh: "You must specify the tax registration certificate
            // document in the taxRegistrationDocuments field of the
            // VerificationDetails object" (a verification member).
            "BD",
            json!({
                "registrationId": "123456789-0101",
                "registrationType": "VAT",
                "legalName": "Example Bangladesh Limited",
                "legalAddress": {
                    "addressLine1": "Gulshan Avenue 1",
                    "city": "Dhaka",
                    "postalCode": "1212",
                    "countryCode": "BD",
                },
            }),
            json!({
                "registrationId": "123456789-0101",
                "registrationType": "VAT",
                "legalName": "Example Bangladesh Limited",
                "legalAddress": {
                    "addressLine1": "Gulshan Avenue 1",
                    "city": "Dhaka",
                    "postalCode": "1212",
                    "countryCode": "BD",
                },
                "verificationDetails": document(),
            }),
        ),
    ]
}

/// RULE: the per-country rules are one shared set, so both writers refuse the
/// same entry with the same refusal and store the same satisfying entry.
#[tokio::test]
async fn both_writers_judge_one_entry_by_the_same_country_rules() {
    for (country_code, refused_entry, admitted_entry) in rows() {
        assert!(
            COUNTRY_RULES
                .iter()
                .any(|rule| rule.country_code == country_code),
            "case {country_code:?}: the rule set must carry a rule for this country"
        );

        // The entry the country's rules refuse: both writers refuse it the same
        // way, and neither stores anything.
        let mut refusals = Vec::new();
        for writer in WRITERS {
            let case = format!("{country_code} refused via {writer}");
            let router = emulator();
            let account_id = "000000000001";
            let answer = write(&router, writer, account_id, &refused_entry).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: the entry must be refused, got {answer:?}"
            );
            let read = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: a refused write must store nothing, got {read:?}"
            );
            refusals.push((writer, refusal(&answer)));
        }
        assert_eq!(
            refusals[0].1, refusals[1].1,
            "case {country_code:?}: {} and {} must refuse one entry identically, got {refusals:?}",
            refusals[0].0, refusals[1].0
        );

        // And the entry they admit stores through both, reporting the status it
        // was stored with.
        for writer in WRITERS {
            let case = format!("{country_code} admitted via {writer}");
            let router = emulator();
            let account_id = "000000000001";
            let answer = write(&router, writer, account_id, &admitted_entry).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the entry must be stored, got {answer:?}"
            );
            assert_eq!(
                answer.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the write reports the status it stored, got {:?}",
                answer.body
            );
            let read = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            assert_eq!(
                read.body
                    .pointer("/taxRegistration/registrationId")
                    .and_then(Value::as_str),
                admitted_entry.get("registrationId").and_then(Value::as_str),
                "case {case:?}: the stored registration must read back, got {:?}",
                read.body
            );
        }
    }
}
