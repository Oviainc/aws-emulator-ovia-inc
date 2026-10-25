// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise workspace task slice
//! (`CreateTask` -> `UpdateTask` -> `DescribeTask`), driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created task is persisted and reads back exactly as it was created, at its
//!   first version, under the ARN that carries both its workspace and its name;
//! * an accepted write APPENDS a version: the version counter rises by one per
//!   write, an update replaces only the members the request carries and preserves
//!   every member it omits, and a read naming an earlier version still answers
//!   from the configuration that version carried;
//! * a task is addressed by `(workspaceName, taskName)`, so one name is free in
//!   every other workspace and taken in its own;
//! * a create and an update both owe a transition the clock makes due, so a read
//!   taken straight afterwards still observes `CREATING` / `UPDATING`;
//! * a write the model's own constraints refuse changes nothing, and a write
//!   naming a task no workspace holds is the modelled not-found refusal.
//!
//! The HTTP harness (`call`, `router`, `error_code`) is the one the asset suite
//! already has; this suite reuses it rather than growing a second.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::FIRST_TASK_VERSION;

use crate::asset_lifecycle::{call, error_code, router, REGION};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "plant-floor";

/// `POST|GET /workspaces/{workspaceName}/tasks`, as `OPERATIONS.md` reports the
/// route.
///
/// This module owns the task slice's harness: the helpers a sibling suite under
/// `tests/all/` also needs are `pub(crate)`, the way `asset_lifecycle` exposes
/// `call` / `router` / `error_code`, so no second copy of a route or a body
/// builder can drift from this one.
pub(crate) fn tasks_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/tasks")
}

/// `GET|PUT|DELETE /workspaces/{workspaceName}/tasks/{taskName}`.
pub(crate) fn task_uri(workspace: &str, name: &str) -> String {
    format!("/workspaces/{workspace}/tasks/{name}")
}

/// The ARN form this emulator mints for a task: the workspace as well as the
/// name, since the name is unique only within the workspace.
fn expected_arn(workspace: &str, name: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:task/{workspace}/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `state` of a `ResourceStatus` in a response body.
pub(crate) fn state(body: &Value) -> &str {
    body["status"]["state"].as_str().unwrap_or("")
}

/// An ECR image URI of the private-registry form `EcrUri`'s pattern admits
/// (smithy_query query="shape" shape="ContainerTaskConfiguration": `ecrUri`).
pub(crate) const PRIVATE_IMAGE: &str = "123456789012.dkr.ecr.us-west-2.amazonaws.com/analytics:v1";

/// The same member's public-registry form.
pub(crate) const PUBLIC_IMAGE: &str = "public.ecr.aws/docker/library/python:3.12";

/// An IAM role ARN of the form `IamRoleArn`'s pattern admits (same shape,
/// `taskExecutionRole`).
pub(crate) const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/SiteWiseTaskRole";

/// The `ProcessingType` values, from the enum itself (smithy_query query="shape"
/// shape="ProcessingType").
pub(crate) const PROCESSING_TYPES: [&str; 2] = [
    "GENERIC_COMPUTE_PROCESSING",
    "HARDWARE_ACCELERATED_PROCESSING",
];

/// The `ProcessingUnit` values, from the enum itself (smithy_query query="shape"
/// shape="ProcessingUnit").
pub(crate) const PROCESSING_UNITS: [&str; 14] = [
    "UNITS_2", "UNITS_4", "UNITS_8", "UNITS_12", "UNITS_16", "UNITS_24", "UNITS_32", "UNITS_36",
    "UNITS_48", "UNITS_60", "UNITS_64", "UNITS_72", "UNITS_84", "UNITS_96",
];

/// The `StorageClass` values an `EphemeralStorageConfiguration` admits
/// (smithy_query query="shape" shape="StorageClass").
const STORAGE_CLASSES: [&str; 4] = ["STANDARD_1", "STANDARD_2", "THROUGHPUT_1", "THROUGHPUT_2"];

/// The published bounds of `timeoutSeconds` (`@range(min: 60, max: 86400)`).
const TIMEOUT_MIN: i64 = 60;
/// See [`TIMEOUT_MIN`].
const TIMEOUT_MAX: i64 = 86_400;

/// The published bounds of `storageSizeInGiB` (`@range(min: 1, max: 16384)`).
const STORAGE_SIZE_MIN: i64 = 1;
/// See [`STORAGE_SIZE_MIN`].
const STORAGE_SIZE_MAX: i64 = 16_384;

/// A `containerTaskConfiguration` carrying only the members the shape requires.
pub(crate) fn minimal_container(processing_type: &str, processing_unit: &str) -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": PRIVATE_IMAGE,
            "taskExecutionRole": EXECUTION_ROLE,
            "processingType": processing_type,
            "processingUnit": processing_unit,
        }
    })
}

