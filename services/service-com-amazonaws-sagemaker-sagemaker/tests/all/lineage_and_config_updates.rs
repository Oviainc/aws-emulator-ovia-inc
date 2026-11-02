// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the configuration and lineage-action updates:
//! `UpdateAction`, `UpdateAppImageConfig` and `UpdateNotebookInstanceLifecycleConfig`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The RULE these tests assert, over a TABLE of inputs rather than one recorded call: an
//! update APPLIES the members its request carries to the stored resource and leaves every
//! other stored value alone, so the matching `Describe*` reports the updated values and
//! the resource's `LastModifiedTime` moves with the call. A name no resource is stored
//! under is refused and writes nothing.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/sagemaker/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000",
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `target` and require a 200, returning the parsed output.
    async fn ok(&self, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(target, body).await;
        assert_eq!(
            status, 200,
            "case {target}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn f64_field(case: &str, body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member} in {body}"))
}

/// A `Properties` map of `count` entries, for a case that differs along the ENTRY COUNT.
fn properties_of(count: usize) -> Value {
    let map: Map<String, Value> = (0..count)
        .map(|index| (format!("key{index}"), json!(format!("value{index}"))))
        .collect();
    Value::Object(map)
}

/// One row of an update table: the create body's extra members, the update to send, and
/// the members a following describe must report.
struct Row {
    case: &'static str,
    /// Extra members the create carries beside the resource's name.
    created: Value,
    /// The update body, without the name the test fills in.
    update: Value,
    /// Members a `Describe*` must report after the update, compared one by one.
    expected: Value,
}

