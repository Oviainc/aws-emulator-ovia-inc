// SPDX-License-Identifier: CC0-1.0

//! The bill-scenario lifecycle, driven end-to-end over this service's awsJson1_0 wire
//! (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`) through the same
//! `MultiServiceRouter` the production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the
//! rows differ along whatever the rule generalises over (enum value, name shape, entry
//! count, amount count, page size, filter name and match option).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state, ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
    EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// A valid `CostCategoryArn` for this emulator's account: the shape's pattern is
/// `^arn:aws[-a-z0-9]*:ce::[0-9]{12}:costcategory/[a-f0-9-]{36}$` (smithy_query:
/// `shape="CostCategoryArn"`).
const COST_CATEGORY_ARN: &str = "arn:aws:ce::000000000000:costcategory/0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0";

/// A 12-digit `AccountId`, the form `^\d{12}$` requires.
const USAGE_ACCOUNT_ID: &str = "000000000000";

/// The retention window a scenario's `expiresAt` is expected to sit after its
/// `createdAt`, declared HERE rather than read from the implementation so the
/// assertion fails if the handler's window changes.
const EXPECTED_RETENTION_SECS: i64 = 15 * 24 * 60 * 60;

/// One emulator: a single shared engine behind the production router.
///
/// `pub(crate)` so every suite in this test target drives the service the same way:
/// `tests/all.rs` is one crate, and a second copy of this harness could drift from the
/// front door this one exercises.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    /// The same engine the service's operations hold, so a test can age the store the
    /// way the clock would.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests carry an unsigned `Authorization` header: it is here to name
        // the service in its SigV4 credential scope (how the router picks a service),
        // not to be verified.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header(
                "x-amz-target",
                format!("AWSBCMPricingCalculator.{operation}"),
            )
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260101/us-east-1/bcm-pricing-calculator/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=unsigned",
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let payload = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// Create a scenario and return its id, failing the test if the call did not
    /// succeed.
    async fn create_scenario(&self, name: &str) -> String {
        let (status, body) = self.call("CreateBillScenario", json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "CreateBillScenario({name}): expected 200, got {status} with {body}"
        );
        body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateBillScenario({name}): no id in {body}"))
            .to_string()
    }
}

