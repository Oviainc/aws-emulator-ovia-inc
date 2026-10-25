// SPDX-License-Identifier: CC0-1.0

//! `ListLongTermPricing` (and the `CreateLongTermPricing` that persists what it lists),
//! driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `ListLongTermPricing` reports one `LongTermPricingListEntry` per long-term pricing
//! contract this emulator holds, in the order they were created, paging them with
//! `MaxResults` as the page size and a `NextToken` that resumes the walk where the previous
//! page stopped; each entry reports the contract stored under that id — the term and the
//! device type it was created for, whether it renews itself, the window it runs for (the
//! create's instant to the same day of the month a whole number of calendar months later)
//! and the status the clock puts it in — together with the jobs ordered under it and which
//! of those still has a device. A token this service did not mint for this listing is
//! refused with `InvalidNextTokenException`.
//!
//! The rows differ along what the rule generalises over: how many contracts the store holds
//! (none, one, several — the listing is walked again after every create), the
//! `LongTermPricingType` (every value of the modelled catalog, so every term), the
//! `SnowballType` (several values of the modelled catalog), whether the create declared the
//! auto-renew flag and which way, how many jobs are ordered under the contract (none, one,
//! two, and an export order, which is a main job plus its job parts), whether those jobs
//! are still live or cancelled, the page size (absent, each published bound of the modelled
//! range, sizes that divide the listing and sizes that do not), and the form of the token
//! (unminted, truncated, altered, another listing's).
//!
//! The term arithmetic is asserted separately against a hand-written calendar table, since
//! no request can move this process's clock across a month boundary.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `complete_order`, `string`, `assert_prefixed_uuid`) is the one
//! `job_lifecycle` and `cluster_listing` already drive the service with, so every call here
//! goes through the same front door a client uses and nothing reaches into the state struct.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The job parts an export order is split into, the term each `LongTermPricingType` names and
// the calendar arithmetic a create applies to it, read from the crate rather than restated,
// so a row cannot drift from the behaviour it exercises.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_job::JOB_PARTS_PER_EXPORT_JOB;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_long_term_pricing::{
    plus_calendar_months, LONG_TERM_PRICING_TERM_MONTHS,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    LONG_TERM_PRICING_TYPE_VALUES, SNOWBALL_TYPE_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    encode_cursor, LONG_TERM_PRICING_STATUS_ACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_creation::UNKNOWN_LONG_TERM_PRICING_ID;
use super::job_lifecycle::{
    assert_prefixed_uuid, call_in_region, complete_order, create_address, deliverable_address, ok,
    refused, router, string,
};

/// The region every request in this suite signs for — the one the rest of this crate's
/// suites sign for, carried in the SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// `ListLongTermPricingRequest$MaxResults` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListLongTermPricing")`), so these are the
/// smallest and largest page sizes the member admits.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// How far the start date an entry reports may sit from this test's own clock reading: the
/// contract's term starts when the create is served, which is moments before the assertion.
const CLOCK_SKEW_SECONDS: f64 = 300.0;

/// The term `pricing_type` names, in whole calendar months, read from the crate's own table
/// rather than restated here.
fn term_months(pricing_type: &str) -> Option<u32> {
    LONG_TERM_PRICING_TERM_MONTHS
        .iter()
        .find(|(name, _)| *name == pricing_type)
        .map(|(_, months)| *months)
}

/// One job to order under a contract: the `JobType` it is placed for, and whether it is
/// cancelled once created.
struct Order {
    job_type: &'static str,
    cancel: bool,
}

/// One contract row: the term and device type to create it for, whether the create declares
/// the auto-renew flag, and the jobs ordered under it.
struct Row {
    case: String,
    pricing_type: &'static str,
    snowball_type: &'static str,
    auto_renew: Option<bool>,
    orders: Vec<Order>,
}

