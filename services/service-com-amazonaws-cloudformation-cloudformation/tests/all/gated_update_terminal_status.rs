// SPDX-License-Identifier: CC0-1.0

//! An UPDATE held at a resource-signal wait that is never satisfied reaches a TERMINAL
//! `StackStatus` on its own, as a client's waiter polls for it.
//!
//! RULE: CloudFormation "doesn't proceed with a stack creation or update until resources
//! receive the required number of signals or the timeout period is exceeded"
//! (`SignalResource`), and "if the wait condition doesn't receive the specified number of
//! success signals before the `Timeout` period expires, CloudFormation assumes that the wait
//! condition has failed" — so an update whose gate gets FEWER success signals than its
//! `Count` within its `Timeout` fails, and "if the stack update fails, CloudFormation rolls
//! back the stack to the previous stack configuration": the stack settles on
//! `UPDATE_ROLLBACK_COMPLETE` with `Stack$StackStatusReason` carrying the wait's own
//! diagnosis, the resources the update had already changed in the services that own them are
//! put back, the ones only the failed update declared are gone from
//! `ListStackResources`, the stack's `UPDATE_ROLLBACK_IN_PROGRESS` event is recorded, and the
//! rolled-back stack still takes a further update. Nothing a client does is needed for any of
//! it: the expiry is the clock's.
//!
//! What this suite pins, which no other here does, is the CLIENT'S side of that rule: a
//! waiter — `aws cloudformation wait stack-update-complete`, botocore's
//! `stack_update_complete`, any script that polls — re-reads `DescribeStacks` until the
//! status is one the model's `StackStatus` enum ends an update in, and that poll loop must
//! TERMINATE. Every row therefore drives the whole sequence over the wire with no reach into
//! the store and no fixed sleep for the outcome: the waiter polls, and a row that never
//! settles fails on its own bounded deadline naming the statuses it did see.
//!
//! The rows span what the rule generalises over: how the declaration spells `Timeout` (a
//! string and a number) and `Count` (likewise), whether the gate is ordered after the
//! resource the update changes (`DependsOn`) or independent of it, how many of the signals it
//! waits for actually arrive (none, and one of the two it asks for), and what the rollback has
//! to undo (a changed resource alone, and a changed one beside one the update added).
//! Declarations asking to wait a span a test cannot sit out are the table in
//! `held_signal_waits.rs`, and the span one is held for is the table in
//! `update_rollback_recovery.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_stack, member, router, wait_for_the_transition};
use crate::held_signal_waits::listed_resources;
use crate::update_rollback_recovery::{
    parameter, parameter_value, reason_of, stack_events, status_of, template,
};

/// The logical id the gate is declared under, and the one a signal names it by.
const GATE: &str = "Gate";

/// The logical id of the handle the gate's `Handle` property points at.
const HANDLE: &str = "Handle";

/// The logical id of the resource every row's update CHANGES.
const CHANGED: &str = "Param";

/// The logical id of the resource a row's update ADDS.
const ADDED: &str = "Added";

/// The statuses an UPDATE can END in, from the model's `StackStatus` enum
/// (`com.amazonaws.cloudformation#StackStatus`): the completed update, the failed one, and the
/// two the rollback of a failed one settles in. Every other value of that enum for the update
/// path is an `*_IN_PROGRESS`, which is a waiter's cue to poll again.
const TERMINAL_UPDATE_STATUSES: [&str; 4] = [
    status::UPDATE_COMPLETE,
    status::UPDATE_FAILED,
    status::UPDATE_ROLLBACK_COMPLETE,
    status::UPDATE_ROLLBACK_FAILED,
];

/// How often a waiter re-reads `DescribeStacks`.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How much longer than the span a declaration asked for a waiter polls before calling the
/// update wedged: room for the failure's own rollback and for a loaded machine, so a row that
/// fails here failed because nothing terminal was ever reported.
const CONVERGENCE_MARGIN: Duration = Duration::from_secs(5);

/// One row: how the gate the update adds is declared, how many of its signals arrive, and what
/// the rollback of that update has to undo.
struct GatedUpdateCase {
    case: &'static str,
    stack: &'static str,
    /// `Timeout` exactly as the declaration spells it — a template may spell a numeric
    /// property as a number or as a string, and both deploy.
    timeout: Value,
    /// The span that `Timeout` asks for, which is what bounds the waiter's own deadline.
    asked_for: Duration,
    /// `Count` exactly as the declaration spells it.
    count: Value,
    /// The number of success signals `count` spells.
    expects: usize,
    /// Whether the gate is ordered AFTER the resource the update changes.
    depends_on_the_changed_resource: bool,
    /// How many DISTINCT success signals this row delivers — fewer than [`Self::expects`] in
    /// every row, which is what leaves the gate unsatisfied.
    signals: usize,
    /// The parameter the stack is created with: its name, and the value it holds before the
    /// update.
    created: (&'static str, &'static str),
    /// The value the update writes for that parameter.
    updated: &'static str,
    /// The parameter the update ADDS, which the rollback has to remove again.
    added: Option<(&'static str, &'static str)>,
    /// The value a further, valid update writes once the stack has rolled back.
    then: &'static str,
}

