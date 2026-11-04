// SPDX-License-Identifier: CC0-1.0

//! Wait conditions and `SignalResource`, end to end over the awsQuery wire.
//!
//! The rule under test: CloudFormation does not complete a stack operation whose template
//! declares a wait condition until that wait condition has received the number of success
//! signals its `Count` asks for (or its `Timeout` runs out, which fails the operation) —
//! "CloudFormation doesn't proceed with a stack creation or update until resources receive
//! the required number of signals or the timeout period is exceeded". While it waits, the
//! stack stays `*_IN_PROGRESS` however long ago the call that started it returned, the wait
//! condition is reported `*_IN_PROGRESS`, and the resources whose creation depends on it are
//! not reported at all; `SignalResource` is the call that delivers a signal, and only a
//! `StackName` that names no live stack is refused (HTTP 400 `ValidationError`) — a signal
//! nothing is waiting for is accepted, recorded and releases nothing.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a client sees.
//! The table spans the class the rule generalises over: both template spellings, a `Count`
//! stated as a string, as a number and not stated at all, one wait condition and two of
//! them, dependency stated by `DependsOn` as a string and as a list, a dependency reached
//! through another resource, repeated and distinct `UniqueId`s, both signal statuses, and a
//! `Timeout` that has and has not run out.

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, router,
    wait_for_the_transition, xml_text,
};
use crate::stack_resources::{resources_of, wire_resources};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status,
};
use smithy_server_runtime::MultiServiceRouter;

/// `SignalResource` for one resource of one stack.
pub(crate) async fn signal(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    unique_id: &str,
    status: &str,
) -> (u16, String) {
    call(
        router,
        "SignalResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
            member("UniqueId", unique_id),
            member("Status", status),
        ],
    )
    .await
}

/// The `StackStatus` a stack reports right now.
async fn status_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(status, 200, "DescribeStacks {stack:?}: body {body}");
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The `StackStatusReason` a stack reports right now.
async fn status_reason_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = describe_stack(router, stack).await;
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}

