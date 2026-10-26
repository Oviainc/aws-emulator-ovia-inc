// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListEvents`, the read over the events the resources DevOps
//! Guru evaluates emitted.
//!
//! The RULES these tests pin:
//!
//! * The read reports the events the store holds, each rendered with exactly the
//!   members `Event` models and the values the record holds, in the store's stable
//!   `EventId` order.
//! * Each `ListEventsFilters` member names ONE property of an event, so an event is
//!   reported exactly while every filter the request carries admits it (the filters are
//!   joined with AND), a filter the request omits restricts nothing, and an event the
//!   analysis recorded no such property for is not admitted by a filter on it.
//! * A filter value outside its modelled enum, and a `MaxResults` outside its modelled
//!   range, are refused with HTTP 400.
//! * A walk pages at the `MaxResults` the caller asked for: no page exceeds it, and the
//!   pages concatenate to the whole selection exactly once, with no token on the last
//!   page; a request naming no `MaxResults` is served the service's own page size.
//! * A page token is bound to the filter set it was issued for, so another selection's
//!   token, and a token whose page no longer exists, are refused with
//!   `ValidationException` (400) rather than resolving to some other page.
//! * An `InsightId` the store holds no insight under names no insight and is refused
//!   with `ResourceNotFoundException` (404), naming the id asked for; an insight that
//!   exists with no event of its own is an empty list, not a refusal.
//! * An `AccountId` other than the one this emulator serves is an authorization refusal
//!   (`AccessDeniedException`, 403), decided before any lookup, so it cannot report
//!   whether the insight exists; an absent `AccountId` is the caller's own.
//! * It reports: the store is identical before and after, however often it is read.
//! * `DeleteInsight` deletes the insight "along with the associated anomalies, events
//!   and recommendations", so the events related to it go with it while the events of
//!   another insight, and those related to none, are left alone.
//!
//! DevOps Guru finds an event in an operational data source it reads through its own
//! analysis, so no client request creates one; these tests therefore write records
//! straight into the engine's store (the same store the handler reads through
//! `crate::state::service_state`) and then drive the operation over the wire through the
//! same [`MultiServiceRouter`] the emulator boots.
//!
//! Each test exercises its rule over a SET of inputs differing along what the rule
//! generalises over: which optional members an event carries, which data source, class
//! and source it came from, which insight it is related to and which resources it is
//! about, which filter member the request names and with which value, how many filters
//! it combines, the requested page size including each modelled bound and one past it,
//! and which account is named.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    EventResource, EVENT_CLASS_VALUES, EVENT_DATA_SOURCE_VALUES,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, EventRecord, InsightRecord,
    ResourceCoverage, LIST_EVENTS_MAX_RESULTS, PAGE_SIZE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A base instant (epoch milliseconds) the seeded events occurred at. A whole number
/// of seconds, and the offsets below are whole half-seconds, so each instant is exactly
/// representable in the epoch-seconds `double` a JSON protocol carries a `Timestamp` as.
const BASE_MILLIS: i64 = 1_700_000_000_000;

/// The `ResourceType` an insight-shaped `ResourceNotFoundException` reports.
const INSIGHT_RESOURCE_TYPE: &str = "INSIGHT";

/// A twelve-digit account id that is NOT the account this emulator serves
/// (`engine::emulator_account_id`), as `AwsAccountId`'s `^\d{12}$` pattern requires.
const OTHER_ACCOUNT_ID: &str = "111122223333";

