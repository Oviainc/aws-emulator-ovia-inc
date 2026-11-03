// SPDX-License-Identifier: CC0-1.0

//! `GetSamplingTargets` -> `GetSamplingStatisticSummaries`, end to end over the
//! wire.
//!
//! Every case drives the SAME front door the production router does, through the
//! shared `Xray`/`Answer` harness, so the assertions are about HTTP responses
//! rather than internal state.
//!
//! The rules these tables check:
//!
//! * a reported rule is answered ONE `SamplingTargetDocument` carrying the
//!   rule's STORED `FixedRate` (to its full precision), so a rule created or
//!   updated with another rate serves that rate from the next call on, and the
//!   reporting interval the share it grants comes with
//!   (`sampling_target_cadence_and_processed_window.rs` owns that rule);
//! * a service is told the allocation of a cycle it already took part in, so its
//!   FIRST report carries no reservoir share at all, and deleting the rule does
//!   not cancel a share already granted;
//! * a rule's `ReservoirSize` is a pool shared by every service reporting for
//!   it: each service is allocated the share of it its own reported
//!   `RequestCount` is of everything reported, the shares never add up to more
//!   than the pool, and the only service reporting gets all of it;
//! * statistics reported against a rule the account does not hold and no
//!   division of a reservoir stands for are answered in `UnprocessedStatistics`,
//!   never with an allocation;
//! * the documents arrive in the `Priority` order a service applies its rules
//!   in;
//! * reported statistics are READ BACK by `GetSamplingStatisticSummaries`,
//!   accumulated per rule into ten-second windows, counter for counter and to
//!   the full range of the wire member — and reading them changes nothing;
//! * a rule is boosted only where its configuration asked for it, at a rate no
//!   greater than the configured maximum and at most once per cooldown window.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_sampling_statistic_summaries::SAMPLING_STATISTIC_SUMMARIES_PAGE_SIZE;
use service_com_amazonaws_xray_awsxray::ops::get_sampling_targets::UNKNOWN_RULE_ERROR_CODE;
use service_com_amazonaws_xray_awsxray::state::{
    DEFAULT_SAMPLING_RULE_FIXED_RATE, DEFAULT_SAMPLING_RULE_NAME,
    DEFAULT_SAMPLING_RULE_RESERVOIR_SIZE, MAX_SAMPLING_BOOST_SERVICES_PER_RULE,
    SAMPLING_TARGET_INTERVAL_SECONDS, SAMPLING_WINDOW_SECONDS, UNKNOWN_SAMPLING_RULE_MESSAGE,
};

use crate::trace_ingest_and_groups::{Answer, Xray};

/// Now, in seconds of epoch time — the clock a reporting service stamps its
/// documents with.
pub(crate) fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// A 24-character hexadecimal `ClientID`, which is the only length the model
/// accepts (`@length(min: 24, max: 24)`).
pub(crate) fn client_id(seed: u32) -> String {
    format!("{seed:024x}")
}

/// A sampling rule definition, with an optional `(MaxRate, CooldownWindowMinutes)`
/// boost configuration.
fn rule_body(
    name: &str,
    priority: i64,
    fixed_rate: f64,
    reservoir_size: i64,
    boost: Option<(f64, i64)>,
) -> Value {
    let mut rule = json!({
        "RuleName": name,
        "ResourceARN": "*",
        "Priority": priority,
        "FixedRate": fixed_rate,
        "ReservoirSize": reservoir_size,
        "ServiceName": "*",
        "ServiceType": "*",
        "Host": "*",
        "HTTPMethod": "*",
        "URLPath": "*",
        // The model documents the sampling rule format as `1`.
        "Version": 1,
    });
    if let Some((max_rate, cooldown)) = boost {
        rule["SamplingRateBoost"] = json!({"MaxRate": max_rate, "CooldownWindowMinutes": cooldown});
    }
    json!({ "SamplingRule": rule })
}

/// `CreateSamplingRule` over the front door.
pub(crate) async fn create_rule(
    xray: &Xray,
    name: &str,
    priority: i64,
    fixed_rate: f64,
    reservoir_size: i64,
    boost: Option<(f64, i64)>,
) -> Answer {
    xray.call(
        "/CreateSamplingRule",
        &rule_body(name, priority, fixed_rate, reservoir_size, boost),
    )
    .await
}

/// One `SamplingStatisticsDocument`.
pub(crate) fn statistics(
    rule_name: &str,
    client: &str,
    at: f64,
    requests: i64,
    sampled: i64,
    borrowed: i64,
) -> Value {
    json!({
        "RuleName": rule_name,
        "ClientID": client,
        "Timestamp": at,
        "RequestCount": requests,
        "SampledCount": sampled,
        "BorrowCount": borrowed,
    })
}

/// `GetSamplingTargets` for the given statistics documents.
pub(crate) async fn get_targets(xray: &Xray, documents: &[Value]) -> Answer {
    xray.call(
        "/SamplingTargets",
        &json!({ "SamplingStatisticsDocuments": documents }),
    )
    .await
}

