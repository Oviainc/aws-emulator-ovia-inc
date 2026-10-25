// SPDX-License-Identifier: CC0-1.0

//! `ListHookResults` over the awsQuery wire: the Hook invocation results an account and
//! Region holds.
//!
//! The rule every case here holds the service to: **`ListHookResults` reports the Hook
//! invocation results THIS account and Region persists, for exactly the parameter
//! combination the request names — all of them when it names none, one Hook's when it names
//! `TypeArn` (narrowed further by `Status`), and one invocation target's when it names
//! `TargetId` with `TargetType` — in invocation order, one bounded page at a time;
//! withholding the two members the model makes conditional on the target form, answering
//! the empty list for a filter that selects none of them, and refusing any other
//! combination of those members.**
//!
//! The table below is one row per supported parameter combination, over seeded records
//! spanning two Hooks, every `HookStatus`, three target types (one of them not a filterable
//! one) and two Regions, so a row's expected ids distinguish each filter from every other.
//!
//! The store's WRITER is a Hook INVOCATION, which nothing in this emulator performs: a
//! Hook's verdict comes from running its own handler, and none of those can run here (see
//! `state::hook_results`), so the records below are seeded straight into the store — the
//! same device `extension_registry.rs` uses for records whose writers are not available.
//! Every READ goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::hook_results::{
    failure_mode, hook_status, invocation_point, target_type, StoredHookResult,
    MAX_HOOK_RESULTS,
};
use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    registry_type, type_arn,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{self, PAGE_SIZE};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, member, router_with_engine, store, xml_text, xml_texts, ACCOUNT_ID, REGION,
};

/// Another Region's records, for the rows that hold a read to the Region it was signed for.
const OTHER_REGION: &str = "us-west-2";

/// The two Hooks the seeded invocations belong to, and a third that was never invoked.
/// `HookTypeName` is `@length(10..196)`, which every one of these satisfies.
const HOOK_A: &str = "My::Sample::Hook";
const HOOK_B: &str = "Other::Sample::Hook";
const HOOK_C: &str = "Quiet::Sample::Hook";

/// The version of the Hook that was invoked (`HookTypeVersionId`,
/// `@pattern("^[A-Za-z0-9-]+$")`).
const VERSION_ID: &str = "00000001";

/// A `HookInvocationId`: `@length(36..36)` in the 8-4-4-4-12 lowercase-hex form, built from
/// an index so a case can name the record it expects.
fn hook_result_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012}")
}

/// The ARN of a change set, which is what `TargetId` carries for a change-set target: "For
/// change sets, this is the change set ARN."
fn change_set_arn(name: &str) -> String {
    format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:changeSet/{name}/{}", hook_result_id(9))
}

/// A stack's ARN, for the record whose target type is one no request may filter on.
fn stack_arn(name: &str) -> String {
    format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{}", hook_result_id(8))
}

/// The `HookRequestToken` a Cloud Control API operation answers, which is what `TargetId`
/// carries for a `CLOUD_CONTROL` target.
const CLOUD_CONTROL_TOKEN: &str = "11111111-2222-4333-8444-555555555555";

/// One instant per record, so a case asserting `InvokedAt` reads the record's own value.
fn invoked_at(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64)
}

