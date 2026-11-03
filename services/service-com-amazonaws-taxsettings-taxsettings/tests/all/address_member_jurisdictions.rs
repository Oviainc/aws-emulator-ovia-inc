// SPDX-License-Identifier: CC0-1.0

//! The jurisdictions an `Address` member is ACCEPTED for, over the restJson1
//! wire.
//!
//! ONE rule: an address member the model documents the API as accepting only for
//! certain countries may be carried only by an address held in one of them, so an
//! address held anywhere else that carries it is refused with
//! `ValidationException` (HTTP 400, `FieldValidationFailed`) naming that
//! parameter and nothing is stored, while an address held in a country that
//! accepts it stores the member and reads it back. The rule is about the ADDRESS,
//! so every operation that takes one is judged by it — the legal address of a
//! `TaxRegistrationEntry` through both writers (`PutTaxRegistration`,
//! `BatchPutTaxRegistration`) and the address of a
//! `SupplementalTaxRegistrationEntry`.
//!
//! The catalog under test is the service's own
//! (`service_com_amazonaws_taxsettings_taxsettings::state::ADDRESS_MEMBER_JURISDICTIONS`),
//! read off the model
//! (`smithy_query(query="shape",shape="com.amazonaws.taxsettings#Address")`:
//! `addressLine3` is documented "Currently, the Tax Settings API accepts the
//! `addressLine3` parameter only for Saudi Arabia"), so a member added to it is
//! driven by every row below rather than by a case written for one request.
//!
//! The tables span what the rule generalises over: every member of the catalog,
//! the countries that accept it (in both cases its two-letter code admits) and
//! nine that do not — on five continents, including countries whose own
//! per-country rules would otherwise claim the entry — the FORM of the value
//! (a building number, each published bound of the member's 1..=60 length and one
//! past the high bound), whether the member was carried at all, and all three
//! write paths that take an address.
//!
//! The harness (`emulator`, `call`, `field_list`) lives in
//! `tax_registration_lifecycle.rs`, the two entry writers in `country_rules.rs`
//! and the supplemental write and listing in `supplemental_tax_registrations.rs`;
//! nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    parameter_name, AddressMemberJurisdiction, ADDRESS_MEMBER_JURISDICTIONS,
    ERROR_CODE_FIELD_VALIDATION_FAILED, SUPPLEMENTAL_TRN_COUNTRIES,
};

use crate::country_rules::{write, WRITERS};
use crate::supplemental_tax_registrations::{list_all, put as put_supplemental};
use crate::tax_registration_lifecycle::{call, emulator, field_list, Answer};

/// The account every row writes under.
const ACCOUNT_ID: &str = "000000000001";

/// One jurisdiction a row submits its address in.
#[derive(Debug, Clone, Copy)]
struct Jurisdiction {
    /// The class this country stands for, as a failing case names it.
    label: &'static str,
    /// Its `Address$countryCode`.
    country_code: &'static str,
    /// Whether this country's own per-country rules require more of an entry
    /// than the table below builds. For such a country only the REFUSAL can be
    /// asserted — which is the point of the row: the member is refused before the
    /// country's own requirements are read — so it is left out of the control
    /// that stores an address carrying no restricted member.
    has_own_requirements: bool,
}

/// The jurisdictions a restricted member is submitted in. None of them is in any
/// catalog row's accepting set (each row skips a country it accepts), so each is
/// a country the member must be refused for.
///
/// `jp` is `JP` spelled the other way the member's own pattern (`^[a-zA-Z]+$`)
/// admits, and `KE`, `TR` and `CA` carry per-country requirements of their own
/// (`…::state::COUNTRY_RULES`) that would refuse these entries for another
/// member if the address were judged after them.
const JURISDICTIONS: &[Jurisdiction] = &[
    Jurisdiction {
        label: "a country in Asia",
        country_code: "JP",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "a country in Asia, lower-cased",
        country_code: "jp",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "a country in Europe",
        country_code: "NL",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "a country in North America",
        country_code: "US",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "a country in South America",
        country_code: "BR",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "a country in Oceania",
        country_code: "AU",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "the country this service stores a supplemental TRN for",
        country_code: "CH",
        has_own_requirements: false,
    },
    Jurisdiction {
        label: "a country that requires an additional-info member",
        country_code: "KE",
        has_own_requirements: true,
    },
    Jurisdiction {
        label: "a country that requires more of an address",
        country_code: "TR",
        has_own_requirements: true,
    },
    Jurisdiction {
        label: "a country that requires a state or region",
        country_code: "CA",
        has_own_requirements: true,
    },
];

