// SPDX-License-Identifier: CC0-1.0

//! What a benefit allocation reports for `FulfillmentType` when its record holds NONE, and
//! what the store records for a benefit that offers none — end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>` — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! `crate::allocation_fulfillment_types` owns the rule for a record that HOLDS a fulfillment
//! type (the reported value is the allocation's own, not its benefit's offer list, and the
//! filter selects on it). The two rules under test here are the other half — the absent
//! value, and the write that must not produce an undeclared one — each stated over a CLASS of
//! inputs and checked as a table whose rows differ along what the rule generalises over:
//!
//! 1. **A record the store holds no fulfillment type for reports NONE**: an enum-typed
//!    member carries only a value the modelled `FulfillmentType` enum declares, so
//!    `GetBenefitAllocation$FulfillmentType` is ABSENT for such a record rather than an
//!    unmodelled empty string, `BenefitAllocationSummary$FulfillmentTypes` says the same for
//!    the same record, and no value of the `FulfillmentTypes` filter selects it — while the
//!    catalog's other allocations keep reporting their own. Rows: every seeded catalog, each
//!    checked with its record's own type recorded and then with nothing recorded.
//! 2. **The store records an allocation only with a fulfillment type its benefit offers**: a
//!    grant for a benefit that offers one records that type, and a grant for a benefit that
//!    offers none records no allocation at all rather than one carrying a value outside the
//!    modelled enum. Rows: a benefit offering no type, one offering each modelled type, and
//!    benefits offering two of them.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::FULFILLMENT_TYPE_VALUES;
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, Benefit, BENEFIT_ID_PREFIX, FULFILLMENT_TYPE_ACCESS, FULFILLMENT_TYPE_CASH,
    FULFILLMENT_TYPE_CREDITS, ID_BODY_LEN, SEEDED_CATALOGS,
};

use crate::allocation_fulfillment_types::reported;
use crate::benefit_allocations::{grant, member, members, stored_allocations};
use crate::benefit_application_lifecycle::{Harness, CATALOG};
use crate::get_benefit_allocation::get;

/// The `BenefitStatus` value a benefit this suite adds carries, so it is offered exactly as
/// a seeded one is.
const ACTIVE: &str = "ACTIVE";

/// Leave the stored allocation `id` of `catalog` with NO fulfillment recorded, directly in
/// the store.
///
/// Every path that records an allocation records one of its benefit's offered types or no
/// allocation at all, so this state is reachable through no operation and no grant — which
/// is exactly why the rule it falls under is exercised by placing it here. The behaviour
/// under test is still read and asserted over the wire.
fn clear_fulfillment(harness: &Harness, case: &str, catalog: &str, id: &str) {
    let state = service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    let allocation = store
        .catalogs
        .get_mut(catalog)
        .and_then(|stored| stored.allocations.get_mut(id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: {id:?} names no stored allocation of {catalog:?}")
        });
    // No type, so no fulfillment behind one either.
    allocation.fulfillment_type = String::new();
    allocation.fulfillment = None;
}

/// Add a benefit offering `offers` to `catalog`, directly in the store, and report its id.
///
/// The catalog is seeded, read-only service data — no operation creates a benefit — so a
/// benefit whose offer list the seeds do not contain is placed here, exactly as a grant is.
fn add_benefit(harness: &Harness, catalog: &str, name: &str, offers: &[&str]) -> String {
    // `ben-` + ID_BODY_LEN `[0-9a-z]` characters, the form of the modelled `BenefitId`, so
    // a row's name must fit that body and draw on that alphabet.
    assert!(
        name.len() <= ID_BODY_LEN && name.chars().all(|c| c.is_ascii_lowercase()),
        "case {name:?}: a row's name must fit a BenefitId body of {ID_BODY_LEN} lowercase \
         ASCII characters"
    );
    let id = format!("{BENEFIT_ID_PREFIX}{name:0>width$}", width = ID_BODY_LEN);
    let state = service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    let stored = store
        .catalogs
        .get_mut(catalog)
        .unwrap_or_else(|| panic!("the service serves the catalog {catalog:?}"));
    stored.benefits.insert(
        id.clone(),
        Benefit {
            id: id.clone(),
            name: name.to_string(),
            description: format!("A benefit offering {offers:?}"),
            programs: Vec::new(),
            fulfillment_types: offers.iter().map(|offer| (*offer).to_string()).collect(),
            benefit_request_schema: None,
            status: ACTIVE.to_string(),
        },
    );
    id
}

/// The allocation ids `catalog` holds, in the order the store keys them.
fn stored_ids(harness: &Harness, catalog: &str) -> Vec<String> {
    stored_allocations(harness, catalog)
        .iter()
        .map(|allocation| allocation.id.clone())
        .collect()
}

/// The summary `ListBenefitAllocations` reports for `id` in `catalog` under `filter`, read
/// through the shared report helper.
async fn summary_of(
    harness: &Harness,
    case: &str,
    catalog: &str,
    id: &str,
    filter: &Value,
) -> Option<Value> {
    reported(harness, case, catalog, filter)
        .await
        .into_iter()
        .find(|summary| member(summary, "Id").as_deref() == Some(id))
}

/// The `FulfillmentTypes` of the summary of `id`, and the `FulfillmentType` of its single
/// report — the two reports of ONE record, which must say the same.
async fn both_reports(
    harness: &Harness,
    case: &str,
    catalog: &str,
    id: &str,
) -> (Option<String>, Vec<String>) {
    let body = get(harness, case, catalog, id).await;
    let summary = summary_of(harness, case, catalog, id, &json!({}))
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the catalog must report {id:?}"));
    (member(&body, "FulfillmentType"), members(&summary, "FulfillmentTypes"))
}

