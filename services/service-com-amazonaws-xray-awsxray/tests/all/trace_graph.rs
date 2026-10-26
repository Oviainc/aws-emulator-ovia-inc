// SPDX-License-Identifier: CC0-1.0

//! `GetTraceGraph`: the service graph of named traces, end to end over the wire.
//!
//! Every case ingests segment documents with `PutTraceSegments` and reads the
//! graph back with `GetTraceGraph` through the same front door the production
//! router uses, so the assertions are about HTTP responses.
//!
//! The rules under test, each checked over a table whose rows differ along what
//! the rule generalises over — the shape of the trace (a chain, a fan-out, a
//! repeated call, an in-progress segment), what each segment recorded, and
//! whether a subsegment left the process:
//!
//! * a node is one distinct service `name` the stored segments report, and
//!   beside every ROOT service stands the node `Service$Type` documents as
//!   `client`, "represents the clients that sent requests to a root service",
//!   carrying that service's own name with one edge into it;
//! * a subsegment reports no service of its own unless its `namespace` says
//!   the call left the process: `remote` names "a downstream service of
//!   indeterminate type" and `aws` an AWS service, while a subsegment with no
//!   such namespace adds NO downstream node and no edge, however it arrived
//!   (inline or as its own `type: "subsegment"` document) and however deeply it
//!   is nested;
//! * an edge exists exactly where a stored `parent_id` links segments of two
//!   DIFFERENT services, and each edge names its downstream node by a
//!   `ReferenceId` that resolves inside the same graph;
//! * every node's `Root`, `State`, window and `SummaryStatistics` are
//!   recomputed from the stored segments: only completed requests are counted,
//!   classified fault / throttle / error / ok by the flags their documents
//!   declared, and a node the graph knows only through another service's
//!   downstream subsegment is `passive` rather than `active` or `unknown`;
//! * the graph covers exactly the requested traces, a `NextToken` is emitted
//!   only while nodes are unread and resumes exactly once, and a request this
//!   operation cannot serve — an unstored trace id, a token it never issued,
//!   more ids than it accepts — is REFUSED with `InvalidRequestException`.

use serde_json::{json, Value};
use service_com_amazonaws_xray_awsxray::ops::get_trace_graph::{
    TRACE_GRAPH_PAGE_SIZE, TRACE_ID_NOT_FOUND,
};
use service_com_amazonaws_xray_awsxray::state::{
    MAX_TRACE_IDS_PER_CALL, SERVICE_STATE_ACTIVE, SERVICE_STATE_PASSIVE, SERVICE_STATE_UNKNOWN,
    SERVICE_TYPE_CLIENT,
};

use super::trace_ingest_and_groups::{array, close, trace_id, Answer, Xray};

/// `GetTraceGraph` for the given ids, optionally resuming from a token.
async fn trace_graph(xray: &Xray, ids: &[String], token: Option<&str>) -> Answer {
    let mut body = json!({ "TraceIds": ids });
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/TraceGraph", &body).await
}

/// One node of an answered graph, as the assertions compare it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Node {
    name: String,
    node_type: Option<String>,
    root: bool,
    state: String,
}

