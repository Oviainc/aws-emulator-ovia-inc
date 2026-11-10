// SPDX-License-Identifier: CC0-1.0

//! The PERMISSIONS a stack operation runs under, end to end over the awsQuery wire.
//!
//! The rule under test: **a stack's resource changes are carried out with the credentials of
//! the stack's service role — "CloudFormation uses the role's credentials to make calls on
//! your behalf" — so a change whose API call that role's identity policies DENY is not made:
//! the resource takes the failed status with the owning service's own unauthorized refusal,
//! and the stack takes the failed terminal status of the operation.** A rollback's calls are
//! that same stack's work, so a rollback the role is denied leaves the stack in
//! `UPDATE_ROLLBACK_FAILED` — the state `ContinueUpdateRollback` resumes from, and
//! `ResourcesToSkip` steps over by leaving the resource exactly as the failed update made
//! it.
//!
//! The table's rows differ along what the rule generalises over: how the denied ACTION is
//! spelled (named in full, a wildcard), what the deny's `Resource` element names (every
//! resource, the resource's own ARN), which WIRE the owning service speaks (AWS Systems
//! Manager on awsJson, Amazon SNS on awsQuery) and which operation of the deployment is
//! denied (a create, the revert of an update). Two rows are controls: a role denied a
//! DIFFERENT action of the same service deploys the stack, and so does one carrying only an
//! allow — so no row passes against a gate that refuses every call.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, status, SERVICE_PRINCIPAL,
};

use crate::stack_lifecycle::{
    call, create_stack, iam, member, router, sns, wait_for_the_transition, xml_text,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, resource_status_of, stack_events, status_of, template,
    template_with_an_unsignalled_gate, WAITED_OUT,
};

/// The trust policy a stack's service role must carry: CloudFormation has to be able to
/// assume it at all before its permissions decide anything
/// (`state::role_trust_refusal`).
pub(crate) fn trusts_cloudformation() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// A role trusting CloudFormation and carrying `statements` as its one inline permissions
/// policy, answered with the ARN IAM persisted for it.
pub(crate) async fn service_role(
    router: &MultiServiceRouter,
    name: &str,
    statements: Value,
) -> String {
    let (http_status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trusts_cloudformation()),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "role {name:?}: expected CreateRole to succeed, got {http_status} {created}"
    );
    let arn = xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("role {name:?}: CreateRole reported no Arn: {created}"));
    put_role_policy(router, name, statements).await;
    arn
}

/// Install (or replace) the inline permissions policy of `name`.
pub(crate) async fn put_role_policy(
    router: &MultiServiceRouter,
    name: &str,
    statements: Value,
) {
    let document = json!({"Version": "2012-10-17", "Statement": statements}).to_string();
    let (http_status, body) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "deployment"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "role {name:?}: expected PutRolePolicy to succeed, got {http_status} {body}"
    );
}

/// Everything the role is allowed, which every row grants: the deny rows are about what one
/// statement takes back out of it.
pub(crate) fn allows_everything() -> Value {
    json!([{"Effect": "Allow", "Action": "*", "Resource": "*"}])
}

/// Whether Amazon SNS holds a topic under `name` in the Region these stacks deploy in.
async fn topic_exists(router: &MultiServiceRouter, name: &str) -> bool {
    let (http_status, body) = sns(router, "ListTopics", &[]).await;
    assert_eq!(
        http_status, 200,
        "expected ListTopics to succeed, got {http_status} {body}"
    );
    body.contains(&format!(":{name}<"))
}

/// What one row deploys and what the role's policy says about it.
struct Row {
    case: &'static str,
    /// The stack and role name, which also names the resource the template declares.
    stack: &'static str,
    /// The statements on the role beside [`allows_everything`].
    denies: Value,
    /// The template the stack is created from.
    resources: Value,
    /// The logical id whose status the row asserts.
    logical_id: &'static str,
    /// The status the resource must reach: `CREATE_FAILED` for a denied call,
    /// `CREATE_COMPLETE` where nothing denies it.
    resource_settles_to: &'static str,
}

