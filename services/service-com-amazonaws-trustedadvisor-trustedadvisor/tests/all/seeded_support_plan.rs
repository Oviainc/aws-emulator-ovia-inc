// SPDX-License-Identifier: CC0-1.0

//! The support plan the emulated account holds on a FRESHLY BOOTED emulator —
//! the account a black-box client talks to when it has done nothing but point
//! itself at the endpoint.
//!
//! The rule this suite pins is about the account the service answers, not about
//! any one call: **a support subscription arrives from outside this API, so the
//! account the emulator presents holds the plan every AWS account includes until
//! it subscribes to another — a plan whose entitlement excludes the Trusted
//! Advisor API — and every operation the service binds therefore refuses a
//! freshly booted emulator with the modelled 403 `AccessDeniedException`, ahead
//! of resolving any identifier and ahead of any write; once a plan whose
//! entitlement covers the API is recorded from outside the API, the same
//! operations answer with their own answers computed from the store.**
//!
//! AWS requires a Business, Enterprise On-Ramp or Enterprise Support plan to use
//! this API at all, and the API models no operation that reads, buys or changes a
//! plan (`smithy_query(query="list_operations",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`:
//! 12 operations, none plan-related), so nothing a caller sends can bring a
//! subscription into existence and the plan is an input the emulator takes from
//! its operator.
//!
//! So the tables below span the whole class the rule covers: every one of the 12
//! operations the service binds (reading and mutating, implemented and still
//! scaffolded — the same rows `tests/all/support_level_gate.rs` drives the gate
//! with), every support plan AWS offers, every published plan name in the forms
//! an operator writes it, and several identifier forms that name nothing.
//!
//! Nothing in this suite's environment names a plan, so the rows about a booted
//! emulator are about the plan the store starts on by itself; the rows about a
//! subscribed account record the plan the way something outside the API records
//! one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    service_state, support_level_refusal, SupportPlan, SUPPORT_PLAN_VAR,
    UNSUBSCRIBED_SUPPORT_PLAN,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    dispatch, error_code, label, recommendation_arn, resource_arn, set_support_plan,
};
use crate::support_level_gate::well_formed_requests;

