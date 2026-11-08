// SPDX-License-Identifier: CC0-1.0

//! How a refusal of a value outside the SET its member accepts SPELLS that set, over every
//! member of this service whose refusal enumerates one.
//!
//! The rule under test: **a refusal of the form `The parameter <member> must be a value in the
//! set <set>.` spells `<set>` the way this service announces that set — its own value order
//! inside its own bracket form — and never derives the order from the declaration the set comes
//! from.** The announcement belongs to the SET, so every member accepting the same values
//! announces them alike whichever operation and whichever member path the value arrived on, and
//! a set with no announcement of its own is spelled in the order its declaration holds inside
//! bare brackets.
//!
//! The rows differ along what the rule generalises over: WHICH set the member accepts (the
//! measurement units, the storage resolutions, the statistic names, the readable
//! granularities), which of the two bracket forms that set is announced in, which operation the
//! value arrived on, HOW the value misses the set, and WHERE in the request it sits — since the
//! member path the sentence names is taken from the request. The value a recorded request
//! happened to carry is one row among them.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, now_secs};
use crate::statistic_name_refusals::STATISTIC_VALUES;
use crate::unit_read_identity::STANDARD_UNITS;

/// The namespace every request in this suite names.
const NAMESPACE: &str = "MyApp/AnnouncedSets";

/// The error class a refusal the BOUNDARY validator makes carries, as the body of the answer
/// names it: the modelled `InvalidParameterValueException`, whose legacy `awsQueryError` code
/// (`InvalidParameterValue`) is what a CloudWatch client branches on and travels in the
/// `x-amzn-query-error` header.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// The same refusal made by a HANDLER, as this harness's `awsJson` envelope spells it: the
/// same modelled shape name, since the class is the same whichever layer refused the request.
const INVALID_PARAMETER_VALUE_SHAPE: &str = "InvalidParameterValueException";

/// The 27 units a measurement may be expressed in, in the order the SERVICE announces them —
/// which is not the order the model declares them in (`STANDARD_UNITS`) and is not derivable
/// from it. Recorded from the service's own refusal.
pub const ANNOUNCED_UNITS: [&str; 27] = [
    "Megabits",
    "Terabits",
    "Gigabits",
    "Count",
    "Bytes",
    "Gigabytes",
    "Gigabytes/Second",
    "Kilobytes",
    "Kilobits/Second",
    "Terabytes",
    "Terabits/Second",
    "Bytes/Second",
    "Percent",
    "Megabytes",
    "Megabits/Second",
    "Milliseconds",
    "Microseconds",
    "Kilobytes/Second",
    "Gigabits/Second",
    "Megabytes/Second",
    "Bits",
    "Bits/Second",
    "Count/Second",
    "Seconds",
    "Kilobits",
    "Terabytes/Second",
    "None",
];

/// The sentence a refusal of a value outside a set states: the member path the value arrived
/// on, and the set as the service announces it.
fn in_set_message(parameter: &str, announced_set: &str) -> String {
    format!("The parameter {parameter} must be a value in the set {announced_set}.")
}

/// What a refusal of `parameter` holding a value outside the measurement units says: those
/// units in the service's own order, inside the padded brackets it announces them in.
pub fn unit_in_set_message(parameter: &str) -> String {
    in_set_message(parameter, &format!("[ {} ]", ANNOUNCED_UNITS.join(", ")))
}

/// The resolutions a datum may be stored at as the service announces them ("Valid values are 1
/// and 60", `MetricDatum$StorageResolution`): inside padded brackets.
const ANNOUNCED_RESOLUTIONS: &str = "[ 1, 60 ]";

/// The sub-minute granularities a metric is readable at as the service announces them
/// (`GetMetricStatisticsInput$Period`): inside bare brackets.
const ANNOUNCED_PERIODS: &str = "[1, 5, 10, 20, 30]";

/// The statistic names as the service announces them: the order `Statistic` declares them in,
/// inside bare brackets.
fn announced_statistics() -> String {
    format!("[{}]", STATISTIC_VALUES.join(", "))
}

/// A minute-aligned instant three minutes in the past: a closed whole-minute bucket inside
/// every retention tier, so no row is refused for its range instead of its value.
fn closed_minute() -> f64 {
    (now_secs() / 60.0).floor() * 60.0 - 3.0 * 60.0
}

/// A `GetMetricStatistics` request that is valid but for what `patch` puts on it.
fn read(patch: Value) -> Value {
    let bucket = closed_minute();
    crate::harness::merged(
        json!({
            "Namespace": NAMESPACE,
            "MetricName": "Probe",
            "StartTime": bucket - 120.0,
            "EndTime": bucket + 300.0,
            "Period": 60,
            "Statistics": ["Sum"],
        }),
        &patch,
    )
}

