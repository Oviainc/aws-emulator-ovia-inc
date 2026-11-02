// SPDX-License-Identifier: CC0-1.0

//! The datum-level rules `PutMetricData` refuses a publish by, and the words each refusal
//! carries.
//!
//! Three rules, each asserted over a set of inputs:
//!
//! 1. **A statistic set has to describe a span of observations**: `StatisticSet$Minimum` is
//!    "The minimum value of the sample set" and `$Maximum` "The maximum value of the sample
//!    set" (`smithy_query(query="shape",shape="StatisticSet")`), so a set whose largest
//!    observation is below its smallest describes no span and is refused — rather than stored,
//!    where every later `Average`, `Minimum` and `Maximum` read of the series would be folded
//!    out of it.
//! 2. **A value outside an enumerated type is refused as the parameter-value fault the
//!    operation models**, naming the member path it arrived on and the whole value set the type
//!    declares (`PutMetricData` models `InvalidParameterValueException`).
//! 3. **A strictly validated entity that is missing an identifying attribute is told WHICH
//!    one**: `Entity$KeyAttributes` is what an entity is identified by, and
//!    `PutMetricDataInput$StrictEntityValidation` enumerates `MissingRequiredFields` as the
//!    failure for a key attribute its type needs — so the refusal names the attribute and the
//!    type, both read off the request, which is what a caller repairs the publish from.
//!
//! Every refusal is asserted with the member path of the datum it is about, so the rows differ
//! along WHERE the offending datum sits: the first of a batch, a later one, and one that rode
//! in on `EntityMetricData` rather than `MetricData`.

use serde_json::{json, Value};

use crate::harness::{build_router, call, error_code, error_message, now_secs, ok};

/// The namespace every row of this suite publishes into.
const NAMESPACE: &str = "Probe/DatumRules";

/// The parameter-value fault this operation models for an input value it cannot accept.
const PARAMETER_VALUE: &str = "InvalidParameterValueException";

/// A `MetricDatum` carrying `observations` for `metric`.
fn datum(metric: &str, observations: Value) -> Value {
    let mut datum = json!({"MetricName": metric, "Timestamp": now_secs() - 60.0});
    for (member, value) in observations.as_object().cloned().unwrap_or_default() {
        datum[member] = value;
    }
    datum
}

/// A statistic set of `count` observations summing to `sum`, bounded by `minimum` and
/// `maximum`.
fn statistic_set(count: f64, sum: f64, minimum: f64, maximum: f64) -> Value {
    json!({
        "StatisticValues": {
            "SampleCount": count,
            "Sum": sum,
            "Minimum": minimum,
            "Maximum": maximum,
        },
    })
}

/// How many datapoints the window around `at` holds for `metric` — 0 for a series a refused
/// publish never created.
async fn stored_samples(
    router: &smithy_server_runtime::MultiServiceRouter,
    metric: &str,
) -> f64 {
    let now = now_secs();
    let stats = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": now - 600.0,
            "EndTime": now + 60.0,
            "Period": 60,
            "Statistics": ["SampleCount"],
        }),
    )
    .await;
    stats["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| point["SampleCount"].as_f64().unwrap_or_default())
        .sum()
}

