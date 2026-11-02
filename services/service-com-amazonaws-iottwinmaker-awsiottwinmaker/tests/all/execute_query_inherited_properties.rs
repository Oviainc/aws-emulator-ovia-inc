// SPDX-License-Identifier: CC0-1.0

//! What a QUERY sees of a component's properties, end to end over HTTP.
//!
//! RULE under test: every reader of an entity's component properties — the
//! `components[].properties` array of a `NODE` row and the relationships a
//! `MATCH` pattern hops over included — enumerates the properties the component
//! EFFECTIVELY has (the records the entity stores plus the ones the component
//! type it instantiates, or a type that type extends, defines) and reports for
//! each of them the value a read of it resolves: the most recent value written
//! through either plane, else the `defaultValue` its effective definition
//! declares, and nothing at all when there is neither. A property whose resolved
//! value carries a `relationshipValue` naming an entity of the same workspace is
//! therefore an edge, whether the component stores that value itself, inherits
//! it as its definition's default, or had it written through the time-series
//! data plane.
//!
//! The rows differ along what the rule generalises over: WHERE the definition
//! lives (the component type the component instantiates, a type that type
//! extends), WHERE the value comes from (the definition's default, a value the
//! entity's create stored, a data-plane write, a data-plane write over a stored
//! value, a `RESET_VALUE` falling back to the default, nothing at all), the DATA
//! TYPE it travels in (`STRING`, `DOUBLE`, `RELATIONSHIP`) and WHICH component
//! carries it (a top-level one and a composite one addressed by its
//! `componentPath`). Each row is cross-read through `ListProperties`, which
//! already resolves a component's properties this way, so the two readers are
//! pinned to one answer.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator};

/// The workspace every case below queries.
const WORKSPACE: &str = "queryinheritance";

/// The component type at the root of the chain: a string property with a default
/// and a relationship property with a default, neither of which any entity below
/// restates.
const BASE: &str = "com.example.base";
/// The component type the fixture's components instantiate. It EXTENDS [`BASE`],
/// so a component of it carries that type's definitions too.
const SENSOR: &str = "com.example.sensor";
/// A component type declaring a COMPOSITE component type of [`SENSOR`], so an
/// entity component of this type carries a composite component at `Rig/inner`.
const RIG: &str = "com.example.rig";

/// Instants one minute apart, as the ISO 8601 `Time` a data-plane write carries.
const T0: &str = "2026-05-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-05-01T00:01:00Z";

/// The target entity the inherited defaults point at.
const HUB: &str = "hub";
/// The other target entity, the one a stored value and a data-plane write point
/// at.
const SPARE: &str = "spare";

/// A `DataValue` carrying a relationship to `target_entity_id`.
fn relationship(target_entity_id: &str) -> Value {
    json!({"relationshipValue": {"targetEntityId": target_entity_id}})
}

