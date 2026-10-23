// SPDX-License-Identifier: CC0-1.0

//! `ResourceNotFoundException` identifies the resource it could not find by that
//! resource's ARN.
//!
//! The rule under test: when this service refuses a request because a resource the
//! request named is not held, the error's `resourceId` is that resource's Amazon Resource
//! Name — partition, service namespace, the requesting credential scope's region, the
//! account and the resource's path under its parents — not the bare path segment the
//! request named it by. `resourceType` names the kind, the `message` names the kind that
//! was missing without interpolating the caller's identifier, and the status stays 404.
//!
//! The table below varies what the rule generalises over: which operation refuses
//! (read, delete, and the three creates that resolve a parent), WHICH resource in the
//! chain is missing (workflow definition, workflow run, session), the signing region the
//! ARN is built in, and the form of the name — one character, the modelled maximum of 40,
//! and a name mixing the underscores, hyphens and digits
//! `^[a-zA-Z0-9_-]{1,40}$` allows. The recorded case (a missing definition read in
//! `us-east-1`) is one row of it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::state::DELETION_COMPLETION_DELAY;

use crate::workflow_lifecycle::{text, Emulator};

/// The definition every case resolves its "present" parent through.
const PRESENT: &str = "present";

/// A well-formed `UuidString` no record was ever minted for.
const ABSENT_UUID: &str = "00000000-0000-4000-8000-000000000000";

/// `WorkflowDefinitionName` is `len 1..40, pattern ^[a-zA-Z0-9_-]{1,40}$`
/// (smithy_query shape="WorkflowDefinitionName"), so this is a name AT the upper bound.
const MAX_LENGTH_NAME: &str = "max-length-definition-name-0123456789_ab";

/// The ARN this emulator gives the resource at `resource_path` in `region`, spelled out
/// here rather than taken from the crate's own helper so the case pins the WIRE form:
/// `arn:aws:nova-act:<region>:<account>:<resource path>`.
fn expected_arn(region: &str, resource_path: &str) -> String {
    format!(
        "arn:aws:nova-act:{region}:{}:{resource_path}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One refusal: the call that makes it, and the resource it must name.
struct NotFoundCase {
    label: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// What `resourceType` must report.
    resource_type: &'static str,
    /// The resource part of the ARN `resourceId` must carry.
    resource_path: String,
}

/// Assert one 404 body against the rule.
fn assert_identified_by_arn(
    label: &str,
    region: &str,
    status: u16,
    answer: &Value,
    resource_type: &str,
    resource_path: &str,
) {
    assert_eq!(
        status, 404,
        "case {label:?}: expected HTTP 404, got {status} {answer}"
    );
    assert_eq!(
        answer["__type"], "ResourceNotFoundException",
        "case {label:?}: expected ResourceNotFoundException, got {answer}"
    );
    assert_eq!(
        answer["resourceType"], resource_type,
        "case {label:?}: expected resourceType {resource_type:?}, got {answer}"
    );
    let expected = expected_arn(region, resource_path);
    let actual = answer["resourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: resourceId must be a string, got {answer}"));
    assert_eq!(
        actual, expected,
        "case {label:?}: expected resourceId {expected:?}, got {actual:?}"
    );

    // The identifier is parseable as an ARN: a client that splits it gets this service's
    // partition, namespace, the request's region and the emulator's account back.
    let segments: Vec<&str> = actual.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {label:?}: an ARN has six colon-separated segments, got {actual:?}"
    );
    assert_eq!(
        [segments[0], segments[1], segments[2], segments[3], segments[4]],
        ["arn", "aws", "nova-act", region, engine::EMULATOR_ACCOUNT_ID],
        "case {label:?}: ARN prefix must name this service, region and account, got {actual:?}"
    );

    // `message` is `NonBlankString` (`^[\s\S]+$`): non-blank, naming the KIND that was
    // missing and interpolating nothing from the request — the identifier the caller
    // asked for is what `resourceId` above carries.
    let message = answer["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: message must be a string, got {answer}"));
    assert!(
        !message.trim().is_empty(),
        "case {label:?}: message must be non-blank, got {message:?}"
    );
    let identifier = resource_path
        .rsplit('/')
        .next()
        .expect("a resource path has a last segment");
    assert!(
        !message.contains(identifier),
        "case {label:?}: message must not interpolate the request's identifier \
         {identifier:?}, got {message:?}"
    );
    let kind = resource_type.replace('-', " ");
    assert!(
        message.to_lowercase().contains(&kind),
        "case {label:?}: message must name the missing kind {kind:?}, got {message:?}"
    );
    // The same sentence for every kind in the chain, derived from the kind itself: the
    // recorded definition case reads "Workflow definition not found", so a missing run
    // reads "Workflow run not found" and a missing session "Session not found".
    let expected_message = {
        let mut characters = kind.chars();
        let first = characters.next().expect("a non-empty resource type");
        format!("{}{} not found", first.to_uppercase(), characters.as_str())
    };
    assert_eq!(
        message, expected_message,
        "case {label:?}: expected message {expected_message:?}, got {message:?}"
    );
}