/// Every resource a stack reports right now, as `(logical id, resource status)`, sorted.
async fn reported(router: &MultiServiceRouter, stack: &str) -> Vec<(String, String)> {
    let (status, body) = resources_of(router, stack).await;
    assert_eq!(
        status, 200,
        "DescribeStackResources {stack:?}: body {body}"
    );
    let mut rows: Vec<(String, String)> = wire_resources(&body)
        .into_iter()
        .map(|resource| {
            (
                resource.logical_id.clone().unwrap_or_default(),
                resource.status.clone().unwrap_or_default(),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The error code an XML envelope carries.
fn error_code(body: &str) -> String {
    xml_text(body, "Code").unwrap_or_default()
}

/// The `ResourceStatusReason` a stack reports for ONE of its resources, read through the
/// `LogicalResourceId` filter so the answer carries that resource alone.
async fn reason_for(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources {stack:?}/{logical_id:?}: body {body}"
    );
    xml_text(&body, "ResourceStatusReason")
}

/// One row of the barrier table.
struct BarrierCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The signals the operation waits for, in the order the case sends them:
    /// `(LogicalResourceId, UniqueId)`.
    signals: Vec<(&'static str, &'static str)>,
    /// What the stack reports while it waits: the resources the operation has created,
    /// each with the status it is in.
    started: Vec<(&'static str, &'static str)>,
    /// The resources whose creation waits on a wait condition, which are not reported at
    /// all while it holds.
    unstarted: Vec<&'static str>,
}

/// The class the rule covers: a template declaring at least one wait condition, however it
/// states the count it waits for and however the resources after it depend on it.
fn barrier_cases() -> Vec<BarrierCase> {
    vec![
        BarrierCase {
            case: "one wait condition, Count stated as a string, DependsOn a name",
            stack: "wait-one",
            template: r#"{
                "Resources": {
                    "Gate": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Ready": {
                        "Type": "AWS::SNS::Topic",
                        "Properties": {"TopicName": "wait-one-ready"}
                    },
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "DependsOn": "Ready",
                        "Properties": {
                            "Handle": {"Ref": "Gate"},
                            "Timeout": "300",
                            "Count": "1"
                        }
                    },
                    "After": {
                        "Type": "AWS::SNS::Topic",
                        "DependsOn": "Wait",
                        "Properties": {"TopicName": "wait-one-after"}
                    }
                }
            }"#
            .to_string(),
            signals: vec![("Wait", "signal-1")],
            started: vec![
                ("Gate", resource_status::CREATE_COMPLETE),
                ("Ready", resource_status::CREATE_COMPLETE),
                ("Wait", resource_status::CREATE_IN_PROGRESS),
            ],
            unstarted: vec!["After"],
        },
        BarrierCase {
            case: "Count stated as a number, two signals, a dependency chain behind it",
            stack: "wait-two-signals",
            template: r#"{
                "Resources": {
                    "Gate": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "Properties": {
                            "Handle": {"Ref": "Gate"},
                            "Timeout": 300,
                            "Count": 2
                        }
                    },
                    "After": {
                        "Type": "AWS::SNS::Topic",
                        "DependsOn": ["Wait"],
                        "Properties": {"TopicName": "wait-two-after"}
                    },
                    "Later": {
                        "Type": "AWS::SNS::Topic",
                        "Properties": {
                            "TopicName": "wait-two-later",
                            "DisplayName": {"Ref": "After"}
                        }
                    }
                }
            }"#
            .to_string(),
            // The same id twice is ONE signal ("each signal requires a different unique
            // ID"), so the third of these is what releases the barrier.
            signals: vec![("Wait", "host-a"), ("Wait", "host-a"), ("Wait", "host-b")],
            started: vec![
                ("Gate", resource_status::CREATE_COMPLETE),
                ("Wait", resource_status::CREATE_IN_PROGRESS),
            ],
            unstarted: vec!["After", "Later"],
        },
        BarrierCase {
            case: "a YAML template whose wait condition states no Count at all",
            stack: "wait-yaml",
            template: concat!(
                "Resources:\n",
                "  Gate:\n",
                "    Type: AWS::CloudFormation::WaitConditionHandle\n",
                "  Wait:\n",
                "    Type: AWS::CloudFormation::WaitCondition\n",
                "    Properties:\n",
                "      Handle: !Ref Gate\n",
                "      Timeout: '300'\n",
                "  After:\n",
                "    Type: AWS::SNS::Topic\n",
                "    DependsOn: Wait\n",
                "    Properties:\n",
                "      TopicName: wait-yaml-after\n",
            )
            .to_string(),
            signals: vec![("Wait", "only-one")],
            started: vec![
                ("Gate", resource_status::CREATE_COMPLETE),
                ("Wait", resource_status::CREATE_IN_PROGRESS),
            ],
            unstarted: vec!["After"],
        },
        BarrierCase {
            case: "two wait conditions, both waited at, each gating its own resource",
            stack: "wait-both",
            template: r#"{
                "Resources": {
                    "GateOne": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "GateTwo": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "WaitOne": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "Properties": {"Handle": {"Ref": "GateOne"}, "Timeout": "300"}
                    },
                    "WaitTwo": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "Properties": {"Handle": {"Ref": "GateTwo"}, "Timeout": "300"}
                    },
                    "AfterOne": {
                        "Type": "AWS::SNS::Topic",
                        "DependsOn": "WaitOne",
                        "Properties": {"TopicName": "wait-both-one"}
                    },
                    "AfterTwo": {
                        "Type": "AWS::SNS::Topic",
                        "DependsOn": "WaitTwo",
                        "Properties": {"TopicName": "wait-both-two"}
                    }
                }
            }"#
            .to_string(),
            signals: vec![("WaitOne", "first"), ("WaitTwo", "second")],
            started: vec![
                ("GateOne", resource_status::CREATE_COMPLETE),
                ("GateTwo", resource_status::CREATE_COMPLETE),
                ("WaitOne", resource_status::CREATE_IN_PROGRESS),
                ("WaitTwo", resource_status::CREATE_IN_PROGRESS),
            ],
            unstarted: vec!["AfterOne", "AfterTwo"],
        },
    ]
}

