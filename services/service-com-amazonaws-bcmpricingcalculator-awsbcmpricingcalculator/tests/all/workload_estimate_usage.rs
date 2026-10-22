// SPDX-License-Identifier: CC0-1.0

//! The usage lines of a workload estimate, driven end-to-end over this service's
//! awsJson1_0 wire (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`)
//! through the same `MultiServiceRouter` the production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the rows
//! differ along what the rule generalises over (entry count, amount magnitude, the
//! optional members an entry carries, the keys a request repeats, replay count, the
//! member a replayed token disagrees on, how long the caller waits).
//!
//! The wire harness is the one `bill_scenario_lifecycle` already defines; a second copy
//! could drift from the front door that one exercises.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    ops::types::BATCH_CREATE_WORKLOAD_ESTIMATE_USAGE_CODE_VALUES, state,
    ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
};

use crate::bill_scenario_lifecycle::{error_code, is_resource_id, Emulator};
use crate::workload_estimate_lifecycle::create_estimate;

/// A 12-digit `AccountId`, the form `^\d{12}$` requires.
const USAGE_ACCOUNT_ID: &str = "000000000000";

/// One `BatchCreateWorkloadEstimateUsageEntry`.
///
/// Every required member is sent; `group` and `historicalUsage` are the optional ones
/// the table varies. `HistoricalUsageEntity` requires `billInterval` and
/// `filterExpression` (smithy_query: `shape="HistoricalUsageEntity"`), and every member
/// of `Expression` is optional, so the narrowest expression is the empty one.
fn entry(key: &str, amount: f64, group: Option<&str>, historical: bool) -> Value {
    let mut entry = json!({
        "key": key,
        "serviceCode": "AmazonEC2",
        "usageType": "BoxUsage:m5.xlarge",
        "operation": "RunInstances",
        "usageAccountId": USAGE_ACCOUNT_ID,
        "amount": amount,
    });
    if let Some(group) = group {
        entry["group"] = json!(group);
    }
    if historical {
        entry["historicalUsage"] = json!({
            "serviceCode": "AmazonEC2",
            "usageType": "BoxUsage:m5.xlarge",
            "operation": "RunInstances",
            "usageAccountId": USAGE_ACCOUNT_ID,
            "billInterval": { "start": 1_760_000_000, "end": 1_762_678_400 },
            "filterExpression": {},
        });
    }
    entry
}

