// SPDX-License-Identifier: CC0-1.0

//! What the account-wide storage configuration decides about INGESTION
//! (`BatchPutAssetPropertyValue`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! Two rules, both from `PutStorageConfiguration`'s own model doc
//! (`smithy_query(query="shape", shape="PutStorageConfigurationRequest")`):
//!
//! 1. `disassociatedDataStorage`: "ENABLED – IoT SiteWise accepts time series that
//!    aren't associated with asset properties." / "DISABLED – IoT SiteWise doesn't
//!    accept time series (data streams) that aren't associated with asset
//!    properties." So while the configuration in force names it disabled, an entry
//!    landing on a stream no asset property is associated with is refused per
//!    entry and nothing is stored for it — neither a stream the write would create
//!    nor a sample on one the store already holds — while an entry addressed by
//!    the ids of an asset property, or by an alias an asset property carries, is
//!    unaffected.
//! 2. `disallowIngestNullNaN`: "Describes the configuration for ingesting NULL and
//!    NaN data. By default the feature is allowed. The feature is disallowed if
//!    the value is `true`." So while it is true, a TQV carrying NULL or NaN data is
//!    turned away whatever the data type of the stream it addresses and however the
//!    entry addressed it, and every other reading still ingests.
//!
//! Each table spans the class its rule generalises over rather than the one
//! reported case: the states a put can leave a setting in (including the account no
//! put has configured at all), the ways an entry addresses the stream it writes to,
//! and every reading a `Variant` can carry.
//!
//! The harness (`router`, `call`, `create_model`, …) is the one `asset_lifecycle`
//! defines, and the storage-configuration put is the one `storage_configuration`
//! defines, re-used rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, create_model, property_catalog, property_id, router};
use super::storage_configuration::put as put_storage;
use super::support::now_seconds;

/// The two `DisassociatedDataStorageState` values, written out here as an oracle
/// independent of the crate under test. Source: `smithy_query(query="shape",
/// shape="DisassociatedDataStorageState")`, which reports
/// `values: ["ENABLED", "DISABLED"]`.
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// The service-managed hot tier every put below names, so no table row depends on
/// a cold-tier destination. Source: `smithy_query(query="shape",
/// shape="StorageType")`, which reports
/// `values: ["SITEWISE_DEFAULT_STORAGE", "MULTI_LAYER_STORAGE"]`.
const HOT_TIER: &str = "SITEWISE_DEFAULT_STORAGE";

/// Put a storage configuration and assert the account took it.
async fn configure(router: &MultiServiceRouter, body: Value) {
    let (status, answer) = put_storage(router, body.clone()).await;
    assert_eq!(
        status, 200,
        "PutStorageConfiguration {body}: got {status} {answer}"
    );
}

/// Ingest ONE TQV, with `address` naming the stream the way the entry does
/// (`propertyAlias`, or `assetId` and `propertyId`), and answer the response body.
async fn ingest(
    router: &MultiServiceRouter,
    entry_id: &str,
    address: &Value,
    value: Value,
    seconds: i64,
) -> Value {
    let mut entry = json!({
        "entryId": entry_id,
        "propertyValues": [{
            "value": value,
            // Anchored inside `[now - 7 days, now + 10 minutes]`, the only window
            // BatchPutAssetPropertyValue ingests a TQV in.
            "timestamp": {"timeInSeconds": seconds},
        }],
    });
    for (member, named) in address.as_object().expect("an addressing object") {
        entry[member.as_str()] = named.clone();
    }
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [entry]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchPutAssetPropertyValue {entry_id}: got {status} {body}"
    );
    body
}

