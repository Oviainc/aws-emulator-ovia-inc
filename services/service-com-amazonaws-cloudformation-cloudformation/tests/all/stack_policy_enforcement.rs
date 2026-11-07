// SPDX-License-Identifier: CC0-1.0

//! A stack policy ENFORCED on `UpdateStack`, and the in-place property update of an
//! `AWS::SNS::Topic`, end to end over the awsQuery wire.
//!
//! **The first rule under test.** A resource action an update plans is carried out only
//! where the stack policy in force permits it: an explicit `Deny` matching the action and
//! the resource wins over every `Allow`, and an action no statement allows is not permitted
//! either. A denied action does not happen — the stack keeps the template, the parameters
//! and the resources it had — the stack settles at `UPDATE_ROLLBACK_COMPLETE`, and the
//! protected resource's own event is `UPDATE_FAILED` naming the statement, the action and
//! the resource. Which action a resource is held to follows from what the update would do
//! to it: a change that keeps its physical id is `Update:Modify`, one that replaces it is
//! `Update:Replace`, and dropping it from the template is `Update:Delete`.
//! `StackPolicyDuringUpdateBody` overrides the stored policy for exactly one update and is
//! never stored.
//!
//! The rows span the class the rule generalises over rather than the one recorded case: each
//! of the three actions, the `Update:*` wildcard and an exactly-named action, a policy that
//! denies an action the update does not plan, a resource named exactly and by wildcard, a
//! statement conditioned on the resource type (matching and not), a policy with no `Allow`
//! at all, and a resource the policy says nothing about.
//!
//! The same policy governs the OTHER operation that deploys an update to a live stack,
//! `ExecuteChangeSet`, which is held to the plan its change set stored: the last case here
//! is that one, down to the change set being left in the modelled `EXECUTE_FAILED`.
//!
//! **The second rule under test.** A changed declaration of a resource the stack already has
//! is applied THROUGH the owning service's own API, so the resource keeps its physical id:
//! for an `AWS::SNS::Topic` every property is an attribute of the topic, and a changed,
//! added or dropped one reaches Amazon SNS as the write of that attribute — read back here
//! through SNS's own `GetTopicAttributes`.

use crate::stack_lifecycle::{
    call, deploy, describe_stack, list_members, member, router, sns, wait_for_the_transition,
    xml_text, xml_texts,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

/// The blanket permission every documented stack policy that means to allow anything
/// carries, which the rows that protect ONE resource sit behind.
const ALLOW_EVERYTHING: &str =
    r#"{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}"#;

/// A template declaring one protected `AWS::SSM::Parameter` and one unprotected sibling,
/// each with the value the row asks for and the protected one under `name`.
fn template(protected_name: &str, protected_value: &str, free_value: &str) -> String {
    format!(
        r#"{{"Resources":{{
             "Protected":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Name":"{protected_name}","Type":"String",
                 "Value":"{protected_value}"}}}},
             "Free":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Type":"String","Value":"{free_value}"}}}}}}}}"#
    )
}

/// A template declaring the unprotected sibling alone: the update that DROPS the protected
/// resource.
fn template_without_the_protected_resource(free_value: &str) -> String {
    format!(
        r#"{{"Resources":{{
             "Free":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Type":"String","Value":"{free_value}"}}}}}}}}"#
    )
}

/// What a row expects of the update: the stack completes it, or the policy denies it with
/// this statement index — counted from ONE, as the document numbers its statements to a
/// reader — and the action and resource THAT statement forbids.
enum Expected {
    Completed,
    Denied {
        statement: &'static str,
        /// The action the denied resource's update plans, which is what a denial reports
        /// where NO statement allows it (there is no statement to quote then).
        action: &'static str,
        /// The `Action` the denying statement itself carries, which is what a denial
        /// attributed to a statement reports.
        denies: &'static str,
        /// ...and its `Resource`.
        protects: &'static str,
        logical_id: &'static str,
    },
}

/// One row: a stack with two parameters, a policy, and an update to hold to it.
struct PolicyCase {
    case: &'static str,
    stack: &'static str,
    /// The policy document in force, whose statements are in document order — the index of
    /// the one that denies is what the refusal reports.
    policy: String,
    /// The template the update submits.
    updated: String,
    expected: Expected,
}

