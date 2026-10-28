// SPDX-License-Identifier: CC0-1.0

//! A restated property group REFINES the inherited one, end to end over HTTP.
//!
//! RULE under test: a property group is resolved over every declaration that
//! reaches it — the entity component's own, then the component type it
//! instantiates and the types that type extends, nearest first — and each member
//! takes the value of the NEAREST declaration that sets it, both members of a
//! group request being optional. So a declaration that restates only `groupType`
//! keeps the `propertyNames` it inherits, and one that restates only
//! `propertyNames` keeps the inherited `groupType`; the members no declaration
//! sets fall back to what the response requires (`groupType` to the only value
//! the model's `GroupType` enum admits, `propertyNames` to the empty list). Every
//! reader of a group resolves it alike: `GetEntity`, `ListComponents`,
//! `GetComponentType` and `GetPropertyValue`'s `propertyGroupName`.
//!
//! Member optionality is the model's:
//! smithy_query(query="shape", shape="PropertyGroupRequest,ComponentPropertyGroupRequest")
//! — neither `groupType` nor `propertyNames` is required on either request shape,
//! while both are required on `PropertyGroupResponse` /
//! `ComponentPropertyGroupResponse`.
//!
//! The rows differ along what the rule generalises over: WHICH members the
//! nearer declaration restates (only the type, only the names, both, neither),
//! HOW FAR away the declaration that carries the names is (the component's own,
//! the type it instantiates, a type that type extends), and whether any
//! declaration carries them at all.

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below reads from.
const WORKSPACE: &str = "propgroups";

/// The component type at the root of the chain: three property definitions and
/// one property group naming two of them.
const BASE: &str = "com.example.base";
/// A component type that extends [`BASE`] and RESTATES its group carrying only
/// `groupType`, so the names a reader reports for it have to come from two
/// declarations away.
const DERIVED: &str = "com.example.derived";

/// The one component name every case declares, so every assertion addresses the
/// same component whatever the row put in it.
const COMPONENT: &str = "Spec";

/// The group every case resolves.
const GROUP: &str = "Readings";

/// Create the workspace and the two component types the cases instantiate.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    // In reference order: `extendsFrom` has to name a component type the
    // workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    "Serial": {"dataType": {"type": "STRING"}, "defaultValue": {"stringValue": "s-1"}},
                    "Tag": {"dataType": {"type": "STRING"}, "defaultValue": {"stringValue": "t-1"}},
                    "Pressure": {"dataType": {"type": "DOUBLE"}, "defaultValue": {"doubleValue": 2.5}},
                },
                "propertyGroups": {GROUP: {"groupType": "TABULAR", "propertyNames": ["Serial", "Tag"]}},
            }),
        ),
        (
            DERIVED,
            json!({
                "extendsFrom": [BASE],
                "propertyGroups": {GROUP: {"groupType": "TABULAR"}},
            }),
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

/// One row of the rule: the component an entity's create carries, and the group
/// every reader has to report for it.
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The single component the create declares, under the name [`COMPONENT`].
    component: Value,
    /// The group name the row resolves.
    group_name: &'static str,
    /// The `groupType` the reader has to report.
    group_type: &'static str,
    /// The `propertyNames` the reader has to report, in order.
    property_names: Vec<&'static str>,
    /// Whether the reported group is inherited: a group the component declares
    /// itself is its own, even where it only refines the inherited one.
    is_inherited: bool,
}

