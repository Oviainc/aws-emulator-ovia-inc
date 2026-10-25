// SPDX-License-Identifier: CC0-1.0

//! `CreateEntity`'s `compositeComponents`, end to end over HTTP.
//!
//! RULE under test: a `compositeComponents` entry is accepted exactly when its
//! key resolves as a composite component path — its first segment names one of
//! the entity's components, and each further segment names an entry in the
//! `compositeComponentTypes` of the component type the segment before it
//! reached. A path that resolves is stored under the component it hangs off, so
//! `GetEntity` reports it as that component's `compositeComponents.<name>`
//! carrying the `componentPath` it was created by; a path that does not resolve
//! is a 400 `ValidationException` and no entity is created.
//!
//! The accepted rows differ along what the rule generalises over: the DEPTH of
//! the path, how MANY composite components a create carries, which components
//! they hang off, the optional members each one carries, and the characters the
//! `Name` of a segment admits. The refused rows differ along WHICH step of the
//! resolution fails.

use serde_json::{json, Value};

use crate::harness::{Emulator, REGION};

/// The workspace every case below is created in.
const WORKSPACE: &str = "composites";

/// The component types every case below resolves its paths against.
///
/// `com.example.sensor` declares two composite component types, one of which
/// (`nested`) instantiates a type that is itself composite, so a path two
/// levels deep resolves; `com.example.plain` declares none; and
/// `com.example.anon` declares one that names no `componentTypeId`.
async fn seed_workspace(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            "com.example.inner",
            json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            "com.example.deep",
            json!({"compositeComponentTypes": {"leaf": {"componentTypeId": "com.example.inner"}}}),
        ),
        (
            "com.example.sensor",
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": "com.example.inner"},
                "nested": {"componentTypeId": "com.example.deep"},
                "leaf": {"componentTypeId": "com.example.inner"},
            }}),
        ),
        ("com.example.plain", json!({})),
        (
            "com.example.anon",
            json!({"compositeComponentTypes": {"anon": {}}}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }
}

/// A component that instantiates `com.example.sensor`, the type whose
/// `compositeComponentTypes` the paths below resolve through.
fn sensor_component() -> Value {
    json!({"componentTypeId": "com.example.sensor"})
}

/// One composite component a read of the created entity must report.
#[derive(Debug)]
struct Expected {
    /// The entity component the composite component hangs off.
    component: &'static str,
    /// Its own name: the last segment of its path, which is the key
    /// `compositeComponents` reports it under.
    name: &'static str,
    /// The component type the declaration its path resolved to names.
    component_type_id: &'static str,
    /// The whole path the create addressed it by.
    component_path: &'static str,
    /// The description the create gave it, absent when it gave none.
    description: Option<&'static str>,
}

