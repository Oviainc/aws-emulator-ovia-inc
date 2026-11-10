// SPDX-License-Identifier: CC0-1.0

//! The service role a stack KEEPS, end to end over the awsQuery wire.
//!
//! RULE: a stack's service role is replaced only by an operation that NAMES one —
//! `RoleARN`: "CloudFormation uses this role for all future operations on the stack … If you
//! don't specify a value, CloudFormation uses the role that was previously associated with
//! the stack" — so an operation acting on an existing stack without naming a role leaves the
//! role already associated with it in force: `DescribeStacks` keeps reporting it, and the
//! resource calls that operation makes are still made as it.
//!
//! The recorded trace this suite was written for is ONE row of the first table: a stack
//! created by a `CREATE` change set that named a role, then deployed again by a change set
//! that names none. The rows differ along what the rule generalises over — which operation
//! established the role (`CreateStack`, an executed `CREATE` change set, none at all), which
//! operation acts next (an executed `UPDATE` change set, `UpdateStack`) and whether that
//! operation names a role of its own — and the second table reads the role that stays in
//! force through what it is allowed to DO, which is the point of keeping it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::change_set_lifecycle::{create_change_set, execute_change_set, settle_the_store};
use crate::service_role_permissions::{allows_everything, service_role};
use crate::stack_lifecycle::{
    call, create_stack, member, router_with_engine, sns, xml_text,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, resource_status_of, stack_events, status_of, template,
};

/// The `RoleARN` a stack reports right now, or `None` when it reports none.
async fn role_of(router: &MultiServiceRouter, stack: &str) -> Option<String> {
    let (http_status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "DescribeStacks {stack:?}: expected 200, got {http_status} {body}"
    );
    xml_text(&body, "RoleARN")
}

/// Which operation put the stack's service role in force before the one under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Established {
    /// `CreateStack` named it.
    ByCreateStack,
    /// A `CREATE` change set named it and was executed.
    ByACreateChangeSet,
    /// Nothing named one: the stack runs with the caller's own permissions.
    NotAtAll,
}

/// The operation that acts on the stack next, and whether it names a role of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    /// An `UPDATE` change set carrying no `RoleARN`, executed.
    AChangeSetNamingNone,
    /// An `UPDATE` change set carrying another role, executed.
    AChangeSetNamingAnother,
    /// `UpdateStack` carrying no `RoleARN`.
    AnUpdateNamingNone,
    /// `UpdateStack` carrying another role.
    AnUpdateNamingAnother,
}

impl Then {
    /// Whether the operation names a role of its own.
    fn names_another_role(self) -> bool {
        matches!(
            self,
            Then::AChangeSetNamingAnother | Then::AnUpdateNamingAnother
        )
    }
}

/// Which role the stack must report once the second operation has settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InForce {
    /// The role the first operation named.
    TheFirst,
    /// The role the second operation named.
    TheSecond,
    /// None: no operation ever named one.
    None,
}

struct Row {
    case: &'static str,
    /// The stack name, which also names this row's roles and its parameter.
    stack: &'static str,
    established: Established,
    then: Then,
    in_force: InForce,
}

fn rows() -> Vec<Row> {
    vec![
        // The recorded case: a CDK deploy passes the role once, the next deploy passes none.
        Row {
            case: "a CREATE change set's role, through a change set that names none",
            stack: "role-kept-changeset",
            established: Established::ByACreateChangeSet,
            then: Then::AChangeSetNamingNone,
            in_force: InForce::TheFirst,
        },
        Row {
            case: "a CREATE change set's role, replaced by a change set that names another",
            stack: "role-swapped-changeset",
            established: Established::ByACreateChangeSet,
            then: Then::AChangeSetNamingAnother,
            in_force: InForce::TheSecond,
        },
        Row {
            case: "CreateStack's role, through a change set that names none",
            stack: "role-kept-create",
            established: Established::ByCreateStack,
            then: Then::AChangeSetNamingNone,
            in_force: InForce::TheFirst,
        },
        Row {
            case: "CreateStack's role, through an update that names none",
            stack: "role-kept-update",
            established: Established::ByCreateStack,
            then: Then::AnUpdateNamingNone,
            in_force: InForce::TheFirst,
        },
        Row {
            case: "CreateStack's role, replaced by an update that names another",
            stack: "role-swapped-update",
            established: Established::ByCreateStack,
            then: Then::AnUpdateNamingAnother,
            in_force: InForce::TheSecond,
        },
        // A stack that never had a role has none to keep: "If no role is available,
        // CloudFormation uses a temporary session that is generated from your user
        // credentials."
        Row {
            case: "no role at all, through a change set that names none",
            stack: "role-none-changeset",
            established: Established::NotAtAll,
            then: Then::AChangeSetNamingNone,
            in_force: InForce::None,
        },
        Row {
            case: "no role at all, until a change set names one",
            stack: "role-none-then-named",
            established: Established::NotAtAll,
            then: Then::AChangeSetNamingAnother,
            in_force: InForce::TheSecond,
        },
    ]
}