/// The app-boundary key the seeded events' tag collections are under.
const BOUNDARY_KEY: &str = "devops-guru-boundary";

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

    /// The reported `Events`, in the order the reply listed them.
    fn events(&self) -> Vec<Value> {
        self.at("/Events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The reported `Id`s, in the order the reply listed them.
    fn ids(&self) -> Vec<String> {
        self.events()
            .iter()
            .map(|event| {
                event
                    .get("Id")
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

/// A reported `Timestamp` as the milliseconds the store keeps: a JSON protocol carries
/// one as epoch SECONDS, so a millisecond instant comes back fractional.
fn millis_of(reported: Option<&Value>) -> Option<i64> {
    let seconds = reported.and_then(Value::as_f64)?;
    Some((seconds * 1000.0).round() as i64)
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

    /// Seed the events the analysis would have found, together with every insight they
    /// are related to.
    fn seed(&self, rows: &[Row]) {
        for insight_id in rows.iter().filter_map(|row| row.insight_id) {
            self.seed_insight(insight_id);
        }
        let state = self.store();
        let mut store = state.lock();
        for row in rows {
            store.events.insert(row.id.to_string(), row.record());
        }
    }

    /// A stable rendering of the whole store, for the "a read does not write" check.
    fn snapshot(&self) -> String {
        format!("{:?}", *self.store().lock())
    }

    async fn list(
        &self,
        filters: &Value,
        max_results: Option<i64>,
        account_id: Option<&str>,
        token: Option<&str>,
    ) -> Reply {
        let mut body = json!({ "Filters": filters });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(account_id) = account_id {
            body["AccountId"] = json!(account_id);
        }
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("POST", "/events", &body).await
    }

    /// `ListEvents` with the filter set, and nothing else, a case names.
    async fn list_with(&self, filters: &Value) -> Reply {
        self.list(filters, None, None, None).await
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

/// One resource an event names: its `Type`, `Name` and `Arn`, each optional as the
/// model has them.
type ResourceRow = (
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// One row of the seeded table: an event as the analysis would have found it.
#[derive(Debug, Clone)]
struct Row {
    id: &'static str,
    /// `Time`, absent when the analysis placed no instant on the event.
    time_millis: Option<i64>,
    event_source: Option<&'static str>,
    name: Option<&'static str>,
    data_source: Option<&'static str>,
    event_class: Option<&'static str>,
    resources: Vec<ResourceRow>,
    /// The CloudFormation stacks the event's `ResourceCollection` names.
    stacks: Vec<&'static str>,
    /// The tag values the event's `ResourceCollection` names under [`BOUNDARY_KEY`].
    tag_values: Vec<&'static str>,
    /// The insight the analysis related the event to, if any.
    insight_id: Option<&'static str>,
}

impl Row {
    fn record(&self) -> EventRecord {
        let names_resources = !self.stacks.is_empty() || !self.tag_values.is_empty();
        EventRecord {
            id: self.id.to_string(),
            time_millis: self.time_millis,
            event_source: self.event_source.map(str::to_string),
            name: self.name.map(str::to_string),
            data_source: self.data_source.map(str::to_string),
            event_class: self.event_class.map(str::to_string),
            resources: self
                .resources
                .iter()
                .map(|(r#type, name, arn)| EventResource {
                    r#type: r#type.map(str::to_string),
                    name: name.map(str::to_string),
                    arn: arn.map(str::to_string),
                })
                .collect(),
            resource_collection: names_resources.then(|| ResourceCoverage {
                cloudformation_stack_names: self
                    .stacks
                    .iter()
                    .map(|stack_name| (*stack_name).to_string())
                    .collect(),
                tag_collections: if self.tag_values.is_empty() {
                    BTreeMap::new()
                } else {
                    let values: BTreeSet<String> = self
                        .tag_values
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect();
                    [(BOUNDARY_KEY.to_string(), values)].into_iter().collect()
                },
            }),
            associated_insight_id: self.insight_id.map(str::to_string),
        }
    }

    /// This row as the reply must report it, with `Time` left out (a JSON protocol
    /// reports it as a fractional number, which is compared on its own) and every
    /// absent optional member omitted.
    fn expected_without_time(&self) -> Value {
        let mut event = Map::new();
        event.insert("Id".to_string(), json!(self.id));
        if let Some(event_source) = self.event_source {
            event.insert("EventSource".to_string(), json!(event_source));
        }
        if let Some(name) = self.name {
            event.insert("Name".to_string(), json!(name));
        }
        if let Some(data_source) = self.data_source {
            event.insert("DataSource".to_string(), json!(data_source));
        }
        if let Some(event_class) = self.event_class {
            event.insert("EventClass".to_string(), json!(event_class));
        }
        if !self.resources.is_empty() {
            let resources: Vec<Value> = self
                .resources
                .iter()
                .map(|(r#type, name, arn)| {
                    let mut resource = Map::new();
                    if let Some(r#type) = r#type {
                        resource.insert("Type".to_string(), json!(r#type));
                    }
                    if let Some(name) = name {
                        resource.insert("Name".to_string(), json!(name));
                    }
                    if let Some(arn) = arn {
                        resource.insert("Arn".to_string(), json!(arn));
                    }
                    Value::Object(resource)
                })
                .collect();
            event.insert("Resources".to_string(), json!(resources));
        }
        if !self.stacks.is_empty() || !self.tag_values.is_empty() {
            let mut collection = Map::new();
            if !self.stacks.is_empty() {
                collection.insert(
                    "CloudFormation".to_string(),
                    json!({ "StackNames": self.stacks }),
                );
            }
            if !self.tag_values.is_empty() {
                collection.insert(
                    "Tags".to_string(),
                    json!([{ "AppBoundaryKey": BOUNDARY_KEY, "TagValues": self.tag_values }]),
                );
            }
            event.insert("ResourceCollection".to_string(), Value::Object(collection));
        }
        Value::Object(event)
    }
}

/// The insight ids the seeded table relates its events to.
const PAYMENTS_INSIGHT: &str = "i-payments";
const CHECKOUT_INSIGHT: &str = "i-checkout";

/// The events the analysis would have found, spanning the properties the filters
/// select on: every modelled `EventClass` and `EventDataSource`, three event sources,
/// both halves of a resource collection, an event related to each of two insights and
/// to none, an event with no instant, and an event carrying no optional member at all.
fn rows() -> Vec<Row> {
    vec![
        Row {
            id: "e-0001-config-change",
            time_millis: Some(BASE_MILLIS),
            event_source: Some("cloudformation.amazonaws.com"),
            name: Some("UpdateStack"),
            data_source: Some("AWS_CLOUD_TRAIL"),
            event_class: Some("CONFIG_CHANGE"),
            resources: vec![(
                Some("AWS::CloudFormation::Stack"),
                Some("payments-api"),
                Some("arn:aws:cloudformation:us-east-1:123456789012:stack/payments-api/one"),
            )],
            stacks: vec!["payments-api"],
            tag_values: vec![],
            insight_id: Some(PAYMENTS_INSIGHT),
        },
        Row {
            id: "e-0002-deployment",
            time_millis: Some(BASE_MILLIS + 1_000),
            event_source: Some("codedeploy.amazonaws.com"),
            name: Some("DeploymentSucceeded"),
            data_source: Some("AWS_CODE_DEPLOY"),
            event_class: Some("DEPLOYMENT"),
            resources: vec![(Some("AWS::CodeDeploy::Deployment"), Some("d-1"), None)],
            stacks: vec![],
            tag_values: vec!["payments"],
            insight_id: Some(PAYMENTS_INSIGHT),
        },
        Row {
            id: "e-0003-infrastructure",
            time_millis: Some(BASE_MILLIS + 2_500),
            event_source: Some("aws.events"),
            name: None,
            data_source: Some("AWS_CLOUD_TRAIL"),
            event_class: Some("INFRASTRUCTURE"),
            resources: vec![],
            stacks: vec!["checkout-api"],
            tag_values: vec![],
            insight_id: Some(CHECKOUT_INSIGHT),
        },
        Row {
            id: "e-0004-security-change",
            time_millis: Some(BASE_MILLIS + 3_000),
            event_source: Some("iam.amazonaws.com"),
            name: Some("AttachRolePolicy"),
            data_source: Some("AWS_CLOUD_TRAIL"),
            event_class: Some("SECURITY_CHANGE"),
            resources: vec![(None, Some("devops-guru-role"), None)],
            stacks: vec![],
            tag_values: vec![],
            insight_id: None,
        },
        Row {
            id: "e-0005-schema-change",
            time_millis: Some(BASE_MILLIS + 4_000),
            event_source: Some("rds.amazonaws.com"),
            name: Some("ModifyDBInstance"),
            data_source: Some("AWS_CLOUD_TRAIL"),
            event_class: Some("SCHEMA_CHANGE"),
            resources: vec![],
            stacks: vec![],
            // Sorted: the store keeps tag values as a set, and reports them in order.
            tag_values: vec!["orders", "payments"],
            insight_id: None,
        },
        Row {
            id: "e-0006-bare",
            time_millis: None,
            event_source: None,
            name: None,
            data_source: None,
            event_class: None,
            resources: vec![],
            stacks: vec![],
            tag_values: vec![],
            insight_id: None,
        },
    ]
}

/// Every seeded id, in the store's stable key order.
fn all_ids() -> Vec<String> {
    let mut ids: Vec<String> = rows().iter().map(|row| row.id.to_string()).collect();
    ids.sort();
    ids
}

fn ids(wanted: &[&str]) -> Vec<String> {
    wanted.iter().map(|id| (*id).to_string()).collect()
}

/// RULE: the read reports the events the store holds, each rendered with exactly the
/// members `Event` models and the values the record holds, in `EventId` order.
#[tokio::test]
async fn reported_events_carry_the_stored_members_in_id_order() {
    let guru = Guru::new();
    let rows = rows();
    guru.seed(&rows);

    let reply = guru.list_with(&json!({})).await;
    assert_eq!(
        reply.status, 200,
        "an unfiltered read must be served: {reply:?}"
    );
    assert_eq!(
        reply.ids(),
        all_ids(),
        "every stored event must be reported once, in id order: {reply:?}"
    );
    assert_eq!(
        reply.next_token(),
        None,
        "a selection that fits one page carries no token: {reply:?}"
    );

    for (row, mut reported) in rows.iter().zip(reply.events()) {
        let case = row.id;
        let time = reported
            .as_object_mut()
            .and_then(|event| event.remove("Time"));
        assert_eq!(
            millis_of(time.as_ref()),
            row.time_millis,
            "case {case:?}: expected Time {:?}, got {time:?}",
            row.time_millis
        );
        assert_eq!(
            reported,
            row.expected_without_time(),
            "case {case:?}: expected {}, got {reported}",
            row.expected_without_time()
        );
    }
}

/// RULE: each filter member names one property of an event, the filters a request
/// carries are joined with AND, a filter it omits restricts nothing, and an event the
/// analysis recorded no such property for is not admitted by a filter on it.
#[tokio::test]
async fn each_filter_selects_exactly_the_events_with_that_property() {
    let guru = Guru::new();
    guru.seed(&rows());

    let payments_tag = json!({
        "Tags": [{ "AppBoundaryKey": BOUNDARY_KEY, "TagValues": ["payments"] }]
    });
    // (case, Filters, the ids the reply must report)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter restricts nothing", json!({}), all_ids()),
        (
            "InsightId selects the events related to that insight",
            json!({ "InsightId": PAYMENTS_INSIGHT }),
            ids(&["e-0001-config-change", "e-0002-deployment"]),
        ),
        (
            "InsightId selects another insight's events",
            json!({ "InsightId": CHECKOUT_INSIGHT }),
            ids(&["e-0003-infrastructure"]),
        ),
        (
            "EventTimeRange is inclusive at both bounds",
            json!({ "EventTimeRange": {
                "FromTime": seconds(BASE_MILLIS),
                "ToTime": seconds(BASE_MILLIS),
            } }),
            ids(&["e-0001-config-change"]),
        ),
        (
            "EventTimeRange selects the events inside it",
            json!({ "EventTimeRange": {
                "FromTime": seconds(BASE_MILLIS + 1_000),
                "ToTime": seconds(BASE_MILLIS + 3_000),
            } }),
            ids(&[
                "e-0002-deployment",
                "e-0003-infrastructure",
                "e-0004-security-change",
            ]),
        ),
        (
            "an event with no Time lies in no EventTimeRange",
            json!({ "EventTimeRange": {
                "FromTime": seconds(BASE_MILLIS - 1_000),
                "ToTime": seconds(BASE_MILLIS + 10_000),
            } }),
            ids(&[
                "e-0001-config-change",
                "e-0002-deployment",
                "e-0003-infrastructure",
                "e-0004-security-change",
                "e-0005-schema-change",
            ]),
        ),
        (
            "an EventTimeRange that holds no instant selects nothing",
            json!({ "EventTimeRange": {
                "FromTime": seconds(BASE_MILLIS + 3_000),
                "ToTime": seconds(BASE_MILLIS + 1_000),
            } }),
            vec![],
        ),
        (
            "EventClass CONFIG_CHANGE",
            json!({ "EventClass": "CONFIG_CHANGE" }),
            ids(&["e-0001-config-change"]),
        ),
        (
            "EventClass DEPLOYMENT",
            json!({ "EventClass": "DEPLOYMENT" }),
            ids(&["e-0002-deployment"]),
        ),
        (
            "EventClass INFRASTRUCTURE",
            json!({ "EventClass": "INFRASTRUCTURE" }),
            ids(&["e-0003-infrastructure"]),
        ),
        (
            "EventClass SECURITY_CHANGE",
            json!({ "EventClass": "SECURITY_CHANGE" }),
            ids(&["e-0004-security-change"]),
        ),
        (
            "EventClass SCHEMA_CHANGE",
            json!({ "EventClass": "SCHEMA_CHANGE" }),
            ids(&["e-0005-schema-change"]),
        ),
        (
            "EventSource matches the event's own source",
            json!({ "EventSource": "cloudformation.amazonaws.com" }),
            ids(&["e-0001-config-change"]),
        ),
        (
            "EventSource aws.events",
            json!({ "EventSource": "aws.events" }),
            ids(&["e-0003-infrastructure"]),
        ),
        (
            "EventSource no stored event came from",
            json!({ "EventSource": "lambda.amazonaws.com" }),
            vec![],
        ),
        (
            "DataSource AWS_CLOUD_TRAIL",
            json!({ "DataSource": "AWS_CLOUD_TRAIL" }),
            ids(&[
                "e-0001-config-change",
                "e-0003-infrastructure",
                "e-0004-security-change",
                "e-0005-schema-change",
            ]),
        ),
        (
            "DataSource AWS_CODE_DEPLOY",
            json!({ "DataSource": "AWS_CODE_DEPLOY" }),
            ids(&["e-0002-deployment"]),
        ),
        (
            "ResourceCollection naming one covered stack",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["payments-api"] } } }),
            ids(&["e-0001-config-change"]),
        ),
        (
            "ResourceCollection naming two stacks selects the events about either",
            json!({ "ResourceCollection": {
                "CloudFormation": { "StackNames": ["payments-api", "checkout-api"] }
            } }),
            ids(&["e-0001-config-change", "e-0003-infrastructure"]),
        ),
        (
            "ResourceCollection naming a tag value selects the events about it",
            json!({ "ResourceCollection": payments_tag }),
            ids(&["e-0002-deployment", "e-0005-schema-change"]),
        ),
        (
            "ResourceCollection naming another tag value",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": BOUNDARY_KEY, "TagValues": ["orders"] }]
            } }),
            ids(&["e-0005-schema-change"]),
        ),
        (
            "ResourceCollection naming a tag key nothing is tagged with",
            json!({ "ResourceCollection": {
                "Tags": [{ "AppBoundaryKey": "other-boundary", "TagValues": ["payments"] }]
            } }),
            vec![],
        ),
        (
            "ResourceCollection naming a stack no event is about",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["ghost-api"] } } }),
            vec![],
        ),
        (
            "a ResourceCollection that names no resource restricts nothing",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": [] } } }),
            all_ids(),
        ),
        (
            "two filters are joined with AND",
            json!({ "EventClass": "CONFIG_CHANGE", "DataSource": "AWS_CLOUD_TRAIL" }),
            ids(&["e-0001-config-change"]),
        ),
        (
            "two filters no event satisfies together select nothing",
            json!({ "EventClass": "CONFIG_CHANGE", "DataSource": "AWS_CODE_DEPLOY" }),
            vec![],
        ),
        (
            "every filter member at once",
            json!({
                "InsightId": PAYMENTS_INSIGHT,
                "EventTimeRange": {
                    "FromTime": seconds(BASE_MILLIS + 1_000),
                    "ToTime": seconds(BASE_MILLIS + 9_000),
                },
                "EventClass": "DEPLOYMENT",
                "EventSource": "codedeploy.amazonaws.com",
                "DataSource": "AWS_CODE_DEPLOY",
                "ResourceCollection": payments_tag,
            }),
            ids(&["e-0002-deployment"]),
        ),
        (
            "an insight filter and a source no event of it came from",
            json!({ "InsightId": PAYMENTS_INSIGHT, "EventSource": "aws.events" }),
            vec![],
        ),
    ];

    // The class table above covers every modelled `EventClass` and `EventDataSource`,
    // so a value added to either enum shows up as a case this table does not name.
    for class in EVENT_CLASS_VALUES {
        assert!(
            cases
                .iter()
                .any(|(case, _, _)| case.contains(&format!("EventClass {class}"))),
            "case {class:?}: every modelled EventClass must have a row"
        );
    }
    for data_source in EVENT_DATA_SOURCE_VALUES {
        assert!(
            cases
                .iter()
                .any(|(case, _, _)| case.contains(&format!("DataSource {data_source}"))),
            "case {data_source:?}: every modelled EventDataSource must have a row"
        );
    }

    for (case, filters, expected) in cases {
        let reply = guru.list_with(&filters).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {reply:?}"
        );
        assert_eq!(
            reply.ids(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.ids()
        );
    }
}

