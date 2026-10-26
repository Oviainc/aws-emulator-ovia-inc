// SPDX-License-Identifier: CC0-1.0

//! The ONE page window every `@paginated` listing of this service reports through
//! ([`keyspaces::page_window`]), driven end to end over the service's own `awsJson1_0`
//! wire protocol through the harness in [`crate::keyspace_table_lifecycle`].
//!
//! The rule under test: a page of a listing carries at most the `maxResults` items the
//! caller asked for, carries a `nextToken` exactly when a further item follows its last
//! one — so a page that carries a token is full and the final page carries none — and the
//! pages concatenated are the whole collection in the one total order the unpaged listing
//! reports.
//!
//! The rows of the table are the four `@paginated` operations the service models
//! (`ListKeyspaces`, `ListTables`, `ListTypes`, `ListTagsForResource` — the model's
//! `@paginated` operations, as `smithy_query(query="digest")` lists them), each crossed
//! with the page sizes below: page sizes that divide the collection, ones that do not, the
//! whole collection in one page, the absent `maxResults`, and the modelled maximum. Every
//! one of those operations reaches the collection through the shared window, so one table
//! drives that window through each of them rather than through one example.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_ok, keyspace_arn, minimal_schema, settle_wait, Emulator,
};

/// The keyspace whose tables, types and tags the table below pages.
const KEYSPACE: &str = "pw_paged";

/// One row of the operation table: a `@paginated` listing, the request members that name
/// what it lists, the output member its page arrives in, and how an item of that page is
/// named so pages of different shapes compare as the same kind of sequence.
struct Listing {
    operation: &'static str,
    scope: Value,
    member: &'static str,
    item_name: fn(&Value) -> String,
}

/// The `keyspaceName` of a `KeyspaceSummary`.
fn keyspace_name(item: &Value) -> String {
    member_string(item, "keyspaceName")
}

/// The `tableName` of a `TableSummary`.
fn table_name(item: &Value) -> String {
    member_string(item, "tableName")
}

/// A `ListTypes` item, which is the type name itself rather than a structure.
fn type_name(item: &Value) -> String {
    item.as_str()
        .unwrap_or_else(|| panic!("a types item must be a string, got {item}"))
        .to_string()
}

/// The `key` of a `Tag`.
fn tag_key(item: &Value) -> String {
    member_string(item, "key")
}

fn member_string(item: &Value, member: &str) -> String {
    item.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a page item must carry `{member}`, got {item}"))
        .to_string()
}

/// The items of one page, named: an output list member that is absent (a required-length
/// list the response omits when it is empty) is an empty page, not a missing one.
fn page_items(listing: &Listing, body: &Value) -> Vec<String> {
    match body.get(listing.member) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(listing.item_name).collect(),
        Some(other) => panic!(
            "case {:?}: `{}` must be a list, got {other}",
            listing.operation, listing.member
        ),
    }
}

