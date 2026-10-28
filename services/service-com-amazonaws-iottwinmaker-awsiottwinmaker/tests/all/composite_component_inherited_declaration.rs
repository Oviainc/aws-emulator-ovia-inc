// SPDX-License-Identifier: CC0-1.0

//! A composite component path resolved through an INHERITED
//! `compositeComponentTypes` declaration, end to end over HTTP.
//!
//! RULE under test: each segment below the first of a `ComponentPath` is
//! resolved against the EFFECTIVE `compositeComponentTypes` of the component
//! type the segment before it reached — that type's own declarations plus the
//! ones its `extendsFrom` chain contributes, nearest first, the component type
//! instantiated being the one the nearest declaration that names any refers to.
//! That is the same effective schema `GetComponentType` reports, so a
//! declaration published as part of a component type's schema is one a path
//! through it can be stood up on, whether the type declares it itself or
//! inherits it.
//!
//! The rows differ along what the rule generalises over: how many `extendsFrom`
//! hops the declaration is reached through, which DEPTH of the path the
//! inherited declaration sits at, whether the nearer declaration refines the
//! inherited one or leaves the id to it, and which of the two writers of an
//! entity's composite components (`CreateEntity`'s `compositeComponents`,
//! `UpdateEntity`'s `compositeComponentUpdates`) stands the composite component
//! up. The refused rows differ along which step of the resolution finds nothing
//! on the whole chain.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case below works in. Each test builds its own emulator,
/// so the id is shared without the cases sharing state.
const WORKSPACE: &str = "inheritedComposites";

/// The component type every resolved declaration below names, unless a nearer
/// declaration refines it.
const LEAF: &str = "com.example.leaf";
/// The component type the refining declaration names instead of [`LEAF`].
const OTHER_LEAF: &str = "com.example.otherleaf";
/// Declares a composite component type of its own, and is itself only reached
/// through an inherited declaration — so a path two levels deep has an
/// inherited declaration at EACH level.
const DEEP_BASE: &str = "com.example.deepbase";
/// Extends [`DEEP_BASE`] and declares nothing itself.
const DEEP_CHILD: &str = "com.example.deepchild";
/// The declarer: every composite component type the children below inherit is
/// declared here.
const BASE: &str = "com.example.base";
/// One hop from [`BASE`], declaring nothing itself.
const CHILD: &str = "com.example.child";
/// Two hops from [`BASE`], through [`CHILD`].
const GRANDCHILD: &str = "com.example.grandchild";
/// One hop from [`BASE`], restating `refined` with another `componentTypeId`.
const REFINER: &str = "com.example.refiner";
/// One hop from [`BASE`], restating `inner` with NO `componentTypeId`, so the
/// id stays the inherited one.
const PARTIAL: &str = "com.example.partial";
/// A chain that declares no composite component type anywhere.
const PLAIN_BASE: &str = "com.example.plainbase";
/// One hop from [`PLAIN_BASE`], declaring nothing either.
const PLAIN_CHILD: &str = "com.example.plainchild";

/// An emulator holding the workspace and the inheritance chains the cases
/// resolve their paths through.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );
    // In reference order: `extendsFrom` and `compositeComponentTypes` may only
    // name a component type the workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            LEAF,
            json!({"propertyDefinitions": {"Reading": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            OTHER_LEAF,
            json!({"propertyDefinitions": {"Other": {"dataType": {"type": "STRING"}}}}),
        ),
        (
            DEEP_BASE,
            json!({"compositeComponentTypes": {"leaf": {"componentTypeId": LEAF}}}),
        ),
        (DEEP_CHILD, json!({"extendsFrom": [DEEP_BASE]})),
        (
            BASE,
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": LEAF},
                "nested": {"componentTypeId": DEEP_CHILD},
                "refined": {"componentTypeId": LEAF},
                "anon": {},
            }}),
        ),
        (CHILD, json!({"extendsFrom": [BASE]})),
        (GRANDCHILD, json!({"extendsFrom": [CHILD]})),
        (
            REFINER,
            json!({
                "extendsFrom": [BASE],
                "compositeComponentTypes": {"refined": {"componentTypeId": OTHER_LEAF}},
            }),
        ),
        (
            PARTIAL,
            json!({
                "extendsFrom": [BASE],
                "compositeComponentTypes": {"inner": {}},
            }),
        ),
        (PLAIN_BASE, json!({})),
        (PLAIN_CHILD, json!({"extendsFrom": [PLAIN_BASE]})),
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
    emulator
}