#[tokio::test]
async fn a_stack_keeps_its_service_role_until_an_operation_names_another() {
    for row in rows() {
        let (router, engine) = router_with_engine();
        let case = row.case;
        let stack = row.stack;
        let first = service_role(&router, &format!("{stack}-first"), allows_everything()).await;
        let second = service_role(&router, &format!("{stack}-second"), allows_everything()).await;
        let name = format!("/{stack}/p");
        let before = template(json!({"Param": parameter(&name, "before")}));
        let after = template(json!({"Param": parameter(&name, "after")}));

        match row.established {
            Established::ByCreateStack => {
                let (http_status, body) =
                    create_stack(&router, stack, &before, &[member("RoleARN", &first)]).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
                );
            }
            Established::NotAtAll => {
                let (http_status, body) = create_stack(&router, stack, &before, &[]).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
                );
            }
            Established::ByACreateChangeSet => {
                let (http_status, body) = create_change_set(
                    &router,
                    stack,
                    "the-plan",
                    &[
                        member("ChangeSetType", "CREATE"),
                        member("TemplateBody", &before),
                        member("RoleARN", &first),
                    ],
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected CreateChangeSet to succeed, got {http_status} {body}"
                );
                settle_the_store(&router, &engine).await;
                let (http_status, body) =
                    execute_change_set(&router, "the-plan", Some(stack)).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected ExecuteChangeSet to succeed, got {http_status} \
                     {body}"
                );
            }
        }
        settle_the_store(&router, &engine).await;
        // Whatever established the role, the stack is CREATED: the change-set row's stack is
        // the one the CREATE change set had left in review.
        let created = status_of(&router, stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the stack to be created, got {created:?}"
        );
        let established = role_of(&router, stack).await;
        let expected_established = match row.established {
            Established::NotAtAll => None,
            _ => Some(first.clone()),
        };
        assert_eq!(
            established, expected_established,
            "case {case:?}: expected the created stack to report {expected_established:?}, got \
             {established:?}"
        );

        let named = row.then.names_another_role();
        match row.then {
            Then::AChangeSetNamingNone | Then::AChangeSetNamingAnother => {
                let mut members = vec![
                    member("ChangeSetType", "UPDATE"),
                    member("TemplateBody", &after),
                ];
                if named {
                    members.push(member("RoleARN", &second));
                }
                let (http_status, body) =
                    create_change_set(&router, stack, "the-next-plan", &members).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected the second CreateChangeSet to succeed, got \
                     {http_status} {body}"
                );
                settle_the_store(&router, &engine).await;
                let (http_status, body) =
                    execute_change_set(&router, "the-next-plan", Some(stack)).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected the second ExecuteChangeSet to succeed, got \
                     {http_status} {body}"
                );
            }
            Then::AnUpdateNamingNone | Then::AnUpdateNamingAnother => {
                let mut members = vec![
                    member("StackName", stack),
                    member("TemplateBody", &after),
                ];
                if named {
                    members.push(member("RoleARN", &second));
                }
                let (http_status, body) = call(&router, "UpdateStack", &members).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected UpdateStack to succeed, got {http_status} {body}"
                );
            }
        }
        settle_the_store(&router, &engine).await;

        let updated = status_of(&router, stack).await;
        assert_eq!(
            updated,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the second operation to complete, got {updated:?}"
        );
        // The deployment really happened under whichever role was in force.
        let deployed = parameter_value(&router, &name).await;
        assert_eq!(
            deployed.as_deref(),
            Some("after"),
            "case {case:?}: expected the second deployment to have applied its change, got \
             {deployed:?}"
        );
        let reported = role_of(&router, stack).await;
        let expected = match row.in_force {
            InForce::TheFirst => Some(first.clone()),
            InForce::TheSecond => Some(second.clone()),
            InForce::None => None,
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the stack to report {expected:?}, got {reported:?}"
        );
    }
}

/// An `AWS::SNS::Topic` declaration: the resource the roles below are all allowed to create,
/// so the create is not what any case's denial is about.
fn topic(name: &str) -> Value {
    json!({
        "Type": "AWS::SNS::Topic",
        "Properties": {"TopicName": name},
    })
}

/// What the role in force on the SECOND deployment is, which is what it may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActsAs {
    /// The change set names no role, so the stack's own — the one denied the second
    /// deployment's call — is still what the calls are made as.
    TheRoleTheStackKept,
    /// The change set names a role that allows the call: a CONTROL, so no case passes
    /// against a gate that refuses the second deployment whatever role it runs under.
    ARoleNamedAfresh,
}

