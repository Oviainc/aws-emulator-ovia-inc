// SPDX-License-Identifier: CC0-1.0

//! `ExecuteQuery` over the entity HIERARCHY, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the same
//! entry the served emulator uses — against entities created through `CreateEntity`, so
//! each row a query reports is a projection of state this emulator actually persisted.
//!
//! The RULE: the graph a query runs over carries every relationship this service stores
//! between two entities of the workspace, which includes the hierarchy an entity's stored
//! parent records — a built-in relationship from the child to its parent, labelled
//! `isChildOf` — and not only the relationships a caller modelled as component
//! properties. The cases differ along the direction and the label of the hop, the number
//! of hops, the kind of relationship each hop walks, the end the predicate constrains and
//! the write that changed the stored hierarchy.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case in this suite queries. Each test builds its own emulator, so
/// the id is shared without the cases sharing state.
const WORKSPACE: &str = "hierarchyWs";

/// The component type the fixture labels its mixer node with, so a hierarchy hop can be
/// constrained by a node label too.
const MIXER_TYPE: &str = "com.example.mixer";

/// The label of the relationship the service keeps for the entity hierarchy, as AWS's
/// knowledge graph names it.
const IS_CHILD_OF: &str = "isChildOf";

/// One fixture entity: its id, the id it is stored under as a parent (`None` for an
/// entity created without one) and, where it has a component, that component's type and
/// the relationship PROPERTY stored on it as `(propertyName, targetEntityId)`.
struct Fixture {
    entity_id: &'static str,
    parent_entity_id: Option<&'static str>,
    component: Option<(&'static str, Vec<(&'static str, &'static str)>)>,
}

/// The fixture hierarchy: a site holding a line, the line holding a mixer and an oven,
/// one entity outside the hierarchy altogether, and — on the mixer — a relationship
/// PROPERTY beside its stored parent, so a query sees both kinds of relationship at once.
///
/// Listed in creation order: a parent is created before the entity stored under it,
/// which is what `CreateEntity` requires of a named parent.
fn fixture() -> Vec<Fixture> {
    vec![
        Fixture {
            entity_id: "site",
            parent_entity_id: None,
            component: None,
        },
        Fixture {
            entity_id: "line",
            parent_entity_id: Some("site"),
            component: None,
        },
        Fixture {
            entity_id: "oven",
            parent_entity_id: Some("line"),
            component: None,
        },
        Fixture {
            entity_id: "mixer",
            parent_entity_id: Some("line"),
            component: Some((MIXER_TYPE, vec![("feeds", "oven")])),
        },
        Fixture {
            entity_id: "spare",
            parent_entity_id: None,
            component: None,
        },
    ]
}

/// Create `WORKSPACE` and its fixture entities.
async fn seed() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "creating {WORKSPACE}: expected 200, got {created:?}"
    );
    for entity in fixture() {
        create(&emulator, entity.entity_id, entity.parent_entity_id, {
            let mut body = json!({});
            if let Some((type_id, relationships)) = &entity.component {
                let mut properties = json!({});
                for (property_name, target_entity_id) in relationships {
                    properties[*property_name] = json!({
                        "definition": {
                            "dataType": {
                                "type": "RELATIONSHIP",
                                "relationship": {"relationshipType": property_name},
                            },
                        },
                        "value": {"relationshipValue": {"targetEntityId": target_entity_id}},
                    });
                }
                body["components"] = json!({
                    "primary": {"componentTypeId": type_id, "properties": properties},
                });
            }
            body
        })
        .await;
    }
    emulator
}

/// `CreateEntity` for `entity_id`, stored under `parent_entity_id` when one is named,
/// carrying whatever `extra` adds to the request.
async fn create(
    emulator: &Emulator,
    entity_id: &str,
    parent_entity_id: Option<&str>,
    extra: Value,
) {
    let mut body = json!({"entityId": entity_id, "entityName": entity_id});
    if let Some(parent_entity_id) = parent_entity_id {
        body["parentEntityId"] = json!(parent_entity_id);
    }
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            body[name.as_str()] = value.clone();
        }
    }
    let created = emulator.create_entity(WORKSPACE, body).await;
    assert_eq!(
        created.status, 200,
        "creating {entity_id:?}: expected 200, got {created:?}"
    );
}

