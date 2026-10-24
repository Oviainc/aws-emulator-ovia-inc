// SPDX-License-Identifier: CC0-1.0

//! The support plan the emulated account holds on a FRESHLY BOOTED emulator —
//! the account a black-box client talks to when it has done nothing but point
//! itself at the endpoint.
//!
//! The rule this suite pins is about the seeded account, not about any one call:
//! **the account the emulator presents holds an AWS Support plan whose
//! entitlement includes the Trusted Advisor API, so every operation of the
//! service answers a freshly booted emulator with its own answer rather than the
//! support-level 403 `AccessDeniedException`; the gate itself is unchanged and
//! still refuses every plan that excludes the API.**
//!
//! AWS requires a Business, Enterprise On-Ramp or Enterprise Support plan to use
//! this API at all, so every real caller of it — and therefore every recorded
//! request and response — comes from an entitled account, and the API models no
//! operation that reads, buys or changes a plan
//! (`smithy_query(query="list_operations",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`:
//! 12 operations, none plan-related). An account that booted unentitled could
//! therefore never become entitled over the wire.
//!
//! So the tables below span the whole class the rule covers: every one of the 12
//! operations the service binds (reading and mutating, implemented and still
//! scaffolded — the same rows `tests/all/support_level_gate.rs` drives the gate
//! with), and every support plan AWS offers. Nothing here records a plan in
//! process before the first request; the suite that does is the gate suite.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{
    service_state, support_level_refusal, SupportPlan,
};
use service_com_amazonaws_trustedadvisor_trustedadvisor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    dispatch, emulator_on_plan, error_code, label, recommendation_arn, set_support_plan,
};
use crate::support_level_gate::well_formed_requests;

/// One row of the "a booted emulator serves this" table: the operation, its HTTP
/// method, the URI the model binds it to, its body (`None` where it takes no
/// payload), and the output member a 200 must carry.
type ServedRow = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    Option<&'static str>,
);

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
/// engine and NOTHING recorded in process, so the store is the seeded one
/// `Default` builds on the first request that reaches the service.
///
/// Deliberately not `emulator_on_plan`, which records a support plan before the
/// first request — the state this suite is about is the one no caller touched.
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

/// The seeded account is an entitled one: whatever plan the store starts on, its
/// entitlement includes the Trusted Advisor API, so the gate's own input says
/// "serve this request".
#[tokio::test]
async fn the_seeded_account_holds_a_plan_that_includes_the_api() {
    let (_router, engine) = emulator_as_booted();
    let plan = booted_plan(&engine);
    assert!(
        plan.includes_trusted_advisor_api(),
        "case {plan:?}: a freshly booted emulator must present an account whose plan \
         includes the Trusted Advisor API, got a plan that excludes it"
    );
    let refusal = support_level_refusal(&*engine);
    assert!(
        refusal.is_none(),
        "case {plan:?}: the support-level gate must not refuse the seeded account, got \
         {refusal:?}"
    );
}

/// Every operation the service binds answers a freshly booted emulator with its
/// own answer — never the support-level 403. The rows are the 12 well-formed
/// requests, one per operation, reading and mutating alike.
#[tokio::test]
async fn every_operation_answers_a_freshly_booted_emulator() {
    for (operation, method, path, body) in well_formed_requests() {
        // A fresh emulator per row: a row that mutates must not change what the
        // next row reads, and each row must hold on a store nobody touched.
        let (router, _engine) = emulator_as_booted();
        let (status, errortype, response) = dispatch(&router, method, &path, body).await;
        let (denied_status, denied_code, _message) = DENIED;
        assert_ne!(
            status, denied_status,
            "case {operation:?}: a freshly booted emulator must not refuse the caller, got \
             {status} with {response}"
        );
        assert_ne!(
            errortype.as_deref(),
            Some(denied_code),
            "case {operation:?}: expected no {denied_code}, got {response}"
        );
        assert!(
            OWN_ANSWERS.contains(&status),
            "case {operation:?}: expected one of {OWN_ANSWERS:?}, got {status} with {response}"
        );
    }
}

