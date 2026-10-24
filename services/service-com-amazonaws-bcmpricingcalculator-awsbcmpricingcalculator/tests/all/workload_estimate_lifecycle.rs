// SPDX-License-Identifier: CC0-1.0

//! The workload-estimate lifecycle, driven end-to-end over this service's awsJson1_0
//! wire (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`) through the
//! same `MultiServiceRouter` the production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the rows
//! differ along whatever the rule generalises over (name shape, rate type, tag count,
//! replay count, the member a replayed token disagrees on, the age of the estimate).
//!
//! The wire harness is the one `bill_scenario_lifecycle` already defines; a second copy
//! could drift from the front door that one exercises.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state, ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
};

use crate::bill_scenario_lifecycle::{error_code, is_resource_id, seconds, Emulator};

/// The retention window an estimate's `expiresAt` is expected to sit after its
/// `createdAt`, declared HERE rather than read from the implementation so the assertion
/// fails if the handler's window changes.
const EXPECTED_RETENTION_SECS: i64 = 15 * 24 * 60 * 60;

/// Every `WorkloadEstimateRateType` the model declares (smithy_query:
/// `shape="WorkloadEstimateRateType"`), plus `None` for a request that sends none.
const RATE_TYPES: [Option<&str>; 4] = [
    None,
    Some("BEFORE_DISCOUNTS"),
    Some("AFTER_DISCOUNTS"),
    Some("AFTER_DISCOUNTS_AND_COMMITMENTS"),
];

/// One row of the create table.
struct CreateCase {
    name: &'static str,
    rate_type: Option<&'static str>,
    tags: Option<Value>,
}

/// The `CreateWorkloadEstimate` request a row asks for.
fn create_request(case: &CreateCase) -> Value {
    let mut request = json!({ "name": case.name });
    if let Some(rate_type) = case.rate_type {
        request["rateType"] = json!(rate_type);
    }
    if let Some(tags) = case.tags.clone() {
        request["tags"] = tags;
    }
    request
}

/// Create an estimate and return its id, failing the test if the call did not succeed.
///
/// `pub(crate)` so every suite in this test target mints an estimate the same way.
pub(crate) async fn create_estimate(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .call("CreateWorkloadEstimate", json!({ "name": name }))
        .await;
    assert_eq!(
        status, 200,
        "CreateWorkloadEstimate({name}): expected 200, got {status} with {body}"
    );
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkloadEstimate({name}): no id in {body}"))
        .to_string()
}

