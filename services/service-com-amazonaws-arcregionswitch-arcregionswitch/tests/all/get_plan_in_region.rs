// SPDX-License-Identifier: CC0-1.0

//! `GetPlanInRegion` over the wire.
//!
//! The rule under test: a Region-scoped read of a plan reports the stored plan record its
//! `arn` names — the same projection `GetPlan` reports — when the plan is available in the
//! Region the request reached, and the modelled `ResourceNotFoundException` (HTTP 404)
//! otherwise; a plan is available in exactly the Regions its own `regions` names
//! (`ListPlansInRegion` is documented to list the plans "available in the current Amazon
//! Web Services Region", and `regions` is the plan's Region pair). The Region a request
//! reached is the Region of its SigV4 credential scope.
//!
//! The request/response helpers are shared with [`crate::plan_lifecycle`]: both suites
//! drive the emulator through the same `MultiServiceRouter::dispatch` the router serves
//! on a socket (`awsJson1_0`, `POST /`, `X-Amz-Target: ArcRegionSwitch.<Operation>`).

use serde_json::{json, Value};

use crate::plan_lifecycle::{
    call, call_in_region, create_request, default_workflows, error_code, router, workflow, ACCOUNT,
};

/// Regions no row's plan covers, used as the Regions a plan is NOT available in. Each is
/// a value of the model's `Region` shape (pattern `^[a-z]{2}-[a-z-]+-\d+$`, from
/// `smithy_query(query="shape", shape="Region", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
const UNCOVERED_REGIONS: &[&str] = &["me-south-1", "ca-central-1", "ap-northeast-3"];

/// The plan rows every case spans: Region pairs that differ in their Regions, their
/// ordering and their geography, and plans that differ in which optional members they
/// carry and how many workflows and steps they hold.
fn plan_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "two US Regions, required members only",
            create_request(
                "us-pair",
                ["us-east-1", "us-west-2"],
                "activePassive",
                default_workflows(),
            ),
        ),
        (
            "the same Region pair in the other order",
            create_request(
                "us-pair-reversed",
                ["us-west-2", "us-east-1"],
                "activePassive",
                json!([workflow("activate", &["only-step"])]),
            ),
        ),
        (
            "two European Regions, active/active with optional members",
            {
                let mut request = create_request(
                    "eu-pair",
                    ["eu-west-1", "eu-central-1"],
                    "activeActive",
                    json!([
                        workflow("activate", &["a1", "a2", "a3"]),
                        workflow("deactivate", &["d1"]),
                    ]),
                );
                request["description"] = json!("a plan covering two European Regions");
                request["recoveryTimeObjectiveMinutes"] = json!(45);
                request
            },
        ),
        (
            "two Regions in different hemispheres, a primary Region named",
            {
                let mut request = create_request(
                    "ap-sa-pair",
                    ["ap-southeast-2", "sa-east-1"],
                    "activePassive",
                    json!([workflow("activate", &["shift"])]),
                );
                request["primaryRegion"] = json!("ap-southeast-2");
                request
            },
        ),
    ]
}

/// The Regions a `CreatePlan` request names.
fn regions_of(request: &Value) -> Vec<String> {
    request["regions"]
        .as_array()
        .expect("a create request names its Regions")
        .iter()
        .map(|region| {
            region
                .as_str()
                .expect("a Region is a string")
                .to_string()
        })
        .collect()
}

