// SPDX-License-Identifier: CC0-1.0

//! The TEXT `PutMetricAlarm` refuses a configuration with, for the two rules that relate
//! `Period` to the rest of the request.
//!
//! A refusal's message is part of its contract: the wire code is `ValidationError` for dozens
//! of unrelated CloudWatch refusals, so the message is the only thing that tells one refusal
//! from another. Each rule below answers with ONE fixed sentence per error class, carrying
//! nothing from the request — so the same sentence comes back whatever period, evaluation
//! period count or time zone the refused request named, on a CREATE and on an UPDATE alike,
//! and the alarm store is left exactly as it was.
//!
//! 1. The evaluation horizon (`EvaluationPeriods` x `Period`) past the ceiling of its period's
//!    regime: one sentence for a period shorter than an hour, one for an hour or longer,
//!    selected only by the regime.
//! 2. A `Period` no clock boundary matches on an alarm whose `EvaluationWindow` is a wall
//!    clock window: one sentence naming the admissible windows in units.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises over —
//! the period, the number of evaluation periods, which side of the regime boundary the period
//! falls on, and the kind of evaluation window — rather than along a name.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    build_router, call, described_alarm, error_code, error_message, merged, metric_alarm_request,
    AlarmKind,
};

/// The error class CloudWatch refuses a configuration it would not create with, as the body
/// of the answer names it: the request-validation class, whose legacy query code
/// (`ValidationError`) travels in the `x-amzn-query-error` header instead.
const REFUSAL_CODE: &str = "ValidationException";

/// The refusal for an evaluation horizon longer than a day, which is the ceiling for a period
/// shorter than an hour.
const DAY_HORIZON_REFUSAL: &str = "Metrics cannot be checked across more than a day \
                                   (EvaluationPeriods * Period must be <= 86400) for alarms \
                                   using period < 3600";

/// The refusal for an evaluation horizon longer than a week, the ceiling for a period of an
/// hour or longer.
const WEEK_HORIZON_REFUSAL: &str = "Metrics cannot be checked across more than a week \
                                    (EvaluationPeriods * Period must be <= 604800) for alarms \
                                    using period >= 3600";

/// The refusal for a `Period` no clock boundary matches on an alarm aligned to clock
/// boundaries.
const WALL_CLOCK_REFUSAL: &str =
    "Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week.";

/// Assert that `request` is refused with HTTP 400 `ValidationError` carrying exactly
/// `expected`, and that it stored no alarm under `name`.
async fn assert_refused_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    request: &Value,
    expected: &str,
) {
    let (status, body) = call(router, "PutMetricAlarm", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the configuration to be refused with HTTP 400, got {status} \
         with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, REFUSAL_CODE,
        "case {case:?}: expected the error code {REFUSAL_CODE}, got {code:?} in {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal {expected:?}, got {message:?}"
    );
    let stored = described_alarm(router, AlarmKind::Metric, name).await;
    assert_eq!(
        stored, None,
        "case {case:?}: a refused configuration must store nothing, got {stored:?}"
    );
}

/// Assert that `request` is accepted and the alarm it configures reads back with the `Period`
/// and `EvaluationPeriods` it was created with.
async fn assert_accepted(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    request: &Value,
    period: i64,
    periods: i64,
) {
    let (status, body) = call(router, "PutMetricAlarm", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the configuration to be accepted with HTTP 200, got {status} \
         with body {body}"
    );
    let stored = described_alarm(router, AlarmKind::Metric, name)
        .await
        .unwrap_or_default();
    assert_eq!(
        stored.get("Period").and_then(Value::as_i64),
        Some(period),
        "case {case:?}: the stored alarm must carry the period it was created with, got {stored}"
    );
    assert_eq!(
        stored.get("EvaluationPeriods").and_then(Value::as_i64),
        Some(periods),
        "case {case:?}: the stored alarm must carry the evaluation periods it was created \
         with, got {stored}"
    );
}

