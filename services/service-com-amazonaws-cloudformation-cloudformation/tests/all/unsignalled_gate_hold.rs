// SPDX-License-Identifier: CC0-1.0

//! How long an UNSIGNALLED resource-signal gate stays in flight, and addressable, end to end
//! over the awsQuery wire.
//!
//! RULE: **a resource whose declaration waits for success signals keeps its own
//! `*_IN_PROGRESS` status — and keeps being reported by every per-resource read of the stack
//! — for the whole span that declaration asked to wait, so a client may poll one logical id
//! until its signal arrives; a per-resource read is refused only for a logical id the
//! stack's deployment does not declare.** CloudFormation "doesn't proceed with a stack
//! creation or update until resources receive the required number of signals or the timeout
//! period is exceeded" (`SignalResource`), and the span it waits for is the one the
//! declaration states: an `AWS::CloudFormation::WaitCondition`'s `Timeout` ("the number of
//! seconds to wait", maximum twelve hours) or a `CreationPolicy`'s `ResourceSignal.Timeout`
//! ("in ISO8601 duration format", "Default: PT5M", "Maximum: PT12H").
//!
//! What a client does with that: the whole `cfn-signal` bootstrap pattern is a host that
//! polls its own gate until it is open and then signals it, so a gate that stopped being
//! reportable part-way through the span it asked for turns a normal wait into a fatal
//! "no such resource in this stack" for every tool that watches a deploy.
//!
//! The rows span what the rule generalises over: which declaration asks for the wait (a
//! `CreationPolicy`'s `ResourceSignal` and a wait condition's own properties), how the span
//! is spelled (an ISO 8601 duration, a number of seconds as a string and as a number, and a
//! declaration that states none at all, which falls back to the documented default), how
//! many signals the gate asks for, and how many independent gates one create opens. Each row
//! is read at several ages inside the span it asked for, up to one second short of it, and
//! the far bound — the first instant past that span — is the second case below.
//!
//! No case sleeps a span measured in seconds: each one moves the barrier's own `opened_at`
//! back through `held_signal_waits::age_the_wait`, exactly as that suite does.

use std::time::Duration;

use service_com_amazonaws_cloudformation_cloudformation::state::wait_conditions::DEFAULT_TIMEOUT;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::harness::{call, create_stack, member, router_with_engine, xml_text};
use crate::held_signal_waits::age_the_wait;
// The readers and the gate record this suite needs are the ones the ready-set suite already
// drives the same resources through, so there is one spelling of each: `Gate::behind` is the
// ready set's own question (what is NOT reported while a gate holds) and is left empty here,
// where the rule is about the gate itself.
use crate::in_flight_resource_status::resource_status_in_stack;
use crate::independent_gate_ready_set::{
    describe_resource, resource_status_of, signal, stack_status_of, Gate,
};

/// One row: a template whose gates wait on nothing, and the span its declarations ask the
/// wait to run for.
struct HoldRow {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The gates the template declares, every one of them unsignalled while the row holds.
    gates: &'static [Gate],
    /// The resource that waits on no gate, which is created while the gates hold.
    ready: &'static str,
    /// The span every gate of this row asks to wait for.
    asked_for: Duration,
}

/// Two mutually independent gates a `CreationPolicy`'s `ResourceSignal` opens for ten
/// minutes, with a topic that waits on nothing beside them.
fn two_iso_gates() -> String {
    r#"{"Resources":{
        "GateA":{"Type":"AWS::CloudFormation::WaitCondition",
          "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT10M"}}},
        "GateB":{"Type":"AWS::CloudFormation::WaitCondition",
          "CreationPolicy":{"ResourceSignal":{"Count":1,"Timeout":"PT10M"}}},
        "Fanout":{"Type":"AWS::SNS::Topic",
          "Properties":{"TopicName":"hold-iso-fanout"}}}}"#
        .to_string()
}

/// One gate whose own `Timeout` property states the seconds as a STRING, and whose `Count`
/// asks for two signals.
fn string_seconds_gate() -> String {
    r#"{"Resources":{
        "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
        "Gate":{"Type":"AWS::CloudFormation::WaitCondition",
          "Properties":{"Handle":{"Ref":"Handle"},"Timeout":"600","Count":"2"}},
        "Fanout":{"Type":"AWS::SNS::Topic",
          "Properties":{"TopicName":"hold-string-fanout"}}}}"#
        .to_string()
}

