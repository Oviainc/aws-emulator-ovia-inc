// SPDX-License-Identifier: CC0-1.0

//! How `ListClusterSnapshots` reports the snapshots this service holds.
//!
//! The rule these suites assert: the operation pages over the stored snapshots in
//! `snapshotArn` order, reporting each as a `ClusterSnapshotInList` whose members agree
//! with what `GetClusterSnapshot` reports for the same ARN, narrowed to the snapshots
//! whose source cluster is the one `clusterArn` names and whose type is the one
//! `snapshotType` names. A filter that selects nothing is an EMPTY page, not a refusal:
//! the operation models no not-found error.
//!
//! The rules generalise over which filters a request combines, over how many snapshots
//! each cluster has and over the page size, so the tables below span the filter
//! combinations, both documented `snapshotType` spellings, a cluster no snapshot
//! belongs to, and page sizes at and past the modelled bounds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `@required` members of `ClusterSnapshotInList`.
///
/// Source: `smithy_query(query="shape", shape="ClusterSnapshotInList")` — all five
/// members are `@required`.
const SNAPSHOT_IN_LIST_MEMBERS: &[&str] = &[
    "snapshotName",
    "snapshotArn",
    "clusterArn",
    "status",
    "snapshotCreationTime",
];

/// The inclusive bounds `ListClusterSnapshots$maxResults` is modelled with.
///
/// Source: `smithy_query(query="operation", operation="ListClusterSnapshots")` —
/// `maxResults` carries `range(min: 20, max: 100)`.
const MAX_RESULTS_RANGE: (i32, i32) = (20, 100);

/// The values `snapshotType` names, as the enum spells them on the wire.
///
/// Source: `smithy_query(query="shape", shape="SnapshotType")` — `MANUAL` /
/// `AUTOMATED`; the filter member's own doc writes the same two values in lowercase
/// prose, so both spellings are sent below.
const SNAPSHOT_TYPES: &[&str] = &["AUTOMATED", "MANUAL"];

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

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A `CreateCluster` body with every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    })
}

/// Create a cluster that must succeed and return its ARN.
async fn create_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, response) = call(router, "POST", "/cluster", Some(minimal_create(name))).await;
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

