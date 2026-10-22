// SPDX-License-Identifier: CC0-1.0

//! The per-item failure channel of the WorkSpaces batch operations, end to end over the
//! awsJson1_1 wire.
//!
//! `CreateWorkspaces`, `StartWorkspaces`, `StopWorkspaces` and `TerminateWorkspaces`
//! report a per-request failure INSIDE a 200 response, in
//! `FailedCreateWorkspaceRequest$ErrorCode` / `FailedWorkspaceChangeRequest$ErrorCode`.
//! Both members target `com.amazonaws.workspaces#ErrorType`, a free-form per-item code
//! with no enum; the operations themselves model NO errors, so the 200 body is their only
//! failure channel.
//!
//! The rules under test, stated over the set of inputs each class spans rather than over
//! any one request:
//!
//! * A per-item `ErrorCode` is a per-item code from the service's dotted
//!   `<Condition>[.<Resource>]` vocabulary — never the name of a modelled exception
//!   shape, which only a top-level error can carry. Asserted over every per-item failure
//!   this emulator can produce, across all four implemented batch operations.
//! * An id that names no stored WorkSpace is `ResourceNotFound.Workspace` with the
//!   service's FIXED message, identical for every id: the code and message do not vary
//!   with the id the request carried, whatever its length or character mix inside the
//!   model's `^ws-[0-9a-z]{8,63}$`.
//! * A batch reports ONLY the items that failed, whatever the count and the order the
//!   failing items sit in, and the items that succeeded take effect.
//! * An input the model's own constraints refuse (a `WorkspaceId` past the pattern's
//!   63-character bound, a list past its 25-item bound) is refused at the TOP level, not
//!   turned into a per-item failure.

use serde_json::{json, Value};
use service_com_amazonaws_workspaces_workspacesservice::state::{
    ERROR_CODE_BUNDLE_NOT_FOUND, ERROR_CODE_DIRECTORY_NOT_FOUND, ERROR_CODE_INVALID_PARAMETERS,
    ERROR_CODE_LIMIT_EXCEEDED, ERROR_CODE_WORKSPACE_EXISTS, ERROR_CODE_WORKSPACE_INVALID_STATE,
    ERROR_CODE_WORKSPACE_NOT_FOUND, MESSAGE_WORKSPACE_NOT_FOUND,
};

use crate::workspace_lifecycle::{
    call, create_one, emulator_with_engine, failed_requests, register_directory, settle,
};

/// The per-item values a WorkSpaces response carries for an unknown `WorkspaceId`,
/// written out as LITERALS: this is the recorded wire contract, so the suite pins it
/// here rather than asserting the service's own constant against itself.
const RECORDED_NOT_FOUND_CODE: &str = "ResourceNotFound.Workspace";
const RECORDED_NOT_FOUND_MESSAGE: &str = "The specified WorkSpace could not be found.";

/// Every per-item `ErrorCode` this service reports, as `crate::state` declares them — a
/// COMPLETENESS guard for [`assert_is_item_code`], not the behaviour under test: the
/// recorded literals above and the structural form checks are what carry the rule.
const ITEM_ERROR_CODES: &[&str] = &[
    ERROR_CODE_WORKSPACE_NOT_FOUND,
    ERROR_CODE_WORKSPACE_INVALID_STATE,
    ERROR_CODE_WORKSPACE_EXISTS,
    ERROR_CODE_DIRECTORY_NOT_FOUND,
    ERROR_CODE_BUNDLE_NOT_FOUND,
    ERROR_CODE_INVALID_PARAMETERS,
    ERROR_CODE_LIMIT_EXCEEDED,
];

/// The three batch CHANGE operations and the request member each takes, so a case can be
/// stated once and run against all of them.
const CHANGE_OPERATIONS: [(&str, &str); 3] = [
    ("StartWorkspaces", "StartWorkspaceRequests"),
    ("StopWorkspaces", "StopWorkspaceRequests"),
    ("TerminateWorkspaces", "TerminateWorkspaceRequests"),
];

