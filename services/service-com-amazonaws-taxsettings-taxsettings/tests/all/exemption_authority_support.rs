// SPDX-License-Identifier: CC0-1.0

//! `PutTaxExemption` against the jurisdictions the exemption type it names
//! applies in, and the inheritance indicator both exemption reads report, end to
//! end over the restJson1 wire.
//!
//! Two rules:
//!
//! * a submission that names a type the service PUBLISHES must name a
//!   jurisdiction that type applies in — the catalog's own
//!   `applicableJurisdictions` for that entry — and is otherwise refused with the
//!   modelled `ValidationException` (HTTP 400), the code for a field that failed
//!   validation, that refusal class's fixed text and `fieldList` naming the
//!   `authority`, recording nothing. A type the collection does not publish is
//!   not judged this way: the collection says nothing about where it applies.
//! * every account the exemption reads answer for carries
//!   `heritageObtainedDetails`, and it is false, because no operation this
//!   service models records an exemption inherited from a consolidated billing
//!   family's management account — reported by `ListTaxExemptions` and
//!   `BatchGetTaxExemptions` alike, and for an account holding no exemption as
//!   much as for one holding several.
//!
//! The first table is built FROM the published collection, so every entry of it
//! is driven — each with a jurisdiction it names, one it does not, the country
//! alone and a country it does not name — rather than the one entry an issue
//! cited. The second spans the evidence that an account is one these reads answer
//! for (the caller's own, a stored TRN, stored exemptions) against both read
//! paths.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs`, the
//! certificate builder and the map readers in `tax_exemptions.rs`; nothing here
//! builds a second copy of either.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    tax_exemption_type_catalog, EXEMPTION_AUTHORITY_NOT_SUPPORTED,
    ERROR_CODE_FIELD_VALIDATION_FAILED, TAX_EXEMPTION_TYPE_CATALOG,
};

use crate::tax_exemptions::{certificate, reported_exemptions};
use crate::tax_registration_lifecycle::{call, emulator, field_list};

/// The account every submission below names: the one this emulator presents as
/// the caller's own, so what each row is judged on is the type and the
/// jurisdiction.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// One published entry's jurisdictions: the `(country, state)` pairs the catalog
/// carries for it.
fn jurisdictions_of(index: usize) -> Vec<(String, Option<String>)> {
    let catalog = tax_exemption_type_catalog();
    let entry = catalog
        .get(index)
        .cloned()
        .unwrap_or_else(|| panic!("the published catalog has an entry {index}"));
    entry
        .applicable_jurisdictions
        .unwrap_or_default()
        .into_iter()
        .map(|authority| (authority.country, authority.state))
        .collect()
}

/// A state of the United States no entry of the published collection names, so a
/// submission claiming a published type there has nothing to be granted under.
///
/// Read from the collection rather than written down, so the row still asserts a
/// refusal if an entry is published for every state: the five states that levy no
/// general sales tax are the candidates (Alaska, Delaware, Montana, New
/// Hampshire and Oregon), and the first of them no entry carries is used.
fn a_state_no_entry_names(index: usize) -> String {
    let held = jurisdictions_of(index);
    ["AK", "DE", "MT", "NH", "OR"]
        .into_iter()
        .find(|candidate| {
            !held.iter().any(|(_, state)| {
                state
                    .as_deref()
                    .is_some_and(|state| state.eq_ignore_ascii_case(candidate))
            })
        })
        .map(std::string::ToString::to_string)
        .expect("one of the states that levy no sales tax is named by no entry")
}

/// Submit `exemption_type` in `authority` for the caller's own account, and
/// answer what the call said and what the account then holds.
async fn submit(exemption_type: &str, authority: &Value) -> (crate::tax_registration_lifecycle::Answer, Vec<Value>) {
    let router = emulator();
    let answer = call(
        &router,
        "PutTaxExemption",
        &json!({
            "accountIds": [account()],
            "authority": authority,
            "exemptionType": exemption_type,
            "exemptionCertificate": certificate("certificate.pdf"),
        }),
    )
    .await;
    let listed = call(&router, "ListTaxExemptions", &json!({})).await;
    let held = reported_exemptions(&listed, account());
    (answer, held)
}

