// SPDX-License-Identifier: CC0-1.0

//! The data type a value written to a component's property has to carry, over
//! HTTP.
//!
//! RULE: a value a write stores for a property carries a value of the data type
//! that property's RESOLVED definition declares — the declaration the request
//! carries, the one already stored, or the one inherited from the component type
//! the holder instantiates — so a value of any other type, and a value carrying
//! no typed value at all, is refused with a 400 `ValidationException` and
//! nothing is stored. A property no `dataType` reaches is constrained by
//! nothing, so any typed value may be written to it.
//!
//! The rows span the data type declared (every member of the modelled `Type`
//! value list — `smithy_query(query="shape",shape="Type")`), the member the
//! written value travels in, where the declaration comes from (the entry
//! itself, an earlier write, the component type), the `updateType` the entry
//! names (including none at all), and the holder the properties hang off (a
//! component and a composite component), because that is what the rule
//! generalises over.
//!
//! Every accepted write is read back — through `GetEntity` for a component's
//! property and through `ListProperties` for a composite component's, the read
//! those reach — and every refusal is checked against the same reads, so each
//! case proves what the store holds rather than what one response said.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case works in. Each test builds its own emulator, so the
/// id is shared without the cases sharing state.
const WORKSPACE: &str = "propertyValueTypeWs";

/// The component type the cases' components and composite components
/// instantiate. It declares one property per modelled data type (so a value can
/// be written against a declaration the entry itself does not carry) and one
/// composite component type naming itself (so a composite path one level down
/// resolves to the same schema).
const TYPED: &str = "com.example.typed";

/// The component every case's top-level properties hang off.
const COMPONENT: &str = "Sensor";

/// The `componentPath` of the composite component the composite cases address.
const COMPOSITE_PATH: &str = "Sensor/inner";

/// The entity the relationship values point at, so a `RELATIONSHIP` value names
/// an entity of the workspace.
const TARGET_ENTITY: &str = "target";

/// One data type and two values: one of that type, and one of another.
///
/// `declared` is a member of the modelled `Type` value list
/// (`smithy_query(query="shape",shape="Type")`), and every member of that list
/// has a row here.
struct TypeCase {
    /// The `dataType.type` the property is declared with.
    declared: &'static str,
    /// A value carrying a value of `declared`.
    matching: Value,
    /// A value carrying a value of some OTHER data type.
    mismatching: Value,
}

/// Every modelled `Type`, paired with a value of that type and a value of
/// another — the `DataValue` member each type's value travels in comes from
/// `smithy_query(query="shape",shape="DataValue")`.
fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            declared: "STRING",
            matching: json!({"stringValue": "sn-1"}),
            mismatching: json!({"doubleValue": 1.0}),
        },
        TypeCase {
            declared: "DOUBLE",
            matching: json!({"doubleValue": 21.5}),
            mismatching: json!({"booleanValue": true}),
        },
        TypeCase {
            // INTEGER and LONG are distinct members of the value list, so each
            // is the other's mistyped value.
            declared: "INTEGER",
            matching: json!({"integerValue": 7}),
            mismatching: json!({"longValue": 7}),
        },
        TypeCase {
            declared: "LONG",
            matching: json!({"longValue": 4294967296i64}),
            mismatching: json!({"integerValue": 7}),
        },
        TypeCase {
            declared: "BOOLEAN",
            matching: json!({"booleanValue": true}),
            mismatching: json!({"stringValue": "true"}),
        },
        TypeCase {
            declared: "LIST",
            matching: json!({"listValue": [{"stringValue": "a"}, {"stringValue": "b"}]}),
            mismatching: json!({"mapValue": {"a": {"stringValue": "a"}}}),
        },
        TypeCase {
            declared: "MAP",
            matching: json!({"mapValue": {"a": {"stringValue": "a"}}}),
            mismatching: json!({"listValue": [{"stringValue": "a"}]}),
        },
        TypeCase {
            declared: "RELATIONSHIP",
            matching: json!({"relationshipValue": {
                "targetEntityId": TARGET_ENTITY,
                "targetComponentName": COMPONENT,
            }}),
            mismatching: json!({"stringValue": TARGET_ENTITY}),
        },
    ]
}

/// The property of [`TYPED`] declared with `declared`: the declaration a write
/// that carries none of its own INHERITS.
fn inherited_property(declared: &str) -> String {
    format!("Inherited{declared}")
}

/// The message an error body carries.
fn message(resp: &Resp) -> &str {
    resp.member("message").as_str().unwrap_or_default()
}

