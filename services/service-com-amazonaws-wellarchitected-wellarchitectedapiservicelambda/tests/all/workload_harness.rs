// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every workload suite in this crate shares.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
pub(crate) struct Client {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
    pub(crate) error_code: String,
}

impl Client {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Fill the store straight through the state the handlers share, so a test can reach
    /// the workload quota without issuing ten thousand HTTP requests. The boundary itself
    /// is still crossed over HTTP.
    pub(crate) fn seed_workloads(&self, count: usize) {
        let state = self
            .engine
            .state::<service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>();
        let mut store = state.lock();
        for index in 0..count {
            let workload_id = store.mint_workload_id();
            let workload_name = format!("seeded-{index:06}");
            store.workload_name_index.insert(
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::normalize_workload_name(
                    &workload_name,
                ),
                workload_id.clone(),
            );
            store.workloads.insert(
                workload_id.clone(),
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::WorkloadRecord {
                    workload_arn: format!(
                        "arn:aws:wellarchitected:us-west-2:000000000000:workload/{workload_id}"
                    ),
                    workload_id,
                    workload_name,
                    description: "a seeded workload".to_string(),
                    environment: "PRODUCTION".to_string(),
                    updated_at: std::time::SystemTime::now(),
                    account_ids: Vec::new(),
                    aws_regions: vec!["us-east-1".to_string()],
                    non_aws_regions: Vec::new(),
                    pillar_priorities: Vec::new(),
                    architectural_design: None,
                    review_owner: Some("seed-owner".to_string()),
                    is_review_owner_update_acknowledged: false,
                    industry_type: None,
                    industry: None,
                    notes: None,
                    improvement_status: "NOT_APPLICABLE".to_string(),
                    lenses: Vec::new(),
                    owner: "000000000000".to_string(),
                    tags: std::collections::BTreeMap::new(),
                    applications: Vec::new(),
                    discovery_config: None,
                    jira_configuration: None,
                },
            );
        }
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-west-2/wellarchitected/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef",
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if error_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            error_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    pub(crate) async fn create(&self, body: Value) -> Answer {
        self.send("POST", "/workloads", Some(body)).await
    }

    pub(crate) async fn get(&self, workload_id: &str) -> Answer {
        self.send("GET", &format!("/workloads/{workload_id}"), None)
            .await
    }

    pub(crate) async fn update(&self, workload_id: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/workloads/{workload_id}"), Some(body))
            .await
    }

    pub(crate) async fn delete(&self, workload_id: &str, token: &str) -> Answer {
        self.send(
            "DELETE",
            &format!("/workloads/{workload_id}?ClientRequestToken={token}"),
            None,
        )
        .await
    }

    pub(crate) async fn list(&self, body: Value) -> Answer {
        self.send("POST", "/workloadsSummaries", Some(body)).await
    }
}

/// A minimal valid `CreateWorkload` request; callers override the members their case
/// varies. `AwsRegions` and `ReviewOwner` are present because the CreateWorkload doc
/// requires both even though the model does not.
pub(crate) fn minimal_create(name: &str, token: &str) -> Value {
    json!({
        "WorkloadName": name,
        "Description": "a workload under test",
        "Environment": "PRODUCTION",
        "Lenses": ["wellarchitected"],
        "AwsRegions": ["us-east-1"],
        "ReviewOwner": "owner@example.com",
        "ClientRequestToken": token,
    })
}

/// The `WorkloadId` a successful `CreateWorkload` answered with, checked against the
/// modelled `^[0-9a-f]{32}$` pattern.
pub(crate) fn created_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkload 200, got {answer:?}"
    );
    let id = answer
        .body
        .get("WorkloadId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: WorkloadId must match ^[0-9a-f]{{32}}$, got {id:?}"
    );
    id
}
