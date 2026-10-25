// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every workload suite in this crate shares.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::{
    AnswerRecord, ChoiceAnswerRecord,
};

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
                service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::normalize_unique_name(
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
                    profiles: Vec::new(),
                },
            );
        }
    }

    /// Record an answer to one question of one lens review straight through the shared
    /// state, so a test has a stored answer for the reads to report. The answers
    /// `UpdateAnswer` records over the wire are exercised by `super::answer_updates`;
    /// this seeds one for the suites that are about a DIFFERENT read.
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

    /// Record an answer to one question of one REVIEW TEMPLATE's lens review straight
    /// through the shared state, so a test has a stored answer for the reads to report.
    /// `UpdateReviewTemplateAnswer` — the operation that answers a template question
    /// over the wire — is exercised over HTTP by `super::review_template_answer_updates`;
    /// this seed is for the reads that need many answered questions at once.
    pub(crate) fn record_template_answer(
        &self,
        template_arn: &str,
        lens: &str,
        question_id: &str,
        answer: service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord,
    ) {
        let state = self
            .engine
            .state::<service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>();
        let mut store = state.lock();
        let template = store
            .review_templates
            .get_mut(template_arn)
            .expect("the review template under test must exist");
        template
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

    /// `ListReviewTemplates`: `GET /reviewTemplates`, with both paging members bound to
    /// the query string. `query` is passed as sent, empty for a request that names none.
    pub(crate) async fn list_review_templates(&self, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/reviewTemplates".to_string()
        } else {
            format!("/reviewTemplates?{query}")
        };
        self.send("GET", &uri, None).await
    }

    /// `DeleteReviewTemplate`: `DELETE /reviewTemplates/{TemplateArn}` with the
    /// `@idempotencyToken` bound to `@httpQuery("ClientRequestToken")`. The ARN is a
    /// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment,
    /// exactly as an SDK sends it.
    pub(crate) async fn delete_review_template(&self, template_arn: &str, token: &str) -> Answer {
        let encoded = encode_path_segment(template_arn);
        self.send(
            "DELETE",
            &format!("/reviewTemplates/{encoded}?ClientRequestToken={token}"),
            None,
        )
        .await
    }

    /// `UpdateReviewTemplateLensReview`:
    /// `PATCH /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}`. Both labels are
    /// non-greedy, so each travels percent-encoded in one path segment, exactly as an
    /// SDK sends it.
    pub(crate) async fn update_review_template_lens_review(
        &self,
        template_arn: &str,
        lens: &str,
        body: Value,
    ) -> Answer {
        self.send(
            "PATCH",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            Some(body),
        )
        .await
    }

    /// `GetReviewTemplateLensReview`:
    /// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}`.
    pub(crate) async fn get_review_template_lens_review(
        &self,
        template_arn: &str,
        lens: &str,
    ) -> Answer {
        self.send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            None,
        )
        .await
    }

    /// `UpdateReviewTemplateAnswer`:
    /// `PATCH /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers/{QuestionId}`.
    /// Every label is non-greedy, so each travels percent-encoded in one path segment,
    /// exactly as an SDK sends it.
    pub(crate) async fn update_review_template_answer(
        &self,
        template_arn: &str,
        lens: &str,
        question_id: &str,
        body: Value,
    ) -> Answer {
        self.send(
            "PATCH",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(body),
        )
        .await
    }

    /// `GetReviewTemplateAnswer`:
    /// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers/{QuestionId}`.
    pub(crate) async fn get_review_template_answer(
        &self,
        template_arn: &str,
        lens: &str,
        question_id: &str,
    ) -> Answer {
        self.send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers/{}",
                encode_path_segment(template_arn),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
    }

    /// `ListReviewTemplateAnswers`:
    /// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers`, with every
    /// filter and paging member bound to the query string. `query` is passed as sent,
    /// empty for a request that names none.
    pub(crate) async fn list_review_template_answers(
        &self,
        template_arn: &str,
        lens: &str,
        query: &str,
    ) -> Answer {
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        };
        self.send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers{suffix}",
                encode_path_segment(template_arn),
                encode_path_segment(lens)
            ),
            None,
        )
        .await
    }

    /// `CreateProfile`: `POST /profiles`.
    pub(crate) async fn create_profile(&self, body: Value) -> Answer {
        self.send("POST", "/profiles", Some(body)).await
    }

    /// `GetProfile`: `GET /profiles/{ProfileArn}`, with the optional `ProfileVersion`
    /// bound to the query string. The ARN is a non-greedy `@httpLabel`, so it travels
    /// percent-encoded in one path segment, exactly as an SDK sends it.
    pub(crate) async fn get_profile(
        &self,
        profile_arn: &str,
        profile_version: Option<&str>,
    ) -> Answer {
        let encoded = encode_path_segment(profile_arn);
        let uri = match profile_version {
            Some(version) => format!(
                "/profiles/{encoded}?ProfileVersion={}",
                encode_path_segment(version)
            ),
            None => format!("/profiles/{encoded}"),
        };
        self.send("GET", &uri, None).await
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

    /// `UpdateWorkloadShare`: `PATCH /workloads/{WorkloadId}/shares/{ShareId}`.
    pub(crate) async fn update_share(
        &self,
        workload_id: &str,
        share_id: &str,
        body: Value,
    ) -> Answer {
        self.send(
            "PATCH",
            &format!("/workloads/{workload_id}/shares/{share_id}"),
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

    /// `UpdateShareInvitation`: `PATCH /shareInvitations/{ShareInvitationId}`. The id is a
    /// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment, exactly
    /// as an SDK sends it.
    pub(crate) async fn update_share_invitation(
        &self,
        share_invitation_id: &str,
        body: Value,
    ) -> Answer {
        let encoded = encode_path_segment(share_invitation_id);
        self.send("PATCH", &format!("/shareInvitations/{encoded}"), Some(body))
            .await
    }

    /// Move the stored profile `profile_arn` to `profile_version`, as a revision of that
    /// profile would.
    ///
    /// Minting a profile's next version belongs to `UpdateProfile`, still a stub with its
    /// own issue, so a suite that needs a profile whose latest version is AHEAD of the
    /// version a workload's association pinned — associating pins the version the profile
    /// is at — seeds that revision here, exactly as the suites above seed a stored answer
    /// whose writer is a stub. The operation under test and the reads of its result still
    /// cross the HTTP boundary.
    pub(crate) fn revise_profile_version(&self, profile_arn: &str, profile_version: &str) {
        let state = self.store();
        let mut store = state.lock();
        let profile = store
            .profiles
            .get_mut(profile_arn)
            .expect("the profile under test must exist");
        profile.profile_version = profile_version.to_string();
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

/// Create a review template applying `lenses` and answer its `TemplateArn` — the one
/// review-template seeding helper the suites in this crate share.
pub(crate) async fn review_template(client: &Client, name: &str, lenses: &[&str]) -> String {
    let answer = client
        .create_review_template(json!({
            "TemplateName": name,
            "Description": "a review template under test",
            "Lenses": lenses,
            "ClientRequestToken": format!("tok-{name}"),
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateReviewTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An `AnswerRecord` that records `selected` as the best practices in place, `choices` as
/// the status of each choice it speaks about, whether the question applies and the reason
/// it does not — the one answer builder the suites in this crate share. The answer's own
/// notes and each choice's notes are what `UpdateAnswer` / `UpdateReviewTemplateAnswer`
/// write over the wire, so a seeded answer carries none.
pub(crate) fn answered(
    selected: &[&str],
    choices: &[(&str, &str)],
    is_applicable: bool,
    reason: Option<&str>,
) -> AnswerRecord {
    AnswerRecord {
        selected_choices: selected.iter().map(ToString::to_string).collect(),
        choice_answers: choices
            .iter()
            .map(|(choice_id, status)| {
                (
                    (*choice_id).to_string(),
                    ChoiceAnswerRecord {
                        status: (*status).to_string(),
                        reason: None,
                        notes: None,
                    },
                )
            })
            .collect(),
        is_applicable,
        reason: reason.map(ToString::to_string),
        notes: None,
    }
}

/// The ARN an AWS-OFFICIAL lens carries: "the lens ARN, such as
/// `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias` doc) — the account
/// segment is EMPTY, because AWS publishes the lens rather than an account owning it.
/// Spelled out here (rather than read back out of the code under test) with the account
/// placeholder taken from the emulator's one source of truth, so a caller would fail if
/// an owner account appeared in it.
pub(crate) fn official_lens_arn(region: &str, alias: &str) -> String {
    let arn = format!("arn:aws:wellarchitected:{region}::lens/{alias}");
    assert!(
        !arn.contains(engine::emulator_account_id()),
        "an AWS-official lens ARN names no owner account, got {arn:?}"
    );
    arn
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

/// One member of an answer's body as a string, empty when the body carries none. The one
/// member reader the suites in this crate share.
pub(crate) fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A minimal valid `CreateProfile` request: `ProfileName`, `ProfileDescription`,
/// `ProfileQuestions` and `ClientRequestToken` are all `@required`
/// (`smithy_query(query="operation", operation="CreateProfile")`). The one profile request
/// builder this crate's suites share.
pub(crate) fn minimal_profile(name: &str) -> Value {
    json!({
        "ProfileName": name,
        "ProfileDescription": "a profile under test",
        "ProfileQuestions": [{
            "QuestionId": "priority",
            "SelectedChoiceIds": ["priority_cost"],
        }],
        "ClientRequestToken": format!("tok-{name}"),
    })
}

/// A profile minted over HTTP: its `ProfileArn` and the `ProfileVersion` the service put
/// it at, both checked against the modelled forms
/// (`^arn:aws[-a-z]*:wellarchitected:[a-z]{2}(-gov)?-[a-z]+-\d:\d{12}:profile/[a-z0-9]+$`
/// and `@length(1, 32)` `^[A-Za-z0-9-]+$`). The one profile-minting helper this crate's
/// suites share.
pub(crate) async fn created_profile(client: &Client, name: &str) -> (String, String) {
    let answer = client.create_profile(minimal_profile(name)).await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateProfile 200, got {answer:?}"
    );
    let arn = member(&answer, "ProfileArn").to_string();
    let version = member(&answer, "ProfileVersion").to_string();
    assert!(
        arn.starts_with(&format!(
            "arn:aws:wellarchitected:us-west-2:{}:profile/",
            engine::emulator_account_id()
        )),
        "case {name:?}: unexpected ProfileArn {arn:?}"
    );
    assert!(
        (1..=32).contains(&version.len())
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {name:?}: ProfileVersion must match ^[A-Za-z0-9-]{{1,32}}$, got {version:?}"
    );
    (arn, version)
}

/// The `Workload` a `GetWorkload` reports, as a JSON object.
pub(crate) async fn stored_workload(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    answer.body.get("Workload").cloned().unwrap_or(Value::Null)
}

/// The `Workload$Profiles` a workload reports, empty for a workload that reports none.
pub(crate) async fn profiles_of(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    stored_workload(client, workload_id, case)
        .await
        .get("Profiles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `LensReview$Profiles` the workload's review of one lens reports: "the profiles
/// associated with the workload" (`smithy_query(query="doc", shape="LensReview",
/// member="Profiles")`), so a review has to agree with `GetWorkload` about them.
pub(crate) async fn lens_review_profiles(
    client: &Client,
    workload_id: &str,
    lens: &str,
    case: &str,
) -> Vec<Value> {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .and_then(|review| review.get("Profiles"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Workload$UpdatedAt` a workload reports, as the epoch seconds restJson1 carries it.
pub(crate) async fn updated_at_of(client: &Client, workload_id: &str, case: &str) -> f64 {
    stored_workload(client, workload_id, case)
        .await
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the workload must report UpdatedAt"))
}

/// Move the stored workload's `UpdatedAt` back by `age`, so a write the handler makes to
/// it is observable whatever the clock's granularity between two in-process requests.
/// Only the SEED reaches state directly; the write under test is the handler's.
pub(crate) fn age_workload(client: &Client, workload_id: &str, age: Duration) {
    let state = client.store();
    let mut store = state.lock();
    let workload = store
        .workloads
        .get_mut(workload_id)
        .expect("the workload under test must exist");
    workload.updated_at = SystemTime::now() - age;
}

/// The refusal a lookup that resolved nothing gets: 404 `ResourceNotFoundException`, with
/// the modelled `ResourceType` of the resource that was missing and the identifier AS SENT.
pub(crate) fn assert_not_found(
    case: &str,
    answer: &Answer,
    resource_type: &str,
    resource_id: &str,
    subject: &str,
) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        resource_type,
        "case {case:?}: expected ResourceType {resource_type}, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must be the identifier as sent, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[ResourceNotFound] No {subject} was found in account {}.",
            engine::emulator_account_id()
        ),
        "case {case:?}: unexpected Message, got {answer:?}"
    );
}

/// A minimal valid `CreateReviewTemplate` request; callers override the members their
/// case varies. The one review-template request builder this crate's suites share.
pub(crate) fn minimal_template(name: &str, token: &str) -> Value {
    json!({
        "TemplateName": name,
        "Description": "a review template under test",
        "Lenses": ["wellarchitected"],
        "ClientRequestToken": token,
    })
}

/// The `TemplateArn` a successful `CreateReviewTemplate` answered with, checked against
/// the modelled `TemplateArn` pattern and its `@length(50, 250)`.
pub(crate) fn created_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateReviewTemplate 200, got {answer:?}"
    );
    let arn = answer
        .body
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let expected_prefix = format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_default();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: TemplateArn must be {expected_prefix}<32 hex>, got {arn:?}"
    );
    assert!(
        (50..=250).contains(&arn.len()),
        "case {case:?}: TemplateArn must satisfy @length(50, 250), got {} characters in {arn:?}",
        arn.len()
    );
    arn
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
