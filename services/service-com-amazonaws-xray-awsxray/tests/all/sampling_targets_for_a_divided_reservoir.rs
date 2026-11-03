// SPDX-License-Identifier: CC0-1.0

//! `GetSamplingTargets` answers a reported statistics document out of the
//! DIVISION of the named rule's reservoir, end to end over the wire.
//!
//! The rule these tables check: the target computation is keyed by rule NAME —
//! which is all a `SamplingTargetDocument` identifies what it allocates by — and
//! answers out of the division it last made for that name, so once a rule's
//! reservoir has been divided EVERY statistics document naming it is answered a
//! target document (the rate the division was made under, the share the
//! reporting service holds or, for a service that was in no division, what the
//! zero-sum pool left unclaimed, and an expiry computed from the call time),
//! whatever `ClientID` reports it and whether or not the account still holds the
//! rule; a document is answered in `UnprocessedStatistics` only when the account
//! holds no rule under the name AND no division of its reservoir stands.
//!
//! Rows differ along the size of the pool (none at all, one request, several,
//! thousands), how many services were in the division (one, two, three, four),
//! how much of the pool their traffic claimed (all of it, part of it, none of
//! it), the rate the rule carries, and whether the rule is deleted before the
//! newcomer reports.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_sampling_targets::UNKNOWN_RULE_ERROR_CODE;
use service_com_amazonaws_xray_awsxray::state::{
    SAMPLING_TARGET_INTERVAL_SECONDS, UNKNOWN_SAMPLING_RULE_MESSAGE,
};

use crate::sampling_targets::{
    client_id, create_rule, get_targets, list, now, statistics, target_for,
};
use crate::xray_wire::{Answer, Xray};

/// The rule every row of the first table configures and divides.
const RULE: &str = "divided-reservoir";

/// A `ClientID` that was in no division — the brand-new reporter.
const NEWCOMER: u32 = 99;

/// What one answer served for a rule: its rate, the share it granted and the
/// cadence that share came with.
type Served = (Option<f64>, Option<i64>, Option<i64>);

/// One row: a rule's configuration, the services that were in the division of
/// its reservoir, and what that division left unclaimed.
struct Case {
    case: &'static str,
    /// The rule's `FixedRate`, which the served document must carry.
    fixed_rate: f64,
    /// The rule's `ReservoirSize`: the pool the division shares out.
    reservoir_size: i64,
    /// The `RequestCount` each service in the division reported.
    reported: Vec<i64>,
    /// What the division left UNCLAIMED, written down rather than recomputed:
    /// the pool less the floored share each reporting service's traffic earned
    /// it (services reporting no traffic at all split the pool evenly).
    unclaimed: i64,
}

/// The table.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one service reporting everything claims the whole pool",
            fixed_rate: 0.42,
            reservoir_size: 15,
            reported: vec![500],
            // 15 of 15 to the only service in the division.
            unclaimed: 0,
        },
        Case {
            case: "two services halving the traffic halve the pool",
            fixed_rate: 0.5,
            reservoir_size: 10,
            reported: vec![50, 50],
            // 5 + 5 of 10.
            unclaimed: 0,
        },
        Case {
            case: "three equal services leave the floored remainder",
            fixed_rate: 0.123_456_789_012_345,
            reservoir_size: 10,
            reported: vec![1, 1, 1],
            // floor(10 / 3) = 3 each, so 9 of 10 are claimed.
            unclaimed: 1,
        },
        Case {
            case: "a one-request pool nobody's share rounds up to",
            fixed_rate: 0.0,
            reservoir_size: 1,
            reported: vec![90, 10],
            // floor(1 * 0.9) = floor(1 * 0.1) = 0, so the whole pool is free.
            unclaimed: 1,
        },
        Case {
            case: "four services reporting no traffic split the pool evenly",
            fixed_rate: 0.25,
            reservoir_size: 8,
            reported: vec![0, 0, 0, 0],
            // 8 / 4 = 2 each.
            unclaimed: 0,
        },
        Case {
            case: "no reservoir at all, at the modelled maximum rate",
            fixed_rate: 1.0,
            reservoir_size: 0,
            reported: vec![20],
            // There is no pool, so there is nothing to claim or to leave.
            unclaimed: 0,
        },
        Case {
            case: "a reservoir of thousands, one claimant",
            fixed_rate: 0.9,
            reservoir_size: 2500,
            reported: vec![7],
            unclaimed: 0,
        },
    ]
}

/// What the answer served for `rule_name`: its rate, its share and its cadence.
fn served(answer: &Answer, rule_name: &str, case: &str) -> Served {
    let target = target_for(answer, rule_name).unwrap_or_else(|| {
        panic!("case {case:?}: a reported rule must be answered a target, got {answer:?}")
    });
    (
        target.get("FixedRate").and_then(Value::as_f64),
        target.get("ReservoirQuota").and_then(Value::as_i64),
        target.get("Interval").and_then(Value::as_i64),
    )
}

