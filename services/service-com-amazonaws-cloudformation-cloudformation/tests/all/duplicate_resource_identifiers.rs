// SPDX-License-Identifier: CC0-1.0

//! Two declarations of one template claiming ONE identifier, end to end over the awsQuery
//! wire.
//!
//! Two rules are under test.
//!
//! * A resource's physical name is the EVALUATED value of the property its type takes its
//!   name from, including when that value is built from ANOTHER resource of the same
//!   template: CloudFormation creates a resource only once the resources it references
//!   exist, so a `Ref` to one of them resolves to the identifier that resource carries, and
//!   only a declaration that names the resource nowhere gets CloudFormation's generated
//!   `<stack>-<logical id>-<unique id>` name.
//! * So a template whose two declarations resolve to ONE identifier asks the owning service
//!   to create the second resource under a name it already holds, and that service's own
//!   uniqueness rule refuses it: the second resource is `CREATE_FAILED` in the stack's event
//!   history while the first stays `CREATE_COMPLETE`, and the stack takes the terminal status
//!   its failed create is owed — rolled back to `ROLLBACK_COMPLETE`, with what it created
//!   disposed of, unless the operation asked for the failed state to be kept.
//!
//! The first table's rows differ along what the rule generalises over: how the second
//! declaration spells the name it claims (a `Ref` to the first resource, an `Fn::Sub` over
//! it, the same literal text), which service owns the type whose uniqueness rule refuses the
//! create, and — as a control — two declarations whose names DIFFER, which both deploy. The
//! second table spans the deployment configurations a failed create settles under.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, iam, member, router, wait_for_the_transition, xml_text,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, stack_events, status_of, template,
};

/// The trust policy the `AWS::IAM::Role` rows declare, so the role is one IAM accepts.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ec2.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// One row: the template to deploy, and what the two resources must reach.
struct Row {
    case: &'static str,
    stack: &'static str,
    resources: Value,
    /// The status the SECOND declaration must reach.
    second_settles_to: &'static str,
    /// The identifier BOTH declarations claim, which is what the first resource must report
    /// as its physical id — and what the second must report too, since it asked for exactly
    /// that resource.
    claimed: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the second names the first with a Ref",
            stack: "dup-ref",
            resources: json!({
                "A": parameter("/dup-ref/p", "resource-a"),
                "B": {
                    "Type": "AWS::SSM::Parameter",
                    "Properties": {
                        "Name": {"Ref": "A"},
                        "Type": "String",
                        "Value": "resource-b",
                    },
                },
            }),
            second_settles_to: resource_status::CREATE_FAILED,
            claimed: "/dup-ref/p",
        },
        Row {
            case: "the second builds the name from the first with an Fn::Sub",
            stack: "dup-sub",
            resources: json!({
                "A": parameter("/dup-sub/p", "resource-a"),
                "B": {
                    "Type": "AWS::SSM::Parameter",
                    "Properties": {
                        "Name": {"Fn::Sub": "${A}"},
                        "Type": "String",
                        "Value": "resource-b",
                    },
                },
            }),
            second_settles_to: resource_status::CREATE_FAILED,
            claimed: "/dup-sub/p",
        },
        Row {
            case: "both state the same name outright",
            stack: "dup-literal",
            resources: json!({
                "A": parameter("/dup-literal/p", "resource-a"),
                "B": parameter("/dup-literal/p", "resource-b"),
            }),
            second_settles_to: resource_status::CREATE_FAILED,
            claimed: "/dup-literal/p",
        },
        // Another owning service, with its own uniqueness rule for the type's name
        // (`EntityAlreadyExists`): the rule is the owning service's to state, not this
        // crate's.
        Row {
            case: "two roles claiming one role name",
            stack: "dup-role",
            resources: json!({
                "A": {
                    "Type": "AWS::IAM::Role",
                    "Properties": {
                        "RoleName": "dup-role-name",
                        "AssumeRolePolicyDocument": trust_policy(),
                    },
                },
                "B": {
                    "Type": "AWS::IAM::Role",
                    "Properties": {
                        "RoleName": {"Ref": "A"},
                        "AssumeRolePolicyDocument": trust_policy(),
                    },
                },
            }),
            second_settles_to: resource_status::CREATE_FAILED,
            claimed: "dup-role-name",
        },
        // A CONTROL: two declarations of the same type whose names DIFFER both deploy, so no
        // row passes against a deployment that fails every second resource.
        Row {
            case: "two parameters whose names differ",
            stack: "dup-none",
            resources: json!({
                "A": parameter("/dup-none/a", "resource-a"),
                "B": parameter("/dup-none/b", "resource-b"),
            }),
            second_settles_to: resource_status::CREATE_COMPLETE,
            claimed: "/dup-none/a",
        },
    ]
}

