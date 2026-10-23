// SPDX-License-Identifier: CC0-1.0

//! `DescribeDimensionKeys`, end to end over the wire.
//!
//! The rules under test are the ones the model, the member documentation and the
//! service's recorded behaviour state: `StartTime` must be earlier than
//! `EndTime`, and a window that is not is refused with the modelled
//! `InvalidArgumentException` (HTTP 400); a window that is gets the same granular
//! alignment `GetResourceMetrics` reports (`AlignedStartTime` at or before the
//! requested start, `AlignedEndTime` the latest period boundary the service
//! holds — its own clock floored to the effective period — both on a boundary of
//! that period); and the key list is what the window HOLDS — a dimension key is
//! a group of DB sessions sampled on a running engine, and the data sources this
//! emulator serves run none, so `Keys` is empty rather than a top-N list of
//! invented sessions.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator};

/// One day in seconds — a multiple of every documented period.
const ONE_DAY: i64 = 86_400;

/// The granularities `PeriodInSeconds` documents: 1 (one second), 60 (one
/// minute), 300 (five minutes), 3600 (one hour), 86400 (twenty-four hours).
const DOCUMENTED_PERIODS: &[i64] = &[1, 60, 300, 3600, 86400];

/// Epoch seconds, as the service reads its own clock.
fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is at or after the Unix epoch")
            .as_secs(),
    )
    .expect("the current time fits in an i64")
}

/// `seconds` floored to a `period` boundary measured from the Unix epoch.
fn floor_to(seconds: i64, period: i64) -> i64 {
    seconds - seconds.rem_euclid(period)
}

/// A window start that leaves a `span`-second window entirely in the past, two
/// days clear of the service's clock, and on a boundary of every documented
/// period.
fn past_start(span: i64) -> i64 {
    floor_to(unix_now() - span - 2 * ONE_DAY, ONE_DAY)
}

/// A monitored data source: an immutable DB resource id of the form the model
/// documents on every `Identifier` member.
const IDENTIFIER: &str = "db-ABCDEFG1234567";

/// Dimension groups `GroupBy.Group` may name, from the model's own
/// `DimensionGroup$Group` documentation and the dimension groups Performance
/// Insights publishes for RDS (`db`, `db.application`, `db.host`, `db.sql`,
/// `db.sql_tokenized`, `db.user`, `db.wait_event`, `db.wait_event_type`). The
/// rows below span several of them, including ones no recording cites.
const GROUPS: &[&str] = &[
    "db.sql",
    "db.sql_tokenized",
    "db.wait_event",
    "db.user",
    "db.host",
];

fn dimension_keys_input(
    identifier: &str,
    group: &str,
    metric: &str,
    start: i64,
    end: i64,
    period: Option<i64>,
) -> Value {
    let mut input = json!({
        "ServiceType": "RDS",
        "Identifier": identifier,
        "StartTime": start,
        "EndTime": end,
        "Metric": metric,
        "GroupBy": {"Group": group},
    });
    if let Some(period) = period {
        input["PeriodInSeconds"] = json!(period);
    }
    input
}