/// Assert that `code` has the per-item form the rule gives — a condition name, at most
/// one dot-qualifying resource, letters only — rather than naming a modelled exception
/// shape, and that it is one of the codes this service reports.
fn assert_is_item_code(case: &str, code: &str) {
    assert!(
        !code.ends_with("Exception"),
        "case {case}: a per-item ErrorCode must not name a modelled exception shape, got {code:?}"
    );
    let mut parts = code.split('.');
    let condition = parts.next().unwrap_or_default();
    let resource = parts.next();
    assert!(
        parts.next().is_none(),
        "case {case}: a per-item code carries at most one dot, got {code:?}"
    );
    assert!(
        !condition.is_empty() && condition.chars().all(|c| c.is_ascii_alphabetic()),
        "case {case}: the condition part must be a non-empty letters-only name, got {code:?}"
    );
    assert!(
        resource.is_none_or(|name| !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphabetic())),
        "case {case}: the resource part must be a non-empty letters-only name, got {code:?}"
    );
    assert!(
        ITEM_ERROR_CODES.contains(&code),
        "case {case}: expected a per-item code from {ITEM_ERROR_CODES:?}, got {code:?}"
    );
}

/// Ids no WorkSpace this emulator mints ever carries, spanning the model's
/// `WorkspaceId` pattern `^ws-[0-9a-z]{8,63}$`: both published length bounds, a
/// digits-only suffix, a letters-only one and a mixed one.
fn unknown_ids() -> Vec<String> {
    vec![
        // The 8-character lower bound, digits only.
        "ws-00000000".to_string(),
        // Letters only, comfortably inside the bounds.
        "ws-nosuchworkspace".to_string(),
        // Mixed digits and letters.
        "ws-0a1b2c3d4e5f6".to_string(),
        // The 63-character upper bound.
        format!("ws-{}", "z".repeat(63)),
    ]
}

/// The service's own per-item constants must spell the recorded wire values.
///
/// The suites above assert the WIRE through the literals, so this is what ties
/// `crate::state`'s constants to the recording: a handler written against the constant
/// cannot drift from what AWS answers without this failing.
#[test]
fn the_service_constants_spell_the_recorded_not_found_pair() {
    assert_eq!(
        ERROR_CODE_WORKSPACE_NOT_FOUND, RECORDED_NOT_FOUND_CODE,
        "ERROR_CODE_WORKSPACE_NOT_FOUND must be the recorded per-item code, got {ERROR_CODE_WORKSPACE_NOT_FOUND:?}"
    );
    assert_eq!(
        MESSAGE_WORKSPACE_NOT_FOUND, RECORDED_NOT_FOUND_MESSAGE,
        "MESSAGE_WORKSPACE_NOT_FOUND must be the recorded per-item message, got {MESSAGE_WORKSPACE_NOT_FOUND:?}"
    );
}

