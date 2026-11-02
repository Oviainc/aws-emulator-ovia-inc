// SPDX-License-Identifier: CC0-1.0

//! What a stack reports about ONE resource while the operation that is acting on it is
//! still in flight, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`StackResource$ResourceStatus` is the
//! status of THAT resource, so it names the action the operation is performing on the
//! resource — a resource the operation brings into the stack is reported `CREATE_IN_PROGRESS`
//! and then `CREATE_COMPLETE` even when the stack-level operation is an UPDATE, and only a
//! resource the stack already holds is reported `UPDATE_IN_PROGRESS` — while the stack itself
//! reports the operation's own status and refuses a second operation on it.**
//!
//! The barrier the rows are read through is an `AWS::CloudFormation::WaitCondition`, which is
//! what holds an operation open long enough for a client to read the resource it is waiting
//! at. The rows differ along what the rule generalises over: whether the revision ADDS the
//! gate or re-declares one the stack already has, and which read reports it (the single
//! `DescribeStackResource` and the whole-stack `DescribeStackResources`). The competing
//! operation, the refusal's own wording and the post-signal completion are the same rule read
//! from the stack's side.

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, stack_cannot_be_updated, status,
};
use smithy_server_runtime::MultiServiceRouter;

/// A template with one topic, and optionally a wait condition gating nothing else.
fn template(topic: &str, gate: Option<&str>) -> String {
    let gate = gate.map_or_else(String::new, |count| {
        format!(
            r#","Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
               "Gate":{{"Type":"AWS::CloudFormation::WaitCondition",
                        "Properties":{{"Handle":{{"Ref":"Handle"}},"Timeout":"300","Count":"{count}"}}}}"#
        )
    });
    format!(
        r#"{{"Resources":{{"Topic":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{topic}"}}}}{gate}}}}}"#
    )
}

/// The `ResourceStatus` and `ResourceStatusReason` `DescribeStackResource` reports for one
/// resource.
async fn resource_detail(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> (String, String) {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackResource to answer for {logical_id:?}, got \
         {code} {body}"
    );
    (
        xml_text(&body, "ResourceStatus").unwrap_or_default(),
        xml_text(&body, "ResourceStatusReason").unwrap_or_default(),
    )
}

/// The `ResourceStatus` the whole-stack read reports for one resource.
///
/// Shared with `unsignalled_gate_hold.rs`, which reads the same member through the same
/// operation, so the two cannot drift about how the whole-stack read is taken.
pub(crate) async fn resource_status_in_stack(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {code} {body}"
    );
    xml_text(&body, "ResourceStatus").unwrap_or_default()
}

/// The `StackStatus` a stack reports right now.
async fn status_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(code, 200, "DescribeStacks {stack:?}: body {body}");
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The stack id a stack reports, which the refusal of a competing operation names it by.
async fn stack_id_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = describe_stack(router, stack).await;
    xml_text(&body, "StackId").unwrap_or_default()
}

/// One row: whether the revision adds the gate or re-declares one the stack already has, and
/// the resource status the operation in flight must report for it.
struct GateCase {
    case: &'static str,
    stack: &'static str,
    topic: &'static str,
    /// The gate the CREATE declares, if any: its `Count`.
    created_gate: Option<&'static str>,
    /// The gate the revision declares: its `Count`.
    revised_gate: &'static str,
    /// Signals the create needs before it completes, under their own unique ids.
    create_signals: &'static [&'static str],
    /// The resource status the operation in flight reports for the gate, and the reason.
    in_flight: (&'static str, &'static str),
    /// The resource status the gate carries once the operation completes.
    completed: &'static str,
}

