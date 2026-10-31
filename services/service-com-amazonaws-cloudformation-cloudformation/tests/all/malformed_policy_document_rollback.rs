// SPDX-License-Identifier: CC0-1.0

//! A stack whose `AWS::IAM::ManagedPolicy` carries a policy document IAM REFUSES, end to
//! end over the awsQuery wire.
//!
//! RULE: a resource the service that owns it refuses to create fails that resource with
//! that service's own refusal as the reason — reported as the SDK of that service raises
//! it, so the sentence IAM worded is followed by the diagnostic of the call (the service,
//! the HTTP status of the answer) and closed by the `HandlerErrorCode` class the refusal
//! falls in — and the stack is then rolled back: the resources the create had already
//! made are removed from the services that own them and the stack settles on
//! `ROLLBACK_COMPLETE`, which is what `OnFailure`'s documented default says.
//!
//! The rows differ along what the rule generalises over: which element of the managed
//! policy's statement is missing (its actions, its resources), which statement of the
//! document is the incomplete one, and — the control row — a document IAM accepts, where
//! the very same template completes and both resources stay. Every expected value is
//! computed from what the row declares.
//!
//! The router is the one `stack_lifecycle.rs` builds, which mounts IAM beside
//! CloudFormation on one engine exactly as the production router does, so the policy the
//! stack declares is created by calling IAM itself.

use serde_json::{json, Value};

use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, status,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, iam, member, resource_event_reason,
    router, wait_for_the_transition, xml_blocks, xml_text, xml_texts, ACCOUNT_ID,
};

/// The logical id of the role the template declares first.
const ROLE: &str = "RoleA";

/// The logical id of the managed policy that depends on it.
const POLICY: &str = "BadPolicy";

/// A trust policy IAM accepts for the role: the role is not what is under test here.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root")},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// The template: a role with a pinned `RoleName`, and a managed policy that `DependsOn`
/// it and is attached to it — so the role is created first and is what the rollback has to
/// remove when the policy fails.
fn template(role_name: &str, policy_name: &str, path: &str, document: Value) -> String {
    json!({
        "Resources": {
            ROLE: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role_name,
                    "AssumeRolePolicyDocument": trust_policy()
                }
            },
            POLICY: {
                "Type": "AWS::IAM::ManagedPolicy",
                "DependsOn": ROLE,
                "Properties": {
                    "ManagedPolicyName": policy_name,
                    "Path": path,
                    "Roles": [{"Ref": ROLE}],
                    "PolicyDocument": document
                }
            }
        }
    })
    .to_string()
}

/// Every `(LogicalResourceId, ResourceStatus)` pair the stack's event stream carries.
async fn resource_events(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter_map(|entry| {
            Some((
                xml_text(entry, "LogicalResourceId")?,
                xml_text(entry, "ResourceStatus")?,
            ))
        })
        .collect()
}

/// `(HTTP status, error code, message)` of an `iam:GetRole` for `role_name`.
async fn get_role(router: &MultiServiceRouter, role_name: &str) -> (u16, String, String) {
    let (code, body) = iam(router, "GetRole", &[("RoleName", role_name)]).await;
    (
        code,
        xml_text(&body, "Code").unwrap_or_default(),
        xml_text(&body, "Message").unwrap_or_default(),
    )
}

/// Whether IAM holds a customer managed policy at `policy_arn`, asked the way a client
/// checks what a template's `Path` holds: the policies listed under that path prefix.
async fn policy_is_listed(router: &MultiServiceRouter, path: &str, policy_arn: &str) -> bool {
    let (code, body) = iam(
        router,
        "ListPolicies",
        &[("PathPrefix", path), ("Scope", "Local")],
    )
    .await;
    assert_eq!(
        code, 200,
        "ListPolicies({path}) must answer 200, got {code} {body}"
    );
    xml_texts(&body, "Arn").iter().any(|arn| arn == policy_arn)
}

/// One row: the policy document the template declares, and what IAM must be seen to say
/// about it.
struct Row {
    case: &'static str,
    stack: &'static str,
    document: fn() -> Value,
    /// The sentence IAM words its refusal with, or `None` for a document IAM accepts.
    refusal: Option<&'static str>,
}

/// A document whose one statement names an action but no resource — the shape the
/// recorded template carries.
fn no_resource() -> Value {
    json!({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": "s3:GetObject"}]})
}

/// A document whose one statement names a resource but no action.
fn no_action() -> Value {
    json!({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Resource": "*"}]})
}

/// A document whose SECOND statement is the incomplete one.
fn second_statement_incomplete() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "s3:GetObject", "Resource": "*"},
            {"Effect": "Deny", "Action": "s3:PutObject"}
        ]
    })
}

