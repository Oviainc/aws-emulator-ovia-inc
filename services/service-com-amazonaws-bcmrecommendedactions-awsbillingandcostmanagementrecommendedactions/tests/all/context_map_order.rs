// SPDX-License-Identifier: CC0-1.0

//! `ListRecommendedActions` reports; it never writes. So the BYTES of its response
//! are a function of the records the store holds and the request alone: reading
//! unchanged records twice must answer identically, down to the order the keys of
//! each record's `RecommendedAction$context` map are written in.
//!
//! The suite drives real awsJson1_0 requests through [`MultiServiceRouter::dispatch`]
//! and asserts on the RAW response text, because a parsed `serde_json::Value` holds
//! an object's members in sorted order whatever order the wire carried them in and
//! so cannot see this difference.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

use bcm::state::{ActionType, Feature, Severity, StoredAction};

/// The awsJson1_0 target this service answers.
const TARGET: &str = "AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name — the routing signal the router reads. The signature is not
/// checked: request authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261028/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One emulator: a shared engine, and a router with this service mounted on it.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator, holding the inventory the service computed for this
    /// account.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The same emulator with that inventory cleared, so a case reasons about
    /// exactly the records it seeds.
    fn empty() -> Self {
        let emulator = Self::new();
        let state = emulator.engine
            .state::<bcm::ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>();
        let mut guard = state.lock();
        guard.recommended_actions.clear();
        assert!(
            guard.recommended_actions.is_empty(),
            "case \"empty emulator\": clearing the boot inventory must leave the store empty"
        );
        drop(guard);
        emulator
    }

    /// Install recommended actions through the state seam (the service models no
    /// create operation).
    fn seed(&self, actions: Vec<StoredAction>) {
        self.engine
            .state::<bcm::ComAmazonawsBcmrecommendedactionsAwsbillingandcostmanagementrecommendedactionsState>()
            .lock()
            .recommended_actions
            .seed(actions);
    }

    /// `ListRecommendedActions` with `body` as the payload, reporting the status and
    /// the response payload AS BYTES (decoded as UTF-8 text, unparsed).
    async fn list_raw(&self, body: &Value) -> (u16, String) {
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
        let text = String::from_utf8(response.body().to_vec())
            .expect("the response body is UTF-8 text");
        (status, text)
    }
}

/// A stored record whose `context` holds exactly `context`.
///
/// Every other member is fixed, so a case's only variable is the context map.
fn action_with_context(id: &str, context: &[(String, String)]) -> StoredAction {
    StoredAction {
        id: id.to_string(),
        action_type: ActionType::CreateBudget,
        account_id: engine::emulator_account_id().to_string(),
        severity: Severity::Warning,
        feature: Feature::Budgets,
        context: context.iter().cloned().collect(),
        next_steps: vec!["Create a budget in the Billing console".to_string()],
        last_updated_timestamp: "2026-10-28T03:11:08Z".to_string(),
    }
}

