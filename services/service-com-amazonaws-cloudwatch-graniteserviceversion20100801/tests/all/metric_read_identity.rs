// SPDX-License-Identifier: CC0-1.0

//! The identity and the reach of a metric READ, driven over the wire the service speaks
//! through the shared `harness`.
//!
//! Four rules, each asserted over a TABLE whose rows differ along what the rule generalises
//! over rather than along the one input any recording happens to carry:
//!
//! 1. A datapoint's identity includes its UNIT: "If you omit Unit, all data that was
//!    collected with any unit is returned, along with the corresponding units that were
//!    specified when the data was reported to CloudWatch. If you specify a unit, the operation
//!    returns only data that was collected with that unit specified."
//!    (`GetMetricStatisticsInput$Unit`) — so one window holds one span per published unit, and
//!    never a cross-unit aggregate.
//! 2. A read sees a datapoint only at a granularity whose retention tier still holds it:
//!    "Data points with a period of less than 60 seconds are available for 3 hours. […] Data
//!    points with a period of 60 seconds (1-minute) are available for 15 days. Data points
//!    with a period of 300 seconds (5-minute) are available for 63 days. Data points with a
//!    period of 3600 seconds (1 hour) are available for 455 days (15 months)."
//!    (`GetMetricStatistics`), stated from the request's side on `Period`: "If the StartTime
//!    parameter specifies a time stamp that is greater than 3 hours ago, you must specify the
//!    period as follows or no data points in that time range is returned".
//! 3. A read asking for more datapoints than one call returns is refused with the count the
//!    REQUEST asked for, the cap, and the two remedies the operation documents.
//! 4. `Statistics` and `ExtendedStatistics` are an either/or combination, and a constraint the
//!    boundary validator catches on a dimension collection or inside a statistic list is
//!    answered as the parameter fault those operations model.

use crate::harness::{build_router, build_router_with_engine, call, error_code, error_message, ok};
use serde_json::{json, Value};

/// The namespace every series in this suite is published under.
const NAMESPACE: &str = "MyApp/ReadIdentity";

/// One hour, in the epoch seconds a published time stamp carries.
const HOUR: f64 = 60.0 * 60.0;

/// One day, in the same seconds.
const DAY: f64 = 24.0 * HOUR;

/// How many datapoints one `GetMetricStatistics` call returns: "The maximum number of data
/// points returned from a single call is 1,440." (`GetMetricStatistics`)
const MAX_DATAPOINTS: i64 = 1440;

/// A minute-aligned instant ten minutes in the past: recent enough to sit inside every
/// retention tier, and aligned so a requested start time rounds to where the request put it.
fn recent() -> f64 {
    (crate::harness::now_secs() / 60.0).floor() * 60.0 - 10.0 * 60.0
}

/// The datapoints of a `GetMetricStatistics` answer, ordered by unit then time stamp so a row
/// compares a SET of series rather than one serialization order.
fn datapoints(body: &Value) -> Vec<Value> {
    let mut points = body
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    points.sort_by(|left, right| {
        let key = |point: &Value| {
            (
                point
                    .get("Unit")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                point.get("Timestamp").and_then(Value::as_f64).unwrap_or(0.0) as i64,
            )
        };
        key(left).cmp(&key(right))
    });
    points
}

/// The `(Unit, Sum, SampleCount)` of every datapoint of an answer, which is what a per-unit
/// series is observed as.
fn series_of(body: &Value) -> Vec<(String, f64, f64)> {
    datapoints(body)
        .iter()
        .map(|point| {
            (
                point
                    .get("Unit")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                point.get("Sum").and_then(Value::as_f64).unwrap_or(f64::NAN),
                point
                    .get("SampleCount")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
            )
        })
        .collect()
}

