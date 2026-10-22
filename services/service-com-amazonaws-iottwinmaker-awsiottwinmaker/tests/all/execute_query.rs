// SPDX-License-Identifier: CC0-1.0

//! `ExecuteQuery`: the knowledge-graph query language, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the same
//! entry the served emulator uses — against entities created through `CreateEntity`, so
//! each row a query reports is a projection of state this emulator actually persisted.
//!
//! The tests assert RULES over tables of statements rather than one recorded case: which
//! rows a `WHERE` predicate selects over a set of members and operators, which rows a
//! `MATCH` pattern selects over the directions and hop counts it can be written with,
//! which column types the `SELECT` list reports, that the pages of one query partition
//! its rows, and that a statement outside the grammar is refused rather than answered.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every read-only case queries.
const WORKSPACE: &str = "query-workspace";

/// The component type ids the fixture labels its nodes with.
const MIXER_TYPE: &str = "com.example.mixer";
const LINE_TYPE: &str = "com.example.line";
const OVEN_TYPE: &str = "com.example.oven";

/// One fixture entity's component: its name, its type (which is the node label it gives
/// the entity) and the relationships stored on it as `(propertyName, targetEntityId)`.
struct Component {
    name: &'static str,
    type_id: &'static str,
    relationships: Vec<(&'static str, &'static str)>,
}

/// One fixture entity: its id, its name, its description, and the component the create
/// carries.
struct Fixture {
    entity_id: &'static str,
    entity_name: &'static str,
    description: &'static str,
    component: Option<Component>,
}

/// One row of a table whose expected answer is a pair per reported row.
type PairCase<'a> = (&'a str, &'a str, Vec<(&'static str, &'static str)>);

/// The fixture graph: a line that is the location of one mixer and monitors an oven, an
/// oven that feeds the other mixer, a mixer nothing points at, and two entities with no
/// component at all — one of which carries a relationship to an entity the workspace
/// does not hold, which is therefore no edge.
fn fixture() -> Vec<Fixture> {
    vec![
        Fixture {
            entity_id: "line_1",
            entity_name: "Cookie Line",
            description: "the cookie line",
            component: Some(Component {
                name: "line",
                type_id: LINE_TYPE,
                relationships: vec![("isLocationOf", "mixer_0"), ("monitors", "oven_0")],
            }),
        },
        Fixture {
            entity_id: "mixer_0",
            entity_name: "Mixer 0",
            description: "",
            component: Some(Component {
                name: "mixer",
                type_id: MIXER_TYPE,
                relationships: Vec::new(),
            }),
        },
        Fixture {
            entity_id: "mixer_1",
            entity_name: "Mixer 1",
            description: "the second mixer",
            component: Some(Component {
                name: "mixer",
                type_id: MIXER_TYPE,
                relationships: Vec::new(),
            }),
        },
        Fixture {
            entity_id: "oven_0",
            entity_name: "Oven 0",
            description: "",
            component: Some(Component {
                name: "oven",
                type_id: OVEN_TYPE,
                relationships: vec![("feeds", "mixer_1")],
            }),
        },
        Fixture {
            entity_id: "dangling",
            entity_name: "Dangling",
            description: "",
            component: Some(Component {
                name: "link",
                type_id: OVEN_TYPE,
                relationships: vec![("feeds", "no_such_entity")],
            }),
        },
        Fixture {
            entity_id: "spare_part",
            entity_name: "Spare Part",
            description: "",
            component: None,
        },
    ]
}

/// Every fixture entity id, in the `entityId` order a query reports them in.
fn all_ids() -> Vec<String> {
    let mut ids: Vec<String> = fixture()
        .iter()
        .map(|entity| entity.entity_id.to_string())
        .collect();
    ids.sort();
    ids
}

