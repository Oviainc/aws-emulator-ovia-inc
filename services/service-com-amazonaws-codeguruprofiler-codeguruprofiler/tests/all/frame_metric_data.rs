// SPDX-License-Identifier: CC0-1.0

//! `BatchGetFrameMetricData`, end to end over HTTP through the router this service mounts
//! in production.
//!
//! The rules these cases assert, each over a table of inputs rather than one request:
//!
//! * The time period reported is the one the request names with 1 or 2 of `startTime`,
//!   `endTime` and `period`, completed with the longest period the operation reports over
//!   (one day, which `period`'s doc fixes) where the request leaves a side open — and a
//!   period that is no interval, or longer than that maximum, is refused.
//! * The time steps are the aggregation windows of the reported resolution, aligned to
//!   UTC: one day at five-minute resolution is the 288 steps the `endTimes` doc names, and
//!   a step whose window holds a profile the service has not aggregated yet is reported as
//!   an unprocessed instance instead, leaving a missing time step.
//! * A frame metric's value for a step is the metric's own definition over the profiles
//!   stored for that step: for `AggregatedRelativeTotalTime` ("Metric value aggregated for
//!   all instances of a frame name in a profile relative to the root frame"), the sample
//!   counts recorded for every frame of that name, including the frames it calls, over the
//!   counts of the whole profile — both only in the thread states the metric asks about.
//!
//! The emulator, the HTTP plumbing and the response reducer are the shared
//! [`crate::profiling_group_lifecycle`] harness, and a profile is placed at a chosen
//! instant through the one shared [`crate::profile_times::place_receipts`].

use std::collections::BTreeMap;

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::profile_times::{place_profile, place_receipts};
use crate::profiling_group_lifecycle::{Answer, Harness};

/// 2020-06-01T00:00:00Z, the reference instant these cases place receipts around. It is a
/// whole number of days past the epoch, so an aggregation window aligned to UTC starts
/// here for every one of the three resolutions.
const DAY_START: i64 = 1_590_969_600;

/// One day in seconds.
const DAY: i64 = 24 * 60 * 60;

/// One hour in seconds.
const HOUR: i64 = 60 * 60;

/// Five minutes in seconds — the service's most granular aggregation period, and so the
/// finest resolution a series can be reported at.
const FIVE_MINUTES: i64 = 5 * 60;

/// How close two metric values have to be to count as the same ratio. The values are
/// doubles (the modelled `FrameMetricValue`), so a ratio such as 6/9 is compared with a
/// tolerance rather than bit for bit.
const TOLERANCE: f64 = 1e-9;

/// How many frame metrics one request may ask about
/// (`state::MAX_FRAME_METRICS_PER_REQUEST`), restated here so these cases pin the wire
/// behaviour at the bound rather than importing the value they compare against.
const MAX_FRAME_METRICS: usize = 100;

/// How many thread states one frame metric may name
/// (`state::MAX_THREAD_STATES_PER_METRIC`), restated here for the same reason.
const MAX_THREAD_STATES: usize = 32;

/// An instant as the ISO 8601 form the modelled `date-time` members use.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// An instant as this service renders one it NAMES IN A MESSAGE: the same ISO 8601 form,
/// carrying the fixed, zero-padded 3-digit millisecond field the model's own doc spells
/// that form with. The one shared [`crate::interval_validation::canonical`] states that
/// rendering for every suite in this target.
fn iso_in_message(at: i64) -> String {
    crate::interval_validation::canonical(&iso(at))
}

/// The emulator's own clock, as the test reads it: a case about a window that ends "now"
/// has to say which instant it expected.
fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_secs(),
    )
    .expect("the current instant fits in an i64")
}

/// One modelled timestamp read back as epoch seconds. restJson1 carries a `Timestamp`
/// either in the ISO 8601 form or as epoch seconds depending on the member's
/// `timestampFormat`, and these cases are about WHICH instant is reported, so both
/// renderings are read the same way.
fn instant(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| panic!("unparseable instant {text:?}: {error}"))
            .as_secs_f64() as i64,
        serde_json::Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("unparseable instant {seconds:?}")) as i64,
        other => panic!("neither a timestamp string nor epoch seconds: {other:?}"),
    }
}

/// The response's `startTime` and `endTime`, as epoch seconds.
fn reported_period(answer: &Answer) -> (i64, i64) {
    let json = answer.json();
    let start = json
        .get("startTime")
        .unwrap_or_else(|| panic!("no startTime in {json:?}"));
    let end = json
        .get("endTime")
        .unwrap_or_else(|| panic!("no endTime in {json:?}"));
    (instant(start), instant(end))
}

/// The response's `resolution`.
fn reported_resolution(answer: &Answer) -> String {
    let json = answer.json();
    json.get("resolution")
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| panic!("no resolution in {json:?}"))
        .to_string()
}

