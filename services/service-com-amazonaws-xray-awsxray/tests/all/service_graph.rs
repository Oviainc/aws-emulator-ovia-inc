// SPDX-License-Identifier: CC0-1.0

//! Invariants of the service graph `GetServiceGraph` answers, end to end over
//! the wire.
//!
//! Every case drives the same front door the production router does (the
//! harness in `trace_ingest_and_groups`), so the assertions below are about
//! HTTP responses rather than internal state. What the graph REPORTS for one
//! fixture is pinned by `graphs_rules_retrievals_and_insights`; what is under
//! test here is what must hold for ANY stored traffic, and the shapes a
//! recomputed graph has to survive:
//!
//! * every completed request is counted exactly once, so
//!   `OkCount + ErrorStatistics.TotalCount + FaultStatistics.TotalCount ==
//!   TotalCount` on every node and every edge, and each histogram's `Count`s sum
//!   to no more than that total;
//! * an edge's `ReferenceId` names a node of the same graph, on whatever page
//!   that node lands, and paging answers every node exactly once;
//! * a trace shape no SDK emits — a deep chain, a `parent_id` that names a
//!   segment of another trace, two segments naming each other as parent — is
//!   ANSWERED from what is stored rather than refused or left to loop;
//! * an instant a reader cannot turn into the modelled `Timestamp` never
//!   reaches the store: a document carrying one is refused at ingest, and one at
//!   the far edge of the accepted range is served.
//!
//! The rows differ along what each rule generalises over: the topology (a chain,
//! a fan-out, repeated calls, two traces through one service, a cycle), the
//! recorded status (200, 404, 429, 503), completeness (an in-progress segment),
//! chain depth, the node count relative to a page, and the magnitude of the
//! recorded instants.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::trace_ingest_and_groups::{array, close, root_document, trace_id, Answer, Xray};

/// `GetServiceGraph` over a window, optionally resumed from a token.
async fn service_graph(xray: &Xray, start: f64, end: f64, token: Option<&str>) -> Answer {
    let mut body = json!({ "StartTime": start, "EndTime": end });
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/ServiceGraph", &body).await
}

/// A segment document with a parent, a span and an optional recorded status.
fn child_document(
    id: &str,
    parent: &str,
    trace: &str,
    name: &str,
    start: f64,
    end: Option<f64>,
    status: Option<i64>,
) -> Value {
    let mut document = root_document(id, trace, name, start, end);
    document["parent_id"] = json!(parent);
    if let Some(status) = status {
        document["http"] = json!({"response": {"status": status}});
    }
    document
}

/// Now, in seconds of epoch time: the fixtures record traffic just before it so
/// every window names an interval of recorded traffic.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The counts one `SummaryStatistics` block carries. A member the answer leaves
/// off reads as `-1`, so a missing count fails its assertion rather than
/// passing as a zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    total: i64,
    ok: i64,
    errors: i64,
    throttle: i64,
    error_other: i64,
    faults: i64,
}

fn counts_of(summary: &Value) -> Counts {
    let number = |parent: &Value, member: &str| parent.get(member).and_then(Value::as_i64).unwrap_or(-1);
    let errors = summary.get("ErrorStatistics").cloned().unwrap_or(Value::Null);
    let faults = summary.get("FaultStatistics").cloned().unwrap_or(Value::Null);
    Counts {
        total: number(summary, "TotalCount"),
        ok: number(summary, "OkCount"),
        errors: number(&errors, "TotalCount"),
        throttle: number(&errors, "ThrottleCount"),
        error_other: number(&errors, "OtherCount"),
        faults: number(&faults, "TotalCount"),
    }
}

/// The `Count`s of one histogram member.
fn histogram_counts(owner: &Value, member: &str) -> i64 {
    array(owner, member)
        .iter()
        .map(|entry| entry.get("Count").and_then(Value::as_i64).unwrap_or(-1))
        .sum()
}