/// What a table row expects of a node: `Name`, `Type`, `Root`, `State`.
type ExpectedNode = (&'static str, Option<&'static str>, bool, &'static str);

impl Node {
    /// The expectation a row writes, as a `Node`.
    fn expected((name, node_type, root, state): ExpectedNode) -> Self {
        Self {
            name: name.to_string(),
            node_type: node_type.map(str::to_string),
            root,
            state: state.to_string(),
        }
    }
}

/// The answered nodes, sorted so a row compares a set rather than an order the
/// model does not promise.
fn nodes(answer: &Answer) -> Vec<Node> {
    let mut nodes: Vec<Node> = array(&answer.body, "Services")
        .iter()
        .map(|service| Node {
            name: service["Name"].as_str().unwrap_or_default().to_string(),
            node_type: service["Type"].as_str().map(str::to_string),
            root: service["Root"].as_bool().unwrap_or_default(),
            state: service["State"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    nodes.sort();
    nodes
}

/// A node's label for an edge assertion: its name, and the type that tells a
/// `client` node apart from the service it calls.
fn label(service: &Value) -> String {
    let name = service["Name"].as_str().unwrap_or_default().to_string();
    match service["Type"].as_str() {
        Some(node_type) => format!("{name}#{node_type}"),
        None => name,
    }
}

/// The answered edges as `(caller, downstream)` labels, resolved through
/// `ReferenceId`, sorted.
fn edges(answer: &Answer) -> Vec<(String, String)> {
    let services = array(&answer.body, "Services");
    let by_reference: std::collections::BTreeMap<i64, String> = services
        .iter()
        .map(|service| (service["ReferenceId"].as_i64().unwrap_or(-1), label(service)))
        .collect();
    let mut found = Vec::new();
    for service in services {
        for edge in array(service, "Edges") {
            let downstream = edge["ReferenceId"].as_i64().unwrap_or(-1);
            found.push((
                label(service),
                by_reference
                    .get(&downstream)
                    .cloned()
                    .unwrap_or_else(|| format!("<unresolved reference {downstream}>")),
            ));
        }
    }
    found.sort();
    found
}

/// A segment document: `name`, with the given id, parent and span.
fn segment(
    trace: &str,
    id: &str,
    parent: Option<&str>,
    name: &str,
    start: f64,
    end: Option<f64>,
) -> Value {
    let mut document = json!({
        "trace_id": trace,
        "id": id,
        "name": name,
        "start_time": start,
    });
    if let Some(parent) = parent {
        document["parent_id"] = json!(parent);
    }
    match end {
        Some(end) => document["end_time"] = json!(end),
        None => document["in_progress"] = json!(true),
    }
    document
}

/// One inline subsegment, with or without a `namespace`.
fn subsegment(id: &str, name: &str, namespace: Option<&str>) -> Value {
    let mut document = json!({
        "id": id,
        "name": name,
        "start_time": 100.1_f64,
        "end_time": 100.3_f64,
    });
    if let Some(namespace) = namespace {
        document["namespace"] = json!(namespace);
    }
    document
}

// =============================================================================
// 1. Which stored data becomes a node, and which does not
// =============================================================================

/// One ingest -> graph case.
struct GraphCase {
    case: &'static str,
    trace: String,
    documents: Vec<String>,
    nodes: Vec<ExpectedNode>,
    edges: Vec<(&'static str, &'static str)>,
}

fn graph_cases() -> Vec<GraphCase> {
    let mut cases = Vec::new();

    // Row 1 is the recorded case: a segment with an inline subsegment that
    // declares no `namespace`. The subsegment records work inside the service's
    // own process, so the graph holds that service and its client, and no
    // downstream node named after the subsegment.
    let trace = trace_id(201);
    let mut root = segment(&trace, "2010000000000001", None, "frontend", 100.0, Some(100.5));
    root["subsegments"] = json!([subsegment("2010000000000002", "local-work", None)]);
    cases.push(GraphCase {
        case: "a subsegment without a namespace adds no downstream node",
        trace: trace.clone(),
        documents: vec![root.to_string()],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
        ],
        edges: vec![("frontend#client", "frontend")],
    });

    // The same holds whatever the subsegment carries beside its name: it is the
    // absence of a namespace that makes it in-process work.
    let trace = trace_id(202);
    let mut root = segment(&trace, "2020000000000001", None, "frontend", 100.0, Some(100.5));
    let mut inline = subsegment("2020000000000002", "GET /inventory", None);
    inline["http"] = json!({"request": {"method": "GET", "url": "http://localhost/inventory"}, "response": {"status": 200}});
    inline["subsegments"] = json!([subsegment("2020000000000003", "nested-work", None)]);
    root["subsegments"] = json!([inline]);
    cases.push(GraphCase {
        case: "a nested subsegment with an http block but no namespace adds no node",
        trace: trace.clone(),
        documents: vec![root.to_string()],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
        ],
        edges: vec![("frontend#client", "frontend")],
    });

    // A downstream service that reported its OWN segment under the caller is a
    // node, reached by one edge, and is not a root: its segment has a parent.
    let trace = trace_id(203);
    cases.push(GraphCase {
        case: "a chain of two services is two nodes, one edge, plus the root's client",
        trace: trace.clone(),
        documents: vec![
            segment(&trace, "2030000000000001", None, "frontend", 100.0, Some(100.5)).to_string(),
            segment(
                &trace,
                "2030000000000002",
                Some("2030000000000001"),
                "backend",
                100.1,
                Some(100.4),
            )
            .to_string(),
        ],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
            ("backend", None, false, SERVICE_STATE_ACTIVE),
        ],
        edges: vec![("frontend", "backend"), ("frontend#client", "frontend")],
    });

    // A fan-out is one edge per distinct downstream service.
    let trace = trace_id(204);
    cases.push(GraphCase {
        case: "a fan-out is one edge per downstream service",
        trace: trace.clone(),
        documents: vec![
            segment(&trace, "2040000000000001", None, "frontend", 100.0, Some(100.9)).to_string(),
            segment(
                &trace,
                "2040000000000002",
                Some("2040000000000001"),
                "backend",
                100.1,
                Some(100.4),
            )
            .to_string(),
            segment(
                &trace,
                "2040000000000003",
                Some("2040000000000001"),
                "cache",
                100.2,
                Some(100.3),
            )
            .to_string(),
        ],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
            ("backend", None, false, SERVICE_STATE_ACTIVE),
            ("cache", None, false, SERVICE_STATE_ACTIVE),
        ],
        edges: vec![
            ("frontend", "backend"),
            ("frontend", "cache"),
            ("frontend#client", "frontend"),
        ],
    });

    // Two segments of ONE service are one node, and a call a service makes to
    // itself is no edge: an edge links segments of DIFFERENT services.
    let trace = trace_id(205);
    cases.push(GraphCase {
        case: "two segments of one service are one node with no self-edge",
        trace: trace.clone(),
        documents: vec![
            segment(&trace, "2050000000000001", None, "worker", 100.0, Some(100.5)).to_string(),
            segment(
                &trace,
                "2050000000000002",
                Some("2050000000000001"),
                "worker",
                100.1,
                Some(100.2),
            )
            .to_string(),
        ],
        nodes: vec![
            ("worker", None, true, SERVICE_STATE_ACTIVE),
            ("worker", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
        ],
        edges: vec![("worker#client", "worker")],
    });

    // A service whose only segment is still in progress completed nothing, so
    // nothing is known about its state.
    let trace = trace_id(206);
    cases.push(GraphCase {
        case: "a service with only an in-progress segment is in an unknown state",
        trace: trace.clone(),
        documents: vec![
            segment(&trace, "2060000000000001", None, "slow", 100.0, None).to_string(),
        ],
        nodes: vec![
            ("slow", None, true, SERVICE_STATE_UNKNOWN),
            ("slow", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
        ],
        edges: vec![("slow#client", "slow")],
    });

    // A subsegment that declares a `namespace` recorded a call that LEFT the
    // process, which names a downstream service: `remote` is "a downstream
    // service of indeterminate type".
    let trace = trace_id(207);
    let mut root = segment(&trace, "2070000000000001", None, "frontend", 100.0, Some(100.5));
    root["subsegments"] = json!([subsegment("2070000000000002", "payments", Some("remote"))]);
    cases.push(GraphCase {
        case: "a remote-namespace subsegment adds one downstream node and one edge",
        trace: trace.clone(),
        documents: vec![root.to_string()],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
            ("payments", Some("remote"), false, SERVICE_STATE_PASSIVE),
        ],
        edges: vec![
            ("frontend", "payments#remote"),
            ("frontend#client", "frontend"),
        ],
    });

    // `aws` says the call was an AWS SDK call, which `Service$Type` types as
    // `AWS::<Service>`.
    let trace = trace_id(208);
    let mut root = segment(&trace, "2080000000000001", None, "frontend", 100.0, Some(100.5));
    root["subsegments"] = json!([subsegment("2080000000000002", "DynamoDB", Some("aws"))]);
    cases.push(GraphCase {
        case: "an aws-namespace subsegment adds an AWS-typed downstream node",
        trace: trace.clone(),
        documents: vec![root.to_string()],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
            ("DynamoDB", Some("AWS::DynamoDB"), false, SERVICE_STATE_PASSIVE),
        ],
        edges: vec![
            ("frontend", "DynamoDB#AWS::DynamoDB"),
            ("frontend#client", "frontend"),
        ],
    });

    // Nesting changes nothing: a remote subsegment nested inside a local one is
    // still a call that left the process, and the local one still is not.
    let trace = trace_id(209);
    let mut root = segment(&trace, "2090000000000001", None, "frontend", 100.0, Some(100.5));
    let mut local = subsegment("2090000000000002", "local-work", None);
    local["subsegments"] = json!([subsegment("2090000000000003", "cache", Some("remote"))]);
    root["subsegments"] = json!([local]);
    cases.push(GraphCase {
        case: "a remote subsegment nested under a local one still adds its node",
        trace: trace.clone(),
        documents: vec![root.to_string()],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
            ("cache", Some("remote"), false, SERVICE_STATE_PASSIVE),
        ],
        edges: vec![("frontend", "cache#remote"), ("frontend#client", "frontend")],
    });

    // A subsegment sent as its OWN `type: "subsegment"` document follows the
    // same rule: with a namespace it names a downstream service, and the
    // document itself is never a service of the graph.
    let trace = trace_id(210);
    let mut sent_alone = segment(
        &trace,
        "2100000000000002",
        Some("2100000000000001"),
        "queue",
        100.1,
        Some(100.3),
    );
    sent_alone["type"] = json!("subsegment");
    sent_alone["namespace"] = json!("remote");
    let mut local_alone = segment(
        &trace,
        "2100000000000003",
        Some("2100000000000001"),
        "local-work",
        100.1,
        Some(100.2),
    );
    local_alone["type"] = json!("subsegment");
    cases.push(GraphCase {
        case: "a subsegment sent as its own document follows the namespace rule",
        trace: trace.clone(),
        documents: vec![
            segment(&trace, "2100000000000001", None, "frontend", 100.0, Some(100.5)).to_string(),
            sent_alone.to_string(),
            local_alone.to_string(),
        ],
        nodes: vec![
            ("frontend", None, true, SERVICE_STATE_ACTIVE),
            ("frontend", Some(SERVICE_TYPE_CLIENT), false, SERVICE_STATE_UNKNOWN),
            ("queue", Some("remote"), false, SERVICE_STATE_PASSIVE),
        ],
        edges: vec![("frontend", "queue#remote"), ("frontend#client", "frontend")],
    });

    cases
}