/// Create `WORKSPACE` and its fixture entities.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "creating {WORKSPACE}: expected 200, got {created:?}"
    );
    for entity in fixture() {
        let mut body = json!({
            "entityId": entity.entity_id,
            "entityName": entity.entity_name,
            "description": entity.description,
        });
        if let Some(component) = &entity.component {
            let mut properties = json!({
                // A scalar property, so a node's reported components carry a value that
                // is not a relationship too.
                "temperature": {
                    "definition": {"dataType": {"type": "DOUBLE"}},
                    "value": {"doubleValue": 50.5},
                },
            });
            for (relationship_name, target_entity_id) in &component.relationships {
                properties[*relationship_name] = json!({
                    "definition": {
                        "dataType": {
                            "type": "RELATIONSHIP",
                            "relationship": {"relationshipType": relationship_name},
                        },
                    },
                    "value": {"relationshipValue": {"targetEntityId": target_entity_id}},
                });
            }
            body["components"] = json!({
                component.name: {
                    "componentTypeId": component.type_id,
                    "properties": properties,
                },
            });
        }
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "creating {}: expected 200, got {created:?}",
            entity.entity_id
        );
    }
}

/// Run one statement against `workspace_id`, asserting it succeeded.
async fn query_in(emulator: &Emulator, workspace_id: &str, statement: &str) -> Resp {
    let resp = emulator
        .execute_query(json!({"workspaceId": workspace_id, "queryStatement": statement}))
        .await;
    assert_eq!(
        resp.status, 200,
        "{statement:?}: expected 200, got {resp:?}"
    );
    resp
}

