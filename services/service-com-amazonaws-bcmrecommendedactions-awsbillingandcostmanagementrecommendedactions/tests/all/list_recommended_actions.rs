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
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
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

    /// `ListRecommendedActions` with `body` as the request payload.
    async fn list(&self, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", TARGET)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
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
    let emulator = Emulator::new();

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
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();

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
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();
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
async fn a_filter_value_outside_the_enum_its_key_names_is_refused() {
    let emulator = Emulator::new();
    emulator.seed(fixture());

    // Each row carries a value that is not a member of the enum the key names —
    // including near-misses in case, spelling and pluralisation. `ActionFilter$key`
    // and `$matchOption` target enum shapes, so the framework's own constraint
    // validator refuses those two before the handler runs and answers the same 400
    // `ValidationException` without the modelled `reason`/`fieldList` members; the
    // last element of each row says whether the modelled members are expected.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "FEATURE with a lowercase member",
            json!([{"key": "FEATURE", "matchOption": "EQUALS", "values": ["budgets"]}]),
            true,
        ),
        (
            "FEATURE with a value from another enum",
            json!([{"key": "FEATURE", "matchOption": "EQUALS", "values": ["CRITICAL"]}]),
            true,
        ),
        (
            "SEVERITY with an unmodelled level",
            json!([{"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": ["SEVERE"]}]),
            true,
        ),
        (
            "TYPE with a pluralised member",
            json!([{"key": "TYPE", "matchOption": "EQUALS", "values": ["CREATE_BUDGETS"]}]),
            true,
        ),
        (
            "one bad value beside a good one",
            json!([{"key": "TYPE", "matchOption": "EQUALS", "values": ["CREATE_BUDGET", "CREATE_BUDGETS"]}]),
            true,
        ),
        (
            "an unmodelled filter key",
            json!([{"key": "ACCOUNT_ID", "matchOption": "EQUALS", "values": ["000000000000"]}]),
            false,
        ),
        (
            "an unmodelled match option",
            json!([{"key": "FEATURE", "matchOption": "CONTAINS", "values": ["BUDGETS"]}]),
            false,
        ),
        // `FilterValues` is modelled `length: {min: 1}`, so an empty list is refused
        // too — whether by the framework's constraint validation or by the handler's
        // backstop, hence `false` for the modelled members.
        (
            "an empty values list",
            json!([{"key": "FEATURE", "matchOption": "EQUALS", "values": []}]),
            false,
        ),
        (
            "an empty values list beside a well-formed filter",
            json!([
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]},
                {"key": "SEVERITY", "matchOption": "NOT_EQUALS", "values": []}
            ]),
            false,
        ),
    ];

    for (case, filters, modelled_members) in cases {
        let (status, body) = emulator
            .list(&json!({"filter": {"actions": filters}}))
            .await;
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
            "case {case:?}: a refused filter must not return a listing, got {body}"
        );
        if !modelled_members {
            continue;
        }
        // `ValidationException` models `reason` as required, with
        // `ValidationExceptionReason` as its enum (`smithy_query(query="shape",
        // shape="ValidationExceptionReason")`), and `fieldList` naming the member
        // at fault — a client reads both.
        let reason = body.get("reason").and_then(Value::as_str);
        assert!(
            matches!(
                reason,
                Some("unknownOperation" | "cannotParse" | "fieldValidationFailed" | "other")
            ),
            "case {case:?}: expected a ValidationExceptionReason member, got {reason:?} in {body}"
        );
        let field_names: Vec<&str> = body
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            field_names
                .iter()
                .any(|name| name.starts_with("filter.actions[0]")),
            "case {case:?}: expected fieldList to name the offending filter member, got {field_names:?} in {body}"
        );
    }
}

#[tokio::test]
async fn listing_never_changes_what_a_later_caller_sees() {
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();
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
    let emulator = Emulator::new();
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
