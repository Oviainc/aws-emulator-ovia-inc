// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAnomaliesForInsight`, the anomaly listing of one insight.
//!
//! The RULES these tests pin:
//!
//! * The read reports the anomalies the store holds that BELONG to the requested
//!   insight, each on the response list its kind selects (`ProactiveAnomalies` or
//!   `ReactiveAnomalies`, never both), with exactly the members that summary shape
//!   models and the values the record holds.
//! * An anomaly of another insight, and one the analysis associated with no insight, is
//!   not reported for this one.
//! * `StartTimeRange` selects the anomalies whose behaviour STARTED inside it, each
//!   bound inclusive and an absent bound unbounded on that side.
//! * `Filters.ServiceCollection.ServiceNames` selects the anomalies found in ANY ONE of
//!   the services it names; an anomaly attributed to no service is in no such set, and a
//!   request naming no service restricts nothing.
//! * A value outside its modelled domain — a service name outside `ServiceName`, a
//!   `MaxResults` outside its range — is refused with HTTP 400.
//! * A walk pages at the `MaxResults` the caller asked for, counted across BOTH lists
//!   together, and the pages concatenate to the whole selection exactly once with no
//!   token on the last page.
//! * A page token is bound to the insight AND to what the filters select, so another
//!   insight's token, another selection's token and a token whose page no longer exists
//!   are refused with `ValidationException` (400).
//! * An `InsightId` the store holds no insight under names no insight and is refused
//!   with `ResourceNotFoundException` (404) naming the id asked for; an insight that
//!   exists with no anomaly of its own is empty lists, not a refusal.
//! * An `AccountId` the caller cannot reach is an authorization refusal
//!   (`AccessDeniedException`, 403), decided before any lookup, so it is the same answer
//!   whether or not the insight exists.
//! * It reports: the store is identical before and after, however often it is read.
//!
//! DevOps Guru detects anomalies through its own analysis, so no client request creates
//! one; these tests therefore write records straight into the engine's store (the same
//! store the handler reads through `crate::state::service_state`) and then drive the
//! operation over the wire through the same [`MultiServiceRouter`] the emulator boots,
//! so every assertion is about wire behaviour.
//!
//! Each test exercises its rule over a SET of inputs differing along what the rule
//! generalises over: anomaly kind, severity, status, reactive anomaly type, which
//! optional members a record carries, how many services an anomaly is attributed to and
//! which, the window asked for and which of its bounds are named, the requested page
//! size, the shape of the requested insight id, and the account the read is scoped to.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    AnomalyResource, AnomalySourceDetails, AnomalySourceMetadata, CloudWatchMetricsDetail,
    ANOMALY_SEVERITY_VALUES, ANOMALY_STATUS_VALUES, ANOMALY_TYPE_VALUES, SERVICE_NAME_VALUES,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    AnomalyRecord, ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord,
    ResourceCoverage, LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS, PAGE_SIZE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A twelve-digit account id other than the emulator's own
/// (`engine::emulator_account_id`), as `AwsAccountId`'s `^\d{12}$` pattern requires.
/// This emulator is in no organization, so this account is not reachable from it.
const OTHER_ACCOUNT_ID: &str = "111122223333";

/// The insight the listing under test is about.
const INSIGHT_ID: &str = "insight-anomalies-1";

/// A second insight, whose own anomalies this listing must not report.
const OTHER_INSIGHT_ID: &str = "insight-anomalies-2";

/// The `ResourceType` an insight-shaped `ResourceNotFoundException` reports.
const INSIGHT_RESOURCE_TYPE: &str = "INSIGHT";

/// A base instant (epoch milliseconds) the seeded anomaly windows are placed around. A
/// whole number of seconds, and every offset below is a whole second, so each instant is
/// exactly representable in the epoch-seconds `double` a JSON protocol carries a
/// `Timestamp` as.
const BASE_MILLIS: i64 = 1_700_000_000_000;

/// One minute in milliseconds, the unit the seeded start instants are spaced by.
const MINUTE_MILLIS: i64 = 60_000;

/// One hour in milliseconds, the length of a seeded anomaly's closed window.
const HOUR_MILLIS: i64 = 3_600_000;

/// `AnomalyStatus`'s "the behaviour ended" member — see `ANOMALY_STATUS_VALUES`.
const STATUS_CLOSED: &str = "CLOSED";

/// One reply, as a client sees it.
struct Reply {
    status: u16,
    body: Value,
    /// The error SHAPE NAME the reply carries, from the `x-amzn-errortype` header or the
    /// body's `__type`, with any namespace/URL prefix stripped. Empty on success.
    error_name: String,
    raw: String,
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} {}", self.status, self.raw)
    }
}

