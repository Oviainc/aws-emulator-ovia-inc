// SPDX-License-Identifier: CC0-1.0

//! `UpdateEntity`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`,
//! the same entry the served emulator uses — so a route, a label, a status code
//! and a serialized body are all exercised, not just a handler body. What the
//! update persisted is read back through `GetEntity` (and, for the lifecycle
//! rule, `ListEntities` and `ListComponents`), so each suite proves the store
//! changed rather than that one response was assembled.
//!
//! Four RULES, each asserted over a TABLE of inputs rather than one case:
//!
//! * an update PATCHES: a member it names replaces the stored value for that
//!   member and a member it leaves out keeps what is persisted, while the two
//!   collection members are addressed per ENTRY — each entry's `updateType`
//!   says whether the component, composite component, property or property
//!   group its key names is created, patched or removed, and at the property
//!   level `CREATE` and `UPDATE` both write the entry whether or not the
//!   property was there;
//! * the call answers the non-terminal `UPDATING` and the CLOCK settles the
//!   entity back in `ACTIVE`, whether or not anybody polled for it;
//! * `parentEntityUpdate` re-parents the entity or detaches it to the root
//!   sentinel, and refuses any parent that would make the stored parent graph
//!   cyclic;
//! * a refused update leaves the stored entity exactly as it was, with a label
//!   naming something the store does not hold answered 404 and every rule this
//!   handler owns answered 400.
//!
//! Two further suites cover the ceilings on one entity's contents and the
//! removal of the composite components declared below a removed one.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    ENTITY_SETTLE_SPAN, MAX_COMPONENTS_PER_ENTITY, MAX_PROPERTIES_PER_COMPONENT,
    MAX_PROPERTY_GROUPS_PER_COMPONENT, ROOT_PARENT_ENTITY_ID, STATE_ACTIVE, STATE_UPDATING,
};

use crate::harness::{Emulator, Resp};

/// The workspace every case in this suite works in. Each test builds its own
/// emulator, so the id is shared without the cases sharing state.
const WORKSPACE: &str = "updateEntityWs";

/// The component type the baseline entity's component instantiates. It declares
/// two `compositeComponentTypes`, so a composite path one level down resolves.
const SENSOR_TYPE: &str = "com.example.sensor";
/// The component type both composite declarations of [`SENSOR_TYPE`] name. It
/// declares one composite component type of its own, so a path TWO levels below
/// a component resolves.
const INNER_TYPE: &str = "com.example.inner";
/// The component type [`INNER_TYPE`]'s own composite declaration names.
const LEAF_TYPE: &str = "com.example.leaf";
/// A component type carrying an externally stored property and no data
/// connector, which is what makes a component type ABSTRACT — a base type to
/// extend rather than one to instantiate.
const ABSTRACT_TYPE: &str = "com.example.abstract";

/// Block until the transition an update owes has fallen due.
///
/// The emulator stores the instant and never sleeps for it, so a test that wants
/// to observe the settled state is the one that has to wait; the margin keeps
/// the wait from racing the span's own boundary.
fn wait_until_settled() {
    sleep(ENTITY_SETTLE_SPAN + Duration::from_millis(100));
}

/// The message an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message").as_str().unwrap_or_default().to_string()
}

/// The `status.state` a read reports for an entity.
fn reported_state(resp: &Resp) -> Option<&str> {
    resp.member("status")["state"].as_str()
}