/// What one created row looks like once the store holds it: the id the service minted, and
/// the jobs ordered under it.
struct Contract {
    row: Row,
    long_term_pricing_id: String,
    /// The `JobId` every `CreateJob` under this contract answered with, in creation order.
    /// An export order answers with its main job's id; the job parts that order mints carry
    /// the contract too and are listed beside it.
    ordered_job_ids: Vec<String>,
    /// How many stored jobs the contract is expected to be associated with: one per plain
    /// order, and a main job plus its job parts per export order.
    expected_job_count: usize,
    /// The id of the newest ordered job that still has a device of its own, when it is one
    /// this suite knows the id of. `None` when no ordered job has a device left, and
    /// `Unknown` when the device-bearing job is one of an export order's parts, whose ids
    /// `CreateJob` does not answer with.
    expected_active_job: ActiveJob,
}

/// What this suite can say about the `CurrentActiveJob` a contract's entry must report.
#[derive(Debug, PartialEq, Eq)]
enum ActiveJob {
    /// No ordered job has a device of its own any more.
    None,
    /// The job with this id is the newest one that still has a device.
    Known(String),
    /// A job part of an export order holds the device, so the entry must report one of the
    /// associated jobs other than the order's main job.
    PartOfExportOrder { main_job_id: String },
}

/// Every contract row. The terms span the whole modelled `LongTermPricingType` catalog, the
/// device types several values of the modelled `SnowballType` catalog, the auto-renew flag
/// absent and both ways, and the orders none, one, two (one of them cancelled), an export
/// order (a main job plus its job parts) and one that is cancelled outright.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a one-year contract no job is ordered under".to_string(),
            pricing_type: "OneYear",
            snowball_type: "EDGE",
            auto_renew: None,
            orders: Vec::new(),
        },
        Row {
            case: "a renewing one-month contract with one import order".to_string(),
            pricing_type: "OneMonth",
            snowball_type: "EDGE_C",
            auto_renew: Some(true),
            orders: vec![Order {
                job_type: "IMPORT",
                cancel: false,
            }],
        },
        Row {
            case: "a three-year contract with a live order and a cancelled one".to_string(),
            pricing_type: "ThreeYear",
            snowball_type: "SNC1_SSD",
            auto_renew: Some(false),
            orders: vec![
                Order {
                    job_type: "IMPORT",
                    cancel: false,
                },
                Order {
                    job_type: "IMPORT",
                    cancel: true,
                },
            ],
        },
        Row {
            case: "a renewing one-year contract with an export order".to_string(),
            pricing_type: "OneYear",
            snowball_type: "V3_5S",
            auto_renew: Some(true),
            orders: vec![Order {
                job_type: "EXPORT",
                cancel: false,
            }],
        },
        Row {
            case: "a one-month contract whose only order was cancelled".to_string(),
            pricing_type: "OneMonth",
            snowball_type: "EDGE_S",
            auto_renew: None,
            orders: vec![Order {
                job_type: "IMPORT",
                cancel: true,
            }],
        },
        Row {
            case: "a renewing three-year contract for a rack device".to_string(),
            pricing_type: "ThreeYear",
            snowball_type: "RACK_5U_C",
            auto_renew: Some(true),
            orders: Vec::new(),
        },
    ]
}

/// The page sizes the listing is walked with: absent, each published bound of the modelled
/// range, and sizes that do and do not divide the listing.
fn page_sizes() -> Vec<(String, Option<i64>)> {
    let mut sizes: Vec<(String, Option<i64>)> = vec![
        ("no MaxResults".to_string(), None),
        (
            "MaxResults at the published minimum".to_string(),
            Some(MAX_RESULTS_MIN),
        ),
        (
            "MaxResults at the published maximum".to_string(),
            Some(MAX_RESULTS_MAX),
        ),
    ];
    for size in [1i64, 2, 4] {
        sizes.push((format!("MaxResults {size}"), Some(size)));
    }
    sizes
}

