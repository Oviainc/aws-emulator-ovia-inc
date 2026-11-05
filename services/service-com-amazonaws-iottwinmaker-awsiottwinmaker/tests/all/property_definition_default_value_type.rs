// SPDX-License-Identifier: CC0-1.0

//! The data type a property definition's `defaultValue` has to carry, over
//! HTTP.
//!
//! RULE under test: a `defaultValue` a write stores on a property definition is
//! a value of the property, because a read reports it AS the property's value
//! for as long as nothing has been written to the property, so it carries a
//! value of the `dataType` that definition RESOLVES to — its own, or the one it
//! inherits from the component types it extends or from the component type its
//! holder instantiates. A default of any other type is refused with HTTP 400
//! `ValidationException` and nothing is stored, so no read can report a value
//! beside a `dataType` it contradicts. A default naming no typed value at all
//! contradicts no `dataType` and is stored.
//!
//! The rows span what the rule generalises over:
//!
//! * every member of the modelled `Type` value list
//!   (`smithy_query(query="shape",shape="Type")`), each paired with a value of
//!   that type and a value of another — the `DataValue` member each one travels
//!   in comes from `smithy_query(query="shape",shape="DataValue")`;
//! * every writer of a property definition: `CreateComponentType`,
//!   `UpdateComponentType`, `CreateEntity` and `UpdateEntity`;
//! * where the `dataType` the default is judged against comes from: the
//!   declaration itself, the `extendsFrom` chain of a component type, and the
//!   component type a component instantiates — including the two patches that
//!   take an inherited `dataType` AWAY from an already stored default
//!   (`extendsFrom` re-pointed, `componentTypeId` re-pointed).
//!
//! Every accepted default is read back — through `GetComponentType` for a
//! component type's own definition and through `GetEntity` for the value a
//! component's property reports — so each case proves what the store holds
//! rather than what one response said.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case works in. Each test builds its own emulator, so the
/// id is shared without the cases sharing state.
const WORKSPACE: &str = "defaultValueTypeWs";

/// The property every case declares a default for.
const PROPERTY: &str = "Reading";

/// The component every entity case's properties hang off.
const COMPONENT: &str = "Sensor";

/// The entity a `RELATIONSHIP` value points at, so such a value names an entity
/// of the workspace.
const TARGET_ENTITY: &str = "target";

/// One modelled `Type`, with a default value OF that type and one of another.
struct TypeCase {
    /// The `dataType.type` the property is declared with.
    declared: &'static str,
    /// A `defaultValue` carrying a value of `declared`.
    matching: Value,
    /// A `defaultValue` carrying a value of some OTHER data type.
    mismatching: Value,
    /// The `Type` `mismatching` carries: what a read would report the mistyped
    /// default as, and so what the refusal names beside `declared`.
    mismatching_type: &'static str,
}

/// Every modelled `Type`, paired with a default of that type and a default of
/// another.
///
/// The value list is the modelled `Type`'s
/// (`smithy_query(query="shape",shape="Type")`) and the member each type's value
/// travels in is `DataValue`'s
/// (`smithy_query(query="shape",shape="DataValue")`), so every type this service
/// declares a property with has a row here rather than only the one an issue
/// happened to name.
fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            declared: "STRING",
            matching: json!({"stringValue": "unset"}),
            mismatching: json!({"integerValue": 5}),
            mismatching_type: "INTEGER",
        },
        TypeCase {
            declared: "DOUBLE",
            matching: json!({"doubleValue": 1.5}),
            mismatching: json!({"booleanValue": true}),
            mismatching_type: "BOOLEAN",
        },
        TypeCase {
            // INTEGER and LONG are distinct members of the value list, so each
            // is the other's mistyped default.
            declared: "INTEGER",
            matching: json!({"integerValue": 7}),
            mismatching: json!({"longValue": 7}),
            mismatching_type: "LONG",
        },
        TypeCase {
            declared: "LONG",
            matching: json!({"longValue": 4294967296i64}),
            mismatching: json!({"integerValue": 7}),
            mismatching_type: "INTEGER",
        },
        TypeCase {
            declared: "BOOLEAN",
            matching: json!({"booleanValue": true}),
            mismatching: json!({"stringValue": "true"}),
            mismatching_type: "STRING",
        },
        TypeCase {
            declared: "LIST",
            matching: json!({"listValue": [{"stringValue": "a"}, {"stringValue": "b"}]}),
            mismatching: json!({"mapValue": {"a": {"stringValue": "a"}}}),
            mismatching_type: "MAP",
        },
        TypeCase {
            declared: "MAP",
            matching: json!({"mapValue": {"a": {"stringValue": "a"}}}),
            mismatching: json!({"listValue": [{"stringValue": "a"}]}),
            mismatching_type: "LIST",
        },
        TypeCase {
            declared: "RELATIONSHIP",
            matching: json!({"relationshipValue": {
                "targetEntityId": TARGET_ENTITY,
                "targetComponentName": COMPONENT,
            }}),
            mismatching: json!({"stringValue": TARGET_ENTITY}),
            mismatching_type: "STRING",
        },
    ]
}

