// SPDX-License-Identifier: CC0-1.0

//! What a workload estimate COSTS, and what renaming one does, driven end-to-end over
//! this service's awsJson1_0 wire (`POST /`, `X-Amz-Target:
//! AWSBCMPricingCalculator.<Operation>`) through the same `MultiServiceRouter` the
//! production front door uses.
//!
//! Each suite asserts a RULE over a table of inputs, not one recorded request: the rows
//! differ along what the rule generalises over (the service code, usage type, operation
//! and amount of the usage, the region its usage type names, whether a rate is published
//! for it at all, the number of lines and of batches, and the shape of a new name).
//!
//! The expected prices are stated HERE, from the published AWS on-demand price lists,
//! rather than read back from the implementation: an EC2 Linux instance-hour in
//! `us-east-1` costs USD 0.0104 for t3.micro, 0.096 for m5.large, 0.107 for m5.large in
//! `eu-west-1`, 0.0114 for t3.micro in `eu-west-1` and 0.17 for c5.xlarge in `us-east-1`
//! (<https://aws.amazon.com/ec2/pricing/on-demand/>); S3 Standard storage is USD 0.023
//! per GB-month (<https://aws.amazon.com/s3/pricing/>).
//!
//! The wire harness is the one `bill_scenario_lifecycle` defines; a second copy could
//! drift from the front door that one exercises.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::state;

use crate::bill_scenario_lifecycle::{error_code, Emulator};
use crate::workload_estimate_lifecycle::create_estimate;

/// A 12-digit `AccountId`, the form `^\d{12}$` requires.
const USAGE_ACCOUNT_ID: &str = "000000000000";

/// One usage a row models, and the price this service must report for it.
struct PriceCase {
    case: &'static str,
    service_code: &'static str,
    usage_type: &'static str,
    operation: &'static str,
    amount: f64,
    /// The pricing location the usage type names, absent when its region prefix names no
    /// region this service knows.
    location: Option<&'static str>,
    /// The unit of measure the usage type is counted in, absent when this service
    /// publishes nothing for that usage type under that operation.
    unit: Option<&'static str>,
    /// What `amount` of it costs, absent when no rate is published for it there.
    cost: Option<f64>,
}