/// A `GetMetricStatistics` request over the `Service=api` series of `metric`.
fn read(metric: &str, start: f64, end: f64, period: i64, unit: Option<&str>) -> Value {
    let mut request = json!({
        "Namespace": NAMESPACE,
        "MetricName": metric,
        "Dimensions": [{"Name": "Service", "Value": "api"}],
        "StartTime": start,
        "EndTime": end,
        "Period": period,
        "Statistics": ["Sum", "SampleCount"],
    });
    if let (Some(unit), Value::Object(members)) = (unit, &mut request) {
        members.insert("Unit".to_string(), json!(unit));
    }
    request
}

/// Write one datapoint of the `Service=api` series of `metric` straight into the store, at an
/// instant a publish can no longer stamp, stored at `storage_resolution` seconds of
/// granularity.
///
/// The ingest window `PutMetricData` documents is about WHEN a datapoint may be written, not
/// about how long CloudWatch keeps one, so the aged series a retention row reads is seeded the
/// way the write that created it left the store.
fn seed_datapoint(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    metric: &str,
    at_secs: f64,
    value: f64,
    storage_resolution: i32,
) {
    use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

    let key = cloudwatch::MetricKey {
        namespace: NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions: [("Service".to_string(), "api".to_string())]
            .into_iter()
            .collect(),
    };
    engine
        .state::<cloudwatch::ComAmazonawsCloudwatchGraniteserviceversion20100801State>()
        .lock()
        .append_datapoints(
            key,
            vec![cloudwatch::StoredDatum {
                timestamp_secs: at_secs,
                sample_count: 1.0,
                sum: value,
                minimum: value,
                maximum: value,
                unit: Some("Count".to_string()),
                storage_resolution,
            }],
        )
        .expect("the metric store has room for the seeded series");
}

