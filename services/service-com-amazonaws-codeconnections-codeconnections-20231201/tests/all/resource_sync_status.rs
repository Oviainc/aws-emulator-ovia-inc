// SPDX-License-Identifier: CC0-1.0

//! `GetResourceSyncStatus` over the wire.
//!
//! The rule these suites assert: `GetResourceSyncStatus` resolves the
//! `(SyncType, ResourceName)` pair its input names against this service's store and
//! reports only sync state the service really recorded. A pair it has recorded no sync
//! state for — whether or not the resource has a sync configuration — SUCCEEDS with a
//! payload carrying none of `DesiredState`, `LatestSync` and `LatestSuccessfulSync`,
//! rather than being refused: a client polling before it has configured the resource
//! must be able to tell "never synced" from "bad request". Nothing in this emulator
//! performs a Git sync, so no attempt is ever recorded by the API alone and no status is
//! synthesised; and, being a report, the call leaves the store exactly as a later caller
//! finds it. `absent_resource_answers.rs` carries the other half of the rule: a sync
//! state seeded into the store IS reported, member for member.
//!
//! The table spans the class the rule generalises over: resource-name form and length
//! (each published bound of the modeled `ResourceName` `length 1..100`,
//! `pattern ^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`, from `smithy_query(query="operation",
//! operation="GetResourceSyncStatus")`), and whether the resource has a sync
//! configuration at all. `SyncType` has the single modeled value `CFN_STACK_SYNC`
//! (`smithy_query(query="shape", shape="SyncConfigurationType")`), so every row carries
//! it.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers (`router`, `call`, `create`), so these tests
//! exercise the same path a signed SDK request takes.

use smithy_server_runtime::MultiServiceRouter;

use super::connection_lifecycle::{call, create, router};
use super::git_sync::error_code;

/// The only value the modeled `SyncConfigurationType` enum accepts, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`.
const SYNC_TYPE: &str = "CFN_STACK_SYNC";

/// The published bounds of the modeled `ResourceName` member (`length 1..100`), from
/// `smithy_query(query="operation", operation="GetResourceSyncStatus")`.
const RESOURCE_NAME_MIN_LEN: usize = 1;
const RESOURCE_NAME_MAX_LEN: usize = 100;

/// One resource whose sync status a row asks for.
struct ResourceCase {
    /// What a failing assertion names this row by.
    label: &'static str,
    resource_name: String,
}

/// The table: resource names differing in length (each published bound of
/// `ResourceName` 1..100) and in form (mixed case, digits, the `_` and `-` the modeled
/// pattern admits after the first character) — the dimensions the rule generalises
/// over. None of these is the name the issue's recording cited.
fn resource_cases() -> Vec<ResourceCase> {
    vec![
        ResourceCase {
            label: "a plain mixed-case stack name",
            resource_name: "DemoStack".to_string(),
        },
        ResourceCase {
            label: "a name at the published minimum length",
            resource_name: "A".repeat(RESOURCE_NAME_MIN_LEN),
        },
        ResourceCase {
            label: "a name at the published maximum length",
            resource_name: "r".repeat(RESOURCE_NAME_MAX_LEN),
        },
        ResourceCase {
            label: "a name carrying the hyphen the pattern admits",
            resource_name: "Stack-1".to_string(),
        },
        ResourceCase {
            label: "a name carrying the underscore the pattern admits",
            resource_name: "stack_2".to_string(),
        },
        ResourceCase {
            label: "a name of digits and mixed case",
            resource_name: "MixedCase123".to_string(),
        },
    ]
}

/// A `RoleArn` matching the modeled `IamRoleArn` pattern
/// `^arn:aws(-[\w]+)*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$`.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/git-sync", engine::emulator_account_id())
}

/// The `GetResourceSyncStatus` body for one row.
fn status_body(resource_name: &str) -> serde_json::Value {
    serde_json::json!({ "SyncType": SYNC_TYPE, "ResourceName": resource_name })
}

/// Create a sync configuration for `resource_name` through the wire, so the resource is
/// one the store really holds, and return the `RepositoryLinkId` it was created against.
async fn configure_sync(
    router: &MultiServiceRouter,
    index: usize,
    resource_name: &str,
) -> String {
    let connection_arn = create(router, &format!("rss-conn-{index}"), "GitHub").await;

    let (status, body) = call(
        router,
        "CreateRepositoryLink",
        serde_json::json!({
            "ConnectionArn": connection_arn,
            "OwnerId": format!("owner-{index}"),
            "RepositoryName": format!("repo-{index}"),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_name:?}: expected 200 from CreateRepositoryLink, got {status} {body}"
    );
    let link_id = body["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {resource_name:?}: CreateRepositoryLink returned no id: {body}")
        })
        .to_string();

    let (status, body) = call(
        router,
        "CreateSyncConfiguration",
        serde_json::json!({
            "Branch": "main",
            "ConfigFile": "stack.yaml",
            "RepositoryLinkId": link_id,
            "ResourceName": resource_name,
            "RoleArn": role_arn(),
            "SyncType": SYNC_TYPE,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {resource_name:?}: expected 200 from CreateSyncConfiguration, got {status} {body}"
    );

    link_id
}

/// Assert one `GetResourceSyncStatus` answer is the empty success: HTTP 200, no error
/// code, and no synthesised sync state — no `LatestSync`, no `LatestSuccessfulSync`, no
/// `DesiredState`.
fn assert_no_sync_status(case: &str, status: u16, body: &serde_json::Value) {
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from GetResourceSyncStatus, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "",
        "case {case}: expected no error code, got {body}"
    );
    for member in ["LatestSync", "LatestSuccessfulSync", "DesiredState"] {
        assert!(
            body[member].is_null(),
            "case {case}: no {member} may be reported for a sync nothing ran, got {body}"
        );
    }
}