/// The values a restricted member is submitted with: a building number, as the
/// member's own documentation gives as the example, and each published bound of
/// its 1..=60 length (`smithy_query(query="shape",shape="…#AddressLine3")`).
fn accepted_values() -> Vec<(String, String)> {
    vec![
        ("a building number".to_string(), "1234".to_string()),
        (
            "one character, the low length bound".to_string(),
            "7".to_string(),
        ),
        (
            "sixty characters, the high length bound".to_string(),
            "B".repeat(60),
        ),
    ]
}

/// A legal address held in `country_code`, carrying `member` when one is given.
fn address(country_code: &str, member: Option<(&str, &str)>) -> Value {
    let mut address = json!({
        "addressLine1": "1 Example Street",
        "city": "Example City",
        "postalCode": "10115",
        "countryCode": country_code,
    });
    if let Some((name, value)) = member {
        address
            .as_object_mut()
            .expect("an address is a JSON object")
            .insert(name.to_string(), json!(value));
    }
    address
}

/// A `TaxRegistrationEntry` whose legal address is held in `country_code` and
/// carries `member` when one is given.
///
/// `VAT` is a registration type the service ties to no single country and whose
/// address the request supplies, so what decides each answer is the address.
fn entry(country_code: &str, member: Option<(&str, &str)>) -> Value {
    json!({
        "registrationId": "VAT-0000000001",
        "registrationType": "VAT",
        "legalName": "Example Holdings",
        "legalAddress": address(country_code, member),
    })
}

/// Assert `answer` is the refusal of `member` carried where the API does not
/// accept it, and that `case` stored nothing.
async fn assert_refused(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    rule: &AddressMemberJurisdiction,
    answer: &Answer,
) {
    let parameter = parameter_name(rule.member);
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.get("errorCode").as_str(),
        Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
        "case {case:?}: expected errorCode {ERROR_CODE_FIELD_VALIDATION_FAILED:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        field_list(answer),
        vec![parameter.clone()],
        "case {case:?}: expected fieldList to name {parameter:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.get("message").as_str(),
        Some(rule.message().as_str()),
        "case {case:?}: expected the documented rule stated in words, got {:?}",
        answer.body
    );
    let read = call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": ACCOUNT_ID }),
    )
    .await;
    assert_eq!(
        read.status, 404,
        "case {case:?}: a refused write must store nothing, got {read:?}"
    );
}

/// RULE: an address held in a country the API does not accept the member for is
/// refused for that member — whatever the value, whichever writer, and before any
/// of that country's own requirements is read.
#[tokio::test]
async fn a_member_only_some_jurisdictions_accept_is_refused_for_an_address_held_elsewhere() {
    assert!(
        !ADDRESS_MEMBER_JURISDICTIONS.is_empty(),
        "the catalog must name at least one member the API accepts only for some countries"
    );

    for rule in ADDRESS_MEMBER_JURISDICTIONS {
        for jurisdiction in JURISDICTIONS {
            if rule.accepts(jurisdiction.country_code) {
                continue;
            }
            for (form, value) in accepted_values() {
                for writer in WRITERS {
                    let case = format!(
                        "{} carrying {form} in {} ({}), written via {writer}",
                        rule.member, jurisdiction.label, jurisdiction.country_code
                    );
                    let router = emulator();
                    let answer = write(
                        &router,
                        writer,
                        ACCOUNT_ID,
                        &entry(
                            jurisdiction.country_code,
                            Some((rule.member, value.as_str())),
                        ),
                    )
                    .await;
                    assert_refused(&router, &case, rule, &answer).await;
                }
            }
        }
    }
}

/// RULE, the other side of it: an address held in a country that DOES accept the
/// member stores it and reads it back, at every length the member admits — and a
/// value past its high length bound is refused by the member's own constraint.
#[tokio::test]
async fn a_member_reads_back_from_an_address_held_in_a_jurisdiction_that_accepts_it() {
    for rule in ADDRESS_MEMBER_JURISDICTIONS {
        assert!(
            !rule.country_codes.is_empty(),
            "member {:?}: a restricted member must name the countries that accept it",
            rule.member
        );
        for country_code in rule.country_codes {
            // The code's own pattern carries no meaning in its case, so both
            // spellings name the same accepting jurisdiction.
            for spelling in [country_code.to_ascii_uppercase(), country_code.to_lowercase()] {
                for (form, value) in accepted_values() {
                    for writer in WRITERS {
                        let case = format!(
                            "{} carrying {form} in {spelling}, written via {writer}",
                            rule.member
                        );
                        let router = emulator();
                        let submitted = entry(&spelling, Some((rule.member, value.as_str())));
                        let answer =
                            write(&router, writer, ACCOUNT_ID, &submitted).await;
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?}: an accepting jurisdiction must store the member, \
                             got {answer:?}"
                        );
                        let read = call(
                            &router,
                            "GetTaxRegistration",
                            &json!({ "accountId": ACCOUNT_ID }),
                        )
                        .await;
                        assert_eq!(
                            read.body
                                .pointer(&format!("/taxRegistration/legalAddress/{}", rule.member))
                                .and_then(Value::as_str),
                            Some(value.as_str()),
                            "case {case:?}: expected the stored member to read back as \
                             {value:?}, got {:?}",
                            read.body
                        );
                    }
                }

                // One character past the high bound of the member's own length:
                // the constraint on the member, which an accepting jurisdiction
                // does not lift.
                let case = format!("{} one character past its length bound", rule.member);
                let router = emulator();
                let answer = write(
                    &router,
                    "PutTaxRegistration",
                    ACCOUNT_ID,
                    &entry(&spelling, Some((rule.member, "C".repeat(61).as_str()))),
                )
                .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: a value past the member's length bound must be refused, \
                     got {answer:?}"
                );
            }
        }
    }
}