/// A `PutMetricData` request whose `position`th datum (1-based) is stored at `resolution`, the
/// ones before it at a resolution the member accepts — so the member path a refusal names comes
/// from the request.
fn publish_at_resolution(position: usize, resolution: i64) -> Value {
    let data: Vec<Value> = (1..=position)
        .map(|index| {
            json!({
                "MetricName": format!("Probe{index}"),
                "Value": 1.0,
                "StorageResolution": if index == position { resolution } else { 60 },
            })
        })
        .collect();
    json!({"Namespace": NAMESPACE, "MetricData": data})
}

/// A `GetMetricStatistics` request whose `position`th statistic (1-based) is `name`, the ones
/// before it statistics the member accepts.
fn read_with_statistic_at(position: usize, name: &str) -> Value {
    let statistics: Vec<Value> = (1..=position)
        .map(|index| {
            json!(if index == position {
                name
            } else {
                STATISTIC_VALUES[index - 1]
            })
        })
        .collect();
    read(json!({"Statistics": statistics}))
}

/// One row: a request carrying one value outside the set its member accepts, and the whole
/// refusal it must be answered with.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The operation the request is sent to.
    operation: &'static str,
    /// The request itself.
    request: Value,
    /// The wire error code the refusal must carry.
    code: &'static str,
    /// The whole sentence the refusal must state.
    message: String,
}

#[tokio::test]
async fn a_value_outside_a_set_is_refused_with_that_set_as_the_service_announces_it() {
    let router = build_router();

    // The announcement re-orders the set it announces and never changes it: every value the
    // model declares is announced, and nothing else is.
    let mut declared = STANDARD_UNITS;
    declared.sort_unstable();
    let mut announced = ANNOUNCED_UNITS;
    announced.sort_unstable();
    assert_eq!(
        announced, declared,
        "the announced units must be exactly the declared ones, re-ordered: announced \
         {announced:?} against declared {declared:?}"
    );

    let mut rows: Vec<Row> = Vec::new();

    // The measurement units, announced in the service's own order inside padded brackets. The
    // values miss the set in a different way per row: a word that names no unit, a declared
    // value in another case, a declared value with its rate suffix changed, the empty value.
    for value in ["Furlongs", "bytes", "Bytes/Minute", ""] {
        rows.push(Row {
            case: "a unit outside the units a measurement may carry",
            operation: "GetMetricStatistics",
            request: read(json!({"Unit": value})),
            code: INVALID_PARAMETER_VALUE,
            message: unit_in_set_message("Unit"),
        });
    }

    // The storage resolutions, announced inside padded brackets: a value under the minute and
    // one over it, an hour, and the same miss on a later datum — the member path is the
    // request's, not a fixed one.
    for (position, resolution) in [(1, 2), (1, 59), (2, 3_600), (3, 30)] {
        rows.push(Row {
            case: "a resolution outside the resolutions a datum may be stored at",
            operation: "PutMetricData",
            request: publish_at_resolution(position, resolution),
            code: INVALID_PARAMETER_VALUE_SHAPE,
            message: in_set_message(
                &format!("MetricData.member.{position}.StorageResolution"),
                ANNOUNCED_RESOLUTIONS,
            ),
        });
    }

    // The statistic names, announced in declaration order inside bare brackets: a percentile,
    // whose place is the other statistic member, a declared name in another case, a name that
    // is no statistic at all, and a trimmed-mean function — each at its own position in the
    // list.
    for (position, name) in [(1, "p99"), (2, "sum"), (3, "Median"), (1, "TM(10%:90%)")] {
        rows.push(Row {
            case: "a statistic name outside the statistics the member computes",
            operation: "GetMetricStatistics",
            request: read_with_statistic_at(position, name),
            code: INVALID_PARAMETER_VALUE,
            message: in_set_message(
                &format!("Statistics.member.{position}.<list element>"),
                &announced_statistics(),
            ),
        });
    }

    // The readable granularities, announced inside bare brackets: the SUB-MINUTE set is what a
    // period shorter than a minute had to come from, so the rows are either side of a published
    // sub-minute value and one just under a minute. A period OVER a minute had to be a whole
    // number of minutes instead, and is refused against that requirement rather than against a
    // set (`crate::period_refusal_message` tables both halves).
    for period in [2, 7, 59] {
        rows.push(Row {
            case: "a period outside the granularities a metric is readable at",
            operation: "GetMetricStatistics",
            request: read(json!({"Period": period})),
            code: INVALID_PARAMETER_VALUE_SHAPE,
            message: in_set_message("Period", ANNOUNCED_PERIODS),
        });
    }

    for Row {
        case,
        operation,
        request,
        code,
        message,
    } in &rows
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *code),
            "case {case:?} ({operation}, request {request}): expected HTTP 400 {code}, got \
             {status} with {body}"
        );
        let stated = error_message(&body);
        assert_eq!(
            &stated, message,
            "case {case:?} ({operation}, request {request}): expected {message:?}, got \
             {stated:?}"
        );
    }
}
