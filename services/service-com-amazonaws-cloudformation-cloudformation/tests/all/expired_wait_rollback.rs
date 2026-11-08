// SPDX-License-Identifier: CC0-1.0

//! A stack operation whose RESOURCE-SIGNAL WAIT runs out is FAILED by the clock and then
//! ROLLED BACK, and the rollback itself waits at the wait conditions the configuration it
//! puts back declares again.
//!
//! RULE: "CloudFormation doesn't proceed with a stack creation or update until resources
//! receive the required number of signals or the timeout period is exceeded"
//! (`SignalResource`), and "if the wait condition doesn't receive the specified number of
//! success signals before the `Timeout` period expires, CloudFormation assumes that the wait
//! condition has failed" — so the operation FAILS, and a failed operation does not stop
//! there:
//!
//! * "ROLLBACK — Roll back the stack creation. This is the default"
//!   (`CreateStackInput$OnFailure`): the create goes through `ROLLBACK_IN_PROGRESS` to
//!   `ROLLBACK_COMPLETE` and the resources it had already made are deleted in the services
//!   that own them;
//! * "if the stack update fails, CloudFormation rolls back the stack to the previous stack
//!   configuration": the update goes through `UPDATE_ROLLBACK_IN_PROGRESS` to
//!   `UPDATE_ROLLBACK_COMPLETE`, leaving the resources it did not touch alone — a resource
//!   the template re-declares unchanged is not created again, whether or not any service here
//!   provisions its type;
//! * a wait condition the ROLLBACK deploys again is not the satisfied one the stack had ("you
//!   can't update a wait condition that has already been satisfied unless you also change the
//!   handle"), so it waits for its own signals and a rollback whose wait runs out leaves the
//!   stack in `UPDATE_ROLLBACK_FAILED` — the state `ContinueUpdateRollback` resumes from.
//!
//! Nothing a client does is needed for any of it: the expiry is the clock's, and the rollback
//! is what the failure owes. Every case drives the whole sequence over the awsQuery wire, and
//! reads the resources back through the service that OWNS them (AWS Systems Manager), so what
//! it asserts is what a client sees rather than what this store recorded.
//!
//! The rows span what the rule generalises over: which declaration opened the wait (an
//! `AWS::CloudFormation::WaitCondition` and a `CreationPolicy`'s `ResourceSignal`), how the
//! declaration spells `Timeout` (as a string, as a number and as an ISO 8601 duration) and
//! `Count` (one signal and two), and how many of the signals it asks for arrive (none, and
//! one of two).

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::stack_lifecycle::{call, create_stack, member, router, wait_for_the_transition};
use crate::update_rollback_recovery::{
    parameter_value, stack_events, status_of, reason_of, resource_status_of,
};
use crate::wait_conditions::signal;

/// How long a case waits past a declaration's own span before reading the outcome: the
/// expiry is due on the clock, and the status the failure owes falls due one transition
/// later.
fn past(span: Duration) -> Duration {
    span + Duration::from_millis(900)
}

/// One row of the create table: the declaration that opens the wait, the span it asks for,
/// and the reason its expiry is worded with.
struct ExpiredCreate {
    case: &'static str,
    stack: &'static str,
    /// The template, which always declares a SIBLING parameter created BEFORE the gate: the
    /// rollback has to delete it, which is what proves the rollback reached the owning
    /// service.
    template: String,
    /// The logical id the wait is declared on.
    gate: &'static str,
    /// The span the declaration asks to wait for.
    asked_for: Duration,
    /// Signals the case delivers before the span is over — fewer than the declaration asks
    /// for in every row, which is what leaves the wait unsatisfied.
    signals: &'static [&'static str],
    /// The reason the stack reports the failure with, as CloudFormation words it for that
    /// declaration.
    reason: String,
    /// The name of the sibling parameter the rollback has to delete.
    sibling: &'static str,
}

