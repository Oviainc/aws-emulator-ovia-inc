// SPDX-License-Identifier: CC0-1.0

//! `GetBenefit`, end to end over the wire.
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
//! 1. **The report is the store**: the operation reports the one benefit the request
//!    names, member for member from the stored record, and the ARN it reports is the
//!    benefit ARN of the catalog the record lives in, in the region the request was signed
//!    for. Rows: every stored benefit of every seeded catalog — which spans every
//!    `FulfillmentType` the model declares (including a benefit offering two), both
//!    `BenefitStatus` values, and a benefit whose request needs no fields.
//! 2. **Either identifier form names the same record**: the bare `ben-...` id and a
//!    benefit ARN of that catalog are answered identically. Rows: every stored benefit of
//!    every seeded catalog, in both forms.
//! 3. **`BenefitRequestSchema` is the stored schema**: reported as a JSON-Schema object
//!    whose `properties` each declare a type, for exactly the benefits the store holds one
//!    for, and ABSENT for a benefit whose request needs no fields. Rows: every stored
//!    benefit, which covers both sides.
//! 4. **An identifier that names no benefit of the catalog is refused**
//!    `ResourceNotFoundException` (404) with the service's own lookup message. Rows: a
//!    well-formed id no catalog holds, a benefit ARN of the other catalog, an ARN of
//!    another resource type of the same catalog (an application, an allocation), and an
//!    ARN of another service.
//! 5. **A catalog the service does not serve is refused** `ResourceNotFoundException`
//!    (404) with the catalog-lookup message, whatever the identifier. Rows: two catalogs
//!    the service does not serve, each with a stored id and with an unknown one.
//! 6. **The read changes nothing a later caller sees**: two reads of any benefit are
//!    identical, and `ListBenefits` reports exactly the same catalog after the reads as
//!    before them.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::{
    BENEFIT_STATUS_VALUES, FULFILLMENT_TYPE_VALUES,
};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, service_state, Benefit, ALLOCATION_RESOURCE, APPLICATION_RESOURCE,
    BENEFIT_KIND, BENEFIT_RESOURCE, CATALOG_KIND, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, Harness, CATALOG};
use crate::resource_tagging::{error_message, expected_arn};

/// The other catalog the service serves, so a row can name an ARN of a catalog that is not
/// the one the request asked about.
const OTHER_CATALOG: &str = "Sandbox";

/// A benefit id the modelled `GetBenefit$Identifier` pattern admits and no catalog holds,
/// so it names nothing.
const UNKNOWN_BENEFIT: &str = "ben-00000000000000";

/// An application id the modelled `BenefitApplicationId` pattern admits, for the row that
/// names an ARN of another resource type.
const SOME_APPLICATION: &str = "benappl-00000000000000";

/// An allocation id the modelled `BenefitAllocationId` pattern admits, for the row that
/// names an ARN of another resource type.
const SOME_ALLOCATION: &str = "benalloc-00000000000000";

/// The benefits the store holds for `catalog`, in the order it keys them.
///
/// Read straight from the engine, so an assertion can compare the REPORT against the
/// stored record rather than against another report.
fn stored_benefits(harness: &Harness, catalog: &str) -> Vec<Benefit> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .catalogs
        .get(catalog)
        .map(|stored| stored.benefits.values().cloned().collect())
        .unwrap_or_default()
}

/// One `GetBenefit` call that must answer 200.
async fn get(harness: &Harness, case: &str, catalog: &str, identifier: &str) -> Value {
    harness
        .ok(
            case,
            "GetBenefit",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await
}

/// One `GetBenefit` call that must be refused with `ResourceNotFoundException` (404)
/// carrying `expected_message`.
async fn refused(
    harness: &Harness,
    case: &str,
    catalog: &str,
    identifier: &str,
    expected_message: &str,
) {
    let (status, out) = harness
        .call(
            "GetBenefit",
            &json!({ "Catalog": catalog, "Identifier": identifier }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the lookup's own 404, got {status} with {out:?}"
    );
    assert_eq!(
        error_code(&out),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {out:?}"
    );
    assert_eq!(
        error_message(&out),
        expected_message,
        "case {case:?}: expected the service's lookup message, got {out:?}"
    );
}

/// The string list `member` of a report.
fn strings(case: &str, body: &Value, member: &str) -> Vec<String> {
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} list, got {body:?}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {member} holds a non-string entry: {body:?}")
                })
                .to_string()
        })
        .collect()
}

/// The string member `member` of a report.
fn text(case: &str, body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} string, got {body:?}"))
        .to_string()
}