/// `GetSamplingTargets` carrying boost statistics beside the sampling ones.
pub(crate) async fn get_targets_with_boost(
    xray: &Xray,
    documents: &[Value],
    boost_documents: &[Value],
) -> Answer {
    xray.call(
        "/SamplingTargets",
        &json!({
            "SamplingStatisticsDocuments": documents,
            "SamplingBoostStatisticsDocuments": boost_documents,
        }),
    )
    .await
}

/// `GetSamplingStatisticSummaries`, resuming from `token` when one is given.
async fn get_summaries(xray: &Xray, token: Option<&str>) -> Answer {
    let mut body = json!({});
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/SamplingStatisticSummaries", &body).await
}

/// The named list member of an answer, or an empty slice.
pub(crate) fn list(answer: &Answer, member: &str) -> Vec<Value> {
    answer
        .get(member)
        .as_array()
        .map_or_else(Vec::new, Clone::clone)
}

/// The target document for `rule_name`, if the answer carries one.
pub(crate) fn target_for(answer: &Answer, rule_name: &str) -> Option<Value> {
    list(answer, "SamplingTargetDocuments")
        .into_iter()
        .find(|target| target.get("RuleName").and_then(Value::as_str) == Some(rule_name))
}

/// One row of the rule table.
#[derive(Debug, Clone)]
struct Case {
    case: &'static str,
    name: &'static str,
    priority: i64,
    fixed_rate: f64,
    reservoir_size: i64,
}

/// The table: the rows differ along `Priority` (both modelled bounds and the
/// values between), the precision and the modelled bounds of `FixedRate`, and
/// the `ReservoirSize` of the pool — including a rule with no reservoir at all.
fn rule_cases() -> Vec<Case> {
    vec![
        Case {
            case: "no reservoir at all, the modelled minimum fixed rate",
            name: "no-reservoir",
            priority: 9999,
            fixed_rate: 0.0,
            reservoir_size: 0,
        },
        Case {
            case: "a one-request reservoir at the highest priority",
            name: "tiny-reservoir",
            priority: 1,
            fixed_rate: 0.05,
            reservoir_size: 1,
        },
        Case {
            case: "a fixed rate carrying fifteen significant digits",
            name: "precise-rate",
            priority: 500,
            fixed_rate: 0.123_456_789_012_345,
            reservoir_size: 10,
        },
        Case {
            case: "the modelled maximum fixed rate and a large reservoir",
            name: "always-sampled",
            priority: 42,
            fixed_rate: 1.0,
            reservoir_size: 2500,
        },
    ]
}

