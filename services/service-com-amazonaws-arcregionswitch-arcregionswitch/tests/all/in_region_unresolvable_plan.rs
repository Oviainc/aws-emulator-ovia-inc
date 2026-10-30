// SPDX-License-Identifier: CC0-1.0

//! The status the Region-scoped (`...InRegion`) reads answer an unresolvable plan `arn`
//! with.
//!
//! The rule under test: a Region-scoped read whose `arn` resolves to no plan from the
//! Region the call reached answers the service's not-found refusal — the modelled
//! `ResourceNotFoundException` shape, its wire error code and its message, built from the
//! ARN the request carried — at HTTP status 400, while the Region-agnostic reads answer
//! the same shape, code and message at the 404 the shape's `@httpError` names. The status
//! is the only difference between the two classes of read.
//!
//! An `arn` resolves from a Region when a plan is stored under it AND that plan's own
//! `regions` names the Region: a plan carries exactly two (`RegionList`,
//! `@length(min: 2, max: 2)`), and `ListPlansInRegion` is documented to list the plans
//! "available in the current Amazon Web Services Region"
//! (`smithy_query(query="operation", operation="ListPlansInRegion", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
//!
//! Every case drives the emulator through the same `MultiServiceRouter::dispatch` the
//! router serves on a socket, so what these tests observe is what an SDK observes:
//! `awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`.

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::{
    plan_not_found_message, IN_REGION_NOT_FOUND_STATUS,
};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::plan_lifecycle::{
    call, call_in_region, create_request, error_code, error_message, router, ACCOUNT,
};
use crate::route53_health_checks::{health_check_step, record_sets};

/// The operations the rule covers: every read of this service that takes a plan `arn` and
/// answers for the Region the call reached. `ListPlansInRegion` is Region-scoped too but
/// takes no `arn`, so it has no argument to resolve.
const REGION_SCOPED_READS: &[&str] = &["GetPlanInRegion", "ListRoute53HealthChecksInRegion"];

/// The Region-agnostic reads of the same plan, which resolve an ARN account-wide: the
/// control for the rule, since only the status may differ.
const REGION_AGNOSTIC_READS: &[&str] = &["GetPlan", "ListRoute53HealthChecks"];

/// The status a Region-agnostic read answers the same refusal with: the one
/// `ResourceNotFoundException`'s `@httpError` names
/// (`smithy_query(query="shape", shape="ResourceNotFoundException", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
const MODELLED_NOT_FOUND_STATUS: u16 = 404;

/// A `CreatePlan` request for a plan covering `regions`, with one `Route53HealthCheck`
/// step so the Region-scoped list has something to report when the `arn` does resolve.
fn plan_request(name: &str, regions: [&str; 2]) -> Value {
    create_request(
        name,
        regions,
        "activePassive",
        json!([{
            "workflowTargetAction": "activate",
            "workflowDescription": "the activate workflow",
            "steps": [health_check_step(
                "hc",
                "Zalpha",
                "example.com",
                record_sets(&regions),
            )],
        }]),
    )
}

/// Create a plan covering `regions` and return its ARN.
async fn create_plan(router: &MultiServiceRouter, name: &str, regions: [&str; 2]) -> String {
    let (status, created) = call(router, "CreatePlan", &plan_request(name, regions)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreatePlan expected 200, got {status} with {created}"
    );
    created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The request body each read under test takes for `arn`: every one of them names the
/// plan with that single required member.
fn request_for(arn: &str) -> Value {
    json!({"arn": arn})
}

/// Assert `operation`, called in `region` with `arn`, answered the service's not-found
/// refusal at `expected_status`: the same shape, wire error code and message whichever
/// status it carries.
async fn assert_not_found_refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    region: &str,
    arn: &str,
    expected_status: u16,
) {
    let (status, body) = call_in_region(router, region, operation, &request_for(arn)).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status}, got {status} with {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {code:?} in {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message,
        plan_not_found_message(arn),
        "case {case:?}: expected the message built from the request's ARN, got {message:?}"
    );
}

/// An `arn` the store holds no plan under is refused by every Region-scoped read, from
/// every Region, at the Region-scoped status.
#[tokio::test]
async fn a_region_scoped_read_refuses_an_arn_no_plan_is_stored_under_at_the_in_region_status() {
    let router = router();
    // A plan that IS stored, so each row is about the ARN it names rather than about an
    // empty store; and one that was stored and deleted.
    let kept = create_plan(&router, "kept", ["us-east-1", "us-west-2"]).await;
    let deleted = create_plan(&router, "doomed", ["us-east-1", "us-west-2"]).await;
    let (status, body) = call(&router, "DeletePlan", &request_for(&deleted)).await;
    assert_eq!(
        status, 200,
        "DeletePlan expected 200, got {status} with {body}"
    );

    // Rows differ along why the ARN resolves to no plan. Each satisfies `PlanArn`'s
    // pattern, so the boundary validator passes it to the handler.
    let arns: Vec<(&str, String)> = vec![
        (
            "an ARN no plan was ever created under",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
        ),
        (
            "a stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/kept:zzzzzz"),
        ),
        ("a plan that was deleted", deleted.clone()),
        (
            "another account's plan",
            "arn:aws:arc-region-switch::210987654321:plan/kept:aaaaaa".to_string(),
        ),
        (
            "another partition's plan",
            format!("arn:aws-us-gov:arc-region-switch::{ACCOUNT}:plan/kept:aaaaaa"),
        ),
    ];
    // The Region reached is a dimension of its own: a Region the stored plan covers and
    // ones it does not must answer alike when nothing is stored under the ARN.
    let regions = ["us-east-1", "eu-west-1", "ap-northeast-3"];

    for (why, arn) in arns {
        assert_ne!(
            arn, kept,
            "case {why:?}: the row must not name the stored plan"
        );
        for operation in REGION_SCOPED_READS {
            for region in regions {
                let case = format!("{operation} in {region} on {why}");
                assert_not_found_refusal(
                    &router,
                    &case,
                    operation,
                    region,
                    &arn,
                    IN_REGION_NOT_FOUND_STATUS,
                )
                .await;
            }
        }
    }
}

