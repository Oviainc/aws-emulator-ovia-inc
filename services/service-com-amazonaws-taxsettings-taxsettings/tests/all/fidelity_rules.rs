// SPDX-License-Identifier: CC0-1.0

//! The fidelity rules this service answers requests by, over the wire.
//!
//! Each test states ONE rule and asserts it over a table spanning the class the
//! rule generalises over — every `TaxRegistrationType` the model lists, both
//! `HeritageStatus` values, several member paths and every constraint class the
//! boundary validator can report — rather than over the one request that
//! happened to be recorded.
//!
//! The harness (`emulator_with_engine`, `call`, and the two helpers that move a
//! clock-owed TRN validation) lives in `tax_registration_lifecycle.rs`; nothing
//! here builds a second one.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    parameter_name, type_with_derived_legal_address, DEFAULT_HERITAGE_STATUS,
    DERIVED_MEMBER_UNKNOWN, HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_OUT, STATUS_DELETED,
    STATUS_PENDING, STATUS_REJECTED, STATUS_VERIFIED, TRN_VALIDATION_DELAY,
};

use crate::tax_registration_lifecycle::{
    call, emulator, emulator_with_engine, field_list, hold_validations_open, make_validations_due,
};

/// Every value of the model's `TaxRegistrationType` enum
/// (`smithy_query(query="shape", shape="TaxRegistrationType")`).
const REGISTRATION_TYPES: &[&str] = &[
    "VAT", "GST", "CPF", "CNPJ", "SST", "TIN", "NRIC", "PAN", "NIP",
];

/// Every value of the model's `TaxRegistrationStatus` enum
/// (`smithy_query(query="shape", shape="TaxRegistrationStatus")`).
///
/// `pub(crate)` because `tests/all/pending_trn_conflict.rs` asserts the
/// pending-TRN rule against each of them.
pub(crate) const REGISTRATION_STATUSES: &[&str] = &[
    STATUS_VERIFIED,
    STATUS_PENDING,
    STATUS_DELETED,
    STATUS_REJECTED,
];

/// A legal address that satisfies every constraint on `Address`.
fn legal_address() -> Value {
    json!({
        "addressLine1": "1 Example Street",
        "city": "Example City",
        "stateOrRegion": "Example Region",
        "postalCode": "400001",
        "countryCode": "IN",
    })
}

/// The `registrationId` an entry of `registration_type`, held in
/// `country_code`, carries.
///
/// A write refuses a number its jurisdiction's authority does not issue
/// (`state::check_registration_number`), so a jurisdiction and type this service
/// publishes a structure for carries a number that satisfies it — the layout and
/// check digits that authority publishes — and every other pair a plain
/// synthetic id, which no published structure judges. `None` is a jurisdiction
/// the entry does not name, which the service reads off the type itself.
pub(crate) fn number_of_type(registration_type: &str, country_code: Option<&str>) -> String {
    match (registration_type, country_code.unwrap_or_default()) {
        // Nine digits closed by the check digit Germany's rule computes for the
        // eight before it, and France's two-digit key over its SIREN.
        ("VAT", "DE") => "DE136695976".to_string(),
        ("VAT", "FR") => "FR15345678901".to_string(),
        // Kenya's published PIN layout, India's GSTIN and PAN layouts.
        ("TIN", "KE") => "P051234567M".to_string(),
        ("GST", _) => "29ABCDE1234F1Z5".to_string(),
        ("PAN", _) => "ABCDE1234F".to_string(),
        // The check digits Brazil's two registers publish for these bodies.
        ("CNPJ", _) => "11222333000181".to_string(),
        ("CPF", _) => "11144477735".to_string(),
        _ => format!("{registration_type}-0000000001"),
    }
}

