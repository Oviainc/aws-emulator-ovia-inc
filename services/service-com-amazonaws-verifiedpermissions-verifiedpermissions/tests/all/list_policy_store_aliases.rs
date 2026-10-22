// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `ListPolicyStoreAliases`.
//!
//! The rule under test: the operation reports every policy store alias the
//! account holds — whatever its `state` — narrowed by `filter`, in a stable
//! order, in pages of at most `maxResults` items (5 when the caller names none,
//! 50 at most), with a `nextToken` present exactly when items remain and
//! resuming exactly where the page stopped.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! listed alias is only listed if it really reached state.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, error_code, ok, store_with_validation_off as store, SIGNING_REGION};

/// The prefix every alias name carries, from
/// `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias name
/// must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The page size the operation uses when the caller names no `maxResults`, from
/// `ListPolicyStoreAliasesInput$maxResults`'s documentation: "If you do not
/// specify this parameter, the operation defaults to 5 policy store aliases per
/// response" (the model states the same as that member's `@default: 5`).
const DEFAULT_PAGE_SIZE: usize = 5;

/// The largest page the operation builds, from the same sentence: "You can
/// specify a maximum of 50 policy store aliases per response."
const MAX_PAGE_SIZE: usize = 50;

/// How many pages a drain will walk before declaring the cursor broken. Far
/// above any row below, so a cursor that never advances fails the test instead
/// of hanging it.
const PAGE_WALK_LIMIT: usize = 500;

/// The alias name for index `i`, zero-padded so lexicographic order and numeric
/// order agree and a row can state the order it expects.
fn alias_name(i: usize) -> String {
    format!("{ALIAS_PREFIX}store-{i:03}")
}

/// The ARN an alias of `alias_name` must carry — the same shape
/// `tests/all/policy_store_alias.rs` pins for `CreatePolicyStoreAlias`, repeated
/// here because this suite asserts the LIST item carries it too.
fn expected_alias_arn(region: &str, alias_name: &str) -> String {
    format!("arn:aws:verifiedpermissions:{region}:000000000000:{alias_name}")
}

/// Create an alias, requiring success.
async fn create_alias(router: &MultiServiceRouter, alias_name: &str, store_id: &str) -> Value {
    ok(
        router,
        "CreatePolicyStoreAlias",
        &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
    )
    .await
}