impl Reply {
    fn at(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer)
    }

    fn str_at(&self, pointer: &str) -> Option<&str> {
        self.at(pointer).and_then(Value::as_str)
    }

    /// The reported anomalies of one list, in the order the reply listed them.
    fn list(&self, member: &str) -> Vec<Value> {
        self.at(&format!("/{member}"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The ids one reported list carries, in the order the reply listed them.
    fn ids_of(&self, member: &str) -> Vec<String> {
        self.list(member)
            .iter()
            .map(|anomaly| {
                anomaly
                    .get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    fn proactive_ids(&self) -> Vec<String> {
        self.ids_of("ProactiveAnomalies")
    }

    fn reactive_ids(&self) -> Vec<String> {
        self.ids_of("ReactiveAnomalies")
    }

    /// Every id the reply reports, the proactive list before the reactive one.
    fn ids(&self) -> Vec<String> {
        let mut ids = self.proactive_ids();
        ids.extend(self.reactive_ids());
        ids
    }

    /// The reported anomaly with this id, whichever list carries it.
    fn anomaly(&self, id: &str) -> Option<Value> {
        self.list("ProactiveAnomalies")
            .into_iter()
            .chain(self.list("ReactiveAnomalies"))
            .find(|anomaly| anomaly.get("Id").and_then(Value::as_str) == Some(id))
    }

    fn next_token(&self) -> Option<String> {
        self.str_at("/NextToken").map(str::to_string)
    }
}

/// A fresh emulator holding only this service, over an engine the test keeps a handle to
/// so it can seed the anomalies DevOps Guru's analysis would have detected.
///
/// Its own harness rather than a shared one: the suites in this crate each build the
/// router they need, and there is no shared harness module to reuse yet.
struct Guru {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Guru {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process-wide switch.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_devopsguru_capstonecontrolplaneservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// The very store the handler reads: `state::<T>()` is keyed by type, so this is the
    /// same value `crate::state::service_state` hands a request.
    fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState> {
        self.engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>()
    }

    /// Seed the insight DevOps Guru's analysis would have produced.
    fn seed_insight(&self, insight_id: &str) {
        let record = InsightRecord {
            id: insight_id.to_string(),
            name: format!("insight {insight_id}"),
            insight_type: InsightRecord::PROACTIVE.to_string(),
            severity: "HIGH".to_string(),
            status: "ONGOING".to_string(),
            start_time_millis: BASE_MILLIS,
            ..InsightRecord::default()
        };
        self.store()
            .lock()
            .insights
            .insert(insight_id.to_string(), record);
    }

    fn seed_anomaly(&self, record: AnomalyRecord) {
        self.store()
            .lock()
            .anomalies
            .insert(record.id.clone(), record);
    }

    /// Seed both insights and every row's anomaly.
    fn seed(&self, rows: &[Row]) {
        self.seed_insight(INSIGHT_ID);
        self.seed_insight(OTHER_INSIGHT_ID);
        for row in rows {
            self.seed_anomaly(row.record());
        }
    }

    /// A stable rendering of the whole store, for the "a read does not write" check.
    fn snapshot(&self) -> String {
        format!("{:?}", *self.store().lock())
    }

    async fn list(&self, insight_id: &str, body: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/anomalies/insight/{insight_id}"),
            body,
        )
        .await
    }

    async fn call(&self, method: &str, path: &str, body: &Value) -> Reply {
        let payload = if body.is_null() {
            Bytes::new()
        } else {
            Bytes::from(body.to_string())
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", HOST)
            .header("content-type", "application/json")
            .body(payload)
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let raw = format!("{errortype} {}", String::from_utf8_lossy(&bytes));
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        // restJson1 names the error in the `x-amzn-errortype` header, the body's
        // `__type`, or both; either may carry a `namespace#Name` or URL prefix.
        let declared = if errortype.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            errortype
        };
        let error_name = declared
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            body,
            error_name,
            raw,
        }
    }
}

/// One seeded anomaly, as DevOps Guru's analysis would have detected it.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    /// Which kind the anomaly is, and so which response list reports it.
    proactive: bool,
    severity: &'static str,
    status: &'static str,
    start_millis: i64,
    /// The insight the analysis associated it with, or none at all.
    associated_insight_id: Option<&'static str>,
    /// The services the anomalous behaviour was found in, each one of
    /// `SERVICE_NAME_VALUES`; empty for an anomaly attributed to none.
    service_names: Vec<&'static str>,
    /// `ReactiveAnomaly.Type`, one of `ANOMALY_TYPE_VALUES`, on a reactive row.
    anomaly_type: Option<&'static str>,
    /// Whether this row carries every optional member its kind models, or only the
    /// required ones: the model makes most members optional, so both forms must report.
    full: bool,
}

impl Row {
    fn closed(&self) -> bool {
        self.status == STATUS_CLOSED
    }

    fn end_millis(&self) -> Option<i64> {
        self.closed().then_some(self.start_millis + HOUR_MILLIS)
    }

    fn open_millis(&self) -> i64 {
        self.start_millis + MINUTE_MILLIS
    }

    fn close_millis(&self) -> Option<i64> {
        self.closed()
            .then_some(self.start_millis + HOUR_MILLIS + MINUTE_MILLIS)
    }

    fn update_millis(&self) -> Option<i64> {
        (self.proactive && self.full).then_some(self.start_millis + 2 * MINUTE_MILLIS)
    }

    fn prediction(&self) -> Option<(i64, Option<i64>)> {
        (self.proactive && self.full)
            .then_some((self.start_millis + HOUR_MILLIS, Some(self.start_millis + 2 * HOUR_MILLIS)))
    }

    fn limit(&self) -> Option<f64> {
        (self.proactive && self.full).then_some(42.5)
    }

    fn name(&self) -> Option<String> {
        (!self.proactive && self.full).then(|| format!("anomaly {}", self.id))
    }

    fn causal_anomaly_id(&self) -> Option<String> {
        (!self.proactive && self.full).then(|| format!("{}-causal", self.id))
    }

    fn description(&self) -> Option<String> {
        self.full.then(|| format!("behaviour of {}", self.id))
    }

    fn resource_names(&self) -> Vec<String> {
        if !self.full {
            return Vec::new();
        }
        vec![format!("resource-{}", self.id)]
    }

    fn record(&self) -> AnomalyRecord {
        AnomalyRecord {
            id: self.id.clone(),
            proactive: self.proactive,
            severity: self.severity.to_string(),
            status: self.status.to_string(),
            start_time_millis: self.start_millis,
            end_time_millis: self.end_millis(),
            open_time_millis: self.open_millis(),
            close_time_millis: self.close_millis(),
            update_time_millis: self.update_millis(),
            prediction_start_time_millis: self.prediction().map(|(start, _)| start),
            prediction_end_time_millis: self.prediction().and_then(|(_, end)| end),
            limit: self.limit(),
            associated_insight_id: self.associated_insight_id.map(str::to_string),
            service_names: self.service_names.iter().map(|name| name.to_string()).collect(),
            resource_collection: self.full.then(|| ResourceCoverage {
                cloudformation_stack_names: BTreeSet::from(["ops-stack".to_string()]),
                tag_collections: BTreeMap::from([(
                    "DevOps-Guru-app".to_string(),
                    BTreeSet::from(["payments".to_string()]),
                )]),
            }),
            source_details: self.full.then(|| AnomalySourceDetails {
                cloud_watch_metrics: Some(vec![CloudWatchMetricsDetail {
                    metric_name: Some(format!("metric-{}", self.id)),
                    namespace: Some("AWS/Lambda".to_string()),
                    stat: Some("Average".to_string()),
                    unit: Some("Count".to_string()),
                    period: 60,
                    ..CloudWatchMetricsDetail::default()
                }]),
                performance_insights_metrics: None,
            }),
            source_metadata: (self.proactive && self.full).then(|| AnomalySourceMetadata {
                source: Some("CW_METRICS".to_string()),
                source_resource_name: Some(format!("source-{}", self.id)),
                source_resource_type: Some("AWS".to_string()),
            }),
            anomaly_resources: self
                .resource_names()
                .into_iter()
                .map(|name| AnomalyResource {
                    name: Some(name),
                    r#type: Some("AWS".to_string()),
                })
                .collect(),
            anomaly_type: self.anomaly_type.map(str::to_string),
            name: self.name(),
            causal_anomaly_id: self.causal_anomaly_id(),
            description: self.description(),
        }
    }
}

/// The seeded anomalies: every combination of kind with the model's `AnomalySeverity`
/// and `AnomalyStatus` catalogs (`ANOMALY_SEVERITY_VALUES`, `ANOMALY_STATUS_VALUES` in
/// the service's `ops/types/shared_types.rs`, generated from the model's enums), walking
/// the `AnomalyType` catalog, the service attributions, the optional members and the
/// start instants alongside them — plus one anomaly of ANOTHER insight and one the
/// analysis associated with no insight.
fn rows() -> Vec<Row> {
    // The service attributions the rows walk, drawn from `SERVICE_NAME_VALUES`: none at
    // all, one service, and two — including entries no other suite exercises.
    let attributions: [Vec<&'static str>; 4] = [
        vec!["LAMBDA"],
        vec!["RDS", "KINESIS"],
        Vec::new(),
        vec!["STEP_FUNCTIONS"],
    ];
    let mut rows = Vec::new();
    for proactive in [true, false] {
        for (s, severity) in ANOMALY_SEVERITY_VALUES.iter().enumerate() {
            for (c, status) in ANOMALY_STATUS_VALUES.iter().enumerate() {
                let index = rows.len();
                rows.push(Row {
                    id: format!("anomaly-{index:02}"),
                    proactive,
                    severity,
                    status,
                    start_millis: BASE_MILLIS + index as i64 * MINUTE_MILLIS,
                    associated_insight_id: Some(INSIGHT_ID),
                    service_names: attributions[index % attributions.len()].clone(),
                    anomaly_type: (!proactive)
                        .then(|| ANOMALY_TYPE_VALUES[(s + c) % ANOMALY_TYPE_VALUES.len()]),
                    full: index % 2 == 0,
                });
            }
        }
    }
    // An anomaly of the OTHER insight, and one the analysis associated with none: both
    // live in the same map and neither belongs to the insight under test.
    rows.push(Row {
        id: "anomaly-other".to_string(),
        proactive: true,
        severity: "HIGH",
        status: "ONGOING",
        start_millis: BASE_MILLIS,
        associated_insight_id: Some(OTHER_INSIGHT_ID),
        service_names: vec!["LAMBDA"],
        anomaly_type: None,
        full: true,
    });
    rows.push(Row {
        id: "anomaly-unassociated".to_string(),
        proactive: false,
        severity: "LOW",
        status: "ONGOING",
        start_millis: BASE_MILLIS,
        associated_insight_id: None,
        service_names: vec!["LAMBDA"],
        anomaly_type: Some("CAUSAL"),
        full: true,
    });
    rows
}

/// The rows that belong to the insight under test, in the store's key order.
fn rows_of_insight(rows: &[Row]) -> Vec<Row> {
    let mut selected: Vec<Row> = rows
        .iter()
        .filter(|row| row.associated_insight_id == Some(INSIGHT_ID))
        .cloned()
        .collect();
    selected.sort_by(|left, right| left.id.cmp(&right.id));
    selected
}

/// Every id a listing reports, in the store's key order, so a selection and a reply can
/// be compared as sets of ids.
fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// An instant as a JSON-protocol `Timestamp`: epoch seconds, fractional to the
/// millisecond.
fn seconds(millis: i64) -> f64 {
    millis as f64 / 1000.0
}

/// Walk every page of one request body, following its cursor to the end, and answer the
/// ids seen with the page sizes they came in.
async fn walk(guru: &Guru, case: &str, body: &Value, page_size: usize) -> Vec<String> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..64 {
        let mut request = body.clone();
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let reply = guru.list(INSIGHT_ID, &request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?} page {page}: expected HTTP 200, got {reply:?}"
        );
        let ids = reply.ids();
        assert!(
            ids.len() <= page_size,
            "case {case:?} page {page}: expected at most {page_size} anomalies across both lists, got {}",
            ids.len()
        );
        seen.extend(ids);
        token = reply.next_token();
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {case:?}: the walk did not terminate");
}

/// The catalogs this suite's rows are drawn from are the model's own enums.
#[test]
fn seeded_values_are_modelled_values() {
    for row in rows() {
        assert!(
            ANOMALY_SEVERITY_VALUES.contains(&row.severity),
            "case {:?}: {:?} is not a modelled AnomalySeverity, which are {ANOMALY_SEVERITY_VALUES:?}",
            row.id,
            row.severity
        );
        assert!(
            ANOMALY_STATUS_VALUES.contains(&row.status),
            "case {:?}: {:?} is not a modelled AnomalyStatus, which are {ANOMALY_STATUS_VALUES:?}",
            row.id,
            row.status
        );
        for service_name in &row.service_names {
            assert!(
                SERVICE_NAME_VALUES.contains(service_name),
                "case {:?}: {service_name:?} is not a modelled ServiceName, which are {SERVICE_NAME_VALUES:?}",
                row.id
            );
        }
        if let Some(anomaly_type) = row.anomaly_type {
            assert!(
                ANOMALY_TYPE_VALUES.contains(&anomaly_type),
                "case {:?}: {anomaly_type:?} is not a modelled AnomalyType, which are {ANOMALY_TYPE_VALUES:?}",
                row.id
            );
        }
    }
}

/// RULE: the read reports the anomalies of the requested insight on the list their kind
/// selects, with exactly the members that summary shape models and the values the record
/// holds.
#[tokio::test]
async fn reports_each_anomaly_of_the_insight_on_the_list_its_kind_selects() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);

    let reply = guru
        .list(
            INSIGHT_ID,
            &json!({ "MaxResults": LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {INSIGHT_ID:?}: expected HTTP 200, got {reply:?}"
    );

    let selected = rows_of_insight(&rows);
    let expected_proactive: Vec<String> = selected
        .iter()
        .filter(|row| row.proactive)
        .map(|row| row.id.clone())
        .collect();
    let expected_reactive: Vec<String> = selected
        .iter()
        .filter(|row| !row.proactive)
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(
        reply.proactive_ids(),
        expected_proactive,
        "case {INSIGHT_ID:?}: expected the proactive anomalies in the store's key order, got {reply:?}"
    );
    assert_eq!(
        reply.reactive_ids(),
        expected_reactive,
        "case {INSIGHT_ID:?}: expected the reactive anomalies in the store's key order, got {reply:?}"
    );

    for row in &selected {
        let case = &row.id;
        let anomaly = reply
            .anomaly(case)
            .unwrap_or_else(|| panic!("case {case:?}: not reported at all, got {reply:?}"));
        let member = |name: &str| anomaly.get(name).cloned();
        let string = |name: &str| {
            member(name)
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let millis = |pointer: &str| {
            anomaly
                .pointer(pointer)
                .and_then(Value::as_f64)
                .map(|at| (at * 1000.0).round() as i64)
        };
        assert_eq!(
            string("Severity").as_deref(),
            Some(row.severity),
            "case {case:?}: expected the stored severity, got {anomaly:?}"
        );
        assert_eq!(
            string("Status").as_deref(),
            Some(row.status),
            "case {case:?}: expected the stored status, got {anomaly:?}"
        );
        assert_eq!(
            millis("/AnomalyTimeRange/StartTime"),
            Some(row.start_millis),
            "case {case:?}: expected the stored start instant, got {anomaly:?}"
        );
        assert_eq!(
            millis("/AnomalyTimeRange/EndTime"),
            row.end_millis(),
            "case {case:?}: expected the stored end instant, got {anomaly:?}"
        );
        assert_eq!(
            millis("/AnomalyReportedTimeRange/OpenTime"),
            Some(row.open_millis()),
            "case {case:?}: expected the stored open instant, got {anomaly:?}"
        );
        assert_eq!(
            millis("/AnomalyReportedTimeRange/CloseTime"),
            row.close_millis(),
            "case {case:?}: expected the stored close instant, got {anomaly:?}"
        );
        assert_eq!(
            string("AssociatedInsightId").as_deref(),
            Some(INSIGHT_ID),
            "case {case:?}: expected the insight asked about, got {anomaly:?}"
        );
        assert_eq!(
            string("Description"),
            row.description(),
            "case {case:?}: expected the stored description, got {anomaly:?}"
        );

        if row.proactive {
            assert_eq!(
                millis("/UpdateTime"),
                row.update_millis(),
                "case {case:?}: expected the stored update instant, got {anomaly:?}"
            );
            assert_eq!(
                millis("/PredictionTimeRange/StartTime"),
                row.prediction().map(|(start, _)| start),
                "case {case:?}: expected the stored prediction window, got {anomaly:?}"
            );
            assert_eq!(
                member("Limit").as_ref().and_then(Value::as_f64),
                row.limit(),
                "case {case:?}: expected the stored limit, got {anomaly:?}"
            );
            // The members only `ReactiveAnomalySummary` models must not appear on a
            // proactive anomaly.
            for absent in ["Type", "Name", "CausalAnomalyId"] {
                assert!(
                    member(absent).is_none(),
                    "case {case:?}: {absent} is modelled only on a reactive anomaly, got {anomaly:?}"
                );
            }
        } else {
            assert_eq!(
                string("Type").as_deref(),
                row.anomaly_type,
                "case {case:?}: expected the stored anomaly type, got {anomaly:?}"
            );
            assert_eq!(
                string("Name"),
                row.name(),
                "case {case:?}: expected the stored name, got {anomaly:?}"
            );
            assert_eq!(
                string("CausalAnomalyId"),
                row.causal_anomaly_id(),
                "case {case:?}: expected the stored causal anomaly id, got {anomaly:?}"
            );
            // The members only `ProactiveAnomalySummary` models must not appear on a
            // reactive anomaly.
            for absent in ["UpdateTime", "PredictionTimeRange", "Limit", "SourceMetadata"] {
                assert!(
                    member(absent).is_none(),
                    "case {case:?}: {absent} is modelled only on a proactive anomaly, got {anomaly:?}"
                );
            }
        }

        let reported_resources: Vec<String> = anomaly
            .get("AnomalyResources")
            .and_then(Value::as_array)
            .map(|resources| {
                resources
                    .iter()
                    .map(|resource| {
                        resource
                            .get("Name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported_resources,
            row.resource_names(),
            "case {case:?}: expected the stored anomaly resources, got {anomaly:?}"
        );
    }
}

/// RULE: an anomaly of another insight, and one associated with no insight, is not
/// reported for this one.
#[tokio::test]
async fn only_the_anomalies_of_the_requested_insight_are_reported() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);

    for (case, insight_id) in [("the insight under test", INSIGHT_ID), ("the other insight", OTHER_INSIGHT_ID)] {
        let reply = guru
            .list(
                insight_id,
                &json!({ "MaxResults": LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {reply:?}"
        );
        let expected = sorted(
            rows.iter()
                .filter(|row| row.associated_insight_id == Some(insight_id))
                .map(|row| row.id.clone())
                .collect(),
        );
        assert_eq!(
            sorted(reply.ids()),
            expected,
            "case {case:?}: expected only the anomalies of that insight, got {reply:?}"
        );
    }
}

/// One row of the `StartTimeRange` table: the case name, the range as a request carries
/// it, and the instants that range holds.
type WindowCase = (&'static str, Value, Box<dyn Fn(i64) -> bool>);

/// RULE: `StartTimeRange` selects the anomalies whose behaviour started inside it, each
/// bound inclusive and an absent bound unbounded on that side.
#[tokio::test]
async fn the_start_time_range_selects_the_anomalies_that_started_in_it() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);
    let selected = rows_of_insight(&rows);
    let earliest = selected
        .iter()
        .map(|row| row.start_millis)
        .min()
        .expect("the seeded insight must own an anomaly");
    let latest = selected
        .iter()
        .map(|row| row.start_millis)
        .max()
        .expect("the seeded insight must own an anomaly");

    // (case, the range as sent, what it holds)
    let cases: Vec<WindowCase> = vec![
        (
            "both bounds on one instant: inclusive at each end",
            json!({ "FromTime": seconds(earliest), "ToTime": seconds(earliest) }),
            Box::new(move |at| at == earliest),
        ),
        (
            "a window inside the seeded span",
            json!({
                "FromTime": seconds(earliest + 2 * MINUTE_MILLIS),
                "ToTime": seconds(earliest + 4 * MINUTE_MILLIS),
            }),
            Box::new(move |at| {
                at >= earliest + 2 * MINUTE_MILLIS && at <= earliest + 4 * MINUTE_MILLIS
            }),
        ),
        (
            "FromTime only: unbounded above",
            json!({ "FromTime": seconds(earliest + 3 * MINUTE_MILLIS) }),
            Box::new(move |at| at >= earliest + 3 * MINUTE_MILLIS),
        ),
        (
            "ToTime only: unbounded below",
            json!({ "ToTime": seconds(earliest + 3 * MINUTE_MILLIS) }),
            Box::new(move |at| at <= earliest + 3 * MINUTE_MILLIS),
        ),
        (
            "a window spanning every seeded instant",
            json!({ "FromTime": seconds(earliest), "ToTime": seconds(latest) }),
            Box::new(|_| true),
        ),
        (
            "a window before every seeded instant",
            json!({
                "FromTime": seconds(earliest - 2 * HOUR_MILLIS),
                "ToTime": seconds(earliest - HOUR_MILLIS),
            }),
            Box::new(|_| false),
        ),
        (
            "an empty interval: FromTime after ToTime holds no instant",
            json!({ "FromTime": seconds(latest), "ToTime": seconds(earliest) }),
            Box::new(|_| false),
        ),
    ];

    for (case, range, holds) in cases {
        let body = json!({
            "StartTimeRange": range,
            "MaxResults": LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS,
        });
        let reply = guru.list(INSIGHT_ID, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {reply:?}"
        );
        let expected = sorted(
            selected
                .iter()
                .filter(|row| holds(row.start_millis))
                .map(|row| row.id.clone())
                .collect(),
        );
        assert_eq!(
            sorted(reply.ids()),
            expected,
            "case {case:?}: expected the anomalies that started in the window, got {reply:?}"
        );
    }
}

/// RULE: the service-name filter selects the anomalies found in any one of the services
/// it names; an anomaly attributed to no service is in no such set, and a request naming
/// no service restricts nothing.
#[tokio::test]
async fn the_service_filter_selects_anomalies_found_in_any_named_service() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);
    let selected = rows_of_insight(&rows);

    // The filters walked, each a set of `SERVICE_NAME_VALUES` entries: one service, a
    // set of two, a repeated value (the same set), a service nothing is attributed to,
    // and no filter at all.
    let cases: Vec<(&str, Option<Vec<&str>>)> = vec![
        ("one named service", Some(vec!["LAMBDA"])),
        ("two named services", Some(vec!["RDS", "KINESIS"])),
        ("a service no anomaly was found in", Some(vec!["DYNAMO_DB"])),
        ("a value repeated names the same set", Some(vec!["LAMBDA", "LAMBDA"])),
        ("a service only one row carries", Some(vec!["STEP_FUNCTIONS"])),
        ("an empty list restricts nothing", Some(Vec::new())),
        ("no filter at all", None),
    ];

    for (case, service_names) in cases {
        let mut body = json!({ "MaxResults": LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS });
        if let Some(service_names) = &service_names {
            body["Filters"] = json!({ "ServiceCollection": { "ServiceNames": service_names } });
        }
        let reply = guru.list(INSIGHT_ID, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {reply:?}"
        );
        let expected = sorted(
            selected
                .iter()
                .filter(|row| match &service_names {
                    None => true,
                    Some(wanted) if wanted.is_empty() => true,
                    Some(wanted) => wanted
                        .iter()
                        .any(|service_name| row.service_names.contains(service_name)),
                })
                .map(|row| row.id.clone())
                .collect(),
        );
        assert_eq!(
            sorted(reply.ids()),
            expected,
            "case {case:?}: expected the anomalies found in a named service, got {reply:?}"
        );
    }
}

/// RULE: a value outside its modelled domain is refused with HTTP 400 rather than
/// answered with some page.
#[tokio::test]
async fn a_value_outside_its_modelled_domain_is_refused() {
    let guru = Guru::new();
    guru.seed(&rows());

    let service_filter = |service_name: &str| {
        json!({ "Filters": { "ServiceCollection": { "ServiceNames": [service_name] } } })
    };
    let cases: Vec<(&str, Value)> = vec![
        (
            "a service name outside the modelled enum",
            service_filter("LAMBDA_FUNCTION"),
        ),
        (
            "a service name in the wrong case",
            service_filter("lambda"),
        ),
        (
            "MaxResults below the modelled minimum",
            json!({ "MaxResults": 0 }),
        ),
        (
            "MaxResults one past the modelled maximum",
            json!({ "MaxResults": LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS + 1 }),
        ),
        (
            "a NextToken outside the modelled pattern",
            json!({ "NextToken": "not-a-uuid" }),
        ),
    ];

    for (case, body) in cases {
        let reply = guru.list(INSIGHT_ID, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
    }
}

/// RULE: a walk pages at the `MaxResults` the caller asked for, counted across both
/// lists, and the pages concatenate to the whole selection exactly once.
#[tokio::test]
async fn a_walk_pages_at_max_results_across_both_lists() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);
    let expected = sorted(
        rows_of_insight(&rows)
            .iter()
            .map(|row| row.id.clone())
            .collect(),
    );

    // Each published bound of `ListAnomaliesForInsightMaxResults` (range 1..500), sizes
    // in between, and no `MaxResults` at all (the service's own page size).
    let sizes: Vec<Option<i32>> = vec![
        Some(1),
        Some(2),
        Some(5),
        Some(LIST_ANOMALIES_FOR_INSIGHT_MAX_RESULTS),
        None,
    ];
    for size in sizes {
        let case = format!("MaxResults {size:?}");
        let mut body = json!({});
        if let Some(size) = size {
            body["MaxResults"] = json!(size);
        }
        let page_size = size.map_or(PAGE_SIZE, |size| size as usize);
        let seen = walk(&guru, &case, &body, page_size).await;
        assert_eq!(
            sorted(seen.clone()),
            expected,
            "case {case:?}: expected the whole selection exactly once, got {seen:?}"
        );
    }
}

/// RULE: a page token is bound to the insight AND to what the filters select, so a
/// foreign token and a token whose page no longer exists are refused.
#[tokio::test]
async fn a_cursor_is_bound_to_the_insight_and_the_selection() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);
    // The other insight needs more than one anomaly for a cursor of its own.
    for index in 0..3 {
        guru.seed_anomaly(
            Row {
                id: format!("anomaly-other-{index}"),
                proactive: index % 2 == 0,
                severity: "LOW",
                status: "ONGOING",
                start_millis: BASE_MILLIS + index * MINUTE_MILLIS,
                associated_insight_id: Some(OTHER_INSIGHT_ID),
                service_names: vec!["LAMBDA"],
                anomaly_type: (index % 2 == 1).then_some("CAUSAL"),
                full: false,
            }
            .record(),
        );
    }

    let first = guru.list(INSIGHT_ID, &json!({ "MaxResults": 1 })).await;
    let cursor = first
        .next_token()
        .expect("a walk of the seeded anomalies at one per page must carry a token");
    let other = guru.list(OTHER_INSIGHT_ID, &json!({ "MaxResults": 1 })).await;
    let other_cursor = other
        .next_token()
        .expect("a walk of the other insight's anomalies must carry a token");

    // (case, the insight the token is replayed against, the body it is replayed in)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "another insight's token",
            OTHER_INSIGHT_ID,
            json!({ "MaxResults": 1, "NextToken": cursor }),
        ),
        (
            "this insight replayed with another insight's token",
            INSIGHT_ID,
            json!({ "MaxResults": 1, "NextToken": other_cursor }),
        ),
        (
            "a token issued for another selection's window",
            INSIGHT_ID,
            json!({
                "MaxResults": 1,
                "NextToken": cursor,
                "StartTimeRange": { "FromTime": seconds(BASE_MILLIS) },
            }),
        ),
        (
            "a token issued for another selection's services",
            INSIGHT_ID,
            json!({
                "MaxResults": 1,
                "NextToken": cursor,
                "Filters": { "ServiceCollection": { "ServiceNames": ["LAMBDA"] } },
            }),
        ),
    ];
    for (case, insight_id, body) in cases {
        let reply = guru.list(insight_id, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }

    // A cursor into a collection that has since shrunk names a page that no longer
    // exists, which is the same refusal rather than a silently empty last page.
    let shrunk = Guru::new();
    shrunk.seed(&rows);
    let page = shrunk.list(INSIGHT_ID, &json!({ "MaxResults": 1 })).await;
    let stale = page
        .next_token()
        .expect("a walk at one per page must carry a token");
    shrunk.store().lock().anomalies.clear();
    let reply = shrunk
        .list(INSIGHT_ID, &json!({ "MaxResults": 1, "NextToken": stale }))
        .await;
    assert_eq!(
        reply.status, 400,
        "case \"a cursor into a collection that has since shrunk\": expected HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_name, "ValidationException",
        "case \"a cursor into a collection that has since shrunk\": expected ValidationException, got {reply:?}"
    );
}

