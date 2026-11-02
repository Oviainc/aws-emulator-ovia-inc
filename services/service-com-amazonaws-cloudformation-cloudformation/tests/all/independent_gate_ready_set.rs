// SPDX-License-Identifier: CC0-1.0

//! What a create does with the resources it can start AT ONCE, end to end over the awsQuery
//! wire.
//!
//! RULE: **a create schedules every resource whose dependencies are already satisfied
//! together, so every resource-signal gate of one template that waits on nothing is in its
//! `CREATE_IN_PROGRESS` status at the same time — one pass of reads observes them all in
//! flight — and each of them stays there until it has received the number of SUCCESS signals
//! its OWN declaration asks for, whichever declaration that is.** CloudFormation "doesn't
//! proceed with a stack creation or update until resources receive the required number of
//! signals or the timeout period is exceeded" (`SignalResource`), and that wait is one
//! resource's: a resource with no gate before it is created while the gates hold, a resource
//! behind a gate is not reported until that gate has completed, and a join over several
//! branches is not reported until the last of them has.
//!
//! What a client does with that: the whole `CreationPolicy`/`ResourceSignal` bootstrap
//! pattern is one host signalling its own gate, so a deploy of several independent branches
//! has to expose every gate at once — a stack that reached them one at a time would deadlock
//! a caller that holds each branch until it sees its own gate open.
//!
//! The rows span what the rule generalises over: how MANY independent gates one create opens
//! (two and three), which declaration opens each (a `CreationPolicy`'s `ResourceSignal` on a
//! wait condition, a wait condition's own `Handle`/`Timeout`/`Count` properties, and a
//! `CreationPolicy` on a resource of another type whose resource an owning service really
//! holds), how many signals a gate asks for (one and two, so a partly signalled gate is
//! observed still waiting), the types behind the gates (buckets, parameters and topics an
//! owning service holds, handles and a topic policy no service provisions), how the
//! dependency is stated (`DependsOn` as a name and as a list, and a `Ref` inside a property
//! list), and the resources that wait on NOTHING, which are complete while every gate is
//! still open.
//!
//! `resource_signal_waits.rs` pins what ONE declaration asks of a wait and the two-gate
//! diamond behind `CreationPolicy`; `stack_event_timeline.rs` pins the event timeline a held
//! graph produces. What is pinned HERE is the ready-set: every gate a create can start is
//! open in one pass of reads, whatever opened it, and releasing one releases its own branch
//! alone.

use crate::harness::{call, create_stack, member, router, wait_for_the_transition, xml_text};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

/// `SignalResource` for one resource of one stack.
pub(crate) async fn signal(
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

/// `DescribeStackResource` for one resource: the HTTP status and the body, so a case can
/// assert either an answer or the refusal a resource the walk has not started gets.
pub(crate) async fn describe_resource(
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

/// The `ResourceStatus` a stack reports for one of its resources right now.
pub(crate) async fn resource_status_of(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (http_status, body) = describe_resource(router, stack, logical_id).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected {logical_id} to be described, got {http_status} {body}"
    );
    xml_text(&body, "ResourceStatus").unwrap_or_default()
}

/// The `StackStatus` a stack reports right now.
pub(crate) async fn stack_status_of(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
) -> String {
    let (http_status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {http_status} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// One gate of a row: the resource that waits for signals, the `UniqueId` of each signal its
/// `Count` asks for, and the resources whose creation waits behind it.
///
/// Shared with `unsignalled_gate_hold.rs`, which reads the same gates over time and declares
/// nothing behind them.
pub(crate) struct Gate {
    pub(crate) logical_id: &'static str,
    pub(crate) signals: &'static [&'static str],
    pub(crate) behind: &'static [&'static str],
}

/// One row: a template whose gates wait on nothing and on each other not at all.
struct GateRow {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The resources that wait on no gate: created while every gate is still open.
    ready: &'static [&'static str],
    /// The gates, in the order the row releases them.
    gates: &'static [Gate],
    /// The resources that wait behind EVERY gate: reported only once the last gate is
    /// released.
    joins: &'static [&'static str],
}

/// The policy document the topic-policy join declares: one statement on the topic that waits
/// on nothing, written as the intrinsic the deployment resolves.
const JOIN_POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{
    "Sid":"GrantTheAccount","Effect":"Allow","Principal":"*","Action":"sns:Publish",
    "Resource":"*"}]}"#;

