// SPDX-License-Identifier: CC0-1.0

//! The members of a `TaxRegistrationEntry` the service fills in itself, over the
//! wire.
//!
//! ONE rule: `legalName` and `legalAddress` are not `@required` on the entry, so
//! a write requires one only where the registration's jurisdiction does. Where
//! the service documents that it populates the member from the tax registration
//! number — Brazil for the legal name, Brazil's `CNPJ` type and Mexico for both,
//! and India's `PAN`, whose address AWS reads off the number — an entry that
//! omits it is STORED and reads back with the member filled in; everywhere else
//! the omission is `ValidationException` / `MissingInput` naming the parameter.
//!
//! The table spans the class that rule generalises over: both carved-out members,
//! each carved-out jurisdiction and each registration type the model ties to one
//! country, each side of every carve-out (the entry the jurisdiction admits and
//! the entry it refuses), a country code in either case, an entry carrying
//! neither member, and both writers of an entry. The Brazilian `CNPJ` entry
//! carrying only `registrationId` and `registrationType` is ONE row.
//!
//! The source of every row is the model:
//! `smithy_query(query="shape",shape="com.amazonaws.taxsettings#TaxRegistrationEntry")`
//! for the two members' own notes ("If you're setting a TRN in Brazil, you don't
//! need to specify the legal name. For TRNs in other countries, you must specify
//! the legal name"; "If you're setting a TRN in Brazil for the CNPJ tax type, you
//! don't need to specify the legal address. For TRNs in other countries and for
//! CPF tax types Brazil, you must specify the legal address") and
//! `smithy_query(query="doc",shape="com.amazonaws.taxsettings#BatchPutTaxRegistration")`
//! for Mexico ("You do not need to provide address and legal name. These will be
//! populated based on your tax registration number").
//!
//! The harness (`emulator_with_engine`, `call`, `hold_validations_open`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    DERIVED_MEMBER_UNKNOWN, ERROR_CODE_FIELD_VALIDATION_FAILED, ERROR_CODE_MISSING_INPUT,
    MISSING_LEGAL_ADDRESS, MISSING_LEGAL_ADDRESS_JURISDICTION, STATUS_PENDING,
};

use crate::country_requirements::certificate_document;
use crate::tax_registration_lifecycle::{
    call, emulator_with_engine, field_list, hold_validations_open,
};

/// The account every row writes to.
const ACCOUNT_ID: &str = "000000000001";

/// What the service must answer for one entry.
#[derive(Debug, Clone)]
enum Expect {
    /// The entry is stored: the reported registration carries this legal name
    /// and this `legalAddress.countryCode`, and — when the entry carried no
    /// address — the whole address the service filled in.
    Stored {
        legal_name: &'static str,
        country_code: &'static str,
        /// `true` when the stored address is the one the service filled in
        /// rather than one the request supplied.
        address_filled_in: bool,
    },
    /// The entry is refused with `ValidationException` (400) carrying
    /// `error_code` and naming `parameter` in `fieldList`, and nothing is
    /// stored.
    Refused {
        error_code: &'static str,
        parameter: &'static str,
    },
}

/// One entry submitted to `PutTaxRegistration`, and what must come back.
struct Case {
    name: &'static str,
    entry: Value,
    expect: Expect,
}

/// The expectation for an entry a carve-out admits, whose address the request
/// supplied.
const fn stored_with(legal_name: &'static str, country_code: &'static str) -> Expect {
    Expect::Stored {
        legal_name,
        country_code,
        address_filled_in: false,
    }
}

/// The expectation for an entry a carve-out admits that carried no address, so
/// the service filled the whole address in.
const fn stored_with_filled_in_address(
    legal_name: &'static str,
    country_code: &'static str,
) -> Expect {
    Expect::Stored {
        legal_name,
        country_code,
        address_filled_in: true,
    }
}

/// The expectation for an entry whose jurisdiction requires the member it omits.
const fn missing(parameter: &'static str) -> Expect {
    Expect::Refused {
        error_code: ERROR_CODE_MISSING_INPUT,
        parameter,
    }
}

/// The address the service reports for a registration whose entry carried none:
/// the jurisdiction the entry identified, and the model's own value for an
/// address member whose value is not known.
fn filled_in_address(country_code: &str) -> Value {
    json!({
        "addressLine1": DERIVED_MEMBER_UNKNOWN,
        "city": DERIVED_MEMBER_UNKNOWN,
        "postalCode": DERIVED_MEMBER_UNKNOWN,
        "countryCode": country_code,
    })
}