/// The invocation this suite's table is written against: six records, in the order they
/// were recorded.
fn seeded_results() -> Vec<StoredHookResult> {
    let mut first = StoredHookResult::new(
        hook_result_id(1),
        REGION,
        HOOK_A,
        type_arn(REGION, registry_type::HOOK, HOOK_A),
        VERSION_ID,
        hook_status::COMPLETE_SUCCEEDED,
        target_type::CHANGE_SET,
        change_set_arn("first-change-set"),
        invoked_at(1),
    );
    first.type_configuration_version_id = Some("00000002".to_string());
    first.hook_execution_target = Some(stack_arn("hooked"));

    let mut second = StoredHookResult::new(
        hook_result_id(2),
        REGION,
        HOOK_A,
        type_arn(REGION, registry_type::HOOK, HOOK_A),
        VERSION_ID,
        hook_status::COMPLETE_FAILED,
        target_type::CHANGE_SET,
        change_set_arn("first-change-set"),
        invoked_at(2),
    );
    second.hook_status_reason = Some("The target did not satisfy the Hook".to_string());
    second.hook_execution_target = Some(stack_arn("hooked"));

    let mut third = StoredHookResult::new(
        hook_result_id(3),
        REGION,
        HOOK_B,
        type_arn(REGION, registry_type::HOOK, HOOK_B),
        VERSION_ID,
        hook_status::IN_PROGRESS,
        target_type::CHANGE_SET,
        change_set_arn("second-change-set"),
        invoked_at(3),
    );
    third.failure_mode = failure_mode::WARN.to_string();

    let mut fourth = StoredHookResult::new(
        hook_result_id(4),
        REGION,
        HOOK_A,
        type_arn(REGION, registry_type::HOOK, HOOK_A),
        VERSION_ID,
        hook_status::FAILED,
        target_type::CLOUD_CONTROL,
        CLOUD_CONTROL_TOKEN,
        invoked_at(4),
    );
    fourth.hook_execution_target = Some(CLOUD_CONTROL_TOKEN.to_string());

    // A target type that is not a filterable one: it is reported by the unfiltered read,
    // and no request may select it.
    let fifth = StoredHookResult::new(
        hook_result_id(5),
        REGION,
        HOOK_B,
        type_arn(REGION, registry_type::HOOK, HOOK_B),
        VERSION_ID,
        hook_status::COMPLETE_SUCCEEDED,
        target_type::STACK,
        stack_arn("hooked"),
        invoked_at(5),
    );

    // Another Region's record, on the SAME Hook and the SAME target id as the first one.
    let sixth = StoredHookResult::new(
        hook_result_id(6),
        OTHER_REGION,
        HOOK_A,
        type_arn(OTHER_REGION, registry_type::HOOK, HOOK_A),
        VERSION_ID,
        hook_status::COMPLETE_SUCCEEDED,
        target_type::CHANGE_SET,
        change_set_arn("first-change-set"),
        invoked_at(6),
    );

    vec![first, second, third, fourth, fifth, sixth]
}

/// The router with the seeded records already in its store.
fn router_with_seeded_results() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for result in seeded_results() {
            held.record_hook_result(result);
        }
    }
    router
}

/// The `HookResultId`s one response reports, in the order it reports them.
fn reported_ids(body: &str) -> Vec<String> {
    xml_texts(body, "HookResultId")
}

/// One row of the parameter-combination table: the case name, the members the request
/// carries, and the indexes of the seeded records the response must report.
type CombinationCase = (&'static str, Vec<(String, String)>, Vec<usize>);

/// Which of the seeded records a row's store is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seeded {
    /// Every record [`seeded_results`] builds.
    Everything,
    /// Only the other Region's record, so a read signed for [`REGION`] must report none.
    OnlyAnotherRegion,
    /// Nothing at all: an account that has never had a Hook invoked.
    Nothing,
}