/// The usage lines the store holds for `id`, as `(line id, amount)` in stored order.
fn stored_usage(emulator: &Emulator, id: &str) -> Vec<(String, f64)> {
    emulator
        .engine
        .state::<ServiceState>()
        .lock()
        .workload_estimate(id)
        .map(|estimate| {
            estimate
                .usage
                .iter()
                .map(|line| (line.id.clone(), line.amount))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the create table.
struct CreateCase {
    entries: usize,
    amount: f64,
    group: Option<&'static str>,
    historical: bool,
}

/// RULE: a batch create mints one usage line per entry under the workload estimate the
/// request names, stores it there, and answers from what it persisted — a fresh
/// `ResourceId` per line, the entry's own `key`, `serviceCode`, `usageType`,
/// `operation`, `usageAccountId`, `group` and `historicalUsage` reported back, and the
/// entry's `amount` as the line's `quantity.amount`. The members that come from the
/// price list this emulator does not carry (`location`, `cost`, `currency`, `status`
/// and the quantity's `unit`) are absent rather than invented.
///
/// The table spans the entry count (1 up to `BatchCreateWorkloadEstimateUsageEntries`'
/// modelled maximum of 25), the amount's magnitude (zero, fractional, large) and
/// whether the entry carries the optional `group` and `historicalUsage`.
#[tokio::test]
async fn batch_created_usage_is_what_the_store_and_a_replay_report() {
    let cases = [
        CreateCase {
            entries: 1,
            amount: 1.0,
            group: None,
            historical: false,
        },
        CreateCase {
            entries: 2,
            amount: 0.0,
            group: Some("group-a"),
            historical: false,
        },
        CreateCase {
            entries: 3,
            amount: 0.5,
            group: None,
            historical: true,
        },
        CreateCase {
            entries: 5,
            amount: 1_234_567.875,
            group: Some("g-9"),
            historical: true,
        },
        // The longest list `BatchCreateWorkloadEstimateUsageEntries` accepts (len 1..25).
        CreateCase {
            entries: 25,
            amount: 42.0,
            group: None,
            historical: false,
        },
    ];

    for case in &cases {
        let label = format!(
            "entries={} amount={} group={:?} historical={}",
            case.entries, case.amount, case.group, case.historical
        );
        let emulator = Emulator::new();
        let estimate = create_estimate(&emulator, "usage").await;

        let entries: Vec<Value> = (0..case.entries)
            .map(|index| {
                entry(
                    &format!("k{index}"),
                    case.amount,
                    case.group,
                    case.historical,
                )
            })
            .collect();
        let request = json!({
            "workloadEstimateId": estimate,
            "usage": entries,
            "clientToken": "create-token",
        });

        let (status, batch) = emulator
            .call("BatchCreateWorkloadEstimateUsage", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label}: expected 200, got {status} with {batch}"
        );
        let items = batch["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: no items in {batch}"))
            .clone();
        assert_eq!(
            items.len(),
            case.entries,
            "case {label}: expected one item per entry, got {} in {batch}",
            items.len()
        );
        assert_eq!(
            batch["errors"].as_array().map_or(0, Vec::len),
            0,
            "case {label}: expected no per-entry errors, got {:?}",
            batch["errors"]
        );

        let mut seen: Vec<String> = Vec::new();
        for (index, (item, sent)) in items.iter().zip(&entries).enumerate() {
            let id = item["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_resource_id(&id),
                "case {label} entry {index}: expected a lowercase-hex UUID id, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case {label} entry {index}: expected a fresh id, got the repeated {id}"
            );
            seen.push(id);

            for field in [
                "key",
                "serviceCode",
                "usageType",
                "operation",
                "usageAccountId",
                "group",
                "historicalUsage",
            ] {
                let want = sent.get(field).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    item[field], want,
                    "case {label} entry {index}: expected {field} {want:?}, got {:?}",
                    item[field]
                );
            }
            assert_eq!(
                item["quantity"]["amount"].as_f64(),
                Some(case.amount),
                "case {label} entry {index}: expected quantity.amount {}, got {:?}",
                case.amount,
                item["quantity"]["amount"]
            );
            assert_eq!(
                item["quantity"]["unit"],
                Value::Null,
                "case {label} entry {index}: expected the quantity's unit absent (it comes from the price list), got {:?}",
                item["quantity"]["unit"]
            );
            for absent in ["location", "cost", "currency", "status"] {
                assert_eq!(
                    item[absent],
                    Value::Null,
                    "case {label} entry {index}: expected {absent} absent (it comes from the price list), got {:?}",
                    item[absent]
                );
            }
        }

        // What the response reported is what the store holds, in creation order.
        let stored = stored_usage(&emulator, &estimate);
        let want: Vec<(String, f64)> = seen.iter().map(|id| (id.clone(), case.amount)).collect();
        assert_eq!(
            stored, want,
            "case {label}: expected the store to hold the created lines, got {stored:?}"
        );

        // And a replay of the same request answers out of that store: the same lines,
        // with the ids the first call assigned, and no second set.
        let (status, replayed) = emulator
            .call("BatchCreateWorkloadEstimateUsage", request)
            .await;
        assert_eq!(
            status, 200,
            "case {label}: expected the replay to answer 200, got {status} with {replayed}"
        );
        assert_eq!(
            replayed["items"], batch["items"],
            "case {label}: expected the replay to report the first call's lines, got {:?}",
            replayed["items"]
        );
        let after_replay = stored_usage(&emulator, &estimate);
        assert_eq!(
            after_replay, stored,
            "case {label}: expected the replay to mint nothing, got {after_replay:?}"
        );
    }
}

