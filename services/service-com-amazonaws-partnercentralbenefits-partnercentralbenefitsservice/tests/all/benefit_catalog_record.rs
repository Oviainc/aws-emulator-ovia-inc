// SPDX-License-Identifier: CC0-1.0

//! The catalog record of the benefit an application is for, end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rules under test, each stated over a CLASS of inputs and checked as a table whose
//! rows differ along what the rule generalises over:
//!
//! 1. **A benefit the catalog no longer offers cannot be applied for**: `BenefitStatus` is
//!    `ACTIVE | INACTIVE`, and an application for a benefit that is not `ACTIVE` is refused
//!    with the modelled `ValidationException` (400) naming `BenefitIdentifier`, minting
//!    nothing; one for an `ACTIVE` benefit is served. Rows: every stored benefit of every
//!    seeded catalog — which spans both statuses — in both identifier forms the modelled
//!    pattern admits (the bare `ben-...` id and a benefit ARN of that catalog).
//! 2. **A details document is judged against that benefit's `BenefitRequestSchema`**: the
//!    schema describes the fields a request for the benefit carries, so a document must
//!    have the JSON type the schema declares and give each declared field it carries a
//!    value of the type declared for it; anything else is refused with the modelled
//!    `ValidationException` (400) naming the field's own path. A field the schema does not
//!    declare is the caller's own, and so is a declared field the document leaves out (the
//!    member is modelled optional on every operation that writes it, and the application
//!    it creates is a draft). Rows: every `ACTIVE` benefit the store holds a schema for,
//!    each declared field of it in a conforming and in a non-conforming type, and four
//!    document shapes the schema's own `type` refuses.
//! 3. **One rule, wherever the member is written**: `UpdateBenefitApplication` and
//!    `AmendBenefitApplication` judge the document they would store by the same schema, and
//!    a refused write changes nothing. Rows: an update and an amendment of a declared
//!    string field (which conform) and of a declared number field written as text (which
//!    does not).

use serde_json::{json, Map, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    BENEFIT_IDENTIFIER_MEMBER, BENEFIT_RESOURCE, BENEFIT_STATUS_ACTIVE, DETAILS_MEMBER,
    SEEDED_CATALOGS, VALIDATION_REASON_BUSINESS,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::resource_tagging::expected_arn;

/// A field name no seeded `BenefitRequestSchema` declares, so a document carrying it
/// carries something the schema says nothing about.
const UNDECLARED_FIELD: &str = "PartnersOwnNote";

/// One benefit of a catalog, as the catalog's own records report it.
struct CatalogBenefit {
    id: String,
    /// A `BenefitStatus` value, from `GetBenefit$Status`.
    status: String,
    /// `GetBenefit$BenefitRequestSchema`, absent for a benefit whose request needs no
    /// fields.
    schema: Option<Value>,
}

/// Every benefit of `catalog`, read through `ListBenefits` and `GetBenefit`.
///
/// The rows of the tables below are the catalog's own records rather than a list written
/// here, so a change to the seeded catalog changes the rows rather than breaking them.
async fn catalog_benefits(harness: &Harness, catalog: &str) -> Vec<CatalogBenefit> {
    let mut benefits = Vec::new();
    for summary in harness.benefits(catalog).await {
        let id = text(catalog, &summary, "Id");
        let got = harness
            .ok(
                &id,
                "GetBenefit",
                &json!({ "Catalog": catalog, "Identifier": id }),
            )
            .await;
        benefits.push(CatalogBenefit {
            status: text(&id, &got, "Status"),
            schema: got.get("BenefitRequestSchema").cloned(),
            id,
        });
    }
    assert!(
        !benefits.is_empty(),
        "case {catalog:?}: the seeded catalog holds no benefit to apply for"
    );
    benefits
}

/// The `(field, declared JSON type)` pairs `schema` declares, in field order.
///
/// The schema is a JSON-Schema object: `type` names the JSON type of the position and
/// `properties` maps each declared field to the schema its value must satisfy
/// (`smithy_query(query="doc",shape="GetBenefitOutput",member="BenefitRequestSchema")` —
/// "the schema definition that describes the required fields for requesting this
/// benefit").
fn declared_fields(case: &str, schema: &Value) -> Vec<(String, String)> {
    assert_eq!(
        schema.get("type"),
        Some(&json!("object")),
        "case {case:?}: expected a schema declaring an object, got {schema:?}"
    );
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: the schema declares no properties: {schema:?}"));
    properties
        .iter()
        .map(|(field, property)| {
            let json_type = property
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the field {field:?} declares no JSON type: {property:?}")
                });
            (field.clone(), json_type.to_string())
        })
        .collect()
}