#[tokio::test]
async fn an_unknown_id_is_the_same_per_item_not_found_failure_on_every_batch_operation() {
    let (router, _engine) = emulator_with_engine();

    // The invariant: the (code, message) pair does not depend on the id, so every row
    // collects its message and they must all be the one fixed sentence.
    let mut seen_messages = Vec::new();
    for (operation, member) in CHANGE_OPERATIONS {
        for workspace_id in unknown_ids() {
            let case = format!("{operation}({workspace_id})");
            let (status, body) = call(
                &router,
                operation,
                &json!({ member: [{ "WorkspaceId": workspace_id }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: a per-item failure is reported under 200, body {body:?}"
            );
            let reported = failed_requests(&case, &body);
            assert_eq!(
                reported.len(),
                1,
                "case {case}: expected exactly one FailedRequests entry, got {reported:?}"
            );
            let (reported_id, code, message) = &reported[0];
            assert_eq!(
                reported_id, &workspace_id,
                "case {case}: the failure names the WorkSpace it was asked about, got {reported:?}"
            );
            assert_is_item_code(&case, code);
            assert_eq!(
                code, RECORDED_NOT_FOUND_CODE,
                "case {case}: expected ErrorCode {RECORDED_NOT_FOUND_CODE}, got {code:?}"
            );
            assert_eq!(
                message, RECORDED_NOT_FOUND_MESSAGE,
                "case {case}: expected the fixed ErrorMessage {RECORDED_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(&workspace_id),
                "case {case}: the fixed message must not interpolate the id, got {message:?}"
            );
            seen_messages.push(message.clone());
        }
    }

    assert!(
        seen_messages
            .windows(2)
            .all(|pair| pair[0] == pair[1]),
        "the not-found message must be the same text for every id and operation, got {seen_messages:?}"
    );
}

#[tokio::test]
async fn every_per_item_failure_condition_reports_a_per_item_code() {
    let (router, engine) = emulator_with_engine();
    // A create needs a registered directory and a stored bundle: `wsb-amznvalue` is one
    // of the Amazon-provided catalog bundles the store is seeded with.
    register_directory(&router, "d-1234567890").await;
    let taken = json!({
        "DirectoryId": "d-1234567890",
        "UserName": "alice",
        "BundleId": "wsb-amznvalue",
    });
    let available = create_one(&router, &taken).await;
    settle(&engine);

    // (case name, operation, request body, the expected per-item ErrorCode). The rows
    // span every condition this emulator reports per item, over all four implemented
    // batch operations: an unknown id, a state that forbids the change, an item naming
    // no WorkSpace at all (`StartRequest$WorkspaceId` / `StopRequest$WorkspaceId` are
    // optional in the model), a create colliding with a stored WorkSpace, a create whose
    // member combination the model forbids, and a create naming a directory or a bundle
    // that no state holds.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "terminate unknown",
            "TerminateWorkspaces",
            json!({ "TerminateWorkspaceRequests": [{ "WorkspaceId": "ws-00000000" }] }),
            ERROR_CODE_WORKSPACE_NOT_FOUND,
        ),
        (
            "start an AVAILABLE WorkSpace",
            "StartWorkspaces",
            json!({ "StartWorkspaceRequests": [{ "WorkspaceId": available }] }),
            ERROR_CODE_WORKSPACE_INVALID_STATE,
        ),
        (
            "start without a WorkspaceId",
            "StartWorkspaces",
            json!({ "StartWorkspaceRequests": [{}] }),
            ERROR_CODE_INVALID_PARAMETERS,
        ),
        (
            "stop without a WorkspaceId",
            "StopWorkspaces",
            json!({ "StopWorkspaceRequests": [{}] }),
            ERROR_CODE_INVALID_PARAMETERS,
        ),
        (
            "create over a taken directory slot",
            "CreateWorkspaces",
            json!({ "Workspaces": [taken.clone()] }),
            ERROR_CODE_WORKSPACE_EXISTS,
        ),
        (
            "create user-decoupled without a WorkspaceName",
            "CreateWorkspaces",
            json!({ "Workspaces": [{
                "DirectoryId": "d-1234567890",
                "UserName": "[UNDEFINED]",
                "BundleId": "wsb-amznvalue",
            }] }),
            ERROR_CODE_INVALID_PARAMETERS,
        ),
        (
            "create in a directory nothing registered",
            "CreateWorkspaces",
            json!({ "Workspaces": [{
                "DirectoryId": "d-99999999",
                "UserName": "carol",
                "BundleId": "wsb-amznvalue",
            }] }),
            ERROR_CODE_DIRECTORY_NOT_FOUND,
        ),
        (
            "create from a bundle the store does not hold",
            "CreateWorkspaces",
            json!({ "Workspaces": [{
                "DirectoryId": "d-1234567890",
                "UserName": "dave",
                "BundleId": "wsb-00000000",
            }] }),
            ERROR_CODE_BUNDLE_NOT_FOUND,
        ),
    ];

    for (case, operation, request, expected_code) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: a per-item failure is reported under 200, body {body:?}"
        );
        let reported = body["FailedRequests"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: FailedRequests must be a list, got {body:?}"));
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected exactly one FailedRequests entry, got {reported:?}"
        );
        let code = reported[0]["ErrorCode"].as_str().unwrap_or_default();
        assert_is_item_code(case, code);
        assert_eq!(
            code, expected_code,
            "case {case}: expected ErrorCode {expected_code}, got {reported:?}"
        );
        assert!(
            !reported[0]["ErrorMessage"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case}: a per-item failure carries a message, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_mixed_batch_reports_only_the_items_that_failed() {
    // (case name, how many stored WorkSpaces to terminate, how many unknown ids to
    // mix in, whether the unknown ids come first). The rows differ along the count of
    // items, how many of them fail, and where the failing items sit in the list.
    let cases: [(&str, usize, usize, bool); 5] = [
        ("one stored, one unknown", 1, 1, false),
        ("unknown first", 1, 1, true),
        ("two stored around two unknown", 2, 2, false),
        ("every item unknown", 0, 3, false),
        // The model caps the list at 25 items; this row sits exactly on that bound.
        ("the full 25-item batch", 21, 4, true),
    ];

    for (case, stored_count, unknown_count, unknown_first) in cases {
        let (router, _engine) = emulator_with_engine();
        register_directory(&router, "d-1234567890").await;
        let mut stored = Vec::new();
        for index in 0..stored_count {
            stored.push(
                create_one(
                    &router,
                    &json!({
                        "DirectoryId": "d-1234567890",
                        "UserName": format!("user{index}"),
                        "BundleId": "wsb-amznvalue",
                    }),
                )
                .await,
            );
        }
        let unknown: Vec<String> = unknown_ids().into_iter().take(unknown_count).collect();

        let mut items: Vec<Value> = Vec::new();
        let mut push = |ids: &[String]| {
            for id in ids {
                items.push(json!({ "WorkspaceId": id }));
            }
        };
        if unknown_first {
            push(&unknown);
            push(&stored);
        } else {
            push(&stored);
            push(&unknown);
        }

        // Terminating is allowed from every state a stored WorkSpace can be in, so the
        // stored items must succeed whatever the clock owes them.
        let (status, body) = call(
            &router,
            "TerminateWorkspaces",
            &json!({ "TerminateWorkspaceRequests": items }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the batch answers 200, body {body:?}"
        );
        let reported = failed_requests(case, &body);
        let reported_ids: Vec<String> = reported.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            reported_ids, unknown,
            "case {case}: only the unknown ids may be reported, got {reported:?}"
        );
        for (id, code, message) in &reported {
            assert_is_item_code(case, code);
            assert_eq!(
                code, RECORDED_NOT_FOUND_CODE,
                "case {case}: id {id} must be reported as {RECORDED_NOT_FOUND_CODE}, got {code:?}"
            );
            assert_eq!(
                message, RECORDED_NOT_FOUND_MESSAGE,
                "case {case}: id {id} must carry the fixed message, got {message:?}"
            );
        }

        // The items that succeeded took effect: every stored WorkSpace is now
        // TERMINATING, read back through `DescribeWorkspaces`.
        for workspace_id in &stored {
            let (status, body) = call(
                &router,
                "DescribeWorkspaces",
                &json!({ "WorkspaceIds": [workspace_id] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeWorkspaces({workspace_id}) answers 200, body {body:?}"
            );
            assert_eq!(
                body["Workspaces"][0]["State"].as_str(),
                Some("TERMINATING"),
                "case {case}: {workspace_id} must be TERMINATING after the batch, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_input_the_model_refuses_is_a_top_level_error_not_a_per_item_failure() {
    let (router, _engine) = emulator_with_engine();

    // One past each published bound: a `WorkspaceId` whose suffix is 64 characters
    // (the pattern allows 8..63) and a list of 26 items (the member allows 1..25).
    let too_long_id = format!("ws-{}", "z".repeat(64));
    let mut cases: Vec<(String, &str, Value)> = Vec::new();
    for (operation, member) in CHANGE_OPERATIONS {
        cases.push((
            format!("{operation}: a 64-character WorkspaceId suffix"),
            operation,
            json!({ member: [{ "WorkspaceId": too_long_id }] }),
        ));
        // Every id here satisfies the pattern, so the only thing wrong with the
        // request is the item count.
        let oversized: Vec<Value> = (0..26)
            .map(|index| json!({ "WorkspaceId": format!("ws-unknown{index:04}") }))
            .collect();
        cases.push((
            format!("{operation}: a 26-item batch"),
            operation,
            json!({ member: oversized }),
        ));
    }

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: the request must be refused at the top level, body {body:?}"
        );
        assert!(
            body["FailedRequests"].is_null(),
            "case {case}: a refused request reports no per-item failures, got {body:?}"
        );
    }
}
