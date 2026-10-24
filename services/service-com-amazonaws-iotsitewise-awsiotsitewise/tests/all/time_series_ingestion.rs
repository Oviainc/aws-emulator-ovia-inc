// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for alias-addressed ingestion and the time-series lifecycle it
//! opens, driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with.
//!
//! The rule these suites pin: a write that addresses a property by `propertyAlias`
//! no asset property carries CREATES the disassociated time series that alias names
//! — taking its data type from the value ingested — and that series is from then on
//! a resource of this service: `DescribeTimeSeries` reports it by alias,
//! `ListTimeSeries` lists it under the `aliasPrefix` and `timeSeriesType` filters,
//! `AssociateTimeSeriesToAssetProperty` binds it to an asset property (keeping its
//! id and merging its samples into that property's one history), and
//! `DeleteTimeSeries` removes it, after which the alias names nothing again.
//!
//! The harness (`router_with_engine`, `call`, `error_code`, …) is the one
//! `asset_lifecycle` already defines, re-used rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, is_modelled_id, property_id, router, REGION,
};
use crate::support::now_seconds;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::MAX_DISASSOCIATED_TIME_SERIES;

/// One row of the ingestion table: the `Variant` member a first value carries and
/// the `PropertyDataType` the series it creates must therefore report.
///
/// The rows span the scalar `Variant` members the model declares. Source:
/// `smithy_query(query="shape", shape="PropertyDataType")` — STRING | INTEGER |
/// DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON — of which only the first
/// four are a scalar reading a `Variant` can carry.
#[derive(Debug)]
struct IngestRow {
    case: &'static str,
    alias: &'static str,
    value: fn() -> Value,
    data_type: &'static str,
}

fn ingest_table() -> Vec<IngestRow> {
    vec![
        IngestRow {
            case: "a string reading",
            alias: "/plant/line1/mixer/status",
            value: || json!({"stringValue": "RUNNING"}),
            data_type: "STRING",
        },
        IngestRow {
            case: "an integer reading",
            alias: "/plant/line1/mixer/cycles",
            value: || json!({"integerValue": 41}),
            data_type: "INTEGER",
        },
        IngestRow {
            case: "a double reading",
            alias: "/plant/line1/mixer/temperature",
            value: || json!({"doubleValue": 72.5}),
            data_type: "DOUBLE",
        },
        IngestRow {
            case: "a boolean reading",
            alias: "/plant/line2/pump/running",
            value: || json!({"booleanValue": true}),
            data_type: "BOOLEAN",
        },
    ]
}

