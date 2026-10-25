// SPDX-License-Identifier: CC0-1.0

//! The time-series data plane — `BatchPutPropertyValues` and
//! `GetPropertyValueHistory` — end to end over HTTP.
//!
//! RULE under test: `BatchPutPropertyValues` stores each entry's values in the
//! series of the TIME SERIES property its `entityPropertyReference` resolves
//! to, under the instant each value is written for, and applies the entries it
//! can serve while reporting the ones it cannot in `errorEntries` — one error
//! per failed entry, carrying that entry. `GetPropertyValueHistory` reads those
//! values back: the ones its window (both bounds inclusive) and its
//! `propertyFilters` select, in the direction `orderByTime` names, a
//! `maxResults` page at a time, resampled when `interpolation` asks for it. A
//! property that is not a time series property has no history, so naming one is
//! refused rather than answered.
//!
//! The rows of each table below differ along what the rule generalises over:
//! the DATA TYPE the value travels in (`STRING`, `DOUBLE`, `INTEGER`, `LONG`,
//! `BOOLEAN`), the FORM the instant arrives in (an ISO 8601 `Z` instant, one
//! carrying a numeric UTC offset, one carrying a fractional second, and the
//! deprecated `timestamp`), WHICH component carries the property (a top-level
//! one addressed by `componentName` and a composite one addressed by its
//! `componentPath`), how many values and entries one call carries, and every
//! shape of entry the service cannot serve.

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "timeseries";

/// The component type whose property definitions mark one property of each data
/// type as a time series property, beside one (`Label`) that is not — the case
/// the data plane serves no value of.
const SENSOR: &str = "com.example.sensor";
/// A component type declaring a COMPOSITE component type of [`SENSOR`], so an
/// entity component of this type carries a composite component addressed by the
/// path `Rig/inner`.
const RIG: &str = "com.example.rig";

/// Instants one minute apart, as the ISO 8601 `Time` a write carries. Written
/// out rather than computed so a failing assertion names the instant it is
/// about.
const T0: &str = "2026-01-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-01-01T00:01:00Z";
/// See [`T0`].
const T2: &str = "2026-01-01T00:02:00Z";
/// See [`T0`].
const T3: &str = "2026-01-01T00:03:00Z";
/// See [`T0`].
const T4: &str = "2026-01-01T00:04:00Z";

/// The workspace, component types and entities every case below uses: one
/// entity carrying a top-level component of [`SENSOR`], and one carrying a
/// component of [`RIG`] with a composite component of [`SENSOR`] inside it.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    "Count": {"dataType": {"type": "INTEGER"}, "isTimeSeries": true},
                    "Ticks": {"dataType": {"type": "LONG"}, "isTimeSeries": true},
                    "Status": {"dataType": {"type": "STRING"}, "isTimeSeries": true},
                    "Online": {"dataType": {"type": "BOOLEAN"}, "isTimeSeries": true},
                    "Label": {"dataType": {"type": "STRING"}},
                    "Tag": {"dataType": {"type": "STRING"}, "isExternalId": true},
                },
            }),
        ),
        (
            RIG,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": SENSOR}}}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let entities: Vec<Value> = vec![
        json!({
            "entityId": "unit-1",
            "entityName": "unit-1",
            "components": {"Sensor": {
                "componentTypeId": SENSOR,
                // The external id this component is addressed by where a
                // reference names no entityId.
                "properties": {"Tag": {"value": {"stringValue": "sensor-tag"}}},
            }},
        }),
        json!({
            "entityId": "rig-1",
            "entityName": "rig-1",
            "components": {"Rig": {"componentTypeId": RIG}},
            "compositeComponents": {"Rig/inner": {}},
        }),
    ];
    for body in entities {
        let created = emulator.create_entity(WORKSPACE, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {body:?}: expected 200, got {created:?}"
        );
    }
}

/// One entry writing `values` to the property `property` of `unit-1`'s
/// top-level `Sensor` component.
fn sensor_entry(property: &str, values: Value) -> Value {
    json!({
        "entityPropertyReference": {
            "entityId": "unit-1",
            "componentName": "Sensor",
            "propertyName": property,
        },
        "propertyValues": values,
    })
}

