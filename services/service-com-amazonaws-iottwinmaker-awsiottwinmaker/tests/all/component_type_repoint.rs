// SPDX-License-Identifier: CC0-1.0

//! Re-pointing the component type a STORED component instantiates, end to end
//! over HTTP.
//!
//! RULE under test: a writer of a component's definitions refuses a request that
//! would leave the component holding a property definition with no `dataType` at
//! all — neither one of its own nor one inherited from the component type it
//! instantiates AFTER the patch — and it answers that question over every
//! definition the patched component would hold, not only over the ones the
//! request itself carried, because a declaration may legitimately omit its
//! `dataType` while REFINING one its component type declares, so the entry that
//! re-points `componentTypeId` is the call that takes that inheritance away.
//! Where the request names no `componentTypeId` it changes none of the rule's
//! inputs, so a declaration some earlier write left without an inheritable
//! `dataType` is not that patch's to refuse.
//!
//! Member optionality is the model's:
//! smithy_query(query="shape", shape="PropertyDefinitionRequest,PropertyDefinitionResponse,ComponentUpdateRequest")
//! — every member of `PropertyDefinitionRequest` is optional (which is what lets
//! a component's declaration restate only the members it changes) while
//! `dataType` is REQUIRED on the `PropertyDefinitionResponse` every reader
//! reports, and `ComponentUpdateRequest.componentTypeId` is the member an update
//! re-points the inheritance with.
//!
//! The rows differ along what the rule generalises over: WHERE the `dataType`
//! comes from after the patch (the declaration's own, the new type's own, one
//! the new type inherits along `extendsFrom`, the type the component already
//! had, nowhere at all), whether the new type is one the workspace HOLDS, how
//! MANY stored declarations the patch would strand (so the refusal names the
//! first in property-name order), and whether the same request also supplies or
//! deletes the declaration in question.

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case works in. Each test builds its own emulator, so the
/// id is shared without the cases sharing state.
const WORKSPACE: &str = "repointWs";

/// The component type the components are created instantiating: it declares both
/// properties the cases refine, each with a `dataType` a refining declaration can
/// inherit.
const GAUGE: &str = "com.example.gauge";
/// A component type that declares NEITHER of the properties the cases refine, so
/// re-pointing a component at it leaves a refining declaration inheriting
/// nothing.
const PLAIN: &str = "com.example.plain";
/// A component type that declares [`PROPERTY`] itself, with another `dataType`
/// than [`GAUGE`] does.
const COUNTER: &str = "com.example.counter";
/// A component type that declares nothing of its own and extends [`COUNTER`], so
/// a declaration refining [`PROPERTY`] reaches its `dataType` one type further
/// along the chain.
const DERIVED: &str = "com.example.derived";
/// A component type whose composite component type is a [`GAUGE`], so a
/// composite component can carry the same refining declaration a component does.
const HOLDER: &str = "com.example.holder";
/// An id no case creates: a component type the workspace does not hold, which a
/// component may still name and which supplies no inheritance.
const ABSENT: &str = "com.example.absent";

/// The property every case refines.
const PROPERTY: &str = "Reading";
/// A second property of [`GAUGE`], refined by the row that strands two
/// declarations at once. It sorts BEFORE [`PROPERTY`] in byte order, so a walk in
/// property-name order names it first.
const OTHER_PROPERTY: &str = "Label";
/// The composite component [`HOLDER`] declares.
const COMPOSITE: &str = "inner";
/// The one component name every case addresses.
const COMPONENT: &str = "Spec";

