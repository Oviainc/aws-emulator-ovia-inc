// SPDX-License-Identifier: CC0-1.0

//! `DeleteComponentType`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The suites assert RULES over a table of inputs rather than one recorded
//! case: a delete removes the component type from the workspace that contains
//! it, answers the `DELETING` state and stops its ARN resolving, whatever the
//! component type carried and whichever lifecycle state it was in; a component
//! type something stored still references is refused with a
//! `ValidationException` that changes nothing, and becomes deletable once the
//! reference is gone; and a label that names a resource the store does not hold
//! is a `ResourceNotFoundException`.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    COMPONENT_TYPE_SETTLE_SPAN, SCHEMA_INITIALIZER_FUNCTION, STATE_ACTIVE, STATE_CREATING,
    STATE_DELETING,
};

use crate::harness::{component_type_arn, Emulator};

/// Block until the `CREATING` -> `ACTIVE` transition a create owes has fallen
/// due.
///
/// The emulator stores the instant and never sleeps for it, so a test that
/// wants to observe the settled state is the one that has to wait; the margin
/// keeps the wait from racing the span's own boundary.
fn wait_until_settled() {
    sleep(COMPONENT_TYPE_SETTLE_SPAN + Duration::from_millis(100));
}

/// Which kind of stored reference a case points at the component type under
/// test, and whether that kind keeps it alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// An entity component that INSTANTIATES the type.
    EntityComponent,
    /// Another component type that `extendsFrom` it.
    ExtendsFrom,
    /// Another component type that names it in `compositeComponentTypes`.
    CompositeComponentType,
    /// Both an entity component and an extending component type at once.
    EntityComponentAndExtendsFrom,
    /// A `RELATIONSHIP` property definition on another component type whose
    /// `targetComponentTypeId` names it. NOT a reference that keeps it alive:
    /// no writer in this service requires a relationship target to resolve, so
    /// it never blocks a delete.
    RelationshipTarget,
}

impl Reference {
    /// Whether a delete of the referenced component type is refused while this
    /// reference stands.
    fn blocks(self) -> bool {
        match self {
            Reference::EntityComponent
            | Reference::ExtendsFrom
            | Reference::CompositeComponentType
            | Reference::EntityComponentAndExtendsFrom => true,
            Reference::RelationshipTarget => false,
        }
    }
}

/// The id of the component type every case of the reference suite deletes.
const TARGET: &str = "com.example.target";

