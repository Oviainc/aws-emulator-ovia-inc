// SPDX-License-Identifier: CC0-1.0

//! The workspace a logging scope belongs to, for the two operations that address
//! one: `DescribeLoggingOptions` (`GET /logging?workspaceName=…`) and
//! `PutLoggingOptions` (`PUT /logging`).
//!
//! The rule: an operation whose request names a `workspaceName` resolves that
//! workspace through the workspace registry BEFORE the setting scoped to it, so a
//! request naming a workspace this service does not hold is refused with the
//! modelled `ResourceNotFoundException` (HTTP 404) stating the resource KIND and
//! the identifier the request supplied, and neither reports nor stores a setting
//! for it. A request naming NO workspace addresses the account-and-region-wide
//! scope, which belongs to no workspace and is always served.
//!
//! The rows span the whole class rather than the one request an issue recorded:
//! both operations, workspace names spanning the modelled `WorkspaceName` form,
//! a workspace that is held, one that was never opened, and one deleted since —
//! and, for the read, a name whose scope the store was seeded with directly, so
//! the refusal cannot be passing for "nothing configured".
//!
//! The expected message is written out BY HAND as an independent oracle: this
//! suite never calls the crate's message helpers, so a handler that drifts to a
//! template of its own cannot agree with itself.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use std::time::SystemTime;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    LoggingOptions, LOGGING_LEVEL_VALUES,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, DEFAULT_LOGGING_LEVEL};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::logging_options::{describe, put_level, reported_level, scope_label};
use crate::support::ensure_workspace;

/// The resource KIND this service words a workspace absence with, as the wire
/// reports it.
const WORKSPACE_KIND: &str = "Workspace";

/// Workspace names no `CreateWorkspace` of this suite opens, each inside the
/// modelled `WorkspaceName` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`) so the
/// request reaches the handler at all: the shortest name the length admits, a
/// mixed-case name with both punctuation characters the pattern allows, a
/// digits-only name, and one at the published maximum length.
///
/// Bounds from `smithy_query(query="operation", operation="DescribeLoggingOptions")`,
/// whose `workspaceName` targets `WorkspaceName`.
const UNHELD_WORKSPACES: [&str; 4] = [
    "g",
    "Ghost_9-east",
    "404",
    "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
];

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// `DELETE /workspaces/{name}`.
async fn delete_workspace(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/workspaces/{name}"), None).await
}

/// Assert `answer` is the modelled refusal for the absent workspace `workspace`.
fn assert_workspace_absent(label: &str, workspace: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 404,
        "{label} on the unheld workspace {workspace:?}: a request naming a workspace this \
         service does not hold is refused, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "{label} on the unheld workspace {workspace:?}: unexpected error code in {answer}"
    );
    assert_eq!(
        error_message(answer),
        absent(WORKSPACE_KIND, workspace),
        "{label} on the unheld workspace {workspace:?}: the refusal must name the WORKSPACE as \
         the absent resource, got {answer}"
    );
}

