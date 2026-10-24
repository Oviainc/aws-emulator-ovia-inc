// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAnomaly`, the anomaly detail read.
//!
//! The RULES these tests pin:
//!
//! * `DescribeAnomaly` answers the anomaly the store holds under the requested `Id`,
//!   rendered into the response member its kind selects (`ProactiveAnomaly` or
//!   `ReactiveAnomaly`, never both), reporting exactly what the record holds and only
//!   the members that member's shape models.
//! * An `Id` the store does not hold names no anomaly and is refused with
//!   `ResourceNotFoundException` (404), naming the id asked for.
//! * An `AccountId` the caller cannot reach is an authorization refusal
//!   (`AccessDeniedException`, 403), decided before any lookup, so it is the same
//!   answer whether or not the anomaly exists; an absent `AccountId` is the caller's
//!   own account.
//! * It reports: the store is identical before and after, however often it is read.
//! * `DeleteInsight` deletes "the insight along with the associated anomalies", so an
//!   anomaly whose `AssociatedInsightId` names the deleted insight is gone afterwards
//!   while every other anomaly is untouched.
//!
//! DevOps Guru detects anomalies through its own analysis, so no client request
//! creates one; these tests therefore write records straight into the engine's store
//! (the same store the handler reads through `crate::state::service_state`) and then
//! drive the operation over the wire through the same [`MultiServiceRouter`] the
//! emulator boots, so every assertion is about wire behaviour.
//!
//! Each test exercises its rule over a SET of inputs that differ along what the rule
//! generalises over: anomaly kind, severity, status, reactive anomaly type, the form
//! of the anomaly's resource collection, the metric statistic its source details
//! carry, how many resources it names, the presence of each optional member, the
//! shape of the requested id, and the account the read is scoped to.

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    AnomalyResource, AnomalySourceDetails, AnomalySourceMetadata, CloudWatchMetricsDetail,
    ANOMALY_SEVERITY_VALUES, ANOMALY_STATUS_VALUES, ANOMALY_TYPE_VALUES,
    CLOUD_WATCH_METRICS_STAT_VALUES,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    AnomalyRecord, ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord,
    ResourceCoverage, ANOMALY_RESOURCE_TYPE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A twelve-digit account id other than the emulator's own
/// (`engine::emulator_account_id`), as `AwsAccountId`'s `^\d{12}$` pattern requires.
/// This emulator is in no organization, so this account is not reachable from it.
const OTHER_ACCOUNT_ID: &str = "111122223333";

/// A base instant (epoch milliseconds) the seeded anomalies are placed around.
const BASE_MILLIS: i64 = 1_700_000_000_123;

/// One hour in milliseconds, the unit the seeded anomaly windows are spaced by.
const HOUR_MILLIS: i64 = 3_600_000;

/// `AnomalyStatus`'s "the behaviour ended" member — see `ANOMALY_STATUS_VALUES`.
const STATUS_CLOSED: &str = "CLOSED";

/// One reply, as a client sees it.
struct Reply {
    status: u16,
    body: Value,
    /// The error SHAPE NAME the reply carries, from the `x-amzn-errortype` header or
    /// the body's `__type`, with any namespace/URL prefix stripped. Empty on success.
    error_name: String,
    raw: String,
}

impl Reply {
    fn at(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer)
    }

    fn str_at(&self, pointer: &str) -> Option<&str> {
        self.at(pointer).and_then(Value::as_str)
    }

    fn f64_at(&self, pointer: &str) -> Option<f64> {
        self.at(pointer).and_then(Value::as_f64)
    }

    fn strings_at(&self, pointer: &str) -> Vec<String> {
        self.at(pointer)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A modelled `Timestamp` member read back as epoch milliseconds. restJson1
    /// renders a timestamp as epoch seconds, so the millisecond part comes back as
    /// the fraction.
    fn millis_at(&self, pointer: &str) -> Option<i64> {
        let seconds = self.at(pointer).and_then(Value::as_f64)?;
        Some((seconds * 1000.0).round() as i64)
    }
}

