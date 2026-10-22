// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeInsight`, the insight detail read.
//!
//! The RULES these tests pin:
//!
//! * `DescribeInsight` answers the insight the store holds under the requested `Id`,
//!   rendered into the response member its `InsightType` selects (`ProactiveInsight`
//!   or `ReactiveInsight`, never both), reporting exactly what the record holds.
//! * An `Id` the store does not hold names no insight and is refused with
//!   `ResourceNotFoundException` (404), naming the id asked for.
//! * An `AccountId` the caller cannot reach is an authorization refusal
//!   (`AccessDeniedException`, 403), decided before any lookup, so it is the same
//!   answer whether or not the insight exists; an absent `AccountId` is the caller's
//!   own account.
//! * It reports: the store is identical before and after, however often it is read.
//!
//! The listing side of the same store is covered by `insight_listing.rs`.
//!
//! DevOps Guru mints insights from its own analysis, so no client request creates
//! one; these tests therefore write records straight into the engine's store (the
//! same store the handler reads through `crate::state::service_state`) and then drive
//! the operation over the wire through the same [`MultiServiceRouter`] the emulator
//! boots, so every assertion is about wire behaviour.
//!
//! Each test exercises its rule over a SET of inputs that differ along what the rule
//! generalises over: insight type, severity, status, the form of the insight's
//! resource collection, the presence of the optional members, the shape of the
//! requested id, and the account the read is scoped to.

use bytes::Bytes;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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

/// A twelve-digit account id other than the emulator's own
/// (`engine::emulator_account_id`), as `AwsAccountId`'s `^\d{12}$` pattern requires.
/// This emulator is in no organization, so this account is not reachable from it.
const OTHER_ACCOUNT_ID: &str = "111122223333";

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

impl Reply {
    fn at(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer)
    }

    fn str_at(&self, pointer: &str) -> Option<&str> {
        self.at(pointer).and_then(Value::as_str)
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
/// handle to so it can seed the insights DevOps Guru's analysis would have raised.
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

    fn seed(&self, record: InsightRecord) {
        self.store()
            .lock()
            .insights
            .insert(record.id.clone(), record);
    }

    fn insight_count(&self) -> usize {
        self.store().lock().insights.len()
    }

    async fn describe_insight(&self, id: &str, account_id: Option<&str>) -> Reply {
        let path = match account_id {
            Some(account_id) => format!("/insights/{id}?AccountId={account_id}"),
            None => format!("/insights/{id}"),
        };
        self.call("GET", &path).await
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

/// The forms an insight's `ResourceCollection` takes: either half of the coverage
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

/// One row of the round-trip table: an insight as DevOps Guru's analysis would have
/// raised it.
#[derive(Debug, Clone)]
struct Row {
    id: String,
    insight_type: &'static str,
    severity: &'static str,
    status: &'static str,
    coverage: CoverageForm,
    start_millis: i64,
    end_millis: Option<i64>,
    prediction: Option<(i64, Option<i64>)>,
    ssm_ops_item_id: Option<String>,
    description: Option<String>,
}

impl Row {
    fn record(&self) -> InsightRecord {
        InsightRecord {
            id: self.id.clone(),
            name: format!("{} insight over {:?}", self.insight_type, self.coverage),
            insight_type: self.insight_type.to_string(),
            severity: self.severity.to_string(),
            status: self.status.to_string(),
            start_time_millis: self.start_millis,
            end_time_millis: self.end_millis,
            prediction_start_time_millis: self.prediction.map(|(start, _)| start),
            prediction_end_time_millis: self.prediction.and_then(|(_, end)| end),
            resource_collection: self.coverage.coverage(),
            ssm_ops_item_id: self.ssm_ops_item_id.clone(),
            description: self.description.clone(),
            ..InsightRecord::default()
        }
    }

    /// The response member this row's insight type selects.
    fn member(&self) -> &'static str {
        if self.insight_type == "PROACTIVE" {
            "ProactiveInsight"
        } else {
            "ReactiveInsight"
        }
    }

    /// The member it does NOT select, which must stay absent.
    fn other_member(&self) -> &'static str {
        if self.insight_type == "PROACTIVE" {
            "ReactiveInsight"
        } else {
            "ProactiveInsight"
        }
    }
}

