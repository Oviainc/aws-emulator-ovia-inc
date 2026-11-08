// SPDX-License-Identifier: CC0-1.0

//! `ListFunctions`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * the listing answers from the STORED functions — "Gets a list of all CloudFront
//!   functions in your Amazon Web Services account" — each listed ONCE and in name order,
//!   every item projected from the stage record it reports;
//! * "You can optionally apply a filter to return only the functions that are in the
//!   specified stage, either DEVELOPMENT or LIVE": a request that names no stage lists every
//!   function under the stage a function is created in, and `Stage=LIVE` lists exactly the
//!   functions a publish has copied into that stage — so a function appears there when, and
//!   only when, it was published;
//! * the page contract every CloudFront listing states: `MaxItems` is the page size and is
//!   echoed, `Marker` resumes after the name the previous page ended at, `NextMarker` arrives
//!   exactly when matching functions remain, `Quantity` states the length of `Items`, and a
//!   page that lists nothing carries no `Items` element at all;
//! * a `MaxItems` no page size can be taken from is refused with the modelled
//!   `InvalidArgument`.
//!
//! The rows span what those rules generalise over: the names (every character class
//! `FunctionName` admits, so the order is not the insertion order), which functions are
//! published, every page size from one item to past the whole collection, and a store in
//! which no function is published at all.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::function_lifecycle::{create_key_value_store, create_table, Create};
use crate::function_publish::{create_one, publish_function};

/// The two values the modelled `FunctionStage` enum admits (`smithy_query(query="shape",
/// shape="FunctionStage")`), spelled out here so a wrong constant in the crate cannot make
/// these assertions pass.
const STAGE_DEVELOPMENT: &str = "DEVELOPMENT";
const STAGE_LIVE: &str = "LIVE";

/// The status a stage that has never been published reports, and the one a published stage
/// nothing attaches reports.
const STATUS_UNPUBLISHED: &str = "UNPUBLISHED";
const STATUS_UNASSOCIATED: &str = "UNASSOCIATED";

/// Which rows of the create table are published, by index — so the `LIVE` listing has both
/// something to list and something to leave out, and the published names are not contiguous
/// in name order.
const PUBLISHED_ROWS: &[usize] = &[0, 2, 4];

