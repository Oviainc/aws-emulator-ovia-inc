// SPDX-License-Identifier: CC0-1.0

//! The `sector`/`registrationType` PAIRING a country's documentation states, over
//! the wire.
//!
//! ONE rule: where the operation's documentation pairs each `registrationType`
//! value with the `sector` it belongs to, an entry whose type is not one the
//! declared sector admits is refused with `ValidationException` (HTTP 400) /
//! `FieldValidationFailed` naming `REGISTRATION_TYPE`, and nothing is persisted —
//! while an entry whose type the declared sector admits is stored and reads back.
//!
//! The table spans the class that rule generalises over: every `Sector` value the
//! model lists (`Business | Individual | Government`,
//! `smithy_query(query="shape",shape="com.amazonaws.taxsettings#Sector")`) and an
//! entry declaring none, crossed with every registration type the pairing country
//! documents plus one outside its set, in either spelling of the country code —
//! and a country whose documentation pairs the two members with nothing, which
//! the rule must leave alone. The reported Business/NRIC entry is ONE row.
//!
//! Both writers of a `TaxRegistrationEntry` are driven for every row, because the
//! pairing is a rule of the shared judge rather than of one operation.
//!
//! The source of the pairing is
//! `smithy_query(query="doc",shape="com.amazonaws.taxsettings#PutTaxRegistration")`:
//! "`RegistrationType` valid values are `NRIC` for individual, and TIN and sales
//! and service tax (SST) for Business", beside "The sector valid values are
//! `Business` and `Individual`."
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs` and
//! the two-writer driver in `country_rules.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    parameter_name, sector_registration_types, ERROR_CODE_FIELD_VALIDATION_FAILED, STATUS_PENDING,
};

use crate::country_rules::{write, WRITERS};
use crate::tax_registration_lifecycle::{call, emulator, field_list, Answer};

/// Every value of the model's `Sector` enum
/// (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#Sector")`),
/// plus `None` for an entry that declares no sector at all.
const SECTORS: &[Option<&str>] = &[
    None,
    Some("Business"),
    Some("Individual"),
    Some("Government"),
];

/// The registration type the Malaysian documentation pairs with the individual
/// sector: "`RegistrationType` valid values are `NRIC` for individual".
const DOCUMENTED_INDIVIDUAL_TYPES: &[&str] = &["NRIC"];

/// The registration types the same sentence pairs with the business sector: "and
/// TIN and sales and service tax (SST) for Business".
const DOCUMENTED_BUSINESS_TYPES: &[&str] = &["TIN", "SST"];

/// A registration type that sentence pairs with NEITHER sector, and which the
/// country's value set therefore does not admit at all. `VAT` is a value of the
/// model's `TaxRegistrationType` enum, so the boundary validator admits it and the
/// country's own rules are what decide the answer.
const TYPE_OUTSIDE_THE_COUNTRY_SET: &str = "VAT";

/// The `sector` value the Malaysian documentation admits no registration type for,
/// because it admits the sector itself for no entry: "The sector valid values are
/// `Business` and `Individual`."
const SECTOR_OUTSIDE_THE_COUNTRY_SET: &str = "Government";

/// What the service must answer for one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The country's rules admit the entry: it is stored and reads back.
    Stored,
    /// The country's rules refuse it: 400 `ValidationException` carrying
    /// `FieldValidationFailed` and naming this member in `fieldList`.
    Refused(&'static str),
}

/// One row: the entry submitted, and what must come back.
struct Case {
    name: String,
    entry: Value,
    expect: Expect,
}

