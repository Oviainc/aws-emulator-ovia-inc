// SPDX-License-Identifier: CC0-1.0

//! `SignalResource`: what a resource signal does to the stack it names, over the awsQuery
//! wire.
//!
//! RULE: a signal is accepted and recorded for ANY logical id of a stack that resolves in
//! the Region the request was signed for — kept under that logical id and the signal's
//! `UniqueId`, so the same unique id sent again is the same signal — and it acts on the
//! stack only where something is waiting for it: while the stack's create or update is in
//! flight and the signalled resource's declaration waits for signals on THAT operation (a
//! `CreationPolicy`'s `ResourceSignal` for a create, an `UpdatePolicy`'s
//! `WaitOnResourceSignals` for an update, a wait condition for either), the signal is
//! reported as an event on that resource and a `FAILURE` signal fails the operation at
//! once, which a create then rolls back unless the request asked for the failed state to be
//! kept. Every other signal — a resource nothing waits on, a logical id the template
//! declares nothing under, a signal that arrives after the operation settled — is accepted
//! and changes nothing. Only a stack that does not resolve is refused, with HTTP 400
//! `ValidationError`.
//!
//! The rows differ along what the rule generalises over: the declaration form that waits,
//! the operation in flight, the signal's status, how the stack is addressed (name or id),
//! whether the signalled resource is declared at all, and whether the signal is the first
//! under its unique id.

use std::time::Instant;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, create_stack, describe_stack, member, router_with_engine, store,
    wait_for_the_transition, xml_blocks, xml_text, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_failure_summary, status, STATUS_TRANSITION_DELAY,
};

/// The logical id of the resource every template here declares as the one a signal is
/// about, so a row says what the declaration IS rather than what it is called.
const GATE: &str = "Gate";

/// How many reads a row that signals a SETTLED create gives the create to settle in: a stack
/// is owed its terminal status by the clock and by the services that own its resources, and a
/// read is what asks them.
const SETTLE_ATTEMPTS: usize = 20;

/// The Region no stack of this suite lives in: a stack name resolves only in the Region the
/// request is signed for.
const OTHER_REGION: &str = "us-west-2";

/// Which declaration form a row's template gives the signalled resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declared {
    /// A resource with no wait declared on it at all — a topic the stack really creates in
    /// Amazon SNS.
    NothingWaiting,
    /// `CreationPolicy: {ResourceSignal: …}`, which is what a CREATE waits on.
    CreationPolicy,
    /// `UpdatePolicy: {AutoScalingRollingUpdate: {WaitOnResourceSignals: true}}`, which is
    /// what an UPDATE waits on.
    UpdatePolicy,
    /// `AWS::CloudFormation::WaitCondition`, which waits for signals whichever operation
    /// deploys it.
    WaitCondition,
}

/// The template a row deploys: one resource under [`GATE`], declared the row's way.
///
/// The types are the ones AWS documents each attribute on — `CreationPolicy` applies to
/// `AWS::EC2::Instance`, `AWS::AutoScaling::AutoScalingGroup` and
/// `AWS::CloudFormation::WaitCondition`, and `AutoScalingRollingUpdate` to an Auto Scaling
/// group — so no row leans on an attribute the type does not take. `marker` goes into the
/// declaration so an update can change the template without changing the wait.
fn template(declared: Declared, marker: &str) -> String {
    let gate = match declared {
        Declared::NothingWaiting => json!({
            "Type": "AWS::SNS::Topic",
            "Properties": {"DisplayName": marker}
        }),
        Declared::CreationPolicy => json!({
            "Type": "AWS::EC2::Instance",
            "Properties": {
                "ImageId": "ami-0123456789abcdef0",
                // A published instance type: the stack LAUNCHES this instance in Amazon EC2,
                // which refuses a type it does not publish — so the row's own marker goes in
                // a tag, which is a declared property an update can change just as freely.
                "InstanceType": "t3.micro",
                "Tags": [{"Key": "Marker", "Value": marker}]
            },
            "CreationPolicy": {"ResourceSignal": {"Count": 1, "Timeout": "PT5M"}}
        }),
        Declared::UpdatePolicy => json!({
            "Type": "AWS::AutoScaling::AutoScalingGroup",
            "Properties": {"MinSize": "1", "MaxSize": "1", "AvailabilityZones": [marker]},
            "UpdatePolicy": {
                "AutoScalingRollingUpdate": {
                    "MinInstancesInService": "0",
                    "WaitOnResourceSignals": true
                }
            }
        }),
        Declared::WaitCondition => json!({
            "Type": "AWS::CloudFormation::WaitCondition",
            "Properties": {"Count": 1, "Timeout": marker}
        }),
    };
    json!({"Resources": {GATE: gate}}).to_string()
}

