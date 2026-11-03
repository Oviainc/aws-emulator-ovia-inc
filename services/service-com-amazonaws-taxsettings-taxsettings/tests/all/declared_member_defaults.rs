// SPDX-License-Identifier: CC0-1.0

//! The defaults the model declares for a structure member, end to end over the
//! restJson1 wire.
//!
//! ONE rule: a member the model declares a `@default` for takes that declared
//! value when the request omits it, so the record the write stores — and
//! therefore every read of it — carries the declared value instead of leaving the
//! member absent. A member that declares no default is untouched: it is reported
//! only where it was submitted.
//!
//! The two shapes of this service whose members declare one are `Address`
//! (`addressLine1` and `city`, both `"Unknown"`) and `MalaysiaAdditionalInfo`
//! (`serviceTaxCodes`, the empty list); each declares it with `@addedDefault`,
//! which keeps the member nullable on the wire, so a request may omit it and the
//! service substitutes the declared value.
//!
//! The tables span the class the rule generalises over rather than one request:
//! which of the defaulted members a request carries and which it omits, the
//! members that declare no default beside them, several jurisdictions, and all
//! three write paths that take an address (`PutTaxRegistration`,
//! `BatchPutTaxRegistration`, `PutSupplementalTaxRegistration`) read back through
//! each reader that reports one.
//!
//! The harness (`emulator`, `emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::ADDRESS_MEMBER_DEFAULT;

use crate::supplemental_tax_registrations::{list_all, put as put_supplemental};
use crate::tax_registration_lifecycle::{call, emulator};

/// The `Address` members the model declares a default for
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#Address")`:
/// `addressLine1` and `city` each carry
/// `{"default": "Unknown", "addedDefault": true}`).
const ADDRESS_MEMBERS_WITH_A_DEFAULT: &[&str] = &["addressLine1", "city"];

/// The optional `Address` members the same query shows declaring NO default, so
/// an address that omits one is reported without it.
const ADDRESS_MEMBERS_WITHOUT_A_DEFAULT: &[&str] = &[
    "addressLine2",
    "addressLine3",
    "districtOrCounty",
    "stateOrRegion",
];

/// The `additionalTaxInformation` country block with a member that declares a
/// default, and that member: `MalaysiaAdditionalInfo$serviceTaxCodes` carries
/// `{"default": [], "addedDefault": true}`
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#MalaysiaAdditionalInfo")`).
/// No other block of `AdditionalInfoRequest` has one.
const BLOCK_WITH_A_DEFAULT: (&str, &str) = ("malaysiaAdditionalInfo", "serviceTaxCodes");

/// An `Address`, as a read of the stored record reports it: the members the
/// request submitted, plus the declared default of each defaulted member it
/// omitted.
///
/// The one reader for every suite that round-trips an address, so no two suites
/// can read this rule differently.
pub(crate) fn address_as_stored(submitted: &Value) -> Value {
    let mut address = submitted.clone();
    let members = address
        .as_object_mut()
        .expect("an address is a JSON object");
    for member in ADDRESS_MEMBERS_WITH_A_DEFAULT {
        members
            .entry(*member)
            .or_insert_with(|| json!(ADDRESS_MEMBER_DEFAULT));
    }
    address
}

/// One `additionalTaxInformation` country block, as a read of the stored record
/// reports it: the members the request submitted, plus the declared default of
/// the one defaulted member, where that block has it and the request omitted it.
pub(crate) fn country_block_as_stored(country: &str, submitted: &Value) -> Value {
    let (block_with_a_default, defaulted_member) = BLOCK_WITH_A_DEFAULT;
    if country != block_with_a_default {
        return submitted.clone();
    }
    let mut block = submitted.clone();
    block
        .as_object_mut()
        .expect("a country block is a JSON object")
        .entry(defaulted_member)
        .or_insert_with(|| json!([]));
    block
}

