// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE every DevOps Guru not-found refusal carries.
//!
//! The RULE these tests pin: a `ResourceNotFoundException` this service raises
//! reports a fixed sentence, not a per-handler phrasing — the refusing operation's
//! own name in brackets, then
//! `failed - Resource Not found for awsAccountId [<account>] for type [<lookup
//! type>] with id [<id>]`, whose variable parts are the account the request resolved
//! to (its `AccountId` when it carries one, otherwise the caller's own account), the
//! kind of identifier the lookup was resolving (`findingId` for an insight id, which
//! is the identifier DevOps Guru resolves an insight by, `anomalyId` for an anomaly
//! id) and the id the request asked for.
//!
//! The rule is about the refusal, not about one operation, so each test walks EVERY
//! operation that refuses an id it cannot resolve, against a set of ids spanning what
//! `InsightId` / `AnomalyId` admit (one character, a word, a uuid-shaped id, the
//! 100-character maximum) and both ways of naming the account.
//!
//! The error's own modelled `ResourceId` and `ResourceType` members are asserted
//! alongside, so a change to the message cannot quietly drop them.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    AnomalyRecord, ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord,
    ANOMALY_RESOURCE_TYPE, INSIGHT_RESOURCE_TYPE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// A base instant (epoch milliseconds) the seeded records are placed at.
const BASE_MILLIS: i64 = 1_700_000_000_123;

/// The identifier kind the message reports for an `InsightId`. Written out here
/// rather than imported, so the test states the wording independently of the code
/// that renders it.
const FINDING_ID: &str = "findingId";

/// The identifier kind the message reports for an `AnomalyId`, for the same reason.
const ANOMALY_ID: &str = "anomalyId";

/// The id of the insight every suite seeds, so a refusal below is about the id the
/// request asked for and not about an empty store.
const STORED_INSIGHT: &str = "insight-stored";

/// The id of the seeded anomaly, under the same reasoning.
const STORED_ANOMALY: &str = "anomaly-stored";

/// One reply, as a client sees it.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
    /// The error SHAPE NAME the reply carries, from the `x-amzn-errortype` header or
    /// the body's `__type`, with any namespace/URL prefix stripped. Empty on success.
    error_name: String,
    raw: String,
}

impl Reply {
    fn str_at(&self, pointer: &str) -> Option<&str> {
        self.body.pointer(pointer).and_then(Value::as_str)
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
        let guru = Self { router, engine };
        guru.seed();
        guru
    }

    /// The very store the handler reads: `state::<T>()` is keyed by type, so this is
    /// the same value `crate::state::service_state` hands a request.
    fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState> {
        self.engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>()
    }

    /// The one insight and the one anomaly DevOps Guru's analysis would have raised.
    fn seed(&self) {
        let state = self.store();
        let mut store = state.lock();
        store.insights.insert(
            STORED_INSIGHT.to_string(),
            InsightRecord {
                id: STORED_INSIGHT.to_string(),
                name: "stored insight".to_string(),
                insight_type: InsightRecord::PROACTIVE.to_string(),
                severity: "HIGH".to_string(),
                status: "ONGOING".to_string(),
                start_time_millis: BASE_MILLIS,
                ..InsightRecord::default()
            },
        );
        store.anomalies.insert(
            STORED_ANOMALY.to_string(),
            AnomalyRecord {
                id: STORED_ANOMALY.to_string(),
                proactive: true,
                severity: "HIGH".to_string(),
                status: "ONGOING".to_string(),
                start_time_millis: BASE_MILLIS,
                ..AnomalyRecord::default()
            },
        );
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

/// How the account a request resolves to is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// No `AccountId` member at all: the caller's own account.
    Caller,
    /// The caller's own account, named explicitly.
    OwnAccountNamed,
}

impl Scope {
    const ALL: [Scope; 2] = [Scope::Caller, Scope::OwnAccountNamed];

    /// The `AccountId` the request carries, if any.
    fn account_id(self) -> Option<String> {
        match self {
            Scope::Caller => None,
            Scope::OwnAccountNamed => Some(::engine::emulator_account_id().to_string()),
        }
    }
}

/// Every operation that refuses an id it cannot resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    DescribeInsight,
    DeleteInsight,
    DescribeAnomaly,
    ListAnomaliesForInsight,
    ListRecommendations,
    ListAnomalousLogGroups,
    ListEvents,
}

