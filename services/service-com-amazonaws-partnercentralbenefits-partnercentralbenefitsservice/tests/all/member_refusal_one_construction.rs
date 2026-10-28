// SPDX-License-Identifier: CC0-1.0

//! The single construction of a one-member `ValidationException`, driven end to end over
//! the wire through an operation from each former hand-rolled copy.
//!
//! The rule under test: a refusal of ONE input member is built by one constructor from the
//! (member, requirement, reason) triple — `state::member_validation_exception` — so its
//! message (the member clause under the AWS validation envelope), its
//! `ValidationExceptionReason` and its one-member `FieldList` move together on every
//! operation that answers one, and no handler can carry its own wiring of the three.
//!
//! The table spans the class rather than one call: every path that used to compose the
//! exception by hand — the benefit's request schema and the benefit's status (through
//! `CreateBenefitApplication`), the reserved tag-key namespace (through `TagResource` and
//! `UntagResource`, which name different members), an unamendable `FieldPath` (through
//! `AmendBenefitApplication`, at two positions in the amendment list, so the member path
//! varies) and the association bound (through `AssociateBenefitApplicationResource`) —
//! crossed with both `ValidationExceptionReason` values this service answers with. The
//! document guards on `BenefitApplicationDetails` (through `CreateBenefitApplication`) are
//! in the table too: they are the same composition, so they went through the one
//! constructor with the rest.
//!
//! Each row's expected body is taken from the constructor itself, so a change to what it
//! builds changes what every row must answer rather than leaving one of them behind.