/// Every row of the table. Each entry satisfies every constraint the boundary
/// validator enforces, so what decides the answer is the jurisdiction's own rule
/// about the two optional members.
fn cases() -> Vec<Case> {
    vec![
        // Brazil, CNPJ: neither member is needed.
        Case {
            name: "Brazilian CNPJ entry carrying only the registration id and type",
            entry: json!({
                "registrationId": "11222333000181",
                "registrationType": "CNPJ",
            }),
            expect: stored_with_filled_in_address(DERIVED_MEMBER_UNKNOWN, "BR"),
        },
        Case {
            name: "Brazilian CNPJ entry carrying verification details and nothing else",
            entry: json!({
                "registrationId": "11222333000181",
                "registrationType": "CNPJ",
                "verificationDetails": { "dateOfBirth": "1980-01-01" },
            }),
            expect: stored_with_filled_in_address(DERIVED_MEMBER_UNKNOWN, "BR"),
        },
        Case {
            name: "Brazilian CNPJ entry carrying a legal name but no address",
            entry: json!({
                "registrationId": "11222333000181",
                "registrationType": "CNPJ",
                "legalName": "Exemplo Industria Ltda",
            }),
            expect: stored_with_filled_in_address("Exemplo Industria Ltda", "BR"),
        },
        Case {
            name: "Brazilian CNPJ entry carrying an address but no legal name",
            entry: json!({
                "registrationId": "11222333000181",
                "registrationType": "CNPJ",
                "legalAddress": { "postalCode": "01310-000", "countryCode": "BR" },
            }),
            expect: stored_with(DERIVED_MEMBER_UNKNOWN, "BR"),
        },
        // The country is matched the way the member's own pattern admits it
        // (`^[a-zA-Z]+$`), so either case identifies the same jurisdiction.
        Case {
            name: "Brazilian entry whose country code is lower case and carries no legal name",
            entry: json!({
                "registrationId": "11222333000181",
                "registrationType": "VAT",
                "legalAddress": { "postalCode": "01310-000", "countryCode": "br" },
            }),
            expect: stored_with(DERIVED_MEMBER_UNKNOWN, "br"),
        },
        // Brazil, CPF: the legal name is carved out, the address is not.
        Case {
            name: "Brazilian CPF entry carrying an address but no legal name",
            entry: json!({
                "registrationId": "12345678909",
                "registrationType": "CPF",
                "legalAddress": {
                    "postalCode": "22041-001",
                    "stateOrRegion": "Rio de Janeiro",
                    "countryCode": "BR",
                },
            }),
            expect: stored_with(DERIVED_MEMBER_UNKNOWN, "BR"),
        },
        Case {
            name: "Brazilian CPF entry carrying no address",
            entry: json!({
                "registrationId": "12345678909",
                "registrationType": "CPF",
                "legalName": "Maria Exemplo",
            }),
            expect: missing("LEGAL_ADDRESS"),
        },
        // Mexico: both members are carved out, and the country's own documented
        // requirement (a CSF document) still applies.
        Case {
            name: "Mexican entry carrying its CSF document but no legal name",
            entry: json!({
                "registrationId": "EXA800101AB1",
                "registrationType": "VAT",
                "legalAddress": { "postalCode": "01000", "countryCode": "MX" },
                "verificationDetails": certificate_document(),
            }),
            expect: stored_with(DERIVED_MEMBER_UNKNOWN, "MX"),
        },
        Case {
            name: "Mexican entry carrying neither a legal name nor its CSF document",
            entry: json!({
                "registrationId": "EXA800101AB1",
                "registrationType": "VAT",
                "legalAddress": { "postalCode": "01000", "countryCode": "MX" },
            }),
            expect: missing("TAX_REGISTRATION_DOCUMENTS"),
        },
        // India's PAN: the address is the service's to fill in, so an entry
        // carrying none is stored and one submitting an address is refused.
        Case {
            name: "India PAN entry carrying a legal name and no address",
            entry: json!({
                "registrationId": "ABCDE1234F",
                "registrationType": "PAN",
                "legalName": "Acme India Pvt Ltd",
            }),
            expect: stored_with_filled_in_address("Acme India Pvt Ltd", "IN"),
        },
        Case {
            name: "India PAN entry submitting an address",
            entry: json!({
                "registrationId": "ABCDE1234F",
                "registrationType": "PAN",
                "legalName": "Acme India Pvt Ltd",
                "legalAddress": { "postalCode": "560025", "countryCode": "IN" },
            }),
            expect: Expect::Refused {
                error_code: ERROR_CODE_FIELD_VALIDATION_FAILED,
                parameter: "LEGAL_ADDRESS",
            },
        },
        Case {
            name: "India PAN entry carrying no legal name",
            entry: json!({
                "registrationId": "ABCDE1234F",
                "registrationType": "PAN",
            }),
            expect: missing("LEGAL_NAME"),
        },
        // A jurisdiction in neither carve-out requires both members.
        Case {
            name: "German VAT entry carrying no legal name",
            entry: json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
            expect: missing("LEGAL_NAME"),
        },
        Case {
            name: "Malaysian NRIC entry carrying no address",
            entry: json!({
                "registrationId": "800101011234",
                "registrationType": "NRIC",
                "legalName": "Siti Contoh",
            }),
            expect: missing("LEGAL_ADDRESS"),
        },
        // An entry whose type the model ties to no one country, carrying no
        // address, identifies no jurisdiction: there is no country to read
        // either carve-out from, nor one to report the registration as held in.
        Case {
            name: "VAT entry carrying neither a legal address nor a country-specific type",
            entry: json!({
                "registrationId": "VAT-0000000001",
                "registrationType": "VAT",
                "legalName": "Acme Holdings",
            }),
            expect: missing("LEGAL_ADDRESS"),
        },
        Case {
            name: "GST entry carrying neither member",
            entry: json!({
                "registrationId": "GST-0000000001",
                "registrationType": "GST",
            }),
            expect: missing("LEGAL_ADDRESS"),
        },
    ]
}