/// RULE: every `compositeComponents` key that resolves is accepted, stored and
/// reported back under the component it hangs off, with its own name, the
/// component type its declaration names and the path it was created by.
#[tokio::test]
async fn a_resolvable_composite_component_path_is_created_and_read_back() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;

    let rows: Vec<(&str, Value, Value, Vec<Expected>)> = vec![
        (
            // The depth the issue's own reproduction uses: one segment below
            // the component.
            "oneLevel",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor/inner": {"description": "nested"}}),
            vec![Expected {
                component: "Sensor",
                name: "inner",
                component_type_id: "com.example.inner",
                component_path: "Sensor/inner",
                description: Some("nested"),
            }],
        ),
        (
            // Two composite components on ONE component, one of them carrying
            // no optional member at all.
            "twoOnOneComponent",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor/inner": {}, "Sensor/nested": {"description": "composite"}}),
            vec![
                Expected {
                    component: "Sensor",
                    name: "inner",
                    component_type_id: "com.example.inner",
                    component_path: "Sensor/inner",
                    description: None,
                },
                Expected {
                    component: "Sensor",
                    name: "nested",
                    component_type_id: "com.example.deep",
                    component_path: "Sensor/nested",
                    description: Some("composite"),
                },
            ],
        ),
        (
            // TWO levels down: `nested` instantiates `com.example.deep`, which
            // declares `leaf`.
            "twoLevelsDeep",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor/nested/leaf": {"description": "deep"}}),
            vec![Expected {
                component: "Sensor",
                name: "leaf",
                component_type_id: "com.example.inner",
                component_path: "Sensor/nested/leaf",
                description: Some("deep"),
            }],
        ),
        (
            // One composite component on each of TWO components, so the path's
            // first segment is what decides where each lands.
            "twoComponents",
            json!({"Left": sensor_component(), "Right": sensor_component()}),
            json!({"Left/inner": {"description": "left"}, "Right/inner": {"description": "right"}}),
            vec![
                Expected {
                    component: "Left",
                    name: "inner",
                    component_type_id: "com.example.inner",
                    component_path: "Left/inner",
                    description: Some("left"),
                },
                Expected {
                    component: "Right",
                    name: "inner",
                    component_type_id: "com.example.inner",
                    component_path: "Right/inner",
                    description: Some("right"),
                },
            ],
        ),
        (
            // `Name`'s pattern is `^[a-zA-Z_\-0-9]+$` (smithy_query shape
            // `Name`), so a component name carrying `_`, `-` and digits is a
            // legal first segment.
            "punctuatedName",
            json!({"Sensor_1-A": sensor_component()}),
            json!({"Sensor_1-A/inner": {}}),
            vec![Expected {
                component: "Sensor_1-A",
                name: "inner",
                component_type_id: "com.example.inner",
                component_path: "Sensor_1-A/inner",
                description: None,
            }],
        ),
        (
            // Every optional member of `CompositeComponentRequest` at once:
            // description, properties and propertyGroups.
            "everyMember",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor/inner": {
                "description": "fully specified",
                "properties": {
                    "Pressure": {
                        "definition": {"dataType": {"type": "DOUBLE"}},
                        "value": {"doubleValue": 12.5},
                    },
                },
                "propertyGroups": {
                    "readings": {"groupType": "TABULAR", "propertyNames": ["Pressure"]},
                },
            }}),
            vec![Expected {
                component: "Sensor",
                name: "inner",
                component_type_id: "com.example.inner",
                component_path: "Sensor/inner",
                description: Some("fully specified"),
            }],
        ),
        (
            // A create that carries NO compositeComponents still reports the
            // member as an empty map: the row the rule's boundary sits on.
            "noneAtAll",
            json!({"Sensor": sensor_component()}),
            json!({}),
            vec![],
        ),
    ];

    for (entity_id, components, composite_components, expected) in rows {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": components,
                    "compositeComponents": composite_components,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {entity_id:?}: expected 200 from CreateEntity, got {created:?}"
        );

        let read = emulator.get_entity(WORKSPACE, entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {entity_id:?}: expected 200 from GetEntity, got {read:?}"
        );

        for want in &expected {
            let reported =
                &read.member("components")[want.component]["compositeComponents"][want.name];
            assert!(
                reported.is_object(),
                "case {entity_id:?}: expected compositeComponents.{} on component {}, got {:?}",
                want.name,
                want.component,
                read.member("components")[want.component]
            );
            assert_eq!(
                reported["componentName"].as_str(),
                Some(want.name),
                "case {entity_id:?} {}: wrong componentName, got {reported:?}",
                want.component_path
            );
            assert_eq!(
                reported["componentTypeId"].as_str(),
                Some(want.component_type_id),
                "case {entity_id:?} {}: wrong componentTypeId, got {reported:?}",
                want.component_path
            );
            assert_eq!(
                reported["componentPath"].as_str(),
                Some(want.component_path),
                "case {entity_id:?} {}: wrong componentPath, got {reported:?}",
                want.component_path
            );
            assert_eq!(
                reported["description"].as_str(),
                want.description,
                "case {entity_id:?} {}: wrong description, got {reported:?}",
                want.component_path
            );
            assert_eq!(
                reported["status"]["state"].as_str(),
                Some("ACTIVE"),
                "case {entity_id:?} {}: expected the entity's own state, got {reported:?}",
                want.component_path
            );
        }

        // Exactly the resolved paths and nothing else: a component reports the
        // composite components created on it, and none created on another.
        let reported_components = read
            .member("components")
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert!(
            !reported_components.is_empty(),
            "case {entity_id:?}: expected the created components, got {read:?}"
        );
        for (component_name, component) in &reported_components {
            let mut reported: Vec<String> = component["compositeComponents"]
                .as_object()
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default();
            reported.sort();
            let mut wanted: Vec<String> = expected
                .iter()
                .filter(|want| want.component == component_name)
                .map(|want| want.name.to_string())
                .collect();
            wanted.sort();
            assert_eq!(
                reported, wanted,
                "case {entity_id:?} component {component_name}: expected exactly these \
                 compositeComponents, got {:?}",
                component["compositeComponents"]
            );
            assert_eq!(
                component["areAllCompositeComponentsReturned"].as_bool(),
                Some(true),
                "case {entity_id:?} component {component_name}: nothing was truncated, got {:?}",
                component["areAllCompositeComponentsReturned"]
            );
        }
    }
}