/// A fresh emulator holding only this service, over an engine the test keeps a
/// handle to so it can seed the anomalies DevOps Guru's analysis would have detected.
///
/// Its own harness rather than a shared one: the suites in this crate each build the
/// router they need, and there is no shared harness module to reuse yet.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
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

    /// The very store the handler reads: `state::<T>()` is keyed by type, so this is
    /// the same value `crate::state::service_state` hands a request.
    fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState> {
        self.engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>()
    }

    fn seed(&self, record: AnomalyRecord) {
        self.store()
            .lock()
            .anomalies
            .insert(record.id.clone(), record);
    }

    fn seed_insight(&self, id: &str) {
        let record = InsightRecord {
            id: id.to_string(),
            name: format!("insight {id}"),
            insight_type: InsightRecord::PROACTIVE.to_string(),
            severity: "HIGH".to_string(),
            status: "ONGOING".to_string(),
            start_time_millis: BASE_MILLIS,
            ..InsightRecord::default()
        };
        self.store().lock().insights.insert(id.to_string(), record);
    }

    fn anomaly_ids(&self) -> Vec<String> {
        self.store().lock().anomalies.keys().cloned().collect()
    }

    async fn describe_anomaly(&self, id: &str, account_id: Option<&str>) -> Reply {
        let path = match account_id {
            Some(account_id) => format!("/anomalies/{id}?AccountId={account_id}"),
            None => format!("/anomalies/{id}"),
        };
        self.call("GET", &path).await
    }

    async fn delete_insight(&self, id: &str) -> Reply {
        self.call("DELETE", &format!("/insights/{id}")).await
    }

    async fn call(&self, method: &str, path: &str) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", HOST)
            .header("content-type", "application/json")
            .body(Bytes::new())
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
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

/// The forms an anomaly's `ResourceCollection` takes: either half of the coverage
/// shape, both halves, and no collection at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageForm {
    None,
    Stacks,
    Tags,
    Both,
}

impl CoverageForm {
    /// Every form, so a table can walk them.
    const ALL: [CoverageForm; 4] = [
        CoverageForm::None,
        CoverageForm::Stacks,
        CoverageForm::Tags,
        CoverageForm::Both,
    ];

    fn stack_names(self) -> Vec<String> {
        match self {
            CoverageForm::Stacks | CoverageForm::Both => {
                vec!["ops-stack-beta".to_string(), "ops-stack-alpha".to_string()]
            }
            _ => Vec::new(),
        }
    }

    fn tag_values(self) -> Vec<(String, Vec<String>)> {
        match self {
            CoverageForm::Tags | CoverageForm::Both => vec![(
                "DevOps-Guru-app".to_string(),
                vec!["payments".to_string(), "frontend".to_string()],
            )],
            _ => Vec::new(),
        }
    }

    /// The stored coverage, or no coverage at all for [`CoverageForm::None`].
    fn coverage(self) -> Option<ResourceCoverage> {
        if self == CoverageForm::None {
            return None;
        }
        Some(ResourceCoverage {
            cloudformation_stack_names: self.stack_names().into_iter().collect(),
            tag_collections: self
                .tag_values()
                .into_iter()
                .map(|(key, values)| (key, values.into_iter().collect::<BTreeSet<String>>()))
                .collect::<BTreeMap<String, BTreeSet<String>>>(),
        })
    }
}

/// One row of the round-trip table: an anomaly as DevOps Guru's analysis would have
/// detected it.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    proactive: bool,
    severity: &'static str,
    status: &'static str,
    coverage: CoverageForm,
    start_millis: i64,
    end_millis: Option<i64>,
    open_millis: i64,
    close_millis: Option<i64>,
    update_millis: Option<i64>,
    prediction: Option<(i64, Option<i64>)>,
    limit: Option<f64>,
    associated_insight_id: Option<String>,
    anomaly_type: Option<&'static str>,
    name: Option<String>,
    causal_anomaly_id: Option<String>,
    description: Option<String>,
    /// `AnomalyResources`, as `(Name, Type)` pairs.
    resources: Vec<(String, String)>,
    /// `SourceMetadata`, as `(Source, SourceResourceName, SourceResourceType)`.
    source_metadata: Option<(String, String, String)>,
    /// `SourceDetails.CloudWatchMetrics[0]`'s metric name and statistic, when the
    /// anomaly carries metric evidence at all.
    metric: Option<(String, &'static str)>,
}