/// Rule 1: a record the store holds no fulfillment type for reports none, in both reports,
/// and is selected by no `FulfillmentTypes` filter.
#[tokio::test]
async fn an_allocation_with_no_recorded_fulfillment_type_reports_none() {
    let harness = Harness::new();

    for catalog in SEEDED_CATALOGS {
        let ids = stored_ids(&harness, catalog);
        let (id, others) = ids.split_first().unwrap_or_else(|| {
            panic!("case {catalog:?}: the seeded catalog must hold an allocation")
        });

        // The control row: with a fulfillment type recorded, both reports carry it and the
        // filter for it selects the record — so the assertions below cannot pass by the
        // member being absent all along.
        let case = format!("{catalog}/{id}/recorded");
        let (single, summary_types) = both_reports(&harness, &case, catalog, id).await;
        let recorded = single.unwrap_or_else(|| {
            panic!("case {case:?}: a seeded allocation is granted with a fulfillment type")
        });
        assert!(
            FULFILLMENT_TYPE_VALUES.contains(&recorded.as_str()),
            "case {case:?}: FulfillmentType {recorded:?} is not a modelled FulfillmentType"
        );
        assert_eq!(
            summary_types,
            vec![recorded.clone()],
            "case {case:?}: expected the summary to report {recorded:?}, got {summary_types:?}"
        );
        let selected = summary_of(
            &harness,
            &case,
            catalog,
            id,
            &json!({ "FulfillmentTypes": [recorded] }),
        )
        .await;
        assert!(
            selected.is_some(),
            "case {case:?}: {recorded:?} must select the record it is recorded for"
        );

        // The row under test: nothing recorded, so nothing reported — in either report.
        let case = format!("{catalog}/{id}/nothing recorded");
        clear_fulfillment(&harness, &case, catalog, id);
        let (single, summary_types) = both_reports(&harness, &case, catalog, id).await;
        assert_eq!(
            single, None,
            "case {case:?}: an allocation no fulfillment type was recorded for reports no \
             FulfillmentType, got {single:?}"
        );
        assert!(
            summary_types.is_empty(),
            "case {case:?}: the summary of that same record reports no FulfillmentTypes, got \
             {summary_types:?}"
        );
        for wanted in FULFILLMENT_TYPE_VALUES {
            let case = format!("{case} filtered by {wanted}");
            let selected = summary_of(
                &harness,
                &case,
                catalog,
                id,
                &json!({ "FulfillmentTypes": [wanted] }),
            )
            .await;
            assert!(
                selected.is_none(),
                "case {case:?}: a record with no fulfillment recorded is selected by no \
                 fulfillment type, got {selected:?}"
            );
        }

        // Only that record changed: every other allocation of the catalog still reports the
        // type it was granted with.
        for other in others {
            let case = format!("{catalog}/{other}/unaffected");
            let (single, summary_types) = both_reports(&harness, &case, catalog, other).await;
            let single = single.unwrap_or_else(|| {
                panic!("case {case:?}: this record still holds a fulfillment type")
            });
            assert_eq!(
                summary_types,
                vec![single.clone()],
                "case {case:?}: expected both reports to carry {single:?}, got {summary_types:?}"
            );
        }
    }
}

/// Rule 2: a grant records the fulfillment the benefit offers, and records nothing for a
/// benefit that offers none.
#[tokio::test]
async fn a_grant_records_an_allocation_only_with_a_fulfillment_type_its_benefit_offers() {
    let harness = Harness::new();

    // (the benefit's name, the `FulfillmentTypes` it OFFERS) — no type, each modelled type
    // on its own (source: `smithy_query(query="shape",shape="FulfillmentType")`, re-exported
    // as FULFILLMENT_TYPE_VALUES), and two of them.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("offersnothing", Vec::new()),
        ("offerscash", vec![FULFILLMENT_TYPE_CASH]),
        ("offerscredits", vec![FULFILLMENT_TYPE_CREDITS]),
        ("offersaccess", vec![FULFILLMENT_TYPE_ACCESS]),
        (
            "offerscashcred",
            vec![FULFILLMENT_TYPE_CASH, FULFILLMENT_TYPE_CREDITS],
        ),
        (
            "offersaccesses",
            vec![FULFILLMENT_TYPE_ACCESS, FULFILLMENT_TYPE_CASH],
        ),
    ];

    for (name, offers) in &rows {
        let case = format!("a benefit offering {offers:?}");
        let benefit_id = add_benefit(&harness, CATALOG, name, offers);
        let before = stored_ids(&harness, CATALOG);
        let granted = grant(&harness, CATALOG, &benefit_id, None, &case);

        let Some(first_offered) = offers.first() else {
            assert_eq!(
                granted, None,
                "case {case:?}: a benefit offering no fulfillment type can fulfill nothing, \
                 so the grant must record no allocation, got {granted:?}"
            );
            assert_eq!(
                stored_ids(&harness, CATALOG),
                before,
                "case {case:?}: a refused grant must leave the catalog's allocations as they \
                 were"
            );
            continue;
        };

        let id = granted.unwrap_or_else(|| {
            panic!("case {case:?}: granting an allocation of {benefit_id:?} must succeed")
        });
        let (single, summary_types) = both_reports(&harness, &case, CATALOG, &id).await;
        assert_eq!(
            single.as_deref(),
            Some(*first_offered),
            "case {case:?}: an allocation is fulfilled with one of its benefit's offered \
             types, got {single:?}"
        );
        assert_eq!(
            summary_types,
            vec![(*first_offered).to_string()],
            "case {case:?}: expected the summary to report {first_offered:?}, got \
             {summary_types:?}"
        );
    }
}