/// Whether `value` is a `ResourceId`: the lowercase-hex v4 UUID form
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` the shape declares.
pub(crate) fn is_resource_id(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    parts.len() == GROUPS.len()
        && parts.iter().zip(GROUPS).all(|(part, width)| {
            part.len() == width
                && part
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
}

/// The `error_code`/`__type` an awsJson1_0 error body carries, without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn seconds(value: &Value, field: &str) -> i64 {
    value[field]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a numeric {field} in {value}")) as i64
}

/// A `UsageAmount` entry: `startHour` is an epoch-second timestamp on awsJson1_0.
fn amount(start_hour: i64, value: f64) -> Value {
    json!({ "startHour": start_hour, "amount": value })
}

/// One row of the create table: the request's shape, as the rule generalises over it.
struct CreateCase {
    name: &'static str,
    preference: Option<&'static str>,
    arn: Option<&'static str>,
    tags: Option<Value>,
}

/// RULE: a created scenario is stored, and every reader answers from that stored
/// scenario — the same id, name, status, timestamps, interval and group-sharing fields.
///
/// The table spans what the rule generalises over: the name's shape, and each
/// `GroupSharingPreferenceEnum` value (smithy_query: `shape="GroupSharingPreferenceEnum"`)
/// including its absence, with and without the cost-category ARN and tags.
#[tokio::test]
async fn a_created_scenario_is_what_every_reader_reports() {
    let cases = vec![
        CreateCase {
            name: "plain",
            preference: None,
            arn: None,
            tags: None,
        },
        CreateCase {
            name: "with-hyphens-and-9-digits",
            preference: Some("OPEN"),
            arn: None,
            tags: None,
        },
        CreateCase {
            name: "MixedCase123",
            preference: Some("PRIORITIZED"),
            arn: Some(COST_CATEGORY_ARN),
            tags: None,
        },
        CreateCase {
            name: "restricted",
            preference: Some("RESTRICTED"),
            arn: Some(COST_CATEGORY_ARN),
            tags: Some(json!({ "team": "finops", "env": "test" })),
        },
        // `BillScenarioName` is `len ..64`: the longest name the model accepts.
        CreateCase {
            name: "a234567890123456789012345678901234567890123456789012345678901234",
            preference: Some("OPEN"),
            arn: None,
            tags: None,
        },
        CreateCase {
            name: "x",
            preference: None,
            arn: None,
            tags: Some(json!({})),
        },
    ];

    let emulator = Emulator::new();

    for CreateCase {
        name,
        preference,
        arn,
        tags,
    } in cases
    {
        let case = format!("name={name} preference={preference:?} arn={}", arn.is_some());

        let mut request = json!({ "name": name });
        if let Some(preference) = preference {
            request["groupSharingPreference"] = json!(preference);
        }
        if let Some(arn) = arn {
            request["costCategoryGroupSharingPreferenceArn"] = json!(arn);
        }
        if let Some(tags) = tags {
            request["tags"] = tags;
        }

        let before = state::now_secs();
        let (status, created) = emulator.call("CreateBillScenario", request).await;
        let after = state::now_secs();
        assert_eq!(
            status, 200,
            "case {case}: expected CreateBillScenario to answer 200, got {status} with {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_resource_id(&id),
            "case {case}: expected a lowercase-hex UUID id, got {id:?}"
        );
        assert_eq!(
            created["name"].as_str(),
            Some(name),
            "case {case}: expected the name back, got {:?}",
            created["name"]
        );
        assert_eq!(
            created["status"].as_str(),
            Some("READY"),
            "case {case}: expected status READY, got {:?}",
            created["status"]
        );
        assert_eq!(
            created["groupSharingPreference"].as_str(),
            preference,
            "case {case}: expected groupSharingPreference {preference:?}, got {:?}",
            created["groupSharingPreference"]
        );
        assert_eq!(
            created["costCategoryGroupSharingPreferenceArn"].as_str(),
            arn,
            "case {case}: expected costCategoryGroupSharingPreferenceArn {arn:?}, got {:?}",
            created["costCategoryGroupSharingPreferenceArn"]
        );

        let created_at = seconds(&created, "createdAt");
        assert!(
            (before..=after).contains(&created_at),
            "case {case}: expected createdAt within [{before}, {after}], got {created_at}"
        );
        let expires_at = seconds(&created, "expiresAt");
        assert_eq!(
            expires_at,
            created_at + EXPECTED_RETENTION_SECS,
            "case {case}: expected expiresAt one retention window after createdAt, got {expires_at}"
        );

        // The bill interval is the UTC calendar month the create call fell in: both
        // bounds sit on a midnight boundary, the creation instant is inside it, and a
        // calendar month is 28..31 days long.
        let interval_start = seconds(&created["billInterval"], "start");
        let interval_end = seconds(&created["billInterval"], "end");
        assert_eq!(
            interval_start % 86_400,
            0,
            "case {case}: expected billInterval.start on a midnight boundary, got {interval_start}"
        );
        assert_eq!(
            interval_end % 86_400,
            0,
            "case {case}: expected billInterval.end on a midnight boundary, got {interval_end}"
        );
        assert!(
            (interval_start..interval_end).contains(&created_at),
            "case {case}: expected createdAt {created_at} inside [{interval_start}, {interval_end})"
        );
        let days = (interval_end - interval_start) / 86_400;
        assert!(
            (28..=31).contains(&days),
            "case {case}: expected a calendar month of 28..31 days, got {days}"
        );

        // GetBillScenario answers from the same stored scenario.
        let (status, fetched) = emulator
            .call("GetBillScenario", json!({ "identifier": id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetBillScenario to answer 200, got {status} with {fetched}"
        );
        for field in [
            "id",
            "name",
            "status",
            "createdAt",
            "expiresAt",
            "billInterval",
            "groupSharingPreference",
            "costCategoryGroupSharingPreferenceArn",
        ] {
            assert_eq!(
                fetched[field], created[field],
                "case {case}: expected GetBillScenario to report the created {field} {:?}, got {:?}",
                created[field], fetched[field]
            );
        }

        // And so does ListBillScenarios, for the same scenario's summary.
        let (status, listed) = emulator.call("ListBillScenarios", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case}: expected ListBillScenarios to answer 200, got {status} with {listed}"
        );
        let summary = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {listed}"))
            .iter()
            .find(|item| item["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case}: {id} missing from {listed}"));
        for field in ["name", "status", "createdAt", "expiresAt", "billInterval"] {
            assert_eq!(
                summary[field], created[field],
                "case {case}: expected the summary's {field} to be {:?}, got {:?}",
                created[field], summary[field]
            );
        }
    }
}

/// RULE: a replayed `@idempotencyToken` answers with the resource the first call
/// created; it never mints a second one.
///
/// The table spans the token's shape (`ClientToken` is `len 1..64`, pattern
/// `^[\u0021-\u007E]+$`) and the number of replays.
#[tokio::test]
async fn replaying_a_client_token_returns_the_first_scenario() {
    let cases: [(&str, usize); 4] = [
        ("t", 1),
        ("plain-token-1", 2),
        ("~punctuation!@#$%^&*()_+", 1),
        (
            "0123456789012345678901234567890123456789012345678901234567890123",
            3,
        ),
    ];

    let emulator = Emulator::new();
    let mut expected_count = 0usize;

    for (token, replays) in cases {
        let request = json!({ "name": "idempotent", "clientToken": token });
        let (status, first) = emulator
            .call("CreateBillScenario", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case token={token}: expected 200, got {status} with {first}"
        );
        expected_count += 1;

        for replay in 0..replays {
            let (status, again) = emulator
                .call("CreateBillScenario", request.clone())
                .await;
            assert_eq!(
                status, 200,
                "case token={token} replay={replay}: expected 200, got {status} with {again}"
            );
            assert_eq!(
                again["id"], first["id"],
                "case token={token} replay={replay}: expected the first id {:?}, got {:?}",
                first["id"], again["id"]
            );
            assert_eq!(
                again["createdAt"], first["createdAt"],
                "case token={token} replay={replay}: expected the first createdAt {:?}, got {:?}",
                first["createdAt"], again["createdAt"]
            );
        }

        let (_, listed) = emulator
            .call("ListBillScenarios", json!({ "maxResults": 25 }))
            .await;
        let count = listed["items"].as_array().map_or(0, Vec::len);
        assert_eq!(
            count, expected_count,
            "case token={token}: expected {expected_count} stored scenarios, got {count} in {listed}"
        );
    }
}

/// RULE: a batch create mints one modification per entry, stores it under the named
/// scenario, and the list reader reports exactly those rows with their assigned ids —
/// one `quantity` per requested `amount`.
///
/// The table spans the entry count (1 up to the modelled maximum of 25) and the number
/// of `amounts` an entry carries, including none.
#[tokio::test]
async fn batch_created_modifications_are_what_the_list_reader_reports() {
    let cases: [(usize, usize); 5] = [(1, 1), (2, 0), (3, 3), (5, 2), (25, 1)];

    for (entries, amounts) in cases {
        let case = format!("entries={entries} amounts={amounts}");
        let emulator = Emulator::new();
        let scenario = emulator.create_scenario("batch").await;

        let usage_modifications: Vec<Value> = (0..entries)
            .map(|index| {
                json!({
                    "key": format!("k{index}"),
                    "serviceCode": "AmazonEC2",
                    "usageType": format!("BoxUsage:m5.{index}xlarge"),
                    "operation": "RunInstances",
                    "usageAccountId": USAGE_ACCOUNT_ID,
                    "group": "group-a",
                    "amounts": (0..amounts)
                        .map(|hour| amount(1_760_000_000 + 3_600 * hour as i64, index as f64 + 1.0))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();

        let (status, batch) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                json!({
                    "billScenarioId": scenario,
                    "usageModifications": usage_modifications,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {batch}"
        );

        let created = batch["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {batch}"))
            .clone();
        assert_eq!(
            created.len(),
            entries,
            "case {case}: expected one item per entry, got {} in {batch}",
            created.len()
        );
        assert_eq!(
            batch["errors"].as_array().map_or(0, Vec::len),
            0,
            "case {case}: expected no per-entry errors, got {:?}",
            batch["errors"]
        );

        for (index, item) in created.iter().enumerate() {
            let id = item["id"].as_str().unwrap_or_default();
            assert!(
                is_resource_id(id),
                "case {case} entry {index}: expected a lowercase-hex UUID id, got {id:?}"
            );
            assert_eq!(
                item["key"].as_str(),
                Some(format!("k{index}").as_str()),
                "case {case} entry {index}: expected the entry's key back, got {:?}",
                item["key"]
            );
            let quantities = item["quantities"].as_array().map_or(0, Vec::len);
            assert_eq!(
                quantities, amounts,
                "case {case} entry {index}: expected one quantity per amount, got {quantities}"
            );
        }

        // Every created row comes back from the list reader, in creation order, with
        // the id it was assigned.
        let (status, listed) = emulator
            .call(
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": scenario, "maxResults": 25 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected ListBillScenarioUsageModifications 200, got {status} with {listed}"
        );
        let rows = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {listed}"));
        assert_eq!(
            rows.len(),
            entries,
            "case {case}: expected {entries} persisted modifications, got {} in {listed}",
            rows.len()
        );
        for (index, (row, item)) in rows.iter().zip(created.iter()).enumerate() {
            for field in [
                "id",
                "serviceCode",
                "usageType",
                "operation",
                "usageAccountId",
                "group",
                "quantities",
            ] {
                assert_eq!(
                    row[field], item[field],
                    "case {case} row {index}: expected {field} {:?}, got {:?}",
                    item[field], row[field]
                );
            }
        }
    }
}

/// RULE: an entry that fails on its own is reported in `errors` against its `key`,
/// while the other entries of the same request are still created.
///
/// The model makes `key` the identity of an entry within a request (`BatchCreate...Entry$key`:
/// "This key is useful to identify errors associated with any usage entry"), so two
/// entries sharing one key cannot both be answered: the later one conflicts.
#[tokio::test]
async fn a_duplicate_entry_key_fails_only_that_entry() {
    let cases: [(&[&str], usize, usize); 4] = [
        (&["a", "b", "c"], 3, 0),
        (&["a", "a"], 1, 1),
        (&["a", "b", "a", "b"], 2, 2),
        (&["dup", "dup", "dup"], 1, 2),
    ];

    for (keys, expected_items, expected_errors) in cases {
        let case = format!("keys={keys:?}");
        let emulator = Emulator::new();
        let scenario = emulator.create_scenario("partial").await;

        let usage_modifications: Vec<Value> = keys
            .iter()
            .map(|key| {
                json!({
                    "key": key,
                    "serviceCode": "AWSKMS",
                    "usageType": "Requests",
                    "operation": "Decrypt",
                    "usageAccountId": USAGE_ACCOUNT_ID,
                    "amounts": [amount(1_760_000_000, 10.0)],
                })
            })
            .collect();

        let (status, batch) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                json!({ "billScenarioId": scenario, "usageModifications": usage_modifications }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: a per-entry failure is not a request failure; got {status} with {batch}"
        );
        assert_eq!(
            batch["items"].as_array().map_or(0, Vec::len),
            expected_items,
            "case {case}: expected {expected_items} created items, got {:?}",
            batch["items"]
        );
        let errors = batch["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no errors list in {batch}"));
        assert_eq!(
            errors.len(),
            expected_errors,
            "case {case}: expected {expected_errors} per-entry errors, got {errors:?}"
        );
        for error in errors {
            assert!(
                error["key"]
                    .as_str()
                    .is_some_and(|key| keys.contains(&key)),
                "case {case}: expected the error keyed by a request entry, got {error}"
            );
            assert_eq!(
                error["errorCode"].as_str(),
                Some("CONFLICT"),
                "case {case}: expected errorCode CONFLICT, got {:?}",
                error["errorCode"]
            );
        }

        // Only the accepted entries were persisted.
        let (_, listed) = emulator
            .call(
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": scenario, "maxResults": 25 }),
            )
            .await;
        assert_eq!(
            listed["items"].as_array().map_or(0, Vec::len),
            expected_items,
            "case {case}: expected {expected_items} persisted modifications, got {listed}"
        );
    }
}

/// RULE: `maxResults` is a page size and `nextToken` is emitted only while items
/// remain, so paginating to the end yields every item exactly once, in order.
///
/// The table spans the collection size against the page size, including the member's
/// published bounds (1 and 25) and the absent case, which uses the modelled maximum.
#[tokio::test]
async fn paging_a_scenario_list_yields_every_item_once() {
    let cases: [(usize, Option<i32>); 7] = [
        (0, None),
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (7, Some(25)),
        (26, None),
        (30, Some(25)),
    ];

    for (total, page_size) in cases {
        let case = format!("total={total} maxResults={page_size:?}");
        let emulator = Emulator::new();
        let mut expected = Vec::new();
        for index in 0..total {
            expected.push(emulator.create_scenario(&format!("s{index}")).await);
        }

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({});
            if let Some(page_size) = page_size {
                request["maxResults"] = json!(page_size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, listed) = emulator.call("ListBillScenarios", request).await;
            assert_eq!(
                status, 200,
                "case {case} page {pages}: expected 200, got {status} with {listed}"
            );
            let items = listed["items"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case} page {pages}: no items in {listed}"));
            let limit = page_size.unwrap_or(25) as usize;
            assert!(
                items.len() <= limit,
                "case {case} page {pages}: expected at most {limit} items, got {}",
                items.len()
            );
            for item in items {
                seen.push(item["id"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match listed["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case}: expected every scenario once in creation order, got {seen:?}"
        );
    }
}

/// RULE: a `nextToken` this service did not issue FOR THIS REQUEST is refused as
/// invalid input — never read as an offset into a different list and answered with a
/// silently wrong page.
///
/// The table covers a token that is not one of ours at all, and a real token of ours
/// replayed against a request it was not measured against.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let emulator = Emulator::new();
    let scenario = emulator.create_scenario("tokens").await;
    emulator.create_scenario("second").await;

    // A real token: the first of two pages of scenarios.
    let (status, page) = emulator
        .call("ListBillScenarios", json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "seeding a real token: expected 200, got {status} with {page}"
    );
    let real = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("seeding a real token: no nextToken in {page}"))
        .to_string();

    let foreign = [
        "not-hex",
        "00",
        "6f66667365743a30",
        "zzzz",
        "62636d70632d6f66667365743a78",
    ];

    for token in foreign {
        for (operation, request) in [
            ("ListBillScenarios", json!({ "nextToken": token })),
            (
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": scenario, "nextToken": token }),
            ),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {operation} token={token}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {operation} token={token}: expected ValidationException, got {body}"
            );
        }
    }

    // The same REAL token, replayed against requests it was not measured against.
    for (case, operation, request) in [
        (
            "another operation",
            "ListBillScenarioUsageModifications",
            json!({ "billScenarioId": scenario, "nextToken": real }),
        ),
        (
            "another filter set",
            "ListBillScenarios",
            json!({
                "maxResults": 1,
                "nextToken": real,
                "filters": [{ "name": "STATUS", "values": ["READY"] }],
            }),
        ),
    ] {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
    }

    // And it still works for the request it WAS measured against.
    let (status, body) = emulator
        .call(
            "ListBillScenarios",
            json!({ "maxResults": 1, "nextToken": real }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the token's own request: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["items"].as_array().map_or(0, Vec::len),
        1,
        "the token's own request: expected the second page, got {body}"
    );
}

/// RULE: a page size outside the member's declared `range 1..25` is refused before any
/// item is read.
#[tokio::test]
async fn a_page_size_outside_the_declared_range_is_refused() {
    let emulator = Emulator::new();
    emulator.create_scenario("bounds").await;

    for (page_size, expected) in [(0, 400), (1, 200), (25, 200), (26, 400), (-1, 400)] {
        let (status, body) = emulator
            .call("ListBillScenarios", json!({ "maxResults": page_size }))
            .await;
        assert_eq!(
            status, expected,
            "case maxResults={page_size}: expected {expected}, got {status} with {body}"
        );
    }
}

/// RULE: a `filters` entry keeps only the items whose value for the attribute the
/// filter names satisfies the filter's values under its `matchOption`; several filters
/// are AND-ed and the values of one filter are OR-ed.
///
/// The table covers each `ListBillScenariosFilterName` (smithy_query:
/// `shape="ListBillScenariosFilterName"`) against each `MatchOption`.
#[tokio::test]
async fn scenario_filters_select_on_the_named_attribute() {
    let emulator = Emulator::new();

    let (_, alpha) = emulator
        .call(
            "CreateBillScenario",
            json!({ "name": "alpha-one", "groupSharingPreference": "OPEN" }),
        )
        .await;
    let (_, beta) = emulator
        .call(
            "CreateBillScenario",
            json!({
                "name": "beta-two",
                "groupSharingPreference": "PRIORITIZED",
                "costCategoryGroupSharingPreferenceArn": COST_CATEGORY_ARN,
            }),
        )
        .await;
    let alpha_id = alpha["id"].as_str().unwrap_or_default().to_string();
    let beta_id = beta["id"].as_str().unwrap_or_default().to_string();

    let cases: Vec<(Value, Vec<&String>)> = vec![
        (
            json!([{ "name": "NAME", "values": ["alpha-one"] }]),
            vec![&alpha_id],
        ),
        (
            json!([{ "name": "NAME", "values": ["alpha"], "matchOption": "STARTS_WITH" }]),
            vec![&alpha_id],
        ),
        (
            json!([{ "name": "NAME", "values": ["-tw"], "matchOption": "CONTAINS" }]),
            vec![&beta_id],
        ),
        (
            json!([{ "name": "NAME", "values": ["alpha-one", "beta-two"] }]),
            vec![&alpha_id, &beta_id],
        ),
        (
            json!([{ "name": "STATUS", "values": ["READY"] }]),
            vec![&alpha_id, &beta_id],
        ),
        (json!([{ "name": "STATUS", "values": ["FAILED"] }]), vec![]),
        (
            json!([{ "name": "GROUP_SHARING_PREFERENCE", "values": ["PRIORITIZED"] }]),
            vec![&beta_id],
        ),
        (
            json!([{ "name": "COST_CATEGORY_ARN", "values": [COST_CATEGORY_ARN] }]),
            vec![&beta_id],
        ),
        (
            json!([
                { "name": "STATUS", "values": ["READY"] },
                { "name": "NAME", "values": ["beta"], "matchOption": "STARTS_WITH" },
            ]),
            vec![&beta_id],
        ),
        (
            json!([
                { "name": "STATUS", "values": ["READY"] },
                { "name": "GROUP_SHARING_PREFERENCE", "values": ["RESTRICTED"] },
            ]),
            vec![],
        ),
    ];

    for (filters, expected) in cases {
        let case = filters.to_string();
        let (status, listed) = emulator
            .call("ListBillScenarios", json!({ "filters": filters }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        let got: Vec<String> = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {listed}"))
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let want: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            got, want,
            "case {case}: expected {want:?}, got {got:?}"
        );
    }
}

/// RULE: the same filtering rule holds for a scenario's usage modifications, over the
/// attribute each `ListUsageFilterName` names — including the `HISTORICAL_*` half,
/// which reads the modification's `historicalUsage`.
#[tokio::test]
async fn usage_modification_filters_select_on_the_named_attribute() {
    let emulator = Emulator::new();
    let scenario = emulator.create_scenario("usagefilters").await;

    let (status, batch) = emulator
        .call(
            "BatchCreateBillScenarioUsageModification",
            json!({
                "billScenarioId": scenario,
                "usageModifications": [
                    {
                        "key": "ec2",
                        "serviceCode": "AmazonEC2",
                        "usageType": "BoxUsage:m5.large",
                        "operation": "RunInstances",
                        "usageAccountId": USAGE_ACCOUNT_ID,
                        "group": "compute",
                        "amounts": [amount(1_760_000_000, 24.0)],
                    },
                    {
                        "key": "kms",
                        "serviceCode": "AWSKMS",
                        "usageType": "Requests",
                        "operation": "Decrypt",
                        "usageAccountId": "000000000001",
                        "group": "security",
                        "historicalUsage": {
                            "serviceCode": "AWSKMS",
                            "usageType": "Requests",
                            "operation": "Encrypt",
                            "location": "US East (N. Virginia)",
                            "usageAccountId": "000000000001",
                            "billInterval": { "start": 1_757_000_000, "end": 1_759_000_000 },
                            "filterExpression": {},
                        },
                    },
                ],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding the modifications: expected 200, got {status} with {batch}"
    );
    let ec2_id = batch["items"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let kms_id = batch["items"][1]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let cases: Vec<(Value, Vec<&String>)> = vec![
        (
            json!([{ "name": "SERVICE_CODE", "values": ["AmazonEC2"] }]),
            vec![&ec2_id],
        ),
        (
            json!([{ "name": "USAGE_TYPE", "values": ["BoxUsage"], "matchOption": "STARTS_WITH" }]),
            vec![&ec2_id],
        ),
        (
            json!([{ "name": "OPERATION", "values": ["crypt"], "matchOption": "CONTAINS" }]),
            vec![&kms_id],
        ),
        (
            json!([{ "name": "USAGE_ACCOUNT_ID", "values": ["000000000001"] }]),
            vec![&kms_id],
        ),
        (
            json!([{ "name": "USAGE_GROUP", "values": ["compute", "security"] }]),
            vec![&ec2_id, &kms_id],
        ),
        (
            // `location` is only resolvable from the pricing catalog this emulator does
            // not carry, so nothing stores one and the filter matches nothing. Revisit
            // this row if a pricing location is ever stored.
            json!([{ "name": "LOCATION", "values": ["US East (N. Virginia)"] }]),
            vec![],
        ),
        (
            json!([{ "name": "HISTORICAL_OPERATION", "values": ["Encrypt"] }]),
            vec![&kms_id],
        ),
        (
            json!([{ "name": "HISTORICAL_SERVICE_CODE", "values": ["AWSKMS"] }]),
            vec![&kms_id],
        ),
        (
            json!([{ "name": "HISTORICAL_LOCATION", "values": ["US East"], "matchOption": "STARTS_WITH" }]),
            vec![&kms_id],
        ),
        (
            json!([
                { "name": "SERVICE_CODE", "values": ["AWSKMS"] },
                { "name": "HISTORICAL_USAGE_TYPE", "values": ["Requests"] },
            ]),
            vec![&kms_id],
        ),
    ];

    for (filters, expected) in cases {
        let case = filters.to_string();
        let (status, listed) = emulator
            .call(
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": scenario, "filters": filters }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        let got: Vec<String> = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {listed}"))
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let want: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(got, want, "case {case}: expected {want:?}, got {got:?}");
    }
}

/// RULE: an operation that names a bill scenario it cannot find answers
/// `ResourceNotFoundException` (HTTP 404) — never a 200 with an empty body.
///
/// The table spans the three ways an id can be absent (never created, deleted, and a
/// well-formed id of another shape) across every reader and writer that resolves one.
#[tokio::test]
async fn an_absent_scenario_is_a_not_found_for_every_operation_that_resolves_one() {
    let emulator = Emulator::new();

    let deleted = emulator.create_scenario("doomed").await;
    let (status, body) = emulator
        .call("DeleteBillScenario", json!({ "identifier": deleted }))
        .await;
    assert_eq!(
        status, 200,
        "deleting a live scenario: expected 200, got {status} with {body}"
    );

    let absent = [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        deleted.as_str(),
    ];

    for id in absent {
        for (operation, request) in [
            ("GetBillScenario", json!({ "identifier": id })),
            (
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": id }),
            ),
            (
                "BatchCreateBillScenarioUsageModification",
                json!({
                    "billScenarioId": id,
                    "usageModifications": [{
                        "key": "k1",
                        "serviceCode": "AmazonEC2",
                        "usageType": "BoxUsage:m5.large",
                        "operation": "RunInstances",
                        "usageAccountId": USAGE_ACCOUNT_ID,
                    }],
                }),
            ),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 404,
                "case {operation} id={id}: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} id={id}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(id),
                "case {operation} id={id}: expected the error to name the resource, got {body}"
            );
        }
    }
}

/// RULE: a delete removes the scenario and everything stored under it, and — being
/// `@idempotent` with no not-found error in its model — repeating it still succeeds.
#[tokio::test]
async fn deleting_a_scenario_removes_it_and_repeats_cleanly() {
    let emulator = Emulator::new();

    // Two scenarios, so the delete is shown to remove one WITHOUT disturbing the other.
    let kept = emulator.create_scenario("kept").await;
    let doomed = emulator.create_scenario("doomed").await;

    for scenario in [&kept, &doomed] {
        let (status, batch) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                json!({
                    "billScenarioId": scenario,
                    "usageModifications": [{
                        "key": "k1",
                        "serviceCode": "AmazonEC2",
                        "usageType": "BoxUsage:m5.large",
                        "operation": "RunInstances",
                        "usageAccountId": USAGE_ACCOUNT_ID,
                        "amounts": [amount(1_760_000_000, 1.0)],
                    }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "seeding {scenario}: expected 200, got {status} with {batch}"
        );
    }

    for attempt in 0..3 {
        let (status, body) = emulator
            .call("DeleteBillScenario", json!({ "identifier": doomed }))
            .await;
        assert_eq!(
            status, 200,
            "case attempt={attempt}: expected an idempotent delete to answer 200, got {status} with {body}"
        );
    }

    let (status, body) = emulator
        .call("GetBillScenario", json!({ "identifier": doomed }))
        .await;
    assert_eq!(
        status, 404,
        "after delete: expected 404 from GetBillScenario, got {status} with {body}"
    );

    let (status, listed) = emulator.call("ListBillScenarios", json!({})).await;
    assert_eq!(
        status, 200,
        "after delete: expected 200 from ListBillScenarios, got {status} with {listed}"
    );
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .unwrap_or_else(|| panic!("after delete: no items in {listed}"))
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![kept.as_str()],
        "after delete: expected only the kept scenario, got {ids:?}"
    );

    // The surviving scenario kept its own modifications.
    let (status, listed) = emulator
        .call(
            "ListBillScenarioUsageModifications",
            json!({ "billScenarioId": kept }),
        )
        .await;
    assert_eq!(
        status, 200,
        "after delete: expected 200 for the kept scenario's modifications, got {status} with {listed}"
    );
    assert_eq!(
        listed["items"].as_array().map_or(0, Vec::len),
        1,
        "after delete: expected the kept scenario's modification to survive, got {listed}"
    );
}

/// RULE: the retention window a scenario advertises through `expiresAt` is ENFORCED, not
/// merely reported — once that instant has passed the scenario is gone from every
/// reader, and a scenario still inside its window is untouched.
///
/// The table spans how far past (or short of) the expiry the clock has moved, including
/// the boundary instant itself. The store is aged directly, the way the clock would:
/// the handlers store the due instant and never sleep for it.
#[tokio::test]
async fn a_scenario_past_its_expiry_is_gone_from_every_reader() {
    let cases: [(i64, bool); 5] = [
        (-1, false),
        (0, false),
        (-3_600, false),
        (-30 * 24 * 60 * 60, false),
        (60, true),
    ];

    for (offset, expected_live) in cases {
        let case = format!("expiresAt = now{offset:+}");
        let emulator = Emulator::new();
        let untouched = emulator.create_scenario("untouched").await;
        let aged = emulator.create_scenario("aged").await;

        {
            let store = emulator.engine.state::<ServiceState>();
            let mut guard = store.lock();
            let scenario = guard
                .bill_scenario_mut(&aged)
                .unwrap_or_else(|| panic!("case {case}: {aged} was not stored"));
            scenario.expires_at =
                aws_smithy_types::DateTime::from_secs(state::now_secs() + offset);
        }

        let (status, body) = emulator
            .call("GetBillScenario", json!({ "identifier": aged }))
            .await;
        let expected_status = if expected_live { 200 } else { 404 };
        assert_eq!(
            status, expected_status,
            "case {case}: expected GetBillScenario {expected_status}, got {status} with {body}"
        );

        let (status, listed) = emulator
            .call("ListBillScenarios", json!({ "maxResults": 25 }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected ListBillScenarios 200, got {status} with {listed}"
        );
        let ids: Vec<&str> = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {listed}"))
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            ids.contains(&aged.as_str()),
            expected_live,
            "case {case}: expected the aged scenario listed={expected_live}, got {ids:?}"
        );
        assert!(
            ids.contains(&untouched.as_str()),
            "case {case}: expected the in-window scenario to survive, got {ids:?}"
        );

        // Expiring a scenario must not strand its idempotency record either.
        let store = emulator.engine.state::<ServiceState>();
        let tracked = store.lock().idempotency.len();
        assert_eq!(
            tracked, 0,
            "case {case}: expected no idempotency records for token-less creates, got {tracked}"
        );
    }
}

/// RULE: a create that would take the store past its quota is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) instead of growing the store without
/// bound; everything already stored is still readable.
#[tokio::test]
async fn creating_past_the_scenario_quota_is_refused() {
    let emulator = Emulator::new();

    for index in 0..state::MAX_BILL_SCENARIOS {
        let (status, body) = emulator
            .call("CreateBillScenario", json!({ "name": format!("s{index}") }))
            .await;
        assert_eq!(
            status, 200,
            "case index={index}: expected 200 below the quota, got {status} with {body}"
        );
    }

    // Every create from the quota onwards is refused, and refused the same way.
    for attempt in 0..3 {
        let (status, body) = emulator
            .call("CreateBillScenario", json!({ "name": "overflow" }))
            .await;
        assert_eq!(
            status, 402,
            "case attempt={attempt}: expected 402 at the quota, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case attempt={attempt}: expected ServiceQuotaExceededException, got {body}"
        );
    }

    // The store is unchanged by the refusals.
    let stored = emulator
        .engine
        .state::<ServiceState>()
        .lock()
        .bill_scenarios
        .len();
    assert_eq!(
        stored,
        state::MAX_BILL_SCENARIOS,
        "expected the store to hold exactly the quota, got {stored}"
    );
}

/// RULE: the same quota rule holds for the usage modifications of ONE scenario — a
/// batch that would take it past the limit is refused with HTTP 402 and stores nothing.
#[tokio::test]
async fn batch_creating_past_the_modification_quota_is_refused() {
    let emulator = Emulator::new();
    let scenario = emulator.create_scenario("quota").await;

    let batch = |offset: usize, count: usize| {
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "key": format!("k{}", offset + index),
                    "serviceCode": "AmazonEC2",
                    "usageType": "BoxUsage:m5.large",
                    "operation": "RunInstances",
                    "usageAccountId": USAGE_ACCOUNT_ID,
                })
            })
            .collect();
        json!({ "billScenarioId": scenario, "usageModifications": entries })
    };

    // `BatchCreate...Entry` lists are `len 1..25`, so fill the scenario 25 at a time.
    let page = 25usize;
    let mut stored = 0usize;
    while stored + page <= state::MAX_USAGE_MODIFICATIONS_PER_BILL_SCENARIO {
        let (status, body) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                batch(stored, page),
            )
            .await;
        assert_eq!(
            status, 200,
            "case stored={stored}: expected 200 below the quota, got {status} with {body}"
        );
        stored += page;
    }

    for count in [1usize, 25] {
        let (status, body) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                batch(stored, count),
            )
            .await;
        assert_eq!(
            status, 402,
            "case count={count}: expected 402 at the quota, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case count={count}: expected ServiceQuotaExceededException, got {body}"
        );
    }

    let (status, listed) = emulator
        .call(
            "ListBillScenarioUsageModifications",
            json!({ "billScenarioId": scenario, "maxResults": 1 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "after the refusals: expected 200, got {status} with {listed}"
    );
    assert!(
        listed["nextToken"].is_string(),
        "after the refusals: expected the stored modifications to still paginate, got {listed}"
    );
}

/// RULE: a replayed `@idempotencyToken` reproduces the FIRST call's answer, including
/// the per-entry errors it reported — not a cleaner one.
#[tokio::test]
async fn replaying_a_batch_reproduces_its_per_entry_errors() {
    let cases: [&[&str]; 3] = [&["a", "a"], &["a", "b", "a"], &["d", "d", "d"]];

    for keys in cases {
        let case = format!("keys={keys:?}");
        let emulator = Emulator::new();
        let scenario = emulator.create_scenario("replaybatch").await;

        let request = json!({
            "billScenarioId": scenario,
            "clientToken": "token-1",
            "usageModifications": keys
                .iter()
                .map(|key| json!({
                    "key": key,
                    "serviceCode": "AWSKMS",
                    "usageType": "Requests",
                    "operation": "Decrypt",
                    "usageAccountId": USAGE_ACCOUNT_ID,
                    "amounts": [amount(1_760_000_000, 5.0)],
                }))
                .collect::<Vec<_>>(),
        });

        let (status, first) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                request.clone(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {first}"
        );

        for replay in 0..2 {
            let (status, again) = emulator
                .call(
                    "BatchCreateBillScenarioUsageModification",
                    request.clone(),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case} replay={replay}: expected 200, got {status} with {again}"
            );
            assert_eq!(
                again["items"], first["items"],
                "case {case} replay={replay}: expected the first items {:?}, got {:?}",
                first["items"], again["items"]
            );
            assert_eq!(
                again["errors"], first["errors"],
                "case {case} replay={replay}: expected the first errors {:?}, got {:?}",
                first["errors"], again["errors"]
            );
        }

        // And nothing was stored twice.
        let (_, listed) = emulator
            .call(
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": scenario, "maxResults": 25 }),
            )
            .await;
        assert_eq!(
            listed["items"].as_array().map_or(0, Vec::len),
            first["items"].as_array().map_or(0, Vec::len),
            "case {case}: a replay stored a second set of modifications: {listed}"
        );
    }
}

/// RULE: an idempotency record never outlives the resources it names. Once they are
/// gone the record is gone with them, so the same token mints a fresh resource instead
/// of replaying one that no longer exists.
///
/// The table removes the recorded resources the two ways they can go: the clock reaching
/// their `expiresAt`, and an explicit delete.
#[tokio::test]
async fn an_idempotency_record_does_not_outlive_its_resources() {
    for removal in ["expiry", "delete"] {
        let emulator = Emulator::new();
        let token = "record-token";

        let (status, first) = emulator
            .call(
                "CreateBillScenario",
                json!({ "name": "tracked", "clientToken": token }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {removal}: expected CreateBillScenario 200, got {status} with {first}"
        );
        let scenario = first["id"].as_str().unwrap_or_default().to_string();

        let (status, batch) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                json!({
                    "billScenarioId": scenario,
                    "clientToken": token,
                    "usageModifications": [{
                        "key": "k1",
                        "serviceCode": "AmazonEC2",
                        "usageType": "BoxUsage:m5.large",
                        "operation": "RunInstances",
                        "usageAccountId": USAGE_ACCOUNT_ID,
                        "amounts": [amount(1_760_000_000, 1.0)],
                    }],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {removal}: expected the batch create 200, got {status} with {batch}"
        );
        let modification = batch["items"][0]["id"].as_str().unwrap_or_default();
        assert!(
            is_resource_id(modification),
            "case {removal}: expected a modification id, got {modification:?}"
        );

        // Both tokens are recorded while their resources exist: the scenario's, and
        // the batch's (which names the modification id, not the scenario's).
        let tracked = emulator
            .engine
            .state::<ServiceState>()
            .lock()
            .idempotency
            .len();
        assert_eq!(
            tracked, 2,
            "case {removal}: expected one record per idempotent request, got {tracked}"
        );

        if removal == "expiry" {
            let store = emulator.engine.state::<ServiceState>();
            let mut guard = store.lock();
            let aged = guard
                .bill_scenario_mut(&scenario)
                .unwrap_or_else(|| panic!("case {removal}: {scenario} was not stored"));
            aged.expires_at = aws_smithy_types::DateTime::from_secs(state::now_secs() - 1);
        } else {
            let (status, body) = emulator
                .call("DeleteBillScenario", json!({ "identifier": scenario }))
                .await;
            assert_eq!(
                status, 200,
                "case {removal}: expected the delete to answer 200, got {status} with {body}"
            );
        }

        // Reading is enough to materialise an expiry the clock already owed.
        let (status, body) = emulator
            .call("GetBillScenario", json!({ "identifier": scenario }))
            .await;
        assert_eq!(
            status, 404,
            "case {removal}: expected the scenario gone, got {status} with {body}"
        );

        let remaining = emulator
            .engine
            .state::<ServiceState>()
            .lock()
            .idempotency
            .len();
        assert_eq!(
            remaining, 0,
            "case {removal}: expected every record pruned with its resources, got {remaining}"
        );

        // So the token mints a fresh scenario rather than replaying the vanished one.
        let (status, again) = emulator
            .call(
                "CreateBillScenario",
                json!({ "name": "tracked", "clientToken": token }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {removal}: expected CreateBillScenario 200, got {status} with {again}"
        );
        let fresh = again["id"].as_str().unwrap_or_default();
        assert!(
            is_resource_id(fresh) && fresh != scenario,
            "case {removal}: expected a fresh id, got {fresh:?} (was {scenario})"
        );
    }
}

/// RULE: a batch `@idempotencyToken` answers only the request it was made for — the
/// same token carrying a DIFFERENT request (another bill scenario, other entries) is a
/// conflict, not a replay, and leaves the record that answers the first request intact.
#[tokio::test]
async fn a_batch_token_answers_only_the_request_it_was_made_for() {
    let emulator = Emulator::new();
    let token = "shared-token";
    let left = emulator.create_scenario("left").await;
    let right = emulator.create_scenario("right").await;

    let request = |scenario: &str, key: &str, historical: bool| {
        let mut entry = json!({
            "key": key,
            "serviceCode": "AWSKMS",
            "usageType": "Requests",
            "operation": "Decrypt",
            "usageAccountId": USAGE_ACCOUNT_ID,
            "amounts": [amount(1_760_000_000, 3.0)],
        });
        if historical {
            entry["historicalUsage"] = json!({
                "serviceCode": "AWSKMS",
                "usageType": "Requests",
                "operation": "Encrypt",
                "usageAccountId": USAGE_ACCOUNT_ID,
                "billInterval": { "start": 1_757_000_000, "end": 1_759_000_000 },
                "filterExpression": {},
            });
        }
        json!({
            "billScenarioId": scenario,
            "clientToken": token,
            "usageModifications": [entry],
        })
    };

    let (status, first) = emulator
        .call(
            "BatchCreateBillScenarioUsageModification",
            request(&left, "k1", false),
        )
        .await;
    assert_eq!(
        status, 200,
        "the first batch: expected 200, got {status} with {first}"
    );
    let created = first["items"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Each row is the same token carrying a request the first call did not make.
    for (case, body) in [
        ("another scenario", request(&right, "k1", false)),
        ("another entry key", request(&left, "k2", false)),
        ("added historicalUsage", request(&left, "k1", true)),
    ] {
        let (status, body) = emulator
            .call("BatchCreateBillScenarioUsageModification", body)
            .await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case}: expected ConflictException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(created.as_str()),
            "case {case}: expected the conflict to name the resource the token created, got {body}"
        );
    }

    // The record still answers the request it was made for.
    let (status, replay) = emulator
        .call(
            "BatchCreateBillScenarioUsageModification",
            request(&left, "k1", false),
        )
        .await;
    assert_eq!(
        status, 200,
        "the replay: expected 200, got {status} with {replay}"
    );
    assert_eq!(
        replay["items"], first["items"],
        "expected the replay to answer the first items {:?}, got {:?}",
        first["items"], replay["items"]
    );

    // And the refusals stored nothing: one modification on `left`, none on `right`.
    for (scenario, expected) in [(&left, 1usize), (&right, 0)] {
        let (status, listed) = emulator
            .call(
                "ListBillScenarioUsageModifications",
                json!({ "billScenarioId": scenario, "maxResults": 25 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case scenario={scenario}: expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed["items"].as_array().map_or(0, Vec::len),
            expected,
            "case scenario={scenario}: expected {expected} modifications, got {listed}"
        );
    }
}

/// RULE: the same rule holds for `CreateBillScenario` — a token replayed with a request
/// the first call did not make is a conflict, and no second scenario is created.
///
/// The table spans each member of the request the token is scoped to.
#[tokio::test]
async fn a_create_token_answers_only_the_request_it_was_made_for() {
    let emulator = Emulator::new();
    let token = "create-token";
    let original = json!({
        "name": "original",
        "clientToken": token,
        "groupSharingPreference": "OPEN",
        "tags": { "team": "finops" },
    });

    let (status, first) = emulator.call("CreateBillScenario", original.clone()).await;
    assert_eq!(
        status, 200,
        "the first create: expected 200, got {status} with {first}"
    );

    let mut different_name = original.clone();
    different_name["name"] = json!("renamed");
    let mut different_preference = original.clone();
    different_preference["groupSharingPreference"] = json!("RESTRICTED");
    let mut different_tags = original.clone();
    different_tags["tags"] = json!({ "team": "platform" });
    let mut added_arn = original.clone();
    added_arn["costCategoryGroupSharingPreferenceArn"] = json!(COST_CATEGORY_ARN);

    for (case, body) in [
        ("name", different_name),
        ("groupSharingPreference", different_preference),
        ("tags", different_tags),
        ("costCategoryGroupSharingPreferenceArn", added_arn),
    ] {
        let (status, answered) = emulator.call("CreateBillScenario", body).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ConflictException",
            "case {case}: expected ConflictException, got {answered}"
        );
        assert_eq!(
            answered["resourceId"], first["id"],
            "case {case}: expected the conflict to name the scenario the token created {:?}, got {answered}",
            first["id"]
        );
    }

    // The original request still replays, and only one scenario was ever created.
    let (status, replay) = emulator.call("CreateBillScenario", original).await;
    assert_eq!(
        status, 200,
        "the replay: expected 200, got {status} with {replay}"
    );
    assert_eq!(
        replay["id"], first["id"],
        "expected the replay to answer the first id {:?}, got {:?}",
        first["id"], replay["id"]
    );

    let (_, listed) = emulator
        .call("ListBillScenarios", json!({ "maxResults": 25 }))
        .await;
    assert_eq!(
        listed["items"].as_array().map_or(0, Vec::len),
        1,
        "expected exactly one scenario to have been created, got {listed}"
    );
}

/// RULE: every id this service assigns is a distinct, well-formed `ResourceId` — the
/// lowercase-hex v4 UUID form the shape declares — whichever operation mints it.
///
/// The table spans both minting paths and the counts they mint at once: single creates,
/// and batches of 1, 2 and 25 usage modifications (25 is `BatchCreateBillScenarioUsage\
/// ModificationEntries`' declared maximum). Collected across the whole suite, no id may
/// repeat: a repeated id would overwrite somebody's resource.
#[tokio::test]
async fn every_minted_id_is_a_distinct_well_formed_resource_id() {
    let emulator = Emulator::new();
    let mut seen: Vec<String> = Vec::new();

    for batch_size in [1_usize, 2, 25] {
        let case = format!("batch of {batch_size}");
        let scenario = emulator.create_scenario(&format!("mint-{batch_size}")).await;
        assert!(
            is_resource_id(&scenario),
            "case {case}: expected a ResourceId-shaped scenario id, got {scenario:?}"
        );
        seen.push(scenario.clone());

        let entries: Vec<Value> = (0..batch_size)
            .map(|index| {
                json!({
                    "key": format!("k{index}"),
                    "serviceCode": "AmazonEC2",
                    "usageType": "BoxUsage:m5.large",
                    "operation": "RunInstances",
                    "usageAccountId": USAGE_ACCOUNT_ID,
                    "amounts": [amount(1_767_225_600, index as f64 + 1.0)],
                })
            })
            .collect();
        let (status, created) = emulator
            .call(
                "BatchCreateBillScenarioUsageModification",
                json!({ "billScenarioId": scenario, "usageModifications": entries }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {created}"
        );
        let items = created["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {created}"));
        assert_eq!(
            items.len(),
            batch_size,
            "case {case}: expected {batch_size} items, got {created}"
        );
        for item in items {
            let id = item["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_resource_id(&id),
                "case {case}: expected a ResourceId-shaped modification id, got {id:?}"
            );
            seen.push(id);
        }
    }

    let mut distinct = seen.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "expected every minted id to be distinct, got {} ids with {} distinct: {seen:?}",
        seen.len(),
        distinct.len()
    );
}