/// The `snapshotArn`s of one page, in the order the page reported them, after checking
/// every `@required` member of each entry is present.
fn arns_of(case: &str, page: &Value) -> Vec<String> {
    let items = page["snapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry `snapshots`, got {page}"));
    for item in items {
        for member in SNAPSHOT_IN_LIST_MEMBERS {
            assert!(
                item.get(member)
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty()),
                "case {case:?}: ClusterSnapshotInList member `{member}` missing or empty \
                 in {item}"
            );
        }
    }
    items
        .iter()
        .map(|item| item["snapshotArn"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn list_cluster_snapshots_narrows_to_the_cluster_and_type_the_request_names() {
    let router = emulator();
    // Two clusters with a different number of snapshots each, so a per-cluster filter
    // cannot pass by returning everything.
    let first = create_cluster(&router, "alfa").await;
    let second = create_cluster(&router, "bravo").await;
    let mut first_arns = Vec::new();
    for name in ["alfa-one", "alfa-two", "alfa-three"] {
        first_arns.push(create_snapshot(&router, &first, name).await);
    }
    let mut second_arns = Vec::new();
    for name in ["bravo-one", "bravo-two"] {
        second_arns.push(create_snapshot(&router, &second, name).await);
    }
    let mut every_arn: Vec<String> = first_arns.iter().chain(second_arns.iter()).cloned().collect();
    every_arn.sort();
    first_arns.sort();
    second_arns.sort();
    let absent_cluster = format!(
        "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/00000000-0000-0000-0000-000000000000"
    );

    // One row per filter combination: none, each cluster, a cluster no snapshot belongs
    // to, an identifier that is not an ARN, each documented type spelling, and a
    // cluster and a type together.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter", String::new(), every_arn.clone()),
        (
            "clusterArn of the cluster with three snapshots",
            format!("clusterArn={}", label(&first)),
            first_arns.clone(),
        ),
        (
            "clusterArn of the cluster with two snapshots",
            format!("clusterArn={}", label(&second)),
            second_arns.clone(),
        ),
        (
            "clusterArn of a cluster this store does not hold",
            format!("clusterArn={}", label(&absent_cluster)),
            Vec::new(),
        ),
        (
            "clusterArn that is not an ARN at all",
            "clusterArn=not-an-arn-at-all".to_string(),
            Vec::new(),
        ),
        (
            "snapshotType naming the type a caller-made snapshot has",
            "snapshotType=MANUAL".to_string(),
            every_arn.clone(),
        ),
        (
            "snapshotType in the lowercase spelling the documentation uses",
            "snapshotType=manual".to_string(),
            every_arn.clone(),
        ),
        (
            // AWS reserves AUTOMATED for the snapshots it takes itself on the backup
            // window, and this service mints none, so the filter selects nothing.
            "snapshotType naming the type the service reserves for itself",
            "snapshotType=AUTOMATED".to_string(),
            Vec::new(),
        ),
        (
            "snapshotType reserved for the service, lowercase",
            "snapshotType=automated".to_string(),
            Vec::new(),
        ),
        (
            "both filters, agreeing",
            format!("clusterArn={}&snapshotType=MANUAL", label(&first)),
            first_arns.clone(),
        ),
        (
            "both filters, disagreeing",
            format!("clusterArn={}&snapshotType=AUTOMATED", label(&first)),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in rows {
        let page = list(&router, case, &query).await;
        let mut reported = arns_of(case, &page);
        assert!(
            page.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a page that listed everything carries no nextToken, got {page}"
        );
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly {expected:?}, got {reported:?}"
        );
    }

    // Every listed entry agrees with what the single-snapshot reader reports for the
    // same ARN: the listing projects the stored record, it does not invent a summary.
    let page = list(&router, "round trip", "").await;
    for item in page["snapshots"].as_array().expect("snapshots").iter() {
        let arn = item["snapshotArn"].as_str().expect("snapshotArn");
        let (status, read) = call(&router, "GET", &format!("/cluster-snapshot/{}", label(arn)), None).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the listed snapshot must be readable, got {status} with {read}"
        );
        for member in SNAPSHOT_IN_LIST_MEMBERS {
            assert_eq!(
                item.get(member),
                read["snapshot"].get(member),
                "case {arn:?}: the listing and the reader must agree on `{member}`: \
                 {item} vs {read}"
            );
        }
    }

    // A deleted snapshot leaves the listing; the rest stay.
    let gone = first_arns[0].clone();
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/cluster-snapshot/{}", label(&gone)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete: the snapshot must be deleted, got {status} with {deleted}"
    );
    let page = list(&router, "after a delete", "").await;
    let reported = arns_of("after a delete", &page);
    assert!(
        !reported.contains(&gone),
        "case after a delete: {gone:?} must no longer be listed, got {reported:?}"
    );
    assert_eq!(
        reported.len(),
        every_arn.len() - 1,
        "case after a delete: every other snapshot must still be listed, got {reported:?}"
    );
}

#[tokio::test]
async fn list_cluster_snapshots_pages_a_filtered_selection_exactly_once() {
    // A filter and paging together: the page-size check is reached before the
    // filters, so a page of a filtered walk can be shorter than `maxResults` and a
    // page can even be empty while the previous one carried a cursor. Whatever the
    // page shapes, the walk must report the filtered subset exactly once and nothing
    // outside it.
    let router = emulator();
    let wanted = create_cluster(&router, "echo").await;
    let other = create_cluster(&router, "foxtrot").await;
    let mut wanted_arns = Vec::new();
    // Interleaved, so the two clusters' snapshots are mixed in snapshotArn order and
    // the filter has to skip entries inside every page.
    for index in 0..(MAX_RESULTS_RANGE.0 + 5) {
        wanted_arns.push(create_snapshot(&router, &wanted, &format!("echo-{index}")).await);
        create_snapshot(&router, &other, &format!("foxtrot-{index}")).await;
    }
    wanted_arns.sort();

    let case = format!("clusterArn with maxResults={}", MAX_RESULTS_RANGE.0);
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut query = vec![
            format!("clusterArn={}", label(&wanted)),
            format!("maxResults={}", MAX_RESULTS_RANGE.0),
        ];
        if let Some(cursor) = &token {
            query.push(format!("nextToken={cursor}"));
        }
        let page = list(&router, &case, &query.join("&")).await;
        let reported = arns_of(&case, &page);
        assert!(
            reported.len() <= MAX_RESULTS_RANGE.0 as usize,
            "case {case:?}: a page of {} exceeds the page size, got {page}",
            reported.len()
        );
        seen.extend(reported.iter().cloned());
        pages += 1;
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            pages <= 2 * wanted_arns.len() + 2,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
    }
    assert!(
        pages > 1,
        "case {case:?}: the filtered walk must cross a page boundary, walked {pages}"
    );
    let walked = seen.clone();
    seen.sort();
    assert_eq!(
        seen, wanted_arns,
        "case {case:?}: the pages together must be exactly the filtered snapshots, once \
         each, got {walked:?}"
    );
}

