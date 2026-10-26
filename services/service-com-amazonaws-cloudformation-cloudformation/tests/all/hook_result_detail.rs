// SPDX-License-Identifier: CC0-1.0

//! `GetHookResult` over the awsQuery wire: the DETAIL of one recorded Hook invocation
//! result.
//!
//! The rule every case here holds the service to: **`GetHookResult` reports the ONE Hook
//! invocation result this account and the Region the request was signed for persists under
//! the `HookResultId` it names — every member read off that record and none off the
//! request, unchanged by being read — and refuses with the modelled
//! `HookResultNotFoundException` (HTTP 404, wire code `HookResultNotFound`) when it holds
//! no record under that id.**
//!
//! The table below is one row per shape a record can take: each `HookStatus`, both
//! `HookFailureMode`s, a record carrying every optional member and one carrying none of
//! them, an aliased Hook (`OriginalTypeName`), a `CLOUD_CONTROL` invocation beside the
//! change-set ones, and another Region's record — so a row's expected members distinguish
//! each record from every other and from the request that asked for it.
//!
//! The store's WRITER is a Hook INVOCATION, which nothing in this emulator performs (see
//! `state::hook_results`), so the records below are seeded straight into the store, the
//! same device `hook_result_invocations.rs` uses for the summaries read. Every READ goes
//! through the real request path (form-encoded body in, XML envelope out), so what is
//! asserted is what a client sees.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::change_set_hooks::target_type as hook_target_type;
use service_com_amazonaws_cloudformation_cloudformation::state::hook_results::{
    annotation_severity_level, annotation_status, failure_mode, hook_status, invocation_point,
    target_action, target_type, StoredAnnotation, StoredHookResult, StoredHookTarget,
    MAX_ANNOTATIONS_PER_RESULT,
};
use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    registry_type, type_arn,
};
use service_com_amazonaws_cloudformation_cloudformation::state::HOOK_RESULT_NOT_FOUND;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, member, router_with_engine, store, xml_blocks, xml_text, xml_texts, ACCOUNT_ID,
    REGION,
};

/// The Region a row holds the read to the Region it was signed for with.
const OTHER_REGION: &str = "eu-west-1";

/// The Hooks the seeded invocations belong to. `HookTypeName` is `@length(10..196)`, which
/// each of these satisfies.
const HOOK_A: &str = "My::Sample::Hook";
const HOOK_B: &str = "Other::Sample::Hook";

/// The public type name of the Hook `HOOK_ALIAS` is an alias for: "if you activate
/// `AWS::Hooks::GuardHook` with alias `MyCompany::Custom::GuardHook`, then `TypeName` will
/// be `MyCompany::Custom::GuardHook` and `OriginalTypeName` will be
/// `AWS::Hooks::GuardHook`" (`GetHookResultOutput$OriginalTypeName`).
const HOOK_ALIAS: &str = "MyCompany::Custom::GuardHook";
const HOOK_ORIGINAL: &str = "AWS::Hooks::GuardHook";

/// The version of the Hook that was invoked (`HookTypeVersionId`,
/// `@pattern("^[A-Za-z0-9-]+$")`).
const VERSION_ID: &str = "00000001";

/// The version of the Hook configuration used during invocation
/// (`HookTypeConfigurationVersionId`, same pattern).
const CONFIGURATION_VERSION_ID: &str = "00000002";

/// A `HookInvocationId`: `@length(36..36)` in the 8-4-4-4-12 lowercase-hex form the model's
/// pattern admits, built from an index so a case can name the record it asks for.
fn hook_result_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012}")
}

/// The ARN of a change set, which is what a change-set invocation's `TargetId` carries:
/// "For change sets, this is the change set ARN."
fn change_set_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{region}:{ACCOUNT_ID}:changeSet/{name}/{}",
        hook_result_id(99)
    )
}

