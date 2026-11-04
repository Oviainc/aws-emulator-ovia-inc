// SPDX-License-Identifier: CC0-1.0

//! The catalog lookup every operation that binds `Catalog` does, end to end over the
//! wire.
//!
//! The rule under test: every resource this service holds lives inside a catalog, so an
//! operation whose required `Catalog` names no stored catalog has nothing to look up
//! inside it and answers 404 `ResourceNotFoundException` naming the CATALOG and the name
//! the request supplied — before any resource inside that catalog is resolved, and
//! whatever other identifier the request carried.
//!
//! The tables span the class the rule covers, not one call: every operation the model
//! binds a `Catalog` member on (the well-formed request per operation that
//! `crate::service_error_rules` already keeps, with its `Catalog` replaced) crossed with
//! several catalog names the modelled `CatalogName` pattern admits
//! (`^[A-Za-z0-9_-]+$`, `smithy_query(query="shape",shape="CatalogName")`, which
//! constrains no length), and — for the application-identified operations — every form
//! the modelled `BenefitApplicationIdentifier` admits, naming an application that really
//! exists as well as one that never did.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, CATALOG_KIND, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{
    error_code, text, Harness, ACCOUNT, CATALOG, REGION,
};
use crate::service_error_rules::{message_of, well_formed_requests, UNKNOWN_APPLICATION};

/// The operations that bind a required `Catalog` member: every one this service declares
/// except the three tagging operations, which name their resource by ARN and bind no
/// catalog (`smithy_query(query="digest")`).
const CATALOG_BOUND_OPERATIONS: &[&str] = &[
    "AmendBenefitApplication",
    "AssociateBenefitApplicationResource",
    "CancelBenefitApplication",
    "CreateBenefitApplication",
    "DisassociateBenefitApplicationResource",
    "GetBenefit",
    "GetBenefitAllocation",
    "GetBenefitApplication",
    "ListBenefitAllocations",
    "ListBenefitApplications",
    "ListBenefits",
    "RecallBenefitApplication",
    "SubmitBenefitApplication",
    "UpdateBenefitApplication",
];

/// The application-identified operations whose identifier member is `Identifier`, and the
/// two whose member is `BenefitApplicationIdentifier` — the operations that resolve an
/// application inside the catalog the request names.
const APPLICATION_IDENTIFIED_OPERATIONS: &[&str] = &[
    "GetBenefitApplication",
    "SubmitBenefitApplication",
    "RecallBenefitApplication",
    "AssociateBenefitApplicationResource",
    "DisassociateBenefitApplicationResource",
];

/// Catalog names no catalog of this service answers to, each one the modelled
/// `CatalogName` pattern admits: a hyphenated name, the production catalog's own name in
/// another case (the names are matched exactly), digits only, an underscored name, a
/// single character, and a long one — the pattern bounds the length not at all.
///
/// Shared with `crate::write_gate_order`, which drives the same spellings at the other
/// entitlement.
pub(crate) fn unknown_catalogs() -> Vec<(&'static str, String)> {
    vec![
        (
            // The instance the issue reported.
            "a hyphenated name",
            "not-a-real-catalog-34556872".to_string(),
        ),
        (
            "the production catalog in another case",
            CATALOG.to_ascii_lowercase(),
        ),
        ("digits only", "0123456789".to_string()),
        ("an underscored name", "not_a_real_catalog".to_string()),
        ("a single character", "x".to_string()),
        ("a long name", "c".repeat(256)),
    ]
}

/// The same request, naming `catalog` instead.
pub(crate) fn with_catalog(body: &Value, catalog: &str) -> Value {
    let mut body = body.clone();
    body["Catalog"] = json!(catalog);
    body
}

