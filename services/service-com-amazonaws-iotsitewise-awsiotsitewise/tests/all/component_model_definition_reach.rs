// SPDX-License-Identifier: CC0-1.0

//! What a RENAME inside a reused `COMPONENT_MODEL` does to the assets that
//! already instantiate the models reusing it, end to end over HTTP through the
//! same `MultiServiceRouter` the emulator boots with.
//!
//! The sibling suite `component_model_definition_propagation` pins what a
//! definition ADDED to or REMOVED from a reused component model does; this one
//! is the third kind of write a caller makes to such a model and the one neither
//! adds nor removes anything: a definition KEPT, under a new name and a new
//! unit. Because "You can't change the type or data type of an existing
//! property", the name and the unit are what an update may change, and because
//! an update is matched to the property it rewrites by the id that property
//! carries, the asset's property must report the new name and unit under the
//! SAME id — the stored definition is read through, not copied.
//!
//! The rows are the dimension the sibling suite holds fixed: whether the update
//! lands after the transition the call before it owed has fallen due, or while
//! the component model is still settling it, with the assets read straight
//! after. What a composite model contributes is read from the component model's
//! stored record, so no settling state decides what an asset carries.
//!
//! The statuses written out here come from
//! `smithy_query(query="operation", operation="UpdateAssetModel,DescribeAssetProperty")`
//! on this crate's `smithy-model/`: `UpdateAssetModel` answers 202 and
//! `DescribeAssetProperty` 200.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, create_asset, property_id, router};
use crate::asset_model_composite_model::{create_composite, PAST_THE_OWED_TRANSITION};

/// One property of the component model before the rename, paired with what the
/// rename makes of it. The pair differs along the members a rename may touch:
/// one definition changes both its name and its unit, the other takes a unit it
/// did not have.
struct Rename {
    before: &'static str,
    after: &'static str,
    unit_before: Option<&'static str>,
    unit_after: &'static str,
}

/// The two definitions every row's component model carries, and their renames.
fn renames() -> [Rename; 2] {
    [
        Rename {
            before: "Flow",
            after: "Throughput",
            unit_before: Some("m3/s"),
            unit_after: "l/s",
        },
        Rename {
            before: "Torque",
            after: "Torsion",
            unit_before: None,
            unit_after: "N.m",
        },
    ]
}

/// One row: whether the rename lands after the transition the create before it
/// owed has fallen due, or while the component model is still settling it.
struct Row {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    settled: bool,
}

/// Create a `COMPONENT_MODEL` carrying the definitions the renames start from,
/// and return its id.
async fn component_model(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let properties: Vec<Value> = renames()
        .iter()
        .map(|rename| {
            let mut definition = json!({
                "name": rename.before,
                "dataType": "DOUBLE",
                "type": {"measurement": {}},
            });
            if let Some(unit) = rename.unit_before {
                definition["unit"] = json!(unit);
            }
            definition
        })
        .collect();
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModel({name}) got {status} {created}"
    );
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
        .to_string()
}

/// An asset model with one base property of its own and a composite model
/// reusing `component_id`, plus an asset of it. Answers
/// `(assetModelId, compositeModelId, assetId)`.
async fn host_with_asset(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    component_id: &str,
) -> (String, String, String) {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelProperties": [
                {"name": "Uptime", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModel({name}) got {status} {created}"
    );
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
        .to_string();

    let (status, created) = create_composite(
        router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": component_id,
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModelCompositeModel on {name} got {status} {created}"
    );
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no composite model id in {created}"))
        .to_string();

    let asset_id = create_asset(router, &format!("{name}-1"), &model_id).await;
    (model_id, composite_id, asset_id)
}

/// What `DescribeAssetModelCompositeModel` reports composite model
/// `composite_id` of `model_id` contributes.
async fn contributed(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    composite_id: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/asset-models/{model_id}/composite-models/{composite_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAssetModelCompositeModel got {status} {body}"
    );
    body["assetModelCompositeModelProperties"].clone()
}

/// `GET /assets/{asset_id}/properties/{property_id}`.
async fn describe_property(
    router: &MultiServiceRouter,
    case: &str,
    asset_id: &str,
    property: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties/{property}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeAssetProperty {property} got {status} {body}"
    );
    body
}