/// Assert the counting invariants on one `SummaryStatistics` owner (a `Service`
/// or an `Edge`).
fn assert_counts(case: &str, what: &str, owner: &Value) {
    let summary = owner
        .get("SummaryStatistics")
        .cloned()
        .unwrap_or(Value::Null);
    let counts = counts_of(&summary);
    assert!(
        counts.total >= 0 && counts.ok >= 0 && counts.errors >= 0 && counts.faults >= 0,
        "case {case:?}: {what} must carry every count, got {summary:?}"
    );
    assert_eq!(
        counts.ok + counts.errors + counts.faults,
        counts.total,
        "case {case:?}: {what} must count each completed request exactly once, got {counts:?}"
    );
    assert_eq!(
        counts.throttle + counts.error_other,
        counts.errors,
        "case {case:?}: {what} ErrorStatistics must split into throttles and others, got {counts:?}"
    );
    for member in ["ResponseTimeHistogram", "DurationHistogram"] {
        let counted = histogram_counts(owner, member);
        assert!(
            counted <= counts.total,
            "case {case:?}: {what} {member} counts ({counted}) must not exceed TotalCount ({}), got {owner:?}",
            counts.total
        );
    }
}

/// Walk every page of the graph of `[start, end)`, answering the services in
/// order. Fails the test if paging does not terminate or repeats a service.
async fn whole_graph(case: &str, xray: &Xray, start: f64, end: f64) -> Vec<Value> {
    let mut services: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answer = service_graph(xray, start, end, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetServiceGraph must succeed, got {answer:?}"
        );
        for service in array(&answer.body, "Services") {
            let name = service
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // A node is its NAME and its TYPE: a root service's `client` node
            // carries that service's own name, so the name alone no longer
            // identifies a node.
            let node = match service.get("Type").and_then(Value::as_str) {
                Some(node_type) => format!("{name}#{node_type}"),
                None => name,
            };
            assert!(
                seen.insert(node.clone()),
                "case {case:?}: node {node:?} must appear on exactly one page"
            );
            services.push(service.clone());
        }
        pages += 1;
        assert!(
            pages <= 20,
            "case {case:?}: paging must terminate, stopped after {pages} pages"
        );
        match answer.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    services
}

/// Assert every invariant that must hold for any graph: the counts, and that
/// each edge names a node of the same graph.
fn assert_graph_invariants(case: &str, services: &[Value]) {
    let by_reference: BTreeMap<i64, &Value> = services
        .iter()
        .filter_map(|service| Some((service.get("ReferenceId").and_then(Value::as_i64)?, service)))
        .collect();
    assert_eq!(
        by_reference.len(),
        services.len(),
        "case {case:?}: every service must carry a distinct ReferenceId, got {services:?}"
    );
    for service in services {
        let name = service
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_counts(case, &format!("service {name:?}"), service);
        for edge in array(service, "Edges") {
            let destination = edge.get("ReferenceId").and_then(Value::as_i64);
            assert!(
                destination.is_some_and(|id| by_reference.contains_key(&id)),
                "case {case:?}: the edge out of {name:?} must name a node of this graph, got {edge:?}"
            );
            assert_counts(case, &format!("the edge out of {name:?}"), edge);
        }
    }
}

/// One submitted topology, and what the graph of it must contain.
struct ShapeCase {
    case: &'static str,
    documents: Vec<String>,
    /// Every service name the graph must carry.
    services: &'static [&'static str],
    /// Every `(caller, called)` pair the graph must carry.
    edges: &'static [(&'static str, &'static str)],
}