/// The name the protected parameter is created under, per stack.
fn protected_name(stack: &str) -> String {
    format!("{stack}-protected")
}

fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "Deny Update:* on the protected resource, whose value the update changes",
            stack: "policy-deny-all-modify",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
                    "Resource":"LogicalResourceId/Protected"}"#,
            ]),
            updated: template("policy-deny-all-modify-protected", "v2", "v2"),
            expected: Expected::Denied {
                statement: "2",
                action: "Update:Modify",
                denies: "Update:*",
                protects: "LogicalResourceId/Protected",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            case: "Deny Update:Modify, named exactly, on the resource the update modifies",
            stack: "policy-deny-modify",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Modify","Principal":"*",
                    "Resource":"LogicalResourceId/Protected"}"#,
            ]),
            updated: template("policy-deny-modify-protected", "v2", "v1"),
            expected: Expected::Denied {
                statement: "2",
                action: "Update:Modify",
                denies: "Update:Modify",
                protects: "LogicalResourceId/Protected",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            case: "Deny Update:Replace, with the update changing the create-only name",
            stack: "policy-deny-replace",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Replace","Principal":"*",
                    "Resource":"LogicalResourceId/Protected"}"#,
            ]),
            updated: template("policy-deny-replace-renamed", "v1", "v1"),
            expected: Expected::Denied {
                statement: "2",
                action: "Update:Replace",
                denies: "Update:Replace",
                protects: "LogicalResourceId/Protected",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            // A `Resource` WILDCARD covering the protected resource rather than its logical
            // id spelled out, because the update DROPS that resource: a stored statement
            // naming a logical id the submitted template does not declare is refused before
            // the update is weighed at all
            // (`stored_stack_policy_validation.rs` is the suite that pins that refusal).
            case: "Deny Update:Delete, with the update dropping the resource",
            stack: "policy-deny-delete",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Delete","Principal":"*",
                    "Resource":"LogicalResourceId/Prot*"}"#,
            ]),
            updated: template_without_the_protected_resource("v1"),
            expected: Expected::Denied {
                statement: "2",
                action: "Update:Delete",
                denies: "Update:Delete",
                protects: "LogicalResourceId/Prot*",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            // The action the policy denies is not the action the update plans, so the
            // statement does not apply and the blanket Allow is what governs.
            case: "Deny Update:Replace, with the update only modifying the resource",
            stack: "policy-deny-replace-modify-allowed",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Replace","Principal":"*",
                    "Resource":"LogicalResourceId/Protected"}"#,
            ]),
            updated: template(
                "policy-deny-replace-modify-allowed-protected",
                "v2",
                "v1",
            ),
            expected: Expected::Completed,
        },
        PolicyCase {
            case: "a Deny whose Resource wildcard covers the protected resource",
            stack: "policy-deny-wildcard-resource",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
                    "Resource":"LogicalResourceId/Prot*"}"#,
            ]),
            updated: template("policy-deny-wildcard-resource-protected", "v2", "v1"),
            expected: Expected::Denied {
                statement: "2",
                action: "Update:Modify",
                denies: "Update:*",
                protects: "LogicalResourceId/Prot*",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            case: "a Deny naming another resource of the template, which the update leaves \
                   alone",
            stack: "policy-deny-another-resource",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
                    "Resource":"LogicalResourceId/Free"}"#,
            ]),
            updated: template("policy-deny-another-resource-protected", "v2", "v1"),
            expected: Expected::Completed,
        },
        PolicyCase {
            case: "a Deny conditioned on the type the protected resource is",
            stack: "policy-deny-matching-type",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*","Resource":"*",
                    "Condition":{"StringEquals":{"ResourceType":
                      ["AWS::SSM::Parameter"]}}}"#,
            ]),
            updated: template("policy-deny-matching-type-protected", "v2", "v1"),
            expected: Expected::Denied {
                statement: "2",
                action: "Update:Modify",
                denies: "Update:*",
                protects: "*",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            case: "a Deny conditioned on a type no resource of the stack is",
            stack: "policy-deny-other-type",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*","Resource":"*",
                    "Condition":{"StringEquals":{"ResourceType":["AWS::SQS::Queue"]}}}"#,
            ]),
            updated: template("policy-deny-other-type-protected", "v2", "v1"),
            expected: Expected::Completed,
        },
        PolicyCase {
            // Setting a policy protects the stack's resources: an action no statement
            // allows is not permitted, so the sibling this policy says nothing about is
            // denied too.
            case: "a policy that allows nothing, with the update modifying the sibling",
            stack: "policy-allows-nothing",
            policy: policy_of(&[
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
                    "Resource":"LogicalResourceId/Protected"}"#,
            ]),
            updated: template("policy-allows-nothing-protected", "v1", "v2"),
            expected: Expected::Denied {
                statement: "none",
                action: "Update:Modify",
                denies: "",
                protects: "",
                logical_id: "Free",
            },
        },
        PolicyCase {
            // The same reading at the bound: a document that IS a policy and carries no
            // statement at all allows nothing, so the action it says nothing about is
            // denied like any other.
            case: "a policy whose statement list is empty",
            stack: "policy-no-statements",
            policy: policy_of(&[]),
            updated: template("policy-no-statements-protected", "v2", "v1"),
            expected: Expected::Denied {
                statement: "none",
                action: "Update:Modify",
                denies: "",
                protects: "",
                logical_id: "Protected",
            },
        },
        PolicyCase {
            // A body that is no policy DOCUMENT at all — it carries no `Statement` element
            // — governs nothing: no update is failed on account of a document nobody could
            // read.
            case: "a body that is not a policy document",
            stack: "policy-not-a-document",
            policy: "{}".to_string(),
            updated: template("policy-not-a-document-protected", "v2", "v1"),
            expected: Expected::Completed,
        },
    ]
}

