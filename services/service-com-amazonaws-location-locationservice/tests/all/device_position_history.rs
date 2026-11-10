// SPDX-License-Identifier: CC0-1.0

//! `GetDevicePositionHistory`: the half-open sample-time window, the page order and
//! the refusals, end to end over the wire.
//!
//! RULE: a position-history read reports the samples the tracker holds for the device
//! whose `SampleTime` lies in the half-open window `[StartTimeInclusive,
//! EndTimeExclusive)` — the start instant included, the end instant excluded —
//! oldest sample first, at most `MaxResults` per page, with a `NextToken` that
//! resumes exactly after the page it was minted with; a window that does not run
//! forwards is refused, and the read mutates nothing.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, so each call is a
//! separate HTTP request against one shared engine: a sample that survives from the
//! write call to the read call survived because it was PERSISTED.
//!
//! The tables span what the rule generalises over: where a window's bounds fall
//! relative to the stored instants (outside, straddling, coinciding with a sample at
//! either end), the number of samples a window selects (none, one, several, all), the
//! page size (every divisor and non-divisor of the sample count, and the absent
//! member), and the form of a malformed cursor.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, emulator, iso_millis, millis_of, wire_time, Answer};

/// The tracker every case in this suite writes under.
const TRACKER: &str = "history-tracker";

/// The device every case in this suite writes under. Satisfies `Id`'s
/// `^[-._\p{L}\p{N}]+$`.
const DEVICE: &str = "fleet.device_7";

/// The stored sample times, ascending, written out of order below so the ordering the
/// read reports is the store's and not the request's.
///
/// They are whole seconds and sub-second instants inside one minute, so a window
/// bound can be placed before, between, on and after them.
const SAMPLES: [&str; 5] = [
    "2024-03-01T12:00:00Z",
    "2024-03-01T12:00:10Z",
    "2024-03-01T12:00:20.500Z",
    "2024-03-01T12:00:30Z",
    "2024-03-01T12:00:40Z",
];

async fn create_tracker(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        "POST",
        "/tracking/v0/trackers",
        Some(json!({ "TrackerName": name })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "CreateTracker {name:?}: expected 200, got {} {}",
        answer.status, answer.body
    );
}

async fn batch_update(router: &MultiServiceRouter, tracker: &str, updates: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tracking/v0/trackers/{tracker}/positions"),
        Some(json!({ "Updates": updates })),
    )
    .await
}

async fn history(
    router: &MultiServiceRouter,
    tracker: &str,
    device: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/tracking/v0/trackers/{tracker}/devices/{device}/list-positions"),
        Some(body),
    )
    .await
}

/// The sample times a history answer reports, in the order it reported them.
fn reported_sample_times(answer: &Answer) -> Vec<i64> {
    let body = answer.json();
    let positions = body
        .get("DevicePositions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DevicePositions must be a list, got {body}"));
    positions
        .iter()
        .map(|position| millis_of(position, "SampleTime"))
        .collect()
}

