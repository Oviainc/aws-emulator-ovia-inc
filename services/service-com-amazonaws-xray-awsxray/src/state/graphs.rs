// SPDX-License-Identifier: CC0-1.0

//! Service graphs: the ONE derivation of the `Service` list both graph readers
//! answer from — `GetServiceGraph` over a time window and `GetTraceGraph` over
//! named traces — plus the node and edge accumulators it builds and the page it
//! is cut into.
//!
//! One concern, one file: every node, edge, count, time and histogram here is
//! recomputed from the stored segment documents, so the two readers can never
//! describe the same stored segments differently.

use std::collections::BTreeMap;

use super::{
    classify, inferred_service_type, timestamp_of, Classification, SegmentRecord, SubsegmentRecord,
    TraceRecord, MAX_HISTOGRAM_ENTRIES,
};
use crate::ops::types::{
    Edge, EdgeStatistics, ErrorStatistics, FaultStatistics, HistogramEntry, Service,
    ServiceStatistics,
};

/// `Service$State` for a node that has at least one completed segment in the
/// graph's window: the service answered.
pub const SERVICE_STATE_ACTIVE: &str = "active";

/// `Service$State` for a node whose every segment in the graph's window is still
/// in progress: the service reported, but nothing is known yet about how it
/// answered.
pub const SERVICE_STATE_UNKNOWN: &str = "unknown";

/// `Service$State` for a node the graph knows only through another service's
/// downstream subsegment — a service that submitted no segment of its own, so
/// X-Ray knows it passively, by being called.
pub const SERVICE_STATE_PASSIVE: &str = "passive";

/// `Service$Type` for the node that "represents the clients that sent requests
/// to a root service".
///
/// The clients report no segment of their own, so the node carries the root
/// service's name and the requests that service received.
pub const SERVICE_TYPE_CLIENT: &str = "client";

/// `Service$Type` for "a downstream service of indeterminate type", which is
/// what a `remote`-namespace subsegment records.
pub const SERVICE_TYPE_REMOTE: &str = "remote";

/// The most inline subsegments one derivation walks, across every trace and
/// segment it covers.
///
/// A trace holds at most [`super::MAX_SEGMENTS_PER_TRACE`] segments and a
/// segment at most [`super::MAX_SUBSEGMENTS_PER_SEGMENT`] subsegments, so
/// capping each factor still leaves their product to grow; this caps the
/// product. Past it a graph reports the services its segments named, which is
/// the whole topology a trace whose services are instrumented has.
pub const MAX_GRAPH_SUBSEGMENT_VISITS: usize = 50_000;

/// One recorded call, as a graph counts it: a segment the service reported, or
/// a subsegment it recorded inside that segment. The two carry the same timing
/// and the same outcome flags, so the accumulators below take this rather than
/// one shape of document.
#[derive(Debug, Clone, Copy)]
struct Call {
    start_time: f64,
    end_time: Option<f64>,
    error: bool,
    fault: bool,
    throttle: bool,
}

impl Call {
    /// The call a stored segment records.
    fn of_segment(segment: &SegmentRecord) -> Self {
        Self {
            start_time: segment.start_time,
            end_time: segment.end_time,
            error: segment.is_error(),
            fault: segment.is_fault(),
            throttle: segment.is_throttle(),
        }
    }

    /// The call an inline subsegment records. A subsegment that carries no
    /// `start_time` is timed from its segment, which is the only instant known
    /// for it.
    fn of_subsegment(subsegment: &SubsegmentRecord, segment: &SegmentRecord) -> Self {
        Self {
            start_time: subsegment.start_time.unwrap_or(segment.start_time),
            end_time: subsegment.end_time,
            error: subsegment.is_error(),
            fault: subsegment.is_fault(),
            throttle: subsegment.is_throttle(),
        }
    }

    /// How long the call took, in seconds; `None` while it is in progress.
    fn duration(&self) -> Option<f64> {
        self.end_time.map(|end| end - self.start_time)
    }

    /// The latest instant this call is known to have reached.
    fn latest(&self) -> f64 {
        self.end_time.unwrap_or(self.start_time)
    }
}