/// One row of the empty-answer table: the case name, the request's members, and what its
/// store holds.
type EmptyCase = (&'static str, Vec<(String, String)>, Seeded);

#[tokio::test]
async fn each_supported_parameter_combination_reports_the_invocations_it_names() {
    let router = router_with_seeded_results();
    let hook_a = type_arn(REGION, registry_type::HOOK, HOOK_A);
    let hook_b = type_arn(REGION, registry_type::HOOK, HOOK_B);
    let hook_c = type_arn(REGION, registry_type::HOOK, HOOK_C);

    // Every combination the operation documents, one row each, against the seeded records.
    let cases: Vec<CombinationCase> = vec![
        (
            "no parameters: every invocation this Region holds",
            vec![],
            vec![1, 2, 3, 4, 5],
        ),
        (
            "TypeArn only: one Hook's invocations",
            vec![member("TypeArn", &hook_a)],
            vec![1, 2, 4],
        ),
        (
            "TypeArn only: the other Hook's invocations",
            vec![member("TypeArn", &hook_b)],
            vec![3, 5],
        ),
        (
            "TypeArn and Status: the one invocation in that status",
            vec![
                member("TypeArn", &hook_a),
                member("Status", hook_status::COMPLETE_FAILED),
            ],
            vec![2],
        ),
        (
            "TypeArn and Status: a status that Hook carries none of",
            vec![
                member("TypeArn", &hook_a),
                member("Status", hook_status::IN_PROGRESS),
            ],
            vec![],
        ),
        (
            "TypeArn only: a Hook that was never invoked",
            vec![member("TypeArn", &hook_c)],
            vec![],
        ),
        (
            "TargetId and TargetType: a change set's invocations",
            vec![
                member("TargetType", target_type::CHANGE_SET),
                member("TargetId", &change_set_arn("first-change-set")),
            ],
            vec![1, 2],
        ),
        (
            "TargetId and TargetType: another change set's invocations",
            vec![
                member("TargetType", target_type::CHANGE_SET),
                member("TargetId", &change_set_arn("second-change-set")),
            ],
            vec![3],
        ),
        (
            "TargetId and TargetType: a Cloud Control operation's invocations",
            vec![
                member("TargetType", target_type::CLOUD_CONTROL),
                member("TargetId", CLOUD_CONTROL_TOKEN),
            ],
            vec![4],
        ),
    ];

    for (case, members, expected) in &cases {
        let expected_ids: Vec<String> = expected.iter().copied().map(hook_result_id).collect();
        let (status, body) = call(&router, "ListHookResults", members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            reported_ids(&body),
            expected_ids,
            "case {case:?}: expected {expected_ids:?}, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds every match, so no NextToken, got {body}"
        );
    }

    // The read reports, it does not consume: the same request answers the same records the
    // second time, and the unfiltered read is unchanged by every filtered one above.
    for (case, members, expected) in &cases {
        let expected_ids: Vec<String> = expected.iter().copied().map(hook_result_id).collect();
        let (status, body) = call(&router, "ListHookResults", members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 on the repeat read, got {status} {body}"
        );
        assert_eq!(
            reported_ids(&body),
            expected_ids,
            "case {case:?}: the repeat read must report {expected_ids:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn one_summary_carries_every_member_the_record_holds() {
    let router = router_with_seeded_results();
    let hook_a = type_arn(REGION, registry_type::HOOK, HOOK_A);

    let (status, body) = call(
        &router,
        "ListHookResults",
        &[
            member("TypeArn", &hook_a),
            member("Status", hook_status::COMPLETE_FAILED),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"one record's members\": expected 200, got {status} {body}"
    );

    // The record this row seeded, member by member: every one is read off the record, not
    // off the request.
    for (tag, expected) in [
        ("HookResultId", hook_result_id(2)),
        ("InvocationPoint", invocation_point::PRE_PROVISION.to_string()),
        ("FailureMode", failure_mode::FAIL.to_string()),
        ("TypeName", HOOK_A.to_string()),
        ("TypeVersionId", VERSION_ID.to_string()),
        ("TypeArn", hook_a.clone()),
        ("Status", hook_status::COMPLETE_FAILED.to_string()),
        (
            "HookStatusReason",
            "The target did not satisfy the Hook".to_string(),
        ),
        ("TargetType", target_type::CHANGE_SET.to_string()),
        ("TargetId", change_set_arn("first-change-set")),
        ("HookExecutionTarget", stack_arn("hooked")),
    ] {
        assert_eq!(
            xml_text(&body, tag).as_deref(),
            Some(expected.as_str()),
            "case {tag:?}: expected {expected:?}, got {body}"
        );
    }
    // `InvokedAt` is the record's own instant ([`invoked_at`] of this record's index),
    // rendered as the modelled timestamp.
    assert_eq!(
        xml_text(&body, "InvokedAt").as_deref(),
        Some("2023-11-14T22:13:22Z"),
        "case \"InvokedAt\": the record's own instant, got {body}"
    );
    // A member the record does not carry is absent rather than empty.
    assert_eq!(
        xml_text(&body, "TypeConfigurationVersionId"),
        None,
        "case \"TypeConfigurationVersionId\": absent for a record without one, got {body}"
    );
}

#[tokio::test]
async fn the_target_form_echoes_its_target_and_withholds_the_two_conditional_members() {
    let router = router_with_seeded_results();
    let hook_a = type_arn(REGION, registry_type::HOOK, HOOK_A);
    let first_change_set = change_set_arn("first-change-set");

    // `InvokedAt` and `HookExecutionTarget` are "only shown in responses when the request
    // does not specify TargetType and TargetId filters", and `TargetId`/`TargetType` on the
    // response itself name the target the response is about.
    let (status, target_form) = call(
        &router,
        "ListHookResults",
        &[
            member("TargetType", target_type::CHANGE_SET),
            member("TargetId", &first_change_set),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the target form\": expected 200, got {status} {target_form}"
    );
    let summaries = reported_ids(&target_form).len();
    assert_eq!(
        summaries, 2,
        "case \"the target form\": the change set's two invocations, got {target_form}"
    );
    for tag in ["InvokedAt", "HookExecutionTarget"] {
        assert_eq!(
            xml_text(&target_form, tag),
            None,
            "case \"the target form\": {tag} is withheld, got {target_form}"
        );
    }
    let echoed_targets = xml_texts(&target_form, "TargetId");
    assert_eq!(
        echoed_targets.len(),
        summaries + 1,
        "case \"the target form\": the response echoes TargetId beside each summary's, got {target_form}"
    );
    assert!(
        echoed_targets.iter().all(|id| *id == first_change_set),
        "case \"the target form\": every TargetId is the requested one, got {echoed_targets:?}"
    );
    assert_eq!(
        xml_texts(&target_form, "TargetType").len(),
        summaries + 1,
        "case \"the target form\": the response echoes TargetType too, got {target_form}"
    );

    // The Hook form names no target, so it reports both conditional members and echoes
    // neither `TargetId` nor `TargetType` of its own.
    let (status, hook_form) =
        call(&router, "ListHookResults", &[member("TypeArn", &hook_a)]).await;
    assert_eq!(
        status, 200,
        "case \"the Hook form\": expected 200, got {status} {hook_form}"
    );
    let hook_summaries = reported_ids(&hook_form).len();
    assert_eq!(
        hook_summaries, 3,
        "case \"the Hook form\": that Hook's three invocations, got {hook_form}"
    );
    assert_eq!(
        xml_texts(&hook_form, "InvokedAt").len(),
        hook_summaries,
        "case \"the Hook form\": every summary reports InvokedAt, got {hook_form}"
    );
    assert_eq!(
        xml_texts(&hook_form, "HookExecutionTarget").len(),
        hook_summaries,
        "case \"the Hook form\": every summary reports its HookExecutionTarget, got {hook_form}"
    );
    assert_eq!(
        xml_texts(&hook_form, "TargetId").len(),
        hook_summaries,
        "case \"the Hook form\": one TargetId per summary and none echoed, got {hook_form}"
    );
}

#[tokio::test]
async fn a_filter_that_selects_no_recorded_invocation_is_an_empty_list() {
    // Each supported combination is a query over the recorded invocations, so one that
    // selects none answers the empty list — never a refusal, and never another filter's
    // records.
    let cases: Vec<EmptyCase> = vec![
        (
            "a change set never invoked against",
            vec![
                member("TargetType", target_type::CHANGE_SET),
                member("TargetId", &change_set_arn("no-such-change-set")),
            ],
            Seeded::Everything,
        ),
        (
            "a Cloud Control request token with no invocations",
            vec![
                member("TargetType", target_type::CLOUD_CONTROL),
                member("TargetId", "99999999-8888-4777-8666-555555555555"),
            ],
            Seeded::Everything,
        ),
        (
            "a Hook that was never invoked",
            vec![member(
                "TypeArn",
                &type_arn(REGION, registry_type::HOOK, HOOK_C),
            )],
            Seeded::Everything,
        ),
        (
            "a target whose only invocation is another Region's",
            vec![
                member("TargetType", target_type::CHANGE_SET),
                member("TargetId", &change_set_arn("first-change-set")),
            ],
            Seeded::OnlyAnotherRegion,
        ),
        (
            "the unfiltered read of a store holding nothing",
            vec![],
            Seeded::Nothing,
        ),
    ];
    for (case, members, seeded) in cases {
        // Each row gets its own store: everything, only what the OTHER Region holds (so the
        // read is held to the Region it was signed for), or nothing at all.
        let (router, engine) = router_with_engine();
        {
            let state = store(&engine);
            let mut held = state.lock();
            for result in seeded_results().into_iter().filter(|result| match seeded {
                Seeded::Everything => true,
                Seeded::OnlyAnotherRegion => result.region != REGION,
                Seeded::Nothing => false,
            }) {
                held.record_hook_result(result);
            }
        }

        let (status, body) = call(&router, "ListHookResults", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: expected no summaries, got {body}"
        );
        assert!(
            body.contains("<HookResults"),
            "case {case:?}: HookResults is reported as an empty element, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: an empty page carries no NextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn a_combination_of_members_the_operation_does_not_support_is_refused() {
    let router = router_with_seeded_results();
    let hook_a = type_arn(REGION, registry_type::HOOK, HOOK_A);
    let first_change_set = change_set_arn("first-change-set");

    // Only four combinations are supported: none, `TypeArn`, `TypeArn` with `Status`, and
    // `TargetId` with `TargetType` — where the target type must be a filterable one.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "Status without TypeArn",
            vec![member("Status", hook_status::COMPLETE_SUCCEEDED)],
        ),
        (
            "TargetId without TargetType",
            vec![member("TargetId", &first_change_set)],
        ),
        (
            "TargetType without TargetId",
            vec![member("TargetType", target_type::CHANGE_SET)],
        ),
        (
            "the target form together with TypeArn",
            vec![
                member("TargetType", target_type::CHANGE_SET),
                member("TargetId", &first_change_set),
                member("TypeArn", &hook_a),
            ],
        ),
        (
            "the target form together with Status",
            vec![
                member("TargetType", target_type::CHANGE_SET),
                member("TargetId", &first_change_set),
                member("Status", hook_status::COMPLETE_SUCCEEDED),
            ],
        ),
        (
            "a STACK target, which is not a supported filter option",
            vec![
                member("TargetType", target_type::STACK),
                member("TargetId", &stack_arn("hooked")),
            ],
        ),
        (
            "a RESOURCE target, which is not a supported filter option",
            vec![
                member("TargetType", target_type::RESOURCE),
                member("TargetId", &stack_arn("hooked")),
            ],
        ),
    ];
    for (case, members) in cases {
        let (status, body) = call(&router, "ListHookResults", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn the_invocation_results_are_read_back_one_bounded_page_at_a_time() {
    let (router, engine) = router_with_engine();
    let total = PAGE_SIZE + 3;
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..total {
            held.record_hook_result(StoredHookResult::new(
                hook_result_id(index),
                REGION,
                HOOK_A,
                type_arn(REGION, registry_type::HOOK, HOOK_A),
                VERSION_ID,
                hook_status::COMPLETE_SUCCEEDED,
                target_type::CHANGE_SET,
                change_set_arn("paged-change-set"),
                invoked_at(index),
            ));
        }
    }

    let (status, first) = call(&router, "ListHookResults", &[]).await;
    assert_eq!(
        status, 200,
        "case \"the first page\": expected 200, got {status} {first}"
    );
    let first_ids = reported_ids(&first);
    assert_eq!(
        first_ids.len(),
        PAGE_SIZE,
        "case \"the first page\": at most {PAGE_SIZE} items, got {}",
        first_ids.len()
    );
    let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
        panic!("case \"the first page\": a page with more behind it carries a NextToken, got {first}")
    });

    let (status, second) = call(&router, "ListHookResults", &[member("NextToken", &token)]).await;
    assert_eq!(
        status, 200,
        "case \"the resumed page\": expected 200, got {status} {second}"
    );
    let second_ids = reported_ids(&second);
    assert_eq!(
        second_ids.len(),
        total - PAGE_SIZE,
        "case \"the resumed page\": the remainder, got {second_ids:?}"
    );
    assert_eq!(
        xml_text(&second, "NextToken"),
        None,
        "case \"the resumed page\": the last page carries no NextToken, got {second}"
    );
    let mut paged = first_ids.clone();
    paged.extend(second_ids);
    let distinct: std::collections::BTreeSet<String> = paged.iter().cloned().collect();
    assert_eq!(
        distinct.len(),
        total,
        "case \"the pages together\": every record exactly once, got {} ids",
        paged.len()
    );

    // A cursor this collection never issued is a bad request, not an empty page.
    for (case, bad_token) in [
        (
            "a cursor past the end",
            state::encode_next_token(state::PageOf::HookResults, total + 1),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(state::PageOf::StackEvents, 0),
        ),
    ] {
        let (status, body) =
            call(&router, "ListHookResults", &[member("NextToken", &bad_token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn the_store_keeps_at_most_the_retention_bound_of_invocation_results() {
    let (router, engine) = router_with_engine();
    let beyond_the_bound = 3;
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..MAX_HOOK_RESULTS + beyond_the_bound {
            held.record_hook_result(StoredHookResult::new(
                hook_result_id(index),
                REGION,
                HOOK_A,
                type_arn(REGION, registry_type::HOOK, HOOK_A),
                VERSION_ID,
                hook_status::COMPLETE_SUCCEEDED,
                target_type::CHANGE_SET,
                change_set_arn("bounded-change-set"),
                invoked_at(index),
            ));
        }
        assert_eq!(
            held.hook_results.len(),
            MAX_HOOK_RESULTS,
            "case \"past the bound\": the store keeps at most {MAX_HOOK_RESULTS}, got {}",
            held.hook_results.len()
        );
    }

    // The OLDEST records are the reclaimed ones, so the first page starts at the first
    // record still held.
    let (status, body) = call(&router, "ListHookResults", &[]).await;
    assert_eq!(
        status, 200,
        "case \"past the bound\": expected 200, got {status} {body}"
    );
    assert_eq!(
        reported_ids(&body).first().map(String::as_str),
        Some(hook_result_id(beyond_the_bound).as_str()),
        "case \"past the bound\": the oldest surviving record comes first, got {body}"
    );
}
