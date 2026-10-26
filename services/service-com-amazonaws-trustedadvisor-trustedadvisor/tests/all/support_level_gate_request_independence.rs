// SPDX-License-Identifier: CC0-1.0

//! What the support-level refusal is allowed to depend on.
//!
//! The rule this suite pins is about the WHOLE class of well-formed requests, not about
//! any one call: **the refusal an account whose AWS Support plan does not include the
//! Trusted Advisor API gets depends on nothing in the request but its well-formedness —
//! whatever filters, page size, page token, language, identifier or payload it carries,
//! the answer is the same modelled 403 `AccessDeniedException`, carrying the modelled
//! `message` and nothing else about the store — while the SAME request from an account
//! whose plan does include the API is answered by the operation itself.**
//!
//! That is the half the sibling suites do not state. `tests/all/support_level_gate.rs`
//! and `tests/all/seeded_support_plan.rs` vary the PLAN and the OPERATION over one
//! well-formed request each; the rows here vary what the request CARRIES, so a gate that
//! fired only for the simplest form of a call — or that let a filtered, paged or
//! differently-identified call through to the handler — would fail here and nowhere else.
//! Two consequences of the rule are rows rather than separate tests: a page token the
//! service never issued is a 403 for an unentitled account and the handler's own 400 for
//! an entitled one, and an identifier that names nothing is a 403 and the handler's own
//! 404 — so the gate is shown to precede both resolutions. The identifier rows deliberately
//! overlap `tests/all/support_level_gate.rs::the_gate_precedes_identifier_resolution`: they
//! are here as members of this one table, so that what each row varies is the request's
//! shape and nothing else.
//!
//! Every status, error shape, filter name, enum value and bound below comes from the
//! model, not from a recording:
//! `smithy_query(query="operation",operation="ListChecks,ListRecommendations,GetRecommendation,ListRecommendationResources,UpdateRecommendationLifecycle,BatchUpdateRecommendationResourceExclusion",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — `AccessDeniedException` is bound to every one of them with errorKind client and
//! httpError 403; `maxResults` is `@range(1, 200)` on the two catalogue lists and
//! `@range(1, 5000)` on the resource list; `nextToken` is `@length(4, 10000)`; the
//! exclusion batch is `@length(1, 25)` and `@uniqueItems`; and the enum values come from
//! `smithy_query(query="shape",shape="RecommendationPillar,RecommendationSource,RecommendationLanguage,RecommendationType,RecommendationStatus,ResourceStatus,ExclusionStatus,UpdateRecommendationLifecycleStage,UpdateRecommendationLifecycleStageReasonCode")`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;

use crate::recommendation_loop::{
    check_arn, dispatch, emulator_on_plan, error_code, label, recommendation_arn, resource_arn,
};

/// The refusal the support-level gate answers with: the status the model binds
/// `AccessDeniedException` to, the shape name restJson1 carries both in `x-amzn-errortype`
/// and in the body's `__type`, and the sentence the service states them with.
const DENIED: (u16, &str, &str) = (
    403,
    "AccessDeniedException",
    "Access denied due to support level",
);

/// The members a refusal body may carry: the modelled `message` of
/// `AccessDeniedException` and the discriminator restJson1 writes beside it. Anything else
/// would be the gate reporting something about a store the caller may not read.
///
/// Compared as a SET, so the assertion is about which members are there and not about the
/// order a serializer happens to write them in.
const REFUSAL_MEMBERS: [&str; 2] = ["__type", "message"];

/// A `nextToken` the service never issued, at the modelled minimum length (4).
const UNISSUED_SHORT_TOKEN: &str = "aaaa";

/// Another one, longer and not base64 at all, so the two token rows differ in form as
/// well as in length.
const UNISSUED_TOKEN: &str = "not-a-token-this-service-issued";

/// An `updateReason` the modelled `@length(10, 4096)` admits.
const UPDATE_REASON: &str = "handled by the standing runbook";