/// The whole history of one property of `unit-1`'s `Sensor` component, with
/// `extra` merged into the request so a case can add a window, a filter or a
/// page size.
async fn sensor_history(emulator: &Emulator, property: &str, extra: Value) -> crate::harness::Resp {
    let mut body = json!({
        "entityId": "unit-1",
        "componentName": "Sensor",
        "selectedProperties": [property],
    });
    if let Some(extra) = extra.as_object() {
        for (member, value) in extra {
            body[member] = value.clone();
        }
    }
    emulator.get_property_value_history(WORKSPACE, body).await
}

/// Every `(time, value)` pair a history answer reports, series by series, in
/// the order it reports them.
fn reported(response: &crate::harness::Resp) -> Vec<(String, Value)> {
    response
        .member("propertyValues")
        .as_array()
        .map(|series| {
            series
                .iter()
                .flat_map(|one| {
                    one["values"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                })
                .map(|value| {
                    (
                        value["time"].as_str().unwrap_or_default().to_string(),
                        value["value"].clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the round-trip table: a value of one data type, written with one
/// form of instant, read back as the value and the instant text the write
/// carried.
#[derive(Debug)]
struct RoundTrip {
    case: &'static str,
    property: &'static str,
    /// The whole modelled `PropertyValue` the entry carries.
    written: fn() -> Value,
    /// The `time` the read reports: the text the write carried, or the
    /// rendering of the instant for a write that carried only the deprecated
    /// `timestamp`.
    expected_time: &'static str,
    /// The `DataValue` the read reports.
    expected_value: fn() -> Value,
    /// A window that holds the value, so the instant the write was READ as is
    /// asserted and not only the text it was echoed with.
    window: fn() -> Value,
}

/// RULE: a value written to a time series property is read back by
/// `GetPropertyValueHistory` as the value and the instant it was written with,
/// whatever data type the value travels in and whatever form the instant
/// arrived in.
#[tokio::test]
async fn a_written_value_reads_back_for_every_data_type_and_instant_form() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: [RoundTrip; 5] = [
        RoundTrip {
            case: "a STRING value at a Z instant",
            property: "Status",
            written: || json!({"time": T0, "value": {"stringValue": "idle"}}),
            expected_time: T0,
            expected_value: || json!({"stringValue": "idle"}),
            window: || json!({"startTime": T0, "endTime": T0}),
        },
        RoundTrip {
            case: "a DOUBLE value at an instant carrying a fractional second",
            property: "Temp",
            written: || json!({"time": "2026-01-01T00:00:00.250Z", "value": {"doubleValue": 21.5}}),
            expected_time: "2026-01-01T00:00:00.250Z",
            expected_value: || json!({"doubleValue": 21.5}),
            window: || json!({"startTime": T0, "endTime": T1}),
        },
        RoundTrip {
            // The instant is T0 expressed in a +02:00 zone, so a window of
            // exactly T0 holding it is what proves the offset was read.
            case: "an INTEGER value at an instant carrying a numeric UTC offset",
            property: "Count",
            written: || json!({"time": "2026-01-01T02:00:00+02:00", "value": {"integerValue": 7}}),
            expected_time: "2026-01-01T02:00:00+02:00",
            expected_value: || json!({"integerValue": 7}),
            window: || json!({"startTime": T0, "endTime": T0}),
        },
        RoundTrip {
            // 1767312000 is 2026-01-02T00:00:00Z: the deprecated member
            // carries the instant as epoch seconds, and the read reports it as
            // the `time` that replaced it.
            case: "a LONG value at an instant carried by the deprecated timestamp member",
            property: "Ticks",
            written: || json!({"timestamp": 1_767_312_000, "value": {"longValue": 90}}),
            expected_time: "2026-01-02T00:00:00Z",
            expected_value: || json!({"longValue": 90}),
            window: || json!({"startTime": "2026-01-02T00:00:00Z", "endTime": "2026-01-02T00:00:00Z"}),
        },
        RoundTrip {
            case: "a BOOLEAN value at a Z instant",
            property: "Online",
            written: || json!({"time": T1, "value": {"booleanValue": true}}),
            expected_time: T1,
            expected_value: || json!({"booleanValue": true}),
            window: || json!({"startTime": T1, "endTime": T1}),
        },
    ];

    for row in &rows {
        let written = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([sensor_entry(row.property, json!([(row.written)()]))]),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {:?}: expected 200, got {written:?}",
            row.case
        );
        assert_eq!(
            written.member("errorEntries"),
            &json!([]),
            "case {:?}: a servable entry earns no error, got {written:?}",
            row.case
        );

        let read = sensor_history(&emulator, row.property, (row.window)()).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from the history, got {read:?}",
            row.case
        );
        let values = reported(&read);
        assert_eq!(
            values,
            vec![(row.expected_time.to_string(), (row.expected_value)())],
            "case {:?}: expected the written value back, got {read:?}",
            row.case
        );
    }
}

/// One entry of the partial-failure batch: what it writes, and the error code
/// it earns — `None` for an entry the service serves.
#[derive(Debug)]
struct BatchCase {
    case: &'static str,
    entry: fn() -> Value,
    expected_code: Option<&'static str>,
}

/// RULE: every entry is judged on its own, so a batch mixing entries the
/// service can serve with entries it cannot stores the servable ones and
/// reports the rest in `errorEntries` — a reference naming an entity or a
/// component the workspace does not hold as a `ResourceNotFoundException`, and
/// an entry the service cannot be asked to serve at all as a
/// `ValidationException`.
#[tokio::test]
async fn a_batch_applies_the_servable_entries_and_reports_the_rest() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let cases: [BatchCase; 9] = [
        BatchCase {
            case: "a servable write to a top-level component",
            entry: || sensor_entry("Temp", json!([{"time": T0, "value": {"doubleValue": 1.5}}])),
            expected_code: None,
        },
        BatchCase {
            case: "a servable write to a composite component",
            entry: || {
                json!({
                    "entityPropertyReference": {
                        "entityId": "rig-1",
                        "componentPath": "Rig/inner",
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": 2.5}}],
                })
            },
            expected_code: None,
        },
        BatchCase {
            case: "a reference to an entity the workspace does not hold",
            entry: || {
                json!({
                    "entityPropertyReference": {
                        "entityId": "ghost-9",
                        "componentName": "Sensor",
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": 3.5}}],
                })
            },
            expected_code: Some("ResourceNotFoundException"),
        },
        BatchCase {
            case: "a reference to a component the entity does not have",
            entry: || {
                json!({
                    "entityPropertyReference": {
                        "entityId": "unit-1",
                        "componentName": "Absent",
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": 4.5}}],
                })
            },
            expected_code: Some("ResourceNotFoundException"),
        },
        BatchCase {
            case: "a reference to a property the component does not have",
            entry: || sensor_entry("Nowhere", json!([{"time": T0, "value": {"doubleValue": 5.5}}])),
            expected_code: Some("ValidationException"),
        },
        BatchCase {
            case: "a reference to a property that is not a time series property",
            entry: || sensor_entry("Label", json!([{"time": T0, "value": {"stringValue": "tag"}}])),
            expected_code: Some("ValidationException"),
        },
        BatchCase {
            case: "a value of another data type than the property's",
            entry: || sensor_entry("Count", json!([{"time": T0, "value": {"stringValue": "seven"}}])),
            expected_code: Some("ValidationException"),
        },
        BatchCase {
            case: "a value carrying neither time nor timestamp",
            entry: || sensor_entry("Ticks", json!([{"value": {"longValue": 6}}])),
            expected_code: Some("ValidationException"),
        },
        BatchCase {
            case: "a value whose time is not an ISO8601 DateTime",
            entry: || {
                sensor_entry(
                    "Status",
                    json!([{"time": "not-a-timestamp-at-all", "value": {"stringValue": "x"}}]),
                )
            },
            expected_code: Some("ValidationException"),
        },
    ];

    let entries: Vec<Value> = cases.iter().map(|case| (case.entry)()).collect();
    let written = emulator
        .batch_put_property_values(WORKSPACE, json!(entries))
        .await;
    assert_eq!(
        written.status, 200,
        "a partially servable batch is answered, not refused: got {written:?}"
    );

    // Every reported error, as the entry it is about and the code it carries.
    let reported_errors: Vec<(Value, String)> = written
        .member("errorEntries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|entry| entry["errors"].as_array().cloned().unwrap_or_default())
        .map(|error| {
            (
                error["entry"].clone(),
                error["errorCode"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();

    for case in &cases {
        let entry = (case.entry)();
        let found = reported_errors
            .iter()
            .find(|(reported, _)| reported == &entry)
            .map(|(_, code)| code.as_str());
        assert_eq!(
            found, case.expected_code,
            "case {:?}: expected the error code {:?}, got {found:?} in {written:?}",
            case.case, case.expected_code
        );
    }
    let expected_errors = cases
        .iter()
        .filter(|case| case.expected_code.is_some())
        .count();
    assert_eq!(
        reported_errors.len(),
        expected_errors,
        "expected {expected_errors} errors, got {written:?}"
    );

    // The servable entries were applied...
    let temp = sensor_history(&emulator, "Temp", json!({})).await;
    assert_eq!(
        reported(&temp),
        vec![(T0.to_string(), json!({"doubleValue": 1.5}))],
        "the servable top-level write is readable back, got {temp:?}"
    );
    let composite = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({
                "entityId": "rig-1",
                "componentPath": "Rig/inner",
                "selectedProperties": ["Temp"],
            }),
        )
        .await;
    assert_eq!(
        reported(&composite),
        vec![(T0.to_string(), json!({"doubleValue": 2.5}))],
        "the servable composite write is readable back, got {composite:?}"
    );
    // ...and the unservable ones stored nothing.
    for property in ["Count", "Ticks", "Status"] {
        let read = sensor_history(&emulator, property, json!({})).await;
        assert_eq!(
            reported(&read),
            Vec::new(),
            "property {property:?}: an unservable entry stores nothing, got {read:?}"
        );
    }
}

/// RULE: the instant is the value's identity, so a value written for an instant
/// the series already holds replaces it rather than being appended beside it.
#[tokio::test]
async fn a_value_written_for_a_held_instant_replaces_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let first = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([sensor_entry(
                "Temp",
                json!([{"time": T0, "value": {"doubleValue": 10.0}}])
            )]),
        )
        .await;
    assert_eq!(first.status, 200, "the first write: got {first:?}");

    let second = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([sensor_entry(
                "Temp",
                json!([
                    {"time": T0, "value": {"doubleValue": 20.0}},
                    {"time": T1, "value": {"doubleValue": 30.0}},
                ])
            )]),
        )
        .await;
    assert_eq!(second.status, 200, "the second write: got {second:?}");

    let read = sensor_history(&emulator, "Temp", json!({})).await;
    assert_eq!(
        reported(&read),
        vec![
            (T0.to_string(), json!({"doubleValue": 20.0})),
            (T1.to_string(), json!({"doubleValue": 30.0})),
        ],
        "the instant written twice holds one value, got {read:?}"
    );
}

/// One row of the selection table: what the read asks for beside the property,
/// and the values it reports.
#[derive(Debug)]
struct Selection {
    case: &'static str,
    request: fn() -> Value,
    expected: fn() -> Vec<(String, Value)>,
}

/// One value of the seeded series, as the pair a read reports.
fn seeded(time: &str, value: f64) -> (String, Value) {
    (time.to_string(), json!({"doubleValue": value}))
}

/// RULE: a history read reports the values its window (both bounds inclusive)
/// and its `propertyFilters` select, in the direction `orderByTime` names.
#[tokio::test]
async fn the_window_the_filters_and_the_order_select_what_a_history_reports() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([sensor_entry(
                "Temp",
                json!([
                    {"time": T0, "value": {"doubleValue": 10.0}},
                    {"time": T1, "value": {"doubleValue": 20.0}},
                    {"time": T2, "value": {"doubleValue": 30.0}},
                    {"time": T3, "value": {"doubleValue": 40.0}},
                    {"time": T4, "value": {"doubleValue": 50.0}},
                ])
            )]),
        )
        .await;
    assert_eq!(written.status, 200, "seeding the series: got {written:?}");

    let rows: [Selection; 7] = [
        Selection {
            case: "no window at all",
            request: || json!({}),
            expected: || {
                vec![
                    seeded(T0, 10.0),
                    seeded(T1, 20.0),
                    seeded(T2, 30.0),
                    seeded(T3, 40.0),
                    seeded(T4, 50.0),
                ]
            },
        },
        Selection {
            case: "a window whose bounds are values of the series",
            request: || json!({"startTime": T1, "endTime": T3}),
            expected: || vec![seeded(T1, 20.0), seeded(T2, 30.0), seeded(T3, 40.0)],
        },
        Selection {
            case: "a window open at its earlier end",
            request: || json!({"endTime": T1}),
            expected: || vec![seeded(T0, 10.0), seeded(T1, 20.0)],
        },
        Selection {
            // 1767225660 is T1 as epoch seconds: the deprecated member bounds
            // the window the same way the ISO 8601 one does.
            case: "a window bounded by the deprecated startDateTime",
            request: || json!({"startDateTime": 1_767_225_660i64}),
            expected: || {
                vec![
                    seeded(T1, 20.0),
                    seeded(T2, 30.0),
                    seeded(T3, 40.0),
                    seeded(T4, 50.0),
                ]
            },
        },
        Selection {
            case: "newest first",
            request: || json!({"orderByTime": "DESCENDING"}),
            expected: || {
                vec![
                    seeded(T4, 50.0),
                    seeded(T3, 40.0),
                    seeded(T2, 30.0),
                    seeded(T1, 20.0),
                    seeded(T0, 10.0),
                ]
            },
        },
        Selection {
            case: "a filter on the value",
            request: || {
                json!({"propertyFilters": [
                    {"propertyName": "Temp", "operator": ">", "value": {"doubleValue": 30.0}},
                ]})
            },
            expected: || vec![seeded(T3, 40.0), seeded(T4, 50.0)],
        },
        Selection {
            case: "a filter naming no operator, which compares for equality",
            request: || {
                json!({"propertyFilters": [
                    {"propertyName": "Temp", "value": {"doubleValue": 30.0}},
                ]})
            },
            expected: || vec![seeded(T2, 30.0)],
        },
    ];

    for row in &rows {
        let read = sensor_history(&emulator, "Temp", (row.request)()).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200, got {read:?}",
            row.case
        );
        assert_eq!(
            reported(&read),
            (row.expected)(),
            "case {:?}: the wrong values came back, got {read:?}",
            row.case
        );
        assert!(
            read.member("nextToken").is_null(),
            "case {:?}: a whole answer carries no nextToken, got {read:?}",
            row.case
        );
    }
}

/// RULE: `maxResults` caps how many values one answer carries and `nextToken`
/// resumes at the next one, so paging a history walks every value exactly once.
#[tokio::test]
async fn a_history_pages_through_every_value_once() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([sensor_entry(
                "Temp",
                json!([
                    {"time": T0, "value": {"doubleValue": 10.0}},
                    {"time": T1, "value": {"doubleValue": 20.0}},
                    {"time": T2, "value": {"doubleValue": 30.0}},
                    {"time": T3, "value": {"doubleValue": 40.0}},
                    {"time": T4, "value": {"doubleValue": 50.0}},
                ])
            )]),
        )
        .await;
    assert_eq!(written.status, 200, "seeding the series: got {written:?}");

    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..5 {
        let mut request = json!({"maxResults": 2});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let read = sensor_history(&emulator, "Temp", request).await;
        assert_eq!(
            read.status, 200,
            "page {page}: expected 200, got {read:?}"
        );
        let values = reported(&read);
        assert!(
            values.len() <= 2,
            "page {page}: expected at most 2 values, got {read:?}"
        );
        walked.extend(values);
        token = read
            .member("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(token, None, "the walk ends with no further page: {walked:?}");
    assert_eq!(
        walked,
        vec![
            seeded(T0, 10.0),
            seeded(T1, 20.0),
            seeded(T2, 30.0),
            seeded(T3, 40.0),
            seeded(T4, 50.0),
        ],
        "paging walks every value once, in time order"
    );
}

/// RULE: `interpolation` reports the series resampled at a fixed interval
/// across the window — a point where a value was written carries that value and
/// a point between two of them carries the value `LINEAR` interpolation puts
/// there.
#[tokio::test]
async fn linear_interpolation_resamples_the_window() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([sensor_entry(
                "Temp",
                json!([
                    {"time": T0, "value": {"doubleValue": 10.0}},
                    {"time": T2, "value": {"doubleValue": 30.0}},
                ])
            )]),
        )
        .await;
    assert_eq!(written.status, 200, "seeding the series: got {written:?}");

    let read = sensor_history(
        &emulator,
        "Temp",
        json!({
            "startTime": T0,
            "endTime": T2,
            "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 60},
        }),
    )
    .await;
    assert_eq!(read.status, 200, "an interpolated read: got {read:?}");
    assert_eq!(
        reported(&read),
        vec![
            (T0.to_string(), json!({"doubleValue": 10.0})),
            // Halfway between the two written values, one minute after the
            // first: the point `LINEAR` interpolation puts there.
            (T1.to_string(), json!({"doubleValue": 20.0})),
            (T2.to_string(), json!({"doubleValue": 30.0})),
        ],
        "expected the series resampled every 60 seconds, got {read:?}"
    );
}

/// RULE: a cross-entity read — one naming a `componentTypeId` and no entity —
/// reports one series per INSTANCE of that component type, each carrying the
/// reference that names the component it came out of.
#[tokio::test]
async fn a_component_type_read_reports_every_instance_of_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([
                sensor_entry("Temp", json!([{"time": T0, "value": {"doubleValue": 11.0}}])),
                {
                    "entityPropertyReference": {
                        "entityId": "rig-1",
                        "componentPath": "Rig/inner",
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T1, "value": {"doubleValue": 22.0}}],
                },
            ]),
        )
        .await;
    assert_eq!(written.status, 200, "seeding both instances: got {written:?}");
    assert_eq!(
        written.member("errorEntries"),
        &json!([]),
        "both entries are servable, got {written:?}"
    );

    let read = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({"componentTypeId": SENSOR, "selectedProperties": ["Temp"]}),
        )
        .await;
    assert_eq!(read.status, 200, "a cross-entity read: got {read:?}");

    // One series per instance, each named by its own reference: the composite
    // one by its `componentPath`, the top-level one by its `componentName`.
    let series: Vec<(Value, Vec<(String, Value)>)> = read
        .member("propertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|one| {
            let values = one["values"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|value| {
                    (
                        value["time"].as_str().unwrap_or_default().to_string(),
                        value["value"].clone(),
                    )
                })
                .collect();
            (one["entityPropertyReference"].clone(), values)
        })
        .collect();
    assert_eq!(
        series,
        vec![
            (
                json!({
                    "entityId": "rig-1",
                    "componentName": "inner",
                    "componentPath": "Rig/inner",
                    "propertyName": "Temp",
                }),
                vec![(T1.to_string(), json!({"doubleValue": 22.0}))],
            ),
            (
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "propertyName": "Temp",
                }),
                vec![(T0.to_string(), json!({"doubleValue": 11.0}))],
            ),
        ],
        "expected one series per instance of {SENSOR}, got {read:?}"
    );
}

