// SPDX-License-Identifier: CC0-1.0

//! `ListBenefitAllocations`, end to end over the wire.
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
//! 1. **The report is the store**: the operation reports every benefit allocation the
//!    named catalog holds, field for field from the stored record, and nothing else; the
//!    ARN it reports is the allocation ARN of the region the request was signed for. Rows:
//!    every seeded catalog, every stored allocation in it, and an allocation AWS granted
//!    for an application.
//! 2. **A filter selects exactly the stored allocations whose field matches it**, an
//!    absent or empty filter selects all of them, and a filter naming something the
//!    catalog does not hold selects none. Rows: every `FulfillmentType` value, a filter
//!    carrying two of them (the member's modelled maximum), every
//!    `BenefitAllocationStatus` value, both identifier forms the modelled `BenefitId` and
//!    `BenefitApplicationIdentifier` admit (a bare id and an ARN), an unknown id, an ARN
//!    of another catalog, and two filters at once.
//! 3. **A page carries at most `MaxResults` summaries, and the `NextToken` resumes exactly
//!    where the page ended**: paging a filter's result set end to end yields that set
//!    once, in the same order, and the token is present exactly when another page
//!    remains. Rows: several population sizes and page sizes, each published `MaxResults`
//!    bound and one past it, and a token carried back with the filter it was issued under.
//! 4. **A `NextToken` this operation did not issue is refused** `ValidationException`
//!    (400) naming the member. Rows: text that is not a token, a token of another record
//!    kind, and a token whose body is not an allocation id.
//! 5. **An allocation past its `ExpiresAt` is no longer usable**, so it is never reported
//!    `ACTIVE`: the report materialises the expiry the clock already made due, and reading
//!    changes nothing else a later caller sees.
//! 6. **A catalog the service does not serve is refused** `ResourceNotFoundException`
//!    (404) with the service's catalog-lookup message.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::ops::types::{
    BENEFIT_ALLOCATION_STATUS_VALUES, FULFILLMENT_TYPE_VALUES,
};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    not_found_message, now_millis, service_state, BenefitAllocation, ALLOCATION_ID_PREFIX,
    ALLOCATION_RESOURCE, BENEFIT_RESOURCE, CATALOG_KIND, ID_BODY_LEN, MAX_PAGE_SIZE,
    SEEDED_CATALOGS,
};

use crate::benefit_application_lifecycle::{error_code, instant, Harness, CATALOG};
use crate::resource_tagging::{error_message, expected_arn};

/// The other catalog the service serves, so a row can name an ARN of a catalog that is not
/// the one the request asked about.
const OTHER_CATALOG: &str = "Sandbox";

/// An application id the modelled `BenefitApplicationIdentifier` admits and no create ever
/// minted, so a filter naming it resolves to nothing.
const UNKNOWN_APPLICATION: &str = "benappl-00000000000000";

/// A benefit id the modelled `BenefitId` admits and no catalog holds.
const UNKNOWN_BENEFIT: &str = "ben-00000000000000";

/// The allocations the store holds for `catalog`, in the order it keys them.
///
/// Read straight from the engine, so an assertion can compare the REPORT against the
/// stored record rather than against another report. Shared with
/// `crate::get_benefit_allocation`, which compares the single-allocation report against the
/// same records.
pub(crate) fn stored_allocations(harness: &Harness, catalog: &str) -> Vec<BenefitAllocation> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .catalogs
        .get(catalog)
        .map(|stored| stored.allocations.values().cloned().collect())
        .unwrap_or_default()
}

/// Have AWS grant an allocation of `benefit_identifier` in `catalog`, optionally as the
/// outcome of the application `application_identifier` names, and report the id it minted.
///
/// This is the out-of-API grant path (`grant_allocation`): the API models no operation that
/// creates an allocation, so a grant arrives from an in-process caller holding the engine,
/// standing in for AWS's own fulfillment workflow.
fn grant(
    harness: &Harness,
    catalog: &str,
    benefit_identifier: &str,
    application_identifier: Option<&str>,
    name: &str,
) -> Option<String> {
    service_state(&**harness.engine())
        .expect("the in-memory engine carries this service's state")
        .lock()
        .grant_allocation(
            catalog,
            benefit_identifier,
            application_identifier,
            name,
            now_millis(),
        )
}