/// Every row of the rule.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "restatingOnlyTheGroupTypeKeepsTheInheritedNames",
            entity_id: "only-group-type",
            component: json!({
                "componentTypeId": BASE,
                "propertyGroups": {GROUP: {"groupType": "TABULAR"}},
            }),
            group_name: GROUP,
            group_type: "TABULAR",
            property_names: vec!["Serial", "Tag"],
            is_inherited: false,
        },
        Case {
            what: "restatingOnlyTheNamesSetsThemAndKeepsTheInheritedGroupType",
            entity_id: "only-names",
            component: json!({
                "componentTypeId": BASE,
                "propertyGroups": {GROUP: {"propertyNames": ["Pressure"]}},
            }),
            group_name: GROUP,
            group_type: "TABULAR",
            property_names: vec!["Pressure"],
            is_inherited: false,
        },
        Case {
            what: "restatingBothMembersReplacesBoth",
            entity_id: "both-members",
            component: json!({
                "componentTypeId": BASE,
                "propertyGroups": {GROUP: {"groupType": "TABULAR", "propertyNames": ["Pressure", "Serial"]}},
            }),
            group_name: GROUP,
            group_type: "TABULAR",
            property_names: vec!["Pressure", "Serial"],
            is_inherited: false,
        },
        Case {
            what: "restatingNothingReportsTheTypesGroupWhole",
            entity_id: "no-restatement",
            component: json!({"componentTypeId": BASE}),
            group_name: GROUP,
            group_type: "TABULAR",
            property_names: vec!["Serial", "Tag"],
            is_inherited: true,
        },
        Case {
            what: "theNamesComeFromTheNearestDeclarationThatCarriesAnyAlongTheChain",
            entity_id: "chain-restatement",
            component: json!({
                "componentTypeId": DERIVED,
                "propertyGroups": {GROUP: {"groupType": "TABULAR"}},
            }),
            group_name: GROUP,
            group_type: "TABULAR",
            property_names: vec!["Serial", "Tag"],
            is_inherited: false,
        },
        Case {
            what: "aTypeThatRestatesOnlyTheGroupTypeStillGivesItsInstanceTheNames",
            entity_id: "chain-inherited",
            component: json!({"componentTypeId": DERIVED}),
            group_name: GROUP,
            group_type: "TABULAR",
            property_names: vec!["Serial", "Tag"],
            is_inherited: true,
        },
        Case {
            what: "aGroupOnlyTheComponentDeclaresTakesItsOwnNames",
            entity_id: "own-group",
            component: json!({
                "componentTypeId": BASE,
                "propertyGroups": {"Spares": {"propertyNames": ["Pressure"]}},
            }),
            group_name: "Spares",
            group_type: "TABULAR",
            property_names: vec!["Pressure"],
            is_inherited: false,
        },
        Case {
            what: "aGroupNoDeclarationGivesNamesResolvesToNone",
            entity_id: "nameless-group",
            component: json!({
                "componentTypeId": BASE,
                "propertyGroups": {"Loose": {"groupType": "TABULAR"}},
            }),
            group_name: "Loose",
            group_type: "TABULAR",
            property_names: vec![],
            is_inherited: false,
        },
    ]
}

/// Create the case's entity and answer the component `GetEntity` reports for it.
async fn created_component(emulator: &Emulator, case: &Case) -> Value {
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": case.entity_id,
                "entityName": case.entity_id,
                "components": {COMPONENT: case.component.clone()},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected 200 from CreateEntity, got {created:?}",
        case.what
    );

    let read = emulator.get_entity(WORKSPACE, case.entity_id).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected 200 from GetEntity, got {read:?}",
        case.what
    );
    read.member("components")[COMPONENT].clone()
}

/// The group a reader reported, as the triple the rule is about.
fn reported(group: &Value) -> (Option<&str>, Vec<Value>, Option<bool>) {
    (
        group["groupType"].as_str(),
        group["propertyNames"].as_array().cloned().unwrap_or_default(),
        group["isInherited"].as_bool(),
    )
}

/// The triple the case expects.
fn wanted(case: &Case) -> (Option<&str>, Vec<Value>, Option<bool>) {
    (
        Some(case.group_type),
        case.property_names.iter().map(|name| json!(name)).collect(),
        Some(case.is_inherited),
    )
}

/// RULE: `GetEntity` reports each member of a component's property group as the
/// nearest declaration that SETS it gives it, so a restatement that carries no
/// `propertyNames` keeps the inherited ones.
#[tokio::test]
async fn a_restated_group_keeps_the_members_the_restatement_left_alone() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let component = created_component(&emulator, &case).await;
        let group = &component["propertyGroups"][case.group_name];
        assert_eq!(
            reported(group),
            wanted(&case),
            "case {:?}: expected the folded group, got {group:?}",
            case.what
        );
    }
}