impl Refusal {
    const ALL: [Refusal; 7] = [
        Refusal::DescribeInsight,
        Refusal::DeleteInsight,
        Refusal::DescribeAnomaly,
        Refusal::ListAnomaliesForInsight,
        Refusal::ListRecommendations,
        Refusal::ListAnomalousLogGroups,
        Refusal::ListEvents,
    ];

    /// The operation name the message's bracketed prefix carries.
    fn operation(self) -> &'static str {
        match self {
            Refusal::DescribeInsight => "DescribeInsight",
            Refusal::DeleteInsight => "DeleteInsight",
            Refusal::DescribeAnomaly => "DescribeAnomaly",
            Refusal::ListAnomaliesForInsight => "ListAnomaliesForInsight",
            Refusal::ListRecommendations => "ListRecommendations",
            Refusal::ListAnomalousLogGroups => "ListAnomalousLogGroups",
            Refusal::ListEvents => "ListEvents",
        }
    }

    /// The kind of identifier this operation's lookup resolves.
    fn lookup_type(self) -> &'static str {
        match self {
            Refusal::DescribeAnomaly => ANOMALY_ID,
            _ => FINDING_ID,
        }
    }

    /// The `ResourceType` the error's own modelled member reports.
    fn resource_type(self) -> &'static str {
        match self {
            Refusal::DescribeAnomaly => ANOMALY_RESOURCE_TYPE,
            _ => INSIGHT_RESOURCE_TYPE,
        }
    }

    /// Whether the operation models an `AccountId` the request can scope it with.
    /// `DeleteInsight` and `ListAnomalousLogGroups` model none, so they are only
    /// ever the caller's own account.
    fn models_account_id(self) -> bool {
        !matches!(
            self,
            Refusal::DeleteInsight | Refusal::ListAnomalousLogGroups
        )
    }

    /// The id that DOES resolve for this operation, for the positive control.
    fn stored_id(self) -> &'static str {
        match self {
            Refusal::DescribeAnomaly => STORED_ANOMALY,
            _ => STORED_INSIGHT,
        }
    }

    /// Drive the operation over the wire for `id`, scoped as `scope` says.
    async fn send(self, guru: &Guru, id: &str, scope: Scope) -> Reply {
        let account_id = scope.account_id();
        match self {
            Refusal::DescribeInsight | Refusal::DescribeAnomaly => {
                let collection = if self == Refusal::DescribeInsight {
                    "insights"
                } else {
                    "anomalies"
                };
                let path = match &account_id {
                    Some(account_id) => {
                        format!("/{collection}/{id}?AccountId={account_id}")
                    }
                    None => format!("/{collection}/{id}"),
                };
                guru.call("GET", &path, &Value::Null).await
            }
            Refusal::DeleteInsight => {
                guru.call("DELETE", &format!("/insights/{id}"), &Value::Null)
                    .await
            }
            Refusal::ListAnomaliesForInsight => {
                let mut body = json!({});
                if let Some(account_id) = &account_id {
                    body["AccountId"] = json!(account_id);
                }
                guru.call("POST", &format!("/anomalies/insight/{id}"), &body)
                    .await
            }
            Refusal::ListRecommendations => {
                let mut body = json!({ "InsightId": id });
                if let Some(account_id) = &account_id {
                    body["AccountId"] = json!(account_id);
                }
                guru.call("POST", "/recommendations", &body).await
            }
            Refusal::ListAnomalousLogGroups => {
                guru.call("POST", "/list-log-anomalies", &json!({ "InsightId": id }))
                    .await
            }
            Refusal::ListEvents => {
                let mut body = json!({ "Filters": { "InsightId": id } });
                if let Some(account_id) = &account_id {
                    body["AccountId"] = json!(account_id);
                }
                guru.call("POST", "/events", &body).await
            }
        }
    }
}