/// `grant`, for a grant that must succeed.
pub(crate) fn granted(
    harness: &Harness,
    catalog: &str,
    benefit_identifier: &str,
    application_identifier: Option<&str>,
    name: &str,
) -> String {
    grant(
        harness,
        catalog,
        benefit_identifier,
        application_identifier,
        name,
    )
    .unwrap_or_else(|| {
        panic!("case {name:?}: granting an allocation of {benefit_identifier:?} in {catalog:?} must succeed")
    })
}

/// One `ListBenefitAllocations` call that must answer 200: its summaries and its
/// `NextToken`.
async fn list(harness: &Harness, case: &str, body: &Value) -> (Vec<Value>, Option<String>) {
    let out = harness.ok(case, "ListBenefitAllocations", body).await;
    let summaries = out["BenefitAllocationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary list, got {out:?}"))
        .clone();
    let token = out["NextToken"].as_str().map(str::to_string);
    (summaries, token)
}

/// The `Id` of each summary, in the order they were reported.
fn ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary["Id"].as_str().unwrap_or("<no Id>").to_string())
        .collect()
}

/// A string member of a summary, or `None` when the summary omits it.
fn member(summary: &Value, name: &str) -> Option<String> {
    summary.get(name).and_then(Value::as_str).map(str::to_string)
}