/// The response's `endTimes`, as epoch seconds in the order they are reported.
fn end_times(answer: &Answer) -> Vec<i64> {
    let json = answer.json();
    json.get("endTimes")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no endTimes array in {json:?}"))
        .iter()
        .map(|entry| {
            instant(
                entry
                    .get("value")
                    .unwrap_or_else(|| panic!("no value in end time {entry:?}")),
            )
        })
        .collect()
}

/// The response's `unprocessedEndTimes`, as epoch seconds per key.
fn unprocessed_end_times(answer: &Answer) -> BTreeMap<String, Vec<i64>> {
    let json = answer.json();
    json.get("unprocessedEndTimes")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("no unprocessedEndTimes map in {json:?}"))
        .iter()
        .map(|(key, times)| {
            (
                key.clone(),
                times
                    .as_array()
                    .unwrap_or_else(|| panic!("unprocessed instances are not a list: {times:?}"))
                    .iter()
                    .map(|entry| {
                        instant(
                            entry
                                .get("value")
                                .unwrap_or_else(|| panic!("no value in instance {entry:?}")),
                        )
                    })
                    .collect(),
            )
        })
        .collect()
}

/// The `values` of the datum at `index`, and the `frameMetric` it is reported for.
fn datum(answer: &Answer, index: usize) -> (serde_json::Value, Vec<f64>) {
    let json = answer.json();
    let data = json
        .get("frameMetricData")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no frameMetricData array in {json:?}"));
    let datum = data
        .get(index)
        .unwrap_or_else(|| panic!("no datum {index} in {json:?}"));
    let values = datum
        .get("values")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no values in datum {datum:?}"))
        .iter()
        .map(|value| {
            value
                .as_f64()
                .unwrap_or_else(|| panic!("a value that is no number: {value:?}"))
        })
        .collect();
    let metric = datum
        .get("frameMetric")
        .cloned()
        .unwrap_or_else(|| panic!("no frameMetric in datum {datum:?}"));
    (metric, values)
}

/// How many datums the response reports.
fn datum_count(answer: &Answer) -> usize {
    let json = answer.json();
    json.get("frameMetricData")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no frameMetricData array in {json:?}"))
        .len()
}

/// One requested frame metric, with the only aggregation the model's `MetricType`
/// declares (`AggregatedRelativeTotalTime`). `pub(crate)` so a sibling suite about a
/// refusal of this operation sends the same body rather than forking it.
pub(crate) fn frame_metric(frame_name: &str, thread_states: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "frameName": frame_name,
        "type": "AggregatedRelativeTotalTime",
        "threadStates": thread_states,
    })
}

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
}

/// `BatchGetFrameMetricData` with `frameMetrics` in the body, the way an SDK sends it:
/// the period and the resolution bind to the query string, the group to the path.
/// `pub(crate)` so a sibling suite drives this operation through the one sender.
pub(crate) async fn frame_metric_data(
    harness: &Harness,
    name: &str,
    query: &str,
    frame_metrics: &[serde_json::Value],
) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups/{name}/frames/-/metrics{query}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "frameMetrics": frame_metrics }))
                .expect("a serializable body"),
        )
        .await
}

/// A group holding one profile with `body` as its document, received at `at`.
///
/// `submittable` says whether `body` is a profile a caller could submit: `PostAgentProfile`
/// parses the data it is handed and refuses data that is not an agent profile, so a case
/// about what a READER tallies for bytes that are NO profile document places them in the
/// store directly ([`crate::profile_times::place_profile`]) instead of posting them.
async fn group_with_profile(
    harness: &Harness,
    name: &str,
    body: Vec<u8>,
    at: i64,
    submittable: bool,
) {
    assert_eq!(
        create_group(harness, name).await.status,
        201,
        "creating group {name} must succeed"
    );
    if !submittable {
        place_profile(harness, name, "application/json", body, at);
        return;
    }
    assert_eq!(
        harness
            .post_agent_profile(name, "application/json", None, body)
            .await
            .status,
        204,
        "posting a profile to {name} must succeed"
    );
    place_receipts(harness, name, &[at]);
}

/// One row of the metric-value table: a stored profile document, the metric asked about
/// and the ratio the one step of the series must report.
struct ValueCase {
    label: &'static str,
    body: Vec<u8>,
    frame_name: &'static str,
    thread_states: Vec<&'static str>,
    expected: f64,
}

/// The epoch-millisecond `start` every submitted profile states: a profile states the
/// start of the period it covers, and `PostAgentProfile` parses the submitted data and
/// refuses a profile without it. The instant itself is immaterial to a frame metric,
/// which is computed from the counts the document records.
const PROFILE_START_MILLIS: i64 = 1_600_000_000_000;

/// A profile document, as JSON bytes: the frames the case records, under the `start` a
/// submitted profile carries.
fn document(value: serde_json::Value) -> Vec<u8> {
    let mut value = value;
    if let Some(fields) = value.as_object_mut() {
        fields.insert(
            "start".to_string(),
            serde_json::json!(PROFILE_START_MILLIS),
        );
    }
    serde_json::to_vec(&value).expect("a serializable document")
}