/// A Malaysian entry of `registration_type`, declaring `sector` when it has one,
/// held under either spelling of the country code.
///
/// Every row carries both members of `malaysiaAdditionalInfo` the country's
/// business and business-reseller rules ask for, so no row can be refused for a
/// missing additional-info member and the only rule left to decide it is the
/// pairing.
fn malaysia_entry(country_code: &str, sector: Option<&str>, registration_type: &str) -> Value {
    let mut entry = json!({
        "registrationId": "880101011234",
        "registrationType": registration_type,
        "legalName": "Contoh Sdn Bhd",
        "legalAddress": { "postalCode": "50450", "countryCode": country_code },
        "additionalTaxInformation": {
            "malaysiaAdditionalInfo": {
                "businessRegistrationNumber": "202001001234",
                "taxInformationNumber": "C1234567890",
            },
        },
    });
    if let Some(sector) = sector {
        entry
            .as_object_mut()
            .expect("an object")
            .insert("sector".to_string(), json!(sector));
    }
    entry
}

/// What the documented sentences say about a Malaysian entry, written out from the
/// documentation rather than read off the crate's table.
fn expected_for_malaysia(sector: Option<&str>, registration_type: &str) -> Expect {
    // "The sector valid values are Business and Individual" — a sector outside
    // that set is refused for the sector, whatever type the entry carries.
    if sector == Some(SECTOR_OUTSIDE_THE_COUNTRY_SET) {
        return Expect::Refused("SECTOR");
    }
    let individual = DOCUMENTED_INDIVIDUAL_TYPES.contains(&registration_type);
    let business = DOCUMENTED_BUSINESS_TYPES.contains(&registration_type);
    // A type the country pairs with no sector is outside its value set.
    if !individual && !business {
        return Expect::Refused("REGISTRATION_TYPE");
    }
    match sector {
        Some("Individual") if !individual => Expect::Refused("REGISTRATION_TYPE"),
        Some("Business") if !business => Expect::Refused("REGISTRATION_TYPE"),
        // An entry declaring no sector states nothing for the pairing to
        // contradict, so the country's value set is all that applies.
        _ => Expect::Stored,
    }
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let types: Vec<&str> = DOCUMENTED_INDIVIDUAL_TYPES
        .iter()
        .chain(DOCUMENTED_BUSINESS_TYPES)
        .copied()
        .chain([TYPE_OUTSIDE_THE_COUNTRY_SET])
        .collect();
    // Both spellings of the country code: `Address$countryCode`'s pattern
    // (`^[a-zA-Z]+$`) admits either, so the pairing must be found for either.
    for country_code in ["MY", "my"] {
        for sector in SECTORS {
            for registration_type in &types {
                cases.push(Case {
                    name: format!(
                        "{country_code} {registration_type} for sector {}",
                        sector.unwrap_or("(none)")
                    ),
                    entry: malaysia_entry(country_code, *sector, registration_type),
                    expect: expected_for_malaysia(*sector, registration_type),
                });
            }
        }
    }
    // A country whose documentation pairs the two members with nothing: every
    // sector it admits takes the registration type the entry carries, so the
    // pairing rule must not reach it. Nepal's own documented sentence ("The
    // sector valid values are Business and Individual") still refuses the third
    // sector.
    for sector in SECTORS {
        let expect = if *sector == Some(SECTOR_OUTSIDE_THE_COUNTRY_SET) {
            Expect::Refused("SECTOR")
        } else {
            Expect::Stored
        };
        let mut entry = json!({
            "registrationId": "600000001",
            "registrationType": TYPE_OUTSIDE_THE_COUNTRY_SET,
            "legalName": "Example Nepal Pvt Ltd",
            "legalAddress": { "postalCode": "44600", "countryCode": "NP" },
        });
        if let Some(sector) = sector {
            entry
                .as_object_mut()
                .expect("an object")
                .insert("sector".to_string(), json!(sector));
        }
        cases.push(Case {
            name: format!(
                "NP {TYPE_OUTSIDE_THE_COUNTRY_SET} for sector {}",
                sector.unwrap_or("(none)")
            ),
            entry,
            expect,
        });
    }
    cases
}