/// An emulator holding the workspace and the component types the cases point
/// their components at.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case works in must exist"
    );
    // In reference order: `extendsFrom` and `compositeComponentTypes` have to
    // name a component type the workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            GAUGE,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "DOUBLE"}, "displayName": "Reading (bar)"},
                    OTHER_PROPERTY: {"dataType": {"type": "STRING"}},
                },
            }),
        ),
        (
            PLAIN,
            json!({"propertyDefinitions": {"Other": {"dataType": {"type": "STRING"}}}}),
        ),
        (
            COUNTER,
            json!({"propertyDefinitions": {PROPERTY: {"dataType": {"type": "LONG"}}}}),
        ),
        (DERIVED, json!({"extendsFrom": [COUNTER]})),
        (
            HOLDER,
            json!({"compositeComponentTypes": {COMPOSITE: {"componentTypeId": GAUGE}}}),
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
    emulator
}

/// One row of the rule: the component a create stores, the `componentUpdates`
/// entry that patches it, and what the service has to answer.
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The declarations the component carries when it is created, keyed by
    /// property name.
    declarations: Value,
    /// The `componentUpdates.<COMPONENT>` entry the patch carries.
    patch: Value,
    /// The property whose definition the refusal has to name, or `None` where the
    /// patch is accepted.
    refused_property: Option<&'static str>,
    /// The `dataType.type` `GetEntity` has to report for [`PROPERTY`] after the
    /// call — the value the create left in place where the patch is refused, and
    /// `None` where the component reports no [`PROPERTY`] at all.
    reported_data_type: Option<&'static str>,
}

