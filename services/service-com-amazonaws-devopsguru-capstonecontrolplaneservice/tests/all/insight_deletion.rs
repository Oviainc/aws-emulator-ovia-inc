// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteInsight`, the insight deletion.
//!
//! The RULES these tests pin:
//!
//! * `DeleteInsight` removes the insight the store holds under the requested `Id` and
//!   answers HTTP 200 with a body carrying no member, so every later read of the insight
//!   store (`DescribeInsight`, `ListInsights`) reports the store without it while every
//!   other insight is left exactly as it was.
//! * An `Id` the store does not hold names no insight, and is refused with
//!   `ResourceNotFoundException` (HTTP 404) naming the id asked for and the insight
//!   resource type; a refused call changes nothing, so deleting the same insight twice
//!   succeeds once and then refuses.
//! * Deletion reaches the insight store only: the feedback a client submitted through
//!   `PutFeedback` is not owned by an insight record (`PutFeedback` accepts feedback
//!   about an insight this account has never seen), so `DescribeFeedback` still reports
//!   it after the insight it is about has been deleted.
//!
//! DevOps Guru mints insights from its own analysis, so no client request creates one;
//! these tests therefore write records straight into the engine's store (the same store
//! the handler reaches through `crate::state::service_state`) and then drive the
//! operations over the wire through the same [`MultiServiceRouter`] the emulator boots,
//! so every assertion is about wire behaviour.
//!
//! Each test exercises its rule over a SET of inputs differing along what the rule
//! generalises over: insight type, severity, status, whether the insight names a
//! resource collection, the shape and length of the requested id, and the feedback
//! option submitted about the insight.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::shared_types::INSIGHT_FEEDBACK_OPTION_VALUES;
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    INSIGHT_SEVERITY_VALUES, INSIGHT_STATUS_VALUES, INSIGHT_TYPE_VALUES,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord, ResourceCoverage,
    INSIGHT_RESOURCE_TYPE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A base instant (epoch milliseconds) the seeded insights are placed around.
const BASE_MILLIS: i64 = 1_700_000_000_123;

