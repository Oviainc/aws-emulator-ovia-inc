// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the asset instant a change to an asset property's alias
//! binding owes, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! The rule these suites pin: an asset's `assetLastUpdateDate` is the instant of
//! the last accepted write to that asset, and a property's alias binding is part
//! of the asset that carries the property — so EVERY accepted call that binds or
//! unbinds a property's alias moves that instant forward, whichever operation
//! makes the change, while a call that leaves the binding as it stood (a repeat, a
//! refusal, a read) leaves it exactly where it was. Every reader derived from the
//! record — `DescribeAsset`, `DescribeTimeSeries` and `ListTimeSeries` — reports
//! the one moved instant.
//!
//! The harness (`router`, `call`, `error_code`, …) is the one `asset_lifecycle`
//! defines, the alias-ingest helper is `time_series_ingestion`'s and the
//! `DescribeTimeSeries` reader is `time_series`'s; only the readers this suite
//! alone needs are local to it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, create_model, error_code, property_id, router};
use crate::support::now_seconds;
use crate::time_series::describe_time_series;
use crate::time_series_ingestion::ingest_by_alias;

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The asset-model properties these suites bind aliases to: one per
/// `PropertyDataType` a measurement can carry, so a row's data type comes from
/// here rather than from the row table. Source:
/// `smithy_query(query="shape", shape="PropertyDataType")` — STRING | INTEGER |
/// DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON, of which the last four
/// are not measurement types.
fn property_definitions() -> Value {
    json!([
        {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
        {"name": "Cycles", "dataType": "INTEGER", "type": {"measurement": {}}},
        {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
        {"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}},
        {"name": "Serial", "dataType": "STRING", "type": {"measurement": {}}},
    ])
}

/// A model with [`property_definitions`] and one asset of it. Returns
/// `(router, assetId, DescribeAsset body)`.
async fn asset_fixture() -> (MultiServiceRouter, String, Value) {
    let router = router();
    let model_id = create_model(&router, "BindingBoiler", property_definitions()).await;
    let asset_id = create_asset(&router, "BindingBoiler1", &model_id).await;
    let asset = describe_asset(&router, &asset_id).await;
    (router, asset_id, asset)
}

/// `DescribeAsset`'s body for `asset_id`, which must answer 200.
async fn describe_asset(router: &MultiServiceRouter, asset_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {asset_id}: got {status} {body}");
    body
}

/// A modelled `Timestamp` member as epoch seconds.
fn epoch_seconds(body: &Value, member: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} is not an epoch-seconds number in {body}"))
}

/// The alias the property named `name` carries in a `DescribeAsset` body, if any.
fn bound_alias(asset: &Value, name: &str) -> Option<String> {
    asset["assetProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetProperties in {asset}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("no property named {name} in {asset}"))
        .get("alias")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `timeSeriesLastUpdateDate` `ListTimeSeries` reports for the series of the
/// `(asset_id, property_id)` pair.
async fn listed_last_update(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/timeseries?assetId={asset_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListTimeSeries: got {status} {body}");
    body["TimeSeriesSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no TimeSeriesSummaries in {body}"))
        .iter()
        .find(|summary| summary["propertyId"].as_str() == Some(property_id))
        .unwrap_or_else(|| panic!("the pair's series is not listed in {body}"))
        ["timeSeriesLastUpdateDate"]
        .clone()
}

/// `PUT /assets/{asset}/properties/{property}` — `UpdateAssetProperty`, which
/// binds the alias it carries and clears the property's alias when it carries
/// none.
async fn update_property(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    alias: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({});
    if let Some(alias) = alias {
        body["propertyAlias"] = json!(alias);
    }
    call(
        router,
        "PUT",
        &format!("/assets/{asset_id}/properties/{property_id}"),
        Some(body),
    )
    .await
}

/// `POST /timeseries/associate?alias=…&assetId=…&propertyId=…`.
async fn associate(
    router: &MultiServiceRouter,
    alias: &str,
    asset_id: &str,
    property_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/timeseries/associate?alias={alias}&assetId={asset_id}&propertyId={property_id}"),
        Some(json!({})),
    )
    .await
}

/// `POST /timeseries/disassociate?alias=…&assetId=…&propertyId=…`.
async fn disassociate(
    router: &MultiServiceRouter,
    alias: &str,
    asset_id: &str,
    property_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/timeseries/disassociate?alias={alias}&assetId={asset_id}&propertyId={property_id}"
        ),
        Some(json!({})),
    )
    .await
}

/// `POST /timeseries/delete?<query>`.
async fn delete_time_series(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/timeseries/delete?{query}"),
        Some(json!({})),
    )
    .await
}