/// An instant as a JSON-protocol `Timestamp`: epoch seconds, fractional to the
/// millisecond the store keeps.
fn seconds(millis: i64) -> f64 {
    millis as f64 / 1000.0
}

/// RULE: a walk pages at the `MaxResults` the caller asked for — no page exceeds it and
/// the pages concatenate to the whole selection exactly once, with no token on the last
/// page — and a request naming none is served the service's own page size.
#[tokio::test]
async fn a_walk_pages_at_max_results_and_concatenates_once() {
    let guru = Guru::new();
    guru.seed(&rows());
    let expected = all_ids();

    // Each modelled bound of `ListEventsMaxResults` (1..200), sizes in between, and no
    // `MaxResults` at all.
    let sizes: Vec<Option<i64>> = vec![
        Some(1),
        Some(2),
        Some(3),
        Some(expected.len() as i64),
        Some(i64::from(LIST_EVENTS_MAX_RESULTS)),
        None,
    ];
    for size in sizes {
        let case = format!("MaxResults {size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let reply = guru.list(&json!({}), size, None, token.as_deref()).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {reply:?}"
            );
            let page = reply.ids();
            let limit = size.unwrap_or(PAGE_SIZE as i64) as usize;
            assert!(
                page.len() <= limit,
                "case {case:?}: a page must carry at most {limit} events, got {:?}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the walk must terminate, got {pages:?} pages"
            );
            match reply.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: the pages must concatenate to the whole selection, got {walked:?}"
        );
    }
}

