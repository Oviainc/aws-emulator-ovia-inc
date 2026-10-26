// SPDX-License-Identifier: CC0-1.0

//! `GetInsightImpactGraph`: the structural service graph a STORED insight
//! answers, and the refusal an `InsightId` naming none gets — end to end over
//! the wire.
//!
//! Every case drives the same front door the production router does (the
//! harness in `xray_wire`), so the assertions are about HTTP responses rather
//! than internal state. Nothing in the X-Ray API creates an insight — X-Ray
//! mints one itself from anomaly detection — so the cases that need one seed the
//! insight store the way the `GetInsight` cases do, and then read it back only
//! through the wire.
//!
//! The tests are tables, and each row differs along what the rule it checks
//! generalises over — the shape of the id, the topology of the stored traffic,
//! the position of the window against the stored segments, what the insight's
//! group selects, the width of the window, the node count relative to a page —
//! because the rules are:
//!
//! * an id that satisfies the modelled `@pattern` but names no stored insight is
//!   refused with `InvalidRequestException` at HTTP 400 and the service's one
//!   fixed "Insight not found" text, never with a 501 or any other code;
//! * a resolved insight answers a graph DERIVED from the stored segment
//!   documents, limited to structural information: a node carries its identity
//!   and the downstream nodes it reaches, and no count, time or state;
//! * the window is half-open — `StartTime` inclusive, `EndTime` exclusive — and
//!   `ServiceGraphStartTime` / `ServiceGraphEndTime` report the interval the
//!   derived graph covers;
//! * the graph is narrowed by the `FilterExpression` of the group the insight
//!   belongs to, and by nothing else;
//! * a window wider than the published maximum, and a `NextToken` this service
//!   never issued, are refused rather than answered;
//! * the operation is paginated: a `NextToken` appears only when the graph was
//!   truncated, resuming from it yields the remainder exactly once, and an
//!   edge's `ReferenceId` names a node of the same graph on whatever page that
//!   node lands.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_insight::INSIGHT_NOT_FOUND;
use service_com_amazonaws_xray_awsxray::ops::get_insight_impact_graph::{
    IMPACT_GRAPH_WINDOW_TOO_WIDE, MAX_IMPACT_GRAPH_WINDOW_SECONDS,
};
use service_com_amazonaws_xray_awsxray::ops::get_service_graph::SERVICE_GRAPH_PAGE_SIZE;
use service_com_amazonaws_xray_awsxray::ops::types::Insight;
use service_com_amazonaws_xray_awsxray::state::ComAmazonawsXrayAwsxrayState;

use crate::trace_ingest_and_groups::{array, close, root_document, trace_id, Answer, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as, truncated
/// to a whole second so a window's width is exactly the width a case states.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds().floor()
}

/// The members a `Service` of `GetServiceGraph` carries that an
/// `InsightImpactGraphService` does NOT: "the service graph is limited to only
/// structural information" (`smithy_query(query="doc",shape="GetInsightImpactGraph")`),
/// so none of them may appear on a node of this answer.
const NON_STRUCTURAL_MEMBERS: [&str; 7] = [
    "SummaryStatistics",
    "StartTime",
    "EndTime",
    "State",
    "Root",
    "DurationHistogram",
    "ResponseTimeHistogram",
];

/// Seed one `ACTIVE` insight into the store no operation in the API writes,
/// belonging to the group `group` names.
fn seed_insight(engine: &Arc<engine::InMemoryEngine>, id: &str, group: Option<&str>) {
    let state = engine.state::<ComAmazonawsXrayAwsxrayState>();
    state.lock().insights.insert(
        id.to_string(),
        Insight {
            insight_id: Some(id.to_string()),
            group_name: group.map(str::to_string),
            state: Some("ACTIVE".to_string()),
            summary: Some("Fault rate above the expected range".to_string()),
            ..Insight::default()
        },
    );
}

/// `GetInsightImpactGraph` over `[start, end)`, optionally resumed from a token.
async fn impact_graph(
    xray: &Xray,
    id: &str,
    start: f64,
    end: f64,
    token: Option<&str>,
) -> Answer {
    let mut body = json!({"InsightId": id, "StartTime": start, "EndTime": end});
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    xray.call("/InsightImpactGraph", &body).await
}

