// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAccountOverview`.
//!
//! The harness (`Emulator`, `Reply` and the insight seeder) is the one
//! `configuration_lifecycle` already provides: one buffered HTTP request through the same
//! [`smithy_server_runtime::MultiServiceRouter`] the emulator boots, so every assertion is
//! about wire behaviour.
//!
//! No modelled operation MINTS an insight — DevOps Guru derives one from anomalies in the
//! covered resources — so a test that needs insights seeds the store's `insights` map the
//! way that analysis would and then asks the service over HTTP.
//!
//! RULES under test:
//!
//! 1. The overview counts the account's OPEN insights, by `InsightType`, among those
//!    CREATED inside the requested range.
//! 2. `MeanTimeToRecoverInMilliseconds` is the mean, over the CLOSED insights created
//!    inside that same range, of how long each took to recover — its time range's end
//!    minus its start — and is zero while the range holds no closed insight that records
//!    an end.
//! 3. The range's two bounds are DAYS: each is floored to the start of its UTC day and
//!    the `ToTime` day counts in full, with an absent `ToTime` meaning the current day.
//! 4. The overview is a pure read: it leaves the store exactly as it was.
//! 5. `FromTime` is modelled required, so a request without it is the modelled HTTP 400
//!    `ValidationException` — never the scaffold's 501.

use serde_json::{json, Value};

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    now_millis, InsightRecord, MILLIS_PER_DAY,
};

use crate::configuration_lifecycle::{Emulator, Reply};

/// `DescribeAccountOverview`'s HTTP binding, from
/// `smithy_query(query="operation",operation="DescribeAccountOverview")`:
/// `POST /accounts/overview` with every member in the body.
async fn describe_overview(emulator: &Emulator, body: &Value) -> Reply {
    emulator.call("POST", "/accounts/overview", Some(body)).await
}

/// One hour, in milliseconds — the step inside a day the rows are placed at.
const HOUR_MILLIS: i64 = 3_600_000;

/// A UTC midnight well inside the range a `Timestamp` can carry: 2023-11-14T00:00:00Z, in
/// epoch milliseconds. It is an exact multiple of [`MILLIS_PER_DAY`], so every row below
/// is placed on a known day boundary and no row depends on the clock.
const DAY_0: i64 = 1_699_920_000_000;

/// `InsightType`'s and `InsightStatus`'s values, from
/// `smithy_query(query="shape",shape="InsightType,InsightStatus")`.
const PROACTIVE: &str = "PROACTIVE";
const REACTIVE: &str = "REACTIVE";
const ONGOING: &str = "ONGOING";
const CLOSED: &str = "CLOSED";

/// A timestamp as restJson1 reads one: epoch SECONDS.
fn epoch_seconds(millis: i64) -> f64 {
    millis as f64 / 1_000.0
}

/// One seeded insight: the members these rows vary — type, status, the instant the
/// insight was created and how long its behaviour lasted — with the rest left at a fixed
/// value, so a difference in the reported numbers can only come from these.
///
/// `recovered_in` is the span between the insight's start and the end of its
/// `InsightTimeRange`; `None` leaves the insight without an end instant, which is what an
/// ongoing insight has and what a closed one whose end nothing recorded has too.
fn insight(
    id: &str,
    insight_type: &str,
    status: &str,
    started_millis: i64,
    recovered_in: Option<i64>,
) -> InsightRecord {
    InsightRecord {
        id: id.to_string(),
        name: format!("DevOps Guru insight {id}"),
        insight_type: insight_type.to_string(),
        severity: "HIGH".to_string(),
        status: status.to_string(),
        start_time_millis: started_millis,
        end_time_millis: recovered_in.map(|span| started_millis + span),
        ..InsightRecord::default()
    }
}