/// The policy document a row's statements make.
fn policy_of(statements: &[&str]) -> String {
    format!(r#"{{"Statement":[{}]}}"#, statements.join(","))
}

/// `SetStackPolicy` with a document, asserted to succeed.
async fn set_policy(router: &MultiServiceRouter, case: &str, stack: &str, policy: &str) {
    let (http_status, body) = call(
        router,
        "SetStackPolicy",
        &[
            member("StackName", stack),
            member("StackPolicyBody", policy),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected SetStackPolicy to succeed, got {http_status} {body}"
    );
}

/// The document `GetStackPolicy` reports for a stack.
async fn stored_policy(router: &MultiServiceRouter, case: &str, stack: &str) -> Option<String> {
    let (http_status, body) =
        call(router, "GetStackPolicy", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected GetStackPolicy to succeed, got {http_status} {body}"
    );
    xml_text(&body, "StackPolicyBody")
}

/// `UpdateStack` with a template body and whatever else the case needs.
async fn update_stack(
    router: &MultiServiceRouter,
    stack: &str,
    body: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("StackName", stack),
        member("TemplateBody", body),
    ];
    members.extend_from_slice(extra);
    call(router, "UpdateStack", &members).await
}

/// The `StackStatus` a stack reports now.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (http_status, body) = describe_stack(router, stack).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {http_status} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The `StackStatusReason` a stack reports now, empty where it reports none.
async fn stack_status_reason(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (http_status, body) = describe_stack(router, stack).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {http_status} {body}"
    );
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}

/// Every `(LogicalResourceId, ResourceStatus, ResourceStatusReason)` the stack's event log
/// carries, newest first.
async fn events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String, String)> {
    let (http_status, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackEvents to succeed, got {http_status} {body}"
    );
    list_members(&body, "StackEvents")
        .into_iter()
        .map(|event| {
            (
                xml_text(&event, "LogicalResourceId").unwrap_or_default(),
                xml_text(&event, "ResourceStatus").unwrap_or_default(),
                xml_text(&event, "ResourceStatusReason").unwrap_or_default(),
            )
        })
        .collect()
}