/// An emulator holding the workspace the cases work in and the component types
/// their components instantiate and their composite paths resolve through.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case works in must exist"
    );
    // In reference order: a component type's `compositeComponentTypes` has to
    // name one the workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            LEAF_TYPE,
            json!({"propertyDefinitions": {"Reading": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            INNER_TYPE,
            json!({
                "propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}},
                "compositeComponentTypes": {"leaf": {"componentTypeId": LEAF_TYPE}},
            }),
        ),
        (
            SENSOR_TYPE,
            json!({
                "propertyDefinitions": {"Serial": {"dataType": {"type": "STRING"}}},
                "compositeComponentTypes": {
                    "inner": {"componentTypeId": INNER_TYPE},
                    "spare": {"componentTypeId": INNER_TYPE},
                },
            }),
        ),
        (
            ABSTRACT_TYPE,
            json!({
                "propertyDefinitions": {
                    "Reading": {
                        "dataType": {"type": "DOUBLE"},
                        "isTimeSeries": true,
                        "isStoredExternally": true,
                    },
                },
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
    emulator
}

/// The create every patch case starts from: an entity carrying one of
/// everything an update can address — a component with a property, a property
/// group and a composite component — so each case can assert both what its
/// patch changed and what it left alone.
fn baseline(entity_id: &str) -> Value {
    json!({
        "entityId": entity_id,
        "entityName": "the name the create persisted",
        "description": "the description the create persisted",
        "components": {
            "Sensor": {
                "componentTypeId": SENSOR_TYPE,
                "description": "the component the create persisted",
                "properties": {
                    "Serial": {
                        "definition": {"dataType": {"type": "STRING"}},
                        "value": {"stringValue": "sn-1"},
                    },
                },
                "propertyGroups": {
                    "readings": {"groupType": "TABULAR", "propertyNames": ["Serial"]},
                },
            },
        },
        "compositeComponents": {
            "Sensor/inner": {"description": "the composite the create persisted"},
        },
    })
}

/// Create the baseline entity `entity_id` and fail the case that could not.
async fn create_baseline(emulator: &Emulator, entity_id: &str) {
    let created = emulator
        .create_entity(WORKSPACE, baseline(entity_id))
        .await;
    assert_eq!(
        created.status, 200,
        "case {entity_id:?}: expected 200 from CreateEntity, got {created:?}"
    );
    assert_eq!(
        created.member("state").as_str(),
        Some(STATE_ACTIVE),
        "case {entity_id:?}: a create settles the entity inside its own call, got {created:?}"
    );
}

/// One patch case: its name, the request body it sends, and the `(JSON pointer,
/// value)` pairs the read afterwards must report — `None` where the patch must
/// have left nothing at that path at all.
type PatchCase = (&'static str, Value, Vec<(&'static str, Option<Value>)>);

/// RULE: an update replaces the stored value of every top-level member the
/// request NAMES and keeps every one it does not, while each entry of
/// `componentUpdates` / `compositeComponentUpdates` (and of the
/// `propertyUpdates` / `propertyGroupUpdates` inside one) is applied as its own
/// `updateType` says — created, patched or removed — with an entry that names
/// no `updateType` applied as the entity's contents make it.
///
/// The rows differ along which member the patch names, along the shape of what
/// it carries (a string, an emptied string, a map entry added, patched or
/// dropped), along the update type each entry names (including none at all) and
/// along how many entries one call carries, because that is what the rule
/// generalises over.
#[tokio::test]
async fn an_update_applies_each_member_and_entry_as_the_request_names_it() {
    let emulator = seeded().await;

    let rows: Vec<PatchCase> = vec![
        (
            // The boundary row: a patch that names nothing changes nothing.
            "names.nothing",
            json!({}),
            vec![
                (
                    "/entityName",
                    Some(json!("the name the create persisted")),
                ),
                (
                    "/description",
                    Some(json!("the description the create persisted")),
                ),
                ("/components/Sensor/componentTypeId", Some(json!(SENSOR_TYPE))),
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-1")),
                ),
                (
                    "/components/Sensor/compositeComponents/inner/componentPath",
                    Some(json!("Sensor/inner")),
                ),
            ],
        ),
        (
            "entityName.replaced",
            json!({"entityName": "the name the update persisted"}),
            vec![
                (
                    "/entityName",
                    Some(json!("the name the update persisted")),
                ),
                (
                    "/description",
                    Some(json!("the description the create persisted")),
                ),
            ],
        ),
        (
            // An empty string is a value the request NAMED, not an omission:
            // `Description` is `@length(min: 0)`, so clearing it is legal.
            "description.emptied",
            json!({"description": ""}),
            vec![
                ("/description", Some(json!(""))),
                (
                    "/entityName",
                    Some(json!("the name the create persisted")),
                ),
            ],
        ),
        (
            "entityName.andDescription",
            json!({"entityName": "Capteur ✓ 1", "description": "both named"}),
            vec![
                ("/entityName", Some(json!("Capteur ✓ 1"))),
                ("/description", Some(json!("both named"))),
            ],
        ),
        (
            "component.created",
            json!({"componentUpdates": {"Extra": {
                "updateType": "CREATE",
                "componentTypeId": INNER_TYPE,
                "description": "the component the update created",
                "propertyUpdates": {"Pressure": {
                    "definition": {"dataType": {"type": "DOUBLE"}},
                    "value": {"doubleValue": 1.5},
                }},
            }}}),
            vec![
                ("/components/Extra/componentTypeId", Some(json!(INNER_TYPE))),
                (
                    "/components/Extra/description",
                    Some(json!("the component the update created")),
                ),
                (
                    "/components/Extra/properties/Pressure/value/doubleValue",
                    Some(json!(1.5)),
                ),
                // The component the create persisted is untouched by an entry
                // that does not name it.
                (
                    "/components/Sensor/description",
                    Some(json!("the component the create persisted")),
                ),
            ],
        ),
        (
            // No `updateType` at all: the entity has no such component, so the
            // entry creates one.
            "component.createdWithNoUpdateType",
            json!({"componentUpdates": {"Extra": {"componentTypeId": INNER_TYPE}}}),
            vec![("/components/Extra/componentTypeId", Some(json!(INNER_TYPE)))],
        ),
        (
            "component.patched",
            json!({"componentUpdates": {"Sensor": {
                "updateType": "UPDATE",
                "description": "the component description the update persisted",
            }}}),
            vec![
                (
                    "/components/Sensor/description",
                    Some(json!("the component description the update persisted")),
                ),
                // Neither named by the entry nor dropped by it.
                ("/components/Sensor/componentTypeId", Some(json!(SENSOR_TYPE))),
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-1")),
                ),
                (
                    "/components/Sensor/propertyGroups/readings/propertyNames",
                    Some(json!(["Serial"])),
                ),
            ],
        ),
        (
            // No `updateType`, and the entity HAS the component: the entry
            // patches it.
            "component.patchedWithNoUpdateType",
            json!({"componentUpdates": {"Sensor": {"description": "patched with no update type"}}}),
            vec![
                (
                    "/components/Sensor/description",
                    Some(json!("patched with no update type")),
                ),
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-1")),
                ),
            ],
        ),
        (
            // Removing the component takes the composite component stored
            // inside it with it: nothing can address a path through a component
            // that is gone.
            "component.deleted",
            json!({"componentUpdates": {"Sensor": {"updateType": "DELETE"}}}),
            vec![
                ("/components/Sensor", None),
                (
                    "/entityName",
                    Some(json!("the name the create persisted")),
                ),
            ],
        ),
        (
            "property.valueReplaced",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Serial": {
                "updateType": "UPDATE",
                "value": {"stringValue": "sn-2"},
            }}}}}),
            vec![
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-2")),
                ),
                // The definition the entry did not name is kept.
                (
                    "/components/Sensor/properties/Serial/definition/dataType/type",
                    Some(json!("STRING")),
                ),
            ],
        ),
        (
            "property.created",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Temperature": {
                "updateType": "CREATE",
                "definition": {"dataType": {"type": "DOUBLE", "unitOfMeasure": "Celsius"}},
                "value": {"doubleValue": 21.5},
            }}}}}),
            vec![
                (
                    "/components/Sensor/properties/Temperature/definition/dataType/unitOfMeasure",
                    Some(json!("Celsius")),
                ),
                (
                    "/components/Sensor/properties/Temperature/value/doubleValue",
                    Some(json!(21.5)),
                ),
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-1")),
                ),
            ],
        ),
        (
            // No `updateType` on a property the component does not have: the
            // entry defines it, exactly as a `CREATE` would.
            "property.createdWithNoUpdateType",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Temperature": {
                "definition": {"dataType": {"type": "DOUBLE"}},
            }}}}}),
            vec![(
                "/components/Sensor/properties/Temperature/definition/dataType/type",
                Some(json!("DOUBLE")),
            )],
        ),
        (
            // `UPDATE` on a property the component does NOT have, and `CREATE`
            // on one it does: a property is wholly described by its entry, so
            // which of the two the caller names does not change what is
            // written — unlike a component, which is brought into being by its
            // entry.
            "property.eitherWriteTypeWritesTheEntry",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {
                "Temperature": {
                    "updateType": "UPDATE",
                    "definition": {"dataType": {"type": "DOUBLE"}},
                    "value": {"doubleValue": 7.5},
                },
                "Serial": {"updateType": "CREATE", "value": {"stringValue": "sn-3"}},
            }}}}),
            vec![
                (
                    "/components/Sensor/properties/Temperature/value/doubleValue",
                    Some(json!(7.5)),
                ),
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-3")),
                ),
                // The definition the `CREATE` entry did not name is kept.
                (
                    "/components/Sensor/properties/Serial/definition/dataType/type",
                    Some(json!("STRING")),
                ),
            ],
        ),
        (
            "property.deleted",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Serial": {
                "updateType": "DELETE",
            }}}}}),
            vec![
                ("/components/Sensor/properties/Serial", None),
                (
                    "/components/Sensor/description",
                    Some(json!("the component the create persisted")),
                ),
            ],
        ),
        (
            // `RESET_VALUE` takes the property's VALUE away and keeps the
            // declaration that gives it meaning.
            "property.valueReset",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Serial": {
                "updateType": "RESET_VALUE",
            }}}}}),
            vec![
                ("/components/Sensor/properties/Serial/value", None),
                (
                    "/components/Sensor/properties/Serial/definition/dataType/type",
                    Some(json!("STRING")),
                ),
            ],
        ),
        (
            "propertyGroup.created",
            json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {"spares": {
                "updateType": "CREATE",
                "groupType": "TABULAR",
                "propertyNames": ["Serial"],
            }}}}}),
            vec![
                (
                    "/components/Sensor/propertyGroups/spares/propertyNames",
                    Some(json!(["Serial"])),
                ),
                (
                    "/components/Sensor/propertyGroups/readings/propertyNames",
                    Some(json!(["Serial"])),
                ),
            ],
        ),
        (
            "propertyGroup.patched",
            json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {"readings": {
                "updateType": "UPDATE",
                "propertyNames": [],
            }}}}}),
            vec![
                (
                    "/components/Sensor/propertyGroups/readings/propertyNames",
                    Some(json!([])),
                ),
                // `groupType` was not named, so the group keeps the stored one.
                (
                    "/components/Sensor/propertyGroups/readings/groupType",
                    Some(json!("TABULAR")),
                ),
            ],
        ),
        (
            "propertyGroup.deleted",
            json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {"readings": {
                "updateType": "DELETE",
            }}}}}),
            vec![
                ("/components/Sensor/propertyGroups/readings", None),
                (
                    "/components/Sensor/properties/Serial/value/stringValue",
                    Some(json!("sn-1")),
                ),
            ],
        ),
        (
            // A composite component is addressed by its `componentPath`; the
            // one created here resolves through the holding component type's
            // second `compositeComponentTypes` declaration.
            "composite.created",
            json!({"compositeComponentUpdates": {"Sensor/spare": {
                "updateType": "CREATE",
                "description": "the composite the update created",
            }}}),
            vec![
                (
                    "/components/Sensor/compositeComponents/spare/componentPath",
                    Some(json!("Sensor/spare")),
                ),
                (
                    "/components/Sensor/compositeComponents/spare/componentTypeId",
                    Some(json!(INNER_TYPE)),
                ),
                (
                    "/components/Sensor/compositeComponents/spare/description",
                    Some(json!("the composite the update created")),
                ),
                (
                    "/components/Sensor/compositeComponents/inner/description",
                    Some(json!("the composite the create persisted")),
                ),
            ],
        ),
        (
            "composite.patched",
            json!({"compositeComponentUpdates": {"Sensor/inner": {
                "updateType": "UPDATE",
                "description": "the composite description the update persisted",
            }}}),
            vec![
                (
                    "/components/Sensor/compositeComponents/inner/description",
                    Some(json!("the composite description the update persisted")),
                ),
                (
                    "/components/Sensor/compositeComponents/inner/componentTypeId",
                    Some(json!(INNER_TYPE)),
                ),
            ],
        ),
        (
            "composite.deleted",
            json!({"compositeComponentUpdates": {"Sensor/inner": {"updateType": "DELETE"}}}),
            vec![
                ("/components/Sensor/compositeComponents/inner", None),
                // The component that held it is still there.
                ("/components/Sensor/componentTypeId", Some(json!(SENSOR_TYPE))),
            ],
        ),
        (
            // A composite component hanging off a component this very call
            // creates: the composite entries are resolved against the
            // components as the same request leaves them.
            "composite.onAComponentThisCallCreates",
            json!({
                "componentUpdates": {"Fresh": {
                    "updateType": "CREATE",
                    "componentTypeId": SENSOR_TYPE,
                }},
                "compositeComponentUpdates": {"Fresh/inner": {
                    "updateType": "CREATE",
                    "description": "created in the same call as its component",
                }},
            }),
            vec![
                (
                    "/components/Fresh/compositeComponents/inner/componentPath",
                    Some(json!("Fresh/inner")),
                ),
                (
                    "/components/Fresh/compositeComponents/inner/description",
                    Some(json!("created in the same call as its component")),
                ),
            ],
        ),
        (
            // Many entries at once, of every kind, so the rule holds for a call
            // that carries more than one.
            "many.entriesAtOnce",
            json!({
                "entityName": "everything patched",
                "componentUpdates": {
                    "Sensor": {
                        "updateType": "UPDATE",
                        "propertyUpdates": {
                            "Serial": {"updateType": "RESET_VALUE"},
                            "Temperature": {
                                "updateType": "CREATE",
                                "definition": {"dataType": {"type": "DOUBLE"}},
                                "value": {"doubleValue": 3.0},
                            },
                        },
                        "propertyGroupUpdates": {"readings": {"updateType": "DELETE"}},
                    },
                    "Extra": {"updateType": "CREATE", "componentTypeId": INNER_TYPE},
                },
                "compositeComponentUpdates": {
                    "Sensor/inner": {"updateType": "DELETE"},
                    "Sensor/spare": {"updateType": "CREATE"},
                },
            }),
            vec![
                ("/entityName", Some(json!("everything patched"))),
                ("/components/Sensor/properties/Serial/value", None),
                (
                    "/components/Sensor/properties/Temperature/value/doubleValue",
                    Some(json!(3.0)),
                ),
                ("/components/Sensor/propertyGroups/readings", None),
                ("/components/Extra/componentTypeId", Some(json!(INNER_TYPE))),
                ("/components/Sensor/compositeComponents/inner", None),
                (
                    "/components/Sensor/compositeComponents/spare/componentPath",
                    Some(json!("Sensor/spare")),
                ),
            ],
        ),
    ];

    for (case, patch, expectations) in rows {
        create_baseline(&emulator, case).await;

        let updated = emulator.update_entity(WORKSPACE, case, patch).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateEntity, got {updated:?}"
        );

        let read = emulator.get_entity(WORKSPACE, case).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetEntity, got {read:?}"
        );
        for (path, expected) in &expectations {
            let actual = read.body.pointer(path);
            assert_eq!(
                actual,
                expected.as_ref(),
                "case {case:?}: expected {path} to be {expected:?}, got {actual:?} in {read:?}"
            );
        }
        // Patching never moves what addresses the entity.
        assert_eq!(
            read.member("entityId").as_str(),
            Some(case),
            "case {case:?}: a patch does not re-key the entity, got {read:?}"
        );
    }
}