/// One row of the rule: a path that must resolve against the effective schema
/// of the component type its first segment's component instantiates.
#[derive(Debug)]
struct Row {
    /// The case name, also the entity id each writer stands its entity up
    /// under.
    case: &'static str,
    /// The component type the entity's `Comp` component instantiates.
    component_type_id: &'static str,
    /// The path the composite component is addressed by.
    component_path: &'static str,
    /// The composite component's own name: the path's last segment, which is
    /// the key a read reports it under.
    name: &'static str,
    /// The component type the resolution must land on.
    resolved: &'static str,
}

/// Every row of the rule.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The declaring type itself: the own map still resolves.
            case: "ownDeclaration",
            component_type_id: BASE,
            component_path: "Comp/inner",
            name: "inner",
            resolved: LEAF,
        },
        Row {
            // ONE `extendsFrom` hop to the declaration.
            case: "oneHopInherited",
            component_type_id: CHILD,
            component_path: "Comp/inner",
            name: "inner",
            resolved: LEAF,
        },
        Row {
            // TWO hops: inheritance is transitive, as it is for every other
            // part of a component type's schema.
            case: "twoHopsInherited",
            component_type_id: GRANDCHILD,
            component_path: "Comp/inner",
            name: "inner",
            resolved: LEAF,
        },
        Row {
            // The nearer declaration REFINES the inherited one, so the nearest
            // id that is named is the one instantiated.
            case: "refinedByTheNearerDeclaration",
            component_type_id: REFINER,
            component_path: "Comp/refined",
            name: "refined",
            resolved: OTHER_LEAF,
        },
        Row {
            // The nearer declaration names no id at all, so the inherited one
            // supplies it.
            case: "nearerDeclarationNamesNoId",
            component_type_id: PARTIAL,
            component_path: "Comp/inner",
            name: "inner",
            resolved: LEAF,
        },
        Row {
            // An inherited declaration at EACH depth: `nested` is inherited
            // from the holding type's parent and reaches a type that inherits
            // `leaf` from ITS parent.
            case: "inheritedAtEveryDepth",
            component_type_id: CHILD,
            component_path: "Comp/nested/leaf",
            name: "leaf",
            resolved: LEAF,
        },
    ]
}

/// What a read must report for a composite component the writer stood up.
fn assert_reported(case: &str, read: &Resp, row: &Row, description: Option<&str>) {
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetEntity, got {read:?}"
    );
    let reported = &read.member("components")["Comp"]["compositeComponents"][row.name];
    assert!(
        reported.is_object(),
        "case {case:?}: expected compositeComponents.{} on Comp, got {:?}",
        row.name,
        read.member("components")["Comp"]
    );
    assert_eq!(
        reported["componentPath"].as_str(),
        Some(row.component_path),
        "case {case:?}: wrong componentPath, got {reported:?}"
    );
    assert_eq!(
        reported["componentName"].as_str(),
        Some(row.name),
        "case {case:?}: wrong componentName, got {reported:?}"
    );
    assert_eq!(
        reported["componentTypeId"].as_str(),
        Some(row.resolved),
        "case {case:?}: wrong componentTypeId, got {reported:?}"
    );
    assert_eq!(
        reported["description"].as_str(),
        description,
        "case {case:?}: wrong description, got {reported:?}"
    );
}

/// RULE: `CreateEntity` accepts a `compositeComponents` path whose segments
/// resolve against the effective schema of the component types they walk
/// through, and stores the composite component as an instance of the type the
/// resolution landed on.
#[tokio::test]
async fn create_entity_resolves_a_path_through_an_inherited_declaration() {
    let emulator = seeded().await;

    for row in rows() {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": row.case,
                    "entityName": row.case,
                    "components": {"Comp": {"componentTypeId": row.component_type_id}},
                    "compositeComponents": {row.component_path: {"description": "created"}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateEntity, got {created:?}",
            row.case
        );

        let read = emulator.get_entity(WORKSPACE, row.case).await;
        assert_reported(row.case, &read, &row, Some("created"));
    }
}