/// RULE: a write requires `legalName` or `legalAddress` only where the
/// registration's jurisdiction does; where the service populates the member from
/// the tax registration number, the entry is stored and reads back with it
/// filled in.
#[tokio::test]
async fn an_entry_is_required_to_carry_only_what_its_jurisdiction_requires() {
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        let answer = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID, "taxRegistrationEntry": case.entry }),
        )
        .await;
        let name = case.name;
        match case.expect {
            Expect::Stored {
                legal_name,
                country_code,
                address_filled_in,
            } => {
                assert_eq!(
                    answer.status, 200,
                    "case {name:?}: expected the entry to be stored, got {answer:?}"
                );
                assert_eq!(
                    answer.get("status").as_str(),
                    Some(STATUS_PENDING),
                    "case {name:?}: expected the stored status reported, got {:?}",
                    answer.body
                );
                // Hold the validation the write started open, so the read
                // reports the record as the write stored it.
                hold_validations_open(&engine);
                let got = call(
                    &router,
                    "GetTaxRegistration",
                    &json!({ "accountId": ACCOUNT_ID }),
                )
                .await;
                let registration = got.get("taxRegistration").clone();
                assert_eq!(
                    registration.get("legalName").and_then(Value::as_str),
                    Some(legal_name),
                    "case {name:?}: expected the reported legal name {legal_name:?}, got {:?}",
                    got.body
                );
                let address = registration.get("legalAddress").cloned().unwrap_or_default();
                assert_eq!(
                    address.get("countryCode").and_then(Value::as_str),
                    Some(country_code),
                    "case {name:?}: expected the record held in {country_code:?}, got {:?}",
                    got.body
                );
                if address_filled_in {
                    assert_eq!(
                        address,
                        filled_in_address(country_code),
                        "case {name:?}: expected the address the service fills in, got {:?}",
                        got.body
                    );
                }
            }
            Expect::Refused {
                error_code,
                parameter,
            } => {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {name:?}: expected 400 ValidationException, got {answer:?}"
                );
                assert_eq!(
                    answer.get("errorCode").as_str(),
                    Some(error_code),
                    "case {name:?}: expected the {error_code} code, got {:?}",
                    answer.body
                );
                assert_eq!(
                    field_list(&answer),
                    vec![parameter.to_string()],
                    "case {name:?}: expected the refusal to name {parameter}, got {:?}",
                    answer.body
                );
                let read = call(
                    &router,
                    "GetTaxRegistration",
                    &json!({ "accountId": ACCOUNT_ID }),
                )
                .await;
                assert_eq!(
                    read.status, 404,
                    "case {name:?}: a refused write must store nothing, got {read:?}"
                );
            }
        }
    }
}