/// An entry of `registration_type`, with a legal address or without one.
fn entry_of_type(registration_type: &str, with_address: bool) -> Value {
    let country_code = with_address.then(|| {
        legal_address()
            .get("countryCode")
            .and_then(Value::as_str)
            .expect("the legal address names its country")
            .to_string()
    });
    let mut entry = json!({
        "registrationId": number_of_type(registration_type, country_code.as_deref()),
        "registrationType": registration_type,
        "legalName": "Example Holdings",
    });
    if with_address {
        entry
            .as_object_mut()
            .expect("an object")
            .insert("legalAddress".to_string(), legal_address());
    }
    entry
}

/// Whether AWS derives this registration type's legal address from the tax
/// number itself, with the country whose rule it is.
fn derived_address_country(registration_type: &str) -> Option<&'static str> {
    type_with_derived_legal_address(registration_type).map(|(_, country)| country)
}

/// The `TaxRegistrationType` values an entry may omit the legal address for, with
/// the `Address$countryCode` the stored record is then held in: India's `PAN`,
/// whose address AWS reads off the tax number, and Brazil's `CNPJ` ("If you're
/// setting a TRN in Brazil for the CNPJ tax type, you don't need to specify the
/// legal address",
/// `smithy_query(query="doc",shape="com.amazonaws.taxsettings#TaxRegistrationEntry",member="legalAddress")`).
/// Every other type the model lists must carry one — including Brazil's `CPF`,
/// which the same sentence excludes ("for CPF tax types Brazil, you must specify
/// the legal address").
const TYPES_NEEDING_NO_LEGAL_ADDRESS: &[(&str, &str)] = &[("PAN", "IN"), ("CNPJ", "BR")];

/// The jurisdiction a record of `registration_type` is held in when its entry
/// carried no legal address, or `None` when such an entry is refused.
fn jurisdiction_without_a_legal_address(registration_type: &str) -> Option<&'static str> {
    TYPES_NEEDING_NO_LEGAL_ADDRESS
        .iter()
        .find(|(declared, _)| *declared == registration_type)
        .map(|(_, country_code)| *country_code)
}