/// Which operation form a row changes the binding with.
#[derive(Debug, Clone, Copy)]
enum Rebind {
    /// `AssociateTimeSeriesToAssetProperty` binds a disassociated stream to the
    /// property.
    Associate,
    /// `UpdateAssetProperty` carrying `propertyAlias` binds the alias.
    UpdateBinds,
    /// `DisassociateTimeSeriesFromAssetProperty` unbinds it again.
    Disassociate,
    /// `UpdateAssetProperty` with `propertyAlias` omitted clears the binding.
    UpdateClears,
    /// `DeleteTimeSeries` addressed by the alias of the associated stream.
    DeleteByAlias,
    /// `DeleteTimeSeries` addressed by the `(assetId, propertyId)` pair.
    DeleteByPair,
}

impl Rebind {
    /// Whether the property has to carry the row's alias BEFORE the call: the
    /// unbinding forms need an association to break, the binding forms need the
    /// property free.
    fn needs_a_binding(self) -> bool {
        match self {
            Rebind::Associate | Rebind::UpdateBinds => false,
            Rebind::Disassociate
            | Rebind::UpdateClears
            | Rebind::DeleteByAlias
            | Rebind::DeleteByPair => true,
        }
    }

    /// The alias the property must carry AFTER the call.
    fn leaves_bound(self) -> bool {
        matches!(self, Rebind::Associate | Rebind::UpdateBinds)
    }
}

/// One row of the rebinding table: the property whose binding changes, the alias,
/// a reading of that property's data type (so the `Associate` row has a
/// disassociated stream of a matching type to bind), and the operation form that
/// changes it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    property: &'static str,
    alias: &'static str,
    value: fn() -> Value,
    call: Rebind,
}

/// The rows differ along everything the rule generalises over: WHICH operation
/// changes the binding, whether the change binds or unbinds, which form addresses
/// the series, the data type of the property (DOUBLE, INTEGER, STRING, BOOLEAN)
/// and the shape of the alias (a deep path, a shallow one, a dotted segment and a
/// single bare segment — `PropertyAlias` forbids only the control characters).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "AssociateTimeSeriesToAssetProperty binds a DOUBLE stream",
            property: "Temperature",
            alias: "/company/windfarm/3/turbine/7/temperature",
            value: || json!({"doubleValue": 72.5}),
            call: Rebind::Associate,
        },
        Row {
            case: "UpdateAssetProperty binds an INTEGER property's alias",
            property: "Cycles",
            alias: "/plant/1/cycles",
            value: || json!({"integerValue": 41}),
            call: Rebind::UpdateBinds,
        },
        Row {
            case: "DisassociateTimeSeriesFromAssetProperty unbinds a STRING alias",
            property: "Status",
            alias: "/plant/1/boiler/status.v2",
            value: || json!({"stringValue": "RUNNING"}),
            call: Rebind::Disassociate,
        },
        Row {
            case: "UpdateAssetProperty clears a BOOLEAN property's alias",
            property: "Running",
            alias: "running",
            value: || json!({"booleanValue": true}),
            call: Rebind::UpdateClears,
        },
        Row {
            case: "DeleteTimeSeries by alias drops the binding",
            property: "Flow",
            alias: "/plant/2/pump/flow",
            value: || json!({"doubleValue": 3.25}),
            call: Rebind::DeleteByAlias,
        },
        Row {
            case: "DeleteTimeSeries by the pair drops the binding",
            property: "Serial",
            alias: "serial",
            value: || json!({"stringValue": "SN-1"}),
            call: Rebind::DeleteByPair,
        },
    ]
}

/// Make the row's call, answering `(status, body)`.
async fn rebind(
    router: &MultiServiceRouter,
    form: Rebind,
    asset_id: &str,
    property: &str,
    alias: &str,
) -> (u16, Value) {
    match form {
        Rebind::Associate => associate(router, alias, asset_id, property).await,
        Rebind::UpdateBinds => update_property(router, asset_id, property, Some(alias)).await,
        Rebind::Disassociate => disassociate(router, alias, asset_id, property).await,
        Rebind::UpdateClears => update_property(router, asset_id, property, None).await,
        Rebind::DeleteByAlias => delete_time_series(router, &format!("alias={alias}")).await,
        Rebind::DeleteByPair => {
            delete_time_series(router, &format!("assetId={asset_id}&propertyId={property}")).await
        }
    }
}

