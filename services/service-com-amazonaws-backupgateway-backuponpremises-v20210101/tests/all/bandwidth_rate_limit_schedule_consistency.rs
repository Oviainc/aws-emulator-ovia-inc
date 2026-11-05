// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the consistency `PutBandwidthRateLimitSchedule` requires of the
//! schedule it is sent, beyond the per-member constraints the boundary validator enforces.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the interval helpers of the sibling
//! `bandwidth_rate_limit_schedule` suite, so what is asserted is the wire behaviour and the
//! only way a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: a bandwidth rate limit schedule is what ONE gateway appliance
//! applies, so each interval is a period of time inside a single day of the week and no
//! minute of any day of the week may carry two rate limits — the service persists a
//! schedule only when every interval of it ends no earlier than it starts, names each day
//! at most once, and shares no minute of a shared day with another interval; a schedule
//! that fails that is refused with the modelled `ValidationException` (HTTP 400) and
//! nothing is written.
//!
//! The windows are INCLUSIVE of their end minute — "The bandwidth rate limit interval ends
//! at the end of the minute. To end an interval at the end of an hour, use the value `59`"
//! (`smithy_query(query="doc",shape="BandwidthRateLimitInterval",
//! member="EndMinuteOfHour")`) — so two intervals that merely touch at one minute DO
//! clash, while an interval ending at `:59` and one starting at the next hour do not.
//!
//! The tables below span that class along what the rule generalises over: how two windows
//! lie against each other (disjoint, touching at one minute, partly overlapping, nested,
//! identical), how their day sets lie against each other (equal, sharing one entry,
//! disjoint), the number of intervals in the schedule (one, a pair, a long list whose
//! offending pair is not the first), where in the list the offending interval sits, and the
//! form of an inverted interval (an earlier hour, an earlier minute of the same hour). The
//! last test is a sweep: one fixed interval against a grid of second intervals, every
//! verdict compared to the rule computed independently in the test.

use serde_json::Value;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::ops::types::BandwidthRateLimitInterval;
use backup_gateway::state::BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID;

use crate::bandwidth_rate_limit_schedule::{
    expected_intervals, get_schedule, interval, persisted_intervals,
};
use crate::gateway_lifecycle::{build_engine, build_router, create, never_created_arn, Reply};
use crate::put_bandwidth_rate_limit_schedule::put_schedule;

/// The minute of the day an interval endpoint names, for the rule computed in this test.
fn minute_of_day(hour_of_day: i32, minute_of_hour: i32) -> i32 {
    hour_of_day * 60 + minute_of_hour
}

/// Whether `one` and `other` cannot be applied together: they share a day of the week and
/// their end-inclusive windows share at least one minute. This is the rule as the test
/// states it, computed here so a sweep below compares the service to the rule rather than
/// to a second copy of the service's own code path.
fn intervals_clash(one: &BandwidthRateLimitInterval, other: &BandwidthRateLimitInterval) -> bool {
    let shares_a_day = one
        .days_of_week
        .iter()
        .any(|day| other.days_of_week.contains(day));
    let one_start = minute_of_day(one.start_hour_of_day, one.start_minute_of_hour);
    let one_end = minute_of_day(one.end_hour_of_day, one.end_minute_of_hour);
    let other_start = minute_of_day(other.start_hour_of_day, other.start_minute_of_hour);
    let other_end = minute_of_day(other.end_hour_of_day, other.end_minute_of_hour);
    shares_a_day && one_start <= other_end && other_start <= one_end
}