#[tokio::test]
async fn the_trace_graph_holds_the_reported_services_and_each_root_services_client() {
    for case in graph_cases() {
        let label = case.case;
        // One emulator per row, so each graph is exactly its own trace's.
        let xray = Xray::new();
        let put = xray.put_segments(&case.documents).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments answered {} with {}",
            put.status, put.body
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {label:?}: documents were refused: {}",
            put.get("UnprocessedTraceSegments")
        );

        let got = trace_graph(&xray, std::slice::from_ref(&case.trace), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: GetTraceGraph answered {} with {}",
            got.status, got.body
        );

        let mut expected: Vec<Node> = case.nodes.iter().copied().map(Node::expected).collect();
        expected.sort();
        let actual = nodes(&got);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected nodes {expected:?}, got {actual:?} (body {})",
            got.body
        );

        let mut expected_edges: Vec<(String, String)> = case
            .edges
            .iter()
            .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
            .collect();
        expected_edges.sort();
        let actual_edges = edges(&got);
        assert_eq!(
            actual_edges, expected_edges,
            "case {label:?}: expected edges {expected_edges:?}, got {actual_edges:?} (body {})",
            got.body
        );

        // "Identifier for the service. Unique within the service map."
        let references: Vec<i64> = array(&got.body, "Services")
            .iter()
            .map(|service| service["ReferenceId"].as_i64().unwrap_or(-1))
            .collect();
        let mut distinct = references.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            references.len(),
            "case {label:?}: ReferenceIds repeat: {references:?}"
        );
        assert!(
            references.iter().all(|reference| *reference >= 0),
            "case {label:?}: a node carries no ReferenceId: {references:?}"
        );
    }
}

