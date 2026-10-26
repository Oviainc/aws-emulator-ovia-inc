// SPDX-License-Identifier: CC0-1.0

//! Which fulfillment types a benefit allocation reports and is selected by, end to end
//! over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test, stated over a CLASS of inputs and checked as a table whose rows
//! differ along what it generalises over: **an allocation's fulfillment types are the ones
//! the allocation itself was fulfilled with, never the wider list of types the benefit it
//! is based on offers** — so `BenefitAllocationSummary$FulfillmentTypes` ("the fulfillment
//! types used for this benefit allocation") holds exactly the type
//! `GetBenefitAllocation$FulfillmentType` reports for the same record, and the
//! `FulfillmentTypes` filter selects an allocation on that same fact.
//!
//! Rows: every stored allocation of every seeded catalog (so every `FulfillmentType` the
//! model declares is covered, each against its benefit's own offer list), an allocation AWS
//! granted for a benefit that offers two types, every single-value filter, every pair of
//! distinct values (the member's modelled maximum length is 2), the empty filter and no
//! filter at all.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::FULFILLMENT_TYPE_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::SEEDED_CATALOGS;

use crate::benefit_allocations::{granted, ids, list, member, members};
use crate::benefit_application_lifecycle::{Harness, CATALOG};
use crate::get_benefit_allocation::get;

/// Every benefit of `catalog` by id, with the `FulfillmentTypes` it OFFERS.
///
/// Read from `ListBenefits`, whose summary member is the BENEFIT's offer list ("the
/// available fulfillment types for this benefit"): an allocation of that benefit is
/// fulfilled with one of its entries, which is the distinction under test.
async fn offered_types(harness: &Harness, catalog: &str) -> BTreeMap<String, Vec<String>> {
    harness
        .benefits(catalog)
        .await
        .iter()
        .map(|benefit| {
            (
                member(benefit, "Id").unwrap_or_default(),
                members(benefit, "FulfillmentTypes"),
            )
        })
        .collect()
}

/// Every allocation `catalog` reports under `filter`, as `(id, BenefitId)` pairs in the
/// order they were reported.
async fn reported(harness: &Harness, case: &str, catalog: &str, filter: &Value) -> Vec<Value> {
    let mut body = filter.clone();
    body["Catalog"] = json!(catalog);
    let (summaries, token) = list(harness, case, &body).await;
    assert_eq!(
        token, None,
        "case {case:?}: the population is far inside one page, so no NextToken is expected"
    );
    summaries
}

/// The `FulfillmentType` `GetBenefitAllocation` reports for each allocation of `catalog`,
/// keyed by allocation id — the sibling read of the same record, so the expectations below
/// come from the service's own answer rather than from its stored fields.
async fn fulfilled_with(harness: &Harness, catalog: &str) -> BTreeMap<String, String> {
    let summaries = reported(harness, catalog, catalog, &json!({})).await;
    let mut used = BTreeMap::new();
    for summary in &summaries {
        let id = member(summary, "Id").unwrap_or_default();
        let case = format!("{catalog}/{id}");
        let body = get(harness, &case, catalog, &id).await;
        used.insert(id, member(&body, "FulfillmentType").unwrap_or_default());
    }
    used
}

/// The report of every stored allocation names the fulfillment that allocation was granted
/// with, not its benefit's offer list.
#[tokio::test]
async fn the_reported_fulfillment_types_are_the_ones_used_for_the_allocation() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        let offered = offered_types(&harness, catalog).await;
        let summaries = reported(&harness, catalog, catalog, &json!({})).await;
        assert!(
            !summaries.is_empty(),
            "case {catalog:?}: the seeded catalog must hold allocations to report"
        );

        // How many rows check the report against a benefit offering MORE types than the
        // allocation used — the case the two facts can be told apart in.
        let mut rows_with_a_wider_offer_list = 0;
        for summary in &summaries {
            let id = member(summary, "Id").unwrap_or_default();
            let case = format!("{catalog}/{id}");
            let types = members(summary, "FulfillmentTypes");

            let body = get(&harness, &case, catalog, &id).await;
            let used = member(&body, "FulfillmentType").unwrap_or_default();
            assert_eq!(
                types,
                vec![used.clone()],
                "case {case:?}: expected the fulfillment used for the allocation \
                 [{used:?}], got {types:?}"
            );
            // Source of the value catalogue: `smithy_query(query="shape",
            // shape="FulfillmentType")`, re-exported as FULFILLMENT_TYPE_VALUES.
            assert!(
                FULFILLMENT_TYPE_VALUES.contains(&used.as_str()),
                "case {case:?}: {used:?} is not a modelled FulfillmentType"
            );

            let benefit_id = member(summary, "BenefitId").unwrap_or_default();
            let offers = offered.get(&benefit_id).unwrap_or_else(|| {
                panic!("case {case:?}: {benefit_id:?} names no benefit of the catalog")
            });
            assert!(
                offers.contains(&used),
                "case {case:?}: an allocation is fulfilled with one of its benefit's \
                 offered types {offers:?}, got {used:?}"
            );
            if offers.len() > 1 {
                rows_with_a_wider_offer_list += 1;
                assert_ne!(
                    &types, offers,
                    "case {case:?}: the report must not be the benefit's whole offer list \
                     {offers:?}, got {types:?}"
                );
            }
        }
        assert!(
            rows_with_a_wider_offer_list > 0,
            "case {catalog:?}: the catalog must hold an allocation of a benefit offering \
             more than one fulfillment type, or the two facts cannot be told apart"
        );
    }
}

