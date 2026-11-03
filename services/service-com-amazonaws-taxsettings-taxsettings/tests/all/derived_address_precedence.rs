// SPDX-License-Identifier: CC0-1.0

//! Precedence inside the shared entry validation: a member the submitted
//! registration TYPE does not admit is refused as a field violation naming that
//! member, BEFORE any rule that would be decided from that member's value.
//!
//! RULE: for a `TaxRegistrationType` whose legal address the service derives from
//! the tax number itself
//! (`service_com_amazonaws_taxsettings_taxsettings::state::TYPES_WITH_DERIVED_LEGAL_ADDRESS`),
//! `legalAddress` is not the caller's to supply, so an entry that submits one is
//! refused 400 `ValidationException` / `FieldValidationFailed` naming the
//! legal-address parameter — whatever that address says — and its `countryCode`
//! is never read as the jurisdiction the registration is held in. A type that
//! DOES admit a submitted address is still judged on the address it carries.
//!
//! The rows span what the rule generalises over: every type in that catalog, the
//! country the submitted address names (the type's own country in three
//! spellings, countries on three other continents, and four countries whose own
//! per-country rules would otherwise claim the entry), the FORM of the address
//! (the members the model requires of one, and every member these jurisdictions
//! accept), and both writers of a `TaxRegistrationEntry` — `PutTaxRegistration`
//! and `BatchPutTaxRegistration`, which share the validator.
//!
//! The harness (`emulator`, `call`, `field_list`, `Answer`) lives in
//! `tax_registration_lifecycle.rs` and the writer fan-out (`WRITERS`, `write`) in
//! `country_rules.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    parameter_name, type_outside_its_country_code, type_with_derived_legal_address,
    ERROR_CODE_FIELD_VALIDATION_FAILED, TYPES_WITH_DERIVED_LEGAL_ADDRESS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::country_rules::{write, WRITERS};
use crate::tax_registration_lifecycle::{call, emulator, field_list, Answer};

/// The account every row writes under.
const ACCOUNT_ID: &str = "000000000001";

/// A registration type the service ties to no single country and whose address
/// the request supplies — the other side of the rule.
const TYPE_ADMITTING_AN_ADDRESS: &str = "VAT";

/// The `Address$countryCode` values a submitted address is tried with, one per
/// class the rule generalises over. Each is a country code, with the label the
/// failure message names the case by.
///
/// `IN`, `in` and `In` are the same jurisdiction — `Address$countryCode` is
/// `^[a-zA-Z]+$`, so its case carries no meaning — and `MY`, `TR`, `KE` and `CA`
/// are countries whose own rules in
/// `service_com_amazonaws_taxsettings_taxsettings::state::COUNTRY_RULES` would
/// refuse one of these entries if the address were read at all.
const SUBMITTED_ADDRESS_COUNTRIES: &[(&str, &str)] = &[
    ("the type's own country", "IN"),
    ("the type's own country, lower-cased", "in"),
    ("the type's own country, mixed-case", "In"),
    ("a country the type does not belong to", "DE"),
    ("a country on another continent", "US"),
    ("a country whose own types the service ties to it", "BR"),
    ("a country with its own registration-type rules", "MY"),
    ("a country that requires more of an address", "TR"),
    ("a country that requires an additional-info member", "KE"),
    ("a country that requires a state or region", "CA"),
];

/// The address forms a row is tried with: the members `Address` requires of one,
/// and every member these jurisdictions accept (`addressLine3` is accepted only
/// for Saudi Arabia, which is none of them — see
/// `crate::address_member_jurisdictions`).
fn address_forms(country_code: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "a minimal address",
            json!({ "postalCode": "560025", "countryCode": country_code }),
        ),
        (
            "a fully populated address",
            json!({
                "addressLine1": "1 Example Street",
                "addressLine2": "Example Floor",
                "city": "Example City",
                "stateOrRegion": "Example Region",
                "districtOrCounty": "Example District",
                "postalCode": "560025",
                "countryCode": country_code,
            }),
        ),
    ]
}

/// An entry of `registration_type`, carrying `legal_address` when there is one.
fn entry_of(registration_type: &str, legal_address: Option<&Value>) -> Value {
    let country_code = legal_address
        .and_then(|address| address.get("countryCode"))
        .and_then(Value::as_str);
    let mut entry = json!({
        "registrationId": crate::fidelity_rules::number_of_type(registration_type, country_code),
        "registrationType": registration_type,
        "legalName": "Example Holdings",
    });
    if let Some(address) = legal_address {
        entry
            .as_object_mut()
            .expect("an object")
            .insert("legalAddress".to_string(), address.clone());
    }
    entry
}

/// The `legalAddress.countryCode` the account's stored registration reports, or
/// `None` when it holds no registration.
async fn stored_country_code(router: &MultiServiceRouter, account_id: &str) -> Option<String> {
    let answer = call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    answer
        .body
        .pointer("/taxRegistration/legalAddress/countryCode")
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)
}

/// Whether `answer` is the field violation that refuses a submitted legal
/// address: the code, the named parameter and the 400 status together.
fn is_legal_address_violation(answer: &Answer) -> bool {
    answer.status == 400
        && answer.error_code == "ValidationException"
        && answer.get("errorCode").as_str() == Some(ERROR_CODE_FIELD_VALIDATION_FAILED)
        && field_list(answer) == vec![parameter_name("taxRegistrationEntry.legalAddress")]
}

