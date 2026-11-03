// SPDX-License-Identifier: CC0-1.0

//! What `PutTaxExemption` admits as the type a submission names, end to end over
//! the restJson1 wire.
//!
//! The rule under test: `PutTaxExemptionRequest$exemptionType` is a free-form
//! string the model constrains only in length (1..=200) and by a pattern that
//! admits every character, and `TaxExemptionType` is a structure of free text
//! rather than an enumeration (`smithy_query(query="operation",
//! operation="PutTaxExemption")` and `smithy_query(query="shape",
//! shape="TaxExemptionType")`) — the model publishes no set of legal values. So a
//! submission is admitted on the constraints the model states and the token is
//! recorded as the caller spelled it: as the entry of the collection
//! `GetTaxExemptionTypes` publishes when it names one (either of that entry's two
//! identifying members, in any case or padding), and otherwise as the token
//! itself, applicable in the jurisdiction the submission named.
//!
//! The table below spans that class along what the rule generalises over: whether
//! the token names a published entry and by which member, its length against both
//! modelled bounds (one character, the 50 of `TaxExemptionType$displayName`, one
//! past it, and the 200 of `exemptionType`), and the characters it carries
//! (letters, digits, punctuation, non-ASCII). Every row is read BACK through
//! `BatchGetTaxExemptions` on a second mount of the service, so what it asserts
//! came from the service's state.
//!
//! The harness (`emulator_with_engine`, `mount_on`, `call`) lives in
//! `tax_registration_lifecycle.rs`, and the certificate a submission attaches
//! and the reader of a reported exemption in `tax_exemptions.rs`; nothing here
//! builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    find_exemption_type, tax_exemption_type_catalog,
};

use crate::tax_exemptions::{certificate, reported_exemptions};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, mount_on};

/// The longest `TaxExemptionType$displayName` the model admits: `@length` 0..50
/// (`smithy_query(query="shape", shape="TaxExemptionType")`).
const DISPLAY_NAME_BOUND: usize = 50;

/// The longest `exemptionType` a submission may name: `@length` 1..=200
/// (`smithy_query(query="operation", operation="PutTaxExemption")`).
const EXEMPTION_TYPE_BOUND: usize = 200;

/// The `TaxExemptionType` a submission of `token` in `stored_authority` must
/// read back as, by the rule: the published entry the token names, or the token
/// itself — its own display name where the member's bound admits it, applicable
/// in the jurisdiction the submission named.
fn expected_type(token: &str, stored_authority: &Value) -> Value {
    if let Some((display_name, description, jurisdictions)) = find_exemption_type(token) {
        let authorities: Vec<Value> = jurisdictions
            .iter()
            .map(|(country, state)| {
                let mut authority = json!({ "country": country });
                if let Some(state) = state {
                    authority["state"] = json!(state);
                }
                authority
            })
            .collect();
        return json!({
            "displayName": display_name,
            "description": description,
            "applicableJurisdictions": authorities,
        });
    }
    let mut entry = json!({
        "description": token,
        "applicableJurisdictions": [stored_authority],
    });
    if token.chars().count() <= DISPLAY_NAME_BOUND {
        entry["displayName"] = json!(token);
    }
    entry
}

/// The rows the admission table submits: a case name and the `exemptionType` the
/// request names.
///
/// The two rows that name a published entry take their token FROM the collection
/// the service publishes, so the table covers whatever that collection holds
/// rather than restating its wording.
fn submitted_tokens() -> Vec<(String, String)> {
    let catalog = tax_exemption_type_catalog();
    let published = catalog
        .first()
        .cloned()
        .expect("the service publishes at least one exemption type");
    let description = published
        .description
        .clone()
        .expect("a published entry carries a description");
    let display_name = published
        .display_name
        .clone()
        .expect("a published entry carries a display name");

    vec![
        (
            "a published entry named by its description".to_string(),
            description,
        ),
        (
            "a published entry named by its display name, padded and re-cased".to_string(),
            format!("  {}  ", display_name.to_uppercase()),
        ),
        (
            "a description the collection does not publish".to_string(),
            "Sales tax exemption certificate held for resale".to_string(),
        ),
        ("one character".to_string(), "x".to_string()),
        (
            format!("a token at the display-name bound of {DISPLAY_NAME_BOUND} characters"),
            "T".repeat(DISPLAY_NAME_BOUND),
        ),
        (
            "a token one character past the display-name bound".to_string(),
            "T".repeat(DISPLAY_NAME_BOUND + 1),
        ),
        (
            format!("a token at the modelled bound of {EXEMPTION_TYPE_BOUND} characters"),
            "T".repeat(EXEMPTION_TYPE_BOUND),
        ),
        (
            "a token carrying non-ASCII letters".to_string(),
            "Befreiung für Wiederverkäufer".to_string(),
        ),
        (
            "a token carrying digits and punctuation".to_string(),
            "501(c)(19) - veterans' organization".to_string(),
        ),
    ]
}