/// A component type id for one row of one case: the ids are per-row so a
/// refused create and an accepted one never collide.
fn type_id(role: &str, declared: &str) -> String {
    format!("com.example.{role}.{}", declared.to_lowercase())
}

/// An entity id for one row of one case.
fn entity_id(role: &str, declared: &str) -> String {
    format!("{role}-{}", declared.to_lowercase())
}

/// The `message` an error body carries.
fn message(response: &Resp) -> String {
    response
        .member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// An emulator holding the workspace every case works in and the entity a
/// `RELATIONSHIP` value points at.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace {WORKSPACE:?}: expected 200, got {created:?}"
    );
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({"entityId": TARGET_ENTITY, "entityName": TARGET_ENTITY}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity {TARGET_ENTITY:?}: expected 200, got {created:?}"
    );
    emulator
}

/// `CreateComponentType` with one property definition.
async fn create_type(
    emulator: &Emulator,
    component_type_id: &str,
    definition: Value,
    extends_from: Option<&str>,
) -> Resp {
    let mut body = json!({"propertyDefinitions": {PROPERTY: definition}});
    if let Some(parent) = extends_from {
        body["extendsFrom"] = json!([parent]);
    }
    emulator
        .create_component_type(WORKSPACE, component_type_id, body)
        .await
}

/// The definition `GetComponentType` reports for [`PROPERTY`].
async fn reported_definition(emulator: &Emulator, component_type_id: &str) -> Value {
    let read = emulator
        .get_component_type(WORKSPACE, component_type_id)
        .await;
    assert_eq!(
        read.status, 200,
        "GetComponentType {component_type_id:?}: expected 200, got {read:?}"
    );
    read.member("propertyDefinitions")[PROPERTY].clone()
}

/// The `PropertyResponse` `GetEntity` reports for one property of the entity's
/// [`COMPONENT`] — the read that puts the property's `value` and its
/// `definition.dataType` in one payload.
async fn reported_property(emulator: &Emulator, entity_id: &str, property_name: &str) -> Value {
    let read = emulator.get_entity(WORKSPACE, entity_id).await;
    assert_eq!(
        read.status, 200,
        "GetEntity {entity_id:?}: expected 200, got {read:?}"
    );
    read.body
        .pointer(&format!(
            "/components/{COMPONENT}/properties/{property_name}"
        ))
        .cloned()
        .unwrap_or_default()
}

/// Assert one refusal: HTTP 400 `ValidationException` whose message names the
/// property and both types it could not reconcile.
fn assert_mistyped_default_refusal(case: &str, refused: &Resp, declared: &str, carried: &str) {
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: a default of another type is refused, got {refused:?}"
    );
    let reported = message(refused);
    for named in [PROPERTY, declared, carried] {
        assert!(
            reported.contains(named),
            "case {case:?}: the refusal must name {named:?}, got {reported:?}"
        );
    }
}

