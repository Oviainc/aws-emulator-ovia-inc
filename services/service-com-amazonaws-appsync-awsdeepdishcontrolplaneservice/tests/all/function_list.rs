// SPDX-License-Identifier: CC0-1.0

//! `ListFunctions`, end to end over HTTP: what a page holds, what resumes it, and
//! which listings are refused.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: a list call reports the functions stored under the
//! GraphQL API it names, in one order that does not depend on how the caller pages, cut
//! into a page of at most `maxResults` entries (the modelled maximum when the request
//! names none) and resumed by the `nextToken` the previous page handed back, which is
//! reported exactly while entries remain; each entry is the record `GetFunction`
//! answers for that id; and the call is a pure read, so paging through an API leaves
//! every later reader seeing the same set.
//!
//! The rows span:
//!
//! 1. the count of functions the API holds — none, one, several, exactly one modelled
//!    page, and more than one page — so neither the page nor its cursor can be keyed to
//!    a single collection size;
//! 2. the `maxResults` the caller names — unspecified, the modelled default sent
//!    explicitly, and values inside the modelled range up to its published maximum;
//! 3. the definition form of the listed functions, rotated across the whole
//!    round-trip table of the `CreateFunction` suite (VTL mapping templates with a
//!    `functionVersion`, and `code` with a `runtime`, at each published `maxBatchSize`
//!    bound and each `SyncConfig` strategy), so the projection cannot be keyed to one
//!    kind of function — and, since that table repeats names, a listing of functions
//!    sharing a name as well;
//! 4. the forms of a listing that addresses nothing: an `apiId` too long to name an
//!    API, an `apiId` of the right shape this account does not hold, and tokens that
//!    name no page of the list.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract suite's;
//! the function table, the data-source helper and the readers are the `CreateFunction`
//! suite's — reused rather than copied.

use serde_json::Value;
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router, Answer};
use crate::functions::{create_data_source, create_function, get_function, rows};

/// The counts of functions an API is listed at: none, one, several, exactly one
/// modelled page, and more than one page.
const COUNTS: &[usize] = &[0, 1, 4, state::DEFAULT_PAGE_SIZE, state::DEFAULT_PAGE_SIZE + 7];

/// The `maxResults` values a walk pages with: unspecified, the modelled default sent
/// explicitly, and values inside the modelled range up to its published maximum.
/// Source: the `maxResults` member of `ListFunctionsRequest` (`@range` 0..25, default
/// 0 — i.e. unspecified, so `0` and an absent member select the same page).
const PAGE_SIZES: &[Option<i32>] = &[None, Some(0), Some(1), Some(2), Some(7), Some(25)];

/// The name of the one data source the seeded functions resolve against.
const DATA_SOURCE: &str = "ds_functions";

/// One page of a listing: the entries it reported, and the cursor that resumes after
/// them.
#[derive(Debug)]
struct ListedPage {
    ids: Vec<String>,
    next_token: Option<String>,
    functions: Vec<Value>,
}

/// Create `count` functions under `api_id`, their definitions rotating across the whole
/// `CreateFunction` round-trip table, and return their minted ids in creation order.
async fn seed(router: &MultiServiceRouter, api_id: &str, count: usize) -> Vec<String> {
    create_data_source(router, api_id, DATA_SOURCE).await;
    let table = rows();
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let row = &table[index % table.len()];
        let created = create_function(router, api_id, DATA_SOURCE, row).await;
        assert_eq!(
            created.status, 200,
            "case seed {} #{index}: expected CreateFunction 200, got {created:?}",
            row.label
        );
        let function_id = created.body["functionConfiguration"]["functionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case seed {} #{index}: expected a minted functionId, got {created:?}",
                    row.label
                )
            })
            .to_string();
        ids.push(function_id);
    }
    ids
}

/// Send one `ListFunctions`, with the `maxResults` and `nextToken` the caller names.
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
        format!("/v1/apis/{api_id}/functions")
    } else {
        format!("/v1/apis/{api_id}/functions?{}", query.join("&"))
    };
    call(router, "GET", &path, None).await
}

