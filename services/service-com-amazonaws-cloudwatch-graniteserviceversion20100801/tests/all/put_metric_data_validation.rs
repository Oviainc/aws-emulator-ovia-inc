// SPDX-License-Identifier: CC0-1.0

//! What `PutMetricData` REFUSES, and how it words the refusal.
//!
//! Three rules are under test, each over a set of inputs rather than one case:
//!
//! 1. **A datum is stored only if CloudWatch could hold it.** A time stamp outside the
//!    documented ingest window ("as much as two weeks before the current date, and as much as
//!    2 hours after the current day and time") and a dimension CloudWatch cannot hold
//!    ("Dimension values must contain only ASCII characters and must include at least one
//!    non-whitespace character. ASCII control characters are not supported", `Dimension$Value`)
//!    are refused, and a refused request stores NOTHING.
//! 2. **A refusal about an input names the REQUEST MEMBER PATH the value arrived on**, so a
//!    client can locate the batch element to fix — including a datum that is not the first,
//!    and one that rode in on `EntityMetricData` rather than `MetricData`.
//! 3. **An entity is validated as `StrictEntityValidation` says it is**: the flag "is
//!    <i>required</i> when EntityMetricData is included", a strict call fails entirely on an
//!    invalid entity, and a lenient one still ingests the metric data.

use serde_json::{json, Value};

use crate::harness::{build_router, call, call_with_headers, error_code, now_secs, ok};

/// The namespace every row of this suite publishes into.
const NAMESPACE: &str = "MyApp/Validation";

/// One hour, in the epoch seconds a time stamp carries.
const HOUR: f64 = 60.0 * 60.0;

/// One day, in the same unit.
const DAY: f64 = 24.0 * HOUR;

/// A `PutMetricData` request of one datum under [`NAMESPACE`].
fn publish(datum: Value) -> Value {
    json!({"Namespace": NAMESPACE, "MetricData": [datum]})
}

/// The message of a refusal body, or the whole body when it carries none.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .map_or_else(|| body.to_string(), ToString::to_string)
}

/// How many series the catalogue holds under [`NAMESPACE`].
async fn stored_series(router: &smithy_server_runtime::MultiServiceRouter) -> usize {
    let listed = ok(router, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
    listed
        .get("Metrics")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// The `SampleCount` of the one window covering `at`, or 0 when the series holds nothing there.
async fn samples_at(
    router: &smithy_server_runtime::MultiServiceRouter,
    metric: &str,
    at: f64,
) -> f64 {
    let stats = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": at - 60.0,
            "EndTime": at + 60.0,
            "Period": 60,
            "Statistics": ["SampleCount"],
        }),
    )
    .await;
    stats
        .get("Datapoints")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point.get("SampleCount").and_then(Value::as_f64))
                .sum()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn only_a_time_stamp_inside_the_ingest_window_is_stored() {
    // (case, how far the time stamp is from now, whether the publish is accepted)
    //
    // The rows walk the window from one edge to the other: just inside and just outside each
    // published bound, the default (no time stamp at all is `now`), and far outside both.
    let rows: &[(&str, f64, bool)] = &[
        ("now", 0.0, true),
        ("an hour ago", -HOUR, true),
        ("thirteen days ago", -13.0 * DAY, true),
        ("a minute inside the two-week bound", -14.0 * DAY + 60.0, true),
        ("a minute outside the two-week bound", -14.0 * DAY - 60.0, false),
        ("a month ago", -30.0 * DAY, false),
        ("a year ago", -365.0 * DAY, false),
        ("an hour ahead", HOUR, true),
        ("a minute inside the two-hour bound", 2.0 * HOUR - 60.0, true),
        ("a minute outside the two-hour bound", 2.0 * HOUR + 60.0, false),
        ("two weeks ahead", 14.0 * DAY, false),
    ];

    for (case, offset, accepted) in rows {
        // A fresh emulator per row, so the row's own publish is the only thing that could
        // have stored anything.
        let router = build_router();
        let at = now_secs() + offset;
        let (status, body) = call(
            &router,
            "PutMetricData",
            &publish(json!({"MetricName": "Windowed", "Value": 1.0, "Timestamp": at})),
        )
        .await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with \
                 {body:?}"
            );
            // Stored, not merely accepted: the datapoint is readable where it was stamped.
            let samples = samples_at(&router, "Windowed", at).await;
            assert_eq!(
                samples, 1.0,
                "case {case:?}: expected the stored datapoint to be readable, got \
                 SampleCount {samples}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the publish to be refused, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the invalid-parameter-value refusal, got {body:?}"
            );
            let series = stored_series(&router).await;
            assert_eq!(
                series, 0,
                "case {case:?}: a refused publish must store nothing, got {series} series"
            );
        }
    }

    // A datum with no time stamp at all is stamped now, which is inside the window.
    let router = build_router();
    let (status, body) = call(
        &router,
        "PutMetricData",
        &publish(json!({"MetricName": "Undated", "Value": 1.0})),
    )
    .await;
    assert_eq!(
        status, 200,
        "a datum with no time stamp must be accepted, got {status} with {body:?}"
    );
}