/// One gate whose own `Timeout` property states the seconds as a NUMBER.
fn number_seconds_gate() -> String {
    r#"{"Resources":{
        "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
        "Gate":{"Type":"AWS::CloudFormation::WaitCondition",
          "Properties":{"Handle":{"Ref":"Handle"},"Timeout":900,"Count":1}},
        "Fanout":{"Type":"AWS::SNS::Topic",
          "Properties":{"TopicName":"hold-number-fanout"}}}}"#
        .to_string()
}

/// One gate whose `ResourceSignal` states no `Timeout` at all, so it waits for the span that
/// attribute documents as its default.
fn unstated_gate() -> String {
    r#"{"Resources":{
        "Gate":{"Type":"AWS::CloudFormation::WaitCondition",
          "CreationPolicy":{"ResourceSignal":{"Count":1}}},
        "Fanout":{"Type":"AWS::SNS::Topic",
          "Properties":{"TopicName":"hold-unstated-fanout"}}}}"#
        .to_string()
}

fn hold_rows() -> Vec<HoldRow> {
    vec![
        HoldRow {
            case: "two independent gates a CreationPolicy opens for ten minutes",
            stack: "hold-two-iso",
            template: two_iso_gates(),
            gates: &[
                Gate {
                    logical_id: "GateA",
                    signals: &["gate-a-host"],
                    behind: &[],
                },
                Gate {
                    logical_id: "GateB",
                    signals: &["gate-b-host"],
                    behind: &[],
                },
            ],
            ready: "Fanout",
            asked_for: Duration::from_secs(600),
        },
        HoldRow {
            case: "a wait condition whose Timeout is seconds spelled as a string, waiting for \
                   two signals",
            stack: "hold-string-seconds",
            template: string_seconds_gate(),
            gates: &[Gate {
                logical_id: "Gate",
                signals: &["host-one", "host-two"],
                behind: &[],
            }],
            ready: "Fanout",
            asked_for: Duration::from_secs(600),
        },
        HoldRow {
            case: "a wait condition whose Timeout is seconds spelled as a number",
            stack: "hold-number-seconds",
            template: number_seconds_gate(),
            gates: &[Gate {
                logical_id: "Gate",
                signals: &["host"],
                behind: &[],
            }],
            ready: "Fanout",
            asked_for: Duration::from_secs(900),
        },
        HoldRow {
            case: "a ResourceSignal that states no Timeout, held to the documented default",
            stack: "hold-unstated",
            template: unstated_gate(),
            gates: &[Gate {
                logical_id: "Gate",
                signals: &["host"],
                behind: &[],
            }],
            ready: "Fanout",
            asked_for: DEFAULT_TIMEOUT,
        },
    ]
}

/// The ages INSIDE the span a declaration asked for that a row is read at: the instant the
/// wait opened, a poll or two in, past the span a stack operation's own clock owes, and one
/// second short of the span the declaration asked for — so a row that fails names the age its
/// gate stopped being reported at.
fn ages_inside(asked_for: Duration) -> Vec<Duration> {
    let mut ages = vec![
        Duration::ZERO,
        Duration::from_secs(3),
        Duration::from_secs(20),
    ];
    if let Some(just_inside) = asked_for.checked_sub(Duration::from_secs(1)) {
        ages.push(just_inside);
    }
    ages.retain(|age| *age < asked_for);
    ages.sort();
    ages.dedup();
    // A row whose declaration asked for a span too short to observe inside would skip every
    // assertion below, so it is a bad row rather than a passing one.
    assert!(
        !ages.is_empty(),
        "a row must ask for a span there is an instant inside, got {asked_for:?}"
    );
    ages
}

/// The case name an observation is read under: the row, and how far into the wait the read
/// is taken — so a failure inside the shared readers names the age as well as the row.
fn at(case: &str, age: Duration) -> String {
    format!("{case}, {age:?} into the wait")
}

/// A logical id no row's template declares, which is what the per-resource refusal is
/// reserved for.
const UNDECLARED: &str = "NotInThisTemplate";