/// The insights every counting row is asserted against: both types, both statuses,
/// creation instants spread over a known grid of days and hours within them, and
/// recovery spans that differ from one another so a mean over any subset is distinctive.
///
/// Day 3 holds a CLOSED insight with no end instant, so no recovery time is accrued for
/// it; day 4 holds nothing at all.
fn fixture() -> Vec<InsightRecord> {
    vec![
        insight("p-day0-open", PROACTIVE, ONGOING, DAY_0 + HOUR_MILLIS, None),
        insight(
            "r-day0-closed-2h",
            REACTIVE,
            CLOSED,
            DAY_0 + 2 * HOUR_MILLIS,
            Some(2 * HOUR_MILLIS),
        ),
        insight(
            "r-day1-open",
            REACTIVE,
            ONGOING,
            DAY_0 + MILLIS_PER_DAY + 3 * HOUR_MILLIS,
            None,
        ),
        insight(
            "p-day1-closed-4h",
            PROACTIVE,
            CLOSED,
            DAY_0 + MILLIS_PER_DAY + 5 * HOUR_MILLIS,
            Some(4 * HOUR_MILLIS),
        ),
        insight(
            "p-day1-open-noon",
            PROACTIVE,
            ONGOING,
            DAY_0 + MILLIS_PER_DAY + 12 * HOUR_MILLIS,
            None,
        ),
        insight(
            "r-day2-open",
            REACTIVE,
            ONGOING,
            DAY_0 + 2 * MILLIS_PER_DAY + 2 * HOUR_MILLIS,
            None,
        ),
        insight(
            "r-day2-closed-6h",
            REACTIVE,
            CLOSED,
            DAY_0 + 2 * MILLIS_PER_DAY + 23 * HOUR_MILLIS,
            Some(6 * HOUR_MILLIS),
        ),
        insight(
            "r-day3-closed-no-end",
            REACTIVE,
            CLOSED,
            DAY_0 + 3 * MILLIS_PER_DAY + HOUR_MILLIS,
            None,
        ),
        insight(
            "p-day5-open-midnight",
            PROACTIVE,
            ONGOING,
            DAY_0 + 5 * MILLIS_PER_DAY,
            None,
        ),
    ]
}

/// The three numbers one reply reports, as a client reads them. All three members are
/// modelled required, so an absent one is a failure rather than a zero.
fn overview(reply: &Reply) -> (Option<i64>, Option<i64>, Option<i64>) {
    (
        reply.at("/ProactiveInsights").and_then(Value::as_i64),
        reply.at("/ReactiveInsights").and_then(Value::as_i64),
        reply
            .at("/MeanTimeToRecoverInMilliseconds")
            .and_then(Value::as_i64),
    )
}

/// An emulator holding the fixture insights.
fn seeded_emulator() -> Emulator {
    let emulator = Emulator::new();
    emulator.seed_insights(fixture());
    emulator
}

/// One row of the window table: the case, `FromTime`, the optional `ToTime`, and the open
/// proactive count, open reactive count and MTTR expected.
type WindowCase<'a> = (&'a str, i64, Option<i64>, i64, i64, i64);

