// SPDX-License-Identifier: CC0-1.0

//! How this service COMPARES stored instants, end to end over HTTP through the router
//! it mounts in production.
//!
//! The rule every case here asserts: every ordering and inclusive-bound test this
//! service makes on a stored timestamp is exact at the full resolution a stored instant
//! carries — whole seconds AND sub-second nanoseconds — so an instant that lies outside
//! a requested closed interval by a fraction of a microsecond is outside it, and two
//! stored instants a nanosecond apart order by the earlier one. The modelled
//! `date-time` members carry sub-second precision on the wire, so a request can name
//! such a bound.
//!
//! Rows vary what the rule generalises over: which operation makes the comparison
//! (`GetProfile`'s window filter, `ListFindingsReports`' range filter and its stable
//! order), which bounds the request names (both, each alone), the sub-second part of the
//! stored instant (none, one nanosecond, the last nanosecond of a second, a half
//! second), the SIGN and size of the gap between the stored instant and the bound (one
//! nanosecond, hundreds of nanoseconds, a whole second), and whether that gap crosses a
//! second boundary.
//!
//! The emulator, the HTTP plumbing and the response reducer are the shared
//! [`crate::profiling_group_lifecycle`] harness, and receipts are placed through the one
//! shared placer in [`crate::profile_times`], groups are created through the one shared
//! creator in [`crate::interval_validation`] and a sub-second bound is rendered by the one
//! shared renderer in [`crate::interval_resolution`], not a fork of any of them.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ComAmazonawsCodeguruprofilerCodeguruprofilerState, FindingsReport, now,
};

use crate::interval_validation::create_group;
use crate::profile_times::place_receipts_at;
use crate::profiling_group_lifecycle::{Answer, Harness};

/// One day in seconds.
const DAY: i64 = 86_400;

/// One second in nanoseconds.
const SECOND_NANOS: i64 = 1_000_000_000;

/// The fixed text a 404 about absent profile DATA carries
/// (`state::NO_PROFILES_IN_TIME_RANGE`), restated here so this suite pins the wire text
/// rather than importing the value it compares against.
const NO_PROFILES_IN_TIME_RANGE: &str = "No profiles found for the requested time range!";

/// A day-aligned instant two days in the past, which every case places its stored
/// instants around.
///
/// It is read off the same clock the service stamps a receipt with, so a profile placed
/// here belongs to an aggregation window that closed long ago and is servable; and it is
/// aligned to a whole day, so it is a window start for each of the three modelled
/// aggregation periods.
fn base_secs() -> i64 {
    (now().secs() - 2 * DAY).div_euclid(DAY) * DAY
}

/// `at` (epoch seconds, sub-second nanoseconds) moved by `offset` nanoseconds, exactly —
/// so a case can name a bound one nanosecond from a stored instant, including across a
/// second boundary.
fn shifted(at: (i64, u32), offset: i64) -> (i64, u32) {
    let total = i128::from(at.0) * i128::from(SECOND_NANOS) + i128::from(at.1) + i128::from(offset);
    let secs = i64::try_from(total.div_euclid(i128::from(SECOND_NANOS)))
        .unwrap_or_else(|_| panic!("instant {at:?} shifted by {offset} ns is out of range"));
    let nanos = u32::try_from(total.rem_euclid(i128::from(SECOND_NANOS)))
        .expect("a sub-second remainder is below one second");
    (secs, nanos)
}

/// An instant as a modelled `date-time` query value, spelled to the nanosecond by the
/// one shared renderer in [`crate::interval_resolution`], so the bound the request names
/// keeps the nanoseconds the case chose.
fn iso_at(instant: (i64, u32)) -> String {
    crate::interval_resolution::at(instant.0, instant.1)
}

/// The instant the rendered query value really names, read back the way the service's
/// own deserializer reads it.
fn parsed(rendered: &str) -> (i64, u32) {
    let parsed = DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| panic!("unparseable date-time {rendered:?}: {error}"));
    (parsed.secs(), parsed.subsec_nanos())
}

/// One `GetProfile` window, as nanosecond offsets from the stored receipt time.
#[derive(Debug)]
struct WindowCase {
    label: &'static str,
    /// The sub-second part of the stored receipt time.
    receipt_nanos: u32,
    /// `startTime`, as nanoseconds from the receipt, or none to leave the bound out.
    start_offset: Option<i64>,
    /// `endTime`, likewise.
    end_offset: Option<i64>,
    /// Whether the receipt is inside the window the two bounds name, so the stored
    /// profile must be served.
    served: bool,
}