/// The `FulfillmentTypes` filter selects an allocation on the fulfillment it was granted
/// with, over every filter the member admits.
#[tokio::test]
async fn the_fulfillment_types_filter_selects_on_the_allocation_s_own_fulfillment() {
    let harness = Harness::new();

    // A benefit that offers two fulfillment types, and a second allocation of it: AWS
    // grants one fulfilled with the first type it offers, while the catalog already holds
    // one fulfilled with another. Two allocations of ONE benefit, fulfilled differently, is
    // what tells the allocation's own fulfillment apart from its benefit's offer list.
    let offered = offered_types(&harness, CATALOG).await;
    let (multi_benefit, multi_offers) = offered
        .iter()
        .find(|(_, offers)| offers.len() > 1)
        .expect("the seeded catalog offers a benefit with two fulfillment types");
    let granted_id = granted(
        &harness,
        CATALOG,
        multi_benefit,
        None,
        "an allocation of a benefit offering two fulfillment types",
    );

    let summaries = reported(&harness, "unfiltered", CATALOG, &json!({})).await;
    let order = ids(&summaries);
    let benefit_of: BTreeMap<String, String> = summaries
        .iter()
        .map(|summary| {
            (
                member(summary, "Id").unwrap_or_default(),
                member(summary, "BenefitId").unwrap_or_default(),
            )
        })
        .collect();
    let used = fulfilled_with(&harness, CATALOG).await;
    assert!(
        order.len() == used.len() && order.contains(&granted_id),
        "the unfiltered page must hold every allocation, including the granted one: {order:?}"
    );

    // (case, the filter, the types it selects) — every single value, every pair of
    // distinct values (the modelled maximum length is 2), the empty list and no filter.
    let mut rows: Vec<(String, Value, Option<Vec<String>>)> = vec![
        ("no FulfillmentTypes filter at all".to_string(), json!({}), None),
        (
            "an empty FulfillmentTypes list".to_string(),
            json!({ "FulfillmentTypes": [] }),
            None,
        ),
    ];
    for value in FULFILLMENT_TYPE_VALUES {
        rows.push((
            format!("the fulfillment type {value}"),
            json!({ "FulfillmentTypes": [value] }),
            Some(vec![(*value).to_string()]),
        ));
    }
    for (index, first) in FULFILLMENT_TYPE_VALUES.iter().enumerate() {
        for second in FULFILLMENT_TYPE_VALUES.iter().skip(index + 1) {
            rows.push((
                format!("the fulfillment types {first} and {second}"),
                json!({ "FulfillmentTypes": [first, second] }),
                Some(vec![(*first).to_string(), (*second).to_string()]),
            ));
        }
    }

    for (case, filter, wanted) in &rows {
        let selected = ids(&reported(&harness, case, CATALOG, filter).await);
        let expected: Vec<String> = order
            .iter()
            .filter(|id| {
                wanted.as_ref().is_none_or(|wanted| {
                    used.get(*id).is_some_and(|value| wanted.contains(value))
                })
            })
            .cloned()
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected exactly {expected:?}, got {selected:?}"
        );
    }

    // The crux, on the two allocations of the one benefit: each is selected by the type IT
    // was fulfilled with and by neither of the others, although their benefit offers both.
    let seeded_id = order
        .iter()
        .find(|id| {
            benefit_of.get(*id).map(String::as_str) == Some(multi_benefit.as_str())
                && *id != &granted_id
        })
        .expect("the seeded catalog holds an allocation of that benefit")
        .clone();
    let granted_used = used
        .get(&granted_id)
        .expect("the granted allocation reports a FulfillmentType")
        .clone();
    let seeded_used = used
        .get(&seeded_id)
        .expect("the seeded allocation reports a FulfillmentType")
        .clone();
    assert_ne!(
        granted_used, seeded_used,
        "the two allocations of {multi_benefit:?} must be fulfilled with different types \
         of its offer list {multi_offers:?}, got {granted_used:?} for both"
    );
    for (case, wanted, selected_id, excluded_id) in [
        (
            "the type the granted allocation was fulfilled with",
            &granted_used,
            &granted_id,
            &seeded_id,
        ),
        (
            "the type the seeded allocation was fulfilled with",
            &seeded_used,
            &seeded_id,
            &granted_id,
        ),
    ] {
        let selected = ids(&reported(
            &harness,
            case,
            CATALOG,
            &json!({ "FulfillmentTypes": [wanted] }),
        )
        .await);
        assert!(
            selected.contains(selected_id),
            "case {case:?}: {wanted:?} must select {selected_id:?}, got {selected:?}"
        );
        assert!(
            !selected.contains(excluded_id),
            "case {case:?}: {wanted:?} must not select {excluded_id:?}, whose benefit \
             offers {multi_offers:?} but which was fulfilled otherwise, got {selected:?}"
        );
    }
}