/// The time span a node or an edge covers, widened call by call.
#[derive(Debug, Default)]
struct Span {
    start: Option<f64>,
    end: Option<f64>,
}

impl Span {
    /// Widen this span to include one call.
    fn widen(&mut self, call: Call) {
        self.start = Some(
            self.start
                .map_or(call.start_time, |start| start.min(call.start_time)),
        );
        self.end = Some(self.end.map_or(call.latest(), |end| end.max(call.latest())));
    }

    /// The span's start as a modelled timestamp.
    fn start_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.start.map(timestamp_of)
    }

    /// The span's end as a modelled timestamp.
    fn end_time(&self) -> Option<::aws_smithy_types::DateTime> {
        self.end.map(timestamp_of)
    }
}

/// The counts `ServiceStatistics` and `EdgeStatistics` both carry, accumulated
/// over the segments a node or an edge covers.
#[derive(Debug, Default)]
struct CallStatistics {
    ok_count: i64,
    throttle_count: i64,
    error_other_count: i64,
    fault_count: i64,
    total_count: i64,
    total_response_time: f64,
}

impl CallStatistics {
    /// Count one call. A fault outranks an error (a 500-series answer is not
    /// also a 400-series one), a throttled error is counted as a throttle rather
    /// than as "other", and a call that recorded neither is OK — which is
    /// exactly the exclusive [`Classification`] every other reader of this
    /// state uses, read here rather than recomputed.
    fn record(&mut self, call: Call) {
        self.total_count += 1;
        // The call's flags are the ones its document declared
        // ([`Call::of_segment`] / [`Call::of_subsegment`] read them through
        // `is_fault`/`is_throttle`/`is_error`), and a recorded status is not one
        // of them, so nothing here consults it.
        match classify(call.fault, call.error, call.throttle) {
            Classification::Fault => self.fault_count += 1,
            Classification::Throttle => self.throttle_count += 1,
            Classification::Error => self.error_other_count += 1,
            Classification::Ok => self.ok_count += 1,
        }
        self.total_response_time += call.duration().unwrap_or(0.0);
    }

    /// The 400-series counts.
    fn error_statistics(&self) -> ErrorStatistics {
        ErrorStatistics {
            throttle_count: Some(self.throttle_count),
            other_count: Some(self.error_other_count),
            total_count: Some(self.throttle_count + self.error_other_count),
        }
    }

    /// The 500-series counts.
    fn fault_statistics(&self) -> FaultStatistics {
        FaultStatistics {
            other_count: Some(self.fault_count),
            total_count: Some(self.fault_count),
        }
    }

    /// These counts as a node's `SummaryStatistics`.
    fn service_statistics(&self) -> ServiceStatistics {
        ServiceStatistics {
            ok_count: Some(self.ok_count),
            error_statistics: Some(self.error_statistics()),
            fault_statistics: Some(self.fault_statistics()),
            total_count: Some(self.total_count),
            total_response_time: Some(self.total_response_time),
        }
    }

    /// These counts as an edge's `SummaryStatistics`.
    fn edge_statistics(&self) -> EdgeStatistics {
        EdgeStatistics {
            ok_count: Some(self.ok_count),
            error_statistics: Some(self.error_statistics()),
            fault_statistics: Some(self.fault_statistics()),
            total_count: Some(self.total_count),
            total_response_time: Some(self.total_response_time),
        }
    }
}

/// One connection from a node to a downstream node, under construction.
#[derive(Debug, Default)]
struct EdgeAccumulator {
    span: Span,
    statistics: CallStatistics,
    response_times: Vec<f64>,
}

impl EdgeAccumulator {
    /// Count one downstream call on this edge.
    fn record(&mut self, call: Call) {
        self.span.widen(call);
        self.statistics.record(call);
        if let Some(duration) = call.duration() {
            self.response_times.push(duration);
        }
    }

