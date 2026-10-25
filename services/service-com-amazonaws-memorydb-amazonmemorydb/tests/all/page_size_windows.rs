// SPDX-License-Identifier: CC0-1.0

//! The page-size window is a property of the RESOURCE SURFACE a listing reads, not of
//! the service — end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * one `MaxResults` is served by the listings of one surface and refused by those of
//!   another: the access-control listings (`DescribeUsers`, `DescribeACLs`) serve
//!   `1..=50`, the cluster-side and parameter-group listings `20..=100`, and a size
//!   outside a listing's own window is refused with the modelled
//!   `InvalidParameterValueException` naming THAT window's bounds in the noun that
//!   surface words the page size with;
//! * the smallest page size a surface serves really pages: an access-control listing
//!   asked for one item at a time hands out one item plus a token per call and the pages
//!   concatenate to the whole set, with nothing repeated or dropped.
//!
//! Which page sizes each listing serves at its own bounds, how a served size bounds the
//! page, and how a token resumes a listing, are asserted for EVERY paginated listing in
//! `tests/all/page_size_window.rs`; this suite is about the windows differing per
//! surface.

use serde_json::{json, Value};

use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::user_lifecycle::create_user;

/// A listing of the ACCESS-CONTROL surface, with the member it answers its items in.
///
/// Recorded behaviour: these serve a `MaxResults` of 1 (one item per page, the token
/// advancing until the set is exhausted) and refuse only a size outside `1..=50`, naming
/// the page size "max results".
const ACCESS_CONTROL_LISTINGS: &[(&str, &str)] = &[("DescribeUsers", "Users"), ("DescribeACLs", "ACLs")];

/// The bounds of the access-control window, and the sentence a size outside it is
/// refused with. Spelled out HERE rather than taken from the crate under test, so the
/// assertions below are independent of the code that enforces the window.
const ACCESS_CONTROL_MIN: i32 = 1;
const ACCESS_CONTROL_MAX: i32 = 50;
const ACCESS_CONTROL_MESSAGE: &str = "Invalid value for max results. Must be between 1 and 50";

/// The other paginated listings, with the member they answer their items in and the
/// sentence each refuses a size outside ITS window with: the cluster-side listings serve
/// `20..=100` and call the page size "max records", the parameter-group surface serves
/// the same bounds and calls it "max results".
const WIDE_LISTINGS: &[(&str, &str, &str)] = &[
    (
        "DescribeClusters",
        "Clusters",
        "Invalid value for max records. Must be between 20 and 100",
    ),
    (
        "DescribeEngineVersions",
        "EngineVersions",
        "Invalid value for max records. Must be between 20 and 100",
    ),
    (
        "DescribeParameterGroups",
        "ParameterGroups",
        "Invalid value for max results. Must be between 20 and 100",
    ),
];

/// The bounds of the window those listings serve.
const WIDE_MIN: i32 = 20;
const WIDE_MAX: i32 = 100;

/// A password that breaks none of the documented password rules.
const PASSWORD: &str = "Sim2realPassword-1";

/// The `message` of a failure body, under either spelling of the member.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// The items one page of a listing reported, under the member that listing answers them
/// in.
fn page_items(body: &Value, items: &str, case: &str) -> Vec<Value> {
    body.get(items)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected {items} in {body}"))
        .clone()
}

/// A store every listing in this suite has something to report from: three users and
/// three ACLs beside the built-ins the account already had, plus a cluster and a
/// parameter group.
async fn seeded_store() -> smithy_server_runtime::MultiServiceRouter {
    let router = router();
    for suffix in ["a", "b", "c"] {
        create_user(&router, &format!("window-user-{suffix}")).await;
        ok(
            &router,
            "CreateACL",
            &json!({
                "ACLName": format!("window-acl-{suffix}"),
                "UserNames": [format!("window-user-{suffix}")],
            }),
            suffix,
        )
        .await;
    }
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "window-cluster",
            "NodeType": "db.t4g.small",
            "ACLName": "window-acl-a",
        }),
        "setup",
    )
    .await;
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": "window-group", "Family": "memorydb_valkey7" }),
        "setup",
    )
    .await;
    router
}

