// SPDX-License-Identifier: CC0-1.0

//! The bound on a caller string a benefit application PERSISTS that the model bounds not
//! at all, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — and every effect is read back through
//! `GetBenefitApplication` and `ListBenefitApplications`.
//!
//! The rule under test: a caller string a write persists that carries no modelled
//! constraint at all is accepted up to this service's own bound on a persisted caller
//! string (`MAX_DETAILS_VALUE_LEN` characters, the longest string member the model lets a
//! caller store on this resource) and refused 400 `ValidationException` past it, with
//! nothing written — on EVERY path that writes it, so no read can report a value another
//! write path would refuse.
//!
//! The rows span what the rule generalises over: which member carries the value (the two
//! the model leaves unconstrained, `Contact$Phone` and `FileInput$BusinessUseCase`, from
//! `smithy_query(query="shape",shape="Contact")` and `shape="FileInput"`), which operation
//! writes it (`CreateBenefitApplication`, `UpdateBenefitApplication`,
//! `AmendBenefitApplication`), the position relative to the bound (empty, at it, one past
//! it, far past it), the character width of the value (ASCII and multi-byte, since a
//! length counts characters) and which element of the list carries it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    MAX_DETAILS_VALUE_LEN, STATUS_PENDING_SUBMISSION, VALIDATION_REASON_FIELD,
};

use crate::amend_benefit_application::{amend_body, amendment, clause};
use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::service_error_rules::{assert_validation_invariants, message_of};

/// The requirement a value past this service's bound on a persisted caller string fails,
/// as this service words a length.
fn too_long() -> String {
    format!("Member must have length less than or equal to {MAX_DETAILS_VALUE_LEN}")
}

/// One member the model constrains not at all, in each form a request and a read names it.
struct Member {
    /// The member as an `Amendment$FieldPath` names it.
    field_path: &'static str,
    /// The `(PartnerContacts, FileDetails)` a write carries to put `value` in this member;
    /// the other list carries a value well inside the bound, so each row moves one member.
    lists: fn(&str) -> (Value, Value),
    /// The member path this service's refusal names when the create or the update supplies
    /// the value: the list member, the element's 1-based position and the collection step,
    /// then the member — the spelling the centralized walk uses for a nested member.
    refusal_path: &'static str,
    /// Where a read reports the stored value: `(list member, element index, member)`.
    reads: (&'static str, usize, &'static str),
}

/// The `PartnerContacts` every row that moves an attachment member carries: one contact,
/// which is as many as the modelled `Contacts` list admits (`@length(max: 1)`).
fn bounded_contacts() -> Value {
    json!([{ "Email": "ada@example.com", "Phone": "+1-555-0100" }])
}

/// The `FileDetails` every row that moves a contact member carries.
fn bounded_files() -> Value {
    json!([{ "FileURI": "s3://partner-bucket/plan.pdf", "BusinessUseCase": "funding request" }])
}

/// The two lists of a write carrying `value` as the contact's `Phone`.
fn phone_lists(value: &str) -> (Value, Value) {
    (
        json!([{ "Email": "ada@example.com", "Phone": value }]),
        bounded_files(),
    )
}

/// The two lists of a write carrying `value` as the attachment's `BusinessUseCase`.
fn use_case_lists(value: &str) -> (Value, Value) {
    (
        bounded_contacts(),
        json!([{ "FileURI": "s3://partner-bucket/plan.pdf", "BusinessUseCase": value }]),
    )
}

/// One create request body carrying `contacts` and `files`.
fn create_body(benefit_id: &str, token: &str, contacts: &Value, files: &Value) -> Value {
    json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "BenefitIdentifier": benefit_id,
        "Name": "unconstrained-member-bounds",
        "PartnerContacts": contacts,
        "FileDetails": files,
    })
}

/// One update request body carrying `contacts` and `files`.
fn update_body(id: &str, revision: &str, token: &str, contacts: &Value, files: &Value) -> Value {
    json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "Identifier": id,
        "Revision": revision,
        "PartnerContacts": contacts,
        "FileDetails": files,
    })
}

/// The ids `ListBenefitApplications` reports for [`CATALOG`], as a set — what a refused
/// create must leave exactly as it was.
async fn listed(harness: &Harness, case: &str) -> BTreeSet<String> {
    let out = harness
        .ok(case, "ListBenefitApplications", &json!({ "Catalog": CATALOG }))
        .await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {out:?}"))
        .iter()
        .map(|summary| text(case, summary, "Id"))
        .collect()
}

