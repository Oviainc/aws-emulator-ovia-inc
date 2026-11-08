// SPDX-License-Identifier: CC0-1.0

//! The SENTENCES three CloudFormation refusals carry, end to end over the awsQuery wire.
//!
//! RULE under test: **a refusal's message is the SERVICE's own wording for that error class —
//! its fixed text exactly, with only the parts the request or the store supplies filled in —
//! and never a restatement of what the model happens to declare.** Three classes stand for
//! it here, and each is a message a client branches on because status and code alone do not
//! discriminate:
//!
//! * the exclusivity a second `StartResourceScan` is refused under, whose sentence tells the
//!   caller to wait for the running scan and names the call that reports its status;
//! * a `ListHookResults` `TargetType` the model admits but the operation does not FILTER on,
//!   refused as `Invalid TargetType: <the value the request named>`;
//! * a value outside an enum's set, whose clause states the value set THIS SERVICE accepts,
//!   which for `OperationStatus` is wider than the set the model declares.
//!
//! The tables span what each rule generalises over: for the scan, the shape of the second
//! start (bare, filtered, under a token of its own) and the retry under the running scan's
//! own token, which is not a refusal at all; for the hook filter, every value the modelled
//! enum carries, the two filterable ones included; for the enum clause, the member that
//! carried the value (two members of one enum, in two different operations) and the value
//! itself.
//!
//! Every sentence is spelled out here rather than imported from the crate, because the
//! sentence IS the requirement: a reworded message must fail this suite.

use std::time::Instant;

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, member, router, xml_text};

/// The sentence a request to START a scan is refused with while one is running: fixed text,
/// naming nothing of the request.
const ANOTHER_SCAN_IS_RUNNING: &str = "There is an active ResourceScan in progress. Please \
     wait for it to finish before starting a new ResourceScan. Describe ResourceScans to see \
     the status of the in-progress ResourceScan.";

/// The `awsQueryError` code the modelled `ResourceScanInProgressException` carries on the
/// wire.
const SCAN_IN_PROGRESS_CODE: &str = "ResourceScanInProgress";

/// This service's one wire code for a request it refuses on input grounds.
const VALIDATION_ERROR: &str = "ValidationError";

/// Every value the model's `ListHookResultsTargetType` enum carries, with whether it is a
/// value the operation FILTERS on.
///
/// Source: `smithy_query(query="shape", shape="ListHookResultsTargetType")` gives
/// `[CHANGE_SET, STACK, RESOURCE, CLOUD_CONTROL]`, and
/// `ListHookResultsInput$TargetType`'s own documentation says "currently, only CHANGE_SET and
/// CLOUD_CONTROL are supported filter options" — so the other two are well-formed wire values
/// the operation refuses at run time.
const MODELLED_TARGET_TYPES: &[(&str, bool)] = &[
    ("CHANGE_SET", true),
    ("STACK", false),
    ("RESOURCE", false),
    ("CLOUD_CONTROL", true),
];

/// A `TargetId` in the modelled `HookResultId` pattern that nothing in the store has a Hook
/// invocation for: the rows are about the TYPE, so every one of them names the same target.
const UNHOOKED_TARGET: &str =
    "arn:aws:cloudformation:us-east-1:000000000000:changeSet/unhooked/0123";

/// `StartResourceScan`, with one `ScanFilters` entry when the row asks for one and a
/// `ClientRequestToken` when it names one.
async fn start_scan(
    router: &MultiServiceRouter,
    types: &[&str],
    token: Option<&str>,
) -> (u16, String) {
    let mut members: Vec<(String, String)> = types
        .iter()
        .enumerate()
        .map(|(position, value)| {
            member(
                &format!("ScanFilters.member.1.Types.member.{}", position + 1),
                value,
            )
        })
        .collect();
    if let Some(token) = token {
        members.push(member("ClientRequestToken", token));
    }
    call(router, "StartResourceScan", &members).await
}

/// One row of the second-start table: what that start carries, and whether it is refused.
struct SecondStart {
    case: &'static str,
    types: &'static [&'static str],
    token: Option<&'static str>,
    refused: bool,
}

