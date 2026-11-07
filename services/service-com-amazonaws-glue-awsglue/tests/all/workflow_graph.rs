// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the graph a Glue workflow read reports: the `Graph` of
//! `GetWorkflow` and the one `GetWorkflowRun` reports for a run of that workflow.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a workflow's graph is the Glue components its triggers make it of.
//! Every trigger whose `WorkflowName` names the workflow is a `TRIGGER` node; every job and
//! crawler one of its actions names is a node the trigger initiates, so the connection is
//! directed FROM the trigger TO that node; every job and crawler one of its predicate's
//! conditions names is a node the trigger waits on, so the connection is directed the other
//! way. A component several triggers name is ONE node, a connection named twice is ONE
//! edge, and a workflow nothing is attached to has a graph with no nodes and no edges.
//!
//! The rows differ along the type of the attached triggers, how many there are, how many
//! actions each initiates, whether a target is a job or a crawler, whether the trigger also
//! waits on a predicate's conditions, and whether two triggers name the same component.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    NODE_TYPE_CRAWLER, NODE_TYPE_JOB, NODE_TYPE_TRIGGER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_in_region, emulator, ok, REGION};

/// The role ARN every job and crawler below is registered with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The job a trigger's action or condition names.
const JOB: &str = "graph-first-job";

/// A second job, so a row can name two jobs and a trigger can run one job while waiting on
/// another.
const OTHER_JOB: &str = "graph-second-job";

/// The crawler an action or a condition can name instead of a job.
const CRAWLER: &str = "graph-target-crawler";

/// Register the jobs and the crawler the trigger definitions below name, so their actions
/// and conditions resolve.
async fn register_components(router: &MultiServiceRouter) {
    for job in [JOB, OTHER_JOB] {
        ok(
            router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            }),
        )
        .await;
    }
    ok(
        router,
        "CreateCrawler",
        &json!({
            "Name": CRAWLER,
            "Role": ROLE_ARN,
            "Targets": { "S3Targets": [{ "Path": "s3://sim2real-glue-crawler/data/" }] },
        }),
    )
    .await;
}

/// A node of a graph, as the `Type` and `Name` it reports — which is what identifies a
/// component inside the workflow, so a row can name the nodes it expects without knowing
/// the order the walk placed them in.
type NodeKey = (String, String);

/// How a row names one node: its `NodeType` and the component's name.
type NamedNode = (&'static str, &'static str);

/// The key for the component `name` of type `node_type`.
fn key(node_type: &str, name: &str) -> NodeKey {
    (node_type.to_string(), name.to_string())
}

/// The graph `GetWorkflow` reports for `workflow`.
async fn workflow_graph(router: &MultiServiceRouter, case: &str, workflow: &str) -> Value {
    let body = ok(
        router,
        "GetWorkflow",
        &json!({ "Name": workflow, "IncludeGraph": true }),
    )
    .await;
    body["Workflow"]
        .get("Graph")
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: IncludeGraph must report a Graph, got {body}"))
}

/// Every node of `graph`, keyed by type and name. A component two triggers name has to be
/// one node, which is what the duplicate check asserts.
fn nodes(case: &str, graph: &Value) -> BTreeMap<NodeKey, Value> {
    let listed = graph
        .get("Nodes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: Graph.Nodes must be a list, got {graph}"));
    let mut found = BTreeMap::new();
    for node in listed {
        let node_type = node
            .get("Type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: a node reports its Type, got {node}"));
        let name = node
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: a node reports its Name, got {node}"));
        assert!(
            found.insert(key(node_type, name), node.clone()).is_none(),
            "case {case}: a component named by several triggers is one node, got {graph}"
        );
    }
    found
}

