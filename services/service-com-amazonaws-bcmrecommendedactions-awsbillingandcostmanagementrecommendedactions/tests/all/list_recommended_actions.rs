// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendedActions`, the one operation this service
//! models, over the real awsJson1_0 wire: `POST /` with
//! `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions`.
//!
//! Every case drives the request through [`MultiServiceRouter::dispatch`] — the same
//! path the emulator's front door uses — so the assertions are about the HTTP
//! behaviour, not about the handler's internals. Records are installed through the
//! state seam (`RecommendedActionsStore::seed`), because the service models no
//! create operation: AWS computes the recommendations itself.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

use bcm::state::{ActionType, Feature, Severity, StoredAction};

/// The awsJson1_0 target this service answers.
const TARGET: &str = "AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name (`bcm-recommended-actions`, per the crate's OPERATIONS.md) — the
/// primary routing signal the router reads. The signature is not checked: request
/// authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One emulator: a shared engine, and a router with this service mounted on it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator: its store holds the inventory the service computed for
    /// this account (`RecommendedActionsStore::default`).
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The same emulator with that inventory cleared, so a case can seed exactly
    /// the records it reasons about and nothing else.
    fn empty() -> Self {
        let emulator = Self::new();
        assert!(
            emulator.clear(),
            "case \"empty emulator\": clearing the boot inventory must leave the store empty"
        );
        emulator
    }

    /// Install recommended actions through the state seam.
    fn seed(&self, actions: Vec<StoredAction>) {
        self.engine
            .state::<bcm::ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>()
            .lock()
            .recommended_actions
            .seed(actions);
    }

    /// How many records the store holds.
    fn stored_count(&self) -> usize {
        self.engine
            .state::<bcm::ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>()
            .lock()
            .recommended_actions
            .len()
    }

    /// Drop every held record (the service recomputed its recommendations down to
    /// none), reporting whether the store ended up empty.
    fn clear(&self) -> bool {
        let state = self.engine
            .state::<bcm::ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>();
        let mut guard = state.lock();
        guard.recommended_actions.clear();
        guard.recommended_actions.is_empty()
    }

    /// `ListRecommendedActions` with `body` as the request payload, reporting the
    /// response's status, announced media type and payload. `request_content_type`
    /// is what the CLIENT sent.
    async fn dispatch(
        &self,
        body: &Value,
        request_content_type: &str,
        target: &str,
    ) -> (u16, String, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", request_content_type)
            .header("x-amz-target", target)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, content_type, payload)
    }

    /// `ListRecommendedActions` with `body` as the request payload, as an
    /// awsJson1_0 client (the media type the model's protocol trait declares) sends
    /// it.
    async fn list(&self, body: &Value) -> (u16, Value) {
        let (status, _, payload) = self
            .dispatch(body, "application/x-amz-json-1.0", TARGET)
            .await;
        (status, payload)
    }
}

/// A stored record. `context`, `nextSteps` and `lastUpdatedTimeStamp` vary per
/// record so a projection that drops or crosses them is visible.
fn action(id: &str, action_type: ActionType, severity: Severity, feature: Feature) -> StoredAction {
    StoredAction {
        id: id.to_string(),
        action_type,
        account_id: engine::emulator_account_id().to_string(),
        severity,
        feature,
        context: BTreeMap::from([
            ("recordId".to_string(), id.to_string()),
            ("feature".to_string(), feature.as_str().to_string()),
        ]),
        next_steps: vec![format!("Review {} in the Billing console", feature.as_str())],
        last_updated_timestamp: format!("2026-10-21T21:{:02}:00Z", id.len()),
    }
}

/// The fixture used by most cases: six records spanning six features, three
/// severities and six action types.
fn fixture() -> Vec<StoredAction> {
    vec![
        action(
            "ra-budget-create",
            ActionType::CreateBudget,
            Severity::Warning,
            Feature::Budgets,
        ),
        action(
            "ra-budget-exceeded",
            ActionType::ReviewBudgetsExceeded,
            Severity::Critical,
            Feature::Budgets,
        ),
        action(
            "ra-payment-due",
            ActionType::PaymentsDue,
            Severity::Critical,
            Feature::Payments,
        ),
        action(
            "ra-freetier-usage",
            ActionType::ReviewFreetierUsageAlerts,
            Severity::Info,
            Feature::FreeTier,
        ),
        action(
            "ra-iam-granular",
            ActionType::MigrateToGranularPermissions,
            Severity::Info,
            Feature::Iam,
        ),
        action(
            "ra-anomaly-review",
            ActionType::ReviewAnomalies,
            Severity::Warning,
            Feature::CostAnomalyDetection,
        ),
    ]
}

/// Ids of the fixture, in insertion order.
fn fixture_ids() -> Vec<String> {
    fixture().into_iter().map(|a| a.id).collect()
}

/// The `recommendedActions` array of a successful response.
fn actions_of<'a>(case: &str, body: &'a Value) -> &'a Vec<Value> {
    match body.get("recommendedActions").and_then(Value::as_array) {
        Some(list) => list,
        None => panic!("case {case:?}: expected a recommendedActions array, got {body}"),
    }
}