#[tokio::test]
async fn every_accepted_change_to_a_property_alias_moves_the_asset_last_update_date() {
    let (router, asset_id, asset) = asset_fixture().await;
    let at = now_seconds() - 120;

    for row in rows() {
        let Row {
            case,
            property: name,
            alias,
            value,
            call: form,
        } = row;
        let pid = property_id(&asset["assetProperties"], name);

        // --- the state the row's call acts on ------------------------------
        if form.needs_a_binding() {
            let (status, body) = update_property(&router, &asset_id, &pid, Some(alias)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: binding the alias first got {status} {body}"
            );
        } else if matches!(form, Rebind::Associate) {
            // `AssociateTimeSeriesToAssetProperty` binds an EXISTING stream, so
            // the alias has to stand for a disassociated one: ingesting against
            // an unbound alias is what creates it, with the data type of the
            // reading.
            ingest_by_alias(&router, alias, value(), at).await;
        }

        let before = describe_asset(&router, &asset_id).await;
        let was = epoch_seconds(&before, "assetLastUpdateDate");
        let (status, series_before) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTimeSeries before got {status} {series_before}"
        );
        assert_eq!(
            series_before["timeSeriesLastUpdateDate"], before["assetLastUpdateDate"],
            "case {case:?}: the pair's series must report the asset's instant, got {series_before}"
        );

        // --- the call under test -------------------------------------------
        let (status, answer) = rebind(&router, form, &asset_id, &pid, alias).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the rebinding call got {status} {answer}"
        );

        // The binding really changed: that is what makes this row a write.
        let after = describe_asset(&router, &asset_id).await;
        let carried = bound_alias(&after, name);
        assert_eq!(
            carried.as_deref(),
            form.leaves_bound().then_some(alias),
            "case {case:?}: expected the property to carry {:?}, got {carried:?}",
            form.leaves_bound().then_some(alias)
        );

        // --- the instant the write owes ------------------------------------
        let now_reported = epoch_seconds(&after, "assetLastUpdateDate");
        assert!(
            now_reported > was,
            "case {case:?}: expected assetLastUpdateDate to move past {was}, got {now_reported}"
        );
        assert_eq!(
            after["assetCreationDate"], before["assetCreationDate"],
            "case {case:?}: assetCreationDate must not move, got {after}"
        );

        // Every reader derived from the record reports the ONE moved instant.
        let (status, series_after) =
            describe_time_series(&router, &format!("assetId={asset_id}&propertyId={pid}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTimeSeries after got {status} {series_after}"
        );
        assert_eq!(
            series_after["timeSeriesLastUpdateDate"], after["assetLastUpdateDate"],
            "case {case:?}: DescribeTimeSeries reports a stale instant in {series_after}"
        );
        let listed = listed_last_update(&router, &asset_id, &pid).await;
        assert_eq!(
            listed, after["assetLastUpdateDate"],
            "case {case:?}: ListTimeSeries reports a stale instant: {listed}"
        );

        // A read settles nothing: the instant stands where the write left it.
        let reread = describe_asset(&router, &asset_id).await;
        assert_eq!(
            reread["assetLastUpdateDate"], after["assetLastUpdateDate"],
            "case {case:?}: reading moved the instant, got {reread}"
        );
    }
}

/// The request a no-change row sends: which binding operation, and what it names.
#[derive(Debug)]
enum Query {
    /// `AssociateTimeSeriesToAssetProperty(alias, assetId, propertyId)`.
    Associate(String, String, String),
    /// `DisassociateTimeSeriesFromAssetProperty(alias, assetId, propertyId)`.
    Disassociate(String, String, String),
    /// `DeleteTimeSeries`, with the whole query string it is addressed by.
    Delete(String),
}

