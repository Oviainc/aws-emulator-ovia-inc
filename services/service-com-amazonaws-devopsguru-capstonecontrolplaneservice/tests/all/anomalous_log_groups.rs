// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAnomalousLogGroups`, the log-anomaly read.
//!
//! The RULES these tests pin:
//!
//! * The read reports the anomalous log groups the store holds FOR the requested
//!   insight, each rendered with exactly the members `AnomalousLogGroup` models and
//!   the values the record holds, in the store's stable `LogGroupName` order, and
//!   echoes the `InsightId` it was asked about.
//! * An `InsightId` the store holds no insight under names no insight and is refused
//!   with `ResourceNotFoundException` (404), naming the id asked for; an insight that
//!   exists with no anomalous log group of its own is an empty list, not a refusal.
//! * A walk pages at the size the caller asked for: no page exceeds it, and the pages
//!   concatenate to the whole collection exactly once, with no token on the last page.
//! * A page token names an offset into ONE insight's log groups, so another insight's
//!   token, and a token whose page no longer exists, are refused with
//!   `ValidationException` (400) rather than resolving to some other page.
//! * It reports: the store is identical before and after, however often it is read.
//! * `DeleteInsight` deletes the insight "along with the associated anomalies", so the
//!   log groups its log anomalies were found in go with it.
//!
//! DevOps Guru finds log anomalies through its own analysis of the covered resources'
//! CloudWatch logs, so no client request creates one; these tests therefore write
//! records straight into the engine's store (the same store the handler reads through
//! `crate::state::service_state`) and then drive the operation over the wire through
//! the same [`MultiServiceRouter`] the emulator boots.
//!
//! Each test exercises its rule over a SET of inputs differing along what the rule
//! generalises over: how many log groups an insight has, whether the impact window is
//! closed, how many log lines were scanned, how many showcases a log group carries and
//! how many classes each showcase has, which `LogAnomalyType` a class reports, which
//! optional class members are present, the requested page size, and the shape of the
//! requested insight id.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    LogAnomalyClass, LogAnomalyShowcase, LOG_ANOMALY_TYPE_VALUES,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    AnomalousLogGroupRecord, ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord,
    PAGE_SIZE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A base instant (epoch milliseconds) the seeded impact windows are placed around.
const BASE_MILLIS: i64 = 1_700_000_000_123;

/// One hour in milliseconds, the unit the seeded windows are spaced by.
const HOUR_MILLIS: i64 = 3_600_000;

/// The `ResourceType` an insight-shaped `ResourceNotFoundException` reports.
const INSIGHT_RESOURCE_TYPE: &str = "INSIGHT";

/// One reply, as a client sees it.
struct Reply {
    status: u16,
    body: Value,
    /// The error SHAPE NAME the reply carries, from the `x-amzn-errortype` header or
    /// the body's `__type`, with any namespace/URL prefix stripped. Empty on success.
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

    /// A modelled `Timestamp` member read back as epoch milliseconds. restJson1
    /// renders a timestamp as epoch seconds, so the millisecond part comes back as the
    /// fraction.
    fn millis_at(&self, pointer: &str) -> Option<i64> {
        let seconds = self.at(pointer).and_then(Value::as_f64)?;
        Some((seconds * 1000.0).round() as i64)
    }

