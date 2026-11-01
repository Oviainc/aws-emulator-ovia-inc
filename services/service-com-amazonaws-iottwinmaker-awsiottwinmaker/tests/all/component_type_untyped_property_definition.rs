// SPDX-License-Identifier: CC0-1.0

//! A component TYPE's property definition that omits `dataType`, end to end
//! over HTTP.
//!
//! RULE under test: `dataType` is optional on `PropertyDefinitionRequest` but
//! REQUIRED on the `PropertyDefinitionResponse` a read reports, so a writer of a
//! component type accepts a declaration without one only where the SAME property
//! name reaches that component type through its `extendsFrom` chain carrying a
//! `dataType` — a declaration that genuinely REFINES an inherited one. A
//! declaration nothing on the chain brings a `dataType` for is refused with the
//! modelled `ValidationException` (HTTP 400) and nothing is persisted, so the
//! store can never hold a definition a read would have to drop; the one chain
//! the rule stands down on is one reaching a component type of the real service
//! this emulator does not model, whose definitions it does not hold.
//!
//! The rows differ along what the rule generalises over: how FAR up the chain
//! the `dataType` comes from (the declaration itself, a parent, a grandparent, a
//! parent that only refines), how MANY parents the type declares (none, one,
//! two), WHO declares it (a created component type, one the service ships, none
//! at all), WHICH property name the chain declares (the same, another, one
//! differing only in case), the `Type` the inherited definition carries, and how
//! many declarations the request's map holds. Both writers of the member are
//! driven, since a patch that re-points or clears `extendsFrom` is the call that
//! takes an inherited `dataType` away.
//!
//! The expectations for a parent the SERVICE ships are read off the catalogue
//! `CreateWorkspace` provisions (`state::built_in_component_types`) rather than
//! restated, so they stay the service's own declaration of it.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::built_in_component_types;

use crate::harness::{Emulator, REGION};

/// The workspace every case below works in. Each test builds its own emulator,
/// so the id is shared without the cases sharing state.
const WORKSPACE: &str = "untypedDefinitionWorkspace";

/// A created parent that declares one property WITH a `dataType`: the
/// declaration a refinement inherits from.
const DECLARES_TEMPERATURE: &str = "com.example.declaresTemperature";
/// A created parent that declares no property at all.
const DECLARES_NOTHING: &str = "com.example.declaresNothing";
/// A created parent that declares a DIFFERENT property.
const DECLARES_ANOTHER: &str = "com.example.declaresAnother";
/// A created parent that itself only REFINES [`DECLARES_TEMPERATURE`]'s
/// declaration, so the `dataType` a type below it inherits comes from two hops
/// up.
const REFINES_TEMPERATURE: &str = "com.example.refinesTemperature";

/// Two component types the service SHIPS, whose declarations a workspace is
/// provisioned with.
const BUILT_IN_ALARM: &str = "com.amazon.iottwinmaker.alarm.basic";
/// See [`BUILT_IN_ALARM`].
const BUILT_IN_DOCUMENTS: &str = "com.amazon.iottwinmaker.documents";
/// An id in the namespace the service reserves for its own component types that
/// this emulator does NOT model: the chain it opens is one whose declarations
/// the emulator does not hold.
const UNMODELLED_PARENT: &str = "com.amazon.iottwinmaker.notmodelled";

/// The `dataType.type` the catalogue declares for one property of a component
/// type the service ships — read off `CreateWorkspace`'s own seed rather than
/// restated here.
fn built_in_data_type(component_type_id: &str, property_name: &str) -> String {
    let catalogue = built_in_component_types(REGION, WORKSPACE, SystemTime::now());
    let record = catalogue
        .get(component_type_id)
        .unwrap_or_else(|| panic!("the catalogue must ship {component_type_id:?}"));
    record
        .property_definitions
        .get(property_name)
        .and_then(|definition| definition.data_type.as_ref())
        .map(|data_type| data_type.r#type.clone())
        .unwrap_or_else(|| {
            panic!("{component_type_id:?} must declare {property_name:?} with a dataType")
        })
}

/// An emulator holding the workspace the cases work in plus every parent they
/// name: a parent has to exist before a create may extend it.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the cases are created in must exist"
    );
    for (component_type_id, body) in [
        (
            DECLARES_TEMPERATURE,
            json!({
                "propertyDefinitions": {
                    "Temperature": {
                        "dataType": {"type": "DOUBLE", "unitOfMeasure": "Celsius"},
                        "isTimeSeries": true,
                    },
                },
            }),
        ),
        (DECLARES_NOTHING, json!({})),
        (
            DECLARES_ANOTHER,
            json!({"propertyDefinitions": {"Serial": {"dataType": {"type": "STRING"}}}}),
        ),
        (
            REFINES_TEMPERATURE,
            json!({
                "extendsFrom": [DECLARES_TEMPERATURE],
                // No `dataType`: this parent refines what it inherits, so the
                // type is still two hops up.
                "propertyDefinitions": {"Temperature": {"displayName": "Temperature"}},
            }),
        ),
    ] {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "the parent {component_type_id:?} the cases extend must exist, got {created:?}"
        );
    }
    emulator
}

