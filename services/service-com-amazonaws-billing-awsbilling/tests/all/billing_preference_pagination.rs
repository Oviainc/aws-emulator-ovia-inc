// SPDX-License-Identifier: CC0-1.0

//! How `GetBillingPreferences` pages, over the awsJson1_0 wire.
//!
//! Every assertion here is about ONE rule: a `nextToken` names the ENTRY the page
//! ended on inside the sequence the request selects, and it is bound to that
//! selection — so continuing a walk resumes strictly after that entry, a
//! preference written between two pages can neither make the walk skip an entry
//! nor report one twice, and a token presented by a request that selects other
//! entries is refused rather than read as a position in that other sequence. The
//! tables span what the rule generalises over: where the interleaved write lands
//! (before the boundary, after it, on an entry already reported), the page size,
//! the kind of feature (stored keys, and a history feature whose entries repeat a
//! key), and how a continuing request differs from the one that minted its token
//! (the same filter values in another order, a different page size, the filter
//! omitted, widened, narrowed, disjoint, another feature).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Every `BillingFeature` the model declares (`BILLING_FEATURE_VALUES`), each of
/// which this suite activates for the caller so the read is about paging rather
/// than about the per-feature access gate (pinned in
/// `tests/all/billing_feature_access.rs`).
const ALL_FEATURES: &[&str] = &[
    "BILLING_ALERTS",
    "CREDIT_LEVEL_SHARING",
    "CREDIT_PREFERENCE_OPTIONS",
    "CREDIT_SHARING",
    "CREDIT_SHARING_HISTORY",
    "RI_SHARING",
    "RI_SHARING_HISTORY",
];

/// A ceiling on the pages one walk below may take, so a walk that fails to make
/// progress fails the test instead of running forever.
const MAX_PAGES: usize = 64;