/// Every row of the rule.
fn cases() -> Vec<Case> {
    // The declaration that REFINES what the component type declares: it names a
    // member of its own and leaves the `dataType` to the type, which is exactly
    // the declaration a re-point can strand.
    let refining = || json!({PROPERTY: {"definition": {"isTimeSeries": true}}});
    vec![
        Case {
            what: "repointToATypeThatDeclaresThePropertyNowhere",
            entity_id: "repoint-strands",
            declarations: refining(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": PLAIN}),
            refused_property: Some(PROPERTY),
            reported_data_type: Some("DOUBLE"),
        },
        Case {
            what: "repointToATypeThatDeclaresThePropertyItself",
            entity_id: "repoint-declared",
            declarations: refining(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": COUNTER}),
            refused_property: None,
            reported_data_type: Some("LONG"),
        },
        Case {
            what: "repointToATypeThatInheritsTheDeclarationFromTheOneItExtends",
            entity_id: "repoint-inherited",
            declarations: refining(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": DERIVED}),
            refused_property: None,
            reported_data_type: Some("LONG"),
        },
        Case {
            what: "repointToTheTypeTheComponentAlreadyInstantiates",
            entity_id: "repoint-restated",
            declarations: refining(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": GAUGE}),
            refused_property: None,
            reported_data_type: Some("DOUBLE"),
        },
        Case {
            what: "repointToAComponentTypeTheWorkspaceDoesNotHold",
            entity_id: "repoint-absent",
            declarations: refining(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": ABSENT}),
            refused_property: Some(PROPERTY),
            reported_data_type: Some("DOUBLE"),
        },
        Case {
            what: "aDeclarationCarryingItsOwnDataTypeInheritsNothingAndSurvives",
            entity_id: "repoint-self-typed",
            declarations: json!({PROPERTY: {"definition": {"dataType": {"type": "STRING"}}}}),
            patch: json!({"updateType": "UPDATE", "componentTypeId": PLAIN}),
            refused_property: None,
            reported_data_type: Some("STRING"),
        },
        Case {
            what: "twoStrandedDeclarationsAreRefusedOnTheFirstInPropertyNameOrder",
            entity_id: "repoint-two-stranded",
            declarations: json!({
                PROPERTY: {"definition": {"isTimeSeries": true}},
                OTHER_PROPERTY: {"definition": {"displayName": "Label (refined)"}},
            }),
            patch: json!({"updateType": "UPDATE", "componentTypeId": PLAIN}),
            refused_property: Some(OTHER_PROPERTY),
            reported_data_type: Some("DOUBLE"),
        },
        Case {
            what: "theSameRequestMaySupplyTheDataTypeTheRepointTakesAway",
            entity_id: "repoint-supplied",
            declarations: refining(),
            patch: json!({
                "updateType": "UPDATE",
                "componentTypeId": PLAIN,
                "propertyUpdates": {PROPERTY: {"definition": {"dataType": {"type": "BOOLEAN"}}}},
            }),
            refused_property: None,
            reported_data_type: Some("BOOLEAN"),
        },
        Case {
            what: "theSameRequestMayDeleteTheDeclarationTheRepointWouldStrand",
            entity_id: "repoint-deleted",
            declarations: refining(),
            patch: json!({
                "updateType": "UPDATE",
                "componentTypeId": PLAIN,
                "propertyUpdates": {PROPERTY: {"updateType": "DELETE"}},
            }),
            refused_property: None,
            reported_data_type: None,
        },
        Case {
            what: "anEntryThatNamesNoComponentTypeIdChangesNoneOfTheRulesInputs",
            entity_id: "repoint-untouched",
            declarations: refining(),
            patch: json!({"updateType": "UPDATE", "description": "patched without a re-point"}),
            refused_property: None,
            reported_data_type: Some("DOUBLE"),
        },
    ]
}

/// The member path a refusal about `property_name` has to name: the member the
/// caller would carry a `dataType` for the stranded declaration in.
fn member(property_name: &str) -> String {
    format!("componentUpdates.{COMPONENT}.propertyUpdates.{property_name}.definition.dataType")
}

/// RULE: `UpdateEntity` answers the component `dataType` rule over every
/// definition the patched component would hold — the stored declarations the
/// request never mentions included — so a re-pointed `componentTypeId` is refused
/// rather than stranding a definition no read can report, and is accepted
/// wherever the patched component still reaches a `dataType`.
#[tokio::test]
async fn repointing_a_components_type_is_refused_where_it_would_strand_a_definition() {
    let emulator = seeded().await;

    for case in cases() {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "entityName": case.entity_id,
                    "components": {COMPONENT: {
                        "componentTypeId": GAUGE,
                        "properties": case.declarations.clone(),
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the component the patch addresses must exist, got {created:?}",
            case.what
        );

        let patched = emulator
            .update_entity(
                WORKSPACE,
                case.entity_id,
                json!({"componentUpdates": {COMPONENT: case.patch.clone()}}),
            )
            .await;
        match case.refused_property {
            Some(property_name) => {
                assert_eq!(
                    (patched.status, patched.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {:?}: expected the modelled refusal, got {patched:?}",
                    case.what
                );
                let named = member(property_name);
                let message = patched.member("message").as_str().unwrap_or_default();
                assert!(
                    message.contains(named.as_str()),
                    "case {:?}: expected the refusal to name {named:?}, got {message:?}",
                    case.what
                );
            }
            None => assert_eq!(
                patched.status, 200,
                "case {:?}: expected 200 from UpdateEntity, got {patched:?}",
                case.what
            ),
        }

        // What a read reports afterwards is the whole point of the rule: an
        // accepted patch leaves every definition reportable, and a refused one
        // leaves the entity exactly as the create wrote it.
        let read = emulator.get_entity(WORKSPACE, case.entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetEntity, got {read:?}",
            case.what
        );
        let component = read.member("components")[COMPONENT].clone();
        let property = &component["properties"][PROPERTY];
        assert_eq!(
            property["definition"]["dataType"]["type"].as_str(),
            case.reported_data_type,
            "case {:?}: expected the reported definition's dataType, got {property:?}",
            case.what
        );
        if case.refused_property.is_some() {
            assert_eq!(
                component["componentTypeId"].as_str(),
                Some(GAUGE),
                "case {:?}: a refused update changes nothing, got {component:?}",
                case.what
            );
        }

        // One property, one answer: `ListProperties` reports the same definition
        // `GetEntity` does, so the two readers cannot disagree about whether the
        // patched component still has one.
        let listed = emulator
            .list_properties(
                WORKSPACE,
                json!({"entityId": case.entity_id, "componentName": COMPONENT}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListProperties, got {listed:?}",
            case.what
        );
        let row = listed
            .member("propertySummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|row| row["propertyName"] == PROPERTY);
        assert_eq!(
            row.as_ref()
                .and_then(|row| row["definition"]["dataType"]["type"].as_str()),
            case.reported_data_type,
            "case {:?}: expected ListProperties to report the definition GetEntity does, got \
             {row:?}",
            case.what
        );
    }
}

/// RULE (the boundary the rule's inputs draw): a COMPOSITE component's
/// definitions inherit from the component type its own record names, so
/// re-pointing the holding component's `componentTypeId` strands none of them and
/// is accepted.
#[tokio::test]
async fn repointing_the_holding_component_leaves_a_composite_components_declaration_alone() {
    let emulator = seeded().await;

    let entity_id = "composite-holder";
    let composite_path = format!("{COMPONENT}/{COMPOSITE}");
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": HOLDER}},
                "compositeComponents": {
                    composite_path.clone(): {
                        "properties": {PROPERTY: {"definition": {"isTimeSeries": true}}},
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the composite component the patch leaves alone must exist, got {created:?}"
    );

    let patched = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"componentUpdates": {COMPONENT: {
                "updateType": "UPDATE",
                "componentTypeId": PLAIN,
            }}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "expected 200 from UpdateEntity: the holding component carries no definition of its own, \
         got {patched:?}"
    );

    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": entity_id, "componentPath": composite_path.clone()}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListProperties, got {listed:?}"
    );
    let row = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row["propertyName"] == PROPERTY);
    assert_eq!(
        row.as_ref()
            .and_then(|row| row["definition"]["dataType"]["type"].as_str()),
        Some("DOUBLE"),
        "expected the composite component's refining declaration to keep the dataType its own \
         component type declares, got {row:?}"
    );
}

/// RULE (the other side of the same boundary): a patch that names no
/// `componentTypeId` changes none of the rule's inputs, so a stored declaration
/// an earlier write to the COMPONENT TYPE left with no inheritable `dataType` is
/// not that patch's to refuse — while a patch that does name the member answers
/// the rule over the record it would store.
#[tokio::test]
async fn a_patch_that_repoints_nothing_is_not_refused_for_an_older_write() {
    let emulator = seeded().await;

    let entity_id = "older-write";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {
                    "componentTypeId": GAUGE,
                    "properties": {PROPERTY: {"definition": {"isTimeSeries": true}}},
                }},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the component the patches address must exist, got {created:?}"
    );

    // The component type stops declaring the property its component refines: a
    // map member a request names is replaced whole, so leaving the property out
    // drops it.
    let retyped = emulator
        .update_component_type(
            WORKSPACE,
            GAUGE,
            json!({"propertyDefinitions": {OTHER_PROPERTY: {"dataType": {"type": "STRING"}}}}),
        )
        .await;
    assert_eq!(
        retyped.status, 200,
        "expected 200 from UpdateComponentType, got {retyped:?}"
    );

    let patched = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"componentUpdates": {COMPONENT: {"description": "an unrelated patch"}}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "expected 200 from UpdateEntity: a patch naming no componentTypeId changes none of the \
         rule's inputs, got {patched:?}"
    );
    let read = emulator.get_entity(WORKSPACE, entity_id).await;
    assert_eq!(
        read.member("components")[COMPONENT]["description"].as_str(),
        Some("an unrelated patch"),
        "expected the unrelated patch to be persisted, got {read:?}"
    );

    // The same entity, patched by a request that DOES name the member the rule
    // resolves against: that call declares the inheritance, so it answers for
    // every definition the component would hold.
    let refused = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"componentUpdates": {COMPONENT: {"componentTypeId": GAUGE}}}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "expected the modelled refusal for a patch that declares the inheritance, got {refused:?}"
    );
    let named = member(PROPERTY);
    let message = refused.member("message").as_str().unwrap_or_default();
    assert!(
        message.contains(named.as_str()),
        "expected the refusal to name {named:?}, got {message:?}"
    );
}