    /// This edge as the modelled `Edge`, naming the downstream node by its
    /// `ReferenceId`.
    fn to_edge(&self, downstream: i32) -> Edge {
        Edge {
            reference_id: Some(downstream),
            start_time: self.span.start_time(),
            end_time: self.span.end_time(),
            summary_statistics: Some(self.statistics.edge_statistics()),
            response_time_histogram: histogram(&self.response_times),
            aliases: None,
            // `link` describes an asynchronous connection, which a stored
            // segment document does not say it is; an edge derived from a
            // parent/child pair is the synchronous default.
            edge_type: None,
            received_event_age_histogram: None,
        }
    }
}

/// What distinguishes one node of a service graph from another.
///
/// A node the stored segments REPORTED is the service of that `name`. Beside it
/// a graph carries the node `Service$Type` documents as `client` — "represents
/// the clients that sent requests to a root service" — which carries the root
/// service's own name, so the name alone does not identify a node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NodeKey {
    /// The canonical service name, which is what `Service$Name` reports.
    name: String,
    /// `Service$Type` for a node no segment reported — `client`, `remote`, or
    /// `AWS::<Service>` — and `None` for one a segment did report, whose type is
    /// that segment's `origin`.
    inferred_type: Option<String>,
}

impl NodeKey {
    /// The node of the service whose segments report `name`.
    fn reported(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inferred_type: None,
        }
    }

    /// The `client` node of the root service called `name`.
    fn client(name: &str) -> Self {
        Self {
            name: name.to_string(),
            inferred_type: Some(SERVICE_TYPE_CLIENT.to_string()),
        }
    }
}

/// The downstream node a SUBSEGMENT names, or `None` when it names none.
///
/// The X-Ray segment-document schema gives a subsegment a `namespace` only when
/// the work left the process: `aws` for an AWS SDK call, `remote` for any other
/// downstream call. `Service$Type` names the matching node types: `remote` is
/// "a downstream service of indeterminate type", and `AWS::<Service>` is what
/// "downstream calls to Amazon Web Services ... that didn't target a specific
/// table" report. A subsegment with neither namespace recorded in-process work
/// and so names no service at all.
fn downstream_node(name: &str, namespace: Option<&str>) -> Option<NodeKey> {
    let inferred_type = inferred_service_type(name, namespace)?;
    Some(NodeKey {
        name: name.to_string(),
        inferred_type: Some(inferred_type),
    })
}

/// One node of a service graph, under construction.
#[derive(Debug, Default)]
struct NodeAccumulator {
    account_id: Option<String>,
    service_type: Option<String>,
    root: bool,
    complete: bool,
    span: Span,
    statistics: CallStatistics,
    durations: Vec<f64>,
    response_times: Vec<f64>,
    /// Downstream node -> the edge that reaches it.
    edges: BTreeMap<NodeKey, EdgeAccumulator>,
}

impl NodeAccumulator {
    /// Fold one request this node's service RECEIVED into it, without claiming
    /// the request's own identity: what an INFERRED node — a `client`, or a
    /// downstream service a subsegment named — knows about a call is its timing
    /// and its outcome, because it reported no segment of its own.
    fn record_call(&mut self, call: Call) {
        self.span.widen(call);
        self.statistics.record(call);
        if let Some(duration) = call.duration() {
            self.durations.push(duration);
            self.response_times.push(duration);
        }
    }

    /// Fold one of this service's segments into the node.
    fn record(&mut self, segment: &SegmentRecord) {
        let call = Call::of_segment(segment);
        self.span.widen(call);
        self.statistics.record(call);
        if segment.parent_id.is_none() {
            // "Indicates that the service was the first service to process a
            // request" — a segment with no parent is the entry point of its
            // trace.
            self.root = true;
        }
        if let Some(duration) = segment.duration() {
            self.complete = true;
            self.durations.push(duration);
            if segment.parent_id.is_none() {
                self.response_times.push(duration);
            }
        }
        if self.service_type.is_none() {
            self.service_type = segment.origin.clone();
        }
        if self.account_id.is_none() {
            self.account_id = segment.account_id.clone();
        }
    }