/// Run one statement against the fixture workspace.
async fn query(emulator: &Emulator, statement: &str) -> Resp {
    query_in(emulator, WORKSPACE, statement).await
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

/// The first column of every row, as text.
fn first_column_text(resp: &Resp) -> Vec<String> {
    rows(resp)
        .iter()
        .map(|row| {
            row.first()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `entityId` of the NODE in the first column of every row.
fn first_column_entity_ids(resp: &Resp) -> Vec<String> {
    rows(resp)
        .iter()
        .map(|row| {
            row.first()
                .map(|node| node["entityId"].as_str().unwrap_or_default().to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// The `(name, type)` of every reported column.
fn columns(resp: &Resp) -> Vec<(String, String)> {
    resp.member("columnDescriptions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|column| {
            (
                column["name"].as_str().unwrap_or_default().to_string(),
                column["type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|id| (*id).to_string()).collect()
}

/// RULE: a `WHERE` predicate selects exactly the entities whose STORED member satisfies
/// it, evaluated per node; a path naming no member of a node is MISSING, so a comparison
/// against it selects nothing while `IS NULL` on it selects everything.
///
/// The rows differ along the member read, the operator applied and the value compared
/// against — which is what the rule generalises over — not along the entity named.
#[tokio::test]
async fn a_where_predicate_selects_the_entities_its_members_satisfy() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let everything = all_ids();
    // Bound before the table so the table holds no reference to a temporary.
    let by_label = format!("SELECT e FROM EntityGraph MATCH (e:{MIXER_TYPE})");
    let by_label_and_predicate =
        format!("SELECT e FROM EntityGraph MATCH (e:{MIXER_TYPE}) WHERE e.entityId = 'mixer_1'");
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "equality on entityName",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName = 'Mixer 0'",
            ids(&["mixer_0"]),
        ),
        (
            "equality on another entityName",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName = 'Cookie Line'",
            ids(&["line_1"]),
        ),
        (
            "equality on entityId",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId = 'oven_0'",
            ids(&["oven_0"]),
        ),
        (
            "equality on a name nothing carries",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName = 'Mixer 7'",
            Vec::new(),
        ),
        (
            "inequality on entityName",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName <> 'Mixer 0'",
            ids(&["dangling", "line_1", "mixer_1", "oven_0", "spare_part"]),
        ),
        (
            "a prefix LIKE pattern",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName LIKE 'Mixer %'",
            ids(&["mixer_0", "mixer_1"]),
        ),
        (
            "a single-character LIKE pattern",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId LIKE 'mixer__'",
            ids(&["mixer_0", "mixer_1"]),
        ),
        (
            "NOT LIKE",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId NOT LIKE 'mixer%'",
            ids(&["dangling", "line_1", "oven_0", "spare_part"]),
        ),
        (
            "an ordering comparison on entityId",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId < 'mixer_1'",
            ids(&["dangling", "line_1", "mixer_0"]),
        ),
        (
            "an inclusive ordering comparison on entityId",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId >= 'oven_0'",
            ids(&["oven_0", "spare_part"]),
        ),
        (
            "a conjunction of two members",
            "SELECT e FROM EntityGraph MATCH (e) \
             WHERE e.entityName LIKE 'Mixer%' AND e.description = ''",
            ids(&["mixer_0"]),
        ),
        (
            "a disjunction",
            "SELECT e FROM EntityGraph MATCH (e) \
             WHERE e.entityId = 'oven_0' OR e.entityId = 'line_1'",
            ids(&["line_1", "oven_0"]),
        ),
        (
            "a negated group",
            "SELECT e FROM EntityGraph MATCH (e) \
             WHERE NOT (e.entityId = 'oven_0' OR e.entityId = 'line_1')",
            ids(&["dangling", "mixer_0", "mixer_1", "spare_part"]),
        ),
        (
            "equality on the empty description every create left empty",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.description = ''",
            ids(&["dangling", "mixer_0", "oven_0", "spare_part"]),
        ),
        ("a node label", &by_label, ids(&["mixer_0", "mixer_1"])),
        (
            "a node label no component carries",
            "SELECT e FROM EntityGraph MATCH (e:com.example.absent)",
            Vec::new(),
        ),
        (
            "a label and a predicate together",
            &by_label_and_predicate,
            ids(&["mixer_1"]),
        ),
        (
            "a comparison against a member no node carries is UNKNOWN",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.notAMember = 'anything'",
            Vec::new(),
        ),
        (
            "and so is its negation",
            "SELECT e FROM EntityGraph MATCH (e) WHERE NOT e.notAMember = 'anything'",
            Vec::new(),
        ),
        (
            "IS NULL is definite about a missing member",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.notAMember IS NULL",
            everything.clone(),
        ),
        (
            "IS NOT NULL on a member every node carries",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName IS NOT NULL",
            everything.clone(),
        ),
        (
            "a numeric comparison on a stored instant",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.creationDate > 0",
            everything.clone(),
        ),
        (
            "a numeric comparison no stored instant satisfies",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.lastUpdateDate < 0",
            Vec::new(),
        ),
        (
            "comparing text against a number is UNKNOWN",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName = 7",
            Vec::new(),
        ),
        (
            "no WHERE clause at all",
            "SELECT e FROM EntityGraph MATCH (e)",
            everything.clone(),
        ),
        (
            "lower case keywords",
            "select e from EntityGraph match (e) where e.entityId = 'mixer_0'",
            ids(&["mixer_0"]),
        ),
    ];

    for (case, statement, expected) in cases {
        let resp = query(&emulator, statement).await;
        let selected = first_column_entity_ids(&resp);
        assert_eq!(
            selected, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {selected:?}"
        );
    }
}

/// RULE: a `MATCH` relationship pattern selects the stored relationships it describes —
/// a component property whose value names a target entity of the same workspace — in the
/// direction the pattern points, and a relationship naming an entity the workspace does
/// not hold is no edge at all.
///
/// The rows differ along the direction, the label and the number of hops.
#[tokio::test]
async fn a_match_pattern_selects_the_stored_relationships() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Bound before the table so the table holds no reference to a temporary.
    let far_end_labelled =
        format!("SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[r]->(b:{MIXER_TYPE})");
    // `(sourceEntityId, targetEntityId)` per reported row.
    let cases: Vec<PairCase> = vec![
        (
            "an outgoing labelled relationship",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:isLocationOf]->(b)",
            vec![("line_1", "mixer_0")],
        ),
        (
            "another outgoing labelled relationship",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:feeds]->(b)",
            vec![("oven_0", "mixer_1")],
        ),
        (
            "every outgoing relationship",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[r]->(b)",
            vec![
                ("line_1", "mixer_0"),
                ("line_1", "oven_0"),
                ("oven_0", "mixer_1"),
            ],
        ),
        (
            "an anonymous outgoing relationship",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)->(b)",
            vec![
                ("line_1", "mixer_0"),
                ("line_1", "oven_0"),
                ("oven_0", "mixer_1"),
            ],
        ),
        (
            "the same relationship read incoming",
            "SELECT b.entityId, a.entityId FROM EntityGraph MATCH (b)<-[:isLocationOf]-(a)",
            vec![("mixer_0", "line_1")],
        ),
        (
            "an undirected relationship matches both ways round",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:isLocationOf]-(b)",
            vec![("line_1", "mixer_0"), ("mixer_0", "line_1")],
        ),
        (
            "a label no relationship carries",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:isParentOf]->(b)",
            Vec::new(),
        ),
        (
            "two hops",
            "SELECT a.entityId, c.entityId FROM EntityGraph MATCH (a)-[r1]->(b)-[r2]->(c)",
            vec![("line_1", "mixer_1")],
        ),
        (
            "two hops with labels",
            "SELECT a.entityId, c.entityId FROM EntityGraph \
             MATCH (a)-[:monitors]->(b)-[:feeds]->(c)",
            vec![("line_1", "mixer_1")],
        ),
        (
            "a hop whose far end is labelled",
            &far_end_labelled,
            vec![("line_1", "mixer_0"), ("oven_0", "mixer_1")],
        ),
        (
            "two patterns joined on a shared variable",
            "SELECT a.entityId, c.entityId FROM EntityGraph \
             MATCH (a)-[:monitors]->(b), (b)-[:feeds]->(c)",
            vec![("line_1", "mixer_1")],
        ),
        (
            "a pattern filtered by a predicate on each end",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[r]->(b) \
             WHERE a.entityName = 'Cookie Line' AND b.entityName LIKE 'Mixer%'",
            vec![("line_1", "mixer_0")],
        ),
        (
            "a relationship whose target is not in the workspace is no edge",
            "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[:feeds]->(b) \
             WHERE a.entityId = 'dangling'",
            Vec::new(),
        ),
    ];

    for (case, statement, expected) in cases {
        let resp = query(&emulator, statement).await;
        let reported: Vec<(String, String)> = rows(&resp)
            .iter()
            .map(|row| {
                (
                    row.first()
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    row.get(1)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(left, right)| ((*left).to_string(), (*right).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: an `EDGE` column reports the relationship itself — its name and the entity and
/// component at each end — read from the property the relationship is stored as.
#[tokio::test]
async fn an_edge_column_reports_the_stored_relationship() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let resp = query(
        &emulator,
        "SELECT r FROM EntityGraph MATCH (a)-[r]->(b) WHERE a.entityId = 'line_1'",
    )
    .await;
    let edges = rows(&resp);
    assert_eq!(
        edges.len(),
        2,
        "the line has two stored relationships, got {edges:?}"
    );
    for (edge, (name, source, component, target)) in edges.iter().zip([
        ("isLocationOf", "line_1", "line", "mixer_0"),
        ("monitors", "line_1", "line", "oven_0"),
    ]) {
        let edge = edge.first().expect("an EDGE column");
        for (member, expected) in [
            ("relationshipName", name),
            ("sourceEntityId", source),
            ("sourceComponentName", component),
            ("targetEntityId", target),
        ] {
            assert_eq!(
                edge[member].as_str(),
                Some(expected),
                "case {name:?}: expected {member} {expected:?}, got {edge:?}"
            );
        }
    }

    // A member of the relationship, selected on its own, is the same value as a VALUE
    // column.
    let resp = query(
        &emulator,
        "SELECT r.relationshipName FROM EntityGraph MATCH (a)-[r]->(b)",
    )
    .await;
    let names = first_column_text(&resp);
    assert_eq!(
        names,
        vec![
            "isLocationOf".to_string(),
            "monitors".to_string(),
            "feeds".to_string()
        ],
        "every stored relationship name, got {names:?}"
    );
}

/// RULE: a `NODE` column reports the entity as this service stores it, so every member
/// it carries is the one `GetEntity` reports for that entity.
#[tokio::test]
async fn a_node_column_reports_the_stored_entity() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for entity in fixture() {
        let read = emulator.get_entity(WORKSPACE, entity.entity_id).await;
        assert_eq!(
            read.status, 200,
            "reading {}: expected 200, got {read:?}",
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
        let node = selected[0].first().expect("a NODE column");
        for member in ["entityId", "entityName", "arn", "description"] {
            assert_eq!(
                node[member],
                read.member(member).clone(),
                "case {:?}: the node's {member} must be what GetEntity reports, got {node:?}",
                entity.entity_id
            );
        }
        let components = node["components"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            components.len(),
            entity.component.iter().count(),
            "case {:?}: the node reports its stored components, got {node:?}",
            entity.entity_id
        );
        if let Some(expected_component) = &entity.component {
            let component = &components[0];
            assert_eq!(
                component["componentName"].as_str(),
                Some(expected_component.name),
                "case {:?}: componentName, got {component:?}",
                entity.entity_id
            );
            assert_eq!(
                component["componentTypeId"].as_str(),
                Some(expected_component.type_id),
                "case {:?}: componentTypeId, got {component:?}",
                entity.entity_id
            );
            let property_names: BTreeSet<String> = component["properties"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|property| {
                    property["propertyName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let mut expected: BTreeSet<String> = expected_component
                .relationships
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect();
            expected.insert("temperature".to_string());
            assert_eq!(
                property_names, expected,
                "case {:?}: the component reports its stored properties, got {component:?}",
                entity.entity_id
            );
        }
    }
}

/// RULE: every selected item is reported as one column, named as it was written and
/// typed by what it projects — `NODE`/`EDGE` for a whole binding, `VALUE` for a member of
/// one — whether or not the statement selected any row.
#[tokio::test]
async fn the_reported_columns_describe_the_select_list() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The `(name, type)` every statement must report.
    let cases: Vec<PairCase> = vec![
        (
            "a node",
            "SELECT e FROM EntityGraph MATCH (e)",
            vec![("e", "NODE")],
        ),
        (
            "a member of a node",
            "SELECT e.entityName FROM EntityGraph MATCH (e)",
            vec![("e.entityName", "VALUE")],
        ),
        (
            "a relationship",
            "SELECT r FROM EntityGraph MATCH (a)-[r]->(b)",
            vec![("r", "EDGE")],
        ),
        (
            "both ends and the relationship between them",
            "SELECT a, r, b.entityId FROM EntityGraph MATCH (a)-[r]->(b)",
            vec![("a", "NODE"), ("r", "EDGE"), ("b.entityId", "VALUE")],
        ),
        (
            "the same member twice",
            "SELECT e.entityId, e.entityId FROM EntityGraph MATCH (e)",
            vec![("e.entityId", "VALUE"), ("e.entityId", "VALUE")],
        ),
        (
            "a statement that selects no row still describes its columns",
            "SELECT e.arn FROM EntityGraph MATCH (e) WHERE e.entityId = 'absent'",
            vec![("e.arn", "VALUE")],
        ),
    ];

    for (case, statement, expected) in cases {
        let resp = query(&emulator, statement).await;
        let reported = columns(&resp);
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(name, kind)| ((*name).to_string(), (*kind).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?} ({statement:?}): expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: the pages of one query partition its row set in order — each page holds at most
/// `maxResults` rows, the last page carries no `nextToken`, and the concatenation is the
/// single-page answer. A token is bound to the query it was minted for.
#[tokio::test]
async fn the_pages_of_a_query_partition_its_rows() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let statement = "SELECT e.entityId FROM EntityGraph MATCH (e)";
    let whole = first_column_text(&query(&emulator, statement).await);
    assert_eq!(
        whole,
        all_ids(),
        "the unpaged answer is every entity, got {whole:?}"
    );

    for page_size in [None, Some(1usize), Some(2), Some(5), Some(100)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"workspaceId": WORKSPACE, "queryStatement": statement});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let resp = emulator.execute_query(request).await;
            assert_eq!(
                resp.status, 200,
                "case maxResults={page_size:?} page {pages}: expected 200, got {resp:?}"
            );
            let page = first_column_text(&resp);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case maxResults={page_size:?} page {pages}: a page holds at most {size} \
                     rows, got {page:?}"
                );
            }
            seen.extend(page);
            next_token = resp.member("nextToken").as_str().map(str::to_string);
            pages += 1;
            if next_token.is_none() {
                break;
            }
            assert!(
                pages < 32,
                "case maxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, whole,
            "case maxResults={page_size:?}: the pages must partition the row set, got {seen:?}"
        );
    }

    // A token minted for one statement does not resume another.
    let first = emulator
        .execute_query(json!({
            "workspaceId": WORKSPACE,
            "queryStatement": statement,
            "maxResults": 1,
        }))
        .await;
    let token = first
        .member("nextToken")
        .as_str()
        .expect("a first page leaves a nextToken")
        .to_string();
    let reused = emulator
        .execute_query(json!({
            "workspaceId": WORKSPACE,
            "queryStatement": "SELECT e.entityName FROM EntityGraph MATCH (e)",
            "nextToken": token,
        }))
        .await;
    assert_eq!(
        reused.status, 400,
        "a token from another query must be refused, got {reused:?}"
    );
}

/// RULE: a statement the query language does not accept, or one naming something the
/// workspace does not define, is refused with the modelled 400 `ValidationException` —
/// never answered with rows and never with the scaffold's 501.
///
/// Oversized input is part of that class: a `WHERE` clause nested past the handler's
/// `MAX_EXPRESSION_DEPTH` (32 — the bound that stops caller-chosen nesting from choosing
/// the parser's stack depth) is refused the same way a malformed one is, whether the
/// nesting is written with `NOT` or with parentheses.
#[tokio::test]
async fn a_statement_outside_the_grammar_is_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Bound before the table so it holds no reference to a temporary. 40 is past the
    // handler's depth ceiling of 32, and both statements stay inside `QueryStatement`'s
    // modelled length of 1000.
    let deep_negation = format!(
        "SELECT e FROM EntityGraph MATCH (e) WHERE {}e.entityId = 'mixer_0'",
        "NOT ".repeat(40)
    );
    let deep_grouping = format!(
        "SELECT e FROM EntityGraph MATCH (e) WHERE {}e.entityId = 'mixer_0'{}",
        "(".repeat(40),
        ")".repeat(40)
    );

    let cases: Vec<(&str, &str)> = vec![
        (
            "a WHERE clause nested past the depth ceiling",
            &deep_negation,
        ),
        (
            "a WHERE clause grouped past the depth ceiling",
            &deep_grouping,
        ),
        ("free text", "this is not a query"),
        ("a bare keyword", "SELECT"),
        ("no FROM clause", "SELECT e"),
        ("an unknown source", "SELECT e FROM Entities MATCH (e)"),
        ("an unclosed node pattern", "SELECT e FROM EntityGraph MATCH (e"),
        (
            "an unclosed relationship pattern",
            "SELECT a FROM EntityGraph MATCH (a)-[r->(b)",
        ),
        (
            "a relationship pattern that points nowhere",
            "SELECT a FROM EntityGraph MATCH (a)-[r](b)",
        ),
        (
            "a variable no pattern declares",
            "SELECT x FROM EntityGraph MATCH (e)",
        ),
        (
            "a WHERE variable no pattern declares",
            "SELECT e FROM EntityGraph MATCH (e) WHERE x.entityId = 'mixer_0'",
        ),
        (
            "a condition with no operator",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName",
        ),
        (
            "a condition comparing a variable rather than a member",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e = 'mixer_0'",
        ),
        (
            "a path of more than one member",
            "SELECT e.components.temperature FROM EntityGraph MATCH (e)",
        ),
        (
            "an unterminated literal",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityName = 'Mixer 0",
        ),
        (
            "a variable matched as a node and as a relationship",
            "SELECT e FROM EntityGraph MATCH (e)-[e]->(b)",
        ),
        (
            "trailing input after the query",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId = 'mixer_0' AND",
        ),
        ("a statement that is not a SELECT", "DELETE FROM EntityGraph"),
        (
            "an unsupported character",
            "SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId ~ 'mixer_0'",
        ),
    ];

    for (case, statement) in cases {
        let resp = emulator
            .execute_query(json!({"workspaceId": WORKSPACE, "queryStatement": statement}))
            .await;
        assert_eq!(
            resp.status, 400,
            "case {case:?} ({statement:?}): expected 400, got {resp:?}"
        );
        assert_eq!(
            resp.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({statement:?}): expected ValidationException, got {resp:?}"
        );
    }

    // The model declares no ResourceNotFoundException for this operation, so an unknown
    // workspace is a bad request too.
    let unknown = emulator
        .execute_query(json!({
            "workspaceId": "no-such-workspace",
            "queryStatement": "SELECT e FROM EntityGraph MATCH (e)",
        }))
        .await;
    assert_eq!(
        unknown.status, 400,
        "an unknown workspace must be refused, got {unknown:?}"
    );
    assert_eq!(
        unknown.error_type.as_deref(),
        Some("ValidationException"),
        "an unknown workspace must be a ValidationException, got {unknown:?}"
    );
}

/// RULE: a query reports the state as it is when it runs — an entity created after an
/// earlier query appears in the next one, and an entity deleted disappears from it,
/// along with the relationships that pointed at it.
#[tokio::test]
async fn a_query_follows_the_state_it_reads() {
    let emulator = Emulator::new();
    let workspace = "query-mutation";
    let created = emulator.create_workspace(workspace).await;
    assert_eq!(
        created.status, 200,
        "creating {workspace}: expected 200, got {created:?}"
    );

    let all_nodes = "SELECT e.entityId FROM EntityGraph MATCH (e)";
    let all_edges = "SELECT a.entityId, b.entityId FROM EntityGraph MATCH (a)-[r]->(b)";

    let empty = query_in(&emulator, workspace, all_nodes).await;
    assert!(
        rows(&empty).is_empty(),
        "an empty workspace selects no row, got {empty:?}"
    );
    assert_eq!(
        columns(&empty),
        vec![("e.entityId".to_string(), "VALUE".to_string())],
        "an empty answer still describes its column, got {empty:?}"
    );

    // `tank_0` first, so the relationship `pump_0` carries has a node to end at.
    for (entity_id, entity_name, body) in [
        ("tank_0", "Tank 0", json!({})),
        (
            "pump_0",
            "Pump 0",
            json!({
                "components": {
                    "plumbing": {
                        "componentTypeId": "com.example.plumbing",
                        "properties": {
                            "feeds": {
                                "definition": {
                                    "dataType": {
                                        "type": "RELATIONSHIP",
                                        "relationship": {"relationshipType": "feeds"},
                                    },
                                },
                                "value": {"relationshipValue": {"targetEntityId": "tank_0"}},
                            },
                        },
                    },
                },
            }),
        ),
    ] {
        let mut request = body;
        request["entityId"] = json!(entity_id);
        request["entityName"] = json!(entity_name);
        let resp = emulator.create_entity(workspace, request).await;
        assert_eq!(
            resp.status, 200,
            "creating {entity_id}: expected 200, got {resp:?}"
        );
        let seen = first_column_text(&query_in(&emulator, workspace, all_nodes).await);
        assert!(
            seen.contains(&entity_id.to_string()),
            "case {entity_id:?}: a created entity must be selected, got {seen:?}"
        );
    }

    // The relationship the create stored is an edge of the graph.
    let edges = rows(&query_in(&emulator, workspace, all_edges).await);
    assert_eq!(
        edges.len(),
        1,
        "the stored relationship must be one edge, got {edges:?}"
    );

    let deleted = emulator.delete_entity(workspace, "tank_0", false).await;
    assert_eq!(
        deleted.status, 200,
        "deleting tank_0: expected 200, got {deleted:?}"
    );
    let seen = first_column_text(&query_in(&emulator, workspace, all_nodes).await);
    assert!(
        !seen.contains(&"tank_0".to_string()),
        "a deleted entity must not be selected, got {seen:?}"
    );
    let edges = rows(&query_in(&emulator, workspace, all_edges).await);
    assert!(
        edges.is_empty(),
        "a relationship whose target was deleted is no longer an edge, got {edges:?}"
    );
}