/// One request this suite drives twice: once as an account the gate refuses, once as one
/// it does not.
struct Row {
    /// What makes this row different from its siblings, named in every assertion.
    case: &'static str,
    method: &'static str,
    /// The path and query string, already percent-escaped where a value needs it.
    path: String,
    body: Option<Value>,
    /// The status an ENTITLED account gets for this same request — the operation's own
    /// answer, which is what the row being well-formed means.
    own_status: u16,
    /// The error shape that answer carries, or `None` where the operation serves it.
    own_errortype: Option<&'static str>,
}

/// One exclusion entry, naming a recommendation resource of the seeded `standard`
/// recommendation (the `RecommendationResourceArn` pattern, built by the shared helper).
fn exclusion(resource_id: &str) -> Value {
    json!({
        "arn": resource_arn("amazon-ebs-snapshots", resource_id),
        "isExcluded": true,
    })
}

/// The rows: every well-formed form of a call the six account-level operations accept,
/// differing along the filters, the page size at each published bound, the page token,
/// the language, the identifier (seeded and naming nothing) and the payload — including
/// the batch at its modelled maximum size.
fn rows() -> Vec<Row> {
    // `low-utilization-ec2-instances` produced the seeded `priority` recommendation, the
    // type whose lifecycle `UpdateRecommendationLifecycle` can move;
    // `amazon-ebs-snapshots` produced a `standard` one, whose resources the exclusion
    // batch accepts.
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let standard = recommendation_arn("amazon-ebs-snapshots");
    let unknown = format!(
        "arn:aws:trustedadvisor::{}:recommendation/not-a-seeded-recommendation",
        engine::EMULATOR_ACCOUNT_ID
    );
    let batch_at_the_modelled_maximum: Vec<Value> = (1..=25)
        .map(|index| exclusion(&format!("res-ebs-{index:04}")))
        .collect();

    vec![
        Row {
            case: "ListChecks with no filter at all",
            method: "GET",
            path: "/v1/checks".to_string(),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListChecks at the modelled minimum page size",
            method: "GET",
            path: "/v1/checks?maxResults=1".to_string(),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListChecks at the modelled maximum page size",
            method: "GET",
            path: "/v1/checks?maxResults=200".to_string(),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListChecks with every filter it models and a non-default language",
            method: "GET",
            path: "/v1/checks?pillar=cost_optimizing&awsService=ec2&source=ta_check&language=ja"
                .to_string(),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            // The gate precedes the handler's own token resolution: an entitled caller
            // gets the modelled 400 for this token, an unentitled one the 403.
            case: "ListChecks with a page token this service never issued",
            method: "GET",
            path: format!("/v1/checks?nextToken={UNISSUED_SHORT_TOKEN}"),
            body: None,
            own_status: 400,
            own_errortype: Some("ValidationException"),
        },
        Row {
            case: "ListRecommendations at the modelled maximum page size",
            method: "GET",
            path: "/v1/recommendations?maxResults=200".to_string(),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListRecommendations filtered by type, status, pillar and language",
            method: "GET",
            path: "/v1/recommendations?type=priority&status=warning&pillar=cost_optimizing\
                   &language=pt_BR"
                .to_string(),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListRecommendations filtered by a seeded check identifier",
            method: "GET",
            path: format!(
                "/v1/recommendations?checkIdentifier={}",
                label(&check_arn("low-utilization-ec2-instances"))
            ),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListRecommendations with a page token this service never issued",
            method: "GET",
            path: format!("/v1/recommendations?nextToken={UNISSUED_TOKEN}"),
            body: None,
            own_status: 400,
            own_errortype: Some("ValidationException"),
        },
        Row {
            case: "GetRecommendation on a seeded recommendation",
            method: "GET",
            path: format!("/v1/recommendations/{}", label(&priority)),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "GetRecommendation on a seeded recommendation in another language",
            method: "GET",
            path: format!("/v1/recommendations/{}?language=zh_TW", label(&priority)),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            // The gate precedes identifier resolution: an entitled caller learns this
            // recommendation does not exist, an unentitled one learns nothing about it.
            case: "GetRecommendation on an identifier that names nothing",
            method: "GET",
            path: format!("/v1/recommendations/{}", label(&unknown)),
            body: None,
            own_status: 404,
            own_errortype: Some("ResourceNotFoundException"),
        },
        Row {
            case: "ListRecommendationResources at the modelled maximum page size",
            method: "GET",
            path: format!(
                "/v1/recommendations/{}/resources?maxResults=5000",
                label(&standard)
            ),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListRecommendationResources filtered by status, exclusion status and region",
            method: "GET",
            path: format!(
                "/v1/recommendations/{}/resources?status=warning&exclusionStatus=included\
                 &regionCode=us-east-1",
                label(&standard)
            ),
            body: None,
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "ListRecommendationResources on an identifier that names nothing",
            method: "GET",
            path: format!("/v1/recommendations/{}/resources", label(&unknown)),
            body: None,
            own_status: 404,
            own_errortype: Some("ResourceNotFoundException"),
        },
        Row {
            case: "UpdateRecommendationLifecycle moving a seeded recommendation to in_progress",
            method: "PUT",
            path: format!("/v1/recommendations/{}/lifecycle", label(&priority)),
            body: Some(json!({
                "lifecycleStage": "in_progress",
                "updateReason": UPDATE_REASON,
                "updateReasonCode": "low_priority",
            })),
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "UpdateRecommendationLifecycle resolving a seeded recommendation",
            method: "PUT",
            path: format!("/v1/recommendations/{}/lifecycle", label(&priority)),
            body: Some(json!({ "lifecycleStage": "resolved" })),
            own_status: 200,
            own_errortype: None,
        },
        Row {
            // The gate precedes the type check too: an entitled caller gets the modelled
            // conflict for a recommendation this operation cannot move.
            case: "UpdateRecommendationLifecycle on a recommendation of the wrong type",
            method: "PUT",
            path: format!("/v1/recommendations/{}/lifecycle", label(&standard)),
            body: Some(json!({ "lifecycleStage": "dismissed" })),
            own_status: 409,
            own_errortype: Some("ConflictException"),
        },
        Row {
            case: "UpdateRecommendationLifecycle on an identifier that names nothing",
            method: "PUT",
            path: format!("/v1/recommendations/{}/lifecycle", label(&unknown)),
            body: Some(json!({ "lifecycleStage": "dismissed" })),
            own_status: 404,
            own_errortype: Some("ResourceNotFoundException"),
        },
        Row {
            case: "BatchUpdateRecommendationResourceExclusion at the modelled minimum batch size",
            method: "PUT",
            path: "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            body: Some(json!({
                "recommendationResourceExclusions": [exclusion("res-ebs-0001")],
            })),
            own_status: 200,
            own_errortype: None,
        },
        Row {
            case: "BatchUpdateRecommendationResourceExclusion at the modelled maximum batch size",
            method: "PUT",
            path: "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            body: Some(json!({
                "recommendationResourceExclusions": batch_at_the_modelled_maximum,
            })),
            own_status: 200,
            own_errortype: None,
        },
    ]
}