/// A document IAM accepts: every statement names both.
fn complete() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": "s3:GetObject", "Resource": "*"}]
    })
}

#[tokio::test]
async fn a_managed_policy_iam_refuses_fails_the_resource_and_rolls_the_stack_back() {
    let rows = [
        Row {
            case: "a statement with an action and no resource",
            stack: "policy-no-resource",
            document: no_resource,
            refusal: Some("Policy statement must contain resources."),
        },
        Row {
            case: "a statement with a resource and no action",
            stack: "policy-no-action",
            document: no_action,
            refusal: Some("Policy statement must contain actions."),
        },
        Row {
            case: "the second statement of two is incomplete",
            stack: "policy-second-statement",
            document: second_statement_incomplete,
            refusal: Some("Policy statement must contain resources."),
        },
        Row {
            case: "a document IAM accepts",
            stack: "policy-complete",
            document: complete,
            refusal: None,
        },
    ];

    for row in rows {
        // One router per row, so what a row leaves in IAM cannot reach the next one.
        let router = router();
        let role_name = format!("{}-role", row.stack);
        let policy_name = format!("{}-policy", row.stack);
        let path = format!("/{}/", row.stack);
        let policy_arn = format!("arn:aws:iam::{ACCOUNT_ID}:policy{path}{policy_name}");

        let (code, created) = create_stack(
            &router,
            row.stack,
            &template(&role_name, &policy_name, &path, (row.document)()),
            &acknowledging_iam(&[]),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200 — the creation starts and the stack \
             reports the outcome — got {code} {created}",
            row.case
        );
        wait_for_the_transition().await;

        let (code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {described}",
            row.case
        );
        let Some(sentence) = row.refusal else {
            // The control row: the very same template, with a document IAM takes, leaves
            // both resources in place.
            assert_eq!(
                xml_text(&described, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {:?}: expected the create to complete, got {described}",
                row.case
            );
            let (code, error, message) = get_role(&router, &role_name).await;
            assert_eq!(
                code, 200,
                "case {:?}: the role the create made must still be there, got {code} \
                 {error} {message}",
                row.case
            );
            assert!(
                policy_is_listed(&router, &path, &policy_arn).await,
                "case {:?}: the policy the create made must be listed under {path:?}",
                row.case
            );
            continue;
        };

        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {:?}: expected the stack to roll back, got {described}",
            row.case
        );
        // The STACK's reason names which resource failed; IAM's own refusal is reported on
        // that resource's event.
        assert_eq!(
            xml_text(&described, "StackStatusReason").unwrap_or_default(),
            resource_failure_summary(failed_action::CREATE, [POLICY]),
            "case {:?}: expected the stack reason to summarise what failed, got {described}",
            row.case
        );

        let reason =
            resource_event_reason(&router, row.stack, POLICY, resource_status::CREATE_FAILED)
                .await;
        for needle in [
            sentence,
            "Service: Iam",
            "Status Code: 400",
            "HandlerErrorCode: InvalidRequest",
        ] {
            assert!(
                reason.contains(needle),
                "case {:?}: expected the failed resource's reason to carry {needle:?}, got \
                 {reason:?}",
                row.case
            );
        }

        // The role was created, the policy failed, and the rollback disposed of the role.
        let events = resource_events(&router, row.stack).await;
        for expected in [
            (ROLE, resource_status::CREATE_COMPLETE),
            (POLICY, resource_status::CREATE_FAILED),
            (ROLE, resource_status::DELETE_COMPLETE),
        ] {
            assert!(
                events
                    .iter()
                    .any(|(logical, event)| logical == expected.0 && event == expected.1),
                "case {:?}: expected the event stream to carry {expected:?}, got {events:?}",
                row.case
            );
        }

        // IAM no longer holds what the rolled-back create had made, and never held the
        // policy it refused.
        let (code, error, message) = get_role(&router, &role_name).await;
        assert_eq!(
            code, 404,
            "case {:?}: the rollback must take the role away, got {code} {error} {message}",
            row.case
        );
        assert_eq!(
            error, "NoSuchEntity",
            "case {:?}: expected IAM's own absence code, got {error:?} {message:?}",
            row.case
        );
        assert_eq!(
            message,
            format!("The role with name {role_name} cannot be found."),
            "case {:?}: expected IAM's own absence wording, got {message:?}",
            row.case
        );
        assert!(
            !policy_is_listed(&router, &path, &policy_arn).await,
            "case {:?}: no policy may be listed under {path:?} after the rollback",
            row.case
        );
    }
}