/// RULE: a component type's own property definition carries a `defaultValue` of
/// the `dataType` it declares — the default is then reported by
/// `GetComponentType` and read AS the property's value by `GetEntity` — and one
/// of another type is refused with nothing stored.
#[tokio::test]
async fn a_component_types_default_value_carries_the_data_type_it_declares() {
    let emulator = seeded().await;

    for case in type_cases() {
        let declared = case.declared;
        let component_type_id = type_id("own", declared);
        let created = create_type(
            &emulator,
            &component_type_id,
            json!({"dataType": {"type": declared}, "defaultValue": case.matching}),
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: a default of the declared type is stored, got {created:?}"
        );
        let reported = reported_definition(&emulator, &component_type_id).await;
        assert_eq!(
            reported["defaultValue"], case.matching,
            "case {declared:?}: expected the created defaultValue, got {reported:?}"
        );
        assert_eq!(
            reported["dataType"]["type"],
            json!(declared),
            "case {declared:?}: expected the created dataType, got {reported:?}"
        );

        // The read side the default reaches: a component of the type reports
        // the default as the property's VALUE, beside that same `dataType`.
        let holder = entity_id("holds", declared);
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": holder,
                    "entityName": holder,
                    "components": {COMPONENT: {"componentTypeId": component_type_id}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: CreateEntity {holder:?} expected 200, got {created:?}"
        );
        let property = reported_property(&emulator, &holder, PROPERTY).await;
        assert_eq!(
            property["value"], case.matching,
            "case {declared:?}: GetEntity reports the default as the property's value, got \
             {property:?}"
        );
        assert_eq!(
            property["definition"]["dataType"]["type"],
            json!(declared),
            "case {declared:?}: the reported value stands beside the declared dataType, got \
             {property:?}"
        );

        // The mistyped default: refused, and the component type is not stored.
        let refused_id = type_id("mistyped", declared);
        let refused = create_type(
            &emulator,
            &refused_id,
            json!({"dataType": {"type": declared}, "defaultValue": case.mismatching}),
            None,
        )
        .await;
        assert_mistyped_default_refusal(declared, &refused, declared, case.mismatching_type);
        let read = emulator.get_component_type(WORKSPACE, &refused_id).await;
        assert_eq!(
            read.status, 404,
            "case {declared:?}: a refused create stores nothing, got {read:?}"
        );
    }
}

/// RULE, where the `dataType` is INHERITED: a definition that only REFINES one
/// its `extendsFrom` chain declares is judged against the inherited `dataType`,
/// so a default of that type is stored and a default of another is refused.
#[tokio::test]
async fn an_inherited_data_type_constrains_a_definition_that_only_refines_it() {
    let emulator = seeded().await;

    for case in type_cases() {
        let declared = case.declared;
        let base = type_id("base", declared);
        let created = create_type(
            &emulator,
            &base,
            json!({"dataType": {"type": declared}}),
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: CreateComponentType {base:?} expected 200, got {created:?}"
        );

        // A refinement carrying nothing but a default of the inherited type.
        let refining = type_id("refines", declared);
        let created = create_type(
            &emulator,
            &refining,
            json!({"defaultValue": case.matching}),
            Some(&base),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: a default of the INHERITED type is stored, got {created:?}"
        );
        let reported = reported_definition(&emulator, &refining).await;
        assert_eq!(
            reported["defaultValue"], case.matching,
            "case {declared:?}: expected the refined defaultValue, got {reported:?}"
        );
        assert_eq!(
            reported["dataType"]["type"],
            json!(declared),
            "case {declared:?}: expected the inherited dataType, got {reported:?}"
        );

        // The same refinement with a default of another type.
        let refused_id = type_id("refines-mistyped", declared);
        let refused = create_type(
            &emulator,
            &refused_id,
            json!({"defaultValue": case.mismatching}),
            Some(&base),
        )
        .await;
        assert_mistyped_default_refusal(declared, &refused, declared, case.mismatching_type);
        let read = emulator.get_component_type(WORKSPACE, &refused_id).await;
        assert_eq!(
            read.status, 404,
            "case {declared:?}: a refused create stores nothing, got {read:?}"
        );
    }
}