#[tokio::test]
async fn a_second_scan_is_refused_with_the_services_own_exclusivity_sentence() {
    let rows = [
        SecondStart {
            case: "a bare second start",
            types: &[],
            token: None,
            refused: true,
        },
        SecondStart {
            case: "a second start filtered to one type",
            types: &["AWS::SNS::Topic"],
            token: None,
            refused: true,
        },
        SecondStart {
            case: "a second start under a token of its own",
            types: &[],
            token: Some("another-start"),
            refused: true,
        },
        // "Specify this token if you plan to retry requests so that CloudFormation knows
        // that you're not attempting to start a new resource scan": the RUNNING scan's own
        // token addresses that scan, so it is answered rather than refused.
        SecondStart {
            case: "the running scan's own token",
            types: &[],
            token: Some("the-running-scan"),
            refused: false,
        },
    ];

    for row in rows {
        // A router per row, so each row meets a store with exactly one scan running.
        let router = router();
        let started = Instant::now();
        let (status, body) = start_scan(&router, &[], Some("the-running-scan")).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first StartResourceScan must be accepted, got {status} {body}",
            row.case
        );
        let running = xml_text(&body, "ResourceScanId").unwrap_or_default();

        let (status, body) = start_scan(&router, row.types, row.token).await;
        if started.elapsed() >= scans::SCAN_DELAY {
            // The clock concluded the first scan before the second call was served, so no
            // scan was running: this row says nothing on a machine that slow.
            continue;
        }
        if !row.refused {
            assert_eq!(
                status, 200,
                "case {:?}: expected the retry to be answered, got {status} {body}",
                row.case
            );
            assert_eq!(
                xml_text(&body, "ResourceScanId").as_deref(),
                Some(running.as_str()),
                "case {:?}: expected the running scan's id {running:?}, got {body}",
                row.case
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {:?}: expected the exclusivity refusal, got {status} {body}",
            row.case
        );
        let code = xml_text(&body, "Code");
        assert_eq!(
            code.as_deref(),
            Some(SCAN_IN_PROGRESS_CODE),
            "case {:?}: expected the modelled code {SCAN_IN_PROGRESS_CODE}, got {code:?} in \
             {body}",
            row.case
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            message, ANOTHER_SCAN_IS_RUNNING,
            "case {:?}: expected {ANOTHER_SCAN_IS_RUNNING:?}, got {message:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_target_type_that_is_not_a_filter_option_is_refused_by_the_value_it_named() {
    let router = router();
    for (target_type, filterable) in MODELLED_TARGET_TYPES {
        let case = *target_type;
        let (status, body) = call(
            &router,
            "ListHookResults",
            &[
                member("TargetType", target_type),
                member("TargetId", UNHOOKED_TARGET),
            ],
        )
        .await;
        let message = xml_text(&body, "Message").unwrap_or_default();
        let invalid = format!("Invalid TargetType: {target_type}");
        if *filterable {
            // A filterable type is a request the operation SERVES, so whatever it answers
            // for a target nothing hooked, it is not this refusal.
            assert_ne!(
                message, invalid,
                "case {case:?}: a supported filter option must not be refused as an invalid \
                 TargetType, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the run-time refusal of a value that is not a filter \
             option, got {status} {body}"
        );
        let code = xml_text(&body, "Code");
        assert_eq!(
            code.as_deref(),
            Some(VALIDATION_ERROR),
            "case {case:?}: expected {VALIDATION_ERROR}, got {code:?} in {body}"
        );
        assert_eq!(
            message, invalid,
            "case {case:?}: expected {invalid:?}, got {message:?}"
        );
    }
}

/// One row of the enum-clause table: the request that carries a value outside an enum, the
/// member path the clause names it at, and the value set the service states for that enum.
struct OffEnumValue {
    case: &'static str,
    action: &'static str,
    members: Vec<(String, String)>,
    value: &'static str,
    path: &'static str,
    value_set: &'static str,
}

/// The value set this service states for the model's `OperationStatus` enum, in the order it
/// states it.
///
/// Source: the service's own refusal of a value outside the set, which reports the
/// cancellation statuses a handler also reports beside the four the model declares
/// (`smithy_query(query="shape", shape="OperationStatus")` gives
/// `[PENDING, IN_PROGRESS, SUCCESS, FAILED]`) — the list a caller fixes its request from.
const OPERATION_STATUS_SET: &str =
    "CANCEL_IN_PROGRESS, SUCCESS, IN_PROGRESS, PENDING, FAILED, CANCEL_COMPLETE";

/// The value set this service states for the model's `TemplateStage` enum: the same two
/// values the model declares (`[Original, Processed]`), in the order the service lists them.
const TEMPLATE_STAGE_SET: &str = "Processed, Original";

#[tokio::test]
async fn a_value_outside_an_enum_states_the_value_set_the_service_accepts() {
    let rows = vec![
        OffEnumValue {
            case: "an operation status no handler reports",
            action: "RecordHandlerProgress",
            members: vec![
                member("BearerToken", "a-handler-invocation"),
                member("OperationStatus", "BOGUS_STATUS"),
            ],
            value: "BOGUS_STATUS",
            path: "operationStatus",
            value_set: OPERATION_STATUS_SET,
        },
        OffEnumValue {
            case: "a valid status in the wrong case",
            action: "RecordHandlerProgress",
            members: vec![
                member("BearerToken", "a-handler-invocation"),
                member("OperationStatus", "in_progress"),
            ],
            value: "in_progress",
            path: "operationStatus",
            value_set: OPERATION_STATUS_SET,
        },
        // The SAME enum at another member of the same operation: the value set is a property
        // of the enum, so the clause states it wherever the enum is reached.
        OffEnumValue {
            case: "the asserted current status, outside the same enum",
            action: "RecordHandlerProgress",
            members: vec![
                member("BearerToken", "a-handler-invocation"),
                member("OperationStatus", "SUCCESS"),
                member("CurrentOperationStatus", "CANCELLED"),
            ],
            value: "CANCELLED",
            path: "currentOperationStatus",
            value_set: OPERATION_STATUS_SET,
        },
        // ...and another enum, of another operation, whose set this service states in its
        // own order: one rule, every enum it holds a stated set for.
        OffEnumValue {
            case: "a template stage outside its enum",
            action: "GetTemplate",
            members: vec![
                member("StackName", "a-stack-no-clause-reaches"),
                member("TemplateStage", "Rendered"),
            ],
            value: "Rendered",
            path: "templateStage",
            value_set: TEMPLATE_STAGE_SET,
        },
    ];

    let router = router();
    for row in rows {
        let (status, body) = call(&router, row.action, &row.members).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the constraint refusal, got {status} {body}",
            row.case
        );
        let code = xml_text(&body, "Code");
        assert_eq!(
            code.as_deref(),
            Some(VALIDATION_ERROR),
            "case {:?}: expected {VALIDATION_ERROR}, got {code:?} in {body}",
            row.case
        );
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{}]",
            row.value, row.path, row.value_set
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.case
        );
    }
}
