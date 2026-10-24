// SPDX-License-Identifier: CC0-1.0

//! `ListClusterJobs`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `ListClusterJobs` reports one `JobListEntry` per job of the named cluster — exactly the
//! node jobs registered against it, whichever call created them, in the order they were
//! created and no job of any other cluster — paging them with `MaxResults` as the page size
//! and a `NextToken` that resumes the walk where the previous page of that same cluster
//! stopped; a `ClusterId` no stored cluster answers to is refused with
//! `InvalidResourceException` and a token this service did not mint for that cluster with
//! `InvalidNextTokenException`.
//!
//! The rows differ along what the rule generalises over: how many node jobs the cluster
//! holds (none, one, several, the quorum, the largest initial size the model admits), which
//! call created them (`CreateCluster`'s `InitialClusterSize`, a later `CreateJob` naming the
//! cluster, or both), the page size (absent, each published bound of the modelled range,
//! sizes that divide the listing and sizes that do not), and the form of the token
//! (unminted, truncated, altered, another cluster's).
//!
//! A row that needs a well-formed token naming a job of its choosing mints it with the
//! crate's own cursor codec (`state::encode_cursor`), so the suite cannot drift from the
//! encoding the handler decodes.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `string`) and the cluster helpers (`base_cluster`, `job_metadata`)
//! are the ones `job_lifecycle`, `cluster_updates` and `cluster_creation` already drive the
//! service with, so every call here goes through the same front door a client uses and
//! nothing reaches into the state struct.

use std::collections::BTreeSet;

use serde_json::{json, Value};
// The quorum of node jobs this service holds a cluster to, read from the crate rather than
// restated, so a row cannot drift from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    encode_cursor, CLUSTER_NODE_QUORUM,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, job_metadata};
use super::job_lifecycle::{
    call_in_region, create_address, deliverable_address, ok, refused, router, string,
};

/// `ListClusterJobsRequest$MaxResults` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListClusterJobs")`), so these are the
/// smallest and largest page sizes the member admits.
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so this is the largest
/// number of node jobs one create can ask for.
const INITIAL_CLUSTER_SIZE_MAX: i64 = 16;

/// The region every request in this suite signs for — the one the rest of this crate's
/// suites sign for, carried in the SigV4 credential scope.
const SIGNED_REGION: &str = "us-west-2";

/// Cluster ids in the modelled `ClusterId` form —
/// `^CID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="shape", shape="ClusterId")`) — that name no stored cluster.
const UNKNOWN_CLUSTER_IDS: &[&str] = &[
    "CID00000000-0000-0000-0000-000000000000",
    "CIDffffffff-ffff-ffff-ffff-ffffffffffff",
    "CID12345678-9abc-def0-1234-56789abcdef0",
];

/// One listing row: a cluster to build, how many node jobs its create asks for and how
/// many a later `CreateJob` adds to it.
struct Row {
    case: String,
    initial_size: Option<i64>,
    added_nodes: usize,
}

/// Every listing row. The node-job count spans none, one, several, the quorum and the
/// largest initial size the model admits, and each count is reached by `CreateCluster`
/// alone, by `CreateJob` alone and by the two together — a cluster's jobs are its node
/// jobs whichever call made them.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // (what the create asks for, how many nodes a later CreateJob adds). A cluster takes
    // no further node once it holds its quorum, so a row's total never passes it unless
    // the create alone asked for more.
    let plan: Vec<(Option<i64>, usize)> = vec![
        (None, 0),
        (Some(0), 0),
        (None, 1),
        (Some(1), 0),
        (Some(0), CLUSTER_NODE_QUORUM),
        (Some(1), 1),
        (Some(2), 1),
        (Some(3), 0),
        (Some(4), 1),
        (Some(CLUSTER_NODE_QUORUM as i64), 0),
        (Some(INITIAL_CLUSTER_SIZE_MAX), 0),
    ];
    for (initial_size, added_nodes) in plan {
        rows.push(Row {
            case: format!(
                "a cluster created with {initial_size:?} node jobs and {added_nodes} added after"
            ),
            initial_size,
            added_nodes,
        });
    }
    rows
}

/// The page sizes every cluster's listing is walked with: absent, each published bound of
/// the modelled range, and sizes that do and do not divide the listing.
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
    for size in [1i64, 2, 3, 4, 7] {
        sizes.push((format!("MaxResults {size}"), Some(size)));
    }
    sizes
}