/// RULE: a create mints one fresh workload estimate and answers from what it persisted —
/// a `ResourceId` nothing else carries, the name the request asked for, the EFFECTIVE
/// rate basis it is priced on (the one the request named, or the service's default when
/// it named none), the create instant as both `createdAt` and the `rateTimestamp` the
/// rates were read at, one retention window later as `expiresAt`, the terminal status an
/// estimate whose cost is resolved carries, and the cost of the usage it holds (none
/// yet) in the one modelled currency — and the reader reports exactly that stored
/// estimate.
///
/// The table spans the name shape (`WorkloadEstimateName` is `len ..64`, pattern
/// `^[a-zA-Z0-9-]+$`), every `WorkloadEstimateRateType` and its absence, and the tag
/// count.
#[tokio::test]
async fn a_created_estimate_is_what_the_reader_reports() {
    let cases = vec![
        CreateCase {
            name: "plain",
            rate_type: None,
            tags: None,
        },
        CreateCase {
            name: "with-hyphens-and-9-digits",
            rate_type: RATE_TYPES[1],
            tags: None,
        },
        CreateCase {
            name: "MixedCase123",
            rate_type: RATE_TYPES[2],
            tags: Some(json!({ "team": "finops" })),
        },
        CreateCase {
            name: "after-discounts-and-commitments",
            rate_type: RATE_TYPES[3],
            tags: Some(json!({ "team": "finops", "env": "test" })),
        },
        // `WorkloadEstimateName` is `len ..64`: the longest name the model accepts.
        CreateCase {
            name: "a234567890123456789012345678901234567890123456789012345678901234",
            rate_type: RATE_TYPES[1],
            tags: Some(json!({})),
        },
        // And the shortest.
        CreateCase {
            name: "x",
            rate_type: None,
            tags: None,
        },
    ];

    let emulator = Emulator::new();
    let mut minted: Vec<String> = Vec::new();

    for case in &cases {
        let label = format!(
            "name={} rateType={:?} tags={}",
            case.name,
            case.rate_type,
            case.tags.is_some()
        );

        let before = state::now_secs();
        let (status, created) = emulator
            .call("CreateWorkloadEstimate", create_request(case))
            .await;
        let after = state::now_secs();
        assert_eq!(
            status, 200,
            "case {label}: expected CreateWorkloadEstimate to answer 200, got {status} with {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_resource_id(&id),
            "case {label}: expected a lowercase-hex UUID id, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {label}: expected a fresh id, got the already-minted {id}"
        );
        minted.push(id.clone());

        assert_eq!(
            created["name"].as_str(),
            Some(case.name),
            "case {label}: expected the name back, got {:?}",
            created["name"]
        );
        // A request that named no rate type is priced on the service's default basis,
        // and the response reports which basis that is rather than leaving the caller to
        // assume one.
        let expected_rate_type = case
            .rate_type
            .unwrap_or(state::DEFAULT_WORKLOAD_ESTIMATE_RATE_TYPE);
        assert_eq!(
            created["rateType"].as_str(),
            Some(expected_rate_type),
            "case {label}: expected rateType {expected_rate_type:?}, got {:?}",
            created["rateType"]
        );
        assert_eq!(
            created["status"].as_str(),
            Some(state::WORKLOAD_ESTIMATE_STATUS_VALID),
            "case {label}: expected the terminal status of an estimate whose cost is \
             resolved, got {:?}",
            created["status"]
        );
        assert_eq!(
            created["costCurrency"].as_str(),
            Some(state::COST_CURRENCY),
            "case {label}: expected the modelled currency, got {:?}",
            created["costCurrency"]
        );
        assert_eq!(
            created["totalCost"].as_f64(),
            Some(0.0),
            "case {label}: expected the cost of an estimate holding no usage to be 0, got {:?}",
            created["totalCost"]
        );
        assert_eq!(
            created["failureMessage"],
            Value::Null,
            "case {label}: expected no failureMessage on a successful create, got {:?}",
            created["failureMessage"]
        );

        let created_at = seconds(&created, "createdAt");
        assert!(
            (before..=after).contains(&created_at),
            "case {label}: expected createdAt within [{before}, {after}], got {created_at}"
        );
        assert_eq!(
            seconds(&created, "rateTimestamp"),
            created_at,
            "case {label}: expected rateTimestamp at the instant the rates were read, got {:?}",
            created["rateTimestamp"]
        );
        let expires_at = seconds(&created, "expiresAt");
        assert_eq!(
            expires_at,
            created_at + EXPECTED_RETENTION_SECS,
            "case {label}: expected expiresAt one retention window after createdAt, got {expires_at}"
        );

        // GetWorkloadEstimate answers from the same stored estimate.
        let (status, fetched) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": id }))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: expected GetWorkloadEstimate to answer 200, got {status} with {fetched}"
        );
        for field in [
            "id",
            "name",
            "status",
            "createdAt",
            "expiresAt",
            "rateType",
            "rateTimestamp",
            "totalCost",
            "costCurrency",
        ] {
            assert_eq!(
                fetched[field], created[field],
                "case {label}: expected GetWorkloadEstimate to report the created {field} {:?}, got {:?}",
                created[field], fetched[field]
            );
        }

        // The tags the request asked for are persisted on the estimate, so the tagging
        // operations (still scaffolds, with their own issues) have them to report.
        let want: Vec<(String, String)> = {
            let mut pairs: Vec<(String, String)> = case
                .tags
                .as_ref()
                .and_then(Value::as_object)
                .map(|map| {
                    map.iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            pairs.sort();
            pairs
        };
        let store = emulator.engine.state::<ServiceState>();
        let got: Vec<(String, String)> = store
            .lock()
            .workload_estimate(&id)
            .unwrap_or_else(|| panic!("case {label}: {id} was not stored"))
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_eq!(
            got, want,
            "case {label}: expected the requested tags stored, got {got:?}"
        );
    }
}

/// RULE: an estimate the service reports as complete is complete when the create
/// answers: its cost is computed from the usage it holds and the rates it is priced at,
/// both in hand by then, so the status the create reports is already the terminal one and
/// no reader — however long it waits, however often it asks — sees it move, or sees any
/// other member of the estimate move either.
///
/// The table spans how long the caller waits before reading: a caller that does not wait
/// at all must see the same settled estimate as one that waits.
#[tokio::test]
async fn a_created_estimate_is_already_settled_and_stays_settled() {
    let waits: [u64; 3] = [0, 150, 650];

    for waited_millis in waits {
        let case = format!("waited={waited_millis}ms");
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": "settled" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the create, got {status} with {created}"
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            created["status"].as_str(),
            Some(state::WORKLOAD_ESTIMATE_STATUS_VALID),
            "case {case}: expected the create to report the terminal status, got {:?}",
            created["status"]
        );
        assert_eq!(
            created["failureMessage"],
            Value::Null,
            "case {case}: expected no failureMessage on an estimate with nothing wrong, \
             got {:?}",
            created["failureMessage"]
        );

        if waited_millis > 0 {
            std::thread::sleep(std::time::Duration::from_millis(waited_millis));
        }

        // Read twice: the status is stable, and reading is not what sets it.
        for attempt in 0..2 {
            let (status, fetched) = emulator
                .call("GetWorkloadEstimate", json!({ "identifier": id }))
                .await;
            assert_eq!(
                status, 200,
                "case {case} attempt={attempt}: expected 200, got {status} with {fetched}"
            );
            for field in [
                "id",
                "name",
                "status",
                "createdAt",
                "expiresAt",
                "rateType",
                "rateTimestamp",
                "totalCost",
                "costCurrency",
            ] {
                assert_eq!(
                    fetched[field], created[field],
                    "case {case} attempt={attempt}: expected the estimate to keep its {field} {:?}, got {:?}",
                    created[field], fetched[field]
                );
            }
        }
    }
}