use serde_json::{json, Map, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    member_validation_exception, reserved_tag_key_requirement, BENEFIT_IDENTIFIER_MEMBER,
    BENEFIT_STATUS_ACTIVE, DETAILS_MEMBER, MAX_ASSOCIATED_RESOURCES_PER_APPLICATION,
    MAX_DETAILS_MEMBERS, SEEDED_CATALOGS, VALIDATION_CODE_INVALID_VALUE,
    VALIDATION_REASON_BUSINESS, VALIDATION_REASON_FIELD,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::disassociate_benefit_application_resource::{
    body as association_body, seed as seed_with_resources,
};
use crate::resource_tagging::tag_payload;
use crate::service_error_rules::{assert_validation_invariants, message_of};

/// The requirement an `AmendBenefitApplication` refusal of a `FieldPath` naming no
/// amendable field states, as that operation words it.
const UNAMENDABLE: &str = "Member must name one of the amendable fields (Name, Description, \
                           BenefitApplicationDetails), or a `.`-separated path into \
                           BenefitApplicationDetails";

/// A tag key in the AWS-reserved namespace, which no caller may supply.
const RESERVED_KEY: &str = "aws:createdBy";

/// One row of the table: the call to make, and the triple the one constructor must build
/// the answer from.
struct Row {
    case: String,
    operation: &'static str,
    body: Value,
    /// The input member the refusal names, spelled as the wire spells it.
    member: String,
    /// The requirement that member failed.
    requirement: String,
    /// The `ValidationExceptionReason` the refusal class carries.
    reason: &'static str,
}

/// The `FieldList` of an error body as `(Name, Message, Code)` triples, in the order the
/// service reported them.
fn field_list(case: &str, body: &Value) -> Vec<(String, String, Option<String>)> {
    body["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no FieldList: {body:?}"))
        .iter()
        .map(|field| {
            (
                text(case, field, "Name"),
                text(case, field, "Message"),
                field["Code"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// A value whose JSON type is NOT the one `json_type` names, so a schema declaring that
/// type at a position refuses it there.
fn value_of_another_type(json_type: &str) -> Value {
    if json_type == "string" {
        json!(7)
    } else {
        json!("not of the declared type")
    }
}

/// A details document holding one member more than this service stores, so the document
/// guard on the member refuses it.
fn oversized_details() -> Map<String, Value> {
    (0..=MAX_DETAILS_MEMBERS)
        .map(|index| (format!("field{index}"), json!("v")))
        .collect()
}

/// The first `ACTIVE` benefit of `catalog` whose stored `BenefitRequestSchema` declares a
/// field: `(benefit id, field name, the JSON type the schema declares for it)`.
///
/// Read out of the catalog over the wire rather than written here, so the row follows the
/// seeded catalog instead of breaking when it changes.
async fn schema_field(harness: &Harness, catalog: &str) -> (String, String, String) {
    for summary in harness.benefits(catalog).await {
        if summary["Status"] != json!(BENEFIT_STATUS_ACTIVE) {
            continue;
        }
        let id = text(catalog, &summary, "Id");
        let got = harness
            .ok(
                &id,
                "GetBenefit",
                &json!({ "Catalog": catalog, "Identifier": id }),
            )
            .await;
        let Some(properties) = got["BenefitRequestSchema"]["properties"].as_object() else {
            continue;
        };
        for (field, property) in properties {
            if let Some(json_type) = property["type"].as_str() {
                return (id, field.clone(), json_type.to_string());
            }
        }
    }
    panic!("case {catalog:?}: no ACTIVE benefit of this catalog declares a request-schema field");
}

/// A benefit the catalog no longer offers: `(its catalog, its id, its `BenefitStatus`)`.
pub(crate) async fn withdrawn_benefit(harness: &Harness) -> (&'static str, String, String) {
    for catalog in SEEDED_CATALOGS {
        for summary in harness.benefits(catalog).await {
            let status = text(catalog, &summary, "Status");
            if status != BENEFIT_STATUS_ACTIVE {
                return (catalog, text(catalog, &summary, "Id"), status);
            }
        }
    }
    panic!("the seeded catalogs must hold a benefit that is not ACTIVE");
}

/// One `AmendBenefitApplication` request body carrying `amendments`.
fn amend_body(identifier: &str, revision: &str, token: &str, amendments: Value) -> Value {
    json!({
        "Catalog": CATALOG,
        "ClientToken": token,
        "Identifier": identifier,
        "Revision": revision,
        "AmendmentReason": "the partner corrected the application",
        "Amendments": amendments,
    })
}

/// One `(FieldPath, NewValue)` amendment.
fn amendment(field_path: &str, new_value: &str) -> Value {
    json!({ "FieldPath": field_path, "NewValue": new_value })
}

/// Every operation that refuses one input member answers exactly the body the single
/// constructor builds from that member, its requirement and its reason.
#[tokio::test]
async fn every_member_refusal_is_the_one_construction() {
    let harness = Harness::new();
    let (schema_benefit, schema_declared_field, schema_json_type) =
        schema_field(&harness, CATALOG).await;
    let (withdrawn_catalog, withdrawn_id, withdrawn_status) = withdrawn_benefit(&harness).await;
    let active_benefit = text("subject", &harness.active_benefit().await, "Id");

    // One application the amendment and the tagging rows act on.
    let subject = harness
        .create(
            "subject",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-one-construction-subject",
                "BenefitIdentifier": active_benefit,
                "Name": "one-construction",
                "Description": "the application the refusal table is asserted on",
            }),
        )
        .await;
    let subject_id = text("subject", &subject, "Id");
    let subject_revision = text("subject", &subject, "Revision");
    let subject_arn = text("subject", &subject, "Arn");

    // One application whose association list is already filled to the store's bound, so
    // the next association is the one past it.
    let filled: Vec<String> = (0..MAX_ASSOCIATED_RESOURCES_PER_APPLICATION)
        .map(|slot| format!("arn:aws:s3:::one-construction-{slot}"))
        .collect();
    let stored: Vec<&str> = filled.iter().map(String::as_str).collect();
    let full_id = seed_with_resources(
        &harness,
        "a record filled to the bound",
        CATALOG,
        "tok-one-construction-full",
        &stored,
    )
    .await;

    // A details document putting a value of the wrong JSON type where the benefit's
    // schema declares one.
    let mut details = Map::new();
    details.insert(
        schema_declared_field.clone(),
        value_of_another_type(&schema_json_type),
    );

    let rows = vec![
        Row {
            case: "a details field contradicting the benefit's request schema".to_string(),
            operation: "CreateBenefitApplication",
            body: json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-one-construction-schema",
                "BenefitIdentifier": schema_benefit,
                "Name": "one-construction",
                "BenefitApplicationDetails": Value::Object(details),
            }),
            member: format!("{DETAILS_MEMBER}.{schema_declared_field}"),
            requirement: format!("Member must be of type {schema_json_type}"),
            reason: VALIDATION_REASON_FIELD,
        },
        Row {
            case: format!("an application for a {withdrawn_status} benefit"),
            operation: "CreateBenefitApplication",
            body: json!({
                "Catalog": withdrawn_catalog,
                "ClientToken": "tok-one-construction-withdrawn",
                "BenefitIdentifier": withdrawn_id,
                "Name": "one-construction",
            }),
            member: BENEFIT_IDENTIFIER_MEMBER.to_string(),
            requirement: format!(
                "Member must name a benefit the catalog offers, which a {withdrawn_status} \
                 benefit is not"
            ),
            reason: VALIDATION_REASON_BUSINESS,
        },
        Row {
            case: "a details document past the member bound this service stores".to_string(),
            operation: "CreateBenefitApplication",
            body: json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-one-construction-details",
                "BenefitIdentifier": active_benefit,
                "Name": "one-construction",
                "BenefitApplicationDetails": Value::Object(oversized_details()),
            }),
            member: DETAILS_MEMBER.to_string(),
            requirement: format!("Member must hold no more than {MAX_DETAILS_MEMBERS} members"),
            reason: VALIDATION_REASON_FIELD,
        },
        Row {
            case: "a reserved tag key supplied to TagResource".to_string(),
            operation: "TagResource",
            body: json!({
                "resourceArn": subject_arn,
                "tags": tag_payload(&[(RESERVED_KEY, "mine")]),
            }),
            member: "tags".to_string(),
            requirement: reserved_tag_key_requirement(),
            reason: VALIDATION_REASON_FIELD,
        },
        Row {
            case: "a reserved tag key named to UntagResource".to_string(),
            operation: "UntagResource",
            body: json!({ "resourceArn": subject_arn, "tagKeys": [RESERVED_KEY] }),
            member: "tagKeys".to_string(),
            requirement: reserved_tag_key_requirement(),
            reason: VALIDATION_REASON_FIELD,
        },
        Row {
            case: "an unamendable path in the first amendment".to_string(),
            operation: "AmendBenefitApplication",
            body: amend_body(
                &subject_id,
                &subject_revision,
                "tok-one-construction-amend-first",
                json!([amendment("Programs", "would-not")]),
            ),
            member: "amendments.1.member.fieldPath".to_string(),
            requirement: UNAMENDABLE.to_string(),
            reason: VALIDATION_REASON_FIELD,
        },
        Row {
            case: "an unamendable path in the second amendment".to_string(),
            operation: "AmendBenefitApplication",
            body: amend_body(
                &subject_id,
                &subject_revision,
                "tok-one-construction-amend-second",
                json!([
                    amendment("Name", "would-apply"),
                    amendment("Programs", "would-not"),
                ]),
            ),
            member: "amendments.2.member.fieldPath".to_string(),
            requirement: UNAMENDABLE.to_string(),
            reason: VALIDATION_REASON_FIELD,
        },
        Row {
            case: "an association past the store's bound".to_string(),
            operation: "AssociateBenefitApplicationResource",
            body: association_body(CATALOG, &full_id, "arn:aws:s3:::one-construction-past"),
            member: "associatedResources".to_string(),
            requirement: format!(
                "Member must have length less than or equal to \
                 {MAX_ASSOCIATED_RESOURCES_PER_APPLICATION}"
            ),
            reason: VALIDATION_REASON_FIELD,
        },
    ];

    // Both reasons this service answers a member refusal with are covered, so the table
    // cannot shrink to one of them.
    for reason in [VALIDATION_REASON_FIELD, VALIDATION_REASON_BUSINESS] {
        assert!(
            rows.iter().any(|row| row.reason == reason),
            "case \"table\": expected a row carrying the {reason:?} reason, got {:?}",
            rows.iter().map(|row| row.reason).collect::<Vec<_>>()
        );
    }

    for row in &rows {
        let case = &row.case;
        let expected = member_validation_exception(&row.member, &row.requirement, row.reason);
        let expected_fields: Vec<(String, String, Option<String>)> = expected
            .field_list
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|field| (field.name, field.message, field.code))
            .collect();

        let (status, out) = harness.call(row.operation, &row.body).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {}: expected the modelled ValidationException's 400, got {status} \
             with {out:?}",
            row.operation
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?} on {}: expected ValidationException, got {out:?}",
            row.operation
        );
        let message = message_of(&out).unwrap_or_else(|| {
            panic!(
                "case {case:?} on {}: the refusal carries no modelled Message: {out:?}",
                row.operation
            )
        });
        assert_eq!(
            message, expected.message,
            "case {case:?} on {}: expected the constructor's message {:?}, got {message:?}",
            row.operation, expected.message
        );
        assert_validation_invariants(case, &message);
        assert_eq!(
            out["Reason"],
            json!(expected.reason),
            "case {case:?} on {}: expected the reason {:?}, got {out:?}",
            row.operation,
            expected.reason
        );
        assert_eq!(
            field_list(case, &out),
            expected_fields,
            "case {case:?} on {}: expected the constructor's field list {expected_fields:?}, \
             got {out:?}",
            row.operation
        );
    }
}