/// The `id`s of a successful response, in response order.
fn ids_of(case: &str, body: &Value) -> Vec<String> {
    actions_of(case, body)
        .iter()
        .map(|entry| {
            entry
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a record carries no id: {entry}"))
                .to_string()
        })
        .collect()
}

/// The wire error code of a failure response (`__type`, bare or namespaced).
fn error_code(case: &str, body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: error body carries no __type: {body}"));
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

#[tokio::test]
async fn an_empty_store_lists_no_actions() {
    let emulator = Emulator::empty();

    let (status, body) = emulator.list(&json!({})).await;

    assert_eq!(status, 200, "case \"empty store\": expected 200, got {status} with {body}");
    assert!(
        actions_of("empty store", &body).is_empty(),
        "case \"empty store\": expected recommendedActions [], got {body}"
    );
    assert!(
        body.get("nextToken").is_none(),
        "case \"empty store\": expected no nextToken, got {body}"
    );
}

#[tokio::test]
async fn an_unfiltered_listing_returns_every_stored_record_and_member() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    let (status, body) = emulator.list(&json!({})).await;

    assert_eq!(status, 200, "case \"unfiltered\": expected 200, got {status} with {body}");
    assert_eq!(
        ids_of("unfiltered", &body),
        fixture_ids(),
        "case \"unfiltered\": expected every seeded id in insertion order, got {body}"
    );

    // Every member of every record is read out of the store, not synthesised.
    for stored in fixture() {
        let case = stored.id.clone();
        let entry = actions_of(&case, &body)
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(stored.id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: record missing from the response: {body}"));
        let expected = json!({
            "id": stored.id,
            "type": stored.action_type.as_str(),
            "accountId": stored.account_id,
            "severity": stored.severity.as_str(),
            "feature": stored.feature.as_str(),
            "context": stored.context.iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect::<serde_json::Map<String, Value>>(),
            "nextSteps": stored.next_steps,
            "lastUpdatedTimeStamp": stored.last_updated_timestamp,
        });
        assert_eq!(
            entry, &expected,
            "case {case:?}: expected the stored record projected verbatim, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_filter_selects_exactly_the_records_its_predicate_keeps() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    // key, matchOption, values -> the ids the predicate keeps. Each row is written
    // out by hand from the fixture above rather than computed, so a handler that
    // evaluates the predicate differently cannot agree with it by construction.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "FEATURE EQUALS [BUDGETS]",
            json!([{"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]}]),
            vec!["ra-budget-create", "ra-budget-exceeded"],
        ),
        (
            "FEATURE NOT_EQUALS [BUDGETS]",
            json!([{"key": "FEATURE", "matchOption": "NOT_EQUALS", "values": ["BUDGETS"]}]),
            vec![
                "ra-payment-due",
                "ra-freetier-usage",
                "ra-iam-granular",
                "ra-anomaly-review",
            ],
        ),
        (
            "SEVERITY EQUALS [CRITICAL, WARNING]",
            json!([{"key": "SEVERITY", "matchOption": "EQUALS", "values": ["CRITICAL", "WARNING"]}]),
            vec![
                "ra-budget-create",
                "ra-budget-exceeded",
                "ra-payment-due",
                "ra-anomaly-review",
            ],
        ),
        (
            "SEVERITY NOT_EQUALS [CRITICAL, WARNING]",
            json!([{"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["CRITICAL", "WARNING"]}]),
            vec!["ra-freetier-usage", "ra-iam-granular"],
        ),
        (
            "TYPE EQUALS [PAYMENTS_DUE]",
            json!([{"key": "TYPE", "matchOption": "EQUALS", "values": ["PAYMENTS_DUE"]}]),
            vec!["ra-payment-due"],
        ),
        (
            "TYPE NOT_EQUALS [PAYMENTS_DUE]",
            json!([{"key": "TYPE", "matchOption": "NOT_EQUALS", "values": ["PAYMENTS_DUE"]}]),
            vec![
                "ra-budget-create",
                "ra-budget-exceeded",
                "ra-freetier-usage",
                "ra-iam-granular",
                "ra-anomaly-review",
            ],
        ),
        (
            "FEATURE EQUALS [TAX_SETTINGS] (matches nothing)",
            json!([{"key": "FEATURE", "matchOption": "EQUALS", "values": ["TAX_SETTINGS"]}]),
            vec![],
        ),
        (
            "FEATURE EQUALS [BUDGETS] AND SEVERITY EQUALS [CRITICAL]",
            json!([
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]},
                {"key": "SEVERITY", "matchOption": "EQUALS", "values": ["CRITICAL"]},
            ]),
            vec!["ra-budget-exceeded"],
        ),
        (
            "FEATURE EQUALS [BUDGETS, PAYMENTS] AND SEVERITY NOT_EQUALS [WARNING]",
            json!([
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS", "PAYMENTS"]},
                {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["WARNING"]},
            ]),
            vec!["ra-budget-exceeded", "ra-payment-due"],
        ),
        (
            "TYPE EQUALS [CREATE_BUDGET] AND TYPE EQUALS [PAYMENTS_DUE] (contradiction)",
            json!([
                {"key": "TYPE", "matchOption": "EQUALS", "values": ["CREATE_BUDGET"]},
                {"key": "TYPE", "matchOption": "EQUALS", "values": ["PAYMENTS_DUE"]},
            ]),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let (status, body) = emulator
            .list(&json!({"filter": {"actions": filters}}))
            .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {body}");
        assert_eq!(
            ids_of(case, &body),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?}: an unpaginated listing must carry no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn not_equals_is_the_complement_of_equals_for_every_published_enum_member() {
    // The catalogs come from this service's Smithy model (`smithy_query(query="shape",
    // shape="Feature,Severity,ActionType")`), reached through the enums the state
    // module declares — so every published member is exercised, including the ones
    // no fixture of this issue cited.
    let emulator = Emulator::empty();

    // One record per ActionType member, each carrying a feature and severity that
    // cycle through their own catalogs.
    let seeded: Vec<StoredAction> = ActionType::ALL
        .iter()
        .enumerate()
        .map(|(index, action_type)| {
            action(
                &format!("ra-{index:02}"),
                *action_type,
                Severity::ALL[index % Severity::ALL.len()],
                Feature::ALL[index % Feature::ALL.len()],
            )
        })
        .collect();
    emulator.seed(seeded.clone());
    let all_ids: Vec<String> = seeded.iter().map(|a| a.id.clone()).collect();

    // key, the value to filter on, and the ids the value selects.
    let mut cases: Vec<(String, &str, String, Vec<String>)> = Vec::new();
    for feature in Feature::ALL {
        let selected = seeded
            .iter()
            .filter(|a| a.feature == *feature)
            .map(|a| a.id.clone())
            .collect();
        cases.push((
            format!("FEATURE {feature}"),
            "FEATURE",
            feature.as_str().to_string(),
            selected,
        ));
    }
    for severity in Severity::ALL {
        let selected = seeded
            .iter()
            .filter(|a| a.severity == *severity)
            .map(|a| a.id.clone())
            .collect();
        cases.push((
            format!("SEVERITY {severity}"),
            "SEVERITY",
            severity.as_str().to_string(),
            selected,
        ));
    }
    for action_type in ActionType::ALL {
        let selected = seeded
            .iter()
            .filter(|a| a.action_type == *action_type)
            .map(|a| a.id.clone())
            .collect();
        cases.push((
            format!("TYPE {action_type}"),
            "TYPE",
            action_type.as_str().to_string(),
            selected,
        ));
    }

    for (case, key, value, selected) in cases {
        let (equals_status, equals_body) = emulator
            .list(&json!({"filter": {"actions": [
                {"key": key, "matchOption": "EQUALS", "values": [value]}
            ]}}))
            .await;
        assert_eq!(
            equals_status, 200,
            "case {case:?} EQUALS: expected 200, got {equals_status} with {equals_body}"
        );
        let kept = ids_of(&case, &equals_body);
        assert_eq!(
            kept, selected,
            "case {case:?} EQUALS: expected {selected:?}, got {kept:?}"
        );

        let (not_status, not_body) = emulator
            .list(&json!({"filter": {"actions": [
                {"key": key, "matchOption": "NOT_EQUALS", "values": [value]}
            ]}}))
            .await;
        assert_eq!(
            not_status, 200,
            "case {case:?} NOT_EQUALS: expected 200, got {not_status} with {not_body}"
        );
        let excluded = ids_of(&case, &not_body);
        let expected_complement: Vec<String> = all_ids
            .iter()
            .filter(|id| !selected.contains(id))
            .cloned()
            .collect();
        assert_eq!(
            excluded, expected_complement,
            "case {case:?} NOT_EQUALS: expected the complement {expected_complement:?}, got {excluded:?}"
        );
        assert!(
            kept.iter().all(|id| !excluded.contains(id)),
            "case {case:?}: EQUALS and NOT_EQUALS overlap: {kept:?} vs {excluded:?}"
        );
        assert_eq!(
            kept.len() + excluded.len(),
            all_ids.len(),
            "case {case:?}: EQUALS ({kept:?}) and NOT_EQUALS ({excluded:?}) must partition the store"
        );
    }
}

#[tokio::test]
async fn paging_reports_every_matching_record_exactly_once() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    // Every page size from one record to one past the whole set, plus the published
    // upper bound of `maxResults` (1..1000, per the model's MaxResults range).
    let page_sizes = [1_i64, 2, 3, 4, 5, 6, 7, 1000];
    // The filter each walk applies, and the ids it must yield in order.
    let queries: Vec<(&str, Value, Vec<String>)> = vec![
        ("unfiltered", json!({}), fixture_ids()),
        (
            "FEATURE NOT_EQUALS [BUDGETS]",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "NOT_EQUALS", "values": ["BUDGETS"]}
            ]}}),
            vec![
                "ra-payment-due".to_string(),
                "ra-freetier-usage".to_string(),
                "ra-iam-granular".to_string(),
                "ra-anomaly-review".to_string(),
            ],
        ),
        (
            "FEATURE EQUALS [TAX_SETTINGS]",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["TAX_SETTINGS"]}
            ]}}),
            vec![],
        ),
    ];

    for (query_name, query, expected) in queries {
        for page_size in page_sizes {
            let case = format!("{query_name} with maxResults {page_size}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;

            loop {
                let mut request = query.clone();
                let object = request
                    .as_object_mut()
                    .unwrap_or_else(|| panic!("case {case:?}: the query is not a JSON object"));
                object.insert("maxResults".to_string(), json!(page_size));
                if let Some(token) = token.as_ref() {
                    object.insert("nextToken".to_string(), json!(token));
                }

                let (status, body) = emulator.list(&request).await;
                assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {body}");
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: pagination did not terminate after {pages} pages, last body {body}"
                );

                let page = ids_of(&case, &body);
                assert!(
                    i64::try_from(page.len()).expect("page length fits an i64") <= page_size,
                    "case {case:?}: a page carried {} records, more than maxResults, got {body}",
                    page.len()
                );
                collected.extend(page.iter().cloned());

                match body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => {
                        assert!(
                            collected.len() < expected.len(),
                            "case {case:?}: a nextToken was returned after reporting every record ({collected:?})"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }

            assert_eq!(
                collected, expected,
                "case {case:?}: the concatenation of pages must equal the unpaginated listing {expected:?}, got {collected:?}"
            );
            let mut deduped = collected.clone();
            deduped.sort();
            deduped.dedup();
            assert_eq!(
                deduped.len(),
                collected.len(),
                "case {case:?}: a record was reported twice: {collected:?}"
            );
        }
    }
}

#[tokio::test]
async fn max_results_outside_the_published_range_is_refused() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    // The model's MaxResults range is 1..1000: each bound is accepted, each value
    // one past it is refused.
    for (case, max_results, expected_status) in [
        ("lower bound 1", 1_i64, 200_u16),
        ("upper bound 1000", 1000, 200),
        ("one below the lower bound", 0, 400),
        ("one past the upper bound", 1001, 400),
    ] {
        let (status, body) = emulator.list(&json!({"maxResults": max_results})).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for maxResults {max_results}, got {status} with {body}"
        );
        if expected_status != 200 {
            assert!(
                body.get("recommendedActions").is_none(),
                "case {case:?}: a refused request must not return a listing, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    // A token minted for a DIFFERENT query: same page size, different filter.
    let (status, body) = emulator
        .list(&json!({
            "maxResults": 1,
            "filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]}
            ]}
        }))
        .await;
    assert_eq!(status, 200, "minting a token: expected 200, got {status} with {body}");
    let foreign_token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("minting a token: expected a nextToken, got {body}"))
        .to_string();

    let cases: Vec<(&str, Value)> = vec![
        ("not a token at all", json!({"nextToken": "not-a-real-token"})),
        ("empty token", json!({"nextToken": ""})),
        ("too short", json!({"nextToken": "deadbeef"})),
        (
            "right length, not hex",
            json!({"nextToken": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}),
        ),
        (
            "right length hex, unknown query fingerprint",
            json!({"nextToken": "00000000000000000000000000000000" .to_string() + "00000001"}),
        ),        (
            "a token issued for a different filter",
            json!({"maxResults": 1, "nextToken": foreign_token}),
        ),
        (
            "an upper-cased copy of a real token",
            json!({"maxResults": 1, "nextToken": foreign_token.to_uppercase()}),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator.list(&request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert!(
            body.get("recommendedActions").is_none(),
            "case {case:?}: a refused token must not silently restart the listing, got {body}"
        );
    }
}

#[tokio::test]
async fn a_token_resumes_the_same_query_it_was_issued_for() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    let query = json!({
        "maxResults": 2,
        "filter": {"actions": [
            {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["INFO"]}
        ]}
    });

    let (status, first) = emulator.list(&query).await;
    assert_eq!(status, 200, "first page: expected 200, got {status} with {first}");
    assert_eq!(
        ids_of("first page", &first),
        vec!["ra-budget-create", "ra-budget-exceeded"],
        "first page: expected the first two matching records, got {first}"
    );
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("first page: expected a nextToken, got {first}"))
        .to_string();

    let mut resumed = query.clone();
    resumed
        .as_object_mut()
        .expect("the query is a JSON object")
        .insert("nextToken".to_string(), json!(token.clone()));
    let (status, second) = emulator.list(&resumed).await;
    assert_eq!(status, 200, "second page: expected 200, got {status} with {second}");
    assert_eq!(
        ids_of("second page", &second),
        vec!["ra-payment-due", "ra-anomaly-review"],
        "second page: expected the next two matching records, got {second}"
    );
    assert!(
        second.get("nextToken").is_none(),
        "second page: the last page must carry no nextToken, got {second}"
    );

    // The same token re-presented returns the same page: a read never advances a
    // cursor held in the service.
    let (status, replayed) = emulator.list(&resumed).await;
    assert_eq!(status, 200, "replayed page: expected 200, got {status} with {replayed}");
    assert_eq!(
        ids_of("replayed page", &replayed),
        ids_of("second page", &second),
        "replayed page: re-presenting a token must report the same page, got {replayed}"
    );
}

