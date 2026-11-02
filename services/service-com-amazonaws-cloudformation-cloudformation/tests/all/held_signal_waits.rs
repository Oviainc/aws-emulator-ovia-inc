// SPDX-License-Identifier: CC0-1.0

//! The SPAN this emulator holds a stack operation at a resource-signal wait for, end to end
//! over the awsQuery wire.
//!
//! The rule: nothing but a client's own `SignalResource` call satisfies the wait a
//! resource's declaration asks for, so an operation held at one that is never signalled is
//! held for exactly the span that declaration asked for and then reaches a TERMINAL status.
//! A client polling `DescribeStacks` therefore always learns what happened, and the whole
//! contract behind the failure is reachable: the rollback restores the resources the update
//! had already changed in the services that own them, drops the ones it had added, records
//! the stack's `UPDATE_ROLLBACK_IN_PROGRESS` event, and leaves the stack able to take a
//! further update.
//!
//! The rows span the class the rule generalises over: a `Timeout` spelled as a string and as
//! a number, the maximum the type publishes, and a declaration that states none at all (held
//! to the documented default) — crossed with the two directions a rollback runs in, a
//! resource the update CHANGED and one it ADDED. How long a declaration's own span is, below,
//! at and above the published maximum, is the table in `update_rollback_recovery.rs`
//! (`a_wait_is_held_for_what_it_asked`) and, for a `CreationPolicy`, in
//! `resource_signal_waits.rs`; that an unsignalled gate stays reported for the whole of that
//! span is `unsignalled_gate_hold.rs`.
//!
//! No case here sleeps its span out: a test must not wait on a wall-clock span measured
//! in seconds, so each one moves the barrier's own `opened_at` back, exactly as
//! `core/router/tests/all/cloudformation_provisions_acm_certificates.rs` does for the
//! stabilisation window — first to just INSIDE the span the declaration asked for, where the
//! wait must still hold, and then past it, where the clock owes the failure.

use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::wait_conditions::DEFAULT_TIMEOUT;
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router_with_engine, store, wait_for_the_transition, xml_text,
    xml_texts, REGION,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, stack_events, status_of, template,
};

/// One row: how the update's unsignalled wait states its `Timeout`, and what the update had
/// already done in the owning service before it was held at that wait.
struct HeldCase {
    case: &'static str,
    stack: &'static str,
    /// `Timeout` exactly as the declaration spells it, or `None` for one that states none —
    /// which is held to the documented default.
    timeout: Option<Value>,
    /// The span that `Timeout` asks the wait to run for, which is the span the rows age the
    /// barrier against.
    asked_for: Duration,
    /// The parameter the stack is created with, and the value it holds then.
    created: (&'static str, &'static str),
    /// The value the update writes for that parameter (the same value where the update does
    /// not touch it).
    updated: &'static str,
    /// A parameter the UPDATE adds, which the rollback has to remove again.
    added: Option<(&'static str, &'static str)>,
}

fn held_cases() -> Vec<HeldCase> {
    vec![
        HeldCase {
            case: "a Timeout of a minute, spelled as a string, over a changed resource",
            stack: "held-minute",
            timeout: Some(json!("60")),
            asked_for: Duration::from_secs(60),
            created: ("/held-minute/p", "v1"),
            updated: "v2",
            added: None,
        },
        HeldCase {
            case: "a Timeout spelled as a number, over a changed resource",
            stack: "held-number",
            timeout: Some(json!(90)),
            asked_for: Duration::from_secs(90),
            created: ("/held-number/p", "before"),
            updated: "after",
            added: None,
        },
        HeldCase {
            case: "the Timeout maximum the type publishes, over a resource the update added",
            stack: "held-maximum",
            timeout: Some(json!("43200")),
            asked_for: Duration::from_secs(43_200),
            created: ("/held-maximum/kept", "kept"),
            updated: "kept",
            added: Some(("/held-maximum/added", "added")),
        },
        HeldCase {
            case: "a declaration that states no Timeout, over a changed and an added resource",
            stack: "held-unstated",
            timeout: None,
            asked_for: DEFAULT_TIMEOUT,
            created: ("/held-unstated/p", "one"),
            updated: "two",
            added: Some(("/held-unstated/added", "added")),
        },
    ]
}

/// The update's template: the stack's own parameter at its new value, whatever the update
/// adds, and the wait condition nothing signals — which `DependsOn` puts AFTER the
/// parameter, so the update has already changed Parameter Store by the time it is held.
fn gated_template(case: &HeldCase) -> String {
    let mut resources: Map<String, Value> = Map::new();
    resources.insert(
        "Param".to_string(),
        parameter(case.created.0, case.updated),
    );
    if let Some((name, value)) = case.added {
        resources.insert("Added".to_string(), parameter(name, value));
    }
    resources.insert(
        "Handle".to_string(),
        json!({"Type": "AWS::CloudFormation::WaitConditionHandle"}),
    );
    let mut properties = Map::new();
    properties.insert("Handle".to_string(), json!({"Ref": "Handle"}));
    properties.insert("Count".to_string(), json!("1"));
    if let Some(timeout) = case.timeout.clone() {
        properties.insert("Timeout".to_string(), timeout);
    }
    resources.insert(
        "Gate".to_string(),
        json!({
            "Type": "AWS::CloudFormation::WaitCondition",
            "DependsOn": "Param",
            "Properties": Value::Object(properties),
        }),
    );
    template(Value::Object(resources))
}

