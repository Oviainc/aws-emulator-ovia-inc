// SPDX-License-Identifier: CC0-1.0

//! Which error class this service answers for an id that names NOTHING, end to
//! end over the restJson1 wire.
//!
//! `DeleteSupplementalTaxRegistration` models both `ValidationException` (400)
//! and `ResourceNotFoundException` (404)
//! (`smithy_query(query="operation", operation="DeleteSupplementalTaxRegistration")`),
//! so the model alone does not say which an unknown `authorityId` gets. The
//! recorded behaviour of the real service does, and these tables pin it so a
//! later reading of the two doc strings cannot flip it:
//!
//! * `DeleteSupplementalTaxRegistration`: an `authorityId` that does not name a
//!   live supplemental registration of the caller's account is refused with the
//!   modelled `ValidationException` (HTTP 400, `errorCode`
//!   `FieldValidationFailed`, `fieldList` naming the `authorityId` parameter) —
//!   junk strings
//!   and a syntactically valid UUID alike, on an empty store and on one holding
//!   another registration, for a registration that never existed and for one
//!   just deleted. Recorded units
//!   `taxsettings.delete-supplemental-tax-registration-not-found` and
//!   `taxsettings.concurrent-delete-supplemental-tax-registration-consistent`,
//!   quoted in
//!   `issues/things-that-now-work/20261023T110604Z-taxsettings-supplemental-trn-and-batch-tax-registration-write-paths.md`
//!   (cause 2), which is where this rule comes from.
//! * the operations addressed by an `accountId` — `GetTaxRegistration` and
//!   `DeleteTaxRegistration` — report an account with no stored registration
//!   with `ResourceNotFoundException` (HTTP 404) and a body carrying its
//!   `message` and no `errorCode`.
//!
//! The two classes are asserted against each other, so neither can be satisfied
//! by answering the same thing to both, and a refused call is always checked to
//! have changed nothing a later caller sees.
//!
//! The tables span the class each rule generalises over rather than one request:
//! the id shapes the existing table does not carry (non-ASCII, whitespace,
//! decimal digits), both published length bounds of `authorityId`
//! (`@length 1..200`) and one past each, every jurisdiction the supported
//! catalog names, an empty store against a populated one, and several accounts.
//!
//! The tables here carry ONLY what the existing suites do not: the id shapes
//! `deleting_by_an_authority_that_names_nothing_is_refused`
//! (`supplemental_tax_registrations.rs`) does not list, the `fieldList` and
//! message-invariance assertions, several registrations against each other, and
//! accounts `reading_an_account_with_no_registration_is_a_404`
//! (`tax_registration_lifecycle.rs`) does not name. Those two tests keep the
//! rows they already own; nothing is copied here.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs` and
//! the supplemental-TRN helpers in `supplemental_tax_registrations.rs`; nothing
//! here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ERROR_CODE_FIELD_VALIDATION_FAILED, SUPPLEMENTAL_TRN_COUNTRIES,
};

use crate::supplemental_tax_registrations::{emulator_supplementing, entry_for, list_all, put};
use crate::tax_registration_lifecycle::{call, emulator, Answer};

/// Assert one answer is the refusal an `authorityId` that names no live
/// supplemental registration gets: HTTP 400, `ValidationException`, the
/// service-defined `FieldValidationFailed` code and a `fieldList` naming the
/// member. Answers the message, which the caller checks does not vary with the
/// id asked about.
fn assert_unknown_authority(case: &str, answer: &Answer) -> String {
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.get("errorCode").as_str(),
        Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
        "case {case:?}: expected the field-validation code, got {:?}",
        answer.body
    );
    let fields: Vec<&str> = answer
        .get("fieldList")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        fields,
        // The wire spelling this service reports a refused parameter under:
        // the member's own name in CONSTANT_CASE, which is the form AWS answers
        // with (observed as `LEGAL_ADDRESS` for the legal-address refusal; see
        // `crate::state::parameter_name`'s doc). Written out rather than
        // computed, so a bug in that helper cannot move both sides of this
        // assertion at once.
        vec!["AUTHORITY_ID"],
        "case {case:?}: the refusal must name the member it is about, got {:?}",
        answer.body
    );
    let message = answer
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty(),
        "case {case:?}: expected a message, got {:?}",
        answer.body
    );
    message
}

/// The id shapes the `authorityId` member's pattern (`^[\s\S]*$`) admits that
/// `deleting_by_an_authority_that_names_nothing_is_refused` does not already
/// cover (it carries a junk string, a well-formed UUID, the 1- and
/// 200-character bounds and punctuation). None names a registration, so each
/// takes the same refusal as those do.
fn unknown_authority_ids() -> Vec<(&'static str, String)> {
    vec![
        ("non-ASCII", "Zürich-авторитет-権限".to_string()),
        ("whitespace", " ".to_string()),
        ("decimal digits", "4815162342".to_string()),
    ]
}

/// RULE: an `authorityId` that names no live supplemental registration is
/// refused with `ValidationException` (400, `FieldValidationFailed`, naming
/// `authorityId`) whatever its shape, on an empty store and on one holding
/// another registration, and the refused delete removes nothing.
#[tokio::test]
async fn an_authority_that_names_nothing_is_refused_the_same_way() {
    let supported = SUPPLEMENTAL_TRN_COUNTRIES[0].0;
    let mut messages = BTreeSet::new();

    for (shape, authority_id) in &unknown_authority_ids() {
        // Both store shapes the rule holds over: nothing stored at all, and a
        // live registration under a DIFFERENT authority.
        for stored_registrations in [0_usize, 1] {
            let case = format!("{shape} against {stored_registrations} stored");
            let (router, _engine) = emulator_supplementing();
            if stored_registrations == 1 {
                let stored = put(&router, &entry_for(supported, "CHE400000001", "Acme AG")).await;
                assert_eq!(
                    stored.status, 200,
                    "case {case:?}: setup write failed: {stored:?}"
                );
            }
            let answer = call(
                &router,
                "DeleteSupplementalTaxRegistration",
                &json!({ "authorityId": authority_id }),
            )
            .await;
            messages.insert(assert_unknown_authority(&case, &answer));
            assert_eq!(
                list_all(&router, None).await.len(),
                stored_registrations,
                "case {case:?}: a refused delete must remove nothing"
            );
        }
    }

    // The refusal is about the member, not about the value asked for, so it
    // reads the same whatever shape the id had — which also means it echoes
    // none of them back.
    assert_eq!(
        messages.len(),
        1,
        "the refusal must not vary with the id asked about, got {messages:?}"
    );
}

