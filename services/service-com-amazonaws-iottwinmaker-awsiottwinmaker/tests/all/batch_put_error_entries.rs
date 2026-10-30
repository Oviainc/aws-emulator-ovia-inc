// SPDX-License-Identifier: CC0-1.0

//! What `BatchPutPropertyValues` reports in `errorEntries`, batch shape by batch
//! shape.
//!
//! RULE under test: `errorEntries` is a per-entry OUTCOME report, so it carries
//! exactly one `BatchPutPropertyErrorEntry` per entry of the request whose
//! values the service could not record and nothing else — which makes it the
//! present, EMPTY list on a batch every entry of which was stored, and makes its
//! length the count of failed entries, never a figure the service pads to.
//! `ErrorEntries`' modelled `@length(1..10)` is a bound on the errors there ARE:
//! its minimum is what every entry the list DOES carry satisfies (one
//! `BatchPutPropertyError`, itself inside `Errors`' own `@length(1..10)`), and
//! its maximum is reached by a batch of `Entries`' modelled ten entries none of
//! which is servable. The service does not invent an error no entry earned to
//! reach a minimum, because an error is a report about one entry of the request
//! and there is no entry to report it against.
//!
//! The rows below differ along what the rule generalises over: HOW MANY entries
//! of one batch are servable and how many are not — from a batch wholly
//! servable, through every mixture, to one wholly unservable at `Entries`'
//! modelled maximum — and the CLASS of each unservable entry.
//!
//! Every modelled length cited here is from
//! `smithy_query(query="shape", shape="ErrorEntries,Errors,Entries,PropertyValues,BatchPutPropertyError")`.
//!
//! `GetPropertyValueHistory` reads the servable entries of every batch back, so
//! what the report of failures says is checked against what the call applied.

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "batchput";

/// The component type whose property definitions mark one property of each data
/// type this suite writes as a time series property.
const SENSOR: &str = "com.example.sensor";

/// The entity every entry below addresses.
const ENTITY: &str = "unit-1";

/// The fewest and the most entries one `BatchPutPropertyValues` request carries:
/// `Entries` is `@length(1..10)`.
const MIN_ENTRIES: usize = 1;
/// See [`MIN_ENTRIES`].
const MAX_ENTRIES: usize = 10;

/// The fewest and the most elements `ErrorEntries` admits — the same bounds
/// `Errors`, the `errors` member of every entry of it, admits: both are
/// `@length(1..10)`.
const MIN_REPORTED: usize = 1;
/// See [`MIN_REPORTED`].
const MAX_REPORTED: usize = 10;

/// The fewest and the most characters `BatchPutPropertyError$errorCode` and
/// `$errorMessage` admit: both are `@length(1..256)`.
const MIN_ERROR_TEXT_CHARS: usize = 1;
/// See [`MIN_ERROR_TEXT_CHARS`].
const MAX_ERROR_TEXT_CHARS: usize = 256;

/// The `errorCode` a per-entry failure carries: the name of the failure the
/// operation models for a request it cannot serve, reported per entry.
const ERROR_CODE: &str = "ValidationException";

/// The workspace, component type and entity every case below writes to.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let created = emulator
        .create_component_type(
            WORKSPACE,
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    "Count": {"dataType": {"type": "INTEGER"}, "isTimeSeries": true},
                    "Ticks": {"dataType": {"type": "LONG"}, "isTimeSeries": true},
                    "Status": {"dataType": {"type": "STRING"}, "isTimeSeries": true},
                },
                // The write connector a value is set THROUGH: without it no
                // write to this component type's properties is accepted.
                "functions": data_writer_functions(),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType: expected 200, got {created:?}"
    );

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": ENTITY,
                "entityName": ENTITY,
                "components": {"Sensor": {"componentTypeId": SENSOR}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity: expected 200, got {created:?}"
    );
}

/// The instant the entry at index `index` of a batch writes for: one minute
/// apart, so every entry of one batch is a distinct value of the series and
/// every entry's JSON is distinct from its neighbours'.
fn instant(index: usize) -> String {
    format!("2026-01-01T00:{index:02}:00Z")
}

/// One entry of a batch writing a `DOUBLE` to the time series property `Temp`,
/// which the service records.
fn servable_entry(index: usize) -> Value {
    sensor_entry(
        "Temp",
        json!([{"time": instant(index), "value": {"doubleValue": index as f64}}]),
    )
}