/// Assert `reply` is the modelled refusal of a schedule that cannot hold together: HTTP
/// 400, the `ValidationException` the operation models
/// (`smithy_query(query="operation",operation="PutBandwidthRateLimitSchedule")`), the
/// `ErrorCode` that names the condition and a message that states it.
fn assert_refused(case: &str, reply: &Reply) {
    assert_eq!(
        (
            reply.status,
            reply.code.as_deref(),
            reply.body.get("ErrorCode").and_then(Value::as_str),
        ),
        (
            400,
            Some("ValidationException"),
            Some(BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID),
        ),
        "case {case:?}: a schedule whose intervals cannot hold together must be refused \
         with 400 ValidationException and the {BANDWIDTH_RATE_LIMIT_SCHEDULE_INVALID:?} \
         ErrorCode, got {} {:?}",
        reply.status,
        reply.body
    );
    assert!(
        reply
            .body
            .get("Message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: the refusal must state why the schedule was refused, got {:?}",
        reply.body
    );
}

/// The schedules that cannot be applied, spanning how the windows and the day sets lie
/// against each other, the length of the schedule and the position of the offending
/// interval.
fn clashing_rows() -> Vec<(&'static str, Vec<BandwidthRateLimitInterval>)> {
    // Twenty intervals — the published maximum of `BandwidthRateLimitIntervals`
    // (`@length(min: 0, max: 20)`) — one per hour on a day of its own, with the LAST two
    // sharing a day and a minute: the offending pair is as far from the front of the list
    // as the model allows.
    let mut twenty: Vec<BandwidthRateLimitInterval> = (0..20)
        .map(|hour| interval(hour, 0, hour, 59, &[hour % 7], Some(51_200)))
        .collect();
    twenty[19] = interval(18, 30, 19, 30, &[18 % 7], Some(102_400));

    vec![
        (
            "two intervals on one day whose windows partly overlap",
            vec![
                interval(9, 0, 17, 59, &[1], Some(51_200)),
                interval(10, 0, 12, 0, &[1], Some(8_000_000)),
            ],
        ),
        (
            "an interval nested inside another on the same day",
            vec![
                interval(0, 0, 23, 59, &[4], Some(51_200)),
                interval(12, 0, 12, 30, &[4], Some(102_400)),
            ],
        ),
        (
            "the same interval twice",
            vec![
                interval(6, 15, 7, 45, &[2, 3], Some(51_200)),
                interval(6, 15, 7, 45, &[2, 3], Some(51_200)),
            ],
        ),
        (
            "two windows touching at a single minute, which the inclusive end covers",
            vec![
                interval(1, 0, 2, 30, &[5], Some(51_200)),
                interval(2, 30, 3, 0, &[5], Some(102_400)),
            ],
        ),
        (
            "day sets that share exactly one entry, on overlapping windows",
            vec![
                interval(8, 0, 9, 0, &[0, 1, 2], Some(51_200)),
                interval(8, 30, 10, 0, &[2, 4, 6], Some(102_400)),
            ],
        ),
        (
            "two single-minute intervals on the same minute of the same day",
            vec![
                interval(13, 7, 13, 7, &[6], Some(51_200)),
                interval(13, 7, 13, 7, &[6], Some(8_000_000_000_000)),
            ],
        ),
        (
            "a clashing pair after intervals that hold together",
            vec![
                interval(0, 0, 0, 59, &[0], Some(51_200)),
                interval(1, 0, 1, 59, &[1], Some(51_200)),
                interval(2, 0, 2, 59, &[2], Some(51_200)),
                interval(2, 59, 3, 30, &[2], Some(102_400)),
            ],
        ),
        (
            "the twenty-interval maximum whose last two intervals clash",
            twenty,
        ),
        (
            "a single interval ending in an earlier hour than it starts",
            vec![interval(22, 0, 6, 0, &[3], Some(51_200))],
        ),
        (
            "a single interval ending in an earlier minute of the hour it starts in",
            vec![interval(10, 30, 10, 29, &[0], None)],
        ),
        (
            "the last interval of a schedule ends before it starts",
            vec![
                interval(0, 0, 1, 0, &[1], Some(51_200)),
                interval(23, 59, 0, 0, &[2], None),
            ],
        ),
        (
            "a DaysOfWeek list that repeats a day",
            vec![interval(9, 0, 10, 0, &[1, 1], Some(51_200))],
        ),
        (
            "a DaysOfWeek list that repeats a day it does not start with",
            vec![interval(9, 0, 10, 0, &[0, 3, 3], None)],
        ),
        (
            "a DaysOfWeek list that repeats a day across other days",
            vec![interval(0, 0, 23, 59, &[6, 0, 2, 6], Some(1_048_576))],
        ),
    ]
}

/// The schedules that CAN be applied — the other side of the same rule: windows that stop
/// short of one another, equal windows on disjoint days, the degenerate single-minute
/// window, and the whole day.
fn holding_rows() -> Vec<(&'static str, Vec<BandwidthRateLimitInterval>)> {
    vec![
        (
            "back-to-back windows on one day: one ends at :59, the next starts on the hour",
            vec![
                interval(9, 0, 9, 59, &[1], Some(51_200)),
                interval(10, 0, 10, 59, &[1], Some(102_400)),
            ],
        ),
        (
            "windows one minute apart on one day",
            vec![
                interval(9, 0, 9, 30, &[1], Some(51_200)),
                interval(9, 32, 9, 40, &[1], Some(102_400)),
            ],
        ),
        (
            "the same window on disjoint days",
            vec![
                interval(9, 0, 17, 0, &[1, 2, 3, 4, 5], Some(51_200)),
                interval(9, 0, 17, 0, &[0, 6], Some(8_000_000)),
            ],
        ),
        (
            "fully overlapping windows on disjoint days",
            vec![
                interval(0, 0, 23, 59, &[0], Some(51_200)),
                interval(6, 0, 7, 0, &[1], Some(102_400)),
                interval(6, 0, 7, 0, &[2], Some(204_800)),
            ],
        ),
        (
            "a single-minute window, start and end on the same minute",
            vec![interval(0, 0, 0, 0, &[0], Some(51_200))],
        ),
        (
            "a window covering a whole day",
            vec![interval(0, 0, 23, 59, &[0, 1, 2, 3, 4, 5, 6], Some(51_200))],
        ),
        (
            "every day of the week in one interval, each named once",
            vec![interval(23, 59, 23, 59, &[6, 5, 4, 3, 2, 1, 0], None)],
        ),
        ("the empty schedule", vec![]),
    ]
}

/// A schedule whose intervals cannot hold together is refused, and the record keeps the
/// schedule it already had: nothing of the refused request is persisted, and a read never
/// reports a schedule the appliance could not apply.
#[tokio::test]
async fn a_schedule_whose_intervals_cannot_hold_together_is_refused_and_writes_nothing() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-clashing").await;

    // A schedule that DOES hold together, so every refusal below is asserted against a
    // record that already carries one: a refused put may neither replace it nor clear it.
    let kept = vec![interval(5, 0, 6, 0, &[3], Some(51_200))];
    assert_eq!(
        put_schedule(&router, &arn, &kept).await.status,
        200,
        "the gateway must take a schedule that holds together before the refusals below"
    );
    let before = persisted_intervals(&engine, &arn);

    for (case, intervals) in &clashing_rows() {
        let reply = put_schedule(&router, &arn, intervals).await;
        assert_refused(case, &reply);
        assert_eq!(
            persisted_intervals(&engine, &arn),
            before,
            "case {case:?}: a refused put must leave the stored schedule exactly as it was"
        );
        let read = get_schedule(&router, &arn).await;
        assert_eq!(
            (
                read.status,
                read.body
                    .get("BandwidthRateLimitIntervals")
                    .cloned()
                    .unwrap_or(Value::Null)
            ),
            (200, expected_intervals(&kept)),
            "case {case:?}: a read must still report the schedule that was accepted, got \
             {} {:?}",
            read.status,
            read.body
        );
    }

    // The schedule is a fact of the REQUEST, so it is judged before the store is consulted:
    // a clashing schedule for an ARN no gateway is stored under is refused as the invalid
    // schedule it is, and still writes nothing.
    let unknown = never_created_arn();
    let reply = put_schedule(
        &router,
        &unknown,
        &[
            interval(9, 0, 17, 59, &[1], Some(51_200)),
            interval(10, 0, 12, 0, &[1], Some(8_000_000)),
        ],
    )
    .await;
    assert_refused("a clashing schedule for an ARN with no stored gateway", &reply);
    let gone = get_schedule(&router, &unknown).await;
    assert_eq!(
        (gone.status, gone.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "a refused put must not have created a record, got {} {:?}",
        gone.status,
        gone.body
    );
}

/// The other side of the rule: every schedule whose intervals hold together is persisted
/// and reads back, so the refusal above is the rule and not a blanket narrowing of the
/// modelled input.
#[tokio::test]
async fn a_schedule_whose_intervals_hold_together_is_accepted() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (index, (case, intervals)) in holding_rows().iter().enumerate() {
        let arn = create(&router, &format!("gw-holds-{index}")).await;
        let reply = put_schedule(&router, &arn, intervals).await;
        assert_eq!(
            (reply.status, reply.str_at("/GatewayArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: a schedule whose intervals hold together must be accepted with \
             the record's own ARN, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_intervals(&engine, &arn),
            format!("{intervals:?}"),
            "case {case:?}: the accepted schedule must be stored as it was sent"
        );
        let read = get_schedule(&router, &arn).await;
        assert_eq!(
            (
                read.status,
                read.body
                    .get("BandwidthRateLimitIntervals")
                    .cloned()
                    .unwrap_or(Value::Null)
            ),
            (200, expected_intervals(intervals)),
            "case {case:?}: the accepted schedule must read back in the order it was sent, \
             got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// A sweep of one fixed interval against a GRID of second intervals — every combination of
/// a start hour, a start minute, a window length and a day set — with each verdict compared
/// to the rule computed in this test, not to a table of expected answers: the service
/// accepts the pair exactly when the two intervals share no minute of a shared day.
#[tokio::test]
async fn whether_a_pair_is_accepted_follows_the_shared_minute_of_a_shared_day() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-sweep").await;

    // The fixed interval: Tuesday and Thursday, 10:00 through the end of 11:29.
    let fixed = interval(10, 0, 11, 29, &[2, 4], Some(51_200));

    // The grid the second interval walks: start hours around the fixed window, start
    // minutes at the hour, inside it and at its last minute, window lengths from the
    // degenerate single minute to one spanning hours, day sets equal to, overlapping and
    // disjoint from the fixed one, and a start hour late enough that the longer windows
    // would run past the end of the day.
    let start_hours = [9, 10, 11, 12, 23];
    let start_minutes = [0, 29, 30, 59];
    let lengths_in_minutes = [0, 30, 95];
    let day_sets: [&[i32]; 5] = [&[2, 4], &[2], &[4, 5], &[0, 1, 3, 5, 6], &[6]];

    // The record starts out carrying the fixed interval alone, which is what a refused put
    // below has to leave behind.
    assert_eq!(
        put_schedule(&router, &arn, std::slice::from_ref(&fixed))
            .await
            .status,
        200,
        "the fixed interval alone must be accepted"
    );
    let mut stored = vec![fixed.clone()];
    let mut accepted = 0usize;
    let mut refused = 0usize;

    for start_hour in start_hours {
        for start_minute in start_minutes {
            for length in lengths_in_minutes {
                let end = minute_of_day(start_hour, start_minute) + length;
                // The grid is built in minutes of the day, so a row that would run past
                // the end of the day is not a row of this class: `EndHourOfDay` is
                // `@range(min: 0, max: 23)` and such a request is refused at the boundary,
                // which the sibling suite covers.
                if end > minute_of_day(23, 59) {
                    continue;
                }
                for days in day_sets {
                    let second = interval(
                        start_hour,
                        start_minute,
                        end / 60,
                        end % 60,
                        days,
                        Some(102_400),
                    );
                    let schedule = vec![fixed.clone(), second.clone()];
                    let case = format!("{fixed:?} with {second:?}");
                    let reply = put_schedule(&router, &arn, &schedule).await;

                    if intervals_clash(&fixed, &second) {
                        refused += 1;
                        assert_refused(&case, &reply);
                    } else {
                        accepted += 1;
                        assert_eq!(
                            (reply.status, reply.str_at("/GatewayArn")),
                            (200, Some(arn.as_str())),
                            "case {case:?}: two intervals sharing no minute of a shared day \
                             must be accepted, got {} {:?}",
                            reply.status,
                            reply.body
                        );
                        stored = schedule;
                    }

                    assert_eq!(
                        persisted_intervals(&engine, &arn),
                        format!("{stored:?}"),
                        "case {case:?}: the store must hold exactly the schedule last \
                         accepted"
                    );
                }
            }
        }
    }

    // The grid really exercised both verdicts, so neither branch above is vacuous.
    assert!(
        accepted > 0 && refused > 0,
        "the sweep must cover both verdicts, got {accepted} accepted and {refused} refused"
    );
}
