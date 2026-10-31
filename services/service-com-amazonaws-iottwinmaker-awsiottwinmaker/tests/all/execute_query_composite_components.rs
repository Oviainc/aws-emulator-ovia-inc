// SPDX-License-Identifier: CC0-1.0

//! `ExecuteQuery` over an entity's COMPOSITE components, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the same
//! entry the served emulator uses — against entities created and patched through
//! `CreateEntity` and `UpdateEntity`, so each row a query reports is a projection of
//! state this emulator actually persisted.
//!
//! The RULE: a composite component is a component of the entity just as the component
//! holding it is — it instantiates a component type and carries properties of its own —
//! so the graph a query runs over is derived from EVERY component an entity holds, at
//! whatever depth: each one's `componentTypeId` is a label of the entity's node, a
//! `relationshipValue` stored on any one of their properties is an edge of the graph, and
//! the node's `components` reports them all.
//!
//! The rows differ along what the rule generalises over: the DEPTH the component sits at
//! (top-level, one level down, two levels down), WHICH of the entity's components holds
//! it, HOW it came to be stored (`CreateEntity`'s `compositeComponents`, `UpdateEntity`'s
//! `compositeComponentUpdates`), whether the relationship it carries sits beside a
//! top-level one, and how many entities one component type labels.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case in this suite queries. Each test builds its own emulator, so
/// the id is shared without the cases sharing state.
const WORKSPACE: &str = "compositeGraph";

/// The component type a composite component of the fixture instantiates. It declares the
/// `RELATIONSHIP` property the fixture stores its relationships in.
const LEAF_TYPE: &str = "com.example.leaf";
/// A component type that is itself composite: it declares `leaf`, so a composite
/// component two levels below the entity's component resolves.
const DEEP_TYPE: &str = "com.example.deep";
/// The component type the fixture's top-level components instantiate, and whose
/// `compositeComponentTypes` every fixture path resolves through.
const RIG_TYPE: &str = "com.example.rig";
/// A component type declaring no composite component type at all.
const PLAIN_TYPE: &str = "com.example.plain";

/// The `RELATIONSHIP` property [`LEAF_TYPE`] declares, which the fixture's composite
/// components store their relationships in — so the label a hop over one matches.
const TARGET: &str = "Target";
/// A relationship property the fixture declares on a TOP-LEVEL component, so one entity
/// carries a relationship on each kind of component at once.
const FEEDS: &str = "Feeds";

/// One top-level component of a fixture entity: its name, the component type it
/// instantiates and the relationship properties the create declares on it as
/// `(propertyName, targetEntityId)`.
type SeedComponent = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
);

/// One composite component of a fixture entity: the `componentPath` it is created under
/// and the entity its [`TARGET`] property points at (`None` for one carrying no
/// property).
type SeedComposite = (&'static str, Option<&'static str>);

/// One fixture entity.
struct Fixture {
    entity_id: &'static str,
    /// The entity's top-level components.
    components: Vec<SeedComponent>,
    /// The composite components the CREATE carries.
    composite_components: Vec<SeedComposite>,
    /// The composite components a following `UpdateEntity` creates.
    composite_updates: Vec<SeedComposite>,
}

/// The fixture: one entity every relationship ends at, one holding a composite component
/// one level down BESIDE a top-level relationship, one holding it under the second of two
/// components, one holding it two levels down, and one whose composite component an
/// `UpdateEntity` created.
fn fixture() -> Vec<Fixture> {
    vec![
        Fixture {
            entity_id: "hub",
            components: vec![("Plain", PLAIN_TYPE, Vec::new())],
            composite_components: Vec::new(),
            composite_updates: Vec::new(),
        },
        Fixture {
            entity_id: "oneLevel",
            components: vec![("Rig", RIG_TYPE, vec![(FEEDS, "hub")])],
            composite_components: vec![("Rig/inner", Some("hub"))],
            composite_updates: Vec::new(),
        },
        Fixture {
            entity_id: "secondComponent",
            components: vec![
                ("Left", PLAIN_TYPE, Vec::new()),
                ("Right", RIG_TYPE, Vec::new()),
            ],
            composite_components: vec![("Right/inner", Some("oneLevel"))],
            composite_updates: Vec::new(),
        },
        Fixture {
            entity_id: "twoLevels",
            components: vec![("Rig", RIG_TYPE, Vec::new())],
            composite_components: vec![("Rig/nested", None), ("Rig/nested/leaf", Some("hub"))],
            composite_updates: Vec::new(),
        },
        Fixture {
            entity_id: "updatedComposite",
            components: vec![("Rig", RIG_TYPE, Vec::new())],
            composite_components: Vec::new(),
            composite_updates: vec![("Rig/spare", Some("hub"))],
        },
    ]
}

