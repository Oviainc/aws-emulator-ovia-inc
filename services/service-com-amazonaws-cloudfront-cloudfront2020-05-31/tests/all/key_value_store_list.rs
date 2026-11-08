// SPDX-License-Identifier: CC0-1.0

//! `ListKeyValueStores`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront's key value store listing
//! states:
//!
//! * the answer reports the key value stores THIS ACCOUNT created, each item carrying the
//!   same record a `DescribeKeyValueStore` of that store answers with, in one stable total
//!   order over the names the stores are held under — so an account that created nothing
//!   lists nothing;
//! * `MaxItems` cuts the page and is echoed as asked for, `Quantity` states the page's
//!   length, `NextMarker` is present exactly when stores remain and names the name the page
//!   ended at, and a request resumes AFTER the name its `Marker` names — so walking the
//!   pages at ANY admissible page size enumerates every store exactly once, in one order;
//! * `Status` narrows the answer to the stores whose own stored status is the one asked
//!   for, so the statuses partition the unfiltered answer and a status no store carries
//!   selects nothing;
//! * a page size that names no page, and a `Marker` that is no key value store name, are
//!   refused with `InvalidArgument` (400), and a refusal changes nothing;
//! * a listing is a READ: it answers the same records again, mints nothing and moves no
//!   store's status or version token.
//!
//! The table rows differ along everything these rules generalise over: the number of stores
//! the account holds (none, two, several), the page size (one, several, the collection size,
//! past it and past the server's own ceiling), where the resume point sits and whether it
//! names a store at all, the status asked for (each of the two a store can carry, and one it
//! cannot), and the form an unusable page size or marker takes.
//!
//! The harness (the router, the request helpers and the element scanner) is the distribution
//! lifecycle suite's, and the key value store request builders are the key value store
//! lifecycle suite's, reused rather than copied.

use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_KEY_VALUE_STORE_MARKER_MESSAGE, KVS_PROVISIONING_DELAY, KVS_STATUS_PROVISIONING,
    KVS_STATUS_READY, MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE, MAX_KEY_VALUE_STORE_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, API_VERSION};
