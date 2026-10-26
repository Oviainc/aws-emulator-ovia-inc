// SPDX-License-Identifier: CC0-1.0

//! `GetAwsNetworkPerformanceData` end-to-end: which series a request is answered with,
//! which metric points each series carries, what latency those points report, how the
//! answers page, and which requests are refused.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with. The XML is
//! read with `roxmltree`, the parser this crate's other suites use, because a data response
//! and the metric points inside it are both `<item>` lists.
//!
//! The rules under test:
//!
//! 1. The operation REPORTS one `DataResponse` per `DataQuery`, in the order the request
//!    gave them, each identifying the series its query named — `Id`, `Source`,
//!    `Destination`, `Metric`, `Statistic` and `Period` come back on the response ("A
//!    user-defined ID associated with a data query that's returned in the `dataResponse`
//!    identifying the query", `DataQuery$Id`).
//! 2. A series is reported with one metric point per aggregation period of the requested
//!    window — "the value is aggregated from the `startDate` to the `endDate`"
//!    (`MetricPoint`) — running from `StartTime`, the last one ending at `EndTime`, each
//!    carrying a positive round-trip latency in milliseconds and the status of a healthy
//!    window.
//! 3. The latency a series reports is the one the Amazon network between its two locations
//!    costs: it is the same whichever way round the pair is named, it is at its smallest
//!    between two locations of one Region, and it grows with the distance between the
//!    Regions named — and a location names the same place under all three forms EC2 gives
//!    it (a Region code, a zone name and a zone ID).
//! 4. `MaxResults`/`NextToken` page the data responses, each page holding at most
//!    `MaxResults` of them, and a walk answers every query exactly once.
//! 5. A request that names no series, no query or no window, or names something that
//!    cannot exist, is refused — never answered as a series that happens to be empty — and
//!    `DryRun` performs nothing.
//! 6. The operation is a read: it answers two identical calls identically.
//!
//! Each table spans the class its rule generalises over (the number of queries, the
//! aggregation period, the form of a location, the order of the queries, the length and
//! placement of the window, the distance between two Regions, the page size) rather than
//! one recorded request.

use std::time::{SystemTime, UNIX_EPOCH};

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code};

/// Every member of the modelled `PeriodType` enum, with the duration its name states.
///
/// SOURCE: `smithy_query(query="shape",shape="PeriodType")`, written out here so these
/// rows pin the published list rather than restating whatever the service holds.
const PERIODS: &[(&str, i64)] = &[
    ("five-minutes", 5 * 60),
    ("fifteen-minutes", 15 * 60),
    ("one-hour", 60 * 60),
    ("three-hours", 3 * 60 * 60),
    ("one-day", 24 * 60 * 60),
    ("one-week", 7 * 24 * 60 * 60),
];

/// Every member of the modelled `MetricType` enum
/// (`smithy_query(query="shape",shape="MetricType")`).
const METRICS: &[&str] = &["aggregate-latency"];

/// Every member of the modelled `StatisticType` enum
/// (`smithy_query(query="shape",shape="StatisticType")`).
const STATISTICS: &[&str] = &["p50"];

/// Locations a series can be between: "The Region or Availability Zone that's the source
/// for the data query. For example, `us-east-1`" (`DataQuery$Source`).
///
/// The rows span the three forms EC2 names one by — a Region name, an Availability Zone's
/// zone name and its zone ID (both of which `DescribeAvailabilityZones` reports for the
/// same zone) — over Regions of differing shapes, including the four-component GovCloud
/// `us-gov-west-1`, and over Regions no recorded request used.
const LOCATIONS: &[&str] = &[
    "us-east-1",
    "eu-west-1",
    "ap-northeast-2",
    "us-gov-west-1",
    "me-central-1",
    "us-east-1a",
    "eu-west-1c",
    "use1-az1",
    "apne2-az3",
];

/// A name of the published Region form that names no Region AWS publishes: a series
/// cannot be between it and anything.
///
/// SOURCE: it is absent from `emulator_telemetry::policy::KNOWN_REGIONS`, the catalog this
/// crate holds Region names to (`ap-southeast-7` is the highest `ap-southeast` it carries).
const REGION_THAT_DOES_NOT_EXIST: &str = "ap-southeast-9";

/// How many data responses one page holds.
///
/// SOURCE: the ceiling the service documents on `MAX_RESPONSES_PER_PAGE`, above which a
/// call's queries are reported over several pages rather than refused.
const MAX_RESPONSES_PER_PAGE: usize = 20;

/// How many metric points one series is reported with at most.
///
/// SOURCE: the ceiling the service documents on `MAX_METRIC_POINTS_PER_SERIES`, which
/// bounds the response to a window the calendar alone bounds.
const MAX_METRIC_POINTS_PER_SERIES: usize = 1_000;