/// Assert `out` is this service's one-member `ValidationException` naming `path` and the
/// length requirement of [`too_long`].
fn assert_refused(case: &str, status: u16, out: &Value, path: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 past the bound on {path:?}, got {status} {out:?}"
    );
    assert_eq!(
        error_code(out),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {out:?}"
    );
    let message = message_of(out)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {out:?}"));
    assert_validation_invariants(case, &message);
    assert_eq!(
        message,
        clause(path, &too_long()),
        "case {case:?}: expected the message {:?}, got {message:?}",
        clause(path, &too_long())
    );
    assert_eq!(
        out["Reason"],
        json!(VALIDATION_REASON_FIELD),
        "case {case:?}: expected the field-validation reason, got {out:?}"
    );
}

/// Create one draft application carrying bounded values in both list members, and return
/// its id — the record the update and the amendment rows below write to.
async fn draft(harness: &Harness, case: &str, benefit_id: &str, token: &str) -> String {
    let created = harness
        .create(
            case,
            &create_body(benefit_id, token, &bounded_contacts(), &bounded_files()),
        )
        .await;
    text(case, &created, "Id")
}

/// Every path that writes one of the two unconstrained members applies the SAME bound to
/// it: a value up to this service's bound is stored and reported back by the create, the
/// update and the amendment alike, and one past it is refused 400 with the same length
/// requirement by all three, leaving the record exactly as it was.
#[tokio::test]
async fn every_write_path_applies_one_bound_to_an_unconstrained_persisted_member() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    let members: [Member; 2] = [
        Member {
            field_path: "PartnerContacts[0].Phone",
            lists: phone_lists,
            refusal_path: "partnerContacts.1.member.phone",
            reads: ("PartnerContacts", 0, "Phone"),
        },
        Member {
            field_path: "FileDetails[0].BusinessUseCase",
            lists: use_case_lists,
            refusal_path: "fileDetails.1.member.businessUseCase",
            reads: ("FileDetails", 0, "BusinessUseCase"),
        },
    ];

    // (case, the value, whether it is inside the bound) — the shortest value the member
    // admits, the longest in each character width, and one and many characters past it.
    let values: Vec<(&str, String, bool)> = vec![
        ("an empty value", String::new(), true),
        (
            "an ascii value at the bound",
            "5".repeat(MAX_DETAILS_VALUE_LEN),
            true,
        ),
        (
            "a multi-byte value at the bound",
            "é".repeat(MAX_DETAILS_VALUE_LEN),
            true,
        ),
        (
            "an ascii value one character past the bound",
            "5".repeat(MAX_DETAILS_VALUE_LEN + 1),
            false,
        ),
        (
            "a multi-byte value one character past the bound",
            "é".repeat(MAX_DETAILS_VALUE_LEN + 1),
            false,
        ),
        (
            "a value far past the bound",
            "5".repeat(10 * MAX_DETAILS_VALUE_LEN),
            false,
        ),
    ];

    for (position, member) in members.iter().enumerate() {
        let (list, index, leaf) = member.reads;
        let id = draft(
            &harness,
            member.field_path,
            &benefit_id,
            &format!("tok-draft-{position}"),
        )
        .await;

        for (row, (label, value, inside)) in values.iter().enumerate() {
            let case = format!("{}, {label}", member.field_path);
            let (contacts, files) = (member.lists)(value);
            let listed_before = listed(&harness, &case).await;
            let before = harness.get(&case, &id).await;

            // 1. the create, which supplies the whole list
            let (status, out) = harness
                .call(
                    "CreateBenefitApplication",
                    &create_body(
                        &benefit_id,
                        &format!("tok-create-{position}-{row}"),
                        &contacts,
                        &files,
                    ),
                )
                .await;
            if *inside {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a value inside the bound must be created, \
                     got {status} {out:?}"
                );
                let created = harness.get(&case, &text(&case, &out, "Id")).await;
                assert_eq!(
                    created[list][index][leaf],
                    json!(value),
                    "case {case:?}: the created value must round-trip, got {created:?}"
                );
            } else {
                assert_refused(&case, status, &out, member.refusal_path);
                assert_eq!(
                    listed(&harness, &case).await,
                    listed_before,
                    "case {case:?}: a refused create must store no application"
                );
            }

            // 2. the update, which supplies the same two lists for an existing record
            let revision = text(&case, &before, "Revision");
            let (status, out) = harness
                .call(
                    "UpdateBenefitApplication",
                    &update_body(
                        &id,
                        &revision,
                        &format!("tok-update-{position}-{row}"),
                        &contacts,
                        &files,
                    ),
                )
                .await;
            if *inside {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a value inside the bound must be updated, \
                     got {status} {out:?}"
                );
                let updated = harness.get(&case, &id).await;
                assert_eq!(
                    updated[list][index][leaf],
                    json!(value),
                    "case {case:?}: the updated value must round-trip, got {updated:?}"
                );
            } else {
                assert_refused(&case, status, &out, member.refusal_path);
                let after = harness.get(&case, &id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused update must change nothing, got {after:?}"
                );
            }

            // 3. the amendment, which writes the one member a `FieldPath` names — the
            // path that already applied the bound, stating the same requirement about the
            // value it was handed.
            let revision = text(&case, &harness.get(&case, &id).await, "Revision");
            let amended = harness.get(&case, &id).await;
            let (status, out) = harness
                .call(
                    "AmendBenefitApplication",
                    &amend_body(
                        &id,
                        &revision,
                        &format!("tok-amend-{position}-{row}"),
                        json!([amendment(member.field_path, value)]),
                    ),
                )
                .await;
            if *inside {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a value inside the bound must be amended in, \
                     got {status} {out:?}"
                );
                let after = harness.get(&case, &id).await;
                assert_eq!(
                    after[list][index][leaf],
                    json!(value),
                    "case {case:?}: the amended value must round-trip, got {after:?}"
                );
            } else {
                assert_refused(&case, status, &out, "amendments.1.member.newValue");
                let after = harness.get(&case, &id).await;
                assert_eq!(
                    after, amended,
                    "case {case:?}: a refused amendment must change nothing, got {after:?}"
                );
            }

            // The draft is still a draft whatever the rows above answered, so the next row
            // writes to the same record.
            let after = harness.get(&case, &id).await;
            assert_eq!(
                after["Status"],
                json!(STATUS_PENDING_SUBMISSION),
                "case {case:?}: the draft must still be updatable, got {after:?}"
            );
        }
    }
}