/// The node each `UniqueId` in `graph` stands for. Every node has to carry one, and no two
/// may share it: it is "the unique Id assigned to the node within the workflow".
fn nodes_by_id(case: &str, graph: &Value) -> HashMap<String, NodeKey> {
    let mut by_id: HashMap<String, NodeKey> = HashMap::new();
    for (node_key, node) in nodes(case, graph) {
        let unique_id = node
            .get("UniqueId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case}: node {node_key:?} must report a UniqueId, got {node}")
            })
            .to_string();
        assert!(
            by_id.insert(unique_id.clone(), node_key.clone()).is_none(),
            "case {case}: UniqueId {unique_id:?} is shared by two nodes, got {graph}"
        );
    }
    by_id
}

/// Every connection of `graph`, as the node its two ends resolve to. A connection named
/// twice has to be one edge, which is what the duplicate check asserts.
fn edges(case: &str, graph: &Value) -> BTreeSet<(NodeKey, NodeKey)> {
    let by_id = nodes_by_id(case, graph);
    let listed = graph
        .get("Edges")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: Graph.Edges must be a list, got {graph}"));
    let mut found = BTreeSet::new();
    for edge in listed {
        let source = end(case, edge, "SourceId", &by_id);
        let destination = end(case, edge, "DestinationId", &by_id);
        assert!(
            found.insert((source, destination)),
            "case {case}: a connection named twice is one edge, got {graph}"
        );
    }
    found
}

/// The node one end of `edge` names.
fn end(case: &str, edge: &Value, member: &str, by_id: &HashMap<String, NodeKey>) -> NodeKey {
    let id = edge
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: an edge reports its {member}, got {edge}"));
    by_id.get(id).cloned().unwrap_or_else(|| {
        panic!("case {case}: {member} {id:?} must name a node of the graph, got {edge}")
    })
}

/// One row of the graph table: the triggers bound to a workflow of its own, and the nodes
/// and directed connections those triggers make its graph of.
struct GraphCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The `CreateTrigger` bodies, each carrying its own `Name` and no `WorkflowName`: the
    /// suite binds every one of them to this row's workflow.
    triggers: Vec<Value>,
    /// The nodes the graph must report, as (`NodeType`, name).
    nodes: Vec<NamedNode>,
    /// The connections the graph must report, each from one node to another.
    edges: Vec<(NamedNode, NamedNode)>,
}