// =============================================================================
// 2. A node's statistics, from what its segments recorded
// =============================================================================

/// One statistics case: a root service whose segments recorded these outcomes.
struct StatisticsCase {
    case: &'static str,
    trace: String,
    documents: Vec<String>,
    ok: i64,
    throttle: i64,
    error_other: i64,
    fault: i64,
    total: i64,
    total_response_time: f64,
}

/// A completed root segment spanning `span` seconds, recording `status` and
/// DECLARING the outcome `flag` beside it, the way an instrumentation that saw
/// that status does.
fn recording(
    trace: &str,
    id: &str,
    start: f64,
    span: f64,
    status: Option<i64>,
    flag: Option<&str>,
) -> String {
    let mut document = segment(trace, id, None, "svc", start, Some(start + span));
    if let Some(status) = status {
        document["http"] = json!({"request": {"method": "GET", "url": "http://localhost/"}, "response": {"status": status}});
    }
    if let Some(flag) = flag {
        document[flag] = json!(true);
    }
    document.to_string()
}

fn statistics_cases() -> Vec<StatisticsCase> {
    // The rows walk the outcome classes a DECLARED flag selects — none ok, an
    // error, a throttle, a fault — beside the status an instrumentation records
    // with each, plus a 500 status that declares nothing (which records no
    // outcome), a request that never completed, and a service that handled more
    // than one.
    let mut cases = Vec::new();

    let trace = trace_id(211);
    cases.push(StatisticsCase {
        case: "a completed 200 is one ok request",
        trace: trace.clone(),
        documents: vec![recording(&trace, "2110000000000001", 200.0, 0.5, Some(200), None)],
        ok: 1,
        throttle: 0,
        error_other: 0,
        fault: 0,
        total: 1,
        total_response_time: 0.5,
    });

    let trace = trace_id(212);
    cases.push(StatisticsCase {
        case: "a completed 404 that declares an error is one other error",
        trace: trace.clone(),
        documents: vec![recording(
            &trace,
            "2120000000000001",
            200.0,
            0.25,
            Some(404),
            Some("error"),
        )],
        ok: 0,
        throttle: 0,
        error_other: 1,
        fault: 0,
        total: 1,
        total_response_time: 0.25,
    });

    let trace = trace_id(213);
    cases.push(StatisticsCase {
        case: "a completed 429 that declares a throttle is one throttled error",
        trace: trace.clone(),
        documents: vec![recording(
            &trace,
            "2130000000000001",
            200.0,
            0.125,
            Some(429),
            Some("throttle"),
        )],
        ok: 0,
        throttle: 1,
        error_other: 0,
        fault: 0,
        total: 1,
        total_response_time: 0.125,
    });

    let trace = trace_id(214);
    cases.push(StatisticsCase {
        case: "a completed 503 that declares a fault is one fault",
        trace: trace.clone(),
        documents: vec![recording(
            &trace,
            "2140000000000001",
            200.0,
            0.75,
            Some(503),
            Some("fault"),
        )],
        ok: 0,
        throttle: 0,
        error_other: 0,
        fault: 1,
        total: 1,
        total_response_time: 0.75,
    });

    // The status alone declares no outcome, so the request is one its service
    // completed without error or fault.
    let trace = trace_id(216);
    cases.push(StatisticsCase {
        case: "a completed 500 that declares no fault is one ok request",
        trace: trace.clone(),
        documents: vec![recording(&trace, "2160000000000001", 200.0, 0.5, Some(500), None)],
        ok: 1,
        throttle: 0,
        error_other: 0,
        fault: 0,
        total: 1,
        total_response_time: 0.5,
    });

    let trace = trace_id(215);
    cases.push(StatisticsCase {
        case: "three segments of one service aggregate into its statistics",
        trace: trace.clone(),
        documents: vec![
            recording(&trace, "2150000000000001", 200.0, 0.25, Some(200), None),
            recording(&trace, "2150000000000002", 201.0, 0.5, Some(200), None),
            recording(
                &trace,
                "2150000000000003",
                202.0,
                0.5,
                Some(500),
                Some("fault"),
            ),
        ],
        ok: 2,
        throttle: 0,
        error_other: 0,
        fault: 1,
        total: 3,
        total_response_time: 1.25,
    });

    cases
}

