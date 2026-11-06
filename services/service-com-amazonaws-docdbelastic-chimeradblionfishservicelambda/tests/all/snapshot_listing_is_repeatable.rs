// SPDX-License-Identifier: CC0-1.0

//! `ListClusterSnapshots` reports, it does not consume.
//!
//! The rule these suites assert: the operation is a reporting read, so calling it —
//! with any filter combination, any page size, and any cursor it issued itself — hands
//! every later caller exactly the state the first call saw. Repeating one request
//! returns an identical page and an identical cursor, a cursor stays usable however
//! often it is replayed, and no other reader of the same resources (the single-snapshot
//! reader, the cluster listing) answers differently because a listing ran.
//!
//! Source: `smithy_query(query="operation", operation="ListClusterSnapshots")` reports
//! `readonly: true` and `paginated(inputToken: nextToken, outputToken: nextToken,
//! pageSize: maxResults, items: snapshots)`.
//!
//! The rule generalises over which filters a request combines, over the page size and
//! over how far into the walk a cursor points, so the tables below span the filter
//! combinations, the modelled page-size bounds and every page of a multi-page walk.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The inclusive bounds `ListClusterSnapshots$maxResults` is modelled with.
///
/// Source: `smithy_query(query="operation", operation="ListClusterSnapshots")` —
/// `maxResults` carries `range(min: 20, max: 100)`.
const MAX_RESULTS_RANGE: (i32, i32) = (20, 100);

/// The terminal status a settled snapshot reports.
///
/// Source: `smithy_query(query="shape", shape="Status")` — `ACTIVE` is the value this
/// service's snapshots settle on once the clock-owed creation transition is due.
const SETTLED_STATUS: &str = "ACTIVE";

/// A SigV4 `Authorization` header for this service in [`REGION`].
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: the shared engine plus a router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one `@httpLabel` or query value, the way an AWS SDK does.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when the
/// body is empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Create a cluster that must succeed and return its ARN.
async fn create_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, response) = call(
        router,
        "POST",
        "/cluster",
        Some(json!({
            "clusterName": name,
            "authType": "PLAIN_TEXT",
            "adminUserName": "admin",
            "adminUserPassword": "Secret123!",
            "shardCapacity": 2,
            "shardCount": 1,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture cluster must be created, got {status} with {response}"
    );
    response["cluster"]["clusterArn"]
        .as_str()
        .expect("the created cluster carries its ARN")
        .to_string()
}

/// Snapshot `cluster_arn` under `name` and return the new snapshot's ARN.
async fn create_snapshot(router: &MultiServiceRouter, cluster_arn: &str, name: &str) -> String {
    let (status, response) = call(
        router,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture snapshot must be created, got {status} with {response}"
    );
    response["snapshot"]["snapshotArn"]
        .as_str()
        .expect("the created snapshot carries its ARN")
        .to_string()
}

/// `GET /cluster-snapshot/{snapshotArn}`, which must answer 200, returning the parsed
/// body.
async fn read_snapshot(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, response) =
        call(router, "GET", &format!("/cluster-snapshot/{}", label(arn)), None).await;
    assert_eq!(
        status, 200,
        "case {arn:?}: the stored snapshot must be readable, got {status} with {response}"
    );
    response
}

/// `GET /cluster-snapshots?<query>`, which must answer 200, returning the parsed body.
async fn list(router: &MultiServiceRouter, case: &str, query: &str) -> Value {
    let path = if query.is_empty() {
        "/cluster-snapshots".to_string()
    } else {
        format!("/cluster-snapshots?{query}")
    };
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a listing answers 200, got {status} with {response}"
    );
    response
}

