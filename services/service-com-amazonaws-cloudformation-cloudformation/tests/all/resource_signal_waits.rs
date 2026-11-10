// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE-SIGNAL WAITS a create is held at, end to end over the awsQuery wire.
//!
//! RULE: **a resource whose declaration asks a create to wait for resource signals — a
//! `CreationPolicy`'s `ResourceSignal`, whatever the resource's type, or an
//! `AWS::CloudFormation::WaitCondition` — is held at `CREATE_IN_PROGRESS` until it has
//! received the number of SUCCESS signals its `Count` asks for, or until the `Timeout` that
//! declaration states runs out (which fails the create); the resources that depend on it are
//! not started before that, and waits with no dependency between them are open at the same
//! time.** CloudFormation "doesn't proceed with a stack creation or update until resources
//! receive the required number of signals or the timeout period is exceeded"
//! (`SignalResource`), and `CreationPolicy` says the same of a resource of any type: "when
//! CloudFormation creates the associated resource, it waits for a success signal".
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a client sees.
//! The rows span the class the rule generalises over: which declaration opens the wait
//! (`CreationPolicy` on a compute resource, on an Auto Scaling group and on a wait
//! condition carrying no properties at all; a wait condition's own properties), `Count`
//! spelled as a number, as a string and not at all, `Timeout` spelled as an ISO 8601
//! duration and as a number of seconds, a dependency stated by `DependsOn` and by a `Ref`,
//! and both template spellings.
//!
//! `wait_conditions.rs` is the suite for the wait an `AWS::CloudFormation::WaitCondition`'s
//! own properties declare, and `stack_event_timeline.rs` for the event timeline a held graph
//! produces; what is pinned HERE is that the same wait is opened by a `CreationPolicy`, for
//! the number of signals and the span THAT attribute states.

use std::time::Duration;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, router_with_engine, store,
    wait_for_the_transition, xml_text, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_status;
use service_com_amazonaws_cloudformation_cloudformation::state::wait_conditions::{
    DEFAULT_SIGNAL_COUNT, DEFAULT_TIMEOUT, MAX_TIMEOUT,
};
use smithy_server_runtime::MultiServiceRouter;

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

/// `DescribeStackResource` for one resource, answered with the HTTP status and the body.
async fn describe_resource(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await
}

/// The `ResourceStatus` and `ResourceStatusReason` a stack reports for one resource.
async fn reported(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> (String, String) {
    let (code, body) = describe_resource(router, stack, logical_id).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected {logical_id:?} to be described, got {code} {body}"
    );
    (
        xml_text(&body, "ResourceStatus").unwrap_or_default(),
        xml_text(&body, "ResourceStatusReason").unwrap_or_default(),
    )
}

/// The `StackStatus` a stack reports right now.
async fn status_of(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The `StackStatusReason` a stack reports right now.
async fn status_reason_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = describe_stack(router, stack).await;
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}

/// One row of the barrier table.
struct WaitCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The logical id the create is held at.
    gate: &'static str,
    /// A resource of the same template whose creation waits on the gate.
    behind: &'static str,
    /// The `UniqueId`s the row signals the gate with, in order: the LAST of them is the one
    /// that brings the wait up to its `Count`.
    signals: &'static [&'static str],
}