/// The addresses the address table submits: they differ in which defaulted
/// member is carried, in which members that declare no default accompany it, and
/// in the jurisdiction the address is held in.
///
/// Every country here is one the service's per-country rules state no extra
/// requirement for, so what decides each answer is the default rule alone. None
/// of them carries `addressLine3`, the one member the API accepts only for a
/// named jurisdiction (`crate::address_member_jurisdictions` covers that rule);
/// it declares no default either way.
fn address_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the two members the model requires",
            json!({ "postalCode": "10115", "countryCode": "DE" }),
        ),
        (
            "addressLine1 submitted, city omitted",
            json!({
                "addressLine1": "ul. Przykladowa 1",
                "postalCode": "00-001",
                "countryCode": "PL",
            }),
        ),
        (
            "city submitted, addressLine1 omitted",
            json!({ "city": "Milano", "postalCode": "20121", "countryCode": "IT" }),
        ),
        (
            "both defaulted members submitted",
            json!({
                "addressLine1": "1 Example Street",
                "city": "Stockholm",
                "postalCode": "11152",
                "countryCode": "SE",
            }),
        ),
        (
            "only members that declare no default submitted",
            json!({
                "addressLine2": "Floor 4",
                "districtOrCounty": "Shibuya",
                "stateOrRegion": "Tokyo",
                "postalCode": "1500001",
                "countryCode": "JP",
            }),
        ),
        (
            "every member these jurisdictions accept submitted",
            json!({
                "addressLine1": "1 Example Street",
                "addressLine2": "Floor 4",
                "districtOrCounty": "Centrum",
                "city": "Amsterdam",
                "stateOrRegion": "Noord-Holland",
                "postalCode": "1011AB",
                "countryCode": "NL",
            }),
        ),
        (
            "a defaulted member carrying the declared value itself",
            json!({
                "addressLine1": ADDRESS_MEMBER_DEFAULT,
                "postalCode": "1050",
                "countryCode": "BE",
            }),
        ),
    ]
}

/// A `TaxRegistrationEntry` carrying `address` as its legal address.
fn entry_with(address: &Value) -> Value {
    json!({
        "registrationId": "DE136695976",
        "registrationType": "VAT",
        "legalName": "Acme Holdings",
        "legalAddress": address,
    })
}

/// Assert `reported` is `submitted` with the declared defaults materialised, and
/// that no member which declares none was invented.
fn assert_address_as_stored(case: &str, where_read: &str, submitted: &Value, reported: &Value) {
    assert_eq!(
        reported,
        &address_as_stored(submitted),
        "case {case:?}: expected {where_read} to report the submitted address with the \
         model's declared defaults materialised, got {reported:?}"
    );
    for member in ADDRESS_MEMBERS_WITH_A_DEFAULT {
        let expected = submitted
            .get(member)
            .cloned()
            .unwrap_or_else(|| json!(ADDRESS_MEMBER_DEFAULT));
        assert_eq!(
            reported.get(member),
            Some(&expected),
            "case {case:?}: expected {where_read} to report {member} as {expected:?}, got {:?}",
            reported.get(member)
        );
    }
    for member in ADDRESS_MEMBERS_WITHOUT_A_DEFAULT {
        assert_eq!(
            reported.get(member),
            submitted.get(member),
            "case {case:?}: {member} declares no default, so {where_read} must report it \
             exactly as submitted ({:?}), got {:?}",
            submitted.get(member),
            reported.get(member)
        );
    }
}

/// RULE: an omitted `Address` member that declares a default is stored with that
/// declared value, so `GetTaxRegistration` and the `accountMetaData` of
/// `ListTaxRegistrations` both report it; a member that declares none stays
/// absent.
#[tokio::test]
async fn an_omitted_address_member_reads_back_with_the_default_the_model_declares() {
    // The value under test is the one the model declares, not a value this
    // emulator chose.
    assert_eq!(
        ADDRESS_MEMBER_DEFAULT, "Unknown",
        "addressLine1 and city declare \"Unknown\" as their default, got \
         {ADDRESS_MEMBER_DEFAULT:?}"
    );

    const ACCOUNT_ID: &str = "000000000001";
    for (case, address) in address_rows() {
        let router = emulator();
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID, "taxRegistrationEntry": entry_with(&address) }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the write must be accepted, got {put:?}"
        );

        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID }),
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: the account must hold a registration, got {got:?}"
        );
        let reported = got
            .body
            .pointer("/taxRegistration/legalAddress")
            .cloned()
            .unwrap_or(Value::Null);
        assert_address_as_stored(case, "GetTaxRegistration", &address, &reported);

        let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must answer, got {listed:?}"
        );
        let listed_address = listed
            .body
            .pointer("/accountDetails/0/accountMetaData/address")
            .cloned()
            .unwrap_or(Value::Null);
        assert_address_as_stored(
            case,
            "the listing's accountMetaData",
            &address,
            &listed_address,
        );
    }
}