/// RULE: an update answers the non-terminal `UPDATING` and the CLOCK settles the
/// entity back in `ACTIVE` once the settle span has elapsed — no caller has to
/// poll for the transition — while the update moves `updateDateTime` forward and
/// leaves `creationDateTime` where the create put it.
#[tokio::test]
async fn an_update_answers_updating_and_the_clock_settles_the_entity_back_in_active() {
    let emulator = seeded().await;

    // The rows differ in whether a reader observed the entity during the settle
    // span, because the transition is the clock's and not a read's, and in which
    // member the patch named, because the lifecycle is the call's and not the
    // patch's.
    let rows: Vec<(&str, bool, Value)> = vec![
        ("polled", true, json!({"description": "polled"})),
        ("unpolled", false, json!({"entityName": "unpolled"})),
        ("emptyPatch", true, json!({})),
    ];

    for (case, polled, patch) in rows {
        create_baseline(&emulator, case).await;
        let created = emulator.get_entity(WORKSPACE, case).await;
        assert_eq!(
            reported_state(&created),
            Some(STATE_ACTIVE),
            "case {case:?}: a created entity is ACTIVE before any update, got {created:?}"
        );
        let creation_date_time = created.member("creationDateTime").as_f64();

        let updated = emulator.update_entity(WORKSPACE, case, patch).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateEntity, got {updated:?}"
        );
        assert_eq!(
            updated.member("state").as_str(),
            Some(STATE_UPDATING),
            "case {case:?}: an update in flight answers UPDATING, got {updated:?}"
        );
        let update_date_time = updated.member("updateDateTime").as_f64();
        assert!(
            update_date_time.is_some(),
            "case {case:?}: expected a numeric updateDateTime, got {updated:?}"
        );

        if polled {
            let during = emulator.get_entity(WORKSPACE, case).await;
            assert_eq!(
                reported_state(&during),
                Some(STATE_UPDATING),
                "case {case:?}: a read inside the settle span still reports UPDATING, \
                 got {during:?}"
            );
            assert_eq!(
                during.member("updateDateTime").as_f64(),
                update_date_time,
                "case {case:?}: a read reports the update's own instant, got {during:?}"
            );
            assert_eq!(
                during.member("creationDateTime").as_f64(),
                creation_date_time,
                "case {case:?}: an update leaves creationDateTime alone, got {during:?}"
            );
            // Every reader of an entity's state agrees with `GetEntity`.
            let listed = emulator
                .list_entities(WORKSPACE, json!({"maxResults": 200}))
                .await;
            let summary = listed
                .member("entitySummaries")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|summary| summary["entityId"].as_str() == Some(case));
            assert_eq!(
                summary
                    .as_ref()
                    .and_then(|summary| summary["status"]["state"].as_str()),
                Some(STATE_UPDATING),
                "case {case:?}: ListEntities reports the same state, got {listed:?}"
            );
            let components = emulator.list_components(WORKSPACE, case, json!({})).await;
            assert_eq!(
                components.member("componentSummaries")[0]["status"]["state"].as_str(),
                Some(STATE_UPDATING),
                "case {case:?}: ListComponents reports the holding entity's state, \
                 got {components:?}"
            );
        }

        wait_until_settled();
        let settled = emulator.get_entity(WORKSPACE, case).await;
        assert_eq!(
            reported_state(&settled),
            Some(STATE_ACTIVE),
            "case {case:?}: the clock settles the entity back in ACTIVE, got {settled:?}"
        );
        // A read reports the transition; it does not cause it, so a second read
        // says the same thing.
        let again = emulator.get_entity(WORKSPACE, case).await;
        assert_eq!(
            reported_state(&again),
            Some(STATE_ACTIVE),
            "case {case:?}: a settled entity stays ACTIVE across reads, got {again:?}"
        );
        assert_eq!(
            again.member("updateDateTime").as_f64(),
            update_date_time,
            "case {case:?}: settling does not move updateDateTime, got {again:?}"
        );
    }
}

