// SPDX-License-Identifier: CC0-1.0

//! `GetDistributionMetricData` driven over the wire, through the same front door
//! a client uses: `POST /` with `X-Amz-Target: Lightsail_20161128.<Operation>`
//! and an `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed. The harness is the sibling suite's
//! (`instance_lifecycle`), so this file stands up no second emulator of its own,
//! and every distribution it reads was created through `CreateDistribution`.
//!
//! The rule under test: reading a distribution metric is a REPORT on the
//! distribution record the store holds and on the observations it holds for that
//! metric. So, for a request made in the one Region the distribution API is
//! served from, for every metric the catalog publishes, asked for in the unit
//! that metric is published in, over any window at any granularity the model
//! admits, the answer is HTTP 200 carrying the requested `metricName` and the
//! datapoints the store has — none, because this emulator serves no viewer
//! request through a distribution and measures nothing on one — the call writes
//! nothing a later caller can see, a metric asked for in a unit it is not
//! published in, a window whose end does not follow its start, or a request
//! naming no statistic is refused with the modelled 400 `InvalidInputException`,
//! a name no distribution record answers to earns the service's `DoesNotExist`
//! envelope, and a request made in any other Region is refused with the
//! documented Region restriction.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

use crate::instance_lifecycle::{error_code, Emulator};

/// The only Region Lightsail serves the distribution API from, from the modelled
/// `InvalidInputException` documentation
/// (`smithy_query(query="doc",shape="InvalidInputException")`): "Domain and
/// distribution APIs are only available in the N. Virginia (us-east-1) Amazon Web
/// Services Region."
const DISTRIBUTION_REGION: &str = "us-east-1";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list, so
/// a case can sit outside the catalog: a scope naming a Region Lightsail does not
/// serve at all is no more the distribution Region than any other.
const OFF_ENUM_REGION: &str = "il-central-1";

/// Every metric a distribution publishes and the unit its data is published in.
///
/// Written out here independently of the service's own catalog, so these rows pin
/// the published list rather than restating whatever the code holds.
///
/// SOURCE: the modelled `GetDistributionMetricDataRequest$metricName`
/// documentation ("Valid distribution metric names are listed below, along with
/// the most useful `statistics` to include in your request, and the published
/// `unit` value"), which states one published unit per metric and covers every
/// member of the modelled `DistributionMetricName` enum. That text writes the two
/// error-rate entries as `4xxErrorRate` and `5xxErrorRate`; the enum members a
/// request carries are `Http4xxErrorRate` and `Http5xxErrorRate`.
const PUBLISHED_UNITS: &[(&str, &str)] = &[
    ("Requests", "None"),
    ("BytesDownloaded", "None"),
    ("BytesUploaded", "None"),
    ("TotalErrorRate", "Percent"),
    ("Http4xxErrorRate", "Percent"),
    ("Http5xxErrorRate", "Percent"),
];

/// The bounds of the modelled `MetricPeriod` range (`@range(min: 60, max:
/// 86400)` on `MetricPeriod`).
const MIN_PERIOD: i64 = 60;
/// See [`MIN_PERIOD`].
const MAX_PERIOD: i64 = 86_400;

/// The granularities a request may ask for: both published bounds of the modelled
/// `MetricPeriod` range and two values between them.
const PERIODS: &[i64] = &[MIN_PERIOD, 300, 3600, MAX_PERIOD];

/// Statistic sets a request may ask for, differing along the count, the members
/// and the order: one statistic, several, every member of the modelled
/// `MetricStatistic` enum, and the same set reversed.
const STATISTIC_SETS: &[&[&str]] = &[
    &["Sum"],
    &["Average"],
    &["Maximum", "Minimum"],
    &["Minimum", "Maximum", "Sum", "Average", "SampleCount"],
    &["SampleCount", "Average", "Sum", "Maximum", "Minimum"],
];

