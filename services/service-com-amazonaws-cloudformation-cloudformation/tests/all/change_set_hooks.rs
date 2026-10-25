// SPDX-License-Identifier: CC0-1.0

//! `DescribeChangeSetHooks` over the awsQuery wire: the Hook invocations one change set
//! plans.
//!
//! The rule every case here holds the service to: **`DescribeChangeSetHooks` answers from
//! the change set the request addresses — by ARN, or by name plus the stack it belongs to —
//! reporting that change set's identity, the Hook invocations it PERSISTS as its plan
//! (narrowed to one resource when the request names a `LogicalResourceId`, one bounded page
//! at a time) and the status that plan is in, which follows the change set's own creation;
//! a change set it cannot resolve is the modelled `ChangeSetNotFoundException`.**
//!
//! The tables below span what the rule generalises over: the creation states a change set
//! passes through, the addressing forms a request may name it by, plans of several sizes
//! over several resources, and every `LogicalResourceId` filter over one plan — including
//! one that selects nothing.
//!
//! A plan's WRITER is the planning CloudFormation performs while it creates a change set,
//! which needs an activated Hook whose configuration targets the operation; nothing in this
//! emulator can put a Hook in that position (see `state::change_set_hooks`), so a change set
//! created here plans nothing and the rows that need a plan seed it straight into the store
//! through `StoredChangeSet::plan_hook` — the same device `hook_result_invocations.rs` uses
//! for records whose writers are not available. Every READ goes through the real request
//! path (form-encoded body in, XML envelope out), so what is asserted is what a client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::change_set_hooks::{
    hooks_status, target_type, StoredChangeSetHook, MAX_CHANGE_SET_HOOKS,
};
use service_com_amazonaws_cloudformation_cloudformation::state::hook_results::{
    failure_mode, invocation_point,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{self, change_action};
use service_com_amazonaws_cloudformation_cloudformation::state::PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, call_in, create_stack, member, router_with_engine, store, xml_blocks, xml_text,
    xml_texts, REGION,
};

/// A template declaring three resources, so a plan can target one of them and leave the
/// others untargeted.
const THREE_RESOURCES: &str = r#"{"Resources":{"Alpha":{"Type":"AWS::SNS::Topic"},"Beta":{"Type":"AWS::S3::Bucket"},"Gamma":{"Type":"AWS::SNS::Topic"}}}"#;

/// The Hooks the seeded plans belong to. `HookTypeName` is `@length(10..196)`, which both
/// satisfy.
const HOOK_A: &str = "My::Sample::Hook";
const HOOK_B: &str = "Other::Sample::Hook";

/// A `HookTypeVersionId` (`@pattern("^[A-Za-z0-9-]+$")`).
const VERSION_ID: &str = "00000001";

/// `CreateChangeSet` for a stack that does not exist yet, which is what a `CREATE` change
/// set brings into existence.
async fn create_creating_change_set(
    router: &MultiServiceRouter,
    stack_name: &str,
    change_set_name: &str,
    template: &str,
) -> (u16, String) {
    call(
        router,
        "CreateChangeSet",
        &[
            member("StackName", stack_name),
            member("ChangeSetName", change_set_name),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", template),
        ],
    )
    .await
}

/// `DescribeChangeSetHooks`, with the members a case varies spelled as an SDK sends them.
async fn describe_hooks(
    router: &MultiServiceRouter,
    change_set: &str,
    stack_name: Option<&str>,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("ChangeSetName", change_set)];
    if let Some(stack_name) = stack_name {
        members.push(member("StackName", stack_name));
    }
    members.extend_from_slice(extra);
    call(router, "DescribeChangeSetHooks", &members).await
}

/// Hold the change set with this ARN BEFORE its creation settles, by pushing the due
/// instant of the transition it owes well into the future.
///
/// The in-flight status is owed only until that instant, so a case that asserts it must
/// hold the store on this side of it rather than race the clock; nothing else about the
/// change set is touched.
fn hold_before_the_transition(engine: &Arc<engine::InMemoryEngine>, change_set_id: &str) {
    let state = store(engine);
    let mut held = state.lock();
    let change_set = held
        .resolve_change_set_mut(REGION, change_set_id, None)
        .expect("the change set the case created");
    let pending = change_set
        .pending
        .as_mut()
        .expect("a change set answered while its creation is in flight");
    pending.due_at = SystemTime::now() + Duration::from_secs(600);
}

