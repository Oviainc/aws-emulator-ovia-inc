// SPDX-License-Identifier: CC0-1.0

//! `ListDataflowEndpointGroups`, end to end over HTTP through the same front door as the
//! rest of this target's suites (the harness, the signed `restJson1` requests and the
//! response reader come from [`crate::scheduling_loop`]; the group create/read/list
//! helpers from [`crate::dataflow_endpoint_groups`]).
//!
//! The RULE under test, one level above any single recorded call: a listing reports every
//! resource of its kind the account holds — each exactly once, with the identifying
//! members its list-item shape declares and no others, in one stable order — in pages of
//! at most the caller's `maxResults` bounded by the modelled maximum, handing back a
//! `nextToken` exactly when items remain after the page and none when the sequence is
//! exhausted; and, being a read, it leaves the store exactly as a later caller would have
//! found it.
//!
//! Asserted here over a table of inputs rather than one call:
//!
//! * an account holding no groups is answered an empty page and no token, at every page
//!   size;
//! * every created group is listed once, carrying the `dataflowEndpointGroupId` and
//!   `dataflowEndpointGroupArn` that `GetDataflowEndpointGroup` reports for it and nothing
//!   else, whatever the group was created with (endpoint count, tags, pass durations);
//! * each page holds exactly as many items as remain up to the page size, and the token it
//!   hands back is present exactly when the sequence continues and satisfies the modelled
//!   `PaginationToken` constraints;
//! * listing — whole, repeated, or abandoned mid-walk — changes nothing a later caller
//!   sees;
//! * `maxResults` is held to its published bounds.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    BAD_REQUEST_ERROR_CODE, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::dataflow_endpoint_groups::{create_group, details, expected_arn, get_group, list_groups};
use crate::scheduling_loop::{harness, is_uuid};

/// The modelled members of `DataflowEndpointListItem`, and the only members a list item
/// may carry (source: `smithy_query(query="shape", shape="DataflowEndpointListItem")` —
/// `dataflowEndpointGroupId` and `dataflowEndpointGroupArn`).
const ITEM_MEMBERS: [&str; 2] = ["dataflowEndpointGroupId", "dataflowEndpointGroupArn"];

/// One row of the group table: a group described a different way, so the listing is
/// asserted over a class of groups rather than over one shape of create request.
struct Row {
    /// Names the row in every assertion.
    label: &'static str,
    /// How many dataflow endpoints the group is created with.
    endpoints: usize,
    /// Whether the create carries tags.
    tags: bool,
    /// `contactPrePassDurationSeconds` / `contactPostPassDurationSeconds`, when set.
    passes: Option<(i64, i64)>,
}

/// Groups differing along everything a create may vary — endpoint count, tags and the
/// pre/post-pass durations at each published bound of
/// `DataflowEndpointGroupDurationInSeconds` (range 30..480,
/// `smithy_query(query="shape", shape="DataflowEndpointGroupDurationInSeconds")`) — none
/// of which the LISTING reports, so every row must be listed the same way.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one endpoint, no tags, no pass durations",
            endpoints: 1,
            tags: false,
            passes: None,
        },
        Row {
            label: "two endpoints and tags",
            endpoints: 2,
            tags: true,
            passes: None,
        },
        Row {
            label: "three endpoints at both duration bounds",
            endpoints: 3,
            tags: false,
            passes: Some((30, 480)),
        },
    ]
}

/// One dataflow endpoint, distinct per `index` so a group may hold several.
fn endpoint(index: usize) -> Value {
    json!({
        "name": format!("downlink-demod-decode-{index}"),
        "address": {"name": format!("172.31.0.{}", 10 + index), "port": 55880 + index},
        "mtu": 1500,
    })
}

/// The `CreateDataflowEndpointGroup` body for `row`.
fn create_body(row: &Row) -> Value {
    let endpoints: Vec<Value> = (0..row.endpoints)
        .map(|index| details("endpoint", &endpoint(index), true))
        .collect();
    let mut body = json!({"endpointDetails": endpoints});
    let object = body.as_object_mut().expect("an object body");
    if row.tags {
        object.insert("tags".to_string(), json!({"mission": row.label}));
    }
    if let Some((pre, post)) = row.passes {
        object.insert("contactPrePassDurationSeconds".to_string(), json!(pre));
        object.insert("contactPostPassDurationSeconds".to_string(), json!(post));
    }
    body
}

