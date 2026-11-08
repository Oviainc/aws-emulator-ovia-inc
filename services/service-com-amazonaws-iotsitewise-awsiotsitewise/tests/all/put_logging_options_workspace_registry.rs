// SPDX-License-Identifier: CC0-1.0

//! What `PutLoggingOptions` does with the `workspaceName` it carries, pinned from
//! the WRITE side: the registry is the only thing that decides whether a named
//! scope can be configured.
//!
//! The rule: an operation whose request names a `workspaceName` resolves that
//! name against the workspace registry — exactly, with no normalisation — before
//! the setting scoped to it, so a put naming a workspace this service does not
//! hold is refused with the modelled `ResourceNotFoundException` (HTTP 404)
//! naming the workspace, and the refused call writes nothing at all: neither the
//! logging scope it would have set nor the workspace it named. A request naming
//! NO workspace addresses the account-and-region-wide scope, which belongs to no
//! workspace and is always served.
//!
//! Where the sibling `logging_options_workspace_scope` suite spans the name forms
//! the model admits, this one spans what "resolves" means: the registry decides
//! (so the very same request flips from refused to served the moment the
//! workspace is opened, and back when it is deleted), it decides on the exact
//! name (so a near miss in case or spelling is an absence), and a refusal is
//! inert (so no workspace and no scope appears behind it).
//!
//! The expected refusal is written out BY HAND as an independent oracle: this
//! suite never calls the crate's message helpers.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::LOGGING_LEVEL_VALUES;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::logging_options::{describe, put_level, reported_level};
use crate::support::ensure_workspace;

/// The resource KIND this service words a workspace absence with, as the wire
/// reports it.
const WORKSPACE_KIND: &str = "Workspace";

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// `GET /workspaces/{name}` — `DescribeWorkspace`, the registry's own reader.
async fn describe_workspace(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &format!("/workspaces/{name}"), None).await
}

/// `DELETE /workspaces/{name}`.
async fn delete_workspace(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/workspaces/{name}"), None).await
}

/// The names `GET /workspaces` lists.
async fn listed_workspaces(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", "/workspaces", None).await;
    assert_eq!(status, 200, "ListWorkspaces: got {status} {body}");
    body["workspaceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("workspaceSummaries must be a list, got {body}"))
        .iter()
        .map(|summary| summary["name"].as_str().unwrap_or("").to_string())
        .collect()
}

/// Assert `answer` is the modelled refusal for the absent workspace `workspace`.
fn assert_workspace_absent(label: &str, workspace: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, 404,
        "case {label:?}: a put naming the workspace {workspace:?}, which this service does not \
         hold, is refused, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "case {label:?}: unexpected error code for the unheld workspace {workspace:?} in {answer}"
    );
    assert_eq!(
        error_message(answer),
        absent(WORKSPACE_KIND, workspace),
        "case {label:?}: the refusal must name the WORKSPACE as the absent resource, got {answer}"
    );
}

