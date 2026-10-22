// SPDX-License-Identifier: CC0-1.0

//! `ListClusters`, end to end over HTTP.
//!
//! The rule under test: an account-wide list operation returns exactly the resources
//! this account has stored, projected the same way a describe projects them, in a stable
//! creation order, cut into pages of at most `MaxResults` items whose `NextToken`
//! resumes after the last item returned.
//!
//! The tables span what the rule generalises over: the number of stored clusters (none,
//! one, several), the page size (one at a time, a size that does not divide the total,
//! the exact total, one past it, each published `MaxResults` bound and one step outside
//! each), and the cursor (absent, round-tripped, and naming nothing stored).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this target shares.
use super::common::{call, create_cluster, enc, error_code, router};

/// One `ListClusters` page: `(status, NextToken, the page's `Cluster` bodies)`.
async fn list_page(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Option<String>, Vec<Value>) {
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("MaxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("NextToken={}", enc(token)));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let (status, body) = call(router, "GET", &format!("/cluster{suffix}"), None).await;
    let clusters = body["Clusters"].as_array().cloned().unwrap_or_default();
    (
        status,
        body["NextToken"].as_str().map(ToString::to_string),
        clusters,
    )
}

/// Every cluster ARN reachable by paging from the start with `max_results` per page.
async fn page_through(router: &MultiServiceRouter, max_results: Option<i32>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, next, clusters) = list_page(router, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results:?}: expected 200, got {status}"
        );
        if let Some(max) = max_results {
            assert!(
                clusters.len() <= max as usize,
                "case MaxResults={max_results:?}: a page holds at most {max} items, got {}",
                clusters.len()
            );
        }
        seen.extend(
            clusters
                .iter()
                .map(|c| c["ClusterArn"].as_str().unwrap_or_default().to_string()),
        );
        pages += 1;
        assert!(
            pages <= seen.len() + 2,
            "case MaxResults={max_results:?}: paging did not terminate after {pages} pages"
        );
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
}