/// The class the rule covers: a create whose wait runs out, however the wait was declared.
fn expired_creates() -> Vec<ExpiredCreate> {
    vec![
        ExpiredCreate {
            case: "a wait condition asking for one signal, Timeout as a string",
            stack: "expired-one",
            gate: "Wait",
            asked_for: Duration::from_secs(1),
            signals: &[],
            reason: "WaitCondition timed out. Received 0 conditions when expecting 1"
                .to_string(),
            sibling: "/expired-one/sibling",
            template: json!({
                "Resources": {
                    "Sibling": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/expired-one/sibling",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "DependsOn": "Sibling",
                        "Properties": {
                            "Handle": {"Ref": "Handle"},
                            "Timeout": "1",
                            "Count": "1",
                        },
                    },
                },
            })
            .to_string(),
        },
        ExpiredCreate {
            case: "a wait condition asking for two signals with one sent, Timeout as a number",
            stack: "expired-partial",
            gate: "Wait",
            asked_for: Duration::from_secs(2),
            signals: &["host-1"],
            reason: "WaitCondition timed out. Received 1 conditions when expecting 2"
                .to_string(),
            sibling: "/expired-partial/sibling",
            template: json!({
                "Resources": {
                    "Sibling": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/expired-partial/sibling",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
                    "Wait": {
                        "Type": "AWS::CloudFormation::WaitCondition",
                        "DependsOn": "Sibling",
                        "Properties": {
                            "Handle": {"Ref": "Handle"},
                            "Timeout": 2,
                            "Count": 2,
                        },
                    },
                },
            })
            .to_string(),
        },
        ExpiredCreate {
            case: "a CreationPolicy's ResourceSignal, Timeout as an ISO 8601 duration",
            stack: "expired-policy",
            gate: "Gated",
            asked_for: Duration::from_secs(1),
            signals: &[],
            reason: "Failed to receive 1 resource signal(s) within the specified duration"
                .to_string(),
            sibling: "/expired-policy/sibling",
            template: json!({
                "Resources": {
                    "Sibling": {
                        "Type": "AWS::SSM::Parameter",
                        "Properties": {
                            "Name": "/expired-policy/sibling",
                            "Type": "String",
                            "Value": "made-by-the-create",
                        },
                    },
                    "Gated": {
                        "Type": "AWS::SSM::Parameter",
                        "DependsOn": "Sibling",
                        "CreationPolicy": {
                            "ResourceSignal": {"Count": 1, "Timeout": "PT1S"},
                        },
                        "Properties": {
                            "Name": "/expired-policy/gated",
                            "Type": "String",
                            "Value": "waits-for-a-signal",
                        },
                    },
                },
            })
            .to_string(),
        },
    ]
}

#[tokio::test]
async fn a_create_whose_wait_runs_out_rolls_back_and_removes_what_it_made() {
    for row in expired_creates() {
        let case = row.case;
        let router = router();
        let (http_status, body) = create_stack(&router, row.stack, &row.template, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
        );
        for unique_id in row.signals {
            let (http_status, body) =
                signal(&router, row.stack, row.gate, unique_id, "SUCCESS").await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected signal {unique_id:?} to be accepted, got \
                 {http_status} {body}"
            );
        }
        tokio::time::sleep(past(row.asked_for)).await;

        // The wait ran out: the create failed and the rollback its `OnFailure` default asks
        // for is what the stack settles in.
        let settled = status_of(&router, row.stack).await;
        if settled == status::ROLLBACK_IN_PROGRESS {
            wait_for_the_transition().await;
        }
        let settled = status_of(&router, row.stack).await;
        assert_eq!(
            settled,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: expected the create whose wait ran out to roll back, got \
             {settled:?}"
        );
        let reason = reason_of(&router, row.stack).await;
        assert_eq!(
            reason, row.reason,
            "case {case:?}: expected the stack to report {:?}, got {reason:?}",
            row.reason
        );

        // The resource the wait was for reports the failure, and the stack's own events
        // carry the two transitions the rollback went through.
        let events = stack_events(case, &router, row.stack).await;
        let failed_resource = (row.gate.to_string(), resource_status::CREATE_FAILED.to_string());
        assert!(
            events.contains(&failed_resource),
            "case {case:?}: expected the resource the wait was for to report \
             {failed_resource:?}, got {events:?}"
        );
        for expected in [status::ROLLBACK_IN_PROGRESS, status::ROLLBACK_COMPLETE] {
            let stack_event = (row.stack.to_string(), expected.to_string());
            assert!(
                events.contains(&stack_event),
                "case {case:?}: expected the stack's own {expected:?} event, got {events:?}"
            );
        }

        // ...and the sibling the create HAD made is gone from the service that owns it: a
        // rollback that reported itself complete without deleting it would leave a live,
        // name-claiming resource behind.
        let held = parameter_value(&router, row.sibling).await;
        assert_eq!(
            held, None,
            "case {case:?}: expected the rollback to delete the sibling {:?} the create had \
             made, got {held:?}",
            row.sibling
        );
    }
}