/// RULE: a delete removes the component type from the workspace that contains
/// it, answers the non-terminal `DELETING` state for the removal it performed,
/// and stops the ARN it was minted under resolving.
///
/// The rows differ along what the component type carries (nothing, names, the
/// singleton flag, property definitions, a schema initializer, a composite
/// entry naming itself) and along the lifecycle state it is in when the delete
/// arrives, because that is what the rule generalises over.
#[tokio::test]
async fn deleting_an_unreferenced_component_type_removes_it() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("ctDelete").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    let rows: Vec<(&str, Value, bool)> = vec![
        ("bare", json!({}), false),
        (
            "named.and.described",
            json!({"componentTypeName": "A sensor", "description": "a described type"}),
            false,
        ),
        ("singleton-type", json!({"isSingleton": true}), false),
        (
            "com.example.withProperties",
            json!({
                "propertyDefinitions": {
                    "serial": {"dataType": {"type": "STRING"}, "isExternalId": true},
                    "temperature": {
                        "dataType": {"type": "DOUBLE", "unitOfMeasure": "Celsius"},
                        "isTimeSeries": true,
                    },
                },
            }),
            false,
        ),
        (
            "com.example.withSchemaInitializer",
            json!({
                "propertyDefinitions": {"alarmKey": {"dataType": {"type": "STRING"}}},
                "functions": {
                    SCHEMA_INITIALIZER_FUNCTION: {
                        "scope": "ENTITY",
                        "implementedBy": {
                            "lambda": {
                                "arn": "arn:aws:lambda:us-west-2:000000000000:function:init",
                            },
                        },
                    },
                },
            }),
            false,
        ),
        (
            // A component type that names ITSELF in its composite entries: the
            // reference goes away with the record that carries it, so it is
            // not one that keeps the record alive.
            "com.example.selfComposite",
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": "com.example.selfComposite"}}}),
            false,
        ),
        // The same delete, but after the clock has settled the component type
        // in ACTIVE: the delete does not depend on the state it finds.
        ("settled.before.the.delete", json!({}), true),
    ];

    for (component_type_id, request, wait_for_active) in rows {
        let case = component_type_id;
        let created = emulator
            .create_component_type("ctDelete", component_type_id, request)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {created:?}"
        );

        if wait_for_active {
            wait_until_settled();
        }
        let before = emulator
            .get_component_type("ctDelete", component_type_id)
            .await;
        let expected_state = if wait_for_active {
            STATE_ACTIVE
        } else {
            STATE_CREATING
        };
        assert_eq!(
            (before.status, before.member("status")["state"].as_str()),
            (200, Some(expected_state)),
            "case {case:?}: expected 200 with the stored state {expected_state}, got {before:?}"
        );

        let deleted = emulator
            .delete_component_type("ctDelete", component_type_id)
            .await;
        assert_eq!(
            (deleted.status, deleted.member("state").as_str()),
            (200, Some(STATE_DELETING)),
            "case {case:?}: expected 200 {STATE_DELETING}, got {deleted:?}"
        );

        let gone = emulator
            .get_component_type("ctDelete", component_type_id)
            .await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the component type must be gone, got {gone:?}"
        );
        let arn = component_type_arn("ctDelete", component_type_id);
        let tags = emulator.list_tags_for_resource(&arn, json!({})).await;
        assert_eq!(
            (tags.status, tags.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the deleted component type's ARN must stop resolving, got {tags:?}"
        );
    }

    // Nothing the cases created is left, so the workspace is empty and
    // deletable: `DeleteWorkspace` refuses one that still holds a component
    // type, which is what makes this a check on the removal and not only on
    // what a read reports.
    let workspace_gone = emulator.delete_workspace("ctDelete").await;
    assert_eq!(
        workspace_gone.status, 200,
        "the workspace must be empty once every case's component type is deleted, \
         got {workspace_gone:?}"
    );
}

