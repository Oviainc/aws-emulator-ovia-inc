// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an asset inherits through a composite model that REUSES
//! a component model, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! The rule under test: a composite model created with a `composedAssetModelId`
//! contributes the properties of the component model it names as that model holds
//! them NOW, so an `UpdateAssetModel` of a component model reaches the assets of
//! EVERY asset model reusing it — "Updates an asset model and all of the assets that
//! were created from the model. Each asset created from the model inherits the
//! updated asset model's property and hierarchy definitions", and "If you remove a
//! property from an asset model, IoT SiteWise deletes all previous data for that
//! property" (`UpdateAssetModel`).
//!
//! The rows differ along the EDIT the component model takes — a definition added, an
//! existing definition's unit changed, a definition removed — and each row is
//! asserted on the assets of TWO models reusing the same component model, through
//! both views of an asset's properties (`ListAssetProperties` and
//! `DescribeAssetProperty`), while the asset of a model that reuses nothing is
//! untouched by any of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the asset/model fixtures this crate's other suites already
// have: one router, one `call`, one `property_catalog`.
use super::asset_lifecycle::{
    call, create_asset, error_code, property_catalog, property_id, router,
};
// The model-side helpers the composite-model suite already drives these operations
// through, so this suite grows no second copy of them.
use super::describe_asset_property::{
    add_composite_model, create_typed_model, describe_model_composite, describe_property,
};

/// The code IoT SiteWise refuses a label that names no stored property with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One asset model that reuses the component model, with the asset created from it.
struct Host {
    /// What tells this host apart in an assertion message.
    case: &'static str,
    /// The host model's `assetModelId`.
    model_id: String,
    /// The `assetModelCompositeModelId` of the composite model reusing the component.
    composite_id: String,
    /// The asset created from the host model.
    asset_id: String,
}

/// The property definitions `model_id` carries right now, in the form
/// `UpdateAssetModel` takes them back: each one keeps the id the create minted, so an
/// update that re-sends them edits the definitions rather than replacing them.
async fn definitions(router: &MultiServiceRouter, model_id: &str) -> Vec<Value> {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel {model_id}: got {status} {body}"
    );
    body["assetModelProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelProperties in {body}"))
        .clone()
}