/// The two-gate diamond: a bucket behind one gate, a parameter behind the other, a topic that
/// waits on nothing, and a topic policy joining both branches — the branches stated by
/// `DependsOn`, the join by a `DependsOn` list plus a `Ref` inside a property list.
fn two_gate_diamond() -> String {
    format!(
        r#"{{"Resources":{{
            "GateA":{{"Type":"AWS::CloudFormation::WaitCondition",
              "CreationPolicy":{{"ResourceSignal":{{"Count":1,"Timeout":"PT10M"}}}}}},
            "GateB":{{"Type":"AWS::CloudFormation::WaitCondition",
              "CreationPolicy":{{"ResourceSignal":{{"Count":1,"Timeout":"PT10M"}}}}}},
            "Fanout":{{"Type":"AWS::SNS::Topic",
              "Properties":{{"TopicName":"ready-set-two-fanout"}}}},
            "BucketA":{{"Type":"AWS::S3::Bucket","DependsOn":"GateA",
              "Properties":{{"BucketName":"ready-set-two-a"}}}},
            "ParamB":{{"Type":"AWS::SSM::Parameter","DependsOn":"GateB",
              "Properties":{{"Name":"/ready-set-two/b","Type":"String",
                "Value":"behind the second gate"}}}},
            "JoinPolicy":{{"Type":"AWS::SNS::TopicPolicy",
              "DependsOn":["BucketA","ParamB"],
              "Properties":{{"Topics":[{{"Ref":"Fanout"}}],
                "PolicyDocument":{JOIN_POLICY_DOCUMENT}}}}}}}}}"#
    )
}

/// The three-gate row, each gate opened by a DIFFERENT declaration: a `CreationPolicy` asking
/// for two signals, a wait condition's own properties asking for one, and a `CreationPolicy`
/// on an `AWS::SNS::Topic` — a resource Amazon SNS really holds — asking for one.
const THREE_MIXED_GATES: &str = r#"{"Resources":{
    "HandleTwo":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "Standalone":{"Type":"AWS::SNS::Topic",
      "Properties":{"TopicName":"ready-set-three-standalone"}},
    "GateOne":{"Type":"AWS::CloudFormation::WaitCondition",
      "CreationPolicy":{"ResourceSignal":{"Count":2,"Timeout":"PT15M"}}},
    "GateTwo":{"Type":"AWS::CloudFormation::WaitCondition",
      "Properties":{"Handle":{"Ref":"HandleTwo"},"Timeout":"300","Count":"1"}},
    "GateThree":{"Type":"AWS::SNS::Topic",
      "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT5M"}},
      "Properties":{"TopicName":"ready-set-three-gate"}},
    "BranchOne":{"Type":"AWS::SNS::Topic","DependsOn":"GateOne",
      "Properties":{"TopicName":"ready-set-three-one"}},
    "BranchTwo":{"Type":"AWS::S3::Bucket","DependsOn":"GateTwo",
      "Properties":{"BucketName":"ready-set-three-two"}},
    "BranchThree":{"Type":"AWS::SNS::Topic","DependsOn":"GateThree",
      "Properties":{"TopicName":"ready-set-three-three"}},
    "Join":{"Type":"AWS::SNS::Topic",
      "DependsOn":["BranchOne","BranchTwo","BranchThree"],
      "Properties":{"TopicName":"ready-set-three-join"}}}}"#;

fn gate_rows() -> Vec<GateRow> {
    vec![
        GateRow {
            case: "two gates a CreationPolicy opens, a bucket and a parameter behind them and \
                   a topic policy joining both branches",
            stack: "ready-set-two",
            template: two_gate_diamond(),
            ready: &["Fanout"],
            gates: &[
                Gate {
                    logical_id: "GateA",
                    signals: &["gate-a"],
                    behind: &["BucketA"],
                },
                Gate {
                    logical_id: "GateB",
                    signals: &["gate-b"],
                    behind: &["ParamB"],
                },
            ],
            joins: &["JoinPolicy"],
        },
        GateRow {
            case: "three gates of three different declarations, one of them asking for two \
                   signals",
            stack: "ready-set-three",
            template: THREE_MIXED_GATES.to_string(),
            ready: &["HandleTwo", "Standalone"],
            gates: &[
                Gate {
                    logical_id: "GateOne",
                    signals: &["one-host-a", "one-host-b"],
                    behind: &["BranchOne"],
                },
                Gate {
                    logical_id: "GateTwo",
                    signals: &["two-host"],
                    behind: &["BranchTwo"],
                },
                Gate {
                    logical_id: "GateThree",
                    signals: &["three-host"],
                    behind: &["BranchThree"],
                },
            ],
            joins: &["Join"],
        },
    ]
}