/// One accepted row: its name, the id the create asks for, the request body, and
/// the `(property name, reported dataType.type)` pairs the read afterwards must
/// report.
type AcceptedCase = (
    &'static str,
    &'static str,
    Value,
    Vec<(&'static str, String)>,
);

/// One refused row: its name, the id the create asks for, the request body, and
/// the property name the refusal has to name.
type RefusedCase = (&'static str, &'static str, Value, &'static str);

/// RULE (acceptance half): a declaration that omits `dataType` is accepted
/// exactly where the `extendsFrom` chain brings one for the same property name,
/// and the read then reports the property with the inherited `dataType`.
#[tokio::test]
async fn a_definition_omitting_a_data_type_is_accepted_where_the_chain_brings_one() {
    let emulator = seeded().await;

    let alarm_status = built_in_data_type(BUILT_IN_ALARM, "alarm_status");
    let documents = built_in_data_type(BUILT_IN_DOCUMENTS, "documents");
    let rows: Vec<AcceptedCase> = vec![
        (
            // The control row: a declaration carrying its own `dataType` needs
            // no inheritance at all.
            "ownDataTypeUnderNoParent",
            "com.example.ok.ownDataType",
            json!({"propertyDefinitions": {"Temperature": {"dataType": {"type": "INTEGER"}}}}),
            vec![("Temperature", "INTEGER".to_string())],
        ),
        (
            "refinesTheParentsOwnDeclaration",
            "com.example.ok.refinesParent",
            json!({
                "extendsFrom": [DECLARES_TEMPERATURE],
                "propertyDefinitions": {"Temperature": {"isRequiredInEntity": true}},
            }),
            vec![("Temperature", "DOUBLE".to_string())],
        ),
        (
            // Two hops: the parent only refines, so the `dataType` comes from
            // the grandparent.
            "refinesThroughAParentThatOnlyRefines",
            "com.example.ok.refinesGrandparent",
            json!({
                "extendsFrom": [REFINES_TEMPERATURE],
                "propertyDefinitions": {"Temperature": {"isRequiredInEntity": true}},
            }),
            vec![("Temperature", "DOUBLE".to_string())],
        ),
        (
            // Two parents, only the second of which declares the property.
            "oneOfTwoParentsDeclaresIt",
            "com.example.ok.secondParentDeclares",
            json!({
                "extendsFrom": [DECLARES_NOTHING, DECLARES_TEMPERATURE],
                "propertyDefinitions": {"Temperature": {"displayName": "T"}},
            }),
            vec![("Temperature", "DOUBLE".to_string())],
        ),
        (
            // A parent the SERVICE ships declares it, which is the documented
            // alarm pattern.
            "refinesABuiltInParentsDeclaration",
            "com.example.ok.refinesBuiltInAlarm",
            json!({
                "extendsFrom": [BUILT_IN_ALARM],
                "propertyDefinitions": {"alarm_status": {"displayName": "Status"}},
            }),
            vec![("alarm_status", alarm_status)],
        ),
        (
            // The same, where the inherited `Type` is a MAP rather than a
            // scalar: the rule is about a `dataType` being inherited at all.
            "refinesABuiltInParentsNonScalarDeclaration",
            "com.example.ok.refinesBuiltInDocuments",
            json!({
                "extendsFrom": [BUILT_IN_DOCUMENTS],
                "propertyDefinitions": {"documents": {"isRequiredInEntity": true}},
            }),
            vec![("documents", documents)],
        ),
        (
            // A map holding one refinement beside one declaration of the type's
            // own: each entry is held to the rule on its own name.
            "aRefinementBesideTheTypesOwnDeclaration",
            "com.example.ok.refinementBesideOwn",
            json!({
                "extendsFrom": [DECLARES_TEMPERATURE],
                "propertyDefinitions": {
                    "Temperature": {"isRequiredInEntity": true},
                    "Humidity": {"dataType": {"type": "LONG"}},
                },
            }),
            vec![
                ("Temperature", "DOUBLE".to_string()),
                ("Humidity", "LONG".to_string()),
            ],
        ),
    ];

    for (case, component_type_id, body, expectations) in rows {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateComponentType, got {created:?}"
        );

        // Read back through the implemented reader: what the create accepted has
        // to be what a later caller sees, which is the whole point of the rule.
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetComponentType, got {read:?}"
        );
        for (property_name, data_type) in &expectations {
            let reported = &read.member("propertyDefinitions")[property_name];
            assert_eq!(
                reported["dataType"]["type"].as_str(),
                Some(data_type.as_str()),
                "case {case:?} property {property_name:?}: expected the resolved dataType \
                 {data_type:?}, got {reported:?}"
            );
        }
    }
}