/// RULE: `UpdateAction` replaces the description and the status the request carries,
/// replaces the whole property map when it carries `Properties` ("the new list of
/// properties. Overwrites the current property list" — smithy_query
/// doc=UpdateActionRequest$Properties), then drops the keys `PropertiesToRemove` names.
#[tokio::test]
async fn update_action_applies_the_members_the_request_carries() {
    // Rows differ along which members the update carries, the direction of the property
    // change (add, replace, prune, no-op), the entry count — including the modelled upper
    // bound of 30 — and the `ActionStatus` member sent.
    let thirty = properties_of(30);
    let rows = vec![
        Row {
            case: "description-status-and-properties-together",
            created: json!({
                "Description": "initial action",
                "Status": "InProgress",
                "Properties": { "approver": "ml-team" },
            }),
            update: json!({
                "Description": "updated action",
                "Status": "Completed",
                "Properties": { "approver": "ml-lead" },
            }),
            expected: json!({
                "Description": "updated action",
                "Status": "Completed",
                "Properties": { "approver": "ml-lead" },
            }),
        },
        Row {
            case: "status-only-keeps-the-description-and-the-properties",
            created: json!({
                "Description": "initial action",
                "Properties": { "approver": "ml-team" },
            }),
            update: json!({ "Status": "Stopped" }),
            expected: json!({
                "Description": "initial action",
                "Status": "Stopped",
                "Properties": { "approver": "ml-team" },
            }),
        },
        Row {
            case: "properties-added-to-an-action-that-held-none",
            created: json!({ "Description": "initial action" }),
            update: json!({ "Properties": { "stage": "prod" } }),
            expected: json!({
                "Description": "initial action",
                "Properties": { "stage": "prod" },
            }),
        },
        Row {
            case: "properties-overwrite-rather-than-merge",
            created: json!({ "Properties": { "a": "1", "b": "2" } }),
            update: json!({ "Properties": { "c": "3" } }),
            expected: json!({ "Properties": { "c": "3" } }),
        },
        Row {
            case: "properties-to-remove-prunes-a-subset",
            created: json!({ "Properties": { "a": "1", "b": "2", "c": "3" } }),
            update: json!({ "PropertiesToRemove": ["b"] }),
            expected: json!({ "Properties": { "a": "1", "c": "3" } }),
        },
        Row {
            case: "the-overwrite-happens-before-the-removal",
            created: json!({ "Properties": { "a": "1" } }),
            update: json!({ "Properties": { "x": "9", "y": "8" }, "PropertiesToRemove": ["x"] }),
            expected: json!({ "Properties": { "y": "8" } }),
        },
        Row {
            case: "removing-from-an-action-that-holds-no-properties-leaves-it-empty",
            created: json!({}),
            update: json!({ "PropertiesToRemove": ["a"] }),
            expected: json!({ "Properties": {} }),
        },
        Row {
            case: "properties-at-the-modelled-maximum-entry-count",
            created: json!({ "Properties": { "a": "1" } }),
            update: json!({ "Properties": thirty.clone() }),
            expected: json!({ "Properties": thirty.clone() }),
        },
        Row {
            case: "an-empty-description-is-a-value-the-model-admits",
            created: json!({ "Description": "initial action" }),
            update: json!({ "Description": "" }),
            expected: json!({ "Description": "" }),
        },
        Row {
            case: "an-update-carrying-no-editable-member-changes-no-value",
            created: json!({
                "Description": "initial action",
                "Status": "InProgress",
                "Properties": { "a": "1" },
            }),
            update: json!({}),
            expected: json!({
                "Description": "initial action",
                "Status": "InProgress",
                "Properties": { "a": "1" },
            }),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let mut body = json!({
            "ActionName": case,
            "ActionType": "ModelDeployment",
            "Source": { "SourceUri": format!("https://example.test/actions/{case}") },
        });
        for (key, value) in row.created.as_object().expect("an object of members") {
            body[key] = value.clone();
        }
        let created = emulator.ok("CreateAction", &body).await;
        let before = emulator
            .ok("DescribeAction", &json!({ "ActionName": case }))
            .await;
        let created_at = f64_field(case, &before, "CreationTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert("ActionName".to_string(), json!(case));
        let updated = emulator.ok("UpdateAction", &Value::Object(update)).await;
        assert_eq!(
            updated.get("ActionArn"),
            created.get("ActionArn"),
            "case {case:?}: the output carries the action's own ARN, got {updated:?}"
        );
        assert_eq!(
            updated.as_object().map(Map::len),
            Some(1),
            "case {case:?}: the output models ActionArn alone, got {updated:?}"
        );

        // Round trip: the update is read back through the implemented reader.
        let described = emulator
            .ok("DescribeAction", &json!({ "ActionName": case }))
            .await;
        for (member, expected) in row.expected.as_object().expect("an object of members") {
            assert_eq!(
                described.get(member),
                Some(expected),
                "case {case:?}: {member} after the update, got {described:?}"
            );
        }
        // Nothing the update did not name moves: the type, the source and the creation
        // instant are the create's.
        assert_eq!(
            described.get("ActionType").and_then(Value::as_str),
            Some("ModelDeployment"),
            "case {case:?}: ActionType is the create's, got {described:?}"
        );
        assert_eq!(
            described.get("Source"),
            body.get("Source"),
            "case {case:?}: Source is the create's, got {described:?}"
        );
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        assert!(
            f64_field(case, &described, "LastModifiedTime") >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {described:?}"
        );
    }
}

/// RULE: `UpdateAppImageConfig` replaces each app configuration the request carries
/// WHOLE — every member inside it is the request's, an explicitly sent `0` included — and
/// leaves the configurations it does not carry exactly as they were.
#[tokio::test]
async fn update_app_image_config_replaces_each_app_configuration_it_carries() {
    // Rows differ along which app configuration the update carries, the values inside it
    // (an explicit 0, the published uid/gid default, a larger one), the number of kernel
    // specifications, and whether the configuration existed before the update.
    let rows = vec![
        Row {
            case: "kernel-gateway-replaced-with-explicit-zero-ids",
            created: json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [{ "Name": "python3", "DisplayName": "Python 3" }],
                    "FileSystemConfig": {
                        "DefaultUid": 1000,
                        "DefaultGid": 100,
                        "MountPath": "/home/sagemaker-user",
                    },
                },
            }),
            update: json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [{ "Name": "python3", "DisplayName": "Python 3 Updated" }],
                    "FileSystemConfig": {
                        "DefaultUid": 0,
                        "DefaultGid": 0,
                        "MountPath": "/root",
                    },
                },
            }),
            expected: json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [{ "Name": "python3", "DisplayName": "Python 3 Updated" }],
                    "FileSystemConfig": {
                        "DefaultUid": 0,
                        "DefaultGid": 0,
                        "MountPath": "/root",
                    },
                },
            }),
        },
        Row {
            case: "kernel-gateway-replaced-with-two-kernel-specs",
            created: json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [{ "Name": "python3" }],
                },
            }),
            update: json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [
                        { "Name": "python3", "DisplayName": "Python 3" },
                        { "Name": "r", "DisplayName": "R 4" },
                    ],
                },
            }),
            expected: json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [
                        { "Name": "python3", "DisplayName": "Python 3" },
                        { "Name": "r", "DisplayName": "R 4" },
                    ],
                },
            }),
        },
        Row {
            case: "a-configuration-the-create-did-not-carry-is-added",
            created: json!({}),
            update: json!({
                "JupyterLabAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--port", "8888"] },
                },
            }),
            expected: json!({
                "JupyterLabAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--port", "8888"] },
                },
            }),
        },
        Row {
            case: "one-app-configuration-is-replaced-and-the-other-is-left-alone",
            created: json!({
                "KernelGatewayImageConfig": { "KernelSpecs": [{ "Name": "python3" }] },
                "CodeEditorAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--old"] },
                },
            }),
            update: json!({
                "CodeEditorAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--new"] },
                },
            }),
            expected: json!({
                "KernelGatewayImageConfig": { "KernelSpecs": [{ "Name": "python3" }] },
                "CodeEditorAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--new"] },
                },
            }),
        },
        Row {
            case: "an-update-carrying-no-app-configuration-changes-no-value",
            created: json!({
                "KernelGatewayImageConfig": { "KernelSpecs": [{ "Name": "python3" }] },
            }),
            update: json!({}),
            expected: json!({
                "KernelGatewayImageConfig": { "KernelSpecs": [{ "Name": "python3" }] },
            }),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let mut body = json!({ "AppImageConfigName": case });
        for (key, value) in row.created.as_object().expect("an object of members") {
            body[key] = value.clone();
        }
        let created = emulator.ok("CreateAppImageConfig", &body).await;
        let before = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": case }),
            )
            .await;
        let created_at = f64_field(case, &before, "CreationTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert("AppImageConfigName".to_string(), json!(case));
        let updated = emulator
            .ok("UpdateAppImageConfig", &Value::Object(update))
            .await;
        assert_eq!(
            updated.get("AppImageConfigArn"),
            created.get("AppImageConfigArn"),
            "case {case:?}: the output carries the configuration's own ARN, got {updated:?}"
        );

        // Round trip: the update is read back through the implemented reader.
        let described = emulator
            .ok(
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": case }),
            )
            .await;
        for (member, expected) in row.expected.as_object().expect("an object of members") {
            assert_eq!(
                described.get(member),
                Some(expected),
                "case {case:?}: {member} after the update, got {described:?}"
            );
        }
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        assert!(
            f64_field(case, &described, "LastModifiedTime") >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {described:?}"
        );
    }
}