impl Row {
    fn record(&self) -> AnomalyRecord {
        AnomalyRecord {
            id: self.id.clone(),
            proactive: self.proactive,
            severity: self.severity.to_string(),
            status: self.status.to_string(),
            start_time_millis: self.start_millis,
            end_time_millis: self.end_millis,
            open_time_millis: self.open_millis,
            close_time_millis: self.close_millis,
            update_time_millis: self.update_millis,
            prediction_start_time_millis: self.prediction.map(|(start, _)| start),
            prediction_end_time_millis: self.prediction.and_then(|(_, end)| end),
            limit: self.limit,
            associated_insight_id: self.associated_insight_id.clone(),
            resource_collection: self.coverage.coverage(),
            source_details: self.metric.as_ref().map(|(metric_name, stat)| {
                AnomalySourceDetails {
                    cloud_watch_metrics: Some(vec![CloudWatchMetricsDetail {
                        metric_name: Some(metric_name.clone()),
                        namespace: Some("AWS/Lambda".to_string()),
                        stat: Some((*stat).to_string()),
                        unit: Some("Count".to_string()),
                        period: 60,
                        ..CloudWatchMetricsDetail::default()
                    }]),
                    performance_insights_metrics: None,
                }
            }),
            source_metadata: self.source_metadata.as_ref().map(
                |(source, resource_name, resource_type)| AnomalySourceMetadata {
                    source: Some(source.clone()),
                    source_resource_name: Some(resource_name.clone()),
                    source_resource_type: Some(resource_type.clone()),
                },
            ),
            anomaly_resources: self
                .resources
                .iter()
                .map(|(name, resource_type)| AnomalyResource {
                    name: Some(name.clone()),
                    r#type: Some(resource_type.clone()),
                })
                .collect(),
            anomaly_type: self.anomaly_type.map(str::to_string),
            name: self.name.clone(),
            causal_anomaly_id: self.causal_anomaly_id.clone(),
            description: self.description.clone(),
        }
    }

    /// The response member this row's kind selects.
    fn member(&self) -> &'static str {
        if self.proactive {
            "ProactiveAnomaly"
        } else {
            "ReactiveAnomaly"
        }
    }

    /// The member it does NOT select, which must stay absent.
    fn other_member(&self) -> &'static str {
        if self.proactive {
            "ReactiveAnomaly"
        } else {
            "ProactiveAnomaly"
        }
    }
}

/// Every combination of anomaly kind with the model's `AnomalySeverity` and
/// `AnomalyStatus` catalogs (`ANOMALY_SEVERITY_VALUES`, `ANOMALY_STATUS_VALUES` in
/// `ops/types/shared_types.rs`, generated from the model's enums), walking the
/// `AnomalyType` catalog, the resource-collection forms, the
/// `CLOUD_WATCH_METRICS_STAT_VALUES` catalog and the optional members alongside them.
fn table() -> Vec<Row> {
    let mut rows = Vec::new();
    for proactive in [true, false] {
        for (s, severity) in ANOMALY_SEVERITY_VALUES.iter().enumerate() {
            for (c, status) in ANOMALY_STATUS_VALUES.iter().enumerate() {
                let index = rows.len() as i64;
                let coverage = CoverageForm::ALL[(s + c) % CoverageForm::ALL.len()];
                let start_millis = BASE_MILLIS + index * 60_000;
                // A window has no end, and a reported range no close instant, until
                // the anomaly closes.
                let closed = *status == STATUS_CLOSED;
                let end_millis = closed.then_some(start_millis + HOUR_MILLIS);
                let open_millis = start_millis + 30_000;
                let close_millis = closed.then_some(start_millis + HOUR_MILLIS + 30_000);
                // Only a proactive anomaly predicts a window, carries a threshold it
                // expects to be crossed, an update instant and source metadata;
                // `ReactiveAnomaly` models none of those members.
                let prediction = proactive.then_some((
                    start_millis + 300_000,
                    closed.then_some(start_millis + 2 * HOUR_MILLIS),
                ));
                // Only a reactive anomaly carries a type, a name and the id of the
                // causal anomaly it elaborates; `ProactiveAnomaly` models none of them.
                let anomaly_type =
                    (!proactive).then(|| ANOMALY_TYPE_VALUES[index as usize % ANOMALY_TYPE_VALUES.len()]);
                let kind = if proactive { "proactive" } else { "reactive" };
                rows.push(Row {
                    // `AnomalyId` is len 1..100 matching `^[\w~.-]*$`, so `~`, `.` and
                    // `-` all belong in a well-formed id.
                    id: format!("anomaly~{kind}.{severity}-{status}"),
                    proactive,
                    severity,
                    status,
                    coverage,
                    start_millis,
                    end_millis,
                    open_millis,
                    close_millis,
                    update_millis: proactive.then_some(open_millis + 120_000),
                    prediction,
                    limit: proactive.then_some(42.5 + index as f64),
                    // An anomaly belongs to an insight only once the analysis has
                    // associated it with one.
                    associated_insight_id: (index % 2 == 0)
                        .then(|| format!("insight-{index:04}")),
                    anomaly_type,
                    name: (!proactive).then(|| format!("{severity} reactive anomaly {index}")),
                    // A `CAUSAL` anomaly's causal id "is always NULL"; a contextual
                    // one names the anomaly it elaborates.
                    causal_anomaly_id: (anomaly_type == Some("CONTEXTUAL"))
                        .then(|| format!("anomaly~causal.{index}")),
                    description: (index % 3 != 0)
                        .then(|| format!("{severity} {kind} anomaly {index}")),
                    // Walk the resource list length too: an anomaly names none, one or
                    // two resources.
                    resources: (0..(index as usize % 3))
                        .map(|position| {
                            (
                                format!("resource-{index}-{position}"),
                                "AWS::Lambda::Function".to_string(),
                            )
                        })
                        .collect(),
                    source_metadata: proactive.then(|| {
                        (
                            "CW_METRICS".to_string(),
                            format!("source-resource-{index}"),
                            "AWS::Lambda::Function".to_string(),
                        )
                    }),
                    metric: (index % 4 != 3).then(|| {
                        (
                            format!("Invocations{index}"),
                            CLOUD_WATCH_METRICS_STAT_VALUES
                                [index as usize % CLOUD_WATCH_METRICS_STAT_VALUES.len()],
                        )
                    }),
                });
            }
        }
    }
    rows
}