/// What identifies a node of the graph: its `Name` and its `Type`, because a
/// root service's `client` node carries that service's own name.
fn label(service: &Value) -> String {
    let name = service.get("Name").and_then(Value::as_str).unwrap_or_default();
    let kind = service.get("Type").and_then(Value::as_str).unwrap_or_default();
    format!("{name}|{kind}")
}

/// Every node of one answer, keyed by the `ReferenceId` it reported.
fn node_labels(answer: &Answer) -> BTreeMap<i64, String> {
    array(&answer.body, "Services")
        .iter()
        .map(|service| {
            (
                service
                    .get("ReferenceId")
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
                label(service),
            )
        })
        .collect()
}

/// Every edge of one answer as (upstream node, downstream node), the downstream
/// end resolved through `labels` so an edge naming no node of the graph is
/// visible in the assertion rather than silently dropped.
fn edge_pairs(answer: &Answer, labels: &BTreeMap<i64, String>) -> BTreeSet<(String, String)> {
    let mut pairs = BTreeSet::new();
    for service in array(&answer.body, "Services") {
        for edge in array(service, "Edges") {
            let downstream = edge.get("ReferenceId").and_then(Value::as_i64).unwrap_or(-1);
            pairs.insert((
                label(service),
                labels
                    .get(&downstream)
                    .cloned()
                    .unwrap_or_else(|| format!("no node carries reference {downstream}")),
            ));
        }
    }
    pairs
}