/// Create the group `row` describes and answer the id the service minted for it.
async fn create_row(router: &MultiServiceRouter, row: &Row) -> String {
    let reply = create_group(router, create_body(row)).await;
    assert_eq!(
        reply.status, 200,
        "case {}: expected 200 from the create, got {} {}",
        row.label, reply.status, reply.body
    );
    let id = reply.string("dataflowEndpointGroupId");
    assert!(
        is_uuid(&id),
        "case {}: the minted id must be a modelled Uuid, got {id:?}",
        row.label
    );
    id
}

/// The items of one page, as objects.
fn page_items(reply_body: &Value) -> Vec<Value> {
    reply_body
        .get("dataflowEndpointGroupList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("dataflowEndpointGroupList must be a list, got {reply_body}"))
        .clone()
}

/// `?maxResults=<size>&nextToken=<token>`, with each part left out when absent.
fn query(size: Option<i64>, token: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(size) = size {
        parts.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        parts.push(format!("nextToken={token}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// Every item the listing reports at `size`, walked page by page, asserting on the way
/// that each page is bounded by the page size and that a token is handed back exactly
/// when the sequence continues.
async fn walk(router: &MultiServiceRouter, size: Option<i64>, expected_total: usize) -> Vec<Value> {
    let bound = size.unwrap_or(MAX_PAGE_SIZE as i64).min(MAX_PAGE_SIZE as i64) as usize;
    let mut items: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than the smallest page could need, so a token that fails to
    // advance fails the test instead of looping forever.
    for page in 0..=expected_total {
        let suffix = query(size, token.as_deref());
        let reply = list_groups(router, &suffix).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={size:?} page-{page}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let on_page = page_items(&reply.body);
        let remaining = expected_total - items.len();
        assert_eq!(
            on_page.len(),
            remaining.min(bound),
            "case maxResults={size:?} page-{page}: a page must hold what remains up to the \
             page size, got {}",
            reply.body
        );
        items.extend(on_page);

        let next = reply.field("nextToken").as_str().map(str::to_string);
        assert_eq!(
            next.is_some(),
            items.len() < expected_total,
            "case maxResults={size:?} page-{page}: a token must be handed back exactly when \
             items remain ({} of {expected_total} listed), got {}",
            items.len(),
            reply.body
        );
        match next {
            None => return items,
            Some(next) => {
                // The token goes back on the wire as this operation's `nextToken`, so it
                // must satisfy that member's own constraints (`PaginationToken`, len
                // 3..1000, pattern ^[A-Za-z0-9-/+_.=]+$ — `smithy_query(query="operation",
                // operation="ListDataflowEndpointGroups")`).
                assert!(
                    (3..=1000).contains(&next.len())
                        && next
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-/+_.=".contains(&b)),
                    "case maxResults={size:?} page-{page}: the token must satisfy \
                     PaginationToken, got {next:?}"
                );
                token = Some(next);
            }
        }
    }
    panic!("case maxResults={size:?}: the listing never closed after {expected_total} items");
}

/// The ids the listing reports at `size`, in the order it reported them.
///
/// `pub(crate)` so the sibling group suite walks the listing through this one helper
/// rather than keeping a second page walker in the same test target.
pub(crate) async fn listed_ids(
    router: &MultiServiceRouter,
    size: Option<i64>,
    expected_total: usize,
) -> Vec<String> {
    walk(router, size, expected_total)
        .await
        .iter()
        .map(|item| {
            item.get("dataflowEndpointGroupId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a list item without an id: {item}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn an_account_holding_no_groups_is_listed_as_empty_with_no_token() {
    let router = harness();

    for size in [None, Some(1), Some(7), Some(MAX_PAGE_SIZE as i64)] {
        let reply = list_groups(&router, &query(size, None)).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={size:?}: expected 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            page_items(&reply.body),
            Vec::<Value>::new(),
            "case maxResults={size:?}: an account with no groups must list none, got {}",
            reply.body
        );
        assert_eq!(
            reply.field("nextToken"),
            &Value::Null,
            "case maxResults={size:?}: an exhausted listing hands back no token, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn every_group_is_listed_once_with_exactly_the_id_and_arn_the_read_reports() {
    let router = harness();

    let table = rows();
    let mut created: Vec<(&'static str, String)> = Vec::new();
    for row in &table {
        created.push((row.label, create_row(&router, row).await));
    }

    let items = walk(&router, None, table.len()).await;
    for (label, id) in &created {
        let listed: Vec<&Value> = items
            .iter()
            .filter(|item| item.get("dataflowEndpointGroupId") == Some(&json!(id)))
            .collect();
        assert_eq!(
            listed.len(),
            1,
            "case {label}: the group must be listed exactly once, got {} occurrences in {items:?}",
            listed.len()
        );

        let item = listed[0].as_object().expect("a list item object");
        let mut members: Vec<&str> = item.keys().map(String::as_str).collect();
        members.sort();
        let mut modelled = ITEM_MEMBERS;
        modelled.sort();
        assert_eq!(
            members, modelled,
            "case {label}: a list item carries exactly the modelled members, got {item:?}"
        );

        // The listing's ARN is the one the read of the same group reports, which is the one
        // the service minted — the listing reports state, it does not mint a second ARN.
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {label}: expected 200 from the read, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            item.get("dataflowEndpointGroupArn"),
            Some(&json!(read.string("dataflowEndpointGroupArn"))),
            "case {label}: the listed ARN must be the one the read reports, got {item:?}"
        );
        assert_eq!(
            item.get("dataflowEndpointGroupArn"),
            Some(&json!(expected_arn(id))),
            "case {label}: the listed ARN must name this group under the request's region \
             and the emulator's account, got {item:?}"
        );
    }

    assert_eq!(
        items.len(),
        table.len(),
        "the listing must report no group that was never created, got {items:?}"
    );
}

#[tokio::test]
async fn the_page_size_bounds_each_page_and_paging_yields_the_same_sequence() {
    let router = harness();

    let table = rows();
    let mut expected: Vec<String> = Vec::new();
    // Enough groups that a page size can divide the sequence evenly, unevenly, exactly,
    // or not bound it at all.
    for index in 0..7 {
        let row = &table[index % table.len()];
        expected.push(create_row(&router, row).await);
    }
    let total = expected.len();

    let whole = listed_ids(&router, None, total).await;
    let mut sorted = whole.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        total,
        "case whole-listing: every group exactly once, got {whole:?}"
    );
    expected.sort();
    assert_eq!(
        sorted, expected,
        "case whole-listing: expected {expected:?}, got {whole:?}"
    );

    for size in [1_i64, 2, 3, 6, 7, 8, MAX_PAGE_SIZE as i64] {
        let paged = listed_ids(&router, Some(size), total).await;
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every group exactly once, in the \
             listing's own order, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = harness();

    let table = rows();
    let mut created: Vec<String> = Vec::new();
    for row in &table {
        created.push(create_row(&router, row).await);
    }
    let total = created.len();

    let before = listed_ids(&router, None, total).await;

    // A whole walk, a repeated walk, and a walk abandoned after its first page (a caller
    // that never came back for the token it was handed): none of them may consume,
    // reorder or hide a group.
    let first_page = list_groups(&router, &query(Some(1), None)).await;
    assert_eq!(
        first_page.status, 200,
        "case abandoned-walk: expected 200, got {} {}",
        first_page.status, first_page.body
    );
    let _ = listed_ids(&router, Some(2), total).await;

    for case in ["after a whole walk", "after an abandoned walk"] {
        let after = listed_ids(&router, None, total).await;
        assert_eq!(
            after, before,
            "case {case}: the listing must be unchanged, expected {before:?}, got {after:?}"
        );
    }

    for (index, id) in created.iter().enumerate() {
        let read = get_group(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case group {index} read back after listing: expected 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("dataflowEndpointGroupId"),
            *id,
            "case group {index} read back after listing: wrong group, got {}",
            read.body
        );
    }
}

#[tokio::test]
async fn max_results_is_held_to_its_published_bounds() {
    let router = harness();
    let table = rows();
    for row in &table {
        create_row(&router, row).await;
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListDataflowEndpointGroups")` — range
    // 1..100). A value the model refuses never reaches the handler: this service's
    // validator answers it.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("inside the range", 50, true),
        ("the upper bound", MAX_PAGE_SIZE as i64, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", MAX_PAGE_SIZE as i64 + 1, false),
    ] {
        let reply = list_groups(&router, &query(Some(size), None)).await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={size} ({case}): expected 200, got {} {}",
                reply.status, reply.body
            );
            assert!(
                page_items(&reply.body).len() <= size.min(table.len() as i64) as usize,
                "case maxResults={size} ({case}): a page must hold at most the page size, \
                 got {}",
                reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case maxResults={size} ({case}): expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case maxResults={size} ({case}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }
}