/// RULE: the same defaults, on the other two write paths that take an address —
/// the per-account fan-out of the registration write, and the supplemental
/// registration write whose address `ListSupplementalTaxRegistrations` reports.
#[tokio::test]
async fn every_write_path_that_takes_an_address_materialises_the_same_defaults() {
    let accounts = ["000000000001", "000000000002"];
    for (case, address) in address_rows() {
        let router = emulator();
        let batch = call(
            &router,
            "BatchPutTaxRegistration",
            &json!({ "accountIds": accounts, "taxRegistrationEntry": entry_with(&address) }),
        )
        .await;
        assert_eq!(
            batch.status, 200,
            "case {case:?}: the batch write must be accepted, got {batch:?}"
        );
        for account_id in accounts {
            let got = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            let reported = got
                .body
                .pointer("/taxRegistration/legalAddress")
                .cloned()
                .unwrap_or(Value::Null);
            assert_address_as_stored(
                case,
                &format!("account {account_id} after the batch write"),
                &address,
                &reported,
            );
        }
    }

    // A supplemental TRN is held in one of the jurisdictions this service
    // supports supplemental registration in, so the rows above are replayed with
    // that country code: what varies is still which defaulted member the address
    // carries.
    for (case, address) in address_rows() {
        let (router, _engine) = crate::supplemental_tax_registrations::emulator_supplementing();
        let mut address = address;
        address
            .as_object_mut()
            .expect("an address is a JSON object")
            .insert("countryCode".to_string(), json!("CH"));
        let stored = put_supplemental(
            &router,
            &json!({
                "registrationId": "CHE-123.456.789",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": address,
            }),
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the supplemental write must be accepted, got {stored:?}"
        );
        let listed = list_all(&router, None).await;
        let reported = listed
            .first()
            .and_then(|record| record.get("address"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_address_as_stored(
            case,
            "ListSupplementalTaxRegistrations",
            &address,
            &reported,
        );
    }
}

/// The country blocks the table submits: Malaysia's with the defaulted member
/// omitted, with one value and with several, and a block of another country —
/// whose members declare no default — beside them.
///
/// Each entry satisfies its country's own documented requirements
/// (`crate::country_requirements` covers those), so what decides the answer here
/// is the default rule alone.
fn country_block_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a Malaysian individual whose block omits serviceTaxCodes",
            json!({
                "registrationId": "880101011234",
                "registrationType": "NRIC",
                "legalName": "Siti Example",
                "sector": "Individual",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": { "taxInformationNumber": "C1234567890" },
                },
            }),
        ),
        (
            "a Malaysian business submitting one service tax code",
            json!({
                "registrationId": "W10-1808-32000055",
                "registrationType": "SST",
                "legalName": "Example Services Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": {
                        "serviceTaxCodes": ["IT Services"],
                        "businessRegistrationNumber": "202001001234",
                        "taxInformationNumber": "C1234567890",
                    },
                },
            }),
        ),
        (
            "a Malaysian business submitting two service tax codes",
            json!({
                "registrationId": "W10-1808-32000056",
                "registrationType": "SST",
                "legalName": "Example Advisory Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": {
                        "serviceTaxCodes": ["Consultancy", "Training Or Coaching"],
                        "businessRegistrationNumber": "202001001235",
                        "taxInformationNumber": "C1234567891",
                    },
                },
            }),
        ),
        (
            "a country block none of whose members declares a default",
            json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "sector": "Business",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Intra-EU" },
                },
            }),
        ),
    ]
}

/// RULE: the same rule on the other shape that declares a default — a stored
/// registration reports `serviceTaxCodes` as the declared empty list where the
/// submitted Malaysian block omitted it, and reports every other country block
/// exactly as submitted.
#[tokio::test]
async fn an_omitted_country_block_member_reads_back_with_the_default_the_model_declares() {
    const ACCOUNT_ID: &str = "000000000001";
    let (block_with_a_default, defaulted_member) = BLOCK_WITH_A_DEFAULT;
    for (case, entry) in country_block_rows() {
        let router = emulator();
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the write must be accepted, got {put:?}"
        );

        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": ACCOUNT_ID }),
        )
        .await;
        let reported = got
            .body
            .pointer("/taxRegistration/additionalTaxInformation")
            .cloned()
            .unwrap_or(Value::Null);
        let submitted = entry
            .get("additionalTaxInformation")
            .and_then(Value::as_object)
            .expect("every row submits a country block");
        for (country, block) in submitted {
            assert_eq!(
                reported.get(country),
                Some(&country_block_as_stored(country, block)),
                "case {case:?}: expected {country} reported with the model's declared \
                 defaults materialised, got {:?}",
                reported.get(country)
            );
        }
        if let Some(block) = submitted.get(block_with_a_default) {
            let expected = block
                .get(defaulted_member)
                .cloned()
                .unwrap_or_else(|| json!([]));
            assert_eq!(
                reported.pointer(&format!("/{block_with_a_default}/{defaulted_member}")),
                Some(&expected),
                "case {case:?}: expected {defaulted_member} reported as {expected:?}, got {:?}",
                reported.get(block_with_a_default)
            );
        }
    }
}