/// RULE: an `InsightId` the store holds no insight under names no insight and is
/// refused; an insight that exists with no anomaly of its own is empty lists.
#[tokio::test]
async fn an_insight_id_naming_no_insight_is_refused_and_one_with_no_anomaly_is_empty() {
    let guru = Guru::new();
    guru.seed(&rows());
    guru.seed_insight("insight-quiet");

    for case in ["insight-missing", "insight_missing", "0"] {
        let reply = guru.list(case, &json!({})).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceId"),
            Some(case),
            "case {case:?}: the refusal must name the id asked for, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceType"),
            Some(INSIGHT_RESOURCE_TYPE),
            "case {case:?}: the refusal must name the resource type, got {reply:?}"
        );
        assert!(
            reply
                .str_at("/Message")
                .is_some_and(|message| message.contains(case)),
            "case {case:?}: the message must name the id asked for, got {reply:?}"
        );
    }

    let quiet = guru.list("insight-quiet", &json!({})).await;
    assert_eq!(
        quiet.status, 200,
        "case \"an insight with no anomaly of its own\": expected HTTP 200, got {quiet:?}"
    );
    assert_eq!(
        quiet.ids(),
        Vec::<String>::new(),
        "case \"an insight with no anomaly of its own\": expected empty lists, got {quiet:?}"
    );
    assert_eq!(
        quiet.next_token(),
        None,
        "case \"an insight with no anomaly of its own\": expected no page token, got {quiet:?}"
    );
}

