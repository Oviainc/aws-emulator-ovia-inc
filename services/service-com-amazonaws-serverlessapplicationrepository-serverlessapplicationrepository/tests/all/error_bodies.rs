// SPDX-License-Identifier: CC0-1.0

//! What a `serverlessrepo` error BODY says, over every error path this crate serves.
//!
//! The rule under test: an error response's `errorCode` member carries the NAME of the
//! condition the service raised — `ApplicationAlreadyExistsException`,
//! `VersionAlreadyExistsException`, `ApplicationNotFoundException`,
//! `VersionNotFoundException`, `InvalidRequestException` — never the HTTP status, which
//! travels in the status line and in `x-amzn-errortype` only. `errorCode` is a modelled
//! member of every error shape this service declares
//! (`smithy_query(query="shape", shape="ConflictException")` and the same for
//! `NotFoundException`/`BadRequestException`), so it is a wire requirement, and it is the
//! only field that separates two conditions sharing one shape and one status: a
//! duplicate application and a duplicate version are both 409 `ConflictException`.
//!
//! The rule covers every error this service RAISES. A request the framework refuses at
//! the boundary against a modelled constraint never reaches a handler, and its body is
//! rendered from the framework's `Refused` (a code and a message, no modelled member),
//! so such a row asserts the status, the shape and the absence of a status in
//! `errorCode`, and is marked with `error_code: None`.
//!
//! Every case drives the real wire path — the router resolves the service from the
//! request's SigV4 credential scope, restJson1 routes on method + URI, and the handler
//! answers out of the one shared engine — so what is asserted is what an SDK client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator: a single shared engine with this service mounted on it.
struct Repo {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the `x-amzn-errortype` header and the parsed body.
struct Answer {
    status: u16,
    error_type_header: String,
    body: Value,
}

impl Answer {
    /// A member of the response object, or `Value::Null` when it is absent.
    fn at(&self, member: &str) -> &Value {
        self.body.get(member).unwrap_or(&Value::Null)
    }

    /// The modelled `errorCode` member of the error body.
    fn error_code(&self) -> &str {
        self.at("errorCode").as_str().unwrap_or("")
    }

    /// The modelled `message` member of the error body.
    fn message(&self) -> &str {
        self.at("message").as_str().unwrap_or("")
    }

    /// The wire error code restJson1 carries as the body's `__type`: the error SHAPE.
    fn shape(&self) -> &str {
        self.at("__type").as_str().unwrap_or("")
    }
}

impl Repo {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here; the error bodies are.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-west-2/serverlessrepo/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef",
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let raw = response.body().to_vec();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_type_header,
            body,
        }
    }
}

/// An `ApplicationId` is an ARN bound to a single, non-greedy URI label — so a client
/// percent-encodes its `:` and `/` to keep it one path segment, as an AWS SDK does.
fn path_label(application_id: &str) -> String {
    application_id
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The smallest body CreateApplication accepts: the three members the model requires.
fn minimal(name: &str) -> Value {
    json!({ "name": name, "author": "example-author", "description": "a demo app" })
}

/// One error path: how to provoke it, and what the answer must say.
struct Case<'a> {
    /// What this row exercises.
    name: &'a str,
    method: &'a str,
    path: String,
    body: Option<Value>,
    /// The status the modelled shape declares as its `httpError`.
    status: u16,
    /// The error SHAPE, as restJson1 reports it in `__type`.
    shape: &'a str,
    /// The condition, as the body's `errorCode` member must report it. `None` for a
    /// request the framework refuses against a MODELLED constraint before any handler
    /// runs (`MaxItems`' `range 1..100`): that body is rendered from the framework's
    /// `Refused`, which carries a code and a message and no modelled member, so this
    /// service cannot put an `errorCode` in it from here. Such a row still asserts the
    /// status, the shape, and that no status leaked into `errorCode`.
    error_code: Option<&'a str>,
}