/// One refused request: what it asks for, and the status and error code it has
/// to be answered with.
#[derive(Debug)]
struct Refusal {
    case: &'static str,
    workspace_id: &'static str,
    request: fn() -> Value,
    status: u16,
    error: &'static str,
}

/// RULE: a history read that addresses nothing readable is refused, naming what
/// about it cannot be served — a property that is not a time series property,
/// or is none of the addressed component's, and an unreadable window, order,
/// interpolation, page size or token are `ValidationException`s; a workspace,
/// entity, component or component type the store does not hold is a
/// `ResourceNotFoundException`.
#[tokio::test]
async fn a_history_read_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let refusals: [Refusal; 14] = [
        Refusal {
            case: "a window whose start comes after its end",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                    "startTime": T3,
                    "endTime": T1,
                })
            },
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "a property that is not a time series property",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Label"],
                })
            },
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "a property the component does not have",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Nowhere"],
                })
            },
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "an entity the workspace does not hold",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "ghost-9",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                })
            },
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            case: "a component the entity does not have",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Absent",
                    "selectedProperties": ["Temp"],
                })
            },
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            case: "a componentPath no component of the entity sits at",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "rig-1",
                    "componentPath": "Rig/elsewhere",
                    "selectedProperties": ["Temp"],
                })
            },
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            case: "a workspace the store does not hold",
            workspace_id: "nosuchworkspace",
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                })
            },
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            case: "a componentType the workspace does not hold",
            workspace_id: WORKSPACE,
            request: || json!({"componentTypeId": "com.example.absent", "selectedProperties": ["Temp"]}),
            status: 404,
            error: "ResourceNotFoundException",
        },
        Refusal {
            case: "a request addressing neither an entity nor a component type",
            workspace_id: WORKSPACE,
            request: || json!({"selectedProperties": ["Temp"]}),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "an entity with no component named",
            workspace_id: WORKSPACE,
            request: || json!({"entityId": "unit-1", "selectedProperties": ["Temp"]}),
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "a time direction the model does not define",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                    "orderByTime": "SIDEWAYS",
                })
            },
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "a startTime that is not an ISO8601 DateTime",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                    "startTime": "this-is-not-a-time!!",
                })
            },
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "an interpolation with no window to space its points across",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                    "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 60},
                })
            },
            status: 400,
            error: "ValidationException",
        },
        Refusal {
            case: "a page size of zero, which the member's minimum of 1 refuses",
            workspace_id: WORKSPACE,
            request: || {
                json!({
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                    "maxResults": 0,
                })
            },
            status: 400,
            error: "ValidationException",
        },
    ];

    for refusal in &refusals {
        let read = emulator
            .get_property_value_history(refusal.workspace_id, (refusal.request)())
            .await;
        assert_eq!(
            read.status, refusal.status,
            "case {:?}: expected status {}, got {read:?}",
            refusal.case, refusal.status
        );
        assert_eq!(
            read.error_type.as_deref(),
            Some(refusal.error),
            "case {:?}: expected the error code {}, got {read:?}",
            refusal.case,
            refusal.error
        );
    }
}

