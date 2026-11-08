// SPDX-License-Identifier: CC0-1.0

//! What a pipeline whose source is an Amazon ECR image, or whose build is an AWS CodeBuild
//! project, DOES — end to end over the wire each service speaks, with CodePipeline, Amazon
//! ECR, CodeBuild and Amazon S3 mounted on one engine, so the execution plane reaches each
//! of them the way the assembled emulator does.
//!
//! The rules under test, each asserted over a SET of inputs rather than one case:
//!
//! * a `Source` action whose provider is `ECR` resolves the image its configuration names
//!   — the repository, at the tag it states or at the default the action documents — and
//!   produces the `imageDetail.json` document of THAT image as its output artifact: the
//!   image's digest is the revision the execution ran and the external execution the action
//!   reports, and the document carries the image's own digest, repository, registry, tags
//!   and URI;
//! * an action whose provider is `CodeBuild` performs its work by STARTING a build of the
//!   project its configuration names, with the environment variables it declares, and
//!   settles from that build's own terminal status — so the action reports the build as its
//!   external execution and never decides the outcome itself;
//! * a declaration either of them cannot be run as is a `ConfigurationError`: the action
//!   fails with a summary saying what is missing or unresolvable, produces no artifact and
//!   starts nothing in the owning service.

use std::io::Read as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for CodePipeline.
const PIPELINE_TARGET: &str = "CodePipeline_20150709";

/// The `X-Amz-Target` prefix Amazon ECR's OPERATIONS.md records.
const ECR_TARGET: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The `X-Amz-Target` prefix CodeBuild's OPERATIONS.md records.
const CODEBUILD_TARGET: &str = "CodeBuild_20161006";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern. IAM is not mounted here, so no role
/// lookup has an answer and the declarations are accepted (only a definitive "no such role"
/// refuses one).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// The Region every call of this suite is signed for, which is the Region the image URI the
/// source action publishes carries.
const REGION: &str = "us-east-1";

/// The account id every ARN and registry id this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a test waits for the clock-owed steps of one execution.
const WAIT: Duration = Duration::from_secs(20);

/// The `FailureType` an action carries when its declaration cannot be executed at all.
const CONFIGURATION_ERROR: &str = "ConfigurationError";

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 5] = [
    "Succeeded",
    "Failed",
    "Stopped",
    "Superseded",
    "Cancelled",
];