/// Create one row's contract and return the id the service minted for it.
async fn create_contract(router: &MultiServiceRouter, row: &Row) -> String {
    let mut body = json!({
        "LongTermPricingType": row.pricing_type,
        "SnowballType": row.snowball_type,
    });
    if let Some(auto_renew) = row.auto_renew {
        body["IsLongTermPricingAutoRenew"] = json!(auto_renew);
    }
    let created = ok(router, "CreateLongTermPricing", body, &row.case).await;
    let long_term_pricing_id = string(&created, "LongTermPricingId", &row.case);
    // The modelled `LongTermPricingId` form: `LTPID` plus a lowercase-hex 8-4-4-4-12 group.
    assert_prefixed_uuid(&long_term_pricing_id, "LTPID", &row.case);
    long_term_pricing_id
}

/// Place one row's orders against the contract `long_term_pricing_id`, and report what the
/// store then holds for it.
async fn place_orders(
    router: &MultiServiceRouter,
    address_id: &str,
    row: Row,
    long_term_pricing_id: String,
) -> Contract {
    let mut ordered_job_ids: Vec<String> = Vec::new();
    let mut expected_job_count = 0usize;
    let mut expected_active_job = ActiveJob::None;
    for order in &row.orders {
        let body = complete_order(&json!({
            "JobType": order.job_type,
            "SnowballType": row.snowball_type,
            "AddressId": address_id,
            "LongTermPricingId": long_term_pricing_id.clone(),
        }));
        let created = ok(router, "CreateJob", body, &row.case).await;
        let job_id = string(&created, "JobId", &row.case);
        let is_export_order = order.job_type == "EXPORT";
        // "A main job represents a successful request to create an export job … each main
        // job will have at least one job part, and each job part is associated with a
        // Snowball" (`JobListEntry$IsMaster`): an export order stores its main job and its
        // job parts, and every one of them is billed to the contract the order named.
        expected_job_count += if is_export_order {
            1 + JOB_PARTS_PER_EXPORT_JOB
        } else {
            1
        };
        if order.cancel {
            ok(
                router,
                "CancelJob",
                json!({ "JobId": job_id.clone() }),
                &row.case,
            )
            .await;
        }
        if !order.cancel {
            expected_active_job = if is_export_order {
                ActiveJob::PartOfExportOrder {
                    main_job_id: job_id.clone(),
                }
            } else {
                ActiveJob::Known(job_id.clone())
            };
        }
        ordered_job_ids.push(job_id);
    }
    Contract {
        row,
        long_term_pricing_id,
        ordered_job_ids,
        expected_job_count,
        expected_active_job,
    }
}