/// RULES 1, 2 and 3: the overview reports the OPEN insights created inside the requested
/// range by type, and the mean recovery time over the CLOSED ones created inside it, over
/// a range of whole UTC days.
///
/// The rows differ along which days the range spans, how many days that is, where inside
/// its day each bound falls, which types and statuses the range catches, how many closed
/// insights it holds and whether the range holds any instant at all.
#[tokio::test]
async fn the_overview_counts_the_insights_created_in_the_requested_day_range() {
    let cases: Vec<WindowCase> = vec![
        // Day 0: one open proactive insight, and one closed reactive one that recovered
        // in two hours.
        (
            "the from day alone",
            DAY_0,
            Some(DAY_0),
            1,
            0,
            2 * HOUR_MILLIS,
        ),
        (
            "a FromTime late in its day still starts at that day's floor",
            DAY_0 + 23 * HOUR_MILLIS,
            Some(DAY_0 + 23 * HOUR_MILLIS),
            1,
            0,
            2 * HOUR_MILLIS,
        ),
        // Day 1: one open insight of each type; the closed one recovered in four hours
        // and is not counted as open.
        (
            "the second day alone",
            DAY_0 + MILLIS_PER_DAY,
            Some(DAY_0 + MILLIS_PER_DAY),
            1,
            1,
            4 * HOUR_MILLIS,
        ),
        // Day 2: an open reactive insight early in the day and a closed one created in
        // its last hour, which a `ToTime` early in that day still counts.
        (
            "a ToTime early in its day still counts that whole day",
            DAY_0 + 2 * MILLIS_PER_DAY,
            Some(DAY_0 + 2 * MILLIS_PER_DAY + 3 * HOUR_MILLIS),
            0,
            1,
            6 * HOUR_MILLIS,
        ),
        // Three days: the mean over 2h, 4h and 6h is 4h.
        (
            "three days at once",
            DAY_0,
            Some(DAY_0 + 2 * MILLIS_PER_DAY),
            2,
            2,
            4 * HOUR_MILLIS,
        ),
        // Every seeded day. Day 3's closed insight records no end, so it accrues no
        // recovery time and the mean is still over the other three.
        (
            "every seeded day",
            DAY_0,
            Some(DAY_0 + 5 * MILLIS_PER_DAY),
            3,
            2,
            4 * HOUR_MILLIS,
        ),
        // RULE 2's zero: a closed insight with no end instant contributes nothing, so a
        // range holding only it has accrued no recovery time.
        (
            "a day whose only insight is closed without an end instant",
            DAY_0 + 3 * MILLIS_PER_DAY,
            Some(DAY_0 + 3 * MILLIS_PER_DAY),
            0,
            0,
            0,
        ),
        (
            "a day nothing was created on",
            DAY_0 + 4 * MILLIS_PER_DAY,
            Some(DAY_0 + 4 * MILLIS_PER_DAY),
            0,
            0,
            0,
        ),
        (
            "a range entirely before the insights",
            DAY_0 - 10 * MILLIS_PER_DAY,
            Some(DAY_0 - MILLIS_PER_DAY),
            0,
            0,
            0,
        ),
        (
            "a range whose end precedes its start holds no instant",
            DAY_0 + 2 * MILLIS_PER_DAY,
            Some(DAY_0),
            0,
            0,
            0,
        ),
        (
            "an instant before the epoch is a day like any other",
            -3 * MILLIS_PER_DAY,
            Some(-2 * MILLIS_PER_DAY),
            0,
            0,
            0,
        ),
    ];

    for (case, from, to, proactive, reactive, mttr) in cases {
        let emulator = seeded_emulator();
        let mut body = json!({ "FromTime": epoch_seconds(from) });
        if let Some(to) = to {
            body["ToTime"] = json!(epoch_seconds(to));
        }

        let reply = describe_overview(&emulator, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the overview must succeed, got {} {}",
            reply.status, reply.raw
        );
        assert_eq!(
            overview(&reply),
            (Some(proactive), Some(reactive), Some(mttr)),
            "case {case:?}: expected ({proactive}, {reactive}, {mttr}) proactive/reactive/MTTR, got {}",
            reply.raw
        );
    }
}