/// A resource with no sync configuration has no recorded sync state either, so the read
/// SUCCEEDS with an empty payload for every row — never the 400 that would make "never
/// synced" indistinguishable from "bad request", and never the unmodeled 501
/// `NotImplemented`.
#[tokio::test]
async fn an_unconfigured_resource_reports_an_empty_status() {
    let router = router();

    // A configuration that IS there, so "unconfigured" can never mean "the store is
    // empty".
    configure_sync(&router, 0, "LiveResource").await;

    for case in resource_cases() {
        let (status, body) = call(
            &router,
            "GetResourceSyncStatus",
            status_body(&case.resource_name),
        )
        .await;
        assert_no_sync_status(case.label, status, &body);
    }
}

/// A resource that IS configured for Git sync still reports no sync state: nothing in
/// this emulator fetches a remote repository's commits, so there is no revision, no
/// commit sha and no event list to report — and the answer is the same empty success as
/// for an unconfigured resource, not a synthesised status.
#[tokio::test]
async fn a_configured_resource_reports_no_attempt_because_nothing_syncs() {
    let router = router();

    for (index, case) in resource_cases().iter().enumerate() {
        configure_sync(&router, index, &case.resource_name).await;

        // The configuration is really there: the sibling reader finds it.
        let (status, configured) = call(
            &router,
            "GetSyncConfiguration",
            status_body(&case.resource_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from GetSyncConfiguration, got {status} {configured}",
            case.label
        );

        let (status, body) = call(
            &router,
            "GetResourceSyncStatus",
            status_body(&case.resource_name),
        )
        .await;
        assert_no_sync_status(case.label, status, &body);
    }
}

/// The report changes nothing a later caller sees: repeated reads answer identically and
/// the sync configuration the resource holds is untouched by them.
#[tokio::test]
async fn reading_the_sync_status_leaves_the_store_as_it_found_it() {
    let router = router();

    for (index, case) in resource_cases().iter().enumerate() {
        configure_sync(&router, index, &case.resource_name).await;

        let (_, before) = call(
            &router,
            "GetSyncConfiguration",
            status_body(&case.resource_name),
        )
        .await;

        let mut answers: Vec<(u16, serde_json::Value)> = Vec::new();
        for _ in 0..3 {
            answers.push(
                call(
                    &router,
                    "GetResourceSyncStatus",
                    status_body(&case.resource_name),
                )
                .await,
            );
        }
        let first = &answers[0];
        for (attempt, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer, first,
                "case {}: read {attempt} must answer exactly as the first did, got {:?}",
                case.label, answer
            );
        }

        let (status, after) = call(
            &router,
            "GetSyncConfiguration",
            status_body(&case.resource_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 from GetSyncConfiguration after the reads, got {status} {after}",
            case.label
        );
        assert_eq!(
            after, before,
            "case {}: reading the sync status must not change the stored configuration, got {after}",
            case.label
        );

        // And the reads left every EARLIER row alone too: each resource configured so
        // far still reports its own configuration.
        for earlier in resource_cases().iter().take(index + 1) {
            let (status, still) = call(
                &router,
                "GetSyncConfiguration",
                status_body(&earlier.resource_name),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {}: {} must still be configured after the reads, got {status} {still}",
                case.label, earlier.label
            );
        }
    }
}

/// The boundary of the modeled `ResourceName` constraint: a name one character past the
/// published maximum, and one breaking the published pattern, are refused with HTTP 400
/// `ValidationException` before the handler runs — so the empty 200 above is the
/// handler's answer for a well-formed name, not a blanket success.
#[tokio::test]
async fn a_resource_name_outside_the_modeled_constraint_is_refused() {
    let router = router();

    for (label, name) in [
        (
            "one character past the published maximum",
            "r".repeat(RESOURCE_NAME_MAX_LEN + 1),
        ),
        ("one character short of the published minimum", String::new()),
        (
            "a name starting with the underscore the pattern admits only later",
            "_leading".to_string(),
        ),
        ("a name carrying a character outside the pattern", "stack/one".to_string()),
    ] {
        let (status, body) = call(&router, "GetResourceSyncStatus", status_body(&name)).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from GetResourceSyncStatus, got {status} {body}"
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("ValidationException")),
            "case {label:?}: expected ValidationException, got {body}"
        );
    }
}