/// The template body a stack carries now, as `GetTemplate` serves it.
async fn stored_template(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (http_status, body) = call(
        router,
        "GetTemplate",
        &[
            member("StackName", stack),
            member("TemplateStage", "Original"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected GetTemplate to succeed, got {http_status} {body}"
    );
    xml_text(&body, "TemplateBody").unwrap_or_default()
}

#[tokio::test]
async fn an_update_action_a_stack_policy_denies_does_not_happen() {
    let router = router();

    for case in policy_cases() {
        let created = template(&protected_name(case.stack), "v1", "v1");
        deploy(&router, case.stack, &created).await;
        set_policy(&router, case.case, case.stack, &case.policy).await;

        // "After the call completes successfully, the stack update starts": the verdict is
        // asynchronous, so a denied update is still answered 200 — this operation models no
        // error for a policy at all.
        let (http_status, body) = update_stack(&router, case.stack, &case.updated, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to be accepted, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let reported = stack_status(&router, case.case, case.stack).await;
        let log = events(&router, case.case, case.stack).await;
        match case.expected {
            Expected::Completed => {
                assert_eq!(
                    reported,
                    status::UPDATE_COMPLETE,
                    "case {:?}: expected the permitted update to complete, got \
                     {reported:?} with {log:?}",
                    case.case
                );
                assert!(
                    !log.iter().any(|(_, resource, reason)| resource
                        == resource_status::UPDATE_FAILED
                        || reason.contains("stack policy")),
                    "case {:?}: expected no policy denial in the event log, got {log:?}",
                    case.case
                );
                assert_eq!(
                    stored_template(&router, case.case, case.stack).await,
                    case.updated,
                    "case {:?}: expected the completed update to install its template",
                    case.case
                );
            }
            Expected::Denied {
                statement,
                action,
                denies,
                protects,
                logical_id,
            } => {
                assert_eq!(
                    reported,
                    status::UPDATE_ROLLBACK_COMPLETE,
                    "case {:?}: expected the denied update to roll the stack back, got \
                     {reported:?} with {log:?}",
                    case.case
                );
                let wanted = match statement {
                    "none" => format!(
                        "Action denied by stack policy: No statement allows [{action}] for \
                         resource [LogicalResourceId/{logical_id}];"
                    ),
                    // The statement that denied it, by its position in the document counted
                    // from one, and what THAT statement forbids.
                    index => format!(
                        "Action denied by stack policy: Statement [#{index}] does not allow \
                         [{denies}] for resource [{protects}];"
                    ),
                };
                let failed: Vec<&(String, String, String)> = log
                    .iter()
                    .filter(|(resource, resource_status, _)| {
                        resource == logical_id
                            && resource_status == resource_status::UPDATE_FAILED
                    })
                    .collect();
                assert_eq!(
                    failed.len(),
                    1,
                    "case {:?}: expected exactly one UPDATE_FAILED event for \
                     {logical_id:?}, got {log:?}",
                    case.case
                );
                assert_eq!(
                    failed[0].2, wanted,
                    "case {:?}: expected the denial reason {wanted:?}, got {:?}",
                    case.case, failed[0].2
                );
                // Nothing was asked of any resource, so what the STACK reports it failed
                // for is the denial itself rather than a summary of failed resources.
                let reported_reason = stack_status_reason(&router, case.case, case.stack).await;
                assert_eq!(
                    reported_reason, wanted,
                    "case {:?}: expected the stack to report the denial as its reason, got \
                     {reported_reason:?}",
                    case.case
                );
                // Nothing of the denied update was deployed: the stack still carries the
                // template it was created with, so the SAME update is a change again once
                // the policy permits it.
                assert_eq!(
                    stored_template(&router, case.case, case.stack).await,
                    created,
                    "case {:?}: expected the denied update to leave the template alone",
                    case.case
                );
                set_policy(
                    &router,
                    case.case,
                    case.stack,
                    &policy_of(&[ALLOW_EVERYTHING]),
                )
                .await;
                let (http_status, body) =
                    update_stack(&router, case.stack, &case.updated, &[]).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: expected the same update to be accepted once the Deny is \
                     lifted, got {http_status} {body}",
                    case.case
                );
                wait_for_the_transition().await;
                wait_for_the_transition().await;
                let reported = stack_status(&router, case.case, case.stack).await;
                assert_eq!(
                    reported,
                    status::UPDATE_COMPLETE,
                    "case {:?}: expected the update to complete once the Deny is lifted, \
                     got {reported:?}",
                    case.case
                );
            }
        }
    }
}

#[tokio::test]
async fn a_during_update_policy_overrides_the_stored_one_for_that_update_alone() {
    let router = router();
    let case = "an override carried by one update";
    let stack = "policy-during-update";
    let created = template(&protected_name(stack), "v1", "v1");
    deploy(&router, stack, &created).await;

    let deny = policy_of(&[
        ALLOW_EVERYTHING,
        r#"{"Effect":"Deny","Action":"Update:Modify","Principal":"*",
            "Resource":"LogicalResourceId/Protected"}"#,
    ]);
    set_policy(&router, case, stack, &deny).await;

    // Denied: the stored policy is what governs an update carrying no override.
    let second = template(&protected_name(stack), "v2", "v1");
    let (http_status, body) = update_stack(&router, stack, &second, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected UpdateStack to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let reported = stack_status(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the stored Deny to block the update, got {reported:?}"
    );

    // The SAME update, carrying an override that permits everything: it goes through.
    let (http_status, body) = update_stack(
        &router,
        stack,
        &second,
        &[member(
            "StackPolicyDuringUpdateBody",
            &policy_of(&[ALLOW_EVERYTHING]),
        )],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the overridden update to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let reported = stack_status(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the override to let the update complete, got {reported:?}"
    );

    // "a temporary overriding stack policy during this update": the stored document is
    // untouched, so the next update with no override is denied again.
    assert_eq!(
        stored_policy(&router, case, stack).await.as_deref(),
        Some(deny.as_str()),
        "case {case:?}: expected the stored policy to be the one set, not the override"
    );
    let third = template(&protected_name(stack), "v3", "v1");
    let (http_status, body) = update_stack(&router, stack, &third, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the next update to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let reported = stack_status(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the un-overridden update to be denied again, got {reported:?}"
    );
}

#[tokio::test]
async fn executing_a_change_set_is_held_to_the_stack_policy_too() {
    let router = router();
    let case = "an UPDATE change set modifying a protected resource";
    let stack = "policy-change-set";
    let change_set = "protected-change";
    let created = template(&protected_name(stack), "v1", "v1");
    deploy(&router, stack, &created).await;

    // The plan the execution would carry out, computed and stored when the change set was
    // created — which is what the policy in force is weighed against.
    let updated = template(&protected_name(stack), "v2", "v1");
    let (http_status, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", change_set),
            member("TemplateBody", &updated),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateChangeSet to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;

    let deny = policy_of(&[
        ALLOW_EVERYTHING,
        r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
            "Resource":"LogicalResourceId/Protected"}"#,
    ]);
    set_policy(&router, case, stack, &deny).await;

    let (http_status, body) = call(
        &router,
        "ExecuteChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", change_set),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected ExecuteChangeSet to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let reported = stack_status(&router, case, stack).await;
    let log = events(&router, case, stack).await;
    assert_eq!(
        reported,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the denied execution to roll the stack back, got \
         {reported:?} with {log:?}"
    );
    let wanted = "Action denied by stack policy: Statement [#2] does not allow \
                  [Update:*] for resource [LogicalResourceId/Protected];";
    assert!(
        log.iter().any(|(resource, status_value, reason)| {
            resource == "Protected"
                && status_value == resource_status::UPDATE_FAILED
                && reason == wanted
        }),
        "case {case:?}: expected the protected resource to report {wanted:?}, got {log:?}"
    );
    // Nothing of the change set was applied: the stack still carries the template it was
    // created with.
    assert_eq!(
        stored_template(&router, case, stack).await,
        created,
        "case {case:?}: expected the denied execution to leave the template alone"
    );
    // The execution was attempted and did not go through, so the change set is not left as
    // one nothing has executed yet.
    let (http_status, body) = call(
        &router,
        "DescribeChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", change_set),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeChangeSet to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "ExecutionStatus").as_deref(),
        Some("EXECUTE_FAILED"),
        "case {case:?}: expected the refused execution to leave the change set \
         EXECUTE_FAILED, got {body}"
    );
}

/// The attribute map Amazon SNS reports for a topic, read through its own wire.
async fn topic_attributes(
    router: &MultiServiceRouter,
    case: &str,
    topic_arn: &str,
) -> Vec<(String, String)> {
    let (http_status, body) = sns(
        router,
        "GetTopicAttributes",
        &[("TopicArn", topic_arn)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected GetTopicAttributes to succeed, got {http_status} {body}"
    );
    xml_texts(&body, "key")
        .into_iter()
        .zip(xml_texts(&body, "value"))
        .collect()
}

/// The physical id the stack reports for one of its resources.
async fn physical_id(router: &MultiServiceRouter, case: &str, stack: &str, logical_id: &str) -> String {
    let (http_status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackResource to succeed, got {http_status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// One step of the SNS table: the properties the update declares for the topic, and what
/// Amazon SNS must report about the topic afterwards.
struct TopicStep {
    case: &'static str,
    properties: &'static str,
    /// Attributes SNS must report with these values.
    reported: Vec<(&'static str, &'static str)>,
    /// Attributes SNS must not report at all, or report empty: the ones the declaration
    /// dropped.
    cleared: Vec<&'static str>,
}

#[tokio::test]
async fn a_changed_sns_topic_declaration_is_applied_to_the_topic_in_place() {
    let router = router();
    let stack = "sns-in-place";
    let name = "sns-in-place-topic";
    let template_with = |properties: &str| {
        format!(
            r#"{{"Resources":{{"Topic":{{"Type":"AWS::SNS::Topic",
                 "Properties":{{"TopicName":"{name}"{properties}}}}}}}}}"#
        )
    };
    deploy(&router, stack, &template_with(r#","DisplayName":"first""#)).await;
    let arn = physical_id(&router, "the create", stack, "Topic").await;

    // The class: a property whose value the update changes, one it adds, and one it drops.
    let steps = vec![
        TopicStep {
            case: "a changed property value",
            properties: r#","DisplayName":"second""#,
            reported: vec![("DisplayName", "second")],
            cleared: vec![],
        },
        TopicStep {
            case: "a property the update adds beside the one it keeps",
            properties: r#","DisplayName":"second","KmsMasterKeyId":"alias/aws/sns""#,
            reported: vec![
                ("DisplayName", "second"),
                ("KmsMasterKeyId", "alias/aws/sns"),
            ],
            cleared: vec![],
        },
        TopicStep {
            case: "a property the update drops",
            properties: r#","KmsMasterKeyId":"alias/aws/sns""#,
            reported: vec![("KmsMasterKeyId", "alias/aws/sns")],
            cleared: vec!["DisplayName"],
        },
    ];

    for step in steps {
        let (http_status, body) =
            update_stack(&router, stack, &template_with(step.properties), &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to be accepted, got {http_status} {body}",
            step.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let reported = stack_status(&router, step.case, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {:?}: expected the in-place update to complete, got {reported:?} with {:?}",
            step.case,
            events(&router, step.case, stack).await
        );
        // The topic was updated, not replaced: "Ref returns the topic ARN", and it is the
        // ARN the stack already published.
        let after = physical_id(&router, step.case, stack, "Topic").await;
        assert_eq!(
            after, arn,
            "case {:?}: expected the topic to keep its ARN, got {after:?}",
            step.case
        );
        // …and the change really reached Amazon SNS, which is where the topic lives.
        let attributes = topic_attributes(&router, step.case, &arn).await;
        for (name, value) in &step.reported {
            let held = attributes
                .iter()
                .find(|(attribute, _)| attribute == name)
                .map(|(_, held)| held.as_str());
            assert_eq!(
                held,
                Some(*value),
                "case {:?}: expected SNS to report {name}={value:?}, got {attributes:?}",
                step.case
            );
        }
        for name in &step.cleared {
            let held = attributes
                .iter()
                .find(|(attribute, _)| attribute == name)
                .map(|(_, held)| held.as_str());
            assert!(
                held.is_none_or(str::is_empty),
                "case {:?}: expected SNS to report no {name}, got {held:?}",
                step.case
            );
        }
    }
}
