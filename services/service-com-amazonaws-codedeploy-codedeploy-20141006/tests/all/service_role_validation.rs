// SPDX-License-Identifier: CC0-1.0

//! The SERVICE ROLE a deployment group is created and updated with, driven over the
//! wire.
//!
//! RULE under test: the `serviceRoleArn` a deployment group runs as must be an IAM
//! role ARN (`arn:<partition>:iam::<account>:role/<name>`) naming a role that lets
//! CodeDeploy assume it, and a request that names anything else is refused with the
//! modelled `InvalidRoleException` (HTTP 400) — the model declares that error for
//! both operations that SET the role and no deployment group is stored. CodeDeploy
//! judges the role BEFORE it judges the request's other member values, and AFTER the
//! resource the request is addressed to has been resolved, so a request that earns
//! two refusals is answered in that order.
//!
//! The role itself belongs to IAM, so every row that needs one creates it through
//! IAM's own `CreateRole` on the same engine ([`Emulator::with_iam`]) rather than
//! writing a record CodeDeploy would then read back.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, given_role, trust_policy, Emulator};

/// The service principal a role has to trust for CodeDeploy to act as it.
const SERVICE_PRINCIPAL: &str = "codedeploy.amazonaws.com";

/// Create an application, answering its name.
async fn given_application(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": name, "computePlatform": "Server"}),
        )
        .await;
    name.to_string()
}

/// A `CreateDeploymentGroup` request for `application` naming `role`, plus `extra`.
fn create_request(application: &str, group: &str, role: &str, extra: &Value) -> Value {
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": role,
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        request[member] = value.clone();
    }
    request
}

/// RULE: a `serviceRoleArn` that is not an IAM role ARN is refused, and no
/// deployment group is created for it.
///
/// Rows differ along what the rule generalises over: the FORM the value takes — a
/// bare word, a role name without the ARN prefix, an ARN of another service, an ARN
/// naming another IAM entity type, an ARN missing fields, one whose role name is
/// empty — and the two ways an absent value arrives (empty and blank). The ARN
/// grammar the rows span is the six-field
/// `arn:<partition>:<service>:<region>:<account>:<resource>` form the IAM User
/// Guide gives for a role (`arn:aws:iam::<account>:role/<name>`).
#[tokio::test]
async fn a_service_role_that_is_no_role_arn_is_refused() {
    let emulator = Emulator::new();
    let application = given_application(&emulator, "role-forms").await;

    // (case, the value sent, the modelled error code, the sentence it carries)
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "not an ARN at all",
            "not-a-role-arn",
            "InvalidRoleException",
            "ARNs must start with 'arn:': not-a-role-arn",
        ),
        (
            "a role name without the ARN prefix",
            "role/CodeDeployRole",
            "InvalidRoleException",
            "ARNs must start with 'arn:': role/CodeDeployRole",
        ),
        (
            "the ARN of another service's resource",
            "arn:aws:s3:::my-bucket",
            "InvalidRoleException",
            "The service role ARN arn:aws:s3:::my-bucket was specified in an invalid format",
        ),
        (
            "an IAM ARN naming a user rather than a role",
            "arn:aws:iam::123456789012:user/deployer",
            "InvalidRoleException",
            "The service role ARN arn:aws:iam::123456789012:user/deployer was specified in an \
             invalid format",
        ),
        (
            "an IAM ARN naming an instance profile",
            "arn:aws:iam::123456789012:instance-profile/deployer",
            "InvalidRoleException",
            "The service role ARN arn:aws:iam::123456789012:instance-profile/deployer was \
             specified in an invalid format",
        ),
        (
            "an ARN with too few fields",
            "arn:aws:iam::123456789012",
            "InvalidRoleException",
            "The service role ARN arn:aws:iam::123456789012 was specified in an invalid format",
        ),
        (
            "an ARN whose role name is empty",
            "arn:aws:iam::123456789012:role/",
            "InvalidRoleException",
            "The service role ARN arn:aws:iam::123456789012:role/ was specified in an invalid \
             format",
        ),
        (
            "an ARN with no partition",
            "arn::iam::123456789012:role/CodeDeployRole",
            "InvalidRoleException",
            "The service role ARN arn::iam::123456789012:role/CodeDeployRole was specified in \
             an invalid format",
        ),
        (
            // A role the request did not give at all: `serviceRoleArn` is
            // `@required` on this operation, so it is that member's
            // `RoleRequiredException` rather than an invalid value.
            "an empty value",
            "",
            "RoleRequiredException",
            "Service role ARN is missing",
        ),
        (
            "a blank value",
            "   ",
            "RoleRequiredException",
            "Service role ARN is missing",
        ),
    ];

    for (index, (case, role, expected_code, expected_message)) in cases.iter().enumerate() {
        let group = format!("group-{index}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(&application, &group, role, &json!({})),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} with {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(*expected_message),
            "case {case:?}: expected the {expected_code} sentence, got {body:?}"
        );
        // Nothing was stored for a request the service refused.
        let (read_status, read_body) = emulator
            .call(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_eq!(
            (read_status, error_code(&read_body).as_str()),
            (400, "DeploymentGroupDoesNotExistException"),
            "case {case:?}: no group may exist after the refusal, got {read_status} with \
             {read_body:?}"
        );
    }
}