/// RULE, under `UpdateComponentType`: the patched record is held to the same
/// rule — a patch carrying a mistyped default is refused, and so is one that
/// re-points `extendsFrom` away from the `dataType` an already stored default
/// was written against; a refused patch leaves the stored definition as it was.
#[tokio::test]
async fn an_update_holds_the_patched_component_type_to_the_same_rule() {
    let emulator = seeded().await;

    for case in type_cases() {
        let declared = case.declared;
        let patched = type_id("patched", declared);
        let created = create_type(
            &emulator,
            &patched,
            json!({"dataType": {"type": declared}, "defaultValue": case.matching}),
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: CreateComponentType {patched:?} expected 200, got {created:?}"
        );

        let refused = emulator
            .update_component_type(
                WORKSPACE,
                &patched,
                json!({"propertyDefinitions": {PROPERTY: {
                    "dataType": {"type": declared},
                    "defaultValue": case.mismatching,
                }}}),
            )
            .await;
        assert_mistyped_default_refusal(declared, &refused, declared, case.mismatching_type);
        let reported = reported_definition(&emulator, &patched).await;
        assert_eq!(
            reported["defaultValue"], case.matching,
            "case {declared:?}: a refused patch stores nothing, so the created defaultValue \
             stands, got {reported:?}"
        );

        // The same patch with a default of the declared type is applied.
        let accepted = emulator
            .update_component_type(
                WORKSPACE,
                &patched,
                json!({"propertyDefinitions": {PROPERTY: {
                    "dataType": {"type": declared},
                    "defaultValue": case.matching,
                    "displayName": "Reading (patched)",
                }}}),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {declared:?}: a patch whose default matches is applied, got {accepted:?}"
        );
        let reported = reported_definition(&emulator, &patched).await;
        assert_eq!(
            reported["displayName"],
            json!("Reading (patched)"),
            "case {declared:?}: expected the patched displayName, got {reported:?}"
        );
        assert_eq!(
            reported["defaultValue"], case.matching,
            "case {declared:?}: expected the patched defaultValue, got {reported:?}"
        );

        // The patch that takes the INHERITED `dataType` away: the refinement
        // below declares nothing but a default, so re-pointing `extendsFrom` at
        // a type that declares the property with another `dataType` is the call
        // that would leave the stored default contradicting it.
        let from = type_id("rebase-from", declared);
        let onto = type_id("rebase-onto", declared);
        for (parent, parent_type) in [(&from, declared), (&onto, case.mismatching_type)] {
            let created = create_type(
                &emulator,
                parent,
                json!({"dataType": {"type": parent_type}}),
                None,
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {declared:?}: CreateComponentType {parent:?} expected 200, got {created:?}"
            );
        }
        let rebased = type_id("rebased", declared);
        let created = create_type(
            &emulator,
            &rebased,
            json!({"defaultValue": case.matching}),
            Some(&from),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: CreateComponentType {rebased:?} expected 200, got {created:?}"
        );
        let refused = emulator
            .update_component_type(WORKSPACE, &rebased, json!({"extendsFrom": [onto]}))
            .await;
        assert_mistyped_default_refusal(declared, &refused, case.mismatching_type, declared);
        let reported = reported_definition(&emulator, &rebased).await;
        assert_eq!(
            reported["dataType"]["type"],
            json!(declared),
            "case {declared:?}: a refused re-point leaves the inheritance as it was, got \
             {reported:?}"
        );
        assert_eq!(
            reported["defaultValue"], case.matching,
            "case {declared:?}: a refused re-point stores nothing, got {reported:?}"
        );
    }
}

/// RULE, where the definition hangs off a COMPONENT: a default a component's own
/// declaration carries is judged against the `dataType` the property resolves to
/// through the component type the component instantiates, by `CreateEntity` and
/// by `UpdateEntity` alike.
#[tokio::test]
async fn a_components_declared_default_carries_the_data_type_the_property_resolves_to() {
    let emulator = seeded().await;

    for case in type_cases() {
        let declared = case.declared;
        let component_type_id = type_id("holder-type", declared);
        let created = create_type(
            &emulator,
            &component_type_id,
            json!({"dataType": {"type": declared}}),
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: CreateComponentType {component_type_id:?} expected 200, got \
             {created:?}"
        );

        // CreateEntity: the mistyped default is refused and no entity is stored.
        let refused_entity = entity_id("create-refused", declared);
        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": refused_entity,
                    "entityName": refused_entity,
                    "components": {COMPONENT: {
                        "componentTypeId": component_type_id,
                        "properties": {PROPERTY: {
                            "definition": {"defaultValue": case.mismatching},
                        }},
                    }},
                }),
            )
            .await;
        assert_mistyped_default_refusal(declared, &refused, declared, case.mismatching_type);
        let read = emulator.get_entity(WORKSPACE, &refused_entity).await;
        assert_eq!(
            read.status, 404,
            "case {declared:?}: a refused create stores no entity, got {read:?}"
        );

        // The same create with a default of the inherited type: stored, and the
        // read reports it as the property's value.
        let holder = entity_id("create-accepted", declared);
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": holder,
                    "entityName": holder,
                    "components": {COMPONENT: {
                        "componentTypeId": component_type_id,
                        "properties": {PROPERTY: {
                            "definition": {"defaultValue": case.matching},
                        }},
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: a default of the inherited type is stored, got {created:?}"
        );
        let property = reported_property(&emulator, &holder, PROPERTY).await;
        assert_eq!(
            property["value"], case.matching,
            "case {declared:?}: GetEntity reports the declared default as the value, got \
             {property:?}"
        );

        // UpdateEntity: a `propertyUpdates` definition is held to the same rule,
        // and a refused patch leaves the stored default in place.
        let refused = emulator
            .update_entity(
                WORKSPACE,
                &holder,
                json!({"componentUpdates": {COMPONENT: {"propertyUpdates": {PROPERTY: {
                    "definition": {
                        "dataType": {"type": declared},
                        "defaultValue": case.mismatching,
                    },
                }}}}}),
            )
            .await;
        assert_mistyped_default_refusal(declared, &refused, declared, case.mismatching_type);
        let property = reported_property(&emulator, &holder, PROPERTY).await;
        assert_eq!(
            property["value"], case.matching,
            "case {declared:?}: a refused patch stores nothing, got {property:?}"
        );
    }
}