/// The construction itself: whatever member, requirement and reason it is handed, it
/// renders the member clause under the single-violation envelope, carries that reason, and
/// names the member once in its `FieldList` under the invalid-value code.
///
/// The member it NAMES is the path the centralized walk reports for that input member —
/// the member in lowerCamel, the steps a handler addressed below it echoed as sent — so one
/// member has one spelling whether a modelled constraint or a handler's own rule refused
/// it. Each row therefore states the member as the WIRE spells it and the path the refusal
/// must carry.
///
/// The rows differ along what the rule generalises over: the shape of the member path (a
/// top-level member, a path into a document, an element path), the case the wire member is
/// spelled in, the length and wording of the requirement, and each
/// `ValidationExceptionReason` the service answers with.
#[test]
fn the_constructor_renders_the_clause_the_reason_and_the_field_list() {
    // A requirement far longer than any one clause of this service, so the rendering is
    // pinned on a long one too.
    let long_requirement = "requirement ".repeat(40);
    let rows: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a top-level member the wire spells in PascalCase",
            BENEFIT_IDENTIFIER_MEMBER,
            "benefitIdentifier",
            "Member must not be null",
            VALIDATION_REASON_FIELD,
        ),
        (
            "a path into the details document",
            "BenefitApplicationDetails.Campaign.Name",
            "benefitApplicationDetails.Campaign.Name",
            "Member must be of type number",
            VALIDATION_REASON_FIELD,
        ),
        (
            "an element path of a list member",
            "amendments.3.member.fieldPath",
            "amendments.3.member.fieldPath",
            UNAMENDABLE,
            VALIDATION_REASON_FIELD,
        ),
        (
            "a business rule of the service's own",
            "tagKeys",
            "tagKeys",
            "Member must have length less than or equal to 50",
            VALIDATION_REASON_BUSINESS,
        ),
        (
            "a one-character member and a long requirement",
            "Q",
            "q",
            &long_requirement,
            VALIDATION_REASON_BUSINESS,
        ),
    ];

    for (case, member, path, requirement, reason) in rows {
        let built = member_validation_exception(member, requirement, reason);
        let expected = format!(
            "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
             {requirement}"
        );
        assert_eq!(
            built.message, expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            built.message
        );
        assert_eq!(
            built.reason, reason,
            "case {case:?}: expected the reason {reason:?}, got {:?}",
            built.reason
        );
        let fields = built.field_list.unwrap_or_default();
        let got: Vec<(String, String, Option<String>)> = fields
            .into_iter()
            .map(|field| (field.name, field.message, field.code))
            .collect();
        let want = vec![(
            path.to_string(),
            requirement.to_string(),
            Some(VALIDATION_CODE_INVALID_VALUE.to_string()),
        )];
        assert_eq!(
            got, want,
            "case {case:?}: expected the field list {want:?}, got {got:?}"
        );
    }
}