/// The rows every suite below drives. A `CONDITIONAL` trigger carries the predicate its
/// type requires, a `SCHEDULED` one its schedule and an `EVENT` one its batching condition,
/// so each row is a definition `CreateTrigger` accepts.
fn graph_cases() -> Vec<GraphCase> {
    vec![
        GraphCase {
            case: "nothing attached to the workflow",
            triggers: Vec::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
        },
        GraphCase {
            // The recorded shape: one on-demand entry point running one job.
            case: "an on-demand trigger running one job",
            triggers: vec![json!({
                "Name": "graph-on-demand",
                "Type": "ON_DEMAND",
                "Actions": [{ "JobName": JOB }],
            })],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-on-demand"),
                (NODE_TYPE_JOB, JOB),
            ],
            edges: vec![(
                (NODE_TYPE_TRIGGER, "graph-on-demand"),
                (NODE_TYPE_JOB, JOB),
            )],
        },
        GraphCase {
            case: "a scheduled trigger running a job and a crawler",
            triggers: vec![json!({
                "Name": "graph-scheduled",
                "Type": "SCHEDULED",
                "Schedule": "cron(15 12 * * ? *)",
                "Actions": [{ "JobName": JOB }, { "CrawlerName": CRAWLER }],
            })],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-scheduled"),
                (NODE_TYPE_JOB, JOB),
                (NODE_TYPE_CRAWLER, CRAWLER),
            ],
            edges: vec![
                ((NODE_TYPE_TRIGGER, "graph-scheduled"), (NODE_TYPE_JOB, JOB)),
                (
                    (NODE_TYPE_TRIGGER, "graph-scheduled"),
                    (NODE_TYPE_CRAWLER, CRAWLER),
                ),
            ],
        },
        GraphCase {
            case: "a conditional trigger waiting on one job and running another",
            triggers: vec![json!({
                "Name": "graph-conditional",
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Conditions": [
                        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
                    ],
                },
                "Actions": [{ "JobName": OTHER_JOB }],
            })],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-conditional"),
                (NODE_TYPE_JOB, JOB),
                (NODE_TYPE_JOB, OTHER_JOB),
            ],
            edges: vec![
                ((NODE_TYPE_JOB, JOB), (NODE_TYPE_TRIGGER, "graph-conditional")),
                (
                    (NODE_TYPE_TRIGGER, "graph-conditional"),
                    (NODE_TYPE_JOB, OTHER_JOB),
                ),
            ],
        },
        GraphCase {
            case: "a conditional trigger waiting on a crawler and two jobs",
            triggers: vec![json!({
                "Name": "graph-crawler-conditional",
                "Type": "CONDITIONAL",
                "Predicate": {
                    "Logical": "AND",
                    "Conditions": [
                        {
                            "LogicalOperator": "EQUALS",
                            "CrawlerName": CRAWLER,
                            "CrawlState": "SUCCEEDED",
                        },
                        { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
                    ],
                },
                "Actions": [{ "JobName": OTHER_JOB }],
            })],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-crawler-conditional"),
                (NODE_TYPE_CRAWLER, CRAWLER),
                (NODE_TYPE_JOB, JOB),
                (NODE_TYPE_JOB, OTHER_JOB),
            ],
            edges: vec![
                (
                    (NODE_TYPE_CRAWLER, CRAWLER),
                    (NODE_TYPE_TRIGGER, "graph-crawler-conditional"),
                ),
                (
                    (NODE_TYPE_JOB, JOB),
                    (NODE_TYPE_TRIGGER, "graph-crawler-conditional"),
                ),
                (
                    (NODE_TYPE_TRIGGER, "graph-crawler-conditional"),
                    (NODE_TYPE_JOB, OTHER_JOB),
                ),
            ],
        },
        GraphCase {
            case: "an event trigger running a crawler",
            triggers: vec![json!({
                "Name": "graph-event",
                "Type": "EVENT",
                "EventBatchingCondition": { "BatchSize": 5, "BatchWindow": 120 },
                "Actions": [{ "CrawlerName": CRAWLER }],
            })],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-event"),
                (NODE_TYPE_CRAWLER, CRAWLER),
            ],
            edges: vec![(
                (NODE_TYPE_TRIGGER, "graph-event"),
                (NODE_TYPE_CRAWLER, CRAWLER),
            )],
        },
        GraphCase {
            // Two triggers of one workflow naming the same job: the entry point runs it and
            // the continuation waits on it, so that job is ONE node with an edge each way.
            case: "two triggers sharing a job, one running it and one waiting on it",
            triggers: vec![
                json!({
                    "Name": "graph-chain-start",
                    "Type": "ON_DEMAND",
                    "Actions": [{ "JobName": JOB }],
                }),
                json!({
                    "Name": "graph-chain-continue",
                    "Type": "CONDITIONAL",
                    "Predicate": {
                        "Conditions": [
                            { "LogicalOperator": "EQUALS", "JobName": JOB, "State": "SUCCEEDED" },
                        ],
                    },
                    "Actions": [{ "CrawlerName": CRAWLER }],
                }),
            ],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-chain-start"),
                (NODE_TYPE_TRIGGER, "graph-chain-continue"),
                (NODE_TYPE_JOB, JOB),
                (NODE_TYPE_CRAWLER, CRAWLER),
            ],
            edges: vec![
                (
                    (NODE_TYPE_TRIGGER, "graph-chain-start"),
                    (NODE_TYPE_JOB, JOB),
                ),
                (
                    (NODE_TYPE_JOB, JOB),
                    (NODE_TYPE_TRIGGER, "graph-chain-continue"),
                ),
                (
                    (NODE_TYPE_TRIGGER, "graph-chain-continue"),
                    (NODE_TYPE_CRAWLER, CRAWLER),
                ),
            ],
        },
        GraphCase {
            // One trigger naming the same job in two actions: one node, one connection.
            case: "a trigger whose two actions name the same job",
            triggers: vec![json!({
                "Name": "graph-repeated-action",
                "Type": "ON_DEMAND",
                "Actions": [
                    { "JobName": JOB },
                    { "JobName": JOB, "Arguments": { "--enable-metrics": "true" } },
                ],
            })],
            nodes: vec![
                (NODE_TYPE_TRIGGER, "graph-repeated-action"),
                (NODE_TYPE_JOB, JOB),
            ],
            edges: vec![(
                (NODE_TYPE_TRIGGER, "graph-repeated-action"),
                (NODE_TYPE_JOB, JOB),
            )],
        },
    ]
}

