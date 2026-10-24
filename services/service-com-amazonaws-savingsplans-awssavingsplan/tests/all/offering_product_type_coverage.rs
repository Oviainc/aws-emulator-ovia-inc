// SPDX-License-Identifier: CC0-1.0

//! The offering catalog sells every product type the service models.
//!
//! One rule, asserted over the WHOLE value set of the model's own
//! `SavingsPlanProductType` enum rather than over the two or three products a
//! recording happened to cite:
//!
//! * every product type the model enumerates is covered by at least one
//!   purchasable offering, so a search narrowed to it answers with the offerings
//!   that cover it, and a plan bought from one of them reports that product
//!   type; and no offering reports a product type the model does not declare.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, so
//! what is pinned is the HTTP behaviour. The harness (`Emulator`, `text`) is the
//! one the sibling lifecycle suite defines.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::ops::types::SAVINGS_PLAN_PRODUCT_TYPE_VALUES;

use crate::savings_plans_lifecycle::{text, Emulator};

/// How many product types the model declares, as a guard on the table below: if
/// the model gains or loses a value, this suite's rows change with it and the
/// count says so out loud.
///
/// Source: `smithy_query(query="shape", shape="SavingsPlanProductType")` lists
/// EC2, Fargate, Lambda, SageMaker, RDS, DSQL, DynamoDB, ElastiCache, DocDB,
/// Neptune, Timestream, Keyspaces, DMS and OpenSearch — the same set the
/// generated [`SAVINGS_PLAN_PRODUCT_TYPE_VALUES`] carries, which is what the
/// rows are taken from.
const MODELLED_PRODUCT_TYPES: usize = 14;

