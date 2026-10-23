// SPDX-License-Identifier: CC0-1.0

//! How this service reports a resource it does not hold, driven end-to-end over its
//! awsJson1_0 wire (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<Operation>`)
//! through the same `MultiServiceRouter` the production front door uses.
//!
//! RULE under test: a `ResourceNotFoundException` message is a FIXED sentence per
//! resource TYPE — it names the kind of resource and never echoes the identifier the
//! request carried, so the same string answers every operation that resolves that kind
//! and every identifier it fails to resolve. The requested identifier is still reported,
//! in the members modelled for it (`resourceId`, beside `resourceType`).
//!
//! The suite asserts that rule over a TABLE: both resource kinds this service holds,
//! every implemented operation that resolves one by id, and four ways an id can fail to
//! resolve. The recorded case (one `GetWorkloadEstimate` for one unknown id) is one row.
//!
//! The wire harness is the one `bill_scenario_lifecycle` already defines; a second copy
//! could drift from the front door that one exercises.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::state;

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// The message this service answers for a workload estimate it does not hold.
///
/// The text is the recorded service's own: it answers this identical string for two
/// DIFFERENT operations reading a workload estimate (`GetWorkloadEstimate` and
/// `ListWorkloadEstimateUsage`), which is what establishes it as a fixed, operation- and
/// identifier-independent sentence rather than one request's answer. Declared here rather
/// than read from `state::workload_estimate_not_found`, so a change to that builder's
/// wording fails this suite instead of being blessed by it.
const WORKLOAD_ESTIMATE_NOT_FOUND: &str =
    "The requested WorkloadEstimate was not found. Check the WorkloadEstimate ID and try again.";

/// The same sentence for a bill scenario: the rule is per resource TYPE, and the type is
/// named by its own Smithy shape name (`state::BILL_SCENARIO_RESOURCE_TYPE`), as the
/// `resourceType` member reports it.
const BILL_SCENARIO_NOT_FOUND: &str =
    "The requested BillScenario was not found. Check the BillScenario ID and try again.";

/// A 12-digit `AccountId`, the form `^\d{12}$` requires.
const USAGE_ACCOUNT_ID: &str = "000000000000";

/// One row of the table: an implemented operation that resolves a resource by id, and
/// what it must report when that id resolves to nothing.
struct NotFoundCase {
    operation: &'static str,
    /// The Smithy shape name of the kind of resource this operation resolves.
    resource_type: &'static str,
    /// The fixed message that kind earns.
    message: &'static str,
}

/// Every implemented operation that resolves one of this service's resources by id.
///
/// The remaining ones (`UpdateWorkloadEstimate`, `GetBillEstimate`, the commitment
/// batches, …) are still scaffold stubs answering HTTP 501, so they have no not-found to
/// frame yet; `DeleteWorkloadEstimate`/`DeleteBillScenario` model no not-found at all (an
/// absent resource is already in the state the call asks for).
const CASES: &[NotFoundCase] = &[
    NotFoundCase {
        operation: "GetWorkloadEstimate",
        resource_type: state::WORKLOAD_ESTIMATE_RESOURCE_TYPE,
        message: WORKLOAD_ESTIMATE_NOT_FOUND,
    },
    NotFoundCase {
        operation: "ListWorkloadEstimateUsage",
        resource_type: state::WORKLOAD_ESTIMATE_RESOURCE_TYPE,
        message: WORKLOAD_ESTIMATE_NOT_FOUND,
    },
    NotFoundCase {
        operation: "BatchCreateWorkloadEstimateUsage",
        resource_type: state::WORKLOAD_ESTIMATE_RESOURCE_TYPE,
        message: WORKLOAD_ESTIMATE_NOT_FOUND,
    },
    NotFoundCase {
        operation: "GetBillScenario",
        resource_type: state::BILL_SCENARIO_RESOURCE_TYPE,
        message: BILL_SCENARIO_NOT_FOUND,
    },
    NotFoundCase {
        operation: "ListBillScenarioUsageModifications",
        resource_type: state::BILL_SCENARIO_RESOURCE_TYPE,
        message: BILL_SCENARIO_NOT_FOUND,
    },
    NotFoundCase {
        operation: "BatchCreateBillScenarioUsageModification",
        resource_type: state::BILL_SCENARIO_RESOURCE_TYPE,
        message: BILL_SCENARIO_NOT_FOUND,
    },
];

/// The request `operation` takes for `id`, with every required member sent so nothing but
/// the unresolvable id can refuse it.
fn request_for(operation: &str, id: &str) -> Value {
    let usage = json!({
        "key": "k0",
        "serviceCode": "AmazonEC2",
        "usageType": "BoxUsage:m5.xlarge",
        "operation": "RunInstances",
        "usageAccountId": USAGE_ACCOUNT_ID,
    });
    match operation {
        "GetWorkloadEstimate" | "GetBillScenario" => json!({ "identifier": id }),
        "ListWorkloadEstimateUsage" => json!({ "workloadEstimateId": id }),
        "ListBillScenarioUsageModifications" => json!({ "billScenarioId": id }),
        "BatchCreateWorkloadEstimateUsage" => {
            let mut entry = usage;
            // `BatchCreateWorkloadEstimateUsageEntry$amount` is `@required`
            // (smithy_query: `shape="BatchCreateWorkloadEstimateUsageEntry"`).
            entry["amount"] = json!(1.0);
            json!({ "workloadEstimateId": id, "usage": [entry] })
        }
        "BatchCreateBillScenarioUsageModification" => {
            json!({ "billScenarioId": id, "usageModifications": [usage] })
        }
        other => panic!("no request is defined for {other}"),
    }
}