/// One mount of the only source form `MountSource` declares (smithy_query
/// query="shape" shape="MountSource": `s3AccessPoint`).
fn s3_mount(name: &str, relative_path: &str, prefix: Option<&str>) -> Value {
    let mut source = json!({
        "accessPointArn": "arn:aws:s3:us-west-2:123456789012:accesspoint/sensor-data",
    });
    if let Some(prefix) = prefix {
        source
            .as_object_mut()
            .expect("an object source")
            .insert("prefix".to_string(), json!(prefix));
    }
    json!({
        "name": name,
        "relativePath": relative_path,
        "source": { "s3AccessPoint": source },
        // The only `MountStorageType` value (smithy_query query="shape"
        // shape="MountStorageType").
        "storageType": "SHARED_STORAGE",
    })
}

/// `CreateTask` with `body`.
pub(crate) async fn create(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &tasks_uri(workspace), Some(body)).await
}

/// `UpdateTask` with `body`.
pub(crate) async fn update(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "PUT", &task_uri(workspace, name), Some(body)).await
}

/// `DescribeTask`, at the latest version or at `version`.
///
/// `taskVersion` rides the query string as `version` (smithy_query
/// query="operation" operation="DescribeTask"), so the label is sent exactly as
/// the caller spells it — including one the modelled form does not admit.
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    workspace: &str,
    name: &str,
    version: Option<&str>,
) -> (u16, Value) {
    let uri = match version {
        Some(version) => format!("{}?version={version}", task_uri(workspace, name)),
        None => task_uri(workspace, name),
    };
    call(router, "GET", &uri, None).await
}

/// A `DescribeTask` body without its lifecycle `status`, which time moves on its
/// own.
pub(crate) fn without_status(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("status");
    }
    copy
}

/// One row of the create-and-read-back table: the configuration a create sends.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    configuration: Value,
    description: Option<&'static str>,
    tags: Option<Value>,
}