fn shape_cases(base: f64) -> Vec<ShapeCase> {
    let chain = trace_id(201);
    let fanned = trace_id(202);
    let repeated = trace_id(203);
    let partial = trace_id(204);
    let twice_a = trace_id(205);
    let twice_b = trace_id(206);
    let orphan = trace_id(207);
    let cyclic = trace_id(208);
    vec![
        ShapeCase {
            case: "a chain of three services",
            documents: vec![
                root_document("c001000000000001", &chain, "front", base, Some(base + 1.0))
                    .to_string(),
                child_document(
                    "c001000000000002",
                    "c001000000000001",
                    &chain,
                    "middle",
                    base + 0.1,
                    Some(base + 0.8),
                    Some(200),
                )
                .to_string(),
                child_document(
                    "c001000000000003",
                    "c001000000000002",
                    &chain,
                    "back",
                    base + 0.2,
                    Some(base + 0.3),
                    Some(200),
                )
                .to_string(),
            ],
            services: &["back", "front", "middle"],
            edges: &[("front", "middle"), ("middle", "back")],
        },
        ShapeCase {
            case: "a fan-out to a fault and a throttle",
            documents: vec![
                root_document("c002000000000001", &fanned, "gate", base, Some(base + 1.0))
                    .to_string(),
                child_document(
                    "c002000000000002",
                    "c002000000000001",
                    &fanned,
                    "down",
                    base + 0.1,
                    Some(base + 0.2),
                    Some(503),
                )
                .to_string(),
                child_document(
                    "c002000000000003",
                    "c002000000000001",
                    &fanned,
                    "busy",
                    base + 0.3,
                    Some(base + 0.4),
                    Some(429),
                )
                .to_string(),
                child_document(
                    "c002000000000004",
                    "c002000000000001",
                    &fanned,
                    "missing",
                    base + 0.5,
                    Some(base + 0.6),
                    Some(404),
                )
                .to_string(),
            ],
            services: &["busy", "down", "gate", "missing"],
            edges: &[("gate", "down"), ("gate", "busy"), ("gate", "missing")],
        },
        ShapeCase {
            case: "repeated calls to one downstream service",
            documents: vec![
                root_document("c003000000000001", &repeated, "front", base, Some(base + 1.0))
                    .to_string(),
                child_document(
                    "c003000000000002",
                    "c003000000000001",
                    &repeated,
                    "db",
                    base + 0.1,
                    Some(base + 0.2),
                    Some(200),
                )
                .to_string(),
                child_document(
                    "c003000000000003",
                    "c003000000000001",
                    &repeated,
                    "db",
                    base + 0.3,
                    Some(base + 0.5),
                    Some(200),
                )
                .to_string(),
            ],
            services: &["db", "front"],
            edges: &[("front", "db")],
        },
        ShapeCase {
            case: "an in-progress downstream segment",
            documents: vec![
                root_document("c004000000000001", &partial, "worker", base, Some(base + 1.0))
                    .to_string(),
                child_document(
                    "c004000000000002",
                    "c004000000000001",
                    &partial,
                    "queue",
                    base + 0.1,
                    None,
                    None,
                )
                .to_string(),
            ],
            services: &["queue", "worker"],
            edges: &[("worker", "queue")],
        },
        ShapeCase {
            case: "two traces through one service",
            documents: vec![
                root_document("c005000000000001", &twice_a, "api", base, Some(base + 0.1))
                    .to_string(),
                root_document("c005000000000002", &twice_b, "api", base + 1.0, Some(base + 1.3))
                    .to_string(),
            ],
            services: &["api"],
            edges: &[],
        },
        ShapeCase {
            case: "a parent_id naming no segment of the trace",
            documents: vec![child_document(
                "c006000000000001",
                "c006000000000099",
                &orphan,
                "lonely",
                base,
                Some(base + 0.2),
                Some(200),
            )
            .to_string()],
            services: &["lonely"],
            edges: &[],
        },
        ShapeCase {
            case: "two segments naming each other as parent",
            documents: vec![
                child_document(
                    "c007000000000001",
                    "c007000000000002",
                    &cyclic,
                    "loop-a",
                    base,
                    Some(base + 0.4),
                    Some(200),
                )
                .to_string(),
                child_document(
                    "c007000000000002",
                    "c007000000000001",
                    &cyclic,
                    "loop-b",
                    base + 0.1,
                    Some(base + 0.3),
                    Some(200),
                )
                .to_string(),
            ],
            services: &["loop-a", "loop-b"],
            edges: &[("loop-a", "loop-b"), ("loop-b", "loop-a")],
        },
    ]
}

