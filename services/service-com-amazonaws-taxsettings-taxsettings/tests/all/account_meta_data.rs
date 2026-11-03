// SPDX-License-Identifier: CC0-1.0

//! `ListTaxRegistrations` reports the account's legal profile, not the TRN alone.
//!
//! RULE: every member of a reported shape that the store holds a value for is
//! projected from that stored value, so each `accountDetails` entry carries the
//! `accountMetaData` legal profile of the account — its name, its address in
//! full, and the role that address plays — beside the TRN.
//!
//! The table below spans the class the rule generalises over rather than one
//! recorded request: addresses with and without a state or region, with and
//! without the optional lines and the district, in several countries, under
//! names of several forms. Every case is driven over the restJson1 wire through
//! the same `MultiServiceRouter::dispatch` the router serves, and each
//! assertion compares the listing against what `GetTaxRegistration` reports for
//! the same account, so nothing is pinned to a literal this test invented.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::ops::types::ADDRESS_ROLE_TYPE_VALUES;
use service_com_amazonaws_taxsettings_taxsettings::state::LEGAL_ADDRESS_ROLE;

use crate::tax_registration_lifecycle::{call, emulator, Answer};

/// One account's registration: the account to store it under, and the entry.
struct MetaCase {
    name: &'static str,
    account_id: &'static str,
    entry: Value,
}

/// The entries this suite stores.
///
/// The rows differ along what the rule generalises over: the shape of the legal
/// address (minimal — only the two `@required` members `postalCode` and
/// `countryCode` — through one carrying every optional member the model lists),
/// whether it has a `stateOrRegion` (so the jurisdiction the metadata maps the
/// address under has one or does not), the country, and the form of the legal
/// name (ASCII, accented, non-Latin script, punctuation).
fn meta_cases() -> Vec<MetaCase> {
    vec![
        MetaCase {
            name: "minimal address, no state or region",
            account_id: "000000000011",
            entry: json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
        },
        MetaCase {
            name: "address with a state or region",
            account_id: "000000000012",
            entry: json!({
                "registrationId": "29ABCDE1234F1Z5",
                "registrationType": "GST",
                "legalName": "Acme India Private Limited",
                "sector": "Business",
                "legalAddress": {
                    "addressLine1": "1 Residency Road",
                    "city": "Bengaluru",
                    "stateOrRegion": "Karnataka",
                    "postalCode": "560025",
                    "countryCode": "IN",
                },
            }),
        },
        MetaCase {
            name: "address carrying a district and a second line",
            account_id: "000000000013",
            entry: json!({
                "registrationId": "12.345.678/0001-95",
                "registrationType": "CNPJ",
                "legalName": "Município de Exemplo",
                "sector": "Government",
                "legalAddress": {
                    "addressLine1": "Rua Exemplo 100",
                    "addressLine2": "Sala 7",
                    "districtOrCounty": "Centro",
                    "city": "Sao Paulo",
                    "stateOrRegion": "SP",
                    "postalCode": "01001-000",
                    "countryCode": "BR",
                },
            }),
        },
        MetaCase {
            name: "address carrying every optional member",
            account_id: "000000000014",
            entry: json!({
                "registrationId": "310123456700003",
                "registrationType": "TIN",
                "legalName": "مثال للتجارة",
                "sector": "Business",
                "legalAddress": {
                    "addressLine1": "King Fahd Road",
                    "addressLine2": "Floor 3",
                    // Saudi Arabia's documented rule: `addressLine3` carries the
                    // building number (`crate::state::COUNTRY_RULES`).
                    "addressLine3": "1234",
                    "districtOrCounty": "Al Olaya",
                    "city": "Riyadh",
                    "stateOrRegion": "Riyadh",
                    "postalCode": "12212",
                    "countryCode": "SA",
                },
            }),
        },
        MetaCase {
            name: "individual with a punctuated legal name",
            account_id: "000000000015",
            entry: json!({
                "registrationId": "123.456.789-09",
                "registrationType": "CPF",
                "legalName": "Maria O'Hara-Exemplo, Jr.",
                "sector": "Individual",
                "legalAddress": {
                    "stateOrRegion": "Rio de Janeiro",
                    "postalCode": "22041-001",
                    "countryCode": "BR",
                },
                "verificationDetails": { "dateOfBirth": "1980-07-15" },
            }),
        },
        MetaCase {
            name: "business in a country whose address needs no state",
            account_id: "000000000016",
            entry: json!({
                "registrationId": "1234563218",
                "registrationType": "NIP",
                "legalName": "Przykład Sp. z o.o.",
                "sector": "Business",
                "legalAddress": {
                    "addressLine1": "ul. Przykładowa 1",
                    "city": "Warszawa",
                    "postalCode": "00-001",
                    "countryCode": "PL",
                },
            }),
        },
    ]
}