fn wait_cases() -> Vec<WaitCase> {
    vec![
        WaitCase {
            case: "a CreationPolicy on a compute resource, Count as a number, Timeout PT10M",
            stack: "signal-wait-instance",
            template: r#"{"Resources":{
                "Gate":{"Type":"AWS::EC2::Instance",
                  "Properties":{"ImageId":"ami-0123456789abcdef0",
                    "InstanceType":"t3.micro"},
                  "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT10M"}}},
                "Behind":{"Type":"AWS::SNS::Topic","DependsOn":"Gate",
                  "Properties":{"TopicName":"signal-wait-instance-behind"}}}}"#,
            gate: "Gate",
            behind: "Behind",
            signals: &["i-0123456789abcdef0"],
        },
        WaitCase {
            case: "a CreationPolicy on an Auto Scaling group, Count as a string, reached \
                   through a Ref",
            stack: "signal-wait-group",
            template: r#"{"Resources":{
                "Gate":{"Type":"AWS::AutoScaling::AutoScalingGroup",
                  "Properties":{"MinSize":"2","MaxSize":"2",
                    "AvailabilityZones":["us-east-1a"],
                    "LaunchTemplate":{"LaunchTemplateId":{"Ref":"GateLaunch"},
                      "Version":{"Fn::GetAtt":["GateLaunch","LatestVersionNumber"]}}},
                  "CreationPolicy":{"ResourceSignal":{"Count":"2","Timeout":"PT1H"}}},
                "GateLaunch":{"Type":"AWS::EC2::LaunchTemplate",
                  "Properties":{"LaunchTemplateData":{"ImageId":"ami-0123456789abcdef0",
                    "InstanceType":"t3.micro"}}},
                "Behind":{"Type":"AWS::SNS::Topic",
                  "Properties":{"TopicName":"signal-wait-group-behind",
                    "DisplayName":{"Ref":"Gate"}}}}}"#,
            gate: "Gate",
            behind: "Behind",
            signals: &["i-00000000000000001", "i-00000000000000002"],
        },
        WaitCase {
            case: "a wait condition whose signalling is declared by a CreationPolicy alone",
            stack: "signal-wait-gate",
            template: r#"{"Resources":{
                "Gate":{"Type":"AWS::CloudFormation::WaitCondition",
                  "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT10M"}}},
                "Behind":{"Type":"AWS::SNS::Topic","DependsOn":["Gate"],
                  "Properties":{"TopicName":"signal-wait-gate-behind"}}}}"#,
            gate: "Gate",
            behind: "Behind",
            signals: &["gate-a"],
        },
        WaitCase {
            case: "a YAML CreationPolicy that states no Count at all",
            stack: "signal-wait-yaml",
            template: concat!(
                "Resources:\n",
                "  Gate:\n",
                "    Type: AWS::EC2::Instance\n",
                "    Properties:\n",
                "      ImageId: ami-0123456789abcdef0\n",
                "      InstanceType: t3.micro\n",
                "    CreationPolicy:\n",
                "      ResourceSignal:\n",
                "        Timeout: PT5M\n",
                "  Behind:\n",
                "    Type: AWS::SNS::Topic\n",
                "    DependsOn: Gate\n",
                "    Properties:\n",
                "      TopicName: signal-wait-yaml-behind\n",
            ),
            gate: "Gate",
            behind: "Behind",
            signals: &["i-0000000000000000a"],
        },
        WaitCase {
            case: "a wait condition's own properties, Timeout in seconds",
            stack: "signal-wait-properties",
            template: r#"{"Resources":{
                "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
                "Gate":{"Type":"AWS::CloudFormation::WaitCondition",
                  "Properties":{"Handle":{"Ref":"Handle"},"Timeout":"600","Count":"1"}},
                "Behind":{"Type":"AWS::SNS::Topic","DependsOn":"Gate",
                  "Properties":{"TopicName":"signal-wait-properties-behind"}}}}"#,
            gate: "Gate",
            behind: "Behind",
            signals: &["gate-properties"],
        },
    ]
}