#[tokio::test]
async fn a_reported_rule_is_served_its_stored_rate_and_a_share_only_once_a_cycle_has_run() {
    let xray = Xray::new();
    let cases = rule_cases();
    let before_rules = now();
    for case in &cases {
        let created = create_rule(
            &xray,
            case.name,
            case.priority,
            case.fixed_rate,
            case.reservoir_size,
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateSamplingRule must succeed, got {created:?}",
            case.case
        );
    }

    let reported_at = now();
    let client = client_id(1);
    let documents: Vec<Value> = cases
        .iter()
        .map(|case| statistics(case.name, &client, reported_at, 120, 6, 1))
        .collect();

    // The FIRST report precedes the first allocation cycle for the rule, so the
    // rule's rate is served but no share of its reservoir is.
    let first = get_targets(&xray, &documents).await;
    assert_eq!(
        first.status, 200,
        "GetSamplingTargets must succeed, got {first:?}"
    );
    for case in &cases {
        let target = target_for(&first, case.name).unwrap_or_else(|| {
            panic!(
                "case {:?}: a reported rule must be answered a target, got {first:?}",
                case.case
            )
        });
        let fixed_rate = target.get("FixedRate").and_then(Value::as_f64);
        assert_eq!(
            fixed_rate,
            Some(case.fixed_rate),
            "case {:?}: expected the rule's stored FixedRate {}, got {fixed_rate:?}",
            case.case,
            case.fixed_rate
        );
        for member in ["ReservoirQuota", "ReservoirQuotaTTL", "Interval"] {
            assert!(
                target.get(member).is_none_or(Value::is_null),
                "case {:?}: a first report holds no allocation yet, so {member} must be absent, got {target:?}",
                case.case
            );
        }
    }

    // The next report is answered the share the cycle just run computed: the
    // only service reporting takes the whole pool.
    let answer = get_targets(&xray, &documents).await;
    assert_eq!(
        answer.status, 200,
        "GetSamplingTargets must succeed, got {answer:?}"
    );

    for case in &cases {
        let target = target_for(&answer, case.name).unwrap_or_else(|| {
            panic!(
                "case {:?}: a reported rule must be answered a target, got {answer:?}",
                case.case
            )
        });
        let fixed_rate = target.get("FixedRate").and_then(Value::as_f64);
        assert_eq!(
            fixed_rate,
            Some(case.fixed_rate),
            "case {:?}: expected the rule's stored FixedRate {}, got {fixed_rate:?}",
            case.case,
            case.fixed_rate
        );
        let interval = target.get("Interval").and_then(Value::as_i64);
        assert_eq!(
            interval,
            Some(i64::from(SAMPLING_TARGET_INTERVAL_SECONDS)),
            "case {:?}: a granted share comes with the interval to come back at ({SAMPLING_TARGET_INTERVAL_SECONDS}), got {interval:?}",
            case.case
        );
        // The only service reporting for the rule is allocated the whole pool.
        let quota = target.get("ReservoirQuota").and_then(Value::as_i64);
        assert_eq!(
            quota,
            Some(case.reservoir_size),
            "case {:?}: the only reporting service takes the whole pool of {}, got {quota:?}",
            case.case,
            case.reservoir_size
        );
        let ttl = target.get("ReservoirQuotaTTL").and_then(Value::as_f64);
        assert!(
            ttl.is_some_and(|ttl| ttl > reported_at),
            "case {:?}: an allocated quota must expire in the future (after {reported_at}), got {ttl:?}",
            case.case
        );
        assert!(
            target.get("SamplingBoost").is_none_or(Value::is_null),
            "case {:?}: no boost was configured or reported, got {target:?}",
            case.case
        );
    }

    assert!(
        list(&answer, "UnprocessedStatistics").is_empty(),
        "every reported rule exists, so nothing is unprocessed, got {answer:?}"
    );

    // The documents arrive in the order a service applies its rules: by
    // `Priority`, then by name.
    let mut expected: Vec<&Case> = cases.iter().collect();
    expected.sort_by_key(|case| (case.priority, case.name));
    let served: Vec<&str> = list(&answer, "SamplingTargetDocuments")
        .iter()
        .filter_map(|target| {
            target
                .get("RuleName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<String>>()
        .iter()
        .map(|name| {
            cases
                .iter()
                .find(|case| case.name == name.as_str())
                .map_or("<unknown>", |case| case.name)
        })
        .collect();
    let expected_names: Vec<&str> = expected.iter().map(|case| case.name).collect();
    assert_eq!(
        served, expected_names,
        "targets must be ordered by the priority of the rule each is for, got {served:?}"
    );

    // `LastRuleModification` is when the configuration last changed, which the
    // creates above moved.
    let modified = answer.get("LastRuleModification").as_f64();
    assert!(
        modified.is_some_and(|at| at >= before_rules),
        "LastRuleModification must report the creates (at or after {before_rules}), got {modified:?}"
    );
}

#[tokio::test]
async fn the_reservoir_is_shared_between_the_services_reporting_for_the_rule() {
    // Rows: how the pool divides between services reporting different shares of
    // the traffic, including a service that reports none of it.
    let rows: Vec<(&str, i64, Vec<i64>)> = vec![
        ("ten shared by halves", 10, vec![50, 50]),
        ("ten shared five/three/two", 10, vec![50, 30, 20]),
        ("one pool, two services", 1, vec![90, 10]),
        ("a pool nobody reports traffic for", 8, vec![0, 0, 0, 0]),
        ("a single service reporting everything", 7, vec![120]),
    ];

    for (label, reservoir_size, requests) in rows {
        let xray = Xray::new();
        let rule = "shared-pool";
        let created = create_rule(&xray, rule, 100, 0.1, reservoir_size, None).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateSamplingRule must succeed, got {created:?}"
        );

        let reported_at = now();
        // Every service reports in ONE call, so the cycle that follows it
        // divides the pool between all of them; the next call is answered the
        // shares that cycle computed.
        let documents: Vec<Value> = requests
            .iter()
            .enumerate()
            .map(|(index, count)| {
                let client = client_id(u32::try_from(index).expect("the index fits a u32") + 1);
                statistics(rule, &client, reported_at, *count, 0, 0)
            })
            .collect();
        let first = get_targets(&xray, &documents).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: GetSamplingTargets must succeed, got {first:?}"
        );

        let total: i64 = requests.iter().sum();
        let mut allocated = 0;
        for (index, count) in requests.iter().enumerate() {
            let client = client_id(u32::try_from(index).expect("the index fits a u32") + 1);
            let answer =
                get_targets(&xray, &[statistics(rule, &client, reported_at, *count, 0, 0)]).await;
            let target = target_for(&answer, rule).unwrap_or_else(|| {
                panic!("case {label:?}: the rule must be answered a target, got {answer:?}")
            });
            let quota = target
                .get("ReservoirQuota")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: a target must carry a quota, got {target:?}")
                });
            #[allow(clippy::cast_precision_loss)]
            let expected = if total > 0 {
                ((reservoir_size as f64) * (*count as f64) / (total as f64)).floor() as i64
            } else {
                ((reservoir_size as f64) / (requests.len() as f64)).floor() as i64
            };
            assert_eq!(
                quota, expected,
                "case {label:?}: a service reporting {count} of {total} requests takes {expected} of a pool of {reservoir_size}, got {quota}"
            );
            allocated += quota;
        }
        assert!(
            allocated <= reservoir_size,
            "case {label:?}: the shares must never add up past the pool of {reservoir_size}, got {allocated}"
        );
    }
}