/// One `SignalResource` call, as an SDK sends it.
async fn signal(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    unique_id: &str,
    signal_status: &str,
) -> (u16, String) {
    signal_in(router, REGION, stack, logical_id, unique_id, signal_status).await
}

/// [`signal`], signed for `region`.
async fn signal_in(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
    unique_id: &str,
    signal_status: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "SignalResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
            member("UniqueId", unique_id),
            member("Status", signal_status),
        ],
    )
    .await
}

/// The reason CloudFormation records for a signal it received, which is what an event about
/// one must carry.
fn signal_reason(signal_status: &str, unique_id: &str) -> String {
    format!("Received {signal_status} signal with UniqueId {unique_id}")
}

/// Every `DescribeStackEvents` event of `stack`, as `(LogicalResourceId, ResourceStatus,
/// ResourceStatusReason, ResourceType)`.
async fn events(
    router: &MultiServiceRouter,
    stack: &str,
) -> Vec<(String, String, String, String)> {
    let (status_code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStackEvents must answer 200 for {stack:?}, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|event| {
            (
                xml_text(event, "LogicalResourceId").unwrap_or_default(),
                xml_text(event, "ResourceStatus").unwrap_or_default(),
                xml_text(event, "ResourceStatusReason").unwrap_or_default(),
                xml_text(event, "ResourceType").unwrap_or_default(),
            )
        })
        .collect()
}

/// Whether the call that has just returned certainly reached the stack while its operation
/// was still in flight.
///
/// The emulator owes the in-progress status only until the transition falls due, so a row
/// that lost the race to the clock on a loaded machine is held to the acceptance alone
/// rather than to what a signal something was waiting for does.
fn reached_the_operation(started: Instant) -> bool {
    started.elapsed() < STATUS_TRANSITION_DELAY
}