#[tokio::test]
async fn a_create_waits_at_its_wait_condition_until_the_signals_arrive() {
    for case in barrier_cases() {
        let BarrierCase {
            case,
            stack,
            template,
            signals,
            started,
            unstarted,
        } = case;
        let router = router();
        let (status, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to succeed, got {status} {body}"
        );

        // Past the instant the clock alone would have completed the create: the barrier is
        // what holds the operation, not a timer, so the stack is still in progress.
        wait_for_the_transition().await;
        let reported_status = status_of(&router, stack).await;
        assert_eq!(
            reported_status, "CREATE_IN_PROGRESS",
            "case {case:?}: expected the create to still be in progress while the wait \
             condition is unsignalled, got {reported_status:?}"
        );

        let expected_started: Vec<(String, String)> = {
            let mut rows: Vec<(String, String)> = started
                .iter()
                .map(|(logical_id, status)| ((*logical_id).to_string(), (*status).to_string()))
                .collect();
            rows.sort();
            rows
        };
        let actual = reported(&router, stack).await;
        assert_eq!(
            actual, expected_started,
            "case {case:?}: expected the resources created so far, got {actual:?}"
        );
        for logical_id in &unstarted {
            assert!(
                !actual.iter().any(|(reported, _)| reported == logical_id),
                "case {case:?}: expected {logical_id:?} not to be reported while its wait \
                 condition holds, got {actual:?}"
            );
        }

        // "Success/failure message associated with the resource": a resource whose work
        // CloudFormation has initiated and not finished says so, and one that is done does
        // not.
        for (logical_id, resource_status) in &started {
            let reason = reason_for(&router, stack, logical_id).await;
            let expected = if *resource_status == resource_status::CREATE_IN_PROGRESS {
                Some("Resource creation Initiated".to_string())
            } else {
                None
            };
            assert_eq!(
                reason, expected,
                "case {case:?}: expected {logical_id:?} in {resource_status} to report \
                 {expected:?} as its status reason, got {reason:?}"
            );
        }

        // Every signal but the last leaves the operation waiting.
        let last = signals.len() - 1;
        for (index, (logical_id, unique_id)) in signals.iter().enumerate() {
            let (status, body) = signal(&router, stack, logical_id, unique_id, "SUCCESS").await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected signal {unique_id:?} to be accepted, got \
                 {status} {body}"
            );
            if index < last {
                let reported_status = status_of(&router, stack).await;
                assert_eq!(
                    reported_status, "CREATE_IN_PROGRESS",
                    "case {case:?}: expected the create to still be in progress after \
                     signal {unique_id:?}, got {reported_status:?}"
                );
            }
        }

        // The releasing signal does not take the resources the operation had already
        // created out of the answer while it finishes.
        let actual = reported(&router, stack).await;
        for (logical_id, _) in &started {
            assert!(
                actual.iter().any(|(reported, _)| reported == logical_id),
                "case {case:?}: expected {logical_id:?} to go on being reported after the \
                 releasing signal, got {actual:?}"
            );
        }

        // "When the wait condition receives the requisite number of success signals,
        // CloudFormation resumes the creation of the stack."
        wait_for_the_transition().await;
        let reported_status = status_of(&router, stack).await;
        assert_eq!(
            reported_status, "CREATE_COMPLETE",
            "case {case:?}: expected the signalled create to complete, got \
             {reported_status:?}"
        );
        let actual = reported(&router, stack).await;
        for (logical_id, _) in &started {
            assert!(
                actual.iter().any(|(reported, reported_status)| reported
                    == logical_id
                    && reported_status == resource_status::CREATE_COMPLETE),
                "case {case:?}: expected {logical_id:?} to be CREATE_COMPLETE once the \
                 create completed, got {actual:?}"
            );
        }
        for logical_id in &unstarted {
            assert!(
                actual.iter().any(|(reported, reported_status)| reported
                    == logical_id
                    && reported_status == resource_status::CREATE_COMPLETE),
                "case {case:?}: expected the resource {logical_id:?} that waited on the \
                 wait condition to be created once it was signalled, got {actual:?}"
            );
        }
        // The round trip through the single-resource read: a resource that was not
        // reported while the barrier held is readable once it is created.
        for logical_id in &unstarted {
            let (status, body) = call(
                &router,
                "DescribeStackResource",
                &[
                    member("StackName", stack),
                    member("LogicalResourceId", logical_id),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {logical_id:?} to be described once created, got \
                 {status} {body}"
            );
            assert!(
                xml_text(&body, "PhysicalResourceId").is_some(),
                "case {case:?}: expected {logical_id:?} to report a physical id, got {body}"
            );
        }
    }
}