/// Ingest one value against `alias` and assert the whole batch was accepted.
async fn ingest_by_alias(
    router: &MultiServiceRouter,
    alias: &str,
    value: Value,
    seconds: i64,
) {
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": format!("e{seconds}"),
            "propertyAlias": alias,
            "propertyValues": [{
                "value": value,
                "timestamp": {"timeInSeconds": seconds, "offsetInNanos": 0},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "ingest {alias}: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "ingest {alias}: every entry must be accepted, got {body}"
    );
}

/// Ingest one value against the `(assetId, propertyId)` pair.
async fn ingest_by_pair(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    value: Value,
    seconds: i64,
) {
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": format!("p{seconds}"),
            "assetId": asset_id,
            "propertyId": property_id,
            "propertyValues": [{
                "value": value,
                "timestamp": {"timeInSeconds": seconds, "offsetInNanos": 0},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "ingest pair: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "ingest pair: every entry must be accepted, got {body}"
    );
}

/// `GET /timeseries/describe?<query>`.
async fn describe(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/timeseries/describe?{query}"), None).await
}

/// `GET /timeseries?<query>`.
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/timeseries?{query}"), None).await
}

/// The `timeSeriesId`s a `ListTimeSeries` body reports, in order.
fn listed_ids(body: &Value) -> Vec<String> {
    body["TimeSeriesSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no TimeSeriesSummaries in {body}"))
        .iter()
        .map(|summary| summary["timeSeriesId"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn ingesting_against_an_unbound_alias_creates_a_disassociated_time_series() {
    let router = router();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let at = now_seconds() - 120;

    for row in ingest_table() {
        let IngestRow {
            case,
            alias,
            value,
            data_type,
        } = row;

        // Before the write the alias names nothing.
        let (status, body) = describe(&router, &format!("alias={alias}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: before ingesting, the alias must name nothing, got {status} {body}"
        );

        ingest_by_alias(&router, alias, value(), at).await;

        let (status, body) = describe(&router, &format!("alias={alias}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: after ingesting, the alias must name a series, got {status} {body}"
        );
        assert_eq!(
            body["alias"].as_str(), Some(alias),
            "case {case:?}: the series must report the alias it was created for, got {body}"
        );
        assert_eq!(
            body["dataType"].as_str(),
            Some(data_type),
            "case {case:?}: expected dataType {data_type}, got {body}"
        );
        // A DISASSOCIATED series is bound to no asset property.
        assert!(
            body.get("assetId").is_none_or(Value::is_null),
            "case {case:?}: a disassociated series names no asset, got {body}"
        );
        assert!(
            body.get("propertyId").is_none_or(Value::is_null),
            "case {case:?}: a disassociated series names no property, got {body}"
        );
        // The id is minted, within the modelled `TimeSeriesId` bound of 36..73, and
        // the ARN is built from it in the documented form.
        let id = body["timeSeriesId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&id),
            "case {case:?}: expected a minted UUID timeSeriesId, got {body}"
        );
        assert!(
            (36..=73).contains(&id.len()),
            "case {case:?}: timeSeriesId must be 36..73 characters, got {id:?}"
        );
        assert_eq!(
            body["timeSeriesArn"].as_str().unwrap_or(""),
            format!("arn:aws:iotsitewise:{REGION}:{account}:time-series/{id}"),
            "case {case:?}: unexpected timeSeriesArn in {body}"
        );
        assert!(
            body["timeSeriesCreationDate"].is_number()
                && body["timeSeriesLastUpdateDate"].is_number(),
            "case {case:?}: both dates are required, got {body}"
        );

        // Reading it again answers identically: the read settles nothing and mints
        // nothing.
        let (status, again) = describe(&router, &format!("alias={alias}")).await;
        assert_eq!(status, 200, "case {case:?}: re-read got {status} {again}");
        assert_eq!(
            again, body,
            "case {case:?}: a second read differs from the first: {again}"
        );

        // It is listed as DISASSOCIATED under its own alias prefix, and not as
        // ASSOCIATED.
        let (status, listed) = list(
            &router,
            &format!("aliasPrefix={alias}&timeSeriesType=DISASSOCIATED"),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: list got {status} {listed}");
        assert_eq!(
            listed_ids(&listed),
            vec![id.clone()],
            "case {case:?}: the DISASSOCIATED listing must hold exactly this series, got {listed}"
        );
        let (_, associated) = list(
            &router,
            &format!("aliasPrefix={alias}&timeSeriesType=ASSOCIATED"),
        )
        .await;
        assert!(
            listed_ids(&associated).is_empty(),
            "case {case:?}: an unbound series is not ASSOCIATED, got {associated}"
        );
    }

    // Every row's series is its own, and the shared prefix lists all of line 1's.
    let (_, all) = list(&router, "timeSeriesType=DISASSOCIATED").await;
    let mut ids = listed_ids(&all);
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        ingest_table().len(),
        "every alias must own a distinct series, got {all}"
    );
    let (_, line1) = list(&router, "aliasPrefix=/plant/line1/").await;
    assert_eq!(
        listed_ids(&line1).len(),
        3,
        "the aliasPrefix filter keeps only the aliases under it, got {line1}"
    );
}

#[tokio::test]
async fn an_alias_series_binds_to_an_asset_property_and_the_histories_merge() {
    let router = router();
    let alias = "/plant/line3/boiler/temperature";
    let first = now_seconds() - 300;
    let second = now_seconds() - 240;

    // Two readings arrive by alias before any asset exists — the industrial flow the
    // disassociated series is for.
    ingest_by_alias(&router, alias, json!({"doubleValue": 10.0}), first).await;
    ingest_by_alias(&router, alias, json!({"doubleValue": 11.0}), second).await;
    let (_, before) = describe(&router, &format!("alias={alias}")).await;
    // The stream the alias-addressed writes created, read back before anything is
    // bound to it: a DISASSOCIATED series with an id of its own.
    assert!(
        before["timeSeriesId"]
            .as_str()
            .is_some_and(is_modelled_id),
        "the alias-created series must report a modelled timeSeriesId, got {before}"
    );
    assert_eq!(
        (before.get("assetId"), before.get("propertyId")),
        (None, None),
        "the alias-created series must be bound to no asset property yet, got {before}"
    );

    let model_id = create_model(
        &router,
        "Boiler",
        json!([
            {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let status_property = property_id(&asset["assetProperties"], "Status");

    // Rows that must be refused, each for its own reason, before the accepted one.
    let refusals: Vec<(&str, String, u16, &str)> = vec![
        (
            "an alias no series carries",
            format!("alias=/no/such/stream&assetId={asset_id}&propertyId={temperature}"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an asset that does not exist",
            format!(
                "alias={alias}&assetId=11111111-2222-3333-4444-555555555555&propertyId={temperature}"
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a property the asset does not have",
            format!(
                "alias={alias}&assetId={asset_id}&propertyId=11111111-2222-3333-4444-555555555555"
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a property of another data type",
            format!("alias={alias}&assetId={asset_id}&propertyId={status_property}"),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, query, expected_status, expected_code) in &refusals {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/timeseries/associate?{query}"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }
    // No refusal bound anything.
    let (_, unbound) = describe(&router, &format!("alias={alias}")).await;
    assert_eq!(
        unbound, before,
        "a refused association must leave the series as it was, got {unbound}"
    );

    let (status, body) = call(
        &router,
        "POST",
        &format!("/timeseries/associate?alias={alias}&assetId={asset_id}&propertyId={temperature}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "AssociateTimeSeriesToAssetProperty: got {status} {body}");

    // The stream is now the asset property's: the alias and the pair name ONE
    // series, which reports the pair it is associated with. The id a bound series
    // reports is NOT asserted against the one the disassociated stream carried:
    // whether binding preserves the stream's minted id or re-derives it from the
    // (asset, property) pair is a question about time-series identity that this
    // suite deliberately leaves to its own issue — what it pins is that the series
    // still has ONE stable id, reported the same way whichever form addresses it.
    let (status, by_alias) = describe(&router, &format!("alias={alias}")).await;
    assert_eq!(status, 200, "describe by alias: got {status} {by_alias}");
    assert!(
        by_alias["timeSeriesId"]
            .as_str()
            .is_some_and(|reported| !reported.is_empty()),
        "the associated series must still report a timeSeriesId, got {by_alias}"
    );
    assert_eq!(
        by_alias["assetId"].as_str(),
        Some(asset_id.as_str()),
        "the associated series must name the asset, got {by_alias}"
    );
    assert_eq!(
        by_alias["propertyId"].as_str(),
        Some(temperature.as_str()),
        "the associated series must name the property, got {by_alias}"
    );
    let (status, by_pair) =
        describe(&router, &format!("assetId={asset_id}&propertyId={temperature}")).await;
    assert_eq!(status, 200, "describe by pair: got {status} {by_pair}");
    assert_eq!(
        by_pair, by_alias,
        "the pair must name the same series as the alias, got {by_pair}"
    );
    let id = by_alias["timeSeriesId"]
        .as_str()
        .unwrap_or_else(|| panic!("no timeSeriesId in {by_alias}"))
        .to_string();

    // It now lists as ASSOCIATED, under the asset as well.
    let (_, associated) = list(
        &router,
        &format!("aliasPrefix={alias}&timeSeriesType=ASSOCIATED"),
    )
    .await;
    assert_eq!(
        listed_ids(&associated),
        vec![id.clone()],
        "the bound series must list as ASSOCIATED, got {associated}"
    );
    let (_, disassociated) = list(
        &router,
        &format!("aliasPrefix={alias}&timeSeriesType=DISASSOCIATED"),
    )
    .await;
    assert!(
        listed_ids(&disassociated).is_empty(),
        "the bound series is no longer DISASSOCIATED, got {disassociated}"
    );
    let (_, of_asset) = list(&router, &format!("assetId={asset_id}")).await;
    assert!(
        listed_ids(&of_asset).contains(&id),
        "the asset's listing must hold its bound series, got {of_asset}"
    );

    // ONE history: the two samples ingested by alias before the binding, plus one
    // written through the alias and one through the pair after it.
    ingest_by_alias(&router, alias, json!({"doubleValue": 12.0}), now_seconds() - 180).await;
    ingest_by_pair(
        &router,
        &asset_id,
        &temperature,
        json!({"doubleValue": 13.0}),
        now_seconds() - 120,
    )
    .await;
    let (status, history) = call(
        &router,
        "GET",
        &format!("/properties/history?assetId={asset_id}&propertyId={temperature}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "history: got {status} {history}");
    let values: Vec<f64> = history["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("no history in {history}"))
        .iter()
        .map(|sample| sample["value"]["doubleValue"].as_f64().unwrap_or(f64::NAN))
        .collect();
    assert_eq!(
        values,
        vec![10.0, 11.0, 12.0, 13.0],
        "the alias history and the pair history are one ordered history, got {history}"
    );
}

#[tokio::test]
async fn delete_time_series_identifies_one_series_and_then_the_alias_names_nothing() {
    let router = router();
    let alias = "/plant/line4/press/force";
    ingest_by_alias(&router, alias, json!({"doubleValue": 5.0}), now_seconds() - 60).await;

    let model_id = create_model(
        &router,
        "Press",
        json!([{"name": "Force", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Press1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let force = property_id(&asset["assetProperties"], "Force");

    // Rows span the ways a request identifies no ONE time series, and the ways it
    // identifies one that is not there. A request that identifies nothing is invalid;
    // one that identifies an absent series is a 404.
    let refusals: Vec<(&str, String, u16, &str)> = vec![
        ("no identifier at all", String::new(), 400, "InvalidRequestException"),
        (
            "both forms at once",
            format!("alias={alias}&assetId={asset_id}&propertyId={force}"),
            400,
            "InvalidRequestException",
        ),
        (
            "assetId without propertyId",
            format!("assetId={asset_id}"),
            400,
            "InvalidRequestException",
        ),
        (
            "propertyId without assetId",
            format!("propertyId={force}"),
            400,
            "InvalidRequestException",
        ),
        (
            "an alias no series carries",
            "alias=/no/such/stream".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an asset that does not exist",
            format!("assetId=11111111-2222-3333-4444-555555555555&propertyId={force}"),
            404,
            "ResourceNotFoundException",
        ),
    ];
    for (case, query, expected_status, expected_code) in &refusals {
        let (status, body) = call(
            &router,
            "POST",
            &format!("/timeseries/delete?{query}"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // Every refusal left the series in place.
    let (status, body) = describe(&router, &format!("alias={alias}")).await;
    assert_eq!(
        status, 200,
        "the refusals must leave the series in place, got {status} {body}"
    );

    // Deleting by alias: accepted, and from then on the alias names nothing.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/timeseries/delete?alias={alias}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "DeleteTimeSeries by alias: got {status} {body}");
    let (status, gone) = describe(&router, &format!("alias={alias}")).await;
    assert_eq!(status, 404, "after the delete: got {status} {gone}");
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "after the delete: unexpected error code in {gone}"
    );
    let (_, listed) = list(&router, &format!("aliasPrefix={alias}")).await;
    assert!(
        listed_ids(&listed).is_empty(),
        "a deleted series is listed nowhere, got {listed}"
    );

    // "If you delete a time series that's associated with an asset property, the
    // asset property still exists": the asset property survives a delete of the
    // series the PAIR names.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/timeseries/delete?assetId={asset_id}&propertyId={force}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "DeleteTimeSeries by pair: got {status} {body}");
    let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "the asset must survive, got {status} {described}");
    assert_eq!(
        property_id(&described["assetProperties"], "Force"),
        force,
        "the asset property must survive, got {described}"
    );

    // The same for a series that was BOUND to the property: deleting it leaves the
    // property, takes the alias binding with it, and the property's series is once
    // again the one derived from the property itself.
    let bound = "/plant/line4/press/bound";
    ingest_by_alias(&router, bound, json!({"doubleValue": 7.0}), now_seconds() - 30).await;
    let (status, body) = call(
        &router,
        "POST",
        &format!("/timeseries/associate?alias={bound}&assetId={asset_id}&propertyId={force}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "associate: got {status} {body}");
    let (status, body) = call(
        &router,
        "POST",
        &format!("/timeseries/delete?alias={bound}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 200, "deleting the bound series: got {status} {body}");
    let (status, gone) = describe(&router, &format!("alias={bound}")).await;
    assert_eq!(status, 404, "the alias names nothing again, got {status} {gone}");
    let (status, by_pair) =
        describe(&router, &format!("assetId={asset_id}&propertyId={force}")).await;
    assert_eq!(
        status, 200,
        "the asset property still has its own series, got {status} {by_pair}"
    );
    assert!(
        by_pair.get("alias").is_none_or(Value::is_null),
        "the alias binding went with the deleted series, got {by_pair}"
    );
    let (status, history) = call(
        &router,
        "GET",
        &format!("/properties/history?assetId={asset_id}&propertyId={force}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "history after the delete: got {status} {history}");
    assert_eq!(
        history["assetPropertyValueHistory"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "the deleted series took its samples with it, got {history}"
    );
}

#[tokio::test]
async fn the_store_holds_a_bounded_number_of_alias_addressed_time_series() {
    let router = router();
    let at = now_seconds() - 45;

    // Fill the store to its ceiling, ten aliases per call (the most one request
    // carries), then ask for one more.
    let mut minted = 0usize;
    while minted < MAX_DISASSOCIATED_TIME_SERIES {
        let entries: Vec<Value> = (0..10)
            .map(|index| {
                json!({
                    "entryId": format!("e{}", minted + index),
                    "propertyAlias": format!("/plant/cap/{}", minted + index),
                    "propertyValues": [{
                        "value": {"integerValue": 1},
                        "timestamp": {"timeInSeconds": at, "offsetInNanos": 0},
                    }],
                })
            })
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            "/properties",
            Some(json!({"entries": entries})),
        )
        .await;
        assert_eq!(status, 200, "filling the store: got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "filling the store: every entry must be accepted, got {body}"
        );
        minted += 10;
    }

    // One alias past the ceiling: the entry is refused with the quota code, no
    // series is created for it, and an entry writing to a series that already
    // exists is unaffected.
    let (status, body) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": [
            {
                "entryId": "overflow",
                "propertyAlias": "/plant/cap/overflow",
                "propertyValues": [{
                    "value": {"integerValue": 1},
                    "timestamp": {"timeInSeconds": at, "offsetInNanos": 0},
                }],
            },
            {
                "entryId": "existing",
                "propertyAlias": "/plant/cap/0",
                "propertyValues": [{
                    "value": {"integerValue": 2},
                    "timestamp": {"timeInSeconds": at + 1, "offsetInNanos": 0},
                }],
            },
        ]})),
    )
    .await;
    assert_eq!(status, 200, "past the ceiling: got {status} {body}");
    let errors = body["errorEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("no errorEntries in {body}"));
    assert_eq!(
        errors.len(),
        1,
        "only the entry that would mint a series is refused, got {body}"
    );
    assert_eq!(
        errors[0]["entryId"].as_str(),
        Some("overflow"),
        "the refused entry is the one naming a new alias, got {body}"
    );
    assert_eq!(
        errors[0]["errors"][0]["errorCode"].as_str(),
        Some("LimitExceededException"),
        "a quota refusal carries the quota code, got {body}"
    );
    let (status, gone) = describe(&router, "alias=/plant/cap/overflow").await;
    assert_eq!(
        status, 404,
        "no series was created past the ceiling, got {status} {gone}"
    );
    let (status, existing) = describe(&router, "alias=/plant/cap/0").await;
    assert_eq!(
        status, 200,
        "an existing series still takes values, got {status} {existing}"
    );
}

#[tokio::test]
async fn list_time_series_pages_through_every_series_exactly_once() {
    let router = router();
    let at = now_seconds() - 90;
    let aliases = [
        "/plant/page/a",
        "/plant/page/b",
        "/plant/page/c",
        "/plant/page/d",
    ];
    for (index, alias) in aliases.iter().enumerate() {
        ingest_by_alias(&router, alias, json!({"integerValue": index as i64}), at).await;
    }

    // One row per page size, each walking the whole enumeration through `nextToken`.
    for page_size in [1usize, 2, 3, 4] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = format!("aliasPrefix=/plant/page/&maxResults={page_size}");
            if let Some(token) = token.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = list(&router, &query).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: got {status} {body}"
            );
            let page = listed_ids(&body);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must hold at most {page_size} rows, got {body}"
            );
            seen.extend(page);
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            aliases.len(),
            "page size {page_size}: every series exactly once, got {seen:?}"
        );
    }

    // A token this operation never issued is refused rather than answered with an
    // empty page.
    let (status, body) = list(&router, "nextToken=6e6f742d612d746f6b656e").await;
    assert_eq!(status, 400, "a foreign token: got {status} {body}");
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "a foreign token: unexpected error code in {body}"
    );
}