/// Build one row's cluster and return its id together with the job ids of its node jobs,
/// in the order the calls created them.
async fn build_cluster(
    router: &MultiServiceRouter,
    address_id: &str,
    row: &Row,
) -> (String, Vec<String>) {
    let mut body = base_cluster(address_id);
    if let Some(size) = row.initial_size {
        body["InitialClusterSize"] = json!(size);
    }
    let created = ok(router, "CreateCluster", body, &row.case).await;
    let cluster_id = string(&created, "ClusterId", &row.case);

    // The create reports the node jobs it made, in the order it made them.
    let mut expected: Vec<String> = created
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string(entry, "JobId", &row.case))
        .collect();
    assert_eq!(
        expected.len(),
        usize::try_from(row.initial_size.unwrap_or(0)).expect("a non-negative initial size"),
        "case {:?}: expected the create to report one entry per node job asked for, got {created}",
        row.case
    );

    // A node added later is a job of the same cluster: "you only need to provide the
    // clusterId value; the other job attributes are inherited from the cluster".
    for index in 0..row.added_nodes {
        let case = format!("{}, added node {index}", row.case);
        let node = ok(
            router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            &case,
        )
        .await;
        expected.push(string(&node, "JobId", &case));
    }

    (cluster_id, expected)
}

/// The entries of one `ListClusterJobs` answer, reading an answer that reports no job as
/// an empty listing: a cluster holding no node job has no entry to report.
fn entries(listed: &Value) -> Vec<Value> {
    listed
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Walk `ListClusterJobs` for `cluster_id` with `max_results` as the page size, returning
/// the ids in the order the pages reported them.
async fn page_through(
    router: &MultiServiceRouter,
    cluster_id: &str,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({ "ClusterId": cluster_id });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = ok(router, "ListClusterJobs", body, case).await;
        let page = entries(&listed);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: expected at most {} pages for {expected_total} jobs, got {pages}",
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
        // A declared page size bounds the page, and a page with jobs after it is full:
        // anything less would make the walk report fewer jobs per page than asked for
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
                    "case {case:?}: expected a page with more jobs after it to be full, got {}",
                    page.len()
                );
            }
        }
        for entry in page {
            seen.push(string(&entry, "JobId", case));
        }
        if next_token.is_none() {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn list_cluster_jobs_pages_over_exactly_that_cluster_s_jobs_in_creation_order() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Cluster Lister", "1 Listing Way", json!({})),
        "the address every listed cluster ships to",
    )
    .await;

    // Every cluster the table builds, with the node jobs it holds in creation order.
    let mut built: Vec<(String, Vec<String>)> = Vec::new();
    for row in rows() {
        let (cluster_id, expected) = build_cluster(&router, &address_id, &row).await;
        built.push((cluster_id, expected));
    }

    let mut all_listed: BTreeSet<String> = BTreeSet::new();
    for (cluster_id, expected) in &built {
        for (size_case, max_results) in page_sizes() {
            let case = format!("{cluster_id} holding {} jobs, {size_case}", expected.len());
            let seen = page_through(
                &router,
                cluster_id,
                max_results,
                expected.len(),
                &case,
            )
            .await;
            assert_eq!(
                &seen, expected,
                "case {case:?}: expected the pages to report every job of the cluster once, \
                 in creation order, got {seen:?}"
            );
        }

        // Each entry reports the job that is stored under it, as `DescribeJob` reports it:
        // the entry and the metadata are built from one record.
        let one_page = ok(
            &router,
            "ListClusterJobs",
            json!({ "ClusterId": cluster_id, "MaxResults": MAX_RESULTS_MAX }),
            cluster_id,
        )
        .await;
        for entry in entries(&one_page) {
            let job_id = string(&entry, "JobId", cluster_id);
            let case = format!("{cluster_id} entry {job_id}");
            assert!(
                all_listed.insert(job_id.clone()),
                "case {case:?}: expected a job to belong to one cluster only, got it twice"
            );
            let metadata = job_metadata(&router, &job_id, &case).await;
            assert_eq!(
                metadata.get("ClusterId").and_then(Value::as_str),
                Some(cluster_id.as_str()),
                "case {case:?}: expected a listed job to be a job of the cluster, got {metadata}"
            );
            for member in [
                "JobState",
                "JobType",
                "SnowballType",
                "CreationDate",
                "Description",
            ] {
                assert_eq!(
                    entry.get(member),
                    metadata.get(member),
                    "case {case:?}: ListClusterJobs and DescribeJob disagree on {member}: \
                     {entry} vs {metadata}"
                );
            }
            // A cluster node is a job of its own, never an export order's main job.
            assert!(
                !entry
                    .get("IsMaster")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "case {case:?}: expected a cluster node to be no main job, got {entry}"
            );
        }
    }

    // The listings partition the stored node jobs: every job the table created is reported
    // by exactly one cluster.
    let expected_total: usize = built.iter().map(|(_, jobs)| jobs.len()).sum();
    assert_eq!(
        all_listed.len(),
        expected_total,
        "expected every created node job to be listed once, got {}",
        all_listed.len()
    );

    // A page size outside the modelled range is refused before the handler runs.
    let (cluster_id, _) = &built[0];
    for (case, max_results) in [
        ("one past the published maximum", MAX_RESULTS_MAX + 1),
        ("one below the published minimum", MAX_RESULTS_MIN - 1),
    ] {
        let (status, parsed) = call_in_region(
            &router,
            SIGNED_REGION,
            "ListClusterJobs",
            json!({ "ClusterId": cluster_id, "MaxResults": max_results }),
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
async fn list_cluster_jobs_refuses_an_id_no_cluster_answers_to() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Known Lister", "2 Listing Way", json!({})),
        "a known address",
    )
    .await;
    // A cluster that does exist, so the refusals below are measured against a store that
    // holds node jobs rather than an empty one.
    let setup = "the cluster the refused rows are measured against";
    let mut body = base_cluster(&address_id);
    body["InitialClusterSize"] = json!(2);
    let created = ok(&router, "CreateCluster", body, setup).await;
    let cluster_id = string(&created, "ClusterId", setup);

    for unknown in UNKNOWN_CLUSTER_IDS {
        for max_results in [None, Some(MAX_RESULTS_MIN), Some(1), Some(MAX_RESULTS_MAX)] {
            let case = format!("the unknown cluster {unknown} with MaxResults {max_results:?}");
            let mut request = json!({ "ClusterId": unknown });
            if let Some(max) = max_results {
                request["MaxResults"] = json!(max);
            }
            refused(
                &router,
                "ListClusterJobs",
                request,
                "InvalidResourceException",
                &case,
            )
            .await;
        }
    }

    // The cluster that does exist still lists its own jobs.
    let listed = ok(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": cluster_id }),
        setup,
    )
    .await;
    assert_eq!(
        entries(&listed).len(),
        2,
        "case {setup:?}: expected the stored cluster's two node jobs, got {listed}"
    );
}