#[tokio::test]
async fn a_create_waits_at_every_declaration_that_asks_for_resource_signals() {
    for case in wait_cases() {
        let router = router();
        let (code, created) = create_stack(&router, case.stack, case.template, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateStack to succeed, got {code} {created}",
            case.case
        );

        // Past the instant the clock alone would have completed the create: what holds the
        // operation is the wait, not a timer.
        wait_for_the_transition().await;
        let stack_status = status_of(case.case, &router, case.stack).await;
        assert_eq!(
            stack_status, "CREATE_IN_PROGRESS",
            "case {:?}: expected the create to still be in progress while the wait is \
             unsignalled, got {stack_status:?}",
            case.case
        );

        // The resource the wait is declared on is reported as the one being created, with
        // the message CloudFormation puts on work it has initiated and not finished.
        let (status, reason) = reported(case.case, &router, case.stack, case.gate).await;
        assert_eq!(
            status,
            resource_status::CREATE_IN_PROGRESS,
            "case {:?}: expected {:?} to be held at CREATE_IN_PROGRESS, got {status:?}",
            case.case,
            case.gate
        );
        assert_eq!(
            reason, "Resource creation Initiated",
            "case {:?}: expected {:?} to report its create as initiated, got {reason:?}",
            case.case, case.gate
        );

        // The resource behind the wait has not been started, so the stack reports nothing
        // for it at all.
        let (code, body) = describe_resource(&router, case.stack, case.behind).await;
        assert_eq!(
            code, 400,
            "case {:?}: expected {:?} not to be reported while the wait holds, got {code} \
             {body}",
            case.case, case.behind
        );
        assert_eq!(
            xml_text(&body, "Code").unwrap_or_default(),
            "ValidationError",
            "case {:?}: expected CloudFormation's own refusal code for {:?}, got {body}",
            case.case,
            case.behind
        );

        // Every signal but the last leaves the create waiting.
        let last = case.signals.len() - 1;
        for (index, unique_id) in case.signals.iter().enumerate() {
            let (code, signalled) = signal(&router, case.stack, case.gate, unique_id).await;
            assert_eq!(
                code, 200,
                "case {:?}: expected signal {unique_id:?} to be accepted, got {code} \
                 {signalled}",
                case.case
            );
            if index < last {
                let stack_status = status_of(case.case, &router, case.stack).await;
                assert_eq!(
                    stack_status, "CREATE_IN_PROGRESS",
                    "case {:?}: expected the create to still be in progress after signal \
                     {unique_id:?}, got {stack_status:?}",
                    case.case
                );
            }
        }

        // The signals it waited for are what let the create finish, and the resource behind
        // the wait is created then — read back through the single-resource read.
        wait_for_the_transition().await;
        let stack_status = status_of(case.case, &router, case.stack).await;
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {:?}: expected the signalled create to complete, got {stack_status:?}",
            case.case
        );
        for logical_id in [case.gate, case.behind] {
            let (code, body) = describe_resource(&router, case.stack, logical_id).await;
            assert_eq!(
                code, 200,
                "case {:?}: expected {logical_id:?} to be described once created, got {code} \
                 {body}",
                case.case
            );
            let status = xml_text(&body, "ResourceStatus").unwrap_or_default();
            assert_eq!(
                status,
                resource_status::CREATE_COMPLETE,
                "case {:?}: expected {logical_id:?} to be CREATE_COMPLETE, got {status:?}",
                case.case
            );
            assert!(
                xml_text(&body, "PhysicalResourceId").is_some(),
                "case {:?}: expected {logical_id:?} to report a physical id, got {body}",
                case.case
            );
        }
    }
}

/// A diamond held at TWO independent `CreationPolicy` waits: one branch behind each gate and
/// a join over both branches.
const GATED_DIAMOND: &str = r#"{"Resources":{
    "GateA":{"Type":"AWS::CloudFormation::WaitCondition",
      "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT10M"}}},
    "GateB":{"Type":"AWS::CloudFormation::WaitCondition",
      "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT10M"}}},
    "BranchA":{"Type":"AWS::SNS::Topic","DependsOn":"GateA",
      "Properties":{"TopicName":"signal-diamond-a"}},
    "BranchB":{"Type":"AWS::SNS::Topic","DependsOn":"GateB",
      "Properties":{"TopicName":"signal-diamond-b"}},
    "Join":{"Type":"AWS::SNS::Topic","DependsOn":["BranchA","BranchB"],
      "Properties":{"TopicName":"signal-diamond-join"}}}}"#;