/// The widest round trip the modelled network can cost, in milliseconds: half the Earth's
/// circumference is about 20,040 km, and a round trip is twice that path, lengthened for a
/// cable route and crossed at about 200 km/ms — under 300 ms even between two antipodes.
/// Nothing this operation reports may exceed it.
const MAX_PLAUSIBLE_ROUND_TRIP_MS: f32 = 300.0;

/// The round trip between Oregon and Northern Virginia, in milliseconds, and how far a
/// report of it may sit from that figure.
///
/// SOURCE: the two Regions are about 3,500 km apart, so a cable round trip between them is
/// a little over 50 ms — which is the order the Amazon backbone shows between those two
/// Regions. The anchor pins the SCALE of what is reported, not only the ordering, so a
/// model whose units or constants moved would be caught.
const OREGON_TO_VIRGINIA_MS: f32 = 50.0;

/// How far a reported round trip may sit from an anchor and still be that round trip.
const ANCHOR_TOLERANCE_MS: f32 = 8.0;

/// How far two instants may differ and still be the same point boundary: the wire carries
/// a `MillisecondDateTime`, so a millisecond is the resolution of the comparison.
const INSTANT_TOLERANCE_SECS: f64 = 0.002;

/// The Region every call in this suite is signed for. A data query names the locations it
/// is about, so this is deliberately NOT one of the locations the queries below use.
const SIGNING_REGION: &str = "eu-central-1";

/// One `DataQuery` of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Query {
    id: String,
    source: String,
    destination: String,
    metric: String,
    statistic: String,
    period: String,
}

/// One `MetricPoint` of a reported series.
#[derive(Debug, Clone)]
struct Point {
    /// `startDate`, in seconds since the epoch.
    start: f64,
    /// `endDate`, in seconds since the epoch.
    end: f64,
    value: f32,
    status: String,
}

/// A query between `source` and `destination` aggregated over `period`, at the one metric
/// and statistic the model publishes.
fn query(id: &str, source: &str, destination: &str, period: &str) -> Query {
    Query {
        id: id.to_string(),
        source: source.to_string(),
        destination: destination.to_string(),
        metric: METRICS[0].to_string(),
        statistic: STATISTICS[0].to_string(),
        period: period.to_string(),
    }
}

/// The `DataQuery.N.*` form fields of a whole `DataQueries` list, as ec2Query flattens a
/// list of structures into indexed keys.
fn query_fields(queries: &[Query]) -> Vec<(String, String)> {
    queries
        .iter()
        .enumerate()
        .flat_map(|(index, query)| {
            let at = |member: &str| format!("DataQuery.{}.{member}", index + 1);
            vec![
                (at("Id"), query.id.clone()),
                (at("Source"), query.source.clone()),
                (at("Destination"), query.destination.clone()),
                (at("Metric"), query.metric.clone()),
                (at("Statistic"), query.statistic.clone()),
                (at("Period"), query.period.clone()),
            ]
        })
        .collect()
}

/// `StartTime`/`EndTime` form fields for the window `[start, end)`, in seconds since the
/// epoch, written the way the model's own example writes them
/// (`2022-06-10T12:00:00.000Z`, `GetAwsNetworkPerformanceDataRequest$StartTime`).
fn window_fields(start: i64, end: i64) -> Vec<(String, String)> {
    vec![
        ("StartTime".to_string(), iso8601(start)),
        ("EndTime".to_string(), iso8601(end)),
    ]
}

/// `at` seconds since the epoch as an ISO 8601 instant.
fn iso8601(at: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an ISO 8601 instant")
}

/// The instant `text` names, in seconds since the epoch.
fn instant(case: &str, text: &str) -> f64 {
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {text:?} is not an instant: {error}"))
        .as_secs_f64()
}

/// A window of three whole hours ending at the top of the current hour: a span every
/// aggregation period the model publishes has windows in, and one that does not depend on
/// where inside the hour a call lands.
fn stable_window() -> (i64, i64) {
    let aligned = now_secs().div_euclid(3600) * 3600;
    (aligned - 3 * 3600, aligned)
}

/// The clock this suite reads its windows from.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// How long the aggregation period `period` lasts, out of [`PERIODS`].
fn period_duration(period: &str) -> i64 {
    PERIODS
        .iter()
        .find(|(name, _)| *name == period)
        .map(|(_, seconds)| *seconds)
        .unwrap_or_else(|| panic!("{period} is no published aggregation period"))
}

/// One `GetAwsNetworkPerformanceData` call carrying `params`, as its HTTP status and XML
/// body.
async fn get_data(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        SIGNING_REGION,
        "GetAwsNetworkPerformanceData",
        params,
    )
    .await
}