/// A relationship property as a create DECLARES it on a component whose component type
/// does not define it: the definition beside the value.
fn declared_relationship(property_name: &str, target_entity_id: &str) -> Value {
    json!({
        "definition": {
            "dataType": {
                "type": "RELATIONSHIP",
                "relationship": {"relationshipType": property_name},
            },
        },
        "value": {"relationshipValue": {"targetEntityId": target_entity_id}},
    })
}

/// The [`TARGET`] property of a composite component: the value alone, the definition
/// being the one [`LEAF_TYPE`] declares.
fn target_property(target_entity_id: &str) -> Value {
    json!({"value": {"relationshipValue": {"targetEntityId": target_entity_id}}})
}

/// Create `WORKSPACE`, the component types the fixture paths resolve through, and every
/// fixture entity — reading each one back, so a query reporting nothing cannot be an
/// entity that was never stored.
async fn seed() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "creating {WORKSPACE}: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            LEAF_TYPE,
            json!({"propertyDefinitions": {
                TARGET: {"dataType": {"type": "RELATIONSHIP"}},
                "Pressure": {"dataType": {"type": "DOUBLE"}},
            }}),
        ),
        (
            DEEP_TYPE,
            json!({"compositeComponentTypes": {"leaf": {"componentTypeId": LEAF_TYPE}}}),
        ),
        (
            RIG_TYPE,
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": LEAF_TYPE},
                "nested": {"componentTypeId": DEEP_TYPE},
                "spare": {"componentTypeId": LEAF_TYPE},
            }}),
        ),
        (PLAIN_TYPE, json!({})),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "creating {component_type_id}: expected 200, got {created:?}"
        );
    }

    for entity in fixture() {
        let mut components = json!({});
        for (component_name, component_type_id, relationships) in &entity.components {
            let mut properties = json!({});
            for (property_name, target_entity_id) in relationships {
                properties[*property_name] =
                    declared_relationship(property_name, target_entity_id);
            }
            components[*component_name] =
                json!({"componentTypeId": component_type_id, "properties": properties});
        }
        let mut composite_components = json!({});
        for (component_path, target_entity_id) in &entity.composite_components {
            let mut composite = json!({});
            if let Some(target_entity_id) = target_entity_id {
                composite["properties"] = json!({TARGET: target_property(target_entity_id)});
            }
            composite_components[*component_path] = composite;
        }
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity.entity_id,
                    "entityName": entity.entity_id,
                    "components": components,
                    "compositeComponents": composite_components,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "creating {}: expected 200, got {created:?}",
            entity.entity_id
        );

        for (component_path, target_entity_id) in &entity.composite_updates {
            let mut update = json!({"updateType": "CREATE"});
            if let Some(target_entity_id) = target_entity_id {
                update["propertyUpdates"] = json!({TARGET: target_property(target_entity_id)});
            }
            let updated = emulator
                .update_entity(
                    WORKSPACE,
                    entity.entity_id,
                    json!({"compositeComponentUpdates": {*component_path: update}}),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "patching {} at {component_path}: expected 200, got {updated:?}",
                entity.entity_id
            );
        }

        let read = emulator.get_entity(WORKSPACE, entity.entity_id).await;
        assert_eq!(
            read.status, 200,
            "reading {} back: expected 200, got {read:?}",
            entity.entity_id
        );
    }
    emulator
}

/// Run one statement against the fixture workspace, asserting it succeeded.
async fn query(emulator: &Emulator, statement: &str) -> Resp {
    let resp = emulator
        .execute_query(json!({"workspaceId": WORKSPACE, "queryStatement": statement}))
        .await;
    assert_eq!(resp.status, 200, "{statement:?}: expected 200, got {resp:?}");
    resp
}