/// Store every case's registration, failing the test if a setup write is refused.
async fn store_every_case(
    router: &smithy_server_runtime::MultiServiceRouter,
    cases: &[MetaCase],
) {
    for case in cases {
        let put = call(
            router,
            "PutTaxRegistration",
            &json!({ "accountId": case.account_id, "taxRegistrationEntry": case.entry }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: setup PutTaxRegistration expected 200, got {} {:?}",
            case.name, put.status, put.body
        );
    }
}

/// The listing entry for one account, from a 200 answer.
fn entry_for<'a>(listed: &'a Answer, account_id: &str) -> &'a Value {
    listed
        .get("accountDetails")
        .as_array()
        .and_then(|details| {
            details
                .iter()
                .find(|detail| detail.get("accountId").and_then(Value::as_str) == Some(account_id))
        })
        .unwrap_or_else(|| panic!("account {account_id} missing from the listing: {listed:?}"))
}

/// Assert the `accountMetaData` of one listing entry against the record
/// `GetTaxRegistration` reports for the same account.
fn assert_meta_data(case_name: &str, detail: &Value, registration: &Value) {
    let meta = detail
        .get("accountMetaData")
        .unwrap_or_else(|| panic!("case {case_name:?}: accountMetaData is absent: {detail:?}"));

    // The name the service holds for the account is the legal name the
    // registration was stored with.
    assert_eq!(
        meta.get("accountName"),
        registration.get("legalName"),
        "case {case_name:?}: expected accountName to be {:?}, got {:?}",
        registration.get("legalName"),
        meta.get("accountName")
    );

    // The metadata carries the address in FULL — the listing's own
    // `taxRegistration` reduces it to a jurisdiction, so this is the only place
    // a listing caller reads it.
    assert_eq!(
        meta.get("address"),
        registration.get("legalAddress"),
        "case {case_name:?}: expected the stored legal address {:?}, got {:?}",
        registration.get("legalAddress"),
        meta.get("address")
    );

    // The role that address plays, and the same role as the key of the role map.
    let address_type = meta
        .get("addressType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        ADDRESS_ROLE_TYPE_VALUES.contains(&address_type),
        "case {case_name:?}: expected addressType to be one of {ADDRESS_ROLE_TYPE_VALUES:?}, \
         got {address_type:?}"
    );
    assert_eq!(
        address_type, LEGAL_ADDRESS_ROLE,
        "case {case_name:?}: the address stored with a TRN is the account's tax address, \
         got {address_type:?}"
    );

    let role_map = meta
        .get("addressRoleMap")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case_name:?}: addressRoleMap is absent: {meta:?}"));
    for role in role_map.keys() {
        assert!(
            ADDRESS_ROLE_TYPE_VALUES.contains(&role.as_str()),
            "case {case_name:?}: addressRoleMap key {role:?} is not an AddressRoleType value \
             ({ADDRESS_ROLE_TYPE_VALUES:?})"
        );
    }
    let mapped = role_map.get(address_type).unwrap_or_else(|| {
        panic!("case {case_name:?}: addressRoleMap has no entry for {address_type:?}: {meta:?}")
    });

    // The jurisdiction filed under that role is the jurisdiction of the stored
    // address: the same projection the TRN beside it is reported through, so the
    // two cannot disagree.
    assert_eq!(
        mapped.get("countryCode"),
        registration.pointer("/legalAddress/countryCode"),
        "case {case_name:?}: expected the mapped country code {:?}, got {:?}",
        registration.pointer("/legalAddress/countryCode"),
        mapped.get("countryCode")
    );
    assert_eq!(
        mapped.get("stateOrRegion"),
        registration.pointer("/legalAddress/stateOrRegion"),
        "case {case_name:?}: expected the mapped state or region {:?}, got {:?}",
        registration.pointer("/legalAddress/stateOrRegion"),
        mapped.get("stateOrRegion")
    );
    assert_eq!(
        mapped,
        detail
            .pointer("/taxRegistration/jurisdiction")
            .unwrap_or(&Value::Null),
        "case {case_name:?}: the mapped jurisdiction must be the one reported beside the TRN, \
         got {mapped:?} and {:?}",
        detail.pointer("/taxRegistration/jurisdiction")
    );
}