/// The series and the metric points of every `<item>` of the `<dataResponseSet>`.
fn reported(case: &str, body: &str) -> Vec<(Query, Vec<Point>)> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: response is not XML: {error}\n{body}"));
    let Some(set) = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "dataResponseSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(|item| {
            let child = |name: &str| {
                item.children()
                    .find(|child| child.is_element() && child.tag_name().name() == name)
            };
            let text = |name: &str| {
                child(name)
                    .map(|child| child.text().unwrap_or_default().to_string())
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a data response carries no <{name}>: {body}")
                    })
            };
            let series = Query {
                id: text("id"),
                source: text("source"),
                destination: text("destination"),
                metric: text("metric"),
                statistic: text("statistic"),
                period: text("period"),
            };
            let points = child("metricPointSet")
                .map(|set| {
                    set.children()
                        .filter(roxmltree::Node::is_element)
                        .map(|point| {
                            let member = |name: &str| {
                                point
                                    .children()
                                    .find(|child| {
                                        child.is_element() && child.tag_name().name() == name
                                    })
                                    .and_then(|child| child.text())
                                    .map(ToString::to_string)
                                    .unwrap_or_else(|| {
                                        panic!(
                                            "case {case:?}: a metric point carries no \
                                             <{name}>: {body}"
                                        )
                                    })
                            };
                            let value = member("value");
                            Point {
                                start: instant(case, &member("startDate")),
                                end: instant(case, &member("endDate")),
                                value: value.parse().unwrap_or_else(|error| {
                                    panic!(
                                        "case {case:?}: value {value:?} is not a number: {error}"
                                    )
                                }),
                                status: member("status"),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            (series, points)
        })
        .collect()
}

/// The point boundaries a window `[start, end)` aggregated over `period` must be reported
/// with by RULE 2: period-long spans from `start`, the last one cut off at `end`, at most
/// [`MAX_METRIC_POINTS_PER_SERIES`] of them.
fn expected_points(start: i64, end: i64, period: i64) -> Vec<(f64, f64)> {
    let (start, end, period) = (start as f64, end as f64, period as f64);
    let count = ((end - start) / period)
        .ceil()
        .clamp(1.0, MAX_METRIC_POINTS_PER_SERIES as f64) as usize;
    (0..count)
        .map(|index| {
            let point_start = start + index as f64 * period;
            (point_start, (point_start + period).min(end))
        })
        .collect()
}

/// RULE 2 over one reported series: its points cover the requested window in period-long
/// steps, each carrying one positive latency and the healthy status.
fn assert_points(case: &str, series: &Query, points: &[Point], window: (i64, i64)) {
    let (start, end) = window;
    let expected = expected_points(start, end, period_duration(&series.period));
    assert_eq!(
        points.len(),
        expected.len(),
        "case {case:?}: series {series:?} over [{start}, {end}) must carry {} metric \
         point(s), got {}",
        expected.len(),
        points.len()
    );
    let first = points
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: series {series:?} carries no metric point"));
    for (index, (point, (expected_start, expected_end))) in
        points.iter().zip(expected.iter()).enumerate()
    {
        assert!(
            (point.start - expected_start).abs() <= INSTANT_TOLERANCE_SECS,
            "case {case:?}: series {series:?} point {index} must start at {expected_start}, \
             got {}",
            point.start
        );
        assert!(
            (point.end - expected_end).abs() <= INSTANT_TOLERANCE_SECS,
            "case {case:?}: series {series:?} point {index} must end at {expected_end}, got {}",
            point.end
        );
        assert!(
            point.end > point.start,
            "case {case:?}: series {series:?} point {index} must aggregate over a span, got \
             [{}, {}]",
            point.start,
            point.end
        );
        assert_eq!(
            point.status, "OK",
            "case {case:?}: series {series:?} point {index} must report a healthy window, got \
             {:?}",
            point.status
        );
        assert!(
            point.value.is_finite()
                && point.value > 0.0
                && point.value <= MAX_PLAUSIBLE_ROUND_TRIP_MS,
            "case {case:?}: series {series:?} point {index} must report a round trip in \
             (0, {MAX_PLAUSIBLE_ROUND_TRIP_MS}] ms, got {}",
            point.value
        );
        assert_eq!(
            point.value, first.value,
            "case {case:?}: series {series:?} reports one latency for every window of it, \
             got {} at point {index} against {}",
            point.value, first.value
        );
    }
}

/// Every query the request gave is answered, in that order, with its own members and with
/// the metric points RULE 2 requires over `window`.
async fn assert_answers(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
    queries: &[Query],
    window: (i64, i64),
) -> String {
    let (status, body) = get_data(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    let answered = reported(case, &body);
    let series: Vec<Query> = answered.iter().map(|(series, _)| series.clone()).collect();
    assert_eq!(
        series, queries,
        "case {case:?}: expected one data response per query, in order, got {series:?}"
    );
    for (series, points) in &answered {
        assert_points(case, series, points, window);
    }
    body
}

/// The round-trip latency the service reports for the series between `source` and
/// `destination`, read off one point of one whole period.
async fn latency(router: &MultiServiceRouter, source: &str, destination: &str) -> f32 {
    let period = PERIODS[0];
    let case = format!("{source} -> {destination}");
    let queries = vec![query("latency", source, destination, period.0)];
    let aligned = now_secs().div_euclid(period.1) * period.1;
    let mut params = query_fields(&queries);
    params.extend(window_fields(aligned - period.1, aligned));
    let (status, body) = get_data(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    let answered = reported(&case, &body);
    let (_, points) = answered
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: no series was reported: {body}"));
    assert_eq!(
        points.len(),
        1,
        "case {case:?}: one whole period is one metric point, got {}",
        points.len()
    );
    points[0].value
}

/// RULE 1: one response per query, echoing the series it names, over a table spanning the
/// number of queries, the aggregation period, the form of each location and the order the
/// queries were given in. Every row asks over the same window, so what varies is the
/// queries alone.
#[tokio::test]
async fn every_query_is_answered_with_its_own_series() {
    let (router, _engine) = super::ec2_query::emulator();
    let window = stable_window();

    let every_period: Vec<Query> = PERIODS
        .iter()
        .enumerate()
        .map(|(index, (period, _))| {
            query(&format!("period-{index}"), "us-east-1", "eu-west-1", period)
        })
        .collect();
    let three = vec![
        query("first", "us-east-1", "eu-west-1", "one-hour"),
        query("second", "ap-northeast-2", "us-east-1", "one-day"),
        query("third", "use1-az1", "apne2-az3", "five-minutes"),
    ];
    let mut reversed = three.clone();
    reversed.reverse();

    let cases: Vec<(&str, Vec<Query>)> = vec![
        (
            "one query between two Regions",
            vec![query("q1", "us-east-1", "eu-west-1", "five-minutes")],
        ),
        (
            "a Region to itself",
            vec![query("q1", "us-east-1", "us-east-1", "one-week")],
        ),
        (
            "a Region to one of its Availability Zones",
            vec![query("q1", "us-east-1", "us-east-1a", "fifteen-minutes")],
        ),
        (
            "an Availability Zone ID to a zone name",
            vec![query("q1", "use1-az1", "eu-west-1c", "three-hours")],
        ),
        ("one query per aggregation period", every_period),
        ("three queries", three),
        ("the same three queries, reversed", reversed),
    ];

    for (case, queries) in cases {
        let mut params = query_fields(&queries);
        params.extend(window_fields(window.0, window.1));
        assert_answers(&router, case, &params, &queries, window).await;
    }
}

/// RULE 1 and RULE 2: every location form, metric and statistic the model publishes names
/// a series that is answered, with the points of the window — including values no recorded
/// request used.
#[tokio::test]
async fn every_published_location_metric_and_statistic_is_answered() {
    let (router, _engine) = super::ec2_query::emulator();
    let window = stable_window();

    for source in LOCATIONS {
        for destination in LOCATIONS {
            for metric in METRICS {
                for statistic in STATISTICS {
                    for (period, _) in PERIODS {
                        let queries = vec![Query {
                            id: "catalog".to_string(),
                            source: (*source).to_string(),
                            destination: (*destination).to_string(),
                            metric: (*metric).to_string(),
                            statistic: (*statistic).to_string(),
                            period: (*period).to_string(),
                        }];
                        let case =
                            format!("{source} -> {destination}, {metric}/{statistic} over {period}");
                        let mut params = query_fields(&queries);
                        params.extend(window_fields(window.0, window.1));
                        assert_answers(&router, &case, &params, &queries, window).await;
                    }
                }
            }
        }
    }
}

/// RULE 2: whatever window a request asks over, the series is covered by period-long
/// metric points running from its `StartTime`, the last one ending at its `EndTime`.
///
/// The rows span, for every aggregation period the model publishes: a window aligned to
/// that period and one straddling it, a window shorter than one period and one many
/// periods long, a window ahead of the clock, the epoch itself, an instant before the
/// epoch and a span so long that the ceiling on points is what bounds the answer — so no
/// span a `MillisecondDateTime` can name is answered with anything but HTTP 200 and the
/// points of that span.
#[tokio::test]
async fn every_window_is_covered_by_period_long_metric_points() {
    let (router, _engine) = super::ec2_query::emulator();
    let now = now_secs();

    /// The last instant an ISO 8601 four-digit year can name.
    const END_OF_CALENDAR: i64 = 253_402_300_799;
    /// Midnight on 1 January 1900, before the epoch.
    const BEFORE_THE_EPOCH: i64 = -2_208_988_800;

    for (period, period_secs) in PERIODS {
        let aligned = now.div_euclid(*period_secs) * period_secs;
        let rows: Vec<(String, i64, i64)> = vec![
            (
                "one whole window, on the grid".to_string(),
                aligned - period_secs,
                aligned,
            ),
            (
                "many windows, on the grid".to_string(),
                aligned - 12 * period_secs,
                aligned,
            ),
            (
                "shorter than the period, inside one window".to_string(),
                aligned - period_secs / 2,
                aligned - period_secs / 4,
            ),
            (
                "straddling the grid".to_string(),
                aligned - period_secs - period_secs / 3,
                aligned - period_secs / 3,
            ),
            ("ending now".to_string(), aligned - period_secs, now + 1),
            (
                "ahead of the clock".to_string(),
                now + period_secs,
                now + 3 * period_secs,
            ),
            (
                "before the epoch".to_string(),
                BEFORE_THE_EPOCH,
                BEFORE_THE_EPOCH + 24 * 3600,
            ),
            (
                "more windows than the ceiling holds".to_string(),
                now - 3600,
                END_OF_CALENDAR,
            ),
        ];

        for (row, start, end) in rows {
            let case = format!("{period}: {row}");
            let queries = vec![query("windowed", "us-east-1", "eu-west-1", period)];
            let mut params = query_fields(&queries);
            params.extend(window_fields(start, end));
            assert_answers(&router, &case, &params, &queries, (start, end)).await;
        }
    }
}

/// RULE 3: the latency a series reports is the cost of the network between its two
/// locations.
///
/// The rows span the distance classes a pair of Regions can be at — two Regions of one
/// geography, the two coasts of one continent, a crossing of the Atlantic, of Eurasia, of
/// the Pacific, and a pair nearly antipodal — and the three forms a location is named by.
/// Every Region of the catalog this service accepts a location out of is asked about, so no
/// Region is answered with a latency the model cannot place.
#[tokio::test]
async fn the_latency_reported_follows_the_network_between_the_locations() {
    let (router, _engine) = super::ec2_query::emulator();

    // Pairs in increasing order of how far apart AWS publishes the two Regions.
    const INCREASING_DISTANCE: &[(&str, &str)] = &[
        ("us-east-1", "us-east-2"),
        ("us-east-1", "us-west-2"),
        ("us-east-1", "eu-west-1"),
        ("eu-west-1", "ap-northeast-1"),
        ("us-west-2", "ap-southeast-2"),
        ("sa-east-1", "ap-northeast-2"),
    ];

    // Within one Region there is no distance to cross, so that latency is the floor every
    // other pair stands above.
    let within_a_region = latency(&router, "us-east-1", "us-east-1").await;
    assert!(
        within_a_region > 0.0 && within_a_region < MAX_PLAUSIBLE_ROUND_TRIP_MS,
        "case \"within one Region\": expected a positive round trip, got {within_a_region}"
    );

    // One absolute anchor: the ordering rows below would hold for any model that merely
    // grew with distance, so one pair is held to the round trip its distance costs.
    let oregon_to_virginia = latency(&router, "us-west-2", "us-east-1").await;
    assert!(
        (oregon_to_virginia - OREGON_TO_VIRGINIA_MS).abs() <= ANCHOR_TOLERANCE_MS,
        "case \"us-west-2 -> us-east-1\": expected about {OREGON_TO_VIRGINIA_MS} ms \
         (within {ANCHOR_TOLERANCE_MS}), got {oregon_to_virginia}"
    );

    // Every Region the service accepts a location out of is a Region it can report a
    // series for, and a Region to itself is the floor whichever Region it is.
    for region in ::emulator_telemetry::policy::KNOWN_REGIONS {
        let reported = latency(&router, region, region).await;
        assert_eq!(
            reported, within_a_region,
            "case {region:?}: a Region to itself costs what any Region to itself costs, \
             {within_a_region}, got {reported}"
        );
    }

    let mut previous: Option<(&str, &str, f32)> = None;
    for (source, destination) in INCREASING_DISTANCE {
        let there = latency(&router, source, destination).await;
        let back = latency(&router, destination, source).await;
        assert_eq!(
            there, back,
            "case \"{source} -> {destination}\": the network costs the same either way \
             round, got {there} and {back}"
        );
        assert!(
            there > within_a_region && there <= MAX_PLAUSIBLE_ROUND_TRIP_MS,
            "case \"{source} -> {destination}\": expected a round trip above the \
             within-a-Region {within_a_region} ms and at most \
             {MAX_PLAUSIBLE_ROUND_TRIP_MS} ms, got {there}"
        );
        if let Some((previous_source, previous_destination, previous_latency)) = previous {
            assert!(
                there > previous_latency,
                "case \"{source} -> {destination}\": further apart than \
                 {previous_source} -> {previous_destination}, so it must cost more than \
                 {previous_latency} ms, got {there}"
            );
        }
        previous = Some((source, destination, there));
    }

    // A zone name and a zone ID name a place in the Region they belong to, so a series
    // between two zones costs what the series between their Regions costs.
    let between_regions = latency(&router, "us-east-1", "eu-west-1").await;
    for (source, destination) in [
        ("us-east-1a", "eu-west-1c"),
        ("use1-az1", "eu-west-1"),
        ("us-east-1", "euw1-az2"),
        ("use1-az3", "euw1-az1"),
    ] {
        let reported = latency(&router, source, destination).await;
        assert_eq!(
            reported, between_regions,
            "case \"{source} -> {destination}\": a zone is in its Region, so this costs \
             what us-east-1 -> eu-west-1 costs, {between_regions}, got {reported}"
        );
    }
}

/// RULE 4: `MaxResults` and `NextToken` page the data responses: each page holds at most
/// `MaxResults` of them, a walk answers every query exactly once in request order, and the
/// last page carries no token.
#[tokio::test]
async fn pagination_answers_every_query_exactly_once() {
    let (router, _engine) = super::ec2_query::emulator();
    let window = stable_window();

    let queries: Vec<Query> = (0..5)
        .map(|index| {
            query(
                &format!("q{index}"),
                LOCATIONS[index],
                LOCATIONS[LOCATIONS.len() - 1 - index],
                PERIODS[index % PERIODS.len()].0,
            )
        })
        .collect();

    for page_size in [1usize, 2, 3, 5, 6] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<Query> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = query_fields(&queries);
            params.extend(window_fields(window.0, window.1));
            params.push(("MaxResults".to_string(), page_size.to_string()));
            if let Some(token) = &token {
                params.push(("NextToken".to_string(), token.clone()));
            }
            let (status, body) = get_data(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 on page {}, got {status} with {body}",
                pages + 1
            );
            let page = reported(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} responses, got {}",
                page.len()
            );
            for (series, points) in &page {
                assert_points(&case, series, points, window);
            }
            seen.extend(page.into_iter().map(|(series, _)| series));
            pages += 1;
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                pages <= queries.len(),
                "case {case:?}: a walk of {} queries took {pages} pages",
                queries.len()
            );
        }
        assert_eq!(
            seen, queries,
            "case {case:?}: a walk answers every query exactly once, in request order"
        );
        let expected_pages = queries.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// One refusal row: what the case is, the form fields it sends, and the HTTP status and
/// EC2 error code it must be refused with.
type Refusal = (String, Vec<(String, String)>, u16, &'static str);

/// RULE 4: a call carrying more queries than one page holds is PAGED, not refused: the
/// ceiling bounds the page, so every query is still answered exactly once.
#[tokio::test]
async fn more_queries_than_one_page_holds_are_paged() {
    let (router, _engine) = super::ec2_query::emulator();
    let window = stable_window();

    // One more query than a page holds, so the walk takes two pages.
    let queries: Vec<Query> = (0..=MAX_RESPONSES_PER_PAGE)
        .map(|index| query(&format!("q{index}"), "us-east-1", "eu-west-1", "one-hour"))
        .collect();

    let mut seen: Vec<Query> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params = query_fields(&queries);
        params.extend(window_fields(window.0, window.1));
        if let Some(token) = &token {
            params.push(("NextToken".to_string(), token.clone()));
        }
        let (status, body) = get_data(&router, &params).await;
        assert_eq!(
            status, 200,
            "case \"{} queries\": expected HTTP 200 on page {}, got {status} with {body}",
            queries.len(),
            pages + 1
        );
        let page = reported("more queries than one page holds", &body);
        assert!(
            page.len() <= MAX_RESPONSES_PER_PAGE,
            "case \"{} queries\": a page holds at most {MAX_RESPONSES_PER_PAGE} responses, \
             got {}",
            queries.len(),
            page.len()
        );
        seen.extend(page.into_iter().map(|(series, _)| series));
        pages += 1;
        token = element(&body, "nextToken");
        if token.is_none() {
            break;
        }
        assert!(
            pages <= queries.len(),
            "case \"{} queries\": a walk took {pages} pages",
            queries.len()
        );
    }
    assert_eq!(
        seen, queries,
        "case \"{} queries\": every query is answered exactly once, in request order",
        queries.len()
    );
    assert_eq!(
        pages, 2,
        "case \"{} queries\": one more than a page holds takes two pages, got {pages}",
        queries.len()
    );
}

/// RULE 5: a request that names no series, no query or no window, or names something that
/// cannot exist, is refused — with the error code EC2 gives for that kind of fault — rather
/// than answered as a series that happens to be empty.
#[tokio::test]
async fn an_unanswerable_request_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let window = stable_window();

    let well_formed = query("q1", "us-east-1", "eu-west-1", "five-minutes");
    /// A well-formed request — one query over the stable window — with one member of the
    /// query replaced.
    fn with(base: &Query, member: &str, value: &str) -> Vec<(String, String)> {
        let mut replaced = base.clone();
        match member {
            "Id" => replaced.id = value.to_string(),
            "Source" => replaced.source = value.to_string(),
            "Destination" => replaced.destination = value.to_string(),
            "Metric" => replaced.metric = value.to_string(),
            "Statistic" => replaced.statistic = value.to_string(),
            "Period" => replaced.period = value.to_string(),
            other => panic!("no DataQuery member named {other}"),
        }
        let mut params = query_fields(&[replaced]);
        let window = stable_window();
        params.extend(window_fields(window.0, window.1));
        params
    }
    /// The same, with one member of the query left out entirely.
    fn without(base: &Query, member: &str) -> Vec<(String, String)> {
        let dropped = format!("DataQuery.1.{member}");
        let mut params: Vec<(String, String)> = query_fields(std::slice::from_ref(base))
            .into_iter()
            .filter(|(name, _)| *name != dropped)
            .collect();
        let window = stable_window();
        params.extend(window_fields(window.0, window.1));
        params
    }
    /// One well-formed query over the stable window, as the base for a row that varies
    /// something outside the query.
    fn well_formed_request(base: &Query) -> Vec<(String, String)> {
        let mut params = query_fields(std::slice::from_ref(base));
        let window = stable_window();
        params.extend(window_fields(window.0, window.1));
        params
    }

    // A token this service DID mint: the first page of a two-query call. The row below
    // presents it back to a call whose query list is too short for its cursor.
    let minted_token = {
        let two = vec![
            well_formed.clone(),
            query("q2", "us-east-1", "eu-west-1", "one-hour"),
        ];
        let mut params = query_fields(&two);
        params.extend(window_fields(window.0, window.1));
        params.push(("MaxResults".to_string(), "1".to_string()));
        let (status, body) = get_data(&router, &params).await;
        assert_eq!(
            status, 200,
            "the first page of two queries must succeed, got {status} {body}"
        );
        element(&body, "nextToken")
            .unwrap_or_else(|| panic!("the first of two queries must answer with a token: {body}"))
    };

    let mut cases: Vec<Refusal> = vec![
        (
            "a source that is no location at all".to_string(),
            with(&well_formed, "Source", "not-a-location"),
            400,
            "InvalidParameterValue",
        ),
        (
            "a destination that is no location at all".to_string(),
            with(&well_formed, "Destination", "Ohio"),
            400,
            "InvalidParameterValue",
        ),
        (
            "a Region name with no number".to_string(),
            with(&well_formed, "Source", "us-east"),
            400,
            "InvalidParameterValue",
        ),
        // A data query NAMES its locations, so a Region is held to the catalog this crate
        // holds every Region name to, not to the Region grammar alone.
        (
            "a Region of the published form that AWS does not publish".to_string(),
            with(&well_formed, "Source", REGION_THAT_DOES_NOT_EXIST),
            400,
            "InvalidParameterValue",
        ),
        (
            "a zone name of a Region that AWS does not publish".to_string(),
            with(
                &well_formed,
                "Destination",
                &format!("{REGION_THAT_DOES_NOT_EXIST}a"),
            ),
            400,
            "InvalidParameterValue",
        ),
        (
            "a zone ID whose prefix abbreviates no Region".to_string(),
            with(&well_formed, "Destination", "zzzz9-az1"),
            400,
            "InvalidParameterValue",
        ),
        (
            "a zone letter past the last zone of the Region".to_string(),
            with(&well_formed, "Source", "us-east-1z"),
            400,
            "InvalidParameterValue",
        ),
        (
            "a zone ID index past the last zone of the Region".to_string(),
            with(&well_formed, "Destination", "use1-az9"),
            400,
            "InvalidParameterValue",
        ),
        // `Metric`, `Statistic` and `Period` target the modelled `MetricType`,
        // `StatisticType` and `PeriodType` enums, and `MaxResults` a modelled `@range`, so a
        // value outside one of them is refused by the centralized modelled-constraint walk
        // ahead of the handler — with the boundary's own code rather than the one the handler
        // answers for the values it checks itself. Either way the request is refused, never
        // answered as a series with no data.
        (
            "a metric outside the modelled enum".to_string(),
            with(&well_formed, "Metric", "aggregate-throughput"),
            400,
            "ValidationError",
        ),
        (
            "a statistic outside the modelled enum".to_string(),
            with(&well_formed, "Statistic", "p99"),
            400,
            "ValidationError",
        ),
        (
            "an aggregation period outside the modelled enum".to_string(),
            with(&well_formed, "Period", "ten-minutes"),
            400,
            "ValidationError",
        ),
        (
            "a period that differs only in case".to_string(),
            with(&well_formed, "Period", "Five-Minutes"),
            400,
            "ValidationError",
        ),
        (
            "a page that would hold no result".to_string(),
            {
                let mut params = well_formed_request(&well_formed);
                params.push(("MaxResults".to_string(), "0".to_string()));
                params
            },
            400,
            // `MaxResults` carries no modelled bound here, so the HANDLER is what refuses a
            // page that would hold no result — with EC2's own code for a value it cannot
            // accept.
            "InvalidParameterValue",
        ),
        (
            "a token this service never issued".to_string(),
            {
                let mut params = well_formed_request(&well_formed);
                params.push(("NextToken".to_string(), "0".to_string()));
                params
            },
            400,
            "InvalidPaginationToken",
        ),
        // A token this service DID mint, presented back to a call whose query list is too
        // short for it: the cursor names no query of this call, so it cannot be resumed
        // from rather than being answered as an empty last page.
        (
            "a token whose cursor is past the end of this call's queries".to_string(),
            {
                let mut params = well_formed_request(&well_formed);
                params.push(("NextToken".to_string(), minted_token.clone()));
                params
            },
            400,
            "InvalidPaginationToken",
        ),
        // The data is asked for OVER a window, and a window is both of its ends.
        (
            "a request naming no window at all".to_string(),
            query_fields(std::slice::from_ref(&well_formed)),
            400,
            "MissingParameter",
        ),
        (
            "a request naming only a StartTime".to_string(),
            {
                let mut params = query_fields(std::slice::from_ref(&well_formed));
                params.push(("StartTime".to_string(), iso8601(now_secs() - 3600)));
                params
            },
            400,
            "MissingParameter",
        ),
        (
            "a request naming only an EndTime".to_string(),
            {
                let mut params = query_fields(std::slice::from_ref(&well_formed));
                params.push(("EndTime".to_string(), iso8601(now_secs())));
                params
            },
            400,
            "MissingParameter",
        ),
        (
            "a window whose end precedes its start".to_string(),
            {
                let mut params = query_fields(std::slice::from_ref(&well_formed));
                params.extend(window_fields(now_secs(), now_secs() - 3600));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a window with no duration".to_string(),
            {
                let mut params = query_fields(std::slice::from_ref(&well_formed));
                params.extend(window_fields(now_secs(), now_secs()));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        // One response answers one query, so a call carrying none asks for nothing.
        (
            "a request carrying no query".to_string(),
            window_fields(window.0, window.1),
            400,
            "MissingParameter",
        ),
        (
            "a dry run performs nothing".to_string(),
            {
                let mut params = well_formed_request(&well_formed);
                params.push(("DryRun".to_string(), "true".to_string()));
                params
            },
            412,
            "DryRunOperation",
        ),
        // The permission probe is the last check, so a dry run of a request that names no
        // series earns that request's own refusal rather than "would have succeeded".
        (
            "a dry run of a request that names no series".to_string(),
            {
                let mut params = with(&well_formed, "Source", "not-a-location");
                params.push(("DryRun".to_string(), "true".to_string()));
                params
            },
            400,
            "InvalidParameterValue",
        ),
    ];

    // A series is named by its source, destination, metric, statistic and aggregation
    // period: a query leaving any of them out names no series, and an empty value names
    // nothing either. The second code is the one an EMPTY value earns: a free-form string
    // member is missing, so the handler refuses it with EC2's own `MissingParameter`, while
    // an enum member's empty value is outside its enum and so is refused by the
    // modelled-constraint walk first, with the boundary's own code.
    for (member, empty_code) in [
        ("Source", "MissingParameter"),
        ("Destination", "MissingParameter"),
        ("Metric", "ValidationError"),
        ("Statistic", "ValidationError"),
        ("Period", "ValidationError"),
    ] {
        cases.push((
            format!("a query carrying no {member}"),
            without(&well_formed, member),
            400,
            "MissingParameter",
        ));
        cases.push((
            format!("a query whose {member} is empty"),
            with(&well_formed, member, ""),
            400,
            empty_code,
        ));
    }

    for (case, params, expected_status, expected_code) in cases {
        let (status, body) = get_data(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected error code {expected_code}, got {code:?} in {body}"
        );
        assert!(
            elements(&body, "dataResponseSet").is_empty(),
            "case {case:?}: a refused request reports no series, got {body}"
        );
    }
}

/// RULE 6: the operation is a READ. A request answered twice is answered identically, and a
/// refused request leaves nothing behind that a later caller could see.
#[tokio::test]
async fn reading_the_data_twice_answers_the_same() {
    let (router, _engine) = super::ec2_query::emulator();
    let window = stable_window();

    let queries = vec![
        query("q1", "us-east-1", "eu-west-1", "one-hour"),
        query("q2", "us-east-1a", "use1-az1", "one-day"),
    ];
    let mut params = query_fields(&queries);
    params.extend(window_fields(window.0, window.1));

    let first = assert_answers(&router, "first read", &params, &queries, window).await;

    // A refusal between the two reads: it must not change what the second read sees.
    let mut broken = queries.clone();
    broken[0].source = "nowhere".to_string();
    let mut broken_params = query_fields(&broken);
    broken_params.extend(window_fields(window.0, window.1));
    let (status, _) = get_data(&router, &broken_params).await;
    assert_eq!(
        status, 400,
        "a source that is no location is refused, got {status}"
    );

    let second = assert_answers(&router, "second read", &params, &queries, window).await;
    assert_eq!(
        strip_request_id(&first),
        strip_request_id(&second),
        "a report answered twice answers identically"
    );
}

/// A response body with its `<requestId>` removed: every call mints a fresh one, so it is
/// the one part of two identical answers that differs.
fn strip_request_id(body: &str) -> String {
    match element(body, "requestId") {
        Some(request_id) => body.replace(&request_id, "<request-id>"),
        None => body.to_string(),
    }
}
