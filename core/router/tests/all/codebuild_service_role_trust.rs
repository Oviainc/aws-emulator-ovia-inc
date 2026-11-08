// SPDX-License-Identifier: CC0-1.0

//! CodeBuild's service-role check, across the two services it spans.
//!
//! RULE: a build project RUNS AS the role the request hands CodeBuild
//! (`CreateProject$serviceRole` is "The ARN of the IAM role that enables CodeBuild to
//! interact with dependent Amazon Web Services services on behalf of the Amazon Web
//! Services account", and `StartBuild$serviceRoleOverride` replaces it for one build), so
//! a role this account's IAM does not hold, or one whose trust policy does not let
//! `codebuild.amazonaws.com` assume it, is refused with `InvalidInputException`
//! (HTTP 400) — and nothing is created or started. The role is resolved against the
//! ACCOUNT's IAM records, which is why this suite lives here: the role is created through
//! the real `CreateRole` on the same engine that serves CodeBuild, so what the check reads
//! is the document IAM persisted and not a copy.
//!
//! The rows differ along the form of the trust policy (which service principal it names,
//! whether it names several, whether the grant is an `Allow`, whether an explicit `Deny`
//! stands beside it, which action it names and which kind of principal it names) and along
//! whether the IAM store holds the role at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder that builds the form body of an awsQuery call and the reader that
// pulls one element's text out of the XML answer are the test crate's own, written once in
// `all/query_wire.rs`: a suite that writes its own drifts from the others.
use crate::query_wire::{element, form_encode};

/// The region every CodeBuild request below is signed for.
const REGION: &str = "us-east-1";

/// One emulator carrying both services over ONE shared engine, exactly as the production
/// router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to IAM, answered with the HTTP status and the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One awsJson1_1 call to CodeBuild, answered with the HTTP status and the JSON body.
async fn codebuild(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("codebuild.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=abc"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("the test body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A `CreateProject` request for `name` that runs as `service_role`.
fn create_project_body(name: &str, service_role: &str) -> Value {
    json!({
        "name": name,
        "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
        "artifacts": {"type": "NO_ARTIFACTS"},
        "environment": {
            "type": "LINUX_CONTAINER",
            "image": "aws/codebuild/standard:7.0",
            "computeType": "BUILD_GENERAL1_SMALL",
        },
        "serviceRole": service_role,
    })
}

/// A trust policy naming `principals` in one `Allow` statement.
fn trust_allowing(principals: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principals },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// Create a role in IAM carrying `document` and hand back its ARN.
async fn role_with_trust(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole {name:?}: expected the role to be persisted, got {status}: {created}"
    );
    element(&created, "Arn")
}

/// The sentence CodeBuild states when it cannot assume the service role a request hands
/// it — fixed text that quotes no role, so the assertion reads it from the service rather
/// than spelling it here.
fn expected_refusal() -> &'static str {
    service_com_amazonaws_codebuild_codebuild_20161006::state::UNASSUMABLE_SERVICE_ROLE_MESSAGE
}

/// One row: a trust policy, and whether a project that runs as a role carrying it is
/// created.
struct Row {
    case: &'static str,
    document: String,
    accepted: bool,
}

#[tokio::test]
async fn create_project_refuses_a_role_whose_trust_policy_withholds_codebuild() {
    let router = emulator();

    let rows = [
        Row {
            case: "codebuild alone",
            document: trust_allowing(&["codebuild.amazonaws.com"]),
            accepted: true,
        },
        Row {
            case: "codebuild among several services",
            document: trust_allowing(&["ec2.amazonaws.com", "codebuild.amazonaws.com"]),
            accepted: true,
        },
        Row {
            case: "codebuild named as a single string rather than a list",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "codebuild.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            accepted: true,
        },
        Row {
            case: "codebuild allowed through an sts wildcard",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "codebuild.amazonaws.com" },
                    "Action": "sts:*"
                }]
            })
            .to_string(),
            accepted: true,
        },
        Row {
            case: "another service only",
            document: trust_allowing(&["ec2.amazonaws.com"]),
            accepted: false,
        },
        Row {
            case: "codebuild denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "Service": "codebuild.amazonaws.com" },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over any `Allow`.
            case: "codebuild both allowed and denied",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "codebuild.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "Service": "codebuild.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    }
                ]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            // The statement names codebuild, but for an action that is not assuming the
            // role, so it grants nothing: a trust policy governs `sts:AssumeRole`.
            case: "codebuild allowed an action that is not assuming the role",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "codebuild.amazonaws.com" },
                    "Action": "s3:GetObject"
                }]
            })
            .to_string(),
            accepted: false,
        },
        Row {
            case: "an account principal rather than a service one",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::000000000000:root" },
                    "Action": "sts:AssumeRole"
                }]
            })
            .to_string(),
            accepted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let role_arn =
            role_with_trust(&router, &format!("codebuild-trust-{index}"), &row.document).await;
        let project = format!("trust-proj-{index}");

        let (status, created) =
            codebuild(&router, "CreateProject", create_project_body(&project, &role_arn)).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: a role that grants codebuild must be accepted, got {status}: {created}",
                row.case
            );
            assert_eq!(
                created["project"]["serviceRole"].as_str(),
                Some(role_arn.as_str()),
                "case {:?}: the project runs as the role it was created with, got {created}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: a role that withholds the grant must be refused, \
                 got {status}: {created}",
                row.case
            );
            assert_eq!(
                created["__type"].as_str(),
                Some("InvalidInputException"),
                "case {:?}: expected InvalidInputException, got {created}",
                row.case
            );
            assert_eq!(
                created["message"].as_str(),
                Some(expected_refusal()),
                "case {:?}: expected CodeBuild's assume-role sentence, got {created}",
                row.case
            );
        }

        // What the account holds afterwards is read back through CodeBuild's own read: a
        // refused create persists NO project.
        let (status, read) =
            codebuild(&router, "BatchGetProjects", json!({"names": [&project]})).await;
        assert_eq!(
            status, 200,
            "case {:?}: BatchGetProjects: expected 200, got {status}: {read}",
            row.case
        );
        let held = read["projects"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            held,
            usize::from(row.accepted),
            "case {:?}: expected {} project(s) to be held, got {read}",
            row.case,
            usize::from(row.accepted)
        );
    }
}