/// The stack an update row starts from: one parameter and a handle, neither of which the
/// update below touches.
const BEFORE_THE_UPDATE: &str = r#"{"Resources":{
    "Kept":{"Type":"AWS::SSM::Parameter",
      "Properties":{"Name":"/expired-update/kept","Type":"String","Value":"kept"}},
    "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// What the update deploys: the same two resources, plus a parameter it adds and a wait
/// condition nothing signals.
const THE_FAILING_UPDATE: &str = r#"{"Resources":{
    "Kept":{"Type":"AWS::SSM::Parameter",
      "Properties":{"Name":"/expired-update/kept","Type":"String","Value":"kept"}},
    "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "Added":{"Type":"AWS::SSM::Parameter",
      "Properties":{"Name":"/expired-update/added","Type":"String","Value":"added"}},
    "Wait":{"Type":"AWS::CloudFormation::WaitCondition",
      "DependsOn":"Added",
      "Properties":{"Handle":{"Ref":"Handle"},"Timeout":"1","Count":"1"}}}}"#;

#[tokio::test]
async fn an_update_whose_wait_runs_out_rolls_back_and_leaves_untouched_resources_alone() {
    let case = "an update that adds a parameter behind a wait nothing signals";
    let stack = "expired-update";
    let router = router();
    let (http_status, body) = create_stack(&router, stack, BEFORE_THE_UPDATE, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, stack).await,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete"
    );

    let token = "expired-update-token";
    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", THE_FAILING_UPDATE),
            member("ClientRequestToken", token),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected UpdateStack to succeed, got {http_status} {body}"
    );
    tokio::time::sleep(past(Duration::from_secs(1))).await;
    let settled = status_of(&router, stack).await;
    if settled == status::UPDATE_ROLLBACK_IN_PROGRESS {
        wait_for_the_transition().await;
    }
    let settled = status_of(&router, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the update whose wait ran out to roll back, got {settled:?}"
    );

    // The stack's own events end with the two transitions of its rollback, and the resources
    // the update did not touch are acted on NOWHERE in THIS operation's events: a resource
    // the template re-declares unchanged is not created again, which holds for the handle (a
    // type no service here provisions) exactly as it does for the parameter. The failure
    // itself is reported on the resource that failed and on the ROLLBACK's own event — a
    // stack rolled straight back never rests in `UPDATE_FAILED`, so no event of its own
    // reports that status. `DescribeStackEvents` reports newest first, so the operation's own
    // events are the ones up to and including the `UPDATE_IN_PROGRESS` that opened it.
    let whole_log = stack_events(case, &router, stack).await;
    let opened = whole_log
        .iter()
        .position(|event| {
            event == &(stack.to_string(), status::UPDATE_IN_PROGRESS.to_string())
        })
        .unwrap_or_else(|| panic!("case {case:?}: no UPDATE_IN_PROGRESS event, got {whole_log:?}"));
    let events = whole_log[..=opened].to_vec();
    for expected in [
        status::UPDATE_ROLLBACK_IN_PROGRESS,
        status::UPDATE_ROLLBACK_COMPLETE,
    ] {
        let stack_event = (stack.to_string(), expected.to_string());
        assert!(
            events.contains(&stack_event),
            "case {case:?}: expected the stack's own {expected:?} event, got {events:?}"
        );
    }
    let rested_in_the_failure = (stack.to_string(), status::UPDATE_FAILED.to_string());
    assert!(
        !events.contains(&rested_in_the_failure),
        "case {case:?}: expected no stack-level {:?} event for an update that rolled \
         straight back, got {events:?}",
        status::UPDATE_FAILED
    );
    for untouched in ["Kept", "Handle"] {
        for forbidden in [
            resource_status::CREATE_IN_PROGRESS,
            resource_status::CREATE_COMPLETE,
            resource_status::UPDATE_COMPLETE,
        ] {
            let event = (untouched.to_string(), forbidden.to_string());
            assert!(
                !events.contains(&event),
                "case {case:?}: expected no {forbidden:?} event for the untouched \
                 {untouched:?}, got {events:?}"
            );
        }
    }

    // What the rollback left in the owning service: the parameter the update ADDED is gone
    // and the one it never touched still holds the value it was created with.
    let added = parameter_value(&router, "/expired-update/added").await;
    assert_eq!(
        added, None,
        "case {case:?}: expected the rollback to delete the parameter the update added, got \
         {added:?}"
    );
    let kept = parameter_value(&router, "/expired-update/kept").await;
    assert_eq!(
        kept,
        Some("kept".to_string()),
        "case {case:?}: expected the untouched parameter to keep its value, got {kept:?}"
    );
}

