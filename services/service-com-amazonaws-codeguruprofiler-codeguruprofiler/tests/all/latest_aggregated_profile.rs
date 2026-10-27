// SPDX-License-Identifier: CC0-1.0

//! What a profiling group's `profilingStatus.latestAggregatedProfile` reports, end to end
//! over HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: a read of a stored group reports, as that group's
//! most recent aggregated profile, the aggregation period and start instant of the
//! aggregated profile that starts latest among those the group holds — the narrowest
//! period-length window aligned to UTC that contains the group's latest receipt whose
//! aggregation has fallen due — and reports nothing there while no aggregation has.
//!
//! Rows vary what the rule generalises over: where a receipt sits inside its window (on a
//! boundary every period shares, part-way in, a fraction of a second before the next
//! window), the era of the instant (before the epoch, the epoch itself, 2020, far
//! future), how many profiles the group holds and whether the latest was posted last, how
//! far apart their windows are (same window, hours, days), and whether any aggregation is
//! due at all.
//!
//! Every read path of a group description is driven on the same stored state
//! (`DescribeProfilingGroup`, the described form of `ListProfilingGroups`,
//! `UpdateProfilingGroup` and a `CreateProfilingGroup` replayed on its `clientToken`), and
//! the reported pair is cross-checked against `ListProfileTimes`, the other reader of the
//! same aggregated profiles.
//!
//! The emulator and the HTTP plumbing are the shared [`crate::profiling_group_lifecycle`]
//! harness, and receipts are placed through [`crate::profile_times::place_receipts_at`],
//! not through a fork of either.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::profile_times::place_receipts_at;
use crate::profiling_group_lifecycle::{Answer, Harness};

/// 2020-06-01T00:00:00Z. A whole number of days past the epoch, so a window aligned to
/// UTC starts here for each of the three aggregation periods at once.
const DAY_START: i64 = 1_590_969_600;

/// One day in seconds, for readable receipt arithmetic.
const DAY: i64 = 24 * 60 * 60;

/// One hour in seconds.
const HOUR: i64 = 60 * 60;

/// Five minutes in seconds — the length the model's `AggregationPeriod` doc states for
/// `PT5M` ("PT5M — 5 minutes"), the narrowest of the three periods it declares (`P1D` —
/// 1 day, `PT1H` — 1 hour, `PT5M` — 5 minutes).
const FIVE_MINUTES: i64 = 5 * 60;

/// An instant as the ISO 8601 form every modelled `date-time` member of this service
/// carries.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// A window so wide it contains every instant these rows place a receipt at, as the two
/// bounds `ListProfileTimes` takes.
const WIDE_START: &str = "1960-01-01T00:00:00Z";
const WIDE_END: &str = "2200-01-01T00:00:00Z";

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

async fn update_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "PUT",
            &format!("/profilingGroups/{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "agentOrchestrationConfig": { "profilingEnabled": true }
            }))
            .expect("a serializable body"),
        )
        .await
}

async fn list_described(harness: &Harness) -> Answer {
    harness
        .send(
            "GET",
            "/profilingGroups?includeDescription=true",
            "application/json",
            Vec::new(),
        )
        .await
}

async fn list_profile_times(harness: &Harness, name: &str, period: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!(
                "/profilingGroups/{name}/profileTimes?startTime={WIDE_START}&endTime={WIDE_END}\
                 &period={period}&orderBy=TimestampAscending"
            ),
            "application/json",
            Vec::new(),
        )
        .await
}

/// The start instants `ListProfileTimes` reports for one period, as epoch seconds in the
/// order the response lists them.
fn reported_profile_times(answer: &Answer) -> Vec<i64> {
    let json = answer.json();
    json.get("profileTimes")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no profileTimes array in {json}"))
        .iter()
        .map(|entry| {
            let start = entry
                .get("start")
                .unwrap_or_else(|| panic!("no start in profile time {entry}"));
            let text = start
                .as_str()
                .unwrap_or_else(|| panic!("start must be a date-time string, got {start}"));
            DateTime::from_str(text, Format::DateTime)
                .unwrap_or_else(|error| panic!("unparseable start {text:?}: {error}"))
                .secs()
        })
        .collect()
}

/// The `(period, start)` pair one description reports as its most recent aggregated
/// profile, or `None` when it reports none. `start` is answered as epoch seconds, after
/// the member has been asserted to be a date-time string carrying no sub-second part —
/// an aggregation window starts on a whole second.
fn latest_aggregated_profile(case: &str, description: &serde_json::Value) -> Option<(String, i64)> {
    let status = description
        .get("profilingStatus")
        .unwrap_or_else(|| panic!("case {case:?}: a read must carry profilingStatus, got {description}"));
    let aggregated = status.get("latestAggregatedProfile")?;
    let period = aggregated
        .get("period")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no period in {aggregated}"))
        .to_string();
    let start_value = aggregated
        .get("start")
        .unwrap_or_else(|| panic!("case {case:?}: no start in {aggregated}"));
    let text = start_value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: start must be a date-time string, got {start_value}"));
    let start = DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: unparseable start {text:?}: {error}"));
    assert_eq!(
        start.subsec_nanos(),
        0,
        "case {case:?}: an aggregation window starts on a whole second, got {text}"
    );
    Some((period, start.secs()))
}