/// One page as the suite reads it, asserting it is a 200 that reports the list member.
fn read_page(case: &str, answer: &Answer) -> ListedPage {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected ListFunctions 200, got {answer:?}"
    );
    let functions: Vec<Value> = answer.body["functions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a functions list, got {answer:?}"))
        .clone();
    let ids = functions
        .iter()
        .map(|function| {
            function["functionId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected every entry to carry a functionId, got {answer:?}")
                })
                .to_string()
        })
        .collect();
    ListedPage {
        ids,
        next_token: answer.body["nextToken"].as_str().map(str::to_string),
        functions,
    }
}

/// Page through the whole API at `max_results` and return every entry, in the order the
/// listing reported them, asserting the page-size bound and the cursor contract on each
/// page.
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
        let case =
            format!("walk {api_id} at maxResults {max_results:?}, page {page_index} from {token:?}");
        let answer = list_page(router, api_id, max_results, token.as_deref()).await;
        let page = read_page(&case, &answer);
        assert!(
            page.ids.len() <= bound,
            "case {case}: expected at most {bound} entries, got {} ({:?})",
            page.ids.len(),
            page.ids
        );
        let remaining = expected_total - collected.len();
        assert_eq!(
            page.ids.len(),
            remaining.min(bound),
            "case {case}: expected the page to be as full as the remainder allows, got {:?}",
            page.ids
        );
        collected.extend(page.functions);
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

