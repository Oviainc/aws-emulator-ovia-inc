// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisassociateTimeSeriesFromAssetProperty`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: the call breaks the ONE association its request
//! names — the alias, and the asset property that carries it — so afterwards the
//! property carries no alias and reports none of the stream's samples, while the
//! data stream the alias named survives as a DISASSOCIATED time series that keeps
//! the identity, data type, creation date and samples it had while associated and
//! is addressable by its alias alone; a request that names an association the
//! store does not hold writes nothing and is refused as an absent resource.
//!
//! The harness (`router_with_engine`, `call`, `error_code`, …) is the one
//! `asset_lifecycle` defines, and the model/asset fixture and the
//! `DescribeTimeSeries` reader are `time_series`'s, re-used rather than copied.

use std::collections::HashSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, property_id, REGION};
use crate::support::now_seconds;
use crate::time_series::{
    asset_fixture, describe_time_series, time_series_id_of, time_series_property_catalog,
};

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The `assetExternalId` [`asset_fixture`] creates its asset with.
const ASSET_EXTERNAL_ID: &str = "boiler-1";

/// Which `CustomID` form a row addresses a resource by.
#[derive(Debug, Clone, Copy)]
enum Label {
    /// The raw id the create minted.
    Raw,
    /// `externalId:<externalId>`.
    External,
}

/// `raw` or `externalId:<external>`, as `form` asks. Panics when a row asks for
/// the external form of a resource that carries no external id, so a mis-written
/// table fails loudly instead of silently testing the raw form twice.
fn label(form: Label, raw: &str, external: Option<&str>, case: &str) -> String {
    match form {
        Label::Raw => raw.to_string(),
        Label::External => format!(
            "externalId:{}",
            external.unwrap_or_else(|| panic!("case {case:?}: no external id to address"))
        ),
    }
}

/// `PUT /assets/{asset}/properties/{property}` — the operation that binds an alias
/// to an asset property, and so the one that makes the association this suite
/// breaks.
async fn bind_alias(
    router: &MultiServiceRouter,
    asset_label: &str,
    property_label: &str,
    alias: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/assets/{asset_label}/properties/{property_label}"),
        Some(json!({"propertyAlias": alias})),
    )
    .await
}

/// `POST /timeseries/disassociate?alias=…&assetId=…&propertyId=…`.
async fn disassociate(
    router: &MultiServiceRouter,
    alias: &str,
    asset_label: &str,
    property_label: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/timeseries/disassociate?alias={alias}&assetId={asset_label}&propertyId={property_label}"
        ),
        None,
    )
    .await
}

/// `GET /properties/latest?propertyAlias=…` — the alias-addressed value reader,
/// which resolves through the same binding this call breaks.
async fn latest_by_alias(router: &MultiServiceRouter, alias: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/properties/latest?propertyAlias={alias}"),
        None,
    )
    .await
}

/// The property named `name` in a `DescribeAsset` body.
fn property_by_name(properties: &Value, name: &str) -> Value {
    properties
        .as_array()
        .unwrap_or_else(|| panic!("expected a property list, got {properties}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("no property named {name} in {properties}"))
        .clone()
}

/// One row of the disassociation table: which property carries the alias, the
/// alias itself, and which `CustomID` form the call addresses each id by.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// A property of `time_series_property_catalog`, so the row's data type and
    /// `dataTypeSpec` come from that catalog rather than from this table.
    property: &'static str,
    alias: &'static str,
    asset_form: Label,
    property_form: Label,
}

/// The rows differ along everything the rule generalises over: the data type of
/// the stream (DOUBLE, INTEGER, STRING, BOOLEAN and a STRUCT that carries a
/// `dataTypeSpec`), the shape of the alias (a deep path, a shallow one, one with a
/// `.` in a segment, and a single bare segment — `PropertyAlias` only forbids the
/// control characters) and which `CustomID` form addresses the asset and the
/// property.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "DOUBLE, deep path, raw ids",
            property: "Temperature",
            alias: "/company/windfarm/3/turbine/7/temperature",
            asset_form: Label::Raw,
            property_form: Label::Raw,
        },
        Row {
            case: "INTEGER, shallow path, external asset id",
            property: "Cycles",
            alias: "/plant/1/cycles",
            asset_form: Label::External,
            property_form: Label::Raw,
        },
        Row {
            case: "STRING, dotted segment, raw ids",
            property: "Status",
            alias: "/plant/1/boiler/status.v2",
            asset_form: Label::Raw,
            property_form: Label::Raw,
        },
        Row {
            case: "BOOLEAN, single bare segment, external asset id",
            property: "Running",
            alias: "running",
            asset_form: Label::External,
            property_form: Label::Raw,
        },
        Row {
            case: "STRUCT with a dataTypeSpec, both ids external",
            property: "AlarmState",
            alias: "/company/windfarm/3/turbine/7/alarm_state",
            asset_form: Label::External,
            property_form: Label::External,
        },
    ]
}