/// RULE, where the patch takes the inherited `dataType` away: re-pointing a
/// component's `componentTypeId` is judged against every definition the
/// component HOLDS, so a re-point that would leave a stored default contradicting
/// the newly inherited `dataType` is refused and the component is untouched.
#[tokio::test]
async fn repointing_a_components_component_type_holds_its_stored_defaults_to_the_rule() {
    let emulator = seeded().await;

    for case in type_cases() {
        let declared = case.declared;
        let from = type_id("component-from", declared);
        let onto = type_id("component-onto", declared);
        for (parent, parent_type) in [(&from, declared), (&onto, case.mismatching_type)] {
            let created = create_type(
                &emulator,
                parent,
                json!({"dataType": {"type": parent_type}}),
                None,
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {declared:?}: CreateComponentType {parent:?} expected 200, got {created:?}"
            );
        }

        let holder = entity_id("repoint", declared);
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": holder,
                    "entityName": holder,
                    "components": {COMPONENT: {
                        "componentTypeId": from,
                        // A declaration that REFINES the inherited one: it
                        // carries the default and leaves the `dataType` to the
                        // component type, which is what a re-point moves.
                        "properties": {PROPERTY: {
                            "definition": {"defaultValue": case.matching},
                        }},
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: CreateEntity {holder:?} expected 200, got {created:?}"
        );

        let refused = emulator
            .update_entity(
                WORKSPACE,
                &holder,
                json!({"componentUpdates": {COMPONENT: {
                    "updateType": "UPDATE",
                    "componentTypeId": onto,
                }}}),
            )
            .await;
        assert_mistyped_default_refusal(declared, &refused, case.mismatching_type, declared);
        let property = reported_property(&emulator, &holder, PROPERTY).await;
        assert_eq!(
            property["definition"]["dataType"]["type"],
            json!(declared),
            "case {declared:?}: a refused re-point leaves the component as it was, got \
             {property:?}"
        );
        assert_eq!(
            property["value"], case.matching,
            "case {declared:?}: a refused re-point stores nothing, got {property:?}"
        );
    }
}

/// RULE at its boundary: a `defaultValue` carrying no typed value at all names
/// no data type, so it contradicts no `dataType` and is stored — by every writer
/// of a definition.
#[tokio::test]
async fn a_default_naming_no_typed_value_contradicts_no_data_type() {
    let emulator = seeded().await;

    // Two rows from the catalog, one scalar and one collection, so the boundary
    // is not read off a single data type.
    for declared in ["STRING", "LIST"] {
        let component_type_id = type_id("empty-default", declared);
        let created = create_type(
            &emulator,
            &component_type_id,
            json!({"dataType": {"type": declared}, "defaultValue": {}}),
            None,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: a default naming no typed value is stored, got {created:?}"
        );
        let reported = reported_definition(&emulator, &component_type_id).await;
        assert_eq!(
            reported["dataType"]["type"],
            json!(declared),
            "case {declared:?}: expected the created dataType, got {reported:?}"
        );

        let holder = entity_id("empty-default", declared);
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": holder,
                    "entityName": holder,
                    "components": {COMPONENT: {
                        "componentTypeId": component_type_id,
                        "properties": {PROPERTY: {"definition": {"defaultValue": {}}}},
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {declared:?}: a component may declare the same default, got {created:?}"
        );
    }
}