/// RULE: `UpdateNotebookInstanceLifecycleConfig` rewrites the hook lists the request
/// carries — each one is the whole script list for that hook (`len 0..1`) — and leaves the
/// hook it does not carry as the create left it.
#[tokio::test]
async fn update_notebook_instance_lifecycle_config_rewrites_the_scripts_it_carries() {
    // Base64 payloads, as the member documents ("the shell script must be a base64-encoded
    // string"): `#!/bin/bash\necho one`, `...two` and an empty script.
    let first = "IyEvYmluL2Jhc2gKZWNobyBvbmU=";
    let second = "IyEvYmluL2Jhc2gKZWNobyB0d28=";
    // Rows differ along which hook the update rewrites, whether the create had set it, and
    // whether the new list is a script or the empty list the model's `len 0..1` admits.
    let rows = vec![
        Row {
            case: "on-start-rewritten",
            created: json!({ "OnStart": [{ "Content": first }] }),
            update: json!({ "OnStart": [{ "Content": second }] }),
            expected: json!({ "OnStart": [{ "Content": second }] }),
        },
        Row {
            case: "on-create-rewritten-and-on-start-left-alone",
            created: json!({
                "OnCreate": [{ "Content": first }],
                "OnStart": [{ "Content": first }],
            }),
            update: json!({ "OnCreate": [{ "Content": second }] }),
            expected: json!({
                "OnCreate": [{ "Content": second }],
                "OnStart": [{ "Content": first }],
            }),
        },
        Row {
            case: "both-hooks-rewritten",
            created: json!({ "OnCreate": [{ "Content": first }] }),
            update: json!({
                "OnCreate": [{ "Content": second }],
                "OnStart": [{ "Content": second }],
            }),
            expected: json!({
                "OnCreate": [{ "Content": second }],
                "OnStart": [{ "Content": second }],
            }),
        },
        Row {
            case: "a-hook-the-create-did-not-carry-is-added",
            created: json!({}),
            update: json!({ "OnStart": [{ "Content": first }] }),
            expected: json!({ "OnStart": [{ "Content": first }] }),
        },
        Row {
            case: "an-empty-list-leaves-the-hook-with-no-script",
            created: json!({ "OnStart": [{ "Content": first }] }),
            update: json!({ "OnStart": [] }),
            expected: json!({ "OnStart": [] }),
        },
        Row {
            case: "an-update-carrying-no-hook-changes-no-script",
            created: json!({ "OnStart": [{ "Content": first }] }),
            update: json!({}),
            expected: json!({ "OnStart": [{ "Content": first }] }),
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let mut body = json!({ "NotebookInstanceLifecycleConfigName": case });
        for (key, value) in row.created.as_object().expect("an object of members") {
            body[key] = value.clone();
        }
        emulator
            .ok("CreateNotebookInstanceLifecycleConfig", &body)
            .await;
        let before = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": case }),
            )
            .await;
        let created_at = f64_field(case, &before, "CreationTime");

        let mut update = row
            .update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        update.insert(
            "NotebookInstanceLifecycleConfigName".to_string(),
            json!(case),
        );
        let updated = emulator
            .ok(
                "UpdateNotebookInstanceLifecycleConfig",
                &Value::Object(update),
            )
            .await;
        assert_eq!(
            updated.as_object().map(Map::len),
            Some(0),
            "case {case:?}: the operation models no output member, got {updated:?}"
        );

        // Round trip: the update is read back through the implemented reader.
        let described = emulator
            .ok(
                "DescribeNotebookInstanceLifecycleConfig",
                &json!({ "NotebookInstanceLifecycleConfigName": case }),
            )
            .await;
        for (member, expected) in row.expected.as_object().expect("an object of members") {
            assert_eq!(
                described.get(member),
                Some(expected),
                "case {case:?}: {member} after the update, got {described:?}"
            );
        }
        assert_eq!(
            f64_field(case, &described, "CreationTime"),
            created_at,
            "case {case:?}: CreationTime never moves, got {described:?}"
        );
        assert!(
            f64_field(case, &described, "LastModifiedTime") >= created_at,
            "case {case:?}: LastModifiedTime must not go backwards, got {described:?}"
        );
    }
}

