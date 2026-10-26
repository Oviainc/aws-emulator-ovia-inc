// SPDX-License-Identifier: CC0-1.0

//! `ListDataSources`, end to end over HTTP: what a page holds, what resumes it, and
//! which listings are refused.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: a list call reports the data sources stored under
//! the API it names, in one order that does not depend on how the caller pages, cut
//! into a page of at most `maxResults` entries (the modelled maximum when the request
//! names none) and resumed by the `nextToken` the previous page handed back, which is
//! reported exactly while entries remain; each entry is the record `GetDataSource`
//! answers for that name; and the call is a pure read, so paging through an API leaves
//! every later reader seeing the same set.
//!
//! The rows span:
//!
//! 1. the count of data sources the API holds — none, one, several, exactly one
//!    modelled page, and more than one page — so neither the page nor its cursor can
//!    be keyed to a single collection size;
//! 2. the `maxResults` the caller names — unspecified, and each of several values
//!    inside the modelled range, including its published maximum;
//! 3. the `DataSourceType` of the listed data sources, rotated across every value the
//!    model declares, so the projection cannot be keyed to one kind of data source;
//! 4. the forms of a listing that addresses nothing: an `apiId` too long to name an
//!    API, an `apiId` of the right shape this account does not hold, and tokens that
//!    name no page of the list.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract
//! suite's; the type catalog, the role ARN and the body builder are the service-role
//! suite's — reused rather than copied.

use serde_json::Value;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_source_service_role::{
    get_data_source, request_body, DATA_SOURCE_TYPES, ROLE_ARN,
};
use crate::error_contract::{call, create_api, router, Answer};

/// The counts of data sources an API is listed at: none, one, several, exactly one
/// modelled page, and more than one page.
const COUNTS: &[usize] = &[0, 1, 4, state::DEFAULT_PAGE_SIZE, state::DEFAULT_PAGE_SIZE + 7];

/// The `maxResults` values a walk pages with: unspecified, the modelled default sent
/// explicitly, and values inside the modelled range up to its published maximum.
/// Source: the `maxResults` member of `ListDataSourcesRequest` (`@range` 0..25,
/// default 0 — i.e. unspecified, so `0` and an absent member select the same page).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(7), Some(25)];

/// One page of a listing: the names it reported, and the cursor that resumes after
/// them.
#[derive(Debug)]
struct ListedPage {
    names: Vec<String>,
    next_token: Option<String>,
    data_sources: Vec<Value>,
}

/// The name the data source at `index` of a seeded API carries. Zero-padded so the
/// creation order and the lexicographic order of the names agree, which lets a walk
/// assert the listing's order without assuming either.
fn seeded_name(index: usize) -> String {
    format!("ds_{index:04}")
}

/// Create `count` data sources under `api_id`, their types rotating across every
/// `DataSourceType` the model declares, and return their names in creation order.
async fn seed(router: &MultiServiceRouter, api_id: &str, count: usize) -> Vec<String> {
    let mut names = Vec::with_capacity(count);
    for index in 0..count {
        let (data_source_type, needs_role) = DATA_SOURCE_TYPES[index % DATA_SOURCE_TYPES.len()];
        let name = seeded_name(index);
        let created = call(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(request_body(
                &name,
                data_source_type,
                needs_role.then_some(ROLE_ARN),
                serde_json::json!({}),
            )),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case seed {name:?} of type {data_source_type}: expected 200, got {created:?}"
        );
        names.push(name);
    }
    names
}