/// The parameter name the Systems Manager rows deploy, per stack.
fn parameter_name(stack: &str) -> String {
    format!("/{stack}/p")
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the create's own action, denied for every resource",
            stack: "role-deny-named",
            denies: json!([{
                "Effect": "Deny",
                "Action": "ssm:PutParameter",
                "Resource": "*",
            }]),
            resources: json!({"Param": parameter(&parameter_name("role-deny-named"), "v")}),
            logical_id: "Param",
            resource_settles_to: resource_status::CREATE_FAILED,
        },
        Row {
            case: "a wildcard action covering the create's call",
            stack: "role-deny-wildcard",
            denies: json!([{
                "Effect": "Deny",
                "Action": "ssm:Put*",
                "Resource": "*",
            }]),
            resources: json!({"Param": parameter(&parameter_name("role-deny-wildcard"), "v")}),
            logical_id: "Param",
            resource_settles_to: resource_status::CREATE_FAILED,
        },
        Row {
            case: "denied on the resource's own ARN rather than on every resource",
            stack: "role-deny-arn",
            denies: json!([{
                "Effect": "Deny",
                "Action": "ssm:*",
                "Resource": "arn:aws:ssm:*:*:parameter*",
            }]),
            resources: json!({"Param": parameter(&parameter_name("role-deny-arn"), "v")}),
            logical_id: "Param",
            resource_settles_to: resource_status::CREATE_FAILED,
        },
        // Amazon SNS speaks awsQuery, where the operation is the request's `Action`
        // parameter rather than an `X-Amz-Target` header: the rule is the same either way.
        Row {
            case: "an awsQuery service's create, denied",
            stack: "role-deny-query",
            denies: json!([{
                "Effect": "Deny",
                "Action": "sns:CreateTopic",
                "Resource": "*",
            }]),
            resources: json!({
                "Topic": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"TopicName": "role-deny-query-topic"},
                },
            }),
            logical_id: "Topic",
            resource_settles_to: resource_status::CREATE_FAILED,
        },
        // A deny scoped to the resource's EXACT ARN, for a type whose physical id is that
        // ARN ("Ref returns the topic ARN"): what the statement names is what the call is
        // authorized against.
        Row {
            case: "denied on one resource's exact ARN",
            stack: "role-deny-exact",
            denies: json!([{
                "Effect": "Deny",
                "Action": "sns:CreateTopic",
                "Resource": "arn:aws:sns:us-east-1:000000000000:role-deny-exact-topic",
            }]),
            resources: json!({
                "Topic": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": {"TopicName": "role-deny-exact-topic"},
                },
            }),
            logical_id: "Topic",
            resource_settles_to: resource_status::CREATE_FAILED,
        },
        // A CONTROL: the role is denied another action of the same service, which this
        // deployment does not make, so the stack deploys.
        Row {
            case: "another action of the same service, denied",
            stack: "role-deny-other",
            denies: json!([{
                "Effect": "Deny",
                "Action": "ssm:DeleteParameter",
                "Resource": "*",
            }]),
            resources: json!({"Param": parameter(&parameter_name("role-deny-other"), "v")}),
            logical_id: "Param",
            resource_settles_to: resource_status::CREATE_COMPLETE,
        },
        // A CONTROL: nothing is denied at all.
        Row {
            case: "a role that denies nothing",
            stack: "role-deny-none",
            denies: json!([]),
            resources: json!({"Param": parameter(&parameter_name("role-deny-none"), "v")}),
            logical_id: "Param",
            resource_settles_to: resource_status::CREATE_COMPLETE,
        },
    ]
}

#[tokio::test]
async fn a_create_whose_call_the_service_role_denies_fails_that_resource() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let mut statements: Vec<Value> = match allows_everything() {
            Value::Array(items) => items,
            other => panic!("case {case:?}: the allow statements are a list, got {other:?}"),
        };
        match row.denies {
            Value::Array(items) => statements.extend(items),
            other => panic!("case {case:?}: the deny statements are a list, got {other:?}"),
        }
        let role_arn = service_role(&router, row.stack, Value::Array(statements)).await;

        let body = template(row.resources.clone());
        // `DisableRollback` keeps the failed create exactly as it failed, so the row reads
        // the status the denied call left rather than the rollback that would follow it.
        let (http_status, response) = create_stack(
            &router,
            row.stack,
            &body,
            &[
                member("RoleARN", &role_arn),
                member("DisableRollback", "true"),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {response}"
        );
        wait_for_the_transition().await;

        let failed = row.resource_settles_to == resource_status::CREATE_FAILED;
        let settled = status_of(&router, row.stack).await;
        let expected_stack = if failed {
            status::CREATE_FAILED
        } else {
            status::CREATE_COMPLETE
        };
        assert_eq!(
            settled, expected_stack,
            "case {case:?}: expected the stack to settle at {expected_stack}, got {settled:?}"
        );
        let reported = resource_status_of(&router, row.stack, row.logical_id).await;
        if !failed {
            assert_eq!(
                reported, row.resource_settles_to,
                "case {case:?}: expected {} to be reported {}, got {reported:?}",
                row.logical_id, row.resource_settles_to
            );
        }
        let events = stack_events(case, &router, row.stack).await;
        assert!(
            events
                .iter()
                .any(|(logical_id, status)| logical_id == row.logical_id
                    && status == row.resource_settles_to),
            "case {case:?}: expected ({}, {}) among the events, got {events:?}",
            row.logical_id,
            row.resource_settles_to
        );

        // What the owning service really holds, which is the point of the rule: a call that
        // was not made created nothing.
        let held = match row.logical_id {
            "Topic" => topic_exists(&router, &format!("{}-topic", row.stack)).await,
            _ => parameter_value(&router, &parameter_name(row.stack))
                .await
                .is_some(),
        };
        assert_eq!(
            held, !failed,
            "case {case:?}: expected the owning service to hold the resource = {}, got {held}",
            !failed
        );

        if failed {
            // The reason a client reads is the refusal itself: the principal the call was
            // made as, the action it asked for and why it was refused.
            let (_, described) = call(
                &router,
                "DescribeStackEvents",
                &[member("StackName", row.stack)],
            )
            .await;
            assert!(
                described.contains("is not authorized to perform")
                    && described.contains("explicit deny"),
                "case {case:?}: expected the unauthorized refusal in the event stream, got \
                 {described}"
            );
        }
    }
}

