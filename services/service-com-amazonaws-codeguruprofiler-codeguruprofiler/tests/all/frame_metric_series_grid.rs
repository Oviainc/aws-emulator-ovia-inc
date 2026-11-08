// SPDX-License-Identifier: CC0-1.0

//! The GRID of a `BatchGetFrameMetricData` series — how many time steps it holds, and
//! which of them carry a value — end to end over HTTP through the router this service
//! mounts in production.
//!
//! The two rules every case here asserts:
//!
//! * A series holds exactly as many time steps as the requested period holds WHOLE steps
//!   of the reported resolution — period ÷ resolution, the count `endTimes`' own doc
//!   fixes: "if the period is one day (PT24H), and the resolution is five minutes (PT5M),
//!   then there are 288 endTimes in the list that are each five minutes apart". The steps
//!   are the aggregation windows of that resolution aligned to UTC, from the one holding
//!   the period's start, so a period whose bounds are not themselves window-aligned
//!   carries no extra partial step at the tail, and a period shorter than one step holds
//!   no whole step at all.
//! * A step the group holds no metric data for keeps its position in
//!   `frameMetricData[].values` — the list is read index by index against `endTimes` — and
//!   carries NO value there. `AggregatedRelativeTotalTime` is a ratio "relative to the
//!   root frame", so a step whose profiles record no count at all in the thread states the
//!   metric asks about has nothing to be relative to; a step that DOES record counts
//!   reports the ratio, which is zero when none of them sit under the named frame.
//!
//! The emulator, the HTTP plumbing and the response reducers are the shared
//! [`crate::profiling_group_lifecycle`] and [`crate::frame_metric_data`] harnesses, not a
//! fork of them, and a profile is placed at a chosen instant through the one shared
//! [`crate::profile_times::place_receipts`].

use crate::frame_metric_data::{
    assert_value, datum, end_times, frame_metric, frame_metric_data, iso, profile_of,
};
use crate::interval_validation::create_group;
use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::Harness;

/// 2020-06-01T00:00:00Z. A whole number of days past the epoch, so an aggregation window
/// aligned to UTC starts here for every one of the three resolutions, and long enough ago
/// that the aggregation every window owes has fallen due.
const DAY_START: i64 = 1_590_969_600;

/// One day in seconds.
const DAY: i64 = 24 * 60 * 60;

/// One hour in seconds.
const HOUR: i64 = 60 * 60;

/// Five minutes in seconds — the service's finest aggregation period, and so the finest
/// resolution a series is reported at.
const FIVE_MINUTES: i64 = 5 * 60;

/// How the period of a request is named: a request names it with 1 or 2 of `startTime`,
/// `endTime` and `period`, and the count of steps is about the period itself rather than
/// about which members named it.
#[derive(Debug, Clone, Copy)]
enum Naming {
    /// `startTime` and `endTime`.
    Bounds,
    /// `startTime` and `period`.
    StartAndLength,
    /// `endTime` and `period`.
    EndAndLength,
}

/// One row of the step-count table: the resolution asked for, where the period sits
/// relative to an aligned window of that resolution, how long it is, and how it is named.
struct GridCase {
    label: &'static str,
    /// `targetResolution` as sent, or `None` to send none — the series is then reported at
    /// the service's finest aggregation period.
    requested: Option<&'static str>,
    /// The reported resolution's length in seconds.
    step: i64,
    /// How far past an aligned window of that resolution the period starts. Always shorter
    /// than one step, so the window holding the start is the one beginning at
    /// [`DAY_START`].
    offset: i64,
    /// The period's length in seconds.
    length: i64,
    naming: Naming,
}