/// How long [`await_settled`] polls for the clock-owed creation transition, and how
/// long it waits between polls.
///
/// The crate keeps the span itself private (`crate::state`'s settle span, well under a
/// second), so the budget here is a generous multiple of it: a span that ever grows
/// past it fails with the message below instead of hanging the suite.
const SETTLE_POLL_BUDGET: Duration = Duration::from_secs(5);
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Wait until every stored snapshot has settled on its terminal status, so a later
/// difference between two pages cannot be the clock-owed creation transition.
///
/// The emulator never sleeps for that transition: it materialises it on the next read,
/// which is what this poll drives.
async fn await_settled(router: &MultiServiceRouter, expected: usize) {
    let deadline = std::time::Instant::now() + SETTLE_POLL_BUDGET;
    let mut settled = 0usize;
    while std::time::Instant::now() < deadline {
        let page = list(router, "settling", &format!("maxResults={}", MAX_RESULTS_RANGE.1)).await;
        let items = page["snapshots"].as_array().cloned().unwrap_or_default();
        settled = items
            .iter()
            .filter(|item| item["status"].as_str() == Some(SETTLED_STATUS))
            .count();
        if settled == expected && page.get("nextToken").is_none_or(Value::is_null) {
            return;
        }
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
    panic!(
        "case settling: of {expected} snapshots only {settled} reached {SETTLED_STATUS} \
         within {SETTLE_POLL_BUDGET:?}"
    );
}

/// Two clusters whose snapshots interleave in `snapshotArn` order, plus more snapshots
/// than the smallest page the model allows, so a walk really crosses a page boundary.
///
/// Returns the router, the two cluster ARNs and every snapshot ARN.
async fn fixture(count: i32) -> (MultiServiceRouter, String, String, Vec<String>) {
    let router = emulator();
    let first = create_cluster(&router, "golf").await;
    let second = create_cluster(&router, "hotel").await;
    let mut arns = Vec::new();
    for index in 0..count {
        arns.push(create_snapshot(&router, &first, &format!("golf-{index}")).await);
        arns.push(create_snapshot(&router, &second, &format!("hotel-{index}")).await);
    }
    arns.sort();
    await_settled(&router, arns.len()).await;
    (router, first, second, arns)
}

#[tokio::test]
async fn list_cluster_snapshots_repeats_every_request_identically() {
    let (router, first, second, arns) = fixture(MAX_RESULTS_RANGE.0 + 3).await;

    // What the other readers of the same resources say before any listing ran. The
    // cluster listing is checked for a real answer first, so comparing it afterwards
    // cannot pass by comparing two refusals.
    let before_clusters = call(&router, "GET", "/clusters", None).await;
    assert_eq!(
        before_clusters.0, 200,
        "case clusters: the cluster listing must answer 200, got {} with {}",
        before_clusters.0, before_clusters.1
    );
    assert!(
        before_clusters.1["clusters"]
            .as_array()
            .is_some_and(|clusters| clusters.len() == 2),
        "case clusters: the witness listing must report both fixture clusters, got {}",
        before_clusters.1
    );
    let mut before_reads = Vec::new();
    for arn in &arns {
        before_reads.push((arn.clone(), read_snapshot(&router, arn).await));
    }

    let total = arns.len();
    let per_cluster = total / 2;
    let smallest_page = MAX_RESULTS_RANGE.0 as usize;
    // One row per request shape the rule generalises over, each paired with how many
    // snapshots its first page must report: no filter, each cluster, a cluster no
    // snapshot belongs to, each documented type spelling, both filters at once, and each
    // modelled page-size bound. The counts are what makes a repeated page meaningful: a
    // row that reports nothing and a row that reports the whole selection are both
    // asserted, so an "identical page" check cannot pass on two empty pages by accident.
    let rows: Vec<(&str, String, usize)> = vec![
        ("no filter", String::new(), total),
        (
            "clusterArn of the first cluster",
            format!("clusterArn={}", label(&first)),
            per_cluster,
        ),
        (
            "clusterArn of the second cluster",
            format!("clusterArn={}", label(&second)),
            per_cluster,
        ),
        (
            "clusterArn of a cluster this store does not hold",
            format!(
                "clusterArn={}",
                label(&format!(
                    "arn:aws:docdb-elastic:{REGION}:000000000000:cluster/\
                     00000000-0000-0000-0000-000000000000"
                ))
            ),
            0,
        ),
        (
            "snapshotType=MANUAL",
            "snapshotType=MANUAL".to_string(),
            total,
        ),
        (
            // The fixture takes every snapshot through `CreateClusterSnapshot`, which
            // mints MANUAL ones, so the type AWS reserves for its own backup window
            // selects nothing here.
            "snapshotType=automated",
            "snapshotType=automated".to_string(),
            0,
        ),
        (
            "both filters at once",
            format!("clusterArn={}&snapshotType=MANUAL", label(&second)),
            per_cluster,
        ),
        (
            "the modelled minimum page size",
            format!("maxResults={}", MAX_RESULTS_RANGE.0),
            smallest_page.min(total),
        ),
        (
            "the modelled maximum page size",
            format!("maxResults={}", MAX_RESULTS_RANGE.1),
            total,
        ),
        (
            "a filter and the minimum page size together",
            format!(
                "clusterArn={}&maxResults={}",
                label(&first),
                MAX_RESULTS_RANGE.0
            ),
            smallest_page.min(per_cluster),
        ),
    ];

    for (case, query, expected_count) in &rows {
        let once = list(&router, case, query).await;
        let twice = list(&router, case, query).await;
        assert_eq!(
            once, twice,
            "case {case:?}: a repeated listing must report the same page, got {once} then \
             {twice}"
        );
        let reported = once["snapshots"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: the response must carry `snapshots`, got {once}"))
            .len();
        assert_eq!(
            reported, *expected_count,
            "case {case:?}: expected {expected_count} snapshots on the first page, got \
             {reported} in {once}"
        );
        for item in once["snapshots"].as_array().expect("snapshots") {
            assert_eq!(
                item["status"].as_str(),
                Some(SETTLED_STATUS),
                "case {case:?}: a settled snapshot keeps its terminal status, got {item}"
            );
        }
    }

    // No other reader of the same resources answers differently because the listings
    // above ran.
    let after_clusters = call(&router, "GET", "/clusters", None).await;
    assert_eq!(
        before_clusters, after_clusters,
        "case clusters: the cluster listing must be unchanged by a snapshot listing, got \
         {before_clusters:?} then {after_clusters:?}"
    );
    for (arn, before) in &before_reads {
        let after = read_snapshot(&router, arn).await;
        assert_eq!(
            *before, after,
            "case {arn:?}: the single-snapshot reader must be unchanged by a listing, got \
             {before:?} then {after:?}"
        );
    }
}

#[tokio::test]
async fn list_cluster_snapshots_cursors_stay_usable_when_replayed() {
    // More snapshots per cluster than the smallest page the model allows, so both the
    // unfiltered and the filtered walk cross a page boundary.
    let (router, first, _second, arns) = fixture(MAX_RESULTS_RANGE.0 + 3).await;
    let page_size = MAX_RESULTS_RANGE.0;

    // Each row is a filter the walk runs under, paired with the ARNs it selects: the
    // cursor's meaning must not depend on whether a filter narrows the walk.
    let mut first_cluster: Vec<String> = Vec::new();
    for arn in &arns {
        let read = read_snapshot(&router, arn).await;
        if read["snapshot"]["clusterArn"].as_str() == Some(first.as_str()) {
            first_cluster.push(arn.clone());
        }
    }
    first_cluster.sort();
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("unfiltered walk", String::new(), arns.clone()),
        (
            "walk narrowed to one cluster",
            format!("clusterArn={}", label(&first)),
            first_cluster.clone(),
        ),
    ];

    for (case, filter, expected) in rows {
        let mut token: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0usize;
        loop {
            let mut query = vec![format!("maxResults={page_size}")];
            if !filter.is_empty() {
                query.push(filter.clone());
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let query = query.join("&");

            // The same page, asked for three times: a cursor is not consumed by being
            // used, and the page it names does not drift.
            let once = list(&router, case, &query).await;
            let twice = list(&router, case, &query).await;
            let thrice = list(&router, case, &query).await;
            assert_eq!(
                once, twice,
                "case {case:?} page {pages}: a replayed cursor must report the same page, \
                 got {once} then {twice}"
            );
            assert_eq!(
                twice, thrice,
                "case {case:?} page {pages}: a cursor must stay usable, got {twice} then \
                 {thrice}"
            );

            for item in once["snapshots"].as_array().expect("snapshots") {
                seen.push(
                    item["snapshotArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            match once.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                pages <= 2 * arns.len() + 2,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }
        assert!(
            pages > 1,
            "case {case:?}: the walk must cross a page boundary, walked {pages}"
        );
        let walked = seen.clone();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case:?}: replaying every cursor must still report the selection exactly \
             once each, got {walked:?}"
        );
    }
}