/// The refusal names the ELEMENT and the member a caller must fix: the element's 1-based
/// position inside the list member, whichever attachment of the list carries the
/// over-long value — and the refused create spends no `ClientToken`, so the same token
/// carrying bounded values still creates.
#[tokio::test]
async fn the_refusal_names_the_element_that_carries_the_over_long_value() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");

    let over_long = "u".repeat(MAX_DETAILS_VALUE_LEN + 1);
    let ok = json!({ "FileURI": "s3://partner-bucket/ok.pdf", "BusinessUseCase": "fine" });
    let bad = json!({
        "FileURI": "s3://partner-bucket/over.pdf",
        "BusinessUseCase": over_long.clone(),
    });

    // (case, the attachments the request carries, the element the refusal must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "the only attachment",
            json!([bad.clone()]),
            "fileDetails.1.member.businessUseCase",
        ),
        (
            "the second of two attachments",
            json!([ok.clone(), bad.clone()]),
            "fileDetails.2.member.businessUseCase",
        ),
        (
            "the first of two attachments",
            json!([bad.clone(), ok.clone()]),
            "fileDetails.1.member.businessUseCase",
        ),
        (
            "the third of three attachments",
            json!([ok.clone(), ok.clone(), bad.clone()]),
            "fileDetails.3.member.businessUseCase",
        ),
    ];

    for (row, (case, files, path)) in rows.iter().enumerate() {
        let token = format!("tok-element-{row}");
        let listed_before = listed(&harness, case).await;
        let (status, out) = harness
            .call(
                "CreateBenefitApplication",
                &create_body(&benefit_id, &token, &bounded_contacts(), files),
            )
            .await;
        assert_refused(case, status, &out, path);
        assert_eq!(
            listed(&harness, case).await,
            listed_before,
            "case {case:?}: a refused create must store no application"
        );

        // The refusal spent nothing: the same token, with every attachment inside the
        // bound, creates the application it would have created.
        let created = harness
            .create(
                case,
                &create_body(
                    &benefit_id,
                    &token,
                    &bounded_contacts(),
                    &json!([ok.clone()]),
                ),
            )
            .await;
        let stored = harness.get(case, &text(case, &created, "Id")).await;
        assert_eq!(
            stored["FileDetails"][0]["BusinessUseCase"],
            json!("fine"),
            "case {case:?}: the bounded attachment must be stored, got {stored:?}"
        );
    }
}