/// The stack the rollback table starts from: a wait condition that IS signalled, so the
/// create completes and the stack has a satisfied wait condition.
const SIGNALLED_WAIT: &str = r#"{"Resources":{
    "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "Wait":{"Type":"AWS::CloudFormation::WaitCondition",
      "Properties":{"Handle":{"Ref":"Handle"},"Timeout":"2","Count":"1"}}}}"#;

/// The update that fails: the same wait condition asking for a signal count it never
/// receives, so the update fails and its rollback has to deploy the declaration above again.
const CHANGED_WAIT: &str = r#"{"Resources":{
    "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "Wait":{"Type":"AWS::CloudFormation::WaitCondition",
      "Properties":{"Handle":{"Ref":"Handle"},"Timeout":"1","Count":"2"}}}}"#;

#[tokio::test]
async fn a_rollback_whose_wait_condition_is_deployed_again_waits_at_it_and_can_be_resumed() {
    let case = "a rollback that deploys a wait condition again";
    let stack = "expired-rollback";
    let router = router();
    let (http_status, body) = create_stack(&router, stack, SIGNALLED_WAIT, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
    );
    let (http_status, body) = signal(&router, stack, "Wait", "created", "SUCCESS").await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected the signal to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    assert_eq!(
        status_of(&router, stack).await,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the signalled create to complete"
    );

    // The update changes the wait condition, so it is no longer the satisfied one the stack
    // had: the signal that satisfied THAT one does not satisfy this one, and nothing signals
    // the new one before its span is over.
    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", CHANGED_WAIT),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected UpdateStack to succeed, got {http_status} {body}"
    );
    tokio::time::sleep(past(Duration::from_secs(1))).await;
    let rolling_back = status_of(&router, stack).await;
    assert_eq!(
        rolling_back,
        status::UPDATE_ROLLBACK_IN_PROGRESS,
        "case {case:?}: expected the failed update to be rolling back and WAITING at the wait \
         condition it deploys again, got {rolling_back:?}"
    );

    // The rollback's own wait runs out too, and a rollback that could not be carried through
    // leaves the stack where a resumed rollback acts on it.
    tokio::time::sleep(past(Duration::from_secs(2))).await;
    let failed = status_of(&router, stack).await;
    assert_eq!(
        failed,
        status::UPDATE_ROLLBACK_FAILED,
        "case {case:?}: expected the rollback whose wait ran out to leave the stack in \
         UPDATE_ROLLBACK_FAILED, got {failed:?}"
    );
    let reported = resource_status_of(&router, stack, "Wait").await;
    assert_eq!(
        reported,
        resource_status::UPDATE_FAILED,
        "case {case:?}: expected the resource the rollback stopped at to report UPDATE_FAILED, \
         the state a resumed rollback may be told to skip, got {reported:?}"
    );

    // "ContinueUpdateRollback … continues rolling back a stack from UPDATE_ROLLBACK_FAILED
    // to UPDATE_ROLLBACK_COMPLETE state", skipping the resource the rollback could not get
    // through.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", stack),
            member("ResourcesToSkip.member.1", "Wait"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected ContinueUpdateRollback to be accepted, got {http_status} \
         {body}"
    );
    wait_for_the_transition().await;
    let resumed = status_of(&router, stack).await;
    assert_eq!(
        resumed,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the resumed rollback to complete, got {resumed:?}"
    );
}