#[tokio::test]
async fn every_refused_member_is_stated_with_a_reason_and_a_field_list() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    // Each row is a refused input and the member path(s) the refusal must name.
    // The class spans every way one request member can be refused — an unmodelled
    // `key` or `matchOption` (both target enum shapes, so the model closes their
    // value sets), a `values` list shorter than its modelled minimum, a `values`
    // ELEMENT outside the modelled length of `FilterValue` (1..1024), a `maxResults`
    // outside its modelled range, and an absent required member — found at the
    // constraint boundary or in the handler alike. A `values` element that is simply
    // not a value any record carries is NOT in this class: it is compared, not
    // validated (see
    // `a_filter_value_outside_a_records_value_set_matches_nothing_rather_than_failing`).
    // Paths are the dotted, 1-based spelling the service's validator uses
    // (`filter.actions.1.member.key`).
    let too_long = "x".repeat(1025);
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        // `FilterValue` is modelled `length: {min: 1, max: 1024}`
        // (`smithy_query(query="shape", shape="FilterValue")`).
        (
            "a values element past the modelled maximum length",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": [too_long]}
            ]}}),
            vec!["filter.actions.1.member.values.1.member"],
        ),
        (
            "an empty values element",
            json!({"filter": {"actions": [
                {"key": "TYPE", "matchOption": "EQUALS", "values": ["CREATE_BUDGET", ""]}
            ]}}),
            vec!["filter.actions.1.member.values.2.member"],
        ),
        (
            "an unmodelled filter key",
            json!({"filter": {"actions": [
                {"key": "ACCOUNT_ID", "matchOption": "EQUALS", "values": ["000000000000"]}
            ]}}),
            vec!["filter.actions.1.member.key"],
        ),
        (
            "an unmodelled match option",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "CONTAINS", "values": ["BUDGETS"]}
            ]}}),
            vec!["filter.actions.1.member.matchOption"],
        ),
        // Two offending members in one request: AWS reports EVERY validation error,
        // so both must be named.
        (
            "an unmodelled key and match option together",
            json!({"filter": {"actions": [
                {"key": "ACCOUNT_ID", "matchOption": "CONTAINS", "values": ["000000000000"]}
            ]}}),
            vec![
                "filter.actions.1.member.key",
                "filter.actions.1.member.matchOption",
            ],
        ),
        // `FilterValues` is modelled `length: {min: 1}`.
        (
            "an empty values list",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": []}
            ]}}),
            vec!["filter.actions.1.member.values"],
        ),
        (
            "an empty values list beside a well-formed filter",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]},
                {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": []}
            ]}}),
            vec!["filter.actions.2.member.values"],
        ),
        // `ActionFilter$key` is modelled required: an absent member is refused as
        // the same shape, naming the member that is missing.
        (
            "an absent required filter member",
            json!({"filter": {"actions": [
                {"matchOption": "EQUALS", "values": ["BUDGETS"]}
            ]}}),
            vec!["filter.actions.1.member.key"],
        ),
        // Not a filter member at all: the same refusal shape covers every member of
        // the input, so `maxResults` outside its modelled range is stated alike.
        (
            "maxResults below the modelled range",
            json!({"maxResults": 0}),
            vec!["maxResults"],
        ),
        (
            "maxResults past the modelled range",
            json!({"maxResults": 1001}),
            vec!["maxResults"],
        ),
    ];

    for (case, request, expected_paths) in cases {
        let (status, body) = emulator.list(&request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert!(
            body.get("recommendedActions").is_none(),
            "case {case:?}: a refused request must not return a listing, got {body}"
        );
        // `ValidationException` models `reason` as REQUIRED, from
        // `ValidationExceptionReason` (`smithy_query(query="shape",
        // shape="ValidationExceptionReason")`); a member-level refusal is
        // `fieldValidationFailed`.
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("fieldValidationFailed"),
            "case {case:?}: expected reason fieldValidationFailed, got {body}"
        );
        // One `fieldList` entry per offending member: the name is the member's path,
        // the message states the constraint it failed.
        let fields = body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList array, got {body}"));
        let names: Vec<&str> = fields
            .iter()
            .filter_map(|field| field.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names, expected_paths,
            "case {case:?}: expected fieldList to name {expected_paths:?}, got {names:?} in {body}"
        );
        for field in fields {
            let message = field.get("message").and_then(Value::as_str).unwrap_or("");
            assert!(
                message.contains("failed to satisfy constraint"),
                "case {case:?}: expected a fieldList message stating the constraint, got {field}"
            );
        }
        // The message is the envelope AWS states, counting the errors it reports and
        // carrying each clause.
        let message = body.get("message").and_then(Value::as_str).unwrap_or("");
        let count = expected_paths.len();
        let expected_prefix = if count == 1 {
            "1 validation error detected: ".to_string()
        } else {
            format!("{count} validation errors detected: ")
        };
        assert!(
            message.starts_with(&expected_prefix),
            "case {case:?}: expected a message starting {expected_prefix:?}, got {message:?}"
        );
        for path in &expected_paths {
            assert!(
                message.contains(path),
                "case {case:?}: expected the message to name {path:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_refusal_names_at_most_the_capped_number_of_members() {
    let emulator = Emulator::empty();

    // A refusal states every offending member, but a bounded number of them: each
    // violation is rendered twice into the body (a clause and a `fieldList` entry),
    // so a request-chosen list length must not grow the response without bound. The
    // ceiling is the boundary validator's own (`MAX_VIOLATIONS` = 100 in
    // vendor/smithy-schema-server/src/constraints.rs), so both refusal paths bound
    // alike. `ActionFilterList` is modelled with no maximum length
    // (`smithy_query(query="shape", shape="ActionFilterList")`), so a list of filters
    // each naming an unmodelled `key` is the length a caller can grow freely.
    const MAX_VIOLATIONS: usize = 100;
    for count in [1_usize, 2, 99, 100, 101, 500] {
        let filters: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "key": format!("NOT_A_FILTER_KEY_{index}"),
                    "matchOption": "EQUALS",
                    "values": ["BUDGETS"],
                })
            })
            .collect();
        let (status, body) = emulator
            .list(&json!({"filter": {"actions": filters}}))
            .await;
        assert_eq!(
            status, 400,
            "case {count} bad keys: expected HTTP 400, got {status} with {body}"
        );
        let fields = body
            .get("fieldList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {count} bad keys: expected a fieldList, got {body}"));
        assert_eq!(
            fields.len(),
            count.min(MAX_VIOLATIONS),
            "case {count} bad keys: expected at most {MAX_VIOLATIONS} named members, got {}",
            fields.len()
        );
    }
}