/// One page size is served by the listings of one surface and refused by those of
/// another, and a refusal names the bounds of the window that listing's OWN surface
/// publishes: the window travels with the resource surface, not with the service.
#[tokio::test]
async fn one_page_size_is_served_by_one_surface_and_refused_by_the_other() {
    let router = seeded_store().await;

    // (page size, served by the access-control listings?, served by the wider ones?).
    // The rows span both windows: each bound of each, one inside and one outside each
    // bound, the sizes only one surface serves, and the sizes neither serves.
    let rows: &[(i32, bool, bool)] = &[
        (ACCESS_CONTROL_MIN, true, false),
        (ACCESS_CONTROL_MIN + 1, true, false),
        (WIDE_MIN - 1, true, false),
        (WIDE_MIN, true, true),
        (WIDE_MIN + 1, true, true),
        (ACCESS_CONTROL_MAX - 1, true, true),
        (ACCESS_CONTROL_MAX, true, true),
        (ACCESS_CONTROL_MAX + 1, false, true),
        (WIDE_MAX, false, true),
        (WIDE_MAX + 1, false, false),
        (0, false, false),
        (-1, false, false),
        (i32::from(i16::MAX), false, false),
    ];

    for (page_size, access_control_serves, wide_serves) in rows.iter().copied() {
        for (operation, items) in ACCESS_CONTROL_LISTINGS.iter().copied() {
            let case = format!("{operation} MaxResults={page_size}");
            let request = json!({ "MaxResults": page_size });
            let (status, body) = call(&router, operation, &request).await;
            if access_control_serves {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the access-control window serves this size, got \
                     {status} with {body}"
                );
                let page = page_items(&body, items, &case);
                assert!(
                    i32::try_from(page.len()).unwrap_or(i32::MAX) <= page_size,
                    "case {case:?}: a page holds at most MaxResults items, got {page:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: expected the modelled invalid-value refusal, got \
                     {status} with {body}"
                );
                assert_eq!(
                    message(&body, &case),
                    ACCESS_CONTROL_MESSAGE,
                    "case {case:?}: expected the access-control window's own bounds, got \
                     {body}"
                );
            }
        }

        for (operation, items, refusal) in WIDE_LISTINGS.iter().copied() {
            let case = format!("{operation} MaxResults={page_size}");
            let request = json!({ "MaxResults": page_size });
            let (status, body) = call(&router, operation, &request).await;
            if wide_serves {
                assert_eq!(
                    status, 200,
                    "case {case:?}: this listing's window serves this size, got {status} \
                     with {body}"
                );
                let page = page_items(&body, items, &case);
                assert!(
                    i32::try_from(page.len()).unwrap_or(i32::MAX) <= page_size,
                    "case {case:?}: a page holds at most MaxResults items, got {page:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: expected the modelled invalid-value refusal, got \
                     {status} with {body}"
                );
                assert_eq!(
                    message(&body, &case),
                    refusal,
                    "case {case:?}: expected this listing's own window bounds, got {body}"
                );
            }
        }
    }
}

/// The smallest page size the access-control surface serves really pages: a listing
/// asked for one item at a time hands out one item plus a token per call, and the pages
/// concatenate to the unpaged listing with nothing repeated or dropped — the paging loop
/// a client cannot run at all if the listing refuses the size.
#[tokio::test]
async fn the_access_control_listings_page_at_the_smallest_served_size() {
    let router = seeded_store().await;

    for (operation, items) in ACCESS_CONTROL_LISTINGS.iter().copied() {
        let case = format!("{operation} unpaged");
        let whole = page_items(&ok(&router, operation, &json!({}), &case).await, items, &case);
        assert!(
            whole.len() > 2,
            "case {case:?}: the set must take several pages at the smallest size, got {} \
             items",
            whole.len()
        );

        // Every page size from the bottom of the window up to one past the set's own
        // size: each walk visits the whole set once, in the listing's own order.
        let largest = i32::try_from(whole.len()).unwrap_or(i32::MAX) + 1;
        for page_size in ACCESS_CONTROL_MIN..=largest {
            let case = format!("{operation} MaxResults={page_size}");
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut request = json!({ "MaxResults": page_size });
                if let Some(token) = token.as_deref() {
                    request["NextToken"] = json!(token);
                }
                let body = ok(&router, operation, &request, &case).await;
                let page = page_items(&body, items, &case);
                token = body
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if token.is_some() {
                    // A page before the last is FULL: the size asked for bounded it.
                    assert_eq!(
                        i32::try_from(page.len()).unwrap_or(i32::MAX),
                        page_size,
                        "case {case:?}: a page before the last holds exactly {page_size} \
                         items, got {}",
                        page.len()
                    );
                }
                walked.extend(page);
                pages += 1;
                assert!(
                    pages <= whole.len(),
                    "case {case:?}: the walk must end, got {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }
            let expected_pages = whole.len().div_ceil(usize::try_from(page_size).unwrap_or(1));
            assert_eq!(
                pages,
                expected_pages.max(1),
                "case {case:?}: a set of {} items takes {expected_pages} pages of \
                 {page_size}, got {pages}",
                whole.len()
            );
            assert_eq!(
                walked, whole,
                "case {case:?}: the pages together must be the whole listing, in order"
            );
        }
    }
}

/// A user a page size of one reaches is the SAME resource the unpaged listing reports:
/// paging narrows the page, it does not change what the service holds, so a one-item
/// walk reports each created user's own persisted state.
#[tokio::test]
async fn paging_one_item_at_a_time_reports_the_persisted_users() {
    let router = router();
    let names = ["walk-user-a", "walk-user-b", "walk-user-c"];
    for name in names {
        ok(
            &router,
            "CreateUser",
            &json!({
                "UserName": name,
                "AccessString": "on ~* +@all",
                "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
            }),
            name,
        )
        .await;
    }

    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let case = "one user per page";
    loop {
        let mut request = json!({ "MaxResults": ACCESS_CONTROL_MIN });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = ok(&router, "DescribeUsers", &request, case).await;
        walked.extend(page_items(&body, "Users", case));
        token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= names.len() + 1,
            "case {case:?}: the walk must not outgrow the set, visited {walked:?}"
        );
    }

    for name in names {
        let walked_user = walked
            .iter()
            .find(|user| user["Name"] == json!(name))
            .unwrap_or_else(|| panic!("case {case:?}: expected {name} in {walked:?}"));
        let described = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": name }),
            case,
        )
        .await;
        assert_eq!(
            walked_user, &page_items(&described, "Users", case)[0],
            "case {case:?}: the paged user must be the one state holds for {name}"
        );
    }
}