#[tokio::test]
async fn a_service_joining_a_divided_reservoir_is_told_a_share_of_nothing_until_the_next_cycle() {
    let xray = Xray::new();
    let rule = "joining";
    let reservoir_size = 4;
    let created = create_rule(&xray, rule, 100, 0.1, reservoir_size, None).await;
    assert_eq!(
        created.status, 200,
        "CreateSamplingRule must succeed, got {created:?}"
    );

    let reported_at = now();
    let first_client = client_id(1);
    let second_client = client_id(2);
    let quota_of = |answer: &Answer, label: &str| -> Option<i64> {
        let target = target_for(answer, rule)
            .unwrap_or_else(|| panic!("{label}: the rule must be served a target, got {answer:?}"));
        target.get("ReservoirQuota").and_then(Value::as_i64)
    };

    // Nothing has divided the pool yet, so the first report is told no share.
    let opening = get_targets(
        &xray,
        &[statistics(rule, &first_client, reported_at, 30, 1, 0)],
    )
    .await;
    assert_eq!(
        quota_of(&opening, "the first report of the rule"),
        None,
        "before any cycle there is no share to be told about, got {opening:?}"
    );

    // Its next report is told the whole pool: it is the only service in it.
    let second = get_targets(
        &xray,
        &[statistics(rule, &first_client, reported_at, 30, 1, 0)],
    )
    .await;
    assert_eq!(
        quota_of(&second, "the second report of the rule"),
        Some(reservoir_size),
        "the only service in the pool is told all of it, got {second:?}"
    );

    // A service joining a pool that has already been divided is told its share
    // of that division, which is nothing — but it IS told it.
    let joining = get_targets(
        &xray,
        &[statistics(rule, &second_client, reported_at, 10, 1, 0)],
    )
    .await;
    assert_eq!(
        quota_of(&joining, "a service joining a divided pool"),
        Some(0),
        "a service reporting into a live cycle is told its share even when it is zero, got {joining:?}"
    );

    // And from the next cycle on it is told the share its traffic earns.
    let settled = get_targets(
        &xray,
        &[statistics(rule, &second_client, reported_at, 10, 1, 0)],
    )
    .await;
    assert_eq!(
        quota_of(&settled, "the cycle after it joined"),
        Some(1),
        "a service reporting 10 of 40 requests earns 1 of a pool of {reservoir_size}, got {settled:?}"
    );
}

#[tokio::test]
async fn statistics_for_a_rule_the_account_does_not_hold_are_unprocessed() {
    let xray = Xray::new();
    let known = "known-rule";
    let created = create_rule(&xray, known, 100, 0.25, 4, None).await;
    assert_eq!(
        created.status, 200,
        "CreateSamplingRule must succeed, got {created:?}"
    );

    // Rows: names no rule was ever created under, and one that was created and
    // deleted again without anything ever reporting for it, so no division of
    // its reservoir stands either — each reported beside a rule that does exist.
    let deleted = "deleted-rule";
    let seeded = create_rule(&xray, deleted, 101, 0.3, 2, None).await;
    assert_eq!(seeded.status, 200, "seeding must succeed, got {seeded:?}");
    let removed = xray
        .call("/DeleteSamplingRule", &json!({"RuleName": deleted}))
        .await;
    assert_eq!(
        removed.status, 200,
        "DeleteSamplingRule must succeed, got {removed:?}"
    );

    let rows = [
        ("a name nothing was created under", "never-created"),
        ("a rule that was deleted again", deleted),
        ("a name differing only in case", "Known-Rule"),
        ("the longest name the model allows", "z".repeat(32).leak()),
    ];

    let reported_at = now();
    let client = client_id(7);
    for (label, unknown) in rows {
        let answer = get_targets(
            &xray,
            &[
                statistics(known, &client, reported_at, 10, 1, 0),
                statistics(unknown, &client, reported_at, 10, 1, 0),
            ],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: GetSamplingTargets must still succeed, got {answer:?}"
        );
        assert!(
            target_for(&answer, unknown).is_none(),
            "case {label:?}: no allocation may be served for a rule nobody configured, got {answer:?}"
        );
        assert!(
            target_for(&answer, known).is_some(),
            "case {label:?}: the rule that does exist is still served, got {answer:?}"
        );
        let unprocessed = list(&answer, "UnprocessedStatistics");
        assert_eq!(
            unprocessed.len(),
            1,
            "case {label:?}: exactly the unknown rule is unprocessed, got {unprocessed:?}"
        );
        let entry = &unprocessed[0];
        assert_eq!(
            entry.get("RuleName").and_then(Value::as_str),
            Some(unknown),
            "case {label:?}: the unprocessed entry names the rule reported, got {entry:?}"
        );
        // The code and the sentence are the service's own: the status a request
        // refused for naming that rule would carry, and the fixed sentence a
        // declined statistics document carries.
        for (member, expected) in [
            ("ErrorCode", UNKNOWN_RULE_ERROR_CODE),
            ("Message", UNKNOWN_SAMPLING_RULE_MESSAGE),
        ] {
            let value = entry.get(member).and_then(Value::as_str);
            assert_eq!(
                value,
                Some(expected),
                "case {label:?}: an unprocessed entry's {member} must be {expected:?}, got {entry:?}"
            );
        }
    }

    // Nothing was recorded for the rules that do not exist, so no summary
    // mentions one.
    let summaries = get_summaries(&xray, None).await;
    assert_eq!(
        summaries.status, 200,
        "GetSamplingStatisticSummaries must succeed, got {summaries:?}"
    );
    let named: Vec<&str> = list(&summaries, "SamplingStatisticSummaries")
        .iter()
        .filter_map(|summary| summary.get("RuleName").and_then(Value::as_str))
        .map(|name| if name == known { known } else { "<other>" })
        .collect();
    assert_eq!(
        named,
        vec![known],
        "only the rule that exists has reported results, got {summaries:?}"
    );
}

