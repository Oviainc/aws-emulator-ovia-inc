// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an `UpdateAssetModel` that REWRITES a model's
//! definition does to the interface relationships that model stands in, driven
//! over HTTP through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule this suite pins: a stored mapping pairs one member of the interface
//! asset model with one member of the model the interface is applied to, so it
//! survives only while both models still declare their half of it — an update
//! that drops a mapped property or hierarchy drops the mappings that named it,
//! on whichever side of the relationship the updated model stands, and leaves
//! every other mapping alone. Every mapping the read then reports resolves to a
//! property the model still declares, which `ListAssetModelProperties` is asked
//! for directly.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, router};
use super::asset_model_interface_relationship::{
    create_model, describe_model, id_of, ids, model_with, property, relationship_uri,
    PAST_THE_OWED_TRANSITION,
};

/// Which side of the relationship the row's `UpdateAssetModel` targets: the
/// model the interface is applied to, or the interface asset model itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    AppliedTo,
    Interface,
}

/// One row of the table: two models, the interface applied to one of them, and
/// one `UpdateAssetModel` that restates the definition of one of the two.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// The properties and hierarchies the INTERFACE asset model declares.
    interface_properties: &'static [&'static str],
    interface_hierarchies: &'static [&'static str],
    /// The properties and hierarchies the applied-to asset model declares. It
    /// holds every name the interface declares, since the application below
    /// matches by property name and creates nothing.
    model_properties: &'static [&'static str],
    model_hierarchies: &'static [&'static str],
    /// The model the update restates, and the members it keeps: a member the
    /// update does not name is gone, which is how a caller removes one.
    updated: Side,
    keep_properties: &'static [&'static str],
    keep_hierarchies: &'static [&'static str],
    /// The interface property / hierarchy names whose mapping must survive the
    /// update, in the interface's own declaration order.
    expected_property_mappings: &'static [&'static str],
    expected_hierarchy_mappings: &'static [&'static str],
}

/// The table: one row per way the update can move a mapped member — the kind of
/// member dropped (property, hierarchy), the side of the relationship the
/// updated model stands on, how many mappings the drop takes with it, and the
/// two rows where nothing mapped is dropped at all.
const ROWS: &[Row] = &[
    Row {
        case: "the applied-to model drops one of two mapped properties",
        interface_properties: &["Temperature", "Pressure"],
        interface_hierarchies: &[],
        model_properties: &["Temperature", "Pressure", "Serial"],
        model_hierarchies: &[],
        updated: Side::AppliedTo,
        keep_properties: &["Temperature", "Serial"],
        keep_hierarchies: &[],
        expected_property_mappings: &["Temperature"],
        expected_hierarchy_mappings: &[],
    },
    Row {
        case: "the applied-to model drops every mapped property",
        interface_properties: &["Temperature", "Pressure"],
        interface_hierarchies: &[],
        model_properties: &["Temperature", "Pressure", "Serial"],
        model_hierarchies: &[],
        updated: Side::AppliedTo,
        keep_properties: &["Serial"],
        keep_hierarchies: &[],
        expected_property_mappings: &[],
        expected_hierarchy_mappings: &[],
    },
    Row {
        case: "the applied-to model drops the mapped hierarchy and keeps its properties",
        interface_properties: &["Temperature"],
        interface_hierarchies: &["Sensors"],
        model_properties: &["Temperature", "Serial"],
        model_hierarchies: &["Sensors", "Valves"],
        updated: Side::AppliedTo,
        keep_properties: &["Temperature", "Serial"],
        keep_hierarchies: &["Valves"],
        expected_property_mappings: &["Temperature"],
        expected_hierarchy_mappings: &[],
    },
    Row {
        case: "the interface drops one of the two properties it declared",
        interface_properties: &["Temperature", "Pressure"],
        interface_hierarchies: &[],
        model_properties: &["Temperature", "Pressure"],
        model_hierarchies: &[],
        updated: Side::Interface,
        keep_properties: &["Temperature"],
        keep_hierarchies: &[],
        expected_property_mappings: &["Temperature"],
        expected_hierarchy_mappings: &[],
    },
    Row {
        case: "the interface drops the hierarchy both models declared",
        interface_properties: &["Temperature"],
        interface_hierarchies: &["Sensors"],
        model_properties: &["Temperature"],
        model_hierarchies: &["Sensors"],
        updated: Side::Interface,
        keep_properties: &["Temperature"],
        keep_hierarchies: &[],
        expected_property_mappings: &["Temperature"],
        expected_hierarchy_mappings: &[],
    },
    Row {
        case: "an update that drops only an UNMAPPED property keeps every mapping",
        interface_properties: &["Temperature", "Pressure"],
        interface_hierarchies: &["Sensors"],
        model_properties: &["Temperature", "Pressure", "Serial"],
        model_hierarchies: &["Sensors"],
        updated: Side::AppliedTo,
        keep_properties: &["Temperature", "Pressure"],
        keep_hierarchies: &["Sensors"],
        expected_property_mappings: &["Temperature", "Pressure"],
        expected_hierarchy_mappings: &["Sensors"],
    },
    Row {
        case: "an update that restates every mapped member keeps every mapping",
        interface_properties: &["Temperature", "Pressure"],
        interface_hierarchies: &["Sensors"],
        model_properties: &["Temperature", "Pressure", "Serial"],
        model_hierarchies: &["Sensors", "Valves"],
        updated: Side::AppliedTo,
        keep_properties: &["Temperature", "Pressure", "Serial"],
        keep_hierarchies: &["Sensors", "Valves"],
        expected_property_mappings: &["Temperature", "Pressure"],
        expected_hierarchy_mappings: &["Sensors"],
    },
];

