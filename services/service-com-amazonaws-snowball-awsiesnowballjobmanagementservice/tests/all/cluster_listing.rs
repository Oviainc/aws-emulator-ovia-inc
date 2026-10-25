// SPDX-License-Identifier: CC0-1.0

//! `ListClusters`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `ListClusters` reports one `ClusterListEntry` per cluster this emulator holds — every
//! stored cluster whatever state it is in, in the order they were created — paging them
//! with `MaxResults` as the page size and a `NextToken` that resumes the walk where the
//! previous page stopped, and each entry reports the cluster `DescribeCluster` reports
//! under the same id; a token this service did not mint for this listing is refused with
//! `InvalidNextTokenException`.
//!
//! The rows differ along what the rule generalises over: how many clusters the store holds
//! (none, one, several — the listing is walked again after every create), the state each
//! cluster is in (awaiting its quorum, holding it, cancelled), whether the cluster carries
//! a description and its length at each published bound of the member, how many node jobs
//! the cluster was created with (none, one, the quorum, the largest initial size the model
//! admits), the page size (absent, each published bound of the modelled range, sizes that
//! divide the listing and sizes that do not), and the form of the token (unminted,
//! truncated, altered, another listing's).
//!
//! A row that needs a well-formed token naming a record of its choosing mints it with the
//! crate's own cursor codec (`state::encode_cursor`), so the suite cannot drift from the
//! encoding the handler decodes.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `string`) and the cluster helpers (`base_cluster`,
//! `cluster_metadata`) are the ones `job_lifecycle`, `cluster_updates` and
//! `cluster_job_listing` already drive the service with, so every call here goes through
//! the same front door a client uses and nothing reaches into the state struct.

use std::collections::BTreeSet;

use serde_json::{json, Value};
// The quorum of node jobs this service holds a cluster to, read from the crate rather than
// restated, so a row cannot drift from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    encode_cursor, CLUSTER_NODE_QUORUM,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, cluster_metadata};
use super::job_lifecycle::{
    call_in_region, create_address, deliverable_address, ok, refused, router, string,
};

/// `ListClustersRequest$MaxResults` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListClusters")`), so these are the
/// smallest and largest page sizes the member admits.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// `CreateClusterRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the
/// shortest and longest descriptions a cluster can carry.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so this is the largest
/// number of node jobs one create can ask for.
const INITIAL_CLUSTER_SIZE_MAX: i64 = 16;

/// The region every request in this suite signs for — the one the rest of this crate's
/// suites sign for, carried in the SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// A cluster id in the modelled `ClusterId` form —
/// `^CID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="shape", shape="ClusterId")`) — that names no stored cluster.
const UNKNOWN_CLUSTER_ID: &str = "CID00000000-0000-0000-0000-000000000000";

/// The states a cluster of this emulator can be observed in: the one it is created in, the
/// one it moves to once its node jobs have all been created, and the one `CancelCluster`
/// puts it in (`shared_types::CLUSTER_STATE_VALUES`). The table below reaches each of
/// them, so the listing is asserted over clusters in different states rather than over one
/// state.
const REACHED_CLUSTER_STATES: &[&str] = &["AwaitingQuorum", "Pending", "Cancelled"];

/// One listing row: a cluster to build, the description it carries, how many node jobs its
/// create asks for and whether it is cancelled once created.
struct Row {
    case: String,
    description: Option<String>,
    initial_size: Option<i64>,
    cancelled: bool,
}

/// Every listing row. The descriptions span absent, each published bound of the member and
/// an ordinary value; the node-job counts span none, one, the quorum and the largest
/// initial size the model admits; and the states span the one a cluster is created in, the
/// one holding its quorum moves it to, and cancellation.
fn rows() -> Vec<Row> {
    let plan: Vec<(Option<String>, Option<i64>, bool)> = vec![
        (None, None, false),
        (Some("x".repeat(DESCRIPTION_MIN_LEN)), Some(0), false),
        (Some("x".repeat(DESCRIPTION_MAX_LEN)), Some(1), false),
        (
            Some("the cluster holding its quorum".to_string()),
            Some(CLUSTER_NODE_QUORUM as i64),
            false,
        ),
        (
            Some("the cancelled cluster".to_string()),
            Some(2),
            true,
        ),
        (None, Some(INITIAL_CLUSTER_SIZE_MAX), false),
        (
            Some("the cancelled cluster that ordered no node".to_string()),
            None,
            true,
        ),
    ];
    plan.into_iter()
        .map(|(description, initial_size, cancelled)| Row {
            case: format!(
                "a cluster described by {} characters with {initial_size:?} node jobs, \
                 cancelled: {cancelled}",
                description.as_ref().map_or(0, String::len)
            ),
            description,
            initial_size,
            cancelled,
        })
        .collect()
}