/// Assert that the submission was refused with this refusal class's own
/// envelope, and that it recorded nothing.
fn assert_refused(
    case: &str,
    answer: &crate::tax_registration_lifecycle::Answer,
    held: &[Value],
) {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: the submission must be refused, got {answer:?}"
    );
    assert_eq!(
        (
            answer.get("errorCode").as_str(),
            answer.get("message").as_str(),
            field_list(answer),
        ),
        (
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            Some(EXEMPTION_AUTHORITY_NOT_SUPPORTED),
            vec!["AUTHORITY".to_string()],
        ),
        "case {case:?}: expected the refusal class's own envelope, got {:?}",
        answer.body
    );
    assert!(
        held.is_empty(),
        "case {case:?}: a refused submission records nothing, got {held:?}"
    );
}

/// RULE: a published type is granted only where the collection says it applies —
/// driven over EVERY entry of that collection, by the display name and by the
/// description, in the jurisdictions it names and in ones it does not.
#[tokio::test]
async fn a_published_type_is_granted_only_where_it_applies() {
    let catalog = tax_exemption_type_catalog();
    assert!(
        !catalog.is_empty(),
        "the service publishes a populated exemption-type catalog, got {catalog:?}"
    );

    for (index, entry) in catalog.iter().enumerate() {
        let display_name = entry
            .display_name
            .clone()
            .unwrap_or_else(|| panic!("entry {index} carries a display name"));
        let description = entry
            .description
            .clone()
            .unwrap_or_else(|| panic!("entry {index} carries a description"));
        let held = jurisdictions_of(index);
        let (country, state) = held
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("entry {index} applies somewhere"));
        let state = state.unwrap_or_else(|| panic!("entry {index} names the state it applies in"));
        let outside = a_state_no_entry_names(index);

        // Named by either identifying member, the jurisdiction is the one the
        // entry carries: admitted, and recorded.
        for (naming, token) in [
            ("its display name", display_name.as_str()),
            ("its description", description.as_str()),
        ] {
            let case = format!("{display_name} by {naming} in {country}/{state}");
            let (answer, recorded) = submit(token, &json!({ "country": country, "state": state })).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (200, ""),
                "case {case:?}: a jurisdiction the entry names must be admitted, got {answer:?}"
            );
            assert_eq!(
                recorded.len(),
                1,
                "case {case:?}: the submission must be recorded, got {recorded:?}"
            );
        }

        // The same state padded and re-cased is the same jurisdiction, because
        // neither distinguishes one from another.
        let case = format!("{display_name} in {country}/{state}, padded and re-cased");
        let (answer, recorded) = submit(
            &display_name,
            &json!({ "country": country.to_lowercase(), "state": format!("  {}  ", state.to_lowercase()) }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: padding and case name the same jurisdiction, got {answer:?}"
        );
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: the submission must be recorded, got {recorded:?}"
        );

        // The country alone claims the exemption for a country the entry does
        // apply in, so it is admitted.
        let case = format!("{display_name} in {country} alone");
        let (answer, recorded) = submit(&display_name, &json!({ "country": country })).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: the country the entry applies in must be admitted, got {answer:?}"
        );
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: the submission must be recorded, got {recorded:?}"
        );

        // A state of that country the entry does NOT name, and a country it does
        // not name at all: refused, and nothing recorded.
        let case = format!("{display_name} in {country}/{outside}");
        let (answer, recorded) =
            submit(&display_name, &json!({ "country": country, "state": outside })).await;
        assert_refused(&case, &answer, &recorded);

        let case = format!("{display_name} in another country");
        let (answer, recorded) = submit(&display_name, &json!({ "country": "DE" })).await;
        assert_refused(&case, &answer, &recorded);

        let case = format!("{display_name} in another country with a state");
        let (answer, recorded) =
            submit(&display_name, &json!({ "country": "CA", "state": "Ontario" })).await;
        assert_refused(&case, &answer, &recorded);
    }
}