/// Put a plan on the change set with this ARN, through the store's own writer.
fn plan_hooks(
    engine: &Arc<engine::InMemoryEngine>,
    change_set_id: &str,
    hooks: Vec<StoredChangeSetHook>,
) {
    let state = store(engine);
    let mut held = state.lock();
    let change_set = held
        .resolve_change_set_mut(REGION, change_set_id, None)
        .expect("the change set the case created");
    for hook in hooks {
        change_set.plan_hook(hook);
    }
}

/// The inner text of every `<tag>` INSIDE the response's `Hooks` list, so an identity
/// member of the response itself is never swept up with a plan's.
fn in_the_plan(body: &str, tag: &str) -> Vec<String> {
    xml_blocks(body, "Hooks")
        .iter()
        .flat_map(|block| xml_texts(block, tag))
        .collect()
}

/// One row of the `LogicalResourceId` table: the case name, the filter the request names,
/// and the Hooks and resources the response must report, in order.
type FilterCase = (&'static str, Option<&'static str>, Vec<&'static str>, Vec<&'static str>);

/// A `CREATE` change set for a new stack, settled, answered with its ARN.
async fn settled_change_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    change_set_name: &str,
) -> String {
    let (code, created) =
        create_creating_change_set(router, stack_name, change_set_name, THREE_RESOURCES).await;
    assert_eq!(
        code, 200,
        "case {change_set_name:?}: expected the change set to be created, got {code} {created}"
    );
    settle_the_store(router, engine).await;
    xml_text(&created, "Id").expect("a change set id")
}

#[tokio::test]
async fn the_hook_plan_status_follows_the_change_sets_creation() {
    let (router, engine) = router_with_engine();

    // A change set is answered while CloudFormation is still creating it, so the plan a
    // read takes before the creation settles is still being computed.
    let (code, created) =
        create_creating_change_set(&router, "planning", "planning-set", THREE_RESOURCES).await;
    assert_eq!(
        code, 200,
        "case \"a creation still in flight\": expected 200, got {code} {created}"
    );
    let change_set_id = xml_text(&created, "Id").expect("a change set id");
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    hold_before_the_transition(&engine, &change_set_id);
    let (code, body) = describe_hooks(&router, &change_set_id, None, &[]).await;
    assert_eq!(
        code, 200,
        "case \"a creation still in flight\": expected 200, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(hooks_status::PLANNING),
        "case \"a creation still in flight\": expected {}, got {body}",
        hooks_status::PLANNING
    );

    // Once the creation has completed, the plan it computed is what the read reports.
    settle_the_store(&router, &engine).await;
    let (code, body) = describe_hooks(&router, &change_set_id, None, &[]).await;
    assert_eq!(
        code, 200,
        "case \"a completed creation\": expected 200, got {code} {body}"
    );
    for (tag, expected) in [
        ("Status", hooks_status::PLANNED.to_string()),
        ("ChangeSetId", change_set_id.clone()),
        ("ChangeSetName", "planning-set".to_string()),
        ("StackId", stack_id.clone()),
        ("StackName", "planning".to_string()),
    ] {
        assert_eq!(
            xml_text(&body, tag).as_deref(),
            Some(expected.as_str()),
            "case \"a completed creation\": {tag} must be {expected:?}, got {body}"
        );
    }
    // Nothing here can activate a Hook, so this change set planned no invocation — which is
    // reported as an empty list rather than an absent member.
    assert!(
        in_the_plan(&body, "TypeName").is_empty(),
        "case \"a completed creation\": a change set nothing planned a Hook for, got {body}"
    );
    assert!(
        body.contains("<Hooks"),
        "case \"a completed creation\": Hooks is reported as an empty element, got {body}"
    );

    // A creation that FAILED computed no plan at all, so there is none to report.
    create_stack(&router, "unchanged", THREE_RESOURCES, &[]).await;
    settle_the_store(&router, &engine).await;
    let (code, created) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", "unchanged"),
            member("ChangeSetName", "unchanged-set"),
            member("TemplateBody", THREE_RESOURCES),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a failed creation\": the change set is still created, got {code} {created}"
    );
    let failed_id = xml_text(&created, "Id").expect("a change set id");
    settle_the_store(&router, &engine).await;
    let (code, body) = describe_hooks(&router, &failed_id, None, &[]).await;
    assert_eq!(
        code, 200,
        "case \"a failed creation\": expected 200, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(hooks_status::UNAVAILABLE),
        "case \"a failed creation\": expected {}, got {body}",
        hooks_status::UNAVAILABLE
    );
}