fn gated_update_cases() -> Vec<GatedUpdateCase> {
    vec![
        GatedUpdateCase {
            case: "a gate ordered after the changed resource, Timeout and Count as strings, \
                   nothing signalled",
            stack: "gated-ordered",
            timeout: json!("1"),
            asked_for: Duration::from_secs(1),
            count: json!("1"),
            expects: 1,
            depends_on_the_changed_resource: true,
            signals: 0,
            created: ("/gated-ordered/p", "v1"),
            updated: "v2",
            added: None,
            then: "v3",
        },
        GatedUpdateCase {
            case: "an independent gate, Timeout and Count as numbers, over a changed and an \
                   added resource",
            stack: "gated-independent",
            timeout: json!(2),
            asked_for: Duration::from_secs(2),
            count: json!(1),
            expects: 1,
            depends_on_the_changed_resource: false,
            signals: 0,
            created: ("/gated-independent/p", "before"),
            updated: "after",
            added: Some(("/gated-independent/added", "added")),
            then: "later",
        },
        GatedUpdateCase {
            case: "a gate whose Count outruns the signals that arrive",
            stack: "gated-partly-signalled",
            timeout: json!("1"),
            asked_for: Duration::from_secs(1),
            count: json!(2),
            expects: 2,
            depends_on_the_changed_resource: true,
            signals: 1,
            created: ("/gated-partly-signalled/p", "one"),
            updated: "two",
            added: Some(("/gated-partly-signalled/added", "added")),
            then: "three",
        },
    ]
}

/// The update's template: the stack's own parameter at the update's new value, whatever the
/// update adds, the handle, and the gate that waits for signals the row does not deliver in
/// full.
fn gated_template(case: &GatedUpdateCase) -> String {
    let mut resources: Map<String, Value> = Map::new();
    resources.insert(
        CHANGED.to_string(),
        parameter(case.created.0, case.updated),
    );
    if let Some((name, value)) = case.added {
        resources.insert(ADDED.to_string(), parameter(name, value));
    }
    resources.insert(
        HANDLE.to_string(),
        json!({"Type": "AWS::CloudFormation::WaitConditionHandle"}),
    );
    let mut gate: Map<String, Value> = Map::new();
    gate.insert(
        "Type".to_string(),
        json!("AWS::CloudFormation::WaitCondition"),
    );
    if case.depends_on_the_changed_resource {
        gate.insert("DependsOn".to_string(), json!(CHANGED));
    }
    gate.insert(
        "Properties".to_string(),
        json!({
            "Handle": {"Ref": HANDLE},
            "Timeout": case.timeout.clone(),
            "Count": case.count.clone(),
        }),
    );
    resources.insert(GATE.to_string(), Value::Object(gate));
    template(Value::Object(resources))
}