/// RULE: documented on `StartTime` — "The value for StartTime must be earlier
/// than the value for EndTime."
#[tokio::test]
async fn a_window_whose_end_does_not_follow_its_start_is_refused() {
    let emulator = emulator();

    // The rows differ along the ordering: an end before the start by a second,
    // by half an hour and by a day, and an end exactly at the start (which
    // describes no window either).
    let t0 = past_start(86_400);
    for (start, end, label) in [
        (t0, t0 - 1, "an end one second before the start"),
        (t0 + 1_800, t0 - 1_800, "an end half an hour before the start"),
        (t0, t0 - 86_400, "an end a day before the start"),
        (t0, t0, "an end equal to the start"),
    ] {
        for group in GROUPS {
            let case = format!("{label} / {group}");
            let answer = call(
                &emulator,
                "DescribeDimensionKeys",
                dimension_keys_input(IDENTIFIER, group, "db.load.avg", start, end, None),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a window whose end follows its start is answered with the aligned
/// window — the requested start floored to the effective period, and the latest
/// boundary the service holds (its clock floored to that period) — and with the
/// dimension keys it holds.
#[tokio::test]
async fn a_well_ordered_window_is_answered_with_the_aligned_window_and_its_keys() {
    let emulator = emulator();

    // The rows differ along the period (every documented value, including both
    // published bounds), whether the requested ends already sit on a boundary,
    // and the dimension group asked for.
    let mut cases: Vec<(i64, i64, Option<i64>, &str)> = Vec::new();
    for &period in DOCUMENTED_PERIODS {
        let t0 = past_start(period * 2);
        for (start_offset, end_offset) in [(0, period), (1, period + 1), (period - 1, period * 2)] {
            for group in GROUPS {
                cases.push((t0 + start_offset, t0 + end_offset, Some(period), group));
            }
        }
    }
    // And one row with no `PeriodInSeconds` at all, where the service picks it.
    let t0 = past_start(1_800);
    cases.push((t0, t0 + 1_800, None, "db.sql"));

    for (start, end, period, group) in cases {
        let case = format!("start={start} end={end} period={period:?} group={group}");
        let before = unix_now();
        let answer = call(
            &emulator,
            "DescribeDimensionKeys",
            dimension_keys_input(IDENTIFIER, group, "db.load.avg", start, end, period),
        )
        .await;
        let after = unix_now();
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} {:?}",
            answer.status, answer.body
        );

        let aligned_start = answer
            .member("AlignedStartTime")
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: no AlignedStartTime in {:?}", answer.body));
        let aligned_end = answer
            .member("AlignedEndTime")
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: no AlignedEndTime in {:?}", answer.body));
        assert!(
            aligned_start < aligned_end,
            "case {case}: expected AlignedStartTime < AlignedEndTime, got \
             {aligned_start}..{aligned_end}"
        );
        if let Some(period) = period {
            assert_eq!(
                aligned_start,
                floor_to(start, period),
                "case {case}: expected the start floored to the {period}s boundary, got \
                 {aligned_start}"
            );
            // The end bound is the service's clock floored to the period — the
            // latest boundary it holds — so the requested end does not drive it.
            let candidates = [floor_to(before, period), floor_to(after, period)];
            assert!(
                candidates.contains(&aligned_end),
                "case {case}: expected AlignedEndTime at the current {period}s boundary \
                 ({candidates:?}), got {aligned_end}"
            );
        } else {
            assert!(
                aligned_start <= start && aligned_end <= after,
                "case {case}: expected a start at or before {start} and an end at or before \
                 the clock, got {aligned_start}..{aligned_end}"
            );
        }

        // A dimension key is a group of DB sessions; this emulator runs no
        // engine and holds none, so the window holds no keys — reported as the
        // empty list, not as a 501 and not as invented sessions.
        assert_eq!(
            answer.member("Keys").as_array().map(Vec::len),
            Some(0),
            "case {case}: expected an empty Keys list, got {:?}",
            answer.body
        );
        // Nothing to page, so no cursor is handed out.
        assert!(
            answer.member("NextToken").is_null(),
            "case {case}: expected no NextToken, got {:?}",
            answer.body
        );
        // `PartitionKeys` is the breakdown `PartitionBy` asks for, and this
        // request asked for none.
        assert!(
            answer.member("PartitionKeys").is_null(),
            "case {case}: expected no PartitionKeys without PartitionBy, got {:?}",
            answer.body
        );
    }
}

/// RULE: "If PartitionBy was present in the request, PartitionKeys contains the
/// breakdown of dimension keys by the specified partitions" — present when the
/// request asks for a breakdown, absent when it does not.
#[tokio::test]
async fn partition_keys_are_reported_exactly_when_partition_by_asks_for_them() {
    let emulator = emulator();

    let t0 = past_start(3_600);
    for partition_by in [None, Some(json!({"Group": "db.user"})), Some(json!({"Group": "db.host", "Limit": 5}))] {
        let case = format!("PartitionBy={partition_by:?}");
        let mut input =
            dimension_keys_input(IDENTIFIER, "db.sql", "db.load.avg", t0, t0 + 3_600, Some(60));
        if let Some(partition_by) = &partition_by {
            input["PartitionBy"] = partition_by.clone();
        }

        let answer = call(&emulator, "DescribeDimensionKeys", input).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let reported = answer.member("PartitionKeys").as_array().map(Vec::len);
        let expected = partition_by.as_ref().map(|_| 0);
        assert_eq!(
            reported, expected,
            "case {case}: expected PartitionKeys {expected:?}, got {:?}",
            answer.body
        );
    }
}

/// RULE: `PeriodInSeconds` accepts the documented granularities and nothing
/// else — the same set `GetResourceMetrics` accepts, because it is the member's
/// own documented value list.
#[tokio::test]
async fn period_in_seconds_accepts_exactly_the_documented_granularities() {
    let emulator = emulator();

    for &period in DOCUMENTED_PERIODS {
        let t0 = past_start(period);
        let answer = call(
            &emulator,
            "DescribeDimensionKeys",
            // A window of one period, so even the one-second granularity stays
            // small.
            dimension_keys_input(IDENTIFIER, "db.sql", "db.load.avg", t0, t0 + period, Some(period)),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case period={period} (documented): expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // Each row is one period off the documented set, including values just
    // below and just above every published one.
    let t0 = past_start(3_600);
    for period in [0, 2, 30, 59, 61, 299, 301, 3_599, 3_601, 86_399, 86_401, 172_800] {
        let answer = call(
            &emulator,
            "DescribeDimensionKeys",
            dimension_keys_input(
                IDENTIFIER,
                "db.sql",
                "db.load.avg",
                t0,
                t0 + 3_600,
                Some(period),
            ),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case period={period} (not documented): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case period={period} (not documented): expected InvalidArgumentException, got {:?}",
            answer.body
        );
    }
}

/// RULE: a reporting operation that materialises nothing per period is not
/// refused for the WIDTH of its window — the data-point budget belongs to the
/// operation that allocates a point per period, not to the shared window
/// arithmetic.
#[tokio::test]
async fn a_window_wider_than_the_data_point_budget_is_still_answered() {
    let emulator = emulator();

    // A week at the finest documented granularity is 604 800 periods, far past
    // the data-point budget `GetResourceMetrics` is held to; this operation
    // answers it, because it reports no series over it.
    let week: i64 = 7 * 86_400;
    let t0 = past_start(week);
    let before = unix_now();
    let answer = call(
        &emulator,
        "DescribeDimensionKeys",
        dimension_keys_input(IDENTIFIER, "db.sql", "db.load.avg", t0, t0 + week, Some(1)),
    )
    .await;
    let after = unix_now();
    assert_eq!(
        answer.status, 200,
        "case a week at one-second granularity: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    let aligned_end = answer.member("AlignedEndTime").as_i64().unwrap_or(-1);
    assert_eq!(
        answer.member("AlignedStartTime").as_i64(),
        Some(t0),
        "case a week at one-second granularity: expected the requested start back, got {:?}",
        answer.body
    );
    assert!(
        (before..=after).contains(&aligned_end),
        "case a week at one-second granularity: expected the end bound at the current second \
         ({before}..={after}), got {aligned_end}"
    );
    assert_eq!(
        answer.member("Keys").as_array().map(Vec::len),
        Some(0),
        "case a week at one-second granularity: expected an empty Keys list, got {:?}",
        answer.body
    );

    // The same window IS refused by the operation that would have to
    // materialise a point per period, so the budget is where the allocation is.
    let metrics = call(
        &emulator,
        "GetResourceMetrics",
        json!({
            "ServiceType": "RDS",
            "Identifier": IDENTIFIER,
            "MetricQueries": [{"Metric": "db.load.avg"}],
            "StartTime": t0,
            "EndTime": t0 + week,
            "PeriodInSeconds": 1,
        }),
    )
    .await;
    assert_eq!(
        metrics.error_code(),
        "InvalidArgumentException",
        "case the same window on GetResourceMetrics: expected InvalidArgumentException, got {} \
         {:?}",
        metrics.status,
        metrics.body
    );
}

/// RULE: the data-source authorization gate this service applies to every
/// operation naming a `(ServiceType, Identifier)` pair applies here too — an
/// identifier that is no immutable DB resource id is the uniform
/// `NotAuthorizedException`, never a 501 and never a served empty answer.
#[tokio::test]
async fn an_unmonitored_data_source_is_refused_as_not_authorized() {
    let emulator = emulator();

    // The rows span the forms an identifier can take without being a resource
    // id: a DB instance name, a bare word, a lowercase token, a token with no
    // kind prefix, an empty token.
    let t0 = past_start(3_600);
    for identifier in [
        "my-production-database",
        "database",
        "db-abcdefghijkl",
        "ABCDEFGHIJKLMNOPQRSTU1VW2X",
        "db-",
        "cluster-",
    ] {
        for service_type in ["RDS", "DOCDB"] {
            let case = format!("{identifier:?} / {service_type}");
            let mut input =
                dimension_keys_input(identifier, "db.sql", "db.load.avg", t0, t0 + 3_600, Some(60));
            input["ServiceType"] = json!(service_type);
            let answer = call(&emulator, "DescribeDimensionKeys", input).await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: expected NotAuthorizedException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_message(),
                service_com_amazonaws_pi_performanceinsightsv20180227::state::NOT_AUTHORIZED_MESSAGE,
                "case {case}: expected the service's uniform authorization message, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: this operation hands out no `NextToken` (there is never a partial
/// page), so a token presented to it was not issued for the result set it
/// claims to resume and is refused rather than silently ignored.
#[tokio::test]
async fn a_cursor_this_operation_never_issued_is_refused() {
    let emulator = emulator();

    // The rows differ in where the token came from: one this service issued for
    // a DIFFERENT operation's result set, and one that is merely inside the
    // modelled `NextToken` pattern.
    let t0 = past_start(3_600);
    let mut paged = json!({
        "ServiceType": "RDS",
        "Identifier": IDENTIFIER,
        "MetricQueries": [{"Metric": "db.load.avg"}, {"Metric": "db.sampledload.avg"}],
        "StartTime": t0,
        "EndTime": t0 + 3_600,
        "PeriodInSeconds": 3_600,
    });
    paged["MaxResults"] = json!(1);
    let issued_elsewhere = call(&emulator, "GetResourceMetrics", paged)
        .await
        .member("NextToken")
        .as_str()
        .expect("a truncated GetResourceMetrics page hands back a cursor")
        .to_string();

    for (label, token) in [
        ("a cursor issued for another operation", issued_elsewhere),
        ("a pattern-valid token nothing issued", "abc123_=-".to_string()),
    ] {
        let mut input =
            dimension_keys_input(IDENTIFIER, "db.sql", "db.load.avg", t0, t0 + 3_600, Some(60));
        input["NextToken"] = json!(token);
        let answer = call(&emulator, "DescribeDimensionKeys", input).await;
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {label}: expected InvalidArgumentException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

/// RULE: the key list is computed from the request and nothing else, so
/// repeating a call answers the same keys and the same start bound, and reading
/// dimension keys changes nothing a later caller sees. The END bound tracks the
/// service's clock, so it is the one member a repeat may advance.
#[tokio::test]
async fn repeating_the_same_request_answers_the_same_keys() {
    let emulator = emulator();
    let t0 = past_start(3_600);
    let input = dimension_keys_input(IDENTIFIER, "db.sql", "db.load.avg", t0 + 17, t0 + 3_600, Some(60));

    let first = call(&emulator, "DescribeDimensionKeys", input.clone()).await;
    let second = call(&emulator, "DescribeDimensionKeys", input).await;
    assert_eq!(
        first.status, 200,
        "case the first call: expected 200, got {} {:?}",
        first.status, first.body
    );
    for member in ["AlignedStartTime", "Keys", "PartitionKeys", "NextToken"] {
        assert_eq!(
            first.member(member),
            second.member(member),
            "case repeated call: expected the same {member}, got {:?} then {:?}",
            first.member(member),
            second.member(member)
        );
    }
    assert!(
        second.member("AlignedEndTime").as_i64() >= first.member("AlignedEndTime").as_i64(),
        "case repeated call: expected the end bound to track the clock forward, got {:?} then \
         {:?}",
        first.member("AlignedEndTime"),
        second.member("AlignedEndTime")
    );

    // And the report store the sibling operations own is untouched.
    let listed = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": IDENTIFIER}),
    )
    .await;
    assert_eq!(
        listed.member("AnalysisReports").as_array().map(Vec::len),
        Some(0),
        "case after two reads: expected the report store untouched, got {:?}",
        listed.body
    );
}