/// The well-formed request per operation that binds a `Catalog`, which is the class the
/// rule spans.
pub(crate) fn catalog_bound_requests() -> Vec<(&'static str, Value)> {
    let requests: Vec<(&'static str, Value)> = well_formed_requests()
        .into_iter()
        .filter(|(_, body)| body.get("Catalog").is_some())
        .collect();
    let mut covered: Vec<&str> = requests.iter().map(|(operation, _)| *operation).collect();
    covered.sort_unstable();
    assert_eq!(
        covered, CATALOG_BOUND_OPERATIONS,
        "the table must span every operation that binds a Catalog, got {covered:?}"
    );
    requests
}

/// One request body for `operation` naming `catalog` and the application `identifier`.
fn application_body(operation: &str, catalog: &str, identifier: &str) -> Value {
    match operation {
        "AssociateBenefitApplicationResource" | "DisassociateBenefitApplicationResource" => json!({
            "Catalog": catalog,
            "BenefitApplicationIdentifier": identifier,
            "ResourceArn": "arn:aws:s3:::catalog-lookup-probe-bucket",
        }),
        "RecallBenefitApplication" => json!({
            "Catalog": catalog,
            "Identifier": identifier,
            "Reason": "a catalog-lookup probe",
        }),
        _ => json!({ "Catalog": catalog, "Identifier": identifier }),
    }
}

/// Call `operation` and require the catalog answer: 404 `ResourceNotFoundException`
/// naming the catalog the request supplied.
pub(crate) async fn assert_catalog_not_found(
    harness: &Harness,
    case: &str,
    operation: &str,
    body: &Value,
) {
    let catalog = body["Catalog"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the request body names no catalog: {body:?}"));
    let (status, response) = harness.call(operation, body).await;
    assert_eq!(
        status, 404,
        "case {case:?} on {operation}: expected 404, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case {case:?} on {operation}: expected ResourceNotFoundException, got {response:?}"
    );
    let message = message_of(&response).unwrap_or_else(|| {
        panic!("case {case:?} on {operation}: the answer carries no message: {response:?}")
    });
    let expected = not_found_message(CATALOG_KIND, catalog);
    assert_eq!(
        message, expected,
        "case {case:?} on {operation}: expected {expected:?}, got {message:?}"
    );
}

/// Every operation that binds a `Catalog`, for every spelling of a catalog this service
/// does not hold: the 404 names the CATALOG and the name the request supplied, not a
/// resource that would have lived inside it.
#[tokio::test]
async fn every_operation_that_binds_a_catalog_reports_the_catalog_it_could_not_find() {
    let harness = Harness::new();
    let requests = catalog_bound_requests();
    for (case, catalog) in unknown_catalogs() {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog.as_str()),
            "case {case:?}: {catalog:?} must name no catalog this service serves"
        );
        for (operation, body) in &requests {
            let body = with_catalog(body, &catalog);
            assert_catalog_not_found(&harness, case, operation, &body).await;
        }
    }
}

/// The catalog is resolved BEFORE the application inside it, so the answer is the same
/// whichever form the identifier takes and whether or not that identifier names a stored
/// application in a catalog that does exist.
#[tokio::test]
async fn a_catalog_that_does_not_exist_outranks_the_identifier_it_was_given() {
    let harness = Harness::new();
    let unknown = "not-a-real-catalog-34556872";
    let case = "an application stored in a catalog that exists";

    // One application that really exists, in a catalog that really exists, so the rows
    // below differ only in the catalog the request names.
    let benefit_id = text(case, &harness.active_benefit_of(CATALOG).await, "Id");
    let created = harness
        .create(
            case,
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "catalog-lookup-probe",
                "BenefitIdentifier": benefit_id,
                "Name": "catalog-lookup-probe",
            }),
        )
        .await;
    let stored_id = text(case, &created, "Id");
    let stored_arn = text(case, &created, "Arn");
    let unknown_arn = format!(
        "arn:aws:partnercentral:{REGION}:{ACCOUNT}:catalog/{CATALOG}/benefit-application/{UNKNOWN_APPLICATION}"
    );

    // (case, identifier) — both forms the modelled `BenefitApplicationIdentifier` admits,
    // naming a stored application and one no create ever minted.
    let identifiers = [
        ("the id of a stored application", stored_id.clone()),
        ("the ARN of a stored application", stored_arn),
        ("an id no create minted", UNKNOWN_APPLICATION.to_string()),
        ("an ARN no create minted", unknown_arn),
    ];
    for (identifier_case, identifier) in identifiers {
        for operation in APPLICATION_IDENTIFIED_OPERATIONS {
            let body = application_body(operation, unknown, &identifier);
            assert_catalog_not_found(&harness, identifier_case, operation, &body).await;
        }
    }

    // Nothing above touched the record: a refused lookup is a read that changed no state,
    // so the application is still there in the catalog it was created in.
    let stored = harness.get_in(case, CATALOG, &stored_id).await;
    assert_eq!(
        stored["Id"],
        json!(stored_id),
        "case {case:?}: the stored application must still be readable, got {stored:?}"
    );
}

/// The other side of the rule: a catalog this service DOES hold is never reported
/// missing, on any operation that binds the member — whatever the request asks of it.
#[tokio::test]
async fn a_catalog_this_service_holds_is_never_reported_missing() {
    let harness = Harness::new();
    let requests = catalog_bound_requests();
    for catalog in SEEDED_CATALOGS {
        let forbidden = not_found_message(CATALOG_KIND, catalog);
        for (operation, body) in &requests {
            let body = with_catalog(body, catalog);
            let (status, response) = harness.call(operation, &body).await;
            let message = message_of(&response).unwrap_or_default();
            assert_ne!(
                message, forbidden,
                "case {catalog:?} on {operation}: a catalog the service serves must never \
                 be reported missing, got {status} with {response:?}"
            );
        }
    }
}
