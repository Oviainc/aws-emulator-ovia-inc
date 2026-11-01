// SPDX-License-Identifier: CC0-1.0

//! The four resource families whose absence this service states in its OWN
//! sentence rather than in the shared `<Kind> with ID <id> not found` form, each
//! pinned over a SET of inputs instead of over one recorded request:
//!
//! 1. A COMPOSITE MODEL of an asset model is addressable only within that asset
//!    model — the same composite-model id can be absent from many models — so the
//!    absence names both halves of the address, each echoed as the request
//!    supplied it, from every operation that addresses one.
//! 2. An ASSET is addressable by its own id or by the `externalId:` selector the
//!    `CustomID` pattern admits; which of the two the REQUEST used is what failed
//!    to resolve, so a selector-addressed request is answered as an EXTERNAL ID
//!    that does not exist, carrying the bare external id, while an
//!    id-addressed one keeps the shared form.
//! 3. A TIME SERIES addressed by its `alias` is resolved as a `dataAlias` within
//!    one account, so the absence names the account the lookup ran in and the
//!    alias it ran for — from the read, the delete and the binding alike.
//! 4. A GATEWAY read and a gateway update are worded differently by the service,
//!    so the two refusals stay two sentences.
//!
//! Every expected string below is written out BY HAND as an independent oracle:
//! this suite calls no message helper of the crate, so a handler that drifts
//! back to a template of its own cannot agree with itself. Everything is driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};
use crate::asset_model_composite_model::{create_composite, PAST_THE_OWED_TRANSITION};
use crate::gateway_capability_configuration::create_gateway;
use crate::gateway_lifecycle::GATEWAYS;
use crate::hierarchies_etags_and_refusals::{create_asset, create_model, error_message};
use crate::support::now_seconds;
use crate::time_series_ingestion::ingest_by_alias;

/// A well-formed `ID` / `CustomID` UUID nothing in this suite is stored under.
const UNKNOWN_ID: &str = "dddddddd-eeee-4fff-8aaa-bbbbbbbbbbbb";

/// A second one, so a request can be wrong in two places at once.
const OTHER_UNKNOWN_ID: &str = "22222222-3333-4444-8555-666666666666";

/// External ids spanning the forms `ExternalId` admits (len 2..128, pattern
/// `^[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+$` — smithy_query query="shape"
/// shape="ExternalId"): the shortest possible one, a word, and one carrying
/// every punctuation character the pattern's middle class allows.
const UNKNOWN_EXTERNAL_IDS: [&str; 3] = ["ab", "no_such_asset", "plant.line-1:boiler_2"];

/// Aliases spanning the forms a `PropertyAlias` takes (len 1.., pattern
/// `^[^\u{0000}-\u{001F}\u{007F}]+$` — smithy_query query="shape"
/// shape="PropertyAlias"): an OPC-UA server data stream path, an OPC-UA node
/// id, and a single-character one.
const UNKNOWN_ALIASES: [&str; 3] = [
    "/company/windfarm/9/turbine/9/no-such",
    "nsu=http://example.com;s=NoSuch",
    "x",
];

/// The sentence a composite model's absence is stated in, built from the labels
/// the request carried.
fn composite_absence(asset_model_label: &str, composite_label: &str) -> String {
    format!("CompositeModel: {composite_label} not found for AssetModel: {asset_model_label}")
}

/// The sentence an asset addressed by the `externalId:` selector is reported
/// absent with: the bare external id, without the selector.
fn external_id_absence(external_id: &str) -> String {
    format!("asset with externalId: {external_id} doesn't exist.")
}