/// A stack with one wait condition that waits for one signal, for the cases that do not
/// need a table of their own.
fn one_wait_condition(stack: &str, timeout: &str, count: &str) -> String {
    format!(
        r#"{{
            "Resources": {{
                "Gate": {{"Type": "AWS::CloudFormation::WaitConditionHandle"}},
                "Wait": {{
                    "Type": "AWS::CloudFormation::WaitCondition",
                    "Properties": {{
                        "Handle": {{"Ref": "Gate"}},
                        "Timeout": "{timeout}",
                        "Count": "{count}"
                    }}
                }},
                "After": {{
                    "Type": "AWS::SNS::Topic",
                    "DependsOn": "Wait",
                    "Properties": {{"TopicName": "{stack}-after"}}
                }}
            }}
        }}"#
    )
}

#[tokio::test]
async fn only_a_stack_that_is_not_there_refuses_a_signal() {
    let router = router();
    // A stack that declares no wait condition at all: its resource is created and waits
    // for nothing.
    let plain = r#"{
        "Resources": {
            "Topic": {
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": "settled-topic"}
            }
        }
    }"#;
    let (status, body) = create_stack(&router, "settled", plain, &[]).await;
    assert_eq!(status, 200, "expected CreateStack to succeed, got {body}");
    wait_for_the_transition().await;

    // A stack that IS waiting, so the refusal cases can be taken against the same router.
    let waiting = one_wait_condition("waiting", "300", "1");
    let (status, body) = create_stack(&router, "waiting", &waiting, &[]).await;
    assert_eq!(status, 200, "expected CreateStack to succeed, got {body}");

    let cases: Vec<(&str, &str, &str)> = vec![
        ("a stack name that names no stack", "no-such-stack", "Wait"),
        (
            "a stack id that names no stack in this Region",
            "arn:aws:cloudformation:us-east-1:000000000000:stack/gone/0000",
            "Wait",
        ),
    ];
    for (case, stack, logical_id) in cases {
        let (status, body) = signal(&router, stack, logical_id, "any-id", "SUCCESS").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the signal to be refused, got {status} {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected a ValidationError, got {code:?} in {body}"
        );
    }

    // A signal nothing is waiting for is accepted and recorded, and releases nothing: the
    // operation waits on its wait condition and on nothing else.
    let accepted: Vec<(&str, &str, &str)> = vec![
        ("a resource the stack never declared", "waiting", "Nope"),
        (
            "a resource of a waiting stack that is not the wait condition",
            "waiting",
            "Gate",
        ),
        ("a resource of a settled stack", "settled", "Topic"),
    ];
    for (case, stack, logical_id) in accepted {
        let (status, body) = signal(&router, stack, logical_id, "any-id", "SUCCESS").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the signal to be accepted, got {status} {body}"
        );
    }
    wait_for_the_transition().await;
    let reported_status = status_of(&router, "waiting").await;
    assert_eq!(
        reported_status, "CREATE_IN_PROGRESS",
        "expected a signal nothing was waiting for to leave the create waiting at its wait \
         condition, got {reported_status:?}"
    );
    let settled_status = status_of(&router, "settled").await;
    assert_eq!(
        settled_status, "CREATE_COMPLETE",
        "expected a signal to a completed stack to change nothing, got {settled_status:?}"
    );

    // The signal the wait condition itself is waiting for releases it, and a further one
    // after that changes nothing.
    let (status, _) = signal(&router, "waiting", "Wait", "released", "SUCCESS").await;
    assert_eq!(status, 200, "expected the signal to be accepted");
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "waiting").await,
        "CREATE_COMPLETE",
        "expected the signalled create to complete"
    );
    let (status, body) = signal(&router, "waiting", "Wait", "late", "SUCCESS").await;
    assert_eq!(
        status, 200,
        "expected a signal after the wait condition was satisfied to be accepted, got \
         {status} {body}"
    );
    assert_eq!(
        status_of(&router, "waiting").await,
        "CREATE_COMPLETE",
        "expected a signal after the create completed to change nothing"
    );
}