/// RULE: a `compositeComponents` key that does NOT resolve is a 400
/// `ValidationException`, and the entity it named is not created.
///
/// The rows differ along which step of the resolution fails, which is what the
/// refusal generalises over.
#[tokio::test]
async fn an_unresolvable_composite_component_path_is_refused() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;

    let rows: Vec<(&str, Value, Value)> = vec![
        (
            // The entity has no component by that name — the case the issue
            // recorded.
            "unknownComponent",
            json!({}),
            json!({"Sensor/inner": {"description": "nested"}}),
        ),
        (
            // The component instantiates no component type, so it declares no
            // composite component at all.
            "componentWithoutType",
            json!({"Sensor": {"description": "typeless"}}),
            json!({"Sensor/inner": {}}),
        ),
        (
            // The component's component type is not one this workspace holds,
            // so nothing can be resolved against it.
            "componentTypeNotInWorkspace",
            json!({"Sensor": {"componentTypeId": "com.example.absent"}}),
            json!({"Sensor/inner": {}}),
        ),
        (
            // The component type exists but declares no compositeComponentTypes.
            "undeclaredComposite",
            json!({"Sensor": {"componentTypeId": "com.example.plain"}}),
            json!({"Sensor/inner": {}}),
        ),
        (
            // The declaration exists but names no componentTypeId, so it names
            // no type to instantiate.
            "declarationWithoutComponentTypeId",
            json!({"Sensor": {"componentTypeId": "com.example.anon"}}),
            json!({"Sensor/anon": {}}),
        ),
        (
            // A single segment names a top-level component, not a composite
            // component inside one.
            "singleSegmentPath",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor": {}}),
        ),
        (
            // `ComponentPath`'s pattern admits `/`, so an empty segment reaches
            // the handler and names nothing.
            "emptySegment",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor//inner": {}}),
        ),
        (
            // One segment too deep: `com.example.inner` declares no composite
            // component types of its own.
            "deeperThanDeclared",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor/inner/leaf": {}}),
        ),
        (
            // A property a create declares is a new property, so its definition
            // must carry a dataType — in a composite component as in a
            // component.
            "compositePropertyWithoutDataType",
            json!({"Sensor": sensor_component()}),
            json!({"Sensor/inner": {"properties": {"p": {"definition": {"isTimeSeries": true}}}}}),
        ),
    ];

    for (entity_id, components, composite_components) in rows {
        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": components,
                    "compositeComponents": composite_components,
                }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {entity_id:?}: expected 400 from CreateEntity, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {entity_id:?}: expected a ValidationException, got {refused:?}"
        );

        let read = emulator.get_entity(WORKSPACE, entity_id).await;
        assert_eq!(
            read.status, 404,
            "case {entity_id:?}: a refused create must store no entity, got {read:?}"
        );
    }
}