/// The sentence a time series addressed by its alias is reported absent with:
/// the account the alias was resolved in — the one every ARN this emulator mints
/// carries — and the alias from the request.
fn data_alias_absence(alias: &str) -> String {
    format!(
        "dataAlias with accountId AwsAccountId{{value={}}}, \
         alias PropertyAlias(super=PropertyAlias{{value={alias}}}) not found",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The sentence a gateway READ is refused with.
fn gateway_read_absence(gateway_id: &str) -> String {
    format!(
        "No gateway with ID {gateway_id} was found. Please verify that you supplied the correct \
         gateway ID and try again."
    )
}

/// The sentence a gateway UPDATE is refused with.
fn gateway_update_absence(gateway_id: &str) -> String {
    format!(
        "A gateway with ID {gateway_id} could not be found. Please verify you supplied the \
         correct gateway ID and try again."
    )
}

/// Drive one request and assert it is the modelled 404 carrying `expected`.
async fn assert_absence(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected: &str,
) {
    let (status, answered) = call(router, method, uri, body).await;
    assert_eq!(
        status, 404,
        "case {case:?}: an absent resource is a 404, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(&answered)
    );
}

#[tokio::test]
async fn a_composite_model_absence_names_the_asset_model_it_was_looked_for_in() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({
            "assetModelName": "Boiler",
            "assetModelProperties": [],
            "assetModelExternalId": "boiler_model"
        }),
    )
    .await;
    // A composite model that EXISTS, on ANOTHER asset model: a label that
    // resolves elsewhere still names nothing on the model the request scoped it
    // to, which is the whole reason the message carries both ids.
    let other_model_id = create_model(
        &router,
        json!({ "assetModelName": "Pump", "assetModelProperties": [] }),
    )
    .await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (status, created) = create_composite(
        &router,
        &other_model_id,
        json!({
            "assetModelCompositeModelName": "Other",
            "assetModelCompositeModelType": "CUSTOM"
        }),
    )
    .await;
    assert_eq!(status, 202, "create composite: got {status} {created}");
    let other_composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelId in {created}"))
        .to_string();
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    // Rows vary along what the rule generalises over: which operation addresses
    // the composite model, which form each of the two labels takes, and whether
    // the composite-model label resolves to nothing at all or to a composite
    // model of a different asset model.
    let operations: Vec<(&str, &str, Option<Value>)> = vec![
        ("DescribeAssetModelCompositeModel", "GET", None),
        (
            "UpdateAssetModelCompositeModel",
            "PUT",
            Some(json!({ "assetModelCompositeModelName": "Renamed" })),
        ),
        ("DeleteAssetModelCompositeModel", "DELETE", None),
    ];
    let addresses: Vec<(&str, String, String)> = vec![
        (
            "an absent composite model id",
            model_id.clone(),
            UNKNOWN_ID.to_string(),
        ),
        (
            "an absent composite model externalId label",
            model_id.clone(),
            "externalId:no_such_composite".to_string(),
        ),
        (
            "an asset model addressed by its own externalId label",
            "externalId:boiler_model".to_string(),
            OTHER_UNKNOWN_ID.to_string(),
        ),
        (
            "a composite model that lives on another asset model",
            model_id.clone(),
            other_composite_id.clone(),
        ),
    ];

    for (operation, method, body) in &operations {
        for (variation, model_label, composite_label) in &addresses {
            let case = format!("{operation}, {variation}");
            assert_absence(
                &router,
                &case,
                method,
                &format!("/asset-models/{model_label}/composite-models/{composite_label}"),
                body.clone(),
                &composite_absence(model_label, composite_label),
            )
            .await;
        }
    }

    // The create addresses a composite model too — the PARENT a nested one is
    // created inside — so the same absence is stated the same way there.
    for (variation, model_label, parent_label) in &addresses {
        let case = format!("CreateAssetModelCompositeModel, {variation}");
        assert_absence(
            &router,
            &case,
            "POST",
            &format!("/asset-models/{model_label}/composite-models"),
            Some(json!({
                "assetModelCompositeModelName": "Nested",
                "assetModelCompositeModelType": "CUSTOM",
                "parentAssetModelCompositeModelId": parent_label
            })),
            &composite_absence(model_label, parent_label),
        )
        .await;
    }
}

#[tokio::test]
async fn an_asset_addressed_by_its_external_id_is_reported_as_a_missing_external_id() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({ "assetModelName": "Boiler", "assetModelProperties": [] }),
    )
    .await;

    // Rows span the operations that resolve an asset label and every external id
    // in the catalog above, so no row can pass by matching one recorded value.
    let operations: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("DescribeAsset", "GET", "", None),
        (
            "UpdateAsset",
            "PUT",
            "",
            Some(json!({ "assetName": "Whatever" })),
        ),
        ("DeleteAsset", "DELETE", "", None),
        ("ListAssetProperties", "GET", "/properties", None),
        ("DescribeAssetProperty", "GET", "/properties/{property}", None),
        (
            "ListAssetRelationships",
            "GET",
            "/assetRelationships?traversalType=PATH_TO_ROOT",
            None,
        ),
    ];
    for (operation, method, suffix, body) in &operations {
        for external_id in UNKNOWN_EXTERNAL_IDS {
            let case = format!("{operation}, externalId {external_id:?}");
            let suffix = suffix.replace("{property}", OTHER_UNKNOWN_ID);
            assert_absence(
                &router,
                &case,
                method,
                &format!("/assets/externalId:{external_id}{suffix}"),
                body.clone(),
                &external_id_absence(external_id),
            )
            .await;
        }
        // The branch is on the FORM the request addressed the asset by, not on
        // the operation: an id-addressed request keeps the form this service
        // states every other absence in.
        let case = format!("{operation}, a raw id");
        let suffix = suffix.replace("{property}", OTHER_UNKNOWN_ID);
        assert_absence(
            &router,
            &case,
            method,
            &format!("/assets/{UNKNOWN_ID}{suffix}"),
            body.clone(),
            &format!("Asset with ID {UNKNOWN_ID} not found"),
        )
        .await;
    }

    // The same label is answered by the asset holding that external id for as
    // long as one does, so the refusal is about RESOLUTION and not about the
    // shape of the label: create, read back through the selector, delete, and
    // the selector then names nothing.
    let asset_id = create_asset(
        &router,
        json!({
            "assetName": "Boiler1",
            "assetModelId": model_id,
            "assetExternalId": "boiler_1"
        }),
    )
    .await;
    let (status, described) = call(&router, "GET", "/assets/externalId:boiler_1", None).await;
    assert_eq!(
        status, 200,
        "the selector must resolve while the asset holds it, got {status} {described}"
    );
    assert_eq!(
        described["assetId"].as_str(),
        Some(asset_id.as_str()),
        "the selector must name the asset carrying it, got {described}"
    );
    let (status, deleted) = call(&router, "DELETE", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 202, "DeleteAsset: got {status} {deleted}");
    assert_absence(
        &router,
        "DescribeAsset by the externalId of a deleted asset",
        "GET",
        "/assets/externalId:boiler_1",
        None,
        &external_id_absence("boiler_1"),
    )
    .await;
}