#[tokio::test]
async fn every_gate_a_create_can_start_is_open_at_once_and_releases_its_own_branch() {
    for row in gate_rows() {
        let case = row.case;
        let stack = row.stack;
        let router = router();
        let (http_status, created) = create_stack(&router, stack, &row.template, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {created}"
        );

        // Past the instant the clock alone would have completed the create: what holds the
        // operation now is the gates, not a timer.
        wait_for_the_transition().await;

        // ONE pass of reads sees every gate in flight, and the resources that wait on
        // nothing already created beside them.
        for gate in row.gates {
            let reported = resource_status_of(case, &router, stack, gate.logical_id).await;
            assert_eq!(
                reported,
                resource_status::CREATE_IN_PROGRESS,
                "case {case:?}: expected {} to be open at the same time as every other gate, \
                 got {reported:?}",
                gate.logical_id
            );
        }
        for logical_id in row.ready {
            let reported = resource_status_of(case, &router, stack, logical_id).await;
            assert_eq!(
                reported,
                resource_status::CREATE_COMPLETE,
                "case {case:?}: expected {logical_id}, which waits on no gate, to be created \
                 while the gates hold, got {reported:?}"
            );
        }
        for gate in row.gates {
            for logical_id in gate.behind.iter().chain(row.joins.iter()) {
                let (http_status, body) = describe_resource(&router, stack, logical_id).await;
                assert_eq!(
                    http_status, 400,
                    "case {case:?}: expected {logical_id} not to be reported while {} holds, \
                     got {http_status} {body}",
                    gate.logical_id
                );
            }
        }

        // Each gate is released by its OWN signals, and by the LAST of them: a gate whose
        // `Count` asks for more than one goes on waiting until that many have arrived.
        for (released, gate) in row.gates.iter().enumerate() {
            let Some((last, all_but_last)) = gate.signals.split_last() else {
                panic!(
                    "case {case:?}: bad row \u{2014} {} asks for no signal at all",
                    gate.logical_id
                )
            };
            for unique_id in all_but_last {
                let (http_status, signalled) =
                    signal(&router, stack, gate.logical_id, unique_id).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected the signal {unique_id} for {} to be accepted, \
                     got {http_status} {signalled}",
                    gate.logical_id
                );
                let reported = resource_status_of(case, &router, stack, gate.logical_id).await;
                assert_eq!(
                    reported,
                    resource_status::CREATE_IN_PROGRESS,
                    "case {case:?}: expected {} to wait for all {} signals its Count asks \
                     for, got {reported:?} after {}",
                    gate.logical_id,
                    gate.signals.len(),
                    unique_id
                );
            }
            let (http_status, signalled) = signal(&router, stack, gate.logical_id, last).await;
            assert_eq!(
                http_status, 200,
                "case {case:?}: expected the signal {last} for {} to be accepted, got \
                 {http_status} {signalled}",
                gate.logical_id
            );
            // Past the instant a transition this signal made due would fall at, read through
            // the same helper the create path above uses: what the reads below assert is the
            // gate this signal released and the gates it did not, never which of them the
            // clock had got to.
            wait_for_the_transition().await;

            // This gate and its own branch are done; every gate the row has not signalled
            // yet is still open, with its branch unreported.
            for logical_id in std::iter::once(&gate.logical_id).chain(gate.behind.iter()) {
                let reported = resource_status_of(case, &router, stack, logical_id).await;
                assert_eq!(
                    reported,
                    resource_status::CREATE_COMPLETE,
                    "case {case:?}: expected {logical_id} to be created once {} had its \
                     signals, got {reported:?}",
                    gate.logical_id
                );
            }
            for waiting in row.gates.iter().skip(released + 1) {
                let reported = resource_status_of(case, &router, stack, waiting.logical_id).await;
                assert_eq!(
                    reported,
                    resource_status::CREATE_IN_PROGRESS,
                    "case {case:?}: expected {} to go on waiting for its own signals after {} \
                     was released, got {reported:?}",
                    waiting.logical_id,
                    gate.logical_id
                );
                for logical_id in waiting.behind {
                    let (http_status, body) = describe_resource(&router, stack, logical_id).await;
                    assert_eq!(
                        http_status, 400,
                        "case {case:?}: expected {logical_id} not to be reported while {} \
                         holds, got {http_status} {body}",
                        waiting.logical_id
                    );
                }
            }

            // The join waits for the LAST of its predecessors, and the stack for the last of
            // its gates.
            let last_gate = released + 1 == row.gates.len();
            if !last_gate {
                for logical_id in row.joins {
                    let (http_status, body) = describe_resource(&router, stack, logical_id).await;
                    assert_eq!(
                        http_status, 400,
                        "case {case:?}: expected {logical_id} not to be reported while a \
                         predecessor is still waiting, got {http_status} {body}"
                    );
                }
                let reported = stack_status_of(case, &router, stack).await;
                assert_eq!(
                    reported,
                    status::CREATE_IN_PROGRESS,
                    "case {case:?}: expected the create to go on waiting at the gates it has \
                     not been signalled for, got {reported:?}"
                );
            }
        }

        // The last signal finishes the walk: the whole graph is created.
        wait_for_the_transition().await;
        let reported = stack_status_of(case, &router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the signalled create to complete, got {reported:?}"
        );
        let every_resource = row
            .ready
            .iter()
            .chain(row.joins.iter())
            .copied()
            .chain(
                row.gates
                    .iter()
                    .flat_map(|gate| std::iter::once(&gate.logical_id).chain(gate.behind.iter()))
                    .copied(),
            );
        for logical_id in every_resource {
            let reported = resource_status_of(case, &router, stack, logical_id).await;
            assert_eq!(
                reported,
                resource_status::CREATE_COMPLETE,
                "case {case:?}: expected {logical_id} to be CREATE_COMPLETE once the create \
                 completed, got {reported:?}"
            );
        }
    }
}