#[tokio::test]
async fn reported_statistics_are_summarised_per_rule_and_ten_second_window() {
    // Rows: the counters reported, and how far into the window the report is
    // stamped — so two reports of one window accumulate into ONE summary and a
    // report a window later is its own row. The counts span zero, one, a large
    // value and the largest the wire member carries.
    let rows: Vec<(&str, i64, i64, i64, f64)> = vec![
        ("nothing matched the rule", 0, 0, 0, 0.0),
        ("one request, one recorded", 1, 1, 0, 1.0),
        ("a million requests", 1_000_000, 10_000, 5, 2.5),
        (
            "the largest count the wire member carries",
            i64::from(i32::MAX),
            0,
            0,
            9.999,
        ),
    ];

    for (label, requests, sampled, borrowed, offset) in rows {
        let xray = Xray::new();
        let rule = "summarised";
        let created = create_rule(&xray, rule, 100, 0.2, 3, None).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateSamplingRule must succeed, got {created:?}"
        );

        // A window start this emulator aligns to: whole tens of seconds of
        // epoch time.
        #[allow(clippy::cast_precision_loss)]
        let window = (now() / SAMPLING_WINDOW_SECONDS as f64).floor() * SAMPLING_WINDOW_SECONDS as f64;
        let first = window + offset;
        #[allow(clippy::cast_precision_loss)]
        let next_window = window + SAMPLING_WINDOW_SECONDS as f64;

        // Two services report the same window, and one of them also reports the
        // window after it.
        let reports = [
            (client_id(1), first),
            (client_id(2), first),
            (client_id(1), next_window),
        ];
        for (client, at) in &reports {
            let answer = get_targets(
                &xray,
                &[statistics(rule, client, *at, requests, sampled, borrowed)],
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: GetSamplingTargets must succeed, got {answer:?}"
            );
        }

        let summaries = get_summaries(&xray, None).await;
        assert_eq!(
            summaries.status, 200,
            "case {label:?}: GetSamplingStatisticSummaries must succeed, got {summaries:?}"
        );
        let rows = list(&summaries, "SamplingStatisticSummaries");
        assert_eq!(
            rows.len(),
            2,
            "case {label:?}: two reported windows are two summaries, got {rows:?}"
        );

        // The counters a wire `Integer` can carry; a total past its range is
        // reported at its maximum rather than wrapped.
        let on_wire = |total: i64| total.min(i64::from(i32::MAX));
        let expected = [
            (window, on_wire(requests * 2), on_wire(sampled * 2), on_wire(borrowed * 2)),
            (next_window, on_wire(requests), on_wire(sampled), on_wire(borrowed)),
        ];
        for (index, (start, expected_requests, expected_sampled, expected_borrowed)) in
            expected.iter().enumerate()
        {
            let summary = &rows[index];
            assert_eq!(
                summary.get("RuleName").and_then(Value::as_str),
                Some(rule),
                "case {label:?}: a summary names the rule reported for, got {summary:?}"
            );
            let timestamp = summary.get("Timestamp").and_then(Value::as_f64);
            assert_eq!(
                timestamp,
                Some(*start),
                "case {label:?}: a summary reports the START of its ten-second window, got {timestamp:?}"
            );
            for (member, expected) in [
                ("RequestCount", expected_requests),
                ("SampledCount", expected_sampled),
                ("BorrowCount", expected_borrowed),
            ] {
                let actual = summary.get(member).and_then(Value::as_i64);
                assert_eq!(
                    actual,
                    Some(*expected),
                    "case {label:?}: {member} is summed across every service reporting the window, expected {expected}, got {actual:?}"
                );
            }
        }

        // Reading changes nothing a later caller sees.
        let again = get_summaries(&xray, None).await;
        assert_eq!(
            again.body, summaries.body,
            "case {label:?}: a repeated read must answer identically, got {:?} then {:?}",
            summaries.body, again.body
        );
    }
}

