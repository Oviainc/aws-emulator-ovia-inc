// SPDX-License-Identifier: CC0-1.0

//! `DeleteComponentType` against an entity's COMPOSITE components, end to end
//! over HTTP.
//!
//! RULE under test: everything an entity stores that instantiates a component
//! type counts as a stored reference to it, and a composite component is one of
//! those instances — it carries a `componentTypeId` of its own and its
//! `definedIn`, inherited property definitions and inherited property groups are
//! resolved against it — so the type is refused with the modelled
//! `ValidationException` (HTTP 400) while any composite component instantiates
//! it, whatever depth it sits at and whether or not the declaration that
//! produced it still stands on the holding component type, and becomes
//! deletable only once no composite component names it any more.
//!
//! The rows differ along what the rule generalises over: the DEPTH of the
//! composite component's path, whether the `compositeComponentTypes`
//! declaration that produced it is still on the holding component type (an
//! `UpdateComponentType` replaces that map whole), and HOW the reference is
//! taken away again — the composite component, the component holding it or the
//! whole entity. A second suite pins the other direction: a composite component
//! of a DIFFERENT component type is not a reference to this one, so it does not
//! refuse its delete.

use serde_json::{json, Value};

use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::STATE_DELETING;

use crate::harness::Emulator;

/// The component type every case deletes: the one the entity's composite
/// component instantiates.
const TARGET: &str = "com.example.target";
/// A component type whose `compositeComponentTypes` names [`TARGET`], so a path
/// one level below a component reaches it.
const HOLDER: &str = "com.example.holder";
/// A component type whose `compositeComponentTypes` names [`TARGET`] and which
/// [`HOLDER`] itself composes, so a path two levels below a component reaches
/// [`TARGET`] through it.
const MIDDLE: &str = "com.example.middle";
/// A component type nothing under test instantiates, used by the negative suite
/// as the type a composite component names INSTEAD of [`TARGET`].
const SIBLING: &str = "com.example.sibling";
/// The property [`TARGET`] defines, which a composite component instantiating
/// it inherits — what disappears from a read if the type is deleted out from
/// under the composite component.
const INHERITED_PROPERTY: &str = "Pressure";
/// The entity every case stands its composite components up on.
const ENTITY: &str = "holderEntity";
/// The entity's top-level component, the first segment of every path below.
const COMPONENT: &str = "Sensor";

/// How deep below the entity's component the composite component that
/// instantiates [`TARGET`] sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// Directly inside the component: `Sensor/inner`.
    One,
    /// Inside a composite component of the component: `Sensor/middle/inner`.
    Two,
}

impl Depth {
    /// The `componentPath` of the composite component that instantiates
    /// [`TARGET`].
    fn path(self) -> &'static str {
        match self {
            Depth::One => "Sensor/inner",
            Depth::Two => "Sensor/middle/inner",
        }
    }

    /// Its own name — the path's last segment, which is the key `GetEntity`
    /// reports it under.
    fn name(self) -> &'static str {
        "inner"
    }

    /// The `compositeComponents` a create needs to stand this case up: every
    /// composite component on the way down, plus the one under test.
    fn composite_components(self) -> Value {
        match self {
            Depth::One => json!({"Sensor/inner": {}}),
            Depth::Two => json!({"Sensor/middle": {}, "Sensor/middle/inner": {}}),
        }
    }
}

/// How a case takes the stored reference away again, after which the same
/// delete must be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// `UpdateEntity` removing the composite component itself.
    CompositeComponent,
    /// `UpdateEntity` removing the top-level component that holds it, which
    /// takes the composite components inside it with it.
    Component,
    /// `DeleteEntity` removing the whole entity.
    Entity,
}