/// One row of the unit-identity table: the observations published at ONE instant, and the
/// per-unit series they must be read back as.
///
/// A row rather than a tuple so the table states what each column is, and so the type of the
/// table itself stays simple.
struct UnitRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The `(unit, value)` observations the row publishes, all at one instant.
    published: Vec<(&'static str, f64)>,
    /// The `(unit, Sum, SampleCount)` series the read must answer with, ordered by unit.
    expected: Vec<(&'static str, f64, f64)>,
}

#[tokio::test]
async fn a_unit_is_part_of_a_datapoints_read_identity() {
    let router = build_router();
    let at = recent();

    // The rows differ along what the rule generalises over: how many distinct units one
    // instant carries, and how many observations each of those units carries. The units are
    // the model's own `StandardUnit` values
    // (`smithy_query(query="shape",shape="StandardUnit")`), including one — `Terabits` — no
    // report of this behaviour cites.
    let rows = vec![
        UnitRow {
            case: "three units at one instant",
            published: vec![("Count", 10.0), ("Milliseconds", 40.0), ("Bytes", 60.0)],
            expected: vec![
                ("Bytes", 60.0, 1.0),
                ("Count", 10.0, 1.0),
                ("Milliseconds", 40.0, 1.0),
            ],
        },
        UnitRow {
            case: "one unit, several observations",
            published: vec![("Count", 1.0), ("Count", 2.0), ("Count", 4.0)],
            expected: vec![("Count", 7.0, 3.0)],
        },
        UnitRow {
            case: "two units, one of them repeated",
            published: vec![("Count", 1.0), ("Bytes", 8.0), ("Count", 2.0)],
            expected: vec![("Bytes", 8.0, 1.0), ("Count", 3.0, 2.0)],
        },
        UnitRow {
            case: "a unit no report of this behaviour cites",
            published: vec![("Terabits", 2.0), ("Percent", 50.0)],
            expected: vec![("Percent", 50.0, 1.0), ("Terabits", 2.0, 1.0)],
        },
    ];

    for (index, UnitRow { case, published, expected }) in rows.into_iter().enumerate() {
        let metric = format!("Units{index}");
        let data: Vec<Value> = published
            .iter()
            .map(|(unit, value)| {
                json!({
                    "MetricName": metric,
                    "Dimensions": [{"Name": "Service", "Value": "api"}],
                    "Value": value,
                    "Unit": unit,
                    "Timestamp": at,
                })
            })
            .collect();
        ok(
            &router,
            "PutMetricData",
            &json!({"Namespace": NAMESPACE, "MetricData": data}),
        )
        .await;

        // A read naming NO unit answers with one datapoint per published unit, each carrying
        // its own unit and its own aggregate — never one cross-unit aggregate.
        let unitless = ok(&router, "GetMetricStatistics", &read(&metric, at, at + 60.0, 60, None)).await;
        let got = series_of(&unitless);
        let want: Vec<(String, f64, f64)> = expected
            .iter()
            .map(|(unit, sum, count)| ((*unit).to_string(), *sum, *count))
            .collect();
        assert_eq!(
            got, want,
            "case {case:?}: expected one datapoint per published unit {want:?}, got {got:?} \
             from {unitless}"
        );

        // A read naming ONE unit answers with that unit's series alone.
        for (unit, sum, count) in &expected {
            let filtered = ok(
                &router,
                "GetMetricStatistics",
                &read(&metric, at, at + 60.0, 60, Some(unit)),
            )
            .await;
            let got = series_of(&filtered);
            assert_eq!(
                got,
                vec![((*unit).to_string(), *sum, *count)],
                "case {case:?}: a read of {unit:?} must answer with that unit alone, got \
                 {got:?} from {filtered}"
            );
        }

        // "If you specify a unit that does not match the data collected, the results of the
        // operation are null."
        let unmatched = ok(
            &router,
            "GetMetricStatistics",
            &read(&metric, at, at + 60.0, 60, Some("Gigabytes/Second")),
        )
        .await;
        let got = series_of(&unmatched);
        assert!(
            got.is_empty(),
            "case {case:?}: a unit nothing was published with must answer with no datapoint, \
             got {got:?} from {unmatched}"
        );

        // Reading is reporting: the same read repeated answers with the same per-unit series,
        // so a caller polling a metric sees a settled answer rather than one that depends on
        // how often it has been asked.
        let again = ok(&router, "GetMetricStatistics", &read(&metric, at, at + 60.0, 60, None)).await;
        assert_eq!(
            datapoints(&again),
            datapoints(&unitless),
            "case {case:?}: a repeated read must answer the same, got {again} after {unitless}"
        );
    }
}

#[tokio::test]
async fn a_read_sees_a_datapoint_only_at_a_granularity_its_tier_still_holds() {
    let (router, engine) = build_router_with_engine();
    let now = crate::harness::now_secs();

    // The rows differ along the two things the rule generalises over: how old the span being
    // read is, and how fine the granularity it is read at. Each published tier boundary is
    // covered from both sides — the granularity the tier admits, and one finer than it.
    //
    // (case, how long ago the span is, the period it is read at, whether the datapoint shows)
    let rows: Vec<(&str, f64, i64, bool)> = vec![
        ("inside the three-hour tier, read at ten seconds", 10.0 * 60.0, 10, true),
        ("inside the three-hour tier, read at a minute", 10.0 * 60.0, 60, true),
        ("past the three-hour tier, read at ten seconds", 4.0 * HOUR, 10, false),
        ("past the three-hour tier, read at a minute", 4.0 * HOUR, 60, true),
        ("inside the fifteen-day tier, read at a minute", 14.0 * DAY, 60, true),
        ("past the fifteen-day tier, read at a minute", 20.0 * DAY, 60, false),
        ("past the fifteen-day tier, read at five minutes", 20.0 * DAY, 300, true),
        ("past the sixty-three-day tier, read at five minutes", 70.0 * DAY, 300, false),
        ("past the sixty-three-day tier, read at an hour", 70.0 * DAY, 3_600, true),
        ("inside the fifteen-month tier, read at an hour", 400.0 * DAY, 3_600, true),
        ("past the fifteen-month tier, read at an hour", 500.0 * DAY, 3_600, false),
    ];

    for (case, age, period, visible) in rows {
        let metric = format!("Tier{}x{}", age as i64, period);
        // The span is aligned on the period it is read at, so the row asserts the tier and
        // not the rounding of a start time off its boundary.
        let span = f64::from(period as i32);
        let at = ((now - age) / span).floor() * span;
        // A span read at a SUB-MINUTE period is one only a high-resolution series has ("Data
        // points with a period of less than 60 seconds are available only for custom metrics
        // defined with a StorageResolution of 1"), because regular resolution stores a
        // datapoint at 1-minute granularity; every other row is the default resolution.
        let resolution = if period < 60 { 1 } else { 60 };
        seed_datapoint(&engine, &metric, at, 12_345.0, resolution);

        let body = ok(
            &router,
            "GetMetricStatistics",
            &read(&metric, at, at + span, period, None),
        )
        .await;
        let got = series_of(&body);
        assert_eq!(
            !got.is_empty(),
            visible,
            "case {case:?}: expected the datapoint to be {}, got {got:?} from {body}",
            if visible { "readable" } else { "aged out" }
        );
    }
}

#[tokio::test]
async fn a_read_over_the_datapoint_cap_is_refused_with_the_count_it_asked_for() {
    let router = build_router();
    let now = crate::harness::now_secs();

    // The rows differ along what the rule generalises over: the period, and how far over (or
    // under) the cap the requested range reaches — including the exact cap, which is served.
    //
    // (case, the period, how many of those periods the range spans)
    let rows: Vec<(&str, i64, i64)> = vec![
        ("one minute, exactly the cap", 60, MAX_DATAPOINTS),
        ("one minute, one period past the cap", 60, MAX_DATAPOINTS + 1),
        ("five minutes, far past the cap", 300, 4_000),
        ("ten seconds, a range past the high-resolution tier", 10, 14_460),
        ("one hour, a quarter past the cap", 3_600, 1_800),
    ];

    for (case, period, periods) in rows {
        let span = f64::from(period as i32);
        let end = (now / span).floor() * span;
        let start = end - span * periods as f64;
        let (status, body) = call(
            &router,
            "GetMetricStatistics",
            &read("Capped", start, end, period, None),
        )
        .await;

        if periods <= MAX_DATAPOINTS {
            assert_eq!(
                status, 200,
                "case {case:?}: a request inside the cap is served, got {status} with {body}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterCombinationException",
            "case {case:?}: expected the combination fault, got {body}"
        );
        // The sentence the operation's own documentation states: the count the REQUEST asked
        // for, the cap it broke, and the two remedies ("you can narrow the specified time
        // range […] or you can increase the specified period").
        let expected = format!(
            "You have requested up to {periods} datapoints, which exceeds the limit of \
             {MAX_DATAPOINTS}. You may reduce the datapoints requested by increasing Period, or \
             decreasing the time range."
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            error_message(&body)
        );
    }
}

#[tokio::test]
async fn the_two_statistic_members_are_one_either_or_combination() {
    let router = build_router();
    let at = recent();

    // (case, the statistic members the request carries, the refusal code or none)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        ("the basic statistics alone", json!({"Statistics": ["Sum"]}), None),
        (
            "every basic statistic the member admits",
            json!({"Statistics": ["Sum", "Average", "Minimum", "Maximum", "SampleCount"]}),
            None,
        ),
        (
            "the extended statistics alone",
            json!({"ExtendedStatistics": ["p90"]}),
            None,
        ),
        (
            "neither statistic member",
            json!({}),
            Some("InvalidParameterCombinationException"),
        ),
    ];

    for (case, members, refusal) in rows {
        let mut request = json!({
            "Namespace": NAMESPACE,
            "MetricName": "EitherOr",
            "StartTime": at,
            "EndTime": at + 60.0,
            "Period": 60,
        });
        if let (Value::Object(into), Value::Object(from)) = (&mut request, &members) {
            for (name, value) in from {
                into.insert(name.clone(), value.clone());
            }
        }
        let (status, body) = call(&router, "GetMetricStatistics", &request).await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a request naming one statistic type is served, got {status} \
                 with {body}"
            ),
            Some(code) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, code),
                    "case {case:?}: expected HTTP 400 {code}, got {status} with {body}"
                );
                // The either/or is one COMBINATION of members, so the refusal names both.
                let expected = "At least one of the parameters Statistics and ExtendedStatistics \
                                must be specified.";
                assert_eq!(
                    error_message(&body),
                    expected,
                    "case {case:?}: expected {expected:?}, got {:?}",
                    error_message(&body)
                );
            }
        }
    }
}