#[tokio::test]
async fn a_rules_lifecycle_moves_what_the_targets_serve() {
    let xray = Xray::new();
    let rule = "lifecycle";
    let client = client_id(3);
    let reported_at = now();
    let report = |answer: &Answer| -> Value {
        target_for(answer, rule)
            .unwrap_or_else(|| panic!("the rule must be served a target, got {answer:?}"))
    };

    let created = create_rule(&xray, rule, 100, 0.05, 2, None).await;
    assert_eq!(
        created.status, 200,
        "CreateSamplingRule must succeed, got {created:?}"
    );

    // The first report is before any cycle: the rate is served, the share is
    // not.
    let first = get_targets(&xray, &[statistics(rule, &client, reported_at, 20, 2, 0)]).await;
    let target = report(&first);
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64)
        ),
        (Some(0.05), None),
        "the created rule's rate is served before any share is, got {target:?}"
    );
    let first_modification = first.get("LastRuleModification").as_f64();

    // The next report is answered the share the first cycle computed.
    let second = get_targets(&xray, &[statistics(rule, &client, reported_at, 20, 2, 0)]).await;
    let target = report(&second);
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64)
        ),
        (Some(0.05), Some(2)),
        "the created rule's rate and whole pool are served, got {target:?}"
    );

    // An update to the rule moves the rate the very next call serves, and the
    // pool the cycle after it shares out.
    let updated = xray
        .call(
            "/UpdateSamplingRule",
            &json!({
                "SamplingRuleUpdate": {"RuleName": rule, "FixedRate": 0.75, "ReservoirSize": 6},
            }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "UpdateSamplingRule must succeed, got {updated:?}"
    );
    let third = get_targets(&xray, &[statistics(rule, &client, reported_at, 20, 2, 0)]).await;
    let target = report(&third);
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64)
        ),
        (Some(0.75), Some(2)),
        "the updated rate is served at once, beside the share of the pool as it stood, got {target:?}"
    );
    let third_modification = third.get("LastRuleModification").as_f64();
    assert!(
        third_modification >= first_modification,
        "LastRuleModification must move with the update, got {first_modification:?} then {third_modification:?}"
    );
    let fourth = get_targets(&xray, &[statistics(rule, &client, reported_at, 20, 2, 0)]).await;
    let target = report(&fourth);
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64)
        ),
        (Some(0.75), Some(6)),
        "the cycle after the update shares out the updated pool, got {target:?}"
    );

    // Deleting the rule does not cancel the allocation the service was already
    // granted: it is still served the target it holds, and its statistics are
    // not unprocessed.
    let removed = xray
        .call("/DeleteSamplingRule", &json!({"RuleName": rule}))
        .await;
    assert_eq!(
        removed.status, 200,
        "DeleteSamplingRule must succeed, got {removed:?}"
    );
    let fifth = get_targets(&xray, &[statistics(rule, &client, reported_at, 20, 2, 0)]).await;
    let target = report(&fifth);
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64)
        ),
        (Some(0.75), Some(6)),
        "a deleted rule still serves the allocation the service holds, got {target:?}"
    );
    assert!(
        list(&fifth, "UnprocessedStatistics").is_empty(),
        "a service holding an allocation is served it rather than told it was unprocessed, got {fifth:?}"
    );
    // And a service that was NOT in the division is answered out of it too: the
    // rate it was made under, and what it left unclaimed — nothing, the earlier
    // service having claimed the whole pool of 6.
    let stranger = get_targets(
        &xray,
        &[statistics(rule, &client_id(44), reported_at, 20, 2, 0)],
    )
    .await;
    let target = report(&stranger);
    assert_eq!(
        (
            target.get("FixedRate").and_then(Value::as_f64),
            target.get("ReservoirQuota").and_then(Value::as_i64)
        ),
        (Some(0.75), Some(0)),
        "a service that was not in the division is told what it left unclaimed, got {target:?}"
    );
    assert!(
        list(&stranger, "UnprocessedStatistics").is_empty(),
        "a standing division answers every service reporting for the rule, got {stranger:?}"
    );
}