/// The `NextToken` a history answer carries, if any.
fn next_token(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// An ISO 8601 rendering of an instant, for the rows whose bounds are relative to the
/// clock rather than written as literals.
fn iso_of_millis(millis: i64) -> String {
    DateTime::from_millis(millis)
        .fmt(Format::DateTime)
        .expect("an epoch-millis instant renders as ISO 8601")
}

/// Stand up one emulator holding [`SAMPLES`] for [`DEVICE`] under [`TRACKER`].
///
/// The updates are sent newest-first and in two batches, so the ascending order a
/// read reports is the one the store keeps rather than the order of arrival.
async fn emulator_with_samples() -> MultiServiceRouter {
    let router = emulator();
    create_tracker(&router, TRACKER).await;
    let update = |iso: &str, longitude: f64| {
        json!({
            "DeviceId": DEVICE,
            "SampleTime": wire_time(iso),
            "Position": [longitude, 47.60],
        })
    };
    for batch in [
        vec![update(SAMPLES[4], -122.34), update(SAMPLES[1], -122.31)],
        vec![
            update(SAMPLES[3], -122.33),
            update(SAMPLES[0], -122.30),
            update(SAMPLES[2], -122.32),
        ],
    ] {
        let answer = batch_update(&router, TRACKER, json!(batch)).await;
        assert_eq!(
            answer.status, 200,
            "seeding BatchUpdateDevicePosition: expected 200, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.json().get("Errors").and_then(Value::as_array),
            Some(&vec![]),
            "seeding BatchUpdateDevicePosition: expected no per-entry errors, got {}",
            answer.body
        );
    }
    router
}

// ---------------------------------------------------------------------------
// The half-open window
// ---------------------------------------------------------------------------

/// One row of the window table: the bounds a read asks for, and the stored sample
/// times (as indices into [`SAMPLES`]) the half-open window selects.
struct WindowCase {
    case: &'static str,
    start: Option<&'static str>,
    end: Option<&'static str>,
    selected: &'static [usize],
}

/// RULE: `[StartTimeInclusive, EndTimeExclusive)` is half-open — a sample stamped at
/// the start instant is reported, one stamped at the end instant is not — and the
/// page is ascending by sample time.
#[tokio::test]
async fn a_window_selects_the_half_open_sample_time_range() {
    let router = emulator_with_samples().await;

    let cases = [
        WindowCase {
            case: "both bounds outside every sample",
            start: Some("2024-03-01T11:00:00Z"),
            end: Some("2024-03-01T13:00:00Z"),
            selected: &[0, 1, 2, 3, 4],
        },
        WindowCase {
            case: "start on the first sample, end on the last: the last is excluded",
            start: Some(SAMPLES[0]),
            end: Some(SAMPLES[4]),
            selected: &[0, 1, 2, 3],
        },
        WindowCase {
            case: "start between two samples, end on a sample",
            start: Some("2024-03-01T12:00:05Z"),
            end: Some(SAMPLES[3]),
            selected: &[1, 2],
        },
        WindowCase {
            case: "a window holding exactly one sub-second sample",
            start: Some(SAMPLES[2]),
            end: Some("2024-03-01T12:00:20.501Z"),
            selected: &[2],
        },
        WindowCase {
            case: "a one-millisecond window just past a sample instant",
            start: Some("2024-03-01T12:00:00.001Z"),
            end: Some("2024-03-01T12:00:00.002Z"),
            selected: &[],
        },
        WindowCase {
            case: "a window entirely after every sample",
            start: Some("2024-03-01T12:01:00Z"),
            end: Some("2024-03-01T12:02:00Z"),
            selected: &[],
        },
        WindowCase {
            case: "a window entirely before every sample",
            start: Some("2024-03-01T00:00:00Z"),
            end: Some("2024-03-01T01:00:00Z"),
            selected: &[],
        },
        WindowCase {
            case: "only a start: the end defaults forward of every sample",
            start: Some("2024-03-01T12:00:15Z"),
            end: None,
            selected: &[2, 3, 4],
        },
    ];

    for WindowCase {
        case,
        start,
        end,
        selected,
    } in cases
    {
        let mut body = json!({});
        if let Some(start) = start {
            body["StartTimeInclusive"] = wire_time(start);
        }
        if let Some(end) = end {
            body["EndTimeExclusive"] = wire_time(end);
        }
        let answer = history(&router, TRACKER, DEVICE, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {}",
            answer.status, answer.body
        );
        let expected: Vec<i64> = selected.iter().map(|i| iso_millis(SAMPLES[*i])).collect();
        let reported = reported_sample_times(&answer);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected sample times {expected:?}, got {reported:?}"
        );
        assert_eq!(
            next_token(&answer),
            None,
            "case {case:?}: a page holding every selected sample must carry no NextToken, got {}",
            answer.body
        );
    }
}