    /// This node as the modelled `Service`.
    fn to_service(&self, key: &NodeKey, reference_ids: &BTreeMap<NodeKey, i32>) -> Service {
        Service {
            reference_id: reference_ids.get(key).copied(),
            name: Some(key.name.clone()),
            names: Some(vec![key.name.clone()]),
            root: Some(self.root),
            account_id: self.account_id.clone(),
            // An inferred node's type is what makes it that node; a reported
            // one's is the `origin` its segments carried.
            r#type: key
                .inferred_type
                .clone()
                .or_else(|| self.service_type.clone()),
            state: Some(
                // A node the graph DISCOVERED downstream reported no segment of
                // its own: it is known passively, by having been called, which
                // is what distinguishes it from a service that reports. A node
                // whose own segments are all still in progress has reported
                // nothing to judge yet, and the `client` node is the stand-in
                // for the callers of a root service rather than a service the
                // graph discovered, so neither is passive.
                match key.inferred_type.as_deref() {
                    Some(SERVICE_TYPE_CLIENT) | None if self.complete => SERVICE_STATE_ACTIVE,
                    Some(SERVICE_TYPE_CLIENT) | None => SERVICE_STATE_UNKNOWN,
                    Some(_) => SERVICE_STATE_PASSIVE,
                }
                .to_string(),
            ),
            start_time: self.span.start_time(),
            end_time: self.span.end_time(),
            edges: Some(
                self.edges
                    .iter()
                    .filter_map(|(downstream, edge)| {
                        Some(edge.to_edge(reference_ids.get(downstream).copied()?))
                    })
                    .collect(),
            ),
            summary_statistics: Some(self.statistics.service_statistics()),
            duration_histogram: histogram(&self.durations),
            response_time_histogram: histogram(&self.response_times),
        }
    }
}

/// The spread of `values` as the modelled `Histogram`: one entry per distinct
/// millisecond-rounded value with how many samples landed on it, ascending, at
/// most [`MAX_HISTOGRAM_ENTRIES`] of them. `None` when nothing was sampled.
fn histogram(values: &[f64]) -> Option<Vec<HistogramEntry>> {
    if values.is_empty() {
        return None;
    }
    let mut buckets: BTreeMap<i64, i32> = BTreeMap::new();
    for value in values {
        #[allow(clippy::cast_possible_truncation)]
        let millis = (value * 1_000.0).round() as i64;
        *buckets.entry(millis).or_default() += 1;
    }
    Some(
        buckets
            .into_iter()
            .take(MAX_HISTOGRAM_ENTRIES)
            .map(|(millis, count)| {
                #[allow(clippy::cast_precision_loss)]
                let value = millis as f64 / 1_000.0;
                HistogramEntry { value, count }
            })
            .collect(),
    )
}