#[tokio::test]
async fn only_a_dimension_cloudwatch_can_hold_is_stored() {
    // (case, the dimension's name, its value, the member at fault, or `None` where the
    // dimension is accepted)
    //
    // The rows differ along what the dimension carries: printable ASCII in a name and a
    // value, the ASCII punctuation and spacing CloudWatch does hold, an ASCII CONTROL
    // character, text outside ASCII altogether (accented, ideographic, emoji), whitespace
    // only, and a name opening with the colon the member forbids. A refused row is answered
    // in the family this operation answers an unusable input value in, naming the member path
    // the text arrived on and the text itself, which the loop builds from the row.
    let rows: &[(&str, &str, &str, Option<&str>)] = &[
        ("plain ASCII", "InstanceId", "i-0123456789abcdef", None),
        ("ASCII punctuation and spacing", "Path group", "/api/v1 (read)", None),
        ("a value of one printable character", "D", "x", None),
        ("a bell in the value", "Host", "bad\u{0007}value", Some("Value")),
        ("a null in the value", "Host", "bad\u{0000}value", Some("Value")),
        ("a newline in the value", "Host", "first\nsecond", Some("Value")),
        ("a tab in the value", "Host", "first\tsecond", Some("Value")),
        ("a control character in the name", "ba\u{0001}d", "ok", Some("Name")),
        ("an accented value", "Server", "café", Some("Value")),
        ("an ideographic value", "Server", "日本語", Some("Value")),
        ("an emoji value", "Server", "server-🚀", Some("Value")),
        ("a non-ASCII name", "Sérvice", "api", Some("Name")),
        ("a value of only spaces", "Host", "   ", Some("Value")),
        ("a name of only spaces", "   ", "api", Some("Name")),
        ("a name opening with a colon", ":reserved", "api", Some("Name")),
    ];

    for (case, name, value, at_fault) in rows {
        let refusal = at_fault.map(|member| {
            let text = if member == "Name" { *name } else { *value };
            format!(
                "The value {text} for parameter \
                 MetricData.member.1.Dimensions.member.1.{member} is invalid."
            )
        });
        let router = build_router();
        let (status, body) = call(
            &router,
            "PutMetricData",
            &publish(json!({
                "MetricName": "Dimensioned",
                "Dimensions": [{"Name": name, "Value": value}],
                "Value": 1.0,
            })),
        )
        .await;

        let Some(expected) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body:?}"
            );
            // The dimension is part of the series' identity, so an accepted one comes back
            // through the catalogue exactly as it was published.
            let listed = ok(&router, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
            assert_eq!(
                listed["Metrics"][0].get("Dimensions"),
                Some(&json!([{"Name": name, "Value": value}])),
                "case {case:?}: expected the published dimension in the catalogue, got {listed}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected the publish to be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the invalid-parameter-value refusal, got {body:?}"
        );
        let got = message(&body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the message {expected:?}, got {got:?}"
        );
        let series = stored_series(&router).await;
        assert_eq!(
            series, 0,
            "case {case:?}: a refused publish must store nothing, got {series} series"
        );
    }
}