#[tokio::test]
async fn the_evaluation_horizon_refusal_states_the_rule_of_the_periods_regime() {
    // (case, Period, EvaluationPeriods, the refusal, or None when the alarm is created)
    //
    // Rows differ along the period, the number of evaluation periods, and which side of the
    // one-hour regime boundary the period falls on: each regime's ceiling exactly, one
    // evaluation period past it, and well past it, at periods the issue's legs did not name
    // (half an hour, a whole week, two hours) as well as the ones they did.
    let rows: [(&str, i64, i64, Option<&str>); 18] = [
        ("one day of ten-second periods", 10, 8_640, None),
        (
            "one period past a day at ten seconds",
            10,
            8_641,
            Some(DAY_HORIZON_REFUSAL),
        ),
        ("one day of one-minute periods", 60, 1_440, None),
        (
            "one period past a day at one minute",
            60,
            1_441,
            Some(DAY_HORIZON_REFUSAL),
        ),
        (
            "well past a day at one minute",
            60,
            1_500,
            Some(DAY_HORIZON_REFUSAL),
        ),
        ("one day of five-minute periods", 300, 288, None),
        (
            "one period past a day at five minutes",
            300,
            289,
            Some(DAY_HORIZON_REFUSAL),
        ),
        ("one day of half-hour periods", 1_800, 48, None),
        (
            "one period past a day at half an hour",
            1_800,
            49,
            Some(DAY_HORIZON_REFUSAL),
        ),
        (
            "a day and a half at the longest sub-hour period",
            3_540,
            37,
            Some(DAY_HORIZON_REFUSAL),
        ),
        ("one day of one-hour periods", 3_600, 24, None),
        ("seven days of one-hour periods", 3_600, 168, None),
        (
            "one period past a week at one hour",
            3_600,
            169,
            Some(WEEK_HORIZON_REFUSAL),
        ),
        ("seven days of one-day periods", 86_400, 7, None),
        (
            "one period past a week at one day",
            86_400,
            8,
            Some(WEEK_HORIZON_REFUSAL),
        ),
        ("one week in a single period", 604_800, 1, None),
        (
            "two weeks in two one-week periods",
            604_800,
            2,
            Some(WEEK_HORIZON_REFUSAL),
        ),
        (
            "a week and a day of two-hour periods",
            7_200,
            97,
            Some(WEEK_HORIZON_REFUSAL),
        ),
    ];

    let router = build_router();
    for (case, period, periods, refusal) in rows {
        let name = format!("horizon-message-{period}-{periods}");
        let request = merged(
            metric_alarm_request(&name),
            &json!({"Period": period, "EvaluationPeriods": periods}),
        );
        match refusal {
            Some(expected) => assert_refused_with(&router, case, &name, &request, expected).await,
            None => assert_accepted(&router, case, &name, &request, period, periods).await,
        }
    }
}