/// Every operation that resolves a resource of this chain reports a missing one by ARN.
#[tokio::test]
async fn a_missing_resource_is_reported_by_its_arn() {
    // Rows differ in signing region and in the form of the missing resource's name.
    let rows: [(&str, &str); 4] = [
        ("us-east-1", "absent"),
        ("eu-west-2", "A"),
        ("ap-southeast-1", MAX_LENGTH_NAME),
        ("us-west-2", "mixed_Name-09"),
    ];
    assert_eq!(
        MAX_LENGTH_NAME.len(),
        40,
        "the upper-bound row must sit exactly on WorkflowDefinitionName's maximum, got {}",
        MAX_LENGTH_NAME.len()
    );

    for (region, missing) in rows {
        let emulator = Emulator::new(region);
        let (status, body) = emulator
            .call("PUT", "/workflow-definitions", Some(json!({ "name": PRESENT })))
            .await;
        assert_eq!(
            status, 201,
            "case {region:?}: the parent definition must be created, got {status} {body}"
        );
        let (status, body) = emulator
            .call(
                "PUT",
                &format!("/workflow-definitions/{PRESENT}/workflow-runs"),
                Some(json!({
                    "modelId": "nova-act-v1",
                    "clientInfo": { "compatibilityVersion": 1 },
                })),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {region:?}: the parent run must be created, got {status} {body}"
        );
        let run_id = text(&body, "workflowRunId");

        let cases: Vec<NotFoundCase> = vec![
            NotFoundCase {
                label: format!("GetWorkflowDefinition on the absent {missing:?}"),
                method: "GET",
                path: format!("/workflow-definitions/{missing}"),
                body: None,
                resource_type: "workflow-definition",
                resource_path: format!("workflow-definition/{missing}"),
            },
            NotFoundCase {
                label: format!("DeleteWorkflowDefinition on the absent {missing:?}"),
                method: "DELETE",
                path: format!("/workflow-definitions/{missing}"),
                body: None,
                resource_type: "workflow-definition",
                resource_path: format!("workflow-definition/{missing}"),
            },
            NotFoundCase {
                label: format!("GetWorkflowRun under the absent {missing:?}"),
                method: "GET",
                path: format!("/workflow-definitions/{missing}/workflow-runs/{run_id}"),
                body: None,
                resource_type: "workflow-definition",
                resource_path: format!("workflow-definition/{missing}"),
            },
            NotFoundCase {
                label: "GetWorkflowRun on an absent run of a present definition".to_string(),
                method: "GET",
                path: format!("/workflow-definitions/{PRESENT}/workflow-runs/{ABSENT_UUID}"),
                body: None,
                resource_type: "workflow-run",
                resource_path: format!(
                    "workflow-definition/{PRESENT}/workflow-run/{ABSENT_UUID}"
                ),
            },
            NotFoundCase {
                label: format!("CreateWorkflowRun under the absent {missing:?}"),
                method: "PUT",
                path: format!("/workflow-definitions/{missing}/workflow-runs"),
                body: Some(json!({
                    "modelId": "nova-act-v1",
                    "clientInfo": { "compatibilityVersion": 1 },
                })),
                resource_type: "workflow-definition",
                resource_path: format!("workflow-definition/{missing}"),
            },
            NotFoundCase {
                label: format!("CreateSession under the absent {missing:?}"),
                method: "PUT",
                path: format!("/workflow-definitions/{missing}/workflow-runs/{run_id}/sessions"),
                body: Some(json!({})),
                resource_type: "workflow-definition",
                resource_path: format!("workflow-definition/{missing}"),
            },
            NotFoundCase {
                label: "CreateSession under an absent run".to_string(),
                method: "PUT",
                path: format!(
                    "/workflow-definitions/{PRESENT}/workflow-runs/{ABSENT_UUID}/sessions"
                ),
                body: Some(json!({})),
                resource_type: "workflow-run",
                resource_path: format!(
                    "workflow-definition/{PRESENT}/workflow-run/{ABSENT_UUID}"
                ),
            },
            NotFoundCase {
                label: "CreateAct under an absent session".to_string(),
                method: "PUT",
                path: format!(
                    "/workflow-definitions/{PRESENT}/workflow-runs/{run_id}/sessions/{ABSENT_UUID}/acts"
                ),
                body: Some(json!({ "task": "search for a product" })),
                resource_type: "session",
                resource_path: format!(
                    "workflow-definition/{PRESENT}/workflow-run/{run_id}/session/{ABSENT_UUID}"
                ),
            },
            NotFoundCase {
                label: "CreateAct under an absent run".to_string(),
                method: "PUT",
                path: format!(
                    "/workflow-definitions/{PRESENT}/workflow-runs/{ABSENT_UUID}/sessions/{ABSENT_UUID}/acts"
                ),
                body: Some(json!({ "task": "search for a product" })),
                resource_type: "workflow-run",
                resource_path: format!(
                    "workflow-definition/{PRESENT}/workflow-run/{ABSENT_UUID}"
                ),
            },
        ];

        for case in cases {
            let label = format!("{} in {region}", case.label);
            let (status, answer) = emulator.call(case.method, &case.path, case.body).await;
            assert_identified_by_arn(
                &label,
                region,
                status,
                &answer,
                case.resource_type,
                &case.resource_path,
            );
        }
    }
}