#[tokio::test]
async fn a_refusal_names_the_request_member_path_the_value_arrived_on() {
    /// A statistic set with `sum` and every other member sound.
    fn statistic_values(sum: Value) -> Value {
        json!({"SampleCount": 1.0, "Sum": sum, "Minimum": 1.0, "Maximum": 1.0})
    }

    /// The special values the operation documents as unsupported ("NaN, +Infinity,
    /// -Infinity"), as the awsJson protocols put a non-finite double on the wire: its name as
    /// a string.
    const NAN: &str = "NaN";
    /// The same, for the infinity the member also refuses.
    const INFINITY: &str = "Infinity";

    // (case, the whole request, the error code, the message)
    //
    // The rows differ along WHICH member carries the bad value and WHERE in the request it
    // sits: the first datum, a later one, a member of a nested structure, an element of a
    // list, and a datum carried by `EntityMetricData` rather than `MetricData`.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "a NaN Value on the first datum",
            publish(json!({"MetricName": "Bad", "Value": NAN})),
            "InvalidParameterValueException",
            "The value NaN for parameter MetricData.member.1.Value is invalid.".to_string(),
        ),
        (
            "a NaN Value on the THIRD datum",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [
                    {"MetricName": "Fine", "Value": 1.0},
                    {"MetricName": "AlsoFine", "Value": 2.0},
                    {"MetricName": "Bad", "Value": NAN},
                ],
            }),
            "InvalidParameterValueException",
            "The value NaN for parameter MetricData.member.3.Value is invalid.".to_string(),
        ),
        (
            "an infinite Value",
            publish(json!({"MetricName": "Bad", "Value": INFINITY})),
            "InvalidParameterValueException",
            "The value Infinity for parameter MetricData.member.1.Value is invalid.".to_string(),
        ),
        (
            "a NaN inside a Values array",
            publish(json!({"MetricName": "Bad", "Values": [1.0, 2.0, NAN]})),
            "InvalidParameterValueException",
            "The value NaN for parameter MetricData.member.1.Values.member.3 is invalid."
                .to_string(),
        ),
        (
            "a NaN inside a StatisticValues set",
            publish(
                json!({"MetricName": "Bad", "StatisticValues": statistic_values(json!(NAN))}),
            ),
            "InvalidParameterValueException",
            "The value NaN for parameter MetricData.member.1.StatisticValues.Sum is invalid."
                .to_string(),
        ),
        (
            "a storage resolution outside the permitted set",
            publish(json!({"MetricName": "Bad", "Value": 1.0, "StorageResolution": 30})),
            "InvalidParameterValueException",
            "The parameter MetricData.member.1.StorageResolution must be a value in the set \
             [ 1, 60 ]."
                .to_string(),
        ),
        (
            "another storage resolution outside it, on a later datum",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [
                    {"MetricName": "Fine", "Value": 1.0},
                    {"MetricName": "Bad", "Value": 1.0, "StorageResolution": 3_600},
                ],
            }),
            "InvalidParameterValueException",
            "The parameter MetricData.member.2.StorageResolution must be a value in the set \
             [ 1, 60 ]."
                .to_string(),
        ),
        (
            "Value together with StatisticValues",
            publish(json!({
                "MetricName": "Both",
                "Value": 1.0,
                "StatisticValues": statistic_values(json!(1.0)),
            })),
            "InvalidParameterCombinationException",
            "The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues \
             are mutually exclusive and you have specified both."
                .to_string(),
        ),
        (
            "Value together with Values",
            publish(json!({"MetricName": "Both", "Value": 1.0, "Values": [1.0]})),
            "InvalidParameterCombinationException",
            "The parameters MetricData.member.1.Value and MetricData.member.1.Values are \
             mutually exclusive and you have specified both."
                .to_string(),
        ),
        (
            "StatisticValues together with Values",
            publish(json!({
                "MetricName": "Both",
                "StatisticValues": statistic_values(json!(1.0)),
                "Values": [1.0],
            })),
            "InvalidParameterCombinationException",
            "The parameters MetricData.member.1.StatisticValues and \
             MetricData.member.1.Values are mutually exclusive and you have specified both."
                .to_string(),
        ),
        (
            "more Values in one datum than the documented 150",
            publish(json!({
                "MetricName": "TooMany",
                "Values": (0..151).map(|index| json!(f64::from(index))).collect::<Vec<Value>>(),
            })),
            "InvalidParameterValueException",
            "The collection MetricData.member.1.Values must not have a size greater than 150."
                .to_string(),
        ),
        (
            "more metrics in one request than the documented 1000",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": (0..1_001)
                    .map(|index| json!({"MetricName": format!("M{index}"), "Value": 1.0}))
                    .collect::<Vec<Value>>(),
            }),
            "InvalidParameterValueException",
            "The collection MetricData must not have a size greater than 1000.".to_string(),
        ),
        (
            "a bad value on a datum carried by EntityMetricData",
            json!({
                "Namespace": NAMESPACE,
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Name": "api"}},
                    "MetricData": [
                        {"MetricName": "Fine", "Value": 1.0},
                        {"MetricName": "Bad", "Value": NAN},
                    ],
                }],
            }),
            "InvalidParameterValueException",
            "The value NaN for parameter EntityMetricData.member.1.MetricData.member.2.Value \
             is invalid."
                .to_string(),
        ),
    ];

    for (case, request, code, expected) in rows {
        let router = build_router();
        let (status, body) = call(&router, "PutMetricData", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the refusal {code}, got {body:?}"
        );
        let got = message(&body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected the message {expected:?}, got {got:?}"
        );
        // "no data will be ingested": a refused batch leaves the store as it was, even
        // though its earlier data were sound.
        let series = stored_series(&router).await;
        assert_eq!(
            series, 0,
            "case {case:?}: a refused batch must store nothing, got {series} series"
        );
    }
}

