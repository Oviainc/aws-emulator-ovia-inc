// SPDX-License-Identifier: CC0-1.0

//! `GetAwsNetworkPerformanceData` end-to-end: which series a request is answered with,
//! which metric points each series carries, how the answers page, and which requests are
//! refused.
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
//! 2. No series carries a metric point, for any location, any aggregation period and any
//!    window: a metric point is a MEASUREMENT of the Amazon network between two locations,
//!    and this emulator has no such network and records no observation of one. The answer
//!    is an empty `metricPointSet` — never a latency it did not measure — and that holds
//!    for a window of any length, anywhere in or out of the calendar.
//! 3. `MaxResults`/`NextToken` page the data responses, each page holding at most
//!    `MaxResults` of them, and a walk answers every query exactly once.
//! 4. A request that names no series, no query or no window, or names something that
//!    cannot exist, is refused — never answered as a series that happens to be empty — and
//!    `DryRun` performs nothing.
//! 5. The operation is a read: it answers two identical calls identically.
//!
//! Each table spans the class its rule generalises over (the number of queries, the
//! aggregation period, the form of a location, the order of the queries, the length and
//! placement of the window, the page size) rather than one recorded request.

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

/// A window of three whole hours ending at the top of the current hour: a span every
/// aggregation period the model publishes has windows in, and one that does not depend on
/// where inside the hour a call lands.
fn stable_window() -> Vec<(String, String)> {
    let aligned = now_secs().div_euclid(3600) * 3600;
    window_fields(aligned - 3 * 3600, aligned)
}

/// The clock this suite reads its windows from.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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

/// The series and the number of metric points of every `<item>` of the `<dataResponseSet>`.
fn reported(case: &str, body: &str) -> Vec<(Query, usize)> {
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
                .map(|set| set.children().filter(roxmltree::Node::is_element).count())
                .unwrap_or_default();
            (series, points)
        })
        .collect()
}

/// Every query the request gave is answered, in that order, with its own members and — by
/// RULE 2 — with no metric point at all.
async fn assert_answers(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
    queries: &[Query],
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
        assert_eq!(
            *points, 0,
            "case {case:?}: this emulator measures no network, so the series {series:?} \
             carries no metric point, got {points} in {body}"
        );
    }
    assert!(
        !body.contains("<value>") && !body.contains("<startDate>"),
        "case {case:?}: no measured value may appear in the answer, got {body}"
    );
    body
}

/// RULE 1: one response per query, echoing the series it names, over a table spanning the
/// number of queries, the aggregation period, the form of each location and the order the
/// queries were given in. Every row asks over the same window, so what varies is the
/// queries alone.
#[tokio::test]
async fn every_query_is_answered_with_its_own_series() {
    let (router, _engine) = super::ec2_query::emulator();

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
        params.extend(stable_window());
        assert_answers(&router, case, &params, &queries).await;
    }
}

/// RULE 1: every location form, metric and statistic the model publishes names a series
/// that is answered — including values no recorded request used.
#[tokio::test]
async fn every_published_location_metric_and_statistic_is_answered() {
    let (router, _engine) = super::ec2_query::emulator();

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
                        params.extend(stable_window());
                        assert_answers(&router, &case, &params, &queries).await;
                    }
                }
            }
        }
    }
}

/// RULE 2: whatever window a request asks over, the series carries no metric point,
/// because nothing measured the network in any of its periods.
///
/// The rows span, for every aggregation period the model publishes: a window aligned to
/// that period and one straddling it, a window shorter than one period and one many
/// periods long, a window ahead of the clock, the epoch itself, an instant before the
/// epoch and an instant past the end of the calendar — so no span a `MillisecondDateTime`
/// can name is answered with an invented measurement, and none of them is answered with
/// anything but HTTP 200.
#[tokio::test]
async fn no_window_of_any_period_carries_a_metric_point() {
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
            ("the whole epoch so far".to_string(), 0, now),
            (
                "before the epoch".to_string(),
                BEFORE_THE_EPOCH,
                BEFORE_THE_EPOCH + 24 * 3600,
            ),
            (
                "past the end of the calendar".to_string(),
                now - 3600,
                END_OF_CALENDAR,
            ),
        ];

        for (row, start, end) in rows {
            let case = format!("{period}: {row}");
            let queries = vec![query("windowed", "us-east-1", "eu-west-1", period)];
            let mut params = query_fields(&queries);
            params.extend(window_fields(start, end));
            assert_answers(&router, &case, &params, &queries).await;
        }
    }
}