/// RULE (the control): the restriction is about the member, so an address that
/// carries none of the restricted members is stored in any of these
/// jurisdictions and reads back without them.
#[tokio::test]
async fn an_address_that_carries_no_restricted_member_is_stored_in_every_jurisdiction() {
    for jurisdiction in JURISDICTIONS {
        if jurisdiction.has_own_requirements {
            continue;
        }
        for writer in WRITERS {
            let case = format!(
                "no restricted member in {} ({}), written via {writer}",
                jurisdiction.label, jurisdiction.country_code
            );
            let router = emulator();
            let answer = write(
                &router,
                writer,
                ACCOUNT_ID,
                &entry(jurisdiction.country_code, None),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an address carrying no restricted member must be stored, \
                 got {answer:?}"
            );
            let read = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": ACCOUNT_ID }),
            )
            .await;
            for rule in ADDRESS_MEMBER_JURISDICTIONS {
                assert_eq!(
                    read.body
                        .pointer(&format!("/taxRegistration/legalAddress/{}", rule.member)),
                    None,
                    "case {case:?}: a member the request did not carry must not be reported, \
                     got {:?}",
                    read.body
                );
            }
        }
    }
}

/// RULE, on the other write path that takes an address: a supplemental TRN is
/// judged by the same rule, so its address is refused for a restricted member its
/// own jurisdiction does not accept and nothing is stored, while the same entry
/// without the member stores and lists.
#[tokio::test]
async fn the_supplemental_write_path_judges_an_address_by_the_same_rule() {
    for (country_code, _) in SUPPLEMENTAL_TRN_COUNTRIES {
        for rule in ADDRESS_MEMBER_JURISDICTIONS {
            let (router, _engine) =
                crate::supplemental_tax_registrations::emulator_supplementing();
            let supplemental = |member: Option<(&str, &str)>| {
                json!({
                    "registrationId": "CHE-123.456.789",
                    "registrationType": "VAT",
                    "legalName": "Example AG",
                    "address": address(country_code, member),
                })
            };

            if !rule.accepts(country_code) {
                for (form, value) in accepted_values() {
                    let case = format!(
                        "a supplemental TRN in {country_code} carrying {} as {form}",
                        rule.member
                    );
                    let answer =
                        put_supplemental(&router, &supplemental(Some((rule.member, &value))))
                            .await;
                    assert_eq!(
                        (answer.status, answer.error_code.as_str()),
                        (400, "ValidationException"),
                        "case {case:?}: expected 400 ValidationException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.get("errorCode").as_str(),
                        Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
                        "case {case:?}: expected errorCode \
                         {ERROR_CODE_FIELD_VALIDATION_FAILED:?}, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        field_list(&answer),
                        vec![parameter_name(rule.member)],
                        "case {case:?}: expected fieldList to name the member, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.get("message").as_str(),
                        Some(rule.message().as_str()),
                        "case {case:?}: expected the documented rule stated in words, got {:?}",
                        answer.body
                    );
                    assert!(
                        list_all(&router, None).await.is_empty(),
                        "case {case:?}: a refused supplemental write must store nothing"
                    );
                }
            }

            let case = format!("a supplemental TRN in {country_code} carrying no restricted member");
            let answer = put_supplemental(&router, &supplemental(None)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the entry must be stored, got {answer:?}"
            );
            let listed = list_all(&router, None).await;
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: exactly the stored registration must be listed, got {listed:?}"
            );
            assert_eq!(
                listed[0].pointer(&format!("/address/{}", rule.member)),
                None,
                "case {case:?}: a member the request did not carry must not be reported, \
                 got {:?}",
                listed[0]
            );
        }
    }
}
