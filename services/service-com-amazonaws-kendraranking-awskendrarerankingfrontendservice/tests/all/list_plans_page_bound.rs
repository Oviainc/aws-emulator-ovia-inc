// SPDX-License-Identifier: CC0-1.0

//! One rule: every `ListRescoreExecutionPlans` response is a PAGE — bounded by the
//! `MaxResults` the request named, or by this service's default page size when it named
//! none — and it carries a `NextToken` exactly when plans remain beyond it.
//!
//! The table varies what the rule generalises over: how many plans the store holds
//! (none, one, one short of a default page, exactly a default page, one past it, and the
//! store's own ceiling, which is wider than one page) and what page size the request
//! asks for (none at all, the smallest the model admits, a size that divides neither
//! population, and the largest the model admits). A row whose population exceeds its
//! page size is what pins the cursor: the walk has to visit every plan exactly once
//! across the pages, and no page may be wider than the bound that row implies.
//!
//! The bound rows at the end pin the page size the model admits: `MaxResults` is
//! `@range(min: 1, max: 50)` (`smithy_query(query="operation",
//! operation="ListRescoreExecutionPlans")`), so each published bound is accepted and one
//! past each is refused — while the table above is what holds the default page, served
//! when a request names no `MaxResults`, within that same maximum.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{call, create_plan, emulator, error_code};

/// One row: a store holding `plans` plans, listed with `max_results` (`None` for a
/// request that names no page size at all).
struct Case {
    case: String,
    plans: usize,
    max_results: Option<i64>,
}

/// The rows of the page-bound table.
fn cases() -> Vec<Case> {
    let default_page = state::DEFAULT_LISTED_PLANS_PER_PAGE;
    let ceiling = state::MAX_RESCORE_EXECUTION_PLANS;
    let modelled_max = state::MAX_LISTED_PLANS_PER_PAGE as i64;

    // The populations the rule spans: the empty store, a single plan, the two sides of a
    // default page, and the whole store, which holds more than one page.
    let populations: Vec<(&str, usize)> = vec![
        ("an empty store", 0),
        ("a single plan", 1),
        ("one short of a default page", default_page - 1),
        ("exactly a default page", default_page),
        ("one past a default page", default_page + 1),
        ("the store's whole ceiling", ceiling),
    ];
    // The page sizes a request may carry, including naming none.
    let sizes: Vec<(&str, Option<i64>)> = vec![
        ("no MaxResults", None),
        ("the smallest page the model admits", Some(1)),
        ("a page size dividing neither population", Some(7)),
        ("the largest page the model admits", Some(modelled_max)),
    ];

    let mut rows = Vec::new();
    for (population_label, plans) in &populations {
        for (size_label, max_results) in &sizes {
            // A one-plan-at-a-time walk over the whole ceiling says nothing the
            // one-past-a-page row does not, at a hundred times the calls.
            if *plans == ceiling && *max_results == Some(1) {
                continue;
            }
            rows.push(Case {
                case: format!("{population_label} listed with {size_label}"),
                plans: *plans,
                max_results: *max_results,
            });
        }
    }
    rows
}

/// One page of a listing: the plan ids it reported, and the token it handed back.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut body = json!({});
    if let Some(size) = max_results {
        body["MaxResults"] = json!(size);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    let (status, answer) = call(router, "ListRescoreExecutionPlans", body).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {answer}");
    let ids = answer["SummaryItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SummaryItems list, got {answer}"))
        .iter()
        .map(|item| {
            item["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary with no Id in {answer}"))
                .to_string()
        })
        .collect();
    let token = answer["NextToken"].as_str().map(str::to_string);
    (ids, token)
}

#[tokio::test]
async fn a_listing_never_exceeds_its_page_bound_and_tokens_the_remainder() {
    let default_page = state::DEFAULT_LISTED_PLANS_PER_PAGE;

    for row in cases() {
        let router = emulator();
        let mut created = Vec::new();
        for n in 0..row.plans {
            created.push(create_plan(&router, &format!("page-plan-{n}")).await);
        }
        created.sort();

        let case = row.case.as_str();
        // What the request asked for, or the service's default when it asked for nothing.
        let bound = row
            .max_results
            .map_or(default_page, |size| usize::try_from(size).unwrap_or(default_page));

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (ids, next) = page(&router, case, row.max_results, token.as_deref()).await;
            pages += 1;
            assert!(
                ids.len() <= bound,
                "case {case:?}: a page must hold at most {bound} plans, got {}",
                ids.len()
            );
            assert!(
                ids.len() <= state::MAX_LISTED_PLANS_PER_PAGE,
                "case {case:?}: no response may be wider than the modelled maximum of {}, got {}",
                state::MAX_LISTED_PLANS_PER_PAGE,
                ids.len()
            );
            seen.extend(ids);
            // The token says, and only says, that plans remain past what was served.
            let remaining = row.plans - seen.len();
            assert_eq!(
                next.is_some(),
                remaining > 0,
                "case {case:?}: with {remaining} plans left after {} seen, NextToken was {next:?}",
                seen.len()
            );
            token = next;
            if token.is_none() {
                break;
            }
            assert!(
                pages <= row.plans + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }

        // The walk is a partition of the store: every plan exactly once, in the stable
        // id order the pages are cut from.
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, created,
            "case {case:?}: the walk must visit every plan exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            row.plans,
            "case {case:?}: expected {} plans over {pages} pages, got {}",
            row.plans,
            seen.len()
        );
        // And it took the number of pages the bound implies — which is more than one
        // wherever the population outgrows the page, default page size included.
        let expected_pages = row.plans.div_ceil(bound).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages at {bound} per page, got {pages}"
        );
    }
}

#[tokio::test]
async fn the_default_page_sits_within_the_modelled_max_results_range() {
    let router = emulator();
    for n in 0..3 {
        create_plan(&router, &format!("bound-plan-{n}")).await;
    }

    let modelled_max = state::MAX_LISTED_PLANS_PER_PAGE as i64;
    // Each published bound of `MaxResults`, and one past each of them.
    let cases: Vec<(String, Value, u16)> = vec![
        ("the minimum the range admits".to_string(), json!(1), 200),
        (
            "the maximum the range admits".to_string(),
            json!(modelled_max),
            200,
        ),
        ("one below the minimum".to_string(), json!(0), 400),
        (
            "one past the maximum".to_string(),
            json!(modelled_max + 1),
            400,
        ),
    ];
    for (case, max_results, expected_status) in cases {
        let (status, answer) = call(
            &router,
            "ListRescoreExecutionPlans",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {answer}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answer),
                state::VALIDATION_EXCEPTION,
                "case {case:?}: got {answer}"
            );
        }
    }
}