/// RULE: an `AccountId` the caller cannot reach is an authorization refusal, decided
/// before any lookup, so it is the same answer whether or not the insight exists.
#[tokio::test]
async fn an_account_the_caller_cannot_reach_is_refused_before_any_lookup() {
    let guru = Guru::new();
    guru.seed(&rows());

    for case in [INSIGHT_ID, "insight-missing"] {
        let reply = guru
            .list(case, &json!({ "AccountId": OTHER_ACCOUNT_ID }))
            .await;
        assert_eq!(
            reply.status, 403,
            "case {case:?}: expected HTTP 403, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {reply:?}"
        );
        assert!(
            reply
                .str_at("/Message")
                .is_some_and(|message| message.contains(OTHER_ACCOUNT_ID)),
            "case {case:?}: the refusal must name the account asked for, got {reply:?}"
        );
    }

    // The caller's OWN account, named explicitly, is served exactly as naming none is.
    let own = engine::emulator_account_id().to_string();
    let named = guru
        .list(INSIGHT_ID, &json!({ "AccountId": own, "MaxResults": 500 }))
        .await;
    let absent = guru.list(INSIGHT_ID, &json!({ "MaxResults": 500 })).await;
    assert_eq!(
        named.status, 200,
        "case \"the caller's own account\": expected HTTP 200, got {named:?}"
    );
    assert_eq!(
        named.ids(),
        absent.ids(),
        "case \"the caller's own account\": expected the same anomalies as naming no account, got {named:?}"
    );
}