/// RULE: a page token is bound to the filter set it was issued for, so another
/// selection's token, and a token whose page no longer exists, name no page.
#[tokio::test]
async fn a_token_is_bound_to_the_filters_it_was_issued_for() {
    let guru = Guru::new();
    guru.seed(&rows());

    let unfiltered = guru.list(&json!({}), Some(1), None, None).await;
    let token = unfiltered
        .next_token()
        .expect("a walk of six events at one per page must carry a token");

    // (case, the filters the token is replayed against)
    let foreign: Vec<(&str, Value)> = vec![
        (
            "a token of the unfiltered selection against a data-source filter",
            json!({ "DataSource": "AWS_CLOUD_TRAIL" }),
        ),
        (
            "a token of the unfiltered selection against an insight filter",
            json!({ "InsightId": PAYMENTS_INSIGHT }),
        ),
        (
            "a token of the unfiltered selection against a time-range filter",
            json!({ "EventTimeRange": {
                "FromTime": seconds(BASE_MILLIS),
                "ToTime": seconds(BASE_MILLIS + 10_000),
            } }),
        ),
        (
            "a token of the unfiltered selection against a resource-collection filter",
            json!({ "ResourceCollection": { "CloudFormation": { "StackNames": ["payments-api"] } } }),
        ),
    ];
    for (case, filters) in foreign {
        let reply = guru.list(&filters, Some(1), None, Some(&token)).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }

    // The same token resolves against the selection it WAS issued for.
    let continued = guru.list(&json!({}), Some(1), None, Some(&token)).await;
    assert_eq!(
        continued.status, 200,
        "case \"the token of its own selection\": expected HTTP 200, got {continued:?}"
    );

    // A token whose page no longer exists: the selection shrank under it.
    let shrink = Guru::new();
    shrink.seed(&rows());
    let first = shrink.list(&json!({}), Some(5), None, None).await;
    let stale = first
        .next_token()
        .expect("a walk of six events at five per page must carry a token");
    shrink.store().lock().events.clear();
    let reply = shrink.list(&json!({}), Some(5), None, Some(&stale)).await;
    assert_eq!(
        reply.status, 400,
        "case \"a cursor into a collection that has since shrunk\": expected HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_name, "ValidationException",
        "case \"a cursor into a collection that has since shrunk\": expected ValidationException, got {reply:?}"
    );
}