/// RULE: an update whose name no resource is stored under is refused at HTTP 400 — with
/// the operation's modelled `ResourceNotFound` where it models one, and otherwise through
/// the `ValidationException` channel — and writes nothing, so the refused call never mints
/// the resource it could not find.
#[tokio::test]
async fn updating_a_configuration_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    // One of each resource exists, so a refusal below is about the name that misses and
    // not about an empty store.
    emulator
        .ok(
            "CreateAction",
            &json!({
                "ActionName": "present",
                "ActionType": "ModelDeployment",
                "Source": { "SourceUri": "https://example.test/actions/present" },
            }),
        )
        .await;
    emulator
        .ok(
            "CreateAppImageConfig",
            &json!({ "AppImageConfigName": "present" }),
        )
        .await;
    emulator
        .ok(
            "CreateNotebookInstanceLifecycleConfig",
            &json!({ "NotebookInstanceLifecycleConfigName": "present" }),
        )
        .await;

    // Rows differ along the resource kind, how the name misses, which members the refused
    // update carried, and the error code the operation models for the fact.
    let cases: Vec<(&str, &str, &str, Value, &str, String)> = vec![
        (
            "action-never-created",
            "UpdateAction",
            "ActionName",
            json!({ "Description": "updated" }),
            "ResourceNotFound",
            format!("Action 'arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/absent' does not exist."),
        ),
        (
            "action-with-properties",
            "UpdateAction",
            "ActionName",
            json!({ "Properties": { "a": "1" } }),
            "ResourceNotFound",
            format!("Action 'arn:aws:sagemaker:us-east-1:{ACCOUNT}:action/absent' does not exist."),
        ),
        (
            "app-image-config-never-created",
            "UpdateAppImageConfig",
            "AppImageConfigName",
            json!({ "KernelGatewayImageConfig": { "KernelSpecs": [{ "Name": "python3" }] } }),
            "ResourceNotFound",
            format!(
                "Could not find AppImageConfig \
                 \"arn:aws:sagemaker:us-east-1:{ACCOUNT}:app-image-config/absent\"."
            ),
        ),
        (
            "app-image-config-no-members",
            "UpdateAppImageConfig",
            "AppImageConfigName",
            json!({}),
            "ResourceNotFound",
            format!(
                "Could not find AppImageConfig \
                 \"arn:aws:sagemaker:us-east-1:{ACCOUNT}:app-image-config/absent\"."
            ),
        ),
        (
            "notebook-lifecycle-config-never-created",
            "UpdateNotebookInstanceLifecycleConfig",
            "NotebookInstanceLifecycleConfigName",
            json!({ "OnStart": [{ "Content": "IyEvYmluL2Jhc2gK" }] }),
            // The operation models no not-found error at all (`smithy_query`
            // operation=UpdateNotebookInstanceLifecycleConfig lists
            // `ResourceLimitExceeded` alone), so the refusal is the validation channel.
            "ValidationException",
            format!(
                "Could not find notebook instance lifecycle configuration \
                 \"arn:aws:sagemaker:us-east-1:{ACCOUNT}:notebook-instance-lifecycle-config/\
                 absent\"."
            ),
        ),
    ];

    for (case, target, name_member, update, expected_code, expected_message) in &cases {
        let mut body = update
            .as_object()
            .expect("every row's update body is an object")
            .clone();
        body.insert((*name_member).to_string(), json!("absent"));
        let (status, parsed) = emulator.call(target, &Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a name nothing is stored under must be refused, \
             got {status} {parsed}"
        );
        assert_eq!(
            &error_code(&parsed),
            expected_code,
            "case {case:?}: error code, got {parsed:?}"
        );
        assert_eq!(
            &message(&parsed),
            expected_message,
            "case {case:?}: the refusal sentence, got {parsed:?}"
        );

        // The refused update created nothing.
        let reader = target.replace("Update", "Describe");
        let mut read_body = Map::new();
        read_body.insert((*name_member).to_string(), json!("absent"));
        let (read_status, read) = emulator.call(&reader, &Value::Object(read_body)).await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: a refused update must mint nothing, got {read_status} {read}"
        );
    }

    // Every resource that does exist is untouched, and still updatable.
    let described = emulator
        .ok("DescribeAction", &json!({ "ActionName": "present" }))
        .await;
    assert_eq!(
        described.get("Description"),
        None,
        "case present: the live action must be unchanged, got {described:?}"
    );
    emulator
        .ok(
            "UpdateAction",
            &json!({ "ActionName": "present", "Description": "updated" }),
        )
        .await;
    let described = emulator
        .ok("DescribeAction", &json!({ "ActionName": "present" }))
        .await;
    assert_eq!(
        described.get("Description").and_then(Value::as_str),
        Some("updated"),
        "case present: the live action is still updatable, got {described:?}"
    );
}