    /// The reported `AnomalousLogGroups`, in the order the reply listed them.
    fn log_groups(&self) -> Vec<Value> {
        self.at("/AnomalousLogGroups")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The reported `LogGroupName`s, in the order the reply listed them.
    fn log_group_names(&self) -> Vec<String> {
        self.log_groups()
            .iter()
            .map(|group| {
                group
                    .get("LogGroupName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    fn next_token(&self) -> Option<String> {
        self.str_at("/NextToken").map(str::to_string)
    }
}

/// A fresh emulator holding only this service, over an engine the test keeps a handle
/// to so it can seed what DevOps Guru's analysis would have written.
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

    /// The very store the handler reads: `state::<T>()` is keyed by type, so this is
    /// the same value `crate::state::service_state` hands a request.
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

    /// Seed the anomalous log groups the log-anomaly analysis would have written for
    /// `insight_id`, and the insight itself.
    fn seed(&self, insight_id: &str, records: Vec<AnomalousLogGroupRecord>) {
        self.seed_insight(insight_id);
        let state = self.store();
        let mut store = state.lock();
        let per_log_group = store
            .anomalous_log_groups
            .entry(insight_id.to_string())
            .or_default();
        for record in records {
            per_log_group.insert(record.log_group_name.clone(), record);
        }
    }

    /// Drop every anomalous log group of `insight_id`, so a cursor into the earlier
    /// collection names a page that no longer exists.
    fn clear_log_groups(&self, insight_id: &str) {
        self.store().lock().anomalous_log_groups.remove(insight_id);
    }

    /// A stable rendering of the whole store, for the "a read does not write" check.
    fn snapshot(&self) -> String {
        format!("{:?}", *self.store().lock())
    }

    async fn list(&self, insight_id: &str, max_results: Option<i32>, token: Option<&str>) -> Reply {
        let mut body = json!({ "InsightId": insight_id });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("POST", "/list-log-anomalies", &body).await
    }

    async fn delete_insight(&self, insight_id: &str) -> Reply {
        self.call("DELETE", &format!("/insights/{insight_id}"), &Value::Null)
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

/// One anomalous log event class of a seeded showcase.
#[derive(Debug, Clone, Copy)]
struct ClassRow {
    /// A member of `LogAnomalyType` — see `LOG_ANOMALY_TYPE_VALUES` in the service's
    /// `ops/types/shared_types.rs`, which is this catalog's source.
    anomaly_type: &'static str,
    /// Whether the class carries every optional member, or only its type: the model
    /// makes all of them optional, so both forms must round-trip.
    full: bool,
}

impl ClassRow {
    fn record(&self, index: usize) -> LogAnomalyClass {
        if !self.full {
            return LogAnomalyClass {
                log_anomaly_type: Some(self.anomaly_type.to_string()),
                ..LogAnomalyClass::default()
            };
        }
        LogAnomalyClass {
            log_stream_name: Some(format!("stream-{index}")),
            log_anomaly_type: Some(self.anomaly_type.to_string()),
            log_anomaly_token: Some(format!("token-{index}")),
            log_event_id: Some(format!("event-{index}")),
            explanation: Some(format!("explanation {index}")),
            number_of_log_lines_occurrences: 3 * index as i32 + 1,
            log_event_timestamp: Some(::aws_smithy_types::DateTime::from_millis(
                BASE_MILLIS + index as i64 * HOUR_MILLIS,
            )),
        }
    }
}

/// One row of the round-trip table: a log group as the log-anomaly analysis would have
/// reported it.
#[derive(Debug, Clone)]
struct Row {
    log_group_name: &'static str,
    impact_start_millis: Option<i64>,
    impact_end_millis: Option<i64>,
    lines_scanned: i32,
    /// One entry per showcase: the classes that showcase carries. An empty outer list
    /// is a log group the analysis showcased nothing for.
    showcases: Vec<Vec<ClassRow>>,
}

impl Row {
    fn record(&self) -> AnomalousLogGroupRecord {
        AnomalousLogGroupRecord {
            insight_id: String::new(),
            log_group_name: self.log_group_name.to_string(),
            impact_start_time_millis: self.impact_start_millis,
            impact_end_time_millis: self.impact_end_millis,
            number_of_log_lines_scanned: self.lines_scanned,
            log_anomaly_showcases: self
                .showcases
                .iter()
                .map(|classes| LogAnomalyShowcase {
                    log_anomaly_classes: (!classes.is_empty()).then(|| {
                        classes
                            .iter()
                            .enumerate()
                            .map(|(index, class)| class.record(index))
                            .collect()
                    }),
                })
                .collect(),
        }
    }
}

fn record_for(insight_id: &str, row: &Row) -> AnomalousLogGroupRecord {
    AnomalousLogGroupRecord {
        insight_id: insight_id.to_string(),
        ..row.record()
    }
}

/// The round-trip table: log groups differing in how their impact window is bounded,
/// how many log lines were scanned, how many showcases they carry, how many classes
/// each showcase has, which `LogAnomalyType` each class reports and which optional
/// class members it carries. `KEYWORD_TOKEN`, `BLOCK_FORMAT` and `NEW_FIELD_NAME` are
/// types the issue report never cited.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            log_group_name: "/aws/lambda/payments",
            impact_start_millis: Some(BASE_MILLIS),
            impact_end_millis: Some(BASE_MILLIS + HOUR_MILLIS),
            lines_scanned: 4_210,
            showcases: vec![vec![
                ClassRow {
                    anomaly_type: "KEYWORD",
                    full: true,
                },
                ClassRow {
                    anomaly_type: "KEYWORD_TOKEN",
                    full: false,
                },
            ]],
        },
        Row {
            log_group_name: "/aws/ecs/checkout",
            // Still open: the analysis placed no end on the impact.
            impact_start_millis: Some(BASE_MILLIS + 2 * HOUR_MILLIS),
            impact_end_millis: None,
            lines_scanned: 17,
            showcases: vec![
                vec![ClassRow {
                    anomaly_type: "HTTP_CODE",
                    full: true,
                }],
                vec![
                    ClassRow {
                        anomaly_type: "BLOCK_FORMAT",
                        full: true,
                    },
                    ClassRow {
                        anomaly_type: "NUMERICAL_NAN",
                        full: false,
                    },
                    ClassRow {
                        anomaly_type: "NEW_FIELD_NAME",
                        full: true,
                    },
                ],
            ],
        },
        Row {
            // No window, nothing scanned and nothing showcased: every optional member
            // of the log group absent, and the modelled default for the count.
            log_group_name: "/aws/rds/orders",
            impact_start_millis: None,
            impact_end_millis: None,
            lines_scanned: 0,
            showcases: Vec::new(),
        },
        Row {
            // A showcase carrying no class at all, beside a very large scanned count.
            log_group_name: "/aws/apigateway/welcome",
            impact_start_millis: Some(BASE_MILLIS - HOUR_MILLIS),
            impact_end_millis: Some(BASE_MILLIS),
            lines_scanned: 2_147_483_647,
            showcases: vec![Vec::new()],
        },
    ]
}

/// The catalog this suite's class rows are drawn from is the model's own enum.
#[test]
fn seeded_log_anomaly_types_are_modelled_values() {
    for row in round_trip_rows() {
        for classes in &row.showcases {
            for class in classes {
                assert!(
                    LOG_ANOMALY_TYPE_VALUES.contains(&class.anomaly_type),
                    "case {}/{}: {:?} is not a modelled LogAnomalyType, which are {:?}",
                    row.log_group_name,
                    class.anomaly_type,
                    class.anomaly_type,
                    LOG_ANOMALY_TYPE_VALUES
                );
            }
        }
    }
}

/// RULE: the read reports the log groups the store holds for the requested insight,
/// with exactly the values each record holds, in `LogGroupName` order, and echoes the
/// insight id asked about.
#[tokio::test]
async fn reports_each_stored_log_group_of_the_insight() {
    let guru = Guru::new();
    let insight_id = "insight-logs-1";
    let rows = round_trip_rows();
    guru.seed(
        insight_id,
        rows.iter().map(|row| record_for(insight_id, row)).collect(),
    );

    let reply = guru.list(insight_id, None, None).await;
    assert_eq!(
        reply.status, 200,
        "case {insight_id}: expected HTTP 200, got {reply:?}"
    );
    assert_eq!(
        reply.str_at("/InsightId"),
        Some(insight_id),
        "case {insight_id}: expected the requested insight id echoed, got {reply:?}"
    );

    let mut expected_names: Vec<String> = rows
        .iter()
        .map(|row| row.log_group_name.to_string())
        .collect();
    expected_names.sort();
    assert_eq!(
        reply.log_group_names(),
        expected_names,
        "case {insight_id}: expected the stored log groups in LogGroupName order, got {reply:?}"
    );

    for row in &rows {
        let position = reply
            .log_group_names()
            .iter()
            .position(|name| name == row.log_group_name)
            .unwrap_or_else(|| panic!("case {}: not reported, got {reply:?}", row.log_group_name));
        let base = format!("/AnomalousLogGroups/{position}");

        assert_eq!(
            reply.millis_at(&format!("{base}/ImpactStartTime")),
            row.impact_start_millis,
            "case {}: expected ImpactStartTime {:?}, got {reply:?}",
            row.log_group_name,
            row.impact_start_millis
        );
        assert_eq!(
            reply.millis_at(&format!("{base}/ImpactEndTime")),
            row.impact_end_millis,
            "case {}: expected ImpactEndTime {:?}, got {reply:?}",
            row.log_group_name,
            row.impact_end_millis
        );
        assert_eq!(
            reply
                .at(&format!("{base}/NumberOfLogLinesScanned"))
                .and_then(Value::as_i64),
            Some(i64::from(row.lines_scanned)),
            "case {}: expected NumberOfLogLinesScanned {}, got {reply:?}",
            row.log_group_name,
            row.lines_scanned
        );

        let showcases = reply
            .at(&format!("{base}/LogAnomalyShowcases"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            showcases.len(),
            row.showcases.len(),
            "case {}: expected {} showcases, got {reply:?}",
            row.log_group_name,
            row.showcases.len()
        );

        for (showcase_index, classes) in row.showcases.iter().enumerate() {
            let reported: Vec<Value> = showcases[showcase_index]
                .get("LogAnomalyClasses")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                reported.len(),
                classes.len(),
                "case {}/showcase {showcase_index}: expected {} classes, got {reply:?}",
                row.log_group_name,
                classes.len()
            );
            for (class_index, class) in classes.iter().enumerate() {
                let expected = class.record(class_index);
                let actual = &reported[class_index];
                let case = format!(
                    "{}/showcase {showcase_index}/class {class_index}",
                    row.log_group_name
                );
                assert_eq!(
                    actual.get("LogAnomalyType").and_then(Value::as_str),
                    expected.log_anomaly_type.as_deref(),
                    "case {case}: expected LogAnomalyType {:?}, got {actual:?}",
                    expected.log_anomaly_type
                );
                assert_eq!(
                    actual.get("LogStreamName").and_then(Value::as_str),
                    expected.log_stream_name.as_deref(),
                    "case {case}: expected LogStreamName {:?}, got {actual:?}",
                    expected.log_stream_name
                );
                assert_eq!(
                    actual.get("LogAnomalyToken").and_then(Value::as_str),
                    expected.log_anomaly_token.as_deref(),
                    "case {case}: expected LogAnomalyToken {:?}, got {actual:?}",
                    expected.log_anomaly_token
                );
                assert_eq!(
                    actual.get("LogEventId").and_then(Value::as_str),
                    expected.log_event_id.as_deref(),
                    "case {case}: expected LogEventId {:?}, got {actual:?}",
                    expected.log_event_id
                );
                assert_eq!(
                    actual.get("Explanation").and_then(Value::as_str),
                    expected.explanation.as_deref(),
                    "case {case}: expected Explanation {:?}, got {actual:?}",
                    expected.explanation
                );
                assert_eq!(
                    actual
                        .get("NumberOfLogLinesOccurrences")
                        .and_then(Value::as_i64),
                    Some(i64::from(expected.number_of_log_lines_occurrences)),
                    "case {case}: expected NumberOfLogLinesOccurrences {}, got {actual:?}",
                    expected.number_of_log_lines_occurrences
                );
                let reported_event_millis = actual
                    .get("LogEventTimestamp")
                    .and_then(Value::as_f64)
                    .map(|seconds| (seconds * 1000.0).round() as i64);
                let expected_event_millis = expected
                    .log_event_timestamp
                    .as_ref()
                    .and_then(|at| at.to_millis().ok());
                assert_eq!(
                    reported_event_millis, expected_event_millis,
                    "case {case}: expected LogEventTimestamp {expected_event_millis:?}, got {actual:?}"
                );
            }
        }
    }

    assert_eq!(
        reply.next_token(),
        None,
        "case {insight_id}: one page holds every log group, so expected no NextToken, got {reply:?}"
    );
}

/// RULE: an insight the store holds with no anomalous log group of its own is an empty
/// list, while an id no insight is stored under is refused as not found, whatever the
/// shape of the id.
#[tokio::test]
async fn unknown_insight_is_not_found_and_an_empty_one_is_an_empty_list() {
    let guru = Guru::new();
    let known = "insight-logs-known";
    guru.seed(known, vec![record_for(known, &round_trip_rows()[0])]);
    // An insight analysis produced, whose logs it flagged nothing in.
    let quiet = "insight-logs-quiet";
    guru.seed_insight(quiet);

    let reply = guru.list(quiet, None, None).await;
    assert_eq!(
        reply.status, 200,
        "case {quiet}: a stored insight with no anomalous logs is not a refusal, got {reply:?}"
    );
    assert!(
        reply.log_groups().is_empty(),
        "case {quiet}: expected an empty AnomalousLogGroups, got {reply:?}"
    );
    assert_eq!(
        reply.str_at("/InsightId"),
        Some(quiet),
        "case {quiet}: expected the requested insight id echoed, got {reply:?}"
    );
    assert_eq!(
        reply.next_token(),
        None,
        "case {quiet}: an empty collection has no next page, got {reply:?}"
    );

    // Ids differing in length, character class and case — each within `InsightId`'s
    // `^[\w-]*$` pattern and 1..100 length, so each reaches the handler.
    for missing in [
        "a",
        "insight-logs-unknown",
        "INSIGHT_LOGS_9",
        &"i".repeat(100),
        &format!("{known}-suffix"),
    ] {
        let reply = guru.list(missing, None, None).await;
        assert_eq!(
            reply.status, 404,
            "case {missing}: expected HTTP 404 for an insight the store does not hold, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ResourceNotFoundException",
            "case {missing}: expected ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceId"),
            Some(missing),
            "case {missing}: expected the refusal to name the id asked for, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceType"),
            Some(INSIGHT_RESOURCE_TYPE),
            "case {missing}: expected the refusal to name the resource kind, got {reply:?}"
        );
    }
}