/// RULE: a filter value outside its modelled enum, and a `MaxResults` outside its
/// modelled range, are refused with HTTP 400 rather than answered with some page.
#[tokio::test]
async fn a_value_outside_its_modelled_domain_is_refused() {
    let guru = Guru::new();
    guru.seed(&rows());

    // (case, Filters, MaxResults)
    let cases: Vec<(&str, Value, Option<i64>)> = vec![
        (
            "an EventClass outside the modelled enum",
            json!({ "EventClass": "DEPLOYMENTS" }),
            None,
        ),
        (
            "an EventClass in the wrong case",
            json!({ "EventClass": "deployment" }),
            None,
        ),
        (
            "a DataSource outside the modelled enum",
            json!({ "DataSource": "AWS_CLOUDTRAIL" }),
            None,
        ),
        (
            "MaxResults below the modelled minimum",
            json!({}),
            Some(0),
        ),
        (
            "MaxResults one past the modelled maximum",
            json!({}),
            Some(i64::from(LIST_EVENTS_MAX_RESULTS) + 1),
        ),
        (
            "an EventSource shorter than the modelled minimum length",
            json!({ "EventSource": "aws.event" }),
            None,
        ),
    ];
    for (case, filters, max_results) in cases {
        let reply = guru.list(&filters, max_results, None, None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
    }

    // `Filters` is required, so a request carrying none is refused too.
    let missing = guru.call("POST", "/events", &json!({})).await;
    assert_eq!(
        missing.status, 400,
        "case \"no Filters at all\": expected HTTP 400, got {missing:?}"
    );
}

/// RULE: an `InsightId` the store holds no insight under names no insight and is
/// refused; an insight that exists with no event of its own is an empty list.
#[tokio::test]
async fn an_insight_id_naming_no_insight_is_refused_and_one_with_no_event_is_empty() {
    let guru = Guru::new();
    guru.seed(&rows());
    guru.seed_insight("i-quiet");

    for case in ["i-missing", "i_missing", "0"] {
        let reply = guru.list_with(&json!({ "InsightId": case })).await;
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

    let quiet = guru.list_with(&json!({ "InsightId": "i-quiet" })).await;
    assert_eq!(
        quiet.status, 200,
        "case \"an insight with no event of its own\": expected HTTP 200, got {quiet:?}"
    );
    assert_eq!(
        quiet.ids(),
        Vec::<String>::new(),
        "case \"an insight with no event of its own\": expected an empty list, got {:?}",
        quiet.ids()
    );
}

/// RULE: an `AccountId` other than the one this emulator serves is an authorization
/// refusal, decided before any lookup; an absent `AccountId` is the caller's own.
#[tokio::test]
async fn another_account_is_refused_before_any_lookup() {
    let guru = Guru::new();
    guru.seed(&rows());
    let own_account = ::engine::emulator_account_id().to_string();
    assert_ne!(
        own_account, OTHER_ACCOUNT_ID,
        "the suite's other account must not be the served one"
    );

    // (case, Filters, AccountId, expected status)
    let cases: Vec<(&str, Value, Option<&str>, u16)> = vec![
        ("no AccountId is the caller's own", json!({}), None, 200),
        (
            "the served account named explicitly",
            json!({}),
            Some(own_account.as_str()),
            200,
        ),
        (
            "another account",
            json!({}),
            Some(OTHER_ACCOUNT_ID),
            403,
        ),
        (
            "another account, with a filter naming a stored insight",
            json!({ "InsightId": PAYMENTS_INSIGHT }),
            Some(OTHER_ACCOUNT_ID),
            403,
        ),
        (
            "another account, with a filter naming no stored insight",
            json!({ "InsightId": "i-missing" }),
            Some(OTHER_ACCOUNT_ID),
            403,
        ),
    ];
    for (case, filters, account_id, expected) in cases {
        let reply = guru.list(&filters, None, account_id, None).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {reply:?}"
        );
        if expected == 403 {
            assert_eq!(
                reply.error_name, "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {reply:?}"
            );
        }
    }

    // The refusal is identical whether or not the insight exists, so it reports
    // nothing about the other account's store.
    let known = guru
        .list(
            &json!({ "InsightId": PAYMENTS_INSIGHT }),
            None,
            Some(OTHER_ACCOUNT_ID),
            None,
        )
        .await;
    let unknown = guru
        .list(
            &json!({ "InsightId": "i-missing" }),
            None,
            Some(OTHER_ACCOUNT_ID),
            None,
        )
        .await;
    assert_eq!(
        known.body, unknown.body,
        "case \"a refusal must not report whether the insight exists\": got {known:?} and {unknown:?}"
    );

    // An absent `AccountId` and the served account named explicitly answer the same.
    let implicit = guru.list(&json!({}), None, None, None).await;
    let explicit = guru
        .list(&json!({}), None, Some(own_account.as_str()), None)
        .await;
    assert_eq!(
        implicit.body, explicit.body,
        "case \"an absent AccountId is the caller's own\": got {implicit:?} and {explicit:?}"
    );
}

/// RULE: it reports — the store is identical before and after, however often and
/// however it is read.
#[tokio::test]
async fn reading_does_not_change_what_a_later_read_sees() {
    let guru = Guru::new();
    guru.seed(&rows());
    let before = guru.snapshot();

    let reads: Vec<(Value, Option<i64>, Option<&str>)> = vec![
        (json!({}), None, None),
        (json!({}), Some(2), None),
        (json!({ "InsightId": PAYMENTS_INSIGHT }), None, None),
        (json!({ "EventClass": "DEPLOYMENT" }), None, None),
        (json!({ "DataSource": "AWS_CLOUD_TRAIL" }), Some(1), None),
        (json!({ "InsightId": "i-missing" }), None, None),
        (json!({}), None, Some(OTHER_ACCOUNT_ID)),
        (json!({ "EventClass": "nonsense" }), None, None),
    ];
    for (filters, max_results, account_id) in &reads {
        let first = guru.list(filters, *max_results, *account_id, None).await;
        let again = guru.list(filters, *max_results, *account_id, None).await;
        assert_eq!(
            first.status, again.status,
            "case {filters}: a repeated read must answer the same, got {first:?} then {again:?}"
        );
        assert_eq!(
            first.body, again.body,
            "case {filters}: a repeated read must answer the same, got {first:?} then {again:?}"
        );
    }
    assert_eq!(
        guru.snapshot(),
        before,
        "case \"the store after every read\": a reporting operation must not write"
    );
}

/// RULE: `DeleteInsight` deletes the insight along with the events associated with it,
/// leaving another insight's events, and the events related to none, alone.
#[tokio::test]
async fn deleting_an_insight_deletes_the_events_related_to_it() {
    let guru = Guru::new();
    guru.seed(&rows());

    let deleted = guru.delete_insight(PAYMENTS_INSIGHT).await;
    assert_eq!(
        deleted.status, 200,
        "case {PAYMENTS_INSIGHT:?}: the insight must be deleted, got {deleted:?}"
    );

    let remaining = guru.list_with(&json!({})).await;
    assert_eq!(
        remaining.ids(),
        ids(&[
            "e-0003-infrastructure",
            "e-0004-security-change",
            "e-0005-schema-change",
            "e-0006-bare",
        ]),
        "case {PAYMENTS_INSIGHT:?}: its events must go with it and no others, got {:?}",
        remaining.ids()
    );

    let orphaned = guru
        .list_with(&json!({ "InsightId": PAYMENTS_INSIGHT }))
        .await;
    assert_eq!(
        orphaned.status, 404,
        "case {PAYMENTS_INSIGHT:?}: the deleted insight names no insight, got {orphaned:?}"
    );

    let other = guru
        .list_with(&json!({ "InsightId": CHECKOUT_INSIGHT }))
        .await;
    assert_eq!(
        other.ids(),
        ids(&["e-0003-infrastructure"]),
        "case {CHECKOUT_INSIGHT:?}: another insight's events are untouched, got {:?}",
        other.ids()
    );
}