/// Create the workspace, the component types the chain is made of and every
/// fixture entity, then make the data-plane writes the cases read back.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    // In reference order: `extendsFrom` and a composite component type name a
    // component type the workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    "Tag": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset-tag"},
                    },
                    "BaseLink": {
                        "dataType": {
                            "type": "RELATIONSHIP",
                            "relationship": {"relationshipType": "BaseLink"},
                        },
                        "defaultValue": relationship(HUB),
                    },
                },
            }),
        ),
        (
            SENSOR,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {
                    "Link": {
                        "dataType": {
                            "type": "RELATIONSHIP",
                            "relationship": {"relationshipType": "Link"},
                        },
                        "defaultValue": relationship(HUB),
                    },
                    // A relationship the time-series data plane writes, and a
                    // number it writes beside it.
                    "Feed": {
                        "dataType": {
                            "type": "RELATIONSHIP",
                            "relationship": {"relationshipType": "Feed"},
                        },
                        "isTimeSeries": true,
                    },
                    "Reading": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    // No default and no write: a property that reads as nothing.
                    "Untouched": {"dataType": {"type": "STRING"}},
                },
                // The write connector the data-plane writes below go THROUGH.
                "functions": data_writer_functions(),
            }),
        ),
        (
            RIG,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": SENSOR}}}),
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

    let entities: Vec<Value> = vec![
        // The two entities the relationships of this fixture end at.
        json!({"entityId": HUB, "entityName": HUB}),
        json!({"entityId": SPARE, "entityName": SPARE}),
        // A component that restates NOTHING: every property it has comes
        // through the type it instantiates or the type that type extends.
        json!({
            "entityId": "unit",
            "entityName": "unit",
            "components": {"Sensor": {"componentTypeId": SENSOR}},
        }),
        // A component that stores a value for properties whose definitions it
        // inherits.
        json!({
            "entityId": "declared",
            "entityName": "declared",
            "components": {"Sensor": {
                "componentTypeId": SENSOR,
                "properties": {
                    "Tag": {"value": {"stringValue": "own-tag"}},
                    "Link": {"value": relationship(SPARE)},
                    "Feed": {"value": relationship(SPARE)},
                },
            }},
        }),
        // A component whose stored values are RESET again, so each property
        // reads as the default its inherited definition declares.
        json!({
            "entityId": "resetunit",
            "entityName": "resetunit",
            "components": {"Sensor": {
                "componentTypeId": SENSOR,
                "properties": {
                    "Tag": {"value": {"stringValue": "temporary"}},
                    "Link": {"value": relationship(SPARE)},
                },
            }},
        }),
        // A composite component restating nothing either.
        json!({
            "entityId": "rig",
            "entityName": "rig",
            "components": {"Rig": {"componentTypeId": RIG}},
            "compositeComponents": {"Rig/inner": {}},
        }),
    ];
    for body in entities {
        let created = emulator.create_entity(WORKSPACE, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {body}: expected 200, got {created:?}"
        );
    }

    let reset = emulator
        .update_entity(
            WORKSPACE,
            "resetunit",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {
                "Tag": {"updateType": "RESET_VALUE"},
                "Link": {"updateType": "RESET_VALUE"},
            }}}}),
        )
        .await;
    assert_eq!(
        reset.status, 200,
        "UpdateEntity resetting resetunit: expected 200, got {reset:?}"
    );

    // The data-plane writes: a number and a relationship on a component that
    // stored neither, and a relationship on one whose create stored another
    // target — the data plane wrote last, so that write is what every reader
    // reports.
    let writes: Vec<(&str, &str, &str, Value)> = vec![
        ("unit", "Reading", T0, json!({"doubleValue": 21.5})),
        ("unit", "Feed", T0, relationship(SPARE)),
        ("declared", "Feed", T1, relationship(HUB)),
    ];
    for (entity_id, property, at, value) in writes {
        let written = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([{
                    "entityPropertyReference": {
                        "entityId": entity_id,
                        "componentName": "Sensor",
                        "propertyName": property,
                    },
                    "propertyValues": [{"time": at, "value": value}],
                }]),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "BatchPutPropertyValues {entity_id}.{property}: expected 200, got {written:?}"
        );
        let entries = written
            .member("errorEntries")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            entries.iter().all(|entry| entry["errors"]
                .as_array()
                .is_none_or(|errors| errors.is_empty())),
            "BatchPutPropertyValues {entity_id}.{property}: expected no errorEntries, \
             got {written:?}"
        );
    }
}

/// Run one statement against the fixture workspace, asserting it was served.
async fn query(emulator: &Emulator, statement: &str) -> Value {
    let resp = emulator
        .execute_query(json!({"workspaceId": WORKSPACE, "queryStatement": statement}))
        .await;
    assert_eq!(
        resp.status, 200,
        "{statement:?}: expected 200, got {resp:?}"
    );
    resp.member("rows").clone()
}