/// RULE: every stored anomaly reads back through `DescribeAnomaly` on the member its
/// kind selects, with each member reporting exactly what the store holds.
#[tokio::test]
async fn stored_anomalies_read_back_on_the_member_their_kind_selects() {
    let emulator = Emulator::new();
    let rows = table();
    assert_eq!(
        rows.len(),
        2 * ANOMALY_SEVERITY_VALUES.len() * ANOMALY_STATUS_VALUES.len(),
        "the table must cover both anomaly kinds over every severity/status combination \
         the model declares"
    );
    for row in &rows {
        emulator.seed(row.record());
    }

    for row in &rows {
        let case = &row.id;
        let reply = emulator.describe_anomaly(&row.id, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected HTTP 200, got {} ({})",
            reply.status, reply.raw
        );

        let member = format!("/{}", row.member());
        assert!(
            reply.at(&member).is_some(),
            "case {case}: expected the anomaly on {member}, got {}",
            reply.raw
        );
        assert!(
            reply.at(&format!("/{}", row.other_member())).is_none(),
            "case {case}: {} must stay absent, got {}",
            row.other_member(),
            reply.raw
        );

        assert_eq!(
            reply.str_at(&format!("{member}/Id")),
            Some(row.id.as_str()),
            "case {case}: expected Id {}, got {}",
            row.id,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/Severity")),
            Some(row.severity),
            "case {case}: expected Severity {}, got {}",
            row.severity,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/Status")),
            Some(row.status),
            "case {case}: expected Status {}, got {}",
            row.status,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/AssociatedInsightId")),
            row.associated_insight_id.as_deref(),
            "case {case}: expected AssociatedInsightId {:?}, got {}",
            row.associated_insight_id,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/Description")),
            row.description.as_deref(),
            "case {case}: expected Description {:?}, got {}",
            row.description,
            reply.raw
        );

        // The anomalous behaviour's own window: start always, end only once it ended.
        assert_eq!(
            reply.millis_at(&format!("{member}/AnomalyTimeRange/StartTime")),
            Some(row.start_millis),
            "case {case}: expected AnomalyTimeRange.StartTime {}, got {}",
            row.start_millis,
            reply.raw
        );
        assert_eq!(
            reply.millis_at(&format!("{member}/AnomalyTimeRange/EndTime")),
            row.end_millis,
            "case {case}: expected AnomalyTimeRange.EndTime {:?}, got {}",
            row.end_millis,
            reply.raw
        );
        // The range over which DevOps Guru itself held the anomaly open, which is a
        // different range from the behaviour's own.
        assert_eq!(
            reply.millis_at(&format!("{member}/AnomalyReportedTimeRange/OpenTime")),
            Some(row.open_millis),
            "case {case}: expected AnomalyReportedTimeRange.OpenTime {}, got {}",
            row.open_millis,
            reply.raw
        );
        assert_eq!(
            reply.millis_at(&format!("{member}/AnomalyReportedTimeRange/CloseTime")),
            row.close_millis,
            "case {case}: expected AnomalyReportedTimeRange.CloseTime {:?}, got {}",
            row.close_millis,
            reply.raw
        );

        // The members only ONE of the two shapes models are reported on that shape and
        // nowhere else.
        assert_eq!(
            reply.millis_at(&format!("{member}/UpdateTime")),
            row.update_millis,
            "case {case}: expected UpdateTime {:?}, got {}",
            row.update_millis,
            reply.raw
        );
        assert_eq!(
            reply.millis_at(&format!("{member}/PredictionTimeRange/StartTime")),
            row.prediction.map(|(start, _)| start),
            "case {case}: expected PredictionTimeRange.StartTime {:?}, got {}",
            row.prediction.map(|(start, _)| start),
            reply.raw
        );
        assert_eq!(
            reply.millis_at(&format!("{member}/PredictionTimeRange/EndTime")),
            row.prediction.and_then(|(_, end)| end),
            "case {case}: expected PredictionTimeRange.EndTime {:?}, got {}",
            row.prediction.and_then(|(_, end)| end),
            reply.raw
        );
        assert_eq!(
            reply.f64_at(&format!("{member}/Limit")),
            row.limit,
            "case {case}: expected Limit {:?}, got {}",
            row.limit,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/Type")),
            row.anomaly_type,
            "case {case}: expected Type {:?}, got {}",
            row.anomaly_type,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/Name")),
            row.name.as_deref(),
            "case {case}: expected Name {:?}, got {}",
            row.name,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/CausalAnomalyId")),
            row.causal_anomaly_id.as_deref(),
            "case {case}: expected CausalAnomalyId {:?}, got {}",
            row.causal_anomaly_id,
            reply.raw
        );

        // The source metadata, and the metric evidence the analysis attached.
        let expected_metadata = row.source_metadata.clone();
        assert_eq!(
            reply.str_at(&format!("{member}/SourceMetadata/Source")),
            expected_metadata.as_ref().map(|(source, _, _)| source.as_str()),
            "case {case}: expected SourceMetadata.Source {:?}, got {}",
            expected_metadata.as_ref().map(|(source, _, _)| source),
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/SourceMetadata/SourceResourceName")),
            expected_metadata.as_ref().map(|(_, name, _)| name.as_str()),
            "case {case}: expected SourceMetadata.SourceResourceName {:?}, got {}",
            expected_metadata.as_ref().map(|(_, name, _)| name),
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/SourceDetails/CloudWatchMetrics/0/MetricName")),
            row.metric.as_ref().map(|(metric_name, _)| metric_name.as_str()),
            "case {case}: expected the stored metric name {:?}, got {}",
            row.metric.as_ref().map(|(metric_name, _)| metric_name),
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/SourceDetails/CloudWatchMetrics/0/Stat")),
            row.metric.as_ref().map(|(_, stat)| *stat),
            "case {case}: expected the stored metric statistic {:?}, got {}",
            row.metric.as_ref().map(|(_, stat)| *stat),
            reply.raw
        );

        // The resources the anomalous behaviour was detected in, in the stored order;
        // an anomaly naming none reports no list at all.
        assert_eq!(
            reply
                .at(&format!("{member}/AnomalyResources"))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default(),
            row.resources.len(),
            "case {case}: expected {} anomaly resource(s), got {}",
            row.resources.len(),
            reply.raw
        );
        for (position, (name, resource_type)) in row.resources.iter().enumerate() {
            let resource = format!("{member}/AnomalyResources/{position}");
            assert_eq!(
                reply.str_at(&format!("{resource}/Name")),
                Some(name.as_str()),
                "case {case}: expected AnomalyResources[{position}].Name {name}, got {}",
                reply.raw
            );
            assert_eq!(
                reply.str_at(&format!("{resource}/Type")),
                Some(resource_type.as_str()),
                "case {case}: expected AnomalyResources[{position}].Type {resource_type}, got {}",
                reply.raw
            );
        }

        // The resources the anomaly is about, reported once each in a stable order; an
        // anomaly over no collection reports none at all.
        let mut expected_stacks = row.coverage.stack_names();
        expected_stacks.sort();
        assert_eq!(
            reply.strings_at(&format!("{member}/ResourceCollection/CloudFormation/StackNames")),
            expected_stacks,
            "case {case}: expected stack names {expected_stacks:?}, got {}",
            reply.raw
        );
        let expected_tags = row.coverage.tag_values();
        assert_eq!(
            reply
                .at(&format!("{member}/ResourceCollection/Tags"))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default(),
            expected_tags.len(),
            "case {case}: expected {} tag collection(s), got {}",
            expected_tags.len(),
            reply.raw
        );
        for (position, (key, values)) in expected_tags.iter().enumerate() {
            let tag = format!("{member}/ResourceCollection/Tags/{position}");
            assert_eq!(
                reply.str_at(&format!("{tag}/AppBoundaryKey")),
                Some(key.as_str()),
                "case {case}: expected AppBoundaryKey {key}, got {}",
                reply.raw
            );
            let mut expected_values = values.clone();
            expected_values.sort();
            assert_eq!(
                reply.strings_at(&format!("{tag}/TagValues")),
                expected_values,
                "case {case}: expected TagValues {expected_values:?}, got {}",
                reply.raw
            );
        }
        if row.coverage == CoverageForm::None {
            assert!(
                reply.at(&format!("{member}/ResourceCollection")).is_none(),
                "case {case}: an anomaly over no resource collection must report none, got {}",
                reply.raw
            );
        }
    }
}

