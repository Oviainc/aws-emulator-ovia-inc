// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what an export task WRITES: the elements `StartExportTask`
//! reports through `GetExportTask` once the clock has carried the task to the terminal
//! `SUCCEEDED`, driven over the wire (`restJson1`) through the same router the emulator
//! serves from.
//!
//! The rule every assertion here is about: the elements an export writes are the source
//! graph's own stored nodes and edges as the request's `exportFilter` selects them — "If
//! the ExportFilter is not provided, then all properties for all labels will be exported.
//! If the ExportFilter is provided but is an empty object, then no data will be exported"
//! (`smithy_query(query="shape",shape="ExportFilter")`), and each of that shape's two
//! members filters "on a per-label basis" for the vertices and for the edges — so a task
//! that ran to completion reports those counts, and a task still running reports neither.
//!
//! So the table spans what the rule generalises over: whether a filter is supplied, which
//! side of the graph it filters, how many labels it names, a secondary label of a
//! multi-labelled node, a label the graph does not hold, and a graph holding no element
//! at all. The graph's data is written through the store's own writers, which is where an
//! accepted openCypher write would put it; every call whose ANSWER is asserted goes over
//! the wire.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    service_state, GraphEdge, GraphNode, COMPLETE_PROGRESS_PERCENTAGE, EXPORT_TASK_SPAN,
    PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneExportRole";

/// An emulator holding only this service, with the engine it was built on, so a test can
/// write a graph's data the way the data plane writes it and then export it over the wire.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GET /exporttasks/{taskIdentifier}`.
async fn get_export(router: &MultiServiceRouter, task_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/exporttasks/{task_id}"), None).await
}

