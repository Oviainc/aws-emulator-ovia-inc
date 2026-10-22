// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeTimeSeries`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: `DescribeTimeSeries` reports, from the shared store,
//! the time series (data stream) of the asset property the request identifies —
//! by the `assetId` and `propertyId` pair or by the alias bound to it, each
//! accepted in the raw-UUID or `externalId:` form — carrying that property's data
//! type and the stable id, ARN and dates of its series; it identifies nothing when
//! neither form (or both) is given, answers a modelled 404 for a form that
//! resolves to no stored asset property, and changes nothing a later caller sees.
//!
//! The harness (`router_with_engine`, `call`, `error_code`, …) is the one
//! `asset_lifecycle` already defines, re-used rather than copied.

use std::collections::HashSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{
    call, create_model, error_code, is_modelled_id, property_id, router_with_engine, REGION,
};

/// One asset-model property definition per `PropertyDataType` a measurement can
/// carry, plus an attribute and a `STRUCT` that needs a `dataTypeSpec`. Named apart
/// from `asset_lifecycle`'s own catalog, which builds the definition JSON directly.
///
/// Source: `smithy_query(query="shape", shape="PropertyDataType")` — STRING |
/// INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON, of which
/// VIDEO/ANNOTATION/JSON are not property-definition types — and the
/// `TimeSeriesSummary$dataTypeSpec` doc ("required for time series that have the
/// STRUCT data type. … Use AWS/ALARM_STATE for alarm state").
///
pub(crate) fn time_series_property_catalog() -> Vec<PropertyRow> {
    vec![
        PropertyRow {
            name: "Temperature",
            data_type: "DOUBLE",
            data_type_spec: None,
            external_id: Some("boiler-temperature"),
            kind: json!({"measurement": {}}),
        },
        PropertyRow {
            name: "Cycles",
            data_type: "INTEGER",
            data_type_spec: None,
            external_id: None,
            kind: json!({"measurement": {}}),
        },
        PropertyRow {
            name: "Status",
            data_type: "STRING",
            data_type_spec: None,
            external_id: None,
            kind: json!({"measurement": {}}),
        },
        PropertyRow {
            name: "Running",
            data_type: "BOOLEAN",
            data_type_spec: None,
            external_id: None,
            kind: json!({"measurement": {}}),
        },
        PropertyRow {
            name: "AlarmState",
            data_type: "STRUCT",
            data_type_spec: Some("AWS/ALARM_STATE"),
            external_id: Some("boiler-alarm"),
            kind: json!({"measurement": {}}),
        },
        PropertyRow {
            name: "Serial",
            data_type: "STRING",
            data_type_spec: None,
            external_id: None,
            kind: json!({"attribute": {"defaultValue": "unset"}}),
        },
    ]
}

/// One row of [`time_series_property_catalog`]: an asset-model property definition
/// and what the time series of the asset property it materialises must report.
#[derive(Debug)]
pub(crate) struct PropertyRow {
    pub(crate) name: &'static str,
    pub(crate) data_type: &'static str,
    pub(crate) data_type_spec: Option<&'static str>,
    pub(crate) external_id: Option<&'static str>,
    /// The `PropertyType` the definition carries.
    pub(crate) kind: Value,
}

/// The catalog ([`time_series_property_catalog`]) as `CreateAssetModel` property
/// definitions.
fn property_definitions() -> Value {
    Value::Array(
        time_series_property_catalog()
            .into_iter()
            .map(|row| {
                let mut definition =
                    json!({"name": row.name, "dataType": row.data_type, "type": row.kind});
                if let Some(spec) = row.data_type_spec {
                    definition["dataTypeSpec"] = json!(spec);
                }
                if let Some(external) = row.external_id {
                    definition["externalId"] = json!(external);
                }
                definition
            })
            .collect(),
    )
}

