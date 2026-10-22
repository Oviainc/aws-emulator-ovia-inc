// SPDX-License-Identifier: CC0-1.0

//! `ListProfileTimes`, end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: `ListProfileTimes` reports the start instants of
//! the aggregated profiles a stored group holds for the requested aggregation period —
//! each posted agent profile belongs to the one period-length window, aligned to UTC,
//! that contains its receipt time, and every such window holding at least one profile
//! is reported once, filtered to the requested time range, ordered by `orderBy`
//! (descending by default) and paged by `maxResults`/`nextToken`.
//!
//! Rows vary what the rule generalises over: the aggregation period (all three the
//! model declares), how many profiles share a window, where a receipt sits inside its
//! window, the requested range, the order, the page size and the profile count —
//! including the group that has no profiles at all, which is the recorded corpus case
//! `list-profile-times-empty-before-any-data`.
//!
//! The emulator, the HTTP plumbing and the response reducer are the shared
//! [`crate::profiling_group_lifecycle`] harness, not a fork of it.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;

use crate::profiling_group_lifecycle::{Answer, Harness};

/// 2020-06-01T00:00:00Z, the reference instant these cases place receipts around. It
/// is a whole number of days past the epoch, so a window aligned to UTC starts here
/// for every one of the three aggregation periods.
const DAY_START: i64 = 1_590_969_600;

/// One day in seconds, for readable receipt and range arithmetic.
const DAY: i64 = 24 * 60 * 60;

/// One hour in seconds.
const HOUR: i64 = 60 * 60;

/// An instant as the ISO 8601 form the modelled `date-time` query members use.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// The reported profile times, in the order the response lists them, as epoch seconds.
///
/// restJson1 carries a `Timestamp` member either as epoch seconds or in the ISO 8601
/// form, depending on the member's `timestampFormat`; these cases are about WHICH
/// instant is reported, so both renderings are read back the same way.
fn profile_times(answer: &Answer) -> Vec<i64> {
    let json = answer.json();
    let times = json
        .get("profileTimes")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no profileTimes array in {json:?}"))
        .clone();
    times
        .iter()
        .map(|entry| {
            let start = entry
                .get("start")
                .unwrap_or_else(|| panic!("no start in profile time {entry:?}"));
            match start {
                serde_json::Value::String(text) => DateTime::from_str(text, Format::DateTime)
                    .unwrap_or_else(|error| panic!("unparseable start {text:?}: {error}"))
                    .as_secs_f64() as i64,
                serde_json::Value::Number(seconds) => seconds
                    .as_f64()
                    .unwrap_or_else(|| panic!("unparseable start {seconds:?}")) as i64,
                other => panic!("start is neither a timestamp string nor seconds: {other:?}"),
            }
        })
        .collect()
}