#[tokio::test]
async fn a_time_series_addressed_by_an_alias_is_reported_as_an_absent_data_alias() {
    let router = router();
    let model_id = create_model(
        &router,
        json!({
            "assetModelName": "Turbine",
            "assetModelProperties": [
                {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}}
            ]
        }),
    )
    .await;
    let asset_id = create_asset(
        &router,
        json!({ "assetName": "Turbine1", "assetModelId": model_id }),
    )
    .await;
    let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    let property_id = described["assetProperties"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no materialised property in {described}"))
        .to_string();

    // Rows span every operation that resolves a time series by its `alias` and
    // every alias form in the catalog above. `AssociateTimeSeriesToAssetProperty`
    // resolves an asset and a property that both exist here, so the alias is the
    // only thing missing from it.
    for alias in UNKNOWN_ALIASES {
        let rows: Vec<(&str, &str, String)> = vec![
            (
                "DescribeTimeSeries",
                "GET",
                format!("/timeseries/describe?alias={alias}"),
            ),
            (
                "DeleteTimeSeries",
                "POST",
                format!("/timeseries/delete?alias={alias}"),
            ),
            (
                "AssociateTimeSeriesToAssetProperty",
                "POST",
                format!(
                    "/timeseries/associate?alias={alias}&assetId={asset_id}\
                     &propertyId={property_id}"
                ),
            ),
        ];
        for (operation, method, uri) in &rows {
            let case = format!("{operation}, alias {alias:?}");
            assert_absence(&router, &case, method, uri, None, &data_alias_absence(alias)).await;
        }
    }

    // A stream the alias really stands under answers both the read and the
    // delete, and only once it is gone does the alias draw the refusal — from
    // the read and from the repeated delete alike.
    let alias = "/company/windfarm/3/turbine/7/temperature";
    ingest_by_alias(&router, alias, json!({"doubleValue": 72.5}), now_seconds()).await;
    let (status, described) = call(
        &router,
        "GET",
        &format!("/timeseries/describe?alias={alias}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the alias must resolve while the stream stands under it, got {status} {described}"
    );
    let (status, deleted) = call(
        &router,
        "POST",
        &format!("/timeseries/delete?alias={alias}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteTimeSeries: got {status} {deleted}");
    for (case, method) in [
        ("DescribeTimeSeries after the delete", "GET"),
        ("DeleteTimeSeries after the delete", "POST"),
    ] {
        let uri = if method == "GET" {
            format!("/timeseries/describe?alias={alias}")
        } else {
            format!("/timeseries/delete?alias={alias}")
        };
        assert_absence(&router, case, method, &uri, None, &data_alias_absence(alias)).await;
    }
}

#[tokio::test]
async fn the_gateway_read_and_the_gateway_update_carry_their_own_sentences() {
    let router = router();
    // A gateway that was really there and then deleted, beside ids nothing was
    // ever stored under: the refusal is about what the store holds now.
    let deleted_id = create_gateway(&router, "edge-1").await;
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("{GATEWAYS}/{deleted_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteGateway: got {status} {deleted}");

    let unknown = [
        ("a deleted gateway", deleted_id.as_str()),
        ("an id nothing was stored under", UNKNOWN_ID),
        ("a second such id", OTHER_UNKNOWN_ID),
    ];
    for (variation, gateway_id) in unknown {
        assert_absence(
            &router,
            &format!("DescribeGateway, {variation}"),
            "GET",
            &format!("{GATEWAYS}/{gateway_id}"),
            None,
            &gateway_read_absence(gateway_id),
        )
        .await;
        assert_absence(
            &router,
            &format!("UpdateGateway, {variation}"),
            "PUT",
            &format!("{GATEWAYS}/{gateway_id}"),
            Some(json!({ "gatewayName": "renamed" })),
            &gateway_update_absence(gateway_id),
        )
        .await;
        assert_ne!(
            gateway_read_absence(gateway_id),
            gateway_update_absence(gateway_id),
            "case {variation:?}: the read and the update are two distinct sentences"
        );
    }
}
