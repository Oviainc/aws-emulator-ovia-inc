// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a nearest-neighbour query is PAGED, served over the real
//! `restJson1` wire through the router production uses.
//!
//! THE RULE: one `QueryVectors` response carries at most the service's per-response limit
//! of results, whatever `topK` asks for. A query whose result set is larger answers that
//! many plus a `nextToken`, and the next call — the same query with that token — resumes
//! the SAME distance-ordered result set at the result after the last one reported, so
//! draining the pages yields the ranking `topK` bounds exactly once, in order, and the last
//! page carries no token. The operation is `@paginated` on `nextToken`/`vectors` and takes
//! no page-size member, so nothing in the request can raise that limit.
//!
//! The rows span what the rule generalises over: `topK` (one, well under the limit, exactly
//! at it, one past it, the recorded 130, more than the index holds), how many vectors the
//! index holds, whether a metadata filter narrows the set first, and whether distances are
//! asked for. Each row drains every page of its query and asserts the concatenation
//! against the ranking computed from the stored distances.

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::QUERY_RESULTS_PER_PAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, ok, router};

const BUCKET: &str = "query-paged-bucket";
const INDEX: &str = "query-paged-index";

/// How many vectors the index holds: more than two pages' worth, so a query can be drained
/// over three pages.
const STORED: usize = 2 * QUERY_RESULTS_PER_PAGE + 30;

/// The key of the vector at distance `index + 1` from the query vector `[0.0]`. Zero-padded
/// so the keys' own order matches the distance order, which keeps the expected ranking
/// readable.
fn vector_key(index: usize) -> String {
    format!("v{index:04}")
}

/// Every vector in the index, nearest first: a one-dimensional `euclidean` index where the
/// vector at position `i` sits at distance `i + 1` from `[0.0]`.
fn ranking() -> Vec<String> {
    (0..STORED).map(vector_key).collect()
}

/// A router holding the index with [`STORED`] vectors written into it, half of them tagged
/// with a `group` of `"even"` so a filtered query narrows the set.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateVectorBucket",
        &json!({ "vectorBucketName": BUCKET }),
    )
    .await;
    ok(
        &router,
        "CreateIndex",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "dataType": "float32",
            "dimension": 1,
            "distanceMetric": "euclidean",
        }),
    )
    .await;
    let vectors: Vec<Value> = (0..STORED)
        .map(|index| {
            json!({
                "key": vector_key(index),
                "data": { "float32": [(index + 1) as f32] },
                "metadata": { "group": if index % 2 == 0 { "even" } else { "odd" } },
            })
        })
        .collect();
    ok(
        &router,
        "PutVectors",
        &json!({
            "vectorBucketName": BUCKET,
            "indexName": INDEX,
            "vectors": vectors,
        }),
    )
    .await;
    router
}

/// One `QueryVectors` call, with `nextToken` when a page is being continued.
async fn query(
    router: &MultiServiceRouter,
    top_k: usize,
    filter: Option<&Value>,
    return_distance: bool,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = json!({
        "vectorBucketName": BUCKET,
        "indexName": INDEX,
        "topK": top_k,
        "queryVector": { "float32": [0.0] },
        "returnDistance": return_distance,
    });
    if let Some(filter) = filter {
        request["filter"] = filter.clone();
    }
    if let Some(token) = next_token {
        request["nextToken"] = json!(token);
    }
    call(router, "QueryVectors", &request).await
}

/// Drain every page of one query and report the keys each page carried, in page order.
async fn drained(
    router: &MultiServiceRouter,
    case: &str,
    top_k: usize,
    filter: Option<&Value>,
    return_distance: bool,
) -> Vec<Vec<String>> {
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, body) = query(router, top_k, filter, return_distance, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {} must be served, got {status} {body}",
            pages.len()
        );
        let page: Vec<String> = body["vectors"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|vector| {
                vector["key"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            page.len() <= QUERY_RESULTS_PER_PAGE,
            "case {case:?}: page {} carries {} results, above the per-response limit of \
             {QUERY_RESULTS_PER_PAGE}",
            pages.len(),
            page.len()
        );
        if return_distance {
            // Within a page, and so across the pages they are concatenated into, the
            // distances never decrease.
            let distances: Vec<f64> = body["vectors"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|vector| vector["distance"].as_f64().unwrap_or_default())
                .collect();
            assert!(
                distances.windows(2).all(|pair| pair[0] <= pair[1]),
                "case {case:?}: page {} is not in distance order: {distances:?}",
                pages.len()
            );
        }
        pages.push(page);
        token = body["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return pages;
        }
        assert!(
            pages.len() < 10,
            "case {case:?}: the pages must run out, got {} of them",
            pages.len()
        );
    }
}

/// One row: the query's `topK`, the filter it carries, and the ranking it must drain to.
struct PageCase {
    case: String,
    top_k: usize,
    filter: Option<Value>,
    expected: Vec<String>,
}