/// The service names one answer reports, the `client` nodes' duplicates folded
/// away.
fn names(answer: &Answer) -> BTreeSet<String> {
    array(&answer.body, "Services")
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// One timestamp member of an answer, as the epoch seconds it travels as.
fn instant(answer: &Answer, member: &str) -> Option<f64> {
    answer.get(member).as_f64()
}

/// A segment document with a parent, which is how one service's call of another
/// is recorded.
fn child_document(id: &str, parent: &str, trace: &str, name: &str, start: f64, end: f64) -> Value {
    let mut document = root_document(id, trace, name, start, Some(end));
    document["parent_id"] = json!(parent);
    document
}

// =============================================================================
// 1. A well-formed id naming no stored insight is refused as invalid
// =============================================================================

#[tokio::test]
async fn a_well_formed_id_naming_no_stored_insight_is_refused_as_invalid() {
    assert_eq!(
        INSIGHT_NOT_FOUND, "Insight not found",
        "the refusal wording must stay the fixed string, got {INSIGHT_NOT_FOUND:?}"
    );

    let (xray, engine) = Xray::with_engine();
    let stored = "11111111-2222-4333-8444-555555555555";
    seed_insight(&engine, stored, None);

    // Rows differ in the shape of the id WITHIN what the modelled @pattern
    // admits: the version nibble (1..5), the variant nibble (8, 9, a, b), the
    // hex casing, and the extremes of the value space.
    let unknown = [
        ("the lowercase form the SDK prints", "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
        ("version 1, variant 9", "00000000-0000-1000-9000-000000000000"),
        ("version 5, variant b, uppercase", "FFFFFFFF-FFFF-5FFF-BFFF-FFFFFFFFFFFF"),
        ("version 3, variant a", "12345678-9abc-3def-a012-3456789abcde"),
        ("version 2, variant B", "0a0a0a0a-1b1b-2c2c-B3b3-4d4d4d4d4d4d"),
    ];
    // Rows differ in the window the unknown id is asked about, so the refusal is
    // about the id rather than about one interval.
    let windows = [
        ("the last hour", now() - 3_600.0, now()),
        ("a one-second window", now() - 1.0, now()),
        ("a window well in the past", now() - 100_000.0, now() - 99_000.0),
    ];

    for (id_case, id) in unknown {
        for (window_case, start, end) in windows {
            let refused = impact_graph(&xray, id, start, end, None).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {id_case:?} over {window_case:?}: an id naming no insight must be refused \
                 400 InvalidRequestException, got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                Some(INSIGHT_NOT_FOUND),
                "case {id_case:?} over {window_case:?}: the refusal must say {INSIGHT_NOT_FOUND:?}, \
                 got {refused:?}"
            );
        }
    }

    // The boundary: the one id the store does hold is answered, so the refusals
    // above are the lookup's and not the operation's.
    let answered = impact_graph(&xray, stored, now() - 3_600.0, now(), None).await;
    assert_eq!(
        answered.status, 200,
        "a stored insight must be answered, got {answered:?}"
    );
    assert_eq!(
        answered.get("InsightId").as_str(),
        Some(stored),
        "the answer must name the stored insight, got {answered:?}"
    );
}

// =============================================================================
// 2. A resolved insight answers the structural graph of the stored segments
// =============================================================================

/// One stored-traffic fixture: the documents to ingest, and the structure the
/// graph derived from them must report.
struct Fixture {
    case: &'static str,
    documents: Vec<String>,
    /// Every node, as `label` renders it.
    nodes: Vec<&'static str>,
    /// Every edge, as (upstream node, downstream node).
    edges: Vec<(&'static str, &'static str)>,
    /// The `AccountId` a node must report, where a document named one.
    accounts: Vec<(&'static str, &'static str)>,
    /// The interval the stored segments cover, which is the interval the graph
    /// must report.
    graph: (f64, f64),
}

fn fixtures(base: f64) -> Vec<Fixture> {
    // Rows differ in topology (a lone service, a call chain, a fan-out) and in
    // what the documents state about the reporting service (an `origin`, an
    // `account_id`), so no single shape of graph satisfies them all.
    let one = trace_id(1);
    let two = trace_id(2);
    let three = trace_id(3);
    vec![
        Fixture {
            case: "a single root segment",
            documents: vec![
                root_document("a100000000000001", &one, "svc", base, Some(base + 1.0)).to_string(),
            ],
            nodes: vec!["svc|", "svc|client"],
            edges: vec![("svc|client", "svc|")],
            accounts: vec![],
            graph: (base, base + 1.0),
        },
        Fixture {
            case: "a root and the service it called",
            documents: vec![
                root_document("a200000000000001", &two, "frontend", base, Some(base + 2.0))
                    .to_string(),
                child_document(
                    "a200000000000002",
                    "a200000000000001",
                    &two,
                    "backend",
                    base + 0.5,
                    base + 1.5,
                )
                .to_string(),
            ],
            nodes: vec!["backend|", "frontend|", "frontend|client"],
            edges: vec![
                ("frontend|client", "frontend|"),
                ("frontend|", "backend|"),
            ],
            accounts: vec![],
            graph: (base, base + 2.0),
        },
        Fixture {
            case: "a typed, owned root fanning out to two services",
            documents: vec![
                {
                    let mut root =
                        root_document("a300000000000001", &three, "gateway", base, Some(base + 3.0));
                    root["origin"] = json!("AWS::EC2::Instance");
                    root["account_id"] = json!("111122223333");
                    root.to_string()
                },
                child_document(
                    "a300000000000002",
                    "a300000000000001",
                    &three,
                    "db",
                    base + 0.2,
                    base + 0.8,
                )
                .to_string(),
                child_document(
                    "a300000000000003",
                    "a300000000000001",
                    &three,
                    "cache",
                    base + 0.3,
                    base + 1.0,
                )
                .to_string(),
            ],
            nodes: vec![
                "cache|",
                "db|",
                "gateway|AWS::EC2::Instance",
                "gateway|client",
            ],
            edges: vec![
                ("gateway|client", "gateway|AWS::EC2::Instance"),
                ("gateway|AWS::EC2::Instance", "db|"),
                ("gateway|AWS::EC2::Instance", "cache|"),
            ],
            accounts: vec![("gateway|AWS::EC2::Instance", "111122223333")],
            graph: (base, base + 3.0),
        },
    ]
}

#[tokio::test]
async fn a_resolved_insight_answers_the_structural_graph_of_the_stored_segments() {
    let base = now() - 600.0;
    let insight_id = "22222222-3333-4444-8555-666666666666";

    for fixture in fixtures(base) {
        let case = fixture.case;
        let (xray, engine) = Xray::with_engine();
        seed_insight(&engine, insight_id, None);
        let ingested = xray.put_segments(&fixture.documents).await;
        assert_eq!(
            ingested.status, 200,
            "case {case:?}: the fixture must be ingested, got {ingested:?}"
        );

        let (start, end) = (base - 10.0, base + 60.0);
        let answered = impact_graph(&xray, insight_id, start, end, None).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: a resolved insight must be answered, got {answered:?}"
        );
        assert_eq!(
            answered.get("InsightId").as_str(),
            Some(insight_id),
            "case {case:?}: the answer must name the stored insight, got {answered:?}"
        );
        for (member, expected) in [("StartTime", start), ("EndTime", end)] {
            let actual = instant(&answered, member);
            assert!(
                actual.is_some_and(|actual| close(actual, expected)),
                "case {case:?}: {member} must be the provided bound {expected}, got {actual:?}"
            );
        }

        let labels = node_labels(&answered);
        let mut reported: Vec<&str> = labels.values().map(String::as_str).collect();
        reported.sort_unstable();
        let mut expected_nodes = fixture.nodes.clone();
        expected_nodes.sort_unstable();
        assert_eq!(
            reported, expected_nodes,
            "case {case:?}: the graph must report the stored services, got {answered:?}"
        );

        let edges = edge_pairs(&answered, &labels);
        let expected_edges: BTreeSet<(String, String)> = fixture
            .edges
            .iter()
            .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
            .collect();
        assert_eq!(
            edges, expected_edges,
            "case {case:?}: the graph must report the stored calls, got {answered:?}"
        );

        for (node, account) in &fixture.accounts {
            let actual = array(&answered.body, "Services")
                .iter()
                .find(|service| label(service) == *node)
                .and_then(|service| service.get("AccountId"))
                .and_then(Value::as_str);
            assert_eq!(
                actual,
                Some(*account),
                "case {case:?}: node {node:?} must report AccountId {account:?}, got {actual:?}"
            );
        }

        // Structural only: a node names itself and what it reaches, and reports
        // nothing a counter would have had to compute.
        for service in array(&answered.body, "Services") {
            for member in NON_STRUCTURAL_MEMBERS {
                assert!(
                    service.get(member).is_none(),
                    "case {case:?}: a node must carry no {member}, got {service:?}"
                );
            }
            for edge in array(service, "Edges") {
                let members: Vec<&String> = edge
                    .as_object()
                    .map(|edge| edge.keys().collect())
                    .unwrap_or_default();
                assert_eq!(
                    members,
                    vec!["ReferenceId"],
                    "case {case:?}: an edge must carry only its ReferenceId, got {edge:?}"
                );
            }
        }

        // The interval the DERIVED graph covers, not the window asked for.
        for (member, expected) in [
            ("ServiceGraphStartTime", fixture.graph.0),
            ("ServiceGraphEndTime", fixture.graph.1),
        ] {
            let actual = instant(&answered, member);
            assert!(
                actual.is_some_and(|actual| close(actual, expected)),
                "case {case:?}: {member} must be the stored interval's {expected}, got {actual:?}"
            );
        }

        // The id this answer names is an id `GetInsight` resolves: one insight,
        // read by both readers.
        let read = xray
            .call("/Insight", &json!({"InsightId": insight_id}))
            .await;
        assert_eq!(
            (
                read.status,
                read.get("Insight")
                    .get("InsightId")
                    .and_then(Value::as_str),
            ),
            (200, Some(insight_id)),
            "case {case:?}: the answered id must resolve through GetInsight, got {read:?}"
        );
    }
}