/// Derive the `Service` list a service graph reports from stored traces.
///
/// This is the one derivation behind both graph readers — `GetServiceGraph` over
/// a time window and `GetTraceGraph` over named traces — so the two can never
/// describe the same stored segments differently.
///
/// A node is one distinct segment `name` (the canonical name of the service that
/// handled the request), plus the `client` node `Service$Type` documents for
/// every root service — "represents the clients that sent requests to a root
/// service" — which carries that service's own name and one edge into it, plus
/// one node per SUBSEGMENT whose `namespace` says the call left the process
/// (`remote`, "a downstream service of indeterminate type", or `aws` for an AWS
/// service). A subsegment reports no service of its own otherwise: one with no
/// such namespace records in-process work, so it adds neither a node nor an
/// edge, and the work it did is already counted on the service that recorded it.
///
/// An edge is a stored `parent_id` link between two segments of DIFFERENT
/// services, counted on the upstream node and naming the downstream one by
/// `ReferenceId`; a `parent_id` that names a subsegment resolves to the service
/// that reported it. Every count, time and histogram comes from the stored
/// segment documents; nothing is echoed from the request.
///
/// `window` selects which segments take part: `Some((start, end))` keeps the
/// segments whose `start_time` falls in the half-open interval [start, end) —
/// the same selection `GetTraceSummaries` makes — and `None` keeps every segment
/// of every trace handed in, which is what a per-trace graph is.
pub fn service_graph<'a>(
    traces: impl IntoIterator<Item = &'a TraceRecord>,
    window: Option<(f64, f64)>,
) -> Vec<Service> {
    let selects = |segment: &SegmentRecord| match window {
        Some((start, end)) => segment.start_time >= start && segment.start_time < end,
        None => true,
    };

    let mut nodes: BTreeMap<NodeKey, NodeAccumulator> = BTreeMap::new();
    // Inline subsegments are walked under one budget across the whole
    // derivation, so a graph over many traces of many segments, each carrying
    // many subsegments, cannot multiply into unbounded work: past the budget a
    // graph reports the services its SEGMENTS named.
    let mut subsegment_budget = MAX_GRAPH_SUBSEGMENT_VISITS;
    for trace in traces {
        for segment in &trace.segments {
            // An inferred segment is not one a service REPORTED: it is this
            // service's own projection of a remote subsegment
            // ([`super::inferred_segments`]), and the downstream node that
            // subsegment names is derived from the subsegment itself below — so
            // counting the projection too would report one downstream call as
            // two nodes.
            if !selects(segment) || segment.is_subsegment() || segment.inferred {
                continue;
            }
            let call = Call::of_segment(segment);
            nodes
                .entry(NodeKey::reported(&segment.name))
                .or_default()
                .record(segment);
            if segment.parent_id.is_none() {
                // The entry point of a trace was reached by a client the stored
                // data names nowhere, which is exactly the node the model
                // documents as `client`: it carries the root service's name,
                // reports the requests it sent, and reaches that service by one
                // edge.
                let client = nodes.entry(NodeKey::client(&segment.name)).or_default();
                client.record_call(call);
                client
                    .edges
                    .entry(NodeKey::reported(&segment.name))
                    .or_default()
                    .record(call);
            }
            // The downstream services this segment's subsegments named: each is
            // one inferred node, reached by one edge from this service.
            let mut pending: Vec<&SubsegmentRecord> = segment.subsegments.iter().rev().collect();
            while let Some(subsegment) = pending.pop() {
                if subsegment_budget == 0 {
                    break;
                }
                subsegment_budget -= 1;
                pending.extend(subsegment.subsegments.iter().rev());
                let Some(downstream) =
                    downstream_node(&subsegment.name, subsegment.namespace.as_deref())
                else {
                    continue;
                };
                if downstream == NodeKey::reported(&segment.name) {
                    continue;
                }
                let subsegment_call = Call::of_subsegment(subsegment, segment);
                nodes
                    .entry(NodeKey::reported(&segment.name))
                    .or_default()
                    .edges
                    .entry(downstream.clone())
                    .or_default()
                    .record(subsegment_call);
                nodes
                    .entry(downstream)
                    .or_default()
                    .record_call(subsegment_call);
            }
        }
        // Edges need both ends, so they are walked once this trace's nodes are
        // known: a segment whose `parent_id` names nothing stored, or whose
        // parent is outside the window, is on no edge. Which service reported
        // each stored id is resolved ONCE for the whole trace, so this pass is
        // linear in the trace's segments however long a chain of subsegment
        // documents is.
        let reporting = reporting_segments(trace);
        for segment in &trace.segments {
            if !selects(segment) || segment.inferred {
                continue;
            }
            // The service that recorded this segment's parent: a `parent_id`
            // naming a subsegment belongs to the service that reported it.
            let Some(parent) = segment
                .parent_id
                .as_deref()
                .and_then(|parent_id| reporting.get(parent_id).copied())
            else {
                continue;
            };
            if !selects(parent) {
                continue;
            }
            let Some(upstream) = nodes.get_mut(&NodeKey::reported(&parent.name)) else {
                continue;
            };
            // A subsegment names a downstream service only through its
            // `namespace`; a segment names the service that handled it.
            let downstream = if segment.is_subsegment() {
                match downstream_node(&segment.name, segment.namespace.as_deref()) {
                    Some(key) => key,
                    None => continue,
                }
            } else {
                NodeKey::reported(&segment.name)
            };
            if downstream == NodeKey::reported(&parent.name) {
                continue;
            }
            let call = Call::of_segment(segment);
            upstream
                .edges
                .entry(downstream.clone())
                .or_default()
                .record(call);
            // A downstream service a subsegment named reported nothing itself,
            // so the call is what the node knows about it.
            if segment.is_subsegment() {
                nodes.entry(downstream).or_default().record_call(call);
            }
        }
    }

    // "Identifier for the service. Unique within the service map": assigned over
    // the nodes in key order — name, then the inferred type that distinguishes a
    // `client` node from the service it calls — so one stored graph always
    // answers with the same ids, across pages of one walk and across calls.
    let reference_ids: BTreeMap<NodeKey, i32> = nodes
        .keys()
        .enumerate()
        .map(|(at, key)| (key.clone(), i32::try_from(at).unwrap_or(i32::MAX)))
        .collect();

    nodes
        .iter()
        .map(|(key, node)| node.to_service(key, &reference_ids))
        .collect()
}