/// The list reports exactly the clusters stored, at every count from none upwards, and
/// each entry carries the same body `DescribeCluster` answers with.
#[tokio::test]
async fn the_list_reports_exactly_the_stored_clusters() {
    let router = router();

    // Row 0 of the table: an account that has created nothing lists nothing — and says
    // so with an empty array and no cursor, not with an error.
    let (status, next, clusters) = list_page(&router, None, None).await;
    assert_eq!(status, 200, "case no clusters stored: expected 200, got {status}");
    assert!(
        clusters.is_empty() && next.is_none(),
        "case no clusters stored: expected an empty page and no NextToken, got {clusters:?} / {next:?}"
    );

    // Rows differ along the number of stored clusters: the list must agree with the
    // store after each create.
    let mut expected: Vec<String> = Vec::new();
    for n in 0..4 {
        let created = create_cluster(&router, json!({ "ClusterName": format!("listed-{n}") })).await;
        let arn = created["ClusterArn"].as_str().unwrap_or_default().to_string();
        expected.push(arn.clone());

        let (_, _, clusters) = list_page(&router, None, None).await;
        let listed: Vec<String> = clusters
            .iter()
            .map(|c| c["ClusterArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {} cluster(s) stored: the list must report each one once, in creation order, got {listed:?}",
            n + 1
        );

        // The listed projection is the stored record, not a body recomputed per
        // operation: it matches what the describe of the same ARN answers.
        let (status, described) =
            call(&router, "GET", &format!("/cluster/{}", enc(&arn)), None).await;
        assert_eq!(
            status, 200,
            "case describe {arn}: expected 200, got {status} {described:?}"
        );
        let listed_entry = clusters
            .iter()
            .find(|c| c["ClusterArn"].as_str() == Some(arn.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        for field in ["ClusterArn", "Name", "Owner", "NetworkType", "ClusterEndpoints"] {
            assert_eq!(
                listed_entry[field], described["Cluster"][field],
                "case {field} of {arn}: the listed projection must equal the described one, got {:?} vs {:?}",
                listed_entry[field], described["Cluster"][field]
            );
        }
    }

    // A deleted cluster leaves the list, so the list is a read of the store and not a
    // record of what was ever created.
    let gone = expected.remove(1);
    let (status, body) = call(&router, "DELETE", &format!("/cluster/{}", enc(&gone)), None).await;
    assert_eq!(
        status, 200,
        "case delete {gone}: expected 200, got {status} {body:?}"
    );
    let (_, _, clusters) = list_page(&router, None, None).await;
    let listed: Vec<String> = clusters
        .iter()
        .map(|c| c["ClusterArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed, expected,
        "case one cluster deleted: it must no longer be listed, got {listed:?}"
    );
}

/// Paging yields every stored cluster exactly once, in a stable order, for every page
/// size from one to past the item count — and both published `MaxResults` bounds serve.
#[tokio::test]
async fn paging_yields_every_stored_cluster_exactly_once() {
    let router = router();

    const TOTAL: usize = 5;
    let mut expected: Vec<String> = Vec::new();
    for n in 0..TOTAL {
        let created = create_cluster(&router, json!({ "ClusterName": format!("paged-{n}") })).await;
        expected.push(
            created["ClusterArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // Page sizes spanning the class: one at a time, a size that does not divide the
    // total, the exact total, one past it, each published bound of the modelled
    // `MaxResults` range 1..1000 (`smithy_query(query="operation",
    // operation="ListClusters")`), and the request that names no MaxResults.
    let cases: &[Option<i32>] = &[Some(1), Some(2), Some(5), Some(6), Some(1000), None];
    for max_results in cases {
        let seen = page_through(&router, *max_results).await;
        assert_eq!(
            seen, expected,
            "case MaxResults={max_results:?}: paging must yield every stored cluster once, \
             in creation order, got {seen:?}"
        );
    }

    // The cursor resumes AFTER the items already returned: the first page's NextToken
    // is the ARN of the next item, and the rest of the list follows it.
    let (status, next, first) = list_page(&router, Some(2), None).await;
    assert_eq!(status, 200, "case first page of two: expected 200, got {status}");
    assert_eq!(
        next.as_deref(),
        Some(expected[2].as_str()),
        "case first page of two: NextToken must name the next item, got {next:?}"
    );
    assert_eq!(
        first.len(),
        2,
        "case first page of two: expected two items, got {}",
        first.len()
    );
}

/// A cursor or a page size outside what the service can serve is refused, with the
/// modelled client error and nothing removed from the store.
#[tokio::test]
async fn an_unservable_cursor_or_page_size_is_refused() {
    let router = router();

    for n in 0..2 {
        create_cluster(&router, json!({ "ClusterName": format!("refusing-{n}") })).await;
    }

    // Rows: a cursor naming no stored cluster, a syntactically valid but foreign ARN,
    // and `MaxResults` one step outside each published bound of range 1..1000. The
    // out-of-range rows are refused by the modelled constraint on
    // `ListClustersRequest$MaxResults` (`range 1..1000`, carried by
    // `LIST_CLUSTERS_INPUT_SCHEMA` and enforced in the request walk before `handle`),
    // so they pin that schema constraint, not handler code.
    let unknown = "arn:aws:route53-recovery-control::000000000000:cluster/00000000-0000-0000-0000-000000000000";
    let cases: Vec<(&str, String)> = vec![
        ("NextToken naming no stored cluster", format!("?NextToken={}", enc(unknown))),
        ("NextToken that is not an ARN at all", "?NextToken=not-a-cursor".to_string()),
        ("MaxResults one below the range minimum", "?MaxResults=0".to_string()),
        ("MaxResults one above the range maximum", "?MaxResults=1001".to_string()),
    ];
    for (case, query) in cases {
        let (status, body) = call(&router, "GET", &format!("/cluster{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body:?}"
        );
    }

    // The refusals changed nothing: the store still serves both clusters.
    let (_, _, clusters) = list_page(&router, None, None).await;
    assert_eq!(
        clusters.len(),
        2,
        "case after four refused requests: both clusters are still listed, got {}",
        clusters.len()
    );
}