/// RULE: a replayed `@idempotencyToken` answers with the estimate the first call
/// created — never a second one — while the SAME token carrying a different request is
/// `ConflictException` (HTTP 409) naming the estimate it already made.
///
/// The table spans the token's shape (`ClientToken` is `len 1..64`, pattern
/// `^[\u0021-\u007E]+$`), the number of replays, and which member the mismatching
/// request disagrees on.
#[tokio::test]
async fn replaying_a_client_token_returns_the_first_estimate() {
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
        let request = json!({
            "name": "replayed",
            "rateType": "AFTER_DISCOUNTS",
            "clientToken": token,
            "tags": { "env": "test" },
        });

        let (status, first) = emulator
            .call("CreateWorkloadEstimate", request.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the first create to answer 200, got {status} with {first}"
        );
        let id = first["id"].as_str().unwrap_or_default().to_string();

        for attempt in 0..replays {
            let (status, replayed) = emulator
                .call("CreateWorkloadEstimate", request.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {case} attempt={attempt}: expected a replay to answer 200, got {status} with {replayed}"
            );
            assert_eq!(
                replayed["id"].as_str(),
                Some(id.as_str()),
                "case {case} attempt={attempt}: expected the first estimate back, got {:?}",
                replayed["id"]
            );
        }

        let stored = emulator
            .engine
            .state::<ServiceState>()
            .lock()
            .workload_estimates
            .len();
        assert_eq!(
            stored, 1,
            "case {case}: expected the replays to mint nothing, got {stored} estimates"
        );

        // The same token for a DIFFERENT request is a conflict, whichever member of the
        // request differs.
        for (member, mismatching) in [
            ("name", json!({ "name": "other", "rateType": "AFTER_DISCOUNTS", "clientToken": token, "tags": { "env": "test" } })),
            ("rateType", json!({ "name": "replayed", "rateType": "BEFORE_DISCOUNTS", "clientToken": token, "tags": { "env": "test" } })),
            ("tags", json!({ "name": "replayed", "rateType": "AFTER_DISCOUNTS", "clientToken": token, "tags": { "env": "prod" } })),
            ("tags-absent", json!({ "name": "replayed", "rateType": "AFTER_DISCOUNTS", "clientToken": token })),
        ] {
            let (status, body) = emulator
                .call("CreateWorkloadEstimate", mismatching)
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
                Some(id.as_str()),
                "case {case} differing={member}: expected the conflict to name the estimate the token made, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(state::WORKLOAD_ESTIMATE_RESOURCE_TYPE),
                "case {case} differing={member}: expected the conflict to name the resource type, got {body}"
            );
        }
    }
}

