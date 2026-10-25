// SPDX-License-Identifier: CC0-1.0

//! The one page-size rule every list operation of this service pages by, driven end to
//! end over HTTP through an operation of each kind.
//!
//! The rule under test: the number of entries a list operation puts on one page is
//! `state::page_size(MaxResults)` — the single helper the crate holds for turning the
//! modelled `MaxResults` into a page size — so `ListServiceVersions`, `ListJobs` and
//! `ListClusterJobs` all read a declared page size, and the 0 the member admits, the same
//! way.
//!
//! Each row is a `MaxResults` along the axis the rule generalises over: absent, each
//! published bound of the modelled range, and sizes that do and do not divide the listing.
//! Every expectation is computed by calling `state::page_size` itself, so an operation
//! that reads `MaxResults` by any other reading cannot pass, and the suite cannot drift
//! from the helper.
//!
//! The harness (`router`, `ok`, `create_address`, `deliverable_address`, `complete_order`,
//! `string`) and the cluster helper (`base_cluster`) are the ones this crate's other
//! suites already drive the service with, so every call here goes through the same front
//! door a client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    page_size, DEFAULT_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::base_cluster;
use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, router, string,
};

/// `MaxResults` is modelled `range 0..100` on every list operation of this service that
/// takes one (`smithy_query(query="operation", operation="ListServiceVersions")`,
/// `smithy_query(query="operation", operation="ListJobs")`,
/// `smithy_query(query="operation", operation="ListClusterJobs")`).
const MAX_RESULTS_MIN: i32 = 0;
const MAX_RESULTS_MAX: i32 = 100;

/// The on-device service whose version catalog the `ListServiceVersions` row walks. Its
/// catalog is the service's own, so the row needs no setup
/// (`shared_types::SERVICE_NAME_VALUES`).
const ON_DEVICE_SERVICE: &str = "KUBERNETES";

/// How many node jobs the cluster of the `ListClusterJobs` row holds, and how many orders
/// the `ListJobs` row stores: enough that a small page size leaves entries after the first
/// page and the published maximum does not.
const SEEDED_CLUSTER_NODES: i64 = 4;
const SEEDED_JOBS: usize = 5;

/// The page sizes every listing is walked with: absent, each published bound of the
/// modelled range, and sizes that do and do not divide the listings above.
fn rows() -> Vec<(String, Option<i32>)> {
    let mut rows: Vec<(String, Option<i32>)> = vec![
        ("no MaxResults".to_string(), None),
        (
            "MaxResults at the published minimum".to_string(),
            Some(MAX_RESULTS_MIN),
        ),
    ];
    for size in [1i32, 2, 3, 4] {
        rows.push((format!("MaxResults {size}"), Some(size)));
    }
    rows.push((
        "MaxResults at the published maximum".to_string(),
        Some(MAX_RESULTS_MAX),
    ));
    rows
}

/// One list call: `base` with `max_results` and `next_token` merged over it, returning the
/// entries of the page and the `NextToken` it carried, if any.
async fn one_page(
    router: &MultiServiceRouter,
    operation: &str,
    base: &Value,
    entries_member: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let mut body = base.clone();
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    let listed = ok(router, operation, body, case).await;
    let entries = listed
        .get(entries_member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let token = listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (entries, token)
}

/// How many entries `operation`'s listing holds, counted by walking it one entry per page
/// — a page size the rule under test is not asked for, so the total is established
/// independently of how an undeclared or zero `MaxResults` is read.
async fn total_entries(
    router: &MultiServiceRouter,
    operation: &str,
    base: &Value,
    entries_member: &str,
    case: &str,
) -> usize {
    let mut total = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let (entries, token) = one_page(
            router,
            operation,
            base,
            entries_member,
            Some(1),
            next_token.as_deref(),
            case,
        )
        .await;
        total += entries.len();
        assert!(
            total <= usize::try_from(MAX_RESULTS_MAX).expect("a positive bound"),
            "case {case:?}: a listing this suite seeds is smaller than the published \
             maximum page, got {total} entries"
        );
        next_token = token;
        if next_token.is_none() {
            break total;
        }
    }
}

