// SPDX-License-Identifier: CC0-1.0

//! The support-level authorization stage of the Trusted Advisor API, over HTTP.
//!
//! The rule this suite pins is about EVERY operation the service binds, not about any
//! one call: **a request from an account whose AWS Support plan does not include the
//! Trusted Advisor API is refused with HTTP 403 `AccessDeniedException` before the
//! request's identifiers are resolved and before anything is written; an account whose
//! plan does include it gets the operation's own answer.**
//!
//! So each test below is a TABLE: the rows span the whole class the rule covers — every
//! one of the 12 operations (`OPERATIONS.md`), reading and mutating, implemented and
//! still scaffolded; every support plan AWS offers; and several identifier forms. The
//! calls the issue happened to record are single rows among them.
//!
//! The gate's ORDER is pinned too, from both sides: the modelled constraints are refused
//! ahead of it (a malformed request is still a 400 `ValidationException`), and it is
//! applied ahead of every lookup and every write (an unentitled caller is never told
//! whether a recommendation exists, and a mutation it may not make leaves the store as
//! it was).
//!
//! The last two tests pin the stage AHEAD of it, for the one operation whose input names
//! an arbitrary AWS resource: **a resource in another AWS account is refused for that
//! resource — 403 `AccessDeniedException` naming the ARN — whatever the support plan,
//! because no plan grants an account access to another account's resources.**
//!
//! Every status and error code asserted here comes from the model —
//! `smithy_query(query="operation",operation="GetRecommendation",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! and its siblings: all 12 operations bind `AccessDeniedException`, errorKind client,
//! httpError 403.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    dispatch, emulator_on_plan, error_code, label, recommendation_arn, resource_arn,
    set_support_plan,
};

/// The sentence the gate refuses with, written out here rather than read from the
/// service's own constant so the text itself is under test. The REQUIREMENT a client
/// branches on is the 403 and the `AccessDeniedException` code; this is the sentence the
/// service states them with.
const SUPPORT_LEVEL_MESSAGE: &str = "Access denied due to support level";

/// The refusal the gate answers with: the status the model binds `AccessDeniedException`
/// to, and the shape's own name, which restJson1 carries both in the body's `__type` and
/// in the `x-amzn-errortype` header.
const DENIED: (u16, &str) = (403, "AccessDeniedException");

/// The plans whose entitlement does NOT cover the Trusted Advisor API. AWS states the
/// requirement for the API as a whole — a Business, Enterprise On-Ramp or Enterprise
/// Support plan is required to use it — so these two are refused whatever they ask for.
const PLANS_WITHOUT_THE_API: &[SupportPlan] = &[SupportPlan::Basic, SupportPlan::Developer];

/// The plans whose entitlement does cover it, from that same requirement.
const PLANS_WITH_THE_API: &[SupportPlan] = &[
    SupportPlan::Business,
    SupportPlan::EnterpriseOnRamp,
    SupportPlan::Enterprise,
];

/// An `OrganizationRecommendationIdentifier` the modelled pattern admits
/// (`^arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+$`, len 20..200).
/// It names no seeded organization recommendation, so an entitled caller gets the
/// operation's own answer for an unknown identifier (404) or, where the operation is
/// still scaffolded, its 501 — the gate is what must answer ahead of either.
const ORGANIZATION_RECOMMENDATION: &str =
    "arn:aws:trustedadvisor:::organization-recommendation/00000000-0000-0000-0000-000000000000";