/// RULE (one group, one answer): `ListComponents` resolves a component's groups
/// the same way `GetEntity` does, so the two readers cannot disagree about the
/// members a restated group ends up with.
#[tokio::test]
async fn every_reader_of_a_component_resolves_its_groups_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let component = created_component(&emulator, &case).await;

        let listed = emulator
            .list_components(WORKSPACE, case.entity_id, json!({}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListComponents, got {listed:?}",
            case.what
        );
        let summary = listed
            .member("componentSummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["componentName"] == COMPONENT)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the {COMPONENT} summary, got {listed:?}",
                    case.what
                )
            });

        assert_eq!(
            reported(&summary["propertyGroups"][case.group_name]),
            reported(&component["propertyGroups"][case.group_name]),
            "case {:?}: expected ListComponents and GetEntity to report one group, got \
             {summary:?}",
            case.what
        );
        assert_eq!(
            reported(&summary["propertyGroups"][case.group_name]),
            wanted(&case),
            "case {:?}: expected the folded group from ListComponents, got {summary:?}",
            case.what
        );
    }
}

/// RULE: `GetComponentType` folds a component TYPE's declarations the same way,
/// so a type that restates an inherited group carrying only `groupType` still
/// reports the names its ancestor declared.
#[tokio::test]
async fn a_component_type_that_restates_a_group_keeps_the_inherited_names() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: Vec<(&str, &str, Vec<&str>, bool)> = vec![
        ("theDeclaringType", BASE, vec!["Serial", "Tag"], false),
        ("theRestatingChild", DERIVED, vec!["Serial", "Tag"], true),
    ];
    for (what, component_type_id, property_names, is_inherited) in rows {
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {what:?}: expected 200 from GetComponentType, got {read:?}"
        );
        let group = &read.member("propertyGroups")[GROUP];
        assert_eq!(
            reported(group),
            (
                Some("TABULAR"),
                property_names.iter().map(|name| json!(name)).collect(),
                Some(is_inherited),
            ),
            "case {what:?}: expected the folded group, got {group:?}"
        );
    }
}

/// RULE: `GetPropertyValue` resolves `propertyGroupName` through the same fold,
/// so every property the resolved group names is readable through it and only a
/// property outside the resolved group is refused.
#[tokio::test]
async fn a_property_group_read_sees_the_folded_names() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        created_component(&emulator, &case).await;

        for property_name in &case.property_names {
            let read = emulator
                .get_property_value(
                    WORKSPACE,
                    json!({
                        "entityId": case.entity_id,
                        "componentName": COMPONENT,
                        "propertyGroupName": case.group_name,
                        "selectedProperties": [property_name],
                    }),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {:?} property {property_name:?}: expected 200 for a property the folded \
                 group names, got {read:?}",
                case.what
            );
            assert!(
                read.member("propertyValues")[property_name]["propertyValue"].is_object(),
                "case {:?} property {property_name:?}: expected the property's value, got {read:?}",
                case.what
            );
        }

        // The boundary the rule sits on: a property of the component that the
        // RESOLVED group does not name is still outside it.
        let outside = ["Serial", "Tag", "Pressure"]
            .into_iter()
            .find(|name| !case.property_names.contains(name))
            .expect("every row leaves one of the three properties out of its group");
        let refused = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "componentName": COMPONENT,
                    "propertyGroupName": case.group_name,
                    "selectedProperties": [outside],
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?} property {outside:?}: expected a selection outside the folded group to be \
             refused, got {refused:?}",
            case.what
        );
    }
}

/// RULE: the component-TYPE branch of `GetPropertyValue` folds the chain too, so
/// a group a type restates carrying only `groupType` still names the properties
/// a tabular query may select.
#[tokio::test]
async fn a_component_type_tabular_read_sees_the_folded_names() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let rows: Vec<(&str, &str, &str, u16)> = vec![
        ("aNameTheDeclaringTypeGroups", BASE, "Serial", 200),
        ("aNameTheRestatingChildInherits", DERIVED, "Serial", 200),
        ("aNameNoDeclarationGroups", DERIVED, "Pressure", 400),
    ];
    for (what, component_type_id, property_name, status) in rows {
        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "componentTypeId": component_type_id,
                    "propertyGroupName": GROUP,
                    "selectedProperties": [property_name],
                    "tabularConditions": {},
                }),
            )
            .await;
        assert_eq!(
            read.status, status,
            "case {what:?} property {property_name:?}: expected {status}, got {read:?}"
        );
    }
}
