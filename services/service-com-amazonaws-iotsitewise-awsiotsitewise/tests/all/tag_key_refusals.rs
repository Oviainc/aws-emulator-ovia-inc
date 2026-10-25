// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE answer every IoT SiteWise operation that accepts
//! a `TagMap` gives a request whose tag key is outside the modelled `TagKey`
//! bound, driven over HTTP through the same `MultiServiceRouter` the emulator
//! boots with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a request this service refuses as malformed — here
//! a `tags` key outside `TagKey`'s own `@length(min: 1, max: 128)`, the bound the
//! framework's constraint walk does not reach because it validates a map's values
//! only — is refused with the client-input error the operation's model declares,
//! `InvalidRequestException` with HTTP 400, and stores nothing; none of these
//! operations declares a client-input error of any other name, so the wire error
//! code a client matches on is the same whichever of them it called. A key the
//! bound admits is accepted and reads back through `ListTagsForResource`.
//!
//! `TagKey` carries `@length` and no `@pattern` (smithy_query query="shape"
//! shape="TagMap,TagKey"), so the shape of a key inside the bound is not
//! constrained by the model and these suites pin no rule about it.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`) is the one the
//! asset suite already has, and the gateway platform and tag reader come from the
//! gateway and tagging suites; this suite reuses them rather than growing a
//! second copy.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, TAG_KEY_MAX_LENGTH, TAG_KEY_MIN_LENGTH,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::gateway_lifecycle::{greengrass_v2, GATEWAYS};
use crate::resource_tags::list_tags;

/// The workspace the workspace-scoped routes work in. `WorkspaceName` is
/// modelled `^[a-zA-Z0-9_-]+$`.
const WORKSPACE: &str = "tag-key-workspace";

/// An ECR image URI and an execution role of the forms `ContainerTaskConfiguration`
/// models (`ecrUri`, `taskExecutionRole`).
const PRIVATE_IMAGE: &str = "123456789012.dkr.ecr.us-west-2.amazonaws.com/analytics:v1";
/// See [`PRIVATE_IMAGE`].
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/SiteWiseTaskRole";

/// The minimum `TaskConfiguration` `CreateTask` requires: one container task with
/// the members the shape marks required.
fn task_configuration() -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": PRIVATE_IMAGE,
            "taskExecutionRole": EXECUTION_ROLE,
            // The first value of each enum (smithy_query query="shape"
            // shape="ProcessingType,ProcessingUnit").
            "processingType": "GENERIC_COMPUTE_PROCESSING",
            "processingUnit": "UNITS_2",
        }
    })
}

/// One operation that accepts a `TagMap`, as this suite drives it: its route, the
/// status a successful call answers, the body it sends for a given case suffix
/// (with `tags` added by the caller), and where the ARN of the tagged resource is
/// found afterwards.
struct Target {
    label: &'static str,
    uri: String,
    success: u16,
    body: Box<dyn Fn(&str) -> Value>,
    /// The ARN whose tag set `ListTagsForResource` must then report, read off the
    /// successful response (a create) or fixed (an operation that tags a resource
    /// that already exists).
    arn: Box<dyn Fn(&Value) -> String>,
}

/// Read a required string member off a response body.
fn member(body: &Value, name: &str) -> String {
    body[name]
        .as_str()
        .unwrap_or_else(|| panic!("no {name} in {body}"))
        .to_string()
}