/// The ids of the entries a walk reported, in the order it reported them.
fn listed_ids(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|function| {
            function["functionId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every walk of an API reports the same functions in the same order, however the
/// caller pages, and every function the API holds exactly once.
#[tokio::test]
async fn every_page_size_walks_the_whole_api_in_one_order() {
    let router = router();

    for count in COUNTS.iter().copied() {
        let api_id = create_api(&router, &format!("Api_list_functions_{count}")).await;
        let mut seeded = seed(&router, &api_id, count).await;
        seeded.sort();

        let mut reference: Option<Vec<String>> = None;
        for max_results in PAGE_SIZES.iter().copied() {
            let listed = walk(&router, &api_id, max_results, count).await;
            let ids = listed_ids(&listed);
            // Every seeded function, exactly once: a set comparison that is blind to
            // the order, which the next assertion pins separately.
            let mut sorted = ids.clone();
            sorted.sort();
            assert_eq!(
                sorted, seeded,
                "case {count} functions at maxResults {max_results:?}: expected every seeded id \
                 exactly once, got {ids:?}"
            );
            match &reference {
                None => reference = Some(ids),
                Some(reference) => assert_eq!(
                    &ids, reference,
                    "case {count} functions at maxResults {max_results:?}: expected the same \
                     order as the first walk {reference:?}, got {ids:?}"
                ),
            }
        }
    }
}

/// Each listed entry is the record `GetFunction` answers for that id, member for member
/// — so a listing projects the stored function rather than a summary of it. Asserted on
/// an API holding the whole round-trip table, whose rows repeat names, so a listing of
/// functions sharing a name still reports each one separately.
#[tokio::test]
async fn a_listed_entry_is_the_record_get_answers() {
    let router = router();
    let api_id = create_api(&router, "Api_list_functions_projection").await;
    // Two passes over the table, so every definition form appears and every name is
    // carried by two different functions.
    let count = rows().len() * 2;
    let seeded = seed(&router, &api_id, count).await;

    for max_results in PAGE_SIZES.iter().copied() {
        let listed = walk(&router, &api_id, max_results, count).await;
        for entry in &listed {
            let function_id = entry["functionId"].as_str().unwrap_or_default();
            let read = get_function(&router, &api_id, function_id).await;
            assert_eq!(
                read.status, 200,
                "case listed {function_id:?} at maxResults {max_results:?}: expected GetFunction \
                 200, got {read:?}"
            );
            assert_eq!(
                &read.body["functionConfiguration"], entry,
                "case listed {function_id:?} at maxResults {max_results:?}: expected the listing \
                 to answer the stored record {:?}, got {entry:?}",
                read.body["functionConfiguration"]
            );
        }
    }

    // Names repeat across the table, so the listing's entries are told apart by their
    // ids: every seeded id is reported, and no id twice.
    let ids = listed_ids(&walk(&router, &api_id, Some(1), count).await);
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        count,
        "case the listing of functions sharing a name: expected {count} distinct ids, got {ids:?}"
    );
    for seeded_id in &seeded {
        assert!(
            ids.contains(seeded_id),
            "case the listing of functions sharing a name: expected {seeded_id:?} among {ids:?}"
        );
    }
}

/// A listing reports the functions of the API it names and no others, even where two
/// APIs hold functions created from the same requests.
#[tokio::test]
async fn a_listing_reports_only_the_api_it_names() {
    let router = router();
    let first = create_api(&router, "Api_list_functions_scope_first").await;
    let second = create_api(&router, "Api_list_functions_scope_second").await;

    // The second API holds FEWER functions, created from the same leading rows of the
    // table, so a listing that crossed APIs would be caught either way.
    let first_ids = seed(&router, &first, 5).await;
    let second_ids = seed(&router, &second, 2).await;

    for (api_id, expected) in [(&first, &first_ids), (&second, &second_ids)] {
        let listed = walk(&router, api_id, Some(1), expected.len()).await;
        let mut ids = listed_ids(&listed);
        ids.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            ids, expected_sorted,
            "case the listing of {api_id}: expected exactly its own functions \
             {expected_sorted:?}, got {ids:?}"
        );
        // Every entry is named under the API that holds it, so the two APIs' functions
        // are different resources.
        for function in &listed {
            let function_id = function["functionId"].as_str().unwrap_or_default();
            let arn = function["functionArn"].as_str().unwrap_or_default();
            let resource = state::function_resource(api_id, function_id);
            assert!(
                arn.ends_with(&resource),
                "case the listing of {api_id}: expected {function_id:?} named under {resource}, \
                 got {arn:?}"
            );
        }
    }
}

/// Listing is a pure read: paging through an API changes nothing a later caller sees,
/// so the walk repeats identically and every entry still reads back.
#[tokio::test]
async fn paging_leaves_the_stored_set_unchanged() {
    let router = router();
    let api_id = create_api(&router, "Api_list_functions_pure_read").await;
    let count = state::DEFAULT_PAGE_SIZE + 3;
    let seeded = seed(&router, &api_id, count).await;

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
    for function_id in &seeded {
        let read = get_function(&router, &api_id, function_id).await;
        assert_eq!(
            read.status, 200,
            "case {function_id:?} after paging: expected GetFunction 200, got {read:?}"
        );
    }
}

/// A listing that addresses nothing is refused: an `apiId` too long to name an API is a
/// 400, one of the right shape this account does not hold a 404, and a token that names
/// no page of the list AppSync's fixed invalid-token text.
#[tokio::test]
async fn a_listing_that_addresses_nothing_is_refused() {
    let router = router();
    let api_id = create_api(&router, "Api_list_functions_refusals").await;
    let seeded = seed(&router, &api_id, 3).await;

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

    // Tokens that name no page of this list: an id of another shape entirely, a value
    // that is no id at all, and a well-formed id this API does not hold.
    let foreign = create_api(&router, "Api_list_functions_token_source").await;
    let foreign_ids = seed(&router, &foreign, 1).await;
    let foreign_id = foreign_ids[0].clone();
    for token in ["bogus", "0", "da2-nosuchkey", foreign_id.as_str()] {
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
        .expect("the first page of three functions carries a nextToken");
    assert!(
        seeded.contains(&cursor),
        "case the cursor of the first page: expected one of the seeded ids {seeded:?}, got \
         {cursor:?}"
    );
    let resumed = list_page(&router, &api_id, Some(1), Some(&cursor)).await;
    let resumed = read_page("resuming from the listing's own token", &resumed);
    assert_eq!(
        resumed.ids.len(),
        1,
        "case resuming from the listing's own token: expected one entry, got {:?}",
        resumed.ids
    );
    assert_ne!(
        resumed.ids, page.ids,
        "case resuming from the listing's own token: expected the next entry, got {:?}",
        resumed.ids
    );
}
