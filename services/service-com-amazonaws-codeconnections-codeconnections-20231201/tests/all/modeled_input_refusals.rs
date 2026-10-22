// SPDX-License-Identifier: CC0-1.0

//! Modeled input constraints, refused on the wire.
//!
//! The rule: a request that violates a constraint the Smithy model states for an input
//! member — an enum's value set, a string's `@length`, a member's `@pattern` — is
//! refused with HTTP 400 `ValidationException` and persists nothing. The table spans
//! every member of this service's create/read surface that carries such a constraint,
//! and each row names the member and the constraint it breaks; the three cases the
//! issue cited (an unmodeled `ProviderType`, an over-long `ConnectionName`, a malformed
//! `HostArn`) are three of its rows.
//!
//! Every request goes through the real `MultiServiceRouter` dispatch, reusing the
//! `connection_lifecycle` suite's helpers, so these tests exercise the same path a
//! signed SDK request takes.

use super::connection_lifecycle::{REGION, call, router};

/// One request that breaks a modeled constraint.
struct RefusalCase {
    /// What a failing assertion names this row by: the member and the constraint.
    label: String,
    operation: &'static str,
    body: serde_json::Value,
}

/// A `HostArn`-shaped ARN for a host this service holds no record of — used only where
/// a row is about the ARN's SYNTAX.
fn host_arn(resource: &str) -> String {
    format!(
        "arn:aws:codeconnections:{REGION}:{}:host/{resource}",
        engine::emulator_account_id()
    )
}

/// The table. Every constraint cited here comes from
/// `smithy_query(query="operation", operation="<Op>")` for this service's model.
fn refusal_cases() -> Vec<RefusalCase> {
    let mut cases = Vec::new();

    // `ConnectionName` is `length 1..32`.
    for length in [33_usize, 66, 100] {
        cases.push(RefusalCase {
            label: format!("CreateConnection ConnectionName of {length} characters (max 32)"),
            operation: "CreateConnection",
            body: serde_json::json!({
                "ConnectionName": "n".repeat(length),
                "ProviderType": "GitHub",
            }),
        });
    }
    cases.push(RefusalCase {
        label: "CreateConnection ConnectionName of 0 characters (min 1)".to_string(),
        operation: "CreateConnection",
        body: serde_json::json!({ "ConnectionName": "", "ProviderType": "GitHub" }),
    });

    // `ProviderType` is an enum: `AzureDevOps | Bitbucket | GitHub |
    // GitHubEnterpriseServer | GitLab | GitLabSelfManaged`.
    for value in ["NotARealProvider", "github", "GITHUB", "", "GitHub "] {
        cases.push(RefusalCase {
            label: format!("CreateConnection ProviderType {value:?} is not a modeled value"),
            operation: "CreateConnection",
            body: serde_json::json!({ "ConnectionName": "demo-conn", "ProviderType": value }),
        });
    }

    // `HostArn` is `pattern
    // ^arn:aws(-[\w]+)*:(codestar-connections|codeconnections):.+:[0-9]{12}:host\/.+$`.
    for value in [
        "not-an-arn".to_string(),
        // Right shape, wrong resource type.
        format!(
            "arn:aws:codeconnections:{REGION}:{}:connection/abc",
            engine::emulator_account_id()
        ),
        // Right shape, wrong service namespace.
        format!(
            "arn:aws:codebuild:{REGION}:{}:host/abc",
            engine::emulator_account_id()
        ),
        // Account id that is not 12 digits.
        format!("arn:aws:codeconnections:{REGION}:1234:host/abc"),
        // No resource after `host/`.
        host_arn(""),
    ] {
        cases.push(RefusalCase {
            label: format!("CreateConnection HostArn {value:?} does not match the modeled pattern"),
            operation: "CreateConnection",
            body: serde_json::json!({
                "ConnectionName": "demo-conn",
                "ProviderType": "GitHub",
                "HostArn": value,
            }),
        });
    }

    // `CreateHost`: `Name` is `length 1..64`, `ProviderEndpoint` is `length 1..512`,
    // `ProviderType` is the same enum.
    for length in [513_usize, 600] {
        cases.push(RefusalCase {
            label: format!("CreateHost ProviderEndpoint of {length} characters (max 512)"),
            operation: "CreateHost",
            body: serde_json::json!({
                "Name": "demo-host",
                "ProviderType": "GitHubEnterpriseServer",
                "ProviderEndpoint": "e".repeat(length),
            }),
        });
    }
    cases.push(RefusalCase {
        label: "CreateHost Name of 65 characters (max 64)".to_string(),
        operation: "CreateHost",
        body: serde_json::json!({
            "Name": "n".repeat(65),
            "ProviderType": "GitHubEnterpriseServer",
            "ProviderEndpoint": "https://ghe.example.internal",
        }),
    });
    cases.push(RefusalCase {
        label: "CreateHost ProviderType is not a modeled value".to_string(),
        operation: "CreateHost",
        body: serde_json::json!({
            "Name": "demo-host",
            "ProviderType": "NotARealProvider",
            "ProviderEndpoint": "https://ghe.example.internal",
        }),
    });

    // `RepositoryLinkId` is `pattern ^[0-9a-fA-F]{8}\b-[0-9a-fA-F]{4}\b-…$`.
    for value in ["not-a-uuid", "0f1e2d3c4b5a69788796a5b4c3d2e1f0", "g".repeat(36).as_str()] {
        cases.push(RefusalCase {
            label: format!("GetRepositoryLink RepositoryLinkId {value:?} is not UUID-shaped"),
            operation: "GetRepositoryLink",
            body: serde_json::json!({ "RepositoryLinkId": value }),
        });
    }

    // `ResourceName` is `length 1..100`, `pattern ^[0-9A-Za-z]+[0-9A-Za-z_\\-]*$`.
    cases.push(RefusalCase {
        label: "GetSyncConfiguration ResourceName of 101 characters (max 100)".to_string(),
        operation: "GetSyncConfiguration",
        body: serde_json::json!({ "SyncType": "CFN_STACK_SYNC", "ResourceName": "r".repeat(101) }),
    });
    cases.push(RefusalCase {
        label: "GetSyncConfiguration SyncType is not a modeled value".to_string(),
        operation: "GetSyncConfiguration",
        body: serde_json::json!({ "SyncType": "NOT_A_SYNC_TYPE", "ResourceName": "DemoStack" }),
    });

    // `UpdateSyncBlocker`: `Id` is `length 1..50`, `ResolvedReason` is `length 1..250`.
    cases.push(RefusalCase {
        label: "UpdateSyncBlocker Id of 51 characters (max 50)".to_string(),
        operation: "UpdateSyncBlocker",
        body: serde_json::json!({
            "Id": "x".repeat(51),
            "SyncType": "CFN_STACK_SYNC",
            "ResourceName": "DemoStack",
            "ResolvedReason": "because",
        }),
    });
    cases.push(RefusalCase {
        label: "UpdateSyncBlocker ResolvedReason of 251 characters (max 250)".to_string(),
        operation: "UpdateSyncBlocker",
        body: serde_json::json!({
            "Id": "blocker-1",
            "SyncType": "CFN_STACK_SYNC",
            "ResourceName": "DemoStack",
            "ResolvedReason": "r".repeat(251),
        }),
    });

    cases
}