/// One hour in milliseconds, the unit the seeded insight windows are spaced by.
const HOUR_MILLIS: i64 = 3_600_000;

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

    /// Whether the reply carries no member at all: an operation whose output shape
    /// models none answers either an empty body or an empty JSON object.
    fn is_empty_payload(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }

    /// The `Id`s of one list member, in the order the reply listed them.
    fn ids_at(&self, pointer: &str) -> Vec<String> {
        self.at(pointer)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.get("Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A fresh emulator holding only this service, over an engine the test keeps a handle
/// to so it can seed the insights DevOps Guru's analysis would have raised.
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

    /// The very store the handler writes: `state::<T>()` is keyed by type, so this is
    /// the same value `crate::state::service_state` hands a request.
    fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState> {
        self.engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>()
    }

    fn seed(&self, records: &[InsightRecord]) {
        let state = self.store();
        let mut store = state.lock();
        for record in records {
            store.insights.insert(record.id.clone(), record.clone());
        }
    }

    fn insight_ids(&self) -> Vec<String> {
        self.store().lock().insights.keys().cloned().collect()
    }

    /// The whole insight store rendered, so a test can assert a refusal changed
    /// nothing at all rather than only that the entry count held.
    fn insight_store_snapshot(&self) -> String {
        format!("{:?}", self.store().lock().insights)
    }

    /// The feedback store rendered, for the same reason.
    fn feedback_store_snapshot(&self) -> String {
        format!("{:?}", self.store().lock().insight_feedback)
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
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

    async fn delete_insight(&self, id: &str) -> Reply {
        self.call("DELETE", &format!("/insights/{id}"), None).await
    }

    async fn describe_insight(&self, id: &str) -> Reply {
        self.call("GET", &format!("/insights/{id}"), None).await
    }

    /// Every insight id `ListInsights` reports for `insight_type`, over a start-time
    /// range wide enough to hold every seeded insight.
    async fn list_any(&self, insight_type: &str) -> Vec<String> {
        let filter = json!({
            "Any": {
                "Type": insight_type,
                "StartTimeRange": {
                    "FromTime": (BASE_MILLIS - HOUR_MILLIS) as f64 / 1_000.0,
                    "ToTime": (BASE_MILLIS + 10 * HOUR_MILLIS) as f64 / 1_000.0,
                }
            }
        });
        let reply = self
            .call("POST", "/insights", Some(&json!({ "StatusFilter": filter })))
            .await;
        assert_eq!(
            reply.status, 200,
            "listing {insight_type} insights must succeed, got {reply:?}"
        );
        let mut ids = reply.ids_at("/ProactiveInsights");
        ids.extend(reply.ids_at("/ReactiveInsights"));
        ids
    }

    async fn put_feedback(&self, insight_id: &str, feedback: &str) -> Reply {
        self.call(
            "PUT",
            "/feedback",
            Some(&json!({ "InsightFeedback": { "Id": insight_id, "Feedback": feedback } })),
        )
        .await
    }

    async fn describe_feedback(&self, insight_id: &str) -> Reply {
        self.call(
            "POST",
            "/feedback",
            Some(&json!({ "InsightId": insight_id })),
        )
        .await
    }
}

/// One row of the deletion table: an insight as DevOps Guru's analysis would have
/// raised it, plus the shape of the id it is stored under.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    insight_type: &'static str,
    severity: &'static str,
    status: &'static str,
    /// Whether the insight names a resource collection, so the table covers a record
    /// that carries one and a record that carries none.
    with_coverage: bool,
    start_millis: i64,
}

impl Row {
    fn record(&self) -> InsightRecord {
        let coverage = self.with_coverage.then(|| ResourceCoverage {
            cloudformation_stack_names: ["ops-stack-alpha".to_string()].into_iter().collect(),
            tag_collections: BTreeMap::from([(
                "DevOps-Guru-app".to_string(),
                BTreeSet::from(["payments".to_string()]),
            )]),
        });
        InsightRecord {
            id: self.id.clone(),
            name: format!("{} insight {}", self.insight_type, self.id),
            insight_type: self.insight_type.to_string(),
            severity: self.severity.to_string(),
            status: self.status.to_string(),
            start_time_millis: self.start_millis,
            end_time_millis: (self.status == "CLOSED").then_some(self.start_millis + HOUR_MILLIS),
            resource_collection: coverage,
            ..InsightRecord::default()
        }
    }
}

/// The id shapes `InsightId` (len 1..100, `^[\w-]*$`) admits, walked alongside the
/// model's own catalogs so the table covers the id forms as well as the record forms:
/// one character, a word with an underscore, a uuid-shaped id and the 100-character
/// maximum.
fn id_shapes() -> [String; 4] {
    [
        "i".to_string(),
        "ongoing_insight-42".to_string(),
        "4f8bb0d2-1a3f-4c21-9fb1-6a9a51f3dd77".to_string(),
        "z".repeat(100),
    ]
}

/// Every combination of the model's `InsightType`, `InsightSeverity` and
/// `InsightStatus` catalogs (`INSIGHT_TYPE_VALUES`, `INSIGHT_SEVERITY_VALUES`,
/// `INSIGHT_STATUS_VALUES` in `ops/types/shared_types.rs`, generated from the model's
/// enums), walking the id shapes and the presence of a resource collection alongside
/// them.
fn table() -> Vec<Row> {
    let shapes = id_shapes();
    let mut rows = Vec::new();
    for insight_type in INSIGHT_TYPE_VALUES {
        for severity in INSIGHT_SEVERITY_VALUES {
            for status in INSIGHT_STATUS_VALUES {
                let index = rows.len();
                // Each row's id carries one of the admitted shapes, suffixed so it is
                // unique across the table while staying inside `^[\w-]*$`.
                let shape = &shapes[index % shapes.len()];
                let id = format!("{}-{index}", &shape[..shape.len().min(90)]);
                rows.push(Row {
                    id,
                    insight_type,
                    severity,
                    status,
                    with_coverage: index % 2 == 0,
                    start_millis: BASE_MILLIS + index as i64 * 60_000,
                });
            }
        }
    }
    rows
}

/// RULE: `DeleteInsight` removes the insight stored under the requested `Id`, answers
/// HTTP 200 with no member, and leaves every other insight untouched — so the deleted
/// insight is gone from every later read of the store and the rest still read back.
#[tokio::test]
async fn deleting_an_insight_removes_it_and_leaves_the_rest() {
    let guru = Guru::new();
    let rows = table();
    assert_eq!(
        rows.len(),
        INSIGHT_TYPE_VALUES.len() * INSIGHT_SEVERITY_VALUES.len() * INSIGHT_STATUS_VALUES.len(),
        "the table must cover every type/severity/status combination the model declares"
    );
    let records: Vec<InsightRecord> = rows.iter().map(Row::record).collect();
    guru.seed(&records);

    let mut remaining: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();

    for row in &rows {
        let case = &row.id;
        // The insight is there before the call.
        let before = guru.describe_insight(case).await;
        assert_eq!(
            before.status, 200,
            "case {case}: the insight must exist before the deletion, got {before:?}"
        );

        let deleted = guru.delete_insight(case).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected HTTP 200 from DeleteInsight, got {deleted:?}"
        );
        assert!(
            deleted.is_empty_payload(),
            "case {case}: DeleteInsightResponse models no member, got {deleted:?}"
        );

        // ROUND TRIP: the detail read no longer resolves the id.
        let after = guru.describe_insight(case).await;
        assert_eq!(
            after.status, 404,
            "case {case}: a deleted insight must no longer be describable, got {after:?}"
        );

        remaining.retain(|id| id != case);
        assert_eq!(
            guru.insight_ids(),
            {
                let mut expected = remaining.clone();
                expected.sort();
                expected
            },
            "case {case}: the store must hold exactly the insights not yet deleted"
        );

        // And the listing of this insight's own type no longer carries it, while every
        // insight that is still stored does.
        let listed = guru.list_any(row.insight_type).await;
        assert!(
            !listed.contains(case),
            "case {case}: a deleted insight must not be listed, got {listed:?}"
        );
        for other in &rows {
            if other.insight_type != row.insight_type || !remaining.contains(&other.id) {
                continue;
            }
            assert!(
                listed.contains(&other.id),
                "case {case}: {} is still stored and must still be listed, got {listed:?}",
                other.id
            );
        }

        // Every insight that is still stored still reads back exactly as it was.
        for other in &rows {
            if !remaining.contains(&other.id) {
                continue;
            }
            let reply = guru.describe_insight(&other.id).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: {} must be untouched by the deletion, got {reply:?}",
                other.id
            );
            let member = if other.insight_type == "PROACTIVE" {
                "ProactiveInsight"
            } else {
                "ReactiveInsight"
            };
            assert_eq!(
                reply.str_at(&format!("/{member}/Severity")),
                Some(other.severity),
                "case {case}: {} must still report Severity {}, got {reply:?}",
                other.id,
                other.severity
            );
        }
    }

    assert!(
        guru.insight_ids().is_empty(),
        "deleting every insight must leave the store empty, got {:?}",
        guru.insight_ids()
    );
}