/// The usage every row of the price table models, and what it must cost.
///
/// The rows differ along the service code, the usage type, the operation (part of a
/// rate's key: the same usage type under another operation is another rate), the region
/// the usage type's prefix names — none, an explicit one, the legacy spelling of one, and
/// one this service names no region for — the amount (zero, fractional, whole, large),
/// and whether a rate is published for the usage at all.
const PRICE_CASES: &[PriceCase] = &[
    PriceCase {
        case: "no region prefix: the region AWS leaves unprefixed",
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:t3.micro",
        operation: "RunInstances",
        amount: 24.0,
        location: Some("us-east-1"),
        unit: Some("Hrs"),
        cost: Some(0.2496),
    },
    PriceCase {
        case: "an explicit region prefix, same rate",
        service_code: "AmazonEC2",
        usage_type: "USW2-BoxUsage:t3.micro",
        operation: "RunInstances",
        amount: 24.0,
        location: Some("us-west-2"),
        unit: Some("Hrs"),
        cost: Some(0.2496),
    },
    PriceCase {
        case: "a region prefix whose published rate differs",
        service_code: "AmazonEC2",
        usage_type: "EUW1-BoxUsage:t3.micro",
        operation: "RunInstances",
        amount: 10.0,
        location: Some("eu-west-1"),
        unit: Some("Hrs"),
        cost: Some(0.114),
    },
    PriceCase {
        case: "the legacy spelling of that region's prefix",
        service_code: "AmazonEC2",
        usage_type: "EU-BoxUsage:m5.large",
        operation: "RunInstances",
        amount: 2.0,
        location: Some("eu-west-1"),
        unit: Some("Hrs"),
        cost: Some(0.214),
    },
    PriceCase {
        case: "a zero amount costs nothing but is still priced",
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:c5.xlarge",
        operation: "RunInstances",
        amount: 0.0,
        location: Some("us-east-1"),
        unit: Some("Hrs"),
        cost: Some(0.0),
    },
    PriceCase {
        case: "a large fractional amount of an hourly rate",
        service_code: "AmazonEC2",
        usage_type: "BoxUsage:m5.large",
        operation: "RunInstances",
        amount: 1_234.5,
        location: Some("us-east-1"),
        unit: Some("Hrs"),
        cost: Some(118.512),
    },
    PriceCase {
        case: "another service, measured in another unit",
        service_code: "AmazonS3",
        usage_type: "TimedStorage-ByteHrs",
        operation: "StandardStorage",
        amount: 100.0,
        location: Some("us-east-1"),
        unit: Some("GB-Mo"),
        cost: Some(2.3),
    },
    PriceCase {
        case: "that same usage type under another operation is another rate, unpublished",
        service_code: "AmazonS3",
        usage_type: "TimedStorage-ByteHrs",
        operation: "PutObject",
        amount: 100.0,
        location: Some("us-east-1"),
        unit: None,
        cost: None,
    },
    PriceCase {
        case: "a known usage type at a location no rate is published for",
        service_code: "AmazonRDS",
        usage_type: "USE2-InstanceUsage:db.t3.micro",
        operation: "CreateDBInstance",
        amount: 5.0,
        location: Some("us-east-2"),
        unit: Some("Hrs"),
        cost: None,
    },
    PriceCase {
        case: "a usage type this service publishes nothing for",
        service_code: "AWSLambda",
        usage_type: "Lambda-GB-Second",
        operation: "Invoke",
        amount: 1_000.0,
        location: Some("us-east-1"),
        unit: None,
        cost: None,
    },
    PriceCase {
        case: "a region prefix this service names no region for",
        service_code: "AmazonEC2",
        usage_type: "ZZZ9-BoxUsage:t3.micro",
        operation: "RunInstances",
        amount: 24.0,
        location: None,
        unit: Some("Hrs"),
        cost: None,
    },
];

/// One `BatchCreateWorkloadEstimateUsageEntry` for `case`.
fn entry(key: &str, case: &PriceCase) -> Value {
    json!({
        "key": key,
        "serviceCode": case.service_code,
        "usageType": case.usage_type,
        "operation": case.operation,
        "usageAccountId": USAGE_ACCOUNT_ID,
        "amount": case.amount,
    })
}

/// A well-formed `ResourceId` (`len 36..36`, lowercase-hex v4 UUID form) that names no
/// stored usage line, so an entry carrying it is answered for what it names rather than
/// for its shape.
const UNKNOWN_LINE_ID: &str = "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0";

/// The two usage types the update and delete tables model, by their index into
/// [`PRICE_CASES`]: 24 hours of a t3.micro (USD 0.2496) and 1234.5 hours of an m5.large
/// (USD 118.512).
const SEEDED: [usize; 2] = [0, 5];

/// Seed an estimate holding one line per entry of [`SEEDED`] and return its id with the
/// line ids, in that order.
async fn seed_two_lines(emulator: &Emulator, name: &str) -> (String, Vec<String>, Vec<f64>) {
    let estimate = create_estimate(emulator, name).await;
    let usage: Vec<Value> = SEEDED
        .iter()
        .enumerate()
        .map(|(index, row)| entry(&format!("k{index}"), &PRICE_CASES[*row]))
        .collect();
    let (status, body) = emulator
        .call(
            "BatchCreateWorkloadEstimateUsage",
            json!({ "workloadEstimateId": estimate, "usage": usage }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding {name}: expected 200, got {status} with {body}"
    );
    let ids: Vec<String> = body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids.len(),
        SEEDED.len(),
        "seeding {name}: expected one line per entry, got {body}"
    );
    let costs: Vec<f64> = SEEDED
        .iter()
        .map(|row| PRICE_CASES[*row].cost.unwrap_or_default())
        .collect();
    (estimate, ids, costs)
}