/// The entries of one `ListLongTermPricing` answer, reading an answer that reports no
/// contract as an empty listing: a store holding none has no entry to report.
fn entries(listed: &Value) -> Vec<Value> {
    listed
        .get("LongTermPricingEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A timestamp member of an entry, as the protocol puts an instant on the wire: epoch
/// seconds.
fn epoch_seconds(entry: &Value, member: &str, case: &str) -> f64 {
    entry
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a timestamp {member}, got {entry}"))
}

/// The `JobIds` an entry reports, reading an absent member as no associated job.
fn job_ids(entry: &Value) -> Vec<String> {
    entry
        .get("JobIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Walk `ListLongTermPricing` with `max_results` as the page size, returning the contract
/// ids in the order the pages reported them.
async fn page_through(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = ok(router, "ListLongTermPricing", body, case).await;
        let page = entries(&listed);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: expected at most {} pages for {expected_total} contracts, got {pages}",
            expected_total + 1
        );
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = &next_token {
            // `NextToken` is modelled `len 1..1024`, so a token this operation mints is one
            // the next request can carry.
            assert!(
                (1..=1024).contains(&token.chars().count()),
                "case {case:?}: expected a NextToken of 1..1024 characters, got {token:?}"
            );
        }
        // A declared page size bounds the page, and a page with contracts after it is full:
        // anything less would make the walk report fewer contracts per page than asked for
        // while still having more to report.
        if let Some(max) = max_results.filter(|max| *max > 0) {
            let max = usize::try_from(max).expect("a positive page size");
            assert!(
                page.len() <= max,
                "case {case:?}: expected at most {max} entries in a page, got {}",
                page.len()
            );
            if next_token.is_some() {
                assert_eq!(
                    page.len(),
                    max,
                    "case {case:?}: expected a page with contracts after it to be full, got {}",
                    page.len()
                );
            }
        }
        for entry in page {
            seen.push(string(&entry, "LongTermPricingId", case));
        }
        if next_token.is_none() {
            break;
        }
    }
    seen
}

/// Assert the entry `entry` reports the contract `contract` as it was created and as the
/// store now holds it.
fn assert_entry(entry: &Value, contract: &Contract) {
    let case = format!(
        "the entry for {} ({})",
        contract.long_term_pricing_id, contract.row.case
    );
    assert_eq!(
        string(entry, "LongTermPricingType", &case),
        contract.row.pricing_type,
        "case {case:?}: expected the term the contract was created for, got {entry}"
    );
    assert_eq!(
        string(entry, "SnowballType", &case),
        contract.row.snowball_type,
        "case {case:?}: expected the device type the contract was created for, got {entry}"
    );
    // A create that declared no flag ordered a contract that does not renew itself.
    let reported_auto_renew = entry
        .get("IsLongTermPricingAutoRenew")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    assert_eq!(
        reported_auto_renew,
        contract.row.auto_renew.unwrap_or(false),
        "case {case:?}: expected the auto-renew flag the create carried, got {entry}"
    );

    // The window: the term starts when the create was served and runs to the same day of
    // the month a whole number of calendar months later, which is the rule the crate's own
    // `long_term_pricing_end_date` states and the calendar table below pins.
    let started = epoch_seconds(entry, "LongTermPricingStartDate", &case);
    let ends = epoch_seconds(entry, "LongTermPricingEndDate", &case);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64();
    assert!(
        (now - started).abs() < CLOCK_SKEW_SECONDS,
        "case {case:?}: expected a start date at the create, got {started} against a clock \
         reading {now}"
    );
    let months = term_months(contract.row.pricing_type)
        .unwrap_or_else(|| panic!("case {case:?}: expected the term to name a length"));
    let expected_end = plus_calendar_months(UNIX_EPOCH + Duration::from_secs_f64(started), months)
        .expect("a term inside the range a timestamp can represent")
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64();
    assert!(
        (ends - expected_end).abs() < 1.0,
        "case {case:?}: expected the {} term to end at {expected_end}, got {ends}",
        contract.row.pricing_type
    );
    // Whatever the term, a contract created now is inside it.
    assert_eq!(
        string(entry, "LongTermPricingStatus", &case),
        LONG_TERM_PRICING_STATUS_ACTIVE,
        "case {case:?}: expected a contract inside its term to be reported active, got {entry}"
    );
    // Nothing has replaced the device of a contract no update has touched.
    assert!(
        entry.get("ReplacementJob").is_none(),
        "case {case:?}: expected no ReplacementJob on a contract no update has touched, got \
         {entry}"
    );

    // The jobs ordered under the contract, and which of them still has a device.
    let listed_jobs = job_ids(entry);
    assert_eq!(
        listed_jobs.len(),
        contract.expected_job_count,
        "case {case:?}: expected {} associated jobs, got {listed_jobs:?}",
        contract.expected_job_count
    );
    for ordered in &contract.ordered_job_ids {
        assert!(
            listed_jobs.contains(ordered),
            "case {case:?}: expected the job {ordered} ordered under the contract to be \
             associated with it, got {listed_jobs:?}"
        );
    }
    let active = entry
        .get("CurrentActiveJob")
        .and_then(Value::as_str)
        .map(str::to_string);
    match &contract.expected_active_job {
        ActiveJob::None => assert!(
            active.is_none(),
            "case {case:?}: expected no CurrentActiveJob while no ordered job has a device, \
             got {active:?}"
        ),
        ActiveJob::Known(job_id) => assert_eq!(
            active.as_deref(),
            Some(job_id.as_str()),
            "case {case:?}: expected the newest job that still has a device, got {active:?}"
        ),
        ActiveJob::PartOfExportOrder { main_job_id } => {
            let active = active.unwrap_or_else(|| {
                panic!("case {case:?}: expected a CurrentActiveJob, got {entry}")
            });
            // "Main jobs aren't associated with any Snowballs. Instead, each main job will
            // have at least one job part, and each job part is associated with a Snowball"
            // (`JobListEntry$IsMaster`), so the device of an export order is a part's.
            assert_ne!(
                &active, main_job_id,
                "case {case:?}: expected the device-bearing job part rather than the export \
                 order's main job, got {active}"
            );
            assert!(
                listed_jobs.contains(&active),
                "case {case:?}: expected the active job to be one of the associated jobs, got \
                 {active} against {listed_jobs:?}"
            );
        }
    }
}

#[tokio::test]
async fn list_long_term_pricing_pages_over_every_stored_contract_in_creation_order() {
    let router = router();

    // A store holding no contract has nothing to report and no page after this one.
    let empty_case = "a store holding no long-term pricing contract";
    let empty = ok(&router, "ListLongTermPricing", json!({}), empty_case).await;
    assert!(
        entries(&empty).is_empty(),
        "case {empty_case:?}: expected no entry, got {empty}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case {empty_case:?}: expected no NextToken on the last page, got {empty}"
    );

    let address_id = create_address(
        &router,
        deliverable_address("Pricing Orderer", "7 Billing Way", json!({})),
        "the address every job ordered under a contract ships to",
    )
    .await;

    // The listing is walked again after every create, so the rule is asserted over a store
    // holding one contract, two, and so on up to the whole table.
    let mut contracts: Vec<Contract> = Vec::new();
    for row in rows() {
        let long_term_pricing_id = create_contract(&router, &row).await;
        contracts.push(place_orders(&router, &address_id, row, long_term_pricing_id).await);
        let created: Vec<String> = contracts
            .iter()
            .map(|contract| contract.long_term_pricing_id.clone())
            .collect();
        for (size_case, max_results) in page_sizes() {
            let case = format!(
                "{} stored contracts, {size_case} ({})",
                created.len(),
                contracts
                    .last()
                    .expect("a contract was just created")
                    .row
                    .case
            );
            let seen = page_through(&router, max_results, created.len(), &case).await;
            assert_eq!(
                &seen, &created,
                "case {case:?}: expected the pages to report every stored contract once, in \
                 creation order, got {seen:?}"
            );
        }
    }

    // Each entry reports the contract stored under its id, and the whole modelled term
    // catalog is reached by the table.
    let one_page_case = "the whole listing on one page";
    let one_page = ok(
        &router,
        "ListLongTermPricing",
        json!({ "MaxResults": MAX_RESULTS_MAX }),
        one_page_case,
    )
    .await;
    let listed = entries(&one_page);
    assert_eq!(
        listed.len(),
        contracts.len(),
        "case {one_page_case:?}: expected every created contract to be listed once, got \
         {one_page}"
    );
    let mut terms: BTreeSet<String> = BTreeSet::new();
    for (entry, contract) in listed.iter().zip(contracts.iter()) {
        assert_eq!(
            string(entry, "LongTermPricingId", one_page_case),
            contract.long_term_pricing_id,
            "case {one_page_case:?}: expected the contracts in creation order, got {one_page}"
        );
        assert_entry(entry, contract);
        terms.insert(string(entry, "LongTermPricingType", one_page_case));
    }
    for term in LONG_TERM_PRICING_TYPE_VALUES {
        assert!(
            terms.contains(*term),
            "expected the listing to report a {term} contract, got {terms:?}"
        );
    }
    // Every device type a row ordered is one the modelled catalog admits.
    for contract in &contracts {
        assert!(
            SNOWBALL_TYPE_VALUES.contains(&contract.row.snowball_type),
            "expected {} to be a modelled SnowballType, got {:?}",
            contract.row.snowball_type,
            SNOWBALL_TYPE_VALUES
        );
    }

    // A read does not move what a later caller sees: the window each entry reports is the
    // one the create fixed, so listing again reports the same instants.
    let again = ok(
        &router,
        "ListLongTermPricing",
        json!({ "MaxResults": MAX_RESULTS_MAX }),
        "the listing read a second time",
    )
    .await;
    assert_eq!(
        entries(&again)
            .iter()
            .map(|entry| (
                entry.get("LongTermPricingId").cloned(),
                entry.get("LongTermPricingStartDate").cloned(),
                entry.get("LongTermPricingEndDate").cloned(),
            ))
            .collect::<Vec<_>>(),
        listed
            .iter()
            .map(|entry| (
                entry.get("LongTermPricingId").cloned(),
                entry.get("LongTermPricingStartDate").cloned(),
                entry.get("LongTermPricingEndDate").cloned(),
            ))
            .collect::<Vec<_>>(),
        "expected a second read to report the same contracts and windows, got {again}"
    );

    // A page size outside the modelled range is refused before the handler runs.
    for (case, max_results) in [
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
    ] {
        let (status, parsed) = call_in_region(
            &router,
            SIGNED_REGION,
            "ListLongTermPricing",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected MaxResults {max_results} to be refused, got {status} with \
             {parsed}"
        );
    }

    // An order billed to a contract this store does not hold is refused, so exactly the ids
    // this listing reports are the ones an order can name.
    refused(
        &router,
        "CreateJob",
        complete_order(&json!({
            "AddressId": address_id,
            "LongTermPricingId": UNKNOWN_LONG_TERM_PRICING_ID,
        })),
        "InvalidResourceException",
        "an order billed to a contract that does not exist",
    )
    .await;
}

#[tokio::test]
async fn list_long_term_pricing_refuses_a_token_it_did_not_mint_for_this_listing() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Token Orderer", "8 Billing Way", json!({})),
        "the address the token suite's job ships to",
    )
    .await;

    // Three contracts, so this listing has pages after its first, plus one job so the store
    // also holds a record a cursor of ANOTHER listing can name.
    let mut created: Vec<String> = Vec::new();
    for (index, pricing_type) in LONG_TERM_PRICING_TYPE_VALUES.iter().enumerate() {
        let row = Row {
            case: format!("contract {index} of the token suite"),
            pricing_type,
            snowball_type: "EDGE",
            auto_renew: None,
            orders: Vec::new(),
        };
        created.push(create_contract(&router, &row).await);
    }
    let job_case = "the job of the token suite";
    let job = ok(
        &router,
        "CreateJob",
        complete_order(&json!({
            "AddressId": address_id,
            "LongTermPricingId": created[0].clone(),
        })),
        job_case,
    )
    .await;
    let job_id = string(&job, "JobId", job_case);

    let first_case = "the first page of the contract listing";
    let first_page = ok(
        &router,
        "ListLongTermPricing",
        json!({ "MaxResults": 1 }),
        first_case,
    )
    .await;
    let token = string(&first_page, "NextToken", first_case);

    let mut altered = token.clone();
    altered.pop();
    altered.push(if token.ends_with('0') { '1' } else { '0' });

    let rows: Vec<(String, String)> = vec![
        ("not a cursor".to_string(), "garbage".to_string()),
        (
            "a cursor for a contract that does not exist".to_string(),
            encode_cursor(UNKNOWN_LONG_TERM_PRICING_ID),
        ),
        (
            "a truncated cursor".to_string(),
            token[..token.len() / 2].to_string(),
        ),
        ("an altered cursor".to_string(), altered),
        (
            "a cursor naming a job rather than a contract".to_string(),
            encode_cursor(&job_id),
        ),
    ];
    for (case, next_token) in rows {
        refused(
            &router,
            "ListLongTermPricing",
            json!({ "NextToken": next_token }),
            "InvalidNextTokenException",
            &case,
        )
        .await;
    }

    // The token this listing did mint still resumes the walk where the first page stopped.
    let second_page = ok(
        &router,
        "ListLongTermPricing",
        json!({ "MaxResults": 1, "NextToken": token }),
        "resuming with the minted token",
    )
    .await;
    let resumed = entries(&second_page);
    assert_eq!(
        resumed.len(),
        1,
        "expected the minted token to resume the walk, got {second_page}"
    );
    assert_eq!(
        string(&resumed[0], "LongTermPricingId", "the resumed page"),
        created[1],
        "expected the resumed page to start at the contract the first page stopped before, got \
         {second_page}"
    );
}