/// An emulator holding the workspace, the component type the cases instantiate
/// and the entity every case writes to, plus the entity a `RELATIONSHIP` value
/// points at.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case works in must exist"
    );
    let mut property_definitions = serde_json::Map::new();
    for case in type_cases() {
        property_definitions.insert(
            inherited_property(case.declared),
            json!({"dataType": {"type": case.declared}}),
        );
    }
    let created = emulator
        .create_component_type(
            WORKSPACE,
            TYPED,
            json!({
                "propertyDefinitions": Value::Object(property_definitions),
                // A composite component type may name the component type that
                // declares it, which is what gives the composite component the
                // same schema one level down.
                "compositeComponentTypes": {"inner": {"componentTypeId": TYPED}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {TYPED:?}: expected 200, got {created:?}"
    );
    for entity_id in [TARGET_ENTITY, ENTITY] {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {COMPONENT: {"componentTypeId": TYPED}},
                    "compositeComponents": {COMPOSITE_PATH: {}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
    emulator
}

/// The entity every case writes its properties to.
const ENTITY: &str = "unit";

/// `UpdateEntity` carrying one `propertyUpdates` entry for the component.
async fn update_property(emulator: &Emulator, property_name: &str, entry: Value) -> Resp {
    emulator
        .update_entity(
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {COMPONENT: {"propertyUpdates": {property_name: entry}}}}),
        )
        .await
}

/// `UpdateEntity` carrying one `propertyUpdates` entry for the composite
/// component — the same modelled `PropertyRequests` map one level down.
async fn update_composite_property(
    emulator: &Emulator,
    property_name: &str,
    entry: Value,
) -> Resp {
    emulator
        .update_entity(
            WORKSPACE,
            ENTITY,
            json!({"compositeComponentUpdates": {COMPOSITE_PATH: {
                "propertyUpdates": {property_name: entry},
            }}}),
        )
        .await
}

/// The value `GetEntity` reports for one property of the component.
async fn reported_value(emulator: &Emulator, property_name: &str) -> Option<Value> {
    let read = emulator.get_entity(WORKSPACE, ENTITY).await;
    assert_eq!(
        read.status, 200,
        "GetEntity {ENTITY:?}: expected 200, got {read:?}"
    );
    read.body
        .pointer(&format!(
            "/components/{COMPONENT}/properties/{property_name}/value"
        ))
        .cloned()
}

/// The value `ListProperties` reports for one property of the component or
/// composite component `address` names — the read a composite component's
/// properties reach, since the `ComponentSummary` `GetEntity` reports one in
/// carries no properties.
async fn listed_value(emulator: &Emulator, address: Value, property_name: &str) -> Option<Value> {
    let mut request = address;
    request["entityId"] = json!(ENTITY);
    let listed = emulator.list_properties(WORKSPACE, request).await;
    assert_eq!(
        listed.status, 200,
        "ListProperties {ENTITY:?}: expected 200, got {listed:?}"
    );
    listed
        .member("propertySummaries")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|summary| summary["propertyName"].as_str() == Some(property_name))
        .and_then(|summary| summary.get("value").cloned())
}

/// The value `ListProperties` reports for one property of the component.
async fn listed_component_value(emulator: &Emulator, property_name: &str) -> Option<Value> {
    listed_value(emulator, json!({"componentName": COMPONENT}), property_name).await
}

/// The value `ListProperties` reports for one property of the composite
/// component.
async fn listed_composite_value(emulator: &Emulator, property_name: &str) -> Option<Value> {
    listed_value(emulator, json!({"componentPath": COMPOSITE_PATH}), property_name).await
}

/// RULE: a value written to a property carries a value of the data type the
/// property's definition declares, whether that declaration arrives in the same
/// entry or was stored by an earlier write; a value of another type is refused
/// and nothing is stored.
///
/// The rows are every modelled `Type`, and each row is driven through the three
/// `updateType`s this arm answers (`CREATE`, `UPDATE`, and none at all), so the
/// rule holds however the entry addresses the property.
#[tokio::test]
async fn a_value_carries_the_data_type_the_entry_or_the_stored_definition_declares() {
    let emulator = seeded().await;

    for case in type_cases() {
        let property_name = format!("Own{}", case.declared);
        let declaration = json!({"dataType": {"type": case.declared}});

        // A `CREATE` carrying both the declaration and a value OF that type is
        // stored, and both reads report it.
        let accepted = update_property(
            &emulator,
            &property_name,
            json!({
                "updateType": "CREATE",
                "definition": declaration,
                "value": case.matching,
            }),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {}: a value of the declared type is stored, got {accepted:?}",
            case.declared
        );
        let reported = reported_value(&emulator, &property_name).await;
        assert_eq!(
            reported.as_ref(),
            Some(&case.matching),
            "case {}: expected GetEntity to report the value written, got {reported:?}",
            case.declared
        );

        // Each refusal shape: the declaration restated in the entry, the
        // declaration left to the stored one, and no `updateType` at all.
        let refusals: Vec<(&str, Value)> = vec![
            (
                "theEntryRestatesTheDeclaration",
                json!({
                    "updateType": "UPDATE",
                    "definition": declaration,
                    "value": case.mismatching,
                }),
            ),
            (
                "theDeclarationIsTheStoredOne",
                json!({"updateType": "UPDATE", "value": case.mismatching}),
            ),
            (
                "theEntryNamesNoUpdateType",
                json!({"value": case.mismatching}),
            ),
            (
                "aCreateOverTheStoredDeclaration",
                json!({"updateType": "CREATE", "value": case.mismatching}),
            ),
        ];
        for (shape, entry) in refusals {
            let refused = update_property(&emulator, &property_name, entry).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {}/{shape}: a value of another type is refused, got {refused:?}",
                case.declared
            );
            assert!(
                message(&refused).contains(&property_name),
                "case {}/{shape}: the refusal names the property the request named, got {:?}",
                case.declared,
                message(&refused)
            );
            let reported = reported_value(&emulator, &property_name).await;
            assert_eq!(
                reported.as_ref(),
                Some(&case.matching),
                "case {}/{shape}: a refused write stores nothing, so GetEntity still reports \
                 the value that was there, got {reported:?}",
                case.declared
            );
            let listed = listed_component_value(&emulator, &property_name).await;
            assert_eq!(
                listed.as_ref(),
                Some(&case.matching),
                "case {}/{shape}: ListProperties reports the same stored value, got {listed:?}",
                case.declared
            );
        }
    }
}