/// The usage lines an estimate holds, as `(id, amount, cost)` in stored order.
async fn listed_lines(emulator: &Emulator, estimate: &str) -> Vec<(String, f64, Option<f64>)> {
    let (status, body) = emulator
        .call(
            "ListWorkloadEstimateUsage",
            json!({ "workloadEstimateId": estimate }),
        )
        .await;
    assert_eq!(
        status, 200,
        "listing {estimate}: expected 200, got {status} with {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap_or_default().to_string(),
                item["quantity"]["amount"].as_f64().unwrap_or_default(),
                item["cost"].as_f64(),
            )
        })
        .collect()
}

/// The `totalCost` the single-estimate reader reports.
async fn total_cost(emulator: &Emulator, estimate: &str) -> f64 {
    let (status, body) = emulator
        .call("GetWorkloadEstimate", json!({ "identifier": estimate }))
        .await;
    assert_eq!(
        status, 200,
        "reading {estimate}: expected 200, got {status} with {body}"
    );
    body["totalCost"]
        .as_f64()
        .unwrap_or_else(|| panic!("reading {estimate}: no totalCost in {body}"))
}

/// One row of the update table.
struct UpdateCase {
    case: &'static str,
    /// The new `amount` the entry sends, `None` when it sends none.
    amount: Option<f64>,
    /// The new `group` the entry sends, `None` when it sends none.
    group: Option<&'static str>,
    /// Whether the entry names a line the estimate does not hold.
    unknown_id: bool,
}