/// The `productTypes` an offering or a plan reports.
fn product_types(value: &Value) -> Vec<String> {
    value
        .get("productTypes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|product| product.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `value` reports coverage of `product`.
fn covers(value: &Value, product: &str) -> bool {
    product_types(value)
        .iter()
        .any(|covered| covered.as_str() == product)
}

/// A search narrowed to any product type the model declares answers with
/// exactly the catalog rows that cover it, and that set is never empty; a value
/// outside the model's set is answered with no offering at all.
#[tokio::test]
async fn every_modelled_product_type_is_sold_by_the_catalog() {
    let emulator = Emulator::new();
    let catalog = emulator.all_offerings().await;
    assert!(
        catalog.len() > 20,
        "the seeded catalog must be a catalog: got {} offerings",
        catalog.len()
    );
    assert_eq!(
        SAVINGS_PLAN_PRODUCT_TYPE_VALUES.len(),
        MODELLED_PRODUCT_TYPES,
        "case the model's value set: expected {MODELLED_PRODUCT_TYPES} product types, got {:?}",
        SAVINGS_PLAN_PRODUCT_TYPE_VALUES
    );

    // The coverage the catalog advertises is exactly the model's value set:
    // nothing modelled is unsold, and nothing unmodelled is advertised.
    let modelled: BTreeSet<&str> = SAVINGS_PLAN_PRODUCT_TYPE_VALUES.iter().copied().collect();
    let advertised: BTreeSet<String> = catalog.iter().flat_map(product_types).collect();
    let advertised_refs: BTreeSet<&str> = advertised.iter().map(String::as_str).collect();
    assert_eq!(
        advertised_refs, modelled,
        "case the catalog's coverage: expected every modelled product type and no other, missing {:?} / unmodelled {:?}",
        modelled.difference(&advertised_refs).collect::<Vec<_>>(),
        advertised_refs.difference(&modelled).collect::<Vec<_>>()
    );

    // Row per product type the model declares — the two the recording cited and
    // the twelve it did not.
    for product in SAVINGS_PLAN_PRODUCT_TYPE_VALUES {
        let found = emulator
            .search_offerings(&json!({ "productType": product }))
            .await;
        assert!(
            !found.is_empty(),
            "case productType {product:?}: the service sells Savings Plans for it, so the search must answer with the offerings that cover it, got none"
        );
        let got: Vec<String> = found
            .iter()
            .map(|offering| text(offering, "offeringId"))
            .collect();
        let expected: Vec<String> = catalog
            .iter()
            .filter(|offering| covers(offering, product))
            .map(|offering| text(offering, "offeringId"))
            .collect();
        assert_eq!(
            got, expected,
            "case productType {product:?}: expected exactly the catalog rows covering it ({} of them), got {}",
            expected.len(),
            got.len()
        );
        for offering in &found {
            let id = text(offering, "offeringId");
            assert!(
                covers(offering, product),
                "case productType {product:?}: offering {id:?} does not cover it, got {:?}",
                product_types(offering)
            );
            for covered in product_types(offering) {
                assert!(
                    modelled.contains(covered.as_str()),
                    "case productType {product:?}: offering {id:?} advertises {covered:?}, which the model does not declare"
                );
            }
            // Every covered product is purchasable: the row names a plan type
            // the model declares, which is what `CreateSavingsPlan` buys.
            assert!(
                matches!(
                    text(offering, "planType").as_str(),
                    "Compute" | "EC2Instance" | "SageMaker" | "Database"
                ),
                "case productType {product:?}: offering {id:?} reports planType {:?}, which the model does not declare",
                text(offering, "planType")
            );
        }
    }

    // One step outside the published value set: a product type the model does
    // not declare is refused by its own enum constraint rather than searched
    // for — including a value that differs only in case from one it declares.
    for case in ["Aurora", "ec2", "DynamoDb", "Snowball"] {
        let answer = emulator
            .call(
                "DescribeSavingsPlansOfferings",
                json!({ "productType": case, "maxResults": 1000 }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case productType {case:?}: a value outside the model's enum must be refused, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.list("searchResults").is_empty(),
            "case productType {case:?}: a refused search must answer with no offering, got {:?}",
            answer.body
        );
    }
}

/// A Savings Plan can be bought for every product type the model declares, and
/// the plan reports the coverage of the offering it was bought from.
#[tokio::test]
async fn a_plan_can_be_bought_for_every_product_type_the_catalog_sells() {
    let emulator = Emulator::new();
    // The commitment varies from row to row — the documented floor, whole and
    // fractional amounts, and one at the widest scale a request may carry — so
    // the rule is not asserted at a single amount.
    let commitments = ["0.001", "0.1", "1", "2.5", "137.25", "0.12345"];

    for (index, product) in SAVINGS_PLAN_PRODUCT_TYPE_VALUES.iter().enumerate() {
        let answer = emulator
            .call(
                "DescribeSavingsPlansOfferings",
                json!({ "productType": product, "maxResults": 1 }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case productType {product:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let offering = answer
            .list("searchResults")
            .first()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case productType {product:?}: the catalog must sell an offering covering it, got {:?}",
                    answer.body
                )
            });
        let offering_id = text(&offering, "offeringId");
        let commitment = commitments[index % commitments.len()];

        let savings_plan_id = emulator
            .purchase(json!({
                "savingsPlanOfferingId": offering_id,
                "commitment": commitment,
                "clientToken": format!("product-type-coverage-{product}"),
            }))
            .await;
        let plan = emulator.plan(&savings_plan_id).await;

        assert!(
            covers(&plan, product),
            "case productType {product:?}: the plan bought from offering {offering_id:?} must report it, got {:?}",
            product_types(&plan)
        );
        assert_eq!(
            product_types(&plan),
            product_types(&offering),
            "case productType {product:?}: the plan's coverage must be the offering's, got {:?}",
            product_types(&plan)
        );
        assert_eq!(
            text(&plan, "savingsPlanType"),
            text(&offering, "planType"),
            "case productType {product:?}: the plan's type must be the offering's, got {:?}",
            text(&plan, "savingsPlanType")
        );
        assert_eq!(
            text(&plan, "offeringId"),
            offering_id,
            "case productType {product:?}: the plan must name the offering it was bought from, got {plan:?}"
        );
    }
}