#[tokio::test]
async fn a_nodes_statistics_count_what_its_segments_recorded() {
    for case in statistics_cases() {
        let label = case.case;
        let xray = Xray::new();
        let put = xray.put_segments(&case.documents).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments answered {} with {}",
            put.status, put.body
        );

        let got = trace_graph(&xray, std::slice::from_ref(&case.trace), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: GetTraceGraph answered {} with {}",
            got.status, got.body
        );
        // The service's own node is the one the stored segments reported; the
        // `client` node beside it carries the requests its clients SENT.
        let services = array(&got.body, "Services");
        let reported: Vec<&Value> = services
            .iter()
            .filter(|service| service["Type"].as_str() != Some(SERVICE_TYPE_CLIENT))
            .collect();
        assert_eq!(
            reported.len(),
            1,
            "case {label:?}: expected exactly one reported node, got {}",
            got.body
        );
        let statistics = &reported[0]["SummaryStatistics"];

        for (member, expected) in [("OkCount", case.ok), ("TotalCount", case.total)] {
            let actual = statistics[member].as_i64();
            assert_eq!(
                actual,
                Some(expected),
                "case {label:?}: expected {member} {expected}, got {actual:?} ({statistics})"
            );
        }
        let errors = &statistics["ErrorStatistics"];
        for (member, expected) in [
            ("ThrottleCount", case.throttle),
            ("OtherCount", case.error_other),
            ("TotalCount", case.throttle + case.error_other),
        ] {
            let actual = errors[member].as_i64();
            assert_eq!(
                actual,
                Some(expected),
                "case {label:?}: expected ErrorStatistics.{member} {expected}, got {actual:?} ({errors})"
            );
        }
        let faults = &statistics["FaultStatistics"];
        for (member, expected) in [("OtherCount", case.fault), ("TotalCount", case.fault)] {
            let actual = faults[member].as_i64();
            assert_eq!(
                actual,
                Some(expected),
                "case {label:?}: expected FaultStatistics.{member} {expected}, got {actual:?} ({faults})"
            );
        }
        let response_time = statistics["TotalResponseTime"].as_f64();
        assert!(
            response_time.is_some_and(|actual| close(actual, case.total_response_time)),
            "case {label:?}: expected TotalResponseTime {}, got {response_time:?}",
            case.total_response_time
        );
    }
}

