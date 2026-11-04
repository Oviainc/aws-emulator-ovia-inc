// SPDX-License-Identifier: CC0-1.0

//! The order `ListApplications` pages the account's applications in.
//!
//! RULE: the listing reports the account's applications MOST RECENTLY CREATED FIRST — the
//! order the recorded service pages them in, and the order this service serves every other
//! sequence it lists (a definition's runs, a run's timeline, an environment's deployments,
//! a profile's hosted configuration versions). The place an application was created in is
//! assigned once and never reissued, so the order is a total one that does not move between
//! calls: the head of the first page is the newest application whatever page size the
//! request asks for, a page boundary falls at the same entry however the walk is cut, and a
//! create or a delete between two calls shifts no page a cursor has already passed.
//!
//! The table spans what the rule generalises over: HOW MANY applications the account holds
//! (one, several, and one past the service's own page size), WHICH page size the walk asks
//! for (absent and both published bounds of `MaxResults`), and WHICH mutation last changed
//! the collection (a create, a delete of the newest, a delete of the oldest, and a create
//! after a delete — which is what a place taken from the map's size rather than counted
//! would reuse). Every assertion names its case and prints the actual value.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::MAX_LIST_PAGE_SIZE;

use crate::appconfig_workflow::{create_application, json_call, router};

/// The ids a listing serves, in the order it served them, walking every page.
///
/// `max_results` is the page size the walk asks for, `None` for a request that names none.
async fn walk(router: &MultiServiceRouter, case: &str, max_results: Option<i32>) -> Vec<String> {
    let mut served: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded, so a listing that never ends its traversal fails rather than hanging.
    for _ in 0..=(MAX_LIST_PAGE_SIZE * 4 + 2) {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("max_results={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("next_token={token}"));
        }
        let uri = if query.is_empty() {
            "/applications".to_string()
        } else {
            format!("/applications?{}", query.join("&"))
        };
        let answer = json_call(router, "GET", &uri, &Value::Null).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: ListApplications must answer 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        served.extend(
            answer
                .json()
                .get("Items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a listed application must carry an Id: {item}")
                        })
                        .to_string()
                }),
        );
        token = answer.string("NextToken");
        if token.is_none() {
            return served;
        }
    }
    panic!("case {case:?}: the paginated walk did not terminate");
}

/// The order the rule requires of an account whose applications were created in `created`:
/// the newest first, so the creation order reversed.
fn newest_first(created: &[String]) -> Vec<String> {
    created.iter().rev().cloned().collect()
}

/// Assert the whole listing, at every page size, is the account's applications newest
/// first.
async fn assert_newest_first(router: &MultiServiceRouter, case: &str, created: &[String]) {
    let expected = newest_first(created);
    // The page sizes are the published bounds of `MaxResults` (`@range 1..50`) and values
    // between, plus a request that names none at all: cutting the same order into different
    // pages cannot change which application comes first.
    for page_size in [None, Some(1), Some(2), Some(3), Some(MAX_LIST_PAGE_SIZE as i32)] {
        let case = format!("{case}, MaxResults={page_size:?}");
        let served = walk(router, &case, page_size).await;
        assert_eq!(
            served, expected,
            "case {case:?}: the listing reports the account's applications most recently \
             created first {expected:?}, got {served:?}"
        );
    }
}

/// The listing reports the applications newest first, whatever page size the walk asks for,
/// as the collection grows one application at a time.
#[tokio::test]
async fn the_listing_reports_the_newest_application_first() {
    let router = router();

    // Rows differ along how many applications the account holds, from one up.
    let mut created: Vec<String> = Vec::new();
    for (case, name) in [
        ("one application", "alpha"),
        ("two applications", "beta"),
        ("three applications", "gamma"),
        ("four applications", "delta"),
        ("five applications", "epsilon"),
    ] {
        created.push(create_application(&router, name).await);
        assert_newest_first(&router, case, &created).await;
    }
}

/// A delete and a later create do not disturb the order: an application's place in the
/// creation sequence is never reissued, so the application created after a delete is the
/// newest one rather than one that takes the deleted application's place.
#[tokio::test]
async fn a_create_after_a_delete_is_the_newest_application() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for name in ["first", "second", "third"] {
        created.push(create_application(&router, name).await);
    }
    assert_newest_first(&router, "three applications", &created).await;

    // Rows differ along WHICH application is taken out of the collection next — the newest,
    // the oldest, one from the middle — and each delete is followed by a create, which is
    // the case a place taken from the collection's size would get wrong.
    for (case, position) in [
        ("the newest deleted", 2_usize),
        ("the oldest deleted", 0),
        ("one from the middle deleted", 1),
    ] {
        let doomed = created.remove(position);
        let answer = json_call(
            &router,
            "DELETE",
            &format!("/applications/{doomed}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            204,
            "case {case:?}: DeleteApplication must answer 204, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_newest_first(&router, case, &created).await;

        created.push(create_application(&router, &format!("after-{position}")).await);
        assert_newest_first(&router, &format!("{case}, then a create"), &created).await;
    }
}

/// One application past the service's own page size: the first page holds the newest
/// entries and the leftover page the oldest, so the cut falls where the order says it does
/// rather than where the store happens to hold a record.
#[tokio::test]
async fn a_collection_past_the_page_size_is_cut_by_the_same_order() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for index in 0..=MAX_LIST_PAGE_SIZE {
        created.push(create_application(&router, &format!("app-{index}")).await);
    }

    let case = "one application past the page size";
    let expected = newest_first(&created);
    let served = walk(&router, case, None).await;
    assert_eq!(
        served, expected,
        "case {case:?}: the pages must concatenate to the newest-first order {expected:?}, got \
         {served:?}"
    );
    assert_eq!(
        served.first(),
        expected.first(),
        "case {case:?}: the head of the first page is the newest application {:?}, got {:?}",
        expected.first(),
        served.first()
    );
    assert_eq!(
        served.last(),
        created.first(),
        "case {case:?}: the last entry served is the oldest application {:?}, got {:?}",
        created.first(),
        served.last()
    );
}