/// A booted emulator does not merely fail to refuse: it SERVES its reporting
/// operations, each with the output member the model binds to its 200, computed
/// from the seeded store.
///
/// Rows differ along the operation and the member it reports — the account
/// catalog, the account and organization recommendation lists, one single
/// recommendation, one recommendation's resources, and the per-resource lookup.
/// Each output member is the one
/// `smithy_query(query="operation",operation="<op>",
/// model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
/// reports in that operation's 200 output.
#[tokio::test]
async fn a_booted_emulator_serves_the_seeded_catalog() {
    let priority = recommendation_arn("low-utilization-ec2-instances");
    let standard = recommendation_arn("amazon-ebs-snapshots");
    let rows: Vec<ServedRow> = vec![
        (
            "ListChecks",
            "GET",
            "/v1/checks".to_string(),
            None,
            Some("checkSummaries"),
        ),
        (
            "ListRecommendations",
            "GET",
            "/v1/recommendations".to_string(),
            None,
            Some("recommendationSummaries"),
        ),
        (
            "GetRecommendation",
            "GET",
            format!("/v1/recommendations/{}", label(&priority)),
            None,
            Some("recommendation"),
        ),
        (
            "ListRecommendationResources",
            "GET",
            format!("/v1/recommendations/{}/resources", label(&standard)),
            None,
            Some("recommendationResourceSummaries"),
        ),
        (
            "ListOrganizationRecommendations",
            "GET",
            "/v1/organization-recommendations".to_string(),
            None,
            Some("organizationRecommendationSummaries"),
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
            None,
            Some("recommendationForResourceSummaries"),
        ),
    ];

    for (operation, method, path, body, member) in rows {
        let (router, _engine) = emulator_as_booted();
        let (status, _errortype, response) = dispatch(&router, method, &path, body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a booted emulator must serve this operation, got {status} \
             with {response}"
        );
        if let Some(member) = member {
            assert!(
                !response[member].is_null(),
                "case {operation:?}: expected a {member} computed from the seeded store, got \
                 {response}"
            );
        }
    }
}

/// What a booted emulator answers is what an ENTITLED account is answered: for
/// every plan whose entitlement covers the API, the reads a booted store serves
/// are identical to the ones a store explicitly put on that plan serves.
///
/// This is the rule stated without naming one plan: the seeded account behaves
/// as an entitled account, whichever entitled plan it holds.
#[tokio::test]
async fn a_booted_store_answers_reads_exactly_as_an_entitled_account() {
    let reads = [
        "/v1/checks",
        "/v1/recommendations",
        "/v1/organization-recommendations",
    ];

    for plan in SupportPlan::ALL
        .iter()
        .filter(|plan| plan.includes_trusted_advisor_api())
    {
        for path in reads {
            let (booted, _booted_engine) = emulator_as_booted();
            let (entitled, _entitled_engine) = emulator_on_plan(*plan);
            let (booted_status, _errortype, booted_body) =
                dispatch(&booted, "GET", path, None).await;
            let (entitled_status, _errortype, entitled_body) =
                dispatch(&entitled, "GET", path, None).await;
            let case = format!("{path} on {plan:?}");
            assert_eq!(
                booted_status, entitled_status,
                "case {case:?}: expected the status an entitled account gets \
                 ({entitled_status}), got {booted_status} with {booted_body}"
            );
            assert_eq!(
                booted_body, entitled_body,
                "case {case:?}: expected the body an entitled account gets \
                 ({entitled_body}), got {booted_body}"
            );
        }
    }
}

/// Seeding an entitled account does not disable the gate: moving the SEEDED store
/// to a plan that excludes the API refuses every operation again with the modelled
/// 403, and moving it to one that includes the API serves them again.
///
/// The rows span every plan AWS offers crossed with every operation the service
/// binds, over one store each — so the gate's classification is observable from
/// the state a boot leaves behind, not only from a store a test pre-loaded.
#[tokio::test]
async fn the_gate_still_classifies_every_plan_over_the_seeded_store() {
    let (denied_status, denied_code, denied_message) = DENIED;

    for plan in SupportPlan::ALL {
        let entitled = plan.includes_trusted_advisor_api();
        for (operation, method, path, body) in well_formed_requests() {
            // One store per row, booted and then moved to `plan`, the way
            // something outside the API records a subscription.
            let (router, engine) = emulator_as_booted();
            set_support_plan(&engine, *plan);
            let case = format!("{operation} on {plan:?}");
            let (status, errortype, response) = dispatch(&router, method, &path, body).await;
            if entitled {
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
                assert_eq!(
                    status, denied_status,
                    "case {case:?}: expected HTTP {denied_status}, got {status} with {response}"
                );
                assert_eq!(
                    errortype.as_deref(),
                    Some(denied_code),
                    "case {case:?}: expected x-amzn-errortype {denied_code}, got {errortype:?}"
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
        }
    }
}