/// A stack's ARN, which is what `HookExecutionTarget` carries for a change-set invocation:
/// "the Amazon Resource Name (ARN) of the target stack".
fn stack_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:cloudformation:{region}:{ACCOUNT_ID}:stack/{name}/{}",
        hook_result_id(98)
    )
}

/// The `HookRequestToken` a Cloud Control API operation answers, which is what `TargetId`
/// carries for a `CLOUD_CONTROL` invocation.
const CLOUD_CONTROL_TOKEN: &str = "11111111-2222-4333-8444-555555555555";

/// One instant per record, so a case asserting `InvokedAt` reads the record's own value.
fn invoked_at(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64)
}

/// `InvokedAt` as the modelled timestamp renders on the wire, for the record at `index`.
fn invoked_at_text(index: usize) -> String {
    // 1_700_000_000 is 2023-11-14T22:13:20Z, so each index is that many seconds past it.
    let seconds = 20 + index;
    format!("2023-11-14T22:13:{seconds:02}Z")
}

/// What one row expects the detailed read to report, member by member: the tag and the
/// text, or the tag and `None` for a member the record does not carry.
type Expected = Vec<(&'static str, Option<String>)>;

/// One row of the table: the case name, the record seeded, and what reading it back by id
/// must report.
struct Row {
    case: &'static str,
    stored: StoredHookResult,
    expected: Expected,
}

/// A record with every optional member unset, so a row states only what it fills in.
#[allow(clippy::too_many_arguments)]
fn record(
    index: usize,
    region: &str,
    type_name: &str,
    status: &str,
    target_type: &str,
    target_id: &str,
) -> StoredHookResult {
    StoredHookResult::new(
        hook_result_id(index),
        region,
        type_name,
        type_arn(region, registry_type::HOOK, type_name),
        VERSION_ID,
        status,
        target_type,
        target_id,
        invoked_at(index),
    )
}

/// The table: one row per shape a recorded invocation can take.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();

    // A completed, successful invocation of a change set, carrying every optional member a
    // record can hold: the configuration version, a status reason, the execution target,
    // the invocation's target and two annotations.
    let mut full = record(
        1,
        REGION,
        HOOK_A,
        hook_status::COMPLETE_SUCCEEDED,
        target_type::CHANGE_SET,
        &change_set_arn(REGION, "first-change-set"),
    );
    full.type_configuration_version_id = Some(CONFIGURATION_VERSION_ID.to_string());
    full.hook_status_reason = Some("Hook succeeded".to_string());
    full.hook_execution_target = Some(stack_arn(REGION, "hooked"));
    full.target = Some(StoredHookTarget::new(
        "AWS::S3::Bucket",
        "hooked-bucket",
        target_action::CREATE,
    ));
    let mut passed = StoredAnnotation::new("GuardRule-S3-01", annotation_status::PASSED);
    passed.status_message = Some("Bucket blocks public access".to_string());
    passed.severity_level = Some(annotation_severity_level::INFORMATIONAL.to_string());
    let mut skipped = StoredAnnotation::new("GuardRule-S3-02", annotation_status::SKIPPED);
    skipped.status_message = Some("Rule does not apply to this bucket".to_string());
    skipped.severity_level = Some(annotation_severity_level::LOW.to_string());
    full.annotations = vec![passed, skipped];
    rows.push(Row {
        case: "a completed invocation carrying every optional member",
        stored: full,
        expected: vec![
            ("HookResultId", Some(hook_result_id(1))),
            ("InvocationPoint", Some(invocation_point::PRE_PROVISION.to_string())),
            ("FailureMode", Some(failure_mode::FAIL.to_string())),
            ("TypeName", Some(HOOK_A.to_string())),
            ("OriginalTypeName", None),
            ("TypeVersionId", Some(VERSION_ID.to_string())),
            (
                "TypeConfigurationVersionId",
                Some(CONFIGURATION_VERSION_ID.to_string()),
            ),
            (
                "TypeArn",
                Some(type_arn(REGION, registry_type::HOOK, HOOK_A)),
            ),
            ("Status", Some(hook_status::COMPLETE_SUCCEEDED.to_string())),
            ("HookStatusReason", Some("Hook succeeded".to_string())),
            ("InvokedAt", Some(invoked_at_text(1))),
            ("TargetType", Some(hook_target_type::RESOURCE.to_string())),
            ("TargetTypeName", Some("AWS::S3::Bucket".to_string())),
            ("TargetId", Some("hooked-bucket".to_string())),
            ("Action", Some(target_action::CREATE.to_string())),
        ],
    });

    // A failed validation, in the other failure mode, whose annotation carries the
    // remediation guidance the operation exists to hand back.
    let mut failed = record(
        2,
        REGION,
        HOOK_B,
        hook_status::COMPLETE_FAILED,
        target_type::CHANGE_SET,
        &change_set_arn(REGION, "second-change-set"),
    );
    failed.failure_mode = failure_mode::WARN.to_string();
    failed.hook_status_reason = Some("The target did not satisfy the Hook".to_string());
    failed.target = Some(StoredHookTarget::new(
        "AWS::S3::Bucket",
        "open-bucket",
        target_action::UPDATE,
    ));
    let mut violation = StoredAnnotation::new("GuardRule-S3-01", annotation_status::FAILED);
    violation.status_message = Some("Bucket does not block public access".to_string());
    violation.remediation_message = Some("Block public access to the bucket".to_string());
    violation.remediation_link =
        Some("https://example.invalid/remediation/s3-public-access".to_string());
    violation.severity_level = Some(annotation_severity_level::CRITICAL.to_string());
    failed.annotations = vec![violation];
    rows.push(Row {
        case: "a failed validation with remediation guidance",
        stored: failed,
        expected: vec![
            ("HookResultId", Some(hook_result_id(2))),
            ("FailureMode", Some(failure_mode::WARN.to_string())),
            ("TypeName", Some(HOOK_B.to_string())),
            ("Status", Some(hook_status::COMPLETE_FAILED.to_string())),
            (
                "HookStatusReason",
                Some("The target did not satisfy the Hook".to_string()),
            ),
            ("InvokedAt", Some(invoked_at_text(2))),
            ("TargetTypeName", Some("AWS::S3::Bucket".to_string())),
            ("TargetId", Some("open-bucket".to_string())),
            ("Action", Some(target_action::UPDATE.to_string())),
        ],
    });

    // An invocation still running: no status reason, no annotations and no target yet, so
    // every one of those members is absent rather than empty.
    rows.push(Row {
        case: "an invocation still in progress, carrying no optional member",
        stored: record(
            3,
            REGION,
            HOOK_A,
            hook_status::IN_PROGRESS,
            target_type::CHANGE_SET,
            &change_set_arn(REGION, "first-change-set"),
        ),
        expected: vec![
            ("HookResultId", Some(hook_result_id(3))),
            ("Status", Some(hook_status::IN_PROGRESS.to_string())),
            ("TypeConfigurationVersionId", None),
            ("HookStatusReason", None),
            ("OriginalTypeName", None),
            ("TargetTypeName", None),
            ("Action", None),
            ("InvokedAt", Some(invoked_at_text(3))),
        ],
    });

    // An invocation that errored during execution, against a Cloud Control operation
    // rather than a change set, on a DELETE action.
    let mut errored = record(
        4,
        REGION,
        HOOK_A,
        hook_status::FAILED,
        target_type::CLOUD_CONTROL,
        CLOUD_CONTROL_TOKEN,
    );
    errored.hook_execution_target = Some(CLOUD_CONTROL_TOKEN.to_string());
    errored.hook_status_reason = Some("The Hook handler returned an error".to_string());
    errored.target = Some(StoredHookTarget::new(
        "AWS::DynamoDB::Table",
        "orders",
        target_action::DELETE,
    ));
    rows.push(Row {
        case: "an invocation that errored, against a Cloud Control operation",
        stored: errored,
        expected: vec![
            ("HookResultId", Some(hook_result_id(4))),
            ("Status", Some(hook_status::FAILED.to_string())),
            ("TargetTypeName", Some("AWS::DynamoDB::Table".to_string())),
            ("TargetId", Some("orders".to_string())),
            ("Action", Some(target_action::DELETE.to_string())),
            ("AnnotationName", None),
            ("InvokedAt", Some(invoked_at_text(4))),
        ],
    });

    // An ALIASED Hook: `TypeName` is the alias and `OriginalTypeName` the public type name
    // it stands for, on an IMPORT action.
    let mut aliased = record(
        5,
        REGION,
        HOOK_ALIAS,
        hook_status::COMPLETE_SUCCEEDED,
        target_type::CHANGE_SET,
        &change_set_arn(REGION, "aliased-change-set"),
    );
    aliased.original_type_name = Some(HOOK_ORIGINAL.to_string());
    aliased.target = Some(StoredHookTarget::new(
        "AWS::SQS::Queue",
        "imported-queue",
        target_action::IMPORT,
    ));
    rows.push(Row {
        case: "an aliased Hook reports both its alias and its original type name",
        stored: aliased,
        expected: vec![
            ("HookResultId", Some(hook_result_id(5))),
            ("TypeName", Some(HOOK_ALIAS.to_string())),
            ("OriginalTypeName", Some(HOOK_ORIGINAL.to_string())),
            (
                "TypeArn",
                Some(type_arn(REGION, registry_type::HOOK, HOOK_ALIAS)),
            ),
            ("Action", Some(target_action::IMPORT.to_string())),
            ("InvokedAt", Some(invoked_at_text(5))),
        ],
    });

    rows
}