/// RULE: adding usage changes what an estimate's cost is computed over, so the estimate
/// re-enters the non-terminal status it was created in and the CLOCK alone settles it
/// back to the terminal one, over a span that is never zero — landing the cost
/// recomputed over the lines it now holds (nothing, since pricing a line needs the
/// price list this emulator does not carry).
///
/// The table spans how long the caller waits before reading, on each side of the window.
#[tokio::test]
async fn adding_usage_reopens_the_cost_recomputation_the_clock_settles() {
    let window = u64::try_from(state::WORKLOAD_ESTIMATE_UPDATING_MILLIS)
        .expect("the window is a positive number of milliseconds");
    assert!(
        window > 0,
        "expected a non-zero window so a caller can observe the transient status, got {window}"
    );

    let cases: [(u64, &str); 3] = [
        (0, state::WORKLOAD_ESTIMATE_STATUS_UPDATING),
        (window + 150, state::WORKLOAD_ESTIMATE_STATUS_VALID),
        (window * 2 + 150, state::WORKLOAD_ESTIMATE_STATUS_VALID),
    ];

    for (waited_millis, expected_status) in cases {
        let case = format!("waited={waited_millis}ms");
        let emulator = Emulator::new();
        let estimate = create_estimate(&emulator, "recompute").await;

        // Let the estimate's own create settle first, so the status the add is observed
        // to re-open is a settled one.
        std::thread::sleep(std::time::Duration::from_millis(window + 150));
        let (status, settled) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": estimate }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the reader, got {status} with {settled}"
        );
        assert_eq!(
            settled["status"].as_str(),
            Some(state::WORKLOAD_ESTIMATE_STATUS_VALID),
            "case {case}: expected the create to have settled, got {:?}",
            settled["status"]
        );

        let (status, batch) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("a", 7.5, None, false), entry("b", 2.25, None, false)],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the add, got {status} with {batch}"
        );

        if waited_millis > 0 {
            std::thread::sleep(std::time::Duration::from_millis(waited_millis));
        }

        // Read twice: a settled status is stable, and a read is not what settles it.
        for attempt in 0..2 {
            let (status, fetched) = emulator
                .call("GetWorkloadEstimate", json!({ "identifier": estimate }))
                .await;
            assert_eq!(
                status, 200,
                "case {case} attempt={attempt}: expected 200, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["status"].as_str(),
                Some(expected_status),
                "case {case} attempt={attempt}: expected status {expected_status}, got {:?}",
                fetched["status"]
            );
            // The cost is the sum over the costs of the lines the estimate holds, and
            // no line carries one, so it stays at the cost of an estimate holding no
            // priced usage.
            assert_eq!(
                fetched["totalCost"].as_f64(),
                Some(0.0),
                "case {case} attempt={attempt}: expected the cost summed over unpriced lines, got {:?}",
                fetched["totalCost"]
            );
            assert_eq!(
                fetched["costCurrency"].as_str(),
                Some(state::COST_CURRENCY),
                "case {case} attempt={attempt}: expected the modelled currency, got {:?}",
                fetched["costCurrency"]
            );
        }

        // The usage itself is untouched by the settling.
        let stored = stored_usage(&emulator, &estimate);
        assert_eq!(
            stored.iter().map(|(_, amount)| *amount).collect::<Vec<_>>(),
            vec![7.5, 2.25],
            "case {case}: expected the added lines to survive the settling, got {stored:?}"
        );
    }
}