/// One row: the receipts a group is placed at, and the aggregated profile its status must
/// then report.
#[derive(Debug)]
struct Row {
    label: &'static str,
    /// The stored receipt instants, (epoch seconds, sub-second nanoseconds), in posting
    /// order — so a row can post the latest profile FIRST.
    receipts: Vec<(i64, u32)>,
    /// The `(period, start)` the status must report, or `None` when it must report none.
    expected: Option<(&'static str, i64)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no profiles at all",
            receipts: Vec::new(),
            expected: None,
        },
        Row {
            // Every period's window starts at this instant, so the narrowest of them is
            // what the status reports.
            label: "one receipt on a boundary all three periods share",
            receipts: vec![(DAY_START, 0)],
            expected: Some(("PT5M", DAY_START)),
        },
        Row {
            label: "one receipt part-way into its window",
            receipts: vec![(DAY_START + 7 * FIVE_MINUTES + 42, 0)],
            expected: Some(("PT5M", DAY_START + 7 * FIVE_MINUTES)),
        },
        Row {
            label: "one receipt a nanosecond before the next window",
            receipts: vec![(DAY_START + FIVE_MINUTES - 1, 999_999_999)],
            expected: Some(("PT5M", DAY_START)),
        },
        Row {
            label: "two receipts in the same window",
            receipts: vec![(DAY_START + 3 * FIVE_MINUTES + 1, 0), (DAY_START + 4 * FIVE_MINUTES - 1, 0)],
            expected: Some(("PT5M", DAY_START + 3 * FIVE_MINUTES)),
        },
        Row {
            label: "receipts an hour apart",
            receipts: vec![(DAY_START, 0), (DAY_START + HOUR + 2 * FIVE_MINUTES + 7, 0)],
            expected: Some(("PT5M", DAY_START + HOUR + 2 * FIVE_MINUTES)),
        },
        Row {
            // The latest receipt is the FIRST profile the group was posted, so the status
            // reports the most recent aggregated profile rather than the last stored one.
            label: "the latest receipt was posted first",
            receipts: vec![(DAY_START + 2 * DAY + HOUR, 0), (DAY_START, 0), (DAY_START + DAY, 0)],
            expected: Some(("PT5M", DAY_START + 2 * DAY + HOUR)),
        },
        Row {
            label: "receipts spread over days, the latest near a day's end",
            receipts: vec![
                (DAY_START - DAY, 0),
                (DAY_START, 0),
                (DAY_START + 2 * DAY + 23 * HOUR + 59 * 60 + 59, 0),
            ],
            expected: Some((
                "PT5M",
                DAY_START + 2 * DAY + 23 * HOUR + 55 * 60,
            )),
        },
        Row {
            // Its window has not closed yet, so nothing has aggregated it: the status
            // reports no aggregated profile even though the receipt is recorded.
            label: "a receipt in the far future",
            receipts: vec![(4_102_444_800, 0)],
            expected: None,
        },
        Row {
            label: "the epoch itself",
            receipts: vec![(0, 0)],
            expected: Some(("PT5M", 0)),
        },
        Row {
            // Before the epoch the window start is the FLOOR, so it precedes the receipt
            // here too: -86_401 s falls in the window starting at -86_700 s.
            label: "a receipt before the epoch",
            receipts: vec![(-DAY - 1, 0)],
            expected: Some(("PT5M", -DAY - FIVE_MINUTES)),
        },
        Row {
            label: "a receipt before the epoch, on a shared boundary",
            receipts: vec![(-DAY, 0)],
            expected: Some(("PT5M", -DAY)),
        },
    ]
}