/// Register this row's workflow and bind every trigger it names to it, answering the
/// workflow's name.
async fn register_case(router: &MultiServiceRouter, index: usize, row: &GraphCase) -> String {
    let workflow = format!("graph-workflow-{index}");
    ok(router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    for trigger in &row.triggers {
        let mut body = trigger.clone();
        body["WorkflowName"] = json!(workflow);
        ok(router, "CreateTrigger", &body).await;
    }
    workflow
}

/// The nodes a row expects, as keys.
fn expected_nodes(row: &GraphCase) -> BTreeSet<NodeKey> {
    row.nodes
        .iter()
        .map(|(node_type, name)| key(node_type, name))
        .collect()
}

/// The connections a row expects, as pairs of keys.
fn expected_edges(row: &GraphCase) -> BTreeSet<(NodeKey, NodeKey)> {
    row.edges
        .iter()
        .map(|(source, destination)| {
            (
                key(source.0, source.1),
                key(destination.0, destination.1),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The workflow's own graph
// ---------------------------------------------------------------------------

/// The graph a workflow read reports is the components its triggers make it of, and the
/// connections run the way the work does: from a trigger to what it initiates, and from a
/// component to the trigger that waits on it.
#[tokio::test]
async fn the_workflow_graph_is_built_from_the_triggers_bound_to_it() {
    let router = emulator();
    register_components(&router).await;

    for (index, row) in graph_cases().iter().enumerate() {
        let case = row.case;
        let workflow = register_case(&router, index, row).await;
        let graph = workflow_graph(&router, case, &workflow).await;

        let reported = nodes(case, &graph);
        assert_eq!(
            reported.keys().cloned().collect::<BTreeSet<NodeKey>>(),
            expected_nodes(row),
            "case {case}: the graph reports the components the workflow holds, got {graph}"
        );
        assert_eq!(
            edges(case, &graph),
            expected_edges(row),
            "case {case}: the graph reports the connections between them, got {graph}"
        );

        // A `TRIGGER` node carries the trigger it stands for ("details of the Trigger when
        // the node represents a Trigger"), and the components it works with carry the
        // details structure of their OWN kind — a job node its `JobDetails`, a crawl node
        // its `CrawlerDetails` — with no run or crawl inside it, because nothing in this
        // workflow has run them.
        for ((node_type, name), node) in &reported {
            if node_type == NODE_TYPE_TRIGGER {
                let trigger = node
                    .get("TriggerDetails")
                    .and_then(|details| details.get("Trigger"))
                    .unwrap_or_else(|| {
                        panic!("case {case}: a TRIGGER node reports its trigger, got {node}")
                    });
                assert_eq!(
                    trigger.get("Name").and_then(Value::as_str),
                    Some(name.as_str()),
                    "case {case}: the node's trigger is the one it stands for, got {trigger}"
                );
                assert_eq!(
                    trigger.get("WorkflowName").and_then(Value::as_str),
                    Some(workflow.as_str()),
                    "case {case}: the node's trigger is bound to this workflow, got {trigger}"
                );
            }
            // (the node kind, the details member that kind carries)
            let own_details = [
                (NODE_TYPE_TRIGGER, "TriggerDetails"),
                (NODE_TYPE_JOB, "JobDetails"),
                (NODE_TYPE_CRAWLER, "CrawlerDetails"),
            ];
            for (kind, details) in own_details {
                let reported_details = node.get(details).filter(|value| !value.is_null());
                if kind == node_type {
                    let reported_details = reported_details.unwrap_or_else(|| {
                        panic!("case {case}: a {node_type} node reports its {details}, got {node}")
                    });
                    if kind != NODE_TYPE_TRIGGER {
                        assert_eq!(
                            reported_details,
                            &json!({}),
                            "case {case}: nothing has run in this workflow, so a {node_type} \
                             node's {details} carries no run, got {node}"
                        );
                    }
                } else {
                    assert!(
                        reported_details.is_none(),
                        "case {case}: a {node_type} node reports no {details}, got {node}"
                    );
                }
            }
        }

        // A read does not change what the next caller sees.
        let again = workflow_graph(&router, case, &workflow).await;
        assert_eq!(
            again, graph,
            "case {case}: the graph must be the same on every read, got {again}"
        );
    }
}

/// A run's graph is the graph of the workflow it is a run of, so both readers report the
/// same components and connections. Only a row with an entry point — a trigger whose firing
/// does not wait on the workflow's own components — can be started, so those are the rows
/// driven here.
#[tokio::test]
async fn a_run_reports_the_graph_of_its_workflow() {
    let router = emulator();
    register_components(&router).await;

    for (index, row) in graph_cases().iter().enumerate() {
        let case = row.case;
        let startable = row.triggers.iter().any(|trigger| {
            trigger.get("Type").and_then(Value::as_str) != Some("CONDITIONAL")
        });
        if !startable {
            continue;
        }
        let workflow = register_case(&router, index, row).await;
        let started = ok(&router, "StartWorkflowRun", &json!({ "Name": workflow })).await;
        let run_id = started
            .get("RunId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: the start must answer a RunId, got {started}"))
            .to_string();

        let body = ok(
            &router,
            "GetWorkflowRun",
            &json!({ "Name": workflow, "RunId": run_id, "IncludeGraph": true }),
        )
        .await;
        let run_graph = body["Run"]
            .get("Graph")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: IncludeGraph must report a Graph, got {body}"));

        assert_eq!(
            nodes(case, &run_graph)
                .keys()
                .cloned()
                .collect::<BTreeSet<NodeKey>>(),
            expected_nodes(row),
            "case {case}: the run reports its workflow's components, got {run_graph}"
        );
        assert_eq!(
            edges(case, &run_graph),
            expected_edges(row),
            "case {case}: the run reports its workflow's connections, got {run_graph}"
        );
    }
}

/// Two components of one workflow are two nodes however long their names are: a node's
/// `UniqueId` is itself held to `NameString`'s bound, so two jobs named at that bound and
/// differing only in their last character must still be two nodes with two ids — and each
/// of the trigger's two connections must reach its own one.
#[tokio::test]
async fn components_named_at_the_modelled_bound_are_still_two_nodes() {
    let router = emulator();
    register_components(&router).await;

    // "len 1..255" on `NameString` (`smithy_query shape=Node`), which a job name is of too.
    let bound = 255;
    let case = "two job names agreeing on all but their last character";
    let first = format!("{}a", "j".repeat(bound - 1));
    let second = format!("{}b", "j".repeat(bound - 1));
    for job in [&first, &second] {
        ok(
            &router,
            "CreateJob",
            &json!({
                "Name": job,
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            }),
        )
        .await;
    }

    let workflow = "graph-long-names";
    ok(&router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    let trigger = "graph-long-name-trigger";
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": trigger,
            "Type": "ON_DEMAND",
            "WorkflowName": workflow,
            "Actions": [{ "JobName": first }, { "JobName": second }],
        }),
    )
    .await;

    let graph = workflow_graph(&router, case, workflow).await;
    assert_eq!(
        nodes(case, &graph)
            .keys()
            .cloned()
            .collect::<BTreeSet<NodeKey>>(),
        BTreeSet::from([
            key(NODE_TYPE_TRIGGER, trigger),
            key(NODE_TYPE_JOB, &first),
            key(NODE_TYPE_JOB, &second),
        ]),
        "case {case}: each job is its own node, got {graph}"
    );
    assert_eq!(
        edges(case, &graph),
        BTreeSet::from([
            (
                key(NODE_TYPE_TRIGGER, trigger),
                key(NODE_TYPE_JOB, &first)
            ),
            (
                key(NODE_TYPE_TRIGGER, trigger),
                key(NODE_TYPE_JOB, &second)
            ),
        ]),
        "case {case}: each connection reaches its own node, got {graph}"
    );
    for node in nodes(case, &graph).values() {
        let unique_id = node
            .get("UniqueId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: a node reports a UniqueId, got {node}"));
        assert!(
            unique_id.chars().count() <= bound,
            "case {case}: a UniqueId is a NameString of at most {bound} characters, got {} in \
             {node}",
            unique_id.chars().count()
        );
    }
}

/// A workflow's graph holds only what is attached to THAT workflow, and only what the
/// endpoint serving the read holds: a trigger is served through the endpoint that created
/// it, while a workflow is per-account, so the same workflow read elsewhere reports the
/// components attached there — none.
#[tokio::test]
async fn a_graph_holds_only_the_triggers_of_that_workflow_and_that_endpoint() {
    let router = emulator();
    register_components(&router).await;

    let cases = [("the read workflow", "own-graph-workflow"), ("another workflow", "other-graph-workflow")];
    for (_case, workflow) in cases {
        ok(&router, "CreateWorkflow", &json!({ "Name": workflow })).await;
    }
    for (index, (_case, workflow)) in cases.iter().enumerate() {
        ok(
            &router,
            "CreateTrigger",
            &json!({
                "Name": format!("own-graph-trigger-{index}"),
                "Type": "ON_DEMAND",
                "WorkflowName": workflow,
                "Actions": [{ "JobName": JOB }],
            }),
        )
        .await;
    }

    // Each workflow's graph carries its own trigger and no other workflow's.
    for (index, (case, workflow)) in cases.iter().enumerate() {
        let graph = workflow_graph(&router, case, workflow).await;
        let own = format!("own-graph-trigger-{index}");
        assert_eq!(
            nodes(case, &graph).keys().cloned().collect::<BTreeSet<NodeKey>>(),
            BTreeSet::from([key(NODE_TYPE_TRIGGER, &own), key(NODE_TYPE_JOB, JOB)]),
            "case {case}: a workflow's graph holds only the triggers bound to it, got {graph}"
        );
    }

    // The same workflow, read through another Region's endpoint: the workflow is there and
    // the trigger is not, so the graph is the one the components of that endpoint make.
    let case = "another Region's endpoint";
    let (status, body) = call_in_region(
        &router,
        "us-west-2",
        "GetWorkflow",
        &json!({ "Name": cases[0].1, "IncludeGraph": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: a per-account workflow is readable in {REGION} and elsewhere, got {status} \
         with {body}"
    );
    let graph = body["Workflow"]
        .get("Graph")
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: IncludeGraph must report a Graph, got {body}"));
    assert!(
        nodes(case, &graph).is_empty(),
        "case {case}: a trigger of another endpoint is not a component of the graph, got {graph}"
    );
    assert!(
        edges(case, &graph).is_empty(),
        "case {case}: a graph with no nodes has no connections, got {graph}"
    );
}