/// A value OF the JSON type `json_type` names, for a field the schema declares that type
/// for. The type names are JSON Schema's own, which is what the stored schema carries.
fn value_of_type(case: &str, json_type: &str) -> Value {
    match json_type {
        "string" => json!("a value of the declared type"),
        "number" => json!(2500),
        "boolean" => json!(true),
        "array" => json!(["one"]),
        "object" => json!({ "Nested": "one" }),
        other => panic!("case {case:?}: the seeded schema declares an unknown type {other:?}"),
    }
}

/// A value of a JSON type `json_type` does NOT name, so the schema refuses it where that
/// type is declared.
fn value_of_another_type(json_type: &str) -> Value {
    if json_type == "string" {
        json!(7)
    } else {
        json!("not of the declared type")
    }
}

/// How many applications `catalog` holds, so a refused write can be shown to have minted
/// nothing.
async fn application_count(harness: &Harness, case: &str, catalog: &str) -> usize {
    let out = harness
        .ok(
            case,
            "ListBenefitApplications",
            &json!({ "Catalog": catalog }),
        )
        .await;
    out["BenefitApplicationSummaries"]
        .as_array()
        .map_or(0, Vec::len)
}

/// A refusal's one `FieldList` entry as `(Name, Message)`.
fn refused_field(case: &str, out: &Value) -> (String, String) {
    let field = out["FieldList"]
        .as_array()
        .and_then(|fields| fields.first())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal names no field: {out:?}"));
    (text(case, field, "Name"), text(case, field, "Message"))
}