/// The refusals an answer reports, as `(entryId, errorCode, errorMessage)`.
fn refusals(body: &Value) -> Vec<(String, String, String)> {
    body["errorEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("no errorEntries in {body}"))
        .iter()
        .flat_map(|entry| {
            let entry_id = entry["entryId"].as_str().unwrap_or("").to_string();
            entry["errors"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |error| {
                    (
                        entry_id.clone(),
                        error["errorCode"].as_str().unwrap_or("").to_string(),
                        error["errorMessage"].as_str().unwrap_or("").to_string(),
                    )
                })
        })
        .collect()
}

/// `GetAssetPropertyValue` through the SAME address the entry used, as
/// `(status, propertyValue)`.
async fn latest(router: &MultiServiceRouter, address: &Value) -> (u16, Value) {
    let uri = match address.get("propertyAlias").and_then(Value::as_str) {
        Some(alias) => format!("/properties/latest?propertyAlias={alias}"),
        None => format!(
            "/properties/latest?assetId={}&propertyId={}",
            address["assetId"].as_str().unwrap_or(""),
            address["propertyId"].as_str().unwrap_or("")
        ),
    };
    let (status, body) = call(router, "GET", &uri, None).await;
    (status, body.get("propertyValue").cloned().unwrap_or(Value::Null))
}

/// Create an asset carrying the shared property catalog and answer
/// `(assetId, assetProperties)`.
async fn asset_with_catalog(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let model_id = create_model(router, name, property_catalog()).await;
    let asset_id = create_asset(router, &format!("{name}1"), &model_id).await;
    let (status, described) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    (asset_id, described["assetProperties"].clone())
}

/// One way an entry addresses the stream it writes to.
struct Form {
    case: &'static str,
    /// The identifying members the entry carries.
    address: Value,
    /// Whether an asset property IS associated with the stream it names: those
    /// streams are never the ones this setting turns away.
    associated: bool,
    /// The reading the stream still reports after an entry the setting turns away,
    /// or `None` when the alias then names no stream at all.
    unchanged: Option<f64>,
}

#[tokio::test]
async fn disassociated_data_storage_decides_whether_a_stream_no_property_owns_takes_ingest() {
    // Rows span the states a put can leave the setting in, plus the account no put
    // has configured at all. The setting does not go back once enabled, so each row
    // runs on an account of its own.
    let rows: [(&str, Option<&str>, bool); 3] = [
        ("no put has configured the account", None, true),
        ("disassociated data storage enabled", Some(ENABLED), true),
        ("disassociated data storage disabled", Some(DISABLED), false),
    ];

    for (case, state, accepts_unowned) in rows {
        let router = router();
        let at = now_seconds() - 3_600;

        // A stream no asset property owns, created before this row's setting is put,
        // so the table covers a stream the store ALREADY holds beside one an entry
        // would create.
        let stored_alias = "/plant/line1/mixer/temperature";
        let seeded = 20.5;
        let answer = ingest(
            &router,
            "seed",
            &json!({"propertyAlias": stored_alias}),
            json!({"doubleValue": seeded}),
            at,
        )
        .await;
        assert!(
            refusals(&answer).is_empty(),
            "case {case:?}: the seeding write must be accepted, got {answer}"
        );

        // An asset property, and an alias bound to it: the two addressing forms that
        // name a stream an asset property IS associated with.
        let (asset_id, properties) = asset_with_catalog(&router, "Mixer").await;
        let temperature = property_id(&properties, "Temperature");
        let owned_alias = "/plant/line1/mixer/owned-temperature";
        let (status, bound) = call(
            &router,
            "PUT",
            &format!("/assets/{asset_id}/properties/{temperature}"),
            Some(json!({"propertyAlias": owned_alias})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssetProperty got {status} {bound}"
        );

        if let Some(state) = state {
            configure(
                &router,
                json!({"storageType": HOT_TIER, "disassociatedDataStorage": state}),
            )
            .await;
        }

        let new_alias = "/plant/line2/pump/pressure";
        let forms = [
            Form {
                case: "an alias no stream answers to yet",
                address: json!({"propertyAlias": new_alias}),
                associated: false,
                unchanged: None,
            },
            Form {
                case: "the alias of a stream the store already holds",
                address: json!({"propertyAlias": stored_alias}),
                associated: false,
                unchanged: Some(seeded),
            },
            Form {
                case: "the assetId and propertyId of an asset property",
                address: json!({"assetId": asset_id, "propertyId": temperature}),
                associated: true,
                unchanged: None,
            },
            Form {
                case: "the alias an asset property carries",
                address: json!({"propertyAlias": owned_alias}),
                associated: true,
                unchanged: None,
            },
        ];

        for (index, form) in forms.iter().enumerate() {
            let expected_accepted = form.associated || accepts_unowned;
            // `PutAssetPropertyValueEntry$entryId` is modelled with the pattern
            // `^[a-zA-Z0-9_-]+$`, so a row's prose name cannot be the entryId.
            let entry_id = format!("e{index}");
            // Ascending, so the row's own sample is the newest one its stream holds.
            let seconds = at + 60 + index as i64;
            let reading = 30.0 + index as f64;
            let answer = ingest(
                &router,
                &entry_id,
                &form.address,
                json!({"doubleValue": reading}),
                seconds,
            )
            .await;
            let refused = refusals(&answer);

            if expected_accepted {
                assert!(
                    refused.is_empty(),
                    "case {case:?} / {:?}: expected the entry to be accepted, got {answer}",
                    form.case
                );
            } else {
                assert_eq!(
                    refused.len(),
                    1,
                    "case {case:?} / {:?}: expected exactly one refusal, got {answer}",
                    form.case
                );
                let (reported_entry, code, message) = &refused[0];
                assert_eq!(
                    reported_entry, &entry_id,
                    "case {case:?} / {:?}: the refusal must name the submitted entryId, \
                     got {answer}",
                    form.case
                );
                assert_eq!(
                    code, "InvalidRequestException",
                    "case {case:?} / {:?}: expected InvalidRequestException, got {code:?} \
                     ({message:?})",
                    form.case
                );
                assert!(
                    message.contains(DISABLED) && message.contains("disassociatedDataStorage"),
                    "case {case:?} / {:?}: the refusal must name the setting in force, \
                     got {message:?}",
                    form.case
                );
            }

            let (status, stored) = latest(&router, &form.address).await;
            if expected_accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?} / {:?}: the accepted entry's stream must be readable, \
                     got {status} {stored}",
                    form.case
                );
                assert_eq!(
                    stored["value"]["doubleValue"].as_f64(),
                    Some(reading),
                    "case {case:?} / {:?}: expected the ingested doubleValue {reading}, \
                     got {stored}",
                    form.case
                );
                assert_eq!(
                    stored["timestamp"]["timeInSeconds"].as_i64(),
                    Some(seconds),
                    "case {case:?} / {:?}: expected the ingested timestamp {seconds}, \
                     got {stored}",
                    form.case
                );
            } else {
                match form.unchanged {
                    Some(held) => {
                        assert_eq!(
                            status, 200,
                            "case {case:?} / {:?}: the stream the store holds must still \
                             answer, got {status} {stored}",
                            form.case
                        );
                        assert_eq!(
                            stored["value"]["doubleValue"].as_f64(),
                            Some(held),
                            "case {case:?} / {:?}: the refused entry must append nothing, \
                             got {stored}",
                            form.case
                        );
                    }
                    None => {
                        assert_eq!(
                            status, 404,
                            "case {case:?} / {:?}: the refused entry must create no stream, \
                             got {status} {stored}",
                            form.case
                        );
                        let alias = form.address["propertyAlias"].as_str().unwrap_or("");
                        let (status, described) = call(
                            &router,
                            "GET",
                            &format!("/timeseries/describe?alias={alias}"),
                            None,
                        )
                        .await;
                        assert_eq!(
                            status, 404,
                            "case {case:?} / {:?}: DescribeTimeSeries must report no series \
                             for a refused alias, got {status} {described}",
                            form.case
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn disallow_ingest_null_nan_turns_away_null_and_nan_readings_whatever_the_data_type() {
    // The readings a `Variant` can carry, each against the catalog property whose
    // `PropertyDataType` holds it. Source: `smithy_query(query="shape",
    // shape="Variant")` — stringValue | integerValue | doubleValue | booleanValue |
    // nullValue — with every `RawValueType` the null form names
    // (`smithy_query(query="shape", shape="RawValueType")`: D | B | S | I | U), and
    // the not-a-number the model allows a doubleValue to carry ("Double.NaN is
    // allowed"), which the awsJson wire form carries as a string.
    let readings: [(&str, &str, Value, bool); 10] = [
        ("a string reading", "Status", json!({"stringValue": "RUNNING"}), false),
        ("an integer reading", "Cycles", json!({"integerValue": 7}), false),
        ("a double reading", "Temperature", json!({"doubleValue": 72.5}), false),
        ("a boolean reading", "Running", json!({"booleanValue": true}), false),
        ("a NaN double reading", "Temperature", json!({"doubleValue": "NaN"}), true),
        ("a null double reading", "Temperature", json!({"nullValue": {"valueType": "D"}}), true),
        ("a null boolean reading", "Running", json!({"nullValue": {"valueType": "B"}}), true),
        ("a null string reading", "Status", json!({"nullValue": {"valueType": "S"}}), true),
        ("a null integer reading", "Cycles", json!({"nullValue": {"valueType": "I"}}), true),
        ("a null unknown-type reading", "Temperature", json!({"nullValue": {"valueType": "U"}}), true),
    ];

    // Rows span the states a put can leave the setting in, plus the account no put
    // has configured at all — the documented default being that the feature is
    // allowed.
    let rows: [(&str, Option<bool>, bool); 3] = [
        ("no put has configured the account", None, true),
        ("NULL and NaN ingestion allowed", Some(false), true),
        ("NULL and NaN ingestion disallowed", Some(true), false),
    ];

    for (case, disallow, allows_null_nan) in rows {
        let router = router();
        let at = now_seconds() - 3_600;
        let (asset_id, properties) = asset_with_catalog(&router, "Boiler").await;

        // A stream no asset property owns, so the table also covers the
        // alias-addressed form: this setting is about the READING, not about how the
        // entry addressed its stream.
        let alias = "/plant/line3/boiler/temperature";
        let answer = ingest(
            &router,
            "seed",
            &json!({"propertyAlias": alias}),
            json!({"doubleValue": 11.5}),
            at,
        )
        .await;
        assert!(
            refusals(&answer).is_empty(),
            "case {case:?}: the seeding write must be accepted, got {answer}"
        );

        if let Some(disallow) = disallow {
            configure(
                &router,
                json!({"storageType": HOT_TIER, "disallowIngestNullNaN": disallow}),
            )
            .await;
        }

        for (index, (reading, property_name, value, null_or_nan)) in readings.iter().enumerate() {
            let expected_accepted = allows_null_nan || !null_or_nan;
            let property = property_id(&properties, property_name);
            // Every addressing form is driven: the ids of the asset property for the
            // table itself, and the alias of the stream no property owns for the
            // double-typed rows, which that stream can hold.
            let addresses: Vec<(&str, Value)> = if *property_name == "Temperature" {
                vec![
                    ("by ids", json!({"assetId": asset_id, "propertyId": property})),
                    ("by alias", json!({"propertyAlias": alias})),
                ]
            } else {
                vec![("by ids", json!({"assetId": asset_id, "propertyId": property}))]
            };

            for (at_form, (addressed, address)) in addresses.into_iter().enumerate() {
                let entry_id = format!("e{index}_{at_form}");
                // Ascending, so an accepted reading is the newest sample its stream
                // holds and a refused one can be told apart from it.
                let seconds = at + 60 + (index * 4 + at_form) as i64;
                let answer = ingest(&router, &entry_id, &address, value.clone(), seconds).await;
                let refused = refusals(&answer);

                if expected_accepted {
                    assert!(
                        refused.is_empty(),
                        "case {case:?} / {reading:?} {addressed}: expected the reading to be \
                         ingested, got {answer}"
                    );
                } else {
                    assert_eq!(
                        refused.len(),
                        1,
                        "case {case:?} / {reading:?} {addressed}: expected exactly one \
                         refusal, got {answer}"
                    );
                    let (reported_entry, code, message) = &refused[0];
                    assert_eq!(
                        reported_entry, &entry_id,
                        "case {case:?} / {reading:?} {addressed}: the refusal must name the \
                         submitted entryId, got {answer}"
                    );
                    assert_eq!(
                        code, "InvalidRequestException",
                        "case {case:?} / {reading:?} {addressed}: expected \
                         InvalidRequestException, got {code:?} ({message:?})"
                    );
                    assert!(
                        message.contains("disallowIngestNullNaN"),
                        "case {case:?} / {reading:?} {addressed}: the refusal must name the \
                         setting in force, got {message:?}"
                    );
                }

                let (status, stored) = latest(&router, &address).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} / {reading:?} {addressed}: the stream must still answer, \
                     got {status} {stored}"
                );
                let newest = stored["timestamp"]["timeInSeconds"].as_i64();
                if expected_accepted {
                    assert_eq!(
                        newest,
                        Some(seconds),
                        "case {case:?} / {reading:?} {addressed}: the ingested TQV must be \
                         the newest sample, got {stored}"
                    );
                } else {
                    assert_ne!(
                        newest,
                        Some(seconds),
                        "case {case:?} / {reading:?} {addressed}: the refused TQV must not be \
                         stored, got {stored}"
                    );
                }
            }
        }
    }
}