/// RULE: a member the selected response shape does not model is not reported, however
/// much the record holds — the record is rendered into the shape its kind selects, and
/// that shape's member set is the model's.
#[tokio::test]
async fn only_the_members_the_selected_shape_models_are_reported() {
    let emulator = Emulator::new();
    // One record carrying EVERY member either shape models, stored once as a
    // proactive anomaly and once as a reactive one, so the only difference between
    // the two answers is the shape each is rendered into.
    let mut everything = table()
        .into_iter()
        .find(|row| row.proactive)
        .expect("the table holds a proactive row")
        .record();
    everything.anomaly_type = Some(ANOMALY_TYPE_VALUES[1].to_string());
    everything.name = Some("an anomaly with every member".to_string());
    everything.causal_anomaly_id = Some("anomaly~causal.0".to_string());

    // (the id, the kind, the members this kind's shape models, the ones it does not)
    let cases: Vec<(&str, bool, &str, Vec<&str>)> = vec![
        (
            "anomaly~every-member.proactive",
            true,
            "ProactiveAnomaly",
            vec!["Type", "Name", "CausalAnomalyId"],
        ),
        (
            "anomaly~every-member.reactive",
            false,
            "ReactiveAnomaly",
            vec!["UpdateTime", "PredictionTimeRange", "Limit", "SourceMetadata"],
        ),
    ];

    for (id, proactive, member, unmodelled) in &cases {
        let mut record = everything.clone();
        record.id = (*id).to_string();
        record.proactive = *proactive;
        emulator.seed(record);

        let reply = emulator.describe_anomaly(id, None).await;
        assert_eq!(
            reply.status, 200,
            "case {id}: expected HTTP 200, got {} ({})",
            reply.status, reply.raw
        );
        // The members this shape DOES model are all there.
        for modelled in ["Id", "Severity", "Status", "AnomalyTimeRange", "SourceDetails"] {
            assert!(
                reply.at(&format!("/{member}/{modelled}")).is_some(),
                "case {id}: expected {member}.{modelled} to be reported, got {}",
                reply.raw
            );
        }
        for absent in unmodelled {
            assert!(
                reply.at(&format!("/{member}/{absent}")).is_none(),
                "case {id}: {member} models no {absent}, so it must not be reported, got {}",
                reply.raw
            );
        }
    }
}