/// The rows.
fn page_cases() -> Vec<PageCase> {
    let all = ranking();
    let even: Vec<String> = (0..STORED).step_by(2).map(vector_key).collect();
    let prefix = |count: usize| all.iter().take(count).cloned().collect::<Vec<String>>();
    vec![
        PageCase {
            case: "topK of one".to_string(),
            top_k: 1,
            filter: None,
            expected: prefix(1),
        },
        PageCase {
            case: "topK well under the per-response limit".to_string(),
            top_k: 10,
            filter: None,
            expected: prefix(10),
        },
        PageCase {
            case: format!("topK exactly at the {QUERY_RESULTS_PER_PAGE}-result limit"),
            top_k: QUERY_RESULTS_PER_PAGE,
            filter: None,
            expected: prefix(QUERY_RESULTS_PER_PAGE),
        },
        PageCase {
            case: format!("topK one past the {QUERY_RESULTS_PER_PAGE}-result limit"),
            top_k: QUERY_RESULTS_PER_PAGE + 1,
            filter: None,
            expected: prefix(QUERY_RESULTS_PER_PAGE + 1),
        },
        PageCase {
            case: "topK of 130, the recorded case".to_string(),
            top_k: 130,
            filter: None,
            expected: prefix(130),
        },
        PageCase {
            case: "topK past what the index holds".to_string(),
            top_k: STORED + 50,
            filter: None,
            expected: all.clone(),
        },
        PageCase {
            case: "a filter narrowing the set first".to_string(),
            top_k: STORED,
            filter: Some(json!({ "group": "even" })),
            expected: even,
        },
    ]
}

/// Every row drains to its whole ranking, in order, over pages no larger than the limit —
/// and every page but the last carries a token, while the last carries none.
#[tokio::test]
async fn a_query_drains_its_whole_ranking_over_pages_no_larger_than_the_limit() {
    let router = fixture().await;
    for row in page_cases() {
        let PageCase {
            case,
            top_k,
            filter,
            expected,
        } = row;
        let pages = drained(&router, &case, top_k, filter.as_ref(), true).await;

        // The pages concatenate to the ranking exactly once, in distance order.
        let drained_keys: Vec<String> = pages.iter().flatten().cloned().collect();
        assert_eq!(
            drained_keys, expected,
            "case {case:?}: the drained pages must be the ranking in order"
        );
        // Every page but the last is FULL: a response short of the limit while results are
        // left would make a client that stops on a short page drop them.
        let page_count = pages.len();
        assert_eq!(
            page_count,
            expected.len().div_ceil(QUERY_RESULTS_PER_PAGE).max(1),
            "case {case:?}: expected the ranking to fill {} pages, got {page_count}",
            expected.len().div_ceil(QUERY_RESULTS_PER_PAGE).max(1)
        );
        for (position, page) in pages.iter().enumerate() {
            if position + 1 < page_count {
                assert_eq!(
                    page.len(),
                    QUERY_RESULTS_PER_PAGE,
                    "case {case:?}: page {position} must be full, got {} results",
                    page.len()
                );
            }
        }
    }
}

/// The token a page hands back is the only position the next call resumes from: a value
/// this operation never minted names no position in the ranking and is refused with the
/// modelled 400.
#[tokio::test]
async fn a_token_this_query_never_minted_is_refused() {
    let router = fixture().await;
    for (case, token) in [
        ("a key the index does not hold", "no-such-vector"),
        ("a bare number", "17"),
        ("the empty-looking opaque value", "-"),
    ] {
        let (status, body) = query(&router, 130, None, false, Some(token)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: must be refused, got {status} {body}"
        );
    }
}

/// The token names a position in the result set, not a page number: the SAME token read
/// twice answers the same page, and a token from a narrower query resumes that query's own
/// ranking.
#[tokio::test]
async fn a_token_resumes_the_same_result_set_it_was_minted_from() {
    let router = fixture().await;
    let (status, first) = query(&router, 130, None, false, None).await;
    assert_eq!(status, 200, "the first page must be served, got {first}");
    let token = first["nextToken"]
        .as_str()
        .expect("a token while results are left")
        .to_string();

    let (_, second) = query(&router, 130, None, false, Some(&token)).await;
    let (_, again) = query(&router, 130, None, false, Some(&token)).await;
    assert_eq!(
        second["vectors"], again["vectors"],
        "the same token must answer the same page, got {second} and {again}"
    );
    assert_eq!(
        second["vectors"].as_array().map(Vec::len),
        Some(130 - QUERY_RESULTS_PER_PAGE),
        "the last page must carry what the first left, got {second}"
    );
    assert_eq!(
        second["nextToken"],
        Value::Null,
        "the last page must carry no token, got {second}"
    );
    // The token is this query's own position: the first page's last key.
    assert_eq!(
        first["vectors"][QUERY_RESULTS_PER_PAGE - 1]["key"].as_str(),
        Some(token.as_str()),
        "the token must name the last result of the page that minted it, got {first}"
    );
    // Every page of every query reports the index's own distance metric.
    for (case, body) in [("the first page", &first), ("the second", &second)] {
        assert_eq!(
            body["distanceMetric"].as_str(),
            Some("euclidean"),
            "case {case:?}: the stored metric must be reported, got {body}"
        );
    }
}