/// One emulator: the shared engine with CodePipeline, Amazon ECR, CodeBuild and S3 on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One awsJson1_1 call to the service `signing_name` claims, under `target`.
    async fn json_call(
        &self,
        signing_name: &str,
        target: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// One CodePipeline operation that must succeed.
    async fn cp_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self
            .json_call("codepipeline", PIPELINE_TARGET, operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// One Amazon ECR operation that must succeed.
    async fn ecr_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.json_call("ecr", ECR_TARGET, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// One CodeBuild operation.
    async fn codebuild(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.json_call("codebuild", CODEBUILD_TARGET, operation, body)
            .await
    }

    /// One S3 request, path style, the way the emulated S3 service routes it.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (parts.status.as_u16(), body.to_vec())
    }

    /// Create a bucket.
    async fn bucket(&self, name: &str) {
        let (status, body) = self.s3("PUT", &format!("/{name}"), b"").await;
        assert_eq!(
            status,
            200,
            "the bucket {name:?} must be created, got {status} with {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Read an object: its status and bytes.
    async fn object(&self, bucket: &str, key: &str) -> (u16, Vec<u8>) {
        self.s3("GET", &format!("/{bucket}/{key}"), b"").await
    }

    /// Push one image manifest into `repository` under `tag`, and answer its digest.
    async fn push(&self, repository: &str, manifest: &str, tag: &str) -> String {
        let pushed = self
            .ecr_ok(
                "PutImage",
                &json!({
                    "repositoryName": repository,
                    "imageManifest": manifest,
                    "imageTag": tag,
                }),
            )
            .await;
        pushed["image"]["imageId"]["imageDigest"]
            .as_str()
            .unwrap_or_else(|| panic!("the push reports the digest it stored, got {pushed}"))
            .to_string()
    }
}

/// A `Source` stage reading the image of `repository`, at `tag` when the row states one,
/// producing `artifact`.
fn ecr_source(repository: &str, tag: Option<&str>, artifact: &str) -> Value {
    let mut configuration = json!({ "RepositoryName": repository });
    if let Some(tag) = tag {
        configuration["ImageTag"] = Value::String(tag.to_string());
    }
    json!({
        "name": "Source",
        "actions": [{
            "name": "Image",
            "actionTypeId": {
                "category": "Source", "owner": "AWS", "provider": "ECR", "version": "1",
            },
            "configuration": configuration,
            "outputArtifacts": [{"name": artifact}],
            "namespace": "SourceVariables",
        }],
    })
}

/// A `Build` stage whose action builds `project`, with the environment variables `declared`
/// when the row states any.
fn codebuild_build(project: &str, artifact: &str, declared: Option<&str>) -> Value {
    let mut configuration = json!({ "ProjectName": project });
    if let Some(declared) = declared {
        configuration["EnvironmentVariables"] = Value::String(declared.to_string());
    }
    json!({
        "name": "Build",
        "actions": [{
            "name": "Build",
            "actionTypeId": {
                "category": "Build", "owner": "AWS", "provider": "CodeBuild", "version": "1",
            },
            "configuration": configuration,
            "inputArtifacts": [{"name": artifact}],
        }],
    })
}

/// A `Deploy` stage delivering `artifact` to `bucket` under `key`, so the bytes the source
/// action produced are readable as one object.
fn s3_deploy(artifact: &str, bucket: &str, key: &str) -> Value {
    json!({
        "name": "Deploy",
        "actions": [{
            "name": "Dep",
            "actionTypeId": {
                "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1",
            },
            "configuration": {"BucketName": bucket, "ObjectKey": key, "Extract": "false"},
            "inputArtifacts": [{"name": artifact}],
        }],
    })
}

/// A manual-approval stage: the non-`Source` stage a declaration has to carry when the row
/// under test delivers nothing.
fn approval_gate() -> Value {
    json!({
        "name": "Gate",
        "actions": [{
            "name": "Approve",
            "actionTypeId": {
                "category": "Approval", "owner": "AWS", "provider": "Manual", "version": "1",
            },
        }],
    })
}

/// A pipeline declaration named `name`, writing its artifacts to `artifact_store`.
fn declaration(name: &str, artifact_store: &str, stages: Vec<Value>) -> Value {
    json!({"pipeline": {
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": artifact_store},
        "stages": stages,
    }})
}

/// A manifest whose bytes — and so whose digest — are the case's own.
fn manifest_of(marker: &str) -> String {
    json!({"schemaVersion": 2, "marker": marker}).to_string()
}

/// Poll until the pipeline's newest execution reaches a terminal status, and answer its
/// summary.
async fn wait_for_terminal(emulator: &Emulator, pipeline: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let listed = emulator
            .cp_ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
            .await;
        let newest = listed["pipelineExecutionSummaries"]
            .as_array()
            .and_then(|summaries| summaries.first())
            .cloned()
            .unwrap_or(Value::Null);
        if TERMINAL.contains(&newest["status"].as_str().unwrap_or_default()) {
            return newest;
        }
        assert!(
            Instant::now() < deadline,
            "the newest execution of {pipeline} never settled, last {newest}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The state of one action of one stage, as `GetPipelineState` reports it.
async fn action_state(emulator: &Emulator, pipeline: &str, stage: &str, action: &str) -> Value {
    let state = emulator.cp_ok("GetPipelineState", &json!({"name": pipeline})).await;
    state["stageStates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|state| state["stageName"].as_str() == Some(stage))
        .and_then(|state| {
            state["actionStates"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|state| state["actionName"].as_str() == Some(action))
        })
        .unwrap_or_else(|| panic!("no state for {stage}/{action} of {pipeline}"))
}

/// The one entry of the archive `bytes` holds, as the text of `path`.
fn archived(bytes: &[u8], path: &str) -> String {
    let mut archive = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes.to_vec()))
        .unwrap_or_else(|error| panic!("the artifact must be an archive: {error}"));
    let mut entry = archive
        .by_name(path)
        .unwrap_or_else(|error| panic!("the archive must carry {path}: {error}"));
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .unwrap_or_else(|error| panic!("the entry {path} must be readable: {error}"));
    text
}

/// One row of the source table: the tag the declaration states (or none, for the default
/// the action documents), and the tag the image is pushed under.
struct SourceRow {
    case: &'static str,
    declared_tag: Option<&'static str>,
    pushed_tag: &'static str,
}

const SOURCE_ROWS: &[SourceRow] = &[
    SourceRow {
        case: "the tag the declaration states",
        declared_tag: Some("release"),
        pushed_tag: "release",
    },
    SourceRow {
        case: "a tag of another form, stating it as well",
        declared_tag: Some("2026.01.1-rc"),
        pushed_tag: "2026.01.1-rc",
    },
    SourceRow {
        // "ImageTag … if not specified, the value defaults to `latest`".
        case: "no tag at all, which sources the default the action documents",
        declared_tag: None,
        pushed_tag: "latest",
    },
];

/// Every row: the source action resolves the image the declaration names and produces that
/// image's own detail as its output artifact, with the image's digest as the revision.
#[tokio::test]
async fn an_ecr_source_action_produces_the_detail_of_the_image_it_resolved() {
    let emulator = Emulator::new();
    emulator.bucket("ecr-source-artifacts").await;

    for (index, row) in SOURCE_ROWS.iter().enumerate() {
        let case = row.case;
        let repository = format!("images-{index}");
        let pipeline = format!("ecr-source-{index}");
        let delivered = format!("detail-{index}.zip");
        emulator
            .ecr_ok("CreateRepository", &json!({"repositoryName": repository}))
            .await;
        // Two images in the repository, so the one the action sources is the one the TAG
        // names rather than whichever the registry happens to hold.
        emulator
            .push(&repository, &manifest_of(&format!("other-{index}")), "other")
            .await;
        let digest = emulator
            .push(
                &repository,
                &manifest_of(&format!("sourced-{index}")),
                row.pushed_tag,
            )
            .await;

        emulator
            .cp_ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    "ecr-source-artifacts",
                    vec![
                        ecr_source(&repository, row.declared_tag, "ImageArt"),
                        s3_deploy("ImageArt", "ecr-source-artifacts", &delivered),
                    ],
                ),
            )
            .await;

        let summary = wait_for_terminal(&emulator, &pipeline).await;
        assert_eq!(
            summary["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the execution must run both stages, got {summary}"
        );
        assert_eq!(
            summary["sourceRevisions"]
                .as_array()
                .and_then(|revisions| revisions.first())
                .and_then(|revision| revision["revisionId"].as_str()),
            Some(digest.as_str()),
            "case {case:?}: the revision the execution ran must be the image's digest \
             ({digest}), got {summary}"
        );

        let state = action_state(&emulator, &pipeline, "Source", "Image").await;
        assert_eq!(
            state["latestExecution"]["externalExecutionId"].as_str(),
            Some(digest.as_str()),
            "case {case:?}: the action must report the image it resolved as its external \
             execution, got {state}"
        );

        // The artifact the action produced, as the deploy action delivered it: the
        // `imageDetail.json` document of the image the registry holds.
        let (status, bytes) = emulator.object("ecr-source-artifacts", &delivered).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the artifact must have been delivered, got {status}"
        );
        let document: Value = serde_json::from_str(&archived(&bytes, "imageDetail.json"))
            .unwrap_or_else(|error| panic!("case {case:?}: imageDetail.json is JSON: {error}"));
        assert_eq!(
            document["ImageDigest"].as_str(),
            Some(digest.as_str()),
            "case {case:?}: the document names the image's own digest, got {document}"
        );
        assert_eq!(
            document["RepositoryName"].as_str(),
            Some(repository.as_str()),
            "case {case:?}: the document names the repository it came from, got {document}"
        );
        assert_eq!(
            document["RegistryId"].as_str(),
            Some(ACCOUNT),
            "case {case:?}: the document names the registry holding it, got {document}"
        );
        assert_eq!(
            document["ImageURI"].as_str(),
            Some(
                format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/{repository}@{digest}").as_str()
            ),
            "case {case:?}: the document's URI addresses the image by digest, got {document}"
        );
        assert_eq!(
            document["ImageTags"],
            json!([row.pushed_tag]),
            "case {case:?}: the document carries the image's own tags, got {document}"
        );
        assert_eq!(
            document["Version"].as_str(),
            Some("1.0"),
            "case {case:?}: the document states the file's version, got {document}"
        );
        assert!(
            document["ImagePushedAt"].is_string(),
            "case {case:?}: the document states when the image was pushed, got {document}"
        );

        // And the action published the image as its output variables, for the actions
        // after it to resolve.
        let variables = summary["pipelineExecutionId"].as_str().map(|id| {
            json!({ "pipelineName": pipeline, "pipelineExecutionId": id })
        });
        if let Some(request) = variables {
            let execution = emulator.cp_ok("GetPipelineExecution", &request).await;
            assert_eq!(
                execution["pipelineExecution"]["artifactRevisions"]
                    .as_array()
                    .and_then(|revisions| revisions.first())
                    .and_then(|revision| revision["revisionId"].as_str()),
                Some(digest.as_str()),
                "case {case:?}: the artifact's revision is the image's digest, got {execution}"
            );
        }
    }
}

/// A `Build` action STARTS a build of the project it names, with the environment variables
/// it declares, and settles from that build's own terminal status — the action decides
/// nothing itself.
#[tokio::test]
async fn a_codebuild_action_starts_the_build_it_settles_from() {
    let emulator = Emulator::new();
    emulator.bucket("build-artifacts").await;
    emulator
        .ecr_ok("CreateRepository", &json!({"repositoryName": "built"}))
        .await;
    let digest = emulator
        .push("built", &manifest_of("built"), "latest")
        .await;
    assert!(digest.starts_with("sha256:"), "the push stored an image");

    let (status, created) = emulator
        .codebuild(
            "CreateProject",
            &json!({
                "name": "pipeline-build",
                "serviceRole": ROLE_ARN,
                "source": {"type": "CODEPIPELINE", "buildspec": "version: 0.2"},
                "artifacts": {"type": "CODEPIPELINE"},
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/standard:7.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateProject must succeed, got {status} with {created}"
    );

    let declared = json!([{"name": "IMAGE_TAG", "value": "latest", "type": "PLAINTEXT"}])
        .to_string();
    emulator
        .cp_ok(
            "CreatePipeline",
            &declaration(
                "build-pipeline",
                "build-artifacts",
                vec![
                    ecr_source("built", Some("latest"), "ImageArt"),
                    codebuild_build("pipeline-build", "ImageArt", Some(&declared)),
                ],
            ),
        )
        .await;

    wait_for_terminal(&emulator, "build-pipeline").await;

    // CodeBuild holds exactly one build of the project, which is the one the action
    // reports as its external execution.
    let (status, listed) = emulator
        .codebuild(
            "ListBuildsForProject",
            &json!({"projectName": "pipeline-build"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ListBuildsForProject must succeed, got {status} with {listed}"
    );
    let builds = listed["ids"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        builds.len(),
        1,
        "the action must have started one build of the project, got {listed}"
    );
    let build_id = builds[0].as_str().unwrap_or_default().to_string();

    let state = action_state(&emulator, "build-pipeline", "Build", "Build").await;
    let latest = &state["latestExecution"];
    assert_eq!(
        latest["externalExecutionId"].as_str(),
        Some(build_id.as_str()),
        "the action must report the build it started, got {state}"
    );

    // What the build reported is what the action reports: the status, and the state the
    // build ended in as the action's detail.
    let (status, read) = emulator
        .codebuild("BatchGetBuilds", &json!({"ids": [build_id]}))
        .await;
    assert_eq!(
        status, 200,
        "BatchGetBuilds must succeed, got {status} with {read}"
    );
    let build = &read["builds"][0];
    assert_eq!(
        build["projectName"].as_str(),
        Some("pipeline-build"),
        "the build must be a build of the project the action named, got {build}"
    );
    let variables = build["environment"]["environmentVariables"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        variables.iter().any(|variable| {
            variable["name"].as_str() == Some("IMAGE_TAG")
                && variable["value"].as_str() == Some("latest")
        }),
        "the action's declared environment variables must reach the build, got {variables:?}"
    );
    let build_status = build["buildStatus"].as_str().unwrap_or_default();
    if build_status == "SUCCEEDED" {
        assert_eq!(
            latest["status"].as_str(),
            Some("Succeeded"),
            "a build that succeeded settles the action Succeeded, got {state}"
        );
    } else {
        assert_eq!(
            latest["status"].as_str(),
            Some("Failed"),
            "a build that did not succeed settles the action Failed, got {state}"
        );
        assert_eq!(
            latest["errorDetails"]["message"].as_str(),
            Some(format!("Build terminated with state: {build_status}").as_str()),
            "the action's detail is the state the build ended in, got {state}"
        );
    }
}

/// One row of the refusal table: the stages a pipeline declares, and the action whose
/// declaration cannot be run.
struct RefusalRow {
    case: &'static str,
    stages: fn() -> Vec<Value>,
    stage: &'static str,
    action: &'static str,
}

/// A declaration either action kind cannot be run as fails that action with the
/// `ConfigurationError` failure type, and starts nothing in the owning service.
///
/// The rows differ along WHAT cannot be run: a source naming no repository, a source that
/// produces no artifact, a source whose repository the registry does not hold, a source
/// whose tag it holds no image for, a build naming no project, and a build whose
/// environment variables are not the document that configuration carries.
#[tokio::test]
async fn a_declaration_that_cannot_be_run_fails_its_action_as_a_configuration_error() {
    let rows: &[RefusalRow] = &[
        RefusalRow {
            case: "a source action naming no repository",
            stages: || {
                vec![
                    json!({
                        "name": "Source",
                        "actions": [{
                            "name": "Image",
                            "actionTypeId": {
                                "category": "Source", "owner": "AWS", "provider": "ECR",
                                "version": "1",
                            },
                            "configuration": {"ImageTag": "latest"},
                            "outputArtifacts": [{"name": "ImageArt"}],
                        }],
                    }),
                    approval_gate(),
                ]
            },
            stage: "Source",
            action: "Image",
        },
        RefusalRow {
            case: "a source action that produces no artifact",
            stages: || {
                vec![
                    json!({
                        "name": "Source",
                        "actions": [{
                            "name": "Image",
                            "actionTypeId": {
                                "category": "Source", "owner": "AWS", "provider": "ECR",
                                "version": "1",
                            },
                            "configuration": {"RepositoryName": "refused", "ImageTag": "latest"},
                        }],
                    }),
                    approval_gate(),
                ]
            },
            stage: "Source",
            action: "Image",
        },
        RefusalRow {
            case: "a source action naming a repository the registry does not hold",
            stages: || {
                vec![
                    ecr_source("never-created", Some("latest"), "ImageArt"),
                    approval_gate(),
                ]
            },
            stage: "Source",
            action: "Image",
        },
        RefusalRow {
            case: "a source action naming a tag the repository holds no image for",
            stages: || {
                vec![
                    ecr_source("refused", Some("never-pushed"), "ImageArt"),
                    approval_gate(),
                ]
            },
            stage: "Source",
            action: "Image",
        },
        RefusalRow {
            case: "a build action naming no project",
            stages: || {
                vec![
                    ecr_source("refused", Some("latest"), "ImageArt"),
                    json!({
                        "name": "Build",
                        "actions": [{
                            "name": "Build",
                            "actionTypeId": {
                                "category": "Build", "owner": "AWS", "provider": "CodeBuild",
                                "version": "1",
                            },
                            "configuration": {"PrimarySource": "ImageArt"},
                            "inputArtifacts": [{"name": "ImageArt"}],
                        }],
                    }),
                ]
            },
            stage: "Build",
            action: "Build",
        },
        RefusalRow {
            case: "a build action whose environment variables are not that document",
            stages: || {
                vec![
                    ecr_source("refused", Some("latest"), "ImageArt"),
                    codebuild_build("never-created", "ImageArt", Some("IMAGE_TAG=latest")),
                ]
            },
            stage: "Build",
            action: "Build",
        },
        RefusalRow {
            case: "a build action naming a project the account does not hold",
            stages: || {
                vec![
                    ecr_source("refused", Some("latest"), "ImageArt"),
                    codebuild_build("never-created", "ImageArt", None),
                ]
            },
            stage: "Build",
            action: "Build",
        },
    ];

    let emulator = Emulator::new();
    emulator.bucket("refused-artifacts").await;
    // The repository the rows that get as far as the build stage source from, with one
    // image at the tag they name.
    emulator
        .ecr_ok("CreateRepository", &json!({"repositoryName": "refused"}))
        .await;
    emulator
        .push("refused", &manifest_of("refused"), "latest")
        .await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let pipeline = format!("refused-{index}");
        emulator
            .cp_ok(
                "CreatePipeline",
                &declaration(&pipeline, "refused-artifacts", (row.stages)()),
            )
            .await;

        let summary = wait_for_terminal(&emulator, &pipeline).await;
        assert_eq!(
            summary["status"].as_str(),
            Some("Failed"),
            "case {case:?}: a declaration that cannot be run fails the execution, got \
             {summary}"
        );
        let state = action_state(&emulator, &pipeline, row.stage, row.action).await;
        let latest = &state["latestExecution"];
        assert_eq!(
            latest["status"].as_str(),
            Some("Failed"),
            "case {case:?}: the action itself must fail, got {state}"
        );
        assert_eq!(
            latest["errorDetails"]["code"].as_str(),
            Some(CONFIGURATION_ERROR),
            "case {case:?}: a declaration that cannot be run is a configuration error, got \
             {state}"
        );
        let detail = latest["errorDetails"]["message"].as_str().unwrap_or_default();
        assert!(
            !detail.is_empty(),
            "case {case:?}: the failure must say why, got {state}"
        );
    }

    // Nothing was started in CodeBuild for a project the account does not hold: the
    // service still has no such project.
    let (status, listed) = emulator
        .codebuild(
            "ListBuildsForProject",
            &json!({"projectName": "never-created"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "CodeBuild must still hold no such project, got {status} with {listed}"
    );
}
