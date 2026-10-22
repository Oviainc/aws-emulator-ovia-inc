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

    /// This emulator's service state, for a test that has to reach a ceiling the HTTP
    /// boundary would take tens of thousands of requests to reach. The boundary itself is
    /// still crossed over HTTP for everything the test asserts about behaviour.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState,
    > {
        self.engine
            .state::<service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>()
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
                    answers: std::collections::BTreeMap::new(),
                },
            );
        }
    }

    /// Record an answer to one question of one lens review straight through the shared
    /// state, so a test has a stored answer for the reads to report. `UpdateAnswer` —
    /// the operation that answers a question over the wire — is still a stub with its
    /// own issue; the READ under test still crosses the HTTP boundary.
    pub(crate) fn record_answer(
        &self,
        workload_id: &str,
        lens: &str,
        question_id: &str,
        answer: service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord,
    ) {
        let state = self
            .engine
            .state::<service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>();
        let mut store = state.lock();
        let workload = store
            .workloads
            .get_mut(workload_id)
            .expect("the workload under test must exist");
        workload
            .answers
            .entry(
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::lens_alias(
                    lens,
                )
                .to_string(),
            )
            .or_default()
            .insert(question_id.to_string(), answer);
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

    pub(crate) async fn create_review_template(&self, body: Value) -> Answer {
        self.send("POST", "/reviewTemplates", Some(body)).await
    }

    /// `GET /reviewTemplates/{TemplateArn}`: the ARN is a non-greedy `@httpLabel`, so it
    /// travels percent-encoded in one path segment, exactly as an SDK sends it.
    pub(crate) async fn get_review_template(&self, template_arn: &str) -> Answer {
        let encoded = encode_path_segment(template_arn);
        self.send("GET", &format!("/reviewTemplates/{encoded}"), None)
            .await
    }

    /// `CreateWorkloadShare`: `POST /workloads/{WorkloadId}/shares`.
    pub(crate) async fn create_share(&self, workload_id: &str, body: Value) -> Answer {
        self.send(
            "POST",
            &format!("/workloads/{workload_id}/shares"),
            Some(body),
        )
        .await
    }

    /// `ListWorkloadShares`: `GET /workloads/{WorkloadId}/shares`, with every filter and
    /// paging member bound to the query string. `query` is passed as sent, empty for a
    /// request that names none.
    pub(crate) async fn list_shares(&self, workload_id: &str, query: &str) -> Answer {
        let uri = if query.is_empty() {
            format!("/workloads/{workload_id}/shares")
        } else {
            format!("/workloads/{workload_id}/shares?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// `ListShareInvitations`: `GET /shareInvitations`, with every filter and paging
    /// member bound to the query string. `query` is passed as sent, empty for a request
    /// that names none.
    pub(crate) async fn list_share_invitations(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/shareInvitations".to_string()
        } else {
            format!("/shareInvitations?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// Record a share of `workload_id` with `shared_with` at `status` straight through
    /// the shared state, and answer its `(ShareId, ShareInvitationId)`.
    ///
    /// A share invitation is offered to the principal a resource was shared WITH, so a
    /// test of the invitation reads needs a share whose recipient is the CALLER — and
    /// that share has no HTTP path here: `CreateWorkloadShare` refuses a `SharedWith`
    /// naming the principal doing the sharing, and this emulator attributes every
    /// request to its one account, so the other account that would offer the share
    /// cannot sign a request. The seed records exactly what `CreateWorkloadShare`
    /// records — `shared_by` is the stored workload's owning account — and the READ
    /// under test still crosses the HTTP boundary.
    pub(crate) fn record_workload_share(
        &self,
        workload_id: &str,
        shared_with: &str,
        permission_type: &str,
        status: &str,
    ) -> (String, String) {
        let state = self
            .engine
            .state::<service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>();
        let mut store = state.lock();
        let shared_by = store
            .workloads
            .get(workload_id)
            .expect("the workload under test must exist")
            .owner
            .clone();
        let share_id = store.mint_share_id();
        let share_invitation_id = store.mint_share_invitation_id();
        store
            .workload_shares
            .entry(workload_id.to_string())
            .or_default()
            .insert(
                share_id.clone(),
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::WorkloadShareRecord {
                    share_id: share_id.clone(),
                    share_invitation_id: share_invitation_id.clone(),
                    workload_id: workload_id.to_string(),
                    shared_by,
                    shared_with: shared_with.to_string(),
                    permission_type: permission_type.to_string(),
                    status: status.to_string(),
                    status_message: None,
                },
            );
        (share_id, share_invitation_id)
    }
}

/// Percent-encode a value so it survives as ONE path segment: everything RFC 3986 does
/// not leave unreserved is escaped, so an ARN's `:` and `/` reach the server's label
/// decoder intact. The one encoder in this crate's tests; every value they send is ASCII.
pub(crate) fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
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