/// The `rowData` lists of a response, in the order they were reported.
fn rows(resp: &Resp) -> Vec<Vec<Value>> {
    resp.member("rows")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|row| row["rowData"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// Every reported row as the text of its columns, which is what a `SELECT` of VALUE
/// columns reports.
fn text_rows(resp: &Resp) -> Vec<Vec<String>> {
    rows(resp)
        .iter()
        .map(|row| {
            row.iter()
                .map(|column| column.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .collect()
}

/// The first column of every reported row, as text.
fn first_column_text(resp: &Resp) -> Vec<String> {
    text_rows(resp)
        .iter()
        .map(|row| row.first().cloned().unwrap_or_default())
        .collect()
}

/// One component of an entity as its name, the component type it instantiates and the
/// `componentPath` it is addressed by — which only a composite component has.
type ReportedComponent = (String, String, Option<String>);

/// Every component `GetEntity` reports for an entity, at every path: its top-level
/// components and the composite components inside them.
fn components_of_read(body: &Value) -> BTreeSet<ReportedComponent> {
    let mut reported = BTreeSet::new();
    for (component_name, component) in body["components"].as_object().cloned().unwrap_or_default() {
        reported.insert((
            component_name,
            component["componentTypeId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            None,
        ));
        for (composite_name, composite) in component["compositeComponents"]
            .as_object()
            .cloned()
            .unwrap_or_default()
        {
            reported.insert((
                composite_name,
                composite["componentTypeId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                composite["componentPath"].as_str().map(str::to_string),
            ));
        }
    }
    reported
}

/// Every component a `NODE` column reports, read the same way.
fn components_of_node(node: &Value) -> BTreeSet<ReportedComponent> {
    node["components"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|component| {
            (
                component["componentName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                component["componentTypeId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                component["componentPath"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// RULE: a node is labelled with the `componentTypeId` of every component the entity
/// holds, so a label pattern selects the entities carrying a component of that type at
/// any path — a composite component's type included.
#[tokio::test]
async fn a_composite_components_component_type_labels_its_entity() {
    let emulator = seed().await;

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a composite component's type, one level down, two levels down and created by \
             an update",
            LEAF_TYPE,
            vec!["oneLevel", "secondComponent", "twoLevels", "updatedComposite"],
        ),
        (
            "a composite component's type that itself holds a composite component",
            DEEP_TYPE,
            vec!["twoLevels"],
        ),
        (
            "a top-level component's type, held by several entities",
            RIG_TYPE,
            vec!["oneLevel", "secondComponent", "twoLevels", "updatedComposite"],
        ),
        (
            "a top-level component's type, on one entity's only component and on the \
             first of another's two",
            PLAIN_TYPE,
            vec!["hub", "secondComponent"],
        ),
        (
            "a component type no component of the workspace instantiates",
            "com.example.absent",
            vec![],
        ),
    ];

    for (case, component_type_id, expected) in cases {
        let statement =
            format!("SELECT e.entityId FROM EntityGraph MATCH (e:{component_type_id})");
        let resp = query(&emulator, &statement).await;
        let selected = first_column_text(&resp);
        assert_eq!(
            selected, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {selected:?}"
        );
    }
}

/// RULE: a relationship stored on a property of ANY component of the entity is an edge of
/// the graph, labelled with the property's name and sourced at the component holding it.
#[tokio::test]
async fn a_relationship_on_a_composite_component_is_an_edge() {
    let emulator = seed().await;

    // `(sourceEntityId, sourceComponentName, targetEntityId)` per reported edge, in the
    // entity order a query reports its rows in.
    type EdgeCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str, &'a str)>);
    let cases: Vec<EdgeCase<'_>> = vec![
        (
            "the property every composite component of the fixture carries",
            TARGET,
            vec![
                ("oneLevel", "inner", "hub"),
                ("secondComponent", "inner", "oneLevel"),
                ("twoLevels", "leaf", "hub"),
                ("updatedComposite", "spare", "hub"),
            ],
        ),
        (
            "a property of a TOP-LEVEL component, beside a composite one on the same entity",
            FEEDS,
            vec![("oneLevel", "Rig", "hub")],
        ),
        (
            "a property name no component of the workspace stores",
            "Absent",
            vec![],
        ),
    ];

    for (case, relationship_name, expected) in cases {
        let statement = format!(
            "SELECT r FROM EntityGraph MATCH (a)-[r:{relationship_name}]->(b)"
        );
        let resp = query(&emulator, &statement).await;
        let reported: Vec<(String, String, String)> = rows(&resp)
            .iter()
            .map(|row| {
                let edge = row.first().cloned().unwrap_or_default();
                assert_eq!(
                    edge["relationshipName"].as_str(),
                    Some(relationship_name),
                    "case {case:?}: expected relationshipName {relationship_name:?}, got \
                     {edge:?}"
                );
                (
                    edge["sourceEntityId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    edge["sourceComponentName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    edge["targetEntityId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String, String)> = expected
            .iter()
            .map(|(source, component, target)| {
                (
                    (*source).to_string(),
                    (*component).to_string(),
                    (*target).to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {reported:?}"
        );
    }

    // The edges compose: a hop over a composite component's relationship reaches a node
    // whose own composite relationship is the next hop.
    let resp = query(
        &emulator,
        "SELECT a.entityId, b.entityId, c.entityId FROM EntityGraph \
         MATCH (a)-[:Target]->(b)-[:Target]->(c)",
    )
    .await;
    let chained = text_rows(&resp);
    assert_eq!(
        chained,
        vec![vec![
            "secondComponent".to_string(),
            "oneLevel".to_string(),
            "hub".to_string()
        ]],
        "two hops over composite relationships: got {chained:?}"
    );

    // A hop constrained by the label of the node it ends at walks the composite
    // component's edge as any other.
    let resp = query(
        &emulator,
        &format!(
            "SELECT a.entityId FROM EntityGraph MATCH (a)-[:{TARGET}]->(b:{PLAIN_TYPE})"
        ),
    )
    .await;
    let labelled = first_column_text(&resp);
    assert_eq!(
        labelled,
        vec![
            "oneLevel".to_string(),
            "twoLevels".to_string(),
            "updatedComposite".to_string()
        ],
        "a labelled hop over composite relationships: got {labelled:?}"
    );
}

/// RULE (the reporting side of the same rule): a `NODE` column reports every component
/// the entity holds, so the components a query reports are exactly the ones `GetEntity`
/// reports — a composite component among them, carrying the `componentPath` this service
/// addresses it by.
#[tokio::test]
async fn a_node_reports_the_composite_components_the_entity_holds() {
    let emulator = seed().await;

    for entity in fixture() {
        let read = emulator.get_entity(WORKSPACE, entity.entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetEntity, got {read:?}",
            entity.entity_id
        );
        let statement = format!(
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId = '{}'",
            entity.entity_id
        );
        let resp = query(&emulator, &statement).await;
        let selected = rows(&resp);
        assert_eq!(
            selected.len(),
            1,
            "case {:?}: one row for one entity, got {selected:?}",
            entity.entity_id
        );
        let node = selected[0].first().cloned().unwrap_or_default();
        let reported = components_of_node(&node);
        let expected = components_of_read(&read.body);
        assert_eq!(
            reported, expected,
            "case {:?}: expected the components GetEntity reports {expected:?}, got \
             {reported:?}",
            entity.entity_id
        );
        // The composite components the fixture stored are among them, with the type and
        // the path they were created under.
        for (component_path, _) in entity
            .composite_components
            .iter()
            .chain(entity.composite_updates.iter())
        {
            let name = component_path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            assert!(
                reported.iter().any(|(reported_name, _, reported_path)| {
                    reported_name == &name
                        && reported_path.as_deref() == Some(*component_path)
                }),
                "case {:?}: expected the composite component {component_path} among \
                 {reported:?}",
                entity.entity_id
            );
        }
    }
}

/// RULE (the same one, as a cross-reader invariant): every component type a READER of an
/// entity reports a component of labels that entity's node, so the knowledge graph cannot
/// disagree with `ListEntities`' `componentTypeId` filter — the sibling reader scoped by
/// the same question — about which components an entity holds.
#[tokio::test]
async fn a_label_selects_what_the_component_type_filter_selects() {
    let emulator = seed().await;

    let mut component_type_ids: BTreeSet<String> = BTreeSet::new();
    for entity in fixture() {
        let read = emulator.get_entity(WORKSPACE, entity.entity_id).await;
        assert_eq!(
            read.status, 200,
            "reading {:?}: expected 200, got {read:?}",
            entity.entity_id
        );
        for (_, component_type_id, _) in components_of_read(&read.body) {
            component_type_ids.insert(component_type_id);
        }
    }
    assert!(
        component_type_ids.contains(LEAF_TYPE),
        "the fixture must hold a composite component of {LEAF_TYPE}, got \
         {component_type_ids:?}"
    );

    for component_type_id in &component_type_ids {
        let listed = emulator
            .list_entities(
                WORKSPACE,
                json!({"filters": [{"componentTypeId": component_type_id}]}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {component_type_id:?}: expected 200 from ListEntities, got {listed:?}"
        );
        let filtered: Vec<String> = listed.member("entitySummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["entityId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let statement =
            format!("SELECT e.entityId FROM EntityGraph MATCH (e:{component_type_id})");
        let resp = query(&emulator, &statement).await;
        let selected = first_column_text(&resp);
        assert_eq!(
            selected, filtered,
            "case {component_type_id:?} ({statement:?}): expected the entities the \
             componentTypeId filter selects {filtered:?}, got {selected:?}"
        );
    }
}