/// A signal nothing is waiting on is accepted, recorded, and changes nothing about the
/// stack: no event names it and the operation in flight reaches the status it was headed
/// for.
#[tokio::test]
async fn a_signal_nothing_waits_on_is_accepted_and_changes_nothing() {
    struct Row {
        case: &'static str,
        stack: &'static str,
        declared: Declared,
        /// The logical id signalled, which is not always one the template declares.
        logical_id: &'static str,
        signal_status: &'static str,
        /// Signal while the create is still in flight, or after it settled.
        while_creating: bool,
        /// The `UniqueId` of the SUCCESS signal that releases the create's own wait before
        /// the row's signal is sent, for a declaration that HOLDS the create open: such a
        /// create cannot settle until it has its signals, so a row that needs it settled
        /// delivers them first. `None` for a declaration nothing waits on during a create.
        release: Option<&'static str>,
        /// Address the stack by its id rather than by its name.
        by_stack_id: bool,
    }

    let rows: &[Row] = &[
        Row {
            case: "a resource with no wait declared on it, signalled SUCCESS once created",
            stack: "signal-plain-success",
            declared: Declared::NothingWaiting,
            logical_id: GATE,
            signal_status: "SUCCESS",
            while_creating: false,
            release: None,
            by_stack_id: false,
        },
        Row {
            case: "the same resource signalled FAILURE while the create is in flight",
            stack: "signal-plain-failure",
            declared: Declared::NothingWaiting,
            logical_id: GATE,
            signal_status: "FAILURE",
            while_creating: true,
            release: None,
            by_stack_id: false,
        },
        Row {
            case: "a logical id the template declares nothing under",
            stack: "signal-undeclared",
            declared: Declared::NothingWaiting,
            logical_id: "NoSuchResource",
            signal_status: "SUCCESS",
            while_creating: true,
            release: None,
            by_stack_id: false,
        },
        Row {
            case: "a creation policy's wait is over once the create settled",
            stack: "signal-late",
            declared: Declared::CreationPolicy,
            logical_id: GATE,
            signal_status: "FAILURE",
            while_creating: false,
            release: Some("i-00release00"),
            by_stack_id: false,
        },
        Row {
            case: "an update policy's wait is not a create's, addressed by stack id",
            stack: "signal-update-policy-on-create",
            declared: Declared::UpdatePolicy,
            logical_id: GATE,
            signal_status: "FAILURE",
            while_creating: true,
            release: None,
            by_stack_id: true,
        },
    ];

    for row in rows {
        let (router, engine) = router_with_engine();
        let (status_code, created) =
            create_stack(&router, row.stack, &template(row.declared, "first"), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            row.case
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_default();
        let addressed = if row.by_stack_id {
            stack_id.clone()
        } else {
            row.stack.to_string()
        };
        // A declaration that HOLDS the create open is released first: the row's own signal
        // is the one nothing is waiting for, and the create has to have reached the status
        // the row signals it in.
        let mut expected: Vec<(String, String, String)> = Vec::new();
        if let Some(release) = row.release {
            let (status_code, released) =
                signal(&router, &addressed, row.logical_id, release, "SUCCESS").await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the releasing signal must be accepted, got {status_code} \
                 {released}",
                row.case
            );
            expected.push((
                row.logical_id.to_string(),
                release.to_string(),
                "SUCCESS".to_string(),
            ));
        }
        if !row.while_creating {
            wait_for_the_transition().await;
            // A SETTLED create is a status a READ reports: a stack reaches its terminal
            // status once the services that own its resources report them ready, which only
            // a read asks them about — so the create is polled to that status the way a
            // client does before the row sends the signal nothing is waiting for.
            for _ in 0..SETTLE_ATTEMPTS {
                let (_, described) = describe_stack(&router, &addressed).await;
                if !xml_text(&described, "StackStatus")
                    .unwrap_or_default()
                    .ends_with("_IN_PROGRESS")
                {
                    break;
                }
                wait_for_the_transition().await;
            }
        }

        let unique_id = "i-0123456789abcdef0";
        let (status_code, signalled) = signal(
            &router,
            &addressed,
            row.logical_id,
            unique_id,
            row.signal_status,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: the signal must be accepted, got {status_code} {signalled}",
            row.case
        );

        // The write the call made: the signal is held against the stack, under the logical
        // id and unique id it named.
        let held = store(&engine);
        let recorded: Vec<(String, String, String)> = held
            .lock()
            .resolve(REGION, &stack_id)
            .expect("the stack the signal was sent to")
            .signals
            .iter()
            .map(|held| {
                (
                    held.logical_resource_id.clone(),
                    held.unique_id.clone(),
                    held.status.clone(),
                )
            })
            .collect();
        drop(held);
        expected.push((
            row.logical_id.to_string(),
            unique_id.to_string(),
            row.signal_status.to_string(),
        ));
        assert_eq!(
            recorded, expected,
            "case {:?}: expected the signals the row sent to be recorded, got {recorded:?}",
            row.case
        );

        wait_for_the_transition().await;
        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            row.case
        );
        let reported = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {:?}: a signal nothing waits on must leave the create alone, got \
             {reported:?} {described}",
            row.case
        );
        let reasons: Vec<String> = events(&router, row.stack)
            .await
            .into_iter()
            .map(|(_, _, reason, _)| reason)
            .collect();
        assert!(
            !reasons.iter().any(|reason| reason.contains(unique_id)),
            "case {:?}: no event may report a signal nothing was waiting for, got \
             {reasons:?}",
            row.case
        );
    }
}