/// RULE: each listed account carries the `accountMetaData` legal profile the
/// service holds for it — the account's name, its address in full and the role
/// that address plays — projected from the stored registration.
#[tokio::test]
async fn listed_accounts_carry_the_stored_legal_profile() {
    let router = emulator();
    let cases = meta_cases();
    store_every_case(&router, &cases).await;

    let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "ListTaxRegistrations expected 200, got {} {:?}",
        listed.status, listed.body
    );

    for case in &cases {
        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": case.account_id }),
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {:?}: GetTaxRegistration expected 200, got {} {:?}",
            case.name, got.status, got.body
        );
        let registration = got.get("taxRegistration").clone();
        // The profile is projected from the STORE, so it reports what was
        // persisted rather than what this request happened to send.
        assert_eq!(
            registration.get("legalName"),
            case.entry.get("legalName"),
            "case {:?}: setup expected the submitted legal name {:?}, got {:?}",
            case.name,
            case.entry.get("legalName"),
            registration.get("legalName")
        );
        assert_meta_data(case.name, entry_for(&listed, case.account_id), &registration);
    }
}

/// RULE (same rule, across pages): the profile travels with the account, so
/// every page of a paginated listing reports it, not just the first.
#[tokio::test]
async fn every_page_reports_the_profile() {
    let router = emulator();
    let cases = meta_cases();
    store_every_case(&router, &cases).await;

    // One account per page, so each case is reported on a page of its own and
    // the walk covers every page the store has.
    let mut next_token: Option<String> = None;
    let mut seen = 0usize;
    for _ in 0..=cases.len() {
        let request = match &next_token {
            Some(token) => json!({ "maxResults": 1, "nextToken": token }),
            None => json!({ "maxResults": 1 }),
        };
        let listed = call(&router, "ListTaxRegistrations", &request).await;
        assert_eq!(
            listed.status, 200,
            "page {seen}: ListTaxRegistrations expected 200, got {} {:?}",
            listed.status, listed.body
        );
        let details = listed
            .get("accountDetails")
            .as_array()
            .cloned()
            .unwrap_or_default();
        for detail in &details {
            let account_id = detail
                .get("accountId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let got = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            assert_eq!(
                got.status, 200,
                "page {seen}: GetTaxRegistration({account_id}) expected 200, got {} {:?}",
                got.status, got.body
            );
            assert_meta_data(
                &format!("paged account {account_id}"),
                detail,
                &got.get("taxRegistration").clone(),
            );
            seen += 1;
        }
        next_token = listed
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        cases.len(),
        "expected every stored account to be reported across the pages ({}), got {seen}",
        cases.len()
    );
}