#[tokio::test]
async fn an_entity_is_validated_as_the_strict_flag_says_it_is() {
    // (case, the request's entity half, whether the publish is accepted)
    //
    // The rows differ along the entity's identifying attributes and the flag: a well-formed
    // service entity and a well-formed resource entity, attributes that are too few, a key
    // name outside the five the member allows, a `Type` that is not an entity type, a service
    // entity missing the attribute its type is identified by, a datum with no entity at all,
    // more entity objects than the documented two, and the same invalid entity under the
    // lenient flag — where the metric data is still ingested.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a well-formed service entity, strictly validated",
            json!({
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Name": "api"}},
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            true,
        ),
        (
            "a well-formed resource entity",
            json!({
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {
                        "Type": "Resource",
                        "ResourceType": "AWS::EC2::Instance",
                        "Identifier": "i-0123456789abcdef",
                    }},
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            true,
        ),
        (
            "an entity-bearing call with no entity object of its own",
            json!({
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            true,
        ),
        (
            "an invalid entity under the lenient flag",
            json!({
                "StrictEntityValidation": false,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "NotAnEntityType", "Name": "api"}},
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            true,
        ),
        (
            "an entity with no identifying attributes at all",
            json!({
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "Entity": {"Attributes": {"AwsAccountId": "000000000000"}},
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            false,
        ),
        (
            "a key name outside the five the member allows",
            json!({
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Nickname": "api"}},
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            false,
        ),
        (
            "a service entity missing the name it is identified by",
            json!({
                "StrictEntityValidation": true,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Environment": "prod"}},
                    "MetricData": [{"MetricName": "Entitied", "Value": 1.0}],
                }],
            }),
            false,
        ),
    ];

    for (case, half, accepted) in rows {
        let router = build_router();
        let mut request = half;
        request["Namespace"] = json!(NAMESPACE);
        let (status, body) = call(&router, "PutMetricData", &request).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body:?}"
            );
            // Entity-associated metric data is ingested too, invalid entity or not: the
            // lenient flag drops the association, never the data.
            let samples = samples_at(&router, "Entitied", now_secs()).await;
            assert_eq!(
                samples, 1.0,
                "case {case:?}: expected the entity's metric data to be stored, got \
                 SampleCount {samples}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the publish to be refused, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: expected the invalid-parameter-value refusal, got {body:?}"
            );
            let series = stored_series(&router).await;
            assert_eq!(
                series, 0,
                "case {case:?}: a refused publish must store nothing, got {series} series"
            );
        }
    }
}

#[tokio::test]
async fn the_reserved_namespace_refusal_reports_the_reserved_prefix() {
    // The rows differ in how the caller SPELLED the reserved prefix. One reserved space is
    // named however it is spelled, so each row is refused — and the refusal reports the
    // prefix in its own spelling, which is the part of the namespace that is invalid, rather
    // than the namespace or the spelling the request carried.
    let rows: &[(&str, &str)] = &[
        ("the canonical spelling", "AWS/EC2"),
        ("lower case", "aws/ec2"),
        ("mixed case", "Aws/Ec2"),
        ("upper case with a lower-case tail", "AWS/redshift"),
        ("the bare prefix", "AWS/"),
    ];

    for (case, namespace) in rows {
        let router = build_router();
        let (status, body) = call(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": namespace,
                "MetricData": [{"MetricName": "Forged", "Value": 1.0}],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {namespace:?} to be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the invalid-parameter-value refusal, got {body:?}"
        );
        let got = message(&body);
        assert_eq!(
            got, "The value AWS/ for parameter Namespace is invalid.",
            "case {case:?}: expected the refusal to report the reserved prefix, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_lenient_call_reports_the_entity_it_dropped() {
    // (case, the entity the call carries, the failure its answer must name)
    //
    // The rows differ along WHY the entity could not be held, which is what the reported
    // failure name distinguishes: a type that is no entity type, identifying attributes that
    // name something the member does not allow, and a type whose own required attribute is
    // absent.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a type that is no entity type",
            json!({"KeyAttributes": {"Type": "NotAnEntityType", "Name": "api"}}),
            "InvalidTypeValue",
        ),
        (
            "a key name the member does not allow",
            json!({"KeyAttributes": {"Type": "Service", "Nickname": "api"}}),
            "InvalidKeyAttributes",
        ),
        (
            "no identifying attributes at all",
            json!({"Attributes": {"AwsAccountId": "000000000000"}}),
            "InvalidKeyAttributes",
        ),
        (
            "a service entity with no name",
            json!({"KeyAttributes": {"Type": "Service", "Environment": "prod"}}),
            "MissingRequiredFields",
        ),
        // Whatever an entity's text is made of, the failure reported is one of the names the
        // member enumerates: no part of the request reaches the header, so text an HTTP header
        // could not carry cannot reach it either.
        (
            "a key name carrying a newline",
            json!({"KeyAttributes": {"Type": "Service", "Na\nme": "api"}}),
            "InvalidKeyAttributes",
        ),
        (
            "a type spelled with characters outside ASCII",
            json!({"KeyAttributes": {"Type": "Sérvice\u{0007}", "Name": "api"}}),
            "InvalidTypeValue",
        ),
    ];

    for (case, entity, failure) in rows {
        let router = build_router();
        let (status, headers, body) = call_with_headers(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "StrictEntityValidation": false,
                "EntityMetricData": [{
                    "Entity": entity,
                    "MetricData": [{"MetricName": "Dropped", "Value": 1.0}],
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a lenient call must still be accepted, got {status} with {body:?}"
        );
        let reported = headers
            .get("x-amz-failure-message")
            .and_then(|value| value.to_str().ok());
        assert_eq!(
            reported,
            Some(failure),
            "case {case:?}: expected the dropped entity to be reported as {failure:?}, got \
             {reported:?}"
        );
        // The metric data is ingested either way: it is the ASSOCIATION the lenient flag
        // drops, never the datapoints.
        let samples = samples_at(&router, "Dropped", now_secs()).await;
        assert_eq!(
            samples, 1.0,
            "case {case:?}: expected the metric data to be stored, got SampleCount {samples}"
        );
    }

    // An entity this service CAN hold leaves nothing to report.
    let router = build_router();
    let (status, headers, body) = call_with_headers(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "StrictEntityValidation": false,
            "EntityMetricData": [{
                "Entity": {"KeyAttributes": {"Type": "Service", "Name": "api"}},
                "MetricData": [{"MetricName": "Kept", "Value": 1.0}],
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a sound entity must be accepted, got {status} with {body:?}"
    );
    assert!(
        headers.get("x-amz-failure-message").is_none(),
        "a sound entity must report no failure, got {:?}",
        headers.get("x-amz-failure-message")
    );
}

#[tokio::test]
async fn an_entity_bearing_call_states_which_entity_outcome_it_wants() {
    let router = build_router();
    // "This parameter is <i>required</i> when EntityMetricData is included."
    let (status, body) = call(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "EntityMetricData": [{
                "Entity": {"KeyAttributes": {"Type": "Service", "Name": "api"}},
                "MetricData": [{"MetricName": "Flagless", "Value": 1.0}],
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an entity-bearing call without the strict flag must be refused, got {status} with \
         {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombinationException",
        "expected the parameter-combination refusal, got {body:?}"
    );
    assert_eq!(
        message(&body),
        "Field StrictEntityValidation is required when setting EntityMetricData",
        "expected the refusal to name the missing flag, got {body:?}"
    );
    let series = stored_series(&router).await;
    assert_eq!(
        series, 0,
        "a refused publish must store nothing, got {series} series"
    );

    // The flag is required only BECAUSE an entity rides along: a plain `MetricData` call
    // needs none.
    let (status, body) = call(
        &router,
        "PutMetricData",
        &json!({"Namespace": NAMESPACE, "MetricData": [{"MetricName": "Plain", "Value": 1.0}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a call with no entity must not need the flag, got {status} with {body:?}"
    );
}

#[tokio::test]
async fn more_entity_objects_than_the_documented_two_are_refused() {
    /// One entity object with its own datum.
    fn entity(index: usize) -> Value {
        json!({
            "Entity": {"KeyAttributes": {"Type": "Service", "Name": format!("service-{index}")}},
            "MetricData": [{"MetricName": format!("E{index}"), "Value": 1.0}],
        })
    }

    // (case, how many entity objects the call carries, whether it is accepted)
    let rows: &[(&str, usize, bool)] = &[
        ("one entity object", 1, true),
        ("the documented two", 2, true),
        ("one past the documented two", 3, false),
        ("far past it", 7, false),
    ];

    for (case, count, accepted) in rows {
        let router = build_router();
        let (status, body) = call(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "StrictEntityValidation": true,
                "EntityMetricData": (0..*count).map(entity).collect::<Vec<Value>>(),
            }),
        )
        .await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body:?}"
            );
            let series = stored_series(&router).await;
            assert_eq!(
                series, *count,
                "case {case:?}: expected {count} series stored, got {series}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the publish to be refused, got {status} with {body:?}"
            );
            let got = message(&body);
            assert_eq!(
                got,
                "EntitySizeTooLarge: The total number of EntityMetricData elements must be \
                 shorter than 2",
                "case {case:?}: expected the entity-size refusal, got {got:?}"
            );
            let series = stored_series(&router).await;
            assert_eq!(
                series, 0,
                "case {case:?}: a refused publish must store nothing, got {series} series"
            );
        }
    }
}