/// The teardown idiom — delete, then read or delete again once the removal is due —
/// reports the gone definition and its gone run by ARN too.
#[tokio::test]
async fn a_definition_removed_by_the_clock_is_reported_by_arn() {
    let region = "eu-central-1";
    let emulator = Emulator::new(region);
    let name = "going-away";
    let (status, body) = emulator
        .call("PUT", "/workflow-definitions", Some(json!({ "name": name })))
        .await;
    assert_eq!(status, 201, "the definition must be created, got {status} {body}");
    let (status, body) = emulator
        .call(
            "PUT",
            &format!("/workflow-definitions/{name}/workflow-runs"),
            Some(json!({
                "modelId": "nova-act-v1",
                "clientInfo": { "compatibilityVersion": 1 },
            })),
        )
        .await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");

    let (status, body) = emulator
        .call("DELETE", &format!("/workflow-definitions/{name}"), None)
        .await;
    assert_eq!(status, 202, "the delete must be accepted, got {status} {body}");

    // The removal is a transition the clock owes; after its span the chain is gone.
    std::thread::sleep(DELETION_COMPLETION_DELAY + Duration::from_millis(250));

    let cases: Vec<NotFoundCase> = vec![
        NotFoundCase {
            label: "re-read the removed definition".to_string(),
            method: "GET",
            path: format!("/workflow-definitions/{name}"),
            body: None,
            resource_type: "workflow-definition",
            resource_path: format!("workflow-definition/{name}"),
        },
        NotFoundCase {
            label: "re-delete the removed definition".to_string(),
            method: "DELETE",
            path: format!("/workflow-definitions/{name}"),
            body: None,
            resource_type: "workflow-definition",
            resource_path: format!("workflow-definition/{name}"),
        },
        NotFoundCase {
            label: "read the run that went with it".to_string(),
            method: "GET",
            path: format!("/workflow-definitions/{name}/workflow-runs/{run_id}"),
            body: None,
            resource_type: "workflow-definition",
            resource_path: format!("workflow-definition/{name}"),
        },
    ];

    for case in cases {
        let (status, answer) = emulator.call(case.method, &case.path, case.body).await;
        assert_identified_by_arn(
            &case.label,
            region,
            status,
            &answer,
            case.resource_type,
            &case.resource_path,
        );
    }
}