/// RULE: every `exemptionType` the model admits is recorded, and read back as
/// the rule records it — the published entry the token names, or the token
/// itself.
#[tokio::test]
async fn every_exemption_type_the_model_admits_is_recorded_and_read_back() {
    let account = "000000000021";
    for (case, token) in submitted_tokens() {
        let (writer, engine) = emulator_with_engine();
        let answer = call(
            &writer,
            "PutTaxExemption",
            &json!({
                "accountIds": [account],
                "authority": { "country": "US", "state": "OH" },
                "exemptionType": token,
                "exemptionCertificate": certificate("certificate.pdf"),
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the submission to be applied, got {answer:?}"
        );
        let case_id = answer
            .get("caseId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a caseId, got {answer:?}"))
            .to_string();
        assert!(
            (1..=200).contains(&case_id.chars().count()),
            "case {case:?}: caseId is a 1..200 string, got {case_id:?}"
        );

        // A SECOND mount of the service on the same engine, constructed after
        // the write: anything it reports came from the service's state.
        let reader = mount_on(&engine);
        let read = call(
            &reader,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": [account] }),
        )
        .await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the record to be readable, got {read:?}"
        );
        let held = reported_exemptions(&read, account);
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected the one submitted exemption, got {held:?}"
        );
        let stored_authority = json!({ "country": "US", "state": "OH" });
        let reported = held[0]
            .get("taxExemptionType")
            .cloned()
            .unwrap_or(Value::Null);
        let expected = expected_type(&token, &stored_authority);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the exemption held under {expected}, got {reported}"
        );
        assert_eq!(
            held[0].get("authority").cloned().unwrap_or(Value::Null),
            stored_authority,
            "case {case:?}: expected the submitted jurisdiction, got {:?}",
            held[0].get("authority")
        );

        // And what it reports stays inside the constraints the model puts on the
        // members it carries.
        let description = reported
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a description, got {reported}"));
        assert!(
            (1..=EXEMPTION_TYPE_BOUND).contains(&description.chars().count()),
            "case {case:?}: description is a 1..200 string, got {description:?}"
        );
        if let Some(display_name) = reported.get("displayName").and_then(Value::as_str) {
            assert!(
                display_name.chars().count() <= DISPLAY_NAME_BOUND,
                "case {case:?}: displayName is a 0..50 string, got {display_name:?}"
            );
        }
    }
}

/// RULE: the exemption's identity is the type it is recorded under and its
/// jurisdiction, so a token the published collection does not name identifies a
/// record exactly as one it does: submitting the same token in the same
/// jurisdiction again replaces that record, and another token adds one beside it.
#[tokio::test]
async fn an_unpublished_token_identifies_one_record_per_jurisdiction() {
    let account = "000000000022";
    // The rows differ along what makes a submission a different exemption: the
    // state of the jurisdiction, the token, and neither (a repeat, re-cased and
    // padded).
    let submissions: &[(&str, &str, Value, usize)] = &[
        (
            "a token the collection does not name",
            "Exemption certificate on file",
            json!({ "country": "US" }),
            1,
        ),
        (
            "the same token in another state",
            "Exemption certificate on file",
            json!({ "country": "US", "state": "OH" }),
            2,
        ),
        (
            "the same token in the same state again, re-cased and padded",
            "  EXEMPTION CERTIFICATE ON FILE  ",
            json!({ "country": "US", "state": " oh " }),
            2,
        ),
        (
            "another token in the first jurisdiction",
            "Exemption certificate pending",
            json!({ "country": "US" }),
            3,
        ),
    ];

    let (router, engine) = emulator_with_engine();
    for (case, token, authority, expected_held) in submissions {
        let answer = call(
            &router,
            "PutTaxExemption",
            &json!({
                "accountIds": [account],
                "authority": authority,
                "exemptionType": token,
                "exemptionCertificate": certificate("certificate.pdf"),
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the submission to be applied, got {answer:?}"
        );

        let reader = mount_on(&engine);
        let read = call(
            &reader,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": [account] }),
        )
        .await;
        let held = reported_exemptions(&read, account);
        assert_eq!(
            held.len(),
            *expected_held,
            "case {case:?}: expected {expected_held} record(s), got {held:?}"
        );
    }
}