/// RULE: an `authorityId` outside its modelled constraints takes the same 400
/// `ValidationException` class, so the refusal does not depend on WHY the id is
/// unusable.
#[tokio::test]
async fn an_authority_outside_its_modelled_constraints_is_refused_too() {
    // `authorityId` is `@required` with `@length 1..200`
    // (`smithy_query(query="operation", operation="DeleteSupplementalTaxRegistration")`):
    // one character past each published bound, plus the member omitted. The
    // body of a boundary refusal is the request walk's, which states the
    // constraint that failed rather than this service's own field-validation
    // code, so the class and a stated reason are what is asserted.
    let cases: Vec<(&str, Value)> = vec![
        ("below the length bound", json!({ "authorityId": "" })),
        (
            "above the length bound",
            json!({ "authorityId": "a".repeat(201) }),
        ),
        ("the required member omitted", json!({})),
    ];
    for (case, body) in &cases {
        let router = emulator();
        let answer = call(&router, "DeleteSupplementalTaxRegistration", body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {answer:?}"
        );
        assert!(
            answer
                .get("message")
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: a refusal states what it refused, got {:?}",
            answer.body
        );
    }
}

/// RULE: the id a write issued stops naming a registration once it is deleted,
/// so the delete is idempotent in effect but not in answer: the first call
/// reports success and every later one takes the unknown-authority refusal.
///
/// Beyond the single-registration sequence
/// `deleting_by_an_authority_that_names_nothing_is_refused` already drives, this
/// holds SEVERAL registrations at once — one per supported jurisdiction — so it
/// also pins that deleting one leaves the others readable, and it checks the
/// refusal's `errorCode` and `fieldList` on the repeat deletes.
#[tokio::test]
async fn an_authority_stops_naming_a_registration_once_it_is_deleted() {
    // One registration per supported jurisdiction, each deleted and then
    // deleted twice more: the rule is about the id's lifetime, not about one
    // recorded id.
    let (router, _engine) = emulator_supplementing();
    let mut authority_ids = Vec::new();
    for (index, (country_code, _)) in SUPPLEMENTAL_TRN_COUNTRIES.iter().enumerate() {
        let stored = put(
            &router,
            &entry_for(
                country_code,
                &format!("CHE60000000{index}"),
                &format!("Acme {index} AG"),
            ),
        )
        .await;
        assert_eq!(stored.status, 200, "setup write {index} failed: {stored:?}");
        authority_ids.push(
            stored
                .get("authorityId")
                .as_str()
                .expect("the write reports an authorityId")
                .to_string(),
        );
    }

    let mut remaining = authority_ids.len();
    for (index, authority_id) in authority_ids.iter().enumerate() {
        let case = format!("registration {index}");
        // While the id names a registration, the listing carries it.
        assert!(
            list_all(&router, None)
                .await
                .iter()
                .any(|listed| listed.get("authorityId").and_then(Value::as_str)
                    == Some(authority_id.as_str())),
            "case {case:?}: the stored registration must read back before the delete"
        );
        let first = call(
            &router,
            "DeleteSupplementalTaxRegistration",
            &json!({ "authorityId": authority_id }),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: a live authority must delete, got {first:?}"
        );
        remaining -= 1;
        assert_eq!(
            list_all(&router, None).await.len(),
            remaining,
            "case {case:?}: the deleted registration must be gone and the others kept"
        );
        for repeat in 1..=2 {
            let again = call(
                &router,
                "DeleteSupplementalTaxRegistration",
                &json!({ "authorityId": authority_id }),
            )
            .await;
            assert_unknown_authority(&format!("{case}, delete {repeat}"), &again);
        }
    }
}

/// RULE: the not-found class belongs to the operations addressed by an
/// `accountId` — an account with no stored tax registration is reported with
/// `ResourceNotFoundException` (404), whose body carries its `message` and no
/// `errorCode` — so the two refusal classes stay distinct and neither answers
/// for the other.
#[tokio::test]
async fn an_account_with_no_registration_is_a_missing_resource() {
    let router = emulator();
    // Accounts the model admits (12 digits) that this store holds nothing for,
    // and that `reading_an_account_with_no_registration_is_a_404` in
    // `tax_registration_lifecycle.rs` does not name.
    for account_id in ["000000000003", "111122223333", "555555555555"] {
        for operation in ["GetTaxRegistration", "DeleteTaxRegistration"] {
            let case = format!("{operation} for {account_id}");
            let answer = call(&router, operation, &json!({ "accountId": account_id })).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected 404 ResourceNotFoundException, got {answer:?}"
            );
            assert!(
                answer.body.get("errorCode").is_none(),
                "case {case:?}: a not-found body carries no errorCode, got {:?}",
                answer.body
            );
            let message = answer.get("message").as_str().unwrap_or_default();
            assert!(
                !message.is_empty(),
                "case {case:?}: expected a message, got {:?}",
                answer.body
            );
        }
    }
}