/// A signal the operation in flight IS waiting for is reported as an event on the signalled
/// resource, and a SUCCESS signal leaves that operation to complete.
#[tokio::test]
async fn a_signal_something_waits_on_is_reported_on_the_resource() {
    struct Row {
        case: &'static str,
        stack: &'static str,
        declared: Declared,
        /// Signal the stack's UPDATE rather than its create.
        updating: bool,
        expected_resource_type: &'static str,
        expected_resource_status: &'static str,
    }

    let rows: &[Row] = &[
        Row {
            case: "a wait condition the create is waiting on",
            stack: "signal-wait-condition",
            declared: Declared::WaitCondition,
            updating: false,
            expected_resource_type: "AWS::CloudFormation::WaitCondition",
            expected_resource_status: "CREATE_IN_PROGRESS",
        },
        Row {
            case: "a creation policy's resource signal",
            stack: "signal-creation-policy",
            declared: Declared::CreationPolicy,
            updating: false,
            expected_resource_type: "AWS::EC2::Instance",
            expected_resource_status: "CREATE_IN_PROGRESS",
        },
        Row {
            case: "an update policy that waits on resource signals",
            stack: "signal-update-policy",
            declared: Declared::UpdatePolicy,
            updating: true,
            expected_resource_type: "AWS::AutoScaling::AutoScalingGroup",
            expected_resource_status: "UPDATE_IN_PROGRESS",
        },
        Row {
            case: "a wait condition the update is waiting on",
            stack: "signal-wait-condition-update",
            declared: Declared::WaitCondition,
            updating: true,
            expected_resource_type: "AWS::CloudFormation::WaitCondition",
            expected_resource_status: "UPDATE_IN_PROGRESS",
        },
    ];

    for row in rows {
        let (router, _engine) = router_with_engine();
        // Timed from the call that STARTS the operation: the transition is due one span
        // after that call, not after the signal.
        let create_started = Instant::now();
        let (status_code, created) =
            create_stack(&router, row.stack, &template(row.declared, "300"), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            row.case
        );
        // An update's signal needs the create out of the way first; a create's signal has
        // to reach the create itself.
        let started = if row.updating {
            // A create that waits on a signal does not complete until it gets one (the
            // `wait_conditions` suite is what pins that wait), so the create's own wait is
            // released here before the update is started. A declaration that waits on
            // nothing during a create is signalled all the same, and nothing turns on it.
            let (status_code, signalled) =
                signal(&router, row.stack, GATE, "create-signal", "SUCCESS").await;
            assert_eq!(
                status_code, 200,
                "case {:?}: the create's signal must be accepted, got {status_code} \
                 {signalled}",
                row.case
            );
            wait_for_the_transition().await;
            let started = Instant::now();
            let (status_code, updated) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", row.stack),
                    member("TemplateBody", &template(row.declared, "600")),
                ],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: UpdateStack must answer 200, got {status_code} {updated}",
                row.case
            );
            started
        } else {
            create_started
        };

        let unique_id = "signal-1";
        let (status_code, signalled) = signal(&router, row.stack, GATE, unique_id, "SUCCESS").await;
        assert_eq!(
            status_code, 200,
            "case {:?}: the signal must be accepted, got {status_code} {signalled}",
            row.case
        );
        if !reached_the_operation(started) {
            // The operation had already settled: nothing was waiting any more, which the
            // suite's first table is what pins.
            continue;
        }

        let reason = signal_reason("SUCCESS", unique_id);
        let reported = events(&router, row.stack).await;
        let on_the_resource: Vec<&(String, String, String, String)> = reported
            .iter()
            .filter(|(logical_id, _, event_reason, _)| logical_id == GATE && *event_reason == reason)
            .collect();
        assert_eq!(
            on_the_resource.len(),
            1,
            "case {:?}: expected exactly one event reporting {reason:?} on {GATE}, got \
             {reported:?}",
            row.case
        );
        let (_, event_status, _, event_type) = on_the_resource[0];
        assert_eq!(
            event_status, row.expected_resource_status,
            "case {:?}: expected the signalled resource to be reported {:?}, got \
             {event_status:?}",
            row.case, row.expected_resource_status
        );
        assert_eq!(
            event_type, row.expected_resource_type,
            "case {:?}: expected the event to report the declared type {:?}, got \
             {event_type:?}",
            row.case, row.expected_resource_type
        );

        // A success signal is what the operation was waiting for: it completes.
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, row.stack).await;
        let settled = if row.updating {
            status::UPDATE_COMPLETE
        } else {
            status::CREATE_COMPLETE
        };
        let stack_status = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            stack_status, settled,
            "case {:?}: expected {settled} after a success signal, got {stack_status:?} \
             {described}",
            row.case
        );
    }
}

