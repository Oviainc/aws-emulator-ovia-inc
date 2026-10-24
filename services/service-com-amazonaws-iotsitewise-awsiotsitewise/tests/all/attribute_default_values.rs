// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the value an asset property STARTS with, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: materialising an asset property from a model
//! definition that declares an initial value — today that is an `attribute`
//! definition's `defaultValue` — gives that property a first sample carrying the
//! declared value in the `Variant` member its `PropertyDataType` names, at the
//! instant the property was materialised and with the quality an ordinary reading
//! carries, so a read of the property straight after the create reports it; a
//! definition that declares no such value leaves the property with no value at
//! all, and a later write at a newer timestamp takes over as the current value.
//!
//! The wire harness (`router`, `call`, `create_model`, …) lives in the
//! `asset_lifecycle` suite; this file reuses it rather than keeping a second copy.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, create_model, property_id, router};
use crate::support::now_seconds;

/// Where `GetAssetPropertyValue` is bound: `GET /properties/latest`, from
/// `smithy_query(query="operation", operation="GetAssetPropertyValue")`.
const LATEST: &str = "/properties/latest";

/// Where `BatchGetAssetPropertyValue` is bound: `POST /properties/batch/latest`.
const BATCH_LATEST: &str = "/properties/batch/latest";

/// Where `BatchPutAssetPropertyValue` is bound, used to write over a default.
const BATCH_PUT: &str = "/properties";

/// Where `GetAssetPropertyValueHistory` is bound: `GET /properties/history`.
const HISTORY: &str = "/properties/history";

/// One row of the table below: a model property definition, and the value the
/// asset's property must report immediately after the asset is created.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    name: &'static str,
    data_type: &'static str,
    /// The `PropertyType` the definition carries.
    kind: Value,
    /// The `Variant` member and value the default must be reported in, or `None`
    /// for a definition that declares no initial value at all.
    expected: Option<(&'static str, Value)>,
}

/// The definitions this suite spans.
///
/// The rows differ along what the rule generalises over: the `PropertyDataType`
/// and so the `Variant` member the default lands in (STRING | INTEGER | DOUBLE |
/// BOOLEAN are the types a `Variant` has a scalar member for, from
/// `smithy_query(query="shape", shape="PropertyDataType,Variant")`), the form of
/// the declared text (negative, fractional, whole, one character), and whether the
/// definition declares an initial value at all (an attribute without a
/// `defaultValue`, and a measurement, which has no such member).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "STRING attribute with a default",
            name: "Serial",
            data_type: "STRING",
            kind: json!({"attribute": {"defaultValue": "SN-0000"}}),
            expected: Some(("stringValue", json!("SN-0000"))),
        },
        Row {
            case: "STRING attribute whose default is one character",
            name: "Tier",
            data_type: "STRING",
            kind: json!({"attribute": {"defaultValue": "A"}}),
            expected: Some(("stringValue", json!("A"))),
        },
        Row {
            case: "INTEGER attribute with a default",
            name: "Slots",
            data_type: "INTEGER",
            kind: json!({"attribute": {"defaultValue": "12"}}),
            expected: Some(("integerValue", json!(12))),
        },
        Row {
            case: "INTEGER attribute with a negative default",
            name: "Offset",
            data_type: "INTEGER",
            kind: json!({"attribute": {"defaultValue": "-7"}}),
            expected: Some(("integerValue", json!(-7))),
        },
        Row {
            case: "DOUBLE attribute with a fractional default",
            name: "Calibration",
            data_type: "DOUBLE",
            kind: json!({"attribute": {"defaultValue": "3.25"}}),
            expected: Some(("doubleValue", json!(3.25))),
        },
        Row {
            case: "DOUBLE attribute with a whole-number default",
            name: "Ratio",
            data_type: "DOUBLE",
            kind: json!({"attribute": {"defaultValue": "4"}}),
            expected: Some(("doubleValue", json!(4.0))),
        },
        Row {
            case: "BOOLEAN attribute defaulting to true",
            name: "Installed",
            data_type: "BOOLEAN",
            kind: json!({"attribute": {"defaultValue": "true"}}),
            expected: Some(("booleanValue", json!(true))),
        },
        Row {
            case: "BOOLEAN attribute defaulting to false",
            name: "Retired",
            data_type: "BOOLEAN",
            kind: json!({"attribute": {"defaultValue": "false"}}),
            expected: Some(("booleanValue", json!(false))),
        },
        Row {
            case: "attribute declaring no default",
            name: "Site",
            data_type: "STRING",
            kind: json!({"attribute": {}}),
            expected: None,
        },
        // A declared default that is no value of the property's data type: nothing
        // is invented for it, so the property is materialised with no value rather
        // than with a fabricated one. (`CreateAssetModel` does not judge a default
        // against the data type — that refusal is its own gap.)
        Row {
            case: "INTEGER attribute whose default is no integer",
            name: "Capacity",
            data_type: "INTEGER",
            kind: json!({"attribute": {"defaultValue": "not-a-number"}}),
            expected: None,
        },
        Row {
            case: "measurement, which declares no default",
            name: "Temperature",
            data_type: "DOUBLE",
            kind: json!({"measurement": {}}),
            expected: None,
        },
    ]
}