#[tokio::test]
async fn the_role_a_stack_kept_is_what_the_next_change_set_acts_as() {
    for acts_as in [ActsAs::TheRoleTheStackKept, ActsAs::ARoleNamedAfresh] {
        let (router, engine) = router_with_engine();
        let case = format!("{acts_as:?}");
        let stack = match acts_as {
            ActsAs::TheRoleTheStackKept => "acts-as-kept",
            ActsAs::ARoleNamedAfresh => "acts-as-afresh",
        };
        // The stack's own role may create the topic but is DENIED the call the second
        // deployment makes.
        let kept = service_role(
            &router,
            &format!("{stack}-kept"),
            json!([
                {"Effect": "Allow", "Action": "*", "Resource": "*"},
                {"Effect": "Deny", "Action": "ssm:PutParameter", "Resource": "*"},
            ]),
        )
        .await;
        let afresh = service_role(&router, &format!("{stack}-afresh"), allows_everything()).await;
        let topic_name = format!("{stack}-topic");
        let parameter_name = format!("/{stack}/p");

        let (http_status, body) = create_change_set(
            &router,
            stack,
            "the-plan",
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &template(json!({"Topic": topic(&topic_name)}))),
                member("RoleARN", &kept),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateChangeSet to succeed, got {http_status} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (http_status, body) = execute_change_set(&router, "the-plan", Some(stack)).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected ExecuteChangeSet to succeed, got {http_status} {body}"
        );
        settle_the_store(&router, &engine).await;
        let created = status_of(&router, stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create the role allows to complete, got {created:?}"
        );
        let (http_status, topics) = sns(&router, "ListTopics", &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected ListTopics to succeed, got {http_status} {topics}"
        );
        assert!(
            topics.contains(&format!(":{topic_name}<")),
            "case {case:?}: expected Amazon SNS to hold {topic_name:?}, got {topics}"
        );

        // The second deployment adds the resource the stack's own role is DENIED.
        let mut members = vec![
            member("ChangeSetType", "UPDATE"),
            member(
                "TemplateBody",
                &template(json!({
                    "Topic": topic(&topic_name),
                    "Param": parameter(&parameter_name, "v"),
                })),
            ),
        ];
        if acts_as == ActsAs::ARoleNamedAfresh {
            members.push(member("RoleARN", &afresh));
        }
        let (http_status, body) =
            create_change_set(&router, stack, "the-next-plan", &members).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the second CreateChangeSet to succeed, got {http_status} \
             {body}"
        );
        settle_the_store(&router, &engine).await;
        // `DisableRollback` keeps a failed execution exactly as it failed, so the case reads
        // the status the denied call left rather than the rollback that would follow it.
        let (http_status, body) = call(
            &router,
            "ExecuteChangeSet",
            &[
                member("ChangeSetName", "the-next-plan"),
                member("StackName", stack),
                member("DisableRollback", "true"),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the second ExecuteChangeSet to succeed, got {http_status} \
             {body}"
        );
        settle_the_store(&router, &engine).await;

        let denied = acts_as == ActsAs::TheRoleTheStackKept;
        let settled = status_of(&router, stack).await;
        let expected_status = if denied {
            status::UPDATE_FAILED
        } else {
            status::UPDATE_COMPLETE
        };
        assert_eq!(
            settled, expected_status,
            "case {case:?}: expected the stack to settle at {expected_status}, got {settled:?}"
        );
        // What Systems Manager really holds is the point of the rule: a call the role in
        // force was denied was never made.
        let held = parameter_value(&router, &parameter_name).await;
        let expected_value = if denied { None } else { Some("v") };
        assert_eq!(
            held.as_deref(),
            expected_value,
            "case {case:?}: expected Parameter Store to hold {expected_value:?}, got {held:?}"
        );
        // A resource whose create was refused is reported through the stack's EVENTS rather
        // than as a resource the stack has (`service_role_permissions.rs` reads a refused
        // create the same way), so only a created one is read back off the stack.
        if !denied {
            let reported = resource_status_of(&router, stack, "Param").await;
            assert_eq!(
                reported,
                resource_status::CREATE_COMPLETE,
                "case {case:?}: expected the resource to be reported {}, got {reported:?}",
                resource_status::CREATE_COMPLETE
            );
        }
        // The stack keeps the role either way: the one it kept, or the one the second change
        // set named.
        let in_force = role_of(&router, stack).await;
        let expected_role = if denied { &kept } else { &afresh };
        assert_eq!(
            in_force.as_deref(),
            Some(expected_role.as_str()),
            "case {case:?}: expected the stack to report {expected_role:?}, got {in_force:?}"
        );
        if denied {
            let events = stack_events(&case, &router, stack).await;
            assert!(
                events
                    .iter()
                    .any(|(logical_id, event_status)| logical_id == "Param"
                        && event_status == resource_status::CREATE_FAILED),
                "case {case:?}: expected (Param, CREATE_FAILED) among the events, got {events:?}"
            );
            let (_, described) =
                call(&router, "DescribeStackEvents", &[member("StackName", stack)]).await;
            assert!(
                described.contains("is not authorized to perform"),
                "case {case:?}: expected the unauthorized refusal in the event stream, got \
                 {described}"
            );
        }
    }
}
