// SPDX-License-Identifier: CC0-1.0

//! The component types a workspace is PROVISIONED with, end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The RULE under test: the component types the service itself ships are not
//! created through this API — a workspace holds them from the instant
//! `CreateWorkspace` returns. So each of them reads back through
//! `GetComponentType` and is reported by `ListComponentTypes` with no create of
//! its own and no wait, carries the abstractness and the property definitions
//! the service publishes for it (which a component type extending it really
//! inherits), and belongs to the SERVICE: its id lies in a reserved namespace no
//! caller can mint in, no caller's update or delete applies to it, and it is not
//! part of the contents a workspace has to be emptied of before it is deleted.
//!
//! The rows span the whole catalogue the service seeds — abstract and concrete
//! entries, one, two and three property definitions, scalar and container data
//! types — crossed with the workspaces it is seeded into; the per-type
//! expectations are read off that catalogue rather than restated, while the ids
//! the AWS IoT TwinMaker user guide publishes are pinned as literals so dropping
//! one is a failure.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    built_in_component_types, ComponentType, BUILT_IN_COMPONENT_TYPE_ID_PREFIX, STATE_ACTIVE,
};

use crate::harness::{
    built_in_component_type_ids, component_type_arn, with_built_in_component_types, Emulator,
    Resp, REGION,
};

/// The ids of the built-in component types the AWS IoT TwinMaker user guide
/// publishes, pinned here as literals: the catalogue the emulator seeds must
/// hold each of them, whatever else it holds.
const PUBLISHED_BUILT_IN_IDS: [&str; 3] = [
    "com.amazon.iotsitewise.connector",
    "com.amazon.iottwinmaker.alarm.basic",
    "com.amazon.iottwinmaker.documents",
];

/// The catalogue `CreateWorkspace` seeds a workspace with, as the records it
/// stores: the SOURCE of every per-type expectation below, so a row is the
/// service's own declaration of a built-in rather than a second copy of it.
fn catalogue(workspace_id: &str) -> BTreeMap<String, ComponentType> {
    built_in_component_types(REGION, workspace_id, std::time::SystemTime::now())
}

/// Create a workspace, asserting it worked, and answer the response (the
/// provisioning instant lives in it).
async fn create_workspace(emulator: &Emulator, workspace_id: &str) -> Resp {
    let created = emulator.create_workspace(workspace_id).await;
    assert_eq!(
        created.status, 200,
        "creating workspace {workspace_id:?} must succeed, got {created:?}"
    );
    created
}