/// RULE: it reports — the store is identical before and after, however often it is read.
#[tokio::test]
async fn the_read_changes_nothing_a_later_caller_sees() {
    let guru = Guru::new();
    guru.seed(&rows());
    let before = guru.snapshot();

    let reads: Vec<(&str, &str, Value)> = vec![
        ("no filters", INSIGHT_ID, json!({})),
        ("one page", INSIGHT_ID, json!({ "MaxResults": 1 })),
        (
            "a window and a service filter",
            INSIGHT_ID,
            json!({
                "StartTimeRange": { "FromTime": seconds(BASE_MILLIS) },
                "Filters": { "ServiceCollection": { "ServiceNames": ["LAMBDA"] } },
            }),
        ),
        ("an insight that does not exist", "insight-missing", json!({})),
        (
            "an account the caller cannot reach",
            INSIGHT_ID,
            json!({ "AccountId": OTHER_ACCOUNT_ID }),
        ),
    ];
    for (case, insight_id, body) in &reads {
        for attempt in 0..2 {
            let reply = guru.list(insight_id, body).await;
            assert!(
                [200, 403, 404].contains(&reply.status),
                "case {case:?} attempt {attempt}: expected a report or a refusal, got {reply:?}"
            );
            assert_eq!(
                guru.snapshot(),
                before,
                "case {case:?} attempt {attempt}: a read must not change the store"
            );
        }
    }
}