/// One `ListPolicyStoreAliases` page, requiring success. `filter` is the
/// `filter.policyStoreId` to narrow by, `max_results` the asked-for page size
/// and `next_token` the cursor to resume from; each is omitted from the request
/// when `None`, so a row tests the absent member rather than a stand-in value.
async fn list_page(
    router: &MultiServiceRouter,
    filter: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Value {
    let mut body = json!({});
    if let Some(store_id) = filter {
        body["filter"] = json!({ "policyStoreId": store_id });
    }
    if let Some(max_results) = max_results {
        body["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["nextToken"] = json!(next_token);
    }
    ok(router, "ListPolicyStoreAliases", &body).await
}

/// The `policyStoreAliases` of one page. The member is **required**, so a
/// response that omits it is a defect rather than an empty page.
fn page_items(case: &str, page: &Value) -> Vec<Value> {
    page["policyStoreAliases"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: policyStoreAliases is required, got {page}"))
        .clone()
}

/// Walk every page and return `(all items in page order, the length of each
/// page)`.
async fn drain(
    case: &str,
    router: &MultiServiceRouter,
    filter: Option<&str>,
    max_results: Option<i64>,
) -> (Vec<Value>, Vec<usize>) {
    let mut items = Vec::new();
    let mut page_lengths = Vec::new();
    let mut token: Option<String> = None;
    for walked in 0.. {
        assert!(
            walked < PAGE_WALK_LIMIT,
            "case {case:?}: the cursor never finished after {walked} pages"
        );
        let page = list_page(router, filter, max_results, token.as_deref()).await;
        let batch = page_items(case, &page);
        page_lengths.push(batch.len());
        items.extend(batch);
        match page["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    (items, page_lengths)
}

/// The `aliasName`s of a list of items, in the order they arrived.
fn names_of(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| item["aliasName"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Paging reports every alias in the account exactly once, in pages of at most
/// the effective size, with `nextToken` present exactly while items remain.
///
/// The rows differ along how many aliases the account holds and what page size
/// is asked for: no aliases at all, a single-item page, the documented default
/// (an absent `maxResults`), a page size that divides the count exactly, one
/// larger than the whole set, and one far above the documented 50-item maximum,
/// which is answered with a 50-item page rather than a larger one.
#[tokio::test]
async fn paging_reports_every_alias_exactly_once() {
    // (case, how many aliases exist, the asked-for maxResults)
    let cases: &[(&str, usize, Option<i64>)] = &[
        ("empty account, default page size", 0, None),
        ("empty account, explicit page size", 0, Some(3)),
        ("one alias, one per page", 1, Some(1)),
        ("seven aliases, default page size", 7, None),
        ("twelve aliases, page size divides the count", 12, Some(4)),
        ("five aliases, page larger than the set", 5, Some(10)),
        ("thirteen aliases, one per page", 13, Some(1)),
        ("at the documented maximum", 51, Some(MAX_PAGE_SIZE as i64)),
        ("above the documented maximum", 51, Some(1_000)),
    ];

    for (case, count, max_results) in cases {
        let router = super::harness::router();
        let store_id = store(&router).await;
        // Created in DESCENDING index order, so the reported order cannot come
        // from insertion order by accident.
        let mut expected: Vec<String> = Vec::new();
        for i in (0..*count).rev() {
            create_alias(&router, &alias_name(i), &store_id).await;
            expected.push(alias_name(i));
        }
        expected.sort();

        let effective = max_results
            .map(|n| usize::try_from(n).unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE))
            .unwrap_or(DEFAULT_PAGE_SIZE);

        let (items, page_lengths) = drain(case, &router, None, *max_results).await;

        let mut reported = names_of(&items);
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: every alias must be reported exactly once, got {reported:?}"
        );
        for (page, length) in page_lengths.iter().enumerate() {
            assert!(
                *length <= effective,
                "case {case:?}: page {page} holds {length} items, more than the effective page size {effective}"
            );
        }
        // Every page but the last is FULL: a short page followed by a cursor
        // would make a client walk more pages than the data needs.
        let full_pages = page_lengths.len().saturating_sub(1);
        for (page, length) in page_lengths.iter().take(full_pages).enumerate() {
            assert_eq!(
                *length, effective,
                "case {case:?}: page {page} is short at {length} items but a cursor followed it"
            );
        }
        assert_eq!(
            page_lengths[0],
            effective.min(*count),
            "case {case:?}: unexpected first-page size, pages were {page_lengths:?}"
        );
    }
}

/// The reported order is stable across calls, and a cursor resumes exactly
/// where the previous page stopped — the invariant pagination rests on.
///
/// The rows differ along the page size, so the resume point falls at a
/// different place in the set each time.
#[tokio::test]
async fn the_order_is_stable_and_a_cursor_resumes_where_the_page_stopped() {
    let router = super::harness::router();
    let store_id = store(&router).await;
    for i in 0..9 {
        create_alias(&router, &alias_name(i), &store_id).await;
    }

    let (whole, _) = drain("whole set", &router, None, Some(50)).await;
    let order = names_of(&whole);
    assert_eq!(
        order.len(),
        9,
        "the whole set must come back in one page, got {order:?}"
    );

    for page_size in [1_i64, 2, 3, 8] {
        let case = format!("page size {page_size}");
        let (items, _) = drain(&case, &router, None, Some(page_size)).await;
        assert_eq!(
            names_of(&items),
            order,
            "case {case:?}: paging must report the same order as a single page"
        );

        // The cursor the first page hands back resumes at the item right after
        // that page, so the two halves concatenate into the whole set.
        let first = list_page(&router, None, Some(page_size), None).await;
        let token = first["nextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a truncated page must carry a cursor, got {first}"))
            .to_string();
        let mut walked = names_of(&page_items(&case, &first));
        let second = list_page(&router, None, Some(1_000), Some(&token)).await;
        walked.extend(names_of(&page_items(&case, &second)));
        assert_eq!(
            walked, order,
            "case {case:?}: resuming from the cursor must continue the same order"
        );
    }
}

/// A `nextToken` this service did not issue is refused as a `ValidationException`
/// rather than answered with an empty page, which would hide the client's
/// mistake. A cursor the service DID issue is accepted.
///
/// The rows differ along how the token is malformed: not an encoding this
/// service produces, an odd-length one, and one carrying characters the cursor
/// alphabet does not use.
#[tokio::test]
async fn a_cursor_this_service_did_not_issue_is_refused() {
    let router = super::harness::router();
    let store_id = store(&router).await;
    for i in 0..4 {
        create_alias(&router, &alias_name(i), &store_id).await;
    }

    let cases: &[(&str, &str)] = &[
        ("not an encoding this service produces", "zzzz"),
        ("odd-length cursor", "abc"),
        ("outside the cursor alphabet", "Q.Q.Q"),
    ];
    for (case, token) in cases {
        let (status, body) = call(
            &router,
            "ListPolicyStoreAliases",
            &json!({ "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );
    }

    // The service's own cursor still works, so the refusal above is about the
    // token and not about the member being set at all.
    let first = list_page(&router, None, Some(2), None).await;
    let token = first["nextToken"]
        .as_str()
        .expect("a truncated page carries a cursor")
        .to_string();
    let resumed = list_page(&router, None, Some(2), Some(&token)).await;
    assert_eq!(
        page_items("issued cursor", &resumed).len(),
        2,
        "the service's own cursor must be accepted, got {resumed}"
    );
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

/// `filter.policyStoreId` reports only the aliases associated with that store,
/// and a filter naming a store the account does not hold reports an empty list —
/// the operation models no `ResourceNotFoundException`, so a filter is a
/// predicate and never a lookup.
///
/// The rows differ along how many aliases the filtered store holds: none, one
/// and several, plus a store id nothing is associated with.
#[tokio::test]
async fn a_filter_selects_only_the_aliases_of_the_named_store() {
    let router = super::harness::router();
    // Three stores, holding 3, 1 and 0 aliases: the counts differ so a filter
    // that ignored the member would show up on every row.
    let mut stores = Vec::new();
    let mut expected: Vec<Vec<String>> = Vec::new();
    let mut next_index = 0;
    for count in [3_usize, 1, 0] {
        let store_id = store(&router).await;
        let mut names = Vec::new();
        for _ in 0..count {
            let name = alias_name(next_index);
            next_index += 1;
            create_alias(&router, &name, &store_id).await;
            names.push(name);
        }
        names.sort();
        stores.push(store_id);
        expected.push(names);
    }

    for (store_id, names) in stores.iter().zip(expected.iter()) {
        let case = format!("store holding {} aliases", names.len());
        let (items, _) = drain(&case, &router, Some(store_id), Some(2)).await;
        let mut reported = names_of(&items);
        reported.sort();
        assert_eq!(
            &reported, names,
            "case {case:?}: the filter must select exactly that store's aliases, got {reported:?}"
        );
        for item in &items {
            assert_eq!(
                item["policyStoreId"].as_str().unwrap_or_default(),
                store_id.as_str(),
                "case {case:?}: a filtered item must name the filtered store, got {item}"
            );
        }
    }

    // A store id nothing is associated with: an empty list, not a failure.
    let page = list_page(&router, Some("PSNOSUCHSTORE000000000"), None, None).await;
    assert!(
        page_items("unknown store", &page).is_empty(),
        "an unmatched filter must report an empty list, got {page}"
    );
    assert!(
        page["nextToken"].is_null(),
        "an empty page must carry no cursor, got {page}"
    );
    // The unfiltered list still holds every alias, so the filter narrowed the
    // response and did not change state.
    let (all, _) = drain("unfiltered", &router, None, Some(50)).await;
    assert_eq!(
        all.len(),
        4,
        "the account's four aliases must all still be listed, got {:?}",
        names_of(&all)
    );
}

// ---------------------------------------------------------------------------
// What each item carries
// ---------------------------------------------------------------------------

/// Every member of a listed item is what `GetPolicyStoreAlias` reports for the
/// same alias, including its `state`: an `Active` alias and a soft-deleted
/// `PendingDeletion` one are both listed, and a hard-deleted one is gone.
///
/// The rows differ along the alias's state, which is what the list has to report
/// faithfully rather than filter on.
#[tokio::test]
async fn a_listed_item_matches_the_reader_for_every_alias_state() {
    let router = super::harness::router();
    let store_id = store(&router).await;

    let active = alias_name(0);
    let pending = alias_name(1);
    let removed = alias_name(2);
    for name in [&active, &pending, &removed] {
        create_alias(&router, name, &store_id).await;
    }
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": pending, "deletionMode": "SoftDelete" }),
    )
    .await;
    ok(
        &router,
        "DeletePolicyStoreAlias",
        &json!({ "aliasName": removed, "deletionMode": "HardDelete" }),
    )
    .await;

    let (items, _) = drain("every state", &router, None, Some(50)).await;
    let mut reported = names_of(&items);
    reported.sort();
    let mut expected = vec![active.clone(), pending.clone()];
    expected.sort();
    assert_eq!(
        reported, expected,
        "a soft-deleted alias stays listed and a hard-deleted one is gone, got {reported:?}"
    );

    for (case, name, expected_state) in [
        ("active alias", &active, "Active"),
        ("soft-deleted alias", &pending, "PendingDeletion"),
    ] {
        let item = items
            .iter()
            .find(|item| item["aliasName"].as_str() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {name} must be listed"));
        let read = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": name }),
        )
        .await;
        assert_eq!(
            (
                item["aliasName"].as_str().unwrap_or_default(),
                item["policyStoreId"].as_str().unwrap_or_default(),
                item["aliasArn"].as_str().unwrap_or_default(),
                item["createdAt"].as_str().unwrap_or_default(),
                item["state"].as_str().unwrap_or_default(),
            ),
            (
                read["aliasName"].as_str().unwrap_or_default(),
                read["policyStoreId"].as_str().unwrap_or_default(),
                read["aliasArn"].as_str().unwrap_or_default(),
                read["createdAt"].as_str().unwrap_or_default(),
                read["state"].as_str().unwrap_or_default(),
            ),
            "case {case:?}: the listed item must match the reader, got {item} vs {read}"
        );
        assert_eq!(
            item["state"].as_str().unwrap_or_default(),
            expected_state,
            "case {case:?}: unexpected state, got {item}"
        );
        assert_eq!(
            item["aliasArn"].as_str().unwrap_or_default(),
            expected_alias_arn(SIGNING_REGION, name),
            "case {case:?}: unexpected aliasArn, got {item}"
        );
    }
}

/// Deleting the policy store an alias names takes the alias out of the list, so
/// the list reports persisted state and not a cache of its own.
#[tokio::test]
async fn deleting_the_store_removes_its_aliases_from_the_list() {
    let router = super::harness::router();
    let doomed_store = store(&router).await;
    let kept_store = store(&router).await;
    let doomed_alias = alias_name(0);
    let kept_alias = alias_name(1);
    create_alias(&router, &doomed_alias, &doomed_store).await;
    create_alias(&router, &kept_alias, &kept_store).await;

    let (before, _) = drain("before the store delete", &router, None, Some(50)).await;
    assert_eq!(
        names_of(&before).len(),
        2,
        "both aliases must be listed first, got {:?}",
        names_of(&before)
    );

    ok(
        &router,
        "DeletePolicyStore",
        &json!({ "policyStoreId": doomed_store }),
    )
    .await;

    let (after, _) = drain("after the store delete", &router, None, Some(50)).await;
    assert_eq!(
        names_of(&after),
        vec![kept_alias],
        "only the surviving store's alias may be listed, got {:?}",
        names_of(&after)
    );
}