/// A plan the store DOES hold is refused the same way by a Region-scoped read made from a
/// Region the plan does not cover — and it reports, unchanged, from each Region it covers.
#[tokio::test]
async fn a_region_scoped_read_refuses_a_stored_plan_from_a_region_it_does_not_cover() {
    let router = router();
    // Rows differ along the plan's Region pair — its ordering and its geography — and
    // along the Region the read reaches: a neighbour of a covered Region, another
    // continent, and a Region in another partition.
    let rows: &[(&str, [&str; 2], &[&str])] = &[
        (
            "two US Regions",
            ["us-east-1", "us-west-2"],
            &["eu-west-1", "us-east-2", "ap-southeast-2"],
        ),
        (
            "the same pair in the other order",
            ["us-west-2", "us-east-1"],
            &["ca-central-1", "us-gov-west-1"],
        ),
        (
            "two European Regions",
            ["eu-west-1", "eu-central-1"],
            &["eu-west-2", "us-east-1", "me-south-1"],
        ),
        (
            "two Regions in different hemispheres",
            ["ap-southeast-2", "sa-east-1"],
            &["ap-southeast-1", "eu-north-1"],
        ),
    ];

    for (index, (case, covered, uncovered)) in rows.iter().enumerate() {
        let arn = create_plan(&router, &format!("plan-{index}"), *covered).await;

        // What each Region-scoped read reports from each Region the plan covers, before
        // any refusal.
        let mut reported: Vec<(&str, &str, Value)> = Vec::new();
        for operation in REGION_SCOPED_READS {
            for region in *covered {
                let (status, body) =
                    call_in_region(&router, region, operation, &request_for(&arn)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: {operation} in {region} expected 200, got {status} with \
                     {body}"
                );
                reported.push((operation, region, body));
            }
        }

        for region in *uncovered {
            assert!(
                !covered.contains(region),
                "case {case:?}: {region} must be a Region the plan does not cover"
            );
            for operation in REGION_SCOPED_READS {
                let refusal = format!("{operation} in {region} on a plan covering {covered:?}");
                assert_not_found_refusal(
                    &router,
                    &refusal,
                    operation,
                    region,
                    &arn,
                    IN_REGION_NOT_FOUND_STATUS,
                )
                .await;
            }
        }

        // The plan still resolves from each Region it covers: the refusals changed nothing
        // a later caller sees.
        for (operation, region, before) in &reported {
            let (status, body) =
                call_in_region(&router, region, operation, &request_for(&arn)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} in {region} expected 200, got {status} with {body}"
            );
            assert_eq!(
                &body, before,
                "case {case:?}: {operation} in {region} expected {before}, got {body}"
            );
        }
    }
}

/// The control: the Region-agnostic reads answer the SAME shape, code and message at the
/// status the model gives that shape, and report a stored plan from any Region at all —
/// so the Region-scoped status is what the rule is about, not the refusal itself.
#[tokio::test]
async fn a_region_agnostic_read_answers_the_same_refusal_at_the_modelled_status() {
    let router = router();
    let arn = create_plan(&router, "global", ["eu-west-1", "eu-central-1"]).await;

    // Rows differ along why the ARN names no plan, as in the Region-scoped cases.
    let absent: Vec<(&str, String)> = vec![
        (
            "an ARN no plan was ever created under",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
        ),
        (
            "a stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/global:zzzzzz"),
        ),
        (
            "another account's plan",
            "arn:aws:arc-region-switch::210987654321:plan/global:aaaaaa".to_string(),
        ),
    ];

    for (why, missing) in absent {
        for operation in REGION_AGNOSTIC_READS {
            let case = format!("{operation} on {why}");
            assert_not_found_refusal(
                &router,
                &case,
                operation,
                "us-east-1",
                &missing,
                MODELLED_NOT_FOUND_STATUS,
            )
            .await;
        }
    }

    // The same reads of the STORED plan answer from a Region the plan does not cover,
    // where the Region-scoped ones refuse: only the Region-scoped reads narrow by Region.
    for region in ["eu-west-1", "us-east-1", "ap-northeast-3"] {
        for operation in REGION_AGNOSTIC_READS {
            let case = format!("{operation} in {region} on the stored plan");
            let (status, body) =
                call_in_region(&router, region, operation, &request_for(&arn)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
        }
    }
}