#[tokio::test]
async fn the_service_managed_default_rule_serves_targets_on_a_fresh_account() {
    let xray = Xray::new();
    let reported_at = now();
    let documents = [statistics(
        DEFAULT_SAMPLING_RULE_NAME,
        &client_id(9),
        reported_at,
        15,
        1,
        1,
    )];
    let first = get_targets(&xray, &documents).await;
    assert_eq!(
        first.status, 200,
        "GetSamplingTargets must succeed, got {first:?}"
    );
    assert!(
        target_for(&first, DEFAULT_SAMPLING_RULE_NAME).is_some(),
        "the account's service-managed rule must be served, got {first:?}"
    );
    let answer = get_targets(&xray, &documents).await;
    let target = target_for(&answer, DEFAULT_SAMPLING_RULE_NAME).unwrap_or_else(|| {
        panic!("the account's service-managed rule must be served, got {answer:?}");
    });
    assert_eq!(
        target.get("FixedRate").and_then(Value::as_f64),
        Some(DEFAULT_SAMPLING_RULE_FIXED_RATE),
        "the service-managed rule serves its own fixed rate, got {target:?}"
    );
    assert_eq!(
        target.get("ReservoirQuota").and_then(Value::as_i64),
        Some(i64::from(DEFAULT_SAMPLING_RULE_RESERVOIR_SIZE)),
        "the only reporting service takes the whole of its pool, got {target:?}"
    );
}

#[tokio::test]
async fn a_boost_is_granted_only_where_it_is_configured_and_once_per_cooldown_window() {
    // Rows: the configured maximum rate and the anomalies reported, against the
    // boost rate that may be granted — never more than the maximum, and none at
    // all without an anomaly or without a configuration asking for one.
    struct BoostCase {
        case: &'static str,
        /// `SamplingRateBoost` as `(MaxRate, CooldownWindowMinutes)`, or no
        /// boost configuration at all.
        configured: Option<(f64, i64)>,
        anomalies: i64,
        total: i64,
        /// The `BoostRate` the rule may be granted.
        expected: Option<f64>,
    }

    let rows = vec![
        BoostCase {
            case: "no boost configured on the rule",
            configured: None,
            anomalies: 10,
            total: 100,
            expected: None,
        },
        BoostCase {
            case: "anomalies below the configured maximum",
            configured: Some((0.4, 5)),
            anomalies: 10,
            total: 100,
            expected: Some(0.1),
        },
        BoostCase {
            case: "anomalies above the configured maximum, capped by it",
            configured: Some((0.25, 5)),
            anomalies: 90,
            total: 100,
            expected: Some(0.25),
        },
        BoostCase {
            case: "every request an anomaly, the maximum at its modelled ceiling",
            configured: Some((1.0, 5)),
            anomalies: 50,
            total: 50,
            expected: Some(1.0),
        },
        BoostCase {
            case: "no anomaly reported at all",
            configured: Some((0.5, 5)),
            anomalies: 0,
            total: 100,
            expected: None,
        },
        BoostCase {
            case: "nothing reported at all",
            configured: Some((0.5, 5)),
            anomalies: 0,
            total: 0,
            expected: None,
        },
    ];

    for BoostCase {
        case: label,
        configured: boost,
        anomalies,
        total,
        expected,
    } in rows
    {
        let xray = Xray::new();
        let rule = "boosted";
        let created = create_rule(&xray, rule, 100, 0.1, 2, boost).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateSamplingRule must succeed, got {created:?}"
        );

        let reported_at = now();
        let boost_document = json!({
            "RuleName": rule,
            "ServiceName": "checkout",
            "Timestamp": reported_at,
            "AnomalyCount": anomalies,
            "TotalCount": total,
            "SampledAnomalyCount": 0,
        });
        let answer = get_targets_with_boost(
            &xray,
            &[statistics(rule, &client_id(1), reported_at, total, 1, 0)],
            ::std::slice::from_ref(&boost_document),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: GetSamplingTargets must succeed, got {answer:?}"
        );
        let target = target_for(&answer, rule).unwrap_or_else(|| {
            panic!("case {label:?}: the rule must be served a target, got {answer:?}")
        });
        let granted = target
            .get("SamplingBoost")
            .and_then(|value| value.get("BoostRate"))
            .and_then(Value::as_f64);
        assert_eq!(
            granted, expected,
            "case {label:?}: expected a boost rate of {expected:?}, got {target:?}"
        );
        if let Some(rate) = granted {
            let max_rate = boost.map_or(0.0, |(max_rate, _)| max_rate);
            assert!(
                rate <= max_rate,
                "case {label:?}: a granted boost never exceeds the configured maximum {max_rate}, got {rate}"
            );
            let ttl = target
                .get("SamplingBoost")
                .and_then(|value| value.get("BoostRateTTL"))
                .and_then(Value::as_f64);
            assert!(
                ttl.is_some_and(|ttl| ttl > reported_at),
                "case {label:?}: a granted boost must expire in the future, got {ttl:?}"
            );

            // Only one boost per cooldown window: the next call inside it is
            // served the target without a boost.
            let again = get_targets_with_boost(
                &xray,
                &[statistics(rule, &client_id(1), reported_at, total, 1, 0)],
                &[boost_document],
            )
            .await;
            let target = target_for(&again, rule).unwrap_or_else(|| {
                panic!("case {label:?}: the rule must still be served, got {again:?}")
            });
            assert!(
                target.get("SamplingBoost").is_none_or(Value::is_null),
                "case {label:?}: a second boost inside the cooldown window must not be granted, got {target:?}"
            );
        }

        // Boost statistics for a rule the account does not hold are unprocessed.
        let unknown = get_targets_with_boost(
            &xray,
            &[],
            &[json!({
                "RuleName": "no-such-rule",
                "ServiceName": "checkout",
                "Timestamp": reported_at,
                "AnomalyCount": 1,
                "TotalCount": 2,
                "SampledAnomalyCount": 0,
            })],
        )
        .await;
        let unprocessed = list(&unknown, "UnprocessedBoostStatistics");
        assert_eq!(
            unprocessed
                .first()
                .and_then(|entry| entry.get("RuleName"))
                .and_then(Value::as_str),
            Some("no-such-rule"),
            "case {label:?}: boost statistics for an unknown rule are unprocessed, got {unknown:?}"
        );
    }
}