/// Another Region's record, read back by a request signed for THAT Region.
fn other_regions_record() -> StoredHookResult {
    let mut stored = record(
        6,
        OTHER_REGION,
        HOOK_A,
        hook_status::COMPLETE_SUCCEEDED,
        target_type::CHANGE_SET,
        &change_set_arn(OTHER_REGION, "first-change-set"),
    );
    stored.hook_execution_target = Some(stack_arn(OTHER_REGION, "hooked"));
    stored
}

/// The router with every row's record, plus the other Region's, already in its store.
fn router_with_seeded_results() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for row in rows() {
            held.record_hook_result(row.stored);
        }
        held.record_hook_result(other_regions_record());
    }
    router
}

/// `GetHookResult` for one id, signed for [`REGION`].
async fn get_hook_result(router: &MultiServiceRouter, hook_result_id: &str) -> (u16, String) {
    call(
        router,
        "GetHookResult",
        &[member("HookResultId", hook_result_id)],
    )
    .await
}

/// The annotations one response reports, in the order it reports them, each as its own
/// block of XML.
fn reported_annotations(body: &str) -> Vec<String> {
    match xml_blocks(body, "Annotations").first() {
        Some(list) => xml_blocks(list, "member"),
        None => Vec::new(),
    }
}

#[tokio::test]
async fn every_shape_of_record_is_reported_member_by_member_from_the_record() {
    let router = router_with_seeded_results();

    for row in rows() {
        let (status, body) = get_hook_result(&router, &row.stored.hook_result_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {body}",
            row.case
        );
        for (tag, expected) in &row.expected {
            let actual = xml_text(&body, tag);
            assert_eq!(
                actual.as_deref(),
                expected.as_deref(),
                "case {:?}: {tag} must be {expected:?}, got {actual:?} in {body}",
                row.case
            );
        }
        // The two structural members are reported exactly when the record holds them, and
        // are absent rather than empty when it does not.
        assert_eq!(
            xml_blocks(&body, "Target").len(),
            usize::from(row.stored.target.is_some()),
            "case {:?}: Target is reported only for a record that holds one, got {body}",
            row.case
        );
        assert_eq!(
            reported_annotations(&body).len(),
            row.stored.annotations.len(),
            "case {:?}: one reported annotation per annotation the record holds, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_annotations_are_reported_in_order_with_every_member_the_record_holds() {
    let router = router_with_seeded_results();

    // The record this suite seeds two annotations on: both are reported, in the order the
    // invocation produced them, each carrying only the members it holds.
    let (status, body) = get_hook_result(&router, &hook_result_id(1)).await;
    assert_eq!(status, 200, "case \"two annotations\": expected 200, got {status} {body}");
    let annotations = reported_annotations(&body);
    assert_eq!(
        annotations.len(),
        2,
        "case \"two annotations\": both are reported, got {annotations:?}"
    );
    assert_eq!(
        xml_texts(&body, "AnnotationName"),
        vec!["GuardRule-S3-01".to_string(), "GuardRule-S3-02".to_string()],
        "case \"two annotations\": in the order the record holds them, got {body}"
    );
    assert_eq!(
        xml_text(&annotations[0], "Status").as_deref(),
        Some(annotation_status::PASSED),
        "case \"the first annotation\": its own status, got {}",
        annotations[0]
    );
    assert_eq!(
        xml_text(&annotations[1], "Status").as_deref(),
        Some(annotation_status::SKIPPED),
        "case \"the second annotation\": its own status, got {}",
        annotations[1]
    );
    for annotation in &annotations {
        assert_eq!(
            xml_text(annotation, "RemediationMessage"),
            None,
            "case \"an annotation with no remediation\": the member is absent, got {annotation}"
        );
    }

    // The failed row's single annotation carries the remediation members, which is what the
    // operation documents it answers for a failed invocation.
    let (status, body) = get_hook_result(&router, &hook_result_id(2)).await;
    assert_eq!(status, 200, "case \"one failing annotation\": expected 200, got {status} {body}");
    let annotations = reported_annotations(&body);
    assert_eq!(
        annotations.len(),
        1,
        "case \"one failing annotation\": the record's one annotation, got {annotations:?}"
    );
    // Read inside the annotation's own element: `Status` is a member of both the result and
    // each annotation, so the whole-body read would answer the result's.
    for (tag, expected) in [
        ("AnnotationName", "GuardRule-S3-01"),
        ("Status", annotation_status::FAILED),
        ("StatusMessage", "Bucket does not block public access"),
        ("RemediationMessage", "Block public access to the bucket"),
        (
            "RemediationLink",
            "https://example.invalid/remediation/s3-public-access",
        ),
        ("SeverityLevel", annotation_severity_level::CRITICAL),
    ] {
        assert_eq!(
            xml_text(&annotations[0], tag).as_deref(),
            Some(expected),
            "case \"one failing annotation\": {tag} must be {expected:?}, got {}",
            annotations[0]
        );
    }
}

#[tokio::test]
async fn an_id_this_region_holds_no_record_under_is_refused_as_not_found() {
    let router = router_with_seeded_results();
    let (empty_router, _engine) = router_with_engine();

    // Each row is an id that addresses no record of the Region the read is signed for.
    let cases: Vec<(&str, &MultiServiceRouter, String)> = vec![
        (
            "an id no invocation was recorded under",
            &router,
            hook_result_id(777),
        ),
        (
            "an id only another Region holds",
            &router,
            hook_result_id(6),
        ),
        (
            "an id of an account that has never had a Hook invoked",
            &empty_router,
            hook_result_id(1),
        ),
    ];

    for (case, router, id) in cases {
        let (status, body) = get_hook_result(router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("HookResultNotFound"),
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            message, HOOK_RESULT_NOT_FOUND,
            "case {case:?}: expected {HOOK_RESULT_NOT_FOUND:?} for the id {id}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_record_is_read_back_only_in_the_region_that_holds_it() {
    let router = router_with_seeded_results();

    // The other Region's record IS addressable there, and this Region's is not: the same
    // store answers each read for the Region its request was signed for.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "GetHookResult",
        &[member("HookResultId", &hook_result_id(6))],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the other Region's own read\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "TypeArn").as_deref(),
        Some(type_arn(OTHER_REGION, registry_type::HOOK, HOOK_A).as_str()),
        "case \"the other Region's own read\": the record's own Hook ARN, got {body}"
    );

    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "GetHookResult",
        &[member("HookResultId", &hook_result_id(1))],
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"this Region's record read from another\": expected 404, got {status} {body}"
    );
}

#[tokio::test]
async fn the_read_reports_and_does_not_consume_the_record() {
    let router = router_with_seeded_results();
    let id = hook_result_id(1);

    let (_, first) = get_hook_result(&router, &id).await;
    let (status, second) = get_hook_result(&router, &id).await;
    assert_eq!(
        status, 200,
        "case \"the repeat read\": expected 200, got {status} {second}"
    );
    for tag in [
        "HookResultId",
        "Status",
        "HookStatusReason",
        "InvokedAt",
        "TargetId",
        "AnnotationName",
    ] {
        assert_eq!(
            xml_texts(&second, tag),
            xml_texts(&first, tag),
            "case \"the repeat read\": {tag} is unchanged by being read, got {second}"
        );
    }

    // And the summaries read still reports it too: neither read consumes the record.
    let (status, listed) = call(&router, "ListHookResults", &[]).await;
    assert_eq!(
        status, 200,
        "case \"the summaries after the detail read\": expected 200, got {status} {listed}"
    );
    assert!(
        xml_texts(&listed, "HookResultId").contains(&id),
        "case \"the summaries after the detail read\": the record is still listed, got {listed}"
    );
}

#[tokio::test]
async fn a_request_that_names_no_result_is_refused_rather_than_answered() {
    let router = router_with_seeded_results();

    let (status, body) = call(&router, "GetHookResult", &[]).await;
    assert_eq!(
        status, 400,
        "case \"no HookResultId\": expected 400, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"no HookResultId\": expected ValidationError, got {body}"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert!(
        message.contains("HookResultId"),
        "case \"no HookResultId\": the refusal names the member, got {message:?}"
    );
}

#[tokio::test]
async fn one_records_annotations_are_bounded_oldest_reclaimed_first() {
    let (router, engine) = router_with_engine();
    let beyond_the_bound = 3;
    let id = hook_result_id(7);
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut stored = record(
            7,
            REGION,
            HOOK_A,
            hook_status::COMPLETE_FAILED,
            target_type::CHANGE_SET,
            &change_set_arn(REGION, "annotated-change-set"),
        );
        stored.annotations = (0..MAX_ANNOTATIONS_PER_RESULT + beyond_the_bound)
            .map(|index| {
                StoredAnnotation::new(format!("GuardRule-{index:04}"), annotation_status::FAILED)
            })
            .collect();
        held.record_hook_result(stored);
    }

    let (status, body) = get_hook_result(&router, &id).await;
    assert_eq!(
        status, 200,
        "case \"past the annotation bound\": expected 200, got {status} {body}"
    );
    let reported = xml_texts(&body, "AnnotationName");
    assert_eq!(
        reported.len(),
        MAX_ANNOTATIONS_PER_RESULT,
        "case \"past the annotation bound\": at most {MAX_ANNOTATIONS_PER_RESULT} are kept, got {}",
        reported.len()
    );
    assert_eq!(
        reported.first().map(String::as_str),
        Some(format!("GuardRule-{beyond_the_bound:04}").as_str()),
        "case \"past the annotation bound\": the oldest surviving annotation comes first, got {reported:?}"
    );
}