/// The recovery chain a denied ROLLBACK drives: the documented cause of
/// `UPDATE_ROLLBACK_FAILED` reached through the stack's own service role, and
/// `ContinueUpdateRollback` getting the stack out of it.
#[tokio::test]
async fn a_rollback_the_service_role_is_denied_parks_the_stack_until_the_resource_is_skipped() {
    let router = router();
    let stack = "role-deny-rollback";
    let name = parameter_name(stack);
    let role_arn = service_role(&router, stack, allows_everything()).await;

    let (http_status, body) = create_stack(
        &router,
        stack,
        &template(json!({"Param": parameter(&name, "before")})),
        &[member("RoleARN", &role_arn)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let created = status_of(&router, stack).await;
    assert_eq!(
        created,
        status::CREATE_COMPLETE,
        "expected the create to complete under a role that allows it, got {created:?}"
    );

    // The update changes the parameter and is then held at a wait condition nothing
    // signals, so it fails on the clock with no call of the client's.
    let updated = template_with_an_unsignalled_gate(json!({"Param": parameter(&name, "after")}));
    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[member("StackName", stack), member("TemplateBody", &updated)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected UpdateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let applied = parameter_value(&router, &name).await;
    assert_eq!(
        applied.as_deref(),
        Some("after"),
        "expected the update to have applied its change before the wait ran out, got \
         {applied:?}"
    );

    // The role is now DENIED the call that would put the parameter back, so the rollback
    // the failed update owes cannot be carried out.
    put_role_policy(
        &router,
        stack,
        json!([
            {"Effect": "Allow", "Action": "*", "Resource": "*"},
            {"Effect": "Deny", "Action": "ssm:PutParameter", "Resource": "*"},
        ]),
    )
    .await;

    tokio::time::sleep(WAITED_OUT).await;
    let parked = status_of(&router, stack).await;
    assert_eq!(
        parked,
        status::UPDATE_ROLLBACK_FAILED,
        "expected a rollback the service role is denied to park the stack in \
         UPDATE_ROLLBACK_FAILED, got {parked:?}"
    );
    let refused = resource_status_of(&router, stack, "Param").await;
    assert_eq!(
        refused,
        resource_status::UPDATE_FAILED,
        "expected the resource the rollback could not put back to be reported UPDATE_FAILED, \
         got {refused:?}"
    );
    let kept = parameter_value(&router, &name).await;
    assert_eq!(
        kept.as_deref(),
        Some("after"),
        "expected the resource to be left as the failed update made it, got {kept:?}"
    );

    // Resumed with nothing skipped, the same denial stands.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected ContinueUpdateRollback to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let again = status_of(&router, stack).await;
    assert_eq!(
        again,
        status::UPDATE_ROLLBACK_FAILED,
        "expected a resumed rollback the role is still denied to re-enter \
         UPDATE_ROLLBACK_FAILED, got {again:?}"
    );

    // Resumed with that resource SKIPPED, no call about it is made at all: the rollback
    // completes and the resource stays exactly as the failed update made it.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", stack),
            member("ResourcesToSkip.member.1", "Param"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected the skipping ContinueUpdateRollback to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let completed = status_of(&router, stack).await;
    assert_eq!(
        completed,
        status::UPDATE_ROLLBACK_COMPLETE,
        "expected the skipping rollback to complete, got {completed:?}"
    );
    let skipped = parameter_value(&router, &name).await;
    assert_eq!(
        skipped.as_deref(),
        Some("after"),
        "expected the skipped resource to be left as the failed update made it, got {skipped:?}"
    );
}