#[tokio::test]
async fn disassociating_detaches_the_alias_and_leaves_the_stream_standing() {
    let (router, _engine, asset_id, asset) = asset_fixture().await;
    let catalog = time_series_property_catalog();
    // Every detached stream must report an id of its own.
    let mut detached_ids: HashSet<String> = HashSet::new();

    for row in rows() {
        let Row {
            case,
            property: name,
            alias,
            asset_form,
            property_form,
        } = row;
        let definition = catalog
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("case {case:?}: {name} is not in the property catalog"));
        let pid = property_id(&asset["assetProperties"], name);
        let asset_label = label(asset_form, &asset_id, Some(ASSET_EXTERNAL_ID), case);
        let property_label = label(property_form, &pid, definition.external_id, case);

        // The association this call breaks has to exist first.
        let (status, bound) = bind_alias(&router, &asset_id, &pid, alias).await;
        assert_eq!(
            status, 200,
            "case {case:?}: binding the alias got {status} {bound}"
        );
        let (status, before) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTimeSeries before got {status} {before}"
        );
        assert_eq!(
            before.get("alias").and_then(Value::as_str),
            Some(alias),
            "case {case:?}: the alias must be bound before the call, got {before}"
        );

        // --- the call under test -------------------------------------------
        let (status, answer) = disassociate(&router, alias, &asset_label, &property_label).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateTimeSeriesFromAssetProperty got {status} {answer}"
        );

        // The asset property survives and carries no alias: neither its own view
        // nor the series addressed by its ids reports one, and the alias now stands
        // for the detached stream rather than for the property.
        let (status, reread) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAsset after got {status} {reread}"
        );
        let property = property_by_name(&reread["assetProperties"], name);
        assert!(
            property.get("alias").is_none_or(Value::is_null),
            "case {case:?}: the property must carry no alias, got {property}"
        );
        let (status, by_pair) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTimeSeries by pair got {status} {by_pair}"
        );
        assert!(
            by_pair.get("alias").is_none_or(Value::is_null),
            "case {case:?}: the pair's series must report no alias, got {by_pair}"
        );
        assert_eq!(
            by_pair["timeSeriesId"].as_str().unwrap_or(""),
            time_series_id_of(&asset_id, &pid),
            "case {case:?}: the pair must still name its own series, got {by_pair}"
        );
        let (status, latest) = latest_by_alias(&router, alias).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the alias must still address its stream, got {status} {latest}"
        );
        assert!(
            latest.get("propertyValue").is_none_or(Value::is_null),
            "case {case:?}: nothing was written to this stream, got {latest}"
        );

        // The STREAM is not deleted with the association: it stands as a
        // disassociated time series under its alias, keeping the data type and
        // creation date it had while associated, reporting no asset property, and
        // with a last-update instant no earlier than the one it carried before the
        // call. Its `timeSeriesId` is its OWN: the asset property's ids still name
        // a series of theirs, and two series are two ids.
        let (status, standing) = describe_time_series(&router, &format!("alias={alias}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the detached stream must still be addressable, got {status} {standing}"
        );
        assert_eq!(
            standing.get("alias").and_then(Value::as_str),
            Some(alias),
            "case {case:?}: the stream must keep its alias, got {standing}"
        );
        assert!(
            standing.get("assetId").is_none_or(Value::is_null)
                && standing.get("propertyId").is_none_or(Value::is_null),
            "case {case:?}: a disassociated stream names no asset property, got {standing}"
        );
        let detached_id = standing["timeSeriesId"].as_str().unwrap_or("").to_string();
        assert_ne!(
            detached_id,
            time_series_id_of(&asset_id, &pid),
            "case {case:?}: the detached stream must not report the pair's id, got {standing}"
        );
        // The modelled `TimeSeriesId` bound, from
        // `smithy_query(query="shape", shape="TimeSeriesSummary")`: length 36..73.
        assert!(
            (36..=73).contains(&detached_id.len()),
            "case {case:?}: timeSeriesId must be 36..73 characters, got {detached_id:?}"
        );
        assert!(
            detached_ids.insert(detached_id.clone()),
            "case {case:?}: timeSeriesId {detached_id:?} is shared with another stream"
        );
        assert_eq!(
            standing["timeSeriesArn"].as_str().unwrap_or(""),
            format!(
                "arn:aws:iotsitewise:{REGION}:{}:time-series/{detached_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case:?}: unexpected timeSeriesArn in {standing}"
        );
        assert_eq!(
            standing["dataType"].as_str().unwrap_or(""),
            definition.data_type,
            "case {case:?}: expected dataType {}, got {standing}",
            definition.data_type
        );
        assert_eq!(
            standing.get("dataTypeSpec").and_then(Value::as_str),
            definition.data_type_spec,
            "case {case:?}: expected dataTypeSpec {:?}, got {standing}",
            definition.data_type_spec
        );
        assert_eq!(
            standing["timeSeriesCreationDate"], before["timeSeriesCreationDate"],
            "case {case:?}: the stream must keep its creation date, got {standing}"
        );
        let (updated, was_updated) = (
            standing["timeSeriesLastUpdateDate"].as_f64().unwrap_or(-1.0),
            before["timeSeriesLastUpdateDate"].as_f64().unwrap_or(-1.0),
        );
        assert!(
            updated >= was_updated && was_updated >= 0.0,
            "case {case:?}: expected a last-update date at or after {was_updated}, got {updated}"
        );

        // Reading it again answers identically: the read settles nothing.
        let (status, again) = describe_time_series(&router, &format!("alias={alias}")).await;
        assert_eq!(status, 200, "case {case:?}: re-read got {status} {again}");
        assert_eq!(
            again, standing,
            "case {case:?}: a second read differs from the first: {again}"
        );

        // The association is gone, so the same call again names one the store does
        // not hold and is refused, leaving the detached stream exactly as it was.
        let (status, replay) = disassociate(&router, alias, &asset_label, &property_label).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a repeated disassociate must be refused, got {status} {replay}"
        );
        assert_eq!(
            error_code(&replay),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {replay}"
        );
        let (status, after_replay) = describe_time_series(&router, &format!("alias={alias}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: after the refused replay got {status} {after_replay}"
        );
        assert_eq!(
            after_replay, standing,
            "case {case:?}: the refused replay changed the stream: {after_replay}"
        );
    }
}