#[tokio::test]
async fn a_group_reports_the_latest_aggregated_profile_its_stored_receipts_derive() {
    for (index, row) in rows().iter().enumerate() {
        let harness = Harness::new();
        let name = format!("aggregated-{index}");
        let case = row.label;
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );

        for post in 0..row.receipts.len() {
            assert_eq!(
                harness
                    .post_agent_profile(
                        &name,
                        "application/json",
                        Some(&format!("ptok-{index}-{post}")),
                        format!("{{\"profile\":\"{name}-{post}\"}}").into_bytes(),
                    )
                    .await
                    .status,
                204,
                "case {case:?}: post {post} must succeed"
            );
        }
        if !row.receipts.is_empty() {
            place_receipts_at(&harness, &name, &row.receipts);
        }

        let expected = row
            .expected
            .map(|(period, start)| (period.to_string(), start));

        // Every path that reports a stored group's description reports the same pair.
        let described = harness.describe(&name).await;
        assert_eq!(described.status, 200, "case {case:?}: describe must answer 200");
        let listed = list_described(&harness).await;
        assert_eq!(listed.status, 200, "case {case:?}: list must answer 200");
        let listed_description = listed
            .json()
            .get("profilingGroups")
            .and_then(|groups| groups.get(0))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no described group in {}", listed.json()));
        let updated = update_group(&harness, &name).await;
        assert_eq!(updated.status, 200, "case {case:?}: update must answer 200");
        let replayed = create_group(&harness, &name).await;
        assert_eq!(
            replayed.status, 201,
            "case {case:?}: replaying the clientToken must answer 201, got {}",
            String::from_utf8_lossy(&replayed.body)
        );

        for (label, description) in [
            ("describe", described.json()),
            ("list(includeDescription=true)", listed_description),
            ("update", updated.json()),
            ("replayed create", replayed.json()),
        ] {
            // A replayed create omits the whole container while the group has no profiling
            // activity, which is the one path with nothing to read the member out of.
            if label == "replayed create" && description.get("profilingStatus").is_none() {
                assert!(
                    expected.is_none(),
                    "case {case:?} ({label}): a group with an aggregated profile must report \
                     profilingStatus, got {description}"
                );
                continue;
            }
            let reported = latest_aggregated_profile(case, &description);
            assert_eq!(
                reported, expected,
                "case {case:?} ({label}): wrong latestAggregatedProfile, got {description}"
            );
        }

        // The same aggregated profiles the other reader of them reports: the pair names the
        // LAST window `ListProfileTimes` reports for that period, ascending.
        if let Some((period, start)) = expected.as_ref() {
            let times = list_profile_times(&harness, &name, period).await;
            assert_eq!(
                times.status, 200,
                "case {case:?}: ListProfileTimes must answer 200, got {}",
                String::from_utf8_lossy(&times.body)
            );
            let reported = reported_profile_times(&times);
            assert_eq!(
                reported.last().copied(),
                Some(*start),
                "case {case:?}: the status must name the latest window {period} reports, got \
                 {reported:?}"
            );
        }

        // Reading the group twice reports the same pair: the member is derived from stored
        // receipts, so a read neither mints nor consumes it.
        let again = harness.describe(&name).await;
        assert_eq!(
            latest_aggregated_profile(case, &again.json()),
            expected,
            "case {case:?}: two reads must agree, got {}",
            String::from_utf8_lossy(&again.body)
        );
    }
}

#[tokio::test]
async fn a_pending_aggregation_is_no_aggregated_profile_until_it_falls_due() {
    // Rows: how many profiles the group holds and the payload each carries. None of them
    // is in an aggregated profile at the instant it is posted — the window it landed in is
    // still open — so the status reports no aggregated profile until the aggregation the
    // window owes has fallen due, and then reports exactly one window.
    for (index, posts) in [1usize, 2, 3].into_iter().enumerate() {
        let harness = Harness::new();
        let name = format!("pending-aggregated-{index}");
        let case = format!("{posts} posted profiles");
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {case:?}: create must succeed"
        );
        for post in 0..posts {
            assert_eq!(
                harness
                    .post_agent_profile(
                        &name,
                        "application/json",
                        Some(&format!("ptok-{index}-{post}")),
                        format!("{{\"profile\":\"{post}\"}}").into_bytes(),
                    )
                    .await
                    .status,
                204,
                "case {case:?}: post {post} must succeed"
            );
        }

        let pending = harness.describe(&name).await.json();
        assert!(
            pending.pointer("/profilingStatus/latestAgentProfileReportedAt").is_some(),
            "case {case:?}: the post must be recorded as received, got {pending}"
        );
        assert_eq!(
            latest_aggregated_profile(&case, &pending),
            None,
            "case {case:?}: a pending aggregation is no aggregated profile, got {pending}"
        );

        harness.complete_aggregation(&name);
        let due = harness.describe(&name).await.json();
        let (period, start) = latest_aggregated_profile(&case, &due).unwrap_or_else(|| {
            panic!("case {case:?}: a due aggregation must be reported, got {due}")
        });
        assert_eq!(
            period, "PT5M",
            "case {case:?}: the narrowest period's window starts latest, got {due}"
        );
        // An aggregated profile's window has CLOSED — the service collects into it until
        // then — so the window the status names ended before the group's latest post.
        let posted_at = DateTime::from_str(
            due.pointer("/profilingStatus/latestAgentProfileReportedAt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no receipt instant in {due}")),
            Format::DateTime,
        )
        .unwrap_or_else(|error| panic!("case {case:?}: unparseable receipt: {error}"))
        .secs();
        assert!(
            start + FIVE_MINUTES <= posted_at,
            "case {case:?}: the reported window [{}] must have closed by [{}], got {due}",
            iso(start),
            iso(posted_at)
        );
        assert_eq!(
            start.rem_euclid(FIVE_MINUTES),
            0,
            "case {case:?}: the window must be aligned to UTC, got {}",
            iso(start)
        );
    }
}