fn next_token(answer: &Answer) -> Option<String> {
    answer
        .json()
        .get("nextToken")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
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

async fn list_profile_times(harness: &Harness, name: &str, query: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/profilingGroups/{name}/profileTimes{query}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// Place the group's stored profiles at `receipts` (epoch seconds), oldest first.
///
/// Posting over HTTP stamps each profile with the clock, so a case that needs profiles
/// spread over hours and days — which is what an aggregation period is about — sets
/// their receipt times in the store afterwards. This writes the same fields
/// `PostAgentProfile` wrote, through the harness's one shared engine, and nothing else.
fn place_receipts(harness: &Harness, name: &str, receipts: &[i64]) {
    let state = harness
        .engine
        .state::<service_com_amazonaws_codeguruprofiler_codeguruprofiler::ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
    let mut guard = state.lock();
    let group = guard
        .groups
        .get_mut(name)
        .unwrap_or_else(|| panic!("no stored group named {name}"));
    assert_eq!(
        group.profiles.len(),
        receipts.len(),
        "group {name} holds {} profiles, not the {} receipts to place",
        group.profiles.len(),
        receipts.len()
    );
    for (profile, at) in group.profiles.iter_mut().zip(receipts) {
        profile.received_at = DateTime::from_secs(*at);
    }
    group.latest_agent_profile_reported_at =
        receipts.iter().max().map(|at| DateTime::from_secs(*at));
}

/// A group holding one posted profile per receipt time, each with its own payload;
/// answers the payloads, in posting order.
async fn group_with_receipts(harness: &Harness, name: &str, receipts: &[i64]) -> Vec<Vec<u8>> {
    assert_eq!(
        create_group(harness, name).await.status,
        201,
        "creating group {name} must succeed"
    );
    let mut payloads = Vec::new();
    for index in 0..receipts.len() {
        let payload = format!("{{\"profile\":\"{name}-{index}\"}}").into_bytes();
        assert_eq!(
            harness
                .post_agent_profile(name, "application/json", None, payload.clone())
                .await
                .status,
            204,
            "posting profile {index} to {name} must succeed"
        );
        payloads.push(payload);
    }
    place_receipts(harness, name, receipts);
    payloads
}

/// One `ListProfileTimes` request against a group holding `receipts`, and the profile
/// times it must answer with.
#[derive(Debug)]
struct Case {
    label: &'static str,
    /// Receipt times of the group's posted profiles, epoch seconds.
    receipts: Vec<i64>,
    /// A modelled `AggregationPeriod` value (`AGGREGATION_PERIOD_VALUES`: P1D, PT1H,
    /// PT5M).
    period: &'static str,
    range: (i64, i64),
    /// A modelled `OrderBy` value (`ORDER_BY_VALUES`), or none for the default.
    order_by: Option<&'static str>,
    max_results: Option<i32>,
    /// The profile times the response must carry, in order, as epoch seconds.
    expected: Vec<i64>,
    /// Whether the response must carry a `nextToken`.
    expect_next: bool,
}

impl Case {
    fn query(&self) -> String {
        let (start, end) = self.range;
        let mut query = format!(
            "?startTime={}&endTime={}&period={}",
            iso(start),
            iso(end),
            self.period
        );
        if let Some(order_by) = self.order_by {
            query.push_str(&format!("&orderBy={order_by}"));
        }
        if let Some(max_results) = self.max_results {
            query.push_str(&format!("&maxResults={max_results}"));
        }
        query
    }
}

/// The table. Rows differ in aggregation period, how receipts share a window, where a
/// receipt sits inside its window, the requested range, the order and the page size.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "five-minute windows: profiles in one window report one start",
            receipts: vec![
                DAY_START,
                DAY_START + 60,
                DAY_START + 299,
                DAY_START + 300,
                DAY_START + 1800,
            ],
            period: "PT5M",
            range: (DAY_START, DAY_START + HOUR),
            order_by: None,
            max_results: None,
            expected: vec![DAY_START + 1800, DAY_START + 300, DAY_START],
            expect_next: false,
        },
        Case {
            label: "the same profiles over an hourly period share one window",
            receipts: vec![
                DAY_START,
                DAY_START + 60,
                DAY_START + 299,
                DAY_START + 300,
                DAY_START + 1800,
            ],
            period: "PT1H",
            range: (DAY_START, DAY_START + HOUR),
            order_by: None,
            max_results: None,
            expected: vec![DAY_START],
            expect_next: false,
        },
        Case {
            label: "the same profiles over a daily period share one window",
            receipts: vec![
                DAY_START,
                DAY_START + 60,
                DAY_START + 299,
                DAY_START + 300,
                DAY_START + 1800,
            ],
            period: "P1D",
            range: (DAY_START, DAY_START + DAY),
            order_by: None,
            max_results: None,
            expected: vec![DAY_START],
            expect_next: false,
        },
        Case {
            label: "TimestampAscending reverses the default order",
            receipts: vec![DAY_START, DAY_START + 300, DAY_START + 1800],
            period: "PT5M",
            range: (DAY_START, DAY_START + HOUR),
            order_by: Some("TimestampAscending"),
            max_results: None,
            expected: vec![DAY_START, DAY_START + 300, DAY_START + 1800],
            expect_next: false,
        },
        Case {
            label: "TimestampDescending named explicitly is the default order",
            receipts: vec![DAY_START, DAY_START + 300, DAY_START + 1800],
            period: "PT5M",
            range: (DAY_START, DAY_START + HOUR),
            order_by: Some("TimestampDescending"),
            max_results: None,
            expected: vec![DAY_START + 1800, DAY_START + 300, DAY_START],
            expect_next: false,
        },
        Case {
            label: "the requested range keeps only the windows it covers",
            receipts: vec![DAY_START, DAY_START + HOUR + 100, DAY_START + 2 * HOUR + 100],
            period: "PT1H",
            range: (DAY_START + HOUR, DAY_START + 2 * HOUR),
            order_by: None,
            max_results: None,
            expected: vec![DAY_START + HOUR],
            expect_next: false,
        },
        Case {
            label: "a window starting exactly at startTime is reported",
            receipts: vec![DAY_START, DAY_START + HOUR + 100],
            period: "PT1H",
            range: (DAY_START + HOUR, DAY_START + 2 * HOUR),
            order_by: Some("TimestampAscending"),
            max_results: None,
            expected: vec![DAY_START + HOUR],
            expect_next: false,
        },
        Case {
            label: "daily windows over three days",
            receipts: vec![
                DAY_START + 10,
                DAY_START + DAY + 100,
                DAY_START + 2 * DAY + 200,
            ],
            period: "P1D",
            range: (DAY_START, DAY_START + 3 * DAY),
            order_by: None,
            max_results: None,
            expected: vec![DAY_START + 2 * DAY, DAY_START + DAY, DAY_START],
            expect_next: false,
        },
        Case {
            label: "a receipt late in its window reports the window start, not the receipt",
            receipts: vec![DAY_START + DAY - 1],
            period: "PT1H",
            range: (DAY_START, DAY_START + DAY),
            order_by: None,
            max_results: None,
            expected: vec![DAY_START + 23 * HOUR],
            expect_next: false,
        },
        Case {
            label: "maxResults bounds the page and hands back a token",
            receipts: vec![DAY_START, DAY_START + 300, DAY_START + 600],
            period: "PT5M",
            range: (DAY_START, DAY_START + HOUR),
            order_by: Some("TimestampAscending"),
            max_results: Some(1),
            expected: vec![DAY_START],
            expect_next: true,
        },
        Case {
            // `maxResults`'s published upper bound (the model: range 1..1000).
            label: "maxResults at its upper bound returns the whole list",
            receipts: vec![DAY_START, DAY_START + 300],
            period: "PT5M",
            range: (DAY_START, DAY_START + HOUR),
            order_by: None,
            max_results: Some(1000),
            expected: vec![DAY_START + 300, DAY_START],
            expect_next: false,
        },
        Case {
            label: "a group with no posted profiles has no profile times",
            receipts: Vec::new(),
            period: "PT5M",
            range: (DAY_START, DAY_START + HOUR),
            order_by: None,
            max_results: None,
            expected: Vec::new(),
            expect_next: false,
        },
        Case {
            label: "a range holding none of the group's windows is empty",
            receipts: vec![DAY_START],
            period: "PT5M",
            range: (DAY_START + DAY, DAY_START + 2 * DAY),
            order_by: None,
            max_results: None,
            expected: Vec::new(),
            expect_next: false,
        },
        Case {
            label: "an empty range (startTime == endTime) is empty",
            receipts: vec![DAY_START],
            period: "PT5M",
            range: (DAY_START, DAY_START),
            order_by: None,
            max_results: None,
            expected: Vec::new(),
            expect_next: false,
        },
        Case {
            label: "a pre-epoch receipt floors to the start of its window",
            receipts: vec![-1],
            period: "PT1H",
            range: (-2 * HOUR, 0),
            order_by: None,
            max_results: None,
            expected: vec![-HOUR],
            expect_next: false,
        },
    ]
}