/// Every combination of the model's `InsightType`, `InsightSeverity` and
/// `InsightStatus` catalogs (`INSIGHT_TYPE_VALUES`, `INSIGHT_SEVERITY_VALUES`,
/// `INSIGHT_STATUS_VALUES` in `ops/types/shared_types.rs`, generated from the
/// model's enums), walking the resource-collection forms and the optional members
/// alongside them.
fn table() -> Vec<Row> {
    let mut rows = Vec::new();
    for (t, insight_type) in INSIGHT_TYPE_VALUES.iter().enumerate() {
        for (s, severity) in INSIGHT_SEVERITY_VALUES.iter().enumerate() {
            for (c, status) in INSIGHT_STATUS_VALUES.iter().enumerate() {
                let index = rows.len() as i64;
                let coverage = CoverageForm::ALL[(t + s + c) % CoverageForm::ALL.len()];
                let start_millis = BASE_MILLIS + index * 60_000;
                // A range has no end until the insight closes.
                let closed = *status == "CLOSED";
                let end_millis = closed.then_some(start_millis + HOUR_MILLIS);
                // Only a proactive insight predicts a problem over a time range;
                // every other row leaves the prediction range absent.
                let prediction = (*insight_type == "PROACTIVE").then_some((
                    start_millis + 300_000,
                    closed.then_some(start_millis + 2 * HOUR_MILLIS),
                ));
                rows.push(Row {
                    id: format!("insight-{insight_type}-{severity}-{status}"),
                    insight_type,
                    severity,
                    status,
                    coverage,
                    start_millis,
                    end_millis,
                    prediction,
                    // Walk the optional members too: an OpsItem exists only when
                    // OpsCenter raised one, and a description only when the insight
                    // carries one.
                    ssm_ops_item_id: (index % 2 == 0).then(|| format!("oi-{index:012}")),
                    description: (index % 3 != 0)
                        .then(|| format!("{severity} {insight_type} insight {index}")),
                })
            }
        }
    }
    rows
}

/// RULE: every stored insight reads back through `DescribeInsight` on the member its
/// `InsightType` selects, with each member reporting exactly what the store holds.
#[tokio::test]
async fn stored_insights_read_back_on_the_member_their_type_selects() {
    let emulator = Emulator::new();
    let rows = table();
    assert_eq!(
        rows.len(),
        INSIGHT_TYPE_VALUES.len() * INSIGHT_SEVERITY_VALUES.len() * INSIGHT_STATUS_VALUES.len(),
        "the table must cover every type/severity/status combination the model declares"
    );
    for row in &rows {
        emulator.seed(row.record());
    }

    for row in &rows {
        let case = &row.id;
        let reply = emulator.describe_insight(&row.id, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected HTTP 200, got {} ({})",
            reply.status, reply.raw
        );

        let member = format!("/{}", row.member());
        assert!(
            reply.at(&member).is_some(),
            "case {case}: expected the insight on {member}, got {}",
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
            reply.str_at(&format!("{member}/Name")),
            Some(row.record().name.as_str()),
            "case {case}: expected the stored Name, got {}",
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/SsmOpsItemId")),
            row.ssm_ops_item_id.as_deref(),
            "case {case}: expected SsmOpsItemId {:?}, got {}",
            row.ssm_ops_item_id,
            reply.raw
        );
        assert_eq!(
            reply.str_at(&format!("{member}/Description")),
            row.description.as_deref(),
            "case {case}: expected Description {:?}, got {}",
            row.description,
            reply.raw
        );

        // The insight's own window: start always, end only once it closed.
        assert_eq!(
            reply.millis_at(&format!("{member}/InsightTimeRange/StartTime")),
            Some(row.start_millis),
            "case {case}: expected InsightTimeRange.StartTime {}, got {}",
            row.start_millis,
            reply.raw
        );
        assert_eq!(
            reply.millis_at(&format!("{member}/InsightTimeRange/EndTime")),
            row.end_millis,
            "case {case}: expected InsightTimeRange.EndTime {:?}, got {}",
            row.end_millis,
            reply.raw
        );

        // A prediction range belongs to a proactive insight only; `ReactiveInsight`
        // models no such member at all.
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

        // The resources the insight was raised over, reported once each in a stable
        // order; an insight over no coverage reports no collection at all.
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
                "case {case}: an insight over no declared resources must report no \
                 ResourceCollection, got {}",
                reply.raw
            );
        }
    }
}

