// SPDX-License-Identifier: CC0-1.0

//! Cover for WHERE a build's output artifacts go, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: `ProjectArtifacts$location` is "the name of the output bucket"
//! for the artifacts type that stores build output in a bucket, and there is no default,
//! so artifacts of that type have to name one. The rule holds wherever a project's or a
//! build's artifacts are SET — `CreateProject`, `UpdateProject`, and the
//! `artifactsOverride` / `secondaryArtifactsOverride` of `StartBuild` and
//! `StartBuildBatch` — and for the primary artifacts and each secondary entry alike. For
//! every other artifacts type the member is documented as ignored, so its absence
//! constrains nothing.
//!
//! A refused `StartBuild` must also leave NO build behind: the refusal is decided before
//! any state of the service is touched, so no build id and no build number is spent on a
//! build AWS never created.
//!
//! Whether the bucket a destination names EXISTS is resolved against S3, so that leg is
//! covered where an S3 to ask is mounted (`core/router/tests/all/`), not here.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use super::project_build_lifecycle::{
    batch_enabled, call, create_project, create_project_body, emulator, error_type, listed_ids,
    merged,
};

/// The `message` an error body carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The same artifacts as a SECONDARY entry: the shape is the same one, so it carries the
/// identifier a secondary artifact is addressed by beside what the row declares.
fn as_secondary(artifacts: &Value) -> Value {
    let mut entry = artifacts.clone();
    if let Value::Object(members) = &mut entry {
        members.insert("artifactIdentifier".to_string(), json!("second"));
    }
    json!([entry])
}

/// Artifacts of the bucket type that name no bucket — every form "names no bucket" takes:
/// the member absent, empty, and nothing but whitespace.
fn destinations_without_a_bucket() -> Vec<(&'static str, Value)> {
    vec![
        ("location omitted", json!({"type": "S3"})),
        ("location empty", json!({"type": "S3", "location": ""})),
        (
            "location all whitespace",
            json!({"type": "S3", "location": "  \n\t "}),
        ),
    ]
}

/// Artifacts that DO describe a destination a build could upload to: a bucket type that
/// names its bucket (with and without a prefix behind it), and each type that names no
/// destination of its own, from the model's whole `ArtifactsType` catalogue
/// (`smithy_query(query="shape", shape="ArtifactsType")` — `CODEPIPELINE`, `S3`,
/// `NO_ARTIFACTS`).
fn valid_destinations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a bucket named",
            json!({"type": "S3", "location": "artifacts-bucket"}),
        ),
        (
            "a bucket named with a prefix behind it",
            json!({"type": "S3", "location": "artifacts-bucket/out"}),
        ),
        ("no artifacts at all", json!({"type": "NO_ARTIFACTS"})),
        (
            "artifacts CodePipeline manages",
            json!({"type": "CODEPIPELINE"}),
        ),
    ]
}