/// One calendar row: an instant a term starts at, the term, and the instant it must end at.
///
/// The dates are written out by hand from the Gregorian calendar — the arithmetic a term of
/// whole months follows — so the rule is pinned against the calendar rather than against the
/// implementation: a day-of-month that the end month does not have lands on that month's
/// last day, and February's length follows the leap year.
const CALENDAR_ROWS: &[(&str, i64, &str, &str, i64)] = &[
    // 2024-01-31T00:00:00Z plus one month is 2024-02-29T00:00:00Z: February 2024 has 29
    // days, so the 31st clamps to the 29th.
    (
        "2024-01-31T00:00:00Z",
        1_706_659_200,
        "OneMonth",
        "2024-02-29T00:00:00Z",
        1_709_164_800,
    ),
    // 2026-01-31T00:00:00Z plus one month is 2026-02-28T00:00:00Z: February 2026 has 28
    // days.
    (
        "2026-01-31T00:00:00Z",
        1_769_817_600,
        "OneMonth",
        "2026-02-28T00:00:00Z",
        1_772_236_800,
    ),
    // A leap day plus a year is the 28th of February in the following, non-leap year.
    (
        "2024-02-29T00:00:00Z",
        1_709_164_800,
        "OneYear",
        "2025-02-28T00:00:00Z",
        1_740_700_800,
    ),
    // The same leap day plus three years, 2027 being no leap year either.
    (
        "2024-02-29T00:00:00Z",
        1_709_164_800,
        "ThreeYear",
        "2027-02-28T00:00:00Z",
        1_803_772_800,
    ),
    // An ordinary day keeps its day of the month, whatever the term.
    (
        "2026-03-15T00:00:00Z",
        1_773_532_800,
        "OneMonth",
        "2026-04-15T00:00:00Z",
        1_776_211_200,
    ),
    (
        "2026-03-15T00:00:00Z",
        1_773_532_800,
        "OneYear",
        "2027-03-15T00:00:00Z",
        1_805_068_800,
    ),
    (
        "2026-03-15T00:00:00Z",
        1_773_532_800,
        "ThreeYear",
        "2029-03-15T00:00:00Z",
        1_868_227_200,
    ),
    // The time of day a term starts at is the time of day it ends at.
    (
        "2026-03-15T12:34:56Z",
        1_773_578_096,
        "OneMonth",
        "2026-04-15T12:34:56Z",
        1_776_256_496,
    ),
];

#[tokio::test]
async fn a_long_term_pricing_term_ends_a_whole_number_of_calendar_months_after_it_starts() {
    for (start_label, start_epoch, pricing_type, end_label, end_epoch) in CALENDAR_ROWS {
        let case = format!("a {pricing_type} term starting {start_label}");
        let start = UNIX_EPOCH
            + Duration::from_secs(u64::try_from(*start_epoch).expect("an instant after the epoch"));
        let months = term_months(pricing_type)
            .unwrap_or_else(|| panic!("case {case:?}: expected {pricing_type} to name a term"));
        let end = plus_calendar_months(start, months).unwrap_or_else(|| {
            panic!("case {case:?}: expected the term to end inside the timestamp range")
        });
        let end_secs = end
            .duration_since(UNIX_EPOCH)
            .expect("an instant after the epoch")
            .as_secs();
        assert_eq!(
            i64::try_from(end_secs).expect("an instant inside the calendar"),
            *end_epoch,
            "case {case:?}: expected the term to end {end_label} ({end_epoch}), got {end_secs}"
        );
    }
}