/// The rows: `taskConfiguration` spanning what `ContainerTaskConfiguration`
/// admits — the required members alone, each `processingType` paired with a
/// `processingUnit` the catalog above lists, ephemeral storage at each published
/// bound, a command of one and of several words, environment variables absent,
/// set and empty, `timeoutSeconds` at each published bound, and mounts with and
/// without a key prefix — crossed with a present / absent `description` and with
/// / without tags.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "required-members-only",
            configuration: minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[0]),
            description: None,
            tags: None,
        },
        CreateCase {
            label: "accelerated-at-the-largest-unit",
            configuration: minimal_container(
                PROCESSING_TYPES[1],
                PROCESSING_UNITS[PROCESSING_UNITS.len() - 1],
            ),
            description: Some("a GPU-accelerated workload"),
            tags: Some(json!({ "team": "ops" })),
        },
        CreateCase {
            label: "a-middle-processing-unit",
            configuration: minimal_container(PROCESSING_TYPES[0], PROCESSING_UNITS[6]),
            description: Some("thirty-two units"),
            tags: None,
        },
        CreateCase {
            label: "public-registry-image",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PUBLIC_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[1],
                }
            }),
            description: None,
            tags: Some(json!({ "registry": "public", "env": "test" })),
        },
        CreateCase {
            label: "smallest-ephemeral-storage",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[2],
                    "ephemeralStorageConfiguration": {
                        "storageClass": STORAGE_CLASSES[0],
                        "storageSizeInGiB": STORAGE_SIZE_MIN,
                    },
                }
            }),
            description: None,
            tags: None,
        },
        CreateCase {
            label: "largest-ephemeral-storage",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[1],
                    "processingUnit": PROCESSING_UNITS[9],
                    "ephemeralStorageConfiguration": {
                        "storageClass": STORAGE_CLASSES[STORAGE_CLASSES.len() - 1],
                        "storageSizeInGiB": STORAGE_SIZE_MAX,
                    },
                }
            }),
            description: Some("the largest volume the range admits"),
            tags: None,
        },
        CreateCase {
            label: "command-and-environment-variables",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[3],
                    "command": ["python", "-m", "analytics.main"],
                    "environmentVariables": { "MODE": "batch", "SHARD": "2" },
                }
            }),
            description: Some("a command of several words"),
            tags: Some(json!({ "team": "data" })),
        },
        CreateCase {
            label: "one-word-command-and-empty-environment",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[4],
                    "command": ["run"],
                    // Sent as an empty map, so the read must report an empty map
                    // and not omit the member.
                    "environmentVariables": {},
                }
            }),
            description: None,
            tags: None,
        },
        CreateCase {
            label: "timeout-at-the-lowest-bound",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[0],
                    "timeoutSeconds": TIMEOUT_MIN,
                }
            }),
            description: None,
            tags: None,
        },
        CreateCase {
            label: "timeout-at-the-highest-bound",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[1],
                    "processingUnit": PROCESSING_UNITS[13],
                    "timeoutSeconds": TIMEOUT_MAX,
                }
            }),
            description: Some("a whole day"),
            tags: None,
        },
        CreateCase {
            label: "two-mounts-one-with-a-prefix",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[5],
                    "mounts": [
                        s3_mount("raw", "data/raw", None),
                        s3_mount("curated", "data/curated", Some("2026/10/")),
                    ],
                }
            }),
            description: None,
            tags: Some(json!({ "mounted": "yes" })),
        },
        CreateCase {
            label: "every-optional-member-at-once",
            configuration: json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[1],
                    "processingUnit": PROCESSING_UNITS[11],
                    "ephemeralStorageConfiguration": {
                        "storageClass": STORAGE_CLASSES[2],
                        "storageSizeInGiB": 64,
                    },
                    "command": ["./entrypoint.sh", "--full"],
                    "timeoutSeconds": 7_200,
                    "environmentVariables": { "LOG_LEVEL": "debug" },
                    "mounts": [s3_mount("shared", "shared", None)],
                }
            }),
            description: Some("everything the shape admits"),
            tags: Some(json!({ "team": "platform", "env": "test" })),
        },
    ]
}

/// The body one row sends.
fn create_body(case: &CreateCase, name: &str) -> Value {
    let mut body = json!({ "taskName": name, "taskConfiguration": case.configuration.clone() });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(tags) = case.tags.clone() {
        object.insert("tags".to_string(), tags);
    }
    body
}