/// RULE: with neither bound, the window is the documented default — the 24 hours
/// before the request — so a sample older than that is outside it while a recent one
/// is inside it. (`GetDevicePositionHistoryRequest$StartTimeInclusive`: "By default,
/// the value will be 24 hours prior to the time that the request is made.")
#[tokio::test]
async fn the_default_window_is_the_twenty_four_hours_before_the_request() {
    let router = emulator();
    create_tracker(&router, TRACKER).await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_millis() as i64;
    let hour = 60 * 60 * 1_000;
    // Rows: inside the default window at both ends of it, and outside it.
    let inside = [now - 23 * hour, now - hour, now - 1_000];
    let outside = [now - 25 * hour, now - 10 * 24 * hour];

    let updates: Vec<Value> = inside
        .iter()
        .chain(outside.iter())
        .map(|millis| {
            json!({
                "DeviceId": DEVICE,
                "SampleTime": wire_time(&iso_of_millis(*millis)),
                "Position": [-122.33, 47.60],
            })
        })
        .collect();
    let seeded = batch_update(&router, TRACKER, json!(updates)).await;
    assert_eq!(
        seeded.status, 200,
        "seeding the default-window samples: expected 200, got {} {}",
        seeded.status, seeded.body
    );

    let answer = history(&router, TRACKER, DEVICE, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "the default window: expected 200, got {} {}",
        answer.status, answer.body
    );
    let mut expected = inside.to_vec();
    expected.sort_unstable();
    let reported = reported_sample_times(&answer);
    assert_eq!(
        reported, expected,
        "the default window: expected only the samples inside the last 24 hours, \
         {expected:?}, got {reported:?}"
    );

    // Only an end: the start still defaults back 24 hours, so the window is the
    // default one truncated at that end.
    let truncated = history(
        &router,
        TRACKER,
        DEVICE,
        json!({ "EndTimeExclusive": wire_time(&iso_of_millis(now - hour)) }),
    )
    .await;
    assert_eq!(
        truncated.status, 200,
        "an end with the default start: expected 200, got {} {}",
        truncated.status, truncated.body
    );
    let reported = reported_sample_times(&truncated);
    assert_eq!(
        reported,
        vec![now - 23 * hour],
        "an end with the default start: expected the samples in the last 24 hours before \
         that end, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// RULE: a page holds at most `MaxResults` samples in ascending sample-time order,
/// and `NextToken` resumes exactly after it, so walking every page yields the window
/// once, in order, whatever the page size.
#[tokio::test]
async fn pages_walk_the_window_once_in_ascending_sample_time_order() {
    let router = emulator_with_samples().await;
    let all: Vec<i64> = SAMPLES.iter().map(|iso| iso_millis(iso)).collect();

    // Rows: every page size from one sample to past the whole window — divisors and
    // non-divisors of the sample count — plus the absent member, whose default page
    // (100) holds the lot.
    let page_sizes: [Option<i64>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(5),
        Some(6),
        None,
    ];

    for max_results in page_sizes {
        let case = format!("MaxResults={max_results:?}");
        let mut walked = Vec::new();
        let mut page_lengths = Vec::new();
        let mut token: Option<String> = None;
        // Bounded by the sample count: one page per sample is the smallest page any
        // `MaxResults` can ask for, so more iterations than that is a defect.
        for round in 0..=SAMPLES.len() {
            let mut body = json!({
                "StartTimeInclusive": wire_time(SAMPLES[0]),
                "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
            });
            if let Some(max_results) = max_results {
                body["MaxResults"] = json!(max_results);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let answer = history(&router, TRACKER, DEVICE, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} page {round}: expected 200, got {} {}",
                answer.status, answer.body
            );
            let page = reported_sample_times(&answer);
            let limit = max_results.unwrap_or(100) as usize;
            assert!(
                page.len() <= limit,
                "case {case:?} page {round}: expected at most {limit} samples, got {page:?}"
            );
            page_lengths.push(page.len());
            walked.extend(page);
            token = next_token(&answer);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            token, None,
            "case {case:?}: the walk must end with no NextToken, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must yield the window once in ascending order, \
             expected {all:?}, got {walked:?} (page lengths {page_lengths:?})"
        );
    }
}

/// RULE: `NextToken` is minted only while samples remain, so the last page of a
/// window carries none even when it is exactly `MaxResults` long.
#[tokio::test]
async fn a_full_last_page_carries_no_next_token() {
    let router = emulator_with_samples().await;

    // Rows: a window whose sample count is exactly the page size, so the page is
    // full yet final; and one with a sample past the page, so a token is minted.
    let rows: [(&str, &str, i64, usize, bool); 3] = [
        ("a full final page", SAMPLES[3], 2, 2, false),
        ("a full page with one sample left", SAMPLES[0], 2, 2, true),
        ("a short final page", SAMPLES[4], 2, 1, false),
    ];

    for (case, start, max_results, expected_len, expects_token) in rows {
        let answer = history(
            &router,
            TRACKER,
            DEVICE,
            json!({
                "StartTimeInclusive": wire_time(start),
                "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
                "MaxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} {}",
            answer.status, answer.body
        );
        let page = reported_sample_times(&answer);
        assert_eq!(
            page.len(),
            expected_len,
            "case {case:?}: expected {expected_len} samples, got {page:?}"
        );
        let token = next_token(&answer);
        assert_eq!(
            token.is_some(),
            expects_token,
            "case {case:?}: expected NextToken present={expects_token}, got {token:?}"
        );
    }
}

/// RULE: a cursor narrows the window it resumes inside; it never widens it, so a page
/// fetched with a tighter window than the one the cursor was minted under reports
/// only what that tighter window holds.
#[tokio::test]
async fn a_cursor_never_widens_the_window_it_resumes_in() {
    let router = emulator_with_samples().await;

    let first = history(
        &router,
        TRACKER,
        DEVICE,
        json!({
            "StartTimeInclusive": wire_time(SAMPLES[0]),
            "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
            "MaxResults": 1,
        }),
    )
    .await;
    let token = next_token(&first).unwrap_or_else(|| {
        panic!("the first page of five samples must carry a NextToken, got {}", first.body)
    });

    // The same cursor, re-sent with a window that ends before the sample it resumes
    // at: the window wins.
    let narrowed = history(
        &router,
        TRACKER,
        DEVICE,
        json!({
            "StartTimeInclusive": wire_time(SAMPLES[0]),
            "EndTimeExclusive": wire_time(SAMPLES[1]),
            "NextToken": token,
        }),
    )
    .await;
    assert_eq!(
        narrowed.status, 200,
        "a narrowed window: expected 200, got {} {}",
        narrowed.status, narrowed.body
    );
    let reported = reported_sample_times(&narrowed);
    assert_eq!(
        reported,
        Vec::<i64>::new(),
        "a narrowed window: a cursor must not report a sample outside the window asked \
         for, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals, and the read that writes nothing
// ---------------------------------------------------------------------------

/// RULE: a window that does not run forwards is a `ValidationException` naming
/// `EndTimeExclusive`, and the refused read leaves the stored history exactly as it
/// was.
#[tokio::test]
async fn a_window_that_does_not_run_forwards_is_refused() {
    let router = emulator_with_samples().await;
    let all: Vec<i64> = SAMPLES.iter().map(|iso| iso_millis(iso)).collect();

    // Rows: the end equal to the start, one millisecond before it, and a whole day
    // before it.
    let rows: [(&str, &str, &str); 3] = [
        ("an empty window", SAMPLES[1], SAMPLES[1]),
        (
            "an end one millisecond before the start",
            "2024-03-01T12:00:10.001Z",
            SAMPLES[1],
        ),
        (
            "an end a day before the start",
            SAMPLES[1],
            "2024-02-29T12:00:10Z",
        ),
    ];

    for (case, start, end) in rows {
        let answer = history(
            &router,
            TRACKER,
            DEVICE,
            json!({
                "StartTimeInclusive": wire_time(start),
                "EndTimeExclusive": wire_time(end),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            "'EndTimeExclusive' must be after 'StartTimeInclusive'.",
            "case {case:?}: expected the inverted-window sentence, got {:?}",
            answer.message()
        );
        let field_names: Vec<String> = answer
            .json()
            .get("fieldList")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            field_names,
            vec!["EndTimeExclusive".to_string()],
            "case {case:?}: expected fieldList to name EndTimeExclusive, got {field_names:?}"
        );
    }

    // The refused reads changed nothing: a read over the whole window still reports
    // every stored sample.
    let after = history(
        &router,
        TRACKER,
        DEVICE,
        json!({
            "StartTimeInclusive": wire_time(SAMPLES[0]),
            "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
        }),
    )
    .await;
    let reported = reported_sample_times(&after);
    assert_eq!(
        reported, all,
        "a refused read must leave the history unchanged, expected {all:?}, got {reported:?}"
    );
}

/// RULE: a `NextToken` this service did not mint is refused with a
/// `ValidationException` rather than read as a page key.
#[tokio::test]
async fn a_cursor_this_service_did_not_mint_is_refused() {
    let router = emulator_with_samples().await;

    // Rows span the forms a bad cursor takes: another collection's cursor, an
    // untagged key, a tagged key that is not an instant, and a tag with no key.
    let rows: [(&str, &str); 5] = [
        ("another list operation's cursor", "trackers:history-tracker"),
        ("an untagged key", "1709294400000"),
        ("a tagged key that is not an instant", "device-positions:noon"),
        ("a tag with no key", "device-positions:"),
        ("a client-invented string", "page-2"),
    ];

    for (case, token) in rows {
        let answer = history(
            &router,
            TRACKER,
            DEVICE,
            json!({
                "StartTimeInclusive": wire_time(SAMPLES[0]),
                "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
                "NextToken": token,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error_code()
        );
    }
}

/// RULE: a history read resolves its tracker as a named resource — an unknown one is
/// a `ResourceNotFoundException` — while a device the tracker holds no samples for is
/// an EMPTY window, not a missing resource: the read is a query over a time range.
#[tokio::test]
async fn an_unknown_tracker_is_not_found_and_a_sampleless_device_is_empty() {
    let router = emulator_with_samples().await;

    let missing = history(&router, "no-such-tracker", DEVICE, json!({})).await;
    assert_eq!(
        missing.status, 404,
        "an unknown tracker: expected 404, got {} {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code(),
        "ResourceNotFoundException",
        "an unknown tracker: expected ResourceNotFoundException, got {:?}",
        missing.error_code()
    );
    assert_eq!(
        missing.message(),
        "Tracker not found: no-such-tracker.",
        "an unknown tracker: expected the tracker-not-found sentence, got {:?}",
        missing.message()
    );

    for device in ["never.sampled", "other-device"] {
        let answer = history(
            &router,
            TRACKER,
            device,
            json!({
                "StartTimeInclusive": wire_time(SAMPLES[0]),
                "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "device {device:?}: expected 200, got {} {}",
            answer.status, answer.body
        );
        let reported = reported_sample_times(&answer);
        assert_eq!(
            reported,
            Vec::<i64>::new(),
            "device {device:?}: a device with no samples must read an empty history, got \
             {reported:?}"
        );
        assert_eq!(
            next_token(&answer),
            None,
            "device {device:?}: an empty history must carry no NextToken, got {}",
            answer.body
        );
    }
}

/// RULE: a device's history holds ONE sample per instant — an update for an instant the
/// device already holds replaces it ([`state::Tracker::record_position`]) — so a page
/// boundary cannot fall inside a group of equal sample times and the inclusive cursor
/// cannot re-emit one.
#[tokio::test]
async fn one_sample_per_instant_survives_pagination() {
    let router = emulator();
    create_tracker(&router, TRACKER).await;

    let instant = "2024-03-01T12:00:00Z";
    // Two updates at the SAME instant in one batch, then a third at that instant in a
    // later request: the newest write is the one the device holds, and it holds one.
    let seeded = batch_update(
        &router,
        TRACKER,
        json!([
            { "DeviceId": DEVICE, "SampleTime": wire_time(instant), "Position": [-122.30, 47.60] },
            { "DeviceId": DEVICE, "SampleTime": wire_time(instant), "Position": [-122.31, 47.61] },
            { "DeviceId": DEVICE, "SampleTime": wire_time(SAMPLES[1]), "Position": [-122.32, 47.62] },
        ]),
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "seeding two updates at one instant: expected 200, got {} {}",
        seeded.status, seeded.body
    );
    let replaced = batch_update(
        &router,
        TRACKER,
        json!([
            { "DeviceId": DEVICE, "SampleTime": wire_time(instant), "Position": [-122.33, 47.63] },
        ]),
    )
    .await;
    assert_eq!(
        replaced.status, 200,
        "replacing the sample at that instant: expected 200, got {} {}",
        replaced.status, replaced.body
    );

    // One sample per page, walked to exhaustion: each instant appears exactly once.
    let mut walked = Vec::new();
    let mut positions = Vec::new();
    let mut token: Option<String> = None;
    for round in 0..4 {
        let mut body = json!({
            "StartTimeInclusive": wire_time(instant),
            "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
            "MaxResults": 1,
        });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let answer = history(&router, TRACKER, DEVICE, body).await;
        assert_eq!(
            answer.status, 200,
            "page {round}: expected 200, got {} {}",
            answer.status, answer.body
        );
        walked.extend(reported_sample_times(&answer));
        if let Some(first) = answer
            .json()
            .get("DevicePositions")
            .and_then(Value::as_array)
            .and_then(|page| page.first())
            .and_then(|sample| sample.get("Position").cloned())
        {
            positions.push(first);
        }
        token = next_token(&answer);
        if token.is_none() {
            break;
        }
    }

    assert_eq!(
        walked,
        vec![iso_millis(instant), iso_millis(SAMPLES[1])],
        "equal sample times: the walk must report each instant exactly once, got {walked:?}"
    );
    assert_eq!(
        positions.first(),
        Some(&json!([-122.33, 47.63])),
        "equal sample times: the instant must hold the position written LAST, got {positions:?}"
    );
}

/// RULE: every member of a reported `DevicePosition` is the stored sample's, so the
/// history reports the same sample the latest-position read does — the position, the
/// accuracy and the properties that were written, plus the service-stamped
/// `ReceivedTime`.
#[tokio::test]
async fn a_reported_sample_carries_every_stored_member() {
    let router = emulator();
    create_tracker(&router, TRACKER).await;

    let sample_time = "2024-03-01T12:00:00Z";
    let seeded = batch_update(
        &router,
        TRACKER,
        json!([{
            "DeviceId": DEVICE,
            "SampleTime": wire_time(sample_time),
            "Position": [-122.33, 47.60],
            "Accuracy": { "Horizontal": 12.5 },
            "PositionProperties": { "driver": "ada", "shift": "night" },
        }]),
    )
    .await;
    assert_eq!(
        seeded.status, 200,
        "seeding one fully populated sample: expected 200, got {} {}",
        seeded.status, seeded.body
    );

    let answer = history(
        &router,
        TRACKER,
        DEVICE,
        json!({
            "StartTimeInclusive": wire_time(sample_time),
            "EndTimeExclusive": wire_time("2024-03-01T12:01:00Z"),
        }),
    )
    .await;
    let body = answer.json();
    let reported = body
        .get("DevicePositions")
        .and_then(Value::as_array)
        .and_then(|positions| positions.first())
        .unwrap_or_else(|| panic!("the history must report the stored sample, got {body}"));

    assert_eq!(
        reported.get("DeviceId").and_then(Value::as_str),
        Some(DEVICE),
        "the reported DeviceId must be the stored one, got {reported}"
    );
    assert_eq!(
        millis_of(reported, "SampleTime"),
        iso_millis(sample_time),
        "the reported SampleTime must be the stored one, got {reported}"
    );
    assert_eq!(
        reported.get("Position"),
        Some(&json!([-122.33, 47.60])),
        "the reported Position must be the stored one, got {reported}"
    );
    assert_eq!(
        reported
            .get("Accuracy")
            .and_then(|accuracy| accuracy.get("Horizontal")),
        Some(&json!(12.5)),
        "the reported Accuracy must be the stored one, got {reported}"
    );
    assert_eq!(
        reported.get("PositionProperties"),
        Some(&json!({ "driver": "ada", "shift": "night" })),
        "the reported PositionProperties must be the stored ones, got {reported}"
    );

    // `ReceivedTime` is stamped by the service when it accepted the update, so it is
    // at or after the sample instant and is reported, not echoed.
    let received = millis_of(reported, "ReceivedTime");
    assert!(
        received >= iso_millis(sample_time),
        "the reported ReceivedTime {received} must be at or after the sample instant {}",
        iso_millis(sample_time)
    );

    // The same sample, through the latest-position read: both projections agree.
    let latest = call(
        &router,
        "GET",
        &format!("/tracking/v0/trackers/{TRACKER}/devices/{DEVICE}/positions/latest"),
        None,
    )
    .await;
    let latest_body = latest.json();
    for member in ["DeviceId", "SampleTime", "ReceivedTime", "Position"] {
        assert_eq!(
            latest_body.get(member),
            reported.get(member),
            "member {member:?}: the history and the latest-position read must report the \
             same stored sample, got {:?} and {:?}",
            latest_body.get(member),
            reported.get(member)
        );
    }
}