#[tokio::test]
async fn a_statistic_set_whose_bounds_cross_is_refused_and_stores_nothing() {
    // The rows differ along what the rule generalises over: how the two bounds sit relative to
    // each other (crossed, equal, ordered), including a set of one observation, and WHERE the
    // offending datum sits in the request — which is what the refusal's member path names.
    //
    // (case, the whole request, the member path the refusal names, or none when accepted)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a maximum below the minimum",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [datum("Crossed", statistic_set(4.0, 100.0, 40.0, 10.0))],
            }),
            Some("MetricData.member.1"),
        ),
        (
            "the same set, as the third datum of a batch",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [
                    datum("Crossed", statistic_set(1.0, 10.0, 10.0, 10.0)),
                    datum("Crossed", statistic_set(2.0, 20.0, 5.0, 15.0)),
                    datum("Crossed", statistic_set(4.0, 100.0, 40.0, 10.0)),
                ],
            }),
            Some("MetricData.member.3"),
        ),
        (
            "the same set, carried as entity metric data",
            json!({
                "Namespace": NAMESPACE,
                "StrictEntityValidation": false,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Name": "probe"}},
                    "MetricData": [datum("Crossed", statistic_set(4.0, 100.0, 40.0, 10.0))],
                }],
            }),
            Some("EntityMetricData.member.1.MetricData.member.1"),
        ),
        (
            "bounds that are equal, the one-observation set",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [datum("Held", statistic_set(1.0, 10.0, 10.0, 10.0))],
            }),
            None,
        ),
        (
            "a maximum above the minimum",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [datum("Held", statistic_set(4.0, 100.0, 10.0, 40.0))],
            }),
            None,
        ),
        (
            "bounds that cross below zero",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [datum("Crossed", statistic_set(2.0, -30.0, -10.0, -20.0))],
            }),
            Some("MetricData.member.1"),
        ),
    ];

    for (case, request, refused_at) in rows {
        // A fresh emulator per row, so what the row's publish left in the store is the only
        // thing the series holds.
        let router = build_router();
        let (status, body) = call(&router, "PutMetricData", &request).await;
        let Some(path) = refused_at else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body}"
            );
            let held = stored_samples(&router, "Held").await;
            assert!(
                held > 0.0,
                "case {case:?}: an accepted statistic set must be stored, got {held} samples"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            PARAMETER_VALUE,
            "case {case:?}: expected the parameter-value fault, got {body}"
        );
        let expected = format!(
            "The parameter {path}.StatisticValues.Maximum must be greater than \
             {path}.StatisticValues.Minimum."
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // "Any validation error (for entity or metric data) will fail the entire request, and
        // no data will be ingested." (`PutMetricDataInput$StrictEntityValidation`)
        let stored = stored_samples(&router, "Crossed").await;
        assert_eq!(
            stored, 0.0,
            "case {case:?}: a refused publish must store nothing, got {stored} samples"
        );
    }
}

#[tokio::test]
async fn a_unit_outside_the_enumerated_type_is_refused_as_a_parameter_value() {
    let router = build_router();

    // The rows differ along how the submitted unit misses the type: another word, a value of
    // the right shape in the wrong case, and a unit name with its divisor spelled another way.
    // The accepted rows are values the type declares
    // (`smithy_query(query="shape",shape="StandardUnit")`), including one — `Terabits/Second` —
    // no report of this behaviour cites.
    //
    // (case, the unit, whether the publish is accepted)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a value the type declares", "Count", true),
        ("a compound value the type declares", "Bytes/Second", true),
        ("a value no report of this cites", "Terabits/Second", true),
        ("the value for a measurement with no unit", "None", true),
        ("a word that is no unit", "Widgets", false),
        ("a declared value in another case", "count", false),
        ("a divisor spelled another way", "Bytes/sec", false),
    ];

    for (case, unit, accepted) in rows {
        let request = json!({
            "Namespace": NAMESPACE,
            "MetricData": [datum("Units", json!({"Value": 1.0, "Unit": unit}))],
        });
        let (status, body) = call(&router, "PutMetricData", &request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            PARAMETER_VALUE,
            "case {case:?}: expected the parameter-value fault, got {body}"
        );
        let actual = error_message(&body);
        // The sentence names the member path and the whole set the type admits, and nothing of
        // the submitted value.
        assert!(
            actual.starts_with("The parameter MetricData.member.1.Unit must be a value in the set [ "),
            "case {case:?}: expected the member path and the accepted set, got {actual:?}"
        );
        for declared in ["Count", "Bytes/Second", "Terabits/Second", "None"] {
            assert!(
                actual.contains(declared),
                "case {case:?}: the set must enumerate {declared}, got {actual:?}"
            );
        }
        assert!(
            !actual.contains(unit),
            "case {case:?}: the submitted value is not echoed, got {actual:?}"
        );
    }
}

/// One row of the sample-count table: the count a statistic set states, the list its datum
/// rides in, and the member path a refusal has to name.
///
/// A row rather than a tuple so the table states what each column is.
struct SampleCountRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The `SampleCount` the set states.
    count: f64,
    /// Whether the datum rides on `EntityMetricData` rather than `MetricData`.
    entity_borne: bool,
    /// The member path the refusal names, or `None` for a set this service stores.
    refused_at: Option<&'static str>,
}