/// Create an asset model of `model_type` declaring one measurement property per
/// name in `properties` and one hierarchy per name in `hierarchies`, each
/// hierarchy accepting children of `child`.
async fn model_with_hierarchies(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    model_type: &str,
    properties: &[&str],
    hierarchies: &[&str],
    child: &str,
) -> String {
    create_model(
        router,
        case,
        json!({
            "assetModelName": name,
            "assetModelType": model_type,
            "assetModelProperties": properties
                .iter()
                .map(|name| property(name))
                .collect::<Vec<Value>>(),
            "assetModelHierarchies": hierarchies
                .iter()
                .map(|name| json!({"name": name, "childAssetModelId": child}))
                .collect::<Vec<Value>>(),
        }),
    )
    .await
}

/// Every property id `ListAssetModelProperties` reports for `asset_model`.
async fn listed_property_ids(
    router: &MultiServiceRouter,
    case: &str,
    asset_model: &str,
) -> BTreeSet<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/asset-models/{asset_model}/properties"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAssetModelProperties got {status} {body:?}"
    );
    ids(&body["assetModelPropertySummaries"])
}

#[tokio::test]
async fn an_update_drops_the_interface_mappings_that_named_what_it_removed() {
    for row in ROWS {
        let case = row.case;
        let router = router();
        let child = model_with(&router, case, "Child", "ASSET_MODEL", &["Temperature"]).await;
        let interface = model_with_hierarchies(
            &router,
            case,
            "Interface",
            "INTERFACE",
            row.interface_properties,
            row.interface_hierarchies,
            &child,
        )
        .await;
        let asset_model = model_with_hierarchies(
            &router,
            case,
            "AppliedTo",
            "ASSET_MODEL",
            row.model_properties,
            row.model_hierarchies,
            &child,
        )
        .await;

        // Applied by NAME, so each interface property maps to the applied-to
        // model's property of the same name and each shared hierarchy name is
        // paired as well.
        let (status, applied) = call(
            &router,
            "PUT",
            &relationship_uri(&asset_model, &interface),
            Some(json!({"propertyMappingConfiguration": {"matchByPropertyName": true}})),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the application got {status} {applied:?}"
        );

        // The definition the update restates, and the ids of the members it is
        // about to drop — the ids a stale mapping would keep naming.
        let updated_id = match row.updated {
            Side::AppliedTo => asset_model.clone(),
            Side::Interface => interface.clone(),
        };
        let updated_name = match row.updated {
            Side::AppliedTo => "AppliedTo",
            Side::Interface => "Interface",
        };
        let before = describe_model(&router, case, &updated_id).await;
        let kept_properties: Vec<Value> = row
            .keep_properties
            .iter()
            .map(|name| {
                let mut kept = property(name);
                kept["id"] = json!(id_of(&before["assetModelProperties"], case, name));
                kept
            })
            .collect();
        let kept_hierarchies: Vec<Value> = row
            .keep_hierarchies
            .iter()
            .map(|name| {
                json!({
                    // A hierarchy carries `id` and `name` just as a property
                    // does, so the one lookup answers for both lists.
                    "id": id_of(&before["assetModelHierarchies"], case, name),
                    "name": name,
                    "childAssetModelId": child,
                })
            })
            .collect();
        let dropped_property_ids: BTreeSet<String> = ids(&before["assetModelProperties"])
            .into_iter()
            .filter(|id| {
                !kept_properties
                    .iter()
                    .any(|kept| kept["id"] == json!(id.as_str()))
            })
            .collect();

        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/asset-models/{updated_id}"),
            Some(json!({
                "assetModelName": updated_name,
                "assetModelProperties": kept_properties,
                "assetModelHierarchies": kept_hierarchies,
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: UpdateAssetModel got {status} {updated:?}"
        );
        // Past the transition the update owes, so both definitions read back as
        // the ones the update left.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        let interface_model = describe_model(&router, case, &interface).await;
        let applied_model = describe_model(&router, case, &asset_model).await;
        let expected_property_mappings: Vec<Value> = row
            .expected_property_mappings
            .iter()
            .map(|name| {
                json!({
                    "assetModelPropertyId":
                        id_of(&applied_model["assetModelProperties"], case, name),
                    "interfaceAssetModelPropertyId":
                        id_of(&interface_model["assetModelProperties"], case, name),
                })
            })
            .collect();
        let expected_hierarchy_mappings: Vec<Value> = row
            .expected_hierarchy_mappings
            .iter()
            .map(|name| {
                json!({
                    "assetModelHierarchyId":
                        id_of(&applied_model["assetModelHierarchies"], case, name),
                    "interfaceAssetModelHierarchyId":
                        id_of(&interface_model["assetModelHierarchies"], case, name),
                })
            })
            .collect();

        let (status, read) = call(
            &router,
            "GET",
            &relationship_uri(&asset_model, &interface),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModelInterfaceRelationship got {status} {read:?}"
        );
        assert_eq!(
            read["propertyMappings"],
            json!(expected_property_mappings),
            "case {case:?}: expected the property mappings {expected_property_mappings:?}, got {:?}",
            read["propertyMappings"]
        );
        assert_eq!(
            read["hierarchyMappings"],
            json!(expected_hierarchy_mappings),
            "case {case:?}: expected the hierarchy mappings {expected_hierarchy_mappings:?}, got {:?}",
            read["hierarchyMappings"]
        );

        // Every mapping the read reports resolves on BOTH sides: the property
        // ids it names are ones the two models still declare, as the property
        // listing of each model answers for itself.
        let listed_model = listed_property_ids(&router, case, &asset_model).await;
        let listed_interface = listed_property_ids(&router, case, &interface).await;
        for mapping in read["propertyMappings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a mapping list, got {read:?}"))
        {
            let target = mapping["assetModelPropertyId"].as_str().unwrap_or_default();
            let source = mapping["interfaceAssetModelPropertyId"]
                .as_str()
                .unwrap_or_default();
            assert!(
                listed_model.contains(target),
                "case {case:?}: the mapping {mapping:?} names {target}, which the applied-to \
                 model no longer declares: {listed_model:?}"
            );
            assert!(
                listed_interface.contains(source),
                "case {case:?}: the mapping {mapping:?} names {source}, which the interface \
                 asset model no longer declares: {listed_interface:?}"
            );
        }

        // And the properties the update dropped are gone for good: neither the
        // listing nor any mapping names them any more.
        let listed_updated = match row.updated {
            Side::AppliedTo => &listed_model,
            Side::Interface => &listed_interface,
        };
        for dropped in &dropped_property_ids {
            assert!(
                !listed_updated.contains(dropped),
                "case {case:?}: the dropped property {dropped} is still listed on \
                 {updated_id}: {listed_updated:?}"
            );
            assert!(
                !read["propertyMappings"].to_string().contains(dropped),
                "case {case:?}: a mapping still names the dropped property {dropped}: {:?}",
                read["propertyMappings"]
            );
        }
    }
}