/// The `PhysicalResourceId` a stack reports for one of its resources.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (http_status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "DescribeStackResources {stack:?}/{logical_id:?}: expected 200, got {http_status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

#[tokio::test]
async fn a_second_declaration_claiming_one_identifier_fails_in_the_owning_service() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let (http_status, body) = create_stack(
            &router,
            row.stack,
            &template(row.resources.clone()),
            &acknowledging_iam(&[]),
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
        );
        wait_for_the_transition().await;

        let failed = row.second_settles_to == resource_status::CREATE_FAILED;
        let events = stack_events(case, &router, row.stack).await;
        assert!(
            events
                .iter()
                .any(|(logical_id, status)| logical_id == "A"
                    && status == resource_status::CREATE_COMPLETE),
            "case {case:?}: expected the FIRST resource to be created, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|(logical_id, status)| logical_id == "B"
                    && status == row.second_settles_to),
            "case {case:?}: expected (\"B\", {}) among the events, got {events:?}",
            row.second_settles_to
        );

        // The name the second declaration RESOLVED to, which is the root of the collision:
        // the identifier the first resource carries, never a generated one.
        let first = physical_id(&router, row.stack, "A").await;
        assert_eq!(
            first, row.claimed,
            "case {case:?}: expected A to carry the name its declaration states, got {first:?}"
        );
        if failed {
            let second = physical_id(&router, row.stack, "B").await;
            assert_eq!(
                second, row.claimed,
                "case {case:?}: expected B's declaration to resolve to the identifier it \
                 claims, got {second:?}"
            );
        }

        let settled = status_of(&router, row.stack).await;
        // A failed create is rolled back, which is what the default deployment
        // configuration owes it (the second table spans the others).
        let expected = if failed {
            status::ROLLBACK_COMPLETE
        } else {
            status::CREATE_COMPLETE
        };
        assert_eq!(
            settled, expected,
            "case {case:?}: expected the stack to settle at {expected}, got {settled:?}"
        );
    }
}

/// A name built from another resource of the same template is that resource's identifier,
/// whether or not anything collides: the second resource is created under exactly that name
/// in the service that owns it.
#[tokio::test]
async fn a_name_built_from_another_resource_is_that_resources_identifier() {
    let router = router();
    let stack = "name-from-sibling";
    let role_name = "name-from-sibling-role";
    let resources = json!({
        "TheRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": role_name,
                "AssumeRolePolicyDocument": trust_policy(),
            },
        },
        // The parameter's NAME is built from the role of the same template, so it is the
        // role's own identifier that decides it.
        "Named": {
            "Type": "AWS::SSM::Parameter",
            "Properties": {
                "Name": {"Fn::Sub": "/named/${TheRole}"},
                "Type": "String",
                "Value": "built-from-the-role",
            },
        },
    });
    let (http_status, body) = create_stack(
        &router,
        stack,
        &template(resources),
        &acknowledging_iam(&[]),
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let settled = status_of(&router, stack).await;
    assert_eq!(
        settled,
        status::CREATE_COMPLETE,
        "expected the stack to deploy, got {settled:?}"
    );

    let expected_name = format!("/named/{role_name}");
    let reported = physical_id(&router, stack, "Named").await;
    assert_eq!(
        reported, expected_name,
        "expected the parameter's name to be built from the role's identifier, got \
         {reported:?}"
    );
    // ...and that is the name Parameter Store really holds it under.
    let held = parameter_value(&router, &expected_name).await;
    assert_eq!(
        held.as_deref(),
        Some("built-from-the-role"),
        "expected Parameter Store to hold the parameter under {expected_name:?}, got {held:?}"
    );
    let (http_status, role) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        http_status, 200,
        "expected the role the name was built from to exist, got {http_status} {role}"
    );
}