/// RULE: an entry that fails on its own is reported in `errors` against its `key` with
/// one of the modelled error codes, while the other entries of the same request are
/// still created — and the request as a whole still succeeds.
///
/// The model makes `key` the identity of an entry within a request
/// (`BatchCreateWorkloadEstimateUsageEntry$key`: "This key is useful to identify errors
/// associated with any usage entry"), so two entries sharing one key cannot both be
/// answered: the later one conflicts. The table spans how the keys of a request repeat.
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
        let estimate = create_estimate(&emulator, "partial").await;

        let usage: Vec<Value> = keys
            .iter()
            .map(|key| entry(key, 3.0, None, false))
            .collect();
        let (status, batch) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({ "workloadEstimateId": estimate, "usage": usage }),
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
        for (index, error) in errors.iter().enumerate() {
            // `BatchCreateWorkloadEstimateUsageCode`'s own values (smithy_query:
            // `shape="BatchCreateWorkloadEstimateUsageCode"`).
            let code = error["errorCode"].as_str().unwrap_or_default();
            assert!(
                BATCH_CREATE_WORKLOAD_ESTIMATE_USAGE_CODE_VALUES.contains(&code),
                "case {case} error {index}: expected a modelled error code, got {code:?}"
            );
            let key = error["key"].as_str().unwrap_or_default();
            assert!(
                keys.contains(&key),
                "case {case} error {index}: expected the error to name a requested key, got {key:?}"
            );
        }

        // Exactly the accepted entries are persisted.
        let stored = stored_usage(&emulator, &estimate).len();
        assert_eq!(
            stored, expected_items,
            "case {case}: expected {expected_items} persisted lines, got {stored}"
        );
    }
}

/// RULE: a batch create that names a workload estimate it cannot find answers
/// `ResourceNotFoundException` (HTTP 404) naming what it looked for and persists
/// nothing — and a `ResourceId` is only ever resolved inside its own resource kind, so a
/// bill scenario's id is not a workload estimate's.
#[tokio::test]
async fn usage_for_an_absent_estimate_is_a_not_found() {
    let emulator = Emulator::new();
    let live = create_estimate(&emulator, "live").await;

    let (status, body) = emulator
        .call("CreateBillScenario", json!({ "name": "neighbour" }))
        .await;
    assert_eq!(
        status, 200,
        "creating the neighbouring scenario: expected 200, got {status} with {body}"
    );
    let scenario = body["id"].as_str().unwrap_or_default().to_string();

    // Never created, a well-formed id of no resource, and the id of a resource of the
    // OTHER kind.
    for id in [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        scenario.as_str(),
    ] {
        let (status, body) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({
                    "workloadEstimateId": id,
                    "usage": [entry("a", 1.0, None, false)],
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case id={id}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case id={id}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(id),
            "case id={id}: expected the error to name what was looked for, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some(state::WORKLOAD_ESTIMATE_RESOURCE_TYPE),
            "case id={id}: expected the error to name the resource type, got {body}"
        );
    }

    // The live estimate is untouched by the refusals.
    let stored = stored_usage(&emulator, &live);
    assert!(
        stored.is_empty(),
        "expected the refusals to persist nothing, got {stored:?}"
    );
}

/// RULE: a replayed `@idempotencyToken` answers with the usage lines the first call
/// created — never a second set — while the SAME token carrying a different request is
/// `ConflictException` (HTTP 409) naming a line it already made.
///
/// The table spans the token's shape (`ClientToken` is `len 1..64`, pattern
/// `^[\u0021-\u007E]+$`), the number of replays, and which member the mismatching
/// request disagrees on.
#[tokio::test]
async fn replaying_a_client_token_returns_the_first_usage_lines() {
    let cases: [(&str, usize); 4] = [
        ("t", 1),
        ("plain-token-1", 2),
        ("~!@#$%^&*()_+punctuation", 3),
        (
            "t234567890123456789012345678901234567890123456789012345678901234",
            1,
        ),
    ];

    for (token, replays) in cases {
        let case = format!("token={token} replays={replays}");
        let emulator = Emulator::new();
        let estimate = create_estimate(&emulator, "replayed").await;
        let other = create_estimate(&emulator, "other").await;
        let request = json!({
            "workloadEstimateId": estimate,
            "usage": [entry("a", 4.0, Some("group-a"), false), entry("b", 8.0, None, true)],
            "clientToken": token,
        });

        let (status, first) = emulator
            .call("BatchCreateWorkloadEstimateUsage", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the first call to answer 200, got {status} with {first}"
        );
        let ids: Vec<String> = stored_usage(&emulator, &estimate)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ids.len(),
            2,
            "case {case}: expected the first call to store both lines, got {ids:?}"
        );

        for attempt in 0..replays {
            let (status, replayed) = emulator
                .call("BatchCreateWorkloadEstimateUsage", request.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {case} attempt={attempt}: expected a replay to answer 200, got {status} with {replayed}"
            );
            assert_eq!(
                replayed["items"], first["items"],
                "case {case} attempt={attempt}: expected the first lines back, got {:?}",
                replayed["items"]
            );
        }
        let after: Vec<String> = stored_usage(&emulator, &estimate)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            after, ids,
            "case {case}: expected the replays to mint nothing, got {after:?}"
        );

        // The same token for a DIFFERENT request is a conflict, whichever member of the
        // request differs.
        for (member, mismatching) in [
            (
                "workloadEstimateId",
                json!({
                    "workloadEstimateId": other,
                    "usage": [entry("a", 4.0, Some("group-a"), false), entry("b", 8.0, None, true)],
                    "clientToken": token,
                }),
            ),
            (
                "amount",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("a", 4.5, Some("group-a"), false), entry("b", 8.0, None, true)],
                    "clientToken": token,
                }),
            ),
            (
                "key",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("c", 4.0, Some("group-a"), false), entry("b", 8.0, None, true)],
                    "clientToken": token,
                }),
            ),
            (
                "group",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("a", 4.0, None, false), entry("b", 8.0, None, true)],
                    "clientToken": token,
                }),
            ),
            (
                "historicalUsage",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("a", 4.0, Some("group-a"), false), entry("b", 8.0, None, false)],
                    "clientToken": token,
                }),
            ),
            (
                "entry-count",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("a", 4.0, Some("group-a"), false)],
                    "clientToken": token,
                }),
            ),
        ] {
            let (status, body) = emulator
                .call("BatchCreateWorkloadEstimateUsage", mismatching)
                .await;
            assert_eq!(
                status, 409,
                "case {case} differing={member}: expected 409, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case} differing={member}: expected ConflictException, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(ids[0].as_str()),
                "case {case} differing={member}: expected the conflict to name a line the token made, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(state::WORKLOAD_ESTIMATE_USAGE_RESOURCE_TYPE),
                "case {case} differing={member}: expected the conflict to name the resource type, got {body}"
            );
        }
    }
}