/// RULE: a reader that names a workload estimate it cannot find answers
/// `ResourceNotFoundException` (HTTP 404) naming what it looked for — never a 200 with
/// an empty body — and a `ResourceId` is only ever resolved inside its own resource
/// kind, so one root resource's id never reads as another's.
#[tokio::test]
async fn an_absent_estimate_is_a_not_found() {
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
    let absent = [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        scenario.as_str(),
    ];
    for id in absent {
        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": id }))
            .await;
        assert_eq!(
            status, 404,
            "case id={id}: expected GetWorkloadEstimate 404, got {status} with {body}"
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

    // And symmetrically: an estimate's id is not a bill scenario.
    let (status, body) = emulator
        .call("GetBillScenario", json!({ "identifier": live }))
        .await;
    assert_eq!(
        status, 404,
        "case a workload estimate id read as a scenario: expected 404, got {status} with {body}"
    );
}

/// RULE: a resource past the `expiresAt` it advertises is gone from every reader, and
/// the idempotency record that named it goes with it; one still inside its window
/// survives untouched.
///
/// The table spans the age of the estimate on each side of the boundary instant.
#[tokio::test]
async fn an_estimate_past_its_expiry_is_gone_from_the_reader() {
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
        let untouched = create_estimate(&emulator, "untouched").await;

        let (status, created) = emulator
            .call(
                "CreateWorkloadEstimate",
                json!({ "name": "aged", "clientToken": "aged-token" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the create, got {status} with {created}"
        );
        let aged = created["id"].as_str().unwrap_or_default().to_string();

        {
            let store = emulator.engine.state::<ServiceState>();
            let mut guard = store.lock();
            let estimate = guard
                .workload_estimate_mut(&aged)
                .unwrap_or_else(|| panic!("case {case}: {aged} was not stored"));
            estimate.expires_at =
                aws_smithy_types::DateTime::from_secs(state::now_secs() + offset);
        }

        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": aged }))
            .await;
        let expected_status = if expected_live { 200 } else { 404 };
        assert_eq!(
            status, expected_status,
            "case {case}: expected GetWorkloadEstimate {expected_status}, got {status} with {body}"
        );

        let (status, body) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": untouched }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the in-window estimate to survive, got {status} with {body}"
        );

        // An expiry must not strand the idempotency record that named the estimate.
        let tracked = emulator
            .engine
            .state::<ServiceState>()
            .lock()
            .idempotency
            .len();
        let expected_tracked = usize::from(expected_live);
        assert_eq!(
            tracked, expected_tracked,
            "case {case}: expected {expected_tracked} idempotency record(s), got {tracked}"
        );
    }
}

/// RULE: a create that would take the store past its quota is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) instead of growing the store without
/// bound, and the refusal stores nothing.
#[tokio::test]
async fn creating_past_the_estimate_quota_is_refused() {
    let emulator = Emulator::new();

    for index in 0..state::MAX_WORKLOAD_ESTIMATES {
        let (status, body) = emulator
            .call(
                "CreateWorkloadEstimate",
                json!({ "name": format!("e{index}") }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case index={index}: expected 200 below the quota, got {status} with {body}"
        );
    }

    // Every create from the quota onwards is refused, and refused the same way.
    for attempt in 0..3 {
        let (status, body) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": "overflow" }))
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
            Some(state::WORKLOAD_ESTIMATE_RESOURCE_TYPE),
            "case attempt={attempt}: expected the refusal to name the resource type, got {body}"
        );
    }

    let stored = emulator
        .engine
        .state::<ServiceState>()
        .lock()
        .workload_estimates
        .len();
    assert_eq!(
        stored,
        state::MAX_WORKLOAD_ESTIMATES,
        "expected the store to hold exactly the quota, got {stored}"
    );
}