/// `(key, value)` entries from string literals, in the order written.
fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The context maps a case drives, as SEEDED (so the written order is not the
/// order a response must carry). The rows differ in entry count, in the written
/// order, in key case, and in whether the keys sort as text or as numbers; no key
/// or value carries a quote, a colon or a brace, which is what lets
/// [`context_object`] and [`key_positions`] read the raw response text.
fn context_cases() -> Vec<(&'static str, Vec<(String, String)>)> {
    let many: Vec<(String, String)> = (0..16)
        .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
        .rev()
        .collect();
    vec![
        ("no entries", Vec::new()),
        ("one entry", entries(&[("feature", "BUDGETS")])),
        (
            "entries written in descending key order",
            entries(&[
                ("zone", "us-east-1a"),
                ("region", "us-east-1"),
                ("feature", "BUDGETS"),
            ]),
        ),
        (
            "keys that differ only in case, plus numeric keys",
            entries(&[
                ("feature", "lower"),
                ("2", "two"),
                ("Feature", "upper"),
                ("10", "ten"),
            ]),
        ),
        (
            "a key that is a prefix of another key",
            entries(&[
                ("accountBudget", "monthly"),
                ("account", "payer"),
                ("accountBudgetAlert", "80-percent"),
            ]),
        ),
        ("sixteen entries", many),
    ]
}

/// The raw text of the one record's `context` object, `{"k":"v",...}` included.
///
/// Read out of the response text rather than parsed, because the whole point is
/// the order the bytes carry. The object has no nested value (the model types
/// `Context` as `map<String, String>`) and no fixture value carries a brace, so
/// the first `}` after it closes it.
fn context_object(case: &str, body: &str) -> String {
    let start = body
        .find("\"context\":{")
        .unwrap_or_else(|| panic!("case {case:?}: response carries no context object: {body}"));
    let rest = &body[start..];
    let end = rest
        .find('}')
        .unwrap_or_else(|| panic!("case {case:?}: unterminated context object: {body}"));
    rest[..=end].to_string()
}

/// Where each of `keys` is written in `object`.
///
/// The search token is the key wrapped as a JSON member name (`"<key>":`), so a
/// key that is a prefix of another key cannot match that other key's position.
fn key_positions(case: &str, object: &str, keys: &[String]) -> Vec<usize> {
    keys.iter()
        .map(|key| {
            let token = format!("\"{key}\":");
            object.find(&token).unwrap_or_else(|| {
                panic!("case {case:?}: context object carries no key {key:?}: {object}")
            })
        })
        .collect()
}

#[tokio::test]
async fn a_records_context_is_written_in_ascending_key_order_whatever_order_it_was_stored_in() {
    for (case, seeded) in context_cases() {
        let emulator = Emulator::empty();
        emulator.seed(vec![action_with_context("ra-budget-create", &seeded)]);

        let (status, body) = emulator.list_raw(&json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );

        let object = context_object(case, &body);
        let mut expected_order: Vec<String> = seeded.iter().map(|(key, _)| key.clone()).collect();
        expected_order.sort();
        let positions = key_positions(case, &object, &expected_order);
        for pair in expected_order.windows(2).zip(positions.windows(2)) {
            let (keys, at) = pair;
            assert!(
                at[0] < at[1],
                "case {case:?}: expected key {:?} to be written before {:?}, got {object}",
                keys[0],
                keys[1]
            );
        }

        // Ordering the entries must not drop or cross any of them.
        let parsed: Value = serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("case {case:?}: body is not JSON ({error}): {body}"));
        let expected: BTreeMap<String, String> = seeded.iter().cloned().collect();
        let actual = parsed
            .get("recommendedActions")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(|entry| entry.get("context"))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no record with a context: {body}"));
        assert_eq!(
            actual,
            json!(expected),
            "case {case:?}: expected the seeded entries, got {actual}"
        );
    }
}

#[tokio::test]
async fn identical_requests_over_unchanged_records_answer_with_identical_bytes() {
    // Every context map above, held by its own record, so one response carries
    // them all; the requests vary over the shapes a caller can send (whole
    // listing, a page, a filtered listing).
    let records: Vec<StoredAction> = context_cases()
        .into_iter()
        .enumerate()
        .map(|(index, (_, seeded))| action_with_context(&format!("ra-{index:02}"), &seeded))
        .collect();
    let requests: Vec<(&str, Value)> = vec![
        ("unfiltered", json!({})),
        ("first page of two", json!({"maxResults": 2})),
        (
            "filtered",
            json!({"filter": {"actions": [
                {"key": "FEATURE", "matchOption": "EQUALS", "values": ["BUDGETS"]}
            ]}}),
        ),
    ];

    for (case, request) in requests {
        // Two emulators built independently, each seeded with the same records:
        // the bytes may not depend on which process, store or map instance
        // answered, only on the records and the request.
        let first = Emulator::empty();
        first.seed(records.clone());
        let second = Emulator::empty();
        second.seed(records.clone());

        let (status, expected) = first.list_raw(&request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {expected}"
        );
        // Where the page reaches the sixteen-entry record, its context leads with
        // its lowest key — so the comparison below is over bytes whose order a
        // per-map hash seed would have scrambled.
        if expected.contains("\"key-00\"") {
            assert!(
                expected.contains("\"context\":{\"key-00\":\"value-0\",\"key-01\":\"value-1\""),
                "case {case:?}: expected the sixteen-entry context to lead with its lowest keys, got {expected}"
            );
        }

        for repeat in 0..4 {
            for (which, emulator) in [("first", &first), ("second", &second)] {
                let (status, body) = emulator.list_raw(&request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} ({which} emulator, repeat {repeat}): expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body, expected,
                    "case {case:?} ({which} emulator, repeat {repeat}): expected byte-identical bytes, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn repeated_reads_of_the_boot_inventory_answer_with_identical_bytes() {
    let emulator = Emulator::new();

    let (status, expected) = emulator.list_raw(&json!({})).await;
    assert_eq!(
        status, 200,
        "case \"boot inventory\": expected 200, got {status} with {expected}"
    );
    assert!(
        expected.contains("\"context\":{"),
        "case \"boot inventory\": expected each record to carry a context object, got {expected}"
    );

    for repeat in 0..4 {
        let (status, body) = emulator.list_raw(&json!({})).await;
        assert_eq!(
            status, 200,
            "case \"boot inventory\" (repeat {repeat}): expected 200, got {status} with {body}"
        );
        assert_eq!(
            body, expected,
            "case \"boot inventory\" (repeat {repeat}): expected byte-identical bytes, got {body}"
        );
    }
}