/// From each Region a plan covers, the Region-scoped read reports the very record
/// `GetPlan` reports — and from any other Region the plan is not there at all.
#[tokio::test]
async fn get_plan_in_region_reports_the_stored_plan_from_each_region_the_plan_covers() {
    let router = router();
    for (case, request) in plan_rows() {
        let (status, created) = call(&router, "CreatePlan", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePlan expected 200, got {status} with {created}"
        );
        let arn = created["plan"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The reference projection: what the global read reports for the same record.
        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        let stored = fetched["plan"].clone();

        for region in regions_of(&request) {
            let (status, in_region) =
                call_in_region(&router, &region, "GetPlanInRegion", &json!({"arn": arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?} in {region}: expected 200, got {status} with {in_region}"
            );
            assert_eq!(
                in_region["plan"], stored,
                "case {case:?} in {region}: expected the stored plan {stored}, got {}",
                in_region["plan"]
            );
        }

        for region in UNCOVERED_REGIONS {
            assert!(
                !regions_of(&request).iter().any(|covered| covered == region),
                "case {case:?}: {region} must be a Region the plan does not cover"
            );
            let (status, body) =
                call_in_region(&router, region, "GetPlanInRegion", &json!({"arn": arn})).await;
            assert_eq!(
                status, 404,
                "case {case:?} in {region}: expected 404, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} in {region}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// A plan that covers neither the Region the create was signed for nor the Region the
/// emulator attributes an unscoped request to is still readable from the Regions it does
/// cover, and still absent from the ones it does not: no Region is privileged.
#[tokio::test]
async fn get_plan_in_region_privileges_no_region() {
    let router = router();
    // Rows differ in the Region pair and in the Region the read is attempted from.
    let cases: &[(&str, [&str; 2], &str, u16)] = &[
        ("a covered Region", ["eu-west-1", "eu-central-1"], "eu-west-1", 200),
        (
            "the other covered Region",
            ["eu-west-1", "eu-central-1"],
            "eu-central-1",
            200,
        ),
        (
            "the Region every write in this suite is signed for",
            ["eu-west-1", "eu-central-1"],
            "us-east-1",
            404,
        ),
        (
            "a Region bordering on one of the plan's",
            ["eu-west-1", "eu-central-1"],
            "eu-west-2",
            404,
        ),
        ("a plan covering the signing Region", ["us-east-1", "us-west-2"], "us-east-1", 200),
        (
            "a plan covering the signing Region, read elsewhere",
            ["us-east-1", "us-west-2"],
            "eu-west-1",
            404,
        ),
    ];

    for (index, (case, regions, region, expected)) in cases.iter().enumerate() {
        let request = create_request(
            &format!("plan-{index}"),
            *regions,
            "activePassive",
            default_workflows(),
        );
        let (status, created) = call(&router, "CreatePlan", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePlan expected 200, got {status} with {created}"
        );
        let arn = created["plan"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) =
            call_in_region(&router, region, "GetPlanInRegion", &json!({"arn": arn})).await;
        assert_eq!(
            status, *expected,
            "case {case:?} in {region}: expected {expected}, got {status} with {body}"
        );
        if *expected == 200 {
            assert_eq!(
                body["plan"]["arn"],
                json!(arn),
                "case {case:?} in {region}: expected the plan {arn}, got {body}"
            );
            assert_eq!(
                body["plan"]["regions"],
                json!(regions),
                "case {case:?} in {region}: expected the plan's Regions {regions:?}, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} in {region}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

/// An ARN the store holds no plan under — never minted, minted for a plan since deleted,
/// or naming another plan's suffix — is the modelled 404, from a Region a plan does cover.
#[tokio::test]
async fn get_plan_in_region_refuses_an_arn_no_plan_is_stored_under() {
    let router = router();
    let request = create_request(
        "present",
        ["eu-west-1", "eu-central-1"],
        "activePassive",
        default_workflows(),
    );
    let (status, created) = call(&router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "CreatePlan expected 200, got {status} with {created}"
    );
    let stored = created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let doomed = create_request(
        "doomed",
        ["eu-west-1", "eu-central-1"],
        "activePassive",
        default_workflows(),
    );
    let (status, created) = call(&router, "CreatePlan", &doomed).await;
    assert_eq!(
        status, 200,
        "CreatePlan expected 200, got {status} with {created}"
    );
    let deleted = created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, body) = call(&router, "DeletePlan", &json!({"arn": deleted})).await;
    assert_eq!(
        status, 200,
        "DeletePlan expected 200, got {status} with {body}"
    );

    // Each row satisfies `PlanArn`'s pattern (the boundary validator refuses anything
    // else before the handler runs) and names no stored plan.
    let cases: Vec<(&str, String)> = vec![
        (
            "a name no plan was created with",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:abc123"),
        ),
        (
            "a stored plan's name with another suffix",
            format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/present:zzzzzz"),
        ),
        ("a plan that was deleted", deleted.clone()),
        (
            "another account's plan",
            "arn:aws:arc-region-switch::210987654321:plan/present:abc123".to_string(),
        ),
    ];

    for (case, arn) in cases {
        assert_ne!(
            arn, stored,
            "case {case:?}: the case must not name the stored plan"
        );
        let (status, body) =
            call_in_region(&router, "eu-west-1", "GetPlanInRegion", &json!({"arn": arn})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// The Region-scoped read reflects the record as it stands: an update is visible from
/// every Region the plan covers, and reading it changes nothing a later caller sees.
#[tokio::test]
async fn get_plan_in_region_reflects_the_plan_as_it_is_written() {
    let router = router();
    let request = create_request(
        "evolving",
        ["us-east-1", "us-west-2"],
        "activePassive",
        default_workflows(),
    );
    let (status, created) = call(&router, "CreatePlan", &request).await;
    assert_eq!(
        status, 200,
        "CreatePlan expected 200, got {status} with {created}"
    );
    let arn = created["plan"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows differ in the configuration the update writes, so each one changes a
    // different part of the record the Region-scoped read must reflect.
    let updates: Vec<(&str, Value)> = vec![
        (
            "the plan as created",
            Value::Null,
        ),
        (
            "one workflow, a description and a recovery time objective",
            json!({
                "arn": arn,
                "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/UpdatedRole"),
                "workflows": [workflow("activate", &["single"])],
                "description": "the plan after its first update",
                "recoveryTimeObjectiveMinutes": 10080,
            }),
        ),
        (
            "every optional member cleared again",
            json!({
                "arn": arn,
                "executionRole": format!("arn:aws:iam::{ACCOUNT}:role/FinalRole"),
                "workflows": default_workflows(),
            }),
        ),
    ];

    for (case, update) in updates {
        if !update.is_null() {
            let (status, updated) = call(&router, "UpdatePlan", &update).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdatePlan expected 200, got {status} with {updated}"
            );
        }

        let (status, fetched) = call(&router, "GetPlan", &json!({"arn": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPlan expected 200, got {status} with {fetched}"
        );
        let stored = fetched["plan"].clone();

        for region in ["us-east-1", "us-west-2"] {
            // Read twice: a report must not change what a later caller sees.
            for attempt in 1..=2 {
                let (status, in_region) =
                    call_in_region(&router, region, "GetPlanInRegion", &json!({"arn": arn})).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} in {region} (read {attempt}): expected 200, got {status} \
                     with {in_region}"
                );
                assert_eq!(
                    in_region["plan"], stored,
                    "case {case:?} in {region} (read {attempt}): expected the stored plan \
                     {stored}, got {}",
                    in_region["plan"]
                );
            }
        }
    }
}