#[tokio::test]
async fn created_task_reads_back_through_describe_for_every_configuration_shape() {
    let router = router();
    let first_version = FIRST_TASK_VERSION.to_string();

    for case in create_cases() {
        let name = format!("task-{}", case.label);
        let (status, created) = create(&router, WORKSPACE, create_body(&case, &name)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {created}",
            case.label
        );
        assert_eq!(
            created["taskName"].as_str().unwrap_or(""),
            name,
            "case {:?}: unexpected taskName in {created}",
            case.label
        );
        assert_eq!(
            created["taskArn"].as_str().unwrap_or(""),
            expected_arn(WORKSPACE, &name),
            "case {:?}: unexpected taskArn in {created}",
            case.label
        );
        assert_eq!(
            created["version"].as_str().unwrap_or(""),
            first_version,
            "case {:?}: a created task is at its first version, got {created}",
            case.label
        );
        assert_eq!(
            state(&created),
            "CREATING",
            "case {:?}: a created task reports CREATING, got {created}",
            case.label
        );

        let (status, described) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeTask expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["taskName"].as_str().unwrap_or(""),
            name,
            "case {:?}: DescribeTask reported another name in {described}",
            case.label
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {:?}: DescribeTask reported another workspace in {described}",
            case.label
        );
        assert_eq!(
            described["taskArn"], created["taskArn"],
            "case {:?}: DescribeTask reported another ARN in {described}",
            case.label
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            first_version,
            "case {:?}: unexpected version in {described}",
            case.label
        );
        assert_eq!(
            described["taskConfiguration"], case.configuration,
            "case {:?}: DescribeTask reported another taskConfiguration in {described}",
            case.label
        );
        assert_eq!(
            described.get("description").cloned(),
            case.description.map(|text| json!(text)),
            "case {:?}: unexpected description in {described}",
            case.label
        );

        let created_at = described["createdAt"].as_f64().unwrap_or(0.0);
        let updated_at = described["updatedAt"].as_f64().unwrap_or(0.0);
        assert!(
            created_at > 0.0,
            "case {:?}: createdAt must be a real instant, got {described}",
            case.label
        );
        assert_eq!(
            created_at, updated_at,
            "case {:?}: an untouched task's updatedAt is its createdAt, got {described}",
            case.label
        );

        // Reading does not change what the next caller sees. The lifecycle state is
        // the one member time moves on its own — the owed CREATING -> ACTIVE
        // transition — so it is compared by value and the rest by equality.
        let (status, again) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: second DescribeTask expected 200, got {status} {again}",
            case.label
        );
        assert!(
            matches!(state(&again), "CREATING" | "ACTIVE"),
            "case {:?}: a created task is CREATING or settled ACTIVE, got {again}",
            case.label
        );
        assert_eq!(
            without_status(&again),
            without_status(&described),
            "case {:?}: DescribeTask answered differently on the second read: {again}",
            case.label
        );

        // The tags the create carried are persisted on the task and read back
        // through `ListTagsForResource`, which resolves them by the task's ARN.
        let (status, listed) = call(
            &router,
            "GET",
            &format!("/tags?resourceArn={}", expected_arn(WORKSPACE, &name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {:?}: unexpected tags in {listed}",
            case.label
        );
    }
}

/// The configuration every row of the update table starts from.
fn base_configuration() -> Value {
    json!({
        "containerTaskConfiguration": {
            "ecrUri": PRIVATE_IMAGE,
            "taskExecutionRole": EXECUTION_ROLE,
            "processingType": PROCESSING_TYPES[0],
            "processingUnit": PROCESSING_UNITS[0],
            "environmentVariables": { "LOG_LEVEL": "info" },
        }
    })
}

/// The description every row of the update table starts from.
const BASE_DESCRIPTION: &str = "the configuration the create wrote";

/// Create the task the update table starts from.
async fn create_base(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    create(
        router,
        WORKSPACE,
        json!({
            "taskName": name,
            "taskConfiguration": base_configuration(),
            "description": BASE_DESCRIPTION,
        }),
    )
    .await
}