/// RULE: a registration type the declared `sector` of its jurisdiction does not
/// admit is refused with `ValidationException` naming the offending parameter and
/// nothing is persisted, while a type that sector admits is stored and reads back
/// — through either writer of a `TaxRegistrationEntry`.
#[tokio::test]
async fn a_type_the_declared_sector_does_not_admit_is_refused_by_both_writers() {
    for case in cases() {
        for writer in WRITERS {
            let name = format!("{} via {writer}", case.name);
            let router = emulator();
            let account_id = "000000000001";
            let answer = write(&router, writer, account_id, &case.entry).await;
            let read = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            match case.expect {
                Expect::Stored => assert_stored(&name, &case.entry, &answer, &read),
                Expect::Refused(parameter) => assert_refused(&name, parameter, &answer, &read),
            }
        }
    }
}

/// The entry was stored, reports the status it was stored with, and reads back
/// with the type and sector it was submitted with.
fn assert_stored(name: &str, entry: &Value, answer: &Answer, read: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (200, ""),
        "case {name:?}: expected the entry to be stored, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.get("status").as_str(),
        Some(STATUS_PENDING),
        "case {name:?}: expected the write to report the status it stored, got {:?}",
        answer.body
    );
    for member in ["registrationType", "sector"] {
        assert_eq!(
            read.body
                .pointer(&format!("/taxRegistration/{member}"))
                .and_then(Value::as_str),
            entry.get(member).and_then(Value::as_str),
            "case {name:?}: expected {member:?} to read back as submitted, got {:?}",
            read.body
        );
    }
}

/// The entry was refused for `parameter`, and nothing was persisted.
fn assert_refused(name: &str, parameter: &str, answer: &Answer, read: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {name:?}: expected 400 ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.get("errorCode").as_str(),
        Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
        "case {name:?}: expected errorCode {ERROR_CODE_FIELD_VALIDATION_FAILED:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        field_list(answer),
        vec![parameter.to_string()],
        "case {name:?}: expected fieldList to name {parameter:?}, got {:?}",
        answer.body
    );
    assert!(
        answer
            .get("message")
            .as_str()
            .is_some_and(|message| !message.trim().is_empty()),
        "case {name:?}: expected the refusal to carry a message, got {:?}",
        answer.body
    );
    assert_eq!(
        (read.status, read.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {name:?}: a refused entry must persist nothing, got {:?}",
        read.body
    );
}

/// RULE (the data): the types a country's pairing admits for a sector are the ones
/// its documentation names for that sector, and a country whose documentation
/// pairs the two members with nothing admits whatever its value set does.
#[test]
fn the_pairing_states_the_documented_sets_per_sector() {
    let documented: [(&str, &[&str]); 2] = [
        ("Individual", DOCUMENTED_INDIVIDUAL_TYPES),
        ("Business", DOCUMENTED_BUSINESS_TYPES),
    ];
    for (sector, types) in documented {
        for country_code in ["MY", "my"] {
            assert_eq!(
                sector_registration_types(country_code, sector),
                Some(types),
                "case ({country_code:?}, {sector:?}): expected the documented types {types:?}, \
                 got {:?}",
                sector_registration_types(country_code, sector)
            );
        }
    }
    for (country_code, sector) in [
        ("MY", SECTOR_OUTSIDE_THE_COUNTRY_SET),
        ("NP", "Business"),
        ("NP", "Individual"),
        ("TR", "Business"),
    ] {
        assert_eq!(
            sector_registration_types(country_code, sector),
            None,
            "case ({country_code:?}, {sector:?}): expected no pairing, got {:?}",
            sector_registration_types(country_code, sector)
        );
    }
    assert_eq!(
        parameter_name("taxRegistrationEntry.registrationType"),
        "REGISTRATION_TYPE",
        "the registration type is named REGISTRATION_TYPE on the wire, got {:?}",
        parameter_name("taxRegistrationEntry.registrationType")
    );
}