/// RULE: an `Id` the store does not hold names no insight and is refused with
/// `ResourceNotFoundException` (HTTP 404) naming the requested id and the insight
/// resource type, leaving the store exactly as it was — so deleting the same insight a
/// second time is refused rather than reported as another deletion.
#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_as_not_found() {
    let guru = Guru::new();
    let rows = table();
    let records: Vec<InsightRecord> = rows.iter().map(Row::record).collect();
    guru.seed(&records);

    // One insight really is deleted first, so the "second delete" row below is a
    // refusal about the id and not about an empty store.
    let already_deleted = rows[0].id.clone();
    let first = guru.delete_insight(&already_deleted).await;
    assert_eq!(
        first.status, 200,
        "the first deletion must succeed, got {first:?}"
    );

    // Ids spanning the shapes `InsightId` admits: a single character, a word, a
    // uuid-shaped id, an id differing from a stored one by one character, the
    // 100-character maximum, and the id just deleted.
    let missing_ids = vec![
        "x".to_string(),
        "no_such_insight".to_string(),
        "00000000-0000-4000-8000-000000000000".to_string(),
        format!("{}0", rows[1].id),
        "a".repeat(100),
        already_deleted.clone(),
    ];

    for id in &missing_ids {
        let snapshot = guru.insight_store_snapshot();
        let reply = guru.delete_insight(id).await;
        assert_eq!(
            reply.status, 404,
            "case {id}: expected HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ResourceNotFoundException",
            "case {id}: expected ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceId"),
            Some(id.as_str()),
            "case {id}: the refusal must name the requested id, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceType"),
            Some(INSIGHT_RESOURCE_TYPE),
            "case {id}: expected ResourceType {INSIGHT_RESOURCE_TYPE}, got {reply:?}"
        );
        assert!(
            reply
                .str_at("/Message")
                .is_some_and(|message| !message.is_empty()),
            "case {id}: the refusal must carry a Message, got {reply:?}"
        );
        assert_eq!(
            guru.insight_store_snapshot(),
            snapshot,
            "case {id}: a refused deletion must leave the store exactly as it was"
        );
    }
}

/// RULE: deletion reaches the insight store only — the feedback a client submitted
/// through `PutFeedback` is not owned by an insight record, so `DescribeFeedback` still
/// reports it after the insight it is about has been deleted.
///
/// The rows differ in the feedback option submitted, walking every value of the model's
/// `InsightFeedbackOption` catalog (`INSIGHT_FEEDBACK_OPTION_VALUES` in
/// `ops/types/shared_types.rs`), over both insight types.
#[tokio::test]
async fn deleting_an_insight_leaves_its_feedback_alone() {
    let guru = Guru::new();
    let rows = table();
    let records: Vec<InsightRecord> = rows.iter().map(Row::record).collect();
    guru.seed(&records);

    for (index, row) in rows.iter().enumerate() {
        let case = &row.id;
        let feedback = INSIGHT_FEEDBACK_OPTION_VALUES[index % INSIGHT_FEEDBACK_OPTION_VALUES.len()];
        let submitted = guru.put_feedback(case, feedback).await;
        assert_eq!(
            submitted.status, 200,
            "case {case}: submitting feedback must succeed, got {submitted:?}"
        );

        let feedback_before = guru.feedback_store_snapshot();
        let deleted = guru.delete_insight(case).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected HTTP 200 from DeleteInsight, got {deleted:?}"
        );
        assert_eq!(
            guru.feedback_store_snapshot(),
            feedback_before,
            "case {case}: deleting an insight must not touch the feedback store"
        );

        let reported = guru.describe_feedback(case).await;
        assert_eq!(
            reported.status, 200,
            "case {case}: the feedback must still be readable, got {reported:?}"
        );
        assert_eq!(
            reported.str_at("/InsightFeedback/Feedback"),
            Some(feedback),
            "case {case}: expected the submitted feedback {feedback}, got {reported:?}"
        );
        assert_eq!(
            reported.str_at("/InsightFeedback/Id"),
            Some(case.as_str()),
            "case {case}: the feedback must still name its insight, got {reported:?}"
        );
    }
}