// =============================================================================
// 3. Pagination and refusals
// =============================================================================

#[tokio::test]
async fn a_next_token_appears_only_when_nodes_are_unread_and_resumes_exactly_once() {
    let xray = Xray::new();
    let trace = trace_id(221);
    // More nodes than one page carries: a root service, its client, and enough
    // distinctly named downstream services to spill over.
    let downstream = TRACE_GRAPH_PAGE_SIZE + 1;
    let mut documents =
        vec![segment(&trace, "2210000000000001", None, "frontend", 100.0, Some(101.0)).to_string()];
    for index in 0..downstream {
        documents.push(
            segment(
                &trace,
                &format!("2211000000{index:06}"),
                Some("2210000000000001"),
                &format!("downstream-{index:02}"),
                100.1,
                Some(100.2),
            )
            .to_string(),
        );
    }
    let put = xray.put_segments(&documents).await;
    assert_eq!(put.status, 200, "the segments land: {}", put.body);

    let mut token: Option<String> = None;
    let mut seen: Vec<i64> = Vec::new();
    let mut pages = 0;
    loop {
        let answer = trace_graph(&xray, std::slice::from_ref(&trace), token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "page {pages}: GetTraceGraph answered {} with {}",
            answer.status, answer.body
        );
        let services = array(&answer.body, "Services");
        assert!(
            services.len() <= TRACE_GRAPH_PAGE_SIZE,
            "page {pages}: carried {} nodes, past the page size {TRACE_GRAPH_PAGE_SIZE}",
            services.len()
        );
        seen.extend(
            services
                .iter()
                .map(|service| service["ReferenceId"].as_i64().unwrap_or(-1)),
        );
        token = answer.get("NextToken").as_str().map(str::to_string);
        pages += 1;
        if token.is_none() {
            break;
        }
        assert!(pages <= 8, "pagination did not terminate after {pages} pages");
    }

    // One node per reported service, plus the root's client, each read once.
    let total = i64::try_from(downstream + 2).expect("the count fits");
    let expected: Vec<i64> = (0..total).collect();
    let mut distinct = seen.clone();
    distinct.sort_unstable();
    assert_eq!(
        distinct, expected,
        "expected every node exactly once ({expected:?}), got {seen:?} over {pages} pages"
    );
    assert!(pages >= 2, "expected the nodes to span pages, got {pages}");
}