/// The table. Rows differ along what the rule generalises over: how many frames the
/// profile holds, how deeply they nest, how many instances share a frame name, how many
/// thread states the metric asks about, whether the profile records a count in them at
/// all, whether the counts are whole or fractional, and whether the document is one this
/// service can read at all.
fn value_cases() -> Vec<ValueCase> {
    vec![
        ValueCase {
            label: "one frame of two, one thread state",
            body: document(serde_json::json!({
                "callgraph": { "A": { "RUNNABLE": 4 }, "B": { "RUNNABLE": 1 } }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.8,
        },
        ValueCase {
            label: "a frame's time includes the frames it calls",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": 4, "B": { "RUNNABLE": 2 } },
                    "C": { "RUNNABLE": 3 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 6.0 / 9.0,
        },
        ValueCase {
            label: "the called frame of the same profile carries its own time",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": 4, "B": { "RUNNABLE": 2 } },
                    "C": { "RUNNABLE": 3 }
                }
            })),
            frame_name: "B",
            thread_states: vec!["RUNNABLE"],
            expected: 2.0 / 9.0,
        },
        ValueCase {
            label: "two thread states count together",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": 4, "BLOCKED": 1 },
                    "C": { "RUNNABLE": 3, "WAITING": 2 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE", "BLOCKED"],
            expected: 5.0 / 8.0,
        },
        ValueCase {
            label: "a thread state the profile never recorded counts nothing",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": 4, "BLOCKED": 1 },
                    "C": { "RUNNABLE": 3 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["TERMINATED"],
            expected: 0.0,
        },
        ValueCase {
            label: "a metric that asks about no thread state counts nothing",
            body: document(serde_json::json!({
                "callgraph": { "A": { "RUNNABLE": 4 }, "B": { "RUNNABLE": 1 } }
            })),
            frame_name: "A",
            thread_states: vec![],
            expected: 0.0,
        },
        ValueCase {
            label: "every instance of the frame name is aggregated",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": 2 },
                    "B": { "A": { "RUNNABLE": 3 } },
                    "C": { "RUNNABLE": 5 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.5,
        },
        ValueCase {
            label: "a frame inside a frame of the same name counts its samples once",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": 2, "B": { "A": { "RUNNABLE": 3 } } },
                    "C": { "RUNNABLE": 5 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.5,
        },
        ValueCase {
            label: "a frame name the profile does not hold",
            body: document(serde_json::json!({
                "callgraph": { "A": { "RUNNABLE": 4 }, "B": { "RUNNABLE": 1 } }
            })),
            frame_name: "NotInTheProfile",
            thread_states: vec!["RUNNABLE"],
            expected: 0.0,
        },
        ValueCase {
            label: "fractional counts",
            body: document(serde_json::json!({
                "callgraph": { "A": { "RUNNABLE": 0.5 }, "B": { "RUNNABLE": 1.5 } }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.25,
        },
        ValueCase {
            label: "frames listed rather than keyed sit in the enclosing frame",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": [{ "RUNNABLE": 2 }, { "RUNNABLE": 1 }],
                    "B": { "RUNNABLE": 1 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.75,
        },
        ValueCase {
            label: "a negative number is no sample count",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "RUNNABLE": -1, "B": { "RUNNABLE": 2 } },
                    "C": { "RUNNABLE": 2 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.5,
        },
        ValueCase {
            label: "a count five frames down still belongs to the frame",
            body: document(serde_json::json!({
                "callgraph": {
                    "A": { "B": { "C": { "D": { "E": { "RUNNABLE": 3 } } } } },
                    "F": { "RUNNABLE": 1 }
                }
            })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.75,
        },
        ValueCase {
            label: "a document that records no count at all",
            body: document(serde_json::json!({ "callgraph": { "A": {}, "B": {} } })),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.0,
        },
        ValueCase {
            label: "bytes that are no document hold no frames",
            body: b"this is not a profile document".to_vec(),
            frame_name: "A",
            thread_states: vec!["RUNNABLE"],
            expected: 0.0,
        },
    ]
}

#[tokio::test]
async fn a_frame_metric_is_the_frames_counts_over_the_profiles_counts() {
    let harness = Harness::new();

    for (index, case) in value_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("frame-values-{index}");
        // A row whose bytes are no document at all cannot be POSTED — `PostAgentProfile`
        // parses the data it is handed — so it is placed in the store directly, which is
        // the only way such bytes get there; every other row is submitted the way an
        // agent submits one.
        let submittable = serde_json::from_slice::<serde_json::Value>(&case.body).is_ok();
        group_with_profile(
            &harness,
            &name,
            case.body.clone(),
            DAY_START + 60,
            submittable,
        )
        .await;

        let requested = frame_metric(case.frame_name, &case.thread_states);
        let answer = frame_metric_data(
            &harness,
            &name,
            &format!(
                "?startTime={}&endTime={}&targetResolution=PT1H",
                iso(DAY_START),
                iso(DAY_START + HOUR)
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
        assert_eq!(
            reported_resolution(&answer),
            "PT1H",
            "case {label:?}: expected the requested resolution, got {:?}",
            reported_resolution(&answer)
        );
        assert_eq!(
            end_times(&answer),
            vec![DAY_START + HOUR],
            "case {label:?}: expected the one hourly step of the period, got {:?}",
            end_times(&answer)
        );
        let (metric, values) = datum(&answer, 0);
        assert_eq!(
            metric, requested,
            "case {label:?}: the datum must name the metric it was asked for, got {metric:?}"
        );
        assert_eq!(
            values.len(),
            1,
            "case {label:?}: expected one value per reported step, got {values:?}"
        );
        assert!(
            (values[0] - case.expected).abs() < TOLERANCE,
            "case {label:?}: expected {}, got {:?}",
            case.expected,
            values[0]
        );
    }
}

/// One row of the resolution table: the requested resolution, the one reported, and how
/// many steps one day is cut into at it.
struct ResolutionCase {
    label: &'static str,
    /// `targetResolution` as sent, or `None` to send none.
    requested: Option<&'static str>,
    reported: &'static str,
    step_seconds: i64,
    steps: usize,
}

/// The three resolutions the model's `AggregationPeriod` declares, and the request that
/// names none. The step counts are the ones the `endTimes` doc states for a one-day
/// period: "if the period is one day (PT24H), and the resolution is five minutes (PT5M),
/// then there are 288 endTimes in the list".
fn resolution_cases() -> Vec<ResolutionCase> {
    vec![
        ResolutionCase {
            label: "five minutes",
            requested: Some("PT5M"),
            reported: "PT5M",
            step_seconds: FIVE_MINUTES,
            steps: 288,
        },
        ResolutionCase {
            label: "one hour",
            requested: Some("PT1H"),
            reported: "PT1H",
            step_seconds: HOUR,
            steps: 24,
        },
        ResolutionCase {
            label: "one day",
            requested: Some("P1D"),
            reported: "P1D",
            step_seconds: DAY,
            steps: 1,
        },
        ResolutionCase {
            label: "no resolution asked for",
            requested: None,
            reported: "PT5M",
            step_seconds: FIVE_MINUTES,
            steps: 288,
        },
    ]
}

#[tokio::test]
async fn the_steps_are_the_aligned_windows_of_the_reported_resolution() {
    let harness = Harness::new();
    // The profile sits in the FIRST window of the period at every resolution, so the one
    // non-zero value is the first step's whatever the resolution is.
    let body = document(serde_json::json!({
        "callgraph": { "A": { "RUNNABLE": 4 }, "B": { "RUNNABLE": 1 } }
    }));

    for (index, case) in resolution_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("frame-steps-{index}");
        group_with_profile(&harness, &name, body.clone(), DAY_START + 60, true).await;

        let mut query = format!(
            "?startTime={}&endTime={}",
            iso(DAY_START),
            iso(DAY_START + DAY)
        );
        if let Some(requested) = case.requested {
            query.push_str(&format!("&targetResolution={requested}"));
        }
        let answer = frame_metric_data(
            &harness,
            &name,
            &query,
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
        assert_eq!(
            reported_resolution(&answer),
            case.reported,
            "case {label:?}: expected resolution {:?}, got {:?}",
            case.reported,
            reported_resolution(&answer)
        );
        assert_eq!(
            reported_period(&answer),
            (DAY_START, DAY_START + DAY),
            "case {label:?}: expected the requested period, got {:?}",
            reported_period(&answer)
        );

        let times = end_times(&answer);
        assert_eq!(
            times.len(),
            case.steps,
            "case {label:?}: expected {} steps of one day, got {}",
            case.steps,
            times.len()
        );
        let expected: Vec<i64> = (0..case.steps)
            .map(|step| {
                DAY_START + case.step_seconds * (i64::try_from(step).unwrap_or_default() + 1)
            })
            .collect();
        assert_eq!(
            times, expected,
            "case {label:?}: the steps must be the aligned windows of {:?}",
            case.reported
        );

        let (_, values) = datum(&answer, 0);
        assert_eq!(
            values.len(),
            times.len(),
            "case {label:?}: expected one value per step, got {} values for {} steps",
            values.len(),
            times.len()
        );
        assert!(
            (values[0] - 0.8).abs() < TOLERANCE,
            "case {label:?}: expected the stored profile's ratio in the first step, got {:?}",
            values[0]
        );
        assert!(
            values[1..].iter().all(|value| *value == 0.0),
            "case {label:?}: a step holding no profile has no time in the frame, got {:?}",
            values
        );
    }
}

#[tokio::test]
async fn a_step_whose_aggregation_is_not_due_is_an_unprocessed_instance() {
    let harness = Harness::new();
    let name = "frame-pending";
    assert_eq!(
        create_group(&harness, name).await.status,
        201,
        "create must succeed"
    );
    let body = document(serde_json::json!({
        "callgraph": { "A": { "RUNNABLE": 4 }, "B": { "RUNNABLE": 1 } }
    }));
    // Posted over HTTP, so the receipt is the clock's: the window it lands in is still
    // open and its aggregation has not fallen due.
    let before = now();
    assert_eq!(
        harness
            .post_agent_profile(name, "application/json", None, body)
            .await
            .status,
        204,
        "the post must succeed"
    );
    let after = now();

    // A period around the post, at the finest resolution, so the step holding it is one
    // of several.
    let query = format!(
        "?startTime={}&endTime={}&targetResolution=PT5M",
        iso(before - 2 * FIVE_MINUTES),
        iso(after + 2 * FIVE_MINUTES)
    );
    let metrics = [frame_metric("A", &["RUNNABLE"])];
    let answer = frame_metric_data(&harness, name, &query, &metrics).await;
    assert_eq!(
        answer.status,
        200,
        "expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );

    // Every aligned five-minute window the period overlaps, which is what the steps of
    // the series are.
    let first = (before - 2 * FIVE_MINUTES).div_euclid(FIVE_MINUTES) * FIVE_MINUTES;
    let mut all_steps: Vec<i64> = Vec::new();
    let mut step = first;
    while step < after + 2 * FIVE_MINUTES {
        all_steps.push(step + FIVE_MINUTES);
        step += FIVE_MINUTES;
    }
    // The receipt is somewhere in `[before, after]`, so the step holding it is the window
    // containing either instant — the same window unless the post straddled a boundary.
    let pending_candidates: Vec<i64> = [before, after]
        .iter()
        .map(|at| at.div_euclid(FIVE_MINUTES) * FIVE_MINUTES + FIVE_MINUTES)
        .collect();

    let unprocessed = unprocessed_end_times(&answer);
    assert_eq!(
        unprocessed.keys().cloned().collect::<Vec<String>>(),
        vec![name.to_string()],
        "the unprocessed instances must be keyed by the group they are about, got {unprocessed:?}"
    );
    let pending = unprocessed
        .get(name)
        .unwrap_or_else(|| panic!("no instances under {name} in {unprocessed:?}"))
        .clone();
    assert_eq!(
        pending.len(),
        1,
        "exactly the step holding the pending profile is unprocessed, got {pending:?}"
    );
    assert!(
        pending_candidates.contains(&pending[0]),
        "the unprocessed instance must be the window holding the receipt, expected one of \
         {pending_candidates:?}, got {:?}",
        pending[0]
    );

    let reported = end_times(&answer);
    assert!(
        !reported.contains(&pending[0]),
        "an unprocessed instance leaves a missing time step, got {reported:?}"
    );
    let expected_reported: Vec<i64> = all_steps
        .iter()
        .copied()
        .filter(|at| *at != pending[0])
        .collect();
    assert_eq!(
        reported, expected_reported,
        "every other step of the period must be reported, got {reported:?}"
    );
    let (_, values) = datum(&answer, 0);
    assert_eq!(
        values.len(),
        reported.len(),
        "expected one value per reported step, got {} values for {} steps",
        values.len(),
        reported.len()
    );
    assert!(
        values.iter().all(|value| *value == 0.0),
        "a profile that is in no aggregated profile yet contributes no value, got {values:?}"
    );

    // Once the aggregation that window owes has fallen due, the step it lands in is
    // reported with the stored profile's ratio and nothing is unprocessed.
    harness.complete_aggregation(name);
    let served = frame_metric_data(&harness, name, &query, &metrics).await;
    assert_eq!(
        served.status,
        200,
        "expected 200 after the aggregation fell due, got {} {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    assert!(
        unprocessed_end_times(&served).is_empty(),
        "an aggregated profile leaves no unprocessed instance, got {:?}",
        unprocessed_end_times(&served)
    );
    let served_steps = end_times(&served);
    assert_eq!(
        served_steps, all_steps,
        "every step of the period must be reported, got {served_steps:?}"
    );
    let (_, served_values) = datum(&served, 0);
    let scored: Vec<f64> = served_values
        .iter()
        .copied()
        .filter(|value| *value != 0.0)
        .collect();
    assert_eq!(
        scored.len(),
        1,
        "exactly the step the profile moved into carries a value, got {served_values:?}"
    );
    assert!(
        (scored[0] - 0.8).abs() < TOLERANCE,
        "expected the stored profile's ratio, got {:?}",
        scored[0]
    );

    // The read is reporting: asking twice answers the same, and the group still reports
    // the receipt it recorded.
    let again = frame_metric_data(&harness, name, &query, &metrics).await;
    assert_eq!(
        again.body, served.body,
        "two reads of one period must agree"
    );
    assert!(
        harness
            .describe(name)
            .await
            .json()
            .pointer("/profilingStatus/latestAgentProfileReportedAt")
            .is_some(),
        "reading a series must leave the group's recorded receipt in place"
    );
}

#[tokio::test]
async fn a_request_that_names_no_metric_still_reports_the_period_and_its_steps() {
    let harness = Harness::new();
    let name = "frame-no-metrics";
    group_with_profile(
        &harness,
        name,
        document(serde_json::json!({ "callgraph": { "A": { "RUNNABLE": 1 } } })),
        DAY_START + 60,
        true,
    )
    .await;

    for (label, body) in [
        ("no frameMetrics member", serde_json::json!({})),
        ("an empty frameMetrics list", serde_json::json!({ "frameMetrics": [] })),
    ] {
        let answer = harness
            .send(
                "POST",
                &format!(
                    "/profilingGroups/{name}/frames/-/metrics?startTime={}&endTime={}\
                     &targetResolution=PT1H",
                    iso(DAY_START),
                    iso(DAY_START + HOUR)
                ),
                "application/json",
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            datum_count(&answer),
            0,
            "case {label:?}: a request naming no metric reports no datum"
        );
        assert_eq!(
            end_times(&answer),
            vec![DAY_START + HOUR],
            "case {label:?}: the steps of the period are reported all the same, got {:?}",
            end_times(&answer)
        );
    }
}

/// One row of the period table: how the request names its time period, and the period
/// the response must report.
struct PeriodCase {
    label: &'static str,
    query: String,
    /// The start the response must report, or `None` when the period's start is relative
    /// to the current instant.
    start: Option<i64>,
    /// How long the reported period must be, in seconds.
    length: i64,
}

fn period_cases() -> Vec<PeriodCase> {
    vec![
        PeriodCase {
            label: "both bounds",
            query: format!(
                "?startTime={}&endTime={}",
                iso(DAY_START),
                iso(DAY_START + HOUR)
            ),
            start: Some(DAY_START),
            length: HOUR,
        },
        PeriodCase {
            label: "a start and a length",
            query: format!("?startTime={}&period=PT1H", iso(DAY_START)),
            start: Some(DAY_START),
            length: HOUR,
        },
        PeriodCase {
            label: "an end and a length",
            query: format!("?endTime={}&period=PT1H", iso(DAY_START + HOUR)),
            start: Some(DAY_START),
            length: HOUR,
        },
        PeriodCase {
            label: "a start and a length with a sub-hour part",
            query: format!("?startTime={}&period=PT5M", iso(DAY_START)),
            start: Some(DAY_START),
            length: FIVE_MINUTES,
        },
        PeriodCase {
            label: "a start alone is the longest period",
            query: format!("?startTime={}", iso(DAY_START)),
            start: Some(DAY_START),
            length: DAY,
        },
        PeriodCase {
            label: "an end alone is the longest period",
            query: format!("?endTime={}", iso(DAY_START)),
            start: Some(DAY_START - DAY),
            length: DAY,
        },
        PeriodCase {
            label: "a length alone ends at the current instant",
            query: "?period=PT1H".to_string(),
            start: None,
            length: HOUR,
        },
        PeriodCase {
            label: "no member at all is the longest period ending now",
            query: String::new(),
            start: None,
            length: DAY,
        },
    ]
}

#[tokio::test]
async fn the_reported_period_is_the_one_the_request_names() {
    let harness = Harness::new();
    let name = "frame-period";
    group_with_profile(
        &harness,
        name,
        document(serde_json::json!({ "callgraph": { "A": { "RUNNABLE": 1 } } })),
        DAY_START + 60,
        true,
    )
    .await;

    for case in period_cases() {
        let label = case.label;
        let before = now();
        let answer = frame_metric_data(
            &harness,
            name,
            &case.query,
            &[frame_metric("A", &["RUNNABLE"])],
        )
        .await;
        let after = now();
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (start, end) = reported_period(&answer);
        assert_eq!(
            end - start,
            case.length,
            "case {label:?}: expected a period {} seconds long, got {start} to {end}",
            case.length
        );
        match case.start {
            Some(expected) => assert_eq!(
                start, expected,
                "case {label:?}: expected the period to start at {expected}, got {start}"
            ),
            None => assert!(
                end >= before && end <= after,
                "case {label:?}: expected the period to end at the current instant \
                 ({before}..={after}), got {end}"
            ),
        }
        // However the period was named, the steps still cover it.
        let times = end_times(&answer);
        let (_, values) = datum(&answer, 0);
        assert_eq!(
            values.len(),
            times.len(),
            "case {label:?}: expected one value per step, got {} values for {} steps",
            values.len(),
            times.len()
        );
        assert!(
            !times.is_empty(),
            "case {label:?}: a period has at least one step"
        );
    }
}

/// One row of the refusal table: the request, and the refusal it must answer with.
#[tokio::test]
async fn a_period_far_from_the_epoch_is_still_a_bounded_series() {
    let harness = Harness::new();
    let name = "frame-far-future";
    group_with_profile(
        &harness,
        name,
        document(serde_json::json!({ "callgraph": { "A": { "RUNNABLE": 1 } } })),
        DAY_START + 60,
        true,
    )
    .await;

    // Instants far from the epoch, in both directions, are stepped like any others: the
    // series is the one-day default at the finest resolution, so it holds the same 288
    // steps and holds no profile data.
    for (label, query) in [
        ("a start in the year 9999", "?startTime=9999-01-01T00:00:00Z"),
        ("an end in the year 9999", "?endTime=9999-01-01T00:00:00Z"),
        ("a start in the year 1800", "?startTime=1800-01-01T00:00:00Z"),
    ] {
        let answer =
            frame_metric_data(&harness, name, query, &[frame_metric("A", &["RUNNABLE"])]).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (start, end) = reported_period(&answer);
        assert_eq!(
            end - start,
            DAY,
            "case {label:?}: expected the longest period, got {start} to {end}"
        );
        let times = end_times(&answer);
        assert_eq!(
            times.len(),
            (DAY / FIVE_MINUTES) as usize,
            "case {label:?}: expected one day of five-minute steps, got {}",
            times.len()
        );
        let (_, values) = datum(&answer, 0);
        assert!(
            values.iter().all(|value| *value == 0.0),
            "case {label:?}: the group holds no profile in that period, got {values:?}"
        );
    }
}

/// One row of the refusal table: the request, and the refusal it must answer with.
struct RefusalCase {
    label: &'static str,
    group: &'static str,
    query: String,
    metrics: Vec<serde_json::Value>,
    status: u16,
    code: &'static str,
    message: String,
}

fn refusal_cases(group: &'static str) -> Vec<RefusalCase> {
    let two_days_on = DAY_START + 2 * DAY;
    vec![
        RefusalCase {
            label: "a group that is not stored",
            group: "no-such-group",
            query: format!("?startTime={}&period=PT1H", iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 404,
            code: "ResourceNotFoundException",
            message: "CodeGuru Profiler can\u{2019}t complete the operation because \
                      no-such-group was not found."
                .to_string(),
        },
        RefusalCase {
            label: "all three members of the period",
            group,
            query: format!(
                "?startTime={}&endTime={}&period=PT1H",
                iso(DAY_START),
                iso(DAY_START + HOUR)
            ),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: "specify 1 or 2 of startTime, endTime and period, not all 3".to_string(),
        },
        RefusalCase {
            label: "a reversed pair of bounds",
            group,
            query: format!(
                "?startTime={}&endTime={}",
                iso(DAY_START + HOUR),
                iso(DAY_START)
            ),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: format!(
                "Interval start time ({}) must be before end time ({})",
                iso_in_message(DAY_START + HOUR),
                iso_in_message(DAY_START)
            ),
        },
        RefusalCase {
            label: "a zero-width pair of bounds",
            group,
            query: format!("?startTime={}&endTime={}", iso(DAY_START), iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: format!(
                "Interval start time ({}) must be before end time ({})",
                iso_in_message(DAY_START),
                iso_in_message(DAY_START)
            ),
        },
        RefusalCase {
            label: "a length of zero",
            group,
            query: format!("?startTime={}&period=PT0S", iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: format!(
                "Interval start time ({}) must be before end time ({})",
                iso_in_message(DAY_START),
                iso_in_message(DAY_START)
            ),
        },
        RefusalCase {
            label: "a pair of bounds two days apart",
            group,
            query: format!(
                "?startTime={}&endTime={}",
                iso(DAY_START),
                iso(two_days_on)
            ),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: format!(
                "the time period for the returned time series values must be at most PT24H \
                 long, got {} to {}",
                iso_in_message(DAY_START),
                iso_in_message(two_days_on)
            ),
        },
        RefusalCase {
            label: "a length of two days",
            group,
            query: format!("?startTime={}&period=P2D", iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: format!(
                "the time period for the returned time series values must be at most PT24H \
                 long, got {} to {}",
                iso_in_message(DAY_START),
                iso_in_message(two_days_on)
            ),
        },
        RefusalCase {
            label: "a length one second past the maximum",
            group,
            query: format!("?startTime={}&period=PT86401S", iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: format!(
                "the time period for the returned time series values must be at most PT24H \
                 long, got {} to {}",
                iso_in_message(DAY_START),
                iso_in_message(DAY_START + DAY + 1)
            ),
        },
        RefusalCase {
            label: "a length that names no span",
            group,
            query: format!("?startTime={}&period=yesterday", iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: "period must name a fixed span of time in the ISO 8601 duration \
                      format, for example P1DT1H1M1S, got yesterday"
                .to_string(),
        },
        RefusalCase {
            label: "a resolution the service does not aggregate at",
            group,
            query: format!("?startTime={}&period=PT1H&targetResolution=PT7M", iso(DAY_START)),
            metrics: vec![frame_metric("A", &["RUNNABLE"])],
            status: 400,
            code: "ValidationException",
            message: "1 validation error detected: Value 'PT7M' at 'targetResolution' failed \
                      to satisfy constraint: Member must satisfy enum value set: \
                      [P1D, PT1H, PT5M]"
                .to_string(),
        },
        RefusalCase {
            label: "an aggregation the metric type does not declare",
            group,
            query: format!("?startTime={}&period=PT1H", iso(DAY_START)),
            metrics: vec![serde_json::json!({
                "frameName": "A",
                "type": "TotalSelfTime",
                "threadStates": ["RUNNABLE"],
            })],
            status: 400,
            code: "ValidationException",
            message: "1 validation error detected: Value 'TotalSelfTime' at \
                      'frameMetrics.1.member.type' failed to satisfy constraint: Member must \
                      satisfy enum value set: [AggregatedRelativeTotalTime]"
                .to_string(),
        },
        RefusalCase {
            label: "the SECOND metric names an aggregation that is not declared",
            group,
            query: format!("?startTime={}&period=PT1H", iso(DAY_START)),
            metrics: vec![
                frame_metric("A", &["RUNNABLE"]),
                serde_json::json!({
                    "frameName": "B",
                    "type": "",
                    "threadStates": ["RUNNABLE"],
                }),
            ],
            status: 400,
            code: "ValidationException",
            message: "1 validation error detected: Value '' at 'frameMetrics.2.member.type' \
                      failed to satisfy constraint: Member must satisfy enum value set: \
                      [AggregatedRelativeTotalTime]"
                .to_string(),
        },
        RefusalCase {
            label: "one frame metric more than the service answers about",
            group,
            query: format!("?startTime={}&period=PT1H", iso(DAY_START)),
            metrics: (0..=MAX_FRAME_METRICS)
                .map(|index| frame_metric(&format!("Frame{index}"), &["RUNNABLE"]))
                .collect(),
            status: 400,
            code: "ValidationException",
            message: format!(
                "Value at 'frameMetrics' failed to satisfy constraint: Member must have \
                 length less than or equal to {MAX_FRAME_METRICS}"
            ),
        },
        RefusalCase {
            label: "one thread state more than a frame metric may name",
            group,
            query: format!("?startTime={}&period=PT1H", iso(DAY_START)),
            metrics: vec![
                frame_metric("A", &["RUNNABLE"]),
                serde_json::json!({
                    "frameName": "B",
                    "type": "AggregatedRelativeTotalTime",
                    "threadStates": (0..=MAX_THREAD_STATES)
                        .map(|index| format!("STATE_{index}"))
                        .collect::<Vec<String>>(),
                }),
            ],
            status: 400,
            code: "ValidationException",
            message: format!(
                "Value at 'frameMetrics.2.threadStates' failed to satisfy constraint: Member \
                 must have length less than or equal to {MAX_THREAD_STATES}"
            ),
        },
    ]
}

#[tokio::test]
async fn a_request_the_operation_cannot_report_on_is_refused() {
    let harness = Harness::new();
    let group = "frame-refusals";
    group_with_profile(
        &harness,
        group,
        document(serde_json::json!({ "callgraph": { "A": { "RUNNABLE": 1 } } })),
        DAY_START + 60,
        true,
    )
    .await;

    for case in refusal_cases(group) {
        let label = case.label;
        let answer = frame_metric_data(&harness, case.group, &case.query, &case.metrics).await;
        assert_eq!(
            answer.status, case.status,
            "case {label:?}: expected {}, got {} {}",
            case.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            case.code,
            "case {label:?}: expected {}, got {}",
            case.code,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_message(),
            case.message,
            "case {label:?}: expected {:?}, got {:?}",
            case.message,
            answer.error_message()
        );
    }

    // At the bound itself the request is answered, so the refusal above is about the
    // length and not about asking for many metrics at all.
    let at_the_bound: Vec<serde_json::Value> = (0..MAX_FRAME_METRICS)
        .map(|index| frame_metric(&format!("Frame{index}"), &["RUNNABLE"]))
        .collect();
    let answer = frame_metric_data(
        &harness,
        group,
        &format!("?startTime={}&period=PT1H", iso(DAY_START)),
        &at_the_bound,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case \"as many frame metrics as the service answers about\": expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        datum_count(&answer),
        MAX_FRAME_METRICS,
        "case \"as many frame metrics as the service answers about\": expected one datum per \
         requested metric"
    );

    // Likewise at the thread-state bound: as many states as a metric may name is answered,
    // and the stored profile's one state is still what counts.
    let mut states: Vec<String> = vec!["RUNNABLE".to_string()];
    while states.len() < MAX_THREAD_STATES {
        states.push(format!("STATE_{}", states.len()));
    }
    let answer = frame_metric_data(
        &harness,
        group,
        &format!("?startTime={}&period=PT1H", iso(DAY_START)),
        &[serde_json::json!({
            "frameName": "A",
            "type": "AggregatedRelativeTotalTime",
            "threadStates": states,
        })],
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case \"as many thread states as a frame metric may name\": expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let (_, values) = datum(&answer, 0);
    assert_eq!(
        values.first().copied(),
        Some(1.0),
        "case \"as many thread states as a frame metric may name\": the stored profile's one \
         frame holds all of its counted time, got {values:?}"
    );
    assert!(
        values[1..].iter().all(|value| *value == 0.0),
        "case \"as many thread states as a frame metric may name\": a step holding no profile \
         has no time in the frame, got {values:?}"
    );
}