/// `rows()` as the `assetModelProperties` of a `CreateAssetModel` request.
fn definitions(rows: &[Row]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                json!({"name": row.name, "dataType": row.data_type, "type": row.kind})
            })
            .collect(),
    )
}

/// `GET /properties/latest` for one `(assetId, propertyId)` pair.
async fn latest(router: &MultiServiceRouter, asset_id: &str, property: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("{LATEST}?assetId={asset_id}&propertyId={property}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "GetAssetPropertyValue {property}: got {status} {body}"
    );
    body
}

/// The entry of `list` whose `entryId` is `entry_id`, if any.
fn find_entry<'a>(list: &'a Value, entry_id: &str) -> Option<&'a Value> {
    list.as_array()?
        .iter()
        .find(|entry| entry["entryId"] == entry_id)
}

/// An RFC 3339 instant, the form an `@httpQuery` timestamp with no
/// `@timestampFormat` takes on the wire.
fn query_date(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

#[tokio::test]
async fn a_new_assets_properties_start_at_the_values_their_definitions_declare() {
    let router = router();
    let table = rows();
    let model_id = create_model(&router, "Boiler", definitions(&table)).await;

    let before = now_seconds();
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;
    let asset = describe(&router, &asset_id).await;

    // One batch read carrying every row, so the two readers of a property's
    // current value are checked against the same store in the same state.
    let ids: Vec<String> = table
        .iter()
        .map(|row| property_id(&asset["assetProperties"], row.name))
        .collect();
    let entries: Vec<Value> = ids
        .iter()
        .enumerate()
        .map(|(at, property)| {
            json!({"entryId": format!("row{at}"), "assetId": asset_id, "propertyId": property})
        })
        .collect();
    let (status, batch) = call(
        &router,
        "POST",
        BATCH_LATEST,
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetAssetPropertyValue: got {status} {batch}"
    );

    for (at, row) in table.iter().enumerate() {
        let property = &ids[at];
        let reported = latest(&router, &asset_id, property).await;
        let entry_id = format!("row{at}");
        let entry = find_entry(&batch["successEntries"], &entry_id).unwrap_or_else(|| {
            panic!(
                "case {:?}: every entry names a stored property, so {entry_id} must be a success entry, got {batch}",
                row.case
            )
        });
        let after = now_seconds();

        match &row.expected {
            None => {
                assert!(
                    reported.get("propertyValue").is_none_or(Value::is_null),
                    "case {:?}: a definition declaring no value leaves the property with none, got {reported}",
                    row.case
                );
                assert!(
                    entry.get("assetPropertyValue").is_none(),
                    "case {:?}: the batch read must report no value either, got {entry}",
                    row.case
                );
            }
            Some((member, value)) => {
                let sample = &reported["propertyValue"];
                assert_eq!(
                    &sample["value"][member], value,
                    "case {:?}: expected the declared default as {member} {value}, got {reported}",
                    row.case
                );
                assert_eq!(
                    sample["value"].as_object().map(|value| value.len()),
                    Some(1),
                    "case {:?}: a Variant carries exactly one member, got {reported}",
                    row.case
                );
                assert_eq!(
                    sample["quality"].as_str(),
                    Some("GOOD"),
                    "case {:?}: expected quality GOOD, got {reported}",
                    row.case
                );
                let seconds = sample["timestamp"]["timeInSeconds"].as_i64();
                assert!(
                    seconds.is_some_and(|seconds| (before..=after).contains(&seconds)),
                    "case {:?}: expected a timestamp at the asset's creation ({before}..={after}), got {reported}",
                    row.case
                );
                let offset = sample["timestamp"]["offsetInNanos"].as_i64();
                assert!(
                    offset.is_some_and(|offset| (0..1_000_000_000).contains(&offset)),
                    "case {:?}: expected a nanosecond offset inside the second, got {reported}",
                    row.case
                );
                assert_eq!(
                    &entry["assetPropertyValue"], sample,
                    "case {:?}: the batch read must report the same sample, got {entry}",
                    row.case
                );
            }
        }
    }

    // Reading changes nothing a later caller sees: the same reads answer the same
    // way, so the default is stored state rather than something a read invents.
    for (at, row) in table.iter().enumerate() {
        let again = latest(&router, &asset_id, &ids[at]).await;
        let expected = match &row.expected {
            None => Value::Null,
            Some((member, value)) => json!({ *member: value }),
        };
        assert_eq!(
            again
                .get("propertyValue")
                .map(|sample| sample["value"].clone())
                .unwrap_or(Value::Null),
            expected,
            "case {:?}: a read must not change what the next caller reads, got {again}",
            row.case
        );
    }
}

#[tokio::test]
async fn two_assets_of_one_model_each_start_at_the_declared_default() {
    let router = router();
    let table = rows();
    let model_id = create_model(&router, "Press", definitions(&table)).await;

    // Every asset created from the model carries the declared value, so a second
    // asset is not reading the first one's series.
    for name in ["Press1", "Press2"] {
        let asset_id = create_asset(&router, name, &model_id).await;
        let asset = describe(&router, &asset_id).await;
        for row in table.iter().filter(|row| row.expected.is_some()) {
            let property = property_id(&asset["assetProperties"], row.name);
            let reported = latest(&router, &asset_id, &property).await;
            let (member, value) = row.expected.as_ref().expect("a declared default");
            assert_eq!(
                &reported["propertyValue"]["value"][member], value,
                "case {:?} on {name}: expected {member} {value}, got {reported}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_written_value_takes_over_from_the_declared_default() {
    let router = router();
    let table = rows();
    let model_id = create_model(&router, "Kiln", definitions(&table)).await;
    let asset_id = create_asset(&router, "Kiln1", &model_id).await;
    let asset = describe(&router, &asset_id).await;

    // One written sample per row that declares a default, at a timestamp newer
    // than the create — `BatchPutAssetPropertyValue` accepts a TQV up to ten
    // minutes ahead of now, so this stays inside the window. The written value
    // differs from the declared one in every row.
    let written: Vec<(&Row, String, Value)> = table
        .iter()
        .filter_map(|row| {
            let (member, _) = row.expected.as_ref()?;
            let fresh = match *member {
                "stringValue" => json!({"stringValue": "SN-9999"}),
                "integerValue" => json!({"integerValue": 999}),
                "doubleValue" => json!({"doubleValue": 99.75}),
                _ => json!({"booleanValue": true}),
            };
            Some((
                row,
                property_id(&asset["assetProperties"], row.name),
                fresh,
            ))
        })
        .collect();

    let put_at = now_seconds() + 120;
    for (row, property, value) in &written {
        let (status, body) = call(
            &router,
            "POST",
            BATCH_PUT,
            Some(json!({"entries": [{
                "entryId": "write",
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": [{
                    "value": value,
                    "timestamp": {"timeInSeconds": put_at, "offsetInNanos": 7},
                    "quality": "GOOD",
                }],
            }]})),
        )
        .await;
        assert_eq!(status, 200, "case {:?}: got {status} {body}", row.case);
        assert_eq!(
            body["errorEntries"].as_array().map_or(0, Vec::len),
            0,
            "case {:?}: the write was refused: {body}",
            row.case
        );
    }

    for (row, property, value) in &written {
        let reported = latest(&router, &asset_id, property).await;
        let member = row.expected.as_ref().expect("a declared default").0;
        assert_eq!(
            &reported["propertyValue"]["value"][member],
            &value[member],
            "case {:?}: a later write takes over as the current value, got {reported}",
            row.case
        );
        assert_eq!(
            reported["propertyValue"]["timestamp"]["timeInSeconds"].as_i64(),
            Some(put_at),
            "case {:?}: expected the written sample's timestamp, got {reported}",
            row.case
        );

        // The default is not erased by the write: it is the OLDER sample of the
        // series, so the history reports both, oldest first.
        let (status, history) = call(
            &router,
            "GET",
            &format!(
                "{HISTORY}?assetId={asset_id}&propertyId={property}&startDate={}&endDate={}",
                query_date(put_at - 3600),
                query_date(put_at + 3600)
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetAssetPropertyValueHistory got {status} {history}",
            row.case
        );
        let samples = history["assetPropertyValueHistory"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            samples.len(),
            2,
            "case {:?}: expected the declared default and the written sample, got {history}",
            row.case
        );
        let expected_default = &row.expected.as_ref().expect("a declared default").1;
        assert_eq!(
            &samples[0]["value"][member], expected_default,
            "case {:?}: the older sample is the declared default, got {history}",
            row.case
        );
        assert_eq!(
            &samples[1]["value"][member],
            &value[member],
            "case {:?}: the newer sample is the written value, got {history}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_property_an_update_adds_starts_at_the_default_it_declares() {
    let router = router();
    let table = rows();
    let model_id = create_model(&router, "Turbine", definitions(&table)).await;
    let asset_id = create_asset(&router, "Turbine1", &model_id).await;
    let asset = describe(&router, &asset_id).await;
    let serial = property_id(&asset["assetProperties"], "Serial");

    // The retained property is given a value of its OWN, distinct from the default
    // its definition declares, so the assertion below fails if the update re-seeds
    // a property the asset already carried.
    let written_at = now_seconds() + 120;
    let (status, body) = call(
        &router,
        "POST",
        BATCH_PUT,
        Some(json!({"entries": [{
            "entryId": "write",
            "assetId": asset_id,
            "propertyId": serial,
            "propertyValues": [{
                "value": {"stringValue": "SN-4242"},
                "timestamp": {"timeInSeconds": written_at},
                "quality": "GOOD",
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "seeding the retained property: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map_or(0, Vec::len),
        0,
        "seeding the retained property: the write was refused: {body}"
    );

    // The update keeps every existing definition and adds two: an attribute
    // declaring a default, and one declaring none.
    let (status, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    let mut properties: Vec<Value> = model["assetModelProperties"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    properties.push(json!({
        "name": "Firmware", "dataType": "STRING",
        "type": {"attribute": {"defaultValue": "v1.0.0"}},
    }));
    properties.push(json!({
        "name": "Operator", "dataType": "STRING", "type": {"attribute": {}},
    }));
    let before = now_seconds();
    let (status, answered) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(json!({"assetModelName": "Turbine", "assetModelProperties": properties})),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {answered}");

    let updated = describe(&router, &asset_id).await;
    let firmware = property_id(&updated["assetProperties"], "Firmware");
    let reported = latest(&router, &asset_id, &firmware).await;
    let after = now_seconds();
    assert_eq!(
        reported["propertyValue"]["value"]["stringValue"].as_str(),
        Some("v1.0.0"),
        "case \"attribute added by an update\": expected the declared default, got {reported}"
    );
    let seconds = reported["propertyValue"]["timestamp"]["timeInSeconds"].as_i64();
    assert!(
        seconds.is_some_and(|seconds| (before..=after).contains(&seconds)),
        "case \"attribute added by an update\": expected a timestamp at the materialisation ({before}..={after}), got {reported}"
    );

    let operator = property_id(&updated["assetProperties"], "Operator");
    let empty = latest(&router, &asset_id, &operator).await;
    assert!(
        empty.get("propertyValue").is_none_or(Value::is_null),
        "case \"attribute added without a default\": expected no value, got {empty}"
    );

    // A property the asset already carried keeps the value it already had rather
    // than being re-seeded by the update.
    let kept = latest(&router, &asset_id, &serial).await;
    assert_eq!(
        kept["propertyValue"]["value"]["stringValue"].as_str(),
        Some("SN-4242"),
        "case \"attribute the update kept\": expected the value written to it, got {kept}"
    );
    assert_eq!(
        kept["propertyValue"]["timestamp"]["timeInSeconds"].as_i64(),
        Some(written_at),
        "case \"attribute the update kept\": expected the written sample's timestamp, got {kept}"
    );
    let kept_again = latest(&router, &asset_id, &serial).await;
    assert_eq!(
        kept_again, kept,
        "case \"attribute the update kept\": its sample must be stable across reads, got {kept_again}"
    );
}

/// `GET /assets/{assetId}`, asserting it answered.
async fn describe(router: &MultiServiceRouter, asset_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {asset_id}: got {status} {body}");
    body
}