/// RULE: a type whose legal address the service derives refuses a submitted
/// address as a field violation naming that member, whatever the address says —
/// so no rule read FROM the address (the jurisdiction a type belongs to, the
/// country's own requirements) can claim the entry first.
#[tokio::test]
async fn a_submitted_address_a_type_does_not_admit_is_refused_before_it_is_read() {
    assert!(
        !TYPES_WITH_DERIVED_LEGAL_ADDRESS.is_empty(),
        "the catalog must name at least one type whose address the service derives"
    );
    // The parameter name is the one the service answers with on the wire, read
    // through the shared vocabulary rather than spelled a second time here.
    assert_eq!(
        parameter_name("taxRegistrationEntry.legalAddress"),
        "LEGAL_ADDRESS",
        "the legal address is named LEGAL_ADDRESS on the wire, got {:?}",
        parameter_name("taxRegistrationEntry.legalAddress")
    );

    for registration_type in TYPES_WITH_DERIVED_LEGAL_ADDRESS {
        let (_, country) = type_with_derived_legal_address(registration_type).unwrap_or_else(|| {
            panic!("type {registration_type:?}: a derived address belongs to a country")
        });
        let expected_message = format!(
            "Legal address should not be provided for {country} {registration_type} Tax \
             Registration."
        );
        for (country_case, country_code) in SUBMITTED_ADDRESS_COUNTRIES {
            for (form, address) in address_forms(country_code) {
                for writer in WRITERS {
                    let case = format!(
                        "{registration_type} submitting {form} in {country_case} \
                         ({country_code}), written via {writer}"
                    );
                    let router = emulator();
                    let answer = write(
                        &router,
                        writer,
                        ACCOUNT_ID,
                        &entry_of(registration_type, Some(&address)),
                    )
                    .await;

                    assert_eq!(
                        (answer.status, answer.error_code.as_str()),
                        (400, "ValidationException"),
                        "case {case:?}: expected 400 ValidationException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.get("errorCode").as_str(),
                        Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
                        "case {case:?}: expected the {ERROR_CODE_FIELD_VALIDATION_FAILED} code, \
                         got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        field_list(&answer),
                        vec![parameter_name("taxRegistrationEntry.legalAddress")],
                        "case {case:?}: expected the refusal to name the legal address, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.get("message").as_str(),
                        Some(expected_message.as_str()),
                        "case {case:?}: expected the rule stated in words, got {:?}",
                        answer.body
                    );
                    // The refusal the type's published jurisdiction code exists
                    // for is a different rule, decided from the customer's own
                    // country: a request refused for submitting the address
                    // cannot have that address read as evidence of where the
                    // customer is.
                    if let Some(jurisdiction_code) =
                        type_outside_its_country_code(registration_type)
                    {
                        assert_ne!(
                            answer.get("errorCode").as_str(),
                            Some(jurisdiction_code),
                            "case {case:?}: the submitted address must not be read as the \
                             customer's country, got {:?}",
                            answer.body
                        );
                    }
                    assert_eq!(
                        stored_country_code(&router, ACCOUNT_ID).await,
                        None,
                        "case {case:?}: a refused write must store nothing"
                    );
                }
            }
        }
    }
}

/// RULE, the other side of it: a type that DOES admit a submitted address is
/// still judged on the address it carries — it is never refused for carrying the
/// member, and an address no country rule objects to is stored and reads back
/// held in that country.
#[tokio::test]
async fn a_type_that_admits_an_address_is_judged_on_the_address_it_carries() {
    assert!(
        type_with_derived_legal_address(TYPE_ADMITTING_AN_ADDRESS).is_none(),
        "{TYPE_ADMITTING_AN_ADDRESS} must be a type whose address the request supplies"
    );

    for (country_case, country_code) in SUBMITTED_ADDRESS_COUNTRIES {
        for (form, address) in address_forms(country_code) {
            for writer in WRITERS {
                let case = format!(
                    "{TYPE_ADMITTING_AN_ADDRESS} submitting {form} in {country_case} \
                     ({country_code}), written via {writer}"
                );
                let router = emulator();
                let answer = write(
                    &router,
                    writer,
                    ACCOUNT_ID,
                    &entry_of(TYPE_ADMITTING_AN_ADDRESS, Some(&address)),
                )
                .await;
                assert!(
                    !is_legal_address_violation(&answer),
                    "case {case:?}: an admitted address must not be refused as a member the type \
                     does not take, got {answer:?}"
                );
                if answer.status == 200 {
                    assert_eq!(
                        stored_country_code(&router, ACCOUNT_ID)
                            .await
                            .as_deref()
                            .map(str::to_ascii_uppercase),
                        Some(country_code.to_ascii_uppercase()),
                        "case {case:?}: a stored registration is held in the country its address \
                         named"
                    );
                }
            }
        }
    }
}

/// RULE, completed: an entry of a type whose address the service derives and that
/// submits none is stored, and reads back held in the country that type belongs
/// to — the address the service filled in, which is why the request may not
/// supply one.
#[tokio::test]
async fn a_type_whose_address_is_derived_stores_the_address_it_derives() {
    for registration_type in TYPES_WITH_DERIVED_LEGAL_ADDRESS {
        let (country_code, _) = type_with_derived_legal_address(registration_type)
            .unwrap_or_else(|| {
                panic!("type {registration_type:?}: a derived address belongs to a country")
            });
        for writer in WRITERS {
            let case = format!("{registration_type} submitting no address, written via {writer}");
            let router = emulator();
            let answer = write(
                &router,
                writer,
                ACCOUNT_ID,
                &entry_of(registration_type, None),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the entry must be stored, got {answer:?}"
            );
            assert_eq!(
                stored_country_code(&router, ACCOUNT_ID).await.as_deref(),
                Some(country_code),
                "case {case:?}: the record must be held in the country the type belongs to"
            );
        }
    }
}