/// One entry of a batch whose VALUES the service cannot record, cycling through
/// the classes of per-entry failure the operation reports: a value of another
/// data type than the property's, a value naming no instant at all, and an
/// instant that is no ISO 8601 DateTime. `index` distinguishes the entry from
/// every other one of the batch, so a reported error is matched back to the
/// entry it is about.
fn unservable_entry(index: usize) -> Value {
    match index % 3 {
        0 => sensor_entry(
            "Count",
            json!([{"time": instant(index), "value": {"stringValue": format!("not-a-number-{index}")}}]),
        ),
        1 => sensor_entry("Ticks", json!([{"value": {"longValue": index}}])),
        // `Time` is `@length(20..35)`, so the text that is no instant has to be
        // at least twenty characters long to reach the handler at all.
        _ => sensor_entry(
            "Status",
            json!([{"time": format!("not-an-instant-at-all-{index:02}"), "value": {"stringValue": "x"}}]),
        ),
    }
}

/// The one further class of unservable entry a batch can carry only once: an
/// entry naming no `propertyValues` MEMBER at all, on a property that resolves
/// and is writable, so the entry fails on its values and not on its reference.
/// (It cannot be repeated with an empty list instead, because `PropertyValues`
/// is `@length(1..10)` on the way IN, so an empty list refuses the whole request
/// at the boundary.)
fn entry_without_values() -> Value {
    json!({
        "entityPropertyReference": {
            "entityId": ENTITY,
            "componentName": "Sensor",
            "propertyName": "Status",
        },
    })
}

/// One entry writing `values` to the property `property` of the entity's
/// top-level `Sensor` component.
fn sensor_entry(property: &str, values: Value) -> Value {
    json!({
        "entityPropertyReference": {
            "entityId": ENTITY,
            "componentName": "Sensor",
            "propertyName": property,
        },
        "propertyValues": values,
    })
}

/// One row: the SHAPE of one batch, as the counts of entries of each kind it
/// carries.
#[derive(Debug)]
struct BatchShape {
    case: &'static str,
    /// How many entries of the batch the service records.
    servable: usize,
    /// How many entries of the batch carry values it cannot record.
    unservable: usize,
    /// Whether the batch also carries the one entry naming no `propertyValues`
    /// member ([`entry_without_values`]).
    without_values: bool,
}

impl BatchShape {
    /// The entries this shape sends, the servable ones first so the order the
    /// errors are reported in is not the order the entries arrive in.
    fn entries(&self) -> Vec<Value> {
        let mut entries: Vec<Value> = (0..self.servable).map(servable_entry).collect();
        entries.extend((0..self.unservable).map(unservable_entry));
        if self.without_values {
            entries.push(entry_without_values());
        }
        entries
    }

    /// The entries this shape expects back in `errorEntries`, each exactly as it
    /// was sent.
    fn expected_failed_entries(&self) -> Vec<Value> {
        let mut failed: Vec<Value> = (0..self.unservable).map(unservable_entry).collect();
        if self.without_values {
            failed.push(entry_without_values());
        }
        failed
    }
}

/// Every reported error, flattened: the entry it is about, paired with the
/// `errorCode` it carries.
fn reported_errors(written: &Resp) -> Vec<(Value, String)> {
    written
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
        .collect()
}