#[tokio::test]
async fn a_request_the_operation_cannot_serve_as_asked_is_refused_as_invalid() {
    let xray = Xray::new();
    let stored = trace_id(231);
    let put = xray
        .put_segments(&[
            segment(&stored, "2310000000000001", None, "frontend", 100.0, Some(100.5)).to_string(),
        ])
        .await;
    assert_eq!(put.status, 200, "the segment lands: {}", put.body);

    let too_many: Vec<String> = (0..=MAX_TRACE_IDS_PER_CALL)
        .map(|n| trace_id(u32::try_from(n).expect("the count fits")))
        .collect();

    let cases: Vec<(&str, Vec<String>, Option<&str>)> = vec![
        (
            "a trace id the store does not hold",
            vec![trace_id(239)],
            None,
        ),
        (
            "a stored id beside an unstored one",
            vec![stored.clone(), trace_id(239)],
            None,
        ),
        (
            "a NextToken that is not a ReferenceId",
            vec![stored.clone()],
            Some("not-a-token"),
        ),
        (
            "a NextToken past the last node",
            vec![stored.clone()],
            Some("97"),
        ),
        ("more trace ids than one call accepts", too_many, None),
    ];

    for (label, ids, token) in cases {
        let refused = trace_graph(&xray, &ids, token).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected HTTP 400, got {} with {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {label:?}: expected InvalidRequestException, got {:?} with {}",
            refused.error_type,
            refused.body
        );
        assert!(
            refused
                .get("Message")
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {label:?}: the refusal carries no Message: {}",
            refused.body
        );
    }

    // The unstored-id refusal is the recorded wording, and it is the whole
    // request that is refused rather than the id quietly dropped.
    let refused = trace_graph(&xray, &[trace_id(239)], None).await;
    assert_eq!(
        refused.get("Message").as_str(),
        Some(TRACE_ID_NOT_FOUND),
        "expected the recorded refusal wording, got {}",
        refused.body
    );
}