#[tokio::test]
async fn a_statistic_set_of_no_samples_is_refused_and_stores_nothing() {
    // The rows differ along the count the set states: none, a negative count, one, and a
    // fraction — a count is a `Double` (`StatisticSet$SampleCount`), so a set may count a
    // fraction of a sample and only a count of no samples at all is refused — and along which
    // list the datum rides in, which is what the refusal's member path names.
    let rows: Vec<SampleCountRow> = vec![
        SampleCountRow {
            case: "no samples at all",
            count: 0.0,
            entity_borne: false,
            refused_at: Some("MetricData.member.1"),
        },
        SampleCountRow {
            case: "a negative count",
            count: -2.0,
            entity_borne: false,
            refused_at: Some("MetricData.member.1"),
        },
        SampleCountRow {
            case: "no samples, carried as entity metric data",
            count: 0.0,
            entity_borne: true,
            refused_at: Some("EntityMetricData.member.1.MetricData.member.1"),
        },
        SampleCountRow {
            case: "one sample",
            count: 1.0,
            entity_borne: false,
            refused_at: None,
        },
        SampleCountRow {
            case: "a fraction of a sample",
            count: 0.5,
            entity_borne: false,
            refused_at: None,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let SampleCountRow {
            case,
            count,
            entity_borne,
            refused_at,
        } = row;
        // A fresh emulator per row, so what the row's publish left in the store is the only
        // thing the series holds.
        let router = build_router();
        let metric = format!("Samples{index}");
        let set = statistic_set(count, 10.0, 10.0, 10.0);
        let request = if entity_borne {
            json!({
                "Namespace": NAMESPACE,
                "StrictEntityValidation": false,
                "EntityMetricData": [{
                    "Entity": {"KeyAttributes": {"Type": "Service", "Name": "probe"}},
                    "MetricData": [datum(&metric, set)],
                }],
            })
        } else {
            json!({"Namespace": NAMESPACE, "MetricData": [datum(&metric, set)]})
        };

        let (status, body) = call(&router, "PutMetricData", &request).await;
        let Some(path) = refused_at else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body}"
            );
            let stored = stored_samples(&router, &metric).await;
            assert_eq!(
                stored, count,
                "case {case:?}: expected the counted samples back, got {stored}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            PARAMETER_VALUE,
            "case {case:?}: expected the parameter-value fault, got {body}"
        );
        let expected =
            format!("The parameter {path}.StatisticValues.SampleCount must be greater than 0.");
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        let stored = stored_samples(&router, &metric).await;
        assert_eq!(
            stored, 0.0,
            "case {case:?}: a refused publish must store nothing, got {stored} samples"
        );
    }
}