/// Send one `ListDataSources`, with the `maxResults` and `nextToken` the caller names.
async fn list_page(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let path = if query.is_empty() {
        format!("/v1/apis/{api_id}/datasources")
    } else {
        format!("/v1/apis/{api_id}/datasources?{}", query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// One page as the suite reads it, asserting it is a 200 that reports the list member.
fn read_page(case: &str, answer: &Answer) -> ListedPage {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListDataSources 200, got {answer:?}"
    );
    let data_sources: Vec<Value> = answer.body["dataSources"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a dataSources list, got {answer:?}"))
        .clone();
    let names = data_sources
        .iter()
        .map(|data_source| {
            data_source["name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected every entry to carry a name, got {answer:?}")
                })
                .to_string()
        })
        .collect();
    ListedPage {
        names,
        next_token: answer.body["nextToken"].as_str().map(str::to_string),
        data_sources,
    }
}

/// Page through the whole API at `max_results` and return every entry, in the order
/// the listing reported them, asserting the page-size bound and the cursor contract on
/// each page.
async fn walk(
    router: &MultiServiceRouter,
    api_id: &str,
    max_results: Option<i32>,
    expected_total: usize,
) -> Vec<Value> {
    // An unspecified `maxResults` — absent, or the modelled default of `0` — selects
    // the whole modelled page.
    let bound = match max_results {
        Some(size) if size > 0 => size as usize,
        _ => state::DEFAULT_PAGE_SIZE,
    };
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A page carries at least one entry while entries remain, so the walk cannot take
    // more pages than there are entries, plus the single page of an empty listing.
    let page_budget = expected_total + 1;
    for page_index in 0..page_budget {
        let case = format!(
            "walk {api_id} at maxResults {max_results:?}, page {page_index} from {token:?}"
        );
        let answer = list_page(router, api_id, max_results, token.as_deref()).await;
        let page = read_page(&case, &answer);
        assert!(
            page.names.len() <= bound,
            "case {case}: expected at most {bound} entries, got {} ({:?})",
            page.names.len(),
            page.names
        );
        let remaining = expected_total - collected.len();
        assert_eq!(
            page.names.len(),
            remaining.min(bound),
            "case {case}: expected the page to be as full as the remainder allows, got {:?}",
            page.names
        );
        collected.extend(page.data_sources);
        // The cursor is reported exactly while entries remain after this page.
        let more = collected.len() < expected_total;
        assert_eq!(
            page.next_token.is_some(),
            more,
            "case {case}: expected a nextToken {}, got {:?}",
            if more { "while entries remain" } else { "on the last page" },
            page.next_token
        );
        match page.next_token {
            None => return collected,
            Some(cursor) => token = Some(cursor),
        }
    }
    panic!("case walk {api_id} at maxResults {max_results:?}: the walk never reached a last page");
}

/// Every walk of an API reports the same data sources in the same order, however the
/// caller pages, and every data source the API holds exactly once.
#[tokio::test]
async fn every_page_size_walks_the_whole_api_in_one_order() {
    let router = router();

    for count in COUNTS.iter().copied() {
        let api_id = create_api(&router, &format!("Api_list_data_sources_{count}")).await;
        let seeded = seed(&router, &api_id, count).await;

        let mut reference: Option<Vec<String>> = None;
        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(&router, &api_id, max_results, count).await;
            let names: Vec<String> = listed
                .iter()
                .map(|data_source| data_source["name"].as_str().unwrap_or_default().to_string())
                .collect();
            // Every seeded data source, exactly once: a set comparison that is blind
            // to the order, which the next assertion pins separately.
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                sorted, seeded,
                "case {count} data sources at maxResults {max_results:?}: expected every seeded \
                 name exactly once, got {names:?}"
            );
            match &reference {
                None => reference = Some(names),
                Some(reference) => assert_eq!(
                    &names, reference,
                    "case {count} data sources at maxResults {max_results:?}: expected the same \
                     order as the first walk {reference:?}, got {names:?}"
                ),
            }
        }
    }
}

/// Each listed entry is the record `GetDataSource` answers for that name, member for
/// member — so a listing projects the stored data source rather than a summary of it.
#[tokio::test]
async fn a_listed_entry_is_the_record_get_answers() {
    let router = router();
    let api_id = create_api(&router, "Api_list_data_sources_projection").await;
    // One data source per `DataSourceType`, so the projection is asserted on every
    // kind of configuration the model declares.
    let count = DATA_SOURCE_TYPES.len();
    seed(&router, &api_id, count).await;

    for max_results in PAGE_SIZES.iter().copied() {
        let listed = walk(&router, &api_id, max_results, count).await;
        for entry in &listed {
            let name = entry["name"].as_str().unwrap_or_default();
            let read = get_data_source(&router, &api_id, name).await;
            assert_eq!(
                read.status, 200,
                "case listed {name:?} at maxResults {max_results:?}: expected GetDataSource 200, \
                 got {read:?}"
            );
            assert_eq!(
                &read.body["dataSource"], entry,
                "case listed {name:?} at maxResults {max_results:?}: expected the listing to \
                 answer the stored record {:?}, got {entry:?}",
                read.body["dataSource"]
            );
        }
    }
}