#[tokio::test]
async fn a_call_that_changes_no_binding_leaves_the_asset_last_update_date_alone() {
    let (router, asset_id, asset) = asset_fixture().await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let cycles = property_id(&asset["assetProperties"], "Cycles");
    let status_property = property_id(&asset["assetProperties"], "Status");
    let alias = "/plant/7/boiler/temperature";
    let at = now_seconds() - 90;

    // One accepted association, so the store holds a binding the rows below fail
    // to change.
    ingest_by_alias(&router, alias, json!({"doubleValue": 12.5}), at).await;
    let (status, bound) = associate(&router, alias, &asset_id, &temperature).await;
    assert_eq!(status, 200, "the first association got {status} {bound}");
    let settled = describe_asset(&router, &asset_id).await;
    let frozen = settled["assetLastUpdateDate"].clone();

    // A second stream, for the row refused over a data type rather than over a
    // resource it cannot resolve. Ingesting against an unbound alias binds
    // nothing to this asset, so it is a row of this table too.
    let other_alias = "/plant/7/boiler/cycles";
    ingest_by_alias(&router, other_alias, json!({"integerValue": 7}), at).await;
    assert_eq!(
        describe_asset(&router, &asset_id).await["assetLastUpdateDate"],
        frozen,
        "a stream created under an unbound alias binds nothing to this asset"
    );

    // Each row leaves the binding in force exactly as it stands: a repeat of the
    // call that made it, and a refusal of every kind the binding operations
    // declare — an unresolvable resource in either position, a data type that is
    // not the property's, and a request that identifies no one series.
    let rows: Vec<(&str, (u16, &str), Query)> = vec![
        (
            "a repeat of the association that is already in force",
            (200, ""),
            Query::Associate(alias.to_string(), asset_id.clone(), temperature.clone()),
        ),
        (
            "an association of a stream whose data type is not the property's",
            (400, "InvalidRequestException"),
            Query::Associate(
                other_alias.to_string(),
                asset_id.clone(),
                status_property.clone(),
            ),
        ),
        (
            "an association naming an alias no stream stands under",
            (404, "ResourceNotFoundException"),
            Query::Associate("/no/such/stream".to_string(), asset_id.clone(), cycles.clone()),
        ),
        (
            "an association naming an asset the store does not hold",
            (404, "ResourceNotFoundException"),
            Query::Associate(
                other_alias.to_string(),
                ABSENT_ID.to_string(),
                cycles.clone(),
            ),
        ),
        (
            "an association naming a property the asset does not have",
            (404, "ResourceNotFoundException"),
            Query::Associate(
                other_alias.to_string(),
                asset_id.clone(),
                ABSENT_ID.to_string(),
            ),
        ),
        (
            "a disassociation naming an association the store does not hold",
            (404, "ResourceNotFoundException"),
            Query::Disassociate(alias.to_string(), asset_id.clone(), cycles.clone()),
        ),
        (
            "a delete naming neither identifying form",
            (400, "InvalidRequestException"),
            Query::Delete(String::new()),
        ),
        (
            "a delete naming half of the pair",
            (400, "InvalidRequestException"),
            Query::Delete(format!("assetId={asset_id}")),
        ),
    ];
    for (case, (expected_status, expected_code), query) in rows {
        let (status, body) = match &query {
            Query::Associate(alias, asset, property) => {
                associate(&router, alias, asset, property).await
            }
            Query::Disassociate(alias, asset, property) => {
                disassociate(&router, alias, asset, property).await
            }
            Query::Delete(query) => delete_time_series(&router, query).await,
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: unexpected error code in {body}"
            );
        }
        let after = describe_asset(&router, &asset_id).await;
        assert_eq!(
            after["assetLastUpdateDate"], frozen,
            "case {case:?}: a call that changed no binding moved the instant, got {after}"
        );
        assert_eq!(
            bound_alias(&after, "Temperature").as_deref(),
            Some(alias),
            "case {case:?}: the binding in force must be untouched, got {after}"
        );
    }

    // The readers derived from the record move nothing either.
    let (status, described) =
        describe_time_series(&router, &format!("assetId={asset_id}&propertyId={temperature}")).await;
    assert_eq!(status, 200, "DescribeTimeSeries got {status} {described}");
    assert_eq!(
        described["timeSeriesLastUpdateDate"], frozen,
        "DescribeTimeSeries must report the instant the last write left, got {described}"
    );
    assert_eq!(
        listed_last_update(&router, &asset_id, &temperature).await,
        frozen,
        "ListTimeSeries must report the instant the last write left"
    );
    let reread = describe_asset(&router, &asset_id).await;
    assert_eq!(
        reread["assetLastUpdateDate"], frozen,
        "reading the asset must not move its instant, got {reread}"
    );
}