/// RULE 2 as an INVARIANT: the mean recovery time of a set of closed insights lies
/// between the shortest and the longest span in it, equals that span when the set holds
/// one, and is the whole-millisecond mean of the set.
///
/// The rows differ along how many closed insights the range holds, how long each took and
/// whether their total divides by their count.
#[tokio::test]
async fn the_mean_time_to_recover_is_the_mean_over_the_closed_insights_of_the_range() {
    // (case, the recovery spans seeded, the mean expected)
    let cases: Vec<(&str, Vec<i64>, i64)> = vec![
        ("one closed insight is its own mean", vec![HOUR_MILLIS], HOUR_MILLIS),
        ("a span of zero", vec![0], 0),
        ("one millisecond", vec![1], 1),
        ("two spans that divide evenly", vec![HOUR_MILLIS, 3 * HOUR_MILLIS], 2 * HOUR_MILLIS),
        // 1 + 2 over two insights is 1.5 ms, which a whole-millisecond mean reports as 1.
        ("a mean that is not a whole millisecond", vec![1, 2], 1),
        ("three spans", vec![1_000, 2_000, 6_000], 3_000),
        (
            "a very long span alongside a very short one",
            vec![10, 30 * 24 * HOUR_MILLIS],
            (10 + 30 * 24 * HOUR_MILLIS) / 2,
        ),
    ];

    for (case, spans, expected) in cases {
        let emulator = Emulator::new();
        let seeded: Vec<InsightRecord> = spans
            .iter()
            .enumerate()
            .map(|(index, span)| {
                insight(
                    &format!("closed-{index}"),
                    if index % 2 == 0 { REACTIVE } else { PROACTIVE },
                    CLOSED,
                    DAY_0 + index as i64 * HOUR_MILLIS,
                    Some(*span),
                )
            })
            .collect();
        emulator.seed_insights(seeded);

        let reply = describe_overview(
            &emulator,
            &json!({ "FromTime": epoch_seconds(DAY_0), "ToTime": epoch_seconds(DAY_0) }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the overview must succeed, got {} {}",
            reply.status, reply.raw
        );
        let reported = overview(&reply);
        assert_eq!(
            reported,
            (Some(0), Some(0), Some(expected)),
            "case {case:?}: expected an MTTR of {expected} over closed insights alone, got {}",
            reply.raw
        );
        let mttr = reported.2.unwrap_or_default();
        let shortest = spans.iter().copied().min().unwrap_or_default();
        let longest = spans.iter().copied().max().unwrap_or_default();
        assert!(
            mttr >= shortest && mttr <= longest,
            "case {case:?}: expected a mean within [{shortest}, {longest}], got {mttr}"
        );
    }
}

/// RULE 3's default: an absent `ToTime` means "the current day", so an insight created
/// moments ago is counted, while the same request bounded two days back ends at that day
/// and counts nothing.
#[tokio::test]
async fn an_absent_to_time_means_the_current_day() {
    let emulator = Emulator::new();
    let created = now_millis() - HOUR_MILLIS;
    emulator.seed_insights(vec![
        insight("open-now", PROACTIVE, ONGOING, created, None),
        insight(
            "closed-now",
            REACTIVE,
            CLOSED,
            created,
            Some(10 * 60 * 1_000),
        ),
    ]);
    let from = now_millis() - 2 * MILLIS_PER_DAY;

    let open_ended =
        describe_overview(&emulator, &json!({ "FromTime": epoch_seconds(from) })).await;
    assert_eq!(
        open_ended.status, 200,
        "case \"no ToTime\": the overview must succeed, got {} {}",
        open_ended.status, open_ended.raw
    );
    assert_eq!(
        overview(&open_ended),
        (Some(1), Some(0), Some(10 * 60 * 1_000)),
        "case \"no ToTime\": today's insights must be counted, got {}",
        open_ended.raw
    );

    let bounded = describe_overview(
        &emulator,
        &json!({ "FromTime": epoch_seconds(from), "ToTime": epoch_seconds(from) }),
    )
    .await;
    assert_eq!(
        overview(&bounded),
        (Some(0), Some(0), Some(0)),
        "case \"ToTime two days back\": an insight created today is outside that day, got {}",
        bounded.raw
    );
}

/// RULE 4: the overview is a pure read, so reading it changes nothing a later caller
/// sees: repeating the same request answers the same numbers, a read of one range does not
/// disturb another's, and the insights themselves still list unchanged afterwards.
#[tokio::test]
async fn reading_the_overview_changes_nothing_a_later_caller_sees() {
    let emulator = seeded_emulator();
    let whole_span = json!({
        "FromTime": epoch_seconds(DAY_0),
        "ToTime": epoch_seconds(DAY_0 + 5 * MILLIS_PER_DAY),
    });
    let one_day = json!({
        "FromTime": epoch_seconds(DAY_0 + MILLIS_PER_DAY),
        "ToTime": epoch_seconds(DAY_0 + MILLIS_PER_DAY),
    });

    let first = describe_overview(&emulator, &whole_span).await;
    let narrow = describe_overview(&emulator, &one_day).await;
    let again = describe_overview(&emulator, &whole_span).await;

    assert_eq!(
        overview(&first),
        (Some(3), Some(2), Some(4 * HOUR_MILLIS)),
        "case \"first read\": expected (3, 2, {}), got {}",
        4 * HOUR_MILLIS,
        first.raw
    );
    assert_eq!(
        overview(&narrow),
        (Some(1), Some(1), Some(4 * HOUR_MILLIS)),
        "case \"narrower range\": expected (1, 1, {}), got {}",
        4 * HOUR_MILLIS,
        narrow.raw
    );
    assert_eq!(
        overview(&again),
        overview(&first),
        "case \"repeated read\": expected {:?}, got {}",
        overview(&first),
        again.raw
    );

    // The account-level health read of the same store, which tallies the SAME insight map
    // without a window: every seeded insight is still there, so no status was moved and no
    // count was consumed by the reads above.
    let health = emulator.call("GET", "/accounts/health", None).await;
    assert_eq!(
        health.status, 200,
        "case \"health afterwards\": the health read must succeed, got {} {}",
        health.status, health.raw
    );
    assert_eq!(
        (
            health.at("/OpenProactiveInsights").and_then(Value::as_i64),
            health.at("/OpenReactiveInsights").and_then(Value::as_i64),
        ),
        (Some(3), Some(2)),
        "case \"health afterwards\": the store must still hold every seeded open insight, got {}",
        health.raw
    );

    // `ListInsights` over the same store: every seeded proactive insight still lists.
    let listed = emulator
        .call(
            "POST",
            "/insights",
            Some(&json!({ "StatusFilter": { "Any": { "Type": PROACTIVE, "StartTimeRange": {} } } })),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"listing afterwards\": the listing must succeed, got {} {}",
        listed.status, listed.raw
    );
    let mut ids: Vec<String> = listed
        .at("/ProactiveInsights")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "p-day0-open".to_string(),
            "p-day1-closed-4h".to_string(),
            "p-day1-open-noon".to_string(),
            "p-day5-open-midnight".to_string(),
        ],
        "case \"listing afterwards\": every seeded proactive insight must still be stored, got {}",
        listed.raw
    );
}