/// Create the workspace and the three component types every case resolves its
/// paths against.
async fn seed(emulator: &Emulator, workspace: &str) {
    let created = emulator.create_workspace(workspace).await;
    assert_eq!(
        created.status, 200,
        "workspace {workspace:?}: expected 200 from CreateWorkspace, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            TARGET,
            json!({"propertyDefinitions": {INHERITED_PROPERTY: {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (SIBLING, json!({})),
        (
            MIDDLE,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": TARGET}}}),
        ),
        (
            HOLDER,
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": TARGET},
                "other": {"componentTypeId": SIBLING},
                "middle": {"componentTypeId": MIDDLE},
            }}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(workspace, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "workspace {workspace:?}: expected 200 from CreateComponentType \
             {component_type_id:?}, got {created:?}"
        );
    }
}

/// Drop every `compositeComponentTypes` declaration from the component types
/// the paths were resolved through, leaving the stored composite components as
/// the only thing that still names [`TARGET`].
///
/// `UpdateComponentType` replaces the map WHOLE, so an empty map is what drops
/// the declarations.
async fn drop_declarations(emulator: &Emulator, workspace: &str, case: &str) {
    for component_type_id in [HOLDER, MIDDLE] {
        let updated = emulator
            .update_component_type(
                workspace,
                component_type_id,
                json!({"compositeComponentTypes": {}}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateComponentType {component_type_id:?} \
             dropping its compositeComponentTypes, got {updated:?}"
        );
    }
}

/// The composite component `GetEntity` reports under `path`'s last segment, or
/// `Value::Null` when it reports none.
async fn reported_composite(emulator: &Emulator, workspace: &str, name: &str) -> Value {
    let entity = emulator.get_entity(workspace, ENTITY).await;
    assert_eq!(
        entity.status, 200,
        "GetEntity in workspace {workspace:?}: expected 200, got {entity:?}"
    );
    entity.member("components")[COMPONENT]["compositeComponents"][name].clone()
}

/// RULE: a composite component of an entity instantiates the component type it
/// names, so that type is still referenced while the composite component is
/// stored — at any depth, and whether or not the declaration that produced it
/// still stands — and its delete is refused until the composite component is
/// gone.
#[tokio::test]
async fn a_composite_component_keeps_the_component_type_it_instantiates_undeletable() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, Depth, bool, Removal)> = vec![
        // The declaration still stands: the holding component type composes
        // TARGET and the entity instantiates it.
        (
            "declared-depth1-removeComposite",
            Depth::One,
            false,
            Removal::CompositeComponent,
        ),
        // The declaration is gone, so the stored composite component is the
        // ONLY thing left naming TARGET — the case the reference scan used to
        // miss.
        (
            "dropped-depth1-removeEntity",
            Depth::One,
            true,
            Removal::Entity,
        ),
        (
            "dropped-depth1-removeComponent",
            Depth::One,
            true,
            Removal::Component,
        ),
        // Two levels down, reached through another composite component.
        (
            "declared-depth2-removeEntity",
            Depth::Two,
            false,
            Removal::Entity,
        ),
        (
            "dropped-depth2-removeComponent",
            Depth::Two,
            true,
            Removal::Component,
        ),
    ];

    for (case, depth, dropped, removal) in rows {
        // One workspace per row, so a row's leftovers cannot reference the next
        // row's component type.
        let workspace = format!("ccRef{}", case.replace('-', ""));
        seed(&emulator, &workspace).await;

        let entity = emulator
            .create_entity(
                &workspace,
                json!({
                    "entityId": ENTITY,
                    "entityName": ENTITY,
                    "components": {COMPONENT: {"componentTypeId": HOLDER}},
                    "compositeComponents": depth.composite_components(),
                }),
            )
            .await;
        assert_eq!(
            entity.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {entity:?}"
        );
        if dropped {
            drop_declarations(&emulator, &workspace, case).await;
        }

        // The composite component instantiates TARGET, so it is refused.
        let refused = emulator.delete_component_type(&workspace, TARGET).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: deleting a component type an entity's composite component \
             instantiates must be refused, got {refused:?}"
        );
        let message = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(ENTITY),
            "case {case:?}: the refusal must name the entity whose composite component still \
             instantiates the type, got {message:?}"
        );

        // The refusal changed nothing, so the composite component's inherited
        // schema still resolves: this is what the delete would have destroyed.
        let composite = reported_composite(&emulator, &workspace, depth.name()).await;
        assert_eq!(
            (
                composite["componentTypeId"].as_str(),
                composite["definedIn"].as_str(),
                composite["componentPath"].as_str()
            ),
            (Some(TARGET), Some(TARGET), Some(depth.path())),
            "case {case:?}: the composite component must still resolve against the type \
             it instantiates, got {composite:?}"
        );
        let properties = emulator
            .list_properties(
                &workspace,
                json!({"entityId": ENTITY, "componentPath": depth.path()}),
            )
            .await;
        let listed: Vec<&str> = properties
            .member("propertySummaries")
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .filter_map(|summary| summary["propertyName"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            properties.status == 200 && listed.contains(&INHERITED_PROPERTY),
            "case {case:?}: the composite component must still inherit {INHERITED_PROPERTY} \
             from the type it instantiates, got {properties:?}"
        );

        // Take the reference away; the same delete then takes.
        match removal {
            Removal::CompositeComponent => {
                let updated = emulator
                    .update_entity(
                        &workspace,
                        ENTITY,
                        json!({
                            "compositeComponentUpdates": {
                                depth.path(): {"updateType": "DELETE"},
                            },
                        }),
                    )
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: the composite component must be removable, got {updated:?}"
                );
            }
            Removal::Component => {
                let updated = emulator
                    .update_entity(
                        &workspace,
                        ENTITY,
                        json!({
                            "componentUpdates": {COMPONENT: {"updateType": "DELETE"}},
                        }),
                    )
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {case:?}: the holding component must be removable, got {updated:?}"
                );
            }
            Removal::Entity => {
                let deleted = emulator.delete_entity(&workspace, ENTITY, false).await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: the entity must be deletable, got {deleted:?}"
                );
            }
        }
        if !dropped {
            // The declarations the paths were resolved through are references of
            // their own, so they have to go before the delete can take.
            drop_declarations(&emulator, &workspace, case).await;
        }

        let deleted = emulator.delete_component_type(&workspace, TARGET).await;
        assert_eq!(
            (deleted.status, deleted.member("state").as_str()),
            (200, Some(STATE_DELETING)),
            "case {case:?}: expected 200 {STATE_DELETING} once no composite component \
             instantiates it, got {deleted:?}"
        );
        let gone = emulator.get_component_type(&workspace, TARGET).await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the component type must be gone, got {gone:?}"
        );
    }
}