/// The `(id, name, unit)` of every property `ListAssetProperties` reports for an
/// asset, over every property it carries (`filter=ALL`). The summaries carry no
/// name, so the name comes from the `path` segment that addresses the property.
async fn listed_properties(
    router: &MultiServiceRouter,
    case: &str,
    asset_id: &str,
) -> Vec<(String, String, String)> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAssetProperties got {status} {body}"
    );
    body["assetPropertySummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            let path = summary["path"].as_array().cloned().unwrap_or_default();
            let name = path
                .last()
                .and_then(|segment| segment["name"].as_str())
                .unwrap_or_default()
                .to_string();
            (
                summary["id"].as_str().unwrap_or_default().to_string(),
                name,
                summary["unit"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE: a definition RENAMED in a reused `COMPONENT_MODEL` — sent back with the
/// id it carries, so it is the same definition under a new name and unit — is
/// reported under that new name and unit, by that same property id, on every
/// asset whose model reuses it, whether or not the write that renamed it has
/// settled; and the asset's own properties are untouched by it.
#[tokio::test]
async fn a_rename_inside_a_reused_component_model_reaches_the_assets_under_the_same_ids() {
    let rows = [
        Row {
            case: "the rename lands after the owed transition has fallen due",
            settled: true,
        },
        Row {
            case: "the rename lands while the component model is still settling",
            settled: false,
        },
    ];

    for Row { case, settled } in rows {
        let router = router();
        let component_name = "Pump";
        let component_id = component_model(&router, case, component_name).await;
        // TWO asset models reuse the one component model, so the rename has to
        // reach the assets of every model reusing it rather than of one of them.
        let hosts = [
            host_with_asset(&router, case, "Plant0", &component_id).await,
            host_with_asset(&router, case, "Plant1", &component_id).await,
        ];

        // The ids the composite models contribute BEFORE the rename, with the
        // asset's own property beside them: the rename must keep the former and
        // leave the latter alone.
        let mut before: Vec<(String, Vec<String>, String)> = Vec::new();
        for (model_id, composite_id, asset_id) in &hosts {
            let listed = contributed(&router, case, model_id, composite_id).await;
            let ids: Vec<String> = renames()
                .iter()
                .map(|rename| property_id(&listed, rename.before))
                .collect();
            let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
            assert_eq!(status, 200, "case {case:?}: DescribeAsset got {status} {asset}");
            let base = property_id(&asset["assetProperties"], "Uptime");
            before.push((asset_id.clone(), ids, base));
        }

        if settled {
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        }

        // The rename: every stored definition sent back as it was described, with
        // its id, carrying the new name and unit.
        let (status, described) = call(
            &router,
            "GET",
            &format!("/asset-models/{component_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModel got {status} {described}"
        );
        let mut definitions = described["assetModelProperties"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no assetModelProperties in {described}"));
        for definition in &mut definitions {
            for rename in renames() {
                if definition["name"] == json!(rename.before) {
                    definition["name"] = json!(rename.after);
                    definition["unit"] = json!(rename.unit_after);
                }
            }
        }
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/asset-models/{component_id}"),
            Some(json!({
                "assetModelName": component_name,
                "assetModelType": "COMPONENT_MODEL",
                "assetModelProperties": definitions,
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: UpdateAssetModel got {status} {updated}"
        );
        if settled {
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        }

        for ((model_id, composite_id, _), (asset_id, ids, base)) in hosts.iter().zip(&before) {
            let listed = contributed(&router, case, model_id, composite_id).await;
            let summaries = listed_properties(&router, case, asset_id).await;
            for (rename, id) in renames().iter().zip(ids) {
                let after = rename.after;
                // The model-side view: the same definition, renamed.
                assert_eq!(
                    property_id(&listed, after),
                    *id,
                    "case {case:?}: renaming {:?} to {after:?} must keep the definition's id, \
                     got {listed}",
                    rename.before
                );
                // The asset-side views: the same property id, under the new name
                // and unit, still reported by the composite model that reuses the
                // component model.
                let body = describe_property(&router, case, asset_id, id).await;
                assert_eq!(
                    body["compositeModel"]["id"].as_str(),
                    Some(composite_id.as_str()),
                    "case {case:?}: {after} must stay under the composite model that reuses \
                     the component model, got {body}"
                );
                assert_eq!(
                    body["compositeModel"]["assetProperty"]["name"].as_str(),
                    Some(after),
                    "case {case:?}: expected the renamed name {after:?}, got {body}"
                );
                assert_eq!(
                    body["compositeModel"]["assetProperty"]["unit"].as_str(),
                    Some(rename.unit_after),
                    "case {case:?}: expected the renamed unit {:?}, got {body}",
                    rename.unit_after
                );
                assert!(
                    summaries.contains(&(
                        id.clone(),
                        after.to_string(),
                        rename.unit_after.to_string()
                    )),
                    "case {case:?}: ListAssetProperties and DescribeAssetProperty must agree \
                     on the renamed property ({id}, {after}, {}), got {summaries:?}",
                    rename.unit_after
                );
            }
            // The rename changed what the composite model contributes and nothing
            // else the asset carries.
            assert!(
                summaries.iter().any(|(id, _, _)| id == base),
                "case {case:?}: the asset's own property {base} must survive the rename, \
                 got {summaries:?}"
            );
        }
    }
}