/// The page sizes the listing is walked with: absent, each published bound of the modelled
/// range, and sizes that do and do not divide the listing.
fn page_sizes() -> Vec<(String, Option<i64>)> {
    let mut sizes: Vec<(String, Option<i64>)> = vec![
        ("no MaxResults".to_string(), None),
        (
            "MaxResults at the published minimum".to_string(),
            Some(MAX_RESULTS_MIN),
        ),
        (
            "MaxResults at the published maximum".to_string(),
            Some(MAX_RESULTS_MAX),
        ),
    ];
    for size in [1i64, 2, 3, 5] {
        sizes.push((format!("MaxResults {size}"), Some(size)));
    }
    sizes
}

/// Build one row's cluster and return the id the service minted for it.
async fn build_cluster(router: &MultiServiceRouter, address_id: &str, row: &Row) -> String {
    let mut body = base_cluster(address_id);
    match &row.description {
        Some(description) => body["Description"] = json!(description),
        // `base_cluster` carries a description, so a row that wants none takes it off.
        None => {
            body.as_object_mut()
                .expect("the cluster order is a JSON object")
                .remove("Description");
        }
    }
    if let Some(size) = row.initial_size {
        body["InitialClusterSize"] = json!(size);
    }
    let created = ok(router, "CreateCluster", body, &row.case).await;
    let cluster_id = string(&created, "ClusterId", &row.case);
    if row.cancelled {
        ok(
            router,
            "CancelCluster",
            json!({ "ClusterId": cluster_id }),
            &row.case,
        )
        .await;
    }
    cluster_id
}