/// RULE: a `FilterTimestamp` window keeps only the scenarios whose instant for the
/// attribute the window names falls inside it; the window is INCLUSIVE at both ends and
/// open at an end the caller leaves unset.
///
/// `FilterTimestamp` carries exactly `afterTimestamp` and `beforeTimestamp`
/// (smithy_query: `shape="FilterTimestamp"` — "Include results after/before this
/// timestamp"), and `ListBillScenarios` applies one such window to `createdAt` and one
/// to `expiresAt`. The table spans the window shapes those two members allow: after
/// only, before only, both, each boundary instant exactly (which must be included), a
/// window that selects nothing, and the two windows AND-ed together — over scenarios
/// whose `createdAt` and `expiresAt` are an hour apart from each other.
#[tokio::test]
async fn a_timestamp_window_selects_on_the_instant_it_names() {
    let emulator = Emulator::new();
    // The store is aged directly, the way the clock would: `createdAt` is minted by the
    // create call, so three scenarios created in the same second are given distinct
    // instants here. Every `expiresAt` stays in the FUTURE, so the expiry purge leaves
    // all three in place and this suite measures the filter, not the retention window.
    let base = state::now_secs();
    let plan = [
        ("old", base - 7_200, base + 3_600),
        ("mid", base - 3_600, base + 7_200),
        ("new", base, base + 10_800),
    ];
    let mut ids = Vec::new();
    for (name, created, expires) in plan {
        let id = emulator.create_scenario(name).await;
        {
            let store = emulator.engine.state::<ServiceState>();
            let mut guard = store.lock();
            let scenario = guard
                .bill_scenario_mut(&id)
                .unwrap_or_else(|| panic!("scenario {name} was not stored"));
            scenario.created_at = aws_smithy_types::DateTime::from_secs(created);
            scenario.expires_at = aws_smithy_types::DateTime::from_secs(expires);
        }
        ids.push(id);
    }
    let (old, mid, fresh) = (ids[0].clone(), ids[1].clone(), ids[2].clone());

    let cases: Vec<(Value, Vec<&String>)> = vec![
        // createdAt, after only: the bound itself is included.
        (
            json!({ "createdAtFilter": { "afterTimestamp": base - 3_600 } }),
            vec![&mid, &fresh],
        ),
        (
            json!({ "createdAtFilter": { "afterTimestamp": base - 7_200 } }),
            vec![&old, &mid, &fresh],
        ),
        // createdAt, after only, past every scenario: nothing is selected.
        (
            json!({ "createdAtFilter": { "afterTimestamp": base + 1 } }),
            vec![],
        ),
        // createdAt, before only: the bound itself is included.
        (
            json!({ "createdAtFilter": { "beforeTimestamp": base - 3_600 } }),
            vec![&old, &mid],
        ),
        (
            json!({ "createdAtFilter": { "beforeTimestamp": base - 7_201 } }),
            vec![],
        ),
        // createdAt, both ends: a closed window.
        (
            json!({ "createdAtFilter": {
                "afterTimestamp": base - 7_200,
                "beforeTimestamp": base - 3_600,
            } }),
            vec![&old, &mid],
        ),
        // createdAt, both ends on ONE instant: still inclusive, so that scenario alone.
        (
            json!({ "createdAtFilter": {
                "afterTimestamp": base - 3_600,
                "beforeTimestamp": base - 3_600,
            } }),
            vec![&mid],
        ),
        // createdAt, a window between two scenarios: nothing falls in it.
        (
            json!({ "createdAtFilter": {
                "afterTimestamp": base - 7_199,
                "beforeTimestamp": base - 3_601,
            } }),
            vec![],
        ),
        // The same rule on the other instant: expiresAt, which orders the scenarios the
        // same way but at different values, so a window cannot pass by reading createdAt.
        (
            json!({ "expiresAtFilter": { "afterTimestamp": base + 7_200 } }),
            vec![&mid, &fresh],
        ),
        (
            json!({ "expiresAtFilter": { "beforeTimestamp": base + 3_600 } }),
            vec![&old],
        ),
        (
            json!({ "expiresAtFilter": {
                "afterTimestamp": base + 3_600,
                "beforeTimestamp": base + 7_200,
            } }),
            vec![&old, &mid],
        ),
        (
            json!({ "expiresAtFilter": { "afterTimestamp": base + 10_801 } }),
            vec![],
        ),
        // Both windows, and a `filters` entry beside them: every selection is AND-ed.
        (
            json!({
                "createdAtFilter": { "afterTimestamp": base - 7_200 },
                "expiresAtFilter": { "beforeTimestamp": base + 7_200 },
            }),
            vec![&old, &mid],
        ),
        (
            json!({
                "createdAtFilter": { "afterTimestamp": base - 3_600 },
                "expiresAtFilter": { "beforeTimestamp": base + 3_600 },
            }),
            vec![],
        ),
        (
            json!({
                "createdAtFilter": { "afterTimestamp": base - 7_200 },
                "filters": [{ "name": "NAME", "values": ["mid"] }],
            }),
            vec![&mid],
        ),
        // No window at all: every live scenario, in creation order.
        (json!({}), vec![&old, &mid, &fresh]),
    ];

    for (request, expected) in cases {
        let case = request.to_string();
        let (status, listed) = emulator.call("ListBillScenarios", request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {listed}"
        );
        let got: Vec<String> = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no items in {listed}"))
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let want: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(got, want, "case {case}: expected {want:?}, got {got:?}");
    }
}
