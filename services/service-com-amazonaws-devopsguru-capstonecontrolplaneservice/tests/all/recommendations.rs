// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRecommendations`, the per-insight recommendation read.
//!
//! The RULES these tests pin:
//!
//! * The read reports the recommendations the store holds FOR the requested insight,
//!   each rendered with exactly the members `Recommendation` models and the values the
//!   record holds, in the store's stable `Name` order.
//! * `Locale` chooses the LANGUAGE the prose is reported in: a recommendation the
//!   analysis rendered in the requested language is reported in it, one it rendered in
//!   no such language is reported in the default language, and a request naming no
//!   locale is served the default language. The language never changes WHICH
//!   recommendations are reported.
//! * A locale outside the modelled `Locale` enum is refused with `ValidationException`
//!   (400).
//! * An `InsightId` the store holds no insight under names no insight and is refused
//!   with `ResourceNotFoundException` (404), naming the id asked for; an insight that
//!   exists with no recommendation of its own is an empty list, not a refusal.
//! * An `AccountId` other than the one this emulator serves is an authorization
//!   refusal (`AccessDeniedException`, 403), decided before any lookup, so it cannot
//!   report whether the insight exists; an absent `AccountId` is the caller's own.
//! * A walk pages at the modelled maximum of `Recommendations`: no page exceeds it,
//!   and the pages concatenate to the whole collection exactly once, with no token on
//!   the last page.
//! * A page token names an offset into ONE insight's recommendations, so another
//!   insight's token, and a token whose page no longer exists, are refused with
//!   `ValidationException` (400) rather than resolving to some other page.
//! * It reports: the store is identical before and after, however often it is read.
//! * `DeleteInsight` deletes the insight "along with the associated anomalies, events
//!   and recommendations", so its recommendations go with it while another insight's
//!   are left alone.
//!
//! DevOps Guru derives a recommendation from the anomalies that generated an insight
//! through its own analysis, so no client request creates one; these tests therefore
//! write records straight into the engine's store (the same store the handler reads
//! through `crate::state::service_state`) and then drive the operation over the wire
//! through the same [`MultiServiceRouter`] the emulator boots.
//!
//! Each test exercises its rule over a SET of inputs differing along what the rule
//! generalises over: how many recommendations an insight has, which optional members
//! each carries, how many related events and anomalies it relates and how much
//! evidence each of those names, which language is asked for and which languages the
//! record holds, the requested page size boundary, the shape of the requested insight
//! id and which account is named.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::ops::types::{
    RecommendationRelatedAnomaly, RecommendationRelatedAnomalyResource,
    RecommendationRelatedAnomalySourceDetail, RecommendationRelatedCloudWatchMetricsSourceDetail,
    RecommendationRelatedEvent, RecommendationRelatedEventResource, LOCALE_VALUES,
};
use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord, RecommendationRecord,
    RecommendationText, DEFAULT_LOCALE, RECOMMENDATIONS_PAGE_SIZE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A base instant (epoch milliseconds) the seeded insights start at.