impl WindowCase {
    fn query(&self, receipt: (i64, u32)) -> String {
        let mut parameters: Vec<String> = Vec::new();
        if let Some(offset) = self.start_offset {
            parameters.push(format!("startTime={}", iso_at(shifted(receipt, offset))));
        }
        if let Some(offset) = self.end_offset {
            parameters.push(format!("endTime={}", iso_at(shifted(receipt, offset))));
        }
        if parameters.is_empty() {
            String::new()
        } else {
            format!("?{}", parameters.join("&"))
        }
    }
}

/// The windows the rule is asserted over. Each row is one receipt time and one window
/// around it; `served` is what the exact comparison says, and every row whose bound sits
/// a fraction of a microsecond from the receipt is one an `f64` of epoch seconds cannot
/// tell from the receipt itself.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            label: "an end bound one nanosecond before the receipt",
            receipt_nanos: 1_000,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(-1),
            served: false,
        },
        WindowCase {
            label: "an end bound exactly at the receipt",
            receipt_nanos: 1_000,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(0),
            served: true,
        },
        WindowCase {
            label: "an end bound one nanosecond after the receipt",
            receipt_nanos: 1_000,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(1),
            served: true,
        },
        WindowCase {
            label: "an end bound 240 nanoseconds before the receipt",
            receipt_nanos: 500_000,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(-240),
            served: false,
        },
        WindowCase {
            label: "a start bound one nanosecond after the receipt",
            receipt_nanos: 1_000,
            start_offset: Some(1),
            end_offset: Some(SECOND_NANOS),
            served: false,
        },
        WindowCase {
            label: "a start bound exactly at the receipt",
            receipt_nanos: 1_000,
            start_offset: Some(0),
            end_offset: Some(SECOND_NANOS),
            served: true,
        },
        WindowCase {
            label: "a start bound one nanosecond before the receipt",
            receipt_nanos: 1_000,
            start_offset: Some(-1),
            end_offset: Some(SECOND_NANOS),
            served: true,
        },
        WindowCase {
            label: "a start bound 240 nanoseconds after the receipt",
            receipt_nanos: 500_000,
            start_offset: Some(240),
            end_offset: Some(SECOND_NANOS),
            served: false,
        },
        WindowCase {
            label: "a window entirely inside the microsecond after the receipt",
            receipt_nanos: 123_456,
            start_offset: Some(1),
            end_offset: Some(999),
            served: false,
        },
        WindowCase {
            label: "a window entirely inside the microsecond before the receipt",
            receipt_nanos: 123_456,
            start_offset: Some(-999),
            end_offset: Some(-1),
            served: false,
        },
        WindowCase {
            label: "a nanosecond-wide window around the receipt",
            receipt_nanos: 500_000_000,
            start_offset: Some(-1),
            end_offset: Some(1),
            served: true,
        },
        WindowCase {
            label: "a whole-second window around a receipt on a second boundary",
            receipt_nanos: 0,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(SECOND_NANOS),
            served: true,
        },
        WindowCase {
            label: "an end bound one nanosecond before a receipt on a second boundary",
            receipt_nanos: 0,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(-1),
            served: false,
        },
        WindowCase {
            label: "a start bound one nanosecond after the last nanosecond of a second",
            receipt_nanos: 999_999_999,
            start_offset: Some(1),
            end_offset: Some(SECOND_NANOS),
            served: false,
        },
        WindowCase {
            label: "an end bound at the last nanosecond of a second",
            receipt_nanos: 999_999_999,
            start_offset: Some(-SECOND_NANOS),
            end_offset: Some(0),
            served: true,
        },
        WindowCase {
            label: "a start bound alone, one nanosecond after the receipt",
            receipt_nanos: 7,
            start_offset: Some(1),
            end_offset: None,
            served: false,
        },
        WindowCase {
            label: "a start bound alone, at the receipt",
            receipt_nanos: 7,
            start_offset: Some(0),
            end_offset: None,
            served: true,
        },
        WindowCase {
            label: "an end bound alone, one nanosecond before the receipt",
            receipt_nanos: 7,
            start_offset: None,
            end_offset: Some(-1),
            served: false,
        },
        WindowCase {
            label: "an end bound alone, at the receipt",
            receipt_nanos: 7,
            start_offset: None,
            end_offset: Some(0),
            served: true,
        },
        WindowCase {
            label: "no bound at all",
            receipt_nanos: 7,
            start_offset: None,
            end_offset: None,
            served: true,
        },
    ]
}

