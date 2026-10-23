// SPDX-License-Identifier: CC0-1.0

//! `GetMinuteUsage`: the account's contact minutes for one calendar month.
//!
//! The RULE under test: minute usage is DERIVED from the contacts this service persisted
//! and from the account's reserved-minutes plan — a contact counts in the calendar month
//! (UTC) its pass starts in, for the whole minutes of antenna time its `startTime` to
//! `endTime` window holds, while its status still holds that antenna time; the minutes
//! whose pass has not started yet are the upcoming ones; and the plan covering that month
//! is what makes the account a reserved-minutes customer, supplies the allocation and is
//! what the remaining minutes are measured against (never below zero).
//!
//! So the suites below assert that rule over a SET of inputs rather than one call:
//!
//! * a table of passes differing in length, in rounding and in which month they start in,
//!   read back month by month (including a leap-year February and a pass that crosses a
//!   month boundary);
//! * every value of the model's `ContactStatus`, asserting which ones still hold antenna
//!   time — and the same release through the front door, with `CancelContact`;
//! * the clock split: a pass already over, one in progress and one still to come;
//! * a table of reserved-minutes plans against a fixed month of scheduled minutes;
//! * the operator setting that puts the account on a plan, including entries it refuses.
//!
//! Every HTTP request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use service_com_amazonaws_groundstation_groundstation::state::{
    parse_reserved_minutes, service_state, ReservedMinutesPlan,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{create_mission_profile, harness_with_engine, send, Reply};

/// Midnight-anchored epoch seconds for a UTC civil instant, so a test case reads as the
/// calendar date it is about.
fn epoch(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, minute, 0))
        .unwrap_or_else(|| panic!("{year}-{month}-{day} {hour}:{minute} is a UTC instant"))
        .and_utc()
        .timestamp()
}

/// The UTC calendar month `instant` (epoch seconds) falls in.
fn month_of(instant: i64) -> (i32, i32) {
    let moment = chrono::DateTime::from_timestamp(instant, 0)
        .unwrap_or_else(|| panic!("{instant} is an epoch-seconds instant"));
    let month = chrono::Datelike::month(&moment);
    (
        chrono::Datelike::year(&moment),
        i32::try_from(month).expect("a month number fits in i32"),
    )
}