/// RULE: a registration type whose legal address AWS derives from the tax number
/// itself does not take one from the request — submitting one is refused with
/// `ValidationException` / `FieldValidationFailed` naming that parameter — while
/// every other type takes the address it was given and stores it.
#[tokio::test]
async fn a_type_whose_address_is_derived_refuses_a_submitted_legal_address() {
    // The one row the recorded behaviour pins: India's `PAN` is a type whose
    // address AWS derives, and the parameter it names is `LEGAL_ADDRESS`. The
    // table below then applies the rule to every type the model lists.
    assert_eq!(
        derived_address_country("PAN"),
        Some("India"),
        "PAN is a type whose legal address AWS derives, got {:?}",
        derived_address_country("PAN")
    );
    assert_eq!(
        parameter_name("taxRegistrationEntry.legalAddress"),
        "LEGAL_ADDRESS",
        "the legal address is named LEGAL_ADDRESS on the wire, got {:?}",
        parameter_name("taxRegistrationEntry.legalAddress")
    );

    for registration_type in REGISTRATION_TYPES {
        for with_address in [true, false] {
            let case = format!("{registration_type} {}", if with_address { "with a legal address" } else { "with no legal address" });
            let router = emulator();
            let answer = call(
                &router,
                "PutTaxRegistration",
                &json!({
                    "accountId": "000000000001",
                    "taxRegistrationEntry": entry_of_type(registration_type, with_address),
                }),
            )
            .await;
            let derived = derived_address_country(registration_type);
            match (derived, with_address) {
                // The address is not the caller's to supply.
                (Some(country), true) => {
                    assert_eq!(
                        (answer.status, answer.error_code.as_str()),
                        (400, "ValidationException"),
                        "case {case:?}: expected 400 ValidationException, got {answer:?}"
                    );
                    assert_eq!(
                        answer.get("errorCode").as_str(),
                        Some("FieldValidationFailed"),
                        "case {case:?}: expected the FieldValidationFailed code, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        field_list(&answer),
                        vec!["LEGAL_ADDRESS".to_string()],
                        "case {case:?}: expected the refusal to name the offending parameter, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.get("message").as_str(),
                        Some(
                            format!(
                                "Legal address should not be provided for {country} \
                                 {registration_type} Tax Registration."
                            )
                            .as_str()
                        ),
                        "case {case:?}: expected the rule stated without the submitted address, \
                         got {:?}",
                        answer.body
                    );
                }
                // An entry that carries no address is stored where the service
                // fills the address in, and refused where the jurisdiction it
                // identifies requires one.
                (_, false) => match jurisdiction_without_a_legal_address(registration_type) {
                    Some(country_code) => {
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?}: expected the entry to be stored, got {answer:?}"
                        );
                        let got = call(
                            &router,
                            "GetTaxRegistration",
                            &json!({ "accountId": "000000000001" }),
                        )
                        .await;
                        let address = got
                            .get("taxRegistration")
                            .get("legalAddress")
                            .cloned()
                            .unwrap_or_default();
                        assert_eq!(
                            address.get("countryCode").and_then(Value::as_str),
                            Some(country_code),
                            "case {case:?}: expected the record held in {country_code}, got {:?}",
                            got.body
                        );
                        assert_eq!(
                            address.get("postalCode").and_then(Value::as_str),
                            Some(DERIVED_MEMBER_UNKNOWN),
                            "case {case:?}: expected the filled-in address to report what no \
                             registry here can supply, got {:?}",
                            got.body
                        );
                    }
                    None => {
                        assert_eq!(
                            (answer.status, answer.error_code.as_str()),
                            (400, "ValidationException"),
                            "case {case:?}: expected 400 ValidationException, got {answer:?}"
                        );
                        assert_eq!(
                            answer.get("errorCode").as_str(),
                            Some("MissingInput"),
                            "case {case:?}: expected the MissingInput code, got {:?}",
                            answer.body
                        );
                        assert_eq!(
                            field_list(&answer),
                            vec!["LEGAL_ADDRESS".to_string()],
                            "case {case:?}: expected the refusal to name the missing parameter, \
                             got {:?}",
                            answer.body
                        );
                    }
                },
                // Every other type takes the address it was given and stores it.
                (None, true) => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the entry to be stored, got {answer:?}"
                    );
                    let got = call(
                        &router,
                        "GetTaxRegistration",
                        &json!({ "accountId": "000000000001" }),
                    )
                    .await;
                    assert_eq!(
                        got.get("taxRegistration").get("legalAddress"),
                        Some(&legal_address()),
                        "case {case:?}: expected the submitted address to round-trip, got {:?}",
                        got.body
                    );
                }
            }
        }
    }
}