// =============================================================================
// 3. An empty window and the half-open selection of segments
// =============================================================================

#[tokio::test]
async fn the_window_selects_the_segments_half_open() {
    let base = now() - 3_000.0;
    let insight_id = "33333333-4444-4555-9666-777777777777";
    let (xray, engine) = Xray::with_engine();
    seed_insight(&engine, insight_id, None);

    for (at, name, n) in [
        (base, "early", 11_u32),
        (base + 10.0, "middle", 12),
        (base + 20.0, "late", 13),
    ] {
        let trace = trace_id(n);
        let document = root_document(&format!("b{n:015x}"), &trace, name, at, Some(at + 1.0));
        let ingested = xray.put_segments(&[document.to_string()]).await;
        assert_eq!(
            ingested.status, 200,
            "the {name:?} segment must be ingested, got {ingested:?}"
        );
    }

    // Rows differ in where the bounds fall against the stored segment starts:
    // `StartTime` is "inclusive of the value provided" and `EndTime` "exclusive
    // of the value provided".
    let cases: [(&str, f64, f64, &[&str]); 5] = [
        ("the whole interval", base - 1.0, base + 21.0, &["early", "late", "middle"]),
        ("a start exactly on a segment", base + 10.0, base + 21.0, &["late", "middle"]),
        ("an end exactly on a segment", base - 1.0, base + 20.0, &["early", "middle"]),
        ("an interval between two segments", base + 11.0, base + 19.0, &[]),
        ("an interval before every segment", base - 100.0, base - 50.0, &[]),
    ];
    for (case, start, end, expected) in cases {
        let answered = impact_graph(&xray, insight_id, start, end, None).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: a window naming an interval must be answered, got {answered:?}"
        );
        let reported = names(&answered);
        let wanted: BTreeSet<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the services {wanted:?}, got {answered:?}"
        );
        // A window over which nothing is stored derives no node, and so reports
        // no interval rather than an invented one.
        if expected.is_empty() {
            assert_eq!(
                (
                    instant(&answered, "ServiceGraphStartTime"),
                    instant(&answered, "ServiceGraphEndTime"),
                ),
                (None, None),
                "case {case:?}: an empty graph must report no interval, got {answered:?}"
            );
        }
    }
}