/// RULE: an `Id` the store does not hold names no anomaly, and is refused with
/// `ResourceNotFoundException` (HTTP 404) naming the id asked for.
#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();
    // One anomaly exists, so the refusals below are about the id asked for and not
    // about an empty store.
    let stored = table().remove(0);
    emulator.seed(stored.record());

    // Ids spanning the shapes `AnomalyId` (len 1..100, `^[\w~.-]*$`) admits: one
    // character, a word, a uuid-shaped id, one using each punctuation character the
    // pattern allows, an id differing from the stored one by a single character, and
    // the 100-character maximum.
    let missing_ids = vec![
        "x".to_string(),
        "no_such_anomaly".to_string(),
        "4f8bb0d2-1a3f-4c21-9fb1-6a9a51f3dd77".to_string(),
        "a~b.c-d".to_string(),
        format!("{}0", stored.id),
        "a".repeat(100),
    ];

    for id in &missing_ids {
        let before = emulator.anomaly_ids();
        let reply = emulator.describe_anomaly(id, None).await;
        assert_eq!(
            reply.status, 404,
            "case {id}: expected HTTP 404, got {} ({})",
            reply.status, reply.raw
        );
        assert_eq!(
            reply.error_name, "ResourceNotFoundException",
            "case {id}: expected ResourceNotFoundException, got {}",
            reply.raw
        );
        assert_eq!(
            reply.str_at("/ResourceId"),
            Some(id.as_str()),
            "case {id}: the refusal must name the requested id, got {}",
            reply.raw
        );
        assert_eq!(
            reply.str_at("/ResourceType"),
            Some(ANOMALY_RESOURCE_TYPE),
            "case {id}: expected ResourceType {ANOMALY_RESOURCE_TYPE}, got {}",
            reply.raw
        );
        assert!(
            reply
                .str_at("/Message")
                .is_some_and(|message| !message.is_empty()),
            "case {id}: the refusal must carry a Message, got {}",
            reply.raw
        );
        assert_eq!(
            emulator.anomaly_ids(),
            before,
            "case {id}: a refused read must not change the store"
        );
    }
}