/// RULE: the entry judge is shared, so `BatchPutTaxRegistration` admits and
/// refuses exactly the entries `PutTaxRegistration` does, for every account it
/// names.
#[tokio::test]
async fn the_batch_writer_admits_the_same_entries() {
    let accounts = ["000000000001", "000000000002", "000000000003"];
    for case in cases() {
        let (router, engine) = emulator_with_engine();
        let answer = call(
            &router,
            "BatchPutTaxRegistration",
            &json!({ "accountIds": accounts, "taxRegistrationEntry": case.entry }),
        )
        .await;
        let name = case.name;
        match case.expect {
            Expect::Stored {
                legal_name,
                country_code,
                ..
            } => {
                assert_eq!(
                    answer.status, 200,
                    "case {name:?}: expected the batch to store the entry, got {answer:?}"
                );
                hold_validations_open(&engine);
                for account_id in accounts {
                    let got = call(
                        &router,
                        "GetTaxRegistration",
                        &json!({ "accountId": account_id }),
                    )
                    .await;
                    let registration = got.get("taxRegistration").clone();
                    assert_eq!(
                        registration.get("legalName").and_then(Value::as_str),
                        Some(legal_name),
                        "case {name:?}: expected account {account_id} to report \
                         {legal_name:?}, got {:?}",
                        got.body
                    );
                    assert_eq!(
                        registration
                            .get("legalAddress")
                            .and_then(|address| address.get("countryCode"))
                            .and_then(Value::as_str),
                        Some(country_code),
                        "case {name:?}: expected account {account_id} held in \
                         {country_code:?}, got {:?}",
                        got.body
                    );
                }
            }
            Expect::Refused { parameter, .. } => {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (400, "ValidationException"),
                    "case {name:?}: expected the batch refused, got {answer:?}"
                );
                assert_eq!(
                    field_list(&answer),
                    vec![parameter.to_string()],
                    "case {name:?}: expected the refusal to name {parameter}, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE: the refusal of an entry carrying no legal address states which of the
/// two things is missing — the member, where the registration's own jurisdiction
/// requires it, or the jurisdiction, where nothing in the entry names one.
#[tokio::test]
async fn the_refusal_states_whether_the_member_or_the_jurisdiction_is_missing() {
    let rows: [(&str, Value, &str); 4] = [
        (
            "Brazilian CPF entry, a jurisdiction that requires the address",
            json!({
                "registrationId": "12345678909",
                "registrationType": "CPF",
                "legalName": "Maria Exemplo",
            }),
            MISSING_LEGAL_ADDRESS,
        ),
        (
            "Malaysian NRIC entry, a jurisdiction that requires the address",
            json!({
                "registrationId": "800101011234",
                "registrationType": "NRIC",
                "legalName": "Siti Contoh",
            }),
            MISSING_LEGAL_ADDRESS,
        ),
        (
            "VAT entry that names no jurisdiction at all",
            json!({
                "registrationId": "VAT-0000000001",
                "registrationType": "VAT",
                "legalName": "Acme Holdings",
            }),
            MISSING_LEGAL_ADDRESS_JURISDICTION,
        ),
        (
            "TIN entry that names no jurisdiction at all",
            json!({
                "registrationId": "TIN-0000000001",
                "registrationType": "TIN",
                "legalName": "Acme Holdings",
            }),
            MISSING_LEGAL_ADDRESS_JURISDICTION,
        ),
    ];
    assert_ne!(
        MISSING_LEGAL_ADDRESS, MISSING_LEGAL_ADDRESS_JURISDICTION,
        "the two refusals must not state the same thing, got {MISSING_LEGAL_ADDRESS:?}"
    );
    for (case, entry, message) in rows {
        let (router, _engine) = emulator_with_engine();
        let answer = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.get("message").as_str(),
            Some(message),
            "case {case:?}: expected the refusal {message:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            field_list(&answer),
            vec!["LEGAL_ADDRESS".to_string()],
            "case {case:?}: expected the refusal to name LEGAL_ADDRESS, got {:?}",
            answer.body
        );
    }
}

/// RULE: a registration whose address the service filled in is held in a real
/// jurisdiction, so every reader reports that country — the listing's
/// `jurisdiction` as much as the single read's `legalAddress`.
#[tokio::test]
async fn a_filled_in_address_is_reported_as_the_registration_s_jurisdiction() {
    // One row per registration type an entry may omit the address for, with the
    // country the model ties that type to.
    for (registration_type, country_code) in [("CNPJ", "BR"), ("PAN", "IN")] {
        let case = format!("{registration_type} with no legal address");
        let (router, engine) = emulator_with_engine();
        let answer = call(
            &router,
            "PutTaxRegistration",
            &json!({
                "accountId": ACCOUNT_ID,
                "taxRegistrationEntry": {
                    "registrationId": crate::fidelity_rules::number_of_type(
                        registration_type,
                        Some(country_code),
                    ),
                    "registrationType": registration_type,
                    "legalName": "Example Holdings",
                },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the entry to be stored, got {answer:?}"
        );
        hold_validations_open(&engine);
        let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
        let accounts = listed
            .get("accountDetails")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            accounts.len(),
            1,
            "case {case:?}: expected the stored registration listed once, got {:?}",
            listed.body
        );
        let jurisdiction = accounts[0]
            .get("taxRegistration")
            .and_then(|registration| registration.get("jurisdiction"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            jurisdiction.get("countryCode").and_then(Value::as_str),
            Some(country_code),
            "case {case:?}: expected the listing to report the jurisdiction \
             {country_code:?}, got {:?}",
            listed.body
        );
    }
}