/// Every plan whose entitlement does not cover the Trusted Advisor API, read from the
/// service's own classification rather than written out again here, so a plan added later
/// is covered by this suite without editing it.
fn plans_the_gate_refuses() -> Vec<SupportPlan> {
    let plans: Vec<SupportPlan> = SupportPlan::ALL
        .iter()
        .copied()
        .filter(|plan| !plan.includes_trusted_advisor_api())
        .collect();
    assert!(
        !plans.is_empty(),
        "the service must classify at least one plan as not covering the API, or the rows \
         below would assert nothing"
    );
    plans
}

/// And every plan whose entitlement does cover it.
fn plans_the_gate_admits() -> Vec<SupportPlan> {
    let plans: Vec<SupportPlan> = SupportPlan::ALL
        .iter()
        .copied()
        .filter(|plan| plan.includes_trusted_advisor_api())
        .collect();
    assert!(
        !plans.is_empty(),
        "the service must classify at least one plan as covering the API, or the rows below \
         would assert nothing"
    );
    plans
}

/// Whatever a well-formed request carries, an account the gate refuses gets the SAME
/// refusal: the modelled 403 `AccessDeniedException`, the gate's sentence, and no other
/// member — and byte for byte the answer the first row got, so no operation can drift
/// into refusing in words of its own.
#[tokio::test]
async fn the_refusal_does_not_depend_on_what_the_request_carries() {
    let (denied_status, denied_code, denied_message) = DENIED;
    for plan in plans_the_gate_refuses() {
        // The answer the first row got, which every later row must match.
        let mut first: Option<(String, Value)> = None;
        for row in rows() {
            // A fresh emulator per row: a row that would mutate must not change what the
            // next row asks about.
            let (router, _engine) = emulator_on_plan(plan);
            let case = format!("{} on {plan:?}", row.case);
            let (status, errortype, response) =
                dispatch(&router, row.method, &row.path, row.body).await;
            assert_eq!(
                status, denied_status,
                "case {case:?}: expected HTTP {denied_status}, got {status} with {response}"
            );
            assert_eq!(
                errortype.as_deref(),
                Some(denied_code),
                "case {case:?}: expected x-amzn-errortype {denied_code}, got {errortype:?} with \
                 {response}"
            );
            assert_eq!(
                error_code(&response),
                denied_code,
                "case {case:?}: expected __type {denied_code}, got {response}"
            );
            assert_eq!(
                response["message"].as_str().unwrap_or_default(),
                denied_message,
                "case {case:?}: expected the message {denied_message:?}, got {response}"
            );
            let members: BTreeSet<&str> = response
                .as_object()
                .map(|body| body.keys().map(String::as_str).collect())
                .unwrap_or_default();
            assert_eq!(
                members,
                BTreeSet::from(REFUSAL_MEMBERS),
                "case {case:?}: a refusal must carry the modelled message and nothing else \
                 about the store, got {response}"
            );
            match &first {
                None => first = Some((case, response)),
                Some((first_case, first_response)) => assert_eq!(
                    &response, first_response,
                    "case {case:?}: the gate must answer exactly as it answered \
                     {first_case:?}, got {response}"
                ),
            }
        }
    }
}