/// One row of the extreme-bound table: the case, `FromTime` and the optional `ToTime` in
/// epoch seconds, and the numbers expected if the bound is accepted.
type ExtremeCase<'a> = (&'a str, f64, Option<f64>, (i64, i64, i64));

/// RULE: a bound at the extreme of what a `Timestamp` can carry is answered by the
/// modelled contract, never by a fault: the day floor SATURATES at the earliest and latest
/// instant this service keeps, so such a request is either a 200 whose range is the
/// saturated span or the modelled 400 `ValidationException` — never a 5xx and never the
/// scaffold's 501.
///
/// The rows differ along which bound is extreme and in which direction.
#[tokio::test]
async fn a_time_bound_at_the_extreme_is_answered_by_the_modelled_contract() {
    // `i64::MIN` milliseconds is about -9_223_372_036_854_775 epoch seconds, so these
    // bounds sit just inside the representable range and their day floors are what would
    // overflow an unchecked multiplication.
    let cases: [ExtremeCase; 4] = [
        // The range then starts at the earliest instant and ends at the end of today, so
        // every seeded insight was created inside it.
        (
            "a FromTime at the negative extreme",
            -9_223_372_036_850_000.0,
            None,
            (3, 2, 4 * HOUR_MILLIS),
        ),
        // Both bounds floor to the same earliest day, so the range is that one day.
        (
            "both bounds at the negative extreme",
            -9_223_372_036_850_000.0,
            Some(-9_223_372_036_850_000.0),
            (0, 0, 0),
        ),
        // A range that starts after the last day holds no instant.
        (
            "a FromTime at the positive extreme",
            9_223_372_036_850_000.0,
            None,
            (0, 0, 0),
        ),
        (
            "a ToTime at the positive extreme spans everything from the from day",
            epoch_seconds(DAY_0),
            Some(9_223_372_036_850_000.0),
            (3, 2, 4 * HOUR_MILLIS),
        ),
    ];

    let mut accepted = 0usize;
    for (case, from, to, (proactive, reactive, mttr)) in cases {
        let emulator = seeded_emulator();
        let mut body = json!({ "FromTime": from });
        if let Some(to) = to {
            body["ToTime"] = json!(to);
        }

        let reply = describe_overview(&emulator, &body).await;
        assert!(
            reply.status == 200 || (reply.status == 400 && reply.names_error("ValidationException")),
            "case {case:?}: expected HTTP 200 or the modelled 400 ValidationException, got {} {}",
            reply.status,
            reply.raw
        );
        if reply.status == 200 {
            accepted += 1;
            assert_eq!(
                overview(&reply),
                (Some(proactive), Some(reactive), Some(mttr)),
                "case {case:?}: expected ({proactive}, {reactive}, {mttr}) over the saturated range, got {}",
                reply.raw
            );
        }
    }

    // The saturating floor is what makes an extreme bound answerable at all, so at least
    // one row must have been ANSWERED rather than refused: otherwise the rows above would
    // assert nothing about it.
    assert!(
        accepted > 0,
        "at least one extreme bound must be answered with a computed overview, none was"
    );
}

/// RULE 5: `FromTime` is modelled required, so a request without it is refused with the
/// operation's HTTP 400 `ValidationException` — never the scaffold's 501 and never
/// numbers over some substituted range.
#[tokio::test]
async fn a_request_without_the_required_from_time_is_refused() {
    for (case, body) in [
        ("no member at all", json!({})),
        (
            "a ToTime but no FromTime",
            json!({ "ToTime": epoch_seconds(DAY_0) }),
        ),
    ] {
        let emulator = seeded_emulator();
        let reply = describe_overview(&emulator, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a missing required FromTime, got {} {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            reply.raw
        );
        assert_eq!(
            overview(&reply),
            (None, None, None),
            "case {case:?}: a refusal must carry no numbers, got {}",
            reply.raw
        );
    }
}