/// The step-count table. Rows differ along what the rule generalises over: the resolution
/// (each of the three the model's `AggregationPeriod` declares, and none asked for), how
/// far the period's bounds sit from an aligned window (on it, a little past it, nearly a
/// whole step past it), how long the period is (one step, several, the longest the
/// operation reports over, a length that is not a whole number of steps, and a length
/// shorter than one step), and which members named the period.
fn grid_cases() -> Vec<GridCase> {
    vec![
        GridCase {
            label: "five-minute steps over an aligned hour",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: 0,
            length: HOUR,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "five-minute steps over an hour whose bounds are not window-aligned",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: 100,
            length: HOUR,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "five-minute steps over an hour starting one second before a window",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: FIVE_MINUTES - 1,
            length: HOUR,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "five-minute steps over exactly one step, unaligned",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: 120,
            length: FIVE_MINUTES,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "five-minute steps over the longest period, unaligned",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: 61,
            length: DAY,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "hourly steps over an unaligned day",
            requested: Some("PT1H"),
            step: HOUR,
            offset: 1_234,
            length: DAY,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "daily steps over an unaligned day",
            requested: Some("P1D"),
            step: DAY,
            offset: 3_601,
            length: DAY,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "no resolution asked for steps by five minutes",
            requested: None,
            step: FIVE_MINUTES,
            offset: 7,
            length: 2 * HOUR,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "a period that is not a whole number of steps holds the whole ones",
            requested: Some("PT1H"),
            step: HOUR,
            offset: 0,
            length: 3 * HOUR + 10 * 60,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "a period shorter than one step holds no whole step",
            requested: Some("PT1H"),
            step: HOUR,
            offset: 0,
            length: 10 * 60,
            naming: Naming::Bounds,
        },
        GridCase {
            label: "a period named by its start and its length",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: 42,
            length: HOUR,
            naming: Naming::StartAndLength,
        },
        GridCase {
            label: "a period named by its end and its length",
            requested: Some("PT5M"),
            step: FIVE_MINUTES,
            offset: 42,
            length: HOUR,
            naming: Naming::EndAndLength,
        },
    ]
}

/// The query one row sends: the period as the row names it, and the resolution it asks
/// for. A length is spelled as the ISO 8601 duration the modelled `period` carries.
fn query_of(case: &GridCase) -> String {
    let start = DAY_START + case.offset;
    let end = start + case.length;
    let mut query = match case.naming {
        Naming::Bounds => format!("?startTime={}&endTime={}", iso(start), iso(end)),
        Naming::StartAndLength => {
            format!("?startTime={}&period=PT{}S", iso(start), case.length)
        }
        Naming::EndAndLength => format!("?endTime={}&period=PT{}S", iso(end), case.length),
    };
    if let Some(requested) = case.requested {
        query.push_str(&format!("&targetResolution={requested}"));
    }
    query
}

/// A group holding one profile, received inside the window that starts at [`DAY_START`],
/// recording four `RUNNABLE` samples under frame `A` and one under `B`.
async fn group_with_a_profile(harness: &Harness, name: &str, received_at: i64) {
    create_group(harness, name).await;
    let body = serde_json::to_vec(&profile_of(
        serde_json::json!({ "A": { "RUNNABLE": 4 }, "B": { "RUNNABLE": 1 } }),
    ))
    .expect("a serializable document");
    let posted = harness
        .post_agent_profile(name, "application/json", None, body)
        .await;
    assert_eq!(
        posted.status,
        204,
        "posting a profile to {name} must succeed, got {} {}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    place_receipts(harness, name, &[received_at]);
}

#[tokio::test]
async fn a_series_holds_the_whole_steps_the_period_holds() {
    let harness = Harness::new();

    for (index, case) in grid_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("series-grid-{index}");
        group_with_a_profile(&harness, &name, DAY_START + 30).await;

        let answer = frame_metric_data(
            &harness,
            &name,
            &query_of(&case),
            &[frame_metric("A", &["RUNNABLE"])],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // period ÷ resolution steps, each one resolution long, from the window holding the
        // period's start — which is the one beginning at `DAY_START`, every row's offset
        // being shorter than its step.
        let expected: Vec<i64> = (1..=case.length / case.step)
            .map(|step| DAY_START + step * case.step)
            .collect();
        let reported = end_times(&answer);
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {label:?}: expected {} steps for a {}s period at {}s steps, got {}: \
             {reported:?}",
            expected.len(),
            case.length,
            case.step,
            reported.len()
        );
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the aligned windows {expected:?}, got {reported:?}"
        );

        // The values are read index by index against the end times, so there is exactly
        // one position per reported step.
        let (_, values) = datum(&answer, 0);
        assert_eq!(
            values.len(),
            reported.len(),
            "case {label:?}: expected one position per reported step, got {} for {}: \
             {values:?}",
            values.len(),
            reported.len()
        );
    }
}