/// The instant the windows in this suite start at, in epoch seconds — an
/// arbitrary fixed point, so a case's window is the same on every run.
const WINDOW_START: i64 = 1_760_000_000;

/// The body member `code` Lightsail sets on a rejection whose cause is that what
/// the request names does not exist.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The origin instance every distribution in this suite pulls from, and the
/// blueprint, bundle and zone it is created from: the origin's own configuration
/// is not what any case here is about.
const ORIGIN_INSTANCE: &str = "metrics-origin";
const ORIGIN_BLUEPRINT: &str = "amazon_linux_2023";
const ORIGIN_INSTANCE_BUNDLE: &str = "nano_3_0";
const ORIGIN_ZONE: &str = "us-east-1a";

/// The distribution bundle every case creates on, from the catalog
/// `GetDistributionBundles` publishes: which plan a distribution is on is not
/// what any case here is about.
const DISTRIBUTION_BUNDLE: &str = "small_1_0";

/// `LightsailDistribution$status` once the clock has deployed the configuration a
/// create stored — the status a distribution rests at, so a case that compares a
/// record before and after a read waits for it first rather than racing the
/// clock-owed transition.
const STATUS_DEPLOYED: &str = "Deployed";

/// Create the one origin resource this suite's distributions pull from.
async fn origin(emulator: &Emulator) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": ORIGIN_ZONE,
                "blueprintId": ORIGIN_BLUEPRINT,
                "bundleId": ORIGIN_INSTANCE_BUNDLE,
            }),
        )
        .await;
}

/// Create distribution `name` over the wire, so every case reports on a record
/// the store really holds.
async fn distribution(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": DISTRIBUTION_BUNDLE,
            }),
        )
        .await;
}

/// The one distribution `GetDistributions` reports for `name`.
async fn read(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok("GetDistributions", json!({"distributionName": name}))
        .await;
    let reported = body["distributions"].as_array().unwrap_or_else(|| {
        panic!("case reading {name}: expected a distributions array, got {body:?}")
    });
    assert_eq!(
        reported.len(),
        1,
        "case reading {name}: a named read answers exactly that distribution, got {reported:?}"
    );
    reported[0].clone()
}