#[tokio::test]
async fn list_cluster_jobs_refuses_a_token_it_did_not_mint_for_that_cluster() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Token Lister", "3 Listing Way", json!({})),
        "the address both clusters ship to",
    )
    .await;

    // Two clusters, each holding node jobs, so a token minted for one can be offered to
    // the other.
    let mut clusters: Vec<(String, Vec<String>)> = Vec::new();
    for (index, size) in [3i64, 2].into_iter().enumerate() {
        let row = Row {
            case: format!("cluster {index} of the token suite"),
            initial_size: Some(size),
            added_nodes: 1,
        };
        clusters.push(build_cluster(&router, &address_id, &row).await);
    }
    let (first_id, first_jobs) = clusters[0].clone();
    let (second_id, second_jobs) = clusters[1].clone();

    let first_page = ok(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": first_id, "MaxResults": 1 }),
        "the first page of the first cluster",
    )
    .await;
    let token = string(&first_page, "NextToken", "the first page of the first cluster");

    let mut altered = token.clone();
    altered.pop();
    altered.push(if token.ends_with('0') { '1' } else { '0' });
    // Each row is a token this service did not mint for the cluster it is offered to: not
    // a cursor at all, a cursor for a job no store holds, a mangled cursor, and a cursor
    // naming a job of the other cluster — including the token that other cluster's own
    // page minted.
    let other_page = ok(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": second_id, "MaxResults": 1 }),
        "the first page of the second cluster",
    )
    .await;
    let rows: Vec<(String, String, String)> = vec![
        (
            "not a cursor".to_string(),
            first_id.clone(),
            "garbage".to_string(),
        ),
        (
            "a cursor for a job that does not exist".to_string(),
            first_id.clone(),
            encode_cursor("JID00000000-0000-0000-0000-000000000000"),
        ),
        (
            "a truncated cursor".to_string(),
            first_id.clone(),
            token[..token.len() / 2].to_string(),
        ),
        ("an altered cursor".to_string(), first_id.clone(), altered),
        (
            "a cursor naming a job of another cluster".to_string(),
            first_id.clone(),
            encode_cursor(&second_jobs[0]),
        ),
        (
            "another cluster's own page token".to_string(),
            first_id.clone(),
            string(
                &other_page,
                "NextToken",
                "the first page of the second cluster",
            ),
        ),
        (
            "this cluster's page token offered to another cluster".to_string(),
            second_id.clone(),
            token.clone(),
        ),
        (
            "a cursor naming a job of the first cluster".to_string(),
            second_id.clone(),
            encode_cursor(&first_jobs[0]),
        ),
    ];
    for (case, cluster_id, next_token) in rows {
        refused(
            &router,
            "ListClusterJobs",
            json!({ "ClusterId": cluster_id, "NextToken": next_token }),
            "InvalidNextTokenException",
            &case,
        )
        .await;
    }

    // The token this service did mint for this cluster still resumes that cluster's walk.
    let second_page = ok(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": first_id, "MaxResults": 1, "NextToken": token }),
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
        string(&resumed[0], "JobId", "the resumed page"),
        first_jobs[1],
        "expected the resumed page to start at the job the first page stopped before, got \
         {second_page}"
    );
}