#[tokio::test]
async fn a_request_that_names_no_stored_association_is_a_modelled_refusal() {
    let (router, _engine, asset_id, asset) = asset_fixture().await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let cycles = property_id(&asset["assetProperties"], "Cycles");
    let alias = "/plant/9/boiler/temperature";

    let (status, bound) = bind_alias(&router, &asset_id, &temperature, alias).await;
    assert_eq!(status, 200, "binding the alias got {status} {bound}");

    // Every way a request can fail to name the one association the store holds:
    // an id that resolves to nothing (in either `CustomID` form), an alias bound to
    // another property, a property that carries no alias, and an alias nothing was
    // ever bound to.
    let refusals = [
        ("unknown asset id", alias, ABSENT_ID.to_string(), temperature.clone()),
        (
            "unknown asset external id",
            alias,
            "externalId:no-such-boiler".to_string(),
            temperature.clone(),
        ),
        ("unknown property id", alias, asset_id.clone(), ABSENT_ID.to_string()),
        (
            "unknown property external id",
            alias,
            asset_id.clone(),
            "externalId:no-such-property".to_string(),
        ),
        (
            "alias bound to another property",
            alias,
            asset_id.clone(),
            cycles.clone(),
        ),
        (
            "property carries no alias",
            "/plant/9/boiler/cycles",
            asset_id.clone(),
            cycles.clone(),
        ),
        (
            "alias nothing is bound to, property carries another",
            "/no/such/stream",
            asset_id.clone(),
            temperature.clone(),
        ),
    ];
    for (case, requested_alias, asset_label, property_label) in refusals {
        let (status, body) =
            disassociate(&router, requested_alias, &asset_label, &property_label).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // None of the refusals wrote anything: the one binding that existed still
    // stands, by its own reader and by the alias-addressed one, and no stream was
    // left standing under an alias the refusals named.
    let (status, by_pair) =
        describe_time_series(&router, &format!("assetId={asset_id}&propertyId={temperature}")).await;
    assert_eq!(status, 200, "after the refusals: got {status} {by_pair}");
    assert_eq!(
        by_pair.get("alias").and_then(Value::as_str),
        Some(alias),
        "a refused call must leave the binding in place, got {by_pair}"
    );
    let (status, latest) = latest_by_alias(&router, alias).await;
    assert_eq!(
        status, 200,
        "the alias must still address the property, got {status} {latest}"
    );
    for absent in ["/plant/9/boiler/cycles", "/no/such/stream"] {
        let (status, body) = describe_time_series(&router, &format!("alias={absent}")).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: a refusal must leave no stream behind, got {status} {body}"
        );
    }
}