/// A put the registry refuses writes NOTHING: the workspace it named is still
/// absent from `DescribeWorkspace` and from `ListWorkspaces`, and the scope it
/// would have configured is not in the store.
///
/// The rows span the levels the model carries and the `WorkspaceName` forms the
/// model admits (len 1..64, pattern `^[a-zA-Z0-9_-]+$`, from
/// `smithy_query(query="operation", operation="PutLoggingOptions")`): the
/// shortest name, a mixed-case name using both punctuation characters the pattern
/// allows, a digits-only name and one at the published maximum length.
#[tokio::test]
async fn a_refused_put_opens_no_workspace_and_stores_no_scope() {
    let (router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the iotsitewise store");

    // One workspace that IS held, so the registry below is not simply empty.
    let held = "Registry_1-west";
    ensure_workspace(&router, held).await;
    let listed_before = listed_workspaces(&router).await;
    let scopes_before = state.lock().logging_options.len();

    let longest = "n".repeat(64);
    let unheld: [&str; 4] = ["n", "No_such-workspace", "0", &longest];

    for workspace in unheld {
        for level in LOGGING_LEVEL_VALUES {
            let (status, answer) = put_level(&router, Some(workspace), level).await;
            assert_workspace_absent(&format!("put {level}"), workspace, status, &answer);
        }

        // The refused put did not open the workspace it named, either: the
        // registry's own reader still reports the absence.
        let (status, answer) = describe_workspace(&router, workspace).await;
        assert_eq!(
            status, 404,
            "case {workspace:?}: DescribeWorkspace after the refused puts: expected the \
             workspace to still be absent, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {workspace:?}: DescribeWorkspace after the refused puts: got {answer}"
        );

        assert!(
            !state.lock().logging_options.contains_key(workspace),
            "case {workspace:?}: the refused puts left a logging scope stored for a workspace \
             nothing holds"
        );
    }

    let listed_after = listed_workspaces(&router).await;
    assert_eq!(
        listed_after, listed_before,
        "the refused puts changed the workspace registry: expected {listed_before:?}, got \
         {listed_after:?}"
    );
    let scopes_after = state.lock().logging_options.len();
    assert_eq!(
        scopes_after, scopes_before,
        "the refused puts changed the number of stored logging scopes from {scopes_before} to \
         {scopes_after}"
    );
}

/// The registry is what decides: the very same put is refused before the
/// workspace is opened, served while it is held — reading back through
/// `DescribeLoggingOptions` — and refused again once it is deleted. The
/// account-wide scope, which belongs to no workspace, answers throughout.
#[tokio::test]
async fn the_registry_decides_whether_a_named_scope_can_be_configured() {
    let router = router();

    let (status, body) = put_level(&router, None, "INFO").await;
    assert_eq!(
        status, 200,
        "put INFO on the account-wide scope: got {status} {body}"
    );

    // The rows differ along the name form and the level configured, so neither
    // is what the case turns on.
    let longest = "p".repeat(64);
    let cases: [(&str, &str); 4] = [
        ("p", "ERROR"),
        ("Phase_2-north", "INFO"),
        ("2026", "OFF"),
        (&longest, "ERROR"),
    ];

    for (workspace, level) in cases {
        let (status, answer) = put_level(&router, Some(workspace), level).await;
        let label = format!("before create, put {level}");
        assert_workspace_absent(&label, workspace, status, &answer);

        ensure_workspace(&router, workspace).await;

        let (status, body) = put_level(&router, Some(workspace), level).await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: put {level} once the workspace is held: got {status} {body}"
        );
        let (status, body) = describe(&router, Some(workspace)).await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: describe the held workspace: got {status} {body}"
        );
        assert_eq!(
            reported_level(&body),
            level,
            "case {workspace:?}: the setting the put stored must read back, got {body}"
        );

        let (status, body) = delete_workspace(&router, workspace).await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: delete the workspace: got {status} {body}"
        );

        let (status, answer) = put_level(&router, Some(workspace), level).await;
        let label = format!("after delete, put {level}");
        assert_workspace_absent(&label, workspace, status, &answer);
    }

    let (status, body) = describe(&router, None).await;
    assert_eq!(
        status, 200,
        "describe the account-wide scope at the end: got {status} {body}"
    );
    assert_eq!(
        reported_level(&body),
        "INFO",
        "the account-wide scope belongs to no workspace, so the refusals and deletes above leave \
         it as the put set it, got {body}"
    );
}

/// The name is resolved EXACTLY: `WorkspaceName` admits both cases and both
/// punctuation characters, and nothing normalises them away, so a near miss of a
/// held workspace is an absence like any other and configures no scope of its
/// own.
#[tokio::test]
async fn a_near_miss_of_a_held_workspace_is_an_absence() {
    let (router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the iotsitewise store");

    let held = "Plant_1-east";
    ensure_workspace(&router, held).await;
    let (status, body) = put_level(&router, Some(held), "ERROR").await;
    assert_eq!(
        status, 200,
        "put ERROR on the held workspace {held}: got {status} {body}"
    );

    // Each row differs from the held name in one way the registry must NOT
    // forgive: letter case (both directions), the punctuation character used, a
    // truncation, and an extra character.
    let near_misses: [(&str, &str); 6] = [
        ("lowercased", "plant_1-east"),
        ("uppercased", "PLANT_1-EAST"),
        ("hyphen for underscore", "Plant-1-east"),
        ("underscore for hyphen", "Plant_1_east"),
        ("truncated", "Plant_1-eas"),
        ("extended", "Plant_1-east2"),
    ];

    for (label, workspace) in near_misses {
        let (status, answer) = put_level(&router, Some(workspace), "OFF").await;
        assert_workspace_absent(label, workspace, status, &answer);
        assert!(
            !state.lock().logging_options.contains_key(workspace),
            "case {label:?}: the refused put stored a scope for {workspace:?}, which no workspace \
             holds"
        );
    }

    // The workspace that IS held keeps the setting its own put stored.
    let (status, body) = describe(&router, Some(held)).await;
    assert_eq!(
        status, 200,
        "describe the held workspace {held} after the near misses: got {status} {body}"
    );
    assert_eq!(
        reported_level(&body),
        "ERROR",
        "the near misses must not touch the setting of {held}, got {body}"
    );
}
