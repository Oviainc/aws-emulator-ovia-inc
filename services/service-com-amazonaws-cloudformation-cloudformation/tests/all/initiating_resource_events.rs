// SPDX-License-Identifier: CC0-1.0

//! The event that reports a resource's work was INITIATED, end to end over the awsQuery
//! wire.
//!
//! RULE: every `StackEvent` a stack operation records for one resource BETWEEN the event
//! that opens that resource's transition and the one that ends it reports work the service
//! that owns the resource has ALREADY been asked for, so it falls due at the START of the
//! span the operation spends on that resource — the second `CREATE_IN_PROGRESS`, whose
//! `ResourceStatusReason` is `Resource creation Initiated`, is in the log from the moment
//! the create was asked for, whatever ends the resource's creation and however long that
//! takes. A client therefore reads "CloudFormation accepted the template and the provider
//! is working" off the resource's own event stream for the whole time the resource is being
//! provisioned, instead of reading a resource that jumps from scheduled straight to
//! complete.
//!
//! Without it there is no observable point at which a resource is being provisioned: a
//! deployment dashboard, a `CREATE_IN_PROGRESS` waiter and anything tracking which resource
//! a stack is working on see a reasonless record and then a completion.
//!
//! The rows differ along what ENDS the resource's creation, which is what the rule
//! generalises over: a create the clock ends (two owning services, one of which answers the
//! create while the database it made is still coming up) and one only a resource signal
//! ends, where no instant interpolated from the end of the resource's span could have
//! produced the record at all. Every case drives the real request path through the router
//! `stack_lifecycle.rs` builds, so what is asserted is what a client calling
//! `cloudformation describe-stack-events` sees.

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};
use crate::stack_resources::element;

/// The fixed text CloudFormation records on the second `CREATE_IN_PROGRESS` event of a
/// resource it has asked an owning service to create. Entirely fixed: no part of it comes
/// from the request.
const RESOURCE_CREATION_INITIATED: &str = "Resource creation Initiated";

/// One `StackEvent` as the wire reports it, reduced to what this suite reads.
#[derive(Debug, Clone)]
struct Event {
    logical_id: String,
    status: String,
    reason: Option<String>,
}

/// Every event a stack has recorded, newest first as `DescribeStackEvents` answers them.
async fn events_of(router: &MultiServiceRouter, stack: &str) -> Vec<Event> {
    let (status_code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| Event {
            logical_id: element(entry, "LogicalResourceId").unwrap_or_default(),
            status: element(entry, "ResourceStatus").unwrap_or_default(),
            reason: element(entry, "ResourceStatusReason"),
        })
        .collect()
}

/// The events one resource has, newest first.
async fn of_resource(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Vec<Event> {
    events_of(router, stack)
        .await
        .into_iter()
        .filter(|event| event.logical_id == logical_id)
        .collect()
}

/// The place of the resource's event in `status` carrying `reason` in the newest-first list.
fn position(events: &[Event], status: &str, reason: Option<&str>) -> Option<usize> {
    events
        .iter()
        .position(|event| event.status == status && event.reason.as_deref() == reason)
}

/// The `StackStatus` a stack reports right now.
async fn status_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (status_code, body) = describe_stack(router, stack).await;
    assert_eq!(
        status_code, 200,
        "DescribeStacks({stack}) must answer 200, got {status_code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// `SignalResource` for one resource of one stack.
async fn signal(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    unique_id: &str,
) -> (u16, String) {
    call(
        router,
        "SignalResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
            member("UniqueId", unique_id),
            member("Status", "SUCCESS"),
        ],
    )
    .await
}

/// What ends the creation of the resource a row is about, which is what decides WHEN the
/// row reads the log: the initiating record is due as the walk reaches the resource either
/// way, and the two differ only in when the walk reaches it and what ends it afterwards.
enum Ends {
    /// The clock: nothing holds the resource up, so the walk reaches it as the operation
    /// starts and its initiating record is in the log by the time `CreateStack` has
    /// answered.
    Clock,
    /// A resource signal under this `UniqueId`, and nothing else. The walk reaches the
    /// resource behind the handle it refers to, and from then until the signal arrives the
    /// initiating record is the whole of what the resource reports.
    Signal(&'static str),
}

/// One row: a template, the resource whose initiating event it is about, and what ends that
/// resource's creation.
struct Case {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    logical_id: &'static str,
    ends: Ends,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // Amazon RDS answers `CreateDBInstance` while the database it made is still
            // coming up, so this is the row whose provider is really working after the
            // create was asked for.
            case: "a database the stack asks Amazon RDS for",
            stack: "initiated-database",
            template: r#"{"Resources":{"Database":{"Type":"AWS::RDS::DBInstance",
                "DeletionPolicy":"Delete",
                "Properties":{"DBInstanceClass":"db.t3.micro","AllocatedStorage":"20",
                  "Engine":"postgres","MasterUsername":"produser",
                  "MasterUserPassword":"an-example-master-password"}}}}"#,
            logical_id: "Database",
            ends: Ends::Clock,
        },
        Case {
            case: "a topic the stack asks Amazon SNS for",
            stack: "initiated-topic",
            template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
                "Properties":{"TopicName":"initiated-topic-name"}}}}"#,
            logical_id: "Topic",
            ends: Ends::Clock,
        },
        Case {
            // Nothing but a signal ends this resource's creation, so its own span has no
            // end for an instant to be interpolated from while it holds.
            case: "a wait condition held unsignalled",
            stack: "initiated-gate",
            template: r#"{"Resources":{
                "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
                "Gate":{"Type":"AWS::CloudFormation::WaitCondition",
                  "Properties":{"Handle":{"Ref":"Handle"},"Timeout":"300","Count":"1"}}}}"#,
            logical_id: "Gate",
            ends: Ends::Signal("released"),
        },
    ]
}