/// One router over ONE shared engine — what `core/router` builds in production,
/// so every call in a test observes the same state.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState>(
            );
        let mut state = state.lock();
        for feature in ALL_FEATURES {
            state.grant_feature_access(feature);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one awsJson1_0 call and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSBilling.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/billing/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// The `__type` an awsJson error body carries, without its namespace prefix.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `UpdateBillingPreferences` for one feature and one or more key/value pairs.
async fn update(router: &MultiServiceRouter, feature: &str, pairs: &[(&str, &str)]) -> (u16, Value) {
    let keys: Vec<Value> = pairs
        .iter()
        .map(|(key, value)| json!({ "key": key, "value": value }))
        .collect();
    let (status, body) = call(
        router,
        "UpdateBillingPreferences",
        &json!({ "feature": feature, "billingPreferencesPerKey": keys }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case setup: expected the update of {feature:?} to succeed, got {status} {body:?}"
    );
    (status, body)
}

/// A `GetBillingPreferences` body: one feature, a page size, and the
/// `PREFERENCE_KEY` filter values when the request carries that filter.
fn request(feature: &str, max_results: u64, keys: Option<&[&str]>) -> Value {
    let mut body = json!({ "features": [feature], "maxResults": max_results });
    if let Some(keys) = keys {
        body["filters"] = json!([{ "name": "PREFERENCE_KEY", "value": keys }]);
    }
    body
}

/// The `(key, value)` pairs a `GetBillingPreferences` body reports, in the order
/// it reported them.
fn reported(body: &Value) -> Vec<(String, String)> {
    body.get("billingPreferences")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry
                            .get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        entry
                            .get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read ONE page and answer what it reported and the token it handed back.
async fn page(
    router: &MultiServiceRouter,
    body: &Value,
    case: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let (status, reply) = call(router, "GetBillingPreferences", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the page to succeed, got {status} {reply:?}"
    );
    let token = reply
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (reported(&reply), token)
}

/// Walk `body` — which may already carry a `nextToken` — to the last page,
/// handing each page's token back in the SAME request, and answer every
/// `(key, value)` reported, in order.
async fn walk(router: &MultiServiceRouter, body: &Value, case: &str) -> Vec<(String, String)> {
    let mut body = body.clone();
    let mut walked: Vec<(String, String)> = Vec::new();
    for taken in 0..MAX_PAGES {
        let (entries, token) = page(router, &body, case).await;
        walked.extend(entries);
        match token {
            Some(token) => body["nextToken"] = json!(token),
            None => return walked,
        }
        assert!(
            taken + 1 < MAX_PAGES,
            "case {case:?}: the walk must terminate, got {MAX_PAGES} pages"
        );
    }
    walked
}

/// The keys of what a walk reported, in the order it reported them.
fn keys_of(walked: &[(String, String)]) -> Vec<String> {
    walked.iter().map(|(key, _)| key.clone()).collect()
}

/// One row of the interleaved-write table: what is stored, how big a page is,
/// what is written after the first page, and what the whole walk must report.
struct WriteBetweenPagesRow {
    case: &'static str,
    /// The keys stored before the walk starts, in request order.
    stored: &'static [&'static str],
    page_size: u64,
    /// The key/value pair written after the first page was read.
    written: (&'static str, &'static str),
    /// The keys the whole walk — first page included — must report, in order.
    expected: &'static [&'static str],
}

#[tokio::test]
async fn a_write_between_two_pages_neither_skips_nor_repeats_an_entry() {
    // The rows differ in where the interleaved write lands relative to the page
    // boundary (before it, after it, on an entry already reported), in the page
    // size, and in how many entries are still to come. A page resumes strictly
    // after the entry the previous page ended on, so every entry that sorts
    // after that boundary is reported exactly once and nothing before it is
    // reported twice.
    let rows: &[WriteBetweenPagesRow] = &[
        WriteBetweenPagesRow {
            case: "a key inserted before the page boundary",
            stored: &["key/1", "key/2", "key/3"],
            page_size: 1,
            written: ("key/0", "ENABLED"),
            expected: &["key/1", "key/2", "key/3"],
        },
        WriteBetweenPagesRow {
            case: "a key inserted after the page boundary",
            stored: &["key/1", "key/3"],
            page_size: 1,
            written: ("key/2", "ENABLED"),
            expected: &["key/1", "key/2", "key/3"],
        },
        WriteBetweenPagesRow {
            case: "the value of an entry already reported changed",
            stored: &["key/1", "key/2", "key/3"],
            page_size: 1,
            written: ("key/1", "DISABLED"),
            expected: &["key/1", "key/2", "key/3"],
        },
        WriteBetweenPagesRow {
            case: "a key inserted past the end, two entries to a page",
            stored: &["key/1", "key/2", "key/3", "key/4"],
            page_size: 2,
            written: ("key/5", "ENABLED"),
            expected: &["key/1", "key/2", "key/3", "key/4", "key/5"],
        },
        WriteBetweenPagesRow {
            case: "a key sorting before every stored one, three to a page",
            stored: &["key/2", "key/3", "key/4", "key/5", "key/6"],
            page_size: 3,
            written: ("key/1", "ENABLED"),
            expected: &["key/2", "key/3", "key/4", "key/5", "key/6"],
        },
    ];

    for row in rows {
        let router = emulator();
        let pairs: Vec<(&str, &str)> = row.stored.iter().map(|key| (*key, "ENABLED")).collect();
        update(&router, "BILLING_ALERTS", &pairs).await;

        let body = request("BILLING_ALERTS", row.page_size, None);
        let (first, token) = page(&router, &body, row.case).await;
        let token = token.unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a first page of {} of {} entries to carry a nextToken",
                row.case,
                row.page_size,
                row.stored.len()
            )
        });

        update(&router, "BILLING_ALERTS", &[row.written]).await;

        let mut resumed = body.clone();
        resumed["nextToken"] = json!(token);
        let rest = walk(&router, &resumed, row.case).await;

        let mut walked = first;
        walked.extend(rest);
        let reported_keys = keys_of(&walked);
        assert_eq!(
            reported_keys, row.expected,
            "case {:?}: expected each entry after the boundary exactly once in key order, got {reported_keys:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_token_is_usable_only_by_a_request_that_selects_the_same_entries() {
    let router = emulator();
    let stored = ["key/0", "key/1", "key/2", "key/3"];
    let pairs: Vec<(&str, &str)> = stored.iter().map(|key| (*key, "ENABLED")).collect();
    update(&router, "BILLING_ALERTS", &pairs).await;
    update(&router, "CREDIT_SHARING", &pairs).await;

    // The token is minted by a FILTERED walk of one entry per page, so its
    // cursor is a position among the three keys that filter selects.
    let minted_under: &[&str] = &["key/1", "key/2", "key/3"];
    let (first, token) = page(
        &router,
        &request("BILLING_ALERTS", 1, Some(minted_under)),
        "the filtered first page",
    )
    .await;
    assert_eq!(
        keys_of(&first),
        vec!["key/1".to_string()],
        "case \"the filtered first page\": expected the first selected key, got {first:?}"
    );
    let token = token.expect("a filtered walk of three entries by one has a second page");

    // Each row presents that token with a request that differs in one way. A
    // request that selects the SAME entries continues the walk — the page size
    // is how much of the walk a caller takes, not which entries it is over —
    // and one that selects other entries is a bad request, not a page.
    struct Row {
        case: &'static str,
        feature: &'static str,
        max_results: u64,
        keys: Option<&'static [&'static str]>,
        expected_status: u16,
        /// The keys the page must report, when it is served.
        expected_keys: &'static [&'static str],
    }
    let rows: &[Row] = &[
        Row {
            case: "the very request that minted it",
            feature: "BILLING_ALERTS",
            max_results: 1,
            keys: Some(&["key/1", "key/2", "key/3"]),
            expected_status: 200,
            expected_keys: &["key/2"],
        },
        Row {
            case: "the same filter values in another order, one repeated",
            feature: "BILLING_ALERTS",
            max_results: 1,
            keys: Some(&["key/3", "key/1", "key/2", "key/1"]),
            expected_status: 200,
            expected_keys: &["key/2"],
        },
        Row {
            case: "the same filter with a larger page size",
            feature: "BILLING_ALERTS",
            max_results: 50,
            keys: Some(&["key/1", "key/2", "key/3"]),
            expected_status: 200,
            expected_keys: &["key/2", "key/3"],
        },
        Row {
            case: "the filter omitted",
            feature: "BILLING_ALERTS",
            max_results: 1,
            keys: None,
            expected_status: 400,
            expected_keys: &[],
        },
        Row {
            case: "a widened filter",
            feature: "BILLING_ALERTS",
            max_results: 1,
            keys: Some(&["key/0", "key/1", "key/2", "key/3"]),
            expected_status: 400,
            expected_keys: &[],
        },
        Row {
            case: "a narrowed filter",
            feature: "BILLING_ALERTS",
            max_results: 1,
            keys: Some(&["key/1", "key/2"]),
            expected_status: 400,
            expected_keys: &[],
        },
        Row {
            case: "a filter selecting none of the same keys",
            feature: "BILLING_ALERTS",
            max_results: 1,
            keys: Some(&["key/9"]),
            expected_status: 400,
            expected_keys: &[],
        },
        Row {
            case: "the same filter on another feature",
            feature: "CREDIT_SHARING",
            max_results: 1,
            keys: Some(&["key/1", "key/2", "key/3"]),
            expected_status: 400,
            expected_keys: &[],
        },
    ];

    for row in rows {
        let mut body = request(row.feature, row.max_results, row.keys);
        body["nextToken"] = json!(token);
        let (status, reply) = call(&router, "GetBillingPreferences", &body).await;
        assert_eq!(
            status, row.expected_status,
            "case {:?}: expected HTTP {}, got {status} {reply:?}",
            row.case, row.expected_status
        );
        if row.expected_status == 200 {
            let reported_keys = keys_of(&reported(&reply));
            assert_eq!(
                reported_keys, row.expected_keys,
                "case {:?}: expected the walk to continue after the boundary, got {reported_keys:?}",
                row.case
            );
        } else {
            assert_eq!(
                error_code(&reply),
                "ValidationException",
                "case {:?}: expected ValidationException, got {reply:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_filtered_walk_reports_every_selected_entry_exactly_once() {
    // The rows differ in how many values the filter carries and in the page
    // size, including a page size that divides the selection and one that does
    // not: a filtered walk reports exactly the selected entries, in key order.
    let stored: Vec<String> = (0..6).map(|index| format!("key/{index}")).collect();
    let rows: Vec<(&str, Vec<&str>, u64)> = vec![
        ("one selected key, one to a page", vec!["key/3"], 1),
        (
            "three selected keys, one to a page",
            vec!["key/0", "key/2", "key/4"],
            1,
        ),
        (
            "three selected keys, two to a page",
            vec!["key/1", "key/3", "key/5"],
            2,
        ),
        (
            "five selected keys, two to a page",
            vec!["key/0", "key/1", "key/2", "key/3", "key/4"],
            2,
        ),
        (
            "every stored key and five the store has not got",
            vec![
                "key/0", "key/1", "key/2", "key/3", "key/4", "key/5", "gone/0", "gone/1", "gone/2",
                "gone/3",
            ],
            3,
        ),
    ];
    for (case, selected, page_size) in rows {
        let router = emulator();
        let pairs: Vec<(&str, &str)> = stored.iter().map(|key| (key.as_str(), "ENABLED")).collect();
        update(&router, "CREDIT_SHARING", &pairs).await;

        let walked = walk(
            &router,
            &request("CREDIT_SHARING", page_size, Some(&selected)),
            case,
        )
        .await;
        let mut expected: Vec<String> = selected
            .iter()
            .filter(|key| stored.iter().any(|held| held == *key))
            .map(|key| (*key).to_string())
            .collect();
        expected.sort();
        let reported_keys = keys_of(&walked);
        assert_eq!(
            reported_keys, expected,
            "case {case:?}: expected every selected stored key exactly once in key order, got {reported_keys:?}"
        );
    }
}

#[tokio::test]
async fn a_history_feature_pages_through_every_recorded_change_exactly_once() {
    // A history feature reports one entry per recorded CHANGE, so one key
    // appears many times and two entries can be identical — which is why the
    // cursor names the recorded change rather than the key. The rows are the
    // page sizes, spanning the modelled `maxResults` bounds and sizes that do
    // and do not divide the record.
    let changes: &[(&str, &str)] = &[
        ("account/000000000000", "ENABLED"),
        ("account/000000000000", "DISABLED"),
        ("ri-sharing", "ENABLED"),
        ("account/000000000000", "ENABLED"),
        ("ri-sharing", "DISABLED"),
    ];
    for page_size in [1_u64, 2, 4, 5, 50] {
        let router = emulator();
        for change in changes {
            update(&router, "RI_SHARING", &[*change]).await;
        }
        let case = format!("{page_size} recorded changes to a page");
        let walked = walk(
            &router,
            &request("RI_SHARING_HISTORY", page_size, None),
            &case,
        )
        .await;
        let expected: Vec<(String, String)> = changes
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every recorded change exactly once, oldest first, got {walked:?}"
        );

        // A change recorded between two pages is reported at the END of the
        // walk, where the record appended it, and no earlier change is skipped
        // or repeated.
        let body = request("RI_SHARING_HISTORY", page_size, None);
        let (first, token) = page(&router, &body, &case).await;
        if let Some(token) = token {
            update(&router, "RI_SHARING", &[("ri-sharing", "ENABLED")]).await;
            let mut resumed = body.clone();
            resumed["nextToken"] = json!(token);
            let mut walked = first;
            walked.extend(walk(&router, &resumed, &case).await);
            let mut expected = expected.clone();
            expected.push(("ri-sharing".to_string(), "ENABLED".to_string()));
            assert_eq!(
                walked, expected,
                "case {case:?}: expected the interleaved change last and every earlier one once, got {walked:?}"
            );
        }

        // The same record, narrowed to one key: the repeated entries of that
        // key are each reported once.
        let case = format!("{page_size} filtered recorded changes to a page");
        let walked = walk(
            &router,
            &request(
                "RI_SHARING_HISTORY",
                page_size,
                Some(&["account/000000000000"]),
            ),
            &case,
        )
        .await;
        let expected: Vec<(String, String)> = changes
            .iter()
            .filter(|(key, _)| *key == "account/000000000000")
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            walked, expected,
            "case {case:?}: expected every recorded change of that key exactly once, got {walked:?}"
        );
    }
}