/// RULE: a read scoped to an account the caller cannot reach is an authorization
/// refusal (`AccessDeniedException`, HTTP 403), decided before any lookup, so it is
/// the same answer whether the anomaly exists or not; the caller's own account is
/// what an absent `AccountId` means.
#[tokio::test]
async fn an_account_the_caller_cannot_reach_is_refused_as_access_denied() {
    let emulator = Emulator::new();
    let own_account = ::engine::emulator_account_id().to_string();
    let stored = table().remove(0);
    emulator.seed(stored.record());

    // (requested id, requested AccountId, expected status)
    let cases: Vec<(&str, Option<&str>, u16)> = vec![
        // The caller's own account, named and unnamed, resolves the anomaly.
        (&stored.id, None, 200),
        (&stored.id, Some(&own_account), 200),
        // Any other account is unreachable, and the answer does not depend on
        // whether the id names a stored anomaly.
        (&stored.id, Some(OTHER_ACCOUNT_ID), 403),
        ("no_such_anomaly", Some(OTHER_ACCOUNT_ID), 403),
        (&stored.id, Some("999988887777"), 403),
        ("no_such_anomaly", Some("999988887777"), 403),
        // An id that is not stored is still a not-found in the caller's own account.
        ("no_such_anomaly", None, 404),
        ("no_such_anomaly", Some(&own_account), 404),
    ];

    for (id, account_id, expected) in cases {
        let reply = emulator.describe_anomaly(id, account_id).await;
        assert_eq!(
            reply.status, expected,
            "case {id:?}/{account_id:?}: expected HTTP {expected}, got {} ({})",
            reply.status, reply.raw
        );
        match expected {
            200 => assert_eq!(
                reply.str_at(&format!("/{}/Id", stored.member())),
                Some(id),
                "case {id:?}/{account_id:?}: expected the anomaly asked for, got {}",
                reply.raw
            ),
            403 => assert_eq!(
                reply.error_name, "AccessDeniedException",
                "case {id:?}/{account_id:?}: expected AccessDeniedException, got {}",
                reply.raw
            ),
            _ => assert_eq!(
                reply.error_name, "ResourceNotFoundException",
                "case {id:?}/{account_id:?}: expected ResourceNotFoundException, got {}",
                reply.raw
            ),
        }
    }
}