/// RULE: an update applies each entry's members to the usage line its `id` names, prices
/// that line again from the usage it now models — so its `cost` is the published rate
/// times the NEW amount — and lands the estimate's cost as the sum over its lines; a
/// member the entry omits leaves the line as it was, and an `id` the estimate does not
/// hold fails only that entry, in `errors`, changing nothing.
///
/// The table spans the new amount (larger, zero, fractional), whether the group moves
/// with it, an entry that changes only the group, and an entry naming no stored line.
#[tokio::test]
async fn a_batch_update_reprices_the_line_and_the_estimate() {
    let cases: &[UpdateCase] = &[
        UpdateCase {
            case: "a larger amount",
            amount: Some(48.0),
            group: None,
            unknown_id: false,
        },
        UpdateCase {
            case: "an amount of zero",
            amount: Some(0.0),
            group: None,
            unknown_id: false,
        },
        UpdateCase {
            case: "a fractional amount and a group",
            amount: Some(1.5),
            group: Some("grp9"),
            unknown_id: false,
        },
        UpdateCase {
            case: "only the group: the cost does not move",
            amount: None,
            group: Some("group-a"),
            unknown_id: false,
        },
        UpdateCase {
            case: "an id the estimate does not hold",
            amount: Some(10.0),
            group: None,
            unknown_id: true,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let (estimate, ids, costs) = seed_two_lines(&emulator, "updated").await;
        let seeded_total = costs.iter().sum::<f64>();
        assert_close(
            &format!("case {:?} (seeded total)", case.case),
            total_cost(&emulator, &estimate).await,
            seeded_total,
        );

        let target = if case.unknown_id {
            UNKNOWN_LINE_ID.to_string()
        } else {
            ids[0].clone()
        };
        let mut entry = json!({ "id": target });
        if let Some(amount) = case.amount {
            entry["amount"] = json!(amount);
        }
        if let Some(group) = case.group {
            entry["group"] = json!(group);
        }
        let (status, body) = emulator
            .call(
                "BatchUpdateWorkloadEstimateUsage",
                json!({ "workloadEstimateId": estimate, "usage": [entry] }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: a per-entry failure is not a request failure; got {status} with {body}",
            case.case
        );

        let items = body["items"].as_array().cloned().unwrap_or_default();
        let errors = body["errors"].as_array().cloned().unwrap_or_default();
        if case.unknown_id {
            assert!(
                items.is_empty(),
                "case {}: expected no updated item, got {body}",
                case.case
            );
            assert_eq!(
                errors.len(),
                1,
                "case {}: expected one per-entry error, got {body}",
                case.case
            );
            assert_eq!(
                errors[0]["id"].as_str(),
                Some(UNKNOWN_LINE_ID),
                "case {}: expected the error to name the id it looked for, got {body}",
                case.case
            );
            assert_eq!(
                errors[0]["errorCode"].as_str(),
                Some("NOT_FOUND"),
                "case {}: expected the NOT_FOUND code, got {body}",
                case.case
            );
            // Nothing moved.
            let lines = listed_lines(&emulator, &estimate).await;
            assert_eq!(
                lines.iter().map(|line| line.2).collect::<Vec<_>>(),
                costs.iter().map(|cost| Some(*cost)).collect::<Vec<_>>(),
                "case {}: expected the refused entry to change nothing, got {lines:?}",
                case.case
            );
            assert_close(
                &format!("case {:?} (total)", case.case),
                total_cost(&emulator, &estimate).await,
                seeded_total,
            );
            continue;
        }

        assert!(
            errors.is_empty(),
            "case {}: expected no per-entry errors, got {body}",
            case.case
        );
        assert_eq!(
            items.len(),
            1,
            "case {}: expected one updated item, got {body}",
            case.case
        );

        // The rate is the one the line's usage type publishes; only the amount moved.
        let expected_amount = case.amount.unwrap_or(PRICE_CASES[SEEDED[0]].amount);
        let rate = PRICE_CASES[SEEDED[0]].cost.unwrap_or_default() / PRICE_CASES[SEEDED[0]].amount;
        let expected_cost = rate * expected_amount;
        assert_eq!(
            items[0]["quantity"]["amount"].as_f64(),
            Some(expected_amount),
            "case {}: expected the new amount reported, got {body}",
            case.case
        );
        assert_close(
            &format!("case {:?} (item cost)", case.case),
            items[0]["cost"].as_f64().unwrap_or_default(),
            expected_cost,
        );
        if let Some(group) = case.group {
            assert_eq!(
                items[0]["group"].as_str(),
                Some(group),
                "case {}: expected the new group reported, got {body}",
                case.case
            );
        }

        // The reader reports the updated line, and the estimate the recomputed total.
        let lines = listed_lines(&emulator, &estimate).await;
        assert_eq!(
            lines.len(),
            SEEDED.len(),
            "case {}: an update must not add or drop a line, got {lines:?}",
            case.case
        );
        assert_eq!(
            lines[0].0, ids[0],
            "case {}: expected the same line id, got {lines:?}",
            case.case
        );
        assert_eq!(
            lines[0].1, expected_amount,
            "case {}: expected the stored amount {expected_amount}, got {lines:?}",
            case.case
        );
        assert_close(
            &format!("case {:?} (stored cost)", case.case),
            lines[0].2.unwrap_or_default(),
            expected_cost,
        );
        assert_close(
            &format!("case {:?} (total)", case.case),
            total_cost(&emulator, &estimate).await,
            expected_cost + costs[1],
        );
    }
}

/// RULE: a delete removes every usage line whose id the request names from the estimate
/// and lands the cost summed over the lines it left — and an id the estimate does not
/// hold fails only that entry, in `errors`, while the ids that do resolve are still
/// removed.
///
/// The table spans which of the stored lines the request names: one, all of them, none
/// of them (an id nothing holds), and a mix of a stored id and an unknown one.
#[tokio::test]
async fn a_batch_delete_removes_the_lines_and_rerolls_the_cost() {
    // Each row: which seeded indexes to delete, how many unknown ids to add.
    let cases: &[(&str, &[usize], usize)] = &[
        ("one of two lines", &[0], 0),
        ("the other of two lines", &[1], 0),
        ("both lines", &[0, 1], 0),
        ("an id nothing holds", &[], 1),
        ("a stored id beside an unknown one", &[1], 1),
    ];

    for (case, deleted, unknown) in cases {
        let emulator = Emulator::new();
        let (estimate, ids, costs) = seed_two_lines(&emulator, "deleted").await;

        let mut request_ids: Vec<String> =
            deleted.iter().map(|index| ids[*index].clone()).collect();
        for _ in 0..*unknown {
            request_ids.push(UNKNOWN_LINE_ID.to_string());
        }
        let (status, body) = emulator
            .call(
                "BatchDeleteWorkloadEstimateUsage",
                json!({ "workloadEstimateId": estimate, "ids": request_ids }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: a per-entry failure is not a request failure; got {status} with {body}"
        );
        let errors = body["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            *unknown,
            "case {case}: expected {unknown} per-entry error(s), got {body}"
        );
        for error in &errors {
            assert_eq!(
                error["errorCode"].as_str(),
                Some("NOT_FOUND"),
                "case {case}: expected the NOT_FOUND code, got {body}"
            );
            assert_eq!(
                error["id"].as_str(),
                Some(UNKNOWN_LINE_ID),
                "case {case}: expected the error to name the id it looked for, got {body}"
            );
        }

        // Exactly the named lines are gone, the others keep their place and their cost.
        let kept: Vec<usize> = (0..SEEDED.len())
            .filter(|index| !deleted.contains(index))
            .collect();
        let lines = listed_lines(&emulator, &estimate).await;
        assert_eq!(
            lines.iter().map(|line| line.0.clone()).collect::<Vec<_>>(),
            kept.iter().map(|index| ids[*index].clone()).collect::<Vec<_>>(),
            "case {case}: expected the surviving lines, got {lines:?}"
        );
        assert_close(
            &format!("case {case:?} (total)"),
            total_cost(&emulator, &estimate).await,
            kept.iter().map(|index| costs[*index]).sum(),
        );
    }
}

/// Fail unless `got` is `want` to a relative tolerance: a cost is a rate times an amount,
/// so the comparison allows the last bits of binary floating-point error rather than
/// demanding a bit-for-bit match.
fn assert_close(label: &str, got: f64, want: f64) {
    let tolerance = 1e-9 * want.abs().max(1.0);
    assert!(
        (got - want).abs() <= tolerance,
        "{label}: expected {want} (±{tolerance}), got {got}"
    );
}

/// Fail unless `item`'s price members are the ones `case` states.
fn assert_priced_as(label: &str, item: &Value, case: &PriceCase) {
    assert_eq!(
        item["location"].as_str(),
        case.location,
        "{label}: expected location {:?}, got {item}",
        case.location
    );
    assert_eq!(
        item["quantity"]["unit"].as_str(),
        case.unit,
        "{label}: expected quantity.unit {:?}, got {item}",
        case.unit
    );
    assert_eq!(
        item["quantity"]["amount"].as_f64(),
        Some(case.amount),
        "{label}: expected quantity.amount {}, got {item}",
        case.amount
    );
    match case.cost {
        Some(want) => {
            let got = item["cost"]
                .as_f64()
                .unwrap_or_else(|| panic!("{label}: expected a cost of {want}, got {item}"));
            assert_close(label, got, want);
            assert_eq!(
                item["currency"].as_str(),
                Some(state::COST_CURRENCY),
                "{label}: expected the modelled currency beside the cost, got {item}"
            );
            assert_eq!(
                item["status"].as_str(),
                Some(state::COST_STATUS_VALID),
                "{label}: expected a priced line's cost status, got {item}"
            );
        }
        None => {
            assert!(
                item["cost"].is_null(),
                "{label}: no rate is published for this usage, so no cost may be \
                 reported, got {item}"
            );
            assert!(
                item["currency"].is_null(),
                "{label}: a line with no cost has no currency to report it in, got {item}"
            );
            assert_eq!(
                item["status"].as_str(),
                Some(state::COST_STATUS_INVALID),
                "{label}: expected an unpriced line's cost status, got {item}"
            );
        }
    }
}

/// RULE: a usage line is priced from the rate this service publishes for its usage type:
/// the line reports the location its usage type's region prefix names (the unprefixed
/// region when it carries none, and nothing at all when the prefix names no region this
/// service knows), the unit that usage type is measured in, and the published rate at
/// that location times the amount as its `cost`, with `currency` and the cost `status`
/// qualifying it — and where no rate is published for that usage at that location, no
/// cost is reported and the cost status says so.
///
/// Every row is asserted through BOTH readers of a line (the batch that creates it and
/// `ListWorkloadEstimateUsage`), so the two cannot disagree.
#[tokio::test]
async fn a_usage_line_is_priced_from_the_published_rate_for_its_usage_type() {
    for case in PRICE_CASES {
        let emulator = Emulator::new();
        let estimate = create_estimate(&emulator, "priced").await;

        let (status, batch) = emulator
            .call(
                "BatchCreateWorkloadEstimateUsage",
                json!({
                    "workloadEstimateId": estimate,
                    "usage": [entry("k0", case)],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the batch, got {status} with {batch}",
            case.case
        );
        let created = batch["items"]
            .as_array()
            .and_then(|items| items.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {}: no item in {batch}", case.case));
        assert_priced_as(
            &format!("case {:?} (batch)", case.case),
            &created,
            case,
        );

        let (status, listed) = emulator
            .call(
                "ListWorkloadEstimateUsage",
                json!({ "workloadEstimateId": estimate }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the list, got {status} with {listed}",
            case.case
        );
        let read_back = listed["items"]
            .as_array()
            .and_then(|items| items.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {}: no item in {listed}", case.case));
        assert_priced_as(
            &format!("case {:?} (list)", case.case),
            &read_back,
            case,
        );

        // And the estimate the line belongs to reports that same cost as its total, with
        // the status its lines leave it in.
        let (status, fetched) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": estimate }))
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the reader, got {status} with {fetched}",
            case.case
        );
        let total = fetched["totalCost"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: no totalCost in {fetched}", case.case));
        assert_close(
            &format!("case {:?} (totalCost)", case.case),
            total,
            case.cost.unwrap_or(0.0),
        );
        let expected_status = if case.cost.is_some() {
            state::WORKLOAD_ESTIMATE_STATUS_VALID
        } else {
            state::WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED
        };
        assert_eq!(
            fetched["status"].as_str(),
            Some(expected_status),
            "case {}: expected status {expected_status}, got {fetched}",
            case.case
        );
    }
}

/// One roll-up row: the usage of each batch, in order.
struct TotalCase {
    case: &'static str,
    /// One list of `(usage type index into [`PRICE_CASES`])` per batch call.
    batches: &'static [&'static [usize]],
}

/// RULE: an estimate's `totalCost` is the sum over the costs of the usage lines it holds,
/// however many calls added them, and its status is the terminal one exactly while every
/// line it holds is priced — an unpriced line leaves it needing the caller's attention,
/// with a `failureMessage` naming the usage type nothing could be priced for.
#[tokio::test]
async fn the_estimate_total_is_the_sum_over_the_lines_it_holds() {
    let cases: &[TotalCase] = &[
        TotalCase {
            case: "no usage at all",
            batches: &[],
        },
        TotalCase {
            case: "one priced line",
            batches: &[&[0]],
        },
        TotalCase {
            case: "the same usage added by two calls",
            batches: &[&[0], &[0]],
        },
        TotalCase {
            case: "several usage types in one call",
            batches: &[&[0, 2, 6]],
        },
        TotalCase {
            case: "priced and unpriced lines together",
            batches: &[&[0], &[9]],
        },
        TotalCase {
            case: "only unpriced lines",
            batches: &[&[9, 7]],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let estimate = create_estimate(&emulator, "rollup").await;

        let mut expected_total = 0.0;
        let mut any_unpriced = false;
        for (batch_index, batch) in case.batches.iter().enumerate() {
            let usage: Vec<Value> = batch
                .iter()
                .enumerate()
                .map(|(index, row)| entry(&format!("k{index}"), &PRICE_CASES[*row]))
                .collect();
            let (status, body) = emulator
                .call(
                    "BatchCreateWorkloadEstimateUsage",
                    json!({ "workloadEstimateId": estimate, "usage": usage }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {} batch={batch_index}: expected 200, got {status} with {body}",
                case.case
            );
            for row in batch.iter() {
                match PRICE_CASES[*row].cost {
                    Some(cost) => expected_total += cost,
                    None => any_unpriced = true,
                }
            }
        }

        let (status, fetched) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": estimate }))
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the reader, got {status} with {fetched}",
            case.case
        );
        let total = fetched["totalCost"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {}: no totalCost in {fetched}", case.case));
        assert_close(
            &format!("case {:?} (totalCost)", case.case),
            total,
            expected_total,
        );
        assert_eq!(
            fetched["costCurrency"].as_str(),
            Some(state::COST_CURRENCY),
            "case {}: expected the modelled currency, got {fetched}",
            case.case
        );

        if any_unpriced {
            assert_eq!(
                fetched["status"].as_str(),
                Some(state::WORKLOAD_ESTIMATE_STATUS_ACTION_NEEDED),
                "case {}: an estimate holding an unpriced line needs attention, got {fetched}",
                case.case
            );
            let message = fetched["failureMessage"].as_str().unwrap_or_default();
            assert!(
                !message.is_empty(),
                "case {}: expected a failureMessage naming the unpriced usage, got {fetched}",
                case.case
            );
            for row in case.batches.iter().flat_map(|batch| batch.iter()) {
                let row = &PRICE_CASES[*row];
                if row.cost.is_none() {
                    assert!(
                        message.contains(row.usage_type),
                        "case {}: expected the failureMessage to name {}, got {message:?}",
                        case.case,
                        row.usage_type
                    );
                }
            }
        } else {
            assert_eq!(
                fetched["status"].as_str(),
                Some(state::WORKLOAD_ESTIMATE_STATUS_VALID),
                "case {}: every line is priced, so the estimate is valid, got {fetched}",
                case.case
            );
            assert_eq!(
                fetched["failureMessage"],
                Value::Null,
                "case {}: expected no failureMessage, got {fetched}",
                case.case
            );
        }
    }
}

/// One rename row: what the update sends, and the name the readers must then report.
struct RenameCase {
    case: &'static str,
    /// The `name` the update sends, `None` when it sends none.
    name: Option<&'static str>,
    /// Whether the update also moves `expiresAt` (one hour later than it was).
    moves_expiry: bool,
}

/// RULE: an update writes the members it carries into the stored estimate, so every
/// reader of that estimate afterwards reports the new values and nothing else about it
/// moves; a member the update omits is left as it was.
///
/// The table spans the shape of the new name (`WorkloadEstimateName` is `len ..64`,
/// pattern `^[a-zA-Z0-9-]+$`: hyphenated, mixed case with digits, the longest the model
/// accepts, the shortest, and the name it already had), whether `expiresAt` moves with
/// it, and an update that carries no name at all.
#[tokio::test]
async fn an_update_persists_what_it_changed_for_every_reader() {
    let cases: &[RenameCase] = &[
        RenameCase {
            case: "a hyphenated name",
            name: Some("renamed-estimate"),
            moves_expiry: false,
        },
        RenameCase {
            case: "mixed case and digits",
            name: Some("Renamed9Estimate"),
            moves_expiry: false,
        },
        RenameCase {
            case: "the longest name the model accepts",
            name: Some("r234567890123456789012345678901234567890123456789012345678901234"),
            moves_expiry: false,
        },
        RenameCase {
            case: "the shortest name the model accepts",
            name: Some("r"),
            moves_expiry: false,
        },
        RenameCase {
            case: "the name it already had",
            name: Some("original"),
            moves_expiry: false,
        },
        RenameCase {
            case: "a rename that also moves the expiry",
            name: Some("renamed-and-moved"),
            moves_expiry: true,
        },
        RenameCase {
            case: "no name at all: the stored one survives",
            name: None,
            moves_expiry: true,
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let (status, created) = emulator
            .call("CreateWorkloadEstimate", json!({ "name": "original" }))
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the create, got {status} with {created}",
            case.case
        );
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let created_expiry = created["expiresAt"].as_f64().unwrap_or_default() as i64;

        let mut request = json!({ "identifier": id });
        if let Some(name) = case.name {
            request["name"] = json!(name);
        }
        let new_expiry = created_expiry + 3_600;
        if case.moves_expiry {
            request["expiresAt"] = json!(new_expiry);
        }

        let (status, updated) = emulator.call("UpdateWorkloadEstimate", request).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the update, got {status} with {updated}",
            case.case
        );

        let expected_name = case.name.unwrap_or("original");
        let expected_expiry = if case.moves_expiry {
            new_expiry
        } else {
            created_expiry
        };
        assert_eq!(
            updated["name"].as_str(),
            Some(expected_name),
            "case {}: expected the update to report the name {expected_name:?}, got {updated}",
            case.case
        );
        assert_eq!(
            updated["id"].as_str(),
            Some(id.as_str()),
            "case {}: expected the update to report the same id, got {updated}",
            case.case
        );

        // Every reader of the estimate reports the update, not the pre-update state.
        let (status, fetched) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": id }))
            .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the reader, got {status} with {fetched}",
            case.case
        );
        assert_eq!(
            fetched["name"].as_str(),
            Some(expected_name),
            "case {}: expected the stored name {expected_name:?}, got {fetched}",
            case.case
        );
        assert_eq!(
            fetched["expiresAt"].as_f64().unwrap_or_default() as i64,
            expected_expiry,
            "case {}: expected expiresAt {expected_expiry}, got {fetched}",
            case.case
        );
        // Nothing but the members the update carried moved.
        for field in [
            "id",
            "createdAt",
            "rateType",
            "rateTimestamp",
            "status",
            "totalCost",
            "costCurrency",
        ] {
            assert_eq!(
                fetched[field], created[field],
                "case {}: expected the update to leave {field} at {:?}, got {:?}",
                case.case, created[field], fetched[field]
            );
        }
        // And the update itself answered with what the reader now reports.
        for field in ["id", "name", "expiresAt", "createdAt", "status", "totalCost"] {
            assert_eq!(
                updated[field], fetched[field],
                "case {}: expected the update's {field} {:?} to be the reader's {:?}",
                case.case, updated[field], fetched[field]
            );
        }

        let (status, listed) = emulator.call("ListWorkloadEstimates", json!({})).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from the list, got {status} with {listed}",
            case.case
        );
        let names: Vec<String> = listed["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names,
            vec![expected_name.to_string()],
            "case {}: expected the list to report the updated name, got {listed}",
            case.case
        );
    }
}

/// RULE: an `expiresAt` the retention window has already passed is refused with a 400
/// `ValidationException` naming the member, rather than stored — storing it would answer
/// a 200 describing an estimate the very next reader cannot find — and the refusal
/// changes nothing.
///
/// The table spans the instant on each side of, and exactly at, the boundary.
#[tokio::test]
async fn an_expiry_in_the_past_is_refused_and_changes_nothing() {
    let offsets: [(i64, bool); 5] = [
        (-30 * 24 * 60 * 60, false),
        (-3_600, false),
        (-1, false),
        (0, false),
        (3_600, true),
    ];

    for (offset, accepted) in offsets {
        let case = format!("expiresAt = now{offset:+}");
        let emulator = Emulator::new();
        let id = create_estimate(&emulator, "expiry").await;
        let (status, before) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": &id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the reader, got {status} with {before}"
        );

        let (status, body) = emulator
            .call(
                "UpdateWorkloadEstimate",
                json!({ "identifier": &id, "expiresAt": state::now_secs() + offset }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected a future expiry to be accepted, got {status} with {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains("expiresAt"),
            "case {case}: expected the refusal to name the member, got {body}"
        );

        let (status, after) = emulator
            .call("GetWorkloadEstimate", json!({ "identifier": &id }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the estimate to survive the refusal, got {status} with {after}"
        );
        assert_eq!(
            after, before,
            "case {case}: expected the refusal to change nothing, got {after}"
        );
    }
}