const BASE_MILLIS: i64 = 1_700_000_000_123;

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

    /// The reported `Recommendations`, in the order the reply listed them.
    fn recommendations(&self) -> Vec<Value> {
        self.at("/Recommendations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The reported `Name`s, in the order the reply listed them.
    fn names(&self) -> Vec<String> {
        self.recommendations()
            .iter()
            .map(|item| {
                item.get("Name")
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

    /// Seed the recommendations the analysis would have derived for `insight_id`, and
    /// the insight itself.
    fn seed(&self, insight_id: &str, records: Vec<RecommendationRecord>) {
        self.seed_insight(insight_id);
        let state = self.store();
        let mut store = state.lock();
        let per_name = store
            .recommendations
            .entry(insight_id.to_string())
            .or_default();
        for record in records {
            per_name.insert(record.name.clone(), record);
        }
    }

    /// Drop every recommendation of `insight_id`, so a cursor into the earlier
    /// collection names a page that no longer exists.
    fn clear_recommendations(&self, insight_id: &str) {
        self.store().lock().recommendations.remove(insight_id);
    }

    /// A stable rendering of the whole store, for the "a read does not write" check.
    fn snapshot(&self) -> String {
        format!("{:?}", *self.store().lock())
    }

    async fn list(
        &self,
        insight_id: &str,
        locale: Option<&str>,
        account_id: Option<&str>,
        token: Option<&str>,
    ) -> Reply {
        let mut body = json!({ "InsightId": insight_id });
        if let Some(locale) = locale {
            body["Locale"] = json!(locale);
        }
        if let Some(account_id) = account_id {
            body["AccountId"] = json!(account_id);
        }
        if let Some(token) = token {
            body["NextToken"] = json!(token);
        }
        self.call("POST", "/recommendations", &body).await
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

/// One language's rendering of a row's prose: the locale, its `Description` and its
/// `Reason`. The model makes both optional, so a rendering may carry either alone.
type Rendering = (&'static str, Option<&'static str>, Option<&'static str>);

/// One related event of a row: its `Name` and the `(name, type)` of each resource it
/// names.
type EventRow = (Option<&'static str>, Vec<(&'static str, &'static str)>);

/// One related anomaly of a row: its `AnomalyId`, the `(name, type)` of each resource
/// it names, and the `(namespace, metric name)` of each CloudWatch metric its source
/// details name, one source-detail entry per pair.
type AnomalyRow = (
    Option<&'static str>,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
);

/// One row of the round-trip table: a recommendation as the analysis would have
/// derived it.
#[derive(Debug, Clone)]
struct Row {
    name: &'static str,
    link: Option<&'static str>,
    /// `RecommendationCategory` is an unconstrained string, so a row carries whatever
    /// the analysis categorised the recommendation as, or nothing.
    category: Option<&'static str>,
    renderings: Vec<Rendering>,
    events: Vec<EventRow>,
    anomalies: Vec<AnomalyRow>,
}

impl Row {
    fn record(&self, insight_id: &str) -> RecommendationRecord {
        RecommendationRecord {
            insight_id: insight_id.to_string(),
            name: self.name.to_string(),
            link: self.link.map(str::to_string),
            category: self.category.map(str::to_string),
            text: self
                .renderings
                .iter()
                .map(|(locale, description, reason)| {
                    (
                        (*locale).to_string(),
                        RecommendationText {
                            description: description.map(str::to_string),
                            reason: reason.map(str::to_string),
                        },
                    )
                })
                .collect::<BTreeMap<String, RecommendationText>>(),
            related_events: self
                .events
                .iter()
                .map(|(name, resources)| RecommendationRelatedEvent {
                    name: name.map(str::to_string),
                    resources: (!resources.is_empty()).then(|| {
                        resources
                            .iter()
                            .map(|(name, r#type)| RecommendationRelatedEventResource {
                                name: Some((*name).to_string()),
                                r#type: Some((*r#type).to_string()),
                            })
                            .collect()
                    }),
                })
                .collect(),
            related_anomalies: self
                .anomalies
                .iter()
                .map(|(anomaly_id, resources, metrics)| RecommendationRelatedAnomaly {
                    anomaly_id: anomaly_id.map(str::to_string),
                    resources: (!resources.is_empty()).then(|| {
                        resources
                            .iter()
                            .map(|(name, r#type)| RecommendationRelatedAnomalyResource {
                                name: Some((*name).to_string()),
                                r#type: Some((*r#type).to_string()),
                            })
                            .collect()
                    }),
                    source_details: (!metrics.is_empty()).then(|| {
                        metrics
                            .iter()
                            .map(|(namespace, metric_name)| {
                                RecommendationRelatedAnomalySourceDetail {
                                    cloud_watch_metrics: Some(vec![
                                        RecommendationRelatedCloudWatchMetricsSourceDetail {
                                            namespace: Some((*namespace).to_string()),
                                            metric_name: Some((*metric_name).to_string()),
                                        },
                                    ]),
                                }
                            })
                            .collect()
                    }),
                })
                .collect(),
        }
    }

    /// The rendering this row's prose is reported in when `locale` is asked for: the
    /// requested language when the analysis produced it, else the default language,
    /// else nothing.
    fn rendering(&self, locale: &str) -> Option<&Rendering> {
        self.renderings
            .iter()
            .find(|(held, _, _)| *held == locale)
            .or_else(|| {
                self.renderings
                    .iter()
                    .find(|(held, _, _)| *held == DEFAULT_LOCALE)
            })
    }

    /// This row as the reply must report it in `locale`, with every absent optional
    /// member left out.
    fn expected(&self, locale: &str) -> Value {
        let mut expected = Map::new();
        expected.insert("Name".to_string(), json!(self.name));
        if let Some((_, description, reason)) = self.rendering(locale) {
            if let Some(description) = description {
                expected.insert("Description".to_string(), json!(description));
            }
            if let Some(reason) = reason {
                expected.insert("Reason".to_string(), json!(reason));
            }
        }
        if let Some(link) = self.link {
            expected.insert("Link".to_string(), json!(link));
        }
        if let Some(category) = self.category {
            expected.insert("Category".to_string(), json!(category));
        }
        if !self.events.is_empty() {
            let events: Vec<Value> = self
                .events
                .iter()
                .map(|(name, resources)| {
                    let mut event = Map::new();
                    if let Some(name) = name {
                        event.insert("Name".to_string(), json!(name));
                    }
                    if !resources.is_empty() {
                        event.insert(
                            "Resources".to_string(),
                            json!(resources
                                .iter()
                                .map(|(name, r#type)| json!({ "Name": name, "Type": r#type }))
                                .collect::<Vec<Value>>()),
                        );
                    }
                    Value::Object(event)
                })
                .collect();
            expected.insert("RelatedEvents".to_string(), json!(events));
        }
        if !self.anomalies.is_empty() {
            let anomalies: Vec<Value> = self
                .anomalies
                .iter()
                .map(|(anomaly_id, resources, metrics)| {
                    let mut anomaly = Map::new();
                    if let Some(anomaly_id) = anomaly_id {
                        anomaly.insert("AnomalyId".to_string(), json!(anomaly_id));
                    }
                    if !resources.is_empty() {
                        anomaly.insert(
                            "Resources".to_string(),
                            json!(resources
                                .iter()
                                .map(|(name, r#type)| json!({ "Name": name, "Type": r#type }))
                                .collect::<Vec<Value>>()),
                        );
                    }
                    if !metrics.is_empty() {
                        anomaly.insert(
                            "SourceDetails".to_string(),
                            json!(metrics
                                .iter()
                                .map(|(namespace, metric_name)| json!({
                                    "CloudWatchMetrics": [
                                        { "Namespace": namespace, "MetricName": metric_name }
                                    ]
                                }))
                                .collect::<Vec<Value>>()),
                        );
                    }
                    Value::Object(anomaly)
                })
                .collect();
            expected.insert("RelatedAnomalies".to_string(), json!(anomalies));
        }
        Value::Object(expected)
    }
}

/// The round-trip table: recommendations differing in which optional members they
/// carry, how many languages the analysis rendered their prose in, how many events and
/// anomalies they relate and how much evidence each of those names.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            name: "Scale the Lambda function's reserved concurrency",
            link: Some("https://docs.aws.amazon.com/lambda/latest/dg/configuration-concurrency.html"),
            category: Some("PERFORMANCE"),
            renderings: vec![
                (
                    "EN_US",
                    Some("Invocations are being throttled."),
                    Some("The function reached its concurrency limit."),
                ),
                (
                    "JA_JP",
                    Some("呼び出しがスロットリングされています。"),
                    Some("関数が同時実行数の上限に達しました。"),
                ),
            ],
            events: vec![(
                Some("UpdateFunctionConfiguration"),
                vec![("payments", "AWS::Lambda::Function")],
            )],
            anomalies: vec![(
                Some("anomaly-throttles-1"),
                vec![("payments", "AWS::Lambda::Function")],
                vec![("AWS/Lambda", "Throttles"), ("AWS/Lambda", "Duration")],
            )],
        },
        Row {
            // Every optional member of the recommendation absent except the prose: no
            // link, no category, nothing related.
            name: "Add a read replica",
            link: None,
            category: None,
            renderings: vec![("EN_US", Some("Read latency is rising."), None)],
            events: Vec::new(),
            anomalies: Vec::new(),
        },
        Row {
            // Prose in a language that is NOT the default, and none in the default: a
            // request for another language falls back to nothing rather than to text
            // the analysis never produced.
            name: "Erhöhen Sie die DynamoDB-Kapazität",
            link: Some("https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/"),
            category: Some("COST"),
            renderings: vec![("DE_DE", Some("Die Tabelle wird gedrosselt."), Some("Die Kapazität ist zu niedrig."))],
            events: vec![
                // An event the analysis named no resources for, beside one naming two.
                (Some("ConsoleLogin"), Vec::new()),
                (
                    Some("UpdateTable"),
                    vec![
                        ("orders", "AWS::DynamoDB::Table"),
                        ("orders-index", "AWS::DynamoDB::Index"),
                    ],
                ),
            ],
            anomalies: vec![
                (
                    Some("anomaly-capacity-1"),
                    Vec::new(),
                    vec![("AWS/DynamoDB", "ThrottledRequests")],
                ),
                // A related anomaly with neither resources nor source details.
                (Some("anomaly-capacity-2"), Vec::new(), Vec::new()),
            ],
        },
        Row {
            // A recommendation whose prose carries only a reason, in the default
            // language and in two others.
            name: "Zero in on the deployment",
            link: None,
            category: Some("AVAILABILITY"),
            renderings: vec![
                ("EN_US", None, Some("A deployment preceded the anomaly.")),
                ("PT_BR", None, Some("Uma implantação precedeu a anomalia.")),
                ("ZH_TW", None, Some("異常發生前曾進行部署。")),
            ],
            events: vec![(None, vec![("checkout", "AWS::ECS::Service")])],
            anomalies: vec![(None, vec![("checkout", "AWS::ECS::Service")], Vec::new())],
        },
    ]
}

/// The languages this suite's renderings are drawn from are the model's own enum —
/// `LOCALE_VALUES` in the service's `ops/types/shared_types.rs`, which is this
/// catalog's source.
#[test]
fn seeded_locales_are_modelled_values() {
    for row in round_trip_rows() {
        for (locale, _, _) in &row.renderings {
            assert!(
                LOCALE_VALUES.contains(locale),
                "case {}/{locale}: {locale} is not a modelled Locale, which are {:?}",
                row.name,
                LOCALE_VALUES
            );
        }
    }
    assert!(
        LOCALE_VALUES.contains(&DEFAULT_LOCALE),
        "case default locale: expected {DEFAULT_LOCALE} among the modelled locales, got {LOCALE_VALUES:?}"
    );
}

/// RULE: the read reports the recommendations the store holds for the requested
/// insight, with exactly the members and values each record holds, in `Name` order.
#[tokio::test]
async fn reports_each_stored_recommendation_of_the_insight() {
    let guru = Guru::new();
    let insight_id = "insight-recs-1";
    let rows = round_trip_rows();
    guru.seed(
        insight_id,
        rows.iter().map(|row| row.record(insight_id)).collect(),
    );

    let reply = guru.list(insight_id, None, None, None).await;
    assert_eq!(
        reply.status, 200,
        "case {insight_id}: expected HTTP 200, got {reply:?}"
    );

    let mut expected_names: Vec<String> = rows.iter().map(|row| row.name.to_string()).collect();
    expected_names.sort();
    assert_eq!(
        reply.names(),
        expected_names,
        "case {insight_id}: expected the stored recommendations in Name order, got {reply:?}"
    );

    let reported = reply.recommendations();
    for row in &rows {
        let position = reply
            .names()
            .iter()
            .position(|name| name == row.name)
            .unwrap_or_else(|| panic!("case {}: not reported, got {reply:?}", row.name));
        assert_eq!(
            reported[position],
            row.expected(DEFAULT_LOCALE),
            "case {}: expected {}, got {:?}",
            row.name,
            row.expected(DEFAULT_LOCALE),
            reported[position]
        );
    }
    assert_eq!(
        reply.next_token(),
        None,
        "case {insight_id}: expected no NextToken on a single page, got {reply:?}"
    );
}

/// RULE: an insight that exists with no recommendation of its own is an empty list,
/// not a refusal.
#[tokio::test]
async fn an_insight_with_no_recommendation_reports_an_empty_list() {
    let guru = Guru::new();
    for case in ["insight-empty", "insight_empty_2", "I-3"] {
        guru.seed_insight(case);
        let reply = guru.list(case, None, None, None).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected HTTP 200, got {reply:?}"
        );
        assert_eq!(
            reply.recommendations(),
            Vec::<Value>::new(),
            "case {case}: expected an empty Recommendations list, got {reply:?}"
        );
        assert_eq!(
            reply.next_token(),
            None,
            "case {case}: expected no NextToken, got {reply:?}"
        );
    }
}

/// RULE: `Locale` chooses the language the prose is reported in — the requested
/// language when the analysis produced it, else the default language — and never which
/// recommendations are reported.
#[tokio::test]
async fn prose_is_reported_in_the_requested_language() {
    let guru = Guru::new();
    let insight_id = "insight-locales";
    let rows = round_trip_rows();
    guru.seed(
        insight_id,
        rows.iter().map(|row| row.record(insight_id)).collect(),
    );

    let mut expected_names: Vec<String> = rows.iter().map(|row| row.name.to_string()).collect();
    expected_names.sort();

    // Every language the model declares, plus the request that names none.
    let requested: Vec<Option<&str>> = std::iter::once(None)
        .chain(LOCALE_VALUES.iter().map(|locale| Some(*locale)))
        .collect();
    for locale in requested {
        let reply = guru.list(insight_id, locale, None, None).await;
        let case = locale.unwrap_or("<no Locale>");
        assert_eq!(
            reply.status, 200,
            "case {case}: expected HTTP 200, got {reply:?}"
        );
        assert_eq!(
            reply.names(),
            expected_names,
            "case {case}: expected the same recommendations whichever language is asked for, got {reply:?}"
        );
        let reported = reply.recommendations();
        let served = locale.unwrap_or(DEFAULT_LOCALE);
        for row in &rows {
            let position = reply
                .names()
                .iter()
                .position(|name| name == row.name)
                .unwrap_or_else(|| panic!("case {case}/{}: not reported, got {reply:?}", row.name));
            assert_eq!(
                reported[position],
                row.expected(served),
                "case {case}/{}: expected {}, got {:?}",
                row.name,
                row.expected(served),
                reported[position]
            );
        }
    }
}

/// RULE: a locale outside the modelled `Locale` enum is refused with 400
/// `ValidationException`.
#[tokio::test]
async fn a_locale_outside_the_modelled_enum_is_refused() {
    let guru = Guru::new();
    let insight_id = "insight-bad-locale";
    guru.seed(insight_id, vec![round_trip_rows()[0].record(insight_id)]);

    for case in ["en_us", "EN-US", "XX_XX", "", "EN_US "] {
        let reply = guru.list(insight_id, Some(case), None, None).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ValidationException",
            "case {case:?}: expected a ValidationException, got {reply:?}"
        );
    }
}

/// RULE: an `InsightId` the store holds no insight under is refused with 404
/// `ResourceNotFoundException` naming the id asked for.
#[tokio::test]
async fn an_insight_the_store_does_not_hold_is_refused() {
    let guru = Guru::new();
    let held = "insight-held";
    guru.seed(held, vec![round_trip_rows()[0].record(held)]);

    // Ids differing in shape, each matching `InsightId`'s `^[\w-]*$` pattern: a
    // neighbour of the held id, an underscored one, a single character, and the
    // modelled maximum length of 100.
    for case in [
        "insight-held-2",
        "insight_held",
        "X",
        &"a".repeat(100),
        "INSIGHT-HELD",
    ] {
        let reply = guru.list(case, None, None, None).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: expected HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ResourceNotFoundException",
            "case {case}: expected a ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceId"),
            Some(case),
            "case {case}: expected the refused id reported, got {reply:?}"
        );
        assert_eq!(
            reply.str_at("/ResourceType"),
            Some(INSIGHT_RESOURCE_TYPE),
            "case {case}: expected ResourceType {INSIGHT_RESOURCE_TYPE}, got {reply:?}"
        );
    }
}

/// RULE: an `AccountId` other than the one this emulator serves is an authorization
/// refusal, decided before any lookup, so it cannot report whether the insight exists;
/// an absent `AccountId` is the caller's own account.
#[tokio::test]
async fn another_accounts_recommendations_are_refused() {
    let guru = Guru::new();
    let held = "insight-tenancy";
    guru.seed(held, vec![round_trip_rows()[0].record(held)]);
    let own_account = ::engine::emulator_account_id().to_string();

    // (requested insight, requested AccountId, expected status): twelve-digit ids, as
    // `AwsAccountId`'s `^\d{12}$` pattern requires.
    let cases: Vec<(&str, Option<&str>, u16)> = vec![
        (held, None, 200),
        (held, Some(own_account.as_str()), 200),
        (held, Some("210987654321"), 403),
        (held, Some("123456789012"), 403),
        // A foreign account is refused the SAME way whether the insight exists or not.
        ("insight-absent", Some("210987654321"), 403),
        ("insight-absent", None, 404),
    ];
    for (insight_id, account_id, expected) in cases {
        let reply = guru.list(insight_id, None, account_id, None).await;
        let case = format!("{insight_id} as {account_id:?}");
        assert_eq!(
            reply.status, expected,
            "case {case}: expected HTTP {expected}, got {reply:?}"
        );
        if expected == 403 {
            assert_eq!(
                reply.error_name, "AccessDeniedException",
                "case {case}: expected an AccessDeniedException, got {reply:?}"
            );
        }
    }
}

/// A recommendation named `index`, in the default language only.
fn numbered(insight_id: &str, index: usize) -> RecommendationRecord {
    RecommendationRecord {
        insight_id: insight_id.to_string(),
        name: format!("recommendation-{index:03}"),
        link: None,
        category: Some("PERFORMANCE".to_string()),
        text: BTreeMap::from([(
            DEFAULT_LOCALE.to_string(),
            RecommendationText {
                description: Some(format!("problem {index}")),
                reason: Some(format!("reason {index}")),
            },
        )]),
        related_events: Vec::new(),
        related_anomalies: Vec::new(),
    }
}

/// RULE: a walk pages at the modelled maximum of `Recommendations`, and the pages
/// concatenate to the whole collection exactly once with no token on the last page.
#[tokio::test]
async fn a_walk_pages_at_the_modelled_maximum() {
    // Counts spanning the page boundary: none, one, one short of a page, exactly a
    // page, one past it, and well past two pages.
    for count in [
        0,
        1,
        RECOMMENDATIONS_PAGE_SIZE - 1,
        RECOMMENDATIONS_PAGE_SIZE,
        RECOMMENDATIONS_PAGE_SIZE + 1,
        RECOMMENDATIONS_PAGE_SIZE * 2 + 3,
    ] {
        let guru = Guru::new();
        let insight_id = "insight-pages";
        guru.seed(
            insight_id,
            (0..count).map(|index| numbered(insight_id, index)).collect(),
        );

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let reply = guru.list(insight_id, None, None, token.as_deref()).await;
            assert_eq!(
                reply.status, 200,
                "case {count} recommendations: expected HTTP 200, got {reply:?}"
            );
            let page = reply.names();
            assert!(
                page.len() <= RECOMMENDATIONS_PAGE_SIZE,
                "case {count} recommendations: expected at most {RECOMMENDATIONS_PAGE_SIZE} per page, got {} in {reply:?}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= count / RECOMMENDATIONS_PAGE_SIZE + 2,
                "case {count} recommendations: the walk did not terminate, got {reply:?}"
            );
            match reply.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }

        let expected: Vec<String> = (0..count)
            .map(|index| format!("recommendation-{index:03}"))
            .collect();
        assert_eq!(
            walked, expected,
            "case {count} recommendations: expected the whole collection once in Name order, got {walked:?}"
        );
    }
}

/// RULE: a page token names an offset into ONE insight's recommendations, so another
/// insight's token, and a token whose page no longer exists, are refused with 400
/// `ValidationException`.
#[tokio::test]
async fn a_token_of_another_collection_or_a_shrunken_one_is_refused() {
    let guru = Guru::new();
    let first = "insight-token-a";
    let second = "insight-token-b";
    for insight_id in [first, second] {
        guru.seed(
            insight_id,
            (0..RECOMMENDATIONS_PAGE_SIZE + 2)
                .map(|index| numbered(insight_id, index))
                .collect(),
        );
    }

    let token = guru
        .list(first, None, None, None)
        .await
        .next_token()
        .expect("the first page of a two-page collection must carry a NextToken");

    // The token resolves against the collection it was issued for.
    let reply = guru.list(first, None, None, Some(&token)).await;
    assert_eq!(
        reply.status, 200,
        "case own token: expected HTTP 200, got {reply:?}"
    );

    // (case, insight the token is replayed against, the token)
    let past_the_end = format!("{:08x}-{}", 0xffff, &token[9..]);
    let cases: Vec<(&str, &str, String)> = vec![
        ("another insight's token", second, token.clone()),
        ("a token whose offset is past the end", first, past_the_end),
        (
            "a token of another collection entirely",
            first,
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
    ];
    for (case, insight_id, replayed) in cases {
        let reply = guru.list(insight_id, None, None, Some(&replayed)).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected HTTP 400, got {reply:?}"
        );
        assert_eq!(
            reply.error_name, "ValidationException",
            "case {case}: expected a ValidationException, got {reply:?}"
        );
    }

    // A token this service DID issue, into a collection that has since shrunk: the
    // page it named no longer exists.
    guru.clear_recommendations(first);
    let reply = guru.list(first, None, None, Some(&token)).await;
    assert_eq!(
        reply.status, 400,
        "case a stale cursor: expected HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_name, "ValidationException",
        "case a stale cursor: expected a ValidationException, got {reply:?}"
    );
}

/// One read of the "a read does not write" table.
#[derive(Debug, Clone, Copy)]
struct Read<'a> {
    case: &'a str,
    insight_id: &'a str,
    locale: Option<&'a str>,
    account_id: Option<&'a str>,
    token: Option<&'a str>,
}

/// RULE: it reports — the store is identical before and after, however often and in
/// whichever language it is read.
#[tokio::test]
async fn reading_recommendations_does_not_change_the_store() {
    let guru = Guru::new();
    let insight_id = "insight-readonly";
    let rows = round_trip_rows();
    guru.seed(
        insight_id,
        rows.iter().map(|row| row.record(insight_id)).collect(),
    );
    guru.seed(
        "insight-readonly-pages",
        (0..RECOMMENDATIONS_PAGE_SIZE + 1)
            .map(|index| numbered("insight-readonly-pages", index))
            .collect(),
    );
    let before = guru.snapshot();

    // Reads differing in language, in page and in whether they are refused: none of
    // them may leave a trace in the store.
    let token = guru
        .list("insight-readonly-pages", None, None, None)
        .await
        .next_token()
        .expect("a two-page collection must carry a NextToken");
    let reads: Vec<Read<'_>> = vec![
        Read {
            case: "first read",
            insight_id,
            locale: None,
            account_id: None,
            token: None,
        },
        Read {
            case: "read in German",
            insight_id,
            locale: Some("DE_DE"),
            account_id: None,
            token: None,
        },
        Read {
            case: "read in Japanese",
            insight_id,
            locale: Some("JA_JP"),
            account_id: None,
            token: None,
        },
        Read {
            case: "read again",
            insight_id,
            locale: None,
            account_id: None,
            token: None,
        },
        Read {
            case: "second page",
            insight_id: "insight-readonly-pages",
            locale: None,
            account_id: None,
            token: Some(&token),
        },
        Read {
            case: "refused insight",
            insight_id: "insight-absent",
            locale: None,
            account_id: None,
            token: None,
        },
        Read {
            case: "refused account",
            insight_id,
            locale: None,
            account_id: Some("210987654321"),
            token: None,
        },
    ];
    for read in reads {
        let reply = guru
            .list(read.insight_id, read.locale, read.account_id, read.token)
            .await;
        let case = read.case;
        assert!(
            reply.status == 200 || reply.status >= 400,
            "case {case}: expected an answer, got {reply:?}"
        );
        assert_eq!(
            guru.snapshot(),
            before,
            "case {case}: expected the store unchanged by a read, got a different store"
        );
    }
}

/// RULE: `DeleteInsight` deletes the insight "along with the associated anomalies,
/// events and recommendations", so its recommendations go with it while another
/// insight's are left alone.
#[tokio::test]
async fn deleting_an_insight_deletes_its_recommendations() {
    let guru = Guru::new();
    let deleted = "insight-deleted";
    let kept = "insight-kept";
    for insight_id in [deleted, kept] {
        guru.seed(
            insight_id,
            round_trip_rows()
                .iter()
                .map(|row| row.record(insight_id))
                .collect(),
        );
    }

    let before = guru.list(deleted, None, None, None).await;
    assert_eq!(
        before.names().len(),
        round_trip_rows().len(),
        "case before the delete: expected every seeded recommendation, got {before:?}"
    );

    let delete = guru.delete_insight(deleted).await;
    assert_eq!(
        delete.status, 200,
        "case the delete: expected HTTP 200, got {delete:?}"
    );

    let after = guru.list(deleted, None, None, None).await;
    assert_eq!(
        after.status, 404,
        "case after the delete: expected HTTP 404 for the deleted insight, got {after:?}"
    );
    assert!(
        guru.store()
            .lock()
            .recommendations
            .get(deleted)
            .is_none_or(|per_name| per_name.is_empty()),
        "case after the delete: expected the deleted insight's recommendations reclaimed"
    );

    let other = guru.list(kept, None, None, None).await;
    assert_eq!(
        other.status, 200,
        "case the other insight: expected HTTP 200, got {other:?}"
    );
    assert_eq!(
        other.names().len(),
        round_trip_rows().len(),
        "case the other insight: expected its recommendations untouched, got {other:?}"
    );
}