/// `UpdateAssetModel` on `model_id` with `properties` as its definitions.
async fn update_definitions(
    case: &str,
    router: &MultiServiceRouter,
    model_id: &str,
    name: &str,
    properties: &[Value],
) {
    let (status, body) = call(
        router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(json!({
            "assetModelName": name,
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: UpdateAssetModel {model_id}: got {status} {body}"
    );
}

/// Every property `ListAssetProperties` reports for `asset_id`, as
/// `(propertyId, assetCompositeModelId)`.
async fn listed_properties(
    case: &str,
    router: &MultiServiceRouter,
    asset_id: &str,
) -> Vec<(String, Option<String>)> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAssetProperties {asset_id}: got {status} {body}"
    );
    body["assetPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertySummaries in {body}"))
        .iter()
        .map(|summary| {
            (
                summary["id"].as_str().unwrap_or_default().to_string(),
                summary
                    .get("assetCompositeModelId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// The id the component model's property `name` is contributed to `host` under, read
/// from the service's own view of that composite model.
async fn contributed_id(router: &MultiServiceRouter, host: &Host, name: &str) -> String {
    let described = describe_model_composite(router, &host.model_id, &host.composite_id).await;
    property_id(&described["assetModelCompositeModelProperties"], name)
}

#[tokio::test]
async fn a_component_model_edit_reaches_every_asset_of_every_model_reusing_it() {
    let router = router();

    // The component model two host models reuse. `Speed` is the definition a row
    // below edits, `Revolutions` the one a row removes.
    let component = create_typed_model(
        &router,
        "GearboxComponent",
        "COMPONENT_MODEL",
        json!([
            {"name": "Speed", "dataType": "DOUBLE", "unit": "m/s", "type": {"measurement": {}}},
            {"name": "Revolutions", "dataType": "INTEGER", "type": {"measurement": {}}},
        ]),
    )
    .await;

    // Two hosts, so what the propagation must reach is every model reusing the
    // component model rather than the one that happens to be asked about first.
    let mut hosts: Vec<Host> = Vec::new();
    for (case, name) in [
        ("the first host model reusing the component", "FirstGearboxHost"),
        ("the second host model reusing the component", "SecondGearboxHost"),
    ] {
        let model_id = create_typed_model(&router, name, "ASSET_MODEL", property_catalog()).await;
        let composite_id = add_composite_model(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": "Gearbox",
                "assetModelCompositeModelType": "CUSTOM",
                "composedAssetModelId": &component,
            }),
        )
        .await;
        let asset_id = create_asset(&router, &format!("{name}-1"), &model_id).await;
        hosts.push(Host {
            case,
            model_id,
            composite_id,
            asset_id,
        });
    }

    // A model that reuses nothing: none of the edits below may touch its asset.
    let standalone_model =
        create_typed_model(&router, "StandaloneBoiler", "ASSET_MODEL", property_catalog()).await;
    let standalone_asset = create_asset(&router, "StandaloneBoiler-1", &standalone_model).await;
    let standalone_before =
        listed_properties("the standalone asset", &router, &standalone_asset).await;

    // The ids the removal row needs, read while the definition is still there.
    let mut revolutions: Vec<(usize, String)> = Vec::new();
    for (at, host) in hosts.iter().enumerate() {
        revolutions.push((at, contributed_id(&router, host, "Revolutions").await));
    }

    // Row 1: a definition ADDED to the component model is contributed to every host,
    // and so carried by every asset of one.
    let case = "a definition added to the component model";
    let mut edited = definitions(&router, &component).await;
    edited.push(json!({
        "name": "Torque", "dataType": "DOUBLE", "unit": "Nm", "type": {"measurement": {}},
    }));
    update_definitions(case, &router, &component, "GearboxComponent", &edited).await;

    for host in &hosts {
        let host_case = format!("{case}, on {}", host.case);
        let torque = contributed_id(&router, host, "Torque").await;
        let (status, body) = describe_property(&router, &host.asset_id, &torque).await;
        assert_eq!(
            status, 200,
            "case {host_case:?}: expected the added property described, got {status} {body}"
        );
        assert_eq!(
            body["compositeModel"]["id"].as_str(),
            Some(host.composite_id.as_str()),
            "case {host_case:?}: expected it reported under the reusing composite model, got \
             {body}"
        );
        assert_eq!(
            body["compositeModel"]["assetProperty"]["unit"].as_str(),
            Some("Nm"),
            "case {host_case:?}: expected the added definition's unit Nm, got {body}"
        );
        let listed = listed_properties(&host_case, &router, &host.asset_id).await;
        assert!(
            listed.contains(&(torque.clone(), Some(host.composite_id.clone()))),
            "case {host_case:?}: expected {torque} listed under {composite}, got {listed:?}",
            composite = host.composite_id
        );
    }

    // Row 2: an existing definition CHANGED on the component model shows through on
    // the assets, which take the definition's unit as it stands now.
    let case = "an existing definition's unit changed";
    let mut edited = definitions(&router, &component).await;
    for definition in &mut edited {
        if definition["name"] == "Speed" {
            definition["unit"] = json!("km/h");
        }
    }
    update_definitions(case, &router, &component, "GearboxComponent", &edited).await;

    for host in &hosts {
        let host_case = format!("{case}, on {}", host.case);
        let speed = contributed_id(&router, host, "Speed").await;
        let (status, body) = describe_property(&router, &host.asset_id, &speed).await;
        assert_eq!(
            status, 200,
            "case {host_case:?}: expected the edited property described, got {status} {body}"
        );
        assert_eq!(
            body["compositeModel"]["assetProperty"]["unit"].as_str(),
            Some("km/h"),
            "case {host_case:?}: expected the edited unit km/h, got {body}"
        );
    }

    // Row 3: a definition REMOVED from the component model is no longer contributed,
    // so the assets of every host stop carrying it and stop resolving its id.
    let case = "a definition removed from the component model";
    let edited: Vec<Value> = definitions(&router, &component)
        .await
        .into_iter()
        .filter(|definition| definition["name"] != "Revolutions")
        .collect();
    update_definitions(case, &router, &component, "GearboxComponent", &edited).await;

    for (at, property_id) in &revolutions {
        let host = &hosts[*at];
        let host_case = format!("{case}, on {}", host.case);
        let (status, body) = describe_property(&router, &host.asset_id, property_id).await;
        assert_eq!(
            status, 404,
            "case {host_case:?}: expected the removed property refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {host_case:?}: expected the code {NOT_FOUND}, got {body}"
        );
        let listed = listed_properties(&host_case, &router, &host.asset_id).await;
        assert!(
            !listed.iter().any(|(listed_id, _)| listed_id == property_id),
            "case {host_case:?}: expected {property_id} gone from the asset, got {listed:?}"
        );
    }

    // None of the three edits touched the asset of the model that reuses nothing.
    let case = "the asset of a model that reuses no component model";
    let standalone_after = listed_properties(case, &router, &standalone_asset).await;
    assert_eq!(
        standalone_after, standalone_before,
        "case {case:?}: expected its properties unchanged, got {standalone_after:?}"
    );
}