/// The entries of a list member, or an empty list when the summary omits it.
fn members(summary: &Value, name: &str) -> Vec<String> {
    summary
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `text` as lowercase hex, the encoding this service's pagination tokens use — so a row
/// can hand back a well-formed token of the WRONG kind.
fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// Every allocation of `catalog`, paged through with `max_results` per page: the ids in the
/// order they were reported, and how many pages it took.
async fn page_through(
    harness: &Harness,
    case: &str,
    filter: &Value,
    max_results: Option<i64>,
) -> (Vec<String>, usize) {
    let mut collected = Vec::new();
    let mut pages = 0;
    let mut token: Option<String> = None;
    loop {
        let mut body = filter.clone();
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let (summaries, next) = list(harness, case, &body).await;
        pages += 1;
        if let Some(max_results) = max_results {
            assert!(
                summaries.len() as i64 <= max_results,
                "case {case:?}: a page carries at most {max_results} summaries, got {}",
                summaries.len()
            );
        }
        if next.is_some() {
            assert_eq!(
                summaries.len() as i64,
                max_results.unwrap_or(i64::from(MAX_PAGE_SIZE)),
                "case {case:?}: a page that carries a NextToken must be full, got {}",
                summaries.len()
            );
        }
        collected.extend(ids(&summaries));
        token = next;
        if token.is_none() {
            return (collected, pages);
        }
        assert!(
            pages < 64,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
}

/// Rule 1: every stored allocation of every seeded catalog is reported, field for field
/// from the record, and the ids of the two catalogs' allocations are disjoint.
#[tokio::test]
async fn every_stored_allocation_is_reported_field_for_field_in_every_catalog() {
    let harness = Harness::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for catalog in SEEDED_CATALOGS {
        let (summaries, token) = list(&harness, catalog, &json!({ "Catalog": catalog })).await;
        // The store is read AFTER the call, so the expiries rule 5 covers are already
        // materialised and the two sides compare like for like.
        let stored = stored_allocations(&harness, catalog);

        assert!(
            !stored.is_empty(),
            "case {catalog:?}: the seeded catalog must hold allocations to report"
        );
        assert_eq!(
            token, None,
            "case {catalog:?}: a page holding every allocation carries no NextToken"
        );
        assert_eq!(
            summaries.len(),
            stored.len(),
            "case {catalog:?}: expected {} summaries, got {}",
            stored.len(),
            summaries.len()
        );
        assert_eq!(
            ids(&summaries),
            stored
                .iter()
                .map(|allocation| allocation.id.clone())
                .collect::<Vec<String>>(),
            "case {catalog:?}: the report must be in the store's own id order"
        );

        for allocation in &stored {
            let case = format!("{catalog}/{}", allocation.id);
            let summary = summaries
                .iter()
                .find(|summary| member(summary, "Id").as_deref() == Some(&allocation.id))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the stored allocation was not reported: {summaries:?}")
                });

            let body = allocation
                .id
                .strip_prefix(ALLOCATION_ID_PREFIX)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected an id starting {ALLOCATION_ID_PREFIX:?}, got {:?}",
                        allocation.id
                    )
                });
            assert!(
                body.len() == ID_BODY_LEN
                    && body
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
                "case {case:?}: the id body must be {ID_BODY_LEN} [0-9a-z] characters, got {body:?}"
            );
            assert!(
                seen_ids.insert(allocation.id.clone()),
                "case {case:?}: two catalogs must not share an allocation id"
            );

            assert_eq!(
                member(summary, "Catalog").as_deref(),
                Some(*catalog),
                "case {case:?}: expected Catalog {catalog:?}, got {summary:?}"
            );
            assert_eq!(
                member(summary, "Arn"),
                Some(expected_arn(catalog, ALLOCATION_RESOURCE, &allocation.id)),
                "case {case:?}: expected the allocation ARN of the signed region, got {summary:?}"
            );
            assert_eq!(
                member(summary, "Name").as_deref(),
                Some(allocation.name.as_str()),
                "case {case:?}: expected the stored Name, got {summary:?}"
            );
            assert_eq!(
                member(summary, "Status").as_deref(),
                Some(allocation.status.as_str()),
                "case {case:?}: expected the stored Status, got {summary:?}"
            );
            assert!(
                BENEFIT_ALLOCATION_STATUS_VALUES.contains(&allocation.status.as_str()),
                "case {case:?}: {:?} is not a modelled BenefitAllocationStatus",
                allocation.status
            );
            assert_eq!(
                member(summary, "StatusReason"),
                allocation.status_reason.clone(),
                "case {case:?}: expected the stored StatusReason, got {summary:?}"
            );
            assert_eq!(
                member(summary, "BenefitId").as_deref(),
                Some(allocation.benefit_id.as_str()),
                "case {case:?}: expected the stored BenefitId, got {summary:?}"
            );
            assert_eq!(
                member(summary, "BenefitApplicationId"),
                allocation.benefit_application_id.clone(),
                "case {case:?}: expected the stored BenefitApplicationId, got {summary:?}"
            );
            assert_eq!(
                members(summary, "FulfillmentTypes"),
                allocation.fulfillment_types,
                "case {case:?}: expected the stored FulfillmentTypes, got {summary:?}"
            );
            assert!(
                members(summary, "FulfillmentTypes")
                    .iter()
                    .all(|value| FULFILLMENT_TYPE_VALUES.contains(&value.as_str())),
                "case {case:?}: every FulfillmentTypes entry must be a modelled value, got {summary:?}"
            );
            assert_eq!(
                members(summary, "ApplicableBenefitIds"),
                allocation.applicable_benefit_ids,
                "case {case:?}: expected the stored ApplicableBenefitIds, got {summary:?}"
            );
            assert!(
                members(summary, "ApplicableBenefitIds").contains(&allocation.benefit_id),
                "case {case:?}: the allocation's own benefit is applicable to it, got {summary:?}"
            );
            assert_eq!(
                instant(&case, summary, "CreatedAt"),
                allocation.created_at_millis,
                "case {case:?}: expected the stored CreatedAt, got {summary:?}"
            );
            assert_eq!(
                instant(&case, summary, "ExpiresAt"),
                allocation.expires_at_millis,
                "case {case:?}: expected the stored ExpiresAt, got {summary:?}"
            );
            assert!(
                allocation.expires_at_millis > allocation.starts_at_millis,
                "case {case:?}: an allocation's term must be non-empty, got {allocation:?}"
            );
        }
    }
}