/// A listing reports the data sources of the API it names and no others, even where
/// two APIs hold a data source of the same name.
#[tokio::test]
async fn a_listing_reports_only_the_api_it_names() {
    let router = router();
    let first = create_api(&router, "Api_list_data_sources_scope_first").await;
    let second = create_api(&router, "Api_list_data_sources_scope_second").await;

    // The second API holds FEWER data sources, and every name it holds the first one
    // holds too, so a listing that crossed APIs would be caught either way.
    let first_names = seed(&router, &first, 6).await;
    let second_names = seed(&router, &second, 2).await;

    for (api_id, expected) in [(&first, &first_names), (&second, &second_names)] {
        let listed = walk(&router, api_id, Some(1), expected.len()).await;
        let mut names: Vec<String> = listed
            .iter()
            .map(|data_source| data_source["name"].as_str().unwrap_or_default().to_string())
            .collect();
        names.sort();
        assert_eq!(
            &names, expected,
            "case the listing of {api_id}: expected exactly its own data sources {expected:?}, \
             got {names:?}"
        );
        // Every entry is named under the API that holds it, so the two APIs' data
        // sources of the same name are different resources.
        for data_source in &listed {
            let name = data_source["name"].as_str().unwrap_or_default();
            let arn = data_source["dataSourceArn"].as_str().unwrap_or_default();
            let resource = state::data_source_resource(api_id, name);
            assert!(
                arn.ends_with(&resource),
                "case the listing of {api_id}: expected {name:?} named under {resource}, got \
                 {arn:?}"
            );
        }
    }
}

/// Listing is a pure read: paging through an API changes nothing a later caller sees,
/// so the walk repeats identically and every entry still reads back.
#[tokio::test]
async fn paging_leaves_the_stored_set_unchanged() {
    let router = router();
    let api_id = create_api(&router, "Api_list_data_sources_pure_read").await;
    let count = state::DEFAULT_PAGE_SIZE + 3;
    seed(&router, &api_id, count).await;

    // A first walk at the smallest page, so the listing is paged all the way through.
    let first = walk(&router, &api_id, Some(1), count).await;
    // ... then the same walk again, and one at the modelled maximum.
    let again = walk(&router, &api_id, Some(1), count).await;
    let whole = walk(&router, &api_id, Some(25), count).await;

    assert_eq!(
        again, first,
        "case a repeated walk: expected the same entries as the first walk {first:?}, got \
         {again:?}"
    );
    assert_eq!(
        whole, first,
        "case a walk at the modelled maximum after paging: expected the same entries {first:?}, \
         got {whole:?}"
    );
    for index in 0..count {
        let name = seeded_name(index);
        let read = get_data_source(&router, &api_id, &name).await;
        assert_eq!(
            read.status, 200,
            "case {name:?} after paging: expected GetDataSource 200, got {read:?}"
        );
    }
}

/// A listing that addresses nothing is refused: an `apiId` too long to name an API is a
/// 400, one of the right shape this account does not hold a 404, and a token that names
/// no page of the list AppSync's fixed invalid-token text.
#[tokio::test]
async fn a_listing_that_addresses_nothing_is_refused() {
    let router = router();
    let api_id = create_api(&router, "Api_list_data_sources_refusals").await;
    seed(&router, &api_id, 3).await;

    // An `apiId` longer than an AppSync API id holds more characters than one has, so
    // it names no API at all; an id of the right length this account simply does not
    // hold names nothing.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    let absent = "z".repeat(state::API_ID_LEN);
    for (api, expected_status, expected_error) in [
        (unnameable.as_str(), 400, "BadRequestException"),
        (absent.as_str(), 404, "NotFoundException"),
    ] {
        for max_results in PAGE_SIZES.iter().copied() {
            let answer = list_page(&router, api, max_results, None).await;
            assert_eq!(
                answer.status, expected_status,
                "case list under {api:?} at maxResults {max_results:?}: expected \
                 {expected_status}, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, expected_error,
                "case list under {api:?} at maxResults {max_results:?}: expected \
                 {expected_error}, got {answer:?}"
            );
        }
    }

    // Tokens that name no page of this list: a name the API does not hold, one an
    // earlier page already passed, and values of other shapes entirely.
    for token in ["bogus", "0", "ds_9999", "da2-nosuchkey"] {
        let answer = list_page(&router, &api_id, None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case resuming from {token:?}: expected status 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case resuming from {token:?}: expected BadRequestException, got {answer:?}"
        );
        assert_eq!(
            answer.message, "Invalid pagination token given.",
            "case resuming from {token:?}: expected AppSync's fixed invalid-token text, got \
             {answer:?}"
        );
    }

    // A token the listing itself handed back still resumes, so the refusals above are
    // about tokens that name no page rather than about paging at all.
    let first = list_page(&router, &api_id, Some(1), None).await;
    let page = read_page("the first page of the refusal API", &first);
    let cursor = page
        .next_token
        .expect("the first page of three data sources carries a nextToken");
    let resumed = list_page(&router, &api_id, Some(1), Some(&cursor)).await;
    let resumed = read_page("resuming from the listing's own token", &resumed);
    assert_eq!(
        resumed.names.len(),
        1,
        "case resuming from the listing's own token: expected one entry, got {:?}",
        resumed.names
    );
    assert_ne!(
        resumed.names, page.names,
        "case resuming from the listing's own token: expected the next entry, got {:?}",
        resumed.names
    );
}