/// A model with [`time_series_property_catalog`] and one asset of it, created with an
/// `assetExternalId` so the `externalId:` lookup form has something to resolve.
/// Returns `(router, engine, assetId, DescribeAsset body)`.
pub(crate) async fn asset_fixture() -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    String,
    Value,
) {
    let (router, engine) = router_with_engine();
    let model_id = create_model(&router, "TimeSeriesBoiler", property_definitions()).await;
    let (status, created) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "TimeSeriesBoiler1",
            "assetModelId": model_id,
            "assetExternalId": "boiler-1",
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created}");
    let asset_id = created["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset: no assetId in {created}"))
        .to_string();
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    (router, engine, asset_id, asset)
}

/// The `TimeSeriesId` the pair `(asset_id, property_id)` must be reported under.
///
/// Spelled out here rather than calling the handler's own
/// `state::time_series_id`, so this is an INDEPENDENT oracle: a change to the
/// derivation (the separator included — it must stay inside the modelled `ARN`
/// pattern's character class) fails these assertions instead of agreeing with
/// itself.
pub(crate) fn time_series_id_of(asset_id: &str, property_id: &str) -> String {
    format!("{asset_id}.{property_id}")
}

/// `GET /timeseries/describe?<query>`.
pub(crate) async fn describe_time_series(
    router: &MultiServiceRouter,
    query: &str,
) -> (u16, Value) {
    call(router, "GET", &format!("/timeseries/describe?{query}"), None).await
}