/// RULE (provisioning half): every workspace holds the service's built-in
/// component types from the instant it is created — each reads back with the
/// identity, abstractness, settled status and property definitions the service
/// publishes for it, scoped to the workspace that holds it.
///
/// The rows are the whole catalogue crossed with workspaces whose ids differ in
/// shape and whose contents differ (one empty, one a caller created a component
/// type in), and every read is taken WITHOUT waiting: a provisioned component
/// type has no create in flight for a settle span to end.
#[tokio::test]
async fn every_workspace_is_provisioned_with_the_services_built_in_component_types() {
    let emulator = Emulator::new();

    let ids = built_in_component_type_ids();
    for published in PUBLISHED_BUILT_IN_IDS {
        assert!(
            ids.iter().any(|id| id == published),
            "case {published:?}: the catalogue must hold every built-in the user guide \
             publishes, got {ids:?}"
        );
    }

    // (the workspace, what else the caller created in it)
    let workspaces: Vec<(&str, Vec<&str>)> = vec![
        ("builtInWs", Vec::new()),
        ("built_in-ws2", Vec::new()),
        ("W0", vec!["com.example.own", "plainType"]),
    ];

    for (workspace_id, own) in &workspaces {
        let created = create_workspace(&emulator, workspace_id).await;
        let provisioned_at = created.member("creationDateTime").as_f64();
        for component_type_id in own {
            let created_own = emulator
                .create_component_type(workspace_id, component_type_id, json!({}))
                .await;
            assert_eq!(
                created_own.status, 200,
                "case {component_type_id:?}: the caller's own create must succeed, \
                 got {created_own:?}"
            );
        }

        for (component_type_id, record) in catalogue(workspace_id) {
            let case = format!("{workspace_id}/{component_type_id}");
            let read = emulator
                .get_component_type(workspace_id, &component_type_id)
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: a provisioned component type must read back with no create of \
                 its own, got {read:?}"
            );
            assert_eq!(
                (
                    read.member("workspaceId").as_str(),
                    read.member("componentTypeId").as_str(),
                    read.member("arn").as_str(),
                ),
                (
                    Some(*workspace_id),
                    Some(component_type_id.as_str()),
                    Some(component_type_arn(workspace_id, &component_type_id).as_str()),
                ),
                "case {case:?}: expected the provisioned identity, got {read:?}"
            );
            assert_eq!(
                read.member("isAbstract").as_bool(),
                Some(record.is_abstract),
                "case {case:?}: expected the published abstractness, got {read:?}"
            );
            assert_eq!(
                read.member("status")["state"].as_str(),
                Some(STATE_ACTIVE),
                "case {case:?}: a provisioned component type is settled, nothing being in \
                 flight for it, got {read:?}"
            );
            assert_eq!(
                read.member("status")["error"],
                json!({}),
                "case {case:?}: a status carries its error structure, got {read:?}"
            );
            assert_eq!(
                read.member("extendsFrom"),
                &json!([]),
                "case {case:?}: the catalogue's entries extend nothing, got {read:?}"
            );
            assert_eq!(
                read.member("isSchemaInitialized").as_bool(),
                Some(false),
                "case {case:?}: no schema initializer ran for it, got {read:?}"
            );
            assert_eq!(
                (
                    read.member("creationDateTime").as_f64(),
                    read.member("updateDateTime").as_f64(),
                ),
                (provisioned_at, provisioned_at),
                "case {case:?}: it dates from the workspace that was provisioned with it, \
                 got {read:?}"
            );

            // The property definitions the service publishes for this built-in:
            // every name, each with its data type (and the entry type of a
            // container one) and the flags that name carries.
            let reported = read.member("propertyDefinitions").clone();
            assert_eq!(
                reported
                    .as_object()
                    .map(|definitions| definitions.len())
                    .unwrap_or_default(),
                record.property_definitions.len(),
                "case {case:?}: expected {} property definitions, got {reported:?}",
                record.property_definitions.len()
            );
            for (property_name, definition) in &record.property_definitions {
                let case = format!("{case}/{property_name}");
                let data_type = definition
                    .data_type
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {case:?}: the catalogue declares a dataType"));
                let served = &reported[property_name];
                assert_eq!(
                    served["dataType"]["type"].as_str(),
                    Some(data_type.r#type.as_str()),
                    "case {case:?}: expected the published dataType, got {served:?}"
                );
                match &data_type.nested_type {
                    Some(nested) => assert_eq!(
                        served["dataType"]["nestedType"]["type"].as_str(),
                        Some(nested.r#type.as_str()),
                        "case {case:?}: expected the published nestedType, got {served:?}"
                    ),
                    None => assert!(
                        served["dataType"]["nestedType"].is_null(),
                        "case {case:?}: a scalar dataType carries no nestedType, got {served:?}"
                    ),
                }
                assert_eq!(
                    (
                        served["isExternalId"].as_bool(),
                        served["isTimeSeries"].as_bool(),
                        served["isInherited"].as_bool(),
                    ),
                    (
                        Some(definition.is_external_id.unwrap_or(false)),
                        Some(definition.is_time_series.unwrap_or(false)),
                        Some(false),
                    ),
                    "case {case:?}: expected the published flags on the type's OWN definition, \
                     got {served:?}"
                );
            }
        }

        // The listing says the same: the built-ins beside whatever the caller
        // created, and nothing else.
        let listed = emulator
            .list_component_types(workspace_id, json!({"maxResults": 200}))
            .await;
        let listed_ids: Vec<String> = listed
            .member("componentTypeSummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["componentTypeId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            listed_ids,
            with_built_in_component_types(own.clone()),
            "case {workspace_id:?}: the listing reports the built-ins beside the caller's own, \
             got {listed:?}"
        );
    }
}

/// RULE (inheritance half): a built-in component type resolves as a declared
/// parent, and because the workspace really holds it, the property names it
/// defines are names a child can group — while a name no ancestor defines is
/// refused.
///
/// The rows are the catalogue's entries, each with one of its OWN published
/// property names, so the case differs along which parent and which name; the
/// refusal row is the same patch with a name nothing in the chain defines.
#[tokio::test]
async fn a_built_in_component_type_resolves_as_a_parent_a_child_inherits_from() {
    let emulator = Emulator::new();
    let workspace_id = "inheritBuiltInWs";
    create_workspace(&emulator, workspace_id).await;

    for (parent_id, record) in catalogue(workspace_id) {
        let inherited = record
            .property_definitions
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| panic!("case {parent_id:?}: the catalogue declares a property"));
        let child_id = format!("com.example.child.{}", parent_id.replace('.', "_"));
        let case = format!("{parent_id} <- {child_id}");

        let created = emulator
            .create_component_type(
                workspace_id,
                &child_id,
                json!({"extendsFrom": [parent_id.clone()]}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: extending a provisioned built-in must be accepted, got {created:?}"
        );
        let read = emulator.get_component_type(workspace_id, &child_id).await;
        assert_eq!(
            read.member("extendsFrom"),
            &json!([parent_id.clone()]),
            "case {case:?}: the declared parent is what the read reports, got {read:?}"
        );

        // A group over a property the PARENT defines: accepted, because the
        // workspace holds the parent and so holds that definition.
        let grouped = emulator
            .update_component_type(
                workspace_id,
                &child_id,
                json!({"propertyGroups": {"inherited": {"propertyNames": [inherited.clone()]}}}),
            )
            .await;
        assert_eq!(
            grouped.status, 200,
            "case {case:?}: grouping the inherited property {inherited:?} must be accepted, \
             got {grouped:?}"
        );

        // A name no component type in the chain defines: refused, naming it.
        let refused = emulator
            .update_component_type(
                workspace_id,
                &child_id,
                json!({"propertyGroups": {"inherited": {"propertyNames": ["noSuchProperty"]}}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a group naming an undefined property must be refused, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains("noSuchProperty")),
            "case {case:?}: the refusal must name the property it could not resolve, \
             got {refused:?}"
        );

        // Deleting the child frees the parent of references; the built-in itself
        // is untouched by any of it.
        let deleted = emulator
            .delete_component_type(workspace_id, &child_id)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the child must be deletable, got {deleted:?}"
        );
        let parent = emulator.get_component_type(workspace_id, &parent_id).await;
        assert_eq!(
            parent.status, 200,
            "case {case:?}: the built-in parent must still read back, got {parent:?}"
        );
    }
}

/// RULE (ownership half): the reserved namespace is the SERVICE's — a caller
/// cannot create a component type in it, and no caller's update or delete
/// applies to a built-in the workspace was provisioned with, which is left
/// exactly as it was.
///
/// The rows span the catalogue's ids (which are stored, so a create is the
/// conflict that id deserves), ids inside the reserved namespace that the
/// catalogue does NOT hold (nothing stored, so a write is a 404 and a create is
/// the namespace refusal), and ids just OUTSIDE the namespace — including one
/// shaped like a built-in — which a caller owns like any other.
#[tokio::test]
async fn a_built_in_component_type_is_not_a_callers_to_create_change_or_delete() {
    let emulator = Emulator::new();
    let workspace_id = "ownershipWs";
    create_workspace(&emulator, workspace_id).await;

    for component_type_id in built_in_component_type_ids() {
        let before = emulator
            .get_component_type(workspace_id, &component_type_id)
            .await;
        assert_eq!(
            before.status, 200,
            "case {component_type_id:?}: the built-in must read back, got {before:?}"
        );

        let created = emulator
            .create_component_type(workspace_id, &component_type_id, json!({}))
            .await;
        assert_eq!(
            (created.status, created.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {component_type_id:?}: creating an id the workspace already holds is a \
             conflict, got {created:?}"
        );

        for (what, refused) in [
            (
                "update",
                emulator
                    .update_component_type(
                        workspace_id,
                        &component_type_id,
                        json!({"description": "mine now"}),
                    )
                    .await,
            ),
            (
                "delete",
                emulator
                    .delete_component_type(workspace_id, &component_type_id)
                    .await,
            ),
        ] {
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {component_type_id:?}: a caller's {what} of a built-in must be refused, \
                 got {refused:?}"
            );
            assert!(
                refused
                    .member("message")
                    .as_str()
                    .is_some_and(|message| message.contains(&component_type_id)),
                "case {component_type_id:?}: the refusal must name the component type it \
                 addresses, got {refused:?}"
            );
        }

        let after = emulator
            .get_component_type(workspace_id, &component_type_id)
            .await;
        assert_eq!(
            after.body, before.body,
            "case {component_type_id:?}: a refused write leaves the built-in exactly as it was, \
             got {after:?} against {before:?}"
        );
    }

    // Ids INSIDE the reserved namespace that this emulator does not model: a
    // create is refused for the namespace, while an update or a delete finds
    // nothing stored under the id.
    for component_type_id in [
        "com.amazon.iottwinmaker.notmodelled",
        "com.amazon.x",
        "com.amazon.iotsitewise.connector.edgevideo",
    ] {
        let refused = emulator
            .create_component_type(workspace_id, component_type_id, json!({}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {component_type_id:?}: a create in the reserved namespace must be refused, \
             got {refused:?}"
        );
        assert!(
            refused.member("message").as_str().is_some_and(|message| {
                message.contains(BUILT_IN_COMPONENT_TYPE_ID_PREFIX)
                    && message.contains(component_type_id)
            }),
            "case {component_type_id:?}: the refusal must name the id and the reserved \
             namespace, got {refused:?}"
        );
        for (what, missing) in [
            (
                "update",
                emulator
                    .update_component_type(workspace_id, component_type_id, json!({}))
                    .await,
            ),
            (
                "delete",
                emulator
                    .delete_component_type(workspace_id, component_type_id)
                    .await,
            ),
        ] {
            assert_eq!(
                (missing.status, missing.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {component_type_id:?}: an {what} of an id the store does not hold is a \
                 404, got {missing:?}"
            );
        }
    }

    // Just OUTSIDE the namespace, including an id shaped like a built-in's: a
    // caller's own component type, created and deleted like any other.
    for component_type_id in [
        "com.amazonaws.iottwinmaker.alarm.basic",
        "com.amazon",
        "com.amazonian.widget",
        "plainType",
    ] {
        let created = emulator
            .create_component_type(workspace_id, component_type_id, json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {component_type_id:?}: an id outside the reserved namespace is the caller's, \
             got {created:?}"
        );
        let read = emulator
            .get_component_type(workspace_id, component_type_id)
            .await;
        assert_eq!(
            read.member("isAbstract").as_bool(),
            Some(false),
            "case {component_type_id:?}: a component type created through this API is concrete, \
             got {read:?}"
        );
        let deleted = emulator
            .delete_component_type(workspace_id, component_type_id)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {component_type_id:?}: the caller's own component type is deletable, \
             got {deleted:?}"
        );
    }
}

/// RULE (containment half): the built-ins are the SERVICE's, not the caller's
/// contents — a workspace nobody created a component type in is deletable
/// however many built-ins it holds, the refusal for one that still holds the
/// caller's counts only those, and the built-ins live and die with the workspace
/// that was provisioned with them.
///
/// The rows differ along how many component types the caller created in the
/// workspace (none, one, two), which is what the delete guard counts.
#[tokio::test]
async fn the_built_ins_are_not_the_callers_contents() {
    for own in 0..=2usize {
        let emulator = Emulator::new();
        let workspace_id = "containmentWs";
        create_workspace(&emulator, workspace_id).await;
        let created: Vec<String> = (0..own).map(|i| format!("com.example.own{i}")).collect();
        for component_type_id in &created {
            let created_own = emulator
                .create_component_type(workspace_id, component_type_id, json!({}))
                .await;
            assert_eq!(
                created_own.status, 200,
                "case {own}: the caller's create must succeed, got {created_own:?}"
            );
        }

        if own > 0 {
            let refused = emulator.delete_workspace(workspace_id).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {own}: a workspace still holding the caller's component types must not be \
                 deletable, got {refused:?}"
            );
            assert!(
                refused
                    .member("message")
                    .as_str()
                    .is_some_and(|message| message.contains(&format!("{own} component types"))),
                "case {own}: the refusal must count only the caller's component types, \
                 got {refused:?}"
            );
            for component_type_id in &created {
                let deleted = emulator
                    .delete_component_type(workspace_id, component_type_id)
                    .await;
                assert_eq!(
                    deleted.status, 200,
                    "case {own}: the caller's component type must be deletable, got {deleted:?}"
                );
            }
        }

        let deleted = emulator.delete_workspace(workspace_id).await;
        assert_eq!(
            deleted.status, 200,
            "case {own}: a workspace holding only the service's built-ins is deletable, \
             got {deleted:?}"
        );
        // Gone with the workspace: nothing resolves a built-in of a workspace
        // that is not there any more.
        for component_type_id in built_in_component_type_ids() {
            let read = emulator
                .get_component_type(workspace_id, &component_type_id)
                .await;
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {own}: {component_type_id:?} must not resolve once its workspace is \
                 deleted, got {read:?}"
            );
        }
        // And a workspace created again under the same id is provisioned afresh.
        create_workspace(&emulator, workspace_id).await;
        for component_type_id in built_in_component_type_ids() {
            let read = emulator
                .get_component_type(workspace_id, &component_type_id)
                .await;
            assert_eq!(
                (read.status, read.member("arn").as_str()),
                (
                    200,
                    Some(component_type_arn(workspace_id, &component_type_id).as_str())
                ),
                "case {own}: a workspace created again is provisioned with {component_type_id:?} \
                 again, got {read:?}"
            );
        }
    }
}