#[tokio::test]
async fn list_cluster_snapshots_pages_in_arn_order_over_every_page_size() {
    let router = emulator();
    let cluster = create_cluster(&router, "charlie").await;
    // More snapshots than the smallest page the member can ask for, so the walk really
    // crosses a page boundary.
    let mut all_arns = Vec::new();
    for index in 0..(MAX_RESULTS_RANGE.0 + 5) {
        all_arns.push(create_snapshot(&router, &cluster, &format!("snap-{index}")).await);
    }
    all_arns.sort();

    // The rows differ in page size: the modelled minimum, a size between the bounds,
    // the modelled maximum, and the absent-maxResults case.
    for page_size in [None, Some(MAX_RESULTS_RANGE.0), Some(42), Some(MAX_RESULTS_RANGE.1)] {
        let case = format!("maxResults={page_size:?}");
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let page = list(&router, &case, &query.join("&")).await;
            let reported = arns_of(&case, &page);
            if let Some(size) = page_size {
                assert!(
                    reported.len() <= size as usize,
                    "case {case:?}: a page of {} exceeds the page size, got {page}",
                    reported.len()
                );
            }
            let mut ascending = reported.clone();
            ascending.sort();
            assert_eq!(
                reported, ascending,
                "case {case:?}: a page must be in snapshotArn order, got {reported:?}"
            );
            seen.extend(reported.iter().cloned());
            pages += 1;
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !reported.is_empty(),
                        "case {case:?}: a page carrying a nextToken must carry items too, \
                         got {page}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                pages <= all_arns.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }
        let walked = seen.clone();
        seen.sort();
        assert_eq!(
            seen, all_arns,
            "case {case:?}: the pages together must be exactly the stored snapshots, \
             once each, got {walked:?}"
        );
        let expected_pages = page_size
            .map(|size| all_arns.len().div_ceil(size as usize))
            .unwrap_or(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }
}

#[tokio::test]
async fn list_cluster_snapshots_refuses_a_page_size_a_token_and_a_type_outside_their_domains() {
    let router = emulator();
    let cluster = create_cluster(&router, "delta").await;
    create_snapshot(&router, &cluster, "delta-one").await;

    // `maxResults` is modelled range(min: 20, max: 100): both published bounds pass and
    // one step past each is refused.
    let (min, max) = MAX_RESULTS_RANGE;
    for (case, size, expected) in [
        ("the modelled minimum", min, 200),
        ("the modelled maximum", max, 200),
        ("one below the minimum", min - 1, 400),
        ("one above the maximum", max + 1, 400),
    ] {
        let (status, response) = call(
            &router,
            "GET",
            &format!("/cluster-snapshots?maxResults={size}"),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for maxResults={size}, got {status} \
             with {response}"
        );
    }

    // A token this service never issued is a bad request, not an empty page.
    for case in ["not-a-token", "zzzz", "0"] {
        let (status, response) = call(
            &router,
            "GET",
            &format!("/cluster-snapshots?nextToken={case}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign nextToken must be refused, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
    }

    // A `snapshotType` naming no type this service has is refused, in this service's
    // own refusal wording, with the rejected value as the request spelled it.
    for case in ["MANUEL", "ALL", "automatic", "manual-snapshots"] {
        let (status, response) = call(
            &router,
            "GET",
            &format!("/cluster-snapshots?snapshotType={case}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown snapshotType must be refused, got {status} with \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
        let expected = format!(
            "Invalid Input - '{case}'. SnapshotType must be one of {}.",
            SNAPSHOT_TYPES.join(", ")
        );
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }
}