/// Rule 2: each filter selects exactly the stored allocations whose field matches it.
#[tokio::test]
async fn each_filter_selects_exactly_the_stored_allocations_that_match_it() {
    let harness = Harness::new();
    // One unfiltered call first, so the expiries rule 5 covers are settled and the
    // expected sets below are computed from the same records the rows are checked against.
    let (unfiltered, _) = list(&harness, "unfiltered", &json!({ "Catalog": CATALOG })).await;
    let stored = stored_allocations(&harness, CATALOG);
    assert_eq!(
        ids(&unfiltered).len(),
        stored.len(),
        "the unfiltered page must report every stored allocation"
    );

    // An allocation of an application, so the application filter has something to select;
    // its benefit is the one the application was created for.
    let benefit = harness.active_benefit().await;
    let benefit_identifier = benefit["Id"].as_str().expect("the benefit reports an Id");
    let created = harness
        .create(
            "an application with an allocation",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-allocation-filter",
                "BenefitIdentifier": benefit_identifier,
                "Name": "an application with an allocation",
            }),
        )
        .await;
    let application_id = created["Id"]
        .as_str()
        .expect("the create reports an Id")
        .to_string();
    let application_arn = created["Arn"]
        .as_str()
        .expect("the create reports an Arn")
        .to_string();
    let allocation_of_application = granted(
        &harness,
        CATALOG,
        benefit_identifier,
        Some(&application_id),
        "an allocation of an application",
    );

    let stored = stored_allocations(&harness, CATALOG);
    // The benefit of a seeded allocation, for the identifier rows: taken from the store so
    // the row cannot drift from the seeded data.
    let seeded_benefit = stored
        .iter()
        .find(|allocation| allocation.benefit_application_id.is_none())
        .map(|allocation| allocation.benefit_id.clone())
        .expect("a seeded allocation is granted directly, with no application");

    type Predicate = Box<dyn Fn(&BenefitAllocation) -> bool>;
    let mut rows: Vec<(String, Value, Predicate)> = vec![
        (
            "no filter at all".to_string(),
            json!({}),
            Box::new(|_: &BenefitAllocation| true),
        ),
        (
            "an empty FulfillmentTypes list".to_string(),
            json!({ "FulfillmentTypes": [] }),
            Box::new(|_: &BenefitAllocation| true),
        ),
        (
            "an empty Status list".to_string(),
            json!({ "Status": [] }),
            Box::new(|_: &BenefitAllocation| true),
        ),
        (
            "an empty BenefitIdentifiers list".to_string(),
            json!({ "BenefitIdentifiers": [] }),
            Box::new(|_: &BenefitAllocation| true),
        ),
        (
            "both FulfillmentType values of one benefit".to_string(),
            json!({ "FulfillmentTypes": ["CASH", "CREDITS"] }),
            Box::new(|allocation: &BenefitAllocation| {
                allocation
                    .fulfillment_types
                    .iter()
                    .any(|value| value == "CASH" || value == "CREDITS")
            }),
        ),
        (
            "a benefit named by its bare id".to_string(),
            json!({ "BenefitIdentifiers": [seeded_benefit.clone()] }),
            {
                let wanted = seeded_benefit.clone();
                Box::new(move |allocation: &BenefitAllocation| allocation.benefit_id == wanted)
            },
        ),
        (
            "a benefit named by its ARN".to_string(),
            json!({
                "BenefitIdentifiers": [expected_arn(CATALOG, BENEFIT_RESOURCE, &seeded_benefit)],
            }),
            {
                let wanted = seeded_benefit.clone();
                Box::new(move |allocation: &BenefitAllocation| allocation.benefit_id == wanted)
            },
        ),
        (
            "a benefit id no catalog holds".to_string(),
            json!({ "BenefitIdentifiers": [UNKNOWN_BENEFIT] }),
            Box::new(|_: &BenefitAllocation| false),
        ),
        (
            "a benefit ARN of another catalog".to_string(),
            json!({
                "BenefitIdentifiers": [
                    expected_arn(OTHER_CATALOG, BENEFIT_RESOURCE, &seeded_benefit),
                ],
            }),
            Box::new(|_: &BenefitAllocation| false),
        ),
        (
            "the application that resulted in an allocation, by id".to_string(),
            json!({ "BenefitApplicationIdentifiers": [application_id.clone()] }),
            {
                let wanted = application_id.clone();
                Box::new(move |allocation: &BenefitAllocation| {
                    allocation.benefit_application_id.as_deref() == Some(wanted.as_str())
                })
            },
        ),
        (
            "the same application, by ARN".to_string(),
            json!({ "BenefitApplicationIdentifiers": [application_arn.clone()] }),
            {
                let wanted = application_id.clone();
                Box::new(move |allocation: &BenefitAllocation| {
                    allocation.benefit_application_id.as_deref() == Some(wanted.as_str())
                })
            },
        ),
        (
            "an application nothing holds".to_string(),
            json!({ "BenefitApplicationIdentifiers": [UNKNOWN_APPLICATION] }),
            Box::new(|_: &BenefitAllocation| false),
        ),
        (
            "a status and a fulfillment type at once".to_string(),
            json!({ "Status": ["ACTIVE"], "FulfillmentTypes": ["CREDITS"] }),
            Box::new(|allocation: &BenefitAllocation| {
                allocation.status == "ACTIVE"
                    && allocation.fulfillment_types.iter().any(|v| v == "CREDITS")
            }),
        ),
        (
            "a status that excludes nothing it should keep".to_string(),
            json!({ "Status": ["ACTIVE"], "BenefitApplicationIdentifiers": [application_id.clone()] }),
            {
                let wanted = application_id.clone();
                Box::new(move |allocation: &BenefitAllocation| {
                    allocation.status == "ACTIVE"
                        && allocation.benefit_application_id.as_deref() == Some(wanted.as_str())
                })
            },
        ),
    ];
    // Every value of each filtered enum, so the rows span the catalogue rather than one
    // example of it. Sources: `smithy_query(query="shape",shape="FulfillmentType")` and
    // `shape="BenefitAllocationStatus"`, re-exported as the two constants below.
    for value in FULFILLMENT_TYPE_VALUES {
        rows.push((
            format!("the fulfillment type {value}"),
            json!({ "FulfillmentTypes": [value] }),
            {
                let wanted = (*value).to_string();
                Box::new(move |allocation: &BenefitAllocation| {
                    allocation.fulfillment_types.contains(&wanted)
                })
            },
        ));
    }
    for value in BENEFIT_ALLOCATION_STATUS_VALUES {
        rows.push((format!("the status {value}"), json!({ "Status": [value] }), {
            let wanted = (*value).to_string();
            Box::new(move |allocation: &BenefitAllocation| allocation.status == wanted)
        }));
    }

    let mut matched_something = 0;
    for (case, filter, matches) in &rows {
        let mut body = filter.clone();
        body["Catalog"] = json!(CATALOG);
        let (summaries, token) = list(&harness, case, &body).await;
        let expected: Vec<String> = stored
            .iter()
            .filter(|allocation| matches(allocation))
            .map(|allocation| allocation.id.clone())
            .collect();
        assert_eq!(
            ids(&summaries),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {:?}",
            ids(&summaries)
        );
        assert_eq!(
            token, None,
            "case {case:?}: a page holding every match carries no NextToken"
        );
        if !expected.is_empty() {
            matched_something += 1;
        }
    }
    assert!(
        matched_something >= rows.len() - 4,
        "the table must mostly select something: only {matched_something} of {} rows did",
        rows.len()
    );
    // The allocation of the application is reported with that application's id, so the
    // member is a fact about the record and not an empty member nothing ever fills.
    let (summaries, _) = list(
        &harness,
        "the allocation of an application",
        &json!({ "Catalog": CATALOG, "BenefitApplicationIdentifiers": [application_id.clone()] }),
    )
    .await;
    assert_eq!(
        ids(&summaries),
        vec![allocation_of_application],
        "the application filter must select the allocation granted for it"
    );
    assert_eq!(
        member(&summaries[0], "BenefitApplicationId"),
        Some(application_id),
        "the allocation reports the application that resulted in it, got {summaries:?}"
    );
}