/// Assert `operation` refused `body` with the modelled `ValidationException` (400) naming
/// `member` and the requirement it failed.
async fn assert_refused(
    harness: &Harness,
    case: &str,
    operation: &str,
    body: &Value,
    member: &str,
    requirement: &str,
) {
    let (status, out) = harness.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the modelled ValidationException's 400, got {status} {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {out:?}"
    );
    assert_eq!(
        refused_field(case, &out),
        (member.to_string(), requirement.to_string()),
        "case {case:?}: expected the refusal to name {member:?} and its requirement, got {out:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the benefit's status
// ---------------------------------------------------------------------------

/// Every stored benefit of every seeded catalog, in both identifier forms: an application
/// for one that is not `ACTIVE` is refused and mints nothing, while one for an `ACTIVE`
/// benefit is served.
#[tokio::test]
async fn create_refuses_an_application_for_a_benefit_the_catalog_no_longer_offers() {
    let harness = Harness::new();
    let mut offered = 0usize;
    let mut withdrawn = 0usize;

    for catalog in SEEDED_CATALOGS {
        for benefit in catalog_benefits(&harness, catalog).await {
            for (form, identifier) in [
                ("id", benefit.id.clone()),
                ("arn", expected_arn(catalog, BENEFIT_RESOURCE, &benefit.id)),
            ] {
                let case = format!(
                    "{catalog}: a {} benefit named by its {form}",
                    benefit.status.to_lowercase()
                );
                let before = application_count(&harness, &case, catalog).await;
                let body = json!({
                    "Catalog": catalog,
                    "ClientToken": format!("tok-offered-{}-{}-{form}", catalog, benefit.id),
                    "BenefitIdentifier": identifier,
                    "Name": "status-table",
                });

                if benefit.status == BENEFIT_STATUS_ACTIVE {
                    offered += 1;
                    let created = harness.create(&case, &body).await;
                    let got = harness
                        .get_in(&case, catalog, &text(&case, &created, "Id"))
                        .await;
                    assert_eq!(
                        got["BenefitId"],
                        json!(benefit.id),
                        "case {case:?}: the application must be for the benefit it named, got {got:?}"
                    );
                    continue;
                }

                withdrawn += 1;
                let (status, out) = harness.call("CreateBenefitApplication", &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: a benefit the catalog does not offer must be refused, \
                     got {status} {out:?}"
                );
                assert_eq!(
                    error_code(&out),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {out:?}"
                );
                assert_eq!(
                    out["Reason"],
                    json!(VALIDATION_REASON_BUSINESS),
                    "case {case:?}: expected the business-validation reason, got {out:?}"
                );
                assert_eq!(
                    refused_field(&case, &out).0,
                    BENEFIT_IDENTIFIER_MEMBER,
                    "case {case:?}: expected the refusal to name the benefit member, got {out:?}"
                );
                assert_eq!(
                    application_count(&harness, &case, catalog).await,
                    before,
                    "case {case:?}: a refused create must mint nothing"
                );
            }
        }
    }

    assert!(
        offered > 0 && withdrawn > 0,
        "the seeded catalogs must cover both sides of the rule, got {offered} offered and \
         {withdrawn} withdrawn"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the benefit's request schema
// ---------------------------------------------------------------------------

/// Every `ACTIVE` benefit the store holds a request schema for: a document that satisfies
/// the schema round-trips, and one that contradicts it — in a field's type or in the
/// document's own type — is refused, naming the position that failed, and mints nothing.
#[tokio::test]
async fn create_judges_the_details_document_against_the_benefits_request_schema() {
    let harness = Harness::new();
    let mut conforming_rows = 0usize;
    let mut refused_rows = 0usize;

    for benefit in catalog_benefits(&harness, CATALOG).await {
        if benefit.status != BENEFIT_STATUS_ACTIVE {
            continue;
        }
        let Some(schema) = benefit.schema.as_ref() else {
            continue;
        };
        let fields = declared_fields(&benefit.id, schema);
        assert!(
            !fields.is_empty(),
            "case {:?}: a stored schema must declare a field to test, got {schema:?}",
            benefit.id
        );

        // Every declared field with a value of the declared type, plus one field the
        // schema does not declare: stored as sent, and read back unchanged.
        let mut conforming = Map::new();
        for (field, json_type) in &fields {
            conforming.insert(field.clone(), value_of_type(&benefit.id, json_type));
        }
        conforming.insert(UNDECLARED_FIELD.to_string(), json!("the partner's own note"));
        let details = Value::Object(conforming.clone());
        let case = format!("{}: a document the schema accepts", benefit.id);
        let created = harness
            .create(
                &case,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-schema-ok-{}", benefit.id),
                    "BenefitIdentifier": benefit.id,
                    "BenefitApplicationDetails": details,
                }),
            )
            .await;
        let got = harness.get(&case, &text(&case, &created, "Id")).await;
        assert_eq!(
            got[DETAILS_MEMBER], details,
            "case {case:?}: an accepted document must round-trip unchanged, got {got:?}"
        );
        conforming_rows += 1;

        // One row per declared field: the same document with THAT field of another JSON
        // type is refused against the field's own path.
        for (field, json_type) in &fields {
            let case = format!("{}: {field} as another type", benefit.id);
            let mut wrong = conforming.clone();
            wrong.insert(field.clone(), value_of_another_type(json_type));
            let before = application_count(&harness, &case, CATALOG).await;
            assert_refused(
                &harness,
                &case,
                "CreateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-schema-type-{}-{field}", benefit.id),
                    "BenefitIdentifier": benefit.id,
                    "BenefitApplicationDetails": Value::Object(wrong),
                }),
                &format!("{DETAILS_MEMBER}.{field}"),
                &format!("Member must be of type {json_type}"),
            )
            .await;
            assert_eq!(
                application_count(&harness, &case, CATALOG).await,
                before,
                "case {case:?}: a refused create must mint nothing"
            );
            refused_rows += 1;
        }

        // The schema declares an object, so a document of any other JSON type is refused
        // against the member itself.
        for (shape, document) in [
            ("array", json!([1, 2, 3])),
            ("string", json!("free form")),
            ("number", json!(7)),
            ("boolean", json!(true)),
        ] {
            let case = format!(
                "{}: a {shape} where the schema declares an object",
                benefit.id
            );
            assert_refused(
                &harness,
                &case,
                "CreateBenefitApplication",
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-schema-shape-{}-{shape}", benefit.id),
                    "BenefitIdentifier": benefit.id,
                    "BenefitApplicationDetails": document,
                }),
                DETAILS_MEMBER,
                "Member must be of type object",
            )
            .await;
            refused_rows += 1;
        }

        // A document carrying none of the declared fields contradicts nothing the schema
        // states: the member is modelled optional wherever it is written, so what the
        // schema constrains is the value a document PUTS at a field it declares.
        let case = format!("{}: only a field the schema does not declare", benefit.id);
        harness
            .create(
                &case,
                &json!({
                    "Catalog": CATALOG,
                    "ClientToken": format!("tok-schema-extra-{}", benefit.id),
                    "BenefitIdentifier": benefit.id,
                    "BenefitApplicationDetails": { UNDECLARED_FIELD: "only this" },
                }),
            )
            .await;
        conforming_rows += 1;
    }

    assert!(
        conforming_rows > 0 && refused_rows > 0,
        "the seeded catalog must cover both sides of the rule, got {conforming_rows} \
         accepted and {refused_rows} refused rows"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the same rule on the update and the amendment
// ---------------------------------------------------------------------------

/// The update and the amendment judge the document they would store by the same schema:
/// a write that satisfies it applies, one that contradicts it is refused and changes
/// nothing.
#[tokio::test]
async fn update_and_amend_judge_the_document_they_would_store_by_the_same_schema() {
    let harness = Harness::new();

    // The rule needs a benefit declaring a field of each of two types, so one write can
    // satisfy the schema and the next contradict it at the other field. An amendment
    // carries its `NewValue` as text, so a field declared `number` is exactly the field
    // an amendment cannot satisfy.
    let mut subject: Option<(String, Vec<(String, String)>)> = None;
    for benefit in catalog_benefits(&harness, CATALOG).await {
        if benefit.status != BENEFIT_STATUS_ACTIVE {
            continue;
        }
        let Some(schema) = benefit.schema.as_ref() else {
            continue;
        };
        let fields = declared_fields(&benefit.id, schema);
        let has = |wanted: &str| fields.iter().any(|(_, json_type)| json_type == wanted);
        if has("string") && has("number") {
            subject = Some((benefit.id.clone(), fields));
            break;
        }
    }
    let (benefit_id, fields) = subject.expect(
        "the seeded catalog must hold an ACTIVE benefit declaring a string and a number field",
    );
    let field_of = |wanted: &str| -> String {
        fields
            .iter()
            .find(|(_, json_type)| json_type == wanted)
            .map(|(field, _)| field.clone())
            .unwrap_or_else(|| panic!("the chosen benefit declares a {wanted} field"))
    };
    let string_field = field_of("string");
    let number_field = field_of("number");

    let mut conforming = Map::new();
    for (field, json_type) in &fields {
        conforming.insert(field.clone(), value_of_type("seed", json_type));
    }
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-schema-writes",
                "BenefitIdentifier": benefit_id,
                "BenefitApplicationDetails": Value::Object(conforming.clone()),
            }),
        )
        .await;
    let id = text("seed", &created, "Id");

    // The update: a replacement document whose declared number field carries text is
    // refused, and the stored application is untouched.
    let before = harness.get("seed", &id).await;
    let mut wrong = conforming.clone();
    wrong.insert(number_field.clone(), value_of_another_type("number"));
    let case = format!("an update writing {number_field} as text");
    assert_refused(
        &harness,
        &case,
        "UpdateBenefitApplication",
        &json!({
            "Catalog": CATALOG,
            "ClientToken": "tok-schema-update-wrong",
            "Identifier": id,
            "Revision": text("seed", &before, "Revision"),
            "BenefitApplicationDetails": Value::Object(wrong),
        }),
        &format!("{DETAILS_MEMBER}.{number_field}"),
        "Member must be of type number",
    )
    .await;
    assert_eq!(
        harness.get(&case, &id).await,
        before,
        "case {case:?}: a refused update must change nothing"
    );

    // The same update with a document the schema accepts applies, and is read back.
    let mut replacement = conforming.clone();
    replacement.insert(string_field.clone(), json!("the replacement value"));
    let replacement = Value::Object(replacement);
    let case = format!("an update replacing {string_field}");
    harness
        .ok(
            &case,
            "UpdateBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-schema-update-ok",
                "Identifier": id,
                "Revision": text("seed", &before, "Revision"),
                "BenefitApplicationDetails": replacement.clone(),
            }),
        )
        .await;
    let after_update = harness.get(&case, &id).await;
    assert_eq!(
        after_update[DETAILS_MEMBER], replacement,
        "case {case:?}: an accepted update must store the document it carried, got {after_update:?}"
    );

    // The amendment: writing the declared string field applies, since an amendment's
    // `NewValue` is text and the schema declares text there.
    let case = format!("an amendment writing {string_field}");
    harness
        .ok(
            &case,
            "AmendBenefitApplication",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-schema-amend-ok",
                "Identifier": id,
                "Revision": text(&case, &after_update, "Revision"),
                "AmendmentReason": "the partner corrected the campaign",
                "Amendments": [{
                    "FieldPath": format!("{DETAILS_MEMBER}.{string_field}"),
                    "NewValue": "the amended value",
                }],
            }),
        )
        .await;
    let after_amendment = harness.get(&case, &id).await;
    assert_eq!(
        after_amendment[DETAILS_MEMBER][&string_field],
        json!("the amended value"),
        "case {case:?}: the amendment must have applied, got {after_amendment:?}"
    );

    // Writing the declared number field as text contradicts the schema, so the amendment
    // is refused and nothing changes.
    let case = format!("an amendment writing {number_field} as text");
    assert_refused(
        &harness,
        &case,
        "AmendBenefitApplication",
        &json!({
            "Catalog": CATALOG,
            "ClientToken": "tok-schema-amend-wrong",
            "Identifier": id,
            "Revision": text(&case, &after_amendment, "Revision"),
            "AmendmentReason": "the partner corrected the amount",
            "Amendments": [{
                "FieldPath": format!("{DETAILS_MEMBER}.{number_field}"),
                "NewValue": "250",
            }],
        }),
        &format!("{DETAILS_MEMBER}.{number_field}"),
        "Member must be of type number",
    )
    .await;
    assert_eq!(
        harness.get(&case, &id).await,
        after_amendment,
        "case {case:?}: a refused amendment must change nothing"
    );
}
