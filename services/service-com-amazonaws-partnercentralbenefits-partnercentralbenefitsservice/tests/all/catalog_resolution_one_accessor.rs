// SPDX-License-Identifier: CC0-1.0

//! The ONE catalog resolution the operations that bind a required `Catalog` and then read
//! the catalog's own records go through, driven end to end over the wire.
//!
//! Every assertion goes through the real request path — the shared
//! [`crate::benefit_application_lifecycle::Harness`], i.e. `MultiServiceRouter::dispatch`
//! with `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`.
//!
//! The rule under test: resolving the `Catalog` a request names and refusing a catalog
//! this service does not hold are ONE step, so an operation that goes on to read records
//! out of the catalog reads the very record that resolution returned — on the resolved
//! side the records the operation reports are the records that catalog holds, and on the
//! refused side the answer is the refusal the resolution itself builds.
//!
//! The rows span what the rule generalises over rather than one call: every operation that
//! reads the catalog record after resolving it (`CreateBenefitApplication`, which mints
//! into it, and the three listings `ListBenefits`, `ListBenefitApplications`,
//! `ListBenefitAllocations`, which page it), crossed with every catalog this service
//! holds, several page sizes (so a page break cannot decide the answer), and — on the
//! other side of the rule — several catalog names the modelled `CatalogName` pattern
//! admits (`^[A-Za-z0-9_-]+$`, `smithy_query(query="shape",shape="CatalogName")`, which
//! bounds the length not at all) that name no catalog this service holds.

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    service_state, Catalog, SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, text, Harness};
use crate::service_error_rules::{message_of, UNKNOWN_BENEFIT};

/// An upper bound on the pages [`listed_ids`] will follow, so a cursor that never ends
/// fails the test instead of looping for ever.
const MAX_PAGES: usize = 64;

/// What the state's single catalog resolution answers for `catalog` in this harness's own
/// store: the stored record it found, or the message of the refusal it failed with.
///
/// Both sides of every assertion below come from here, so a change to that one resolution
/// changes what every row expects.
fn resolved(harness: &Harness, catalog: &str) -> Result<Catalog, String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .catalog(catalog)
        .cloned()
        .map_err(|refusal| refusal.message)
}

/// One listing that reads the catalog record the resolution returned.
struct Listing {
    /// The `X-Amz-Target` operation name.
    operation: &'static str,
    /// The output member carrying the page's summaries.
    summaries: &'static str,
    /// The ids the resolved catalog record holds for the collection this listing pages, in
    /// the stored order.
    stored: fn(&Catalog) -> Vec<String>,
}

/// Every listing of this service, with the collection of the resolved record each one must
/// report.
const LISTINGS: [Listing; 3] = [
    Listing {
        operation: "ListBenefits",
        summaries: "BenefitSummaries",
        stored: |catalog| catalog.benefits.keys().cloned().collect(),
    },
    Listing {
        operation: "ListBenefitApplications",
        summaries: "BenefitApplicationSummaries",
        stored: |catalog| catalog.applications.keys().cloned().collect(),
    },
    Listing {
        operation: "ListBenefitAllocations",
        summaries: "BenefitAllocationSummaries",
        stored: |catalog| catalog.allocations.keys().cloned().collect(),
    },
];

/// Page sizes the rows are crossed with: the request naming none (the modelled maximum
/// stands in), and sizes small enough that the collections above break across pages.
const PAGE_SIZES: [Option<i32>; 3] = [None, Some(1), Some(2)];

/// Catalog names this service holds no catalog for, each admitted by the modelled
/// `CatalogName` pattern: a hyphenated name, digits only, an underscored name, a single
/// character and a long one.
fn unheld_catalogs() -> Vec<(&'static str, String)> {
    vec![
        ("a hyphenated name", "held-by-nothing-20261028".to_string()),
        ("digits only", "20261028203039".to_string()),
        ("an underscored name", "held_by_nothing".to_string()),
        ("a single character", "z".to_string()),
        ("a long name", "n".repeat(401)),
    ]
}

/// One well-formed request per operation under the rule, all naming `catalog`: the create
/// that mints into the catalog record, and the three listings that page it.
fn catalog_reading_requests(
    catalog: &str,
    benefit_identifier: &str,
    client_token: &str,
) -> Vec<(&'static str, Value)> {
    let mut requests = vec![(
        "CreateBenefitApplication",
        json!({
            "Catalog": catalog,
            "ClientToken": client_token,
            "BenefitIdentifier": benefit_identifier,
            "Name": "a catalog-resolution probe",
        }),
    )];
    for listing in &LISTINGS {
        requests.push((listing.operation, json!({ "Catalog": catalog })));
    }
    requests
}