// =============================================================================
// 4. The insight's group is what narrows the graph
// =============================================================================

#[tokio::test]
async fn the_group_the_insight_belongs_to_narrows_the_graph() {
    let base = now() - 1_200.0;
    let (xray, engine) = Xray::with_engine();

    for (name, expression) in [("watched", "service(\"frontend\")"), ("faulty", "fault = true")] {
        let created = xray
            .call(
                "/CreateGroup",
                &json!({"GroupName": name, "FilterExpression": expression}),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "creating group {name:?} must succeed, got {created:?}"
        );
    }

    // Two traces: one through `frontend` into `backend` that answered, and one
    // through `lonely` that faulted — so each expression below selects a
    // different subset of the stored traffic.
    let answered_trace = trace_id(21);
    let faulting_trace = trace_id(22);
    let mut faulting = root_document("c000000000000001", &faulting_trace, "lonely", base, Some(base + 1.0));
    faulting["http"] = json!({"response": {"status": 503}});
    let ingested = xray
        .put_segments(&[
            root_document("c100000000000001", &answered_trace, "frontend", base, Some(base + 2.0))
                .to_string(),
            child_document(
                "c100000000000002",
                "c100000000000001",
                &answered_trace,
                "backend",
                base + 0.5,
                base + 1.5,
            )
            .to_string(),
            faulting.to_string(),
        ])
        .await;
    assert_eq!(
        ingested.status, 200,
        "the fixture must be ingested, got {ingested:?}"
    );

    // Rows differ in what the insight's group selects: everything (the reserved
    // account-level group, which carries no expression), one service, one
    // outcome, no group at all, and a group that is not stored.
    let cases: [(&str, &str, Option<&str>, &[&str]); 5] = [
        (
            "an insight that names no group",
            "44444444-5555-4666-8777-888888888881",
            None,
            &["backend", "frontend", "lonely"],
        ),
        (
            "an insight in the reserved Default group",
            "44444444-5555-4666-8777-888888888882",
            Some("Default"),
            &["backend", "frontend", "lonely"],
        ),
        (
            "a group scoped to one service",
            "44444444-5555-4666-8777-888888888883",
            Some("watched"),
            &["backend", "frontend"],
        ),
        (
            "a group scoped to faulting traces",
            "44444444-5555-4666-8777-888888888884",
            Some("faulty"),
            &["lonely"],
        ),
        (
            "a group that is not stored any more",
            "44444444-5555-4666-8777-888888888885",
            Some("deleted-since"),
            &["backend", "frontend", "lonely"],
        ),
    ];

    for (case, insight_id, group, expected) in cases {
        seed_insight(&engine, insight_id, group);
        let answered = impact_graph(&xray, insight_id, base - 10.0, base + 60.0, None).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: the insight must be answered, got {answered:?}"
        );
        let reported = names(&answered);
        let wanted: BTreeSet<String> = expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the services {wanted:?}, got {answered:?}"
        );
    }
}

// =============================================================================
// 5. A window wider than the published maximum is refused
// =============================================================================