/// A FAILURE signal the operation in flight is waiting for fails that operation at once —
/// and the operation is then rolled back the way the request that started it asked: a
/// failed create unwinds what it had made, and a failed update goes back to the previous
/// stack configuration.
#[tokio::test]
async fn a_failure_signal_fails_the_operation_that_waits_for_it() {
    struct Row {
        case: &'static str,
        stack: &'static str,
        declared: Declared,
        updating: bool,
        /// `DisableRollback` / `OnFailure`-style members, exactly as the create carries
        /// them.
        extra: &'static [(&'static str, &'static str)],
        expected_status: &'static str,
        /// The word the reason uses for the operation that failed.
        expected_verb: &'static str,
    }

    let rows: &[Row] = &[
        Row {
            case: "a create waiting on a wait condition rolls back",
            stack: "failure-wait-condition",
            declared: Declared::WaitCondition,
            updating: false,
            extra: &[],
            expected_status: status::ROLLBACK_COMPLETE,
            expected_verb: "create",
        },
        Row {
            case: "a create waiting on a creation policy rolls back",
            stack: "failure-creation-policy",
            declared: Declared::CreationPolicy,
            updating: false,
            extra: &[],
            expected_status: status::ROLLBACK_COMPLETE,
            expected_verb: "create",
        },
        Row {
            case: "DisableRollback keeps the failed create as it failed",
            stack: "failure-no-rollback",
            declared: Declared::CreationPolicy,
            updating: false,
            extra: &[("DisableRollback", "true")],
            expected_status: status::CREATE_FAILED,
            expected_verb: "create",
        },
        Row {
            case: "an update waiting on an update policy fails and is rolled back",
            stack: "failure-update-policy",
            declared: Declared::UpdatePolicy,
            updating: true,
            extra: &[],
            expected_status: status::UPDATE_ROLLBACK_COMPLETE,
            expected_verb: "update",
        },
    ];

    for row in rows {
        let (router, _engine) = router_with_engine();
        let extra: Vec<(String, String)> = row
            .extra
            .iter()
            .map(|(key, value)| member(key, value))
            .collect();
        let create_started = Instant::now();
        let (status_code, created) =
            create_stack(&router, row.stack, &template(row.declared, "300"), &extra).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            row.case
        );
        let started = if row.updating {
            wait_for_the_transition().await;
            let started = Instant::now();
            let (status_code, updated) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", row.stack),
                    member("TemplateBody", &template(row.declared, "600")),
                ],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {:?}: UpdateStack must answer 200, got {status_code} {updated}",
                row.case
            );
            started
        } else {
            create_started
        };

        let unique_id = "i-00f00";
        let (status_code, signalled) = signal(&router, row.stack, GATE, unique_id, "FAILURE").await;
        assert_eq!(
            status_code, 200,
            "case {:?}: a failure signal is still an accepted call, got {status_code} \
             {signalled}",
            row.case
        );
        if !reached_the_operation(started) {
            // Same guard as above: a row that lost the race to the clock is held to the
            // acceptance alone.
            continue;
        }

        wait_for_the_transition().await;
        let (status_code, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            row.case
        );
        let stack_status = xml_text(&described, "StackStatus").unwrap_or_default();
        assert_eq!(
            stack_status, row.expected_status,
            "case {:?}: expected {} after a failure signal, got {stack_status:?} \
             {described}",
            row.case, row.expected_status
        );
        let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        // The STACK's reason is the summary of which resource failed the operation; the
        // signal that failed it is the RESOURCE's own diagnosis, asserted on its event
        // below.
        let expected_reason = resource_failure_summary(row.expected_verb, [GATE]);
        assert_eq!(
            reason, expected_reason,
            "case {:?}: expected the reason to summarise the resource that failed, got \
             {reason:?}",
            row.case
        );
        // The event log carries the signal on the resource itself, with the status that
        // signal left it in.
        let reported = events(&router, row.stack).await;
        let failed_resource = reported.iter().any(|(logical_id, event_status, event_reason, _)| {
            logical_id == GATE
                && event_reason == &signal_reason("FAILURE", unique_id)
                && event_status.ends_with("_FAILED")
        });
        assert!(
            failed_resource,
            "case {:?}: expected an event reporting {GATE} failed by the signal, got \
             {reported:?}",
            row.case
        );
    }
}