/// Every id `listing` reports for `catalog`, following its cursor to the end so a page
/// break cannot change the set.
async fn listed_ids(
    harness: &Harness,
    case: &str,
    listing: &Listing,
    catalog: &str,
    max_results: Option<i32>,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..MAX_PAGES {
        let mut body = json!({ "Catalog": catalog });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let out = harness.ok(case, listing.operation, &body).await;
        let summaries = out[listing.summaries].as_array().cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?} on {}: expected a {} list, got {out:?}",
                listing.operation, listing.summaries
            )
        });
        for summary in &summaries {
            ids.push(text(case, summary, "Id"));
        }
        next_token = out["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return ids;
        }
        assert!(
            page + 1 < MAX_PAGES,
            "case {case:?} on {}: the listing still issued a cursor after {MAX_PAGES} pages, \
             got {ids:?}",
            listing.operation
        );
    }
    ids
}

/// The resolved side: for every catalog this service holds, each listing reports exactly
/// the records the single resolution hands back — and the create mints into that very
/// record.
#[tokio::test]
async fn every_catalog_reading_operation_reports_the_record_the_resolution_returns() {
    let harness = Harness::new();
    for (index, catalog) in SEEDED_CATALOGS.iter().enumerate() {
        let case = format!("the catalog {catalog:?}");
        let benefit_id = text(&case, &harness.active_benefit_of(catalog).await, "Id");

        // The create goes through the same resolution and mints into the record it
        // returns, so the application it answers with must be in that record afterwards —
        // and the listing below must then report it.
        let created = harness
            .create(
                &case,
                &json!({
                    "Catalog": catalog,
                    "ClientToken": format!("catalog-resolution-{index}"),
                    "BenefitIdentifier": benefit_id,
                    "Name": "a catalog-resolution probe",
                }),
            )
            .await;
        let created_id = text(&case, &created, "Id");
        let record = resolved(&harness, catalog).unwrap_or_else(|refusal| {
            panic!("case {case:?}: the resolution must hand back the held catalog, got {refusal:?}")
        });
        assert!(
            record.applications.contains_key(&created_id),
            "case {case:?}: expected the resolved record to hold the created {created_id:?}, \
             got {:?}",
            record.applications.keys().collect::<Vec<_>>()
        );

        for listing in &LISTINGS {
            let expected = (listing.stored)(&record);
            for max_results in PAGE_SIZES {
                let case = format!("{case} at MaxResults {max_results:?}");
                let reported = listed_ids(&harness, &case, listing, catalog, max_results).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?} on {}: expected the resolved record's ids {expected:?}, got \
                     {reported:?}",
                    listing.operation
                );
            }
        }
    }
}

/// The refused side: for a catalog this service holds none of, every one of the same
/// operations answers 404 with exactly the refusal the single resolution builds, and
/// writes nothing.
#[tokio::test]
async fn every_catalog_reading_operation_answers_the_refusal_the_resolution_builds() {
    let harness = Harness::new();
    // The whole store, before any refused request: a refused resolution reaches no record,
    // so a later caller must see exactly this.
    let before = store_snapshot(&harness);
    for (index, (case, catalog)) in unheld_catalogs().into_iter().enumerate() {
        assert!(
            !SEEDED_CATALOGS.contains(&catalog.as_str()),
            "case {case:?}: {catalog:?} must name no catalog this service holds"
        );
        let expected = resolved(&harness, &catalog).err().unwrap_or_else(|| {
            panic!("case {case:?}: the resolution must refuse the unheld catalog {catalog:?}")
        });
        // A `ClientToken` the modelled `[!-~]{1,64}` pattern admits, one per row, so the
        // token never stands in the way of the resolution under test.
        let requests = catalog_reading_requests(
            &catalog,
            UNKNOWN_BENEFIT,
            &format!("catalog-resolution-unheld-{index}"),
        );
        for (operation, body) in &requests {
            let (status, response) = harness.call(operation, body).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {operation}: expected 404, got {status} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?} on {operation}: expected ResourceNotFoundException, got \
                 {response:?}"
            );
            let message = message_of(&response).unwrap_or_else(|| {
                panic!("case {case:?} on {operation}: the answer carries no message: {response:?}")
            });
            assert_eq!(
                message, expected,
                "case {case:?} on {operation}: expected the resolution's refusal {expected:?}, \
                 got {message:?}"
            );
        }
    }
    let after = store_snapshot(&harness);
    assert_eq!(
        after, before,
        "a refused catalog resolution must change no stored record"
    );
}

/// Every catalog this store holds, rendered, so two instants of the store compare as a
/// whole.
///
/// Shared with `crate::write_gate_order`, which compares the store across a run of
/// refused writes.
pub(crate) fn store_snapshot(harness: &Harness) -> String {
    format!(
        "{:?}",
        service_state(&**harness.engine())
            .expect("the in-memory engine carries this service's state")
            .lock()
            .catalogs
    )
}