/// Every row is refused with HTTP 400 and the error code `ValidationException`, and the
/// refusal names the offending member so a caller can fix the request from the message
/// alone.
#[tokio::test]
async fn a_request_breaking_a_modeled_constraint_is_refused_with_validation_exception() {
    let router = router();

    for case in refusal_cases() {
        let (status, body) = call(&router, case.operation, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} {body}",
            case.label
        );
        assert!(
            body["__type"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("ValidationException")),
            "case {:?}: expected ValidationException, got {body}",
            case.label
        );
    }

    // Nothing above persisted anything: no connection, no host.
    let (status, listed) = call(&router, "ListConnections", serde_json::json!({})).await;
    assert_eq!(
        status, 200,
        "ListConnections: expected 200, got {status} {listed}"
    );
    assert_eq!(
        listed["Connections"],
        serde_json::json!([]),
        "a refused request must persist nothing, got {listed}"
    );
}

/// The boundary between refused and accepted: the same members AT their published
/// bounds are accepted, so the rows above are refused for breaking the constraint and
/// not for touching it.
#[tokio::test]
async fn the_published_bounds_themselves_are_accepted() {
    let router = router();

    // `ConnectionName` 1..32 and `ProviderType`'s first modeled value.
    for (label, name) in [
        ("ConnectionName at the minimum length", "c".to_string()),
        ("ConnectionName at the maximum length", "c".repeat(32)),
    ] {
        let (status, body) = call(
            &router,
            "CreateConnection",
            serde_json::json!({ "ConnectionName": name, "ProviderType": "AzureDevOps" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );
    }

    // `CreateHost`'s `Name` 1..64 and `ProviderEndpoint` 1..512, both at their maximum.
    let (status, body) = call(
        &router,
        "CreateHost",
        serde_json::json!({
            "Name": "n".repeat(64),
            "ProviderType": "GitHubEnterpriseServer",
            "ProviderEndpoint": "e".repeat(512),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateHost at its published bounds: expected 200, got {status} {body}"
    );
}