/// `GET /properties/history?<query>` — the samples the query identifies, as
/// `(timeInSeconds, doubleValue)` pairs in the reported order.
async fn history(router: &MultiServiceRouter, query: &str) -> Vec<(i64, f64)> {
    let (status, body) = call(router, "GET", &format!("/properties/history?{query}"), None).await;
    assert_eq!(status, 200, "GetAssetPropertyValueHistory: got {status} {body}");
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetPropertyValueHistory in {body}"))
        .iter()
        .map(|sample| {
            (
                sample["timestamp"]["timeInSeconds"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("a sample with no timeInSeconds in {body}")),
                sample["value"]["doubleValue"]
                    .as_f64()
                    .unwrap_or_else(|| panic!("a sample with no doubleValue in {body}")),
            )
        })
        .collect()
}

/// The samples the asset property reports, by its ids.
async fn history_by_ids(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> Vec<(i64, f64)> {
    history(router, &format!("assetId={asset_id}&propertyId={property_id}")).await
}

#[tokio::test]
async fn the_samples_travel_with_the_stream_the_call_detaches() {
    let (router, _engine, asset_id, asset) = asset_fixture().await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let alias = "/company/windfarm/4/turbine/1/temperature";
    let (status, bound) = bind_alias(&router, &asset_id, &temperature, alias).await;
    assert_eq!(status, 200, "binding the alias got {status} {bound}");

    // Ingest addressed by the alias, so the samples reach the property through the
    // association that is about to be broken. Anchored in the past:
    // BatchPutAssetPropertyValue ingests only TQVs inside
    // `[now - 7 days, now + 10 minutes]`.
    let written: Vec<(i64, f64)> = vec![
        (now_seconds() - 120, 41.0),
        (now_seconds() - 60, 42.5),
    ];
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": "alias-write",
            "propertyAlias": alias,
            "propertyValues": written
                .iter()
                .map(|(seconds, value)| json!({
                    "value": {"doubleValue": value},
                    "timestamp": {"timeInSeconds": seconds},
                }))
                .collect::<Vec<Value>>(),
        }]})),
    )
    .await;
    assert_eq!(status, 200, "BatchPutAssetPropertyValue: got {status} {put}");
    assert_eq!(
        put["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "BatchPutAssetPropertyValue by alias must accept the entry, got {put}"
    );
    assert_eq!(
        history_by_ids(&router, &asset_id, &temperature).await,
        written,
        "the associated property must report what was written to its stream"
    );

    let (status, answer) = disassociate(&router, alias, &asset_id, &temperature).await;
    assert_eq!(status, 200, "disassociate: got {status} {answer}");

    // A time series IS the data stream, and an asset property reports the values of
    // the stream associated with it, so the samples go with the stream the call
    // detached: the property survives, reporting no history and no latest value
    // until a stream is associated with it again.
    assert_eq!(
        history_by_ids(&router, &asset_id, &temperature).await,
        Vec::new(),
        "a property the stream has left must report no history"
    );
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={temperature}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "read back by ids: got {status} {latest}");
    assert!(
        latest.get("propertyValue").is_none_or(Value::is_null),
        "a property the stream has left must report no latest value, got {latest}"
    );

    // The samples are not dropped: the stream holds them, and its alias is what
    // addresses them — the alias-addressed readers report the detached stream.
    assert_eq!(
        history(&router, &format!("propertyAlias={alias}")).await,
        written,
        "the detached stream must still report the samples written to it"
    );
    let (status, latest) = latest_by_alias(&router, alias).await;
    assert_eq!(status, 200, "latest by alias: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["value"]["doubleValue"].as_f64(),
        Some(written[written.len() - 1].1),
        "the detached stream's latest value must be the last sample written, got {latest}"
    );

    // Ingest addressed by the alias reaches the detached stream too: a stream no
    // asset property owns is still a stream.
    let late = (now_seconds() - 30, 43.5);
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": "detached-write",
            "propertyAlias": alias,
            "propertyValues": [{
                "value": {"doubleValue": late.1},
                "timestamp": {"timeInSeconds": late.0},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "BatchPutAssetPropertyValue: got {status} {put}");
    assert_eq!(
        put["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "a write to the detached stream must be accepted, got {put}"
    );
    let mut with_late = written.clone();
    with_late.push(late);
    assert_eq!(
        history(&router, &format!("propertyAlias={alias}")).await,
        with_late,
        "the detached stream must report what was written to it"
    );
    assert_eq!(
        history_by_ids(&router, &asset_id, &temperature).await,
        Vec::new(),
        "a write to the detached stream must not reach the property it left"
    );

    // An alias names ONE stream, so binding the detached one back to an asset
    // property makes it associated again rather than leaving two streams under one
    // alias: the alias and the pair name the same series once more, and the
    // samples the stream carried are the property's history again.
    let (status, rebound) = bind_alias(&router, &asset_id, &temperature, alias).await;
    assert_eq!(status, 200, "rebinding the alias got {status} {rebound}");
    let (status, by_alias) = describe_time_series(&router, &format!("alias={alias}")).await;
    assert_eq!(status, 200, "by alias after rebinding: got {status} {by_alias}");
    assert_eq!(
        by_alias["assetId"].as_str().unwrap_or(""),
        asset_id,
        "a rebound alias must name its asset property again, got {by_alias}"
    );
    assert_eq!(
        by_alias["propertyId"].as_str().unwrap_or(""),
        temperature,
        "a rebound alias must name its asset property again, got {by_alias}"
    );
    assert_eq!(
        by_alias["timeSeriesId"].as_str().unwrap_or(""),
        time_series_id_of(&asset_id, &temperature),
        "unexpected timeSeriesId in {by_alias}"
    );
    assert_eq!(
        history_by_ids(&router, &asset_id, &temperature).await,
        with_late,
        "the samples must come back with the stream the alias names"
    );

    // And it can be detached again: the association is re-made, so the same call
    // succeeds a second time rather than being refused as absent, and the samples
    // leave with the stream again.
    let (status, answer) = disassociate(&router, alias, &asset_id, &temperature).await;
    assert_eq!(status, 200, "second disassociate: got {status} {answer}");
    let (status, standing) = describe_time_series(&router, &format!("alias={alias}")).await;
    assert_eq!(status, 200, "by alias after the second call: got {status} {standing}");
    assert!(
        standing.get("assetId").is_none_or(Value::is_null),
        "the stream must be disassociated again, got {standing}"
    );
    assert_eq!(
        history_by_ids(&router, &asset_id, &temperature).await,
        Vec::new(),
        "the samples must leave with the stream again"
    );
}

#[tokio::test]
async fn a_detached_stream_is_only_re_associated_with_a_property_of_its_own_data_type() {
    let (router, _engine, asset_id, asset) = asset_fixture().await;
    let catalog = time_series_property_catalog();
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let alias = "/plant/7/boiler/temperature";
    let (status, bound) = bind_alias(&router, &asset_id, &temperature, alias).await;
    assert_eq!(status, 200, "binding the alias got {status} {bound}");
    let (status, answer) = disassociate(&router, alias, &asset_id, &temperature).await;
    assert_eq!(status, 200, "disassociate: got {status} {answer}");
    let (status, detached) = describe_time_series(&router, &format!("alias={alias}")).await;
    assert_eq!(status, 200, "by alias after the call: got {status} {detached}");

    // A stream is associated with an asset property only where the two carry the
    // same data type, so binding the detached DOUBLE stream's alias to a property
    // of every OTHER data type in the catalog is refused — and the refusal leaves
    // the stream exactly where it was rather than discarding it.
    for row in catalog.iter().filter(|row| row.data_type != "DOUBLE") {
        let case = row.name;
        let other = property_id(&asset["assetProperties"], case);
        let (status, body) = bind_alias(&router, &asset_id, &other, alias).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        let (status, after) = describe_time_series(&router, &format!("alias={alias}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stream must survive the refusal, got {status} {after}"
        );
        assert_eq!(
            after, detached,
            "case {case:?}: the refusal changed the stream: {after}"
        );
    }

    // A property of the stream's OWN data type takes it: the DOUBLE property it
    // came from, and another DOUBLE property of the same asset would do as well.
    let (status, rebound) = bind_alias(&router, &asset_id, &temperature, alias).await;
    assert_eq!(status, 200, "rebinding the alias got {status} {rebound}");
    let (status, by_alias) = describe_time_series(&router, &format!("alias={alias}")).await;
    assert_eq!(status, 200, "by alias after rebinding: got {status} {by_alias}");
    assert_eq!(
        by_alias["propertyId"].as_str().unwrap_or(""),
        temperature,
        "a rebound alias must name the property it was bound to, got {by_alias}"
    );
}