#[tokio::test]
async fn an_update_writes_a_new_version_and_preserves_the_members_it_omits() {
    let router = router();
    let replacement = json!({
        "containerTaskConfiguration": {
            "ecrUri": PUBLIC_IMAGE,
            "taskExecutionRole": EXECUTION_ROLE,
            "processingType": PROCESSING_TYPES[1],
            "processingUnit": PROCESSING_UNITS[13],
            "timeoutSeconds": TIMEOUT_MAX,
        }
    });

    // (case, the update body, the description / taskConfiguration the task must
    // carry afterwards). One row per subset of the two updatable members.
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "nothing at all",
            json!({}),
            json!(BASE_DESCRIPTION),
            base_configuration(),
        ),
        (
            "the description alone",
            json!({ "description": "a new description" }),
            json!("a new description"),
            base_configuration(),
        ),
        (
            "the configuration alone",
            json!({ "taskConfiguration": replacement.clone() }),
            json!(BASE_DESCRIPTION),
            replacement.clone(),
        ),
        (
            "both members at once",
            json!({ "description": "both replaced", "taskConfiguration": replacement.clone() }),
            json!("both replaced"),
            replacement.clone(),
        ),
    ];

    for (case, body, expected_description, expected_configuration) in cases {
        let name = format!("task-update-{}", case.replace(' ', "-"));
        let (status, created) = create_base(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the base create expected 200, got {status} {created}"
        );

        let (status, updated) = update(&router, WORKSPACE, &name, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateTask expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["version"].as_str().unwrap_or(""),
            (FIRST_TASK_VERSION + 1).to_string(),
            "case {case:?}: an update mints the next version, got {updated}"
        );
        assert_eq!(
            state(&updated),
            "UPDATING",
            "case {case:?}: an accepted update reports UPDATING, got {updated}"
        );

        let (status, described) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTask expected 200, got {status} {described}"
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            (FIRST_TASK_VERSION + 1).to_string(),
            "case {case:?}: the latest version is the one the update minted, got {described}"
        );
        assert_eq!(
            described["description"], expected_description,
            "case {case:?}: unexpected description in {described}"
        );
        assert_eq!(
            described["taskConfiguration"], expected_configuration,
            "case {case:?}: unexpected taskConfiguration in {described}"
        );
        assert!(
            described["updatedAt"].as_f64().unwrap_or(0.0)
                >= described["createdAt"].as_f64().unwrap_or(0.0),
            "case {case:?}: an updated task's updatedAt is not before its createdAt, got {described}"
        );

        // The version the update replaced is still readable, and still carries the
        // configuration it was written with.
        let first = FIRST_TASK_VERSION.to_string();
        let (status, earlier) = describe(&router, WORKSPACE, &name, Some(&first)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading version {first} expected 200, got {status} {earlier}"
        );
        assert_eq!(
            earlier["version"].as_str().unwrap_or(""),
            first,
            "case {case:?}: the read must answer from the version it named, got {earlier}"
        );
        assert_eq!(
            earlier["description"],
            json!(BASE_DESCRIPTION),
            "case {case:?}: version {first} must carry the description the create wrote, got {earlier}"
        );
        assert_eq!(
            earlier["taskConfiguration"],
            base_configuration(),
            "case {case:?}: version {first} must carry the configuration the create wrote, got {earlier}"
        );
    }
}

#[tokio::test]
async fn every_accepted_write_raises_the_version_by_one_and_leaves_the_earlier_ones_readable() {
    let router = router();
    let name = "task-version-history";
    let (status, created) = create_base(&router, name).await;
    assert_eq!(
        status, 200,
        "case {:?}: the base create expected 200, got {status} {created}",
        "the create"
    );

    // One update per word, so the configuration of version N is recognisable from
    // the version alone and a read naming it can be checked against exactly it.
    let words = ["second", "third", "fourth", "fifth"];
    for (index, word) in words.iter().enumerate() {
        let expected_version = FIRST_TASK_VERSION + 1 + index as u64;
        let (status, updated) = update(
            &router,
            WORKSPACE,
            name,
            json!({ "description": format!("the {word} configuration") }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {word:?}: UpdateTask expected 200, got {status} {updated}"
        );
        assert_eq!(
            updated["version"].as_str().unwrap_or(""),
            expected_version.to_string(),
            "case {word:?}: the version counter rises by one per write, got {updated}"
        );
    }

    // Every version a write produced still reads back from its own configuration.
    let (status, described) =
        describe(&router, WORKSPACE, name, Some(&FIRST_TASK_VERSION.to_string())).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {described}",
        "version 1"
    );
    assert_eq!(
        described["description"],
        json!(BASE_DESCRIPTION),
        "case {:?}: must carry the description the create wrote, got {described}",
        "version 1"
    );
    for (index, word) in words.iter().enumerate() {
        let version = (FIRST_TASK_VERSION + 1 + index as u64).to_string();
        let (status, described) = describe(&router, WORKSPACE, name, Some(&version)).await;
        assert_eq!(
            status, 200,
            "case {word:?}: reading version {version} expected 200, got {status} {described}"
        );
        assert_eq!(
            described["description"],
            json!(format!("the {word} configuration")),
            "case {word:?}: version {version} must carry the description that write sent, got {described}"
        );
    }

    // A version no write ever produced names nothing this service holds.
    let past_the_end = (FIRST_TASK_VERSION + 1 + words.len() as u64).to_string();
    let (status, body) = describe(&router, WORKSPACE, name, Some(&past_the_end)).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case {:?}: a version no write produced is a not-found, got {status} {body}",
        past_the_end
    );
}