/// Create `name` and return its minted `ApplicationId`.
async fn created(repo: &Repo, name: &str, body: Value) -> String {
    let answer = repo.call("POST", "/applications", Some(body)).await;
    assert_eq!(
        answer.status, 201,
        "case {name:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer
        .at("applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// THE RULE, over every error path this crate serves: the body's `errorCode` names the
/// condition raised, never the status.
///
/// The rows differ along what the rule generalises over: the operation, the HTTP method,
/// the modelled shape (`ConflictException`, `NotFoundException`, `BadRequestException`),
/// the status (409, 404, 400), the subject of the condition (an application, a version, a
/// request member) and whether the condition shares its shape with another row's.
#[tokio::test]
async fn an_error_body_names_the_condition_not_the_status() {
    let repo = Repo::new();
    let application_id = created(
        &repo,
        "errors-app",
        json!({
            "name": "errors-app",
            "author": "example-author",
            "description": "an app whose error paths are under test",
            "semanticVersion": "1.0.0"
        }),
    )
    .await;
    let other_id = created(&repo, "errors-other-app", minimal("errors-other-app")).await;
    let absent_id = "arn:aws:serverlessrepo:us-west-2:000000000000:applications/absent-app";
    let label = path_label(&application_id);

    let cases = vec![
        Case {
            name: "CreateApplication: the name is already published",
            method: "POST",
            path: "/applications".to_string(),
            body: Some(minimal("errors-app")),
            status: 409,
            shape: "ConflictException",
            error_code: Some("ApplicationAlreadyExistsException"),
        },
        Case {
            name: "CreateApplication: another published name, other members differing",
            method: "POST",
            path: "/applications".to_string(),
            body: Some(json!({
                "name": "errors-other-app",
                "author": "someone-else",
                "description": "a different description entirely",
                "semanticVersion": "2.3.4"
            })),
            status: 409,
            shape: "ConflictException",
            error_code: Some("ApplicationAlreadyExistsException"),
        },
        Case {
            name: "CreateApplicationVersion: the semantic version is already published",
            method: "PUT",
            path: format!("/applications/{label}/versions/1.0.0"),
            body: Some(json!({})),
            status: 409,
            shape: "ConflictException",
            error_code: Some("VersionAlreadyExistsException"),
        },
        Case {
            name: "GetApplication: no such application",
            method: "GET",
            path: format!("/applications/{}", path_label(absent_id)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            error_code: Some("ApplicationNotFoundException"),
        },
        Case {
            name: "DeleteApplication: no such application",
            method: "DELETE",
            path: format!("/applications/{}", path_label(absent_id)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            error_code: Some("ApplicationNotFoundException"),
        },
        Case {
            name: "ListApplicationVersions: no such application",
            method: "GET",
            path: format!("/applications/{}/versions", path_label(absent_id)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            error_code: Some("ApplicationNotFoundException"),
        },
        Case {
            name: "CreateApplicationVersion: no such application",
            method: "PUT",
            path: format!("/applications/{}/versions/1.0.0", path_label(absent_id)),
            body: Some(json!({})),
            status: 404,
            shape: "NotFoundException",
            error_code: Some("ApplicationNotFoundException"),
        },
        Case {
            name: "GetApplication: the application has no such version",
            method: "GET",
            path: format!("/applications/{label}?semanticVersion=9.9.9"),
            body: None,
            status: 404,
            shape: "NotFoundException",
            error_code: Some("VersionNotFoundException"),
        },
        Case {
            name: "GetApplication: an existing application with no versions at all",
            method: "GET",
            path: format!(
                "/applications/{}?semanticVersion=1.0.0",
                path_label(&other_id)
            ),
            body: None,
            status: 404,
            shape: "NotFoundException",
            error_code: Some("VersionNotFoundException"),
        },
        Case {
            name: "CreateApplicationVersion: the version is not a semantic version",
            method: "PUT",
            path: format!("/applications/{label}/versions/1.0"),
            body: Some(json!({})),
            status: 400,
            shape: "BadRequestException",
            error_code: Some("InvalidRequestException"),
        },
        Case {
            name: "CreateApplication: the version is not a semantic version",
            method: "POST",
            path: "/applications".to_string(),
            body: Some(json!({
                "name": "errors-semver-app",
                "author": "example-author",
                "description": "a demo app",
                "semanticVersion": "v1.0.0"
            })),
            status: 400,
            shape: "BadRequestException",
            error_code: Some("InvalidRequestException"),
        },
        Case {
            name: "CreateApplication: the name is outside its documented pattern",
            method: "POST",
            path: "/applications".to_string(),
            body: Some(json!({
                "name": "not a legal name",
                "author": "example-author",
                "description": "a demo app"
            })),
            status: 400,
            shape: "BadRequestException",
            error_code: Some("InvalidRequestException"),
        },
        Case {
            name: "CreateApplication: both a license body and a license URL",
            method: "POST",
            path: "/applications".to_string(),
            body: Some(json!({
                "name": "errors-license-app",
                "author": "example-author",
                "description": "a demo app",
                "licenseBody": "ALL RIGHTS RESERVED",
                "licenseUrl": "https://example.com/LICENSE"
            })),
            status: 400,
            shape: "BadRequestException",
            error_code: Some("InvalidRequestException"),
        },
        Case {
            name: "CreateApplicationVersion: both a template body and a template URL",
            method: "PUT",
            path: format!("/applications/{label}/versions/3.0.0"),
            body: Some(json!({
                "templateBody": "{\"Resources\":{}}",
                "templateUrl": "https://example.com/template.yaml"
            })),
            status: 400,
            shape: "BadRequestException",
            error_code: Some("InvalidRequestException"),
        },
        Case {
            name: "ListApplications: maxItems one past its modelled range",
            method: "GET",
            path: "/applications?maxItems=0".to_string(),
            body: None,
            status: 400,
            shape: "BadRequestException",
            // Refused at the boundary against the modelled `range 1..100`; see the
            // field's note.
            error_code: None,
        },
        Case {
            name: "ListApplicationVersions: a nextToken this service did not mint",
            method: "GET",
            path: format!("/applications/{label}/versions?nextToken=not-a-token"),
            body: None,
            status: 400,
            shape: "BadRequestException",
            error_code: Some("InvalidRequestException"),
        },
    ];

    for case in &cases {
        let answer = repo
            .call(case.method, &case.path, case.body.clone())
            .await;
        let name = case.name;
        assert_eq!(
            answer.status, case.status,
            "case {name:?}: expected status {}, got {} with {}",
            case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            case.shape,
            "case {name:?}: expected the modelled shape {}, got {:?}",
            case.shape,
            answer.shape()
        );
        if let Some(expected) = case.error_code {
            assert_eq!(
                answer.error_code(),
                expected,
                "case {name:?}: errorCode must name the condition {expected}, got {:?}",
                answer.error_code()
            );
        }
        // The defect this rule rules out, stated directly: the status never appears in
        // errorCode, in any form, on any error path.
        assert!(
            answer.error_code().parse::<u16>().is_err(),
            "case {name:?}: errorCode must be a condition name, not a status, got {:?}",
            answer.error_code()
        );
        assert_ne!(
            answer.error_code(),
            case.status.to_string(),
            "case {name:?}: errorCode must not repeat the HTTP status, got {:?}",
            answer.error_code()
        );
        // The status travels in the status line and the error-type header; the header
        // still names the modelled shape.
        assert_eq!(
            answer.error_type_header, case.shape,
            "case {name:?}: expected x-amzn-errortype {}, got {:?}",
            case.shape, answer.error_type_header
        );
        assert!(
            !answer.message().is_empty(),
            "case {name:?}: expected a message beside errorCode, got {}",
            answer.body
        );
    }
}

/// THE RULE's point: two conditions that share one modelled shape and one status are
/// told apart by `errorCode` alone. Asserted on the pair the model collapses — a
/// duplicate application and a duplicate version, both 409 `ConflictException`.
#[tokio::test]
async fn two_conflicts_under_one_shape_carry_different_error_codes() {
    let repo = Repo::new();
    let application_id = created(
        &repo,
        "clash-app",
        json!({
            "name": "clash-app",
            "author": "example-author",
            "description": "an app with a first version",
            "semanticVersion": "1.2.3"
        }),
    )
    .await;
    let label = path_label(&application_id);

    let duplicate_application = repo
        .call("POST", "/applications", Some(minimal("clash-app")))
        .await;
    let duplicate_version = repo
        .call(
            "PUT",
            &format!("/applications/{label}/versions/1.2.3"),
            Some(json!({})),
        )
        .await;

    for (case, answer) in [
        ("duplicate application", &duplicate_application),
        ("duplicate version", &duplicate_version),
    ] {
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.shape(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            answer.shape()
        );
    }
    assert_ne!(
        duplicate_application.error_code(),
        duplicate_version.error_code(),
        "the two 409 conditions must be distinguishable by errorCode alone, both said {:?}",
        duplicate_application.error_code()
    );
    assert_eq!(
        duplicate_application.error_code(),
        "ApplicationAlreadyExistsException",
        "got {:?}",
        duplicate_application.error_code()
    );
    assert_eq!(
        duplicate_version.error_code(),
        "VersionAlreadyExistsException",
        "got {:?}",
        duplicate_version.error_code()
    );
}

/// The `message` beside `errorCode` names the subject the caller must fix: the
/// `ApplicationId` that was not found, or the member that was wrong. Asserted over a set
/// of ids rather than one, because the message is built from the request.
#[tokio::test]
async fn a_message_names_the_subject_of_the_condition() {
    let repo = Repo::new();
    let absent = [
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/absent-one",
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/absent-two",
        "arn:aws:serverlessrepo:us-east-1:000000000000:applications/absent-in-another-region",
    ];

    for application_id in absent {
        for (operation, method, path) in [
            (
                "GetApplication",
                "GET",
                format!("/applications/{}", path_label(application_id)),
            ),
            (
                "DeleteApplication",
                "DELETE",
                format!("/applications/{}", path_label(application_id)),
            ),
            (
                "ListApplicationVersions",
                "GET",
                format!("/applications/{}/versions", path_label(application_id)),
            ),
        ] {
            let answer = repo.call(method, &path, None).await;
            assert_eq!(
                answer.status, 404,
                "case {operation} {application_id:?}: expected 404, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ApplicationNotFoundException",
                "case {operation} {application_id:?}: got {:?}",
                answer.error_code()
            );
            assert!(
                answer.message().contains(application_id),
                "case {operation} {application_id:?}: the message must name the id asked for, \
                 got {:?}",
                answer.message()
            );
        }
    }

    // A refusal of a non-semver version reports the version GRAMMAR and the syntax it
    // must follow — a rule about the member, not about this request, so the sentence is
    // fixed and carries nothing of the request. Its exact wording, over every operation
    // that validates the member, is `tests/all/semantic_version_wording.rs`.
    let application_id = created(&repo, "message-app", minimal("message-app")).await;
    for semantic_version in ["1.0", "1.0.0.0", "v1.0.0", "01.0.0", "1.0.0-"] {
        let answer = repo
            .call(
                "PUT",
                &format!(
                    "/applications/{}/versions/{semantic_version}",
                    path_label(&application_id)
                ),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {semantic_version:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {semantic_version:?}: got {:?}",
            answer.error_code()
        );
        assert!(
            answer.message().contains("semantic version")
                && answer.message().contains("semver.org")
                && !answer.message().contains(semantic_version),
            "case {semantic_version:?}: the refusal must report the semantic-version \
             grammar and its syntax reference and name no request value, got {:?}",
            answer.message()
        );
    }
}