/// A precondition pin on this suite's own helpers, not on a handler: the rows below are
/// only about nanosecond-exact comparison if the bound a request names really carries
/// nanoseconds, so the rendering and the parse are checked first.
#[test]
fn a_requested_date_time_bound_keeps_its_nanoseconds_on_the_wire() {
    for nanos in [0u32, 1, 240, 999, 123_456, 500_000_000, 999_999_999] {
        let instant = (base_secs(), nanos);
        let rendered = iso_at(instant);
        assert_eq!(
            parsed(&rendered),
            instant,
            "case {nanos} ns: {rendered:?} must read back as the same instant, got {:?}",
            parsed(&rendered)
        );
    }
}

#[tokio::test]
async fn get_profile_window_bounds_are_exact_at_nanosecond_resolution() {
    let harness = Harness::new();
    let base = base_secs();

    for (index, case) in window_cases().iter().enumerate() {
        let label = case.label;
        let name = format!("instant-window-{index}");
        create_group(&harness, &name).await;
        let payload =
            format!("{{\"start\":1600000000000,\"profile\":\"{name}\"}}").into_bytes();
        assert_eq!(
            harness
                .post_agent_profile(&name, "application/json", None, payload.clone())
                .await
                .status,
            204,
            "case {label:?}: the post must succeed"
        );
        let receipt = (base, case.receipt_nanos);
        place_receipts_at(&harness, &name, &[receipt]);

        let query = case.query(receipt);
        let answer = harness.get_profile(&name, &query).await;
        let expected = if case.served { 200 } else { 404 };
        assert_eq!(
            answer.status,
            expected,
            "case {label:?}: receipt {receipt:?} with window {query} must answer {expected}, \
             got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if case.served {
            assert_eq!(
                answer.body, payload,
                "case {label:?}: the stored profile must be served byte for byte, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        } else {
            assert_eq!(
                answer.error_message(),
                NO_PROFILES_IN_TIME_RANGE,
                "case {label:?}: a receipt outside the window must answer the fixed no-data \
                 text, got {:?}",
                answer.error_message()
            );
        }
    }
}

/// A `FindingsReportId`: the modelled pattern is
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`.
const REPORT_ID: &str = "11111111-1111-4111-8111-111111111111";

/// One recorded findings report and one listing range, as nanosecond offsets from a
/// shared instant.
#[derive(Debug)]
struct OverlapCase {
    label: &'static str,
    /// The analysed profile window, as (start, end) nanoseconds from the shared instant.
    window: (i64, i64),
    /// The requested `[startTime, endTime]`, likewise.
    range: (i64, i64),
    /// Whether the window overlaps the range, so the report must be listed.
    listed: bool,
}

/// The ranges the overlap rule is asserted over: a window and a range that touch, miss
/// or contain each other by a fraction of a microsecond, plus the whole-second rows the
/// same rule covers.
fn overlap_cases() -> Vec<OverlapCase> {
    vec![
        OverlapCase {
            label: "a window ending one nanosecond before the range starts",
            window: (-SECOND_NANOS, -1),
            range: (0, SECOND_NANOS),
            listed: false,
        },
        OverlapCase {
            label: "a window ending exactly where the range starts",
            window: (-SECOND_NANOS, 0),
            range: (0, SECOND_NANOS),
            listed: true,
        },
        OverlapCase {
            label: "a window ending 240 nanoseconds before the range starts",
            window: (-SECOND_NANOS, -240),
            range: (0, SECOND_NANOS),
            listed: false,
        },
        OverlapCase {
            label: "a window starting one nanosecond after the range ends",
            window: (1, SECOND_NANOS),
            range: (-SECOND_NANOS, 0),
            listed: false,
        },
        OverlapCase {
            label: "a window starting exactly where the range ends",
            window: (0, SECOND_NANOS),
            range: (-SECOND_NANOS, 0),
            listed: true,
        },
        OverlapCase {
            label: "a window starting 240 nanoseconds after the range ends",
            window: (240, SECOND_NANOS),
            range: (-SECOND_NANOS, 0),
            listed: false,
        },
        OverlapCase {
            label: "a nanosecond-wide window inside the range",
            window: (1, 2),
            range: (0, SECOND_NANOS),
            listed: true,
        },
        OverlapCase {
            label: "a nanosecond-wide range inside the window",
            window: (-SECOND_NANOS, SECOND_NANOS),
            range: (-1, 1),
            listed: true,
        },
        OverlapCase {
            label: "a window a whole second before the range",
            window: (-2 * SECOND_NANOS, -SECOND_NANOS),
            range: (0, SECOND_NANOS),
            listed: false,
        },
    ]
}

/// The ids `ListFindingsReports` reports, in the order the response lists them.
fn report_ids(answer: &Answer) -> Vec<String> {
    answer
        .json()
        .get("findingsReportSummaries")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "no findingsReportSummaries in {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Record one findings report against a stored group, through the store's own writer —
/// this emulator runs no analysis of its own, so that writer is the only way one exists.
fn record_report(
    harness: &Harness,
    group: &str,
    id: &str,
    start: (i64, u32),
    end: (i64, u32),
    period: &str,
) {
    let state = harness
        .engine
        .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let mut guard = state.lock();
    assert!(
        guard.record_findings_report(
            group,
            FindingsReport {
                id: id.to_string(),
                profile_start_time: DateTime::from_secs_and_nanos(start.0, start.1),
                profile_end_time: DateTime::from_secs_and_nanos(end.0, end.1),
                total_number_of_findings: 1,
                aggregation_period: period.to_string(),
            },
        ),
        "recording analysis against {group} must find the group"
    );
}

async fn list_findings_reports(harness: &Harness, group: &str, query: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/internal/profilingGroups/{group}/findingsReports{query}"),
            "application/json",
            Vec::new(),
        )
        .await
}

#[tokio::test]
async fn findings_report_range_filter_is_exact_at_nanosecond_resolution() {
    let harness = Harness::new();
    // A half second in, so every row's offsets stay inside one second without landing on
    // a boundary by accident.
    let origin = (base_secs(), 500_000_000u32);

    for (index, case) in overlap_cases().iter().enumerate() {
        let label = case.label;
        let name = format!("instant-overlap-{index}");
        create_group(&harness, &name).await;
        record_report(
            &harness,
            &name,
            REPORT_ID,
            shifted(origin, case.window.0),
            shifted(origin, case.window.1),
            // The modelled `AggregationPeriod` value for a daily profile.
            "P1D",
        );

        let query = format!(
            "?startTime={}&endTime={}",
            iso_at(shifted(origin, case.range.0)),
            iso_at(shifted(origin, case.range.1))
        );
        let answer = list_findings_reports(&harness, &name, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the listing must answer 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let expected: Vec<String> = if case.listed {
            vec![REPORT_ID.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            report_ids(&answer),
            expected,
            "case {label:?}: window {:?} against range {:?} must list {expected:?}, got {:?}",
            case.window,
            case.range,
            report_ids(&answer)
        );
    }
}

#[tokio::test]
async fn findings_reports_order_by_analysed_window_at_nanosecond_resolution() {
    let harness = Harness::new();
    let name = "instant-order";
    create_group(&harness, name).await;
    let origin = (base_secs(), 500_000_000u32);

    // Three analysed windows a few hundred nanoseconds apart, whose ids sort in a
    // DIFFERENT order from their windows — so the order the listing reports is the
    // windows' and not the id tie-break's, which is what an `f64` of epoch seconds would
    // fall through to.
    let earliest = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let middle = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let latest = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    let windows = [(latest, 240_i64), (earliest, 0_i64), (middle, 1_i64)];
    for (id, offset) in windows {
        record_report(
            &harness,
            name,
            id,
            shifted(origin, offset),
            shifted(origin, offset + SECOND_NANOS),
            "P1D",
        );
    }

    let query = format!(
        "?startTime={}&endTime={}",
        iso_at(shifted(origin, -SECOND_NANOS)),
        iso_at(shifted(origin, 2 * SECOND_NANOS))
    );
    let answer = list_findings_reports(&harness, name, &query).await;
    assert_eq!(
        answer.status, 200,
        "the listing must answer 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let expected = vec![
        earliest.to_string(),
        middle.to_string(),
        latest.to_string(),
    ];
    assert_eq!(
        report_ids(&answer),
        expected,
        "reports whose windows are nanoseconds apart must be listed oldest window first, \
         got {:?}",
        report_ids(&answer)
    );
}