/// RULE (refusal half): a declaration that omits `dataType` where the
/// `extendsFrom` chain brings none for that property name is refused with the
/// modelled `ValidationException` (HTTP 400), the refusal names the property,
/// and nothing is persisted.
#[tokio::test]
async fn a_definition_omitting_a_data_type_is_refused_where_the_chain_brings_none() {
    let emulator = seeded().await;

    let untyped = json!({"isTimeSeries": true});
    let rows: Vec<RefusedCase> = vec![
        (
            "noParentAtAll",
            "com.example.no.noParent",
            json!({"propertyDefinitions": {"Temperature": untyped.clone()}}),
            "Temperature",
        ),
        (
            "aParentThatDeclaresNothing",
            "com.example.no.parentDeclaresNothing",
            json!({
                "extendsFrom": [DECLARES_NOTHING],
                "propertyDefinitions": {"Temperature": untyped.clone()},
            }),
            "Temperature",
        ),
        (
            "aParentThatDeclaresAnotherProperty",
            "com.example.no.parentDeclaresAnother",
            json!({
                "extendsFrom": [DECLARES_ANOTHER],
                "propertyDefinitions": {"Temperature": untyped.clone()},
            }),
            "Temperature",
        ),
        (
            // The chain is two hops deep and carries a declaration of another
            // name only: depth brings nothing for THIS property.
            "aTwoHopChainThatDeclaresAnotherProperty",
            "com.example.no.twoHopsDeclareAnother",
            json!({
                "extendsFrom": [REFINES_TEMPERATURE],
                "propertyDefinitions": {"Humidity": untyped.clone()},
            }),
            "Humidity",
        ),
        (
            "twoParentsNeitherOfWhichDeclaresIt",
            "com.example.no.neitherParentDeclares",
            json!({
                "extendsFrom": [DECLARES_NOTHING, DECLARES_ANOTHER],
                "propertyDefinitions": {"Temperature": untyped.clone()},
            }),
            "Temperature",
        ),
        (
            // A parent the SERVICE ships is a parent like any other: it brings
            // only the names it declares.
            "aBuiltInParentThatDeclaresAnotherProperty",
            "com.example.no.builtInDeclaresAnother",
            json!({
                "extendsFrom": [BUILT_IN_ALARM],
                "propertyDefinitions": {"alarm_level": untyped.clone()},
            }),
            "alarm_level",
        ),
        (
            // A property name is a map KEY, so one differing only in case is a
            // different property and inherits nothing.
            "aNameDifferingFromTheInheritedOneOnlyInCase",
            "com.example.no.caseDiffers",
            json!({
                "extendsFrom": [DECLARES_TEMPERATURE],
                "propertyDefinitions": {"temperature": untyped.clone()},
            }),
            "temperature",
        ),
        (
            // Two offending entries: the refusal names the first in property-NAME
            // order, whatever order the request's map was serialized in.
            "theFirstOffendingNameInKeyOrderIsNamed",
            "com.example.no.twoOffenders",
            json!({
                "extendsFrom": [DECLARES_TEMPERATURE],
                "propertyDefinitions": {
                    "zulu": untyped.clone(),
                    "alpha": untyped.clone(),
                    "Temperature": untyped.clone(),
                },
            }),
            "alpha",
        ),
    ];

    for (case, component_type_id, body, named) in rows {
        let refused = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        let message = refused.member("message").as_str().unwrap_or_default();
        assert!(
            message.contains(named) && message.contains("dataType"),
            "case {case:?}: the refusal must name the property it refused on ({named:?}) and the \
             member it is missing, got {message:?}"
        );

        // Nothing was persisted: the store cannot come to hold a definition a
        // read would have to drop.
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create persists nothing, got {read:?}"
        );
    }
}