/// The other side of the rule, and what makes the refusals above the GATE's doing rather
/// than some other objection to the request: every row is a request an account whose plan
/// covers the API is answered by the operation itself — served, or refused for a reason of
/// the operation's own (an unissued page token, an identifier that names nothing, a
/// recommendation of the wrong type), never with the gate's 403.
#[tokio::test]
async fn an_entitled_account_is_answered_by_the_operation_for_every_row() {
    let (denied_status, denied_code, _message) = DENIED;
    for plan in plans_the_gate_admits() {
        for row in rows() {
            let (router, _engine) = emulator_on_plan(plan);
            let case = format!("{} on {plan:?}", row.case);
            let expected_status = row.own_status;
            let expected_errortype = row.own_errortype;
            let (status, errortype, response) =
                dispatch(&router, row.method, &row.path, row.body).await;
            assert_ne!(
                status, denied_status,
                "case {case:?}: an entitled account must not be refused by the gate, got \
                 {status} with {response}"
            );
            assert_ne!(
                errortype.as_deref(),
                Some(denied_code),
                "case {case:?}: expected no {denied_code}, got {response}"
            );
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected the operation's own answer HTTP {expected_status}, got \
                 {status} with {response}"
            );
            assert_eq!(
                errortype.as_deref(),
                expected_errortype,
                "case {case:?}: expected x-amzn-errortype {expected_errortype:?}, got \
                 {errortype:?} with {response}"
            );
        }
    }
}