/// RULE: an `Id` the store does not hold names no insight, and is refused with
/// `ResourceNotFoundException` (HTTP 404) naming the id asked for.
#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_as_not_found() {
    let emulator = Emulator::new();
    // One insight exists, so the refusals below are about the id asked for and not
    // about an empty store.
    let stored = table().remove(0);
    emulator.seed(stored.record());

    // Ids spanning the shapes `InsightId` (len 1..100, `^[\w-]*$`) admits: one
    // character, a word, a uuid-shaped id, an id differing from the stored one by a
    // single character, and the 100-character maximum.
    let missing_ids = vec![
        "x".to_string(),
        "no_such_insight".to_string(),
        "4f8bb0d2-1a3f-4c21-9fb1-6a9a51f3dd77".to_string(),
        format!("{}0", stored.id),
        "a".repeat(100),
    ];

    for id in &missing_ids {
        let before = emulator.insight_count();
        let reply = emulator.describe_insight(id, None).await;
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
            Some(INSIGHT_RESOURCE_TYPE),
            "case {id}: expected ResourceType {INSIGHT_RESOURCE_TYPE}, got {}",
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
            emulator.insight_count(),
            before,
            "case {id}: a refused read must not change the store"
        );
    }
}

/// RULE: a read scoped to an account the caller cannot reach is an authorization
/// refusal (`AccessDeniedException`, HTTP 403), decided before any lookup, so it is
/// the same answer whether the insight exists or not; the caller's own account is
/// what an absent `AccountId` means.
#[tokio::test]
async fn an_account_the_caller_cannot_reach_is_refused_as_access_denied() {
    let emulator = Emulator::new();
    let own_account = ::engine::emulator_account_id().to_string();
    let stored = table().remove(0);
    emulator.seed(stored.record());

    // (requested id, requested AccountId, expected status)
    let cases: Vec<(&str, Option<&str>, u16)> = vec![
        // The caller's own account, named and unnamed, resolves the insight.
        (&stored.id, None, 200),
        (&stored.id, Some(&own_account), 200),
        // Any other account is unreachable, and the answer does not depend on
        // whether the id names a stored insight.
        (&stored.id, Some(OTHER_ACCOUNT_ID), 403),
        ("no_such_insight", Some(OTHER_ACCOUNT_ID), 403),
        (&stored.id, Some("999988887777"), 403),
        ("no_such_insight", Some("999988887777"), 403),
        // An id that is not stored is still a not-found in the caller's own account.
        ("no_such_insight", None, 404),
        ("no_such_insight", Some(&own_account), 404),
    ];

    for (id, account_id, expected) in cases {
        let reply = emulator.describe_insight(id, account_id).await;
        assert_eq!(
            reply.status, expected,
            "case {id:?}/{account_id:?}: expected HTTP {expected}, got {} ({})",
            reply.status, reply.raw
        );
        match expected {
            200 => assert_eq!(
                reply.str_at(&format!("/{}/Id", stored.member())),
                Some(id),
                "case {id:?}/{account_id:?}: expected the insight asked for, got {}",
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

/// RULE: `DescribeInsight` reports, so what one caller reads is what the next one
/// reads — the store is identical before and after, however often it is read.
#[tokio::test]
async fn describing_an_insight_leaves_the_store_unchanged() {
    let emulator = Emulator::new();
    let rows = table();
    for row in &rows {
        emulator.seed(row.record());
    }
    let before = format!("{:?}", emulator.store().lock().insights);

    for row in &rows {
        let case = &row.id;
        let first = emulator.describe_insight(&row.id, None).await;
        let second = emulator.describe_insight(&row.id, None).await;
        let third = emulator.describe_insight(&row.id, None).await;
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
    let absent = emulator.describe_insight("not-a-stored-insight", None).await;
    assert_eq!(
        absent.status, 404,
        "a read of an absent id must refuse, got {}",
        absent.raw
    );
    let refused = emulator
        .describe_insight("not-a-stored-insight", Some(OTHER_ACCOUNT_ID))
        .await;
    assert_eq!(
        refused.status, 403,
        "a read of an unreachable account must refuse, got {}",
        refused.raw
    );

    assert_eq!(
        format!("{:?}", emulator.store().lock().insights),
        before,
        "reading insights must not change the store"
    );
}