/// An `awsResourceArn` the modelled pattern admits
/// (`^arn:aws(-\w+)*:[\w\d-]+:([\w\d-]*)?:[\w\d_-]*([:/].+)*$`, len 20..2048): one of the
/// EC2 instances the seeded catalog flags.
fn aws_resource_arn() -> String {
    format!(
        "arn:aws:ec2:us-east-1:{}:instance/i-0abc1de23f4567890",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A recommendation ARN the modelled `AccountRecommendationIdentifier` admits, with
/// `partition`, `account` and the recommendation id as given.
fn recommendation_arn_of(partition: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:trustedadvisor::{account}:recommendation/{id}")
}

/// One well-formed request per operation the service binds — the whole class the gate
/// covers. Each row: the operation, its HTTP method, the URI the model binds it to, and
/// a body that satisfies every modelled constraint (`None` where the operation takes no
/// payload).
///
/// The identifiers name SEEDED resources, so for an entitled caller each row reaches the
/// operation's own behaviour rather than a not-found.
fn well_formed_requests() -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    // `low-utilization-ec2-instances` produced a `priority` recommendation, the type
    // whose lifecycle `UpdateRecommendationLifecycle` can move; `amazon-ebs-snapshots`
    // produced a `standard` one, whose resources are what the exclusion batch accepts.
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let standard = recommendation_arn("amazon-ebs-snapshots");
    let organization = label(ORGANIZATION_RECOMMENDATION);
    vec![
        ("ListChecks", "GET", "/v1/checks".to_string(), None),
        (
            "ListRecommendations",
            "GET",
            "/v1/recommendations".to_string(),
            None,
        ),
        (
            "GetRecommendation",
            "GET",
            format!("/v1/recommendations/{}", label(&priority)),
            None,
        ),
        (
            "ListRecommendationResources",
            "GET",
            format!("/v1/recommendations/{}/resources", label(&standard)),
            None,
        ),
        (
            "UpdateRecommendationLifecycle",
            "PUT",
            format!("/v1/recommendations/{}/lifecycle", label(&priority)),
            Some(json!({ "lifecycleStage": "dismissed" })),
        ),
        (
            "BatchUpdateRecommendationResourceExclusion",
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            Some(json!({
                "recommendationResourceExclusions": [{
                    "arn": resource_arn("amazon-ebs-snapshots", "res-ebs-0001"),
                    "isExcluded": true,
                }],
            })),
        ),
        (
            "ListRecommendationsForResource",
            "GET",
            format!(
                "/v1/recommendations-for-resource/{}",
                label(&aws_resource_arn())
            ),
            None,
        ),
        (
            "ListOrganizationRecommendations",
            "GET",
            "/v1/organization-recommendations".to_string(),
            None,
        ),
        (
            "GetOrganizationRecommendation",
            "GET",
            format!("/v1/organization-recommendations/{organization}"),
            None,
        ),
        (
            "ListOrganizationRecommendationAccounts",
            "GET",
            format!("/v1/organization-recommendations/{organization}/accounts"),
            None,
        ),
        (
            "ListOrganizationRecommendationResources",
            "GET",
            format!("/v1/organization-recommendations/{organization}/resources"),
            None,
        ),
        (
            "UpdateOrganizationRecommendationLifecycle",
            "PUT",
            format!("/v1/organization-recommendations/{organization}/lifecycle"),
            Some(json!({ "lifecycleStage": "dismissed" })),
        ),
    ]
}

/// The `message` an error body carries, or the empty string when it carries none.
fn message_of(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// Assert that one request is refused with the modelled `AccessDeniedException`: the
/// status the model binds it to, the error shape in BOTH places restJson1 carries it,
/// and `expected_message`.
async fn assert_access_denied(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
    expected_message: &str,
) {
    let (status, errortype, response) = dispatch(router, method, path_and_query, body).await;
    let (expected_status, expected_code) = DENIED;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        expected_code,
        "case {case:?}: expected __type {expected_code}, got {response}"
    );
    assert_eq!(
        errortype.as_deref(),
        Some(expected_code),
        "case {case:?}: expected x-amzn-errortype {expected_code}, got {errortype:?}"
    );
    assert_eq!(
        message_of(&response),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {response}"
    );
}

/// Assert that one request is refused by the SUPPORT-LEVEL gate.
async fn assert_denied(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) {
    assert_access_denied(
        router,
        case,
        method,
        path_and_query,
        body,
        SUPPORT_LEVEL_MESSAGE,
    )
    .await;
}

#[tokio::test]
async fn every_operation_refuses_an_account_whose_support_plan_excludes_the_api() {
    for plan in PLANS_WITHOUT_THE_API {
        let (router, _engine) = emulator_on_plan(*plan);
        for (operation, method, path, body) in well_formed_requests() {
            assert_denied(
                &router,
                &format!("{operation} on {plan:?}"),
                method,
                &path,
                body,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_plan_that_includes_the_api_is_not_refused_by_the_gate() {
    for plan in PLANS_WITH_THE_API {
        for (operation, method, path, body) in well_formed_requests() {
            // A fresh emulator per row: a row that mutates must not change what the
            // next row reads.
            let (router, _engine) = emulator_on_plan(*plan);
            let case = format!("{operation} on {plan:?}");
            let (status, errortype, response) = dispatch(&router, method, &path, body).await;
            assert_ne!(
                status,
                DENIED.0,
                "case {case:?}: an entitled account must not be refused by the gate, got \
                 {status} with {response}"
            );
            assert_ne!(
                errortype.as_deref(),
                Some(DENIED.1),
                "case {case:?}: expected no {} , got {response}",
                DENIED.1
            );
            // The operation's own answer: it is served, it found nothing, it conflicts,
            // or it is one this emulator has not implemented yet (501).
            assert!(
                [200, 404, 409, 501].contains(&status),
                "case {case:?}: expected the operation's own answer, got {status} with \
                 {response}"
            );
        }
    }
}

/// The classification above covers every plan the service models, so a plan added later
/// cannot silently fall on the permissive side of the gate.
#[tokio::test]
async fn every_support_plan_is_classified_by_the_gate() {
    for plan in SupportPlan::ALL {
        let denied = PLANS_WITHOUT_THE_API.contains(plan);
        let allowed = PLANS_WITH_THE_API.contains(plan);
        assert!(
            denied != allowed,
            "case {plan:?}: every plan must be either with or without the API, got \
             denied={denied} allowed={allowed}"
        );
        assert_eq!(
            plan.includes_trusted_advisor_api(),
            allowed,
            "case {plan:?}: the plan's own entitlement disagrees with this suite's table"
        );
    }
}

/// The modelled constraints are refused AHEAD of the gate: a malformed request is a 400
/// `ValidationException` even from an account the gate would refuse — the rows differ
/// along the constraint broken (pattern, length, range, enum, list size, required
/// member).
#[tokio::test]
async fn shape_validation_precedes_the_gate() {
    let (router, _engine) = emulator_on_plan(SupportPlan::Basic);
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "a recommendation identifier shorter than the modelled minimum",
            "GET",
            "/v1/recommendations/arn:aws:ta".to_string(),
            None,
        ),
        (
            "a recommendation identifier outside the modelled pattern",
            "GET",
            format!(
                "/v1/recommendations/{}",
                label("arn:aws:trustedadvisor::00000000000:recommendation/not-twelve-digits")
            ),
            None,
        ),
        (
            "maxResults below the modelled range",
            "GET",
            "/v1/checks?maxResults=0".to_string(),
            None,
        ),
        (
            "maxResults past the modelled range",
            "GET",
            "/v1/recommendations?maxResults=201".to_string(),
            None,
        ),
        (
            "a pillar outside the modelled enum",
            "GET",
            "/v1/checks?pillar=not_a_pillar".to_string(),
            None,
        ),
        (
            "an empty exclusion batch, below the modelled list minimum",
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            Some(json!({ "recommendationResourceExclusions": [] })),
        ),
        (
            "a lifecycle update with no lifecycleStage",
            "PUT",
            format!("/v1/recommendations/{}/lifecycle", label(&priority)),
            Some(json!({})),
        ),
    ];

    for (case, method, path, body) in rows {
        let (status, errortype, response) = dispatch(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 ahead of the gate, got {status} with {response}"
        );
        assert_eq!(
            errortype.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected x-amzn-errortype ValidationException, got {errortype:?} \
             with {response}"
        );
    }
}

/// The gate is applied AHEAD of identifier resolution: a well-formed identifier that
/// names nothing is 403 for an unentitled account — never the 404 that would tell it
/// which recommendations exist — and the SAME request is 404 once the account's plan
/// includes the API.
#[tokio::test]
async fn the_gate_precedes_identifier_resolution() {
    let account = engine::EMULATOR_ACCOUNT_ID;
    // Rows differ along what makes the ARN name nothing: the recorded all-zeros id, an
    // id no seed produced, another partition, and another account.
    let unknown = [
        (
            "the recorded all-zeros recommendation id",
            recommendation_arn_of(
                "aws",
                account,
                "00000000-0000-0000-0000-000000000000",
            ),
        ),
        (
            "a recommendation id no seed produced",
            recommendation_arn_of("aws", account, "not-a-seeded-recommendation"),
        ),
        (
            "a seeded id in another partition",
            recommendation_arn_of(
                "aws-us-gov",
                account,
                "low-utilization-ec2-instances-recommendation",
            ),
        ),
        (
            "a seeded id in another account",
            recommendation_arn_of(
                "aws",
                "123456789012",
                "low-utilization-ec2-instances-recommendation",
            ),
        ),
    ];

    for (label_of_arn, arn) in unknown {
        // Every operation that takes a recommendation identifier, at its modelled URI.
        let requests: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "GetRecommendation",
                "GET",
                format!("/v1/recommendations/{}", label(&arn)),
                None,
            ),
            (
                "ListRecommendationResources",
                "GET",
                format!("/v1/recommendations/{}/resources", label(&arn)),
                None,
            ),
            (
                "UpdateRecommendationLifecycle",
                "PUT",
                format!("/v1/recommendations/{}/lifecycle", label(&arn)),
                Some(json!({ "lifecycleStage": "resolved" })),
            ),
        ];

        for (operation, method, path, body) in requests {
            let case = format!("{operation} with {label_of_arn}");
            let (unentitled, _engine) = emulator_on_plan(SupportPlan::Basic);
            assert_denied(&unentitled, &case, method, &path, body.clone()).await;

            let (entitled, _engine) = emulator_on_plan(SupportPlan::Enterprise);
            let (status, errortype, response) = dispatch(&entitled, method, &path, body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: an entitled account must get the lookup's own answer, got \
                 {status} with {response}"
            );
            assert_eq!(
                errortype.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected x-amzn-errortype ResourceNotFoundException, got \
                 {errortype:?} with {response}"
            );
        }
    }
}

/// The gate is applied AHEAD of every write, so a mutation the account may not make
/// changes nothing: the same requests, replayed once the plan includes the API, find the
/// store exactly as the seed left it.
#[tokio::test]
async fn a_refused_mutation_leaves_the_store_untouched() {
    let (router, engine) = emulator_on_plan(SupportPlan::Basic);
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let standard = recommendation_arn("amazon-ebs-snapshots");
    let resource = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");

    // Both mutating operations, each refused three times: the gate reads state, so a
    // refusal is not a write and cannot be exhausted.
    for attempt in 1..=3 {
        assert_denied(
            &router,
            &format!("UpdateRecommendationLifecycle attempt {attempt}"),
            "PUT",
            &format!("/v1/recommendations/{}/lifecycle", label(&priority)),
            Some(json!({ "lifecycleStage": "resolved", "updateReasonCode": "not_applicable" })),
        )
        .await;
        assert_denied(
            &router,
            &format!("BatchUpdateRecommendationResourceExclusion attempt {attempt}"),
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion",
            Some(json!({
                "recommendationResourceExclusions": [{ "arn": resource, "isExcluded": true }],
            })),
        )
        .await;
    }

    // Now subscribe the account and read the same store back: neither refused mutation
    // reached it.
    set_support_plan(&engine, SupportPlan::Business);

    let (status, _errortype, got) = dispatch(
        &router,
        "GET",
        &format!("/v1/recommendations/{}", label(&priority)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "an entitled GetRecommendation must be served, got {status} with {got}"
    );
    assert_eq!(
        got["recommendation"]["lifecycleStage"].as_str(),
        Some("pending_response"),
        "a refused lifecycle update must not move the stage, got {got}"
    );
    assert!(
        got["recommendation"]["resolvedAt"].is_null(),
        "a refused lifecycle update must not stamp resolvedAt, got {got}"
    );

    let (status, _errortype, got) = dispatch(
        &router,
        "GET",
        &format!("/v1/recommendations/{}/resources", label(&standard)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "an entitled ListRecommendationResources must be served, got {status} with {got}"
    );
    let exclusion = got["recommendationResourceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no recommendationResourceSummaries: {got}"))
        .iter()
        .find(|summary| summary["arn"].as_str() == Some(resource.as_str()))
        .map(|summary| summary["exclusionStatus"].as_str().unwrap_or_default())
        .unwrap_or_else(|| panic!("the seeded resource is not reported: {got}"))
        .to_string();
    assert_eq!(
        exclusion, "included",
        "a refused exclusion batch must not change the resource, got {got}"
    );
}

// ---------------------------------------------------------------------------
// The resource an operation targets is authorized before the support plan
// ---------------------------------------------------------------------------

/// `ListRecommendationsForResource` is the one operation whose input names an ARBITRARY
/// AWS resource, and a resource in ANOTHER account is refused for that resource — 403
/// `AccessDeniedException` naming the ARN that was refused — whatever the caller's
/// support plan, because no plan grants an account access to another account's
/// resources.
///
/// The rows differ along the service, the region, the partition, the resource-id
/// separator (`/` and `:`) and the account the ARN names; the first is the recorded one.
#[tokio::test]
async fn a_resource_in_another_account_is_refused_naming_that_resource() {
    let foreign = [
        (
            "an EC2 instance in another account",
            "arn:aws:ec2:us-east-1:111111111111:instance/i-0123456789abcdef0",
        ),
        (
            "an RDS instance in another account, with a `:` separator",
            "arn:aws:rds:eu-west-1:210987654321:db:reporting-prod",
        ),
        (
            "a Lambda function in another partition and account",
            "arn:aws-us-gov:lambda:us-gov-west-1:123456789012:function:ingest",
        ),
    ];

    // Every plan: the resource refusal does not depend on the support plan.
    for plan in SupportPlan::ALL {
        let (router, _engine) = emulator_on_plan(*plan);
        for (case, arn) in foreign {
            assert_access_denied(
                &router,
                &format!("{case} on {plan:?}"),
                "GET",
                &format!("/v1/recommendations-for-resource/{}", label(arn)),
                None,
                &format!("Access denied to resource: {arn}"),
            )
            .await;
        }
    }
}

/// The other side of that rule: an ARN in the caller's own account — or one whose
/// resource type carries no account segment at all — is NOT refused for the resource. It
/// reaches the support-level stage, and an entitled caller reaches the operation itself
/// (still scaffolded, so 501).
#[tokio::test]
async fn a_resource_the_caller_owns_is_not_refused_for_the_resource() {
    let own = [
        (
            "the caller's own EC2 instance",
            format!(
                "arn:aws:ec2:us-east-1:{}:instance/i-0abc1de23f4567890",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "an S3 bucket, whose ARNs carry no account segment",
            "arn:aws:s3:::trusted-advisor-probe-bucket".to_string(),
        ),
    ];

    for (case, arn) in own {
        let path = format!("/v1/recommendations-for-resource/{}", label(&arn));

        // An unentitled caller: the support-level sentence, not the resource one.
        let (unentitled, _engine) = emulator_on_plan(SupportPlan::Developer);
        assert_denied(&unentitled, case, "GET", &path, None).await;

        // An entitled caller: the operation's own answer.
        let (entitled, _engine) = emulator_on_plan(SupportPlan::EnterpriseOnRamp);
        let (status, _errortype, response) = dispatch(&entitled, "GET", &path, None).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected the operation's own answer, got {status} with {response}"
        );
    }
}