use crate::key_value_store_lifecycle::{
    create_key_value_store, create_table, describe_key_value_store, Create,
};

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/key-value-store`, with `query` appended verbatim
/// (`?MaxItems=2&Status=READY`), from the operation's `http` binding.
async fn list_stores(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/key-value-store{query}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// Reading an answer
// ---------------------------------------------------------------------------

/// One `KeyValueStore` a page reports: every member the shape carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    name: String,
    id: String,
    arn: String,
    comment: String,
    status: String,
    last_modified_time: String,
}

/// Every store a page reports, in page order.
fn page_items(answer: &Answer) -> Vec<Item> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("KeyValueStore")
                .into_iter()
                .map(|store| Item {
                    name: store.text_at("Name").unwrap_or_default().to_string(),
                    id: store.text_at("Id").unwrap_or_default().to_string(),
                    arn: store.text_at("ARN").unwrap_or_default().to_string(),
                    comment: store.text_at("Comment").unwrap_or_default().to_string(),
                    status: store.text_at("Status").unwrap_or_default().to_string(),
                    last_modified_time: store
                        .text_at("LastModifiedTime")
                        .unwrap_or_default()
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The names a page reports, in the order the page carries them.
fn page_names(answer: &Answer) -> Vec<String> {
    page_items(answer)
        .into_iter()
        .map(|item| item.name)
        .collect()
}

/// The store `name`'s record as `DescribeKeyValueStore` reports it — what a listing's own
/// item is held to.
async fn described_item(router: &MultiServiceRouter, name: &str) -> Item {
    let answer = describe_key_value_store(router, name).await;
    assert_eq!(
        answer.status, 200,
        "case {name}: expected HTTP 200 from DescribeKeyValueStore, got {} with {}",
        answer.status, answer.raw
    );
    let store = answer.root();
    Item {
        name: store.text_at("Name").unwrap_or_default().to_string(),
        id: store.text_at("Id").unwrap_or_default().to_string(),
        arn: store.text_at("ARN").unwrap_or_default().to_string(),
        comment: store.text_at("Comment").unwrap_or_default().to_string(),
        status: store.text_at("Status").unwrap_or_default().to_string(),
        last_modified_time: store
            .text_at("LastModifiedTime")
            .unwrap_or_default()
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// The stores under test
// ---------------------------------------------------------------------------

/// Create every store the key value store table describes and report the version token
/// each create handed back, keyed by name.
async fn create_stores(router: &MultiServiceRouter, table: &[Create]) -> Vec<(String, String)> {
    let mut created: Vec<(String, String)> = Vec::new();
    for create in table {
        let answer = create_key_value_store(router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateKeyValueStore, got {} with {}",
            create.case, answer.status, answer.raw
        );
        created.push((create.name.clone(), answer.header("etag").to_string()));
    }
    created
}

/// The names the table's stores are held under, in the order a listing must report them:
/// the one total order over names the records are held in.
fn expected_names(table: &[Create]) -> Vec<String> {
    let mut names: Vec<String> = table.iter().map(|create| create.name.clone()).collect();
    names.sort();
    names
}

/// Two stores, so the status rule is asserted over a collection that is neither empty nor
/// the whole table.
fn status_table() -> Vec<Create> {
    vec![
        Create {
            case: "a store whose name sorts first",
            name: "alpha_store".to_string(),
            comment: Some("first by name".to_string()),
            tags: Vec::new(),
        },
        Create {
            case: "a store whose name sorts second",
            name: "beta-store".to_string(),
            comment: None,
            tags: Vec::new(),
        },
    ]
}

/// Walk the listing at `size` items a page, asserting every page's bookkeeping, and report
/// every name the walk reported in walk order.
async fn walk_pages(
    router: &MultiServiceRouter,
    filter: &str,
    size: usize,
    total: usize,
) -> Vec<String> {
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let query = match &marker {
            Some(value) => format!("?MaxItems={size}&Marker={value}{filter}"),
            None => format!("?MaxItems={size}{filter}"),
        };
        let page = list_stores(router, &query).await;
        assert_eq!(
            page.status, 200,
            "case {query}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page.root().text_at("MaxItems"),
            Some(size.to_string().as_str()),
            "case {query}: expected the requested MaxItems echoed, got {}",
            page.raw
        );
        let names = page_names(&page);
        assert!(
            names.len() <= size.min(ceiling),
            "case {query}: a page may serve at most {} items, got {} in {}",
            size.min(ceiling),
            names.len(),
            page.raw
        );
        assert_eq!(
            page.root().text_at("Quantity"),
            Some(names.len().to_string().as_str()),
            "case {query}: expected Quantity to state the page's length {}, got {}",
            names.len(),
            page.raw
        );
        if names.is_empty() {
            assert!(
                page.root().at("Items").is_none(),
                "case {query}: a page that lists nothing carries no Items element, got {}",
                page.raw
            );
        }
        walked.extend(names.iter().cloned());
        let truncated = walked.len() < total;
        let next = page.root().text_at("NextMarker").map(str::to_string);
        assert_eq!(
            next.is_some(),
            truncated,
            "case {query}: expected NextMarker present {truncated} after {} of {total} \
             stores, got {}",
            walked.len(),
            page.raw
        );
        match next {
            Some(value) => {
                assert_eq!(
                    Some(value.as_str()),
                    names.last().map(String::as_str),
                    "case {query}: expected NextMarker to name the name the page ended at, \
                     got {}",
                    page.raw
                );
                marker = Some(value);
            }
            None => break,
        }
    }
    walked
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// Every key value store a caller can see was created by `CreateKeyValueStore`, so an
/// account that created none lists none: a page of nothing, with no `Items` element and no
/// resume point, paged at this service's default size.
#[tokio::test]
async fn an_account_that_created_nothing_lists_nothing() {
    let router = emulator();

    // Rows: the bare listing, and each parameter a client may send on its own.
    for query in ["", "?MaxItems=1", "?Status=READY", "?Marker=any_name"] {
        let listed = list_stores(&router, query).await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: expected HTTP 200 from ListKeyValueStores, got {} with {}",
            listed.status, listed.raw
        );
        assert_eq!(
            page_items(&listed),
            Vec::<Item>::new(),
            "case {query:?}: expected no stores at all, got {}",
            listed.raw
        );
        assert_eq!(
            listed.root().text_at("Quantity"),
            Some("0"),
            "case {query:?}: expected Quantity 0, got {}",
            listed.raw
        );
        assert!(
            listed.root().at("Items").is_none(),
            "case {query:?}: a page that lists nothing carries no Items element, got {}",
            listed.raw
        );
        assert!(
            listed.root().text_at("NextMarker").is_none(),
            "case {query:?}: expected no NextMarker when nothing remains, got {}",
            listed.raw
        );
    }

    let listed = list_stores(&router, "").await;
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case the default page size: expected MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
}

/// The listing reports each stored record: every store the account created, exactly once,
/// in the one total order over names, each item carrying the record a describe of that
/// store answers with.
#[tokio::test]
async fn the_listing_reports_every_stored_record_in_name_order() {
    let router = emulator();
    let table = create_table();
    create_stores(&router, &table).await;

    // Past the provisioning span, so the status every store reports is the terminal one
    // whichever call reads it — the listing and the describe it is compared against.
    tokio::time::sleep(KVS_PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let listed = list_stores(&router, "").await;
    assert_eq!(
        listed.status, 200,
        "case the whole table: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    let items = page_items(&listed);
    assert_eq!(
        items.iter().map(|item| item.name.clone()).collect::<Vec<_>>(),
        expected_names(&table),
        "case the whole table: expected every store once in name order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(table.len().to_string().as_str()),
        "case the whole table: expected Quantity {}, got {}",
        table.len(),
        listed.raw
    );

    for item in &items {
        let case = item.name.as_str();
        let described = described_item(&router, &item.name).await;
        assert_eq!(
            item, &described,
            "case {case}: expected the listing to report the record the describe answers \
             with, got {item:?}"
        );
        assert!(
            is_minted_uuid(&item.id),
            "case {case}: expected a UUID Id, got {:?}",
            item.id
        );
        assert_eq!(
            item.status, KVS_STATUS_READY,
            "case {case}: expected the settled status, got {:?}",
            item.status
        );
        let create = table
            .iter()
            .find(|create| create.name == item.name)
            .unwrap_or_else(|| panic!("case {case}: the listing reported an unknown store"));
        assert_eq!(
            item.comment,
            create.comment.clone().unwrap_or_default(),
            "case {case}: expected the submitted Comment, got {:?}",
            item.comment
        );
    }
}

/// `MaxItems` cuts the page, `NextMarker` names the name the page ended at and a request
/// resumes after it, so walking the pages at ANY admissible size enumerates every store
/// exactly once in the one order the unpaged listing reports.
#[tokio::test]
async fn walking_the_pages_enumerates_every_store_exactly_once() {
    let router = emulator();
    let table = create_table();
    create_stores(&router, &table).await;
    let order = expected_names(&table);
    let total = order.len();

    // Rows: one store a page, several, exactly the collection size, one past it, and past
    // the server's own ceiling (where the page is cut at the ceiling, not at the size
    // asked for).
    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    for size in [1, 2, total, total + 1, ceiling + 1] {
        let walked = walk_pages(&router, "", size, total).await;
        assert_eq!(
            walked, order,
            "case MaxItems={size}: expected every store exactly once in one order, got \
             {walked:?}"
        );
    }
}

/// "The response includes key value stores in the list that occur after the marker": a
/// request resumes strictly after the name its `Marker` names, whether or not that name
/// still names a store.
#[tokio::test]
async fn a_marker_resumes_after_the_name_it_names() {
    let router = emulator();
    let table = create_table();
    create_stores(&router, &table).await;
    let order = expected_names(&table);

    // Rows: every name a store is held under (so the resume point sits at the start, in the
    // middle and at the end of the order), plus well-formed names no store holds — one
    // sorting before every stored name, one inside the order and one after all of them.
    let mut markers: Vec<String> = order.clone();
    markers.extend([
        "A".to_string(),
        "b_absent".to_string(),
        "z".repeat(MAX_KEY_VALUE_STORE_NAME),
    ]);

    for marker in markers {
        let expected: Vec<String> = order
            .iter()
            .filter(|name| name.as_str() > marker.as_str())
            .cloned()
            .collect();
        let page = list_stores(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            page.status, 200,
            "case Marker={marker}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page_names(&page),
            expected,
            "case Marker={marker}: expected the names after the marker, got {}",
            page.raw
        );
        assert_eq!(
            page.root().text_at("Quantity"),
            Some(expected.len().to_string().as_str()),
            "case Marker={marker}: expected Quantity {}, got {}",
            expected.len(),
            page.raw
        );
    }
}

/// `Status` narrows the answer to the stores carrying that status, read from the stored
/// record: the two statuses a store can carry partition the unfiltered answer, the clock
/// moves which side a store falls on, and a status no store carries selects nothing.
#[tokio::test]
async fn the_status_filter_serves_the_stores_in_that_status() {
    let router = emulator();
    let table = status_table();
    create_stores(&router, &table).await;
    let order = expected_names(&table);

    // Straight after the creates every store is still provisioning, so the transient
    // status serves all of them and the terminal one none.
    assert_status_split(&router, &order, &[], "straight after the creates").await;

    // The clock — not a call — moves every store to the terminal status, so the filter
    // moves with it.
    tokio::time::sleep(KVS_PROVISIONING_DELAY + Duration::from_millis(250)).await;
    assert_status_split(&router, &[], &order, "past the provisioning span").await;

    // The filter composes with paging: the page is cut after the filter, so a walk under a
    // status enumerates exactly the stores carrying it, one page at a time.
    let walked = walk_pages(
        &router,
        &format!("&Status={KVS_STATUS_READY}"),
        1,
        order.len(),
    )
    .await;
    assert_eq!(
        walked, order,
        "case a filtered walk: expected every store in that status exactly once, got \
         {walked:?}"
    );

    // A status a key value store cannot carry — and a status in the wrong case — names no
    // store, so the page lists nothing rather than being refused.
    for status in ["DELETING", "ready", ""] {
        let page = list_stores(&router, &format!("?Status={status}")).await;
        assert_eq!(
            page.status, 200,
            "case Status={status:?}: expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page_names(&page),
            Vec::<String>::new(),
            "case Status={status:?}: expected no store to carry it, got {}",
            page.raw
        );
    }
}

/// Assert the two statuses a key value store can carry partition the unfiltered answer:
/// `provisioning` names the stores the transient status must serve and `ready` the ones the
/// terminal status must.
async fn assert_status_split(
    router: &MultiServiceRouter,
    provisioning: &[String],
    ready: &[String],
    context: &str,
) {
    for (status, expected) in [
        (KVS_STATUS_PROVISIONING, provisioning),
        (KVS_STATUS_READY, ready),
    ] {
        let page = list_stores(router, &format!("?Status={status}")).await;
        assert_eq!(
            page.status, 200,
            "case {status} ({context}): expected HTTP 200, got {} with {}",
            page.status, page.raw
        );
        assert_eq!(
            page_names(&page),
            expected.to_vec(),
            "case {status} ({context}): expected the stores in that status, got {}",
            page.raw
        );
        for item in page_items(&page) {
            assert_eq!(
                item.status, status,
                "case {status} ({context}): expected every item to carry the status asked \
                 for, got {item:?}"
            );
        }
    }

    // The filtered pages together are the unfiltered page: no store falls outside the two
    // statuses, and none is served under both.
    let mut union: Vec<String> = provisioning.to_vec();
    union.extend(ready.iter().cloned());
    union.sort();
    let unfiltered = list_stores(router, "").await;
    assert_eq!(
        page_names(&unfiltered),
        union,
        "case the unfiltered page ({context}): expected the two statuses to partition it, \
         got {}",
        unfiltered.raw
    );
}

/// A page size that names no page, and a `Marker` that is no key value store name, are
/// refused with `InvalidArgument` (400) — and a refusal leaves the listing answering
/// exactly what it answered before.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let table = create_table();
    create_stores(&router, &table).await;
    let order = expected_names(&table);

    // Rows: negative page sizes of two magnitudes — every value no page can be taken from.
    for size in ["-1", "-100"] {
        let answer = list_stores(&router, &format!("?MaxItems={size}")).await;
        assert_eq!(
            answer.status, 400,
            "case MaxItems={size}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case MaxItems={size}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(MAX_ITEMS_INVALID_MESSAGE),
            "case MaxItems={size}: expected the parameter's constraint, got {}",
            answer.raw
        );
    }

    // Rows: a marker one character past the name length bound, one drawing from a
    // character the name pattern excludes, one carrying a character class the pattern
    // excludes entirely, and the empty marker — none of them a resume point this listing
    // ever handed out.
    let markers = [
        "x".repeat(MAX_KEY_VALUE_STORE_NAME + 1),
        "orders.by.region".to_string(),
        "store:1".to_string(),
        String::new(),
    ];
    for marker in markers {
        let answer = list_stores(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 400,
            "case Marker={marker:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case Marker={marker:?}: expected InvalidArgument, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message"),
            Some(INVALID_KEY_VALUE_STORE_MARKER_MESSAGE),
            "case Marker={marker:?}: expected the parameter's constraint, got {}",
            answer.raw
        );
    }

    let listed = list_stores(&router, "").await;
    assert_eq!(
        page_names(&listed),
        order,
        "case after the refusals: a refused listing changes nothing, got {}",
        listed.raw
    );
}

/// A listing is a READ: it answers the same records again, mints nothing of its own and
/// leaves every store's version token and identity exactly as the create left them.
#[tokio::test]
async fn the_listing_is_a_read() {
    let router = emulator();
    let table = create_table();
    let created = create_stores(&router, &table).await;

    let first = list_stores(&router, "").await;
    // Reads over every parameter combination this listing serves, so none of them can be
    // the one that writes.
    for query in [
        "?MaxItems=1",
        "?Status=PROVISIONING",
        "?Status=READY",
        "?MaxItems=2&Marker=a",
    ] {
        let answer = list_stores(&router, query).await;
        assert_eq!(
            answer.status, 200,
            "case {query}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
    }
    let second = list_stores(&router, "").await;
    assert_eq!(
        page_items(&second)
            .into_iter()
            .map(|item| (item.name, item.id, item.arn, item.comment))
            .collect::<Vec<_>>(),
        page_items(&first)
            .into_iter()
            .map(|item| (item.name, item.id, item.arn, item.comment))
            .collect::<Vec<_>>(),
        "case repeated listings: expected the same records again, got {}",
        second.raw
    );

    for (name, etag) in created {
        let described = describe_key_value_store(&router, &name).await;
        assert_eq!(
            described.status, 200,
            "case {name}: expected HTTP 200 from the describe, got {} with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {name}: a listing is no mutation, so the version token stands"
        );
    }
}