/// RULE: a submitted TRN's status is the outcome of validating it — it is stored
/// and reported `Pending` while that validation is outstanding, and every read
/// reports what the validation reached once the clock has made it due, whoever
/// calls and however often.
#[tokio::test]
async fn a_submitted_trn_is_pending_until_its_validation_is_due_then_settles() {
    // The span the clock owes is the project's 250-750 ms band: never zero, so a
    // caller reading straight after the write still sees `Pending` — and still
    // meets the refusals that status carries.
    assert!(
        (Duration::from_millis(250)..=Duration::from_millis(750)).contains(&TRN_VALIDATION_DELAY),
        "the owed validation must fall due 250-750 ms out, got {TRN_VALIDATION_DELAY:?}"
    );

    // Rows differ in the type, the sector and whether documents were submitted:
    // neither the TIMING nor the outcome is a function of any of them — every
    // number a write ADMITTED is one the validation finds no fault in, and a
    // number its jurisdiction's authority does not issue is refused by the write
    // instead of stored, which is pinned in
    // `tests/all/trn_validation_outcome.rs`.
    let rows: &[(&str, Value, &str)] = &[
        (
            "VAT entry",
            json!({
                // Nine digits closed by the check digit Germany's rule computes
                // for the eight before it.
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
            STATUS_VERIFIED,
        ),
        (
            "GST entry with a sector",
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
            STATUS_VERIFIED,
        ),
        (
            "TIN entry carrying a document",
            json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "legalAddress": { "postalCode": "28001", "countryCode": "ES" },
                // Spain's documented rules: the registrationType must be
                // specified, and a `Local` one needs the certificate document
                // (`crate::state::COUNTRY_RULES`).
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Local" },
                },
                "verificationDetails": {
                    "taxRegistrationDocuments": [
                        { "file": { "fileName": "certificate.pdf", "fileContent": "aGVsbG8=" } },
                    ],
                },
            }),
            STATUS_VERIFIED,
        ),
    ];

    for (case, entry, settles_to) in rows {
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
            "case {case:?}: PutTaxRegistration expected 200, got {put:?}"
        );
        assert_eq!(
            put.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {case:?}: expected the write to report the status it stored, got {:?}",
            put.body
        );

        // While the validation is outstanding, every read reports `Pending`.
        hold_validations_open(&engine);
        for reader in ["GetTaxRegistration", "ListTaxRegistrations"] {
            let status = read_status(&router, reader, account_id).await;
            assert_eq!(
                status.as_deref(),
                Some(STATUS_PENDING),
                "case {case:?}: {reader} must report the outstanding status, got {status:?}"
            );
        }

        // Once the clock has made it due, every read reports what the validation
        // reached — and reports it again, because settling what was owed is not
        // a change a read made.
        make_validations_due(&engine);
        for reader in ["GetTaxRegistration", "ListTaxRegistrations", "GetTaxRegistration"] {
            let status = read_status(&router, reader, account_id).await;
            assert_eq!(
                status.as_deref(),
                Some(*settles_to),
                "case {case:?}: {reader} must report the settled status, got {status:?}"
            );
        }

        // Writing the TRN again starts a new validation, so the record is
        // outstanding once more.
        let rewrite = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            rewrite.get("status").as_str(),
            Some(STATUS_PENDING),
            "case {case:?}: expected a rewrite to start a new validation, got {:?}",
            rewrite.body
        );
        hold_validations_open(&engine);
        let status = read_status(&router, "GetTaxRegistration", account_id).await;
        assert_eq!(
            status.as_deref(),
            Some(STATUS_PENDING),
            "case {case:?}: expected the rewritten record to be outstanding, got {status:?}"
        );
    }
}

/// The status `operation` reports for `account_id`, read through `GetTaxRegistration`
/// or found in the `ListTaxRegistrations` page that carries the account.
pub(crate) async fn read_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    account_id: &str,
) -> Option<String> {
    if operation == "ListTaxRegistrations" {
        let listed = call(router, operation, &json!({})).await;
        return listed
            .get("accountDetails")
            .as_array()
            .and_then(|details| {
                details
                    .iter()
                    .find(|detail| detail.get("accountId").and_then(Value::as_str) == Some(account_id))
                    .cloned()
            })
            .and_then(|detail| {
                detail
                    .pointer("/taxRegistration/status")
                    .and_then(Value::as_str)
                    .map(std::string::ToString::to_string)
            });
    }
    let got = call(router, operation, &json!({ "accountId": account_id })).await;
    got.body
        .pointer("/taxRegistration/status")
        .and_then(Value::as_str)
        .map(std::string::ToString::to_string)
}

/// RULE: `GetTaxInheritance` reports an enum value for the account's election,
/// and the one it reports while no election is recorded is the default an account
/// that has not opted in has. What a write records is asserted in
/// `tests/all/tax_inheritance_election.rs`.
#[tokio::test]
async fn tax_inheritance_is_reported_as_an_enum_value() {
    let router = emulator();

    assert!(
        [HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_OUT].contains(&DEFAULT_HERITAGE_STATUS),
        "the default election must be a HeritageStatus value, got {DEFAULT_HERITAGE_STATUS:?}"
    );
    // And it is the one an account that has not opted in has: `OptOut`.
    assert_eq!(
        DEFAULT_HERITAGE_STATUS, HERITAGE_STATUS_OPT_OUT,
        "an account that has not opted in is OptOut, got {DEFAULT_HERITAGE_STATUS:?}"
    );
    // A read records nothing, so repeating it reports the same default.
    for read in 1..=2 {
        let before = call(&router, "GetTaxInheritance", &json!({})).await;
        assert_eq!(
            before.status, 200,
            "case \"read {read}\": GetTaxInheritance expected 200, got {before:?}"
        );
        assert_eq!(
            before.get("heritageStatus").as_str(),
            Some(DEFAULT_HERITAGE_STATUS),
            "case \"read {read}\": expected the default election while none is recorded, got {:?}",
            before.body
        );
    }
}