/// Twelve log groups, enough that every page size below walks more than one page.
fn paging_records(insight_id: &str, count: usize) -> Vec<AnomalousLogGroupRecord> {
    (0..count)
        .map(|index| AnomalousLogGroupRecord {
            insight_id: insight_id.to_string(),
            log_group_name: format!("/aws/service/group-{index:02}"),
            impact_start_time_millis: Some(BASE_MILLIS + index as i64 * HOUR_MILLIS),
            impact_end_time_millis: None,
            number_of_log_lines_scanned: index as i32,
            log_anomaly_showcases: Vec::new(),
        })
        .collect()
}

/// RULE: a walk pages at the size the caller asked for — no page exceeds it — and the
/// pages concatenate to the whole collection, in order, exactly once, with no token on
/// the last page.
#[tokio::test]
async fn pages_at_the_requested_size_and_covers_the_collection_once() {
    let insight_id = "insight-logs-paged";
    let count = 12;
    let expected: Vec<String> = paging_records(insight_id, count)
        .iter()
        .map(|record| record.log_group_name.clone())
        .collect();

    // Page sizes spanning the modelled range 1..200: the smallest, divisors and
    // non-divisors of the collection size, the size itself, one past it, the modelled
    // maximum, and no `MaxResults` at all (the service's own page size).
    for max_results in [
        Some(1),
        Some(3),
        Some(5),
        Some(12),
        Some(13),
        Some(200),
        None,
    ] {
        let guru = Guru::new();
        guru.seed(insight_id, paging_records(insight_id, count));

        let requested = max_results.map_or(PAGE_SIZE, |size| size as usize);
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let reply = guru.list(insight_id, max_results, token.as_deref()).await;
            assert_eq!(
                reply.status, 200,
                "case MaxResults={max_results:?} page {pages}: expected HTTP 200, got {reply:?}"
            );
            let names = reply.log_group_names();
            assert!(
                names.len() <= requested,
                "case MaxResults={max_results:?} page {pages}: expected at most {requested} items, got {}",
                names.len()
            );
            seen.extend(names);
            token = reply.next_token();
            pages += 1;
            assert!(
                pages <= count + 1,
                "case MaxResults={max_results:?}: the walk did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case MaxResults={max_results:?}: expected the whole collection in order exactly once, got {seen:?}"
        );
        let expected_pages = count.div_ceil(requested.max(1));
        assert_eq!(
            pages, expected_pages,
            "case MaxResults={max_results:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: a page token names an offset into ONE insight's log groups, so another
/// insight's token, a token this service never issued, and a token whose page no longer
/// exists are each refused with `ValidationException` rather than resolving to a page.
#[tokio::test]
async fn a_token_resolves_only_against_the_insight_it_was_issued_for() {
    let guru = Guru::new();
    let first = "insight-logs-first";
    let second = "insight-logs-second";
    guru.seed(first, paging_records(first, 4));
    guru.seed(second, paging_records(second, 4));

    let page = guru.list(first, Some(1), None).await;
    let token = page
        .next_token()
        .unwrap_or_else(|| panic!("case {first}: expected a NextToken, got {page:?}"));

    // The same token against the other insight, and two tokens this service never
    // issued for either.
    for (case, insight_id, candidate) in [
        ("another insight's token", second, token.clone()),
        (
            "a token of a foreign shape",
            first,
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an uppercase rewrite",
            first,
            token.to_ascii_uppercase(),
        ),
    ] {
        let reply = guru.list(insight_id, Some(1), Some(&candidate)).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ValidationException",
            "case {case}: expected ValidationException, got {reply:?}"
        );
    }

    // The token this service DID issue still resolves, against its own insight.
    let reply = guru.list(first, Some(1), Some(&token)).await;
    assert_eq!(
        reply.status, 200,
        "case the issuing insight: expected HTTP 200, got {reply:?}"
    );
    assert_eq!(
        reply.log_group_names(),
        vec!["/aws/service/group-01".to_string()],
        "case the issuing insight: expected the second log group, got {reply:?}"
    );

    // A cursor into a collection that has since shrunk names a page that no longer
    // exists: a refusal, not an empty page that would claim the walk had finished.
    guru.clear_log_groups(first);
    let reply = guru.list(first, Some(1), Some(&token)).await;
    assert_eq!(
        reply.status, 400,
        "case a stale cursor: expected HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_name, "ValidationException",
        "case a stale cursor: expected ValidationException, got {reply:?}"
    );
}

/// RULE: the read reports — the store is identical before and after, however often and
/// however it is called, including on the refusals.
#[tokio::test]
async fn reading_never_changes_the_store() {
    let guru = Guru::new();
    let insight_id = "insight-logs-readonly";
    let rows = round_trip_rows();
    guru.seed(
        insight_id,
        rows.iter().map(|row| record_for(insight_id, row)).collect(),
    );

    let before = guru.snapshot();
    for max_results in [None, Some(1), Some(2), Some(200)] {
        for id in [insight_id, "insight-logs-absent"] {
            let reply = guru.list(id, max_results, None).await;
            assert!(
                reply.status == 200 || reply.status == 404,
                "case {id}/MaxResults={max_results:?}: expected a report or a not-found, got {reply:?}"
            );
            assert_eq!(
                guru.snapshot(),
                before,
                "case {id}/MaxResults={max_results:?}: a read must not change the store"
            );
        }
    }
}

/// RULE: `DeleteInsight` deletes the insight along with its associated anomalies, so
/// the log groups its log anomalies were found in are gone with it while another
/// insight's are untouched.
#[tokio::test]
async fn deleting_the_insight_takes_its_anomalous_log_groups() {
    let guru = Guru::new();
    let deleted = "insight-logs-deleted";
    let kept = "insight-logs-kept";
    guru.seed(deleted, paging_records(deleted, 3));
    guru.seed(kept, paging_records(kept, 2));

    let reply = guru.delete_insight(deleted).await;
    assert_eq!(
        reply.status, 200,
        "case {deleted}: expected the delete to succeed, got {reply:?}"
    );

    let reply = guru.list(deleted, None, None).await;
    assert_eq!(
        reply.status, 404,
        "case {deleted}: the insight is gone, so its log groups are no longer reported, got {reply:?}"
    );
    assert_eq!(
        reply.error_name, "ResourceNotFoundException",
        "case {deleted}: expected ResourceNotFoundException, got {reply:?}"
    );
    assert!(
        !guru
            .store()
            .lock()
            .anomalous_log_groups
            .contains_key(deleted),
        "case {deleted}: expected the stored log groups removed with the insight"
    );

    let reply = guru.list(kept, None, None).await;
    assert_eq!(
        reply.status, 200,
        "case {kept}: another insight's log groups are untouched, got {reply:?}"
    );
    assert_eq!(
        reply.log_group_names(),
        vec![
            "/aws/service/group-00".to_string(),
            "/aws/service/group-01".to_string()
        ],
        "case {kept}: expected both stored log groups, got {reply:?}"
    );
}