/// Ids spanning what `InsightId` (len 1..100, `^[\w-]*$`) and `AnomalyId`
/// (len 1..100, `^[\w~.-]*$`) both admit: one character, a word, a uuid-shaped id,
/// an id one character away from a stored one, and the 100-character maximum.
fn missing_ids() -> Vec<String> {
    vec![
        "x".to_string(),
        "0".to_string(),
        "no_such_resource".to_string(),
        "4f8bb0d2-1a3f-4c21-9fb1-6a9a51f3dd77".to_string(),
        format!("{STORED_INSIGHT}0"),
        format!("{STORED_ANOMALY}0"),
        "a".repeat(100),
    ]
}

/// The sentence AWS answers a not-found lookup with, written out here independently
/// of the code that renders it.
fn expected_message(operation: &str, account_id: &str, lookup_type: &str, id: &str) -> String {
    format!(
        "[{operation}] failed - Resource Not found for awsAccountId [{account_id}] \
         for type [{lookup_type}] with id [{id}]"
    )
}

/// RULE: every DevOps Guru not-found refusal reports the fixed sentence with the
/// refusing operation's name, the account the request resolved to, the kind of
/// identifier the lookup resolves and the id asked for — for every operation that
/// refuses an id, every id shape, and either way of naming the account.
#[tokio::test]
async fn every_not_found_refusal_reports_the_account_the_lookup_type_and_the_id() {
    let own_account = ::engine::emulator_account_id().to_string();

    for refusal in Refusal::ALL {
        for scope in Scope::ALL {
            if scope != Scope::Caller && !refusal.models_account_id() {
                continue;
            }
            for id in missing_ids() {
                let guru = Guru::new();
                let case = format!("{:?}/{scope:?}/{id}", refusal);
                let reply = refusal.send(&guru, &id, scope).await;

                assert_eq!(
                    reply.status, 404,
                    "case {case:?}: expected HTTP 404, got {} ({})",
                    reply.status, reply.raw
                );
                assert_eq!(
                    reply.error_name, "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {}",
                    reply.raw
                );

                let expected = expected_message(
                    refusal.operation(),
                    &own_account,
                    refusal.lookup_type(),
                    &id,
                );
                assert_eq!(
                    reply.str_at("/Message"),
                    Some(expected.as_str()),
                    "case {case:?}: expected Message {expected:?}, got {:?}",
                    reply.str_at("/Message")
                );
                assert_eq!(
                    reply.str_at("/ResourceId"),
                    Some(id.as_str()),
                    "case {case:?}: expected ResourceId {id:?}, got {:?}",
                    reply.str_at("/ResourceId")
                );
                assert_eq!(
                    reply.str_at("/ResourceType"),
                    Some(refusal.resource_type()),
                    "case {case:?}: expected ResourceType {:?}, got {:?}",
                    refusal.resource_type(),
                    reply.str_at("/ResourceType")
                );
            }
        }
    }
}

/// RULE: the refusal is about the id the request could not resolve, not about the
/// surface — the same operation serves the id the store DOES hold.
#[tokio::test]
async fn the_same_operation_serves_an_id_the_store_holds() {
    for refusal in Refusal::ALL {
        let guru = Guru::new();
        let case = format!("{:?}", refusal);
        let reply = refusal
            .send(&guru, refusal.stored_id(), Scope::Caller)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an id the store holds must be served, got {} ({})",
            reply.status, reply.raw
        );
    }
}

/// RULE: a refused lookup changes nothing — the message is composed from the request
/// and the account, so repeating it answers the same sentence and leaves the store
/// as it was.
#[tokio::test]
async fn a_refused_lookup_answers_the_same_sentence_and_writes_nothing() {
    let guru = Guru::new();
    let before = format!("{:?}", *guru.store().lock());

    for refusal in Refusal::ALL {
        for id in missing_ids() {
            let case = format!("{:?}/{id}", refusal);
            let first = refusal.send(&guru, &id, Scope::Caller).await;
            let second = refusal.send(&guru, &id, Scope::Caller).await;
            assert_eq!(
                first.str_at("/Message"),
                second.str_at("/Message"),
                "case {case:?}: a repeated refusal must answer the same Message, got {:?} then {:?}",
                first.str_at("/Message"),
                second.str_at("/Message")
            );
            assert_eq!(
                format!("{:?}", *guru.store().lock()),
                before,
                "case {case:?}: a refused lookup must not change the store"
            );
        }
    }
}