#[tokio::test]
async fn an_unsignalled_gate_stays_reported_for_the_whole_span_it_asked_for() {
    for row in hold_rows() {
        let case = row.case;
        let stack = row.stack;
        let (router, engine) = router_with_engine();
        let (http_status, created) = create_stack(&router, stack, &row.template, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {created}"
        );

        let mut aged = Duration::ZERO;
        for age in ages_inside(row.asked_for) {
            // The wait is moved back to the age this observation is taken at, rather than
            // waited out: what each read then sees is the stack at that instant of the wait.
            age_the_wait(&engine, stack, age - aged);
            aged = age;

            for gate in row.gates {
                let reported =
                    resource_status_of(&at(case, age), &router, stack, gate.logical_id).await;
                assert_eq!(
                    reported,
                    resource_status::CREATE_IN_PROGRESS,
                    "case {case:?}: expected the unsignalled {} to still be \
                     CREATE_IN_PROGRESS {age:?} into the {:?} it asked for, got {reported:?}",
                    gate.logical_id,
                    row.asked_for
                );
                let in_stack =
                    resource_status_in_stack(&at(case, age), &router, stack, gate.logical_id)
                        .await;
                assert_eq!(
                    in_stack, reported,
                    "case {case:?}: expected the whole-stack read to agree with the single \
                     resource read for {} {age:?} in, got {in_stack:?}",
                    gate.logical_id
                );
            }
            // ...and the resource that waits on no gate is reported created beside them, so
            // the whole projection stands rather than just the gates.
            let ready = resource_status_of(&at(case, age), &router, stack, row.ready).await;
            assert_eq!(
                ready,
                resource_status::CREATE_COMPLETE,
                "case {case:?}: expected {}, which waits on no gate, to stay created {age:?} \
                 in, got {ready:?}",
                row.ready
            );
            // The stack is still running the create the gates hold open.
            let reported = stack_status_of(case, &router, stack).await;
            assert_eq!(
                reported,
                status::CREATE_IN_PROGRESS,
                "case {case:?}: expected the create to still be in flight {age:?} in, got \
                 {reported:?}"
            );
            // The per-resource refusal is reserved for a logical id the stack does not
            // declare, which is the one read that must fail here.
            let (http_status, body) = describe_resource(&router, stack, UNDECLARED).await;
            assert_eq!(
                http_status, 400,
                "case {case:?}: expected an undeclared logical id to be refused {age:?} in, \
                 got {http_status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected the undeclared logical id's refusal to be a \
                 ValidationError, got {body}"
            );
        }

        // The signals the declarations ask for still release the gates after the hold, and
        // the create completes: every gate is read back as created.
        for gate in row.gates {
            for unique_id in gate.signals {
                let (http_status, signalled) =
                    signal(&router, stack, gate.logical_id, unique_id).await;
                assert_eq!(
                    http_status, 200,
                    "case {case:?}: expected the signal {unique_id} for {} to be accepted, \
                     got {http_status} {signalled}",
                    gate.logical_id
                );
            }
        }
        crate::harness::wait_for_the_transition().await;
        let reported = stack_status_of(case, &router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_COMPLETE,
            "case {case:?}: expected the signalled create to complete after the hold, got \
             {reported:?}"
        );
        for gate in row.gates {
            let reported =
                resource_status_of(&at(case, aged), &router, stack, gate.logical_id).await;
            assert_eq!(
                reported,
                resource_status::CREATE_COMPLETE,
                "case {case:?}: expected the signalled {} to be created, got {reported:?}",
                gate.logical_id
            );
        }
    }
}

#[tokio::test]
async fn a_gate_nothing_signals_fails_the_create_once_the_span_it_asked_for_is_over() {
    for row in hold_rows() {
        let case = row.case;
        let stack = row.stack;
        let (router, engine) = router_with_engine();
        let (http_status, created) = create_stack(&router, stack, &row.template, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {created}"
        );

        // One instant past the span the declaration asked for: the far bound, where the wait
        // is over and the clock owes the failure "if the wait condition doesn't receive the
        // specified number of success signals before the Timeout period expires". What AWS
        // then does with such a stack — roll the create back — is a different rule and not
        // what this case reads: the far bound asserted here is only that the wait HAS an end.
        age_the_wait(&engine, stack, row.asked_for + Duration::from_secs(1));
        let reported = stack_status_of(case, &router, stack).await;
        assert_eq!(
            reported,
            status::CREATE_FAILED,
            "case {case:?}: expected the create to fail once the {:?} the gates asked for was \
             over, got {reported:?}",
            row.asked_for
        );
        let (_, body) = call(&router, "DescribeStacks", &[member("StackName", stack)]).await;
        let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: expected the failed create to say what wait ran out, got {reason:?}"
        );
    }
}