/// RULE: a well-formed role ARN naming a role the account holds whose trust policy
/// does not let CodeDeploy assume it is refused with `InvalidRoleException`, and one
/// that DOES trust CodeDeploy is accepted — the verdict comes from the role's own
/// trust document in IAM, not from the request.
///
/// Rows differ along what the rule generalises over: the principal the trust policy
/// names, the effect it states, the action it grants, and a role IAM holds no trust
/// document for at all.
#[tokio::test]
async fn a_service_role_is_judged_by_its_trust_policy() {
    let emulator = Emulator::with_iam();
    let application = given_application(&emulator, "role-trust").await;

    // (case, the role's trust document, whether the create is accepted)
    let cases: &[(&str, String, bool)] = &[
        (
            "a role trusting CodeDeploy",
            trust_policy(SERVICE_PRINCIPAL),
            true,
        ),
        (
            "a role trusting another service",
            trust_policy("ec2.amazonaws.com"),
            false,
        ),
        (
            "a role trusting CodeDeploy for another action",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "s3:GetObject",
                }],
            })
            .to_string(),
            false,
        ),
        (
            "a role explicitly denying CodeDeploy",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "sts:AssumeRole",
                }],
            })
            .to_string(),
            false,
        ),
        (
            "a role trusting CodeDeploy among several principals",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": ["ec2.amazonaws.com", SERVICE_PRINCIPAL]},
                    "Action": ["sts:TagSession", "sts:AssumeRole"],
                }],
            })
            .to_string(),
            true,
        ),
    ];

    for (index, (case, trust, accepted)) in cases.iter().enumerate() {
        let role = given_role(&emulator, &format!("trust-role-{index}"), trust).await;
        let group = format!("trust-group-{index}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(&application, &group, &role, &json!({})),
            )
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the group to be created, got {status} with {body:?}"
            );
            let read = emulator
                .ok(
                    "GetDeploymentGroup",
                    json!({"applicationName": application, "deploymentGroupName": group}),
                )
                .await;
            assert_eq!(
                read["deploymentGroupInfo"]["serviceRoleArn"].as_str(),
                Some(role.as_str()),
                "case {case:?}: expected the stored role to read back, got {read:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRoleException"),
            "case {case:?}: expected 400 InvalidRoleException, got {status} with {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!(
                    "AWS CodeDeploy does not have the permissions required to assume the role \
                     {role}."
                )
                .as_str()
            ),
            "case {case:?}: expected the sentence naming the role, got {body:?}"
        );
    }
}

/// RULE: a role ARN this emulator's IAM holds no role for is no evidence of a
/// missing trust — the role may belong to another account — so the create goes
/// ahead and the group reads back under it.
#[tokio::test]
async fn a_role_the_account_does_not_hold_is_not_refused() {
    let emulator = Emulator::with_iam();
    let application = given_application(&emulator, "role-elsewhere").await;
    // Rows differ along WHOSE role the ARN names: another account's, and one this
    // account simply never told the emulator about.
    for (case, role) in [
        (
            "another account's role",
            "arn:aws:iam::210987654321:role/CodeDeployRole",
        ),
        (
            "a role of this account IAM holds nothing for",
            "arn:aws:iam::000000000000:role/NeverCreated",
        ),
    ] {
        let group = format!("elsewhere-{}", case.len());
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(&application, &group, role, &json!({})),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group to be created, got {status} with {body:?}"
        );
        let read = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_eq!(
            read["deploymentGroupInfo"]["serviceRoleArn"].as_str(),
            Some(role),
            "case {case:?}: expected the stored role to read back, got {read:?}"
        );
    }
}