#[tokio::test]
async fn every_asset_property_is_a_time_series_the_stored_pair_names() {
    let (router, _engine, asset_id, asset) = asset_fixture().await;
    let account = engine::EMULATOR_ACCOUNT_ID;
    let mut seen_ids: HashSet<String> = HashSet::new();

    for row in time_series_property_catalog() {
        let PropertyRow {
            name,
            data_type,
            data_type_spec,
            external_id,
            ..
        } = row;
        let pid = property_id(&asset["assetProperties"], name);
        let (status, body) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {status} {body}");

        // The series belongs to the pair that owns it: one id per pair, stable and
        // within the modelled `TimeSeriesId` bound of 36..73 characters.
        let reported_id = body["timeSeriesId"].as_str().unwrap_or("").to_string();
        assert_eq!(
            reported_id,
            time_series_id_of(&asset_id, &pid),
            "case {name:?}: expected the series of the ({asset_id}, {pid}) pair, got {body}"
        );
        assert!(
            (36..=73).contains(&reported_id.len()),
            "case {name:?}: timeSeriesId must be 36..73 characters, got {reported_id:?}"
        );
        assert!(
            seen_ids.insert(reported_id.clone()),
            "case {name:?}: timeSeriesId {reported_id:?} is shared with another property"
        );
        assert_eq!(
            body["timeSeriesArn"].as_str().unwrap_or(""),
            format!("arn:aws:iotsitewise:{REGION}:{account}:time-series/{reported_id}"),
            "case {name:?}: unexpected timeSeriesArn in {body}"
        );
        // The ARN must satisfy the pattern the model states for every `ARN` member:
        // `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$` (source:
        // `smithy_query(query="operation", operation="DescribeTimeSeries")`), which
        // the derived `timeSeriesId` is part of.
        let arn = body["timeSeriesArn"].as_str().unwrap_or("");
        assert!(
            arn.strip_prefix("arn:aws:").is_some_and(|rest| rest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric()
                    || matches!(ch, '-' | ':' | '/' | '_' | '.'))),
            "case {name:?}: timeSeriesArn {arn:?} is outside the modelled ARN pattern"
        );

        // The property's own data type, and a `dataTypeSpec` only where the property
        // carries one.
        assert_eq!(
            body["dataType"].as_str().unwrap_or(""),
            data_type,
            "case {name:?}: expected dataType {data_type}, got {body}"
        );
        assert_eq!(
            body.get("dataTypeSpec").and_then(Value::as_str),
            data_type_spec,
            "case {name:?}: expected dataTypeSpec {data_type_spec:?}, got {body}"
        );

        // An asset property's series is ASSOCIATED: it reports the resolved UUIDs of
        // the pair, and no alias until one is bound to the property.
        assert_eq!(
            body["assetId"].as_str().unwrap_or(""),
            asset_id,
            "case {name:?}: expected assetId {asset_id}, got {body}"
        );
        assert_eq!(
            body["propertyId"].as_str().unwrap_or(""),
            pid,
            "case {name:?}: expected propertyId {pid}, got {body}"
        );
        assert!(
            is_modelled_id(body["propertyId"].as_str().unwrap_or("")),
            "case {name:?}: propertyId must be the resolved UUID, got {body}"
        );
        assert!(
            body.get("alias").and_then(Value::as_str).is_none(),
            "case {name:?}: no alias is bound to this property, got {body}"
        );
        assert!(
            body.get("workspaceName").and_then(Value::as_str).is_none(),
            "case {name:?}: the lookup named no workspace, got {body}"
        );

        // The series has existed since the asset materialised the property.
        assert_eq!(
            body["timeSeriesCreationDate"], asset["assetCreationDate"],
            "case {name:?}: expected the asset's creation date, got {body}"
        );
        assert_eq!(
            body["timeSeriesLastUpdateDate"], asset["assetLastUpdateDate"],
            "case {name:?}: expected the asset's last-update date, got {body}"
        );

        // Reading it again answers identically — the read settles nothing and mints
        // nothing.
        let (status, again) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(status, 200, "case {name:?}: re-read got {status} {again}");
        assert_eq!(
            again, body,
            "case {name:?}: a second read differs from the first: {again}"
        );

        // Every `CustomID` form of the same pair names the same series.
        let mut forms = vec![(
            "externalId asset",
            format!("assetId=externalId:boiler-1&propertyId={pid}"),
        )];
        if let Some(external) = external_id {
            forms.push((
                "externalId property",
                format!("assetId={asset_id}&propertyId=externalId:{external}"),
            ));
            forms.push((
                "externalId both",
                format!("assetId=externalId:boiler-1&propertyId=externalId:{external}"),
            ));
        }
        for (form, query) in forms {
            let (status, resolved) = describe_time_series(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {name:?}/{form}: expected 200, got {status} {resolved}"
            );
            assert_eq!(
                resolved, body,
                "case {name:?}/{form}: the {form} form must name the same series, got {resolved}"
            );
        }
    }

    assert_eq!(
        seen_ids.len(),
        time_series_property_catalog().len(),
        "every property must have its own series, got {seen_ids:?}"
    );
}