/// RULE: an error body carries the members this service populates and no more —
/// `errorCode` is a service-defined code, never the exception's own shape name,
/// which the wire carries in `x-amzn-errortype` / `__type`.
#[tokio::test]
async fn a_not_found_body_carries_its_message_and_no_error_code() {
    let router = emulator();
    // Accounts that never had a registration, and one whose registration was
    // deleted: the same not-found answer either way.
    let stored = "000000000001";
    let put = call(
        &router,
        "PutTaxRegistration",
        &json!({
            "accountId": stored,
            "taxRegistrationEntry": entry_of_type("VAT", true),
        }),
    )
    .await;
    assert_eq!(put.status, 200, "setup put failed: {put:?}");
    let deleted = call(
        &router,
        "DeleteTaxRegistration",
        &json!({ "accountId": stored }),
    )
    .await;
    assert_eq!(deleted.status, 200, "setup delete failed: {deleted:?}");

    for account_id in [stored, "000000000002", "999999999999", engine::emulator_account_id()] {
        for operation in ["GetTaxRegistration", "DeleteTaxRegistration"] {
            let case = format!("{operation} for {account_id}");
            let answer = call(&router, operation, &json!({ "accountId": account_id })).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {answer:?}"
            );
            assert!(
                answer
                    .get("message")
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected a message, got {:?}",
                answer.body
            );
            assert!(
                answer.body.get("errorCode").is_none(),
                "case {case:?}: the body must carry no errorCode, got {:?}",
                answer.body
            );
            assert!(
                !answer
                    .get("message")
                    .as_str()
                    .unwrap_or_default()
                    .contains(account_id),
                "case {case:?}: the message must not echo the account it was asked about, got {:?}",
                answer.body
            );
        }
    }
}

/// What a refusal's message must be.
enum Stated {
    /// The whole message, verbatim.
    Exact(String),
    /// The opening of the message; the rest varies with the model's value set.
    StartsWith(String),
    /// No validation envelope at all: the request satisfies every constraint.
    NoEnvelope,
}

/// One constraint-violation case: the request, the status it gets, what its
/// message must state, and the submitted value the message must not echo.
struct Message {
    name: &'static str,
    operation: &'static str,
    body: Value,
    status: u16,
    stated: Stated,
    never_echoed: Option<String>,
}

/// The envelope one violation is stated in.
fn one(clause: &str) -> Stated {
    Stated::Exact(format!("1 validation error detected: {clause}"))
}

/// A `TaxRegistrationEntry` that satisfies every constraint, for cases that vary
/// exactly one member away from valid.
fn valid_entry() -> Value {
    json!({
        "registrationId": "DE136695976",
        "registrationType": "VAT",
        "legalName": "Acme GmbH",
        "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
    })
}

/// A `PutTaxExemption` request whose certificate is named `document_name`.
///
/// The account is the caller's own, so a request the boundary admits reaches the
/// handler and is applied.
fn exemption_request(document_name: &str) -> Value {
    json!({
        "accountIds": [engine::emulator_account_id()],
        "authority": { "country": "US", "state": "CA" },
        "exemptionType": "Reseller",
        "exemptionCertificate": {
            "documentName": document_name,
            "documentFile": "aGVsbG8=",
        },
    })
}

/// A `PutTaxRegistration` body whose entry's `member` is `value`.
fn entry_with(member: &str, value: Value) -> Value {
    let mut entry = valid_entry();
    entry
        .as_object_mut()
        .expect("an object")
        .insert(member.to_string(), value);
    json!({ "accountId": "000000000001", "taxRegistrationEntry": entry })
}