#[tokio::test]
async fn independent_resource_signal_waits_are_open_together_and_release_one_branch_each() {
    const CASE: &str = "a diamond held at two independent creation-policy waits";
    let stack = "signal-diamond";
    let router = router();
    let (code, created) = create_stack(&router, stack, GATED_DIAMOND, &[]).await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected CreateStack to succeed, got {code} {created}"
    );

    // ONE poll reports BOTH heads of the diamond as in flight: the two waits have no
    // dependency between them, so the create opened both rather than reaching them in turn.
    wait_for_the_transition().await;
    for gate in ["GateA", "GateB"] {
        let (status, _) = reported(CASE, &router, stack, gate).await;
        assert_eq!(
            status,
            resource_status::CREATE_IN_PROGRESS,
            "case {CASE:?}: expected {gate} to be open at the same time as the other gate, \
             got {status:?}"
        );
    }
    for behind in ["BranchA", "BranchB", "Join"] {
        let (code, body) = describe_resource(&router, stack, behind).await;
        assert_eq!(
            code, 400,
            "case {CASE:?}: expected {behind} not to be reported while its gate holds, got \
             {code} {body}"
        );
    }

    // Signalling ONE gate releases ONE branch: its own branch is created while the other
    // gate is still waiting, and the join is still unreached because one of its
    // predecessors is.
    let (code, signalled) = signal(&router, stack, "GateA", "gate-a").await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected the signal for GateA to be accepted, got {code} {signalled}"
    );
    let (status, _) = reported(CASE, &router, stack, "BranchA").await;
    assert_eq!(
        status,
        resource_status::CREATE_COMPLETE,
        "case {CASE:?}: expected BranchA to be created once GateA was signalled, got \
         {status:?}"
    );
    let (status, _) = reported(CASE, &router, stack, "GateB").await;
    assert_eq!(
        status,
        resource_status::CREATE_IN_PROGRESS,
        "case {CASE:?}: expected GateB to go on waiting for its own signal, got {status:?}"
    );
    let (code, body) = describe_resource(&router, stack, "Join").await;
    assert_eq!(
        code, 400,
        "case {CASE:?}: expected Join not to be reported while one predecessor is still \
         waiting, got {code} {body}"
    );
    let stack_status = status_of(CASE, &router, stack).await;
    assert_eq!(
        stack_status, "CREATE_IN_PROGRESS",
        "case {CASE:?}: expected the create to go on waiting at GateB, got {stack_status:?}"
    );

    // The second signal is what finishes the walk: every resource of the diamond is created.
    let (code, signalled) = signal(&router, stack, "GateB", "gate-b").await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected the signal for GateB to be accepted, got {code} {signalled}"
    );
    wait_for_the_transition().await;
    let stack_status = status_of(CASE, &router, stack).await;
    assert_eq!(
        stack_status, "CREATE_COMPLETE",
        "case {CASE:?}: expected the signalled create to complete, got {stack_status:?}"
    );
    for logical_id in ["GateA", "GateB", "BranchA", "BranchB", "Join"] {
        let (status, _) = reported(CASE, &router, stack, logical_id).await;
        assert_eq!(
            status,
            resource_status::CREATE_COMPLETE,
            "case {CASE:?}: expected {logical_id} to be CREATE_COMPLETE, got {status:?}"
        );
    }
}

/// One row of the `ResourceSignal` table: what the attribute states, and the wait and count
/// it asks for.
struct PolicyCase {
    case: &'static str,
    stack: &'static str,
    /// `Count` as the attribute spells it, or `None` for one that states none.
    count: Option<&'static str>,
    /// `Timeout` as the attribute spells it, or `None` for one that states none.
    timeout: Option<&'static str>,
    /// The span that attribute ASKS the wait to run for.
    asked_for: Duration,
    signals: usize,
}

impl PolicyCase {
    /// How long the wait is really HELD for that attribute: the span it asked for, which is
    /// the span the gated resource stays in flight for.
    fn held(&self) -> Duration {
        self.asked_for
    }
}