#[tokio::test]
async fn a_rule_keeps_serving_boosts_past_the_services_it_records_grants_for() {
    let xray = Xray::new();
    let rule = "many-services";
    let created = create_rule(&xray, rule, 100, 0.1, 2, Some((0.5, 5))).await;
    assert_eq!(
        created.status, 200,
        "CreateSamplingRule must succeed, got {created:?}"
    );

    let reported_at = now();
    // Past the number of services one rule records grants for, in batches of
    // the modelled maximum of 25 boost documents per call: the oldest grant is
    // dropped rather than the store growing, and every service is still served.
    let services: Vec<usize> = (0..MAX_SAMPLING_BOOST_SERVICES_PER_RULE + 5).collect();
    for batch in services.chunks(25) {
        let documents: Vec<Value> = batch
            .iter()
            .map(|index| {
                json!({
                    "RuleName": rule,
                    "ServiceName": format!("service-{index:04}"),
                    "Timestamp": reported_at,
                    "AnomalyCount": 1,
                    "TotalCount": 10,
                    "SampledAnomalyCount": 0,
                })
            })
            .collect();
        let answer = get_targets_with_boost(&xray, &[], &documents).await;
        assert_eq!(
            answer.status, 200,
            "batch starting at {:?}: GetSamplingTargets must succeed, got {answer:?}",
            batch.first()
        );
        let target = target_for(&answer, rule).unwrap_or_else(|| {
            panic!(
                "batch starting at {:?}: the boosted rule must be served, got {answer:?}",
                batch.first()
            )
        });
        let granted = target
            .get("SamplingBoost")
            .and_then(|boost| boost.get("BoostRate"))
            .and_then(Value::as_f64);
        assert_eq!(
            granted,
            Some(0.1),
            "batch starting at {:?}: every reporting service is granted the anomaly rate it reported, got {target:?}",
            batch.first()
        );
    }
}

#[tokio::test]
async fn summaries_paginate_and_refuse_a_token_they_never_issued() {
    let xray = Xray::new();
    let rule = "paged";
    let created = create_rule(&xray, rule, 100, 0.1, 1, None).await;
    assert_eq!(
        created.status, 200,
        "CreateSamplingRule must succeed, got {created:?}"
    );

    // One window past a page, so the listing is truncated exactly once.
    let windows = SAMPLING_STATISTIC_SUMMARIES_PAGE_SIZE + 1;
    let client = client_id(1);
    #[allow(clippy::cast_precision_loss)]
    let base = (now() / SAMPLING_WINDOW_SECONDS as f64).floor() * SAMPLING_WINDOW_SECONDS as f64;
    for index in 0..windows {
        #[allow(clippy::cast_precision_loss)]
        let at = base + (index as f64) * SAMPLING_WINDOW_SECONDS as f64;
        let answer = get_targets(&xray, &[statistics(rule, &client, at, 1, 1, 0)]).await;
        assert_eq!(
            answer.status, 200,
            "window {index}: GetSamplingTargets must succeed, got {answer:?}"
        );
    }

    let mut seen = 0;
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = get_summaries(&xray, token.as_deref()).await;
        assert_eq!(
            page.status, 200,
            "GetSamplingStatisticSummaries must succeed, got {page:?}"
        );
        let rows = list(&page, "SamplingStatisticSummaries");
        assert!(
            rows.len() <= SAMPLING_STATISTIC_SUMMARIES_PAGE_SIZE,
            "a page holds at most {SAMPLING_STATISTIC_SUMMARIES_PAGE_SIZE} summaries, got {}",
            rows.len()
        );
        seen += rows.len();
        token = page.get("NextToken").as_str().map(str::to_string);
        pages += 1;
        assert!(pages <= 5, "pagination must terminate, got {pages} pages");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, windows,
        "pagination must yield every reported window exactly once"
    );
    assert_eq!(pages, 2, "one window past a page is exactly two pages");

    let bogus = get_summaries(&xray, Some("never-issued|0")).await;
    assert_eq!(
        (bogus.status, bogus.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "a NextToken this service never issued must be refused, got {bogus:?}"
    );
}