/// One row of the deployment-configuration table: what the create states, and what the
/// collision leaves behind.
struct ModeRow {
    case: &'static str,
    stack: &'static str,
    /// The request members that state the deployment configuration.
    stated: Vec<(String, String)>,
    /// The status the stack settles in.
    settles_to: &'static str,
    /// Whether the parameter the FIRST resource created is still there afterwards.
    keeps_what_it_created: bool,
}

fn mode_rows() -> Vec<ModeRow> {
    vec![
        // "STANDARD … This is the default": a failed create is rolled back, and the rollback
        // disposes of what the create had made.
        ModeRow {
            case: "the default mode",
            stack: "dmode-default",
            stated: Vec::new(),
            settles_to: status::ROLLBACK_COMPLETE,
            keeps_what_it_created: false,
        },
        ModeRow {
            case: "STANDARD, stated",
            stack: "dmode-standard",
            stated: vec![member("DeploymentConfig.Mode", "STANDARD")],
            settles_to: status::ROLLBACK_COMPLETE,
            keeps_what_it_created: false,
        },
        // A failed EXPRESS operation keeps what it provisioned, which is the recorded
        // behaviour of such an operation.
        ModeRow {
            case: "EXPRESS",
            stack: "dmode-express",
            stated: vec![member("DeploymentConfig.Mode", "EXPRESS")],
            settles_to: status::CREATE_FAILED,
            keeps_what_it_created: true,
        },
        // ...unless the request states the flag itself, which says what to do either way.
        ModeRow {
            case: "EXPRESS with DisableRollback stated false",
            stack: "dmode-express-rollback",
            stated: vec![
                member("DeploymentConfig.Mode", "EXPRESS"),
                member("DeploymentConfig.DisableRollback", "false"),
            ],
            settles_to: status::ROLLBACK_COMPLETE,
            keeps_what_it_created: false,
        },
        ModeRow {
            case: "STANDARD with DisableRollback stated true",
            stack: "dmode-standard-kept",
            stated: vec![
                member("DeploymentConfig.Mode", "STANDARD"),
                member("DeploymentConfig.DisableRollback", "true"),
            ],
            settles_to: status::CREATE_FAILED,
            keeps_what_it_created: true,
        },
    ]
}

#[tokio::test]
async fn the_deployment_configuration_decides_what_a_failed_create_leaves_behind() {
    for row in mode_rows() {
        let router = router();
        let case = row.case;
        let name = format!("/{}/p", row.stack);
        let resources = json!({
            "A": parameter(&name, "resource-a"),
            "B": {
                "Type": "AWS::SSM::Parameter",
                "Properties": {
                    "Name": {"Ref": "A"},
                    "Type": "String",
                    "Value": "resource-b",
                },
            },
        });
        let (http_status, body) =
            create_stack(&router, row.stack, &template(resources), &row.stated).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
        );
        wait_for_the_transition().await;

        // Whatever the configuration, the collision is the same: the first resource was
        // created and the second was refused.
        let events = stack_events(case, &router, row.stack).await;
        for (logical_id, expected) in [
            ("A", resource_status::CREATE_COMPLETE),
            ("B", resource_status::CREATE_FAILED),
        ] {
            assert!(
                events
                    .iter()
                    .any(|(id, status)| id == logical_id && status == expected),
                "case {case:?}: expected ({logical_id}, {expected}) among the events, got \
                 {events:?}"
            );
        }

        let settled = status_of(&router, row.stack).await;
        assert_eq!(
            settled, row.settles_to,
            "case {case:?}: expected the stack to settle at {}, got {settled:?}",
            row.settles_to
        );
        let held = parameter_value(&router, &name).await;
        assert_eq!(
            held.is_some(),
            row.keeps_what_it_created,
            "case {case:?}: expected Parameter Store to still hold {name:?} = {}, got \
             {held:?}",
            row.keeps_what_it_created
        );
    }
}