/// The class the bound covers: every `Timeout` and `Count` a `CreationPolicy`'s
/// `ResourceSignal` can state.
///
/// The published values are that attribute's own — `Timeout` is "in ISO8601 duration
/// format", "Default: PT5M", "Maximum: PT12H", and `Count`'s "Default: 1" — so the rows
/// include the maximum, one hour past it, a duration whose designators go beyond hours, the
/// degenerate zero, a value that is no duration at all, and each member not stated. Nothing
/// but a client's own signal ends this wait before the span the attribute asked for is over,
/// so what it asks for is also how long the wait is really held.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "ten minutes, one signal",
            stack: "policy-ten-minutes",
            count: Some("1"),
            timeout: Some("PT10M"),
            asked_for: Duration::from_secs(600),
            signals: 1,
        },
        PolicyCase {
            case: "an hour and a count of three",
            stack: "policy-an-hour",
            count: Some("3"),
            timeout: Some("PT1H"),
            asked_for: Duration::from_secs(3_600),
            signals: 3,
        },
        PolicyCase {
            case: "a duration whose designators run past hours",
            stack: "policy-compound",
            count: Some("2"),
            timeout: Some("PT1H30M15S"),
            asked_for: Duration::from_secs(5_415),
            signals: 2,
        },
        PolicyCase {
            case: "the published maximum",
            stack: "policy-maximum",
            count: Some("1"),
            timeout: Some("PT12H"),
            asked_for: MAX_TIMEOUT,
            signals: 1,
        },
        PolicyCase {
            case: "an hour past the published maximum",
            stack: "policy-past-maximum",
            count: Some("1"),
            timeout: Some("PT13H"),
            asked_for: MAX_TIMEOUT,
            signals: 1,
        },
        PolicyCase {
            case: "a day, which is past the published maximum too",
            stack: "policy-a-day",
            count: Some("1"),
            timeout: Some("P1D"),
            asked_for: MAX_TIMEOUT,
            signals: 1,
        },
        PolicyCase {
            case: "a wait that is over the moment it starts",
            stack: "policy-zero",
            count: Some("1"),
            timeout: Some("PT0S"),
            asked_for: Duration::ZERO,
            signals: 1,
        },
        PolicyCase {
            case: "neither member stated",
            stack: "policy-unstated",
            count: None,
            timeout: None,
            asked_for: DEFAULT_TIMEOUT,
            signals: DEFAULT_SIGNAL_COUNT,
        },
        PolicyCase {
            case: "a Timeout that is no duration at all, and a Count of zero",
            stack: "policy-unreadable",
            count: Some("0"),
            timeout: Some("ten minutes"),
            asked_for: DEFAULT_TIMEOUT,
            signals: DEFAULT_SIGNAL_COUNT,
        },
    ]
}

#[tokio::test]
async fn a_resource_signal_waits_for_what_the_attribute_states() {
    // The default a `ResourceSignal` that states no `Timeout` falls back to is inside the
    // maximum the attribute publishes, so no declaration can be held longer than it permits.
    assert!(
        DEFAULT_TIMEOUT < MAX_TIMEOUT,
        "the default wait must be inside the published maximum, got {DEFAULT_TIMEOUT:?} \
         against {MAX_TIMEOUT:?}"
    );
    // ...and every row is held for exactly the span it asked for, which is what the hold
    // assertion below reads off each barrier.

    let (router, engine) = router_with_engine();
    for case in policy_cases() {
        let mut resource_signal = String::new();
        if let Some(count) = case.count {
            resource_signal.push_str(&format!(r#""Count":"{count}""#));
        }
        if let Some(timeout) = case.timeout {
            if !resource_signal.is_empty() {
                resource_signal.push(',');
            }
            resource_signal.push_str(&format!(r#""Timeout":"{timeout}""#));
        }
        let template = format!(
            r#"{{"Resources":{{"Gate":{{
                "Type":"AWS::CloudFormation::WaitCondition",
                "CreationPolicy":{{"ResourceSignal":{{{resource_signal}}}}}}}}}}}"#
        );
        let (code, created) = create_stack(&router, case.stack, &template, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateStack to succeed, got {code} {created}",
            case.case
        );

        let held = store(&engine);
        let locked = held.lock();
        let stack = locked
            .resolve(REGION, case.stack)
            .unwrap_or_else(|| panic!("case {:?}: the stack just created", case.case));
        let barrier = stack
            .wait_conditions
            .first()
            .unwrap_or_else(|| panic!("case {:?}: the wait the policy opened", case.case));
        let asked_for = barrier.declared.timeout;
        assert_eq!(
            asked_for, case.asked_for,
            "case {:?}: expected a Timeout of {:?} to ask for {:?}, got {asked_for:?}",
            case.case, case.timeout, case.asked_for
        );
        let held_for = barrier
            .deadline()
            .duration_since(barrier.opened_at)
            .unwrap_or(Duration::ZERO);
        assert_eq!(
            held_for,
            case.held(),
            "case {:?}: expected a Timeout of {:?} to be held for {:?}, got {held_for:?}",
            case.case,
            case.timeout,
            case.held()
        );
        let count = barrier.count();
        assert_eq!(
            count, case.signals,
            "case {:?}: expected a Count of {:?} to wait for {} signals, got {count}",
            case.case, case.count, case.signals
        );
    }
}