/// One row of the "a subscribed account is served this" table: the operation, its
/// HTTP method, the URI the model binds it to, and the output member its 200 must
/// carry.
type ServedRow = (&'static str, &'static str, String, &'static str);

/// The refusal the support-level gate answers with, as `tests/all/support_level_gate.rs`
/// states it: the status the model binds `AccessDeniedException` to, the shape name
/// restJson1 carries in `x-amzn-errortype` and `__type`, and the gate's sentence.
const DENIED: (u16, &str, &str) = (
    403,
    "AccessDeniedException",
    "Access denied due to support level",
);

/// The statuses that are an operation's OWN answer: served, nothing found, a
/// conflicting lifecycle, or an operation this emulator has not implemented yet
/// (the 501 a scaffold answers).
const OWN_ANSWERS: &[u16] = &[200, 404, 409, 501];

/// A mounted emulator exactly as the router mounts one at boot: a fresh shared
/// engine and NOTHING recorded in process, so the store is the one `Default`
/// builds on the first request that reaches the service.
fn emulator_as_booted() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The plan a booted store carries, read through the service's own accessor.
fn booted_plan(engine: &engine::InMemoryEngine) -> SupportPlan {
    service_state(engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .support_plan()
}

/// A recommendation ARN the modelled `AccountRecommendationIdentifier` admits
/// (`^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$`), with
/// `partition`, `account` and the recommendation id as given.
fn recommendation_arn_of(partition: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:trustedadvisor::{account}:recommendation/{id}")
}

/// Assert one request is refused by the support-level gate: the status the model
/// binds `AccessDeniedException` to, the shape in BOTH places restJson1 carries
/// it, and the gate's sentence.
async fn assert_denied(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) {
    let (denied_status, denied_code, denied_message) = DENIED;
    let (status, errortype, response) = dispatch(router, method, path_and_query, body).await;
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
}

/// The account a boot presents is an UNSUBSCRIBED one: the plan the store starts
/// on is the plan every AWS account includes, its entitlement excludes the API,
/// and the gate's own input therefore says "refuse this request".
#[tokio::test]
async fn the_booted_account_holds_the_plan_an_unsubscribed_account_holds() {
    let (_router, engine) = emulator_as_booted();
    let plan = booted_plan(&engine);
    assert_eq!(
        plan, UNSUBSCRIBED_SUPPORT_PLAN,
        "case {plan:?}: a booted store must hold the plan an account holds until it \
         subscribes ({UNSUBSCRIBED_SUPPORT_PLAN:?}); nothing in this suite names a plan in \
         {SUPPORT_PLAN_VAR}"
    );
    assert!(
        !plan.includes_trusted_advisor_api(),
        "case {plan:?}: the plan an unsubscribed account holds must not include the Trusted \
         Advisor API"
    );
    let refusal = support_level_refusal(&*engine);
    assert!(
        refusal.is_some(),
        "case {plan:?}: the support-level gate must refuse an unsubscribed account, got \
         {refusal:?}"
    );
}

/// Every operation the service binds refuses a freshly booted emulator with the
/// modelled 403. The rows are the 12 well-formed requests, one per operation,
/// reading and mutating, implemented and still scaffolded alike — a scaffold's
/// 501 is not reached either, because the gate answers ahead of it.
#[tokio::test]
async fn every_operation_refuses_a_freshly_booted_emulator() {
    for (operation, method, path, body) in well_formed_requests() {
        // A fresh emulator per row: every row must hold on a store nobody touched.
        let (router, _engine) = emulator_as_booted();
        assert_denied(&router, operation, method, &path, body).await;
    }
}

/// The refusal a booted emulator answers with comes BEFORE identifier
/// resolution: a well-formed identifier that names nothing is the same 403, never
/// the 404 that would tell an unsubscribed caller which recommendations exist.
///
/// The rows differ along what makes the ARN name nothing — an all-zeros id, an id
/// no seed produced, another partition, another account — crossed with every
/// operation that takes a recommendation identifier.
#[tokio::test]
async fn a_booted_emulator_refuses_before_it_resolves_an_identifier() {
    let account = engine::EMULATOR_ACCOUNT_ID;
    let unknown = [
        (
            "an all-zeros recommendation id",
            recommendation_arn_of("aws", account, "00000000-0000-0000-0000-000000000000"),
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
            let (router, _engine) = emulator_as_booted();
            let case = format!("{operation} with {label_of_arn}");
            assert_denied(&router, &case, method, &path, body).await;
        }
    }
}

/// And before any write: the mutating operations, refused on a booted store,
/// leave it exactly as the seed left it — proved by recording a subscription
/// afterwards and reading the same store back.
#[tokio::test]
async fn a_booted_emulator_refuses_before_any_write() {
    let (router, engine) = emulator_as_booted();
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let standard = recommendation_arn("amazon-ebs-snapshots");
    let resource = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");

    for attempt in 1..=3 {
        assert_denied(
            &router,
            &format!("UpdateRecommendationLifecycle attempt {attempt}"),
            "PUT",
            &format!("/v1/recommendations/{}/lifecycle", label(&priority)),
            Some(json!({
                "lifecycleStage": "resolved",
                "updateReasonCode": "not_applicable",
            })),
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
        "a subscribed GetRecommendation must be served, got {status} with {got}"
    );
    assert_eq!(
        got["recommendation"]["lifecycleStage"].as_str(),
        Some("pending_response"),
        "a refused lifecycle update must not move the stage, got {got}"
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
        "a subscribed ListRecommendationResources must be served, got {status} with {got}"
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

/// The plan recorded from outside the API is what the gate evaluates, over the
/// store a boot leaves behind: every plan AWS offers crossed with every operation
/// the service binds, one store each — the plans that cover the API get the
/// operation's own answer, the ones that do not get the modelled 403.
#[tokio::test]
async fn the_recorded_plan_is_what_the_gate_evaluates_over_the_booted_store() {
    let (_denied_status, denied_code, _denied_message) = DENIED;

    for plan in SupportPlan::ALL {
        let covered = plan.includes_trusted_advisor_api();
        for (operation, method, path, body) in well_formed_requests() {
            // One store per row, booted and then moved to `plan`, the way
            // something outside the API records a subscription.
            let (router, engine) = emulator_as_booted();
            set_support_plan(&engine, *plan);
            let case = format!("{operation} on {plan:?}");
            if covered {
                let (status, errortype, response) = dispatch(&router, method, &path, body).await;
                assert!(
                    OWN_ANSWERS.contains(&status),
                    "case {case:?}: expected one of {OWN_ANSWERS:?}, got {status} with \
                     {response}"
                );
                assert_ne!(
                    errortype.as_deref(),
                    Some(denied_code),
                    "case {case:?}: expected no {denied_code}, got {response}"
                );
            } else {
                assert_denied(&router, &case, method, &path, body).await;
            }
        }
    }
}

/// A subscribed account does not merely stop being refused: it is SERVED the
/// reporting operations, each with the output member the model binds to its 200,
/// computed from the store the boot seeded — over every plan whose entitlement
/// covers the API, so the rule names no single plan.
///
/// Each output member is the one `smithy_query(query="operation",operation="<op>",
/// model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
/// reports in that operation's 200 output.
#[tokio::test]
async fn a_subscribed_account_is_served_the_seeded_catalog() {
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let standard = recommendation_arn("amazon-ebs-snapshots");
    let rows: Vec<ServedRow> = vec![
        ("ListChecks", "GET", "/v1/checks".to_string(), "checkSummaries"),
        (
            "ListRecommendations",
            "GET",
            "/v1/recommendations".to_string(),
            "recommendationSummaries",
        ),
        (
            "GetRecommendation",
            "GET",
            format!("/v1/recommendations/{}", label(&priority)),
            "recommendation",
        ),
        (
            "ListRecommendationResources",
            "GET",
            format!("/v1/recommendations/{}/resources", label(&standard)),
            "recommendationResourceSummaries",
        ),
        (
            "ListOrganizationRecommendations",
            "GET",
            "/v1/organization-recommendations".to_string(),
            "organizationRecommendationSummaries",
        ),
        (
            "ListRecommendationsForResource",
            "GET",
            format!(
                "/v1/recommendations-for-resource/{}",
                label(&format!(
                    "arn:aws:ec2:us-east-1:{}:instance/i-0abc1de23f4567890",
                    engine::EMULATOR_ACCOUNT_ID
                ))
            ),
            "recommendationForResourceSummaries",
        ),
    ];

    for plan in SupportPlan::ALL
        .iter()
        .filter(|plan| plan.includes_trusted_advisor_api())
    {
        for (operation, method, path, member) in &rows {
            let (router, engine) = emulator_as_booted();
            set_support_plan(&engine, *plan);
            let case = format!("{operation} on {plan:?}");
            let (status, _errortype, response) = dispatch(&router, method, path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a subscribed account must be served this operation, got \
                 {status} with {response}"
            );
            assert!(
                !response[member].is_null(),
                "case {case:?}: expected a {member} computed from the seeded store, got \
                 {response}"
            );
        }
    }
}

/// The plan an operator names is read as the plan AWS publishes under that name,
/// whatever case or separator it is written with; a name that is not one of the
/// five plans names no plan at all, so the store keeps the unsubscribed one.
///
/// The five names are AWS's own (AWS Support plans: Basic, Developer, Business,
/// Enterprise On-Ramp, Enterprise), and the rows differ along the case, the
/// separator and the surrounding space an operator writes them with.
#[tokio::test]
async fn every_published_plan_name_names_its_plan() {
    let recognized: &[(&str, SupportPlan)] = &[
        ("basic", SupportPlan::Basic),
        ("BASIC", SupportPlan::Basic),
        ("developer", SupportPlan::Developer),
        ("Developer", SupportPlan::Developer),
        ("business", SupportPlan::Business),
        ("Business", SupportPlan::Business),
        ("enterprise", SupportPlan::Enterprise),
        ("ENTERPRISE", SupportPlan::Enterprise),
        ("enterprise-on-ramp", SupportPlan::EnterpriseOnRamp),
        ("enterprise_on_ramp", SupportPlan::EnterpriseOnRamp),
        ("Enterprise On-Ramp", SupportPlan::EnterpriseOnRamp),
        ("EnterpriseOnRamp", SupportPlan::EnterpriseOnRamp),
    ];
    for (name, expected) in recognized {
        let got = SupportPlan::parse(name);
        assert_eq!(
            got,
            Some(*expected),
            "case {name:?}: expected the plan {expected:?}, got {got:?}"
        );
    }

    // Every plan the service models is reachable by its own name, so a plan added
    // later cannot be unnameable by an operator.
    for plan in SupportPlan::ALL {
        let name = format!("{plan:?}");
        let got = SupportPlan::parse(&name);
        assert_eq!(
            got,
            Some(*plan),
            "case {name:?}: every modelled plan must be nameable, got {got:?}"
        );
    }

    let unrecognized = [
        "",
        "   ",
        "enterprise-on-rampage",
        "basics",
        "business-support",
        "none",
        "12345",
    ];
    for name in unrecognized {
        let got = SupportPlan::parse(name);
        assert_eq!(
            got, None,
            "case {name:?}: a name that is not one of the five plans must name no plan, got \
             {got:?}"
        );
    }
}
