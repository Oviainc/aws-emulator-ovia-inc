// SPDX-License-Identifier: CC0-1.0

//! How many rollback triggers one `RollbackConfiguration` may name, end to end over the
//! awsQuery wire.
//!
//! RULE under test: **the trigger list is bounded by the model's own `RollbackTriggers`
//! constraint (`length: min 0, max 5`, which is also the quota AWS publishes for one stack's
//! rollback triggers), so a request naming more triggers than that is refused synchronously
//! with `ValidationError` and the constraint sentence naming the member, the bound and how
//! many the request carried, changing nothing — while a request at or under the bound is
//! accepted and reports back every trigger it named.**
//!
//! The ceiling is what bounds the monitoring the triggers cost: CloudFormation reads each
//! named alarm's state while the operation it monitors is in flight, so the set it reads is
//! fixed by the member rather than by what a request asked for.
//!
//! The rows differ along what the rule generalises over: the COUNT (none, one, the bound
//! itself, one past it, and well past it) and the operation the configuration is supplied on
//! (`CreateStack` and `UpdateStack`, the two that put one in force on a stack).

use service_com_amazonaws_cloudformation_cloudformation::state::MAX_ROLLBACK_TRIGGERS;

use crate::harness::{call, create_stack, member, router, wait_for_the_transition, xml_blocks, xml_text, xml_texts};
use crate::stack_lifecycle::describe_stack;

/// A template every row shares, so the only thing a row varies is the trigger count.
const TEMPLATE: &str = r#"{"Description":"rollback trigger bounds"}"#;

/// The ARN of the `n`th trigger a row names. A rollback trigger names a CloudWatch alarm,
/// and the account id is the one this emulator signs for.
fn alarm_arn(n: usize) -> String {
    format!("arn:aws:cloudwatch:us-east-1:000000000000:alarm:probe-{n}")
}

/// The members that spell one `RollbackConfiguration` naming `count` alarm triggers.
fn rollback_triggers(count: usize) -> Vec<(String, String)> {
    let mut members = vec![member("RollbackConfiguration.MonitoringTimeInMinutes", "5")];
    for index in 0..count {
        let position = index + 1;
        members.push(member(
            &format!("RollbackConfiguration.RollbackTriggers.member.{position}.Arn"),
            &alarm_arn(index),
        ));
        members.push(member(
            &format!("RollbackConfiguration.RollbackTriggers.member.{position}.Type"),
            "AWS::CloudWatch::Alarm",
        ));
    }
    members
}

/// The trigger ARNs the response reports inside its `RollbackTriggers` block, in wire order.
fn reported_trigger_arns(xml: &str) -> Vec<String> {
    match xml_blocks(xml, "RollbackTriggers").into_iter().next() {
        Some(block) => xml_texts(&block, "Arn"),
        None => Vec::new(),
    }
}

/// Assert that `body` is the refusal a trigger list of `count` is owed, which names the
/// member, how many the request carried and the bound it broke — every variable part taken
/// from the row rather than from the answer being checked.
fn assert_the_count_refusal(case: &str, count: usize, status: u16, body: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the call to be refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(body, "Code").as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected a ValidationError, got {body}"
    );
    let expected = format!(
        "1 validation error detected: Value '{count} items' at \
         'rollbackConfiguration.rollbackTriggers' failed to satisfy constraint: Member must \
         have length less than or equal to {MAX_ROLLBACK_TRIGGERS}"
    );
    assert_eq!(
        xml_text(body, "Message").as_deref(),
        Some(expected.as_str()),
        "case {case:?}: expected the message {expected:?}, got {body}"
    );
}

/// Every count a row drives, with whether CloudFormation admits it: the bound comes from the
/// model's `RollbackTriggers` list constraint, read with
/// `smithy_query(query="shape", shape="RollbackTriggers")`.
fn counts() -> Vec<(String, usize, bool)> {
    vec![
        ("no triggers at all".to_string(), 0, true),
        ("one trigger".to_string(), 1, true),
        (
            format!("the bound itself ({MAX_ROLLBACK_TRIGGERS})"),
            MAX_ROLLBACK_TRIGGERS,
            true,
        ),
        (
            "one past the bound".to_string(),
            MAX_ROLLBACK_TRIGGERS + 1,
            false,
        ),
        (
            "well past the bound".to_string(),
            MAX_ROLLBACK_TRIGGERS * 4,
            false,
        ),
    ]
}

#[tokio::test]
async fn create_stack_holds_the_trigger_list_to_the_models_bound() {
    assert_eq!(
        MAX_ROLLBACK_TRIGGERS, 5,
        "the bound is the one the RollbackTriggers list declares, got {MAX_ROLLBACK_TRIGGERS}"
    );

    let router = router();
    for (index, (case, count, accepted)) in counts().into_iter().enumerate() {
        let stack_name = format!("rbt-count-{index}");
        let (status, body) =
            create_stack(&router, &stack_name, TEMPLATE, &rollback_triggers(count)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the create must be accepted, got {status} {body}"
            );
            let (status, described) = describe_stack(&router, &stack_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeStacks to succeed, got {status} {described}"
            );
            let expected: Vec<String> = (0..count).map(alarm_arn).collect();
            assert_eq!(
                reported_trigger_arns(&described),
                expected,
                "case {case:?}: the stack reports the triggers it was created with, got \
                 {described}"
            );
        } else {
            assert_the_count_refusal(&case, count, status, &body);
            // "nothing was created": the name the refused create used names no stack.
            let (status, described) = describe_stack(&router, &stack_name).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create leaves no stack behind, got {status} \
                 {described}"
            );
        }
    }
}

/// `UpdateStack` is held to the same bound, and a refused update leaves the configuration the
/// stack already carried.
#[tokio::test]
async fn update_stack_holds_the_trigger_list_to_the_same_bound() {
    let router = router();
    let (status, body) = create_stack(
        &router,
        "rbt-count-updated",
        TEMPLATE,
        &rollback_triggers(1),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stack under test must be created, got {status} {body}"
    );
    wait_for_the_transition().await;
    // The triggers in force on the stack right now, which is what a REFUSED update must
    // leave untouched: it moves on every accepted row, so each refusal is judged against the
    // configuration the stack really carried when it arrived.
    let mut in_force: Vec<String> = vec![alarm_arn(0)];

    for (case, count, accepted) in counts() {
        let mut members = vec![
            member("StackName", "rbt-count-updated"),
            member(
                "TemplateBody",
                &format!(r#"{{"Description":"revision {count}"}}"#),
            ),
        ];
        members.extend(rollback_triggers(count));
        let (status, body) = call(&router, "UpdateStack", &members).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the update must be accepted, got {status} {body}"
            );
            wait_for_the_transition().await;
            let (_, described) = describe_stack(&router, "rbt-count-updated").await;
            let expected: Vec<String> = (0..count).map(alarm_arn).collect();
            assert_eq!(
                reported_trigger_arns(&described),
                expected,
                "case {case:?}: the update's triggers are in force, got {described}"
            );
            in_force = expected;
        } else {
            assert_the_count_refusal(&case, count, status, &body);
            let (_, described) = describe_stack(&router, "rbt-count-updated").await;
            assert_eq!(
                reported_trigger_arns(&described),
                in_force,
                "case {case:?}: a refused update leaves the stack's own triggers in place, \
                 got {described}"
            );
        }
    }
}