/// One row of the alignment table: which step of a multi-step series the group's one
/// profile sits in, the metric asked about it, and the value that step must carry.
struct AlignmentCase {
    label: &'static str,
    step_index: usize,
    frame_name: &'static str,
    thread_states: &'static [&'static str],
    /// The value the step holding the profile must carry, or `None` when that step holds
    /// no metric data for this metric either.
    expected: Option<f64>,
}

/// The alignment table. Rows differ along which step of the series holds the profile (the
/// first, one in the middle, the last) and along whether the metric has metric data there
/// at all: a frame the profile does not hold is still a step WITH data (the ratio is
/// zero), while a thread state the profile never recorded leaves the step with none.
fn alignment_cases() -> Vec<AlignmentCase> {
    vec![
        AlignmentCase {
            label: "the profile sits in the first step",
            step_index: 0,
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: Some(0.8),
        },
        AlignmentCase {
            label: "the profile sits in a middle step",
            step_index: 5,
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: Some(0.8),
        },
        AlignmentCase {
            label: "the profile sits in the last step",
            step_index: 11,
            frame_name: "A",
            thread_states: &["RUNNABLE"],
            expected: Some(0.8),
        },
        AlignmentCase {
            label: "a frame the profile does not hold is a counted step with a zero ratio",
            step_index: 5,
            frame_name: "NotInTheProfile",
            thread_states: &["RUNNABLE"],
            expected: Some(0.0),
        },
        AlignmentCase {
            label: "a thread state the profile never recorded leaves every step without data",
            step_index: 5,
            frame_name: "A",
            thread_states: &["TERMINATED"],
            expected: None,
        },
    ]
}

#[tokio::test]
async fn only_a_step_with_metric_data_carries_a_value() {
    let harness = Harness::new();
    // An hour at the finest resolution, starting part-way through a window: twelve steps,
    // the first of them the window `DAY_START` begins.
    let start = DAY_START + 100;
    let steps = usize::try_from(HOUR / FIVE_MINUTES).expect("twelve steps");

    for (index, case) in alignment_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("series-alignment-{index}");
        let received_at =
            DAY_START + i64::try_from(case.step_index).expect("a step index") * FIVE_MINUTES + 30;
        group_with_a_profile(&harness, &name, received_at).await;

        let requested = frame_metric(case.frame_name, case.thread_states);
        let answer = frame_metric_data(
            &harness,
            &name,
            &format!(
                "?startTime={}&endTime={}&targetResolution=PT5M",
                iso(start),
                iso(start + HOUR)
            ),
            std::slice::from_ref(&requested),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        let reported = end_times(&answer);
        assert_eq!(
            reported.len(),
            steps,
            "case {label:?}: expected {steps} steps of the hour, got {}: {reported:?}",
            reported.len()
        );
        let (_, values) = datum(&answer, 0);
        assert_eq!(
            values.len(),
            steps,
            "case {label:?}: expected one position per step, got {}: {values:?}",
            values.len()
        );
        // The profile's own step carries the metric's value; every other step holds no
        // profile at all, so it keeps its position and carries no value.
        assert_eq!(
            reported.get(case.step_index).copied(),
            Some(DAY_START + i64::try_from(case.step_index + 1).expect("a step index") * FIVE_MINUTES),
            "case {label:?}: the profile's step must be the window holding its receipt, \
             got {reported:?}"
        );
        for step in 0..steps {
            let expected = if step == case.step_index {
                case.expected
            } else {
                None
            };
            assert_value(&values, step, expected, label);
        }
    }
}