/// RULE: the sentence CodeBuild states for a service role it cannot assume is FIXED text —
/// the three numbered conditions AWS words it with, in that order, with no part of the
/// request quoted back — so it is the same sentence whatever made the role unassumable, and
/// a client that matches on it (or an operator reading a CI failure) gets AWS's own
/// remediation text rather than this emulator's prose.
///
/// The rows differ along WHY the role cannot be assumed: this account holds no such role,
/// the role trusts another service only, and the role's own trust policy denies this
/// service — and along WHICH operation resolves the role (the project's own `serviceRole`
/// and a build's `serviceRoleOverride`).
#[tokio::test]
async fn the_unassumable_service_role_refusal_carries_awss_own_three_conditions() {
    /// The recorded sentence, word for word.
    const SENTENCE: &str = "CodeBuild is not authorized to perform: sts:AssumeRole on \
                            service role. Please verify that: 1) The provided service role \
                            exists, 2) The role name is case-sensitive and matches exactly, \
                            and 3) The role has the necessary trust policy configured.";

    assert_eq!(
        expected_refusal(),
        SENTENCE,
        "the service's own sentence must be the recorded one, got {:?}",
        expected_refusal()
    );

    let router = emulator();
    // The project a build's override is refused against, which runs as a role this
    // service may assume.
    let own_role = role_with_trust(
        &router,
        "codebuild-wording-project",
        &trust_allowing(&["codebuild.amazonaws.com"]),
    )
    .await;
    let (status, created) = codebuild(
        &router,
        "CreateProject",
        create_project_body("wording-proj", &own_role),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject: expected 200, got {status}: {created}"
    );

    // (case, the role a request hands CodeBuild)
    let denied = role_with_trust(
        &router,
        "codebuild-wording-denied",
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Principal": { "Service": "codebuild.amazonaws.com" },
                "Action": "sts:AssumeRole"
            }]
        })
        .to_string(),
    )
    .await;
    let other_service = role_with_trust(
        &router,
        "codebuild-wording-other",
        &trust_allowing(&["ec2.amazonaws.com"]),
    )
    .await;
    let rows: [(&str, String); 3] = [
        (
            "a role this account's IAM holds nothing under",
            "arn:aws:iam::000000000000:role/codebuild-wording-absent".to_string(),
        ),
        ("a role trusted by another service only", other_service),
        ("a role whose trust policy denies this service", denied),
    ];

    for (index, (case, role_arn)) in rows.iter().enumerate() {
        // The role the PROJECT would run as.
        let (status, refused) = codebuild(
            &router,
            "CreateProject",
            create_project_body(&format!("wording-{index}"), role_arn),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateProject must refuse the role, got {status}: {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(SENTENCE),
            "case {case:?}: CreateProject must state AWS's own three conditions, got {refused}"
        );

        // The role one BUILD would run as: the same question, so the same sentence.
        let (status, refused) = codebuild(
            &router,
            "StartBuild",
            json!({"projectName": "wording-proj", "serviceRoleOverride": role_arn}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: StartBuild must refuse the override, got {status}: {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(SENTENCE),
            "case {case:?}: StartBuild must state AWS's own three conditions, got {refused}"
        );
    }
}
#[tokio::test]
async fn start_build_refuses_a_service_role_override_it_cannot_assume() {
    let router = emulator();

    let project_role = role_with_trust(
        &router,
        "codebuild-project-role",
        &trust_allowing(&["codebuild.amazonaws.com"]),
    )
    .await;
    let (status, created) = codebuild(
        &router,
        "CreateProject",
        create_project_body("override-proj", &project_role),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject: expected 200, got {status}: {created}"
    );

    // (case, the role the start overrides with, whether the build starts)
    let other_service = role_with_trust(
        &router,
        "codebuild-other-service",
        &trust_allowing(&["ec2.amazonaws.com"]),
    )
    .await;
    let assumable = role_with_trust(
        &router,
        "codebuild-override-ok",
        &trust_allowing(&["codebuild.amazonaws.com"]),
    )
    .await;
    let rows: [(&str, String, bool); 4] = [
        ("a role codebuild may assume", assumable, true),
        ("a role trusted by another service", other_service, false),
        (
            "a role this account's IAM holds nothing under",
            "arn:aws:iam::000000000000:role/codebuild-never-created".to_string(),
            false,
        ),
        (
            "a role under a path that was never created",
            "arn:aws:iam::000000000000:role/service-role/codebuild-never-created".to_string(),
            false,
        ),
    ];

    let mut started = 0usize;
    for (case, role_arn, starts) in &rows {
        let (status, build) = codebuild(
            &router,
            "StartBuild",
            json!({"projectName": "override-proj", "serviceRoleOverride": role_arn}),
        )
        .await;

        if *starts {
            started += 1;
            assert_eq!(
                status, 200,
                "case {case:?}: a role codebuild may assume must start the build, \
                 got {status}: {build}"
            );
            assert_eq!(
                build["build"]["serviceRole"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the build runs as the override, got {build}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: an unassumable override must be refused, got {status}: {build}"
            );
            assert_eq!(
                build["__type"].as_str(),
                Some("InvalidInputException"),
                "case {case:?}: expected InvalidInputException, got {build}"
            );
            assert_eq!(
                build["message"].as_str(),
                Some(expected_refusal()),
                "case {case:?}: expected CodeBuild's assume-role sentence, got {build}"
            );
        }

        // Only the accepted starts left a build behind, read back through the project's
        // own listing.
        let (status, listed) = codebuild(
            &router,
            "ListBuildsForProject",
            json!({"projectName": "override-proj"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListBuildsForProject: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed["ids"].as_array().map(Vec::len).unwrap_or_default(),
            started,
            "case {case:?}: expected {started} build(s) for the project, got {listed}"
        );
    }

    // The project's OWN role still starts a build: the refusals above were about the
    // override, not about the project.
    let (status, build) = codebuild(
        &router,
        "StartBuild",
        json!({"projectName": "override-proj"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a start with no override must go ahead, got {status}: {build}"
    );
    assert_eq!(
        build["build"]["serviceRole"].as_str(),
        Some(project_role.as_str()),
        "the build runs as the project's own role, got {build}"
    );
}