/// Rule 1 and 2 on the grant path's own refusals: a grant naming something the store does
/// not hold records nothing, so no report can carry an allocation of a benefit or an
/// application that does not exist.
#[tokio::test]
async fn a_grant_naming_nothing_the_store_holds_records_no_allocation() {
    let harness = Harness::new();
    let before = stored_allocations(&harness, CATALOG).len();

    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        ("a catalog the service does not serve", "NotACatalog", UNKNOWN_BENEFIT, None),
        ("a benefit no catalog holds", CATALOG, UNKNOWN_BENEFIT, None),
        (
            "a benefit ARN of another catalog",
            CATALOG,
            "arn:aws:partnercentral:us-west-2:123456789012:catalog/Sandbox/benefit/ben-00000000000000",
            None,
        ),
    ];
    for (case, catalog, benefit, application) in &rows {
        let granted = grant(&harness, catalog, benefit, *application, case);
        assert!(
            granted.is_none(),
            "case {case:?}: the grant must record nothing, got {granted:?}"
        );
    }

    assert_eq!(
        stored_allocations(&harness, CATALOG).len(),
        before,
        "a refused grant must leave the store exactly as it was"
    );
}

/// Rule 3: a page carries at most `MaxResults` summaries and the token resumes exactly
/// where it ended, for several populations, page sizes and filters.
#[tokio::test]
async fn paging_walks_each_result_set_once_in_the_store_s_order() {
    let harness = Harness::new();
    let whole = json!({ "Catalog": CATALOG });
    let (full, _) = page_through(&harness, "the seeded population", &whole, None).await;

    // Grow the population past the page sizes under test, so a page size smaller than the
    // set is exercised at several counts rather than only at the seeded one.
    let benefit = harness.active_benefit().await;
    let benefit_identifier = benefit["Id"].as_str().expect("the benefit reports an Id");
    for index in 0..7 {
        granted(
            &harness,
            CATALOG,
            benefit_identifier,
            None,
            &format!("a granted allocation {index}"),
        );
    }

    let expected: Vec<String> = stored_allocations(&harness, CATALOG)
        .iter()
        .map(|allocation| allocation.id.clone())
        .collect();
    assert_eq!(
        expected.len(),
        full.len() + 7,
        "the grants must have grown the population: {expected:?}"
    );

    // (case, the filter, MaxResults) — the page sizes span 1 (the modelled minimum), a
    // size that divides the set, one that does not, the set's own size, one past it, and
    // 100 (the modelled maximum).
    let filtered = json!({ "Catalog": CATALOG, "Status": ["ACTIVE"] });
    let rows: Vec<(String, &Value, Option<i64>)> = vec![
        ("no MaxResults at all".to_string(), &whole, None),
        ("MaxResults 1, the modelled minimum".to_string(), &whole, Some(1)),
        ("MaxResults 2".to_string(), &whole, Some(2)),
        ("MaxResults 3".to_string(), &whole, Some(3)),
        ("MaxResults 4".to_string(), &whole, Some(4)),
        (
            format!("MaxResults {}, the whole set", expected.len()),
            &whole,
            Some(expected.len() as i64),
        ),
        (
            format!("MaxResults {}, one past the whole set", expected.len() + 1),
            &whole,
            Some(expected.len() as i64 + 1),
        ),
        (
            format!("MaxResults {MAX_PAGE_SIZE}, the modelled maximum"),
            &whole,
            Some(i64::from(MAX_PAGE_SIZE)),
        ),
        ("MaxResults 1 under a status filter".to_string(), &filtered, Some(1)),
        ("MaxResults 2 under a status filter".to_string(), &filtered, Some(2)),
    ];

    for (case, filter, max_results) in &rows {
        let (walked, pages) = page_through(&harness, case, filter, *max_results).await;
        let (whole_page, _) = list(&harness, case, filter).await;
        let in_one_page = ids(&whole_page);
        assert_eq!(
            walked, in_one_page,
            "case {case:?}: paging must yield the set once, in the same order"
        );
        assert_eq!(
            walked.iter().collect::<BTreeSet<&String>>().len(),
            walked.len(),
            "case {case:?}: paging must not report a record twice: {walked:?}"
        );
        if let Some(max_results) = max_results {
            let max_results = *max_results as usize;
            let least = walked.len().div_ceil(max_results).max(1);
            assert!(
                pages == least || pages == least + 1,
                "case {case:?}: {} records at {max_results} per page is {least} pages \
                 (or one more holding the last token), got {pages}",
                walked.len()
            );
        }
    }

    // A `MaxResults` outside the modelled range is refused by the centralized walk, at
    // each published bound's far side.
    for max_results in [0, i64::from(MAX_PAGE_SIZE) + 1] {
        let case = format!("MaxResults {max_results}, outside the modelled range");
        let (status, out) = harness
            .call(
                "ListBenefitAllocations",
                &json!({ "Catalog": CATALOG, "MaxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
    }
}

/// Rule 4: a `NextToken` this operation did not issue is refused, naming the member.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_refused() {
    let harness = Harness::new();

    let rows: Vec<(&str, String)> = vec![
        ("text that is not hex at all", "not-a-token".to_string()),
        ("hex that is not an id", hex("hello")),
        (
            "a token of another record kind",
            hex("benappl-00000000000000"),
        ),
        ("a benefit id, not an allocation id", hex(UNKNOWN_BENEFIT)),
        ("an allocation id with a short body", hex("benalloc-0000")),
        (
            "an allocation id with a character the pattern excludes",
            hex("benalloc-ABCDEFGHIJKLMN"),
        ),
    ];

    for (case, token) in &rows {
        let (status, out) = harness
            .call(
                "ListBenefitAllocations",
                &json!({ "Catalog": CATALOG, "NextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
        assert!(
            error_message(&out).contains("NextToken"),
            "case {case:?}: the refusal must name NextToken, got {out:?}"
        );
    }

    // And a token this operation DID issue is accepted, so the rows above are refused for
    // being foreign and not because every token is.
    let (_, token) = list(
        &harness,
        "a token this operation issued",
        &json!({ "Catalog": CATALOG, "MaxResults": 1 }),
    )
    .await;
    let token = token.expect("a one-record page of the seeded catalog carries a NextToken");
    let (summaries, _) = list(
        &harness,
        "resuming from that token",
        &json!({ "Catalog": CATALOG, "MaxResults": 1, "NextToken": token }),
    )
    .await;
    assert_eq!(
        summaries.len(),
        1,
        "resuming must report the next record, got {summaries:?}"
    );
}

/// Rule 5: an allocation past its `ExpiresAt` is never reported `ACTIVE`; the report
/// materialises that expiry in the store, and changes nothing else a later caller sees.
#[tokio::test]
async fn an_allocation_past_its_expiry_is_reported_inactive_and_the_expiry_is_persisted() {
    let harness = Harness::new();

    // Before anything reads them, the seeded catalog holds exactly one allocation whose
    // term has already closed, and it is still in the status it was granted with.
    let before = stored_allocations(&harness, CATALOG);
    let now = now_millis();
    let due: Vec<&BenefitAllocation> = before
        .iter()
        .filter(|allocation| allocation.has_expired(now))
        .collect();
    assert_eq!(
        due.len(),
        1,
        "the seeded catalog must hold one allocation whose term has closed, got {due:?}"
    );
    assert_eq!(
        due[0].status, "ACTIVE",
        "the expiry must be what moves it, not a status seeded as already moved: {due:?}"
    );
    let expired_id = due[0].id.clone();

    let benefits_before = harness.benefits(CATALOG).await;
    let (first, _) = list(&harness, "the first report", &json!({ "Catalog": CATALOG })).await;

    // Every row of the report: nothing past its ExpiresAt is ACTIVE, and nothing inside
    // its term was moved.
    for summary in &first {
        let case = member(summary, "Id").unwrap_or_default();
        let expires_at = instant(&case, summary, "ExpiresAt");
        let status = member(summary, "Status").unwrap_or_default();
        if expires_at <= now_millis() {
            assert_ne!(
                status, "ACTIVE",
                "case {case:?}: an allocation past its ExpiresAt is not usable, got {summary:?}"
            );
        }
        if case == expired_id {
            assert_eq!(
                status, "INACTIVE",
                "case {case:?}: an expired allocation is INACTIVE, got {summary:?}"
            );
            assert!(
                member(summary, "StatusReason").is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: the expiry must explain itself, got {summary:?}"
            );
        }
    }

    // The expiry is MATERIALISED: the store itself moved, so a reader that never calls
    // this operation sees the same thing.
    let after = stored_allocations(&harness, CATALOG);
    let moved = after
        .iter()
        .find(|allocation| allocation.id == expired_id)
        .expect("the expired allocation is still stored");
    assert_eq!(
        moved.status, "INACTIVE",
        "the expiry must be written to the store, got {moved:?}"
    );
    assert!(
        moved.updated_at_millis > moved.created_at_millis,
        "materialising the expiry must advance UpdatedAt, got {moved:?}"
    );

    // And the read changed nothing else: a second call answers exactly what the first did,
    // and the benefit catalog beside it is untouched.
    let (second, _) = list(&harness, "the second report", &json!({ "Catalog": CATALOG })).await;
    assert_eq!(
        first, second,
        "two reads of the same state must answer identically"
    );
    assert_eq!(
        benefits_before,
        harness.benefits(CATALOG).await,
        "reporting allocations must not change the benefit catalog"
    );
}

/// Rule 6: a catalog the service does not serve is refused, with the catalog-lookup
/// message; every catalog it does serve is served.
#[tokio::test]
async fn a_catalog_the_service_does_not_serve_is_refused_not_found() {
    let harness = Harness::new();

    // Each row satisfies the modelled `CatalogName` pattern `^[A-Za-z0-9_-]+$`, so the
    // only thing that can refuse it is the lookup.
    for catalog in ["NotACatalog", "aws", "AWS-2", "Sandbox_1"] {
        let (status, out) = harness
            .call("ListBenefitAllocations", &json!({ "Catalog": catalog }))
            .await;
        assert_eq!(
            status, 404,
            "case {catalog:?}: expected 404, got {status} with {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ResourceNotFoundException",
            "case {catalog:?}: expected ResourceNotFoundException, got {out:?}"
        );
        assert_eq!(
            error_message(&out),
            not_found_message(CATALOG_KIND, catalog),
            "case {catalog:?}: expected the catalog-lookup message, got {out:?}"
        );
    }

    for catalog in SEEDED_CATALOGS {
        let (summaries, _) = list(&harness, catalog, &json!({ "Catalog": catalog })).await;
        assert!(
            !summaries.is_empty(),
            "case {catalog:?}: a catalog the service serves reports its allocations"
        );
    }
}