/// RULE: `parentEntityUpdate` re-parents the entity to the id its `UPDATE`
/// carries or detaches it to the root sentinel on `DELETE`, and refuses a parent
/// that does not name an entity of the workspace or that would make the stored
/// parent graph cyclic — the entity itself or any entity below it.
#[tokio::test]
async fn a_parent_entity_update_re_parents_detaches_or_is_refused() {
    // (case, the entity patched, the parentEntityUpdate, the parentEntityId the
    // read must report afterwards — `None` where the update must be refused
    // 400).
    let rows: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "detached",
            "branch",
            json!({"updateType": "DELETE"}),
            Some(ROOT_PARENT_ENTITY_ID),
        ),
        (
            // `DELETE` detaches whatever it names, so the id it carries is
            // beside the point.
            "detachedWithAnIdNamed",
            "branch",
            json!({"updateType": "DELETE", "parentEntityId": "other"}),
            Some(ROOT_PARENT_ENTITY_ID),
        ),
        (
            "reParented",
            "branch",
            json!({"updateType": "UPDATE", "parentEntityId": "other"}),
            Some("other"),
        ),
        (
            // Re-parenting an entity that is already a root to the sentinel: the
            // row the rule's boundary sits on.
            "reParentedToTheRootSentinel",
            "branch",
            json!({"updateType": "UPDATE", "parentEntityId": ROOT_PARENT_ENTITY_ID}),
            Some(ROOT_PARENT_ENTITY_ID),
        ),
        (
            // A leaf re-parented sideways, one level deeper than the row above.
            "leafReParented",
            "leaf",
            json!({"updateType": "UPDATE", "parentEntityId": "other"}),
            Some("other"),
        ),
        (
            "ownParent",
            "branch",
            json!({"updateType": "UPDATE", "parentEntityId": "branch"}),
            None,
        ),
        (
            "childAsParent",
            "branch",
            json!({"updateType": "UPDATE", "parentEntityId": "leaf"}),
            None,
        ),
        (
            // Two levels below, so the refusal is the whole subtree's and not
            // just the direct children's.
            "grandchildAsParent",
            "trunk",
            json!({"updateType": "UPDATE", "parentEntityId": "leaf"}),
            None,
        ),
        (
            "unknownParent",
            "branch",
            json!({"updateType": "UPDATE", "parentEntityId": "ghost"}),
            None,
        ),
        (
            "updateWithNoParentId",
            "branch",
            json!({"updateType": "UPDATE"}),
            None,
        ),
        (
            "unmodelledUpdateType",
            "branch",
            json!({"updateType": "MOVE", "parentEntityId": "other"}),
            None,
        ),
    ];

    for (case, entity_id, parent_entity_update, expected) in rows {
        // A fresh emulator per row: each row re-parents the same tree, so one
        // row's write must not be another's starting point.
        let emulator = seeded().await;
        // trunk -> branch -> leaf, plus a root with no children of its own.
        for (id, parent) in [
            ("trunk", None),
            ("other", None),
            ("branch", Some("trunk")),
            ("leaf", Some("branch")),
        ] {
            let mut body = json!({"entityId": id, "entityName": id});
            if let Some(parent) = parent {
                body["parentEntityId"] = json!(parent);
            }
            let created = emulator.create_entity(WORKSPACE, body).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the tree entity {id:?} must be created, got {created:?}"
            );
        }

        let patched = emulator
            .update_entity(
                WORKSPACE,
                entity_id,
                json!({"parentEntityUpdate": parent_entity_update}),
            )
            .await;
        let read = emulator.get_entity(WORKSPACE, entity_id).await;
        match expected {
            Some(parent_entity_id) => {
                assert_eq!(
                    patched.status, 200,
                    "case {case:?}: expected 200 from UpdateEntity, got {patched:?}"
                );
                assert_eq!(
                    read.member("parentEntityId").as_str(),
                    Some(parent_entity_id),
                    "case {case:?}: expected parentEntityId {parent_entity_id:?}, got {read:?}"
                );
                // The parent the entity moved to now has a child, and the one it
                // left has none: `hasChildEntities` is derived from the stored
                // parents, so the move shows up on both ends.
                if parent_entity_id != ROOT_PARENT_ENTITY_ID {
                    let parent = emulator.get_entity(WORKSPACE, parent_entity_id).await;
                    assert_eq!(
                        parent.member("hasChildEntities").as_bool(),
                        Some(true),
                        "case {case:?}: the new parent must report a child, got {parent:?}"
                    );
                }
                let left = emulator.get_entity(WORKSPACE, "trunk").await;
                let still_a_parent = entity_id != "branch";
                assert_eq!(
                    left.member("hasChildEntities").as_bool(),
                    Some(still_a_parent),
                    "case {case:?}: expected trunk.hasChildEntities {still_a_parent}, got {left:?}"
                );
            }
            None => {
                assert_eq!(
                    (patched.status, patched.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {case:?}: expected 400 ValidationException, got {patched:?}"
                );
                assert!(
                    !message(&patched).is_empty(),
                    "case {case:?}: a refusal names what to change, got {patched:?}"
                );
                // A refused update leaves the stored parent where it was.
                let expected_parent = match entity_id {
                    "trunk" => ROOT_PARENT_ENTITY_ID,
                    "leaf" => "branch",
                    _ => "trunk",
                };
                assert_eq!(
                    read.member("parentEntityId").as_str(),
                    Some(expected_parent),
                    "case {case:?}: a refused update keeps the stored parent, got {read:?}"
                );
            }
        }
    }
}