// ---------------------------------------------------------------------------
// Calling the listing
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/function` plus the query the caller filters and pages with — the route
/// the model binds this operation to.
async fn list_functions(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/function{query}"),
        None,
        None,
    )
    .await
}

/// One `FunctionList` page, as the document reports it.
struct Page {
    /// `FunctionSummary$Name` of each item, in document order.
    names: Vec<String>,
    /// `FunctionMetadata$Stage` of each item, in document order.
    stages: Vec<String>,
    /// `FunctionSummary$Status` of each item, in document order.
    statuses: Vec<String>,
    /// `FunctionList$Quantity`, as submitted on the wire.
    quantity: String,
    /// `FunctionList$MaxItems`, as submitted on the wire.
    max_items: String,
    /// `FunctionList$NextMarker`, or `None` when the element is absent.
    next_marker: Option<String>,
    /// Whether the document carries an `Items` element at all.
    has_items: bool,
}

/// Read the page `answer` carries, asserting it is a listing at all.
fn page(answer: &Answer, case: &str) -> Page {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 from the listing, got {} with {}",
        answer.status, answer.raw
    );
    let root = answer.root();
    let items = root.at("Items");
    let summaries = items
        .map(|items| items.children_named("FunctionSummary"))
        .unwrap_or_default();
    Page {
        names: summaries
            .iter()
            .map(|summary| summary.text_at("Name").unwrap_or_default().to_string())
            .collect(),
        stages: summaries
            .iter()
            .map(|summary| {
                summary
                    .text_at("FunctionMetadata/Stage")
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
        statuses: summaries
            .iter()
            .map(|summary| summary.text_at("Status").unwrap_or_default().to_string())
            .collect(),
        quantity: root.text_at("Quantity").unwrap_or_default().to_string(),
        max_items: root.text_at("MaxItems").unwrap_or_default().to_string(),
        next_marker: root.text_at("NextMarker").map(str::to_string),
        has_items: items.is_some(),
    }
}

/// Create every row of the function table, publishing the rows [`PUBLISHED_ROWS`] names, and
/// answer the created functions.
async fn seed(router: &MultiServiceRouter, store_name: &str) -> Vec<Create> {
    let kvs_arn = create_key_value_store(router, store_name).await;
    let table = create_table(&kvs_arn);
    for (index, create) in table.iter().enumerate() {
        let etag = create_one(router, create).await;
        if PUBLISHED_ROWS.contains(&index) {
            let published = publish_function(router, &create.name, &etag).await;
            assert_eq!(
                published.status, 200,
                "case {}: expected HTTP 200 from the publish, got {} with {}",
                create.case, published.status, published.raw
            );
        }
    }
    table
}

/// The names of `table`, in the order a listing serves them: a function is held under its
/// name, so the order is the names' own.
fn ordered_names(table: &[Create]) -> Vec<String> {
    let mut names: Vec<String> = table.iter().map(|create| create.name.clone()).collect();
    names.sort();
    names
}

/// The names of the rows that were published, in listing order.
fn ordered_published_names(table: &[Create]) -> Vec<String> {
    let mut names: Vec<String> = PUBLISHED_ROWS
        .iter()
        .map(|index| table[*index].name.clone())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The listing answers from the stored functions, in name order, and the stage filter decides
/// WHICH record each item is projected from: every function under the stage it is created in,
/// and exactly the published ones under `LIVE`.
#[tokio::test]
async fn the_listing_reports_the_stored_functions_of_the_stage_asked_for() {
    let router = emulator();
    let table = seed(&router, "function_list_store").await;
    let all = ordered_names(&table);
    let published = ordered_published_names(&table);

    for (case, query, expected, stage, status) in [
        (
            "no stage at all",
            String::new(),
            all.clone(),
            STAGE_DEVELOPMENT,
            STATUS_UNPUBLISHED,
        ),
        (
            "the stage a function is created in",
            format!("?Stage={STAGE_DEVELOPMENT}"),
            all.clone(),
            STAGE_DEVELOPMENT,
            STATUS_UNPUBLISHED,
        ),
        (
            "the published stage",
            format!("?Stage={STAGE_LIVE}"),
            published.clone(),
            STAGE_LIVE,
            STATUS_UNASSOCIATED,
        ),
    ] {
        let answer = list_functions(&router, &query).await;
        let page = page(&answer, case);
        assert_eq!(
            page.names, expected,
            "case {case}: expected the stored functions in name order, got {:?}",
            page.names
        );
        assert_eq!(
            page.stages,
            vec![stage.to_string(); expected.len()],
            "case {case}: expected every item in the {stage} stage, got {:?}",
            page.stages
        );
        assert_eq!(
            page.statuses,
            vec![status.to_string(); expected.len()],
            "case {case}: expected every item to report Status {status}, got {:?}",
            page.statuses
        );
        assert_eq!(
            page.quantity,
            expected.len().to_string(),
            "case {case}: expected Quantity to state the length of Items, got {:?}",
            page.quantity
        );
        assert_eq!(
            page.max_items,
            MAX_ITEMS.to_string(),
            "case {case}: expected the default page size to be echoed, got {:?}",
            page.max_items
        );
        assert_eq!(
            page.next_marker, None,
            "case {case}: a page that ends the collection carries no NextMarker, got {:?}",
            page.next_marker
        );
    }
}

/// A function appears under `LIVE` when, and only when, a publish has copied it there: the
/// listing of a store in which nothing is published lists nothing at all, and each publish
/// adds exactly its own function to that listing.
#[tokio::test]
async fn the_live_listing_reports_exactly_what_a_publish_promoted() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_live_list_store").await;
    let table = create_table(&kvs_arn);
    let mut etags = Vec::new();
    for create in &table {
        etags.push(create_one(&router, create).await);
    }

    // Before any publish: every function is in the stage it was created in, and the published
    // stage lists nothing — not an empty `Items` element, but none at all.
    let empty = list_functions(&router, &format!("?Stage={STAGE_LIVE}")).await;
    let empty = page(&empty, "before any publish");
    assert!(
        empty.names.is_empty() && !empty.has_items,
        "case before any publish: expected no Items element at all, got {:?}",
        empty.names
    );
    assert_eq!(
        empty.quantity, "0",
        "case before any publish: expected Quantity 0, got {:?}",
        empty.quantity
    );

    let mut expected: Vec<String> = Vec::new();
    for (index, create) in table.iter().enumerate() {
        let case = create.case;
        let published = publish_function(&router, &create.name, &etags[index]).await;
        assert_eq!(
            published.status, 200,
            "case {case}: expected HTTP 200 from the publish, got {} with {}",
            published.status, published.raw
        );
        expected.push(create.name.clone());
        expected.sort();

        let answer = list_functions(&router, &format!("?Stage={STAGE_LIVE}")).await;
        let live = page(&answer, case);
        assert_eq!(
            live.names, expected,
            "case {case}: expected the published functions in name order, got {:?}",
            live.names
        );

        // The stage a function is created in is unaffected by what was published.
        let development = list_functions(&router, &format!("?Stage={STAGE_DEVELOPMENT}")).await;
        let development = page(&development, case);
        assert_eq!(
            development.names,
            ordered_names(&table),
            "case {case}: expected every stored function under {STAGE_DEVELOPMENT}, got {:?}",
            development.names
        );
    }
}

/// The page contract: `MaxItems` is the page size and is echoed, `Marker` resumes after the
/// name the previous page ended at, `NextMarker` arrives exactly when matching functions
/// remain, and walking the pages enumerates every function exactly once, in name order.
#[tokio::test]
async fn the_pages_enumerate_every_function_exactly_once() {
    let router = emulator();
    let table = seed(&router, "function_page_store").await;
    let all = ordered_names(&table);

    for (stage_case, query, expected) in [
        ("no stage at all", String::new(), all.clone()),
        (
            "the published stage",
            format!("&Stage={STAGE_LIVE}"),
            ordered_published_names(&table),
        ),
    ] {
        // Every page size from one item up to one past the whole collection.
        for size in 1..=expected.len() + 1 {
            let case = format!("{stage_case}, MaxItems {size}");
            let mut walked: Vec<String> = Vec::new();
            let mut seen = BTreeSet::new();
            let mut marker: Option<String> = None;
            let mut pages = 0;

            loop {
                let marker_query = marker
                    .as_deref()
                    .map(|marker| format!("&Marker={marker}"))
                    .unwrap_or_default();
                let answer =
                    list_functions(&router, &format!("?MaxItems={size}{query}{marker_query}"))
                        .await;
                let page = page(&answer, &case);
                pages += 1;
                assert!(
                    pages <= expected.len() + 2,
                    "case {case}: the paging walk does not terminate, got {pages} pages"
                );
                assert!(
                    page.names.len() <= size,
                    "case {case}: expected at most {size} items on a page, got {:?}",
                    page.names
                );
                assert_eq!(
                    page.max_items,
                    size.to_string(),
                    "case {case}: expected the requested page size to be echoed, got {:?}",
                    page.max_items
                );
                assert_eq!(
                    page.quantity,
                    page.names.len().to_string(),
                    "case {case}: expected Quantity to state the length of Items, got {:?}",
                    page.quantity
                );
                for name in &page.names {
                    assert!(
                        seen.insert(name.clone()),
                        "case {case}: the function {name} was listed twice"
                    );
                }
                walked.extend(page.names.iter().cloned());

                let remaining = expected.len() > walked.len();
                match (&page.next_marker, remaining) {
                    (Some(next), true) => {
                        assert_eq!(
                            Some(next.as_str()),
                            page.names.last().map(String::as_str),
                            "case {case}: expected NextMarker to name the end of the page, got \
                             {next:?}"
                        );
                        marker = Some(next.clone());
                    }
                    (None, false) => break,
                    (next, remaining) => panic!(
                        "case {case}: expected NextMarker present exactly while functions remain \
                         ({remaining} remaining), got {next:?}"
                    ),
                }
            }

            assert_eq!(
                walked, expected,
                "case {case}: expected the pages to enumerate every function in name order, got \
                 {walked:?}"
            );
        }
    }
}

/// A `MaxItems` no page size can be taken from is refused with the modelled `InvalidArgument`
/// and lists nothing; a size past the collection is a page that simply ends it.
#[tokio::test]
async fn a_max_items_no_page_size_can_be_taken_from_is_refused() {
    let router = emulator();
    let table = seed(&router, "function_max_items_store").await;

    for (case, value) in [("a negative page size", "-1"), ("far negative", "-2147483648")] {
        let answer = list_functions(&router, &format!("?MaxItems={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for MaxItems={value}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );
        let message = answer.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message, MAX_ITEMS_INVALID_MESSAGE,
            "case {case}: expected the constraint's own sentence, got {message:?}"
        );
    }

    // A page size past the collection ends it in one page.
    let case = "a page size past the whole collection";
    let answer = list_functions(&router, &format!("?MaxItems={}", MAX_ITEMS + 1)).await;
    let page = page(&answer, case);
    assert_eq!(
        page.names,
        ordered_names(&table),
        "case {case}: expected every stored function, got {:?}",
        page.names
    );
    assert_eq!(
        page.next_marker, None,
        "case {case}: a page that ends the collection carries no NextMarker, got {:?}",
        page.next_marker
    );
}