/// RULE: `DescribeAnomaly` reports, so what one caller reads is what the next one
/// reads — the store is identical before and after, however often it is read.
#[tokio::test]
async fn describing_an_anomaly_leaves_the_store_unchanged() {
    let emulator = Emulator::new();
    let rows = table();
    for row in &rows {
        emulator.seed(row.record());
    }
    let before = format!("{:?}", emulator.store().lock().anomalies);

    for row in &rows {
        let case = &row.id;
        let first = emulator.describe_anomaly(&row.id, None).await;
        let second = emulator.describe_anomaly(&row.id, None).await;
        let third = emulator.describe_anomaly(&row.id, None).await;
        assert_eq!(
            first.body, second.body,
            "case {case}: a second read must answer the first read's body, got {}",
            second.raw
        );
        assert_eq!(
            second.body, third.body,
            "case {case}: a third read must answer the same body, got {}",
            third.raw
        );
    }

    // And neither a read of an id that is not there nor a refused cross-account read
    // may mint one.
    let absent = emulator.describe_anomaly("not-a-stored-anomaly", None).await;
    assert_eq!(
        absent.status, 404,
        "a read of an absent id must refuse, got {}",
        absent.raw
    );
    let refused = emulator
        .describe_anomaly("not-a-stored-anomaly", Some(OTHER_ACCOUNT_ID))
        .await;
    assert_eq!(
        refused.status, 403,
        "a read of an unreachable account must refuse, got {}",
        refused.raw
    );

    assert_eq!(
        format!("{:?}", emulator.store().lock().anomalies),
        before,
        "reading anomalies must not change the store"
    );
}

/// RULE: deleting an insight deletes the anomalies associated with THAT insight —
/// `DeleteInsight` "deletes the insight along with the associated anomalies" — and
/// leaves every other anomaly readable.
#[tokio::test]
async fn deleting_an_insight_deletes_the_anomalies_associated_with_it() {
    let emulator = Emulator::new();
    let template = table().remove(0);

    // Anomalies spanning the association forms: two belonging to the insight being
    // deleted, one to another insight, and one to no insight at all.
    let deleted_insight = "insight-deleted";
    let other_insight = "insight-kept";
    emulator.seed_insight(deleted_insight);
    emulator.seed_insight(other_insight);

    // (anomaly id, the insight it belongs to, whether the cascade spares it: an
    // anomaly survives exactly while it does not belong to the deleted insight)
    let cases: Vec<(&str, Option<&str>, bool)> = [
        ("anomaly~assoc.first", Some(deleted_insight)),
        ("anomaly~assoc.second", Some(deleted_insight)),
        ("anomaly~assoc.other", Some(other_insight)),
        ("anomaly~assoc.none", None),
    ]
    .into_iter()
    .map(|(id, insight)| (id, insight, insight != Some(deleted_insight)))
    .collect();

    for (id, insight, _) in &cases {
        let mut record = template.record();
        record.id = (*id).to_string();
        record.associated_insight_id = insight.map(str::to_string);
        emulator.seed(record);
    }

    // Every anomaly resolves before the delete, whichever insight it belongs to.
    for (id, insight, _) in &cases {
        let reply = emulator.describe_anomaly(id, None).await;
        assert_eq!(
            reply.status, 200,
            "case {id:?} (insight {insight:?}): expected HTTP 200 before the delete, got {} ({})",
            reply.status, reply.raw
        );
    }

    let deleted = emulator.delete_insight(deleted_insight).await;
    assert_eq!(
        deleted.status, 200,
        "deleting the insight must succeed, got {} ({})",
        deleted.status, deleted.raw
    );

    for (id, insight, spared) in &cases {
        let reply = emulator.describe_anomaly(id, None).await;
        let expected = if *spared { 200 } else { 404 };
        assert_eq!(
            reply.status, expected,
            "case {id:?} (insight {insight:?}): expected HTTP {expected} after deleting \
             {deleted_insight}, got {} ({})",
            reply.status, reply.raw
        );
    }

    let surviving = emulator.anomaly_ids();
    let mut expected_surviving: Vec<String> = cases
        .iter()
        .filter(|(_, _, spared)| *spared)
        .map(|(id, _, _)| (*id).to_string())
        .collect();
    // The store keys anomalies in a stable sorted order, which is what it reports.
    expected_surviving.sort();
    assert_eq!(
        surviving, expected_surviving,
        "expected exactly the anomalies of other insights to survive, got {surviving:?}"
    );
}