/// The entries of one `ListClusters` answer, reading an answer that reports no cluster as
/// an empty listing: a store holding no cluster has no entry to report.
fn entries(listed: &Value) -> Vec<Value> {
    listed
        .get("ClusterListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Walk `ListClusters` with `max_results` as the page size, returning the cluster ids in
/// the order the pages reported them.
async fn page_through(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = ok(router, "ListClusters", body, case).await;
        let page = entries(&listed);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: expected at most {} pages for {expected_total} clusters, got {pages}",
            expected_total + 1
        );
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = &next_token {
            // `NextToken` is modelled `len 1..1024`, so a token this operation mints is
            // one the next request can carry.
            assert!(
                (1..=1024).contains(&token.chars().count()),
                "case {case:?}: expected a NextToken of 1..1024 characters, got {token:?}"
            );
        }
        // A declared page size bounds the page, and a page with clusters after it is full:
        // anything less would make the walk report fewer clusters per page than asked for
        // while still having more to report.
        if let Some(max) = max_results.filter(|max| *max > 0) {
            let max = usize::try_from(max).expect("a positive page size");
            assert!(
                page.len() <= max,
                "case {case:?}: expected at most {max} entries in a page, got {}",
                page.len()
            );
            if next_token.is_some() {
                assert_eq!(
                    page.len(),
                    max,
                    "case {case:?}: expected a page with clusters after it to be full, got {}",
                    page.len()
                );
            }
        }
        for entry in page {
            seen.push(string(&entry, "ClusterId", case));
        }
        if next_token.is_none() {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn list_clusters_pages_over_every_stored_cluster_in_creation_order() {
    let router = router();

    // A store holding no cluster has nothing to report and no page after this one.
    let empty_case = "a store holding no cluster";
    let empty = ok(&router, "ListClusters", json!({}), empty_case).await;
    assert!(
        entries(&empty).is_empty(),
        "case {empty_case:?}: expected no entry, got {empty}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case {empty_case:?}: expected no NextToken on the last page, got {empty}"
    );

    let address_id = create_address(
        &router,
        deliverable_address("Cluster Lister", "4 Listing Way", json!({})),
        "the address every listed cluster ships to",
    )
    .await;

    // The listing is walked again after every create, so the rule is asserted over a store
    // holding one cluster, two, and so on up to the whole table.
    let mut created: Vec<String> = Vec::new();
    for row in rows() {
        created.push(build_cluster(&router, &address_id, &row).await);
        for (size_case, max_results) in page_sizes() {
            let case = format!(
                "{} stored clusters, {size_case} ({})",
                created.len(),
                row.case
            );
            let seen = page_through(&router, max_results, created.len(), &case).await;
            assert_eq!(
                &seen, &created,
                "case {case:?}: expected the pages to report every stored cluster once, in \
                 creation order, got {seen:?}"
            );
        }
    }

    // Each entry reports the cluster that is stored under it, as `DescribeCluster` reports
    // it: the entry and the metadata are built from one record.
    let one_page = ok(
        &router,
        "ListClusters",
        json!({ "MaxResults": MAX_RESULTS_MAX }),
        "the whole listing on one page",
    )
    .await;
    let mut states: BTreeSet<String> = BTreeSet::new();
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for entry in entries(&one_page) {
        let cluster_id = string(&entry, "ClusterId", "the whole listing on one page");
        let case = format!("the entry for {cluster_id}");
        assert!(
            listed.insert(cluster_id.clone()),
            "case {case:?}: expected a cluster to be reported once, got it twice"
        );
        let metadata = cluster_metadata(&router, &cluster_id, &case).await;
        for member in ["ClusterId", "ClusterState", "CreationDate", "Description"] {
            assert_eq!(
                entry.get(member),
                metadata.get(member),
                "case {case:?}: ListClusters and DescribeCluster disagree on {member}: \
                 {entry} vs {metadata}"
            );
        }
        states.insert(string(&entry, "ClusterState", &case));
    }
    assert_eq!(
        listed.len(),
        created.len(),
        "expected every created cluster to be listed once, got {listed:?}"
    );
    for state in REACHED_CLUSTER_STATES {
        assert!(
            states.contains(*state),
            "expected the listing to report a cluster in {state}, got {states:?}"
        );
    }

    // A page size outside the modelled range is refused before the handler runs.
    for (case, max_results) in [
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
    ] {
        let (status, parsed) = call_in_region(
            &router,
            SIGNED_REGION,
            "ListClusters",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected MaxResults {max_results} to be refused, got {status} \
             with {parsed}"
        );
    }
}

#[tokio::test]
async fn list_clusters_refuses_a_token_it_did_not_mint_for_this_listing() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Token Lister", "5 Listing Way", json!({})),
        "the address every cluster of the token suite ships to",
    )
    .await;

    // Three clusters, each with a node job, so this listing has pages after its first and
    // the store also holds jobs a cursor of ANOTHER listing can name.
    let mut clusters: Vec<String> = Vec::new();
    for index in 0..3usize {
        let row = Row {
            case: format!("cluster {index} of the token suite"),
            description: Some(format!("token suite cluster {index}")),
            initial_size: Some(2),
            cancelled: false,
        };
        clusters.push(build_cluster(&router, &address_id, &row).await);
    }

    let first_case = "the first page of the cluster listing";
    let first_page = ok(
        &router,
        "ListClusters",
        json!({ "MaxResults": 1 }),
        first_case,
    )
    .await;
    let token = string(&first_page, "NextToken", first_case);

    let mut altered = token.clone();
    altered.pop();
    altered.push(if token.ends_with('0') { '1' } else { '0' });

    // A token of another listing of this service: the cursor `ListJobs` mints names a job,
    // which is no cluster this listing holds.
    let jobs_case = "the first page of the job listing";
    let job_page = ok(&router, "ListJobs", json!({ "MaxResults": 1 }), jobs_case).await;
    let job_token = string(&job_page, "NextToken", jobs_case);
    let first_job_id = string(
        &job_page["JobListEntries"][0],
        "JobId",
        jobs_case,
    );

    let rows: Vec<(String, String)> = vec![
        ("not a cursor".to_string(), "garbage".to_string()),
        (
            "a cursor for a cluster that does not exist".to_string(),
            encode_cursor(UNKNOWN_CLUSTER_ID),
        ),
        (
            "a truncated cursor".to_string(),
            token[..token.len() / 2].to_string(),
        ),
        ("an altered cursor".to_string(), altered),
        (
            "a cursor naming a job rather than a cluster".to_string(),
            encode_cursor(&first_job_id),
        ),
        ("the job listing's own page token".to_string(), job_token),
    ];
    for (case, next_token) in rows {
        refused(
            &router,
            "ListClusters",
            json!({ "NextToken": next_token }),
            "InvalidNextTokenException",
            &case,
        )
        .await;
    }

    // The token this listing did mint still resumes the walk where the first page stopped.
    let second_page = ok(
        &router,
        "ListClusters",
        json!({ "MaxResults": 1, "NextToken": token }),
        "resuming with the minted token",
    )
    .await;
    let resumed = entries(&second_page);
    assert_eq!(
        resumed.len(),
        1,
        "expected the minted token to resume the walk, got {second_page}"
    );
    assert_eq!(
        string(&resumed[0], "ClusterId", "the resumed page"),
        clusters[1],
        "expected the resumed page to start at the cluster the first page stopped before, \
         got {second_page}"
    );
}