#[tokio::test]
async fn the_graph_of_any_stored_topology_counts_every_request_once_and_names_its_own_nodes() {
    let base = now() - 3_600.0;
    for case in shape_cases(base) {
        let label = case.case;
        let xray = Xray::new();
        let put = xray.put_segments(&case.documents).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {label:?}: every document must be stored, got {put:?}"
        );

        let services = whole_graph(label, &xray, base - 60.0, base + 60.0).await;
        let names: BTreeSet<&str> = services
            .iter()
            .filter_map(|service| service.get("Name").and_then(Value::as_str))
            .collect();
        let expected: BTreeSet<&str> = case.services.iter().copied().collect();
        assert_eq!(
            names, expected,
            "case {label:?}: the graph must carry one node per service, got {services:?}"
        );

        // The edges, read as the pairs of service names they connect.
        let by_reference: BTreeMap<i64, &str> = services
            .iter()
            .filter_map(|service| {
                Some((
                    service.get("ReferenceId").and_then(Value::as_i64)?,
                    service.get("Name").and_then(Value::as_str)?,
                ))
            })
            .collect();
        let mut pairs: BTreeSet<(&str, &str)> = BTreeSet::new();
        // A `client` node's edge reaches the root service of its own name; the
        // case enumerates the edges BETWEEN services, so the two are collected
        // apart and each asserted for what it is.
        let mut client_edges: BTreeSet<(&str, &str)> = BTreeSet::new();
        for service in &services {
            let source = service
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let from_client = service.get("Type").and_then(Value::as_str) == Some("client");
            for edge in array(service, "Edges") {
                let destination = edge
                    .get("ReferenceId")
                    .and_then(Value::as_i64)
                    .and_then(|id| by_reference.get(&id).copied())
                    .unwrap_or("<unknown>");
                if from_client {
                    client_edges.insert((source, destination));
                } else {
                    pairs.insert((source, destination));
                }
            }
        }
        for (source, destination) in &client_edges {
            assert_eq!(
                source, destination,
                "case {label:?}: a client node must reach the root service of its own name, got {services:?}"
            );
        }
        let expected_pairs: BTreeSet<(&str, &str)> = case.edges.iter().copied().collect();
        assert_eq!(
            pairs, expected_pairs,
            "case {label:?}: the graph must carry one edge per stored parent_id link, got {services:?}"
        );

        assert_graph_invariants(label, &services);
    }
}

#[tokio::test]
async fn a_graph_wider_than_a_page_is_answered_page_by_page_with_stable_reference_ids() {
    use service_com_amazonaws_xray_awsxray::ops::get_service_graph::SERVICE_GRAPH_PAGE_SIZE;

    // Rows differ in node count relative to a page: under it, exactly it, and
    // past it.
    for extra in [-1_i64, 0, 3] {
        let downstream = usize::try_from(
            i64::try_from(SERVICE_GRAPH_PAGE_SIZE).unwrap_or_default() - 1 + extra,
        )
        .unwrap_or_default();
        let total = downstream + 1;
        let label = format!("{total} services against a page of {SERVICE_GRAPH_PAGE_SIZE}");
        let base = now() - 1_800.0;
        let xray = Xray::new();
        let trace = trace_id(210);
        let mut documents =
            vec![root_document("d001000000000001", &trace, "hub", base, Some(base + 0.9)).to_string()];
        for at in 0..downstream {
            documents.push(
                child_document(
                    &format!("d002000000000{at:03}"),
                    "d001000000000001",
                    &trace,
                    &format!("leaf-{at:03}"),
                    base + 0.1,
                    Some(base + 0.2),
                    Some(200),
                )
                .to_string(),
            );
        }
        let put = xray.put_segments(&documents).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments must succeed, got {put:?}"
        );

        let services = whole_graph(&label, &xray, base - 60.0, base + 60.0).await;
        assert_eq!(
            services.len(),
            // The hub's `client` node pages beside the services themselves.
            total + 1,
            "case {label:?}: paging must answer every node exactly once, got {services:?}"
        );
        assert_graph_invariants(&label, &services);

        // The hub's edges name every leaf, whichever page that leaf landed on.
        let hub = services
            .iter()
            .find(|service| {
                service.get("Name").and_then(Value::as_str) == Some("hub")
                    && service.get("Type").and_then(Value::as_str) != Some("client")
            })
            .unwrap_or_else(|| panic!("case {label:?}: the hub must be on the map"));
        assert_eq!(
            array(hub, "Edges").len(),
            downstream,
            "case {label:?}: the hub must carry one edge per leaf, got {hub:?}"
        );
    }
}