/// The seed's `com.example.sensor` declares `leaf` itself AND through `nested`,
/// so `<Component>/leaf` and `<Component>/nested/leaf` both resolve while
/// sharing a last segment. `CompositeComponentResponse` is keyed by `Name`, so
/// one read cannot carry both.
///
/// RULE (the reporting boundary of the same rule): both are STORED, the one the
/// response map carries is the same on every read, and
/// `areAllCompositeComponentsReturned` is false — the flag says what the map
/// could not hold instead of claiming everything was returned.
#[tokio::test]
async fn composite_components_sharing_a_name_are_both_stored_and_reported_as_incomplete() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "collides",
                "entityName": "Collides",
                "components": {"Sensor": sensor_component()},
                "compositeComponents": {
                    "Sensor/leaf": {"description": "shallow"},
                    "Sensor/nested/leaf": {"description": "deep"},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    let first = emulator.get_entity(WORKSPACE, "collides").await;
    assert_eq!(first.status, 200, "expected 200 from GetEntity, got {first:?}");
    let component = first.member("components")["Sensor"].clone();
    let reported = component["compositeComponents"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reported.keys().collect::<Vec<_>>(),
        vec!["leaf"],
        "expected the one name the response map can key, got {component:?}"
    );
    assert_eq!(
        component["areAllCompositeComponentsReturned"].as_bool(),
        Some(false),
        "expected the flag to report what the map could not hold, got {component:?}"
    );
    let reported_path = reported["leaf"]["componentPath"].clone();
    assert!(
        ["Sensor/leaf", "Sensor/nested/leaf"]
            .contains(&reported_path.as_str().unwrap_or_default()),
        "expected one of the two created paths, got {reported_path:?}"
    );

    // Deterministic: a second read reports the same one.
    let second = emulator.get_entity(WORKSPACE, "collides").await;
    assert_eq!(
        second.member("components")["Sensor"]["compositeComponents"]["leaf"]["componentPath"],
        reported_path,
        "expected two reads to report the same composite component, got {second:?}"
    );

    // Both are stored: nothing the create accepted was dropped.
    let state = emulator.state();
    let state = state.lock();
    let stored = &state.region(REGION).workspaces[WORKSPACE].entities["collides"].components
        ["Sensor"]
        .composite_components;
    assert_eq!(
        stored.keys().collect::<Vec<_>>(),
        vec!["leaf", "nested/leaf"],
        "expected both created composite components to be stored, got {stored:?}"
    );
}

/// RULE (the storage side of the same rule): a composite component's properties
/// and property groups are persisted exactly as the create gave them.
///
/// `ComponentSummary` models no properties, so the store is where this round
/// trip is observable until `ListProperties` / `GetPropertyValue` are
/// implemented; the reported members are asserted over HTTP above.
#[tokio::test]
async fn a_composite_component_persists_the_properties_it_was_created_with() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "stored",
                "entityName": "Stored",
                "components": {"Sensor": {"componentTypeId": "com.example.sensor"}},
                "compositeComponents": {
                    "Sensor/nested/leaf": {
                        "description": "deep",
                        "properties": {
                            "Pressure": {
                                "definition": {"dataType": {"type": "DOUBLE"}},
                                "value": {"doubleValue": 12.5},
                            },
                        },
                        "propertyGroups": {
                            "readings": {"groupType": "TABULAR", "propertyNames": ["Pressure"]},
                        },
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    let state = emulator.state();
    let state = state.lock();
    let component = state.region(REGION).workspaces[WORKSPACE].entities["stored"]
        .components
        .get("Sensor")
        .expect("the created component is stored");
    // Keyed by the path BELOW the component, so a composite component nested
    // two levels down is held under the whole remainder of its path.
    let composite = component
        .composite_components
        .get("nested/leaf")
        .unwrap_or_else(|| {
            panic!(
                "expected the composite component under nested/leaf, got {:?}",
                component.composite_components.keys().collect::<Vec<_>>()
            )
        });
    assert_eq!(
        composite.component_path, "Sensor/nested/leaf",
        "expected the created path, got {composite:?}"
    );
    assert_eq!(
        composite.component_type_id, "com.example.inner",
        "expected the component type the declaration names, got {composite:?}"
    );
    let property = composite
        .properties
        .get("Pressure")
        .unwrap_or_else(|| panic!("expected the created property, got {composite:?}"));
    assert_eq!(
        property.value.as_ref().and_then(|value| value.double_value),
        Some(12.5),
        "expected the created property value, got {property:?}"
    );
    assert_eq!(
        composite
            .property_groups
            .get("readings")
            .map(|group| group.property_names.clone()),
        Some(vec!["Pressure".to_string()]),
        "expected the created property group, got {composite:?}"
    );
}