/// Walk every page of one listing at `page_size` (absent meaning the caller named no
/// `maxResults`), returning the items in page order and how many pages it took. The
/// window's own invariants are asserted on the way: no page is longer than what was asked
/// for, and a page carries a token only when it is full.
async fn walk_pages(
    emulator: &Emulator,
    listing: &Listing,
    page_size: Option<usize>,
    total: usize,
) -> (Vec<String>, usize) {
    let case = format!("{} at page size {page_size:?}", listing.operation);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = listing.scope.clone();
        if let Some(size) = page_size {
            request["maxResults"] = json!(size);
        }
        if let Some(resume) = &token {
            request["nextToken"] = json!(resume);
        }
        let page = assert_ok(&case, emulator.call(listing.operation, request).await);
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        let items = page_items(listing, &page);
        if let Some(size) = page_size {
            assert!(
                items.len() <= size,
                "case {case:?}: page {pages} carries {} items, more than the requested {size}",
                items.len()
            );
        }
        seen.extend(items.clone());
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                // The window takes one item beyond the page, so a token is issued only
                // when another item follows: a page carrying one is a FULL page.
                assert_eq!(
                    Some(items.len()),
                    page_size,
                    "case {case:?}: page {pages} carries a nextToken but is not full: {page}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    (seen, pages)
}

/// Create the keyspaces, tables, types and tags the table pages, and hand back the four
/// listings over them.
///
/// Everything is created in an order that is NOT the sorted one, so a page boundary
/// cannot come out right from insertion order alone.
async fn seeded_listings(emulator: &Emulator) -> Vec<Listing> {
    for keyspace in [KEYSPACE, "pw_delta", "pw_alpha", "pw_echo", "pw_bravo"] {
        assert_ok(
            keyspace,
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
    }

    for table in ["t_delta", "t_alpha", "t_echo", "t_bravo", "t_charlie", "t_fox"] {
        assert_ok(
            table,
            emulator
                .call(
                    "CreateTable",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "tableName": table,
                        "schemaDefinition": minimal_schema("pk"),
                    }),
                )
                .await,
        );
    }

    for udt in ["u_delta", "u_alpha", "u_echo", "u_bravo", "u_charlie"] {
        assert_ok(
            udt,
            emulator
                .call(
                    "CreateType",
                    json!({
                        "keyspaceName": KEYSPACE,
                        "typeName": udt,
                        "fieldDefinitions": [{"name": "value", "type": "int"}],
                    }),
                )
                .await,
        );
    }

    let tagged = keyspace_arn(KEYSPACE);
    let tags: Vec<Value> = ["tier", "env", "owner", "team", "app", "costCenter", "zone"]
        .iter()
        .enumerate()
        .map(|(index, key)| json!({"key": key, "value": format!("value{index}")}))
        .collect();
    assert_ok(
        "tag the keyspace",
        emulator
            .call(
                "TagResource",
                json!({"resourceArn": tagged, "tags": tags}),
            )
            .await,
    );

    // Everything created above settles out of its transient status before the listings
    // run, so the pages below are of settled resources.
    tokio::time::sleep(settle_wait()).await;

    vec![
        Listing {
            operation: "ListKeyspaces",
            scope: json!({}),
            member: "keyspaces",
            item_name: keyspace_name,
        },
        Listing {
            operation: "ListTables",
            scope: json!({"keyspaceName": KEYSPACE}),
            member: "tables",
            item_name: table_name,
        },
        Listing {
            operation: "ListTypes",
            scope: json!({"keyspaceName": KEYSPACE}),
            member: "types",
            item_name: type_name,
        },
        Listing {
            operation: "ListTagsForResource",
            scope: json!({"resourceArn": keyspace_arn(KEYSPACE)}),
            member: "tags",
            item_name: tag_key,
        },
    ]
}

/// Every `@paginated` listing reports through the one page window: whatever page size it
/// is driven at, it reports the whole collection in the same total order, in the number of
/// pages that page size implies, and the last page carries no token.
#[tokio::test]
async fn every_paginated_listing_reports_the_whole_collection_through_one_page_window() {
    let emulator = Emulator::start();
    let listings = seeded_listings(&emulator).await;

    for listing in &listings {
        // The unpaged listing is the reference order: `maxResults` absent is the whole
        // collection, up to the modelled maximum page size.
        let whole = assert_ok(
            listing.operation,
            emulator
                .call(listing.operation, listing.scope.clone())
                .await,
        );
        let expected = page_items(listing, &whole);
        assert!(
            whole.get("nextToken").is_none(),
            "case {:?}: a listing that fits in one page carries no nextToken: {whole}",
            listing.operation
        );
        assert!(
            expected.len() > 4,
            "case {:?}: the table needs several items for a page boundary to exist, got {expected:?}",
            listing.operation
        );
        let mut sorted = expected.clone();
        sorted.sort();
        assert_eq!(
            expected, sorted,
            "case {:?}: expected one total order {sorted:?}, got {expected:?}",
            listing.operation
        );

        let sizes = [
            None,
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(expected.len()),
            // One past the collection, and the maximum of the modelled `maxResults`
            // range (`1..1000`).
            Some(expected.len() + 1),
            Some(usize::try_from(keyspaces::MAX_PAGE_SIZE).expect("the page maximum is positive")),
        ];
        for page_size in sizes {
            let case = format!("{} at page size {page_size:?}", listing.operation);
            let (seen, pages) = walk_pages(&emulator, listing, page_size, expected.len()).await;
            assert_eq!(
                seen, expected,
                "case {case:?}: expected the whole collection {expected:?}, got {seen:?}"
            );
            let expected_pages = match page_size {
                Some(size) => expected.len().div_ceil(size).max(1),
                None => 1,
            };
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages for {} items, got {pages}",
                expected.len()
            );
        }
    }
}