#[tokio::test]
async fn a_failure_signal_fails_the_create_and_rolls_it_back() {
    let router = router();
    let template = one_wait_condition("failing", "300", "2");
    let (status, body) = create_stack(&router, "failing", &template, &[]).await;
    assert_eq!(status, 200, "expected CreateStack to succeed, got {body}");

    let (status, body) = signal(&router, "failing", "Wait", "host-x", "FAILURE").await;
    assert_eq!(
        status, 200,
        "expected the failure signal to be accepted, got {status} {body}"
    );

    // "A failure signal causes CloudFormation to immediately fail the stack creation or
    // update": the create does not wait for its remaining signals. The stack's reason
    // summarises which resource failed it; the signal that did so is reported on that
    // resource's own event.
    let reason = status_reason_of(&router, "failing").await;
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Wait"]),
        "expected the failure reason to summarise the resource that failed the create, got \
         {reason:?}"
    );
    let on_the_resource = resource_event_reason(
        &router,
        "failing",
        "Wait",
        resource_status::CREATE_FAILED,
    )
    .await;
    assert!(
        on_the_resource.contains("host-x"),
        "expected the resource's own event to name the signal that failed it, got \
         {on_the_resource:?}"
    );
    wait_for_the_transition().await;
    let reported_status = status_of(&router, "failing").await;
    assert_eq!(
        reported_status, "ROLLBACK_COMPLETE",
        "expected the failed create to be rolled back, got {reported_status:?}"
    );
}