/// One row of the expiry table: the wait that runs out, the signals that did arrive, and the
/// number of signals the failure says never did.
struct ExpiryCase {
    case: &'static str,
    stack: &'static str,
    count: &'static str,
    /// The `UniqueId`s signalled before the wait ran out, which are fewer than `count`.
    signals: &'static [&'static str],
    outstanding: usize,
}

#[tokio::test]
async fn a_resource_signal_wait_that_runs_out_fails_the_create_as_aws_words_it() {
    // Each row's `Timeout` is one second, so the case waits just over that for the wait to
    // have run out.
    let cases: Vec<ExpiryCase> = vec![
        ExpiryCase {
            case: "one signal expected and none sent",
            stack: "expiry-none",
            count: "1",
            signals: &[],
            outstanding: 1,
        },
        ExpiryCase {
            case: "three expected and one sent",
            stack: "expiry-partial",
            count: "3",
            signals: &["i-0000000000000000b"],
            outstanding: 2,
        },
    ];
    for case in cases {
        let router = router();
        let template = format!(
            r#"{{"Resources":{{"Gate":{{
                "Type":"AWS::EC2::Instance",
                "Properties":{{"ImageId":"ami-0123456789abcdef0",
                  "InstanceType":"t3.micro"}},
                "CreationPolicy":{{"ResourceSignal":{{
                  "Count":"{}","Timeout":"PT1S"}}}}}}}}}}"#,
            case.count
        );
        let (code, created) = create_stack(&router, case.stack, &template, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: expected CreateStack to succeed, got {code} {created}",
            case.case
        );
        for unique_id in case.signals {
            let (code, signalled) = signal(&router, case.stack, "Gate", unique_id).await;
            assert_eq!(
                code, 200,
                "case {:?}: expected signal {unique_id:?} to be accepted, got {code} \
                 {signalled}",
                case.case
            );
        }
        tokio::time::sleep(Duration::from_millis(1_300)).await;

        let stack_status = status_of(case.case, &router, case.stack).await;
        assert!(
            stack_status == "ROLLBACK_IN_PROGRESS" || stack_status == "ROLLBACK_COMPLETE",
            "case {:?}: expected the create to fail and roll back once the wait ran out, got \
             {stack_status:?}",
            case.case
        );
        // "Failed to receive N resource signal(s) within the specified duration" is how
        // CloudFormation words a creation policy's wait running out, and N is what never
        // arrived.
        let reason = status_reason_of(&router, case.stack).await;
        let expected = format!(
            "Failed to receive {} resource signal(s) within the specified duration",
            case.outstanding
        );
        assert!(
            reason.contains(&expected),
            "case {:?}: expected the failure to say {expected:?}, got {reason:?}",
            case.case
        );
    }
}