/// RULE: `errorEntries` carries exactly one entry per entry of the request whose
/// values could not be recorded — so it is the present, empty list when every
/// entry was stored, and its length is the count of failed entries at every
/// mixture up to `Entries`' modelled maximum of ten.
#[tokio::test]
async fn error_entries_holds_one_entry_per_failed_entry_and_nothing_else() {
    let rows: [BatchShape; 8] = [
        BatchShape {
            case: "one servable entry",
            servable: 1,
            unservable: 0,
            without_values: false,
        },
        BatchShape {
            case: "several servable entries",
            servable: 4,
            unservable: 0,
            without_values: false,
        },
        BatchShape {
            case: "the modelled maximum batch, every entry servable",
            servable: MAX_ENTRIES,
            unservable: 0,
            without_values: false,
        },
        BatchShape {
            case: "one servable entry beside one unservable one",
            servable: 1,
            unservable: 1,
            without_values: false,
        },
        BatchShape {
            case: "the modelled maximum batch with one unservable entry",
            servable: MAX_ENTRIES - 1,
            unservable: 1,
            without_values: false,
        },
        BatchShape {
            case: "the modelled maximum batch with one servable entry",
            servable: 1,
            unservable: MAX_ENTRIES - 1,
            without_values: false,
        },
        BatchShape {
            case: "the modelled maximum batch, no entry servable",
            servable: 0,
            unservable: MAX_ENTRIES,
            without_values: false,
        },
        BatchShape {
            case: "a servable entry beside one naming no propertyValues member",
            servable: 1,
            unservable: 0,
            without_values: true,
        },
    ];

    for row in &rows {
        // A fresh emulator per row: the series each row writes is read back
        // whole, so no row's values are in another's history.
        let emulator = Emulator::new();
        seed(&emulator).await;

        let entries = row.entries();
        assert!(
            (MIN_ENTRIES..=MAX_ENTRIES).contains(&entries.len()),
            "case {:?}: the row must send a batch inside Entries' modelled length \
             {MIN_ENTRIES}..{MAX_ENTRIES}, sends {}",
            row.case,
            entries.len()
        );
        let written = emulator
            .batch_put_property_values(WORKSPACE, json!(entries))
            .await;
        assert_eq!(
            written.status, 200,
            "case {:?}: expected 200, got {written:?}",
            row.case
        );

        // `errorEntries` is `@required`, so it is a list on the wire whatever
        // the batch did — never an absent or null member.
        let error_entries = written.member("errorEntries").as_array().cloned();
        let error_entries = error_entries.unwrap_or_else(|| {
            panic!(
                "case {:?}: errorEntries must be present as a list, got {written:?}",
                row.case
            )
        });

        let expected_failed = row.expected_failed_entries();
        assert_eq!(
            error_entries.len(),
            expected_failed.len(),
            "case {:?}: expected one errorEntries entry per failed entry ({}), got {} in \
             {written:?}",
            row.case,
            expected_failed.len(),
            error_entries.len()
        );
        // The length of the member is the count of failed entries, so a batch
        // nothing failed in answers the EMPTY list rather than a padded one.
        if expected_failed.is_empty() {
            assert_eq!(
                written.member("errorEntries"),
                &json!([]),
                "case {:?}: a batch every entry of which was stored reports no error, got \
                 {written:?}",
                row.case
            );
        }

        // Every entry the list DOES carry is inside the modelled bounds, at
        // both levels: `ErrorEntries` itself and each entry's `errors`.
        if !error_entries.is_empty() {
            assert!(
                (MIN_REPORTED..=MAX_REPORTED).contains(&error_entries.len()),
                "case {:?}: a non-empty errorEntries must be inside the modelled length \
                 {MIN_REPORTED}..{MAX_REPORTED}, got {} in {written:?}",
                row.case,
                error_entries.len()
            );
        }
        for entry in &error_entries {
            let errors = entry["errors"].as_array().cloned().unwrap_or_default();
            assert!(
                (MIN_REPORTED..=MAX_REPORTED).contains(&errors.len()),
                "case {:?}: every errorEntries entry carries an errors list inside the modelled \
                 length {MIN_REPORTED}..{MAX_REPORTED}, got {} in {entry:?}",
                row.case,
                errors.len()
            );
            for error in &errors {
                let code = error["errorCode"].as_str().unwrap_or_default();
                let message = error["errorMessage"].as_str().unwrap_or_default();
                assert!(
                    (MIN_ERROR_TEXT_CHARS..=MAX_ERROR_TEXT_CHARS).contains(&code.chars().count()),
                    "case {:?}: errorCode must be inside the modelled length \
                     {MIN_ERROR_TEXT_CHARS}..{MAX_ERROR_TEXT_CHARS}, got {code:?} in {error:?}",
                    row.case
                );
                assert!(
                    (MIN_ERROR_TEXT_CHARS..=MAX_ERROR_TEXT_CHARS)
                        .contains(&message.chars().count()),
                    "case {:?}: errorMessage must be inside the modelled length \
                     {MIN_ERROR_TEXT_CHARS}..{MAX_ERROR_TEXT_CHARS}, got {message:?} in \
                     {error:?}",
                    row.case
                );
            }
        }

        // Each reported error is about one entry the request actually carried,
        // and every failed entry is reported exactly once.
        let mut reported = reported_errors(&written);
        for entry in &expected_failed {
            let at = reported.iter().position(|(about, _)| about == entry);
            let at = at.unwrap_or_else(|| {
                panic!(
                    "case {:?}: the failed entry {entry:?} must be reported, got {written:?}",
                    row.case
                )
            });
            let (_, code) = reported.remove(at);
            assert_eq!(
                code, ERROR_CODE,
                "case {:?}: the failed entry {entry:?} must carry the errorCode {ERROR_CODE:?}, \
                 got {code:?} in {written:?}",
                row.case
            );
        }
        assert!(
            reported.is_empty(),
            "case {:?}: no error may be reported for an entry that was stored, got {reported:?} \
             in {written:?}",
            row.case
        );

        // The servable entries of the batch WERE stored: the report of what
        // failed is not the report of what was applied.
        let read = emulator
            .get_property_value_history(
                WORKSPACE,
                json!({
                    "entityId": ENTITY,
                    "componentName": "Sensor",
                    "selectedProperties": ["Temp"],
                }),
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from the history, got {read:?}",
            row.case
        );
        let stored: Vec<Value> = read
            .member("propertyValues")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|series| series["values"].as_array().cloned().unwrap_or_default())
            .map(|value| json!({"time": value["time"], "value": value["value"]}))
            .collect();
        let expected_stored: Vec<Value> = (0..row.servable)
            .map(|index| json!({"time": instant(index), "value": {"doubleValue": index as f64}}))
            .collect();
        assert_eq!(
            stored, expected_stored,
            "case {:?}: expected the servable entries of the batch stored, got {read:?}",
            row.case
        );
    }
}