/// Poll `DescribeStacks` as a client's waiter does, and answer the terminal `StackStatus` the
/// stack settled on.
///
/// A waiter that never gets one is the defect this suite is about, so the loop has its own
/// deadline and reports every status it did see on the way — which is what a client would have
/// had to work with.
async fn poll_for_a_terminal_status(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    within: Duration,
) -> String {
    let deadline = Instant::now() + within;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reported = status_of(router, stack).await;
        if TERMINAL_UPDATE_STATUSES.contains(&reported.as_str()) {
            return reported;
        }
        if seen.last().map(String::as_str) != Some(reported.as_str()) {
            seen.push(reported);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the gated update to reach a terminal StackStatus within \
             {within:?}, got {seen:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Deliver the success signals a row sends, each under its own `UniqueId` so each one counts:
/// "if you send multiple signals to a single resource (such as signaling a wait condition),
/// each signal requires a different unique ID".
async fn signal_the_gate(case: &GatedUpdateCase, router: &MultiServiceRouter) {
    for index in 0..case.signals {
        let unique_id = format!("{}-{index}", case.stack);
        let (http_status, body) = call(
            router,
            "SignalResource",
            &[
                member("StackName", case.stack),
                member("LogicalResourceId", GATE),
                member("UniqueId", &unique_id),
                member("Status", "SUCCESS"),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected the signal {unique_id:?} to be accepted, got {http_status} \
             {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_whose_gate_is_never_satisfied_settles_on_a_rolled_back_status() {
    for case in gated_update_cases() {
        assert!(
            case.signals < case.expects,
            "case {:?}: a row must leave the gate unsatisfied, got {} of {} signals",
            case.case,
            case.signals,
            case.expects
        );
        let router = router();

        // The stack as it stands before the update: one parameter, really created in the
        // service that owns it.
        let created = template(json!({CHANGED: parameter(case.created.0, case.created.1)}));
        let (http_status, body) = create_stack(&router, case.stack, &created, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;
        let settled = status_of(&router, case.stack).await;
        assert_eq!(
            settled,
            status::CREATE_COMPLETE,
            "case {:?}: expected the create to complete before the update, got {settled:?}",
            case.case
        );
        let before = parameter_value(&router, case.created.0).await;
        assert_eq!(
            before.as_deref(),
            Some(case.created.1),
            "case {:?}: expected the created stack's own value for {:?}, got {before:?}",
            case.case,
            case.created.0
        );

        // The update that adds the gate, and the signals the row delivers — fewer than the
        // gate waits for.
        let gated = gated_template(&case);
        let started = Instant::now();
        let (http_status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", case.stack),
                member("TemplateBody", &gated),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
            case.case
        );
        signal_the_gate(&case, &router).await;

        // While the gate holds, the update is in flight and what it already wrote in the
        // owning service is what that service holds. A read that lost the race to the gate's
        // own expiry is held to nothing, since by then the clock owed the failure.
        let in_flight = status_of(&router, case.stack).await;
        let applied = parameter_value(&router, case.created.0).await;
        if started.elapsed() < case.asked_for {
            assert_eq!(
                in_flight,
                status::UPDATE_IN_PROGRESS,
                "case {:?}: expected the update to be held at its gate, got {in_flight:?}",
                case.case
            );
            assert_eq!(
                applied.as_deref(),
                Some(case.updated),
                "case {:?}: expected the update's own value for {:?} while it is held, got \
                 {applied:?}",
                case.case,
                case.created.0
            );
        }

        // The waiter's own loop: the gate's Timeout runs out with no call of the client's, and
        // the stack settles on the rolled-back status, carrying the wait's diagnosis.
        let terminal = poll_for_a_terminal_status(
            case.case,
            &router,
            case.stack,
            case.asked_for + CONVERGENCE_MARGIN,
        )
        .await;
        assert_eq!(
            terminal,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {:?}: expected the failed update to be rolled back, got {terminal:?}",
            case.case
        );
        let reason = reason_of(&router, case.stack).await;
        let expected_reason = format!(
            "WaitCondition timed out. Received {} conditions when expecting {}",
            case.signals, case.expects
        );
        assert_eq!(
            reason, expected_reason,
            "case {:?}: expected the stack to report what the gate was waiting for, got \
             {reason:?}",
            case.case
        );

        // The rollback put back what the update had changed and removed what it had added.
        let restored = parameter_value(&router, case.created.0).await;
        assert_eq!(
            restored.as_deref(),
            Some(case.created.1),
            "case {:?}: expected {:?} back at its pre-update value, got {restored:?}",
            case.case,
            case.created.0
        );
        if let Some((name, _)) = case.added {
            let dropped = parameter_value(&router, name).await;
            assert_eq!(
                dropped, None,
                "case {:?}: expected the parameter the failed update added to be gone, got \
                 {dropped:?}",
                case.case
            );
        }

        // ...and the stack says so: the resource it keeps is still listed, the ones only the
        // failed update declared are not, and the rollback has the stack's own event.
        let listed = listed_resources(&router, case.stack).await;
        assert!(
            listed.contains(&CHANGED.to_string()),
            "case {:?}: expected the resource the stack keeps to still be listed, got \
             {listed:?}",
            case.case
        );
        let mut only_the_failed_update = vec![GATE, HANDLE];
        if case.added.is_some() {
            only_the_failed_update.push(ADDED);
        }
        for gone in only_the_failed_update {
            assert!(
                !listed.contains(&gone.to_string()),
                "case {:?}: expected {gone:?}, which only the failed update declared, to be \
                 gone from the stack's resources, got {listed:?}",
                case.case
            );
        }
        let events = stack_events(case.case, &router, case.stack).await;
        for owed in [
            status::UPDATE_FAILED,
            status::UPDATE_ROLLBACK_IN_PROGRESS,
            status::UPDATE_ROLLBACK_COMPLETE,
        ] {
            assert!(
                events.contains(&(case.stack.to_string(), owed.to_string())),
                "case {:?}: expected the stack's own {owed} event, got {events:?}",
                case.case
            );
        }

        // The rolled-back stack is not wedged: a further valid update is accepted and its own
        // waiter terminates on the completed status.
        let next = template(json!({CHANGED: parameter(case.created.0, case.then)}));
        let (http_status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", case.stack),
                member("TemplateBody", &next),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected the stack to take a further update, got {http_status} {body}",
            case.case
        );
        let updated = poll_for_a_terminal_status(
            case.case,
            &router,
            case.stack,
            CONVERGENCE_MARGIN,
        )
        .await;
        assert_eq!(
            updated,
            status::UPDATE_COMPLETE,
            "case {:?}: expected the further update to complete, got {updated:?}",
            case.case
        );
        let written = parameter_value(&router, case.created.0).await;
        assert_eq!(
            written.as_deref(),
            Some(case.then),
            "case {:?}: expected the further update's own value in Parameter Store, got \
             {written:?}",
            case.case
        );
    }
}