/// The `@pattern` of `ExemptionCertificate$documentName`, as the model declares
/// it (`smithy_query(query="shape", shape="ExemptionCertificate")`), with the
/// `^` / `$` anchors the model spells it with.
const DOCUMENT_NAME_PATTERN: &str =
    r"^([A-Za-z0-9_.-]+)\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])$";

/// RULE: a constraint violation is stated in the standard envelope naming the
/// member path and the constraint — never the value that was refused, and with a
/// `@pattern` stated as the expression itself, without the anchors the model
/// spells it with.
#[tokio::test]
async fn a_constraint_violation_names_the_member_and_the_constraint_only() {
    // `documentName` is length 0..128 including its extension, so `.pdf`
    // leaves 124 characters at the bound and 125 one past it.
    let document_name_at_bound = format!("{}.pdf", "d".repeat(124));
    let document_name_past_bound = format!("{}.pdf", "d".repeat(125));
    let over_long_legal_name = "A".repeat(201);
    let cases = vec![
        // `ExemptionCertificate$documentName`: required, length 0..128,
        // @pattern (the cited case).
        Message {
            name: "certificate named with an extension the pattern does not admit",
            operation: "PutTaxExemption",
            body: exemption_request("sim2real-cert.txt"),
            status: 400,
            stated: one(
                "Value at 'exemptionCertificate.documentName' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: \
                 ([A-Za-z0-9_.-]+)\\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])",
            ),
            never_echoed: Some("sim2real-cert.txt".to_string()),
        },
        Message {
            name: "certificate with no extension at all",
            operation: "PutTaxExemption",
            body: exemption_request("certificate"),
            status: 400,
            stated: one(
                "Value at 'exemptionCertificate.documentName' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: \
                 ([A-Za-z0-9_.-]+)\\.([pP][dD][fF]|[jJ][pP][gG]|[pP][nN][gG])",
            ),
            never_echoed: Some("certificate".to_string()),
        },
        Message {
            name: "certificate name one past the 128-character bound",
            operation: "PutTaxExemption",
            body: exemption_request(&document_name_past_bound),
            status: 400,
            stated: one(
                "Value at 'exemptionCertificate.documentName' failed to satisfy constraint: \
                 Member must have length less than or equal to 128",
            ),
            never_echoed: Some("ddddd".to_string()),
        },
        // A name the pattern admits passes the boundary and reaches the
        // operation, which stores the exemption: a 200, never a refusal and no
        // validation envelope.
        Message {
            name: "certificate named with an upper-case extension the pattern admits",
            operation: "PutTaxExemption",
            body: exemption_request("certificate.PDF"),
            status: 200,
            stated: Stated::NoEnvelope,
            never_echoed: None,
        },
        Message {
            name: "certificate name at the 128-character bound",
            operation: "PutTaxExemption",
            body: exemption_request(&document_name_at_bound),
            status: 200,
            stated: Stated::NoEnvelope,
            never_echoed: None,
        },
        // `accountId`: length 12..12, @pattern ^\d+$.
        Message {
            name: "accountId of twelve characters with a non-digit",
            operation: "GetTaxRegistration",
            body: json!({ "accountId": "00000000000a" }),
            status: 400,
            stated: one(
                "Value at 'accountId' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: \\d+",
            ),
            never_echoed: Some("00000000000a".to_string()),
        },
        // Two violations of one member are stated in one envelope that counts
        // them.
        Message {
            name: "accountId that is neither twelve characters nor digits",
            operation: "GetTaxRegistration",
            body: json!({ "accountId": "abc" }),
            status: 400,
            stated: Stated::StartsWith("2 validation errors detected: ".to_string()),
            never_echoed: Some("abc".to_string()),
        },
        // `maxResults`: @range 1..=1000 — one past each published bound.
        Message {
            name: "maxResults one past the high bound",
            operation: "ListTaxRegistrations",
            body: json!({ "maxResults": 1001 }),
            status: 400,
            stated: one(
                "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
                 than or equal to 1000",
            ),
            never_echoed: Some("1001".to_string()),
        },
        Message {
            name: "maxResults one below the low bound",
            operation: "ListTaxRegistrations",
            body: json!({ "maxResults": 0 }),
            status: 400,
            stated: one(
                "Value at 'maxResults' failed to satisfy constraint: Member must have value \
                 greater than or equal to 1",
            ),
            never_echoed: None,
        },
        // An absent required member has no submitted value, so it is stated as
        // the null it was.
        Message {
            name: "taxRegistrationEntry omitted",
            operation: "PutTaxRegistration",
            body: json!({ "accountId": "000000000001" }),
            status: 400,
            stated: one(
                "Value null at 'taxRegistrationEntry' failed to satisfy constraint: Member must \
                 not be null",
            ),
            never_echoed: None,
        },
        // `legalName`: length 1..200.
        Message {
            name: "legalName one past the 200-character bound",
            operation: "PutTaxRegistration",
            body: entry_with("legalName", json!("A".repeat(201))),
            status: 400,
            stated: one(
                "Value at 'taxRegistrationEntry.legalName' failed to satisfy constraint: Member \
                 must have length less than or equal to 200",
            ),
            never_echoed: Some(over_long_legal_name.clone()),
        },
        // `certifiedEmailId`: @pattern, stated without its anchors like every
        // other one.
        Message {
            name: "certifiedEmailId that is not an email address",
            operation: "PutTaxRegistration",
            body: entry_with("certifiedEmailId", json!("not-an-email")),
            status: 400,
            stated: one(
                "Value at 'taxRegistrationEntry.certifiedEmailId' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: \
                 [A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\\.[A-Za-z]{2,20}",
            ),
            never_echoed: Some("not-an-email".to_string()),
        },
        // A closed enum states the value set it admits, and still not the value
        // it was given.
        Message {
            name: "registrationType outside the TaxRegistrationType enum",
            operation: "PutTaxRegistration",
            body: entry_with("registrationType", json!("VATT")),
            status: 400,
            stated: Stated::StartsWith(
                "1 validation error detected: Value at 'taxRegistrationEntry.registrationType' \
                 failed to satisfy constraint: Member must satisfy enum value set: ["
                    .to_string(),
            ),
            never_echoed: Some("'VATT'".to_string()),
        },
    ];

    for case in cases {
        let router = emulator();
        let answer = call(&router, case.operation, &case.body).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {} from {}, got {answer:?}",
            case.name, case.status, case.operation
        );
        let message = answer.get("message").as_str().unwrap_or_default().to_string();
        match &case.stated {
            Stated::Exact(expected) => assert_eq!(
                &message, expected,
                "case {:?}: expected the message {expected:?}, got {message:?}",
                case.name
            ),
            Stated::StartsWith(opening) => assert!(
                message.starts_with(opening.as_str()),
                "case {:?}: expected a message opening {opening:?}, got {message:?}",
                case.name
            ),
            Stated::NoEnvelope => assert!(
                !message.contains("validation error"),
                "case {:?}: expected no validation envelope, got {message:?}",
                case.name
            ),
        }
        if let Some(never_echoed) = &case.never_echoed {
            assert!(
                !message.contains(never_echoed.as_str()),
                "case {:?}: the message must not echo the refused value {never_echoed:?}, got \
                 {message:?}",
                case.name
            );
        }
        // Whatever the violation, a stated pattern is the expression itself.
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {:?}: a stated pattern must carry no ^ / $ anchors, got {message:?}",
            case.name
        );
    }

    // The anchors the test refuses above are the ones the model really spells
    // the pattern with, so the assertion is about a difference that exists.
    assert!(
        DOCUMENT_NAME_PATTERN.starts_with('^') && DOCUMENT_NAME_PATTERN.ends_with('$'),
        "the model's documentName pattern is anchored: {DOCUMENT_NAME_PATTERN:?}"
    );
}