fn gate_cases() -> Vec<GateCase> {
    vec![
        GateCase {
            case: "an update that ADDS the gate reports it as being created",
            stack: "flight-added",
            topic: "flight-added-topic",
            created_gate: None,
            revised_gate: "1",
            create_signals: &[],
            in_flight: (
                resource_status::CREATE_IN_PROGRESS,
                "Resource creation Initiated",
            ),
            completed: resource_status::CREATE_COMPLETE,
        },
        GateCase {
            case: "an update that CHANGES a gate the stack already holds reports an update",
            stack: "flight-changed",
            topic: "flight-changed-topic",
            created_gate: Some("1"),
            revised_gate: "2",
            create_signals: &["created"],
            in_flight: (
                resource_status::UPDATE_IN_PROGRESS,
                "Resource update Initiated",
            ),
            completed: resource_status::UPDATE_COMPLETE,
        },
    ]
}

/// `SignalResource` for the gate of one stack.
async fn signal(router: &MultiServiceRouter, stack: &str, unique_id: &str) -> (u16, String) {
    call(
        router,
        "SignalResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", "Gate"),
            member("UniqueId", unique_id),
            member("Status", "SUCCESS"),
        ],
    )
    .await
}

#[tokio::test]
async fn a_resource_in_flight_reports_the_action_being_performed_on_it() {
    for case in gate_cases() {
        let GateCase {
            case,
            stack,
            topic,
            created_gate,
            revised_gate,
            create_signals,
            in_flight,
            completed,
        } = case;
        let router = router();
        let (code, body) =
            create_stack(&router, stack, &template(topic, created_gate), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to succeed, got {code} {body}"
        );
        for unique_id in create_signals {
            let (code, body) = signal(&router, stack, unique_id).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected the create's signal to be accepted, got {code} {body}"
            );
        }
        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?}"
        );

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &template(topic, Some(revised_gate))),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected UpdateStack to succeed, got {code} {body}"
        );
        wait_for_the_transition().await;

        // The stack reports the operation it is performing...
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_IN_PROGRESS,
            "case {case:?}: expected the update to be held at the wait condition, got \
             {reported:?}"
        );
        // ...and the gate reports what is being done to the GATE.
        let detail = resource_detail(case, &router, stack, "Gate").await;
        assert_eq!(
            (detail.0.as_str(), detail.1.as_str()),
            in_flight,
            "case {case:?}: expected the gate of the operation in flight to report \
             {in_flight:?}, got {detail:?}"
        );
        let in_stack = resource_status_in_stack(case, &router, stack, "Gate").await;
        assert_eq!(
            in_stack, in_flight.0,
            "case {case:?}: expected the whole-stack read to agree with the single resource \
             read, got {in_stack:?}"
        );

        // A second operation on a stack that already has one in flight is refused, and the
        // refusal names the stack and the status it is in.
        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", stack),
                member("TemplateBody", &template(topic, Some(revised_gate))),
            ],
        )
        .await;
        assert_eq!(
            code, 400,
            "case {case:?}: expected a competing update to be refused, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the service's ValidationError, got {body}"
        );
        let stack_id = stack_id_of(&router, stack).await;
        assert_eq!(
            xml_text(&body, "Message"),
            Some(stack_cannot_be_updated(&stack_id, status::UPDATE_IN_PROGRESS)),
            "case {case:?}: expected the refusal to name the stack and its status, got {body}"
        );
        // The refusal disturbed nothing: the update is still in flight at its gate.
        let detail = resource_detail(case, &router, stack, "Gate").await;
        assert_eq!(
            detail.0, in_flight.0,
            "case {case:?}: expected the refused update to leave the gate as it was, got \
             {detail:?}"
        );

        // The signals the revision asks for release it, and the gate then carries the
        // completion of the action that was being performed on it.
        for index in 0..revised_gate.parse::<usize>().expect("a declared count") {
            let unique_id = format!("released-{index}");
            let (code, body) = signal(&router, stack, &unique_id).await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected signal {unique_id:?} to be accepted, got {code} {body}"
            );
        }
        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::UPDATE_COMPLETE,
            "case {case:?}: expected the signalled update to complete, got {reported:?}"
        );
        let detail = resource_detail(case, &router, stack, "Gate").await;
        assert_eq!(
            detail.0, completed,
            "case {case:?}: expected the completed gate to report {completed:?}, got {detail:?}"
        );
    }
}