/// RULE: the role is judged BEFORE the request's other member values, and AFTER the
/// application the group is created under has been resolved.
///
/// Rows differ along which refusals one request earns at once: a member value
/// outside its modelled enum beside each of the role refusals, the same member
/// beside a role the service accepts (where the member refusal stands), and an
/// unknown application beside an untrusted role.
#[tokio::test]
async fn the_role_is_judged_before_the_other_members_and_after_the_application() {
    let emulator = Emulator::with_iam();
    let application = given_application(&emulator, "role-order").await;
    let untrusted = given_role(
        &emulator,
        "order-untrusted",
        &trust_policy("ec2.amazonaws.com"),
    )
    .await;
    let trusted = given_role(
        &emulator,
        "order-trusted",
        &trust_policy(SERVICE_PRINCIPAL),
    )
    .await;

    // A tag filter whose `type` is outside the modelled `EC2TagFilterType`
    // (`smithy_query(query="shape",shape="EC2TagFilterType")`: KEY_ONLY |
    // VALUE_ONLY | KEY_AND_VALUE), which the boundary validator refuses before any
    // handler runs.
    let off_enum_filters = json!([
        {"Key": "k", "Value": "v", "Type": "KEY_AND_VALUE"},
        {"Key": "k2", "Value": "v2", "Type": "BOGUS_TYPE"},
    ]);

    // (case, the application named, the role named, the extra members, the expected
    // error code)
    let cases: &[(&str, &str, &str, Value, &str)] = &[
        (
            "an off-enum member beside an untrusted role",
            &application,
            &untrusted,
            json!({"ec2TagFilters": off_enum_filters}),
            "InvalidRoleException",
        ),
        (
            "an off-enum member beside a role that is no ARN",
            &application,
            "not-a-role-arn",
            json!({"ec2TagFilters": off_enum_filters}),
            "InvalidRoleException",
        ),
        (
            "an off-enum member beside no role at all",
            &application,
            "",
            json!({"ec2TagFilters": off_enum_filters}),
            "RoleRequiredException",
        ),
        (
            // The role is one the service accepts, so the member refusal is the
            // only one left and it stands exactly as the framework words it.
            "an off-enum member beside a role CodeDeploy can assume",
            &application,
            &trusted,
            json!({"ec2TagFilters": off_enum_filters}),
            "ValidationException",
        ),
        (
            // An application the account does not hold is answered first, whether
            // or not the role would also be refused.
            "an unknown application beside an untrusted role",
            "no-such-application",
            &untrusted,
            json!({}),
            "ApplicationDoesNotExistException",
        ),
    ];

    for (index, (case, named_application, role, extra, expected_code)) in
        cases.iter().enumerate()
    {
        let group = format!("order-group-{index}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                create_request(named_application, &group, role, extra),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} with {body:?}"
        );
    }
}

/// RULE: `UpdateDeploymentGroup` judges a REPLACEMENT service role by the same two
/// questions, and a group whose update is refused keeps the role it had.
#[tokio::test]
async fn a_replacement_service_role_is_judged_the_same_way() {
    let emulator = Emulator::with_iam();
    let application = given_application(&emulator, "role-update").await;
    let original = given_role(
        &emulator,
        "update-original",
        &trust_policy(SERVICE_PRINCIPAL),
    )
    .await;
    let untrusted = given_role(
        &emulator,
        "update-untrusted",
        &trust_policy("ec2.amazonaws.com"),
    )
    .await;
    let trusted = given_role(
        &emulator,
        "update-trusted",
        &trust_policy(SERVICE_PRINCIPAL),
    )
    .await;
    let group = "update-group";
    emulator
        .ok(
            "CreateDeploymentGroup",
            create_request(&application, group, &original, &json!({})),
        )
        .await;

    // Rows differ along the replacement the update names: one that is no ARN, one
    // naming a role CodeDeploy cannot assume, and a blank one — a group always has
    // a role, so a blank replacement is an invalid one rather than a removal.
    for (case, replacement) in [
        ("no ARN at all", "not-a-role-arn"),
        ("a role CodeDeploy cannot assume", untrusted.as_str()),
        ("a blank replacement", "   "),
    ] {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "serviceRoleArn": replacement,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRoleException"),
            "case {case:?}: expected 400 InvalidRoleException, got {status} with {body:?}"
        );
        let read = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_eq!(
            read["deploymentGroupInfo"]["serviceRoleArn"].as_str(),
            Some(original.as_str()),
            "case {case:?}: the group must keep the role it had, got {read:?}"
        );
    }

    // And a replacement the service accepts is stored and reads back.
    emulator
        .ok(
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "serviceRoleArn": trusted,
            }),
        )
        .await;
    let read = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await;
    assert_eq!(
        read["deploymentGroupInfo"]["serviceRoleArn"].as_str(),
        Some(trusted.as_str()),
        "case the trusted replacement: expected the new role to read back, got {read:?}"
    );
}