/// Both operations refuse a `workspaceName` no workspace holds, over every name
/// form the model admits, while the account-wide scope and a workspace that IS
/// held keep answering.
#[tokio::test]
async fn an_unheld_workspace_is_refused_on_both_operations() {
    let (router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the iotsitewise store");

    // One workspace that IS held, so a refusal below can be wrong only about the
    // workspace it names and not about the route or the setting.
    let held = "Plant_1-east";
    ensure_workspace(&router, held).await;
    let (status, body) = put_level(&router, Some(held), "ERROR").await;
    assert_eq!(status, 200, "put ERROR on the held workspace: {status} {body}");
    let (status, body) = put_level(&router, None, "OFF").await;
    assert_eq!(status, 200, "put OFF on the account-wide scope: {status} {body}");

    let configured_before = state.lock().logging_options.len();

    for workspace in UNHELD_WORKSPACES {
        // The read is refused for the workspace, not answered with a level.
        let (status, answer) = describe(&router, Some(workspace)).await;
        assert_workspace_absent("DescribeLoggingOptions", workspace, status, &answer);

        // So is the write, for every level the model carries — and it stores
        // nothing, so no scope is left behind for a workspace nothing holds.
        for level in LOGGING_LEVEL_VALUES {
            let (status, answer) = put_level(&router, Some(workspace), level).await;
            assert_workspace_absent(
                &format!("PutLoggingOptions {level}"),
                workspace,
                status,
                &answer,
            );
        }
        assert!(
            !state.lock().logging_options.contains_key(workspace),
            "the refused calls left a logging scope stored for the unheld workspace \
             {workspace:?}"
        );
    }

    let configured_after = state.lock().logging_options.len();
    assert_eq!(
        configured_after, configured_before,
        "the refused calls changed the number of stored logging scopes from \
         {configured_before} to {configured_after}"
    );

    // The scopes that DO exist still answer with what the puts stored.
    for (scope, level) in [(Some(held), "ERROR"), (None, "OFF")] {
        let (status, body) = describe(&router, scope).await;
        assert_eq!(
            status,
            200,
            "describe {} after the refusals: got {status} {body}",
            scope_label(scope)
        );
        assert_eq!(
            reported_level(&body),
            level,
            "describe {} after the refusals: expected {level}, got {body}",
            scope_label(scope)
        );
    }
}

/// A scope the STORE carries for a workspace the registry does not hold is still
/// refused: the read resolves the workspace, so it cannot be passing for "nothing
/// configured".
#[tokio::test]
async fn a_stored_scope_without_its_workspace_is_still_refused() {
    let (router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the iotsitewise store");

    // Seeded straight into the store — the state a workspace-less scope would be,
    // at a level no unconfigured scope reports, so an answer of any kind would be
    // visibly the stored one.
    let orphan = "orphaned-scope";
    let level = LOGGING_LEVEL_VALUES
        .iter()
        .find(|candidate| **candidate != DEFAULT_LOGGING_LEVEL)
        .expect("a modelled level other than the unconfigured default");
    state.lock().set_logging_options(
        Some(orphan),
        LoggingOptions {
            level: (*level).to_string(),
        },
        SystemTime::now(),
    );

    let (status, answer) = describe(&router, Some(orphan)).await;
    assert_workspace_absent("DescribeLoggingOptions", orphan, status, &answer);

    let (status, answer) = put_level(&router, Some(orphan), level).await;
    assert_workspace_absent("PutLoggingOptions", orphan, status, &answer);
}

/// A workspace deleted since loses its scope with it: what the put configured
/// while the workspace was held is refused once it is gone, and the account-wide
/// scope — which belongs to no workspace — is untouched by the delete.
#[tokio::test]
async fn a_deleted_workspace_takes_its_scope_with_it() {
    let router = router();

    let (status, body) = put_level(&router, None, "INFO").await;
    assert_eq!(status, 200, "seed the account-wide scope: {status} {body}");

    // The rows differ along the name form and the level configured, so the case
    // does not turn on either.
    let longest = "d".repeat(64);
    let cases: [(&str, &str); 3] = [("d", "ERROR"), ("Doomed_1-east", "OFF"), (&longest, "INFO")];

    for (workspace, level) in cases {
        ensure_workspace(&router, workspace).await;

        let (status, body) = put_level(&router, Some(workspace), level).await;
        assert_eq!(
            status, 200,
            "put {level} on the held workspace {workspace}: got {status} {body}"
        );
        let (status, body) = describe(&router, Some(workspace)).await;
        assert_eq!(
            status, 200,
            "describe the held workspace {workspace}: got {status} {body}"
        );
        assert_eq!(
            reported_level(&body),
            level,
            "describe the held workspace {workspace} after putting {level}: got {body}"
        );

        let (status, body) = delete_workspace(&router, workspace).await;
        assert_eq!(
            status, 200,
            "delete the workspace {workspace}: got {status} {body}"
        );

        let (status, answer) = describe(&router, Some(workspace)).await;
        assert_workspace_absent("DescribeLoggingOptions", workspace, status, &answer);
        let (status, answer) = put_level(&router, Some(workspace), level).await;
        assert_workspace_absent("PutLoggingOptions", workspace, status, &answer);
    }

    let (status, body) = describe(&router, None).await;
    assert_eq!(status, 200, "describe the account-wide scope: {status} {body}");
    assert_eq!(
        reported_level(&body),
        "INFO",
        "the account-wide scope after every workspace was deleted: got {body}"
    );
}

/// A workspace that is held reports and takes its own setting, and a newly opened
/// one starts at the unconfigured default: resolving the workspace does not stand
/// in the way of the scopes that exist.
#[tokio::test]
async fn a_held_workspace_reports_and_takes_its_own_setting() {
    let router = router();

    // The rows span the modelled `WorkspaceName` form: the shortest name, a
    // mixed-case name with both allowed punctuation characters, a digits-only
    // name and one at the published maximum length.
    let longest = "h".repeat(64);
    let workspaces: [&str; 4] = ["h", "Held_1-west", "2026", &longest];

    for workspace in workspaces {
        ensure_workspace(&router, workspace).await;

        let (status, body) = describe(&router, Some(workspace)).await;
        assert_eq!(
            status, 200,
            "describe the freshly opened workspace {workspace}: got {status} {body}"
        );
        assert_eq!(
            reported_level(&body),
            DEFAULT_LOGGING_LEVEL,
            "a freshly opened workspace {workspace} must report the unconfigured default \
             {DEFAULT_LOGGING_LEVEL}: got {body}"
        );

        for level in LOGGING_LEVEL_VALUES {
            let (status, body) = put_level(&router, Some(workspace), level).await;
            assert_eq!(
                status, 200,
                "put {level} on the held workspace {workspace}: got {status} {body}"
            );
            let (status, body) = describe(&router, Some(workspace)).await;
            assert_eq!(
                status, 200,
                "describe the held workspace {workspace} after putting {level}: got {status} \
                 {body}"
            );
            assert_eq!(
                reported_level(&body),
                *level,
                "describe the held workspace {workspace} after putting {level}: got {body}"
            );
        }
    }
}