#[tokio::test]
async fn a_filter_value_outside_a_records_value_set_matches_nothing_rather_than_failing() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());
    let all = fixture_ids();

    // `ActionFilter$values` targets `FilterValues`, whose member `FilterValue` is a
    // plain string (`length {min:1,max:1024}`, pattern `[\S\s]*`) —
    // `smithy_query(query="shape", shape="ActionFilter,FilterValues,FilterValue")`.
    // The model closes no value set over it, so a value is COMPARED against each
    // record's field: one no record carries keeps nothing under `EQUALS` and
    // excludes nothing under `NOT_EQUALS`. The rows vary along what the rule
    // generalises over: the filter key (every `FilterName` member), the form of the
    // unmatched value (a near-miss in case, a value drawn from another of this
    // service's enums, a typo, a pluralisation, whitespace, a maximum-length
    // string), and how many values one filter carries.
    let at_max_length = "D".repeat(1024);
    let unmatched: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "a severity no record has",
            "SEVERITY",
            vec!["EXTREME".to_string()],
        ),
        (
            "a lower-cased severity",
            "SEVERITY",
            vec!["critical".to_string()],
        ),
        (
            "a mixed-case severity",
            "SEVERITY",
            vec!["Critical".to_string()],
        ),
        (
            "a feature spelled with spaces",
            "FEATURE",
            vec!["COST ANOMALY DETECTION".to_string()],
        ),
        (
            "a feature value drawn from another enum",
            "FEATURE",
            vec!["CRITICAL".to_string()],
        ),
        (
            "a pluralised action type",
            "TYPE",
            vec!["CREATE_BUDGETS".to_string()],
        ),
        (
            "an action type as a bare word",
            "TYPE",
            vec!["budget".to_string()],
        ),
        ("a single space", "TYPE", vec![" ".to_string()]),
        (
            "a value at the modelled maximum length",
            "FEATURE",
            vec![at_max_length],
        ),
        (
            "several unmatched values at once",
            "SEVERITY",
            vec![
                "EXTREME".to_string(),
                "severe".to_string(),
                "0".to_string(),
            ],
        ),
        (
            "a feature the model publishes that no stored record carries",
            "FEATURE",
            vec!["TAX_SETTINGS".to_string()],
        ),
    ];

    for (case, key, values) in unmatched {
        // EQUALS over values nothing carries keeps no record, and says so with 200.
        let (status, body) = emulator
            .list(&json!({"filter": {"actions": [
                {"key": key, "matchOption": "EQUALS", "values": values}
            ]}}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} EQUALS: expected HTTP 200, got {status} with {body}"
        );
        let kept = ids_of(case, &body);
        assert!(
            kept.is_empty(),
            "case {case:?} EQUALS: expected no record kept, got {kept:?}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case:?} EQUALS: an empty page owes no nextToken, got {body}"
        );

        // NOT_EQUALS is its complement: every stored record, since none matched.
        let (status, body) = emulator
            .list(&json!({"filter": {"actions": [
                {"key": key, "matchOption": "NOT_EQUALS", "values": values}
            ]}}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} NOT_EQUALS: expected HTTP 200, got {status} with {body}"
        );
        let excluded = ids_of(case, &body);
        assert_eq!(
            excluded, all,
            "case {case:?} NOT_EQUALS: expected every stored record {all:?}, got {excluded:?}"
        );
    }

    // An unmatched value contributes no match; it does not poison the filter it sits
    // in nor the conjunction the filter sits in.
    let mixed: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one filter mixing a matched and an unmatched value",
            json!([{"key": "SEVERITY", "matchOption": "EQUALS", "values": ["CRITICAL", "EXTREME"]}]),
            vec!["ra-budget-exceeded", "ra-payment-due"],
        ),
        (
            "NOT_EQUALS over a matched and an unmatched value",
            json!([{"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["CRITICAL", "EXTREME"]}]),
            vec![
                "ra-budget-create",
                "ra-freetier-usage",
                "ra-iam-granular",
                "ra-anomaly-review",
            ],
        ),
        (
            "an unmatched EQUALS filter ANDed with a matching one",
            json!([
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]},
                {"key": "SEVERITY", "matchOption": "EQUALS", "values": ["EXTREME"]},
            ]),
            vec![],
        ),
        (
            "an unmatched NOT_EQUALS filter ANDed with a matching one",
            json!([
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]},
                {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["EXTREME"]},
            ]),
            vec!["ra-budget-create", "ra-budget-exceeded"],
        ),
    ];

    for (case, filters, expected) in mixed {
        let (status, body) = emulator
            .list(&json!({"filter": {"actions": filters}}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let kept = ids_of(case, &body);
        assert_eq!(
            kept, expected,
            "case {case:?}: expected exactly {expected:?}, got {kept:?}"
        );
    }

    // A listing stays a pure read: no row above added, dropped or rewrote a record.
    let held = emulator.stored_count();
    assert_eq!(
        held,
        all.len(),
        "case \"unmatched filter values\": expected the store to still hold {} records, got {held}",
        all.len()
    );
}

#[tokio::test]
async fn every_response_announces_the_services_media_type() {
    let emulator = Emulator::new();

    // What this service's own responses announce does not depend on what the client
    // sent or on whether the call succeeded: one media type for a success and for a
    // failure its handler raises. A refusal of the REQUEST is answered by the input
    // validator in front of the service, which announces the media type the model's
    // protocol trait declares — the last column is the media type each row expects.
    let json_1_0 = "application/x-amz-json-1.0";
    let json_1_1 = "application/x-amz-json-1.1";
    let cases: Vec<(&str, &str, &str, Value, u16, &str)> = vec![
        (
            "success, awsJson1_0 request",
            json_1_0,
            TARGET,
            json!({}),
            200,
            json_1_1,
        ),
        (
            "success, awsJson1_1 request",
            json_1_1,
            TARGET,
            json!({}),
            200,
            json_1_1,
        ),
        (
            "success, no request media type",
            "",
            TARGET,
            json!({}),
            200,
            json_1_1,
        ),
        (
            "filtered success",
            json_1_0,
            TARGET,
            json!({"filter": {"actions": [
                {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["CRITICAL"]}
            ]}}),
            200,
            json_1_1,
        ),
        (
            "a modelled failure the handler raises",
            json_1_0,
            TARGET,
            json!({"nextToken": "not-a-token-this-service-minted"}),
            400,
            json_1_1,
        ),
        (
            "a refused member value",
            json_1_0,
            TARGET,
            json!({"filter": {"actions": [
                {"key": "NOT_A_REAL_FILTER_KEY", "matchOption": "EQUALS", "values": ["BUDGETS"]}
            ]}}),
            400,
            json_1_0,
        ),
        (
            "a refused member value, awsJson1_1 request",
            json_1_1,
            TARGET,
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "NOT_A_REAL_MATCH_OPTION", "values": ["BUDGETS"]}
            ]}}),
            400,
            json_1_0,
        ),
        (
            "an unroutable target",
            json_1_0,
            "AWSBillingAndCostManagementRecommendedActions.NoSuchOperation",
            json!({}),
            501,
            json_1_0,
        ),
    ];

    for (case, request_content_type, target, body, expected_status, expected_media_type) in cases {
        let (status, content_type, payload) = emulator
            .dispatch(&body, request_content_type, target)
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        assert_eq!(
            content_type, expected_media_type,
            "case {case:?}: expected media type {expected_media_type:?}, got {content_type:?} with {payload}"
        );
    }
}