/// Create the row's rule and divide its reservoir between the services that
/// reported: ONE call records every report, and the cycle that follows it
/// shares the pool out between them.
async fn divide(xray: &Xray, case: &Case, at: f64) {
    let created = create_rule(xray, RULE, 100, case.fixed_rate, case.reservoir_size, None).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: CreateSamplingRule must succeed, got {created:?}",
        case.case
    );
    let documents: Vec<Value> = case
        .reported
        .iter()
        .enumerate()
        .map(|(index, count)| statistics(RULE, &in_division(index), at, *count, 0, 0))
        .collect();
    let dividing = get_targets(xray, &documents).await;
    assert_eq!(
        dividing.status, 200,
        "case {:?}: GetSamplingTargets must succeed, got {dividing:?}",
        case.case
    );
}

/// The `ClientID` of the n-th service in the division.
fn in_division(index: usize) -> String {
    client_id(u32::try_from(index).expect("the index fits a u32") + 1)
}

/// `DeleteSamplingRule` for `name`, checking the rule store really no longer
/// holds it.
async fn delete_rule(xray: &Xray, case: &str, name: &str) {
    let removed = xray
        .call("/DeleteSamplingRule", &json!({"RuleName": name}))
        .await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: DeleteSamplingRule must succeed, got {removed:?}"
    );
    let listed = xray.call("/GetSamplingRules", &json!({})).await;
    let names: Vec<String> = list(&listed, "SamplingRuleRecords")
        .iter()
        .filter_map(|record| {
            record
                .get("SamplingRule")
                .and_then(|rule| rule.get("RuleName"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        !names.iter().any(|listed| listed == name),
        "case {case:?}: the deleted rule must be gone from the rule store, got {names:?}"
    );
}

#[tokio::test]
async fn every_client_reporting_for_a_divided_reservoir_is_answered_a_target() {
    for case in cases() {
        // The same row is driven twice: once with the rule deleted before the
        // newcomer reports, once with it left in place. The two must agree,
        // because the division is what the target is computed from.
        let mut answered: Vec<(bool, Served)> = Vec::new();
        for deleted in [false, true] {
            let xray = Xray::new();
            let at = now();
            divide(&xray, &case, at).await;
            if deleted {
                delete_rule(&xray, case.case, RULE).await;
            }

            // A brand-new `ClientID`, which was in no division, is answered the
            // rate the division was made under and whatever the pool has left
            // unclaimed.
            let asked_at = now();
            let newcomer =
                get_targets(&xray, &[statistics(RULE, &client_id(NEWCOMER), at, 1, 1, 0)]).await;
            assert_eq!(
                newcomer.status, 200,
                "case {:?} (deleted: {deleted}): GetSamplingTargets must succeed, got {newcomer:?}",
                case.case
            );
            assert!(
                list(&newcomer, "UnprocessedStatistics").is_empty(),
                "case {:?} (deleted: {deleted}): a client in no division must not be declined, got {newcomer:?}",
                case.case
            );
            let (fixed_rate, quota, interval) = served(&newcomer, RULE, case.case);
            assert_eq!(
                fixed_rate,
                Some(case.fixed_rate),
                "case {:?} (deleted: {deleted}): expected the rule's rate {}, got {fixed_rate:?}",
                case.case,
                case.fixed_rate
            );
            assert_eq!(
                quota,
                Some(case.unclaimed),
                "case {:?} (deleted: {deleted}): expected the unclaimed {} of a pool of {}, got {quota:?}",
                case.case,
                case.unclaimed,
                case.reservoir_size
            );
            assert_eq!(
                interval,
                Some(i64::from(SAMPLING_TARGET_INTERVAL_SECONDS)),
                "case {:?} (deleted: {deleted}): a granted share comes with the cadence to come back at, got {interval:?}",
                case.case
            );
            // The expiry is computed from THIS call, not from the division.
            let target = target_for(&newcomer, RULE).expect("the target was asserted above");
            let ttl = target.get("ReservoirQuotaTTL").and_then(Value::as_f64);
            let span = f64::from(SAMPLING_TARGET_INTERVAL_SECONDS);
            assert!(
                ttl.is_some_and(|ttl| ttl > asked_at && ttl - asked_at <= span + 1.0),
                "case {:?} (deleted: {deleted}): the share must expire within {span}s of the call at {asked_at}, got {ttl:?}",
                case.case
            );

            if deleted {
                // A poller that keeps reporting for the departed rule keeps
                // being answered: nothing about the first report turned it away.
                let again =
                    get_targets(&xray, &[statistics(RULE, &client_id(NEWCOMER), at, 2, 1, 0)]).await;
                assert!(
                    list(&again, "UnprocessedStatistics").is_empty(),
                    "case {:?}: a repeated report for a deleted rule stays answered, got {again:?}",
                    case.case
                );
                assert_eq!(
                    served(&again, RULE, case.case).1,
                    Some(case.unclaimed),
                    "case {:?}: the standing division answers the same share again, got {again:?}",
                    case.case
                );
            }

            // Every service that WAS in the division is still answered the
            // share it holds, and those shares are what the pool gave away.
            let mut claimed = 0_i64;
            for index in 0..case.reported.len() {
                let answer =
                    get_targets(&xray, &[statistics(RULE, &in_division(index), at, 10, 1, 0)]).await;
                assert!(
                    list(&answer, "UnprocessedStatistics").is_empty(),
                    "case {:?} (deleted: {deleted}): a service in the division is answered, not declined, got {answer:?}",
                    case.case
                );
                claimed += served(&answer, RULE, case.case).1.unwrap_or_else(|| {
                    panic!(
                        "case {:?} (deleted: {deleted}): a divided pool grants a share, got {answer:?}",
                        case.case
                    )
                });
            }
            assert_eq!(
                claimed,
                case.reservoir_size - case.unclaimed,
                "case {:?} (deleted: {deleted}): the division gave away {} of a pool of {}, got {claimed}",
                case.case,
                case.reservoir_size - case.unclaimed,
                case.reservoir_size
            );

            answered.push((deleted, (fixed_rate, quota, interval)));
        }

        let in_store = answered
            .iter()
            .find(|(deleted, _)| !*deleted)
            .map(|(_, served)| *served);
        let gone = answered
            .iter()
            .find(|(deleted, _)| *deleted)
            .map(|(_, served)| *served);
        assert_eq!(
            in_store, gone,
            "case {:?}: the target is computed from the division, so the rule store must not change it, got {in_store:?} then {gone:?}",
            case.case
        );
    }
}

/// What the account holds under a name no division stands for.
#[derive(Debug, Clone, Copy)]
enum Seed {
    /// Nothing at all was created.
    Nothing,
    /// A rule exists, but under the name in another case: the store is keyed by
    /// the name as it was created.
    TheNameInAnotherCase,
    /// A rule was created under the name and deleted again, with no statistics
    /// ever reported for it, so no cycle ever divided its reservoir.
    CreatedThenDeleted,
}

#[tokio::test]
async fn a_rule_name_no_division_stands_for_is_unprocessed() {
    // Rows: the boundary of the rule — a name the account holds no rule under
    // and whose reservoir no cycle ever divided.
    let rows = [
        ("a name nothing was created under", Seed::Nothing),
        (
            "a created rule's name in another case",
            Seed::TheNameInAnotherCase,
        ),
        (
            "a rule created and deleted with nothing ever reporting",
            Seed::CreatedThenDeleted,
        ),
    ];

    for (index, (label, seed)) in rows.into_iter().enumerate() {
        let xray = Xray::new();
        let name = format!("absent-{index}");
        match seed {
            Seed::Nothing => {}
            Seed::TheNameInAnotherCase => {
                let created = create_rule(&xray, &name.to_uppercase(), 100, 0.3, 4, None).await;
                assert_eq!(
                    created.status, 200,
                    "case {label:?}: seeding must succeed, got {created:?}"
                );
            }
            Seed::CreatedThenDeleted => {
                let created = create_rule(&xray, &name, 100, 0.3, 4, None).await;
                assert_eq!(
                    created.status, 200,
                    "case {label:?}: seeding must succeed, got {created:?}"
                );
                delete_rule(&xray, label, &name).await;
            }
        }

        let answer =
            get_targets(&xray, &[statistics(&name, &client_id(1), now(), 10, 1, 0)]).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: GetSamplingTargets must still succeed, got {answer:?}"
        );
        assert!(
            target_for(&answer, &name).is_none(),
            "case {label:?}: no division stands for the name, so no target may be served, got {answer:?}"
        );
        let unprocessed = list(&answer, "UnprocessedStatistics");
        assert_eq!(
            unprocessed.len(),
            1,
            "case {label:?}: exactly the reported document is declined, got {unprocessed:?}"
        );
        let entry = &unprocessed[0];
        for (member, expected) in [
            ("RuleName", name.as_str()),
            ("ErrorCode", UNKNOWN_RULE_ERROR_CODE),
            ("Message", UNKNOWN_SAMPLING_RULE_MESSAGE),
        ] {
            let value = entry.get(member).and_then(Value::as_str);
            assert_eq!(
                value,
                Some(expected),
                "case {label:?}: the declined entry's {member} must be {expected:?}, got {entry:?}"
            );
        }
    }
}
