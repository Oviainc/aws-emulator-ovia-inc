// SPDX-License-Identifier: CC0-1.0

//! One refusal, two paths: the service-role refusal a deployment group's create and
//! update answer must be the SAME response whichever path produced it.
//!
//! RULE under test: CodeDeploy judges the service role before the request's other
//! member values, so a role refusal is answered both by the handler (for a body the
//! framework's validating walk accepts) and ahead of that walk (for a body it
//! refuses). A client cannot be shown two different refusals for one rule, so the
//! two paths must answer alike: the same status, the same modelled error code and
//! the same sentence — and that answer must be the modelled refusal, not merely the
//! same on both sides.
//!
//! (This suite's file name cannot be changed from the environment the fix was made
//! in: it has no rename and no delete, and the crate's test layout binds the module
//! name to the file name. The suite is the role-refusal path parity.)

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, given_role, trust_policy, Emulator};

/// A tag filter whose `type` is outside the modelled `EC2TagFilterType`
/// (`smithy_query(query="shape",shape="EC2TagFilterType")`: KEY_ONLY | VALUE_ONLY |
/// KEY_AND_VALUE), which the framework's validating walk refuses before any handler
/// runs. It is what makes a request take the pre-dispatch path.
fn off_enum_filters() -> Value {
    json!([{"Key": "k", "Value": "v", "Type": "BOGUS_TYPE"}])
}

/// RULE: one role refusal, the same modelled answer on both paths.
///
/// Rows differ along the operation that sets the role and along which refusal the
/// role earns — a value that is no ARN, an ARN of the wrong kind, no value at all,
/// and a role the account holds that does not trust CodeDeploy. Each row is sent
/// twice: once with a body the walk accepts (the handler answers) and once with the
/// same body plus an out-of-enum tag filter (the walk would refuse the body, so the
/// role is answered ahead of it).
#[tokio::test]
async fn one_role_refusal_reads_the_same_on_both_paths() {
    let emulator = Emulator::with_iam();
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": "parity", "computePlatform": "Server"}),
        )
        .await;
    let untrusted = given_role(
        &emulator,
        "parity-untrusted",
        &trust_policy("ec2.amazonaws.com"),
    )
    .await;
    let trusted = given_role(
        &emulator,
        "parity-trusted",
        &trust_policy("codedeploy.amazonaws.com"),
    )
    .await;
    // The group an update row addresses, created under a role CodeDeploy can assume.
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": "parity",
                "deploymentGroupName": "parity-group",
                "serviceRoleArn": trusted,
            }),
        )
        .await;

    // (case, the operation, the role the request names, the modelled error code)
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "a create naming a value that is no ARN",
            "CreateDeploymentGroup",
            "not-a-role-arn",
            "InvalidRoleException",
        ),
        (
            "a create naming an ARN of the wrong kind",
            "CreateDeploymentGroup",
            "arn:aws:iam::123456789012:user/deployer",
            "InvalidRoleException",
        ),
        (
            "a create naming no role at all",
            "CreateDeploymentGroup",
            "",
            "RoleRequiredException",
        ),
        (
            "a create naming a role CodeDeploy cannot assume",
            "CreateDeploymentGroup",
            untrusted.as_str(),
            "InvalidRoleException",
        ),
        (
            "an update naming a value that is no ARN",
            "UpdateDeploymentGroup",
            "not-a-role-arn",
            "InvalidRoleException",
        ),
        (
            // A group always HAS a service role, so a blank replacement is an
            // invalid value rather than a member left unset.
            "an update naming a blank replacement",
            "UpdateDeploymentGroup",
            "   ",
            "InvalidRoleException",
        ),
        (
            "an update naming a role CodeDeploy cannot assume",
            "UpdateDeploymentGroup",
            untrusted.as_str(),
            "InvalidRoleException",
        ),
    ];

    for (index, (case, operation, role, expected_code)) in cases.iter().enumerate() {
        let mut answers = Vec::new();
        for (path, filters) in [
            ("the handler", Value::Null),
            ("ahead of the walk", off_enum_filters()),
        ] {
            let mut request = if *operation == "CreateDeploymentGroup" {
                json!({
                    "applicationName": "parity",
                    "deploymentGroupName": format!("parity-new-{index}"),
                    "serviceRoleArn": role,
                })
            } else {
                json!({
                    "applicationName": "parity",
                    "currentDeploymentGroupName": "parity-group",
                    "serviceRoleArn": role,
                })
            };
            if !filters.is_null() {
                request["ec2TagFilters"] = filters;
            }
            let (status, body) = emulator.call(operation, request).await;
            // Each path answers the MODELLED refusal, so neither can pass by
            // agreeing with the other on a wrong answer.
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, *expected_code),
                "case {case:?} through {path}: expected 400 {expected_code}, got {status} \
                 with {body:?}"
            );
            answers.push((path, status, body));
        }
        let (first_path, first_status, first_body) = &answers[0];
        let (second_path, second_status, second_body) = &answers[1];
        assert_eq!(
            (first_status, first_body),
            (second_status, second_body),
            "case {case:?}: {first_path} answered {first_status} {first_body:?} and \
             {second_path} answered {second_status} {second_body:?}"
        );
    }
}