/// Run one statement against the fixture workspace, asserting it succeeded.
async fn query(emulator: &Emulator, statement: &str) -> Resp {
    let resp = emulator
        .execute_query(json!({"workspaceId": WORKSPACE, "queryStatement": statement}))
        .await;
    assert_eq!(
        resp.status, 200,
        "{statement:?}: expected 200, got {resp:?}"
    );
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

/// The first column of every row, as text.
fn first_column(resp: &Resp) -> Vec<String> {
    text_rows(resp)
        .iter()
        .map(|row| row.first().cloned().unwrap_or_default())
        .collect()
}

/// One row of a table whose expected answer is a pair of columns per reported row: the
/// case name, the statement, and the `(first column, second column)` of each row.
type PairCase<'a> = (&'a str, &'a str, Vec<(&'static str, &'static str)>);

fn pairs(expected: &[(&str, &str)]) -> Vec<Vec<String>> {
    expected
        .iter()
        .map(|(left, right)| vec![(*left).to_string(), (*right).to_string()])
        .collect()
}

/// RULE: the hierarchy an entity's stored parent records is a relationship of the graph,
/// so a hop over it selects one row per stored parent/child pair — labelled with the
/// built-in relationship name or unlabelled, in either direction, alone or composed with
/// a relationship stored as a property — while an entity with no stored parent entity has
/// no such relationship at all.
#[tokio::test]
async fn the_entity_hierarchy_is_a_relationship_of_the_graph() {
    let emulator = seed().await;

    // Bound before the table so the table holds no reference to a temporary.
    let labelled_child_node = format!(
        "SELECT child.entityId, parent.entityId FROM EntityGraph \
         MATCH (child:{MIXER_TYPE})-[:isChildOf]->(parent)"
    );
    // `(the first column, the second column)` per reported row.
    let cases: Vec<PairCase> = vec![
        (
            "every stored parent/child pair",
            "SELECT child.entityId, parent.entityId FROM EntityGraph \
             MATCH (child)-[:isChildOf]->(parent)",
            vec![("line", "site"), ("mixer", "line"), ("oven", "line")],
        ),
        (
            // The pairs above, beside the one relationship a property stores.
            "an unlabelled hop walks the hierarchy too",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[r]->(b)",
            vec![
                ("line", "site"),
                ("mixer", "line"),
                ("mixer", "oven"),
                ("oven", "line"),
            ],
        ),
        (
            "and so does an anonymous one",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)->(b)",
            vec![
                ("line", "site"),
                ("mixer", "line"),
                ("mixer", "oven"),
                ("oven", "line"),
            ],
        ),
        (
            "read incoming, a parent reports its children",
            "SELECT parent.entityId, child.entityId FROM EntityGraph \
             MATCH (parent)<-[:isChildOf]-(child)",
            vec![("line", "mixer"), ("line", "oven"), ("site", "line")],
        ),
        (
            "an undirected hop matches both ways round",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:isChildOf]-(b)",
            vec![
                ("line", "site"),
                ("line", "mixer"),
                ("line", "oven"),
                ("mixer", "line"),
                ("oven", "line"),
                ("site", "line"),
            ],
        ),
        (
            "two hierarchy hops reach the grandparent",
            "SELECT c.entityId, g.entityId FROM EntityGraph \
             MATCH (c)-[:isChildOf]->(p)-[:isChildOf]->(g)",
            vec![("mixer", "site"), ("oven", "site")],
        ),
        (
            "two patterns joined on the shared parent",
            "SELECT a.entityId, c.entityId FROM EntityGraph \
             MATCH (a)-[:isChildOf]->(b), (b)-[:isChildOf]->(c)",
            vec![("mixer", "site"), ("oven", "site")],
        ),
        (
            "a property relationship then a hierarchy one",
            "SELECT a.entityId, c.entityId FROM EntityGraph \
             MATCH (a)-[:feeds]->(b)-[:isChildOf]->(c)",
            vec![("mixer", "line")],
        ),
        (
            "a hierarchy hop constrained at the parent end",
            "SELECT child.entityId, parent.entityId FROM EntityGraph \
             MATCH (child)-[:isChildOf]->(parent) WHERE parent.entityId = 'line'",
            vec![("mixer", "line"), ("oven", "line")],
        ),
        (
            "a hierarchy hop constrained at the child end",
            "SELECT child.entityId, parent.entityId FROM EntityGraph \
             MATCH (child)-[:isChildOf]->(parent) WHERE child.entityName = 'oven'",
            vec![("oven", "line")],
        ),
        (
            "a hierarchy hop whose child end is labelled by its component type",
            &labelled_child_node,
            vec![("mixer", "line")],
        ),
        (
            // The root sentinel names no entity, so the top of the hierarchy has no
            // relationship to walk.
            "an entity created without a parent has no hierarchy relationship",
            "SELECT child.entityId, parent.entityId FROM EntityGraph \
             MATCH (child)-[:isChildOf]->(parent) WHERE child.entityId = 'site'",
            Vec::new(),
        ),
        (
            "nor has an entity outside the hierarchy either way round",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:isChildOf]-(b) \
             WHERE a.entityId = 'spare'",
            Vec::new(),
        ),
        (
            "a label no relationship of the graph carries still selects nothing",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:isParentOf]->(b)",
            Vec::new(),
        ),
    ];

    for (case, statement, expected) in cases {
        let resp = query(&emulator, statement).await;
        let reported = text_rows(&resp);
        let expected = pairs(&expected);
        assert_eq!(
            reported, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: an `EDGE` column reports the hierarchy relationship the way it reports any
/// other — its name and the entity at each of its ends — and reports no component at
/// either end, because the hierarchy is held between the entities themselves rather than
/// on a component of one of them, which makes that member MISSING.
#[tokio::test]
async fn an_edge_column_reports_the_hierarchy_relationship() {
    let emulator = seed().await;

    let resp = query(
        &emulator,
        "SELECT r FROM EntityGraph MATCH (child)-[r:isChildOf]->(parent) \
         WHERE child.entityId = 'mixer'",
    )
    .await;
    let selected = rows(&resp);
    assert_eq!(
        selected.len(),
        1,
        "the mixer has one stored parent, got {selected:?}"
    );
    let edge = selected[0].first().expect("an EDGE column").clone();
    for (member, expected) in [
        ("relationshipName", IS_CHILD_OF),
        ("sourceEntityId", "mixer"),
        ("targetEntityId", "line"),
    ] {
        assert_eq!(
            edge[member].as_str(),
            Some(expected),
            "case {member:?}: expected {expected:?}, got {edge:?}"
        );
    }
    for member in ["sourceComponentName", "targetComponentName"] {
        assert!(
            edge.get(member).is_none(),
            "case {member:?}: the hierarchy relationship is held on no component, \
             so the member must be absent, got {edge:?}"
        );
    }

    // The columns of an unlabelled hop report each relationship of the graph, of either
    // kind, by name and by the entities at its ends.
    let resp = query(
        &emulator,
        "SELECT r.relationshipName, r.sourceEntityId, r.targetEntityId FROM EntityGraph \
         MATCH (a)-[r]->(b)",
    )
    .await;
    let reported = text_rows(&resp);
    let expected = [
        [IS_CHILD_OF, "line", "site"],
        [IS_CHILD_OF, "mixer", "line"],
        ["feeds", "mixer", "oven"],
        [IS_CHILD_OF, "oven", "line"],
    ];
    let expected: Vec<Vec<String>> = expected
        .iter()
        .map(|row| row.iter().map(|value| (*value).to_string()).collect())
        .collect();
    assert_eq!(
        reported, expected,
        "every relationship of the graph: expected {expected:?}, got {reported:?}"
    );

    // Which the predicates on that MISSING member partition: the hierarchy
    // relationships on one side, the ones a property stores on the other.
    for (case, statement, expected) in [
        (
            "no source component",
            "SELECT r.relationshipName FROM EntityGraph MATCH (a)-[r]->(b) \
             WHERE r.sourceComponentName IS NULL",
            vec![IS_CHILD_OF, IS_CHILD_OF, IS_CHILD_OF],
        ),
        (
            "a source component",
            "SELECT r.relationshipName FROM EntityGraph MATCH (a)-[r]->(b) \
             WHERE r.sourceComponentName IS NOT NULL",
            vec!["feeds"],
        ),
    ] {
        let resp = query(&emulator, statement).await;
        let reported = first_column(&resp);
        let expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: the hierarchy the graph reports is the hierarchy the service STORES, so it
/// agrees with what `GetEntity` reports for every entity and it moves with every write
/// that moves the stored parent — a create under a parent, a re-parenting update, a
/// detaching one and a delete.
#[tokio::test]
async fn the_reported_hierarchy_follows_the_stored_one() {
    let emulator = seed().await;

    // What the query reports for an entity is what the entity itself reports: its
    // `parentEntityId` (nothing, for an entity stored under the root sentinel) and
    // whether anything is stored under it.
    for entity in fixture() {
        let read = emulator.get_entity(WORKSPACE, entity.entity_id).await;
        assert_eq!(
            read.status, 200,
            "reading {}: expected 200, got {read:?}",
            entity.entity_id
        );
        let parent = parents_of(&emulator, entity.entity_id).await;
        let expected: Vec<String> = entity
            .parent_entity_id
            .map(|parent| vec![parent.to_string()])
            .unwrap_or_default();
        assert_eq!(
            parent, expected,
            "case {:?}: the parent the query reports must be the stored one \
             ({:?} from GetEntity), got {parent:?}",
            entity.entity_id,
            read.member("parentEntityId")
        );
        let children = children_of(&emulator, entity.entity_id).await;
        assert_eq!(
            !children.is_empty(),
            read.member("hasChildEntities").as_bool().unwrap_or(false),
            "case {:?}: the children the query reports ({children:?}) must agree with \
             hasChildEntities, got {read:?}",
            entity.entity_id
        );
    }

    // A create under a parent adds the relationship.
    create(&emulator, "pallet", Some("oven"), json!({})).await;
    let children = children_of(&emulator, "oven").await;
    assert_eq!(
        children,
        vec!["pallet".to_string()],
        "case \"created under a parent\": expected [\"pallet\"], got {children:?}"
    );

    // Re-parenting moves it.
    let patched = emulator
        .update_entity(
            WORKSPACE,
            "pallet",
            json!({"parentEntityUpdate": {"updateType": "UPDATE", "parentEntityId": "site"}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "case \"re-parented\": expected 200 from UpdateEntity, got {patched:?}"
    );
    let parents = parents_of(&emulator, "pallet").await;
    assert_eq!(
        parents,
        vec!["site".to_string()],
        "case \"re-parented\": expected [\"site\"], got {parents:?}"
    );
    let children = children_of(&emulator, "oven").await;
    assert!(
        children.is_empty(),
        "case \"re-parented\": the former parent keeps no child, got {children:?}"
    );

    // Detaching to the root sentinel takes it out of the graph's hierarchy.
    let patched = emulator
        .update_entity(
            WORKSPACE,
            "pallet",
            json!({"parentEntityUpdate": {"updateType": "DELETE"}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "case \"detached\": expected 200 from UpdateEntity, got {patched:?}"
    );
    let parents = parents_of(&emulator, "pallet").await;
    assert!(
        parents.is_empty(),
        "case \"detached\": an entity at the root has no parent relationship, got {parents:?}"
    );

    // And a deleted child takes its relationship with it.
    let deleted = emulator.delete_entity(WORKSPACE, "mixer", false).await;
    assert_eq!(
        deleted.status, 200,
        "case \"deleted\": expected 200 from DeleteEntity, got {deleted:?}"
    );
    let children = children_of(&emulator, "line").await;
    assert_eq!(
        children,
        vec!["oven".to_string()],
        "case \"deleted\": only the remaining child is reported, got {children:?}"
    );
}

/// The parents the graph reports for `entity_id` — at most one, since an entity is stored
/// under one parent.
async fn parents_of(emulator: &Emulator, entity_id: &str) -> Vec<String> {
    let statement = format!(
        "SELECT parent.entityId FROM EntityGraph MATCH (child)-[:isChildOf]->(parent) \
         WHERE child.entityId = '{entity_id}'"
    );
    first_column(&query(emulator, &statement).await)
}

/// The children the graph reports for `entity_id`.
async fn children_of(emulator: &Emulator, entity_id: &str) -> Vec<String> {
    let statement = format!(
        "SELECT child.entityId FROM EntityGraph MATCH (child)-[:isChildOf]->(parent) \
         WHERE parent.entityId = '{entity_id}'"
    );
    first_column(&query(emulator, &statement).await)
}