#[tokio::test]
async fn the_planned_invocations_are_reported_and_narrowed_by_logical_resource_id() {
    let (router, engine) = router_with_engine();
    let change_set_id = settled_change_set(&router, &engine, "planned", "planned-set").await;

    // Two Hooks planned against one resource of the change set and one against another,
    // leaving the third resource untargeted.
    let mut configured = StoredChangeSetHook::new(
        HOOK_A,
        VERSION_ID,
        "Alpha",
        "AWS::SNS::Topic",
        change_action::ADD,
    );
    configured.type_configuration_version_id = Some("00000002".to_string());
    let mut warning = StoredChangeSetHook::new(
        HOOK_B,
        VERSION_ID,
        "Alpha",
        "AWS::SNS::Topic",
        change_action::ADD,
    );
    warning.failure_mode = failure_mode::WARN.to_string();
    let on_the_bucket = StoredChangeSetHook::new(
        HOOK_A,
        VERSION_ID,
        "Beta",
        "AWS::S3::Bucket",
        change_action::ADD,
    );
    plan_hooks(
        &engine,
        &change_set_id,
        vec![configured, warning, on_the_bucket],
    );

    // Every filter over the one plan, including the two that select nothing: the member is
    // a filter over the planned invocations, so one that selects none of them is the empty
    // list rather than a refusal.
    let cases: Vec<FilterCase> = vec![
        (
            "no filter: the whole plan",
            None,
            vec![HOOK_A, HOOK_B, HOOK_A],
            vec!["Alpha", "Alpha", "Beta"],
        ),
        (
            "the resource two Hooks are planned against",
            Some("Alpha"),
            vec![HOOK_A, HOOK_B],
            vec!["Alpha", "Alpha"],
        ),
        (
            "the resource one Hook is planned against",
            Some("Beta"),
            vec![HOOK_A],
            vec!["Beta"],
        ),
        (
            "a resource of the change set no Hook is planned against",
            Some("Gamma"),
            vec![],
            vec![],
        ),
        (
            "a logical id the change set does not carry at all",
            Some("Delta"),
            vec![],
            vec![],
        ),
    ];
    for (case, logical_resource_id, type_names, logical_ids) in &cases {
        let extra = match logical_resource_id {
            Some(logical_resource_id) => vec![member("LogicalResourceId", logical_resource_id)],
            None => vec![],
        };
        let (code, body) = describe_hooks(&router, &change_set_id, None, &extra).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected 200, got {code} {body}"
        );
        assert_eq!(
            in_the_plan(&body, "TypeName"),
            *type_names,
            "case {case:?}: expected {type_names:?}, got {body}"
        );
        assert_eq!(
            in_the_plan(&body, "LogicalResourceId"),
            *logical_ids,
            "case {case:?}: expected {logical_ids:?}, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(hooks_status::PLANNED),
            "case {case:?}: the plan of a created change set is PLANNED, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds the whole plan, so no NextToken, got {body}"
        );
    }

    // The read reports, it does not consume: every row answers the same the second time,
    // and the unfiltered read is unchanged by the filtered ones above.
    for (case, logical_resource_id, type_names, _) in &cases {
        let extra = match logical_resource_id {
            Some(logical_resource_id) => vec![member("LogicalResourceId", logical_resource_id)],
            None => vec![],
        };
        let (code, body) = describe_hooks(&router, &change_set_id, None, &extra).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected 200 on the repeat read, got {code} {body}"
        );
        assert_eq!(
            in_the_plan(&body, "TypeName"),
            *type_names,
            "case {case:?}: the repeat read must report {type_names:?}, got {body}"
        );
    }

    // One planned invocation, member by member: every one is read off the record.
    let (code, body) = describe_hooks(
        &router,
        &change_set_id,
        None,
        &[member("LogicalResourceId", "Beta")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"one planned invocation\": expected 200, got {code} {body}"
    );
    for (tag, expected) in [
        ("InvocationPoint", invocation_point::PRE_PROVISION),
        ("FailureMode", failure_mode::FAIL),
        ("TypeName", HOOK_A),
        ("TypeVersionId", VERSION_ID),
        ("TargetType", target_type::RESOURCE),
        ("LogicalResourceId", "Beta"),
        ("ResourceType", "AWS::S3::Bucket"),
        ("ResourceAction", change_action::ADD),
    ] {
        assert_eq!(
            in_the_plan(&body, tag),
            vec![expected.to_string()],
            "case {tag:?}: expected {expected:?}, got {body}"
        );
    }
    // A member the record does not carry is absent rather than empty.
    assert_eq!(
        in_the_plan(&body, "TypeConfigurationVersionId"),
        Vec::<String>::new(),
        "case \"TypeConfigurationVersionId\": absent for a record without one, got {body}"
    );

    // The record that DOES carry a configuration version reports it, and the one planned
    // with the other failure mode reports that.
    let (code, body) = describe_hooks(
        &router,
        &change_set_id,
        None,
        &[member("LogicalResourceId", "Alpha")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"the two Hooks of one resource\": expected 200, got {code} {body}"
    );
    assert_eq!(
        in_the_plan(&body, "TypeConfigurationVersionId"),
        vec!["00000002".to_string()],
        "case \"the two Hooks of one resource\": the record's configuration version, got {body}"
    );
    assert_eq!(
        in_the_plan(&body, "FailureMode"),
        vec![
            failure_mode::FAIL.to_string(),
            failure_mode::WARN.to_string()
        ],
        "case \"the two Hooks of one resource\": each record's own failure mode, got {body}"
    );
}

#[tokio::test]
async fn the_plan_is_read_back_one_bounded_page_at_a_time() {
    let (router, engine) = router_with_engine();
    let change_set_id = settled_change_set(&router, &engine, "paged", "paged-set").await;
    let total = PAGE_SIZE + 3;
    plan_hooks(
        &engine,
        &change_set_id,
        (0..total)
            .map(|index| {
                StoredChangeSetHook::new(
                    format!("My::Sample::Hook{index}"),
                    VERSION_ID,
                    "Alpha",
                    "AWS::SNS::Topic",
                    change_action::ADD,
                )
            })
            .collect(),
    );

    let (code, first) = describe_hooks(&router, &change_set_id, None, &[]).await;
    assert_eq!(
        code, 200,
        "case \"the first page\": expected 200, got {code} {first}"
    );
    let first_names = in_the_plan(&first, "TypeName");
    assert_eq!(
        first_names.len(),
        PAGE_SIZE,
        "case \"the first page\": at most {PAGE_SIZE} items, got {}",
        first_names.len()
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the first page\": a page with more behind it carries a NextToken, got {first}")
    });

    let (code, second) = describe_hooks(
        &router,
        &change_set_id,
        None,
        &[member("NextToken", &token)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"the resumed page\": expected 200, got {code} {second}"
    );
    let second_names = in_the_plan(&second, "TypeName");
    assert_eq!(
        second_names.len(),
        total - PAGE_SIZE,
        "case \"the resumed page\": the remainder, got {second_names:?}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no NextToken, got {second}"
    );
    let mut paged = first_names.clone();
    paged.extend(second_names);
    let distinct: std::collections::BTreeSet<String> = paged.iter().cloned().collect();
    assert_eq!(
        distinct.len(),
        total,
        "case \"the pages together\": every planned invocation exactly once, got {} of them",
        paged.len()
    );

    // A cursor this collection never issued is a bad request, not an empty page.
    for (case, bad_token) in [
        (
            "a cursor past the end",
            state::encode_next_token(state::PageOf::ChangeSetHooks, total + 1),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(state::PageOf::Changes, 0),
        ),
    ] {
        let (code, body) = describe_hooks(
            &router,
            &change_set_id,
            None,
            &[member("NextToken", &bad_token)],
        )
        .await;
        assert_eq!(
            code, 400,
            "case {case:?}: expected 400, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn a_change_set_plan_keeps_at_most_the_bound_of_invocations() {
    let (router, engine) = router_with_engine();
    let change_set_id = settled_change_set(&router, &engine, "bounded", "bounded-set").await;
    let beyond_the_bound = 3;
    plan_hooks(
        &engine,
        &change_set_id,
        (0..MAX_CHANGE_SET_HOOKS + beyond_the_bound)
            .map(|index| {
                StoredChangeSetHook::new(
                    HOOK_A,
                    VERSION_ID,
                    format!("Resource{index}"),
                    "AWS::SNS::Topic",
                    change_action::ADD,
                )
            })
            .collect(),
    );
    {
        let state = store(&engine);
        let held = state.lock();
        let change_set = held
            .resolve_change_set(REGION, &change_set_id, None)
            .expect("the change set this case created");
        assert_eq!(
            change_set.hooks.len(),
            MAX_CHANGE_SET_HOOKS,
            "case \"past the bound\": the plan keeps at most {MAX_CHANGE_SET_HOOKS}, got {}",
            change_set.hooks.len()
        );
    }

    // The OLDEST records are the reclaimed ones, so the first page starts at the first
    // record still held.
    let (code, body) = describe_hooks(&router, &change_set_id, None, &[]).await;
    assert_eq!(
        code, 200,
        "case \"past the bound\": expected 200, got {code} {body}"
    );
    assert_eq!(
        in_the_plan(&body, "LogicalResourceId").first().map(String::as_str),
        Some(format!("Resource{beyond_the_bound}").as_str()),
        "case \"past the bound\": the oldest surviving record comes first, got {body}"
    );
}

#[tokio::test]
async fn a_change_set_the_request_cannot_resolve_is_refused() {
    let (router, engine) = router_with_engine();
    let change_set_id =
        settled_change_set(&router, &engine, "addressed", "addressed-set").await;
    let absent_arn = format!(
        "{}-no-such-change-set",
        change_set_id.rsplit_once('/').expect("an ARN").0
    );

    // "The name or Amazon Resource Name (ARN) of the change set": an ARN names it on its
    // own, a bare name needs the stack it belongs to — and a name with no stack at all is a
    // bad request, not a change set that does not exist.
    for (case, change_set, stack_name, expected_code, expected_error) in [
        ("the ARN alone", change_set_id.as_str(), None, 200, ""),
        (
            "the ARN with its stack",
            change_set_id.as_str(),
            Some("addressed"),
            200,
            "",
        ),
        (
            "the name with its stack",
            "addressed-set",
            Some("addressed"),
            200,
            "",
        ),
        ("the name alone", "addressed-set", None, 400, "ValidationError"),
        (
            "the name with a stack that does not exist",
            "addressed-set",
            Some("addressed-other"),
            404,
            "ChangeSetNotFound",
        ),
        (
            "a name the stack carries no change set under",
            "nope",
            Some("addressed"),
            404,
            "ChangeSetNotFound",
        ),
        (
            "an ARN no change set carries",
            absent_arn.as_str(),
            None,
            404,
            "ChangeSetNotFound",
        ),
    ] {
        let (code, body) = describe_hooks(&router, change_set, stack_name, &[]).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected HTTP {expected_code}, got {code} {body}"
        );
        if expected_code == 200 {
            assert_eq!(
                xml_text(&body, "ChangeSetId").as_deref(),
                Some(change_set_id.as_str()),
                "case {case:?}: the change set reports its own id, got {body}"
            );
            continue;
        }
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some(expected_error),
            "case {case:?}: expected the {expected_error} error, got {body}"
        );
        if expected_error == "ChangeSetNotFound" {
            let expected_message = format!("ChangeSet [{change_set}] does not exist");
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(expected_message.as_str()),
                "case {case:?}: expected {expected_message:?}, got {body}"
            );
        }
    }

    // The store is read THROUGH the Region the request was signed for: another Region holds
    // no change set of this one's stack.
    let (code, body) = call_in(
        &router,
        "us-west-2",
        "DescribeChangeSetHooks",
        &[member("ChangeSetName", &change_set_id)],
    )
    .await;
    assert_eq!(
        code, 404,
        "case \"another Region's read\": expected 404, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ChangeSetNotFound"),
        "case \"another Region's read\": expected the ChangeSetNotFound error, got {body}"
    );
}