/// RULE: one entity's contents are bounded by this emulator's declared
/// ceilings — a write (create or patch) that would take an entity past the most
/// components it may hold, or a component past the most properties or property
/// groups it may hold, is refused 402 `ServiceQuotaExceededException` with
/// nothing stored, while a write that lands exactly ON a ceiling is served.
///
/// The ceilings are read off the service's own constants rather than restated
/// here, so a ceiling that moves moves these rows with it. The rows differ
/// along which ceiling the write would cross, along whether one call or a series
/// of calls crosses it, and along which writer crosses it.
#[tokio::test]
async fn a_write_past_a_declared_ceiling_is_refused_and_persists_nothing() {
    let emulator = seeded().await;

    /// `count` components named `bulk<n>`, each instantiating the inner type.
    fn components(count: usize) -> Value {
        let mut map = serde_json::Map::new();
        for index in 0..count {
            map.insert(
                format!("bulk{index}"),
                json!({"componentTypeId": INNER_TYPE}),
            );
        }
        Value::Object(map)
    }

    /// `count` property updates named `bulk<n>`, each with a definition a read
    /// can report.
    fn property_updates(count: usize) -> Value {
        let mut map = serde_json::Map::new();
        for index in 0..count {
            map.insert(
                format!("bulk{index}"),
                json!({"definition": {"dataType": {"type": "STRING"}}}),
            );
        }
        Value::Object(map)
    }

    // An entity holding exactly the most components it may: the write ON the
    // ceiling is served.
    let at_ceiling = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "atTheCeiling",
                "entityName": "at the ceiling",
                "components": components(MAX_COMPONENTS_PER_ENTITY),
            }),
        )
        .await;
    assert_eq!(
        at_ceiling.status, 200,
        "a create landing on the component ceiling is served, got {at_ceiling:?}"
    );
    // One more component on it crosses the ceiling.
    let over = emulator
        .update_entity(
            WORKSPACE,
            "atTheCeiling",
            json!({"componentUpdates": {"oneTooMany": {
                "updateType": "CREATE",
                "componentTypeId": INNER_TYPE,
            }}}),
        )
        .await;
    assert_eq!(
        (over.status, over.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "an update past the component ceiling is refused, got {over:?}"
    );
    let read = emulator.get_entity(WORKSPACE, "atTheCeiling").await;
    assert_eq!(
        read.body.pointer("/components/oneTooMany"),
        None,
        "the refused update persisted nothing, got {read:?}"
    );

    // (case, the writer's request, the status it must answer)
    let rows: Vec<(&str, Value, u16)> = vec![
        (
            "createPastTheComponentCeiling",
            json!({
                "entityId": "createPastComponents",
                "entityName": "too many components",
                "components": components(MAX_COMPONENTS_PER_ENTITY + 1),
            }),
            402,
        ),
        (
            "createOnThePropertyCeiling",
            json!({
                "entityId": "createOnProperties",
                "entityName": "properties on the ceiling",
                "components": {"Sensor": {
                    "componentTypeId": INNER_TYPE,
                    "properties": property_updates(MAX_PROPERTIES_PER_COMPONENT),
                }},
            }),
            200,
        ),
        (
            "createPastThePropertyCeiling",
            json!({
                "entityId": "createPastProperties",
                "entityName": "too many properties",
                "components": {"Sensor": {
                    "componentTypeId": INNER_TYPE,
                    "properties": property_updates(MAX_PROPERTIES_PER_COMPONENT + 1),
                }},
            }),
            402,
        ),
    ];
    for (case, body, status) in rows {
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, status,
            "case {case:?}: expected {status} from CreateEntity, got {created:?}"
        );
        if status != 200 {
            assert_eq!(
                created.error_type.as_deref(),
                Some("ServiceQuotaExceededException"),
                "case {case:?}: expected the modelled quota refusal, got {created:?}"
            );
            let listed = emulator
                .list_entities(WORKSPACE, json!({"maxResults": 200}))
                .await;
            let ids: Vec<&str> = listed
                .member("entitySummaries")
                .as_array()
                .map(|summaries| {
                    summaries
                        .iter()
                        .filter_map(|summary| summary["entityId"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                !ids.iter().any(|id| id.starts_with("createPast")),
                "case {case:?}: a refused create stored no entity, got {ids:?}"
            );
        }
    }

    // A series of small patches is bounded by the same ceiling as one big one:
    // the entity's stored contents are what the rule is applied to.
    let entity = "oneAtATime";
    create_baseline(&emulator, entity).await;
    let mut last = None;
    for index in 0..=MAX_PROPERTY_GROUPS_PER_COMPONENT {
        last = Some(
            emulator
                .update_entity(
                    WORKSPACE,
                    entity,
                    json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {
                        format!("group{index}"): {"updateType": "CREATE", "groupType": "TABULAR"},
                    }}}}),
                )
                .await,
        );
    }
    let last = last.expect("the loop sends at least one patch");
    assert_eq!(
        (last.status, last.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "the patch that crosses the property-group ceiling is refused, got {last:?}"
    );
}