/// Rule 3's shape check: a reported `BenefitRequestSchema` is a JSON-Schema object whose
/// `properties` each declare a JSON type.
fn assert_request_schema(case: &str, schema: &Value) {
    assert_eq!(
        schema.get("type"),
        Some(&json!("object")),
        "case {case:?}: expected a schema of type object, got {schema:?}"
    );
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: the schema declares no properties: {schema:?}"));
    assert!(
        !properties.is_empty(),
        "case {case:?}: a reported schema must declare at least one field, got {schema:?}"
    );
    for (field, property) in properties {
        assert!(
            property.get("type").and_then(Value::as_str).is_some(),
            "case {case:?}: the field {field:?} declares no JSON type, got {property:?}"
        );
    }
}

/// Rule 1, rule 2 and rule 3 together, over every stored benefit of every seeded catalog
/// and both identifier forms: the report is the stored record, whichever form named it.
#[tokio::test]
async fn every_stored_benefit_is_reported_member_for_member_under_either_identifier_form() {
    let harness = Harness::new();

    // Both sides of rule 3 must actually be covered by the rows below, and every value the
    // reports carry must be one the model declares.
    let mut with_schema = 0usize;
    let mut without_schema = 0usize;
    let mut statuses = BTreeSet::new();
    let mut fulfillment_types = BTreeSet::new();
    let mut rows = 0usize;

    for catalog in SEEDED_CATALOGS {
        let stored = stored_benefits(&harness, catalog);
        assert!(
            !stored.is_empty(),
            "case {catalog:?}: the seeded catalog holds no benefit to report"
        );
        for benefit in &stored {
            let arn = expected_arn(catalog, BENEFIT_RESOURCE, &benefit.id);
            // Rule 2's rows: the bare id, and a benefit ARN of this catalog.
            for identifier in [benefit.id.clone(), arn.clone()] {
                let case = format!("{catalog}/{} named by {identifier}", benefit.name);
                let out = get(&harness, &case, catalog, &identifier).await;
                rows += 1;

                assert_eq!(
                    text(&case, &out, "Id"),
                    benefit.id,
                    "case {case:?}: expected the stored id, got {out:?}"
                );
                assert_eq!(
                    text(&case, &out, "Catalog"),
                    *catalog,
                    "case {case:?}: expected the catalog the record lives in, got {out:?}"
                );
                assert_eq!(
                    text(&case, &out, "Arn"),
                    arn,
                    "case {case:?}: expected the benefit ARN of the signed region, got {out:?}"
                );
                assert_eq!(
                    text(&case, &out, "Name"),
                    benefit.name,
                    "case {case:?}: expected the stored name, got {out:?}"
                );
                assert_eq!(
                    text(&case, &out, "Description"),
                    benefit.description,
                    "case {case:?}: expected the stored description, got {out:?}"
                );
                assert_eq!(
                    strings(&case, &out, "Programs"),
                    benefit.programs,
                    "case {case:?}: expected the stored programs, got {out:?}"
                );
                assert_eq!(
                    strings(&case, &out, "FulfillmentTypes"),
                    benefit.fulfillment_types,
                    "case {case:?}: expected the stored fulfillment types, got {out:?}"
                );
                let status = text(&case, &out, "Status");
                assert_eq!(
                    status, benefit.status,
                    "case {case:?}: expected the stored status, got {out:?}"
                );
                assert!(
                    BENEFIT_STATUS_VALUES.contains(&status.as_str()),
                    "case {case:?}: {status:?} is not a modelled BenefitStatus, got {out:?}"
                );
                for fulfillment_type in strings(&case, &out, "FulfillmentTypes") {
                    assert!(
                        FULFILLMENT_TYPE_VALUES.contains(&fulfillment_type.as_str()),
                        "case {case:?}: {fulfillment_type:?} is not a modelled FulfillmentType, \
                         got {out:?}"
                    );
                    fulfillment_types.insert(fulfillment_type);
                }
                statuses.insert(status);

                // Rule 3: the schema is reported for exactly the benefits the store holds
                // one for.
                match (
                    benefit.benefit_request_schema.is_some(),
                    out.get("BenefitRequestSchema"),
                ) {
                    (true, Some(schema)) => {
                        assert_request_schema(&case, schema);
                        with_schema += 1;
                    }
                    (false, None) => without_schema += 1,
                    (stored_has, reported) => panic!(
                        "case {case:?}: the store holds a schema: {stored_has}, but the report \
                         carries {reported:?}"
                    ),
                }
            }
        }
    }

    assert!(
        rows >= 2 * SEEDED_CATALOGS.len(),
        "the table must cover every seeded catalog in both identifier forms, got {rows} rows"
    );
    assert!(
        with_schema > 0 && without_schema > 0,
        "the table must cover a benefit WITH a request schema and one WITHOUT, got \
         {with_schema} with and {without_schema} without"
    );
    assert_eq!(
        statuses,
        BENEFIT_STATUS_VALUES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<BTreeSet<_>>(),
        "the table must span every modelled BenefitStatus, got {statuses:?}"
    );
    assert_eq!(
        fulfillment_types,
        FULFILLMENT_TYPE_VALUES
            .iter()
            .map(|value| (*value).to_string())
            .collect::<BTreeSet<_>>(),
        "the table must span every modelled FulfillmentType, got {fulfillment_types:?}"
    );
}