/// RULE: a reference carrying no `entityId` names the component by the EXTERNAL
/// ID one of its properties is, so the write lands on the component carrying
/// that id — and on nothing at all when no component carries it.
#[tokio::test]
async fn an_external_id_reference_addresses_the_component_carrying_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([
                {
                    "entityPropertyReference": {
                        "externalIdProperty": {"Tag": "sensor-tag"},
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": 12.0}}],
                },
                {
                    "entityPropertyReference": {
                        "externalIdProperty": {"Tag": "carried-by-nobody"},
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T1, "value": {"doubleValue": 13.0}}],
                },
            ]),
        )
        .await;
    assert_eq!(written.status, 200, "an external-id write: got {written:?}");

    let codes: Vec<String> = written
        .member("errorEntries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|entry| entry["errors"].as_array().cloned().unwrap_or_default())
        .map(|error| error["errorCode"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        codes,
        vec!["ResourceNotFoundException".to_string()],
        "only the id nothing carries earns an error, got {written:?}"
    );

    let read = sensor_history(&emulator, "Temp", json!({})).await;
    assert_eq!(
        reported(&read),
        vec![(T0.to_string(), json!({"doubleValue": 12.0}))],
        "the external-id write lands on the component carrying the id, got {read:?}"
    );
}

/// RULE: the interpolated points one ANSWER carries are capped across every
/// series in it, so a request whose scope multiplies a window that is fine for
/// one series past that ceiling is refused rather than computed.
#[tokio::test]
async fn an_interpolation_too_large_for_one_answer_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([
                sensor_entry("Temp", json!([{"time": T0, "value": {"doubleValue": 1.0}}])),
                {
                    "entityPropertyReference": {
                        "entityId": "rig-1",
                        "componentPath": "Rig/inner",
                        "propertyName": "Temp",
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": 2.0}}],
                },
            ]),
        )
        .await;
    assert_eq!(written.status, 200, "seeding both instances: got {written:?}");

    // 6001 points per series — inside the ceiling on its own — times the two
    // instances of the component type is past it.
    let read = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": "2026-01-01T01:40:00Z",
                "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 1},
            }),
        )
        .await;
    assert_eq!(
        read.status, 400,
        "an answer-wide interpolation past the ceiling: expected 400, got {read:?}"
    );
    assert_eq!(
        read.error_type.as_deref(),
        Some("ValidationException"),
        "an answer-wide interpolation past the ceiling: expected ValidationException, got {read:?}"
    );
}

/// RULE: the workspace is the resource the request is addressed to, so a write
/// to one the store does not hold is the operation's own
/// `ResourceNotFoundException` rather than a per-entry error.
#[tokio::test]
async fn a_write_to_an_unheld_workspace_is_refused_whole() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let written = emulator
        .batch_put_property_values(
            "nosuchworkspace",
            json!([sensor_entry(
                "Temp",
                json!([{"time": T0, "value": {"doubleValue": 1.0}}])
            )]),
        )
        .await;
    assert_eq!(
        written.status, 404,
        "a write to an unheld workspace: expected 404, got {written:?}"
    );
    assert_eq!(
        written.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "a write to an unheld workspace: expected ResourceNotFoundException, got {written:?}"
    );
}