/// RULE: the data type a value is measured against is the one the property's
/// definition RESOLVES to, so a declaration the holder only INHERITS from the
/// component type it instantiates constrains the value exactly as its own
/// would — for a component and for a composite component alike.
///
/// The rows are every modelled `Type`, written through an entry that carries no
/// definition at all, against both holders of the modelled `PropertyRequests`
/// map.
#[tokio::test]
async fn an_inherited_data_type_constrains_the_value_on_either_holder() {
    let emulator = seeded().await;

    for case in type_cases() {
        let property_name = inherited_property(case.declared);

        // The component: a value of the inherited type is stored, one of
        // another type is refused.
        let accepted =
            update_property(&emulator, &property_name, json!({"value": case.matching})).await;
        assert_eq!(
            accepted.status, 200,
            "case {}: a value of the inherited type is stored, got {accepted:?}",
            case.declared
        );
        let reported = reported_value(&emulator, &property_name).await;
        assert_eq!(
            reported.as_ref(),
            Some(&case.matching),
            "case {}: expected GetEntity to report the value written, got {reported:?}",
            case.declared
        );
        let refused =
            update_property(&emulator, &property_name, json!({"value": case.mismatching})).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {}: a value contradicting the inherited dataType is refused, got {refused:?}",
            case.declared
        );
        let reported = reported_value(&emulator, &property_name).await;
        assert_eq!(
            reported.as_ref(),
            Some(&case.matching),
            "case {}: a refused write stores nothing, got {reported:?}",
            case.declared
        );

        // The composite component, whose declaration reaches it through the
        // component type its path resolved to.
        let accepted = update_composite_property(
            &emulator,
            &property_name,
            json!({"value": case.matching}),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {}/composite: a value of the inherited type is stored, got {accepted:?}",
            case.declared
        );
        let listed = listed_composite_value(&emulator, &property_name).await;
        assert_eq!(
            listed.as_ref(),
            Some(&case.matching),
            "case {}/composite: expected ListProperties to report the value written, got \
             {listed:?}",
            case.declared
        );
        let refused = update_composite_property(
            &emulator,
            &property_name,
            json!({"value": case.mismatching}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {}/composite: a value contradicting the inherited dataType is refused, got \
             {refused:?}",
            case.declared
        );
        let listed = listed_composite_value(&emulator, &property_name).await;
        assert_eq!(
            listed.as_ref(),
            Some(&case.matching),
            "case {}/composite: a refused write stores nothing, got {listed:?}",
            case.declared
        );
    }
}

/// RULE, at its two boundaries: a value carrying NO typed value at all is
/// refused wherever it is written, and a property no `dataType` reaches is
/// constrained by nothing, so any typed value — and a later value of a
/// different type — may be written to it.
#[tokio::test]
async fn an_empty_value_is_refused_and_an_undeclared_property_takes_any_typed_value() {
    let emulator = seeded().await;

    // A value with no member set at all, against a declared property and
    // against an undeclared one: neither names a value of any data type.
    for (case, property_name) in [
        ("aDeclaredProperty", inherited_property("DOUBLE")),
        ("anUndeclaredProperty", "Loose".to_string()),
    ] {
        let refused = update_property(&emulator, &property_name, json!({"value": {}})).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a value carrying no typed value is refused, got {refused:?}"
        );
        let reported = reported_value(&emulator, &property_name).await;
        assert_eq!(
            reported, None,
            "case {case:?}: a refused write stores no property at all, got {reported:?}"
        );
    }

    // A property NOTHING declares a `dataType` for: the component type declares
    // no `Loose`, and the entry carries no definition, so no type constrains the
    // value and successive values of different types are all stored.
    for (case, value) in [
        ("aStringFirst", json!({"stringValue": "no definition"})),
        ("thenADouble", json!({"doubleValue": 2.5})),
        ("thenABoolean", json!({"booleanValue": false})),
    ] {
        let accepted = update_property(&emulator, "Loose", json!({"value": value})).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a property no dataType reaches takes any typed value, got \
             {accepted:?}"
        );
        let reported = reported_value(&emulator, "Loose").await;
        assert_eq!(
            reported.as_ref(),
            Some(&value),
            "case {case:?}: expected GetEntity to report the value written, got {reported:?}"
        );
    }
}