#[tokio::test]
async fn the_boot_inventory_is_reported_and_is_filterable() {
    // The service models no create operation — AWS computes the recommendations —
    // so a caller that never seeded anything must still read the inventory this
    // account has, and an exclusion filter over it must return its complement
    // rather than nothing.
    let emulator = Emulator::new();

    let (status, body) = emulator.list(&json!({})).await;
    assert_eq!(status, 200, "boot inventory: expected 200, got {status} with {body}");
    let boot = actions_of("boot inventory", &body).clone();
    assert!(
        !boot.is_empty(),
        "boot inventory: expected the computed recommendations, got {body}"
    );

    // Every record is a well-formed `RecommendedAction` for this account: the
    // modelled members, each carrying a member of its own enum.
    let account_id = engine::emulator_account_id();
    for entry in &boot {
        let case = entry
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("boot inventory: a record carries no id: {entry}"));
        assert_eq!(
            entry.get("accountId").and_then(Value::as_str),
            Some(account_id),
            "case {case:?}: expected the emulator's account id, got {entry}"
        );
        for member in ["type", "severity", "feature", "lastUpdatedTimeStamp"] {
            assert!(
                entry.get(member).and_then(Value::as_str).is_some_and(|v| !v.is_empty()),
                "case {case:?}: expected a {member}, got {entry}"
            );
        }
    }

    // Severity enum members from `smithy_query(query="shape", shape="Severity")`.
    for severity in ["INFO", "WARNING", "CRITICAL"] {
        let request = |match_option: &str| {
            json!({"filter": {"actions": [
                {"key": "SEVERITY", "matchOption": match_option, "values": [severity]}
            ]}})
        };
        let (status, kept) = emulator.list(&request("EQUALS")).await;
        assert_eq!(status, 200, "case {severity:?} EQUALS: expected 200, got {kept}");
        let (status, excluded) = emulator.list(&request("NOT_EQUALS")).await;
        assert_eq!(status, 200, "case {severity:?} NOT_EQUALS: expected 200, got {excluded}");

        let kept = ids_of(severity, &kept);
        let excluded = ids_of(severity, &excluded);
        let mut union = kept.clone();
        union.extend(excluded.iter().cloned());
        union.sort();
        let mut all: Vec<String> = boot
            .iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        all.sort();
        assert_eq!(
            union, all,
            "case {severity:?}: EQUALS and NOT_EQUALS must partition the inventory, got {kept:?} and {excluded:?}"
        );
        assert!(
            kept.iter().all(|id| !excluded.contains(id)),
            "case {severity:?}: a record cannot be both kept and excluded, got {kept:?} and {excluded:?}"
        );
    }

    // Reading the inventory does not consume it.
    let (status, again) = emulator.list(&json!({})).await;
    assert_eq!(status, 200, "re-read: expected 200, got {status} with {again}");
    assert_eq!(
        actions_of("re-read", &again),
        &boot,
        "re-read: reporting the inventory must not change it, got {again}"
    );
}