/// `GetMinuteUsage` for one month, asserting it answered.
async fn minute_usage(router: &MultiServiceRouter, (year, month): (i32, i32)) -> Reply {
    let reply = send(
        router,
        "POST",
        "/minute-usage",
        Some(json!({"year": year, "month": month})),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "GetMinuteUsage {year}-{month:02}: expected 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// One integer member of a `GetMinuteUsage` answer. Absent is a failure, not a zero: the
/// operation reports every member it models.
fn minutes(reply: &Reply, member: &str, case: &str) -> i64 {
    reply.field(member).as_i64().unwrap_or_else(|| {
        panic!("case {case}: {member} is not an integer in {}", reply.body)
    })
}

/// `ReserveContact` for a window, answering the `contactId`.
async fn reserve_pass(
    router: &MultiServiceRouter,
    profile_arn: &str,
    case: &str,
    start: i64,
    seconds: i64,
) -> String {
    let reply = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": "Ohio-1",
            "startTime": start,
            "endTime": start + seconds,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: ReserveContact expected 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("contactId")
}

/// One pass to reserve, and the minutes of antenna time it holds.
struct Pass {
    case: &'static str,
    start: i64,
    seconds: i64,
    minutes: i64,
}

#[tokio::test]
async fn usage_counts_each_months_passes_in_whole_minutes() {
    let (router, _engine) = harness_with_engine();
    // A one-second minimum viable duration, so the shortest row in the table is a window
    // this profile admits (`minimumViableContactDurationSeconds` is bounded 1..21600 —
    // see OPERATIONS.md's CreateMissionProfile entry).
    let profile_arn = create_mission_profile(&router, "minute usage", 1, Some(120), Some(180)).await;

    // Rows differ along pass length (a whole number of minutes, a part minute, one
    // second, three hours), along which month the pass starts in (the subject month, the
    // month before, the month after, a leap-year February) and along where in the month
    // it sits (the first instant, and the last minute, from which it runs past the month
    // end).
    let passes = [
        Pass {
            case: "whole-minutes-at-the-first-instant-of-the-month",
            start: epoch(2030, 7, 1, 0, 0),
            seconds: 600,
            minutes: 10,
        },
        Pass {
            case: "part-minute-rounds-up",
            start: epoch(2030, 7, 2, 12, 0),
            seconds: 601,
            minutes: 11,
        },
        Pass {
            case: "one-second-pass-counts-one-minute",
            start: epoch(2030, 7, 3, 8, 0),
            seconds: 1,
            minutes: 1,
        },
        Pass {
            case: "three-hour-pass",
            start: epoch(2030, 7, 4, 10, 0),
            seconds: 3 * 3600,
            minutes: 180,
        },
        Pass {
            case: "starts-in-the-months-last-minute-and-runs-past-its-end",
            start: epoch(2030, 7, 31, 23, 59),
            seconds: 2 * 3600,
            minutes: 120,
        },
        Pass {
            case: "starts-at-the-first-instant-of-the-next-month",
            start: epoch(2030, 8, 1, 0, 0),
            seconds: 1800,
            minutes: 30,
        },
        Pass {
            case: "the-month-before",
            start: epoch(2030, 6, 15, 5, 30),
            seconds: 3600,
            minutes: 60,
        },
        Pass {
            case: "a-leap-years-29th-of-february",
            start: epoch(2028, 2, 29, 23, 0),
            seconds: 3660,
            minutes: 61,
        },
    ];

    for pass in &passes {
        reserve_pass(&router, &profile_arn, pass.case, pass.start, pass.seconds).await;
    }

    // Every month the table touches, plus two it does not: a month between the leap
    // February and the subject month, and one just before the subject month.
    let queried = [
        month_of(epoch(2030, 7, 1, 0, 0)),
        month_of(epoch(2030, 6, 1, 0, 0)),
        month_of(epoch(2030, 8, 1, 0, 0)),
        month_of(epoch(2028, 2, 1, 0, 0)),
        month_of(epoch(2028, 3, 1, 0, 0)),
        month_of(epoch(2030, 5, 1, 0, 0)),
    ];

    for (year, month) in queried {
        // The expectation comes from the rule, not from a recorded number: the minutes of
        // the rows whose pass STARTS in this month.
        let expected: i64 = passes
            .iter()
            .filter(|pass| month_of(pass.start) == (year, month))
            .map(|pass| pass.minutes)
            .sum();
        let case = format!("{year}-{month:02}");
        let reply = minute_usage(&router, (year, month)).await;
        assert_eq!(
            minutes(&reply, "totalScheduledMinutes", &case),
            expected,
            "case {case}: totalScheduledMinutes should be the minutes of the passes \
             starting in this month, got {}",
            reply.body
        );
        // Every pass in the table is still ahead of the clock, so all of those minutes
        // are upcoming too.
        assert_eq!(
            minutes(&reply, "upcomingMinutesScheduled", &case),
            expected,
            "case {case}: every pass in this month is in the future, so its minutes are \
             upcoming, got {}",
            reply.body
        );
    }
}

/// Every value of the model's `ContactStatus`, beside whether a contact in it still holds
/// antenna time for the account.
///
/// Source: `smithy_query(query="shape", shape="ContactStatus",
/// model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/\
/// com-amazonaws-groundstation-groundstation.json")` — all thirteen values, not only the
/// ones a request can reach today, because the classification is over the whole enum.
const STATUS_HOLDS_ANTENNA_TIME: &[(&str, bool)] = &[
    ("AVAILABLE", false),
    ("AWS_CANCELLED", false),
    ("AWS_FAILED", false),
    ("CANCELLED", false),
    ("CANCELLING", true),
    ("COMPLETED", true),
    ("FAILED", true),
    ("FAILED_TO_SCHEDULE", false),
    ("PASS", true),
    ("POSTPASS", true),
    ("PREPASS", true),
    ("SCHEDULED", true),
    ("SCHEDULING", true),
];

#[tokio::test]
async fn usage_counts_only_the_statuses_that_hold_antenna_time() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "status table", 60, None, None).await;
    let start = epoch(2029, 4, 9, 6, 0);
    let month = month_of(start);
    let pass_minutes = 45_i64;
    let contact_id = reserve_pass(
        &router,
        &profile_arn,
        "status table",
        start,
        pass_minutes * 60,
    )
    .await;

    for (status, holds) in STATUS_HOLDS_ANTENNA_TIME {
        // Most of these statuses are reached by the scheduler and the pass itself, which
        // no request drives, so the stored contact's status is set directly — the one
        // place this suite reads the store instead of the front door.
        {
            let state = service_state(&*engine).expect("this service's state store");
            let mut guard = state.lock();
            guard
                .contacts
                .get_mut(&contact_id)
                .expect("the reserved contact")
                .contact_status = (*status).to_string();
        }
        let expected = if *holds { pass_minutes } else { 0 };
        let reply = minute_usage(&router, month).await;
        assert_eq!(
            minutes(&reply, "totalScheduledMinutes", status),
            expected,
            "case {status}: a contact in this status should count {expected} minutes, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn cancelling_a_contact_releases_its_minutes() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "cancellation", 60, None, None).await;
    let start = epoch(2031, 11, 4, 14, 0);
    let month = month_of(start);
    let contact_id = reserve_pass(&router, &profile_arn, "cancellation", start, 30 * 60).await;

    let before = minute_usage(&router, month).await;
    assert_eq!(
        minutes(&before, "totalScheduledMinutes", "before-cancelling"),
        30,
        "case before-cancelling: the reserved pass holds 30 minutes, got {}",
        before.body
    );

    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case cancelling: CancelContact expected 200, got {} {}",
        cancelled.status, cancelled.body
    );

    // The read-back the cancellation is visible through, so the usage below is measured
    // against a contact the API itself reports as released.
    let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case cancelling: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    assert_eq!(
        described.string("contactStatus"),
        "CANCELLED",
        "case cancelling: the contact should read back CANCELLED, got {}",
        described.body
    );

    let after = minute_usage(&router, month).await;
    assert_eq!(
        minutes(&after, "totalScheduledMinutes", "after-cancelling"),
        0,
        "case after-cancelling: a cancelled pass releases its minutes, got {}",
        after.body
    );
    assert_eq!(
        minutes(&after, "upcomingMinutesScheduled", "after-cancelling"),
        0,
        "case after-cancelling: a released pass is not upcoming either, got {}",
        after.body
    );
}

#[tokio::test]
async fn upcoming_minutes_are_the_minutes_whose_pass_has_not_started() {
    let now = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch-seconds instant fits in i64");

    // Rows differ in where the pass sits relative to the clock: wholly over, in progress,
    // and still to come. Each runs on its own store so the month it falls in — which can
    // differ between rows near a month boundary — is read in isolation.
    let rows: [(&str, i64, i64, i64); 3] = [
        ("pass-already-over", now - 4 * 3600, 30 * 60, 0),
        ("pass-in-progress", now - 60, 30 * 60, 0),
        ("pass-still-to-come", now + 4 * 3600, 30 * 60, 30),
    ];

    for (case, start, seconds, expected_upcoming) in rows {
        let (router, _engine) = harness_with_engine();
        let profile_arn = create_mission_profile(&router, "clock split", 60, None, None).await;
        reserve_pass(&router, &profile_arn, case, start, seconds).await;

        let reply = minute_usage(&router, month_of(start)).await;
        assert_eq!(
            minutes(&reply, "totalScheduledMinutes", case),
            30,
            "case {case}: the pass holds 30 minutes whenever it runs, got {}",
            reply.body
        );
        assert_eq!(
            minutes(&reply, "upcomingMinutesScheduled", case),
            expected_upcoming,
            "case {case}: expected {expected_upcoming} upcoming minutes, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_reserved_minutes_plan_decides_the_allocation_and_what_remains() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "allocation", 60, None, None).await;
    let start = epoch(2032, 9, 12, 3, 0);
    let (year, month) = month_of(start);
    let scheduled = 90_i64;
    reserve_pass(&router, &profile_arn, "allocation", start, scheduled * 60).await;

    /// A plan covering every month with `minutes`.
    fn standing(minutes: i32) -> ReservedMinutesPlan {
        ReservedMinutesPlan {
            monthly_allocation: Some(minutes),
            by_month: std::collections::HashMap::new(),
        }
    }

    /// A plan covering only `(year, month)`, with `minutes`.
    fn single_month(year: i32, month: i32, minutes: i32) -> ReservedMinutesPlan {
        let mut plan = ReservedMinutesPlan::default();
        plan.by_month.insert((year, month), minutes);
        plan
    }

    // Rows differ along whether a plan covers the month at all, how its allocation
    // compares with the 90 minutes scheduled (above it, equal to it, below it, zero) and
    // whether the covering allocation is the standing one or one month's own.
    let rows: Vec<(&str, ReservedMinutesPlan, bool, i64, i64)> = vec![
        ("no-plan-at-all", ReservedMinutesPlan::default(), false, 0, 0),
        ("standing-plan-above-usage", standing(500), true, 500, 410),
        ("standing-plan-equal-to-usage", standing(90), true, 90, 0),
        ("standing-plan-below-usage", standing(30), true, 30, 0),
        ("standing-plan-of-zero-minutes", standing(0), true, 0, 0),
        (
            "this-months-own-allocation",
            single_month(year, month, 200),
            true,
            200,
            110,
        ),
        (
            "another-months-allocation-only",
            single_month(year, month + 1, 200),
            false,
            0,
            0,
        ),
        (
            "this-month-overrides-the-standing-plan",
            ReservedMinutesPlan {
                monthly_allocation: Some(500),
                by_month: single_month(year, month, 120).by_month,
            },
            true,
            120,
            30,
        ),
    ];

    for (case, plan, expected_customer, expected_allocation, expected_remaining) in rows {
        {
            let state = service_state(&*engine).expect("this service's state store");
            state.lock().reserved_minutes = plan;
        }
        let reply = minute_usage(&router, (year, month)).await;
        assert_eq!(
            reply.field("isReservedMinutesCustomer").as_bool(),
            Some(expected_customer),
            "case {case}: expected isReservedMinutesCustomer {expected_customer}, got {}",
            reply.body
        );
        assert_eq!(
            minutes(&reply, "totalReservedMinuteAllocation", case),
            expected_allocation,
            "case {case}: expected an allocation of {expected_allocation}, got {}",
            reply.body
        );
        assert_eq!(
            minutes(&reply, "totalScheduledMinutes", case),
            scheduled,
            "case {case}: the month's scheduled minutes do not depend on the plan, got {}",
            reply.body
        );
        assert_eq!(
            minutes(&reply, "estimatedMinutesRemaining", case),
            expected_remaining,
            "case {case}: expected {expected_remaining} minutes remaining, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_operator_setting_reads_a_plan_month_by_month() {
    // Rows differ along the form of the setting: a standing commitment, one month's own
    // allocation, both together, surrounding whitespace, and entries that are not a
    // non-negative minute count keyed by a calendar month — each of which commits
    // nothing rather than something unintended.
    let rows: Vec<(&str, &str, Option<i32>, Option<i32>)> = vec![
        ("standing-commitment", "500", Some(500), Some(500)),
        ("one-month-only", "2026-02=750", Some(750), None),
        ("both", "500,2026-02=750", Some(750), Some(500)),
        ("whitespace-and-zero", "  600 , 2026-2 = 0 ", Some(0), Some(600)),
        ("unset", "", None, None),
        ("not-a-number", "many", None, None),
        ("negative-standing", "-5", None, None),
        ("negative-month", "2026-02=-5", None, None),
        ("month-out-of-range", "2026-13=10", None, None),
        ("no-month-at-all", "2026=10", None, None),
    ];

    for (case, setting, expected_february, expected_march) in rows {
        let plan = parse_reserved_minutes(setting);
        assert_eq!(
            plan.allocation_for(2026, 2),
            expected_february,
            "case {case}: February 2026 should be allocated {expected_february:?}, got {:?}",
            plan.allocation_for(2026, 2)
        );
        assert_eq!(
            plan.allocation_for(2026, 3),
            expected_march,
            "case {case}: March 2026 should be allocated {expected_march:?}, got {:?}",
            plan.allocation_for(2026, 3)
        );
    }
}