#[tokio::test]
async fn a_window_wider_than_the_published_maximum_is_refused() {
    // "The time range between the start time and end time can't be more than
    // six hours" (`smithy_query(query="doc",shape="GetInsightImpactGraphRequest",member="EndTime")`).
    assert!(
        close(MAX_IMPACT_GRAPH_WINDOW_SECONDS, 6.0 * 60.0 * 60.0),
        "the published maximum must be six hours, got {MAX_IMPACT_GRAPH_WINDOW_SECONDS}"
    );

    let (xray, engine) = Xray::with_engine();
    let insight_id = "55555555-6666-4777-8888-999999999999";
    seed_insight(&engine, insight_id, None);
    let end = now();

    // Rows walk the bound: well inside it, exactly on it, one second past it,
    // and far past it.
    for (case, width, accepted) in [
        ("a one-second window", 1.0, true),
        ("an hour", 3_600.0, true),
        ("exactly the published bound", MAX_IMPACT_GRAPH_WINDOW_SECONDS, true),
        ("one second past the bound", MAX_IMPACT_GRAPH_WINDOW_SECONDS + 1.0, false),
        ("a week", 7.0 * 86_400.0, false),
    ] {
        let answer = impact_graph(&xray, insight_id, end - width, end, None).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a window inside the bound must be answered, got {answer:?}"
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?}: a window past the bound must be refused, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                Some(IMPACT_GRAPH_WINDOW_TOO_WIDE),
                "case {case:?}: the refusal must state the rule, got {answer:?}"
            );
        }
    }
}

// =============================================================================
// 6. Paging answers every node exactly once, and an unissued token is refused
// =============================================================================

#[tokio::test]
async fn paging_answers_every_node_exactly_once() {
    let base = now() - 900.0;
    let insight_id = "66666666-7777-4888-8999-aaaaaaaaaaaa";
    let (xray, engine) = Xray::with_engine();
    seed_insight(&engine, insight_id, None);

    // Enough distinct services that the graph outgrows one page: each root
    // segment adds its own node plus the `client` node that called it.
    let services = SERVICE_GRAPH_PAGE_SIZE + 1;
    for n in 0..services {
        let trace = trace_id(u32::try_from(n).unwrap_or_default() + 31);
        let document = root_document(
            &format!("d{n:015x}"),
            &trace,
            &format!("svc-{n}"),
            base + n as f64,
            Some(base + n as f64 + 0.5),
        );
        let ingested = xray.put_segments(&[document.to_string()]).await;
        assert_eq!(
            ingested.status, 200,
            "segment {n} must be ingested, got {ingested:?}"
        );
    }

    let mut seen: BTreeMap<i64, String> = BTreeMap::new();
    let mut edges: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let answered = impact_graph(&xray, insight_id, base - 1.0, base + 600.0, token.as_deref()).await;
        assert_eq!(
            answered.status, 200,
            "page {pages}: the insight must be answered, got {answered:?}"
        );
        let page = node_labels(&answered);
        assert!(
            page.len() <= SERVICE_GRAPH_PAGE_SIZE,
            "page {pages}: a page carries at most {SERVICE_GRAPH_PAGE_SIZE} nodes, got {answered:?}"
        );
        for (reference, node) in page {
            assert!(
                seen.insert(reference, node.clone()).is_none(),
                "page {pages}: node {node:?} was answered twice, got {answered:?}"
            );
        }
        for service in array(&answered.body, "Services") {
            let upstream = service
                .get("ReferenceId")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            for edge in array(service, "Edges") {
                edges.insert((
                    upstream,
                    edge.get("ReferenceId").and_then(Value::as_i64).unwrap_or(-1),
                ));
            }
        }
        pages += 1;
        token = answered
            .get("NextToken")
            .as_str()
            .map(str::to_string);
        assert!(
            pages <= services * 2 + 1,
            "paging must terminate; it has answered {pages} pages"
        );
        if token.is_none() {
            break;
        }
    }

    assert!(
        pages > 1,
        "a graph of {} nodes must take more than one page of {SERVICE_GRAPH_PAGE_SIZE}",
        seen.len()
    );
    assert_eq!(
        seen.len(),
        services * 2,
        "every reported service and its client node must be answered exactly once, got {seen:?}"
    );
    for (upstream, downstream) in &edges {
        assert!(
            seen.contains_key(downstream),
            "the edge {upstream} -> {downstream} must name a node of the same graph, got {seen:?}"
        );
    }

    // A token this service never issued is refused rather than answered from
    // the start of the graph.
    for (case, bad) in [
        ("prose", "not-a-reference"),
        ("a reference past the graph", "9999"),
    ] {
        let refused = impact_graph(&xray, insight_id, base - 1.0, base + 600.0, Some(bad)).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: a token this service never issued must be refused, got {refused:?}"
        );
    }
}