#[tokio::test]
async fn profile_times_are_the_aggregation_windows_of_the_stored_profiles() {
    let harness = Harness::new();

    for (index, case) in cases().iter().enumerate() {
        let name = format!("times-case-{index}");
        group_with_receipts(&harness, &name, &case.receipts).await;

        let answer = list_profile_times(&harness, &name, &case.query()).await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected HTTP 200, got {} {}",
            case.label,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            profile_times(&answer),
            case.expected,
            "case {:?} ({}): expected profile times {:?}, got {:?}",
            case.label,
            case.query(),
            case.expected,
            profile_times(&answer)
        );
        assert_eq!(
            next_token(&answer).is_some(),
            case.expect_next,
            "case {:?}: expected nextToken present = {}, got {:?}",
            case.label,
            case.expect_next,
            next_token(&answer)
        );

        // Listing is a read: a second identical call answers identically.
        let again = list_profile_times(&harness, &name, &case.query()).await;
        assert_eq!(
            profile_times(&again),
            profile_times(&answer),
            "case {:?}: a repeated list answered {:?} after {:?}",
            case.label,
            profile_times(&again),
            profile_times(&answer)
        );
    }
}

#[tokio::test]
async fn paging_visits_every_profile_time_exactly_once() {
    let harness = Harness::new();
    // Five distinct five-minute windows, plus a second profile inside one of them so
    // the page walk is over windows and not over posted profiles.
    let receipts = vec![
        DAY_START,
        DAY_START + 60,
        DAY_START + 300,
        DAY_START + 600,
        DAY_START + 900,
        DAY_START + 1200,
    ];
    let name = "times-paging";
    group_with_receipts(&harness, name, &receipts).await;
    let all: Vec<i64> = vec![
        DAY_START,
        DAY_START + 300,
        DAY_START + 600,
        DAY_START + 900,
        DAY_START + 1200,
    ];

    for page_size in [1usize, 2, 3, 5, 7] {
        let mut seen: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!(
                "?startTime={}&endTime={}&period=PT5M&orderBy=TimestampAscending&maxResults={page_size}",
                iso(DAY_START),
                iso(DAY_START + HOUR)
            );
            if let Some(token) = token.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let answer = list_profile_times(&harness, name, &query).await;
            assert_eq!(
                answer.status,
                200,
                "page size {page_size}: expected HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let page = profile_times(&answer);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page carried {} profile times",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 2,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            match next_token(&answer) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "page size {page_size}: paging must visit every profile time once, in order; \
             got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_reported_profile_time_names_a_window_get_profile_can_serve() {
    let harness = Harness::new();
    // Receipts kept away from the window boundaries, so each window holds exactly the
    // profiles this test expects it to.
    let receipts = vec![
        DAY_START + 30,
        DAY_START + 120,
        DAY_START + 330,
        DAY_START + 930,
    ];
    let name = "times-roundtrip";
    let payloads = group_with_receipts(&harness, name, &receipts).await;

    let listed = list_profile_times(
        &harness,
        name,
        &format!(
            "?startTime={}&endTime={}&period=PT5M&orderBy=TimestampAscending",
            iso(DAY_START),
            iso(DAY_START + HOUR)
        ),
    )
    .await;
    assert_eq!(listed.status, 200, "listing must succeed");
    let times = profile_times(&listed);
    assert_eq!(
        times,
        vec![DAY_START, DAY_START + 300, DAY_START + 900],
        "the three five-minute windows the receipts fall in, got {times:?}"
    );

    for start in times {
        // The newest profile whose receipt falls in this window is the one the window's
        // aggregated profile is built from, and the one `GetProfile` must serve for it.
        let expected = receipts
            .iter()
            .enumerate()
            .filter(|(_, at)| **at >= start && **at < start + 300)
            .map(|(index, _)| payloads[index].clone())
            .next_back()
            .unwrap_or_else(|| panic!("window {start} was reported with no profile in it"));
        let answer = harness
            .get_profile(name, &format!("?startTime={}&period=PT5M", iso(start)))
            .await;
        assert_eq!(
            answer.status,
            200,
            "window {start}: GetProfile must serve a reported profile time, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.body,
            expected,
            "window {start}: expected the profile posted in that window, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

#[tokio::test]
async fn every_modelled_aggregation_period_is_served() {
    // The source of the catalog: the generated `AGGREGATION_PERIOD_VALUES`, so a model
    // that gains or renames an `AggregationPeriod` value cannot leave this operation
    // silently refusing one the model declares.
    use service_com_amazonaws_codeguruprofiler_codeguruprofiler::ops::types::AGGREGATION_PERIOD_VALUES;

    let harness = Harness::new();
    let name = "times-every-period";
    group_with_receipts(&harness, name, &[DAY_START + 42]).await;

    for period in AGGREGATION_PERIOD_VALUES {
        let answer = list_profile_times(
            &harness,
            name,
            &format!(
                "?startTime={}&endTime={}&period={period}",
                iso(DAY_START),
                iso(DAY_START + DAY)
            ),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "period {period}: a modelled AggregationPeriod must be served, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        // Whatever the period, the one posted profile falls in exactly one window of
        // it, and that window starts at or before the receipt.
        let times = profile_times(&answer);
        assert_eq!(
            times.len(),
            1,
            "period {period}: one posted profile is one aggregated profile, got {times:?}"
        );
        assert!(
            times[0] <= DAY_START + 42,
            "period {period}: the window must start at or before the receipt, got {times:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_group_is_not_found() {
    let harness = Harness::new();
    group_with_receipts(&harness, "times-known", &[DAY_START]).await;

    let answer = list_profile_times(
        &harness,
        "times-missing",
        &format!(
            "?startTime={}&endTime={}&period=PT5M",
            iso(DAY_START),
            iso(DAY_START + HOUR)
        ),
    )
    .await;
    assert_eq!(
        answer.status,
        404,
        "an unknown group must be a 404, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "an unknown group must report ResourceNotFoundException, got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

#[tokio::test]
async fn requests_outside_the_operation_s_rules_are_refused() {
    let harness = Harness::new();
    let name = "times-refusals";
    group_with_receipts(&harness, name, &[DAY_START, DAY_START + 300]).await;

    let range = format!(
        "startTime={}&endTime={}",
        iso(DAY_START),
        iso(DAY_START + HOUR)
    );
    // Rows: each way a request can be outside what the operation accepts. The
    // `maxResults` rows are the model's published range bounds (1..1000) and one past
    // each; `period` and `orderBy` are the values the model's enums declare.
    let refused: Vec<(&str, String)> = vec![
        (
            "endTime before startTime",
            format!(
                "?startTime={}&endTime={}&period=PT5M",
                iso(DAY_START + HOUR),
                iso(DAY_START)
            ),
        ),
        (
            "a period outside AggregationPeriod",
            format!("?{range}&period=PT10M"),
        ),
        (
            "an orderBy outside OrderBy",
            format!("?{range}&period=PT5M&orderBy=TimestampSideways"),
        ),
        (
            "a nextToken this service never issued",
            format!("?{range}&period=PT5M&nextToken=not-a-cursor"),
        ),
        (
            "a nextToken past the end of the list",
            format!("?{range}&period=PT5M&nextToken=99"),
        ),
        (
            // The group holds two five-minute windows, so a cursor of 2 points exactly
            // at the end — a position this service never issues a token for.
            "a nextToken exactly at the end of the list",
            format!("?{range}&period=PT5M&nextToken=2"),
        ),
        (
            "maxResults below its lower bound",
            format!("?{range}&period=PT5M&maxResults=0"),
        ),
        (
            "maxResults above its upper bound",
            format!("?{range}&period=PT5M&maxResults=1001"),
        ),
    ];

    for (label, query) in refused {
        let answer = list_profile_times(&harness, name, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {label:?} ({query}): expected HTTP 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label:?} ({query}): expected ValidationException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    // A refusal, like a successful list, changed nothing: the group still reports both
    // of its five-minute windows.
    let answer = list_profile_times(&harness, name, &format!("?{range}&period=PT5M")).await;
    assert_eq!(
        profile_times(&answer),
        vec![DAY_START + 300, DAY_START],
        "refused requests must not change what a later list sees, got {}",
        String::from_utf8_lossy(&answer.body)
    );

    // `maxResults` at each published bound is accepted.
    for max_results in [1, 1000] {
        let answer = list_profile_times(
            &harness,
            name,
            &format!("?{range}&period=PT5M&maxResults={max_results}"),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "maxResults={max_results} is inside the model's range and must be accepted, \
             got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}