/// The same `UniqueId` is the same signal: re-sending it is accepted, records no second
/// signal and reports no second event, while another unique id is another signal.
#[tokio::test]
async fn a_repeated_unique_id_is_the_same_signal() {
    let (router, engine) = router_with_engine();
    let stack = "signal-idempotence";
    let started = Instant::now();
    let (status_code, created) = create_stack(
        &router,
        stack,
        &template(Declared::WaitCondition, "300"),
        &[],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {created}"
    );

    for attempt in ["first", "retried"] {
        let (status_code, signalled) = signal(&router, stack, GATE, "signal-1", "SUCCESS").await;
        assert_eq!(
            status_code, 200,
            "case {attempt:?}: the signal must be accepted, got {status_code} {signalled}"
        );
    }
    let (status_code, signalled) = signal(&router, stack, GATE, "signal-2", "SUCCESS").await;
    assert_eq!(
        status_code, 200,
        "case \"a second unique id\": the signal must be accepted, got {status_code} \
         {signalled}"
    );

    let held = store(&engine);
    let recorded: Vec<(String, String)> = held
        .lock()
        .resolve(REGION, stack)
        .expect("the signalled stack")
        .signals
        .iter()
        .map(|signal| (signal.unique_id.clone(), signal.status.clone()))
        .collect();
    drop(held);
    assert_eq!(
        recorded,
        vec![
            ("signal-1".to_string(), "SUCCESS".to_string()),
            ("signal-2".to_string(), "SUCCESS".to_string())
        ],
        "case \"a retried signal\": one signal per unique id must be recorded, got \
         {recorded:?}"
    );

    if reached_the_operation(started) {
        let reasons: Vec<String> = events(&router, stack)
            .await
            .into_iter()
            .map(|(_, _, reason, _)| reason)
            .collect();
        let repeats = reasons
            .iter()
            .filter(|reason| *reason == &signal_reason("SUCCESS", "signal-1"))
            .count();
        assert_eq!(
            repeats, 1,
            "case \"a retried signal\": the retry must report no second event, got \
             {reasons:?}"
        );
    }
}

/// The one refusal: a `StackName` that resolves to no live stack in the Region the request
/// was signed for.
#[tokio::test]
async fn a_signal_to_a_stack_that_does_not_resolve_is_refused() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Addressed {
        /// A name no stack of this account carries.
        AName,
        /// The name of a stack whose deletion completed.
        ADeletedStacksName,
        /// The ID of a stack whose deletion completed, which outlives the name.
        ADeletedStacksId,
        /// The name of a live stack, signalled in another Region.
        AnotherRegion,
    }

    let rows: &[(&str, Addressed)] = &[
        ("a stack name nothing created", Addressed::AName),
        ("a deleted stack, by name", Addressed::ADeletedStacksName),
        ("a deleted stack, by id", Addressed::ADeletedStacksId),
        (
            "a live stack, signalled in another Region",
            Addressed::AnotherRegion,
        ),
    ];

    for (case, addressed) in rows {
        let (router, _engine) = router_with_engine();
        let stack = "signal-refusal";
        let (status_code, created) = create_stack(
            &router,
            stack,
            &template(Declared::WaitCondition, "300"),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStack must answer 200, got {status_code} {created}"
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_default();
        wait_for_the_transition().await;

        if matches!(
            addressed,
            Addressed::ADeletedStacksName | Addressed::ADeletedStacksId
        ) {
            let (status_code, deleted) =
                call(&router, "DeleteStack", &[member("StackName", stack)]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: DeleteStack must answer 200, got {status_code} {deleted}"
            );
            wait_for_the_transition().await;
        }

        let (region, name) = match addressed {
            Addressed::AName => (REGION, "no-such-stack".to_string()),
            Addressed::ADeletedStacksName => (REGION, stack.to_string()),
            Addressed::ADeletedStacksId => (REGION, stack_id.clone()),
            Addressed::AnotherRegion => (OTHER_REGION, stack.to_string()),
        };
        let (status_code, refused) =
            signal_in(&router, region, &name, GATE, "signal-1", "SUCCESS").await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the signal to be refused, got {status_code} {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Code").unwrap_or_default(),
            "ValidationError",
            "case {case:?}: expected CloudFormation's own refusal code, got {refused}"
        );
        assert_eq!(
            xml_text(&refused, "Message").unwrap_or_default(),
            format!("Stack {name} does not exist"),
            "case {case:?}: expected the refusal to name the stack, got {refused}"
        );
    }
}