/// RULE, on the operations that STORE a project's artifacts: artifacts bound for a bucket
/// must name one, as the primary artifacts and as a secondary entry alike, and a refused
/// request stores nothing.
#[tokio::test]
async fn a_project_cannot_store_artifacts_bound_for_a_bucket_it_does_not_name() {
    let router = emulator();
    create_project(&router, "artifacts-update", json!({})).await;

    for (case, artifacts) in destinations_without_a_bucket() {
        // (how the artifacts reach the project, the request)
        let requests: Vec<(&str, Value)> = vec![
            (
                "CreateProject primary",
                create_project_body("artifacts-create", json!({"artifacts": artifacts.clone()})),
            ),
            (
                "CreateProject secondary",
                create_project_body(
                    "artifacts-create",
                    json!({
                        "artifacts": {"type": "NO_ARTIFACTS"},
                        "secondaryArtifacts": as_secondary(&artifacts),
                    }),
                ),
            ),
            (
                "UpdateProject primary",
                json!({"name": "artifacts-update", "artifacts": artifacts.clone()}),
            ),
        ];
        for (operation_case, request) in requests {
            let operation = if operation_case.starts_with("CreateProject") {
                "CreateProject"
            } else {
                "UpdateProject"
            };
            let case = format!("{operation_case} with {case}");
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                message(&body),
                state::ARTIFACTS_LOCATION_REQUIRED_MESSAGE,
                "case {case:?}: expected CodeBuild's artifacts sentence, got {body}"
            );
        }
    }

    // Nothing was created by any refused CreateProject, and the project the refused
    // updates named still carries the artifacts it was created with.
    let (status, projects) = call(
        &router,
        "BatchGetProjects",
        json!({"names": ["artifacts-create", "artifacts-update"]}),
    )
    .await;
    assert_eq!(status, 200, "BatchGetProjects: got {status} with {projects}");
    assert_eq!(
        projects["projectsNotFound"],
        json!(["artifacts-create"]),
        "a refused CreateProject must store nothing, got {projects}"
    );
    assert_eq!(
        projects["projects"][0]["artifacts"]["type"],
        json!("NO_ARTIFACTS"),
        "a refused UpdateProject must leave the stored artifacts alone, got {projects}"
    );

    // The destinations that ARE valid are stored, on both paths.
    for (case, artifacts) in valid_destinations() {
        let name = format!("artifacts-ok-{}", case.replace(' ', "-"));
        let stored = create_project(&router, &name, json!({"artifacts": artifacts.clone()})).await;
        assert_eq!(
            stored["artifacts"]["type"], artifacts["type"],
            "case {case:?}: the stored artifacts must be the ones asked for, got {stored}"
        );
        let (status, updated) = call(
            &router,
            "UpdateProject",
            json!({"name": "artifacts-update", "artifacts": artifacts.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateProject expected 200, got {status} with {updated}"
        );
    }
}

/// RULE, on the operations that START work: an artifacts override bound for a bucket must
/// name one, and a refused start leaves no build — and no batch build — behind.
#[tokio::test]
async fn starting_work_with_artifacts_bound_for_an_unnamed_bucket_is_refused_and_starts_nothing() {
    let router = emulator();
    create_project(
        &router,
        "artifacts-start",
        merged(
            json!({"artifacts": {"type": "S3", "location": "artifacts-bucket"}}),
            batch_enabled(),
        ),
    )
    .await;

    for (case, artifacts) in destinations_without_a_bucket() {
        // (operation, the member the override arrives through, the request)
        let requests: Vec<(&str, Value)> = vec![
            (
                "StartBuild",
                json!({"projectName": "artifacts-start", "artifactsOverride": artifacts.clone()}),
            ),
            (
                "StartBuild",
                json!({
                    "projectName": "artifacts-start",
                    "secondaryArtifactsOverride": as_secondary(&artifacts),
                }),
            ),
            (
                "StartBuildBatch",
                json!({"projectName": "artifacts-start", "artifactsOverride": artifacts.clone()}),
            ),
        ];
        for (operation, request) in requests {
            let case = format!("{operation} with {case} in {request}");
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                message(&body),
                state::ARTIFACTS_LOCATION_REQUIRED_MESSAGE,
                "case {case:?}: expected CodeBuild's artifacts sentence, got {body}"
            );
        }
    }

    // EFFECT ABSENT: no build and no batch build was created by any refused start, so
    // neither a build id nor a build number was spent.
    let (_, builds) = call(
        &router,
        "ListBuildsForProject",
        json!({"projectName": "artifacts-start"}),
    )
    .await;
    assert!(
        listed_ids(&builds).is_empty(),
        "a refused StartBuild must leave no build behind, got {builds}"
    );
    let (_, batches) = call(
        &router,
        "ListBuildBatchesForProject",
        json!({"projectName": "artifacts-start"}),
    )
    .await;
    assert!(
        listed_ids(&batches).is_empty(),
        "a refused StartBuildBatch must leave no batch build behind, got {batches}"
    );

    // The control: the same calls with a destination that names its bucket DO start work,
    // and the build reports the artifacts it was started with.
    for (case, artifacts) in valid_destinations() {
        let (status, body) = call(
            &router,
            "StartBuild",
            json!({"projectName": "artifacts-start", "artifactsOverride": artifacts.clone()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartBuild expected 200, got {status} with {body}"
        );
        assert!(
            body["build"]["id"].is_string(),
            "case {case:?}: an accepted StartBuild mints a build, got {body}"
        );
    }
}