/// Create a graph, poll it into service and return its id. No handler ever sleeps; only
/// this test does, because it is asking about a state the clock reaches on its own.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case graph {name}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case graph {name}: no id in {created}"))
        .to_string();
    for _ in 0..40 {
        let (code, body) = call(router, "GET", &format!("/graphs/{id}"), None).await;
        assert_eq!(
            code, 200,
            "case graph {name}: expected 200 from GetGraph, got {code}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return id;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    panic!("case graph {name}: never became AVAILABLE");
}

/// The nodes and edges every row of the table below exports from: three nodes, one of
/// them carrying two labels, and three edges over two labels.
///
/// Written through the store's own writers, which is where an accepted openCypher write
/// puts a graph's data.
fn seed_sample_data(engine: &Arc<engine::InMemoryEngine>, graph_id: &str) {
    let as_of = SystemTime::now();
    let state = service_state(&**engine).expect("the neptune-graph store is mounted");
    let mut store = state.lock();
    let graph = store
        .graphs
        .get_mut(graph_id)
        .unwrap_or_else(|| panic!("the store holds the graph {graph_id}"));
    for (id, labels) in [
        ("n1", ["person"].as_slice()),
        ("n2", ["employee", "person"].as_slice()),
        ("n3", ["city"].as_slice()),
    ] {
        graph
            .data
            .insert_node(
                id,
                GraphNode {
                    labels: labels.iter().map(|label| (*label).to_string()).collect(),
                    properties: BTreeMap::new(),
                },
                as_of,
            )
            .expect("the sample graph takes its nodes");
    }
    for (id, label, from, to) in [
        ("e1", "knows", "n1", "n2"),
        ("e2", "knows", "n2", "n1"),
        ("e3", "livesIn", "n1", "n3"),
    ] {
        graph
            .data
            .insert_edge(
                id,
                GraphEdge {
                    label: label.to_string(),
                    from_node: from.to_string(),
                    to_node: to.to_string(),
                    properties: BTreeMap::new(),
                },
                as_of,
            )
            .expect("the sample graph takes its edges");
    }
}

/// `POST /exporttasks` for `graph_id`, with `filter` as its `exportFilter` when one is
/// supplied, returning the minted task id.
async fn start_export(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    filter: Option<Value>,
) -> String {
    let mut request = json!({
        "graphIdentifier": graph_id,
        "roleArn": ROLE_ARN,
        "format": "CSV",
        "destination": "s3://export-bucket/run/",
        "kmsKeyIdentifier": KMS_KEY_ARN,
    });
    if let Some(filter) = filter {
        request["exportFilter"] = filter;
    }
    let (status, started) = call(router, "POST", "/exporttasks", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from StartExportTask, got {status}: {started}"
    );
    assert_eq!(
        started["status"],
        json!("INITIALIZING"),
        "case {case:?}: no element has been written when the call returns, expected \
         INITIALIZING, got {}",
        started["status"]
    );
    started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
        .to_string()
}

/// Poll an export task until it reports the terminal `SUCCEEDED` its run ends in, and
/// return that read. Nothing in the service sleeps for those spans; only this test does.
async fn succeeded_task(router: &MultiServiceRouter, case: &str, task_id: &str) -> Value {
    for _ in 0..60 {
        let (status, body) = get_export(router, task_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetExportTask, got {status}: {body}"
        );
        if body["status"] == json!("SUCCEEDED") {
            return body;
        }
        tokio::time::sleep(EXPORT_TASK_SPAN / 4).await;
    }
    let (_, body) = get_export(router, task_id).await;
    panic!(
        "case {case:?}: task {task_id} never reached SUCCEEDED, last status {}",
        body["status"]
    );
}

/// One row of the filter table: the `exportFilter` a request carries and the elements an
/// export under it writes.
struct FilterCase {
    case: &'static str,
    filter: Option<Value>,
    vertices: i64,
    edges: i64,
}

/// A completed export reports the elements its filter selected out of the source graph's
/// stored nodes and edges — all of them when no filter is supplied, none when the filter
/// names no label, and exactly the elements carrying a label it names otherwise.
#[tokio::test]
async fn a_completed_export_reports_the_elements_its_filter_selected() {
    let (router, engine) = emulator();
    let graph_id = graph_in_service(&router, "export-run-source").await;
    seed_sample_data(&engine, &graph_id);

    let cases = vec![
        FilterCase {
            case: "no filter exports every element the graph holds",
            filter: None,
            vertices: 3,
            edges: 3,
        },
        FilterCase {
            case: "an empty filter object exports nothing",
            filter: Some(json!({})),
            vertices: 0,
            edges: 0,
        },
        FilterCase {
            case: "a vertex filter alone exports no edge",
            filter: Some(json!({"vertexFilter": {"person": {}}})),
            vertices: 2,
            edges: 0,
        },
        FilterCase {
            case: "an edge filter alone exports no vertex",
            filter: Some(json!({"edgeFilter": {"knows": {}}})),
            vertices: 0,
            edges: 2,
        },
        FilterCase {
            case: "one label named on each side",
            filter: Some(json!({
                "vertexFilter": {"city": {}},
                "edgeFilter": {"livesIn": {}},
            })),
            vertices: 1,
            edges: 1,
        },
        FilterCase {
            case: "a secondary label of a multi-labelled node, and both edge labels",
            filter: Some(json!({
                "vertexFilter": {
                    "employee": {
                        "properties": {
                            "name": {
                                "outputType": "String",
                                "sourcePropertyName": "name",
                                "multiValueHandling": "PICK_FIRST",
                            },
                        },
                    },
                },
                "edgeFilter": {"knows": {}, "livesIn": {}},
            })),
            vertices: 1,
            edges: 3,
        },
        FilterCase {
            case: "a label the graph does not hold selects nothing",
            filter: Some(json!({
                "vertexFilter": {"product": {}},
                "edgeFilter": {"bought": {}},
            })),
            vertices: 0,
            edges: 0,
        },
    ];

    // Every row's export is accepted first: each task's run is owed by the clock, so the
    // runs overlap instead of this test waiting out one span per row.
    let mut started = Vec::new();
    for row in &cases {
        let task_id = start_export(&router, row.case, &graph_id, row.filter.clone()).await;
        started.push(task_id);
    }

    for (row, task_id) in cases.iter().zip(&started) {
        let case = row.case;
        let settled = succeeded_task(&router, case, task_id).await;
        assert_eq!(
            settled["statusReason"],
            Value::Null,
            "case {case:?}: a run that completed has nothing to explain, got {}",
            settled["statusReason"]
        );
        let details = &settled["exportTaskDetails"];
        assert_eq!(
            details["progressPercentage"],
            json!(COMPLETE_PROGRESS_PERCENTAGE),
            "case {case:?}: a completed run has done all of its work, got {}",
            details["progressPercentage"]
        );
        for (member, expected) in [
            ("numVerticesWritten", row.vertices),
            ("numEdgesWritten", row.edges),
        ] {
            assert_eq!(
                details[member],
                json!(expected),
                "case {case:?}: expected {member} {expected}, got {}",
                details[member]
            );
        }
    }

    // The same rule over a graph holding NO element: an export of it writes nothing,
    // whether or not a filter narrows what it would have written.
    let empty_graph = graph_in_service(&router, "export-run-empty").await;
    for (case, filter) in [
        ("an empty graph with no filter", None),
        (
            "an empty graph with a vertex filter",
            Some(json!({"vertexFilter": {"person": {}}})),
        ),
    ] {
        let task_id = start_export(&router, case, &empty_graph, filter).await;
        let settled = succeeded_task(&router, case, &task_id).await;
        let details = &settled["exportTaskDetails"];
        for member in ["numVerticesWritten", "numEdgesWritten"] {
            assert_eq!(
                details[member],
                json!(0),
                "case {case:?}: the graph holds no element, so {member} is 0, got {}",
                details[member]
            );
        }
    }
}

/// A written-element count is reported only once the run is over, and it is a count of
/// the data the export READ: writes made to the graph after the export was accepted do
/// not change what the task reports, and reading the task never moves it on.
#[tokio::test]
async fn the_counts_are_of_the_data_the_export_read_and_only_once_the_run_is_over() {
    let (router, engine) = emulator();
    let graph_id = graph_in_service(&router, "export-run-snapshot").await;
    seed_sample_data(&engine, &graph_id);

    let case = "an export accepted before the graph grew";
    let task_id = start_export(&router, case, &graph_id, None).await;

    // Read straight after the start call: the run has not written anything yet, so
    // neither count is reported rather than a zero that would claim a completed run.
    let (status, early) = get_export(&router, &task_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetExportTask, got {status}: {early}"
    );
    assert_eq!(
        early["status"],
        json!("INITIALIZING"),
        "case {case:?}: a task read straight after the start call is still INITIALIZING, \
         got {}",
        early["status"]
    );
    for member in ["numVerticesWritten", "numEdgesWritten"] {
        assert_eq!(
            early["exportTaskDetails"][member],
            Value::Null,
            "case {case:?}: nothing is written yet, so {member} is not reported, got {}",
            early["exportTaskDetails"][member]
        );
    }

    // The graph grows while the export runs: the export reads one view of the graph, so
    // the elements it writes are the ones it was accepted over.
    {
        let as_of = SystemTime::now();
        let state = service_state(&*engine).expect("the neptune-graph store is mounted");
        let mut store = state.lock();
        let graph = store
            .graphs
            .get_mut(&graph_id)
            .expect("the store holds the graph");
        graph
            .data
            .insert_node(
                "n4",
                GraphNode {
                    labels: ["person".to_string()].into_iter().collect(),
                    properties: BTreeMap::new(),
                },
                as_of,
            )
            .expect("the graph takes another node");
    }

    let settled = succeeded_task(&router, case, &task_id).await;
    let details = &settled["exportTaskDetails"];
    for (member, expected) in [("numVerticesWritten", 3), ("numEdgesWritten", 3)] {
        assert_eq!(
            details[member],
            json!(expected),
            "case {case:?}: expected the {expected} elements the export read in {member}, \
             got {}",
            details[member]
        );
    }

    // A read reports; it does not move the record on. A second read answers the same
    // terminal status, the same progress and the same counts.
    let (again_status, again) = get_export(&router, &task_id).await;
    assert_eq!(
        again_status, 200,
        "case {case:?}: expected 200 from the second read, got {again_status}: {again}"
    );
    for member in ["status", "statusReason"] {
        assert_eq!(
            again[member], settled[member],
            "case {case:?}: a read must not change {member}, expected {}, got {}",
            settled[member], again[member]
        );
    }
    for member in [
        "progressPercentage",
        "numVerticesWritten",
        "numEdgesWritten",
    ] {
        assert_eq!(
            again["exportTaskDetails"][member], details[member],
            "case {case:?}: a read must not change {member}, expected {}, got {}",
            details[member], again["exportTaskDetails"][member]
        );
    }

    // An export READS its source graph: the graph is where it was.
    let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
    assert_eq!(
        graph_status, 200,
        "case {case:?}: expected 200 from GetGraph, got {graph_status}: {graph}"
    );
    assert_eq!(
        graph["status"],
        json!("AVAILABLE"),
        "case {case:?}: exporting reads the graph, so it stays AVAILABLE, got {}",
        graph["status"]
    );

    // And the write made while the export ran is part of the graph's data: a later
    // export of the same graph writes the element this one did not.
    let later = "an export accepted after the graph grew";
    let later_task = start_export(&router, later, &graph_id, None).await;
    let later_settled = succeeded_task(&router, later, &later_task).await;
    assert_eq!(
        later_settled["exportTaskDetails"]["numVerticesWritten"],
        json!(4),
        "case {later:?}: expected the 4 vertices the graph now holds, got {}",
        later_settled["exportTaskDetails"]["numVerticesWritten"]
    );
}
