// SPDX-License-Identifier: CC0-1.0

//! The `applicationName` / `deploymentGroupName` PAIR `ListDeployments` filters by,
//! driven over the wire.
//!
//! RULE under test: the two members are conditionally required TOGETHER — "If
//! `applicationName` is specified, then `deploymentGroupName` must be specified. If
//! it is not specified, then `deploymentGroupName` must not be specified"
//! (`smithy_query(query="doc",shape="ListDeploymentsInput",member="applicationName")`)
//! — so a request naming one without the other is refused through the member it is
//! MISSING, with that member's own modelled `…RequiredException` (HTTP 400), and a
//! request naming both or neither is served.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, s3_revision, Emulator};

/// The service role the groups here are created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// Create an application with two deployment groups and one deployment on the
/// first, answering `(application, first group, second group, deployment id)`.
async fn seeded(emulator: &Emulator) -> (String, String, String, String) {
    let application = "listing-pairs".to_string();
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Server"}),
        )
        .await;
    let first = "pairs-one".to_string();
    let second = "pairs-two".to_string();
    for group in [&first, &second] {
        emulator
            .ok(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": group,
                    "serviceRoleArn": ROLE,
                }),
            )
            .await;
    }
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": first,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;
    let deployment = created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (application, first, second, deployment)
}

/// The deployment ids a listing answers.
fn listed(body: &Value) -> Vec<String> {
    body["deployments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// RULE: one of the pair without the other is refused through the missing member.
///
/// Rows differ along which half the request carries and what else it carries with
/// it: the application alone, the group alone, and each of them beside another
/// filter the operation accepts — the refusal is about the PAIR, not about the rest
/// of the request.
#[tokio::test]
async fn one_of_the_filter_pair_without_the_other_is_refused() {
    let emulator = Emulator::new();
    let (application, first, _second, _deployment) = seeded(&emulator).await;

    // (case, the request, the modelled error code, the sentence it carries)
    let cases: &[(&str, Value, &str, &str)] = &[
        (
            "the application alone",
            json!({"applicationName": application}),
            "DeploymentGroupNameRequiredException",
            "Deployment Group name is missing",
        ),
        (
            "the application beside a status filter",
            json!({"applicationName": application, "includeOnlyStatuses": ["Created"]}),
            "DeploymentGroupNameRequiredException",
            "Deployment Group name is missing",
        ),
        (
            "the application beside a creation window",
            json!({
                "applicationName": application,
                "createTimeRange": {"start": 1.0, "end": 2.0},
            }),
            "DeploymentGroupNameRequiredException",
            "Deployment Group name is missing",
        ),
        (
            // The pair is checked before the application is resolved, so an
            // application the account does not hold earns the same refusal.
            "an unknown application alone",
            json!({"applicationName": "no-such-application"}),
            "DeploymentGroupNameRequiredException",
            "Deployment Group name is missing",
        ),
        (
            "the group alone",
            json!({"deploymentGroupName": first}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "the group beside an external id",
            json!({"deploymentGroupName": first, "externalId": "stack/x"}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
        (
            "an unknown group alone",
            json!({"deploymentGroupName": "no-such-group"}),
            "ApplicationNameRequiredException",
            "Application name is missing",
        ),
    ];

    for (case, request, expected_code, expected_message) in cases {
        let (status, body) = emulator.call("ListDeployments", request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} with {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(*expected_message),
            "case {case:?}: expected the {expected_code} sentence, got {body:?}"
        );
        assert!(
            body["deployments"].is_null(),
            "case {case:?}: a refusal carries no deployments, got {body:?}"
        );
    }
}

/// RULE: a request naming BOTH halves, or NEITHER, is served — and the ids it
/// answers are the deployments the filter selects.
///
/// Rows differ along which deployments the filter selects: every stored deployment,
/// the one group that holds one, the group that holds none, and a group of an
/// application that does not exist (refused on the resource, not on the pair).
#[tokio::test]
async fn both_halves_or_neither_are_served() {
    let emulator = Emulator::new();
    let (application, first, second, deployment) = seeded(&emulator).await;

    // (case, the request, the ids it must answer)
    let served: &[(&str, Value, Vec<String>)] = &[
        ("neither half", json!({}), vec![deployment.clone()]),
        (
            "both halves, on the group holding the deployment",
            json!({"applicationName": application, "deploymentGroupName": first}),
            vec![deployment.clone()],
        ),
        (
            "both halves, on the group holding none",
            json!({"applicationName": application, "deploymentGroupName": second}),
            Vec::new(),
        ),
        (
            "neither half, beside a status nothing holds",
            json!({"includeOnlyStatuses": ["Stopped"]}),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in served {
        let body = emulator.ok("ListDeployments", request.clone()).await;
        assert_eq!(
            &listed(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body:?}"
        );
    }

    // Both halves naming resources the account does not hold are refused on the
    // RESOURCE, which is what the pair rule leaves to the lookup.
    for (case, request, expected_code) in [
        (
            "both halves, unknown application",
            json!({"applicationName": "no-such-application", "deploymentGroupName": first}),
            "ApplicationDoesNotExistException",
        ),
        (
            "both halves, unknown group",
            json!({"applicationName": application, "deploymentGroupName": "no-such-group"}),
            "DeploymentGroupDoesNotExistException",
        ),
    ] {
        let (status, body) = emulator.call("ListDeployments", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} with {body:?}"
        );
    }
}