#[tokio::test]
async fn a_chain_as_deep_as_a_trace_may_be_is_answered() {
    // A deep chain is the shape that makes a per-segment ancestor walk
    // expensive and a recursive one fail; the graph must simply answer it.
    const CHAIN: usize = 200;
    let base = now() - 1_800.0;
    let xray = Xray::new();
    let trace = trace_id(220);
    let mut documents = Vec::new();
    for at in 0..CHAIN {
        #[allow(clippy::cast_precision_loss)]
        let start = base + at as f64 * 0.001;
        let name = if at == 0 { "head" } else { "chain" };
        let mut document = root_document(
            &format!("e001000000000{at:03}"),
            &trace,
            name,
            start,
            Some(start + 0.2),
        );
        if at > 0 {
            document["parent_id"] = json!(format!("e001000000000{:03}", at - 1));
        }
        documents.push(document.to_string());
    }
    let put = xray.put_segments(&documents).await;
    assert_eq!(put.status, 200, "PutTraceSegments must succeed, got {put:?}");
    assert!(
        array(&put.body, "UnprocessedTraceSegments").is_empty(),
        "every document of the chain must be stored, got {put:?}"
    );

    let services = whole_graph("a 200-deep chain", &xray, base - 60.0, base + 60.0).await;
    let names: BTreeSet<&str> = services
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["chain", "head"]),
        "the chain must be two nodes, got {services:?}"
    );
    assert_graph_invariants("a 200-deep chain", &services);
    let chain = services
        .iter()
        .find(|service| service.get("Name").and_then(Value::as_str) == Some("chain"))
        .expect("the chain node is on the map");
    let counts = counts_of(&chain.get("SummaryStatistics").cloned().unwrap_or(Value::Null));
    assert_eq!(
        counts.total,
        i64::try_from(CHAIN - 1).unwrap_or_default(),
        "every segment of the chain is one completed request, got {counts:?}"
    );
}

#[tokio::test]
async fn an_instant_no_timestamp_can_carry_is_refused_at_ingest_and_the_far_edge_is_served() {
    use service_com_amazonaws_xray_awsxray::state::MAX_SEGMENT_EPOCH_SECONDS;

    // Rows differ in the magnitude of the recorded instant: at the far edge of
    // what a `Timestamp` can carry (stored and served), and past it in each
    // direction (refused as one `UnprocessedTraceSegment`, so nothing a reader
    // cannot convert is ever stored).
    let cases: Vec<(&str, f64, bool)> = vec![
        ("an instant at the far edge of the accepted range", MAX_SEGMENT_EPOCH_SECONDS - 1.0, true),
        ("an instant just past the accepted range", MAX_SEGMENT_EPOCH_SECONDS * 1.1, false),
        ("an instant far past the accepted range", 1e300, false),
        ("an instant far before the epoch", -1e300, false),
    ];

    for (at, (label, start, accepted)) in cases.iter().enumerate() {
        let xray = Xray::new();
        let trace = trace_id(230 + u32::try_from(at).unwrap_or_default());
        let document = root_document(
            &format!("f00100000000000{at}"),
            &trace,
            "edge-of-time",
            *start,
            Some(start + 0.5),
        )
        .to_string();
        let put = xray.put_segments(&[document]).await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTraceSegments must answer, got {put:?}"
        );
        let unprocessed = array(&put.body, "UnprocessedTraceSegments");
        if !accepted {
            assert_eq!(
                unprocessed.len(),
                1,
                "case {label:?}: the document must be reported unprocessed, got {put:?}"
            );
            assert_eq!(
                unprocessed[0].get("ErrorCode").and_then(Value::as_str),
                Some("InvalidStartTime"),
                "case {label:?}: the rejection's code must name the member at fault, got {put:?}"
            );
            continue;
        }

        assert!(
            unprocessed.is_empty(),
            "case {label:?}: an instant inside the range must be stored, got {put:?}"
        );
        // And a read of it answers rather than failing on the conversion.
        // `BatchGetTraces` is the reader that converts a stored instant with no
        // window of its own — the trace-query reads publish how far ahead of the
        // clock their window may reach — so it is the one that can be asked
        // about an instant this far out.
        let answer = xray.batch_get(std::slice::from_ref(&trace), None).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a read of a stored extreme instant must be served, got {answer:?}"
        );
        let traces = array(&answer.body, "Traces");
        assert_eq!(
            traces.len(),
            1,
            "case {label:?}: the stored trace must be reported, got {answer:?}"
        );
        let segments = array(&traces[0], "Segments");
        let reported = segments
            .first()
            .and_then(|segment| segment.get("Document"))
            .and_then(Value::as_str)
            .and_then(|document| serde_json::from_str::<Value>(document).ok())
            .and_then(|document| document.get("start_time").and_then(Value::as_f64));
        assert!(
            reported.is_some_and(|value| close(value, *start)),
            "case {label:?}: the served segment must report the stored instant {start}, got \
             {traces:?}"
        );
    }
}