/// RULE 3: `MaxResults` and `NextToken` page the data responses: each page holds at most
/// `MaxResults` of them, a walk answers every query exactly once in request order, and the
/// last page carries no token.
#[tokio::test]
async fn pagination_answers_every_query_exactly_once() {
    let (router, _engine) = super::ec2_query::emulator();

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
            params.extend(stable_window());
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

/// RULE 3: a call carrying more queries than one page holds is PAGED, not refused: the
/// ceiling bounds the page, so every query is still answered exactly once.
#[tokio::test]
async fn more_queries_than_one_page_holds_are_paged() {
    let (router, _engine) = super::ec2_query::emulator();

    // One more query than a page holds, so the walk takes two pages.
    let queries: Vec<Query> = (0..=MAX_RESPONSES_PER_PAGE)
        .map(|index| query(&format!("q{index}"), "us-east-1", "eu-west-1", "one-hour"))
        .collect();

    let mut seen: Vec<Query> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params = query_fields(&queries);
        params.extend(stable_window());
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

/// RULE 4: a request that names no series, no query or no window, or names something that
/// cannot exist, is refused — with the error code EC2 gives for that kind of fault — rather
/// than answered as a series that happens to be empty.
#[tokio::test]
async fn an_unanswerable_request_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

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
        params.extend(stable_window());
        params
    }
    /// The same, with one member of the query left out entirely.
    fn without(base: &Query, member: &str) -> Vec<(String, String)> {
        let dropped = format!("DataQuery.1.{member}");
        let mut params: Vec<(String, String)> = query_fields(std::slice::from_ref(base))
            .into_iter()
            .filter(|(name, _)| *name != dropped)
            .collect();
        params.extend(stable_window());
        params
    }
    /// One well-formed query over the stable window, as the base for a row that varies
    /// something outside the query.
    fn well_formed_request(base: &Query) -> Vec<(String, String)> {
        let mut params = query_fields(std::slice::from_ref(base));
        params.extend(stable_window());
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
        params.extend(stable_window());
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
        // `StatisticType` and `PeriodType` enums, so a value outside one of them is refused
        // by the centralized modelled-constraint walk ahead of the handler, which answers
        // `ValidationError`. Either way the request is refused, never answered as a series
        // with no data.
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
            stable_window(),
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
    // member is missing, while an enum member's empty value is outside its enum and so is
    // refused by the modelled-constraint walk first.
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

/// RULE 5: the operation is a READ. A request answered twice is answered identically, and a
/// refused request leaves nothing behind that a later caller could see.
#[tokio::test]
async fn reading_the_data_twice_answers_the_same() {
    let (router, _engine) = super::ec2_query::emulator();

    let queries = vec![
        query("q1", "us-east-1", "eu-west-1", "one-hour"),
        query("q2", "us-east-1a", "use1-az1", "one-day"),
    ];
    let mut params = query_fields(&queries);
    params.extend(stable_window());

    let first = assert_answers(&router, "first read", &params, &queries).await;

    // A refusal between the two reads: it must not change what the second read sees.
    let mut broken = queries.clone();
    broken[0].source = "nowhere".to_string();
    let mut broken_params = query_fields(&broken);
    broken_params.extend(stable_window());
    let (status, _) = get_data(&router, &broken_params).await;
    assert_eq!(
        status, 400,
        "a source that is no location is refused, got {status}"
    );

    let second = assert_answers(&router, "second read", &params, &queries).await;
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