/// Create a resource of `kind` and return its id, failing the test if the call did not
/// succeed.
async fn create(emulator: &Emulator, kind: &str, name: &str) -> String {
    let operation = if kind == state::BILL_SCENARIO_RESOURCE_TYPE {
        "CreateBillScenario"
    } else {
        "CreateWorkloadEstimate"
    };
    let (status, body) = emulator.call(operation, json!({ "name": name })).await;
    assert_eq!(
        status, 200,
        "{operation}({name}): expected 200, got {status} with {body}"
    );
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{operation}({name}): no id in {body}"))
        .to_string()
}

/// `id` with its last hex digit changed: a `ResourceId` one character away from a live
/// one, so the id is well formed and still resolves to nothing.
fn neighbouring_id(id: &str) -> String {
    let mut digits: Vec<char> = id.chars().collect();
    if let Some(last) = digits.last_mut() {
        *last = if *last == '0' { '1' } else { '0' };
    }
    digits.into_iter().collect()
}

/// RULE: see the module docs — one fixed message per resource type, identifier- and
/// operation-independent, with the requested identifier reported in `resourceId` and the
/// kind in `resourceType`, under HTTP 404 `ResourceNotFoundException`.
///
/// The table spans both resource kinds, every implemented operation that resolves one, and
/// four ways an id can fail to resolve: an id of all zeroes, one of all `f`s, one a single
/// character away from a live resource's, and the id of a live resource of the OTHER kind
/// (a `ResourceId` is only ever resolved inside its own kind).
#[tokio::test]
async fn the_not_found_message_is_fixed_per_resource_type() {
    let emulator = Emulator::new();
    let live_estimate = create(&emulator, state::WORKLOAD_ESTIMATE_RESOURCE_TYPE, "live").await;
    let live_scenario = create(&emulator, state::BILL_SCENARIO_RESOURCE_TYPE, "livescenario").await;

    for case in CASES {
        // The id of a live resource of the OTHER kind than this operation resolves.
        let other_kind = if case.resource_type == state::BILL_SCENARIO_RESOURCE_TYPE {
            live_estimate.clone()
        } else {
            live_scenario.clone()
        };
        let own_kind = if case.resource_type == state::BILL_SCENARIO_RESOURCE_TYPE {
            live_scenario.clone()
        } else {
            live_estimate.clone()
        };
        let ids = [
            "00000000-0000-0000-0000-000000000000".to_string(),
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            neighbouring_id(&own_kind),
            other_kind,
        ];

        // Collected so the suite can assert the message does not vary with the id, not
        // merely that each id earns the expected string.
        let mut messages: BTreeSet<String> = BTreeSet::new();

        for id in &ids {
            let label = format!("op={} id={id}", case.operation);
            let (status, body) = emulator
                .call(case.operation, request_for(case.operation, id))
                .await;
            assert_eq!(
                status, 404,
                "case {label}: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label}: expected ResourceNotFoundException, got {body}"
            );
            let message = body["message"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                message, case.message,
                "case {label}: expected the fixed message {:?}, got {message:?}",
                case.message
            );
            assert!(
                !message.contains(id.as_str()),
                "case {label}: expected the message not to echo the requested id, got {message:?}"
            );
            // The requested id is reported in the member modelled for it, and the kind
            // beside it.
            assert_eq!(
                body["resourceId"].as_str(),
                Some(id.as_str()),
                "case {label}: expected resourceId to name what was looked for, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(case.resource_type),
                "case {label}: expected resourceType {}, got {body}",
                case.resource_type
            );
            messages.insert(message);
        }

        assert_eq!(
            messages.len(),
            1,
            "case op={}: expected one identifier-independent message, got {messages:?}",
            case.operation
        );
    }
}

/// RULE: the fixed message is per resource TYPE, so two different operations reading the
/// SAME kind answer the SAME string, and the two kinds answer DIFFERENT strings — each
/// naming the kind it is about.
#[tokio::test]
async fn operations_reading_one_kind_share_its_message_and_kinds_differ() {
    let emulator = Emulator::new();
    let absent = "00000000-0000-0000-0000-000000000000";

    let mut per_kind: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for resource_type in [
        state::WORKLOAD_ESTIMATE_RESOURCE_TYPE,
        state::BILL_SCENARIO_RESOURCE_TYPE,
    ] {
        let mut messages: BTreeSet<String> = BTreeSet::new();
        for case in CASES.iter().filter(|c| c.resource_type == resource_type) {
            let (status, body) = emulator
                .call(case.operation, request_for(case.operation, absent))
                .await;
            assert_eq!(
                status, 404,
                "case op={}: expected 404, got {status} with {body}",
                case.operation
            );
            messages.insert(body["message"].as_str().unwrap_or_default().to_string());
        }
        assert_eq!(
            messages.len(),
            1,
            "kind={resource_type}: expected every operation reading it to answer one message, got {messages:?}"
        );
        let message = messages.iter().next().cloned().unwrap_or_default();
        assert!(
            message.contains(resource_type),
            "kind={resource_type}: expected the message to name the kind, got {message:?}"
        );
        per_kind.push((resource_type, messages));
    }

    let (first_kind, first) = &per_kind[0];
    let (second_kind, second) = &per_kind[1];
    assert_ne!(
        first, second,
        "expected {first_kind} and {second_kind} to answer different messages, got {first:?} and {second:?}"
    );
}