#[tokio::test]
async fn the_wall_clock_window_refusal_names_the_admissible_windows() {
    // (case, Period, whether the alarm aligns its window to clock boundaries, whether the
    //  alarm is created)
    //
    // Rows differ along the period and the kind of evaluation window: every period a clock
    // boundary matches, periods that match none (sub-minute, between the matching ones, and
    // longer than the longest), and the same unmatched periods in the default sliding window,
    // which the rule does not speak about.
    let rows: [(&str, i64, bool, bool); 17] = [
        ("one minute, aligned", 60, true, true),
        ("five minutes, aligned", 300, true, true),
        ("one hour, aligned", 3_600, true, true),
        ("one day, aligned", 86_400, true, true),
        ("one week, aligned", 604_800, true, true),
        ("ten seconds, aligned", 10, true, false),
        ("twenty seconds, aligned", 20, true, false),
        ("thirty seconds, aligned", 30, true, false),
        ("two minutes, aligned", 120, true, false),
        ("ten minutes, aligned", 600, true, false),
        ("half an hour, aligned", 1_800, true, false),
        ("two hours, aligned", 7_200, true, false),
        ("twelve hours, aligned", 43_200, true, false),
        ("two days, aligned", 172_800, true, false),
        ("two minutes, sliding", 120, false, true),
        ("half an hour, sliding", 1_800, false, true),
        ("twelve hours, sliding", 43_200, false, true),
    ];

    let router = build_router();
    for (case, period, aligned, accepted) in rows {
        let name = format!("wall-clock-message-{period}-{aligned}");
        let window = if aligned {
            // The time zone varies nothing about the rule, so a row names one the refusal
            // still may not echo.
            json!({"WallClockWindow": {"Timezone": "America/New_York"}})
        } else {
            json!({"SlidingWindow": {}})
        };
        let request = merged(
            metric_alarm_request(&name),
            &json!({
                "Period": period,
                "EvaluationPeriods": 1,
                "EvaluationWindow": window.clone(),
            }),
        );
        if accepted {
            assert_accepted(&router, case, &name, &request, period, 1).await;
            let stored = described_alarm(&router, AlarmKind::Metric, &name)
                .await
                .unwrap_or_default();
            assert_eq!(
                stored.get("EvaluationWindow"),
                Some(&window),
                "case {case:?}: the stored alarm must carry the evaluation window it was \
                 created with, got {stored}"
            );
        } else {
            assert_refused_with(&router, case, &name, &request, WALL_CLOCK_REFUSAL).await;
        }
    }
}

#[tokio::test]
async fn an_update_refused_for_its_period_answers_the_same_text_and_changes_nothing() {
    // The rules hold for the write that REPLACES an alarm exactly as for the one that creates
    // it, and a refused update leaves the alarm it would have replaced untouched: same
    // message, same stored configuration.
    //
    // (case, the members the update names, the refusal it must answer with)
    let rows: [(&str, Value, &str); 4] = [
        (
            "one period past a day at one minute",
            json!({"Period": 60, "EvaluationPeriods": 1_441}),
            DAY_HORIZON_REFUSAL,
        ),
        (
            "one period past a week at one hour",
            json!({"Period": 3_600, "EvaluationPeriods": 169}),
            WEEK_HORIZON_REFUSAL,
        ),
        (
            "a week and a day of two-hour periods",
            json!({"Period": 7_200, "EvaluationPeriods": 97}),
            WEEK_HORIZON_REFUSAL,
        ),
        (
            "two minutes aligned to clock boundaries",
            json!({
                "Period": 120,
                "EvaluationPeriods": 1,
                "EvaluationWindow": {"WallClockWindow": {"Timezone": "UTC"}},
            }),
            WALL_CLOCK_REFUSAL,
        ),
    ];

    let router = build_router();
    for (index, (case, update, expected)) in rows.into_iter().enumerate() {
        let name = format!("update-refusal-{index}");
        // The alarm the refused update would have replaced: a day of one-minute periods, the
        // sub-hour ceiling exactly.
        let created = merged(
            metric_alarm_request(&name),
            &json!({"Period": 60, "EvaluationPeriods": 1_440}),
        );
        assert_accepted(&router, case, &name, &created, 60, 1_440).await;

        let request = merged(metric_alarm_request(&name), &update);
        let (status, body) = call(&router, "PutMetricAlarm", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the update to be refused with HTTP 400, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, REFUSAL_CODE,
            "case {case:?}: expected the error code {REFUSAL_CODE}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );

        let stored = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_default();
        assert_eq!(
            stored.get("Period").and_then(Value::as_i64),
            Some(60),
            "case {case:?}: a refused update must leave the stored period alone, got {stored}"
        );
        assert_eq!(
            stored.get("EvaluationPeriods").and_then(Value::as_i64),
            Some(1_440),
            "case {case:?}: a refused update must leave the stored evaluation periods alone, \
             got {stored}"
        );
        assert_eq!(
            stored.get("EvaluationWindow"),
            None,
            "case {case:?}: a refused update must not give the alarm an evaluation window, got \
             {stored}"
        );
    }
}