/// Assert `operation` puts `state::page_size(MaxResults)` entries on its first page,
/// bounded by the entries the listing holds, for every row — and that it carries a
/// continuation token exactly when entries remain.
async fn assert_pages_by_the_shared_rule(
    router: &MultiServiceRouter,
    operation: &str,
    base: Value,
    entries_member: &str,
) {
    let total = total_entries(router, operation, &base, entries_member, operation).await;
    assert!(
        total > 1,
        "case {operation:?}: this suite needs a listing of more than one entry to tell the \
         page sizes apart, got {total}"
    );

    for (row, max_results) in rows() {
        let case = format!("{operation}, {row}");
        let expected = page_size(max_results).min(total);
        let (entries, token) = one_page(
            router,
            operation,
            &base,
            entries_member,
            max_results,
            None,
            &case,
        )
        .await;
        assert_eq!(
            entries.len(),
            expected,
            "case {case:?}: expected state::page_size({max_results:?}) entries bounded by \
             the {total} the listing holds, got {}",
            entries.len()
        );
        assert_eq!(
            token.is_some(),
            expected < total,
            "case {case:?}: expected a NextToken exactly when entries remain after a page \
             of {expected} of {total}, got {token:?}"
        );
    }
}

#[test]
fn the_shared_rule_reads_a_declared_page_size_and_reads_zero_as_unspecified() {
    // The reading the three operations above are pinned to, stated concretely so it
    // cannot drift silently: a declared size inside the modelled range is the page, and a
    // request that declares none — or declares the 0 the range admits, which would never
    // advance a walk — gets the largest page the member admits.
    let rows: Vec<(String, Option<i32>, usize)> = vec![
        ("no MaxResults".to_string(), None, DEFAULT_MAX_RESULTS),
        (
            "MaxResults at the published minimum".to_string(),
            Some(MAX_RESULTS_MIN),
            DEFAULT_MAX_RESULTS,
        ),
        ("MaxResults 1".to_string(), Some(1), 1),
        ("MaxResults 7".to_string(), Some(7), 7),
        (
            "MaxResults at the published maximum".to_string(),
            Some(MAX_RESULTS_MAX),
            usize::try_from(MAX_RESULTS_MAX).expect("a positive bound"),
        ),
    ];
    for (case, max_results, expected) in rows {
        assert_eq!(
            page_size(max_results),
            expected,
            "case {case:?}: expected a page of {expected}, got {}",
            page_size(max_results)
        );
    }
}

#[tokio::test]
async fn list_service_versions_pages_by_the_shared_page_size_rule() {
    let router = router();
    assert_pages_by_the_shared_rule(
        &router,
        "ListServiceVersions",
        json!({ "ServiceName": ON_DEVICE_SERVICE }),
        "ServiceVersions",
    )
    .await;
}

#[tokio::test]
async fn list_jobs_pages_by_the_shared_page_size_rule() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Page Size Receiver", "9 Page Way", json!({})),
        "the address the seeded orders ship to",
    )
    .await;
    for index in 0..SEEDED_JOBS {
        let case = format!("seeded order {index}");
        // An IMPORT order stores one job record, so the listing holds exactly the orders
        // this loop created.
        let created = ok(
            &router,
            "CreateJob",
            complete_order(&json!({
                "AddressId": address_id,
                "Description": format!("page size order {index}"),
            })),
            &case,
        )
        .await;
        string(&created, "JobId", &case);
    }

    assert_pages_by_the_shared_rule(&router, "ListJobs", json!({}), "JobListEntries").await;
}

#[tokio::test]
async fn list_cluster_jobs_pages_by_the_shared_page_size_rule() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Page Size Cluster", "10 Page Way", json!({})),
        "the address the seeded cluster ships to",
    )
    .await;
    let case = "the seeded cluster";
    let mut body = base_cluster(&address_id);
    body["InitialClusterSize"] = json!(SEEDED_CLUSTER_NODES);
    let created = ok(&router, "CreateCluster", body, case).await;
    let cluster_id = string(&created, "ClusterId", case);

    assert_pages_by_the_shared_rule(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": cluster_id }),
        "JobListEntries",
    )
    .await;
}