#[tokio::test]
async fn a_resource_being_provisioned_reports_that_its_creation_was_initiated() {
    for case in cases() {
        let Case {
            case,
            stack,
            template,
            logical_id,
            ends,
        } = case;
        let router = router();
        let (status_code, body) = create_stack(&router, stack, template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to succeed, got {status_code} {body}"
        );
        if matches!(ends, Ends::Signal(_)) {
            // The walk reaches this resource behind the handle it refers to, and past the
            // instant the clock alone would have completed the operation it is still being
            // created: nothing but its signal ends that.
            wait_for_the_transition().await;
        }

        // The create has been ASKED for, so the record saying so is in the log — read with
        // no wait of its own, exactly as a client polling the stack reads it.
        let events = of_resource(&router, stack, logical_id).await;
        let initiated = position(
            &events,
            resource_status::CREATE_IN_PROGRESS,
            Some(RESOURCE_CREATION_INITIATED),
        );
        assert_eq!(
            initiated,
            Some(0),
            "case {case:?}: expected {logical_id} to report \
             {} {RESOURCE_CREATION_INITIATED:?} as the newest thing said about it once its \
             create had been asked for, got {events:?}",
            resource_status::CREATE_IN_PROGRESS
        );
        // Exactly one such record per create: the resource's creation was initiated once.
        assert_eq!(
            events
                .iter()
                .filter(|event| event.reason.as_deref() == Some(RESOURCE_CREATION_INITIATED))
                .count(),
            1,
            "case {case:?}: expected exactly one initiating event for {logical_id}, got \
             {events:?}"
        );
        // It comes after the event that OPENED the resource's transition, which carries no
        // reason: the log is answered newest first, so the opening record is further down
        // it.
        let opened = position(&events, resource_status::CREATE_IN_PROGRESS, None);
        assert!(
            opened > initiated,
            "case {case:?}: expected {logical_id}'s initiating event to be newer than the \
             event that opened its transition, got {events:?}"
        );

        if let Ends::Signal(unique_id) = ends {
            // ...and the creation it reports has not finished, because what would finish it
            // has not arrived: the record is the resource's answer for the whole of the
            // wait, which no instant measured back from the end of that wait could have
            // produced.
            assert!(
                position(&events, resource_status::CREATE_COMPLETE, None).is_none(),
                "case {case:?}: {logical_id}'s creation has not finished, so no event may \
                 say it did, got {events:?}"
            );
            let (status_code, body) = signal(&router, stack, logical_id, unique_id).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the signal to be accepted, got {status_code} {body}"
            );
        }

        wait_for_the_transition().await;
        let reported = status_of(&router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the create to complete, got {reported:?}"
        );

        // The resource's whole sequence, oldest last: the transition opened, the create was
        // initiated, and only then did the resource finish.
        let events = of_resource(&router, stack, logical_id).await;
        let completed = position(&events, resource_status::CREATE_COMPLETE, None);
        let initiated = position(
            &events,
            resource_status::CREATE_IN_PROGRESS,
            Some(RESOURCE_CREATION_INITIATED),
        );
        let opened = position(&events, resource_status::CREATE_IN_PROGRESS, None);
        assert!(
            completed.is_some() && completed < initiated && initiated < opened,
            "case {case:?}: expected {logical_id}'s events to run newest-first from its \
             completion through its initiating event to the record that opened its \
             transition, got {events:?}"
        );
    }
}