/// Rule 2 stated directly: the two identifier forms produce the SAME body, byte for byte,
/// for every stored benefit of every seeded catalog.
#[tokio::test]
async fn the_bare_id_and_the_benefit_arn_are_answered_identically() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        for benefit in stored_benefits(&harness, catalog) {
            let arn = expected_arn(catalog, BENEFIT_RESOURCE, &benefit.id);
            let case = format!("{catalog}/{}", benefit.name);
            let by_id = get(&harness, &case, catalog, &benefit.id).await;
            let by_arn = get(&harness, &case, catalog, &arn).await;
            assert_eq!(
                by_id, by_arn,
                "case {case:?}: the bare id and the ARN must name the same record, got \
                 {by_id:?} and {by_arn:?}"
            );
        }
    }
}

/// Rule 4: an identifier the modelled pattern admits but which names no benefit OF THE
/// CATALOG the request asked about is refused with the benefit-lookup message.
#[tokio::test]
async fn an_identifier_that_names_no_benefit_of_the_catalog_is_refused() {
    let harness = Harness::new();

    let stored = stored_benefits(&harness, CATALOG);
    let existing = stored
        .first()
        .map(|benefit| benefit.id.clone())
        .expect("the production catalog holds a seeded benefit");
    let other_catalog_benefit = stored_benefits(&harness, OTHER_CATALOG)
        .first()
        .map(|benefit| benefit.id.clone())
        .expect("the sandbox catalog holds a seeded benefit");

    // (case, the identifier the request names)
    let rows: Vec<(&str, String)> = vec![
        ("a well-formed id no catalog holds", UNKNOWN_BENEFIT.to_string()),
        (
            "a benefit ARN of the other catalog",
            expected_arn(OTHER_CATALOG, BENEFIT_RESOURCE, &other_catalog_benefit),
        ),
        (
            "an ARN of the other catalog carrying THIS catalog's benefit id",
            expected_arn(OTHER_CATALOG, BENEFIT_RESOURCE, &existing),
        ),
        (
            "an application ARN of this catalog",
            expected_arn(CATALOG, APPLICATION_RESOURCE, SOME_APPLICATION),
        ),
        (
            "an allocation ARN of this catalog",
            expected_arn(CATALOG, ALLOCATION_RESOURCE, SOME_ALLOCATION),
        ),
        (
            "an ARN of another service naming this benefit",
            expected_arn(CATALOG, BENEFIT_RESOURCE, &existing).replace(":partnercentral:", ":s3:"),
        ),
    ];

    for (case, identifier) in &rows {
        refused(
            &harness,
            case,
            CATALOG,
            identifier,
            &not_found_message(BENEFIT_KIND, identifier),
        )
        .await;
    }
}

/// Rule 5: a catalog the service does not serve is refused with the CATALOG-lookup
/// message, whatever the identifier — so the answer names the resource that was actually
/// missing.
#[tokio::test]
async fn a_catalog_the_service_does_not_serve_is_refused_whatever_the_identifier() {
    let harness = Harness::new();

    let existing = stored_benefits(&harness, CATALOG)
        .first()
        .map(|benefit| benefit.id.clone())
        .expect("the production catalog holds a seeded benefit");

    for catalog in ["NotACatalog", "aws"] {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog),
            "case {catalog:?}: this row must name a catalog the service does NOT serve"
        );
        for identifier in [
            existing.clone(),
            UNKNOWN_BENEFIT.to_string(),
            expected_arn(catalog, BENEFIT_RESOURCE, &existing),
        ] {
            let case = format!("{catalog} named with {identifier}");
            refused(
                &harness,
                &case,
                catalog,
                &identifier,
                &not_found_message(CATALOG_KIND, catalog),
            )
            .await;
        }
    }
}

/// Rule 6: `GetBenefit` reports; it changes nothing a later caller sees. Two reads of any
/// benefit are identical, and the catalog `ListBenefits` reports is the same after the
/// reads as before them.
#[tokio::test]
async fn reading_a_benefit_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        let before = harness.benefits(catalog).await;
        for benefit in stored_benefits(&harness, catalog) {
            let case = format!("{catalog}/{}", benefit.name);
            let first = get(&harness, &case, catalog, &benefit.id).await;
            let second = get(&harness, &case, catalog, &benefit.id).await;
            assert_eq!(
                first, second,
                "case {case:?}: two reads must be identical, got {first:?} then {second:?}"
            );
        }
        // A refused read must leave the catalog alone too.
        refused(
            &harness,
            catalog,
            catalog,
            UNKNOWN_BENEFIT,
            &not_found_message(BENEFIT_KIND, UNKNOWN_BENEFIT),
        )
        .await;
        let after = harness.benefits(catalog).await;
        assert_eq!(
            before, after,
            "case {catalog:?}: the reads must not change what ListBenefits reports"
        );
    }
}