/// Poll `GetDistributions` until `name` reports [`STATUS_DEPLOYED`], answering
/// the record.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_until_deployed(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let distribution = read(emulator, name).await;
        if distribution["status"].as_str() == Some(STATUS_DEPLOYED) {
            return distribution;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for '{STATUS_DEPLOYED}' on {name}: expected it within 5s, got {:?}",
            distribution["status"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One `GetDistributionMetricData` request body.
fn request(
    distribution_name: &str,
    metric_name: &str,
    unit: &str,
    period: i64,
    start_time: i64,
    end_time: i64,
    statistics: &[&str],
) -> Value {
    json!({
        "distributionName": distribution_name,
        "metricName": metric_name,
        "unit": unit,
        "period": period,
        "startTime": start_time,
        "endTime": end_time,
        "statistics": statistics,
    })
}

/// The datapoints an answer carries, or a panic naming `case`.
fn metric_data(case: &str, body: &Value) -> Vec<Value> {
    body["metricData"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a metricData array, got {body:?}"))
        .clone()
}

/// Every Region a distribution call can be made in that is NOT the distribution
/// Region.
///
/// Taken from the crate's own `REGION_NAME_VALUES` — the modelled `RegionName`
/// enum's values (`smithy_query(query="shape",shape="RegionName")`) — minus the
/// distribution Region, so the table cannot drift from the model as the catalog
/// grows, plus [`OFF_ENUM_REGION`].
fn other_regions() -> Vec<&'static str> {
    assert!(
        REGION_NAME_VALUES.contains(&DISTRIBUTION_REGION),
        "case the Region catalog: expected the modelled RegionName enum to list the distribution \
         Region, got {REGION_NAME_VALUES:?}"
    );
    assert!(
        !REGION_NAME_VALUES.contains(&OFF_ENUM_REGION),
        "case the Region catalog: expected {OFF_ENUM_REGION} to sit OUTSIDE the modelled \
         RegionName enum, got {REGION_NAME_VALUES:?}"
    );
    let mut regions: Vec<&'static str> = REGION_NAME_VALUES
        .iter()
        .copied()
        .filter(|region| *region != DISTRIBUTION_REGION)
        .collect();
    regions.push(OFF_ENUM_REGION);
    regions
}

// ---------------------------------------------------------------------------
// The published catalog
// ---------------------------------------------------------------------------

/// RULE: every metric the catalog publishes, asked for in the unit it is
/// published in over a window at any granularity the model admits, is answered
/// with the requested `metricName` and the observations the store holds for it —
/// of which there are none, for every metric and every window.
#[tokio::test]
async fn every_published_metric_answers_its_own_name_and_the_stored_series() {
    let emulator = Emulator::new();
    origin(&emulator).await;
    let name = "metrics-cdn";
    distribution(&emulator, name).await;

    for (index, (metric_name, unit)) in PUBLISHED_UNITS.iter().enumerate() {
        // The rows differ along the granularity, the statistic set and the size
        // and placement of the window as well as the metric: the answer is the
        // stored series whichever of them the request carries.
        let period = PERIODS[index % PERIODS.len()];
        let statistics = STATISTIC_SETS[index % STATISTIC_SETS.len()];
        let start_time = WINDOW_START + (index as i64) * 900;
        let end_time = start_time + period * (index as i64 + 1);
        let case = format!("{metric_name} in {unit} at period {period}");

        let (status, body) = emulator
            .call(
                "GetDistributionMetricData",
                request(
                    name,
                    metric_name,
                    unit,
                    period,
                    start_time,
                    end_time,
                    statistics,
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["metricName"].as_str(),
            Some(*metric_name),
            "case {case}: expected the requested metric name back, got {body:?}"
        );
        let data = metric_data(&case, &body);
        assert!(
            data.is_empty(),
            "case {case}: this emulator serves no viewer request through a distribution, so the \
             series must carry no datapoints, got {data:?}"
        );
    }
}

/// RULE: a window is a window wherever it lies — before the distribution was
/// created, around now, or entirely in the future — and the series is still the
/// stored one.
#[tokio::test]
async fn a_window_is_served_wherever_it_lies_relative_to_the_distribution() {
    let emulator = Emulator::new();
    origin(&emulator).await;
    let name = "window-cdn";
    distribution(&emulator, name).await;

    // Epoch seconds: a window long before this distribution existed, one
    // straddling the recent past, one entirely ahead of any clock this emulator
    // reads, and the smallest window the ordering rule admits (one second).
    let windows: &[(&str, i64, i64)] = &[
        (
            "a window years before the distribution existed",
            1_000_000_000,
            1_000_003_600,
        ),
        (
            "a window ending in the recent past",
            1_760_000_000,
            1_760_086_400,
        ),
        (
            "a window entirely in the future",
            4_000_000_000,
            4_000_003_600,
        ),
        (
            "the shortest window the ordering admits",
            1_760_000_000,
            1_760_000_001,
        ),
    ];

    for (case, start_time, end_time) in windows {
        let (status, body) = emulator
            .call(
                "GetDistributionMetricData",
                request(
                    name,
                    "Requests",
                    "None",
                    MIN_PERIOD,
                    *start_time,
                    *end_time,
                    &["Sum"],
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["metricName"].as_str(),
            Some("Requests"),
            "case {case}: expected the requested metric name back, got {body:?}"
        );
        let data = metric_data(case, &body);
        assert!(
            data.is_empty(),
            "case {case}: expected no datapoints, got {data:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Reading changes nothing
// ---------------------------------------------------------------------------

/// RULE: reading metrics reports; it does not change what a later caller sees.
/// The same request answers the same thing twice, and the distribution record
/// `GetDistributions` reports is exactly what it was before the reads.
#[tokio::test]
async fn reading_metrics_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    origin(&emulator).await;
    let name = "quiet-cdn";
    distribution(&emulator, name).await;
    // The create left the clock owing this distribution its deployment, so the
    // comparison starts from the record it rests at: what must not change is
    // what a READ changes, not what the clock was already going to.
    let before = wait_until_deployed(&emulator, name).await;

    let mut answers = Vec::new();
    for round in 0..2 {
        for (metric_name, unit) in PUBLISHED_UNITS {
            let case = format!("round {round} of {metric_name}");
            let answer = emulator
                .ok(
                    "GetDistributionMetricData",
                    request(
                        name,
                        metric_name,
                        unit,
                        300,
                        WINDOW_START,
                        WINDOW_START + 3_600,
                        &["Sum", "Average"],
                    ),
                )
                .await;
            assert!(
                metric_data(&case, &answer).is_empty(),
                "case {case}: expected no datapoints, got {answer:?}"
            );
            answers.push((case, answer));
        }
    }
    let repeated = answers.len() / 2;
    for index in 0..repeated {
        let (case, first) = &answers[index];
        let (_, second) = &answers[index + repeated];
        assert_eq!(
            first, second,
            "case {case}: a repeated read must answer the same thing, got {second:?}"
        );
    }

    let after = read(&emulator, name).await;
    assert_eq!(
        before, after,
        "case the distribution record after the reads: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals the handler owns
// ---------------------------------------------------------------------------

/// RULE: a request the model admits but that cannot describe a series — a metric
/// asked for in a unit it is not published in, a window whose end does not follow
/// its start, or a request naming no statistic — is refused with the modelled 400
/// `InvalidInputException`, whose message names the member that is wrong.
#[tokio::test]
async fn a_request_that_describes_no_series_is_refused_as_invalid_input() {
    let emulator = Emulator::new();
    origin(&emulator).await;
    let name = "refusal-cdn";
    distribution(&emulator, name).await;
    let before = wait_until_deployed(&emulator, name).await;

    // Each row is the case, the request, and the input member whose rule it
    // broke — which the refusal's message has to name, so a row is pinned to its
    // own rule rather than to any 400.
    let mut rows: Vec<(String, Value, &str)> = Vec::new();

    // Every metric, asked for in a unit it is NOT published in: the unit the
    // OTHER distribution metrics are published in, plus one of another dimension
    // entirely.
    for (metric_name, unit) in PUBLISHED_UNITS {
        let wrong_units: [&str; 2] = if *unit == "None" {
            ["Percent", "Bytes"]
        } else {
            ["None", "Count/Second"]
        };
        for wrong in wrong_units {
            rows.push((
                format!("{metric_name} asked for in {wrong}"),
                request(
                    name,
                    metric_name,
                    wrong,
                    300,
                    WINDOW_START,
                    WINDOW_START + 3_600,
                    &["Sum"],
                ),
                "unit",
            ));
        }
    }

    rows.push((
        "a window that ends where it starts".to_string(),
        request(
            name,
            "Requests",
            "None",
            300,
            WINDOW_START,
            WINDOW_START,
            &["Sum"],
        ),
        "startTime",
    ));
    rows.push((
        "a window that ends before it starts".to_string(),
        request(
            name,
            "TotalErrorRate",
            "Percent",
            MAX_PERIOD,
            WINDOW_START + 3_600,
            WINDOW_START,
            &["Average"],
        ),
        "startTime",
    ));
    // A window inverted within one second: the ordering is of INSTANTS, not of
    // whole seconds, so a sub-second inversion is refused like any other.
    rows.push((
        "a window inverted inside one second".to_string(),
        json!({
            "distributionName": name,
            "metricName": "Requests",
            "unit": "None",
            "period": 300,
            "startTime": WINDOW_START as f64 + 0.75,
            "endTime": WINDOW_START as f64 + 0.25,
            "statistics": ["Sum"],
        }),
        "startTime",
    ));
    rows.push((
        "a request naming no statistic".to_string(),
        request(
            name,
            "BytesDownloaded",
            "None",
            300,
            WINDOW_START,
            WINDOW_START + 3_600,
            &[],
        ),
        "statistics",
    ));

    for (case, body, member) in &rows {
        let (status, answered) = emulator
            .call("GetDistributionMetricData", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "case {case}: expected the wire error InvalidInputException, got {answered:?}"
        );
        assert!(
            answered["message"]
                .as_str()
                .is_some_and(|message| message.contains(member)),
            "case {case}: expected a message naming the member {member}, got {answered:?}"
        );
    }

    // A refused read is still a read: it changed nothing.
    let after = read(&emulator, name).await;
    assert_eq!(
        before, after,
        "case the distribution record after every refusal: expected {before:?}, got {after:?}"
    );
}

/// RULE: a name no distribution record answers to earns the service's
/// `DoesNotExist` envelope: HTTP 400, the wire error `NotFoundException`, the body
/// `code` `DoesNotExist`, and a message naming the resource type and the requested
/// name.
#[tokio::test]
async fn a_name_no_distribution_answers_to_earns_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    origin(&emulator).await;

    // One name this account DOES hold a distribution under, so the cases below
    // differ from it only in not being held.
    let held = "held-cdn";
    distribution(&emulator, held).await;

    // Names differing along what the rule generalises over — length, case, digits
    // and separators — every one of them admitted by the modelled `ResourceName`
    // pattern (`^\w[\w\-]*\w$`) and none of them a name the store holds.
    let names = [
        "ab",
        "never-created",
        "Probe_7",
        "cdn-0123456789",
        "HELD-CDN",
        "held-cdn-2",
    ];
    for name in names {
        let case = format!("GetDistributionMetricData on {name:?}");
        let (status, body) = emulator
            .call(
                "GetDistributionMetricData",
                request(
                    name,
                    "Requests",
                    "None",
                    300,
                    WINDOW_START,
                    WINDOW_START + 3_600,
                    &["Sum"],
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the wire error NotFoundException, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("The Distribution does not exist: {name}").as_str()),
            "case {case}: expected the resource-type/name message, got {body:?}"
        );
    }

    // The one name that IS held is still answered, so these rows differ from it
    // in nothing but being held.
    let served = emulator
        .ok(
            "GetDistributionMetricData",
            request(
                held,
                "Requests",
                "None",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        )
        .await;
    assert!(
        metric_data("the held distribution", &served).is_empty(),
        "case the held distribution: expected an empty series, got {served:?}"
    );
}

// ---------------------------------------------------------------------------
// The Region restriction
// ---------------------------------------------------------------------------

/// The refusal the documentation states verbatim, so its text is a requirement
/// and not an example: `smithy_query(query="doc",shape="InvalidInputException")`
/// reads "Domain and distribution APIs are only available in the N. Virginia
/// (us-east-1) Amazon Web Services Region. Please set your Amazon Web Services
/// Region configuration to us-east-1 to create, view, or edit these resources."
const REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                       (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                       Services Region configuration to us-east-1 to create, view, or edit \
                       these resources.";

/// RULE: the distribution API is served by the N. Virginia (`us-east-1`) endpoint
/// ALONE, so a request whose credential scope names any other Region is refused
/// with the modelled `InvalidInputException` carrying the documented restriction —
/// whatever else the request names, and even for a distribution the account holds
/// — while a request made in `us-east-1`, or one carrying no credential scope at
/// all (served in the service's default Region, which IS `us-east-1`), is served.
#[tokio::test]
async fn the_distribution_metric_api_is_served_from_the_n_virginia_endpoint_alone() {
    let emulator = Emulator::new();
    origin(&emulator).await;
    let name = "regional-cdn";
    distribution(&emulator, name).await;

    for (index, region) in other_regions().into_iter().enumerate() {
        // The rows differ along the metric, the unit and the granularity as well
        // as the Region: the restriction is about where the call was made, not
        // about what it asked for.
        let (metric_name, unit) = PUBLISHED_UNITS[index % PUBLISHED_UNITS.len()];
        let period = PERIODS[index % PERIODS.len()];
        let case = format!("{metric_name} in {region}");
        let (status, body) = emulator
            .call_in(
                Some(region),
                "GetDistributionMetricData",
                request(
                    name,
                    metric_name,
                    unit,
                    period,
                    WINDOW_START,
                    WINDOW_START + period,
                    &["Sum"],
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the wire error InvalidInputException, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(REFUSAL),
            "case {case}: expected the documented Region restriction, got {body:?}"
        );
    }

    // A Region that cannot serve the call is refused BEFORE the distribution is
    // looked up, so a name the account holds nothing under is refused the same
    // way — the Region, not the name, is what decides.
    let (status, body) = emulator
        .call_in(
            Some(OFF_ENUM_REGION),
            "GetDistributionMetricData",
            request(
                "never-created",
                "Requests",
                "None",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an unheld name outside the distribution Region: expected HTTP 400, got {status} \
         {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(REFUSAL),
        "case an unheld name outside the distribution Region: expected the documented Region \
         restriction, got {body:?}"
    );

    // The distribution Region itself, and a request carrying no credential scope
    // at all, are both served.
    for (case, region) in [
        ("the distribution Region", Some(DISTRIBUTION_REGION)),
        ("no credential scope at all", None),
    ] {
        let (status, body) = emulator
            .call_in(
                region,
                "GetDistributionMetricData",
                request(
                    name,
                    "TotalErrorRate",
                    "Percent",
                    300,
                    WINDOW_START,
                    WINDOW_START + 3_600,
                    &["Average"],
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert!(
            metric_data(case, &body).is_empty(),
            "case {case}: expected an empty series, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals the modelled constraints own
// ---------------------------------------------------------------------------

/// RULE: a value outside a modelled constraint is refused by the boundary walk
/// with its own 400 `ValidationException`, before the handler runs — the
/// `MetricPeriod` range one past each published bound, and a name outside each
/// enum this input reaches.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_by_the_boundary_walk() {
    let emulator = Emulator::new();
    origin(&emulator).await;
    let name = "boundary-cdn";
    distribution(&emulator, name).await;

    let rows: &[(&str, Value)] = &[
        (
            "a period one second below the range",
            request(
                name,
                "Requests",
                "None",
                MIN_PERIOD - 1,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "a period one second above the range",
            request(
                name,
                "Requests",
                "None",
                MAX_PERIOD + 1,
                WINDOW_START,
                WINDOW_START + 90_000,
                &["Sum"],
            ),
        ),
        (
            "a metric name outside the modelled enum",
            request(
                name,
                "4xxErrorRate",
                "Percent",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Average"],
            ),
        ),
        (
            "a unit outside the modelled enum",
            request(
                name,
                "Requests",
                "Furlongs",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Sum"],
            ),
        ),
        (
            "a statistic outside the modelled enum",
            request(
                name,
                "Requests",
                "None",
                300,
                WINDOW_START,
                WINDOW_START + 3_600,
                &["Median"],
            ),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = emulator
            .call("GetDistributionMetricData", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case}: expected the boundary walk's ValidationException, got {answered:?}"
        );
    }

    // Both published bounds of the range are themselves accepted.
    for period in [MIN_PERIOD, MAX_PERIOD] {
        let case = format!("the period bound {period}");
        let body = emulator
            .ok(
                "GetDistributionMetricData",
                request(
                    name,
                    "BytesUploaded",
                    "None",
                    period,
                    WINDOW_START,
                    WINDOW_START + period,
                    &["Sum"],
                ),
            )
            .await;
        assert!(
            metric_data(&case, &body).is_empty(),
            "case {case}: expected an empty series, got {body:?}"
        );
    }
}