#[tokio::test]
async fn an_alias_names_the_same_time_series_as_the_pair_it_is_bound_to() {
    let (router, engine, asset_id, asset) = asset_fixture().await;

    // `UpdateAssetProperty` is the operation that binds an alias to an asset
    // property and is still a scaffold stub (its own issue,
    // `20261022T070208Z-iotsitewise-updateassetproperty-stub.md`), so the binding is
    // made directly in the shared store this handler reads — the same store a
    // handler writes through.
    let bindings = [
        ("Temperature", "/company/windfarm/3/turbine/7/temperature"),
        ("Cycles", "/company/windfarm/3/turbine/7/cycles"),
        ("AlarmState", "/company/windfarm/3/turbine/7/alarm_state"),
    ];
    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for (name, alias) in bindings {
            let pid = property_id(&asset["assetProperties"], name);
            let property = store
                .assets
                .get_mut(&asset_id)
                .and_then(|record| {
                    record
                        .properties
                        .iter_mut()
                        .find(|property| property.id == pid)
                })
                .unwrap_or_else(|| panic!("case {name:?}: no stored property {pid}"));
            property.alias = Some(alias.to_string());
            store
                .property_alias_index
                .insert(alias.to_string(), (asset_id.clone(), pid));
        }
    }

    for (name, alias) in bindings {
        let pid = property_id(&asset["assetProperties"], name);
        let (status, by_pair) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(status, 200, "case {name:?}: by pair got {status} {by_pair}");
        assert_eq!(
            by_pair.get("alias").and_then(Value::as_str),
            Some(alias),
            "case {name:?}: the bound alias must be reported, got {by_pair}"
        );

        let (status, by_alias) = describe_time_series(&router, &format!("alias={alias}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: by alias got {status} {by_alias}"
        );
        assert_eq!(
            by_alias, by_pair,
            "case {name:?}: the alias must name the same series as its pair, got {by_alias}"
        );
        assert_eq!(
            by_alias["timeSeriesId"].as_str().unwrap_or(""),
            time_series_id_of(&asset_id, &pid),
            "case {name:?}: unexpected timeSeriesId in {by_alias}"
        );
    }

    // An alias no property is bound to names no series, even when its prefix is one
    // that is bound.
    for case in [
        "/company/windfarm/3/turbine/7",
        "/company/windfarm/3/turbine/7/temperature/extra",
        "/company/windfarm/9/turbine/1/temperature",
    ] {
        let (status, body) = describe_time_series(&router, &format!("alias={case}")).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_lookup_that_names_no_one_time_series_is_a_modelled_refusal() {
    let (router, _engine, asset_id, asset) = asset_fixture().await;
    let pid = property_id(&asset["assetProperties"], "Temperature");
    // A well-formed `ID` that no stored resource carries (the modelled pattern
    // refuses the all-zero UUID, so the lookup must reach the store to fail).
    let absent = "11111111-2222-3333-4444-555555555555";

    // Neither form, half a form, or both forms at once: nothing (or more than one
    // thing) is identified, so the request itself is invalid.
    let invalid = [
        ("no identifier", String::new()),
        ("assetId only", format!("assetId={asset_id}")),
        ("propertyId only", format!("propertyId={pid}")),
        (
            "alias and pair",
            format!("alias=/a/b/c&assetId={asset_id}&propertyId={pid}"),
        ),
        ("alias and assetId", format!("alias=/a/b/c&assetId={asset_id}")),
    ];
    for (case, query) in invalid {
        let (status, body) = describe_time_series(&router, &query).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A form that resolves to no stored asset property is a 404, whichever part of
    // it is absent and whichever label form names it.
    let missing = [
        ("unknown asset", format!("assetId={absent}&propertyId={pid}")),
        (
            "unknown asset externalId",
            format!("assetId=externalId:no-such-boiler&propertyId={pid}"),
        ),
        ("unknown property", format!("assetId={asset_id}&propertyId={absent}")),
        (
            "unknown property externalId",
            format!("assetId={asset_id}&propertyId=externalId:no-such-property"),
        ),
        ("unbound alias", "alias=/no/such/stream".to_string()),
        // `workspaceName` scopes the lookup to a workspace, and `CreateWorkspace` is
        // a scaffold stub, so no workspace exists for one to name.
        (
            "workspace scoped",
            format!("assetId={asset_id}&propertyId={pid}&workspaceName=plant_1"),
        ),
    ];
    for (case, query) in missing {
        let (status, body) = describe_time_series(&router, &query).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // None of the refusals (nor the reads) created, settled or forgot anything: the
    // pair still reports the same series, and the asset still reads back as it was.
    let (status, body) =
        describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
    assert_eq!(status, 200, "after the refusals: got {status} {body}");
    assert_eq!(
        body["timeSeriesId"].as_str().unwrap_or(""),
        time_series_id_of(&asset_id, &pid),
        "after the refusals: unexpected timeSeriesId in {body}"
    );
    let (status, reread) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset after the refusals: got {status} {reread}");
    assert_eq!(
        reread["assetProperties"], asset["assetProperties"],
        "the describes must not have changed the asset's properties: {reread}"
    );
}