#[tokio::test]
async fn a_wait_condition_whose_timeout_runs_out_fails_the_operation() {
    // Each row's `Timeout` is the documented one in SECONDS, so the case waits that long
    // before the operation can have failed. `0` is the degenerate bound: the wait is over
    // the moment it starts.
    let cases: Vec<(&str, &str, &str, u64)> = vec![
        ("a timeout that is over the moment it starts", "expired", "0", 0),
        ("a timeout of one second", "one-second", "1", 1_300),
    ];
    for (case, stack, timeout, wait_ms) in cases {
        let router = router();
        let template = one_wait_condition(stack, timeout, "2");
        let (status, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to succeed, got {status} {body}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

        // The create FAILS when the wait runs out and is then rolled back, which is what
        // `OnFailure`'s documented default asks for: the status a waiter ends on is the
        // rolled-back one, reached on the clock with nothing the client does.
        let reported_status = status_of(&router, stack).await;
        assert!(
            reported_status == "ROLLBACK_IN_PROGRESS" || reported_status == "ROLLBACK_COMPLETE",
            "case {case:?}: expected the create that ran out of wait to be rolling back, got \
             {reported_status:?}"
        );
        wait_for_the_transition().await;
        let settled = status_of(&router, stack).await;
        assert_eq!(
            settled, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the create to roll back once the wait ran out, got \
             {settled:?}"
        );
        let reason = status_reason_of(&router, stack).await;
        assert!(
            reason.contains("Received 0 conditions when expecting 2"),
            "case {case:?}: expected the reason to carry the counts the wait was judged on, \
             got {reason:?}"
        );
    }
}

#[tokio::test]
async fn the_expiry_is_concluded_whichever_operation_is_called_after_it() {
    // The expiry and the rollback it owes are materialised by ONE path, driven by every
    // operation that settles the store — so the stack a client reads is the rolled-back one
    // whether the first call after the wait ran out is a describe, a template read or a
    // mutating operation. `GetTemplate` is the row that reads nothing about the stack's
    // status at all, and `SignalResource` the row that writes.
    let cases: [(&str, &str, &str); 3] = [
        ("GetTemplate, which reports no status", "concluded-template", "GetTemplate"),
        ("SignalResource, a mutating operation", "concluded-signal", "SignalResource"),
        ("RollbackStack, which acts on the failure", "concluded-rollback", "RollbackStack"),
    ];
    for (case, stack, first_call) in cases {
        let router = router();
        let template = one_wait_condition(stack, "0", "1");
        let (status, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to succeed, got {status} {body}"
        );

        // The wait is over the moment it starts, so the failure and its rollback are already
        // owed when this call arrives.
        let members = match first_call {
            "SignalResource" => vec![
                member("StackName", stack),
                member("LogicalResourceId", "Wait"),
                member("UniqueId", "late"),
                member("Status", "SUCCESS"),
            ],
            _ => vec![member("StackName", stack)],
        };
        let (status, body) = call(&router, first_call, &members).await;
        assert!(
            status == 200 || status == 400,
            "case {case:?}: expected {first_call} to be answered, got {status} {body}"
        );
        wait_for_the_transition().await;
        let settled = status_of(&router, stack).await;
        assert!(
            settled == "ROLLBACK_COMPLETE" || settled == "ROLLBACK_IN_PROGRESS",
            "case {case:?}: expected the create whose wait ran out to be rolling back after \
             {first_call}, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn an_update_waits_again_only_where_the_wait_condition_changed() {
    let router = router();
    let template = one_wait_condition("updated", "300", "1");
    let (status, body) = create_stack(&router, "updated", &template, &[]).await;
    assert_eq!(status, 200, "expected CreateStack to succeed, got {body}");
    let (status, _) = signal(&router, "updated", "Wait", "created", "SUCCESS").await;
    assert_eq!(status, 200, "expected the signal to be accepted");
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, "updated").await,
        "CREATE_COMPLETE",
        "expected the signalled create to complete"
    );

    // An update that declares the same wait condition is not held at it a second time:
    // "you can't update a wait condition that has already been satisfied unless you also
    // change the handle".
    let unchanged = r#"{
        "Resources": {
            "Gate": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
            "Wait": {
                "Type": "AWS::CloudFormation::WaitCondition",
                "Properties": {
                    "Handle": {"Ref": "Gate"},
                    "Timeout": "300",
                    "Count": "1"
                }
            },
            "After": {
                "Type": "AWS::SNS::Topic",
                "DependsOn": "Wait",
                "Properties": {"TopicName": "updated-after"}
            },
            "Extra": {
                "Type": "AWS::SNS::Topic",
                "Properties": {"TopicName": "updated-extra"}
            }
        }
    }"#;
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated"),
            member("TemplateBody", unchanged),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected UpdateStack to succeed, got {body}");
    wait_for_the_transition().await;
    let reported_status = status_of(&router, "updated").await;
    assert_eq!(
        reported_status, "UPDATE_COMPLETE",
        "expected an update that re-declares a satisfied wait condition to complete \
         without another signal, got {reported_status:?}"
    );

    // One that CHANGES it is a wait condition that has not been satisfied: the update
    // waits at it, and the two signals it now asks for release it.
    let changed = unchanged.replace(r#""Count": "1""#, r#""Count": "2""#);
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated"),
            member("TemplateBody", &changed),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected UpdateStack to succeed, got {body}");
    wait_for_the_transition().await;
    let reported_status = status_of(&router, "updated").await;
    assert_eq!(
        reported_status, "UPDATE_IN_PROGRESS",
        "expected the update to wait at the changed wait condition, got {reported_status:?}"
    );
    let actual = reported(&router, "updated").await;
    assert!(
        actual.iter().any(|(logical_id, status)| logical_id == "Wait"
            && status == resource_status::UPDATE_IN_PROGRESS),
        "expected the wait condition of an update to be reported UPDATE_IN_PROGRESS, got \
         {actual:?}"
    );
    let reason = reason_for(&router, "updated", "Wait").await;
    assert_eq!(
        reason,
        Some("Resource update Initiated".to_string()),
        "expected the wait condition an update is waiting at to report that its update was \
         initiated, got {reason:?}"
    );
    // An update cannot be started while one is in flight, which is what a client reads
    // while the barrier holds.
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated"),
            member("TemplateBody", unchanged),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a second update to be refused while the first waits, got {status} {body}"
    );

    for unique_id in ["host-1", "host-2"] {
        let (status, body) = signal(&router, "updated", "Wait", unique_id, "SUCCESS").await;
        assert_eq!(
            status, 200,
            "expected signal {unique_id:?} to be accepted, got {status} {body}"
        );
    }
    wait_for_the_transition().await;
    let reported_status = status_of(&router, "updated").await;
    assert_eq!(
        reported_status, "UPDATE_COMPLETE",
        "expected the signalled update to complete, got {reported_status:?}"
    );
}