/// RULE: a batch create that would take an estimate past its usage quota is refused
/// with the modelled `ServiceQuotaExceededException` (HTTP 402) instead of growing the
/// store without bound, and the refusal stores nothing.
#[tokio::test]
async fn adding_usage_past_the_quota_is_refused() {
    let emulator = Emulator::new();
    let estimate = create_estimate(&emulator, "quota").await;

    // The modelled maximum entry count per call, so the quota is reached in as few
    // calls as the model allows.
    let batch_size = 25;
    let mut filled = 0;
    while filled < state::MAX_USAGE_PER_WORKLOAD_ESTIMATE {
        let usage: Vec<Value> = (0..batch_size)
            .map(|index| entry(&format!("k{index}"), 1.0, None, false))
            .collect();
        let (status, body) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({ "workloadEstimateId": estimate, "usage": usage }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case filled={filled}: expected 200 below the quota, got {status} with {body}"
        );
        filled += batch_size;
    }

    // Every call from the quota onwards is refused, and refused the same way.
    for attempt in 0..3 {
        let (status, body) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("over", 1.0, None, false)],
                }),
            )
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
        assert_eq!(
            body["resourceType"].as_str(),
            Some(state::WORKLOAD_ESTIMATE_USAGE_RESOURCE_TYPE),
            "case attempt={attempt}: expected the refusal to name the resource type, got {body}"
        );
    }

    let stored = stored_usage(&emulator, &estimate).len();
    assert_eq!(
        stored,
        state::MAX_USAGE_PER_WORKLOAD_ESTIMATE,
        "expected the store to hold exactly the quota, got {stored}"
    );
}