#[tokio::test]
async fn listing_never_changes_what_a_later_caller_sees() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());
    let before = emulator.stored_count();

    let requests = [
        json!({}),
        json!({"maxResults": 2}),
        json!({"filter": {"actions": [
            {"key": "SEVERITY", "matchOption": "EQUALS", "values": ["CRITICAL"]}
        ]}}),
    ];

    for request in &requests {
        let (first_status, first) = emulator.list(request).await;
        let (second_status, second) = emulator.list(request).await;
        let case = request.to_string();
        assert_eq!(
            first_status, second_status,
            "case {case:?}: repeating a read changed the status: {first_status} then {second_status}"
        );
        assert_eq!(
            first, second,
            "case {case:?}: repeating a read changed the response: {first} then {second}"
        );
    }

    assert_eq!(
        emulator.stored_count(),
        before,
        "listing must not add or drop records: the store held {before} before and {} after",
        emulator.stored_count()
    );
}

#[tokio::test]
async fn clearing_the_store_empties_the_listing() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    let (status, before) = emulator.list(&json!({})).await;
    assert_eq!(status, 200, "before clearing: expected 200, got {status} with {before}");
    assert_eq!(
        ids_of("before clearing", &before).len(),
        fixture().len(),
        "before clearing: expected the whole fixture, got {before}"
    );

    assert!(
        emulator.clear(),
        "clearing the store must leave it empty, but it still holds {} record(s)",
        emulator.stored_count()
    );

    let (status, after) = emulator.list(&json!({})).await;
    assert_eq!(status, 200, "after clearing: expected 200, got {status} with {after}");
    assert!(
        actions_of("after clearing", &after).is_empty(),
        "after clearing: expected recommendedActions [], got {after}"
    );
    assert!(
        after.get("nextToken").is_none(),
        "after clearing: expected no nextToken, got {after}"
    );
}

#[tokio::test]
async fn re_seeding_an_existing_id_updates_it_in_place() {
    let emulator = Emulator::empty();
    emulator.seed(fixture());

    let mut updated = action(
        "ra-budget-create",
        ActionType::CreateBudget,
        Severity::Critical,
        Feature::Budgets,
    );
    updated.last_updated_timestamp = "2026-10-22T08:00:00Z".to_string();
    emulator.seed(vec![updated.clone()]);

    let (status, body) = emulator.list(&json!({})).await;
    assert_eq!(status, 200, "re-seeded listing: expected 200, got {status} with {body}");
    assert_eq!(
        ids_of("re-seeded listing", &body),
        fixture_ids(),
        "re-seeded listing: the record count and order must not change, got {body}"
    );
    let entry = &actions_of("re-seeded listing", &body)[0];
    assert_eq!(
        entry.get("severity").and_then(Value::as_str),
        Some("CRITICAL"),
        "re-seeded listing: expected the updated severity, got {entry}"
    );
    assert_eq!(
        entry.get("lastUpdatedTimeStamp").and_then(Value::as_str),
        Some(updated.last_updated_timestamp.as_str()),
        "re-seeded listing: expected the updated timestamp, got {entry}"
    );
}