/// Every stored id of one trace mapped to the segment that REPORTED the service
/// it belongs to.
///
/// A `type: "subsegment"` document records work inside its parent's service, so
/// it belongs to whichever segment its `parent_id` chain reaches. Resolving that
/// by walking the chain per segment would be quadratic in the trace's segments,
/// so the map is built ONCE: the segments a service reported seed it, and the
/// subsegment documents are attached by walking OUT from the ids already
/// resolved, which visits each document at most twice and ends by itself on a
/// chain that names itself (each parent's children are consumed once).
fn reporting_segments(trace: &TraceRecord) -> BTreeMap<&str, &SegmentRecord> {
    let mut reporting: BTreeMap<&str, &SegmentRecord> = BTreeMap::new();
    let mut attached: BTreeMap<&str, Vec<&SegmentRecord>> = BTreeMap::new();
    for segment in &trace.segments {
        if segment.is_subsegment() {
            if let Some(parent) = segment.parent_id.as_deref() {
                attached.entry(parent).or_default().push(segment);
            }
        } else {
            reporting.insert(segment.id.as_str(), segment);
        }
    }
    let mut frontier: Vec<&str> = reporting.keys().copied().collect();
    while let Some(id) = frontier.pop() {
        let Some(children) = attached.remove(id) else {
            continue;
        };
        let Some(owner) = reporting.get(id).copied() else {
            continue;
        };
        for child in children {
            reporting.insert(child.id.as_str(), owner);
            frontier.push(child.id.as_str());
        }
    }
    reporting
}

/// One page of a service graph: the `Service` values to answer with, and the
/// `NextToken` that resumes after them when the graph was truncated.
///
/// Lives beside [`service_graph`] so the two graph readers cannot drift in their
/// pagination contract.
#[derive(Debug, Clone)]
pub struct ServiceGraphPage {
    /// The page's services, at most `page_size` of them.
    pub services: Vec<Service>,
    /// The token that resumes after this page, or `None` when it is the last.
    pub next_token: Option<String>,
}

/// Cut one page out of a WHOLE derived graph, resuming at `resume`.
///
/// The page is cut after the graph is derived, so every page of one walk reports
/// the same `ReferenceId` for the same service and an edge's `ReferenceId` names
/// a node whichever page that node lands on. The token is the `ReferenceId` the
/// next page starts at, which is what identifies a node: a `client` node carries
/// the same `Name` as the root service it calls, so a name would not. `Err` is
/// the one refusal this makes possible: a token that is not a `ReferenceId` of
/// this graph, which is a token this service never issued.
pub fn page_service_graph(
    services: &[Service],
    resume: Option<&str>,
    page_size: usize,
) -> Result<ServiceGraphPage, String> {
    let unissued = |token: &str| format!("NextToken `{token}` was not issued by this service");
    let from = match resume {
        Some(token) => {
            let reference: i32 = token.parse().map_err(|_| unissued(token))?;
            services
                .iter()
                .position(|service| service.reference_id.is_some_and(|id| id >= reference))
                .ok_or_else(|| unissued(token))?
        }
        None => 0,
    };
    let remaining = &services[from..];
    Ok(ServiceGraphPage {
        services: remaining.iter().take(page_size).cloned().collect(),
        next_token: remaining
            .get(page_size)
            .and_then(|service| service.reference_id)
            .map(|reference| reference.to_string()),
    })
}