/// RULE's other side: the collection says nothing about where a type it does NOT
/// publish applies, so such a submission is admitted in any jurisdiction the
/// model's own constraints allow — including the jurisdictions a published entry
/// is refused in.
#[tokio::test]
async fn a_type_the_collection_does_not_publish_is_admitted_anywhere() {
    let unpublished = "Exemption certificate this collection does not publish";
    let outside = a_state_no_entry_names(0);
    for (case, authority) in [
        ("a state no entry names", json!({ "country": "US", "state": outside })),
        ("another country", json!({ "country": "DE" })),
        ("another country with a state", json!({ "country": "CA", "state": "Ontario" })),
    ] {
        let (answer, recorded) = submit(unpublished, &authority).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: an unpublished type is judged by no collection, got {answer:?}"
        );
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: the submission must be recorded, got {recorded:?}"
        );
    }
}

/// Every published entry is covered by the table above, whatever the collection
/// holds: the table walks the collection itself, so an entry added to it is
/// driven rather than silently going untested.
#[test]
fn the_table_walks_the_whole_published_collection() {
    assert_eq!(
        tax_exemption_type_catalog().len(),
        TAX_EXEMPTION_TYPE_CATALOG.len(),
        "the projection reports every published row, got {} of {}",
        tax_exemption_type_catalog().len(),
        TAX_EXEMPTION_TYPE_CATALOG.len()
    );
    for (index, (display_name, _, jurisdictions)) in TAX_EXEMPTION_TYPE_CATALOG.iter().enumerate() {
        assert!(
            jurisdictions
                .iter()
                .any(|(_, state)| state.is_some()),
            "entry {index} ({display_name}) must name the states it applies in, got \
             {jurisdictions:?}"
        );
    }
}

/// RULE: every account the exemption reads answer for carries the inheritance
/// indicator, and it is false — on both read paths, whether or not the account
/// holds an exemption.
#[tokio::test]
async fn both_reads_report_the_inheritance_indicator_for_every_account() {
    let router = emulator();
    let with_registration = "000000000001";
    let with_exemption = "000000000002";

    let stored = call(
        &router,
        "PutTaxRegistration",
        &json!({
            "accountId": with_registration,
            "taxRegistrationEntry": {
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10117", "countryCode": "DE" },
            },
        }),
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "case \"setup\": the registration must store, got {stored:?}"
    );
    let submitted = call(
        &router,
        "PutTaxExemption",
        &json!({
            "accountIds": [with_exemption],
            "authority": { "country": "US" },
            "exemptionType": "Reseller",
            "exemptionCertificate": certificate("resale.pdf"),
        }),
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "case \"setup\": the exemption must record, got {submitted:?}"
    );

    // The rows differ along the evidence that an account is one these reads
    // answer for, and along whether it holds an exemption at all.
    let rows: &[(&str, &str, bool)] = &[
        ("the caller's own account, holding nothing", account(), false),
        ("an account holding only a registration", with_registration, false),
        ("an account holding an exemption", with_exemption, true),
    ];

    let listed = call(&router, "ListTaxExemptions", &json!({})).await;
    for (case, account_id, holds_an_exemption) in rows.iter().copied() {
        for (reader, answer) in [
            ("ListTaxExemptions", &listed),
            (
                "BatchGetTaxExemptions",
                &call(
                    &router,
                    "BatchGetTaxExemptions",
                    &json!({ "accountIds": [account_id] }),
                )
                .await,
            ),
        ] {
            let case = format!("{case} through {reader}");
            let details = answer
                .get("taxExemptionDetailsMap")
                .get(account_id)
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                details.get("heritageObtainedDetails"),
                Some(&json!(false)),
                "case {case:?}: the indicator is reported and false, got {details}"
            );
            // Nothing is inherited, so the members describing an inheritance
            // carry nothing.
            for member in ["heritageObtainedParentEntity", "heritageObtainedReason"] {
                assert!(
                    details.get(member).is_none(),
                    "case {case:?}: nothing was inherited, so {member} is not reported, got \
                     {details}"
                );
            }
            let exemptions = details
                .get("taxExemptions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                !exemptions.is_empty(),
                holds_an_exemption,
                "case {case:?}: expected the exemptions this service holds for it, got {details}"
            );
        }
    }
}