/// The `rowData` lists of a response, in the order they were reported.
fn row_data(rows: &Value) -> Vec<Vec<Value>> {
    rows.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| row["rowData"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// The properties a query reports for ONE component of ONE entity, as
/// `propertyName -> propertyValue` (`Value::Null` for a property reported with
/// no value).
async fn queried_properties(
    emulator: &Emulator,
    entity_id: &str,
    component_name: &str,
) -> BTreeMap<String, Value> {
    let statement =
        format!("SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId = '{entity_id}'");
    let rows = query(emulator, &statement).await;
    let rows = row_data(&rows);
    assert_eq!(
        rows.len(),
        1,
        "case {entity_id:?}: one row for one entity, got {rows:?}"
    );
    let node = rows[0].first().cloned().unwrap_or(Value::Null);
    let component = node["components"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|component| component["componentName"] == json!(component_name))
        .unwrap_or_else(|| {
            panic!("case {entity_id:?}: the node reports no component {component_name:?}: {node}")
        });
    component["properties"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|property| {
            (
                property["propertyName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                property["propertyValue"].clone(),
            )
        })
        .collect()
}

/// The properties `ListProperties` reports for the same component, as
/// `propertyName -> value` (the modelled `DataValue`, `Value::Null` where the
/// property reads as nothing).
async fn listed_properties(
    emulator: &Emulator,
    entity_id: &str,
    component_name: &str,
    component_path: Option<&str>,
) -> BTreeMap<String, Value> {
    let mut body = json!({"entityId": entity_id});
    match component_path {
        Some(component_path) => body["componentPath"] = json!(component_path),
        None => body["componentName"] = json!(component_name),
    }
    let listed = emulator.list_properties(WORKSPACE, body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "ListProperties {body}: expected 200, got {listed:?}"
    );
    listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|summary| {
            (
                summary["propertyName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary["value"].clone(),
            )
        })
        .collect()
}

/// One row of the property table: which component holds the property, what the
/// property reads as for every reader of a `DataValue`, and the document a query
/// row carries it as.
struct PropertyCase {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    entity_id: &'static str,
    /// The component's own name, which is how a `NODE` row names it.
    component_name: &'static str,
    /// The path `ListProperties` addresses it by, for a COMPOSITE component;
    /// `None` for a top-level one, addressed by name.
    component_path: Option<&'static str>,
    property: &'static str,
    /// The `DataValue` every reader of the property's value reports, `None`
    /// where it reads as nothing at all.
    value: Option<Value>,
    /// The document a `NODE` row's `propertyValue` carries for that value: the
    /// value itself, projected out of the member it travels in.
    projected: Option<Value>,
}

/// Every row of the rule, over the values and the components the fixture holds.
fn property_cases() -> Vec<PropertyCase> {
    vec![
        PropertyCase {
            what: "a string whose definition and default live on the EXTENDED type",
            entity_id: "unit",
            component_name: "Sensor",
            component_path: None,
            property: "Tag",
            value: Some(json!({"stringValue": "unset-tag"})),
            projected: Some(json!("unset-tag")),
        },
        PropertyCase {
            what: "a relationship defaulted on the extended type",
            entity_id: "unit",
            component_name: "Sensor",
            component_path: None,
            property: "BaseLink",
            value: Some(relationship(HUB)),
            projected: Some(json!({"targetEntityId": HUB})),
        },
        PropertyCase {
            what: "a relationship defaulted on the INSTANTIATED type",
            entity_id: "unit",
            component_name: "Sensor",
            component_path: None,
            property: "Link",
            value: Some(relationship(HUB)),
            projected: Some(json!({"targetEntityId": HUB})),
        },
        PropertyCase {
            what: "an inherited property with no write and no default reads as nothing",
            entity_id: "unit",
            component_name: "Sensor",
            component_path: None,
            property: "Untouched",
            value: None,
            projected: None,
        },
        PropertyCase {
            what: "a number the time-series data plane wrote to an inherited property",
            entity_id: "unit",
            component_name: "Sensor",
            component_path: None,
            property: "Reading",
            value: Some(json!({"doubleValue": 21.5})),
            projected: Some(json!(21.5)),
        },
        PropertyCase {
            what: "a relationship the data plane wrote to an inherited property",
            entity_id: "unit",
            component_name: "Sensor",
            component_path: None,
            property: "Feed",
            value: Some(relationship(SPARE)),
            projected: Some(json!({"targetEntityId": SPARE})),
        },
        PropertyCase {
            what: "a value the component stores for a property it inherits the definition of",
            entity_id: "declared",
            component_name: "Sensor",
            component_path: None,
            property: "Tag",
            value: Some(json!({"stringValue": "own-tag"})),
            projected: Some(json!("own-tag")),
        },
        PropertyCase {
            what: "a stored relationship wins over the default the type declares",
            entity_id: "declared",
            component_name: "Sensor",
            component_path: None,
            property: "Link",
            value: Some(relationship(SPARE)),
            projected: Some(json!({"targetEntityId": SPARE})),
        },
        PropertyCase {
            what: "a data-plane write supersedes the stored relationship",
            entity_id: "declared",
            component_name: "Sensor",
            component_path: None,
            property: "Feed",
            value: Some(relationship(HUB)),
            projected: Some(json!({"targetEntityId": HUB})),
        },
        PropertyCase {
            what: "a reset leaves the string default the inherited definition declares",
            entity_id: "resetunit",
            component_name: "Sensor",
            component_path: None,
            property: "Tag",
            value: Some(json!({"stringValue": "unset-tag"})),
            projected: Some(json!("unset-tag")),
        },
        PropertyCase {
            what: "a reset leaves the relationship default the inherited definition declares",
            entity_id: "resetunit",
            component_name: "Sensor",
            component_path: None,
            property: "Link",
            value: Some(relationship(HUB)),
            projected: Some(json!({"targetEntityId": HUB})),
        },
        PropertyCase {
            what: "a COMPOSITE component inherits its type's string property too",
            entity_id: "rig",
            component_name: "inner",
            component_path: Some("Rig/inner"),
            property: "Tag",
            value: Some(json!({"stringValue": "unset-tag"})),
            projected: Some(json!("unset-tag")),
        },
        PropertyCase {
            what: "a composite component's inherited relationship",
            entity_id: "rig",
            component_name: "inner",
            component_path: Some("Rig/inner"),
            property: "Link",
            value: Some(relationship(HUB)),
            projected: Some(json!({"targetEntityId": HUB})),
        },
    ]
}

/// RULE: a `NODE` row reports every property its component HAS — the records the
/// entity stores and the ones its component type gives it — each with the value
/// a read of that property resolves.
#[tokio::test]
async fn a_node_reports_every_property_its_component_has_with_the_value_it_reads_as() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in property_cases() {
        let queried = queried_properties(&emulator, case.entity_id, case.component_name).await;
        let listed = listed_properties(
            &emulator,
            case.entity_id,
            case.component_name,
            case.component_path,
        )
        .await;

        // The two readers enumerate the same set of properties: the component's
        // effective one, not the part its entity's create restated.
        let queried_names: BTreeSet<&String> = queried.keys().collect();
        let listed_names: BTreeSet<&String> = listed.keys().collect();
        assert_eq!(
            queried_names, listed_names,
            "case {:?}: a query must report the properties ListProperties reports, \
             got {queried:?} against {listed:?}",
            case.what
        );

        let reported = queried.get(case.property).cloned().unwrap_or_else(|| {
            panic!(
                "case {:?}: the query reported no property {:?}, got {queried:?}",
                case.what, case.property
            )
        });
        match &case.projected {
            Some(expected) => assert_eq!(
                reported, *expected,
                "case {:?}: expected propertyValue {expected}, got {reported:?}",
                case.what
            ),
            None => assert!(
                reported.is_null(),
                "case {:?}: expected no propertyValue, got {reported:?}",
                case.what
            ),
        }

        // And the DataValue the sibling reader reports for it is the one the
        // projection above is of.
        let listed_value = listed.get(case.property).cloned().unwrap_or(Value::Null);
        match &case.value {
            Some(expected) => assert_eq!(
                listed_value, *expected,
                "case {:?}: ListProperties expected {expected}, got {listed_value:?}",
                case.what
            ),
            None => assert!(
                listed_value.is_null(),
                "case {:?}: ListProperties expected no value, got {listed_value:?}",
                case.what
            ),
        }
    }
}

/// One row of the hop table: the statement and the
/// `(sourceEntityId, targetEntityId, sourceComponentName)` of every relationship
/// it must select.
struct HopCase {
    what: &'static str,
    statement: &'static str,
    expected: Vec<(&'static str, &'static str, &'static str)>,
}

/// RULE: a hop matches a relationship the component's property READS AS, however
/// that value got there — stored on the entity, inherited as the definition's
/// default, or written through the time-series data plane.
#[tokio::test]
async fn a_hop_matches_the_relationship_a_property_reads_as() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let cases = vec![
        HopCase {
            what: "a relationship defaulted on the instantiated type, overridden where the \
                   component stored one",
            statement: "SELECT r.sourceEntityId, r.targetEntityId, r.sourceComponentName \
                        FROM EntityGraph MATCH (a)-[r:Link]->(b)",
            expected: vec![
                ("declared", SPARE, "Sensor"),
                ("resetunit", HUB, "Sensor"),
                ("rig", HUB, "inner"),
                ("unit", HUB, "Sensor"),
            ],
        },
        HopCase {
            what: "a relationship defaulted on the EXTENDED type reaches every component of \
                   the chain",
            statement: "SELECT r.sourceEntityId, r.targetEntityId, r.sourceComponentName \
                        FROM EntityGraph MATCH (a)-[r:BaseLink]->(b)",
            expected: vec![
                ("declared", HUB, "Sensor"),
                ("resetunit", HUB, "Sensor"),
                ("rig", HUB, "inner"),
                ("unit", HUB, "Sensor"),
            ],
        },
        HopCase {
            what: "a relationship the data plane wrote, which supersedes the stored one",
            statement: "SELECT r.sourceEntityId, r.targetEntityId, r.sourceComponentName \
                        FROM EntityGraph MATCH (a)-[r:Feed]->(b)",
            expected: vec![("declared", HUB, "Sensor"), ("unit", SPARE, "Sensor")],
        },
        HopCase {
            what: "a property that reads as no value at all is no relationship",
            statement: "SELECT r.sourceEntityId, r.targetEntityId, r.sourceComponentName \
                        FROM EntityGraph MATCH (a)-[r:Untouched]->(b)",
            expected: Vec::new(),
        },
        HopCase {
            what: "a property whose value is a number is no relationship",
            statement: "SELECT r.sourceEntityId, r.targetEntityId, r.sourceComponentName \
                        FROM EntityGraph MATCH (a)-[r:Reading]->(b)",
            expected: Vec::new(),
        },
    ];

    for case in cases {
        let rows = query(&emulator, case.statement).await;
        let selected: BTreeSet<(String, String, String)> = row_data(&rows)
            .iter()
            .map(|row| {
                let column = |at: usize| {
                    row.get(at)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                (column(0), column(1), column(2))
            })
            .collect();
        let expected: BTreeSet<(String, String, String)> = case
            .expected
            .iter()
            .map(|(source, target, component)| {
                (
                    (*source).to_string(),
                    (*target).to_string(),
                    (*component).to_string(),
                )
            })
            .collect();
        assert_eq!(
            selected, expected,
            "case {:?} ({:?}): expected {expected:?}, got {selected:?}",
            case.what, case.statement
        );
    }
}

/// RULE: a query is a READ — running one twice over unchanged state reports the
/// same rows, and nothing it resolved (a default, a series' latest value) is
/// written back to the component.
#[tokio::test]
async fn a_query_resolving_inherited_values_writes_nothing() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let statement = "SELECT r.sourceEntityId, r.targetEntityId FROM EntityGraph \
                     MATCH (a)-[r:Link]->(b)";
    let first = query(&emulator, statement).await;
    let queried_before = queried_properties(&emulator, "unit", "Sensor").await;
    let second = query(&emulator, statement).await;
    assert_eq!(
        row_data(&first),
        row_data(&second),
        "two queries over unchanged state: expected the same rows"
    );
    let queried_after = queried_properties(&emulator, "unit", "Sensor").await;
    assert_eq!(
        queried_before, queried_after,
        "a query must leave the component's properties as they were"
    );

    // The entity itself still stores NO property record for a property it only
    // inherits: the value a query reported for it was resolved, not stored.
    let entity = emulator.get_entity(WORKSPACE, "unit").await;
    assert_eq!(
        entity.status, 200,
        "GetEntity unit: expected 200, got {entity:?}"
    );
    let definition = &entity.member("components")["Sensor"]["properties"]["Tag"]["definition"];
    assert_eq!(
        definition["isInherited"],
        json!(true),
        "GetEntity unit: Tag must still be an INHERITED definition, got {definition:?}"
    );
}