/// RULE: a component type something stored still references is refused with the
/// modelled `ValidationException` (HTTP 400) and left exactly as it was, and
/// becomes deletable once nothing references it.
///
/// The rows differ along the KIND of reference — an entity component that
/// instantiates the type, a component type that extends it, a component type
/// that composes it, two at once — and include the one kind that is not a
/// reference the service requires to resolve, a `RELATIONSHIP` property
/// definition's target, which never blocks.
#[tokio::test]
async fn a_referenced_component_type_is_only_deletable_once_nothing_references_it() {
    let emulator = Emulator::new();

    for case in [
        Reference::EntityComponent,
        Reference::ExtendsFrom,
        Reference::CompositeComponentType,
        Reference::EntityComponentAndExtendsFrom,
        Reference::RelationshipTarget,
    ] {
        // One workspace per case, so a case's leftovers cannot reference the
        // next case's component type.
        let workspace = format!("refWs{case:?}");
        assert_eq!(
            emulator.create_workspace(&workspace).await.status,
            200,
            "case {case:?}: the workspace must be created"
        );
        let created = emulator
            .create_component_type(&workspace, TARGET, json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the referenced component type must be created, got {created:?}"
        );

        // Stand the case's reference up.
        let referencing_entity = matches!(
            case,
            Reference::EntityComponent | Reference::EntityComponentAndExtendsFrom
        );
        if referencing_entity {
            let entity = emulator
                .create_entity(
                    &workspace,
                    json!({
                        "entityId": "holder",
                        "entityName": "Holder",
                        "components": {
                            "instance": {"componentTypeId": TARGET},
                        },
                    }),
                )
                .await;
            assert_eq!(
                entity.status, 200,
                "case {case:?}: the referencing entity must be created, got {entity:?}"
            );
        }
        let referencing_type: Option<(&str, Value)> = match case {
            Reference::EntityComponent => None,
            Reference::ExtendsFrom | Reference::EntityComponentAndExtendsFrom => Some((
                "com.example.child",
                json!({"extendsFrom": [TARGET]}),
            )),
            Reference::CompositeComponentType => Some((
                "com.example.composite",
                json!({"compositeComponentTypes": {"inner": {"componentTypeId": TARGET}}}),
            )),
            Reference::RelationshipTarget => Some((
                "com.example.related",
                json!({
                    "propertyDefinitions": {
                        "partOf": {
                            "dataType": {
                                "type": "RELATIONSHIP",
                                "relationship": {
                                    "targetComponentTypeId": TARGET,
                                    "relationshipType": "belongsTo",
                                },
                            },
                        },
                    },
                }),
            )),
        };
        if let Some((referencing_id, request)) = &referencing_type {
            let other = emulator
                .create_component_type(&workspace, referencing_id, request.clone())
                .await;
            assert_eq!(
                other.status, 200,
                "case {case:?}: the referencing component type must be created, got {other:?}"
            );
        }

        if case.blocks() {
            let refused = emulator.delete_component_type(&workspace, TARGET).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: deleting a referenced component type must be refused, \
                 got {refused:?}"
            );
            let untouched = emulator.get_component_type(&workspace, TARGET).await;
            assert_eq!(
                untouched.status, 200,
                "case {case:?}: a refused delete must change nothing, got {untouched:?}"
            );

            // Take the references away; the same delete then takes.
            if referencing_entity {
                let entity_gone = emulator.delete_entity(&workspace, "holder", false).await;
                assert_eq!(
                    entity_gone.status, 200,
                    "case {case:?}: the referencing entity must be deletable, got {entity_gone:?}"
                );
            }
            if let Some((referencing_id, _)) = &referencing_type {
                let type_gone = emulator
                    .delete_component_type(&workspace, referencing_id)
                    .await;
                assert_eq!(
                    type_gone.status, 200,
                    "case {case:?}: the referencing component type must be deletable, \
                     got {type_gone:?}"
                );
            }
        }

        let deleted = emulator.delete_component_type(&workspace, TARGET).await;
        assert_eq!(
            (deleted.status, deleted.member("state").as_str()),
            (200, Some(STATE_DELETING)),
            "case {case:?}: expected 200 {STATE_DELETING} once nothing references it, \
             got {deleted:?}"
        );
        let gone = emulator.get_component_type(&workspace, TARGET).await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the component type must be gone, got {gone:?}"
        );
    }
}

/// RULE: both of this operation's labels address an EXISTING resource, so one
/// naming something the store does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) — the same reading `GetComponentType`
/// gives the same two labels.
///
/// The rows differ along WHICH label misses: the workspace, the component type,
/// and the component type a previous delete already removed.
#[tokio::test]
async fn deleting_what_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("missWs").await.status,
        200,
        "the workspace the cases are looked for in must exist"
    );
    assert_eq!(
        emulator
            .create_component_type("missWs", "com.example.once", json!({}))
            .await
            .status,
        200,
        "the component type the repeated-delete case removes must exist"
    );
    assert_eq!(
        emulator
            .delete_component_type("missWs", "com.example.once")
            .await
            .status,
        200,
        "the first delete of the repeated-delete case must succeed"
    );

    let rows: Vec<(&str, &str, &str)> = vec![
        ("no-such-workspace", "noSuchWs", "com.example.absent"),
        ("no-such-component-type", "missWs", "com.example.absent"),
        // An id whose shape is a plain name rather than a dotted one: the
        // reading does not depend on the shape of the label.
        ("plain-id", "missWs", "absentType"),
        ("already-deleted", "missWs", "com.example.once"),
    ];

    for (case, workspace_id, component_type_id) in rows {
        let resp = emulator
            .delete_component_type(workspace_id, component_type_id)
            .await;
        assert_eq!(
            (resp.status, resp.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {resp:?}"
        );
    }
}