/// What a row asserts about the refusal's MESSAGE.
enum MessageCheck {
    /// The whole sentence, asserted exactly: this service owns these words.
    Exact(String),
    /// The facts the clause must name, asserted as fragments. Used where the sentence is the
    /// FRAMEWORK's own constraint clause rather than one of this service's: what the refusal
    /// has to carry is the offending value, the member path and the set that was admitted.
    Names(Vec<&'static str>),
}

/// One row of the boundary-constraint table: the request that breaks a constraint, and the
/// refusal it must be answered with.
struct RefusalRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The operation the request is sent to.
    operation: &'static str,
    /// The request itself.
    request: Value,
    /// The wire error code the refusal must carry.
    code: &'static str,
    /// What the refusal's message must be, or must name.
    message: MessageCheck,
}

#[tokio::test]
async fn a_boundary_constraint_is_stated_as_the_parameter_fault_the_operation_models() {
    let router = build_router();
    let at = recent();

    /// How many dimensions one metric may carry: "You can use up to 30 dimensions per metric
    /// to further clarify what data the metric collects" (`PutMetricData`), which the
    /// modelled dimension lists state as `length 0..30`.
    const MAX_DIMENSIONS: usize = 30;

    /// The maximum `ListMetricsInput$Dimensions` itself declares (`length 0..10`) — below the
    /// per-metric maximum above, so a filter list between the two breaks THAT bound and the
    /// refusal names it.
    const MAX_FILTERS: usize = 10;

    /// The envelope the framework states a refusal it owns in.
    const FRAMEWORK_ENVELOPE: &str = "1 validation error detected: ";
    /// A dimension filter list `count` entries long.
    fn dimensions(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|index| json!({"Name": format!("d{index}"), "Value": "v"}))
                .collect(),
        )
    }

    /// A `GetMetricStatistics` request carrying `members` on top of the metric this suite
    /// refuses reads of.
    fn statistics_request(at: f64, members: Value) -> Value {
        let mut request = json!({
            "Namespace": NAMESPACE,
            "MetricName": "Dimensioned",
            "StartTime": at,
            "EndTime": at + 60.0,
            "Period": 60,
            "Statistics": ["Sum"],
        });
        if let (Value::Object(into), Value::Object(from)) = (&mut request, &members) {
            for (name, value) in from {
                into.insert(name.clone(), value.clone());
            }
        }
        request
    }

    // The rows differ along which collection breaks which bound, on which operation, and at
    // what depth of the request — and include a SCALAR member outside its enum, which keeps
    // the framework's own refusal.
    let rows = vec![
        RefusalRow {
            case: "a dimension filter list one past the metric's dimensions",
            operation: "ListMetrics",
            request: json!({
                "Namespace": NAMESPACE,
                "Dimensions": dimensions(MAX_DIMENSIONS + 1),
            }),
            code: "InvalidParameterValueException",
            message: MessageCheck::Exact(format!(
                "The collection Dimensions must not have a size greater than {MAX_DIMENSIONS}."
            )),
        },
        RefusalRow {
            case: "the over-long list on a read of one metric",
            operation: "GetMetricStatistics",
            request: statistics_request(at, json!({"Dimensions": dimensions(MAX_DIMENSIONS + 1)})),
            code: "InvalidParameterValueException",
            message: MessageCheck::Exact(format!(
                "The collection Dimensions must not have a size greater than {MAX_DIMENSIONS}."
            )),
        },
        RefusalRow {
            case: "the over-long list inside a published datum, named by its own path",
            operation: "PutMetricData",
            request: json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": "Dimensioned",
                    "Value": 1.0,
                    "Dimensions": dimensions(MAX_DIMENSIONS + 1),
                }],
            }),
            code: "InvalidParameterValueException",
            message: MessageCheck::Exact(format!(
                "The collection MetricData.member.1.Dimensions must not have a size greater \
                 than {MAX_DIMENSIONS}."
            )),
        },
        RefusalRow {
            case: "a percentile where the basic statistics belong",
            operation: "GetMetricStatistics",
            request: statistics_request(at, json!({"Statistics": ["p90"]})),
            code: "InvalidParameterValueException",
            // The element's own request member path, down to the list's target shape, and the
            // value set its enumerated type declares — the set, not the submitted value, is
            // what locates the repair (`statistic_name_refusals`, which asserts the sentence
            // over a table of names and positions).
            message: MessageCheck::Exact(
                crate::statistic_name_refusals::value_in_set_message(1),
            ),
        },
        RefusalRow {
            case: "another value outside the same list's enumerated type",
            operation: "GetMetricStatistics",
            request: statistics_request(at, json!({"Statistics": ["Sum", "Median"]})),
            code: "InvalidParameterValueException",
            message: MessageCheck::Exact(
                crate::statistic_name_refusals::value_in_set_message(2),
            ),
        },
        RefusalRow {
            // A SCALAR outside its enum keeps the framework's own CLAUSE — the value and the
            // member path it arrived on — under the request-validation class, the one every
            // validation refusal this service makes carries (`crate::metric_stream_scope`). The
            // read that
            // DOES model a parameter fault answers that instead, over its own enumerated
            // members (`crate::unit_read_identity`).
            case: "a scalar member outside its enum",
            operation: "GetMetricData",
            request: json!({
                "MetricDataQueries": [{
                    "Id": "m1",
                    "MetricStat": {
                        "Metric": {"Namespace": NAMESPACE, "MetricName": "Dimensioned"},
                        "Period": 60,
                        "Stat": "Sum",
                    },
                }],
                "StartTime": at,
                "EndTime": at + 60.0,
                "ScanBy": "ValueDescending",
            }),
            code: "ValidationException",
            message: MessageCheck::Names(vec![FRAMEWORK_ENVELOPE, "'scanBy'"]),
        },
    ];

    for RefusalRow {
        case,
        operation,
        request,
        code,
        message,
    } in rows
    {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {case:?}: expected HTTP 400 {code}, got {status} with {body}"
        );
        let stated = error_message(&body);
        match message {
            MessageCheck::Exact(expected) => assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            ),
            MessageCheck::Names(facts) => {
                for fact in facts {
                    assert!(
                        stated.contains(fact),
                        "case {case:?}: the refusal must name {fact:?}, got {stated:?}"
                    );
                }
            }
        }
    }

    // A collection INSIDE every bound is a request this service answers rather than refuses —
    // and so is one between the LISTING member's own modelled maximum and the dimensions a
    // metric can carry: the published model declares `ListMetricsInput$Dimensions`
    // `length 0..10` while a metric holds up to thirty dimensions, and a listing narrowed by a
    // filter for each of them is applied rather than refused. The rows span the member's own
    // maximum, one past it, and the per-metric maximum itself.
    for count in [MAX_FILTERS, MAX_FILTERS + 1, MAX_DIMENSIONS] {
        let served = ok(
            &router,
            "ListMetrics",
            &json!({"Namespace": NAMESPACE, "Dimensions": dimensions(count)}),
        )
        .await;
        assert_eq!(
            error_code(&served),
            "",
            "case {count} dimension filters: a filter list a metric could carry is served, got \
             {served}"
        );
        assert_eq!(
            served.get("Metrics").and_then(Value::as_array),
            Some(&vec![]),
            "case {count} dimension filters: a filter nothing matches is an empty listing, got \
             {served}"
        );
    }
}