/// RULE: removing a composite component removes the composite components
/// DECLARED INSIDE it, because each of those is addressed by a path through the
/// one that is gone — the same reading that makes removing a top-level component
/// remove the composite components it holds.
#[tokio::test]
async fn deleting_a_composite_component_removes_the_ones_below_it() {
    let emulator = seeded().await;
    const ENTITY: &str = "nestedComposites";
    create_baseline(&emulator, ENTITY).await;

    // A composite component two levels below the component: `Sensor/inner`
    // instantiates the inner type, which declares `leaf`.
    let nested = emulator
        .update_entity(
            WORKSPACE,
            ENTITY,
            json!({"compositeComponentUpdates": {"Sensor/inner/leaf": {
                "updateType": "CREATE",
                "description": "two levels down",
            }}}),
        )
        .await;
    assert_eq!(
        nested.status, 200,
        "expected 200 creating the nested composite component, got {nested:?}"
    );
    let read = emulator.get_entity(WORKSPACE, ENTITY).await;
    assert_eq!(
        read.body
            .pointer("/components/Sensor/compositeComponents/leaf/componentPath"),
        Some(&json!("Sensor/inner/leaf")),
        "the nested composite component must be stored under its own name, got {read:?}"
    );

    // Removing the one it hangs under takes it with it.
    let deleted = emulator
        .update_entity(
            WORKSPACE,
            ENTITY,
            json!({"compositeComponentUpdates": {"Sensor/inner": {"updateType": "DELETE"}}}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected 200 deleting the holding composite component, got {deleted:?}"
    );
    let read = emulator.get_entity(WORKSPACE, ENTITY).await;
    for name in ["inner", "leaf"] {
        assert_eq!(
            read.body
                .pointer(&format!("/components/Sensor/compositeComponents/{name}")),
            None,
            "expected compositeComponents.{name} to be gone, got {read:?}"
        );
    }
    // The component that held them is untouched.
    assert_eq!(
        read.body.pointer("/components/Sensor/componentTypeId"),
        Some(&json!(SENSOR_TYPE)),
        "the holding component survives, got {read:?}"
    );
    // And a `spare` created afterwards still resolves, so the retain above
    // removed only what sat below the deleted path.
    let spare = emulator
        .update_entity(
            WORKSPACE,
            ENTITY,
            json!({"compositeComponentUpdates": {"Sensor/spare": {"updateType": "CREATE"}}}),
        )
        .await;
    assert_eq!(
        spare.status, 200,
        "expected 200 creating a sibling composite component afterwards, got {spare:?}"
    );
}

/// RULE: a label naming something the store does not hold is a 404
/// `ResourceNotFoundException`; every other rule this operation owns is a 400
/// `ValidationException`; and either way the stored entity is left exactly as it
/// was, because the patch is built on a copy and committed last.
///
/// The rows differ along WHICH rule refuses the call — the two labels, each
/// update type applied to contents that cannot take it, an unmodelled update
/// type at each of the three levels that carries one, an unresolvable component
/// path, an abstract component type and a definition that no read could report.
#[tokio::test]
async fn a_refused_update_is_the_rules_own_refusal_and_persists_nothing() {
    let emulator = seeded().await;
    const ENTITY: &str = "refusals";
    create_baseline(&emulator, ENTITY).await;

    // (case, the workspace addressed, the entity addressed, the patch, the
    // status and `x-amzn-errortype` the refusal must carry)
    let rows: Vec<(&str, &str, &str, Value, u16, &str)> = vec![
        (
            "unknownWorkspace",
            "ghostWorkspace",
            ENTITY,
            json!({"description": "nope"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknownEntity",
            WORKSPACE,
            "ghostEntity",
            json!({"description": "nope"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "createAComponentThatIsThere",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"updateType": "CREATE"}}}),
            400,
            "ValidationException",
        ),
        (
            "updateAComponentThatIsNot",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Ghost": {"updateType": "UPDATE"}}}),
            400,
            "ValidationException",
        ),
        (
            "deleteAComponentThatIsNot",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Ghost": {"updateType": "DELETE"}}}),
            400,
            "ValidationException",
        ),
        (
            "deleteAPropertyThatIsNot",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Ghost": {
                "updateType": "DELETE",
            }}}}}),
            400,
            "ValidationException",
        ),
        (
            "resetAPropertyThatIsNot",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Ghost": {
                "updateType": "RESET_VALUE",
            }}}}}),
            400,
            "ValidationException",
        ),
        (
            "createAPropertyGroupThatIsThere",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {"readings": {
                "updateType": "CREATE",
            }}}}}),
            400,
            "ValidationException",
        ),
        (
            "deleteAPropertyGroupThatIsNot",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {"ghosts": {
                "updateType": "DELETE",
            }}}}}),
            400,
            "ValidationException",
        ),
        (
            "aDefinitionWithNoDataType",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Serial": {
                "definition": {"isRequiredInEntity": true},
            }}}}}),
            400,
            "ValidationException",
        ),
        (
            "anAbstractComponentType",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Abstract": {
                "updateType": "CREATE",
                "componentTypeId": ABSTRACT_TYPE,
            }}}),
            400,
            "ValidationException",
        ),
        (
            "anUnresolvableComponentPath",
            WORKSPACE,
            ENTITY,
            json!({"compositeComponentUpdates": {"Sensor/ghost": {"updateType": "CREATE"}}}),
            400,
            "ValidationException",
        ),
        (
            "aCompositeComponentThatIsThere",
            WORKSPACE,
            ENTITY,
            json!({"compositeComponentUpdates": {"Sensor/inner": {"updateType": "CREATE"}}}),
            400,
            "ValidationException",
        ),
        (
            "anUnmodelledComponentUpdateType",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"updateType": "REPLACE"}}}),
            400,
            "ValidationException",
        ),
        (
            "anUnmodelledPropertyUpdateType",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Serial": {
                "updateType": "REPLACE",
            }}}}}),
            400,
            "ValidationException",
        ),
        (
            "anUnmodelledPropertyGroupUpdateType",
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Sensor": {"propertyGroupUpdates": {"readings": {
                "updateType": "REPLACE",
            }}}}}),
            400,
            "ValidationException",
        ),
    ];

    for (case, workspace_id, entity_id, patch, status, error_type) in rows {
        let refused = emulator.update_entity(workspace_id, entity_id, patch).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (status, Some(error_type)),
            "case {case:?}: expected {status} {error_type}, got {refused:?}"
        );
        assert!(
            !message(&refused).is_empty(),
            "case {case:?}: a refusal is built from the request, got {refused:?}"
        );

        // Nothing the baseline persisted moved, and the entity is still in the
        // terminal state the create settled it in: a refused call opens no
        // update window.
        let read = emulator.get_entity(WORKSPACE, ENTITY).await;
        for (path, expected) in [
            ("/entityName", json!("the name the create persisted")),
            (
                "/description",
                json!("the description the create persisted"),
            ),
            (
                "/components/Sensor/properties/Serial/value/stringValue",
                json!("sn-1"),
            ),
            (
                "/components/Sensor/propertyGroups/readings/propertyNames",
                json!(["Serial"]),
            ),
            (
                "/components/Sensor/compositeComponents/inner/componentPath",
                json!("Sensor/inner"),
            ),
            ("/status/state", json!(STATE_ACTIVE)),
        ] {
            assert_eq!(
                read.body.pointer(path),
                Some(&expected),
                "case {case:?}: expected {path} to be unchanged at {expected:?}, got {read:?}"
            );
        }
        assert_eq!(
            read.body.pointer("/components/Abstract"),
            None,
            "case {case:?}: a refused create persists no component, got {read:?}"
        );
    }

    // The abstract refusal is the service's own construction, naming the
    // component type the call asked for — the same message a create's refusal
    // carries, since both apply one rule.
    let abstracted = emulator
        .update_entity(
            WORKSPACE,
            ENTITY,
            json!({"componentUpdates": {"Abstract": {
                "updateType": "CREATE",
                "componentTypeId": ABSTRACT_TYPE,
            }}}),
        )
        .await;
    assert_eq!(
        message(&abstracted),
        format!(
            "Abstract ComponentType {ABSTRACT_TYPE} can not be used for creating a component"
        ),
        "the abstract refusal names the component type the request asked for, got {abstracted:?}"
    );
}