/// Move every wait this stack is held at `by` further into the past, which is how a case
/// reaches an instant the wait was already over at without waiting for it.
pub(crate) fn age_the_wait(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    stack_name: &str,
    by: Duration,
) {
    let held = store(engine);
    let mut locked = held.lock();
    let stack = locked
        .resolve_mut(REGION, stack_name)
        .unwrap_or_else(|| panic!("the stack {stack_name:?} this case created"));
    assert!(
        !stack.wait_conditions.is_empty(),
        "case {stack_name:?}: expected the update to be held at its wait condition, got no \
         barrier at all"
    );
    for barrier in &mut stack.wait_conditions {
        barrier.opened_at -= by;
    }
}

/// Every `LogicalResourceId` `ListStackResources` reports for a stack right now.
pub(crate) async fn listed_resources(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (http_status, body) = call(
        router,
        "ListStackResources",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected ListStackResources {stack_name:?} to succeed, got {http_status} {body}"
    );
    xml_texts(&body, "LogicalResourceId")
}

#[tokio::test]
async fn an_unsignalled_wait_fails_the_update_once_the_span_it_asked_for_is_over() {
    for case in held_cases() {
        // The span is never zero: a client that means to signal has the whole of it to do so
        // in, which is what the "still held" step of each row reads.
        assert!(
            case.asked_for >= Duration::from_secs(2),
            "case {:?}: the span a row's declaration asks for must leave a client room to \
             signal, got {:?}",
            case.case,
            case.asked_for
        );
        let (router, engine) = router_with_engine();
        let created = template(json!({"Param": parameter(case.created.0, case.created.1)}));
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

        let gated = gated_template(&case);
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

        // Just INSIDE the span the declaration asked for the wait still holds: the update is
        // in flight, and what it changed in the owning service is what that service holds.
        age_the_wait(&engine, case.stack, case.asked_for - Duration::from_secs(1));
        let in_flight = status_of(&router, case.stack).await;
        assert_eq!(
            in_flight,
            status::UPDATE_IN_PROGRESS,
            "case {:?}: expected the update to still be held one second inside the span it \
             asked for, got {in_flight:?}",
            case.case
        );
        let applied = parameter_value(&router, case.created.0).await;
        assert_eq!(
            applied.as_deref(),
            Some(case.updated),
            "case {:?}: expected Parameter Store to hold the update's own value for {:?} \
             while the update is held, got {applied:?}",
            case.case,
            case.created.0
        );

        // Past the span it asked for the failure is the clock's: the stack fails, rolls back,
        // and settles.
        age_the_wait(&engine, case.stack, Duration::from_secs(2));
        let settling = status_of(&router, case.stack).await;
        assert!(
            settling == status::UPDATE_ROLLBACK_COMPLETE
                || settling == status::UPDATE_ROLLBACK_IN_PROGRESS,
            "case {:?}: expected the wait to have run out past the span it asked for, got \
             {settling:?}",
            case.case
        );
        wait_for_the_transition().await;
        let terminal = status_of(&router, case.stack).await;
        assert_eq!(
            terminal,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {:?}: expected the stack to settle on a terminal rolled-back status, got \
             {terminal:?}",
            case.case
        );

        // The rollback put back what the update had changed and removed what it had added.
        let restored = parameter_value(&router, case.created.0).await;
        assert_eq!(
            restored.as_deref(),
            Some(case.created.1),
            "case {:?}: expected the rollback to put {:?} back to its pre-update value, got \
             {restored:?}",
            case.case,
            case.created.0
        );
        if let Some((name, _)) = case.added {
            let dropped = parameter_value(&router, name).await;
            assert_eq!(
                dropped, None,
                "case {:?}: expected the rollback to remove the parameter the update added, \
                 got {dropped:?}",
                case.case
            );
        }

        // ...and the stack says so: the wait condition the failed update added is gone from
        // its resources, the resource that survived is still there, and the rollback has its
        // own event.
        let listed = listed_resources(&router, case.stack).await;
        assert!(
            listed.contains(&"Param".to_string()),
            "case {:?}: expected the resource the stack keeps to still be listed, got \
             {listed:?}",
            case.case
        );
        for gone in ["Gate", "Handle"] {
            assert!(
                !listed.contains(&gone.to_string()),
                "case {:?}: expected {gone:?}, which only the failed update declared, to be \
                 gone from the stack's resources, got {listed:?}",
                case.case
            );
        }
        let events = stack_events(case.case, &router, case.stack).await;
        assert!(
            events.contains(&(
                case.stack.to_string(),
                status::UPDATE_ROLLBACK_IN_PROGRESS.to_string()
            )),
            "case {:?}: expected the stack's own UPDATE_ROLLBACK_IN_PROGRESS event, got \
             {events:?}",
            case.case
        );

        // The rolled-back stack is not wedged: a further valid update completes.
        let next = template(json!({"Param": parameter(case.created.0, "after-the-rollback")}));
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
        wait_for_the_transition().await;
        let updated = status_of(&router, case.stack).await;
        assert_eq!(
            updated,
            status::UPDATE_COMPLETE,
            "case {:?}: expected the further update to complete, got {updated:?}",
            case.case
        );
        let written = parameter_value(&router, case.created.0).await;
        assert_eq!(
            written.as_deref(),
            Some("after-the-rollback"),
            "case {:?}: expected the further update's own value in Parameter Store, got \
             {written:?}",
            case.case
        );
        let reason = status_reason_of(&router, case.stack).await;
        assert!(
            reason.is_empty() || !reason.contains("WaitCondition"),
            "case {:?}: expected the completed update to leave no failed-wait reason behind, \
             got {reason:?}",
            case.case
        );
    }
}

/// The `StackStatusReason` a stack reports right now, or the empty string when it reports
/// none.
async fn status_reason_of(router: &MultiServiceRouter, stack_name: &str) -> String {
    let (_, body) = call(
        router,
        "DescribeStacks",
        &[member("StackName", stack_name)],
    )
    .await;
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}