/// RULE (the same rule through the other writer): `UpdateEntity` creates a
/// composite component at a path whose segments resolve against the effective
/// schema of the component types they walk through.
#[tokio::test]
async fn update_entity_resolves_a_path_through_an_inherited_declaration() {
    let emulator = seeded().await;

    for row in rows() {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": row.case,
                    "entityName": row.case,
                    "components": {"Comp": {"componentTypeId": row.component_type_id}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateEntity, got {created:?}",
            row.case
        );

        let updated = emulator
            .update_entity(
                WORKSPACE,
                row.case,
                json!({"compositeComponentUpdates": {row.component_path: {
                    "updateType": "CREATE",
                    "description": "updated",
                }}}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateEntity, got {updated:?}",
            row.case
        );

        let read = emulator.get_entity(WORKSPACE, row.case).await;
        assert_reported(row.case, &read, &row, Some("updated"));
    }
}

/// The writer and the reader of one component type's schema agree: the
/// declaration each accepted path resolved through is the very one
/// `GetComponentType` publishes for the holding component type, with the id the
/// resolution landed on.
#[tokio::test]
async fn the_resolved_declaration_is_the_one_the_component_type_reader_publishes() {
    let emulator = seeded().await;

    for row in rows() {
        // The first segment BELOW the component: the entry the holding
        // component type must publish.
        let name = row.component_path.split('/').nth(1).unwrap_or_else(|| {
            panic!(
                "case {:?}: the path has a segment below its component",
                row.case
            )
        });
        let read = emulator
            .get_component_type(WORKSPACE, row.component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetComponentType, got {read:?}",
            row.case
        );
        let published = &read.member("compositeComponentTypes")[name];
        assert!(
            published.is_object(),
            "case {:?}: expected compositeComponentTypes.{name} on {}, got {:?}",
            row.case,
            row.component_type_id,
            read.member("compositeComponentTypes")
        );
        // For a one-segment-deep row this is the type the composite component
        // instantiates; for a deeper one it is the type the NEXT segment is
        // resolved against.
        let expected = if row.component_path.matches('/').count() == 1 {
            row.resolved
        } else {
            DEEP_CHILD
        };
        assert_eq!(
            published["componentTypeId"].as_str(),
            Some(expected),
            "case {:?}: the reader must publish the id the path resolved to, got {published:?}",
            row.case
        );
    }
}

/// RULE (the boundary): a segment that NO declaration on the whole chain
/// supplies is still a 400 `ValidationException`, and nothing is stored.
///
/// The rows differ along which step of the resolution finds nothing: no
/// declaration anywhere on the chain, a declaration that names no component
/// type to instantiate, and a path one segment deeper than the chain declares.
#[tokio::test]
async fn a_path_no_declaration_on_the_chain_supplies_is_refused() {
    let emulator = seeded().await;

    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            // Neither the type nor its parent declares the segment.
            "nothingOnTheChain",
            PLAIN_CHILD,
            "Comp/inner",
            "declares no compositeComponentTypes.inner",
        ),
        (
            // The only declaration that reaches the segment — an inherited one
            // — names no componentTypeId.
            "inheritedDeclarationWithoutId",
            CHILD,
            "Comp/anon",
            "declares compositeComponentTypes.anon with no componentTypeId",
        ),
        (
            // One segment too deep: the inherited declaration reaches a type
            // whose own chain declares no composite component type.
            "deeperThanTheChainDeclares",
            CHILD,
            "Comp/inner/leaf",
            "declares no compositeComponentTypes.leaf",
        ),
    ];

    for (case, component_type_id, component_path, fragment) in rows {
        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": case,
                    "entityName": case,
                    "components": {"Comp": {"componentTypeId": component_type_id}},
                    "compositeComponents": {component_path: {}},
                }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from CreateEntity, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected a ValidationException, got {refused:?}"
        );
        let message = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(fragment),
            "case {case:?}: expected a message naming {fragment:?}, got {message:?}"
        );
        let read = emulator.get_entity(WORKSPACE, case).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must store no entity, got {read:?}"
        );

        // The same refusal reaches the other writer, which shares the rule.
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": format!("{case}Update"),
                    "entityName": case,
                    "components": {"Comp": {"componentTypeId": component_type_id}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );
        let refused = emulator
            .update_entity(
                WORKSPACE,
                &format!("{case}Update"),
                json!({"compositeComponentUpdates": {component_path: {"updateType": "CREATE"}}}),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400 from UpdateEntity, got {refused:?}"
        );
        let message = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(fragment),
            "case {case:?}: expected a message naming {fragment:?}, got {message:?}"
        );
        let read = emulator
            .get_entity(WORKSPACE, &format!("{case}Update"))
            .await;
        assert_eq!(
            read.member("components")["Comp"]["compositeComponents"]
                .as_object()
                .map(|map| map.len())
                .unwrap_or_default(),
            0,
            "case {case:?}: a refused update must store no composite component, got {read:?}"
        );
    }
}