/// RULE (the one stand-down): where the chain reaches a component type of the
/// real service this emulator does not model, the emulator does not hold that
/// type's declarations, so a `dataType` it cannot find there cannot be called
/// uninherited and the declaration stands — the same stand-down the sibling rule
/// about `propertyGroups` makes on the same unknowable chain.
#[tokio::test]
async fn an_unmodelled_parent_leaves_the_declaration_standing() {
    let emulator = seeded().await;

    let created = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.underAnUnmodelledParent",
            json!({
                "extendsFrom": [UNMODELLED_PARENT],
                "propertyDefinitions": {"Inherited": {"isTimeSeries": true}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"anUnmodelledParent\": a chain whose declarations the emulator does not hold \
         leaves the declaration standing, got {created:?}"
    );

    let read = emulator
        .get_component_type(WORKSPACE, "com.example.underAnUnmodelledParent")
        .await;
    assert_eq!(
        (read.status, read.member("extendsFrom")),
        (200, &json!([UNMODELLED_PARENT])),
        "case \"anUnmodelledParent\": the accepted inheritance is what the read reports, \
         got {read:?}"
    );

    // The same chain brings no licence to a property the emulator CAN see is
    // undeclared once the unknowable parent is gone.
    let refused = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.underAKnownParentOnly",
            json!({
                "extendsFrom": [DECLARES_NOTHING],
                "propertyDefinitions": {"Inherited": {"isTimeSeries": true}},
            }),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"aKnownParentOnly\": a chain the emulator holds whole is held to the rule, \
         got {refused:?}"
    );
}

/// One patch row: its name, the body it sends, and either the property name the
/// refusal must name (`Err`) or the `(property name, reported dataType.type)`
/// pairs the read afterwards must report (`Ok`).
type PatchCase = (
    &'static str,
    Value,
    Result<Vec<(&'static str, &'static str)>, &'static str>,
);

/// RULE (the other writer): `UpdateComponentType` holds the WHOLE patched record
/// to the same rule, because a patch that re-points or clears `extendsFrom` is
/// the call that takes an inherited `dataType` away from a definition an earlier
/// call stored.
#[tokio::test]
async fn a_patch_is_held_to_the_same_rule_over_the_whole_record() {
    let emulator = seeded().await;

    let rows: Vec<PatchCase> = vec![
        (
            // A new entry nothing on the chain declares.
            "anAddedEntryThatInheritsNothing",
            json!({"propertyDefinitions": {"Humidity": {"isTimeSeries": true}}}),
            Err("Humidity"),
        ),
        (
            // The inheritance cleared from under the stored refinement.
            "extendsFromCleared",
            json!({"extendsFrom": []}),
            Err("Temperature"),
        ),
        (
            // The inheritance re-pointed at a parent that declares nothing.
            "extendsFromRePointedAtAParentThatDeclaresNothing",
            json!({"extendsFrom": [DECLARES_NOTHING]}),
            Err("Temperature"),
        ),
        (
            // Re-pointed at a parent that declares the property: the
            // refinement still inherits, from the new parent's declaration.
            "extendsFromRePointedAtAParentThatDeclaresIt",
            json!({"extendsFrom": [REFINES_TEMPERATURE]}),
            Ok(vec![("Temperature", "DOUBLE")]),
        ),
        (
            // Still a refinement of the stored parent's declaration.
            "aRestatedRefinementUnderTheSameParent",
            json!({"propertyDefinitions": {"Temperature": {"displayName": "T"}}}),
            Ok(vec![("Temperature", "DOUBLE")]),
        ),
        (
            // The inheritance dropped and the `dataType` declared instead: the
            // record is reportable without any chain at all.
            "extendsFromClearedAndTheDataTypeDeclared",
            json!({
                "extendsFrom": [],
                "propertyDefinitions": {"Temperature": {"dataType": {"type": "STRING"}}},
            }),
            Ok(vec![("Temperature", "STRING")]),
        ),
    ];

    for (index, (case, patch, expected)) in rows.into_iter().enumerate() {
        // Each row patches its OWN record, created as the refinement the rule
        // accepts, so one row's patch cannot decide the next one's.
        let component_type_id = format!("com.example.patched{index}");
        let created = emulator
            .create_component_type(
                WORKSPACE,
                &component_type_id,
                json!({
                    "extendsFrom": [DECLARES_TEMPERATURE],
                    "propertyDefinitions": {"Temperature": {"isRequiredInEntity": true}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateComponentType, got {created:?}"
        );

        let patched = emulator
            .update_component_type(WORKSPACE, &component_type_id, patch)
            .await;
        match expected {
            Err(named) => {
                assert_eq!(
                    (patched.status, patched.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {case:?}: expected the modelled refusal, got {patched:?}"
                );
                let message = patched.member("message").as_str().unwrap_or_default();
                assert!(
                    message.contains(named) && message.contains("dataType"),
                    "case {case:?}: the refusal must name the property it refused on \
                     ({named:?}), got {message:?}"
                );
                // A refused patch leaves the stored record reportable as it was.
                let read = emulator
                    .get_component_type(WORKSPACE, &component_type_id)
                    .await;
                assert_eq!(
                    read.member("propertyDefinitions")["Temperature"]["dataType"]["type"].as_str(),
                    Some("DOUBLE"),
                    "case {case:?}: a refused patch leaves the definitions as they were, \
                     got {read:?}"
                );
            }
            Ok(expectations) => {
                assert_eq!(
                    patched.status, 200,
                    "case {case:?}: expected 200 from UpdateComponentType, got {patched:?}"
                );
                let read = emulator
                    .get_component_type(WORKSPACE, &component_type_id)
                    .await;
                for (property_name, data_type) in &expectations {
                    let reported = &read.member("propertyDefinitions")[property_name];
                    assert_eq!(
                        reported["dataType"]["type"].as_str(),
                        Some(*data_type),
                        "case {case:?} property {property_name:?}: expected the resolved dataType \
                         {data_type:?}, got {reported:?}"
                    );
                }
            }
        }
    }
}

/// RULE (what the rule buys a reader): every definition a writer accepted is
/// reported by every reader that resolves one — a component type's own read, and
/// the readers of a COMPONENT instantiating the type — so no property is left in
/// the store and invisible on the wire.
#[tokio::test]
async fn every_accepted_definition_is_visible_to_every_reader() {
    let emulator = seeded().await;

    let component_type_id = "com.example.visible";
    let created = emulator
        .create_component_type(
            WORKSPACE,
            component_type_id,
            json!({
                "extendsFrom": [DECLARES_TEMPERATURE],
                "propertyDefinitions": {
                    "Temperature": {"isRequiredInEntity": true},
                    "Humidity": {"dataType": {"type": "DOUBLE"}},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateComponentType, got {created:?}"
    );

    let read = emulator
        .get_component_type(WORKSPACE, component_type_id)
        .await;
    for property_name in ["Humidity", "Temperature"] {
        let reported = &read.member("propertyDefinitions")[property_name];
        assert_eq!(
            reported["dataType"]["type"].as_str(),
            Some("DOUBLE"),
            "case {property_name:?}: GetComponentType must report every stored definition, \
             got {read:?}"
        );
    }

    let entity = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "visible",
                "entityName": "visible",
                "components": {"Spec": {"componentTypeId": component_type_id}},
            }),
        )
        .await;
    assert_eq!(
        entity.status, 200,
        "expected 200 from CreateEntity, got {entity:?}"
    );

    let entity = emulator.get_entity(WORKSPACE, "visible").await;
    for property_name in ["Humidity", "Temperature"] {
        let reported = &entity.member("components")["Spec"]["properties"][property_name]
            ["definition"]["dataType"]["type"];
        assert_eq!(
            reported.as_str(),
            Some("DOUBLE"),
            "case {property_name:?}: GetEntity must report every property the component has, \
             got {entity:?}"
        );
    }

    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": "visible", "componentName": "Spec"}),
        )
        .await;
    let names: Vec<String> = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row["propertyName"].as_str().map(str::to_string))
        .collect();
    for property_name in ["Humidity", "Temperature"] {
        assert!(
            names.iter().any(|name| name == property_name),
            "case {property_name:?}: ListProperties must report every property the component \
             has, got {names:?}"
        );
    }
}