#[tokio::test]
async fn a_task_is_addressed_by_its_workspace_and_its_name() {
    let router = router();
    let name = "shared-name";
    // Two workspaces, including one whose name is a prefix of the other's, so a
    // key built by concatenation rather than by the pair would collide.
    let workspaces = ["plant", "plant-floor", "other-site"];

    for workspace in workspaces {
        let (status, created) = create(
            &router,
            workspace,
            json!({ "taskName": name, "taskConfiguration": base_configuration() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: one name is free in every workspace, got {status} {created}"
        );
        assert_eq!(
            created["taskArn"].as_str().unwrap_or(""),
            expected_arn(workspace, name),
            "case {workspace:?}: the ARN must carry the workspace, got {created}"
        );
    }

    // An update in one workspace moves only that workspace's task.
    let (status, updated) = update(
        &router,
        workspaces[1],
        name,
        json!({ "description": "touched in one workspace only" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: UpdateTask expected 200, got {status} {updated}",
        workspaces[1]
    );
    for workspace in workspaces {
        let expected = if workspace == workspaces[1] {
            (FIRST_TASK_VERSION + 1).to_string()
        } else {
            FIRST_TASK_VERSION.to_string()
        };
        let (status, described) = describe(&router, workspace, name, None).await;
        assert_eq!(
            status, 200,
            "case {workspace:?}: DescribeTask expected 200, got {status} {described}"
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            expected,
            "case {workspace:?}: only the updated workspace's task moves, got {described}"
        );
    }

    // In its own workspace the name is taken.
    let (status, body) = create(
        &router,
        workspaces[1],
        json!({ "taskName": name, "taskConfiguration": base_configuration() }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (409, "ConflictingOperationException"),
        "case {:?}: a name a task already holds collides, got {status} {body}",
        "a second create of the same name"
    );
    assert_eq!(
        body["resourceArn"].as_str().unwrap_or(""),
        expected_arn(workspaces[1], name),
        "case {:?}: the refusal names the task it collided with, got {body}",
        "a second create of the same name"
    );

    // A retry carrying the client token of a create that already succeeded replays
    // that create's answer instead of colliding with it.
    let token = "11111111-1111-1111-1111-111111111111";
    let retried_name = "retried";
    let body = json!({
        "taskName": retried_name,
        "taskConfiguration": base_configuration(),
        "clientToken": token,
    });
    let (status, first) = create(&router, WORKSPACE, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {first}",
        "the first create"
    );
    let (status, retried) = create(&router, WORKSPACE, body).await;
    assert_eq!(
        status, 200,
        "case {:?}: a retry of the same create succeeds, got {status} {retried}",
        "the retry"
    );
    assert_eq!(
        retried["version"], first["version"],
        "case {:?}: the retry reports the version the original minted, got {retried}",
        "the retry"
    );
    assert_eq!(
        retried["taskArn"], first["taskArn"],
        "case {:?}: the retry reports the task the original made, got {retried}",
        "the retry"
    );
}

#[tokio::test]
async fn a_write_naming_a_task_no_workspace_holds_is_the_modelled_not_found() {
    let router = router();
    let held = "held-task";
    let (status, created) = create_base(&router, held).await;
    assert_eq!(
        status, 200,
        "case {:?}: the base create expected 200, got {status} {created}",
        "the create"
    );

    // (case, workspace, task name) — each addresses something this store does not
    // hold, by workspace, by name, or by both.
    let cases = [
        ("an absent task in a workspace that holds one", WORKSPACE, "no-such-task"),
        ("a task of a workspace nothing was ever created in", "empty-workspace", held),
        ("neither the workspace nor the task", "empty-workspace", "no-such-task"),
    ];

    for (case, workspace, task) in cases {
        let (status, body) = update(
            &router,
            workspace,
            task,
            json!({ "description": "an update that must not land" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a modelled not-found, got {status} {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(task),
            "case {case:?}: the refusal names the task the request addressed, got {body}"
        );

        let (status, described) = describe(&router, workspace, task, None).await;
        assert_eq!(
            (status, error_code(&described)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused update creates nothing, got {status} {described}"
        );
    }

    // The task that DOES exist was not touched by any of the refusals above.
    let (status, described) = describe(&router, WORKSPACE, held, None).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} {described}",
        "the untouched task"
    );
    assert_eq!(
        described["version"].as_str().unwrap_or(""),
        FIRST_TASK_VERSION.to_string(),
        "case {:?}: a refused update elsewhere must not bump this task, got {described}",
        "the untouched task"
    );
}

#[tokio::test]
async fn a_write_the_modelled_constraints_refuse_changes_nothing() {
    let router = router();
    let name = "task-refusals";
    let (status, created) = create_base(&router, name).await;
    assert_eq!(
        status, 200,
        "case {:?}: the base create expected 200, got {status} {created}",
        "the create"
    );

    // One row per published bound, one step outside it: the modelled `@range`
    // constraints are refused before the handler runs, so the request is a 400 and
    // the stored task keeps the version and the configuration it had.
    let cases: Vec<(&str, Value)> = vec![
        (
            "a timeout one second below the lowest bound",
            json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[0],
                    "timeoutSeconds": TIMEOUT_MIN - 1,
                }
            }),
        ),
        (
            "a timeout one second above the highest bound",
            json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[0],
                    "timeoutSeconds": TIMEOUT_MAX + 1,
                }
            }),
        ),
        (
            "an ephemeral volume one GiB below the lowest bound",
            json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[0],
                    "ephemeralStorageConfiguration": {
                        "storageClass": STORAGE_CLASSES[0],
                        "storageSizeInGiB": STORAGE_SIZE_MIN - 1,
                    },
                }
            }),
        ),
        (
            "an ephemeral volume one GiB above the highest bound",
            json!({
                "containerTaskConfiguration": {
                    "ecrUri": PRIVATE_IMAGE,
                    "taskExecutionRole": EXECUTION_ROLE,
                    "processingType": PROCESSING_TYPES[0],
                    "processingUnit": PROCESSING_UNITS[0],
                    "ephemeralStorageConfiguration": {
                        "storageClass": STORAGE_CLASSES[0],
                        "storageSizeInGiB": STORAGE_SIZE_MAX + 1,
                    },
                }
            }),
        ),
    ];

    for (case, configuration) in cases {
        let (status, body) = update(
            &router,
            WORKSPACE,
            name,
            json!({ "taskConfiguration": configuration.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the modelled range must be refused, got {status} {body}"
        );

        let (status, described) = describe(&router, WORKSPACE, name, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTask expected 200, got {status} {described}"
        );
        assert_eq!(
            described["version"].as_str().unwrap_or(""),
            FIRST_TASK_VERSION.to_string(),
            "case {case:?}: a refused update must not mint a version, got {described}"
        );
        assert_eq!(
            described["taskConfiguration"],
            base_configuration(),
            "case {case:?}: a refused update must not change the stored configuration, got {described}"
        );

        // The same value is refused by the create, which stores nothing either.
        let fresh = format!("task-refused-{}", case.replace(' ', "-"));
        let (status, body) = create(
            &router,
            WORKSPACE,
            json!({ "taskName": fresh.clone(), "taskConfiguration": configuration }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the create refuses the same value, got {status} {body}"
        );
        let (status, body) = describe(&router, WORKSPACE, &fresh, None).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused create must store nothing, got {status} {body}"
        );
    }
}
