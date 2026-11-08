// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CodeBuild::Project` resources its template declares — in CodeBuild, with the
//! configuration the declaration carries.
//!
//! The rule under test: **a stack reports a resource as created only where the service
//! that owns it created it, through that service's own public API and with every declared
//! property applied, so the handle the stack publishes addresses a resource that service
//! really holds — and deleting the stack releases it there.** For a build project the
//! handle is its NAME ("Ref returns the project name") and its `Arn` attribute, so right
//! after the create waiter returns `BatchGetProjects` answers with the project — rather
//! than reporting the name of its own completed stack in `projectsNotFound` — and
//! `StartBuild` against it starts a build, instead of refusing a project that the stack
//! says exists. Two crates have to agree for that to hold (CloudFormation makes the calls,
//! CodeBuild serves them), so the suite lives here: a crate-local test can only ever see
//! one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! project or leaves the name to CloudFormation, whether it is written in JSON or YAML,
//! which source and artifacts types it declares, whether it carries a buildspec, an
//! environment variable, a cache, a timeout and tags, and the Region the stack is deployed
//! in.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the deployment it completed. A build project has no transient
/// status of its own: `CreateProject` answers with one that is usable at once.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The bounds CodeBuild holds a project's name to (`ProjectName`,
/// `@length(min: 2, max: 150)` in that service's own model), which a GENERATED name is
/// held to as well.
const MIN_PROJECT_NAME: usize = 2;
const MAX_PROJECT_NAME: usize = 150;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`,
/// which is what routes the request and what each handler reads its Region from.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CodeBuild operation over its awsJson1_1 wire, signed for `region`.
async fn codebuild(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("codebuild.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header("authorization", authorization("codebuild", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a build project of `name` carries in `region`: the same total function of the
/// Region, the account and the name CodeBuild itself keys a project by.
fn project_arn(region: &str, name: &str) -> String {
    format!("arn:aws:codebuild:{region}:{ACCOUNT}:project/{name}")
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The project CodeBuild holds under `name`, or `None` where it reports the name in
/// `projectsNotFound` — the answer's own partition, read the way a client reads it.
async fn held_project(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (Option<Value>, Vec<String>) {
    let (status, answer) = codebuild(
        router,
        region,
        "BatchGetProjects",
        &json!({ "names": [name] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected HTTP 200, got {status} with body {answer}"
    );
    let held = answer
        .get("projects")
        .and_then(Value::as_array)
        .and_then(|projects| projects.first())
        .cloned();
    let not_found = answer
        .get("projectsNotFound")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (held, not_found)
}

/// One row of the table: a template, and what the rule says the project it declares must
/// then be in CodeBuild.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the project is created in and
    /// the one its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: &'static str,
    /// The name the template declares, or `None` for a template that names nothing —
    /// whose project carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `source` `BatchGetProjects` must report, built from what the row declares.
    expected_source: fn() -> Value,
    /// The `artifacts.type` it must report.
    expected_artifacts_type: &'static str,
    /// The `environment.image` it must report.
    expected_image: &'static str,
    /// The `timeoutInMinutes` the row declares, or `None` where it declares none and
    /// CodeBuild's own default stands.
    declared_timeout: Option<i64>,
    /// The tags the project's own declaration asks for.
    expected_tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named CODEPIPELINE project in JSON with an inline buildspec and tags",
        region: "us-west-2",
        template: r#"{
            "AWSTemplateFormatVersion": "2010-09-09",
            "Description": "a CodeBuild project driven by a pipeline",
            "Resources": {
                "Build": {
                    "Type": "AWS::CodeBuild::Project",
                    "Properties": {
                        "Name": "orders-image-build",
                        "Description": "builds the orders image",
                        "ServiceRole": "arn:aws:iam::000000000000:role/orders-codebuild",
                        "Source": {
                            "Type": "CODEPIPELINE",
                            "BuildSpec": "version: 0.2\nphases:\n  build:\n    commands:\n      - make\n"
                        },
                        "Artifacts": { "Type": "CODEPIPELINE" },
                        "Environment": {
                            "Type": "LINUX_CONTAINER",
                            "Image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                            "ComputeType": "BUILD_GENERAL1_SMALL",
                            "PrivilegedMode": true,
                            "EnvironmentVariables": [
                                { "Name": "REPO", "Value": "orders", "Type": "PLAINTEXT" }
                            ]
                        },
                        "TimeoutInMinutes": 30,
                        "Tags": [
                            { "Key": "Owner", "Value": "orders" },
                            { "Key": "Tier", "Value": "prod" }
                        ]
                    }
                }
            },
            "Outputs": {
                "ProjectRef": { "Value": { "Ref": "Build" } },
                "ProjectArn": { "Value": { "Fn::GetAtt": ["Build", "Arn"] } }
            }
        }"#,
        declared_name: Some("orders-image-build"),
        expected_source: || {
            json!({
                "type": "CODEPIPELINE",
                "buildspec": "version: 0.2\nphases:\n  build:\n    commands:\n      - make\n",
                "insecureSsl": false,
            })
        },
        expected_artifacts_type: "CODEPIPELINE",
        expected_image: "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
        declared_timeout: Some(30),
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
    },
    Row {
        case: "a named NO_SOURCE project in YAML writing its artifacts to S3",
        region: "us-east-1",
        template: "Resources:\n  \
                   Build:\n    \
                   Type: AWS::CodeBuild::Project\n    \
                   Properties:\n      \
                   Name: nightly-report\n      \
                   ServiceRole: arn:aws:iam::000000000000:role/nightly-codebuild\n      \
                   Source:\n        \
                   Type: NO_SOURCE\n        \
                   BuildSpec: \"version: 0.2\"\n      \
                   Artifacts:\n        \
                   Type: S3\n        \
                   Location: nightly-artifacts\n        \
                   Name: report.zip\n        \
                   Packaging: ZIP\n      \
                   Environment:\n        \
                   Type: ARM_CONTAINER\n        \
                   Image: aws/codebuild/amazonlinux2-aarch64-standard:3.0\n        \
                   ComputeType: BUILD_GENERAL1_LARGE\n      \
                   Cache:\n        \
                   Type: NO_CACHE\n\
                   Outputs:\n  \
                   ProjectRef:\n    \
                   Value: !Ref Build\n  \
                   ProjectArn:\n    \
                   Value: !GetAtt Build.Arn\n",
        declared_name: Some("nightly-report"),
        expected_source: || {
            json!({
                "type": "NO_SOURCE",
                "buildspec": "version: 0.2",
                "insecureSsl": false,
            })
        },
        expected_artifacts_type: "S3",
        expected_image: "aws/codebuild/amazonlinux2-aarch64-standard:3.0",
        declared_timeout: None,
        expected_tags: &[],
    },
    Row {
        case: "a project the template does not name",
        region: "eu-west-1",
        template: r#"{
            "Resources": {
                "Build": {
                    "Type": "AWS::CodeBuild::Project",
                    "Properties": {
                        "ServiceRole": "arn:aws:iam::000000000000:role/unnamed-codebuild",
                        "Source": { "Type": "NO_SOURCE", "BuildSpec": "version: 0.2" },
                        "Artifacts": { "Type": "NO_ARTIFACTS" },
                        "Environment": {
                            "Type": "LINUX_CONTAINER",
                            "Image": "aws/codebuild/standard:7.0",
                            "ComputeType": "BUILD_GENERAL1_MEDIUM"
                        },
                        "Tags": { "Owner": "platform" }
                    }
                }
            },
            "Outputs": {
                "ProjectRef": { "Value": { "Ref": "Build" } },
                "ProjectArn": { "Value": { "Fn::GetAtt": ["Build", "Arn"] } }
            }
        }"#,
        declared_name: None,
        expected_source: || {
            json!({
                "type": "NO_SOURCE",
                "buildspec": "version: 0.2",
                "insecureSsl": false,
            })
        },
        expected_artifacts_type: "NO_ARTIFACTS",
        expected_image: "aws/codebuild/standard:7.0",
        declared_timeout: None,
        expected_tags: &[("Owner", "platform")],
    },
];

/// Every row: the name the stack reports for its `AWS::CodeBuild::Project` is a project
/// CodeBuild really holds, configured as the template declared it; `StartBuild` on the
/// project the stack published starts a build of it; and deleting the stack releases the
/// project there.
#[tokio::test]
async fn a_stack_provisions_the_codebuild_project_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("codebuild-project-stack-{index}");

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", row.template),
                ("Tags.member.1.Key", "Stack"),
                ("Tags.member.1.Value", "codebuild-project-suite"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));

        // "Ref returns the project name": the physical id IS the name the template asked
        // for, or the one CloudFormation generated for a template that named nothing —
        // which is held to the name CodeBuild accepts.
        if let Some(declared) = row.declared_name {
            assert_eq!(
                physical_id, declared,
                "case {case:?}: expected the physical id to be the declared name \
                 {declared:?}, got {physical_id:?}"
            );
        }
        let name_length = physical_id.chars().count();
        assert!(
            (MIN_PROJECT_NAME..=MAX_PROJECT_NAME).contains(&name_length),
            "case {case:?}: expected a name inside CodeBuild's own ProjectName rule, got \
             {physical_id:?}"
        );

        let arn = project_arn(region, &physical_id);
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("ProjectRef"),
            Some(&physical_id),
            "case {case:?}: expected Ref to be the project name {physical_id:?}, got {:?}",
            outputs.get("ProjectRef")
        );
        assert_eq!(
            outputs.get("ProjectArn"),
            Some(&arn),
            "case {case:?}: expected Fn::GetAtt Arn to be {arn}, got {:?}",
            outputs.get("ProjectArn")
        );

        // And CodeBuild itself holds the project the template declared.
        let (held, not_found) = held_project(&router, region, &physical_id).await;
        assert!(
            not_found.is_empty(),
            "case {case:?}: expected the stack's own project to be found, got \
             projectsNotFound {not_found:?}"
        );
        let held = held.unwrap_or_else(|| {
            panic!("case {case:?}: expected BatchGetProjects to report the project {physical_id}")
        });
        assert_eq!(
            held.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the project's ARN to be {arn}, got {held}"
        );
        assert_eq!(
            held.get("name").and_then(Value::as_str),
            Some(physical_id.as_str()),
            "case {case:?}: expected the project to carry the stack's name, got {held}"
        );
        let expected_source = (row.expected_source)();
        assert_eq!(
            held.get("source"),
            Some(&expected_source),
            "case {case:?}: expected the declared source {expected_source}, got {held}"
        );
        assert_eq!(
            held.get("artifacts")
                .and_then(|artifacts| artifacts.get("type"))
                .and_then(Value::as_str),
            Some(row.expected_artifacts_type),
            "case {case:?}: expected the declared artifacts type, got {held}"
        );
        assert_eq!(
            held.get("environment")
                .and_then(|environment| environment.get("image"))
                .and_then(Value::as_str),
            Some(row.expected_image),
            "case {case:?}: expected the declared environment image, got {held}"
        );
        if let Some(timeout) = row.declared_timeout {
            assert_eq!(
                held.get("timeoutInMinutes").and_then(Value::as_i64),
                Some(timeout),
                "case {case:?}: expected the declared timeout {timeout}, got {held}"
            );
        }

        // The tags the declaration asked for are on the project, beside the stack's own
        // and the three CloudFormation records itself.
        let reported: BTreeMap<String, String> = held
            .get("tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'tags' list in {held}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("key")?.as_str()?.to_string(),
                    tag.get("value")?.as_str()?.to_string(),
                ))
            })
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get("Stack").map(String::as_str),
            Some("codebuild-project-suite"),
            "case {case:?}: expected the stack's own tag to be propagated, got {reported:?}"
        );
        assert_eq!(
            reported
                .get("aws:cloudformation:logical-id")
                .map(String::as_str),
            Some("Build"),
            "case {case:?}: expected CloudFormation's own record of the logical id, got \
             {reported:?}"
        );

        // What the stack provisioned takes the calls a client makes on a project of
        // CodeBuild's own: a build of it starts, and the build names that project.
        let (status, build) = codebuild(
            &router,
            region,
            "StartBuild",
            &json!({ "projectName": physical_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartBuild on the stack's own project, got \
             {status} with body {build}"
        );
        assert_eq!(
            build
                .get("build")
                .and_then(|build| build.get("projectName"))
                .and_then(Value::as_str),
            Some(physical_id.as_str()),
            "case {case:?}: expected the build to name the stack's project, got {build}"
        );

        // Deleting the stack releases the project CodeBuild holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (held, not_found) = held_project(&router, region, &physical_id).await;
        assert!(
            held.is_none() && not_found == vec![physical_id.clone()],
            "case {case:?}: expected the project to be gone once the stack deleted it, got \
             {held:?} with projectsNotFound {not_found:?}"
        );
    }
}

/// A declaration CodeBuild REFUSES does not report a resource nothing created: the stack
/// fails with that service's own refusal, and the project the declaration named is not in
/// CodeBuild afterwards.
///
/// The rows differ along WHAT is refused: a name a client already holds, a source that
/// needs a location and states none, and an environment type the model does not declare.
#[tokio::test]
async fn a_project_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    // The project a client already holds, which the first row re-declares.
    let (status, created) = codebuild(
        &router,
        region,
        "CreateProject",
        &json!({
            "name": "already-built",
            "serviceRole": "arn:aws:iam::000000000000:role/client-codebuild",
            "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
            "artifacts": {"type": "NO_ARTIFACTS"},
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
        "expected the client's own CreateProject to succeed, got {status} with body {created}"
    );

    // (case, the resource's declared properties, the name the declaration asks for, and
    // whether a project of that name must still be held once the stack has failed — true
    // only where a client created it)
    let rows: Vec<(&str, Value, &str, bool)> = vec![
        (
            "a project name a client already holds",
            json!({
                "Name": "already-built",
                "ServiceRole": "arn:aws:iam::000000000000:role/stack-codebuild",
                "Source": {"Type": "NO_SOURCE", "BuildSpec": "version: 0.2"},
                "Artifacts": {"Type": "NO_ARTIFACTS"},
                "Environment": {
                    "Type": "LINUX_CONTAINER",
                    "Image": "aws/codebuild/standard:7.0",
                    "ComputeType": "BUILD_GENERAL1_SMALL",
                },
            }),
            "already-built",
            true,
        ),
        (
            "a GitHub source that states no location to clone",
            json!({
                "Name": "sources-nowhere",
                "ServiceRole": "arn:aws:iam::000000000000:role/stack-codebuild",
                "Source": {"Type": "GITHUB"},
                "Artifacts": {"Type": "NO_ARTIFACTS"},
                "Environment": {
                    "Type": "LINUX_CONTAINER",
                    "Image": "aws/codebuild/standard:7.0",
                    "ComputeType": "BUILD_GENERAL1_SMALL",
                },
            }),
            "sources-nowhere",
            false,
        ),
        (
            "an environment type the model does not declare",
            json!({
                "Name": "not-an-environment",
                "ServiceRole": "arn:aws:iam::000000000000:role/stack-codebuild",
                "Source": {"Type": "NO_SOURCE", "BuildSpec": "version: 0.2"},
                "Artifacts": {"Type": "NO_ARTIFACTS"},
                "Environment": {
                    "Type": "TOASTER_CONTAINER",
                    "Image": "aws/codebuild/standard:7.0",
                    "ComputeType": "BUILD_GENERAL1_SMALL",
                },
            }),
            "not-an-environment",
            false,
        ),
    ];

    for (index, (case, properties, name, held_by_a_client)) in rows.into_iter().enumerate() {
        let stack_name = format!("codebuild-project-refusal-{index}");
        let template = json!({
            "Resources": { "Build": {
                "Type": "AWS::CodeBuild::Project",
                "Properties": properties,
            }}
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        let stack_status = opt_element(&described, "StackStatus").unwrap_or_default();
        assert!(
            stack_status.contains("ROLLBACK") || stack_status.contains("FAILED"),
            "case {case:?}: expected the stack not to report success, got {stack_status:?} in \
             {described}"
        );

        // Nothing the refused declaration asked for is in CodeBuild — bar the project a
        // client created, which the stack never owned.
        let (held, not_found) = held_project(&router, region, name).await;
        assert_eq!(
            held.is_some(),
            held_by_a_client,
            "case {case:?}: expected the project present={held_by_a_client} after the failed \
             stack, got {held:?} with projectsNotFound {not_found:?}"
        );
    }

    // The client's own project is still exactly the one it created.
    let (held, not_found) = held_project(&router, region, "already-built").await;
    let held = held.unwrap_or_else(|| {
        panic!(
            "expected the client's own project to still be reported, got projectsNotFound \
             {not_found:?}"
        )
    });
    assert_eq!(
        held.get("serviceRole").and_then(Value::as_str),
        Some("arn:aws:iam::000000000000:role/client-codebuild"),
        "expected the client's own project to keep the role it was created with, got {held}"
    );
}