/// RULE, the other direction: what counts is the component type a composite
/// component NAMES, so a composite component of another component type is no
/// reference to this one and does not refuse its delete.
///
/// Two entities each hold ONE composite component, of a different component
/// type, and a third component type is instantiated by neither: the rows differ
/// along which stored composite component names the type the delete addresses.
#[tokio::test]
async fn a_composite_component_of_another_component_type_is_not_a_reference() {
    let emulator = Emulator::new();
    let workspace = "ccRefOther";
    seed(&emulator, workspace).await;

    // One entity per instantiated type, so removing one reference leaves the
    // other standing and the two are counted apart.
    let entities: Vec<(&str, &str)> = vec![
        ("targetHolder", "Sensor/inner"),
        ("otherHolder", "Sensor/other"),
    ];
    for (entity_id, component_path) in entities {
        let entity = emulator
            .create_entity(
                workspace,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {COMPONENT: {"componentTypeId": HOLDER}},
                    "compositeComponents": {component_path: {}},
                }),
            )
            .await;
        assert_eq!(
            entity.status, 200,
            "entity {entity_id:?}: expected 200 from CreateEntity, got {entity:?}"
        );
    }
    // With the declarations gone, the stored composite components are the only
    // references left, so each delete below is decided by them alone.
    drop_declarations(&emulator, workspace, "other").await;

    for (case, component_type_id, expected_status) in [
        // No composite component instantiates this one, even though a composite
        // component's path was resolved THROUGH it.
        ("middle-unreferenced", MIDDLE, 200u16),
        ("sibling-referenced", SIBLING, 400),
        ("target-referenced", TARGET, 400),
    ] {
        let resp = emulator
            .delete_component_type(workspace, component_type_id)
            .await;
        assert_eq!(
            resp.status, expected_status,
            "case {case:?}: expected {expected_status} for {component_type_id}, got {resp:?}"
        );
    }

    // Take the other type's composite component away: its own type becomes
    // deletable while the type under test stays refused by the composite
    // component that does name it.
    let deleted = emulator.delete_entity(workspace, "otherHolder", false).await;
    assert_eq!(
        deleted.status, 200,
        "the entity holding the other type's composite component must be deletable, \
         got {deleted:?}"
    );
    let sibling = emulator.delete_component_type(workspace, SIBLING).await;
    assert_eq!(
        (sibling.status, sibling.member("state").as_str()),
        (200, Some(STATE_DELETING)),
        "expected 200 {STATE_DELETING} for the other type once nothing instantiates it, \
         got {sibling:?}"
    );
    let target = emulator.delete_component_type(workspace, TARGET).await;
    assert_eq!(
        (target.status, target.error_type.as_deref()),
        (400, Some("ValidationException")),
        "the type the remaining composite component instantiates must stay refused, \
         got {target:?}"
    );
}