/// Every implemented operation whose input carries a `TagMap`, one row each,
/// beside the ARN of the asset model the rows were built around — the resource
/// `TagResource` tags, and the one whose tag set a refusal must leave untouched.
///
/// The list is the `tags`-bearing inputs this crate implements (the remaining
/// `tags`-bearing operations — `CreateProject` and `CreateDashboard` — are still
/// scaffolded 501 stubs and are left to their own stub issues;
/// `CreateApplication` and `CreateComputationModel` are implemented and carry
/// this same tag-key cover in their own suites).
async fn targets(router: &MultiServiceRouter) -> (String, Vec<Target>) {
    // A stored asset model: the operation that creates an asset needs one to
    // point at, and the operation that tags an existing resource needs a real
    // resource ARN.
    let (status, model) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({ "assetModelName": "tag-key-host-model" })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the host asset model must be created, got {status} {model}"
    );
    let host_model_id = member(&model, "assetModelId");
    let host_model_arn = member(&model, "assetModelArn");
    let tagged_arn = host_model_arn.clone();

    // A stored portal: an access policy is about a Monitor resource this store
    // holds, so the policy rows point at this one.
    let (status, portal) = call(
        router,
        "POST",
        "/portals",
        Some(json!({
            "portalName": "tag-key-host-portal",
            "portalContactEmail": "admin@example.com",
            "roleArn": "arn:aws:iam::123456789012:role/service-role",
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "the host portal must be created, got {status} {portal}"
    );
    let host_portal_id = member(&portal, "portalId");

    let rows = vec![
        Target {
            label: "CreateAssetModel",
            uri: "/asset-models".to_string(),
            success: 202,
            body: Box::new(|suffix| json!({ "assetModelName": format!("model-{suffix}") })),
            arn: Box::new(|created| member(created, "assetModelArn")),
        },
        Target {
            label: "CreateAsset",
            uri: "/assets".to_string(),
            success: 202,
            body: Box::new(move |suffix| {
                json!({ "assetName": format!("asset-{suffix}"), "assetModelId": host_model_id })
            }),
            arn: Box::new(|created| member(created, "assetArn")),
        },
        Target {
            label: "CreatePortal",
            uri: "/portals".to_string(),
            success: 202,
            body: Box::new(|suffix| {
                json!({
                    "portalName": format!("portal-{suffix}"),
                    "portalContactEmail": "admin@example.com",
                    "roleArn": "arn:aws:iam::123456789012:role/service-role",
                })
            }),
            arn: Box::new(|created| member(created, "portalArn")),
        },
        Target {
            label: "CreateGateway",
            uri: GATEWAYS.to_string(),
            success: 201,
            body: Box::new(|suffix| {
                json!({
                    "gatewayName": format!("gateway-{suffix}"),
                    "gatewayPlatform": greengrass_v2(&format!("thing-{suffix}"), None),
                })
            }),
            arn: Box::new(|created| member(created, "gatewayArn")),
        },
        Target {
            label: "CreateDataset",
            uri: "/datasets".to_string(),
            success: 202,
            body: Box::new(|suffix| {
                json!({
                    "datasetName": format!("dataset-{suffix}"),
                    "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
                })
            }),
            arn: Box::new(|created| member(created, "datasetArn")),
        },
        Target {
            label: "CreateWorkspace",
            uri: "/workspaces".to_string(),
            success: 200,
            body: Box::new(|suffix| {
                json!({
                    "workspaceName": format!("workspace-{suffix}"),
                    "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
                })
            }),
            arn: Box::new(|created| member(created, "workspaceArn")),
        },
        Target {
            label: "CreatePipeline",
            uri: format!("/workspaces/{WORKSPACE}/pipelines"),
            success: 200,
            body: Box::new(|suffix| {
                json!({ "pipelineName": format!("pipeline-{suffix}"), "computations": [] })
            }),
            arn: Box::new(|created| member(created, "pipelineArn")),
        },
        Target {
            label: "CreateTask",
            uri: format!("/workspaces/{WORKSPACE}/tasks"),
            success: 200,
            body: Box::new(|suffix| {
                json!({
                    "taskName": format!("task-{suffix}"),
                    "taskConfiguration": task_configuration(),
                })
            }),
            arn: Box::new(|created| member(created, "taskArn")),
        },
        Target {
            label: "CreateAccessPolicy",
            uri: "/access-policies".to_string(),
            success: 201,
            body: Box::new(move |_| {
                json!({
                    "accessPolicyIdentity": { "user": { "id": "identity-center-user" } },
                    "accessPolicyResource": { "portal": { "id": host_portal_id } },
                    // One of the two `Permission` values (smithy_query
                    // query="shape" shape="Permission").
                    "accessPolicyPermission": "ADMINISTRATOR",
                })
            }),
            arn: Box::new(|created| member(created, "accessPolicyArn")),
        },
        Target {
            label: "TagResource",
            uri: format!("/tags?resourceArn={host_model_arn}"),
            success: 200,
            body: Box::new(|_| json!({})),
            arn: Box::new(move |_| host_model_arn.clone()),
        },
    ];
    (tagged_arn, rows)
}

/// The body `target` sends for `case_at`, carrying `key` as its one tag key.
fn body_with_key(target: &Target, case_at: usize, key: &str) -> Value {
    let mut body = (target.body)(&format!("{}-{case_at}", target.label.to_lowercase()));
    body.as_object_mut()
        .expect("an object body")
        .insert("tags".to_string(), json!({ key: "value" }));
    body
}

/// How many resources of every kind the store holds, beside the tag set of
/// `arn` — the fingerprint a refused call must leave exactly as it was.
async fn fingerprint(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) -> (usize, Value) {
    let state = service_state(&**engine).expect("the iotsitewise store is mounted");
    let held = {
        let store = state.lock();
        store.asset_models.len()
            + store.access_policies.len()
            + store.assets.len()
            + store.gateways.len()
            + store.portals.len()
            + store.pipelines.len()
            + store.tasks.len()
            + store.datasets.len()
            + store.workspaces.len()
    };
    let (_, tags) = list_tags(router, arn).await;
    (held, tags["tags"].clone())
}

/// One key under test: a label, the key, and whether `TagKey`'s own
/// `@length(min: 1, max: 128)` admits it.
///
/// The bound is read from this service's Smithy model — `TagMap`'s key target
/// (smithy_query query="shape" shape="TagMap,TagKey") — and each published end is
/// tested at its edge and one character past it. `@length` counts CHARACTERS, so
/// a multi-byte key is measured the same way an ASCII one is. The rows are built
/// from the crate's constants, so the model's own literals are asserted against
/// them once below, and the bound is pinned to the model rather than to the code.
fn key_cases() -> Vec<(&'static str, String, bool)> {
    assert_eq!(
        (TAG_KEY_MIN_LENGTH, TAG_KEY_MAX_LENGTH),
        (1, 128),
        "TagKey's modelled @length(min: 1, max: 128) is the bound these rows walk, \
         got ({TAG_KEY_MIN_LENGTH}, {TAG_KEY_MAX_LENGTH})"
    );
    vec![
        (
            "ascii key at the minimum length",
            "k".repeat(TAG_KEY_MIN_LENGTH),
            true,
        ),
        (
            "ascii key at the maximum length",
            "k".repeat(TAG_KEY_MAX_LENGTH),
            true,
        ),
        (
            "multi-byte key at the maximum length",
            "é".repeat(TAG_KEY_MAX_LENGTH),
            true,
        ),
        (
            "key of mixed form well inside the bound",
            "cost centre.42/north_line:a".to_string(),
            true,
        ),
        (
            "the empty key, one character short of the minimum",
            String::new(),
            false,
        ),
        (
            "ascii key one character past the maximum",
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            false,
        ),
        (
            "multi-byte key one character past the maximum",
            "é".repeat(TAG_KEY_MAX_LENGTH + 1),
            false,
        ),
        (
            "ascii key far past the maximum",
            "k".repeat(TAG_KEY_MAX_LENGTH * 4),
            false,
        ),
    ]
}

#[tokio::test]
async fn a_tag_key_outside_the_modelled_bound_is_refused_with_the_modelled_error_code() {
    let (router, engine) = router_with_engine();
    let (tagged_arn, targets) = targets(&router).await;
    let refused: Vec<(&str, String)> = key_cases()
        .into_iter()
        .filter(|(_, _, accepted)| !accepted)
        .map(|(label, key, _)| (label, key))
        .collect();

    for target in &targets {
        for (at, (key_case, key)) in refused.iter().enumerate() {
            let case = format!("{} / {key_case}", target.label);
            let before = fingerprint(&router, &engine, &tagged_arn).await;

            let (status, answer) = call(
                &router,
                "POST",
                &target.uri,
                Some(body_with_key(target, at, key)),
            )
            .await;

            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: a key of {} characters must be refused with the modelled \
                 client-input error, got {status} {answer}",
                key.chars().count()
            );
            let after = fingerprint(&router, &engine, &tagged_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused call must store nothing, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_tag_key_inside_the_modelled_bound_is_accepted_and_reads_back() {
    let router = router();
    let (_, targets) = targets(&router).await;
    let accepted: Vec<(&str, String)> = key_cases()
        .into_iter()
        .filter(|(_, _, accepted)| *accepted)
        .map(|(label, key, _)| (label, key))
        .collect();

    for target in &targets {
        for (at, (key_case, key)) in accepted.iter().enumerate() {
            let case = format!("{} / {key_case}", target.label);

            let (status, answer) = call(
                &router,
                "POST",
                &target.uri,
                Some(body_with_key(target, at, key)),
            )
            .await;
            assert_eq!(
                status, target.success,
                "case {case:?}: a key of {} characters is inside the bound, so the call must \
                 answer {}, got {status} {answer}",
                key.chars().count(),
                target.success
            );

            // The accepted key is read back through the tag reader, off the one
            // shared store: the tagged resource is the one the call named.
            let arn = (target.arn)(&answer);
            let (status, held) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the tags of {arn} must be readable, got {status} {held}"
            );
            assert_eq!(
                held["tags"][key].as_str(),
                Some("value"),
                "case {case:?}: the accepted key must be persisted under {arn}, got {held}"
            );
        }
    }
}