#[tokio::test]
async fn a_refused_value_is_quoted_the_way_the_service_prints_a_double() {
    let router = build_router();
    // "Values must be in the range of -2^360 to 2^360." (`PutMetricData`) — the magnitude the
    // rows cross, which is also the one a value AT the limit is accepted on.
    let limit = 2.0_f64.powi(360);

    // The rows differ along what the rendering generalises over: the sign of the value, how
    // far past the limit its magnitude reaches, whether the shortest digits that round-trip it
    // are one digit or many, and which member carries it.
    //
    // (case, the datum, the member path, the value as the refusal quotes it, or none when
    // accepted)
    let rows: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "a magnitude just past the limit",
            json!({"Value": 4.697_085_165_547_666_5e108}),
            "MetricData.member.1.Value",
            Some("4.6970851655476665E108"),
        ),
        (
            "a magnitude whose mantissa is one digit",
            json!({"Value": 1e300}),
            "MetricData.member.1.Value",
            Some("1.0E300"),
        ),
        (
            "the same magnitude, negative",
            json!({"Value": -1e300}),
            "MetricData.member.1.Value",
            Some("-1.0E300"),
        ),
        (
            "the largest double there is",
            json!({"Value": f64::MAX}),
            "MetricData.member.1.Value",
            Some("1.7976931348623157E308"),
        ),
        (
            "a magnitude inside an element of a Values array",
            json!({"Values": [1.0, 1e300]}),
            "MetricData.member.1.Values.member.2",
            Some("1.0E300"),
        ),
        (
            "a magnitude inside a statistic set",
            json!({"StatisticValues": {
                "SampleCount": 1.0, "Sum": 1e300, "Minimum": 1.0, "Maximum": 1.0,
            }}),
            "MetricData.member.1.StatisticValues.Sum",
            Some("1.0E300"),
        ),
        (
            "a value exactly at the published limit",
            json!({"Value": limit}),
            "MetricData.member.1.Value",
            None,
        ),
    ];

    for (index, (case, observations, path, quoted)) in rows.into_iter().enumerate() {
        let metric = format!("Magnitudes{index}");
        let request = json!({
            "Namespace": NAMESPACE,
            "MetricData": [datum(&metric, observations)],
        });
        let (status, body) = call(&router, "PutMetricData", &request).await;
        let Some(quoted) = quoted else {
            assert_eq!(
                status, 200,
                "case {case:?}: a value at the limit is accepted, got {status} with {body}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let expected = format!("The value {quoted} for parameter {path} is invalid.");
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// One row of the entity table: the attributes an entity is submitted with, and the
/// identifying attribute its type needs that they leave out.
///
/// A row rather than a tuple so the table states what each column is.
struct EntityRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The `KeyAttributes` the entity is submitted with.
    attributes: Value,
    /// The attribute the refusal has to name and the type it is required for, or `None` for
    /// an entity this service can hold.
    missing: Option<(&'static str, &'static str)>,
}

#[tokio::test]
async fn a_strict_entity_names_the_key_attribute_it_is_missing() {
    let router = build_router();

    // The rows differ along which attribute its type is identified by is absent, and which
    // type that is: the attributes are `Entity$KeyAttributes`' own ("There are five allowed
    // attributes (key names): Type, ResourceType, Identifier, Name, and Environment"). The
    // accepted rows carry the attributes their type needs.
    let rows: Vec<EntityRow> = vec![
        EntityRow {
            case: "a service identified by no name",
            attributes: json!({"Type": "Service", "Environment": "probe"}),
            missing: Some(("Name", "Service")),
        },
        EntityRow {
            case: "a resource identified by no identifier",
            attributes: json!({"Type": "Resource", "ResourceType": "AWS::EC2::Instance"}),
            missing: Some(("Identifier", "Resource")),
        },
        EntityRow {
            case: "a resource that names no resource type",
            attributes: json!({"Type": "Resource", "Identifier": "i-1234567890abcdef0"}),
            missing: Some(("ResourceType", "Resource")),
        },
        EntityRow {
            case: "a service that is identified by its name",
            attributes: json!({"Type": "Service", "Name": "checkout"}),
            missing: None,
        },
        EntityRow {
            case: "a resource identified by both of its attributes",
            attributes: json!({
                "Type": "Resource",
                "ResourceType": "AWS::EC2::Instance",
                "Identifier": "i-1234567890abcdef0",
            }),
            missing: None,
        },
    ];

    for (index, EntityRow { case, attributes, missing }) in rows.into_iter().enumerate() {
        let metric = format!("Entities{index}");
        let request = json!({
            "Namespace": NAMESPACE,
            "StrictEntityValidation": true,
            "EntityMetricData": [{
                "Entity": {"KeyAttributes": attributes},
                "MetricData": [datum(&metric, json!({"Value": 2.0}))],
            }],
        });
        let (status, body) = call(&router, "PutMetricData", &request).await;
        let Some((attribute, entity_type)) = missing else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the publish to be accepted, got {status} with {body}"
            );
            let stored = stored_samples(&router, &metric).await;
            assert!(
                stored > 0.0,
                "case {case:?}: an accepted publish must store its datum, got {stored} samples"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            PARAMETER_VALUE,
            "case {case:?}: expected the parameter-value fault, got {body}"
        );
        let expected = format!(
            "MissingRequiredFields: Missing required key attribute {attribute} for entity of \
             type {entity_type}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        let stored = stored_samples(&router, &metric).await;
        assert_eq!(
            stored, 0.0,
            "case {case:?}: a strictly refused publish must store nothing, got {stored} samples"
        );
    }
}
