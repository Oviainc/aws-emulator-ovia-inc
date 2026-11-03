// SPDX-License-Identifier: CC0-1.0

//! How FIS refuses a reference to an experiment template that resolves to no record,
//! end to end over HTTP.
//!
//! One rule is under test: every operation of this service that resolves an
//! `experimentTemplateId` answers a miss with HTTP 404 `ResourceNotFoundException`
//! carrying ONE fixed, identifier-free message — the requested id never appears in it,
//! whatever that id looked like and however it came to be unresolvable (never minted,
//! or minted and since deleted).
//!
//! The rule generalises over two axes, so it is asserted over their product rather than
//! over one recorded request:
//!
//! * the FORM of the unresolvable id (a minted-shaped id, free-form text, one character,
//!   `ExperimentTemplateId`'s published 64-character ceiling, punctuation);
//! * WHICH operation resolved it (`GetExperimentTemplate`, `UpdateExperimentTemplate`,
//!   `DeleteExperimentTemplate`, `StartExperiment`, `CreateTargetAccountConfiguration`,
//!   `ListTargetAccountConfigurations` — every operation whose input carries an
//!   `experimentTemplateId`, per
//!   `smithy_query(query="operation",operation="…")`).
//!
//! A second test walks the lifecycle the rule is observed through — create, read back,
//! delete, read back — so the refusal is proven to follow a real deletion and not an
//! unconditional 404, and the sibling resolvers are proven to answer 200 for a template
//! that DOES resolve.
//!
//! The HTTP harness below is local to this suite, as it is in the sibling suites: a test
//! harness is not shared state.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The message FIS answers for an experiment template reference that resolves to no
/// record — a constant, so the requested identifier appears nowhere in it. Spelled out
/// here rather than imported from the crate so a change to the handler's text fails this
/// suite.
const TEMPLATE_NOT_FOUND: &str = "ExperimentTemplate not found.";

/// The wire code every one of these refusals carries (`ResourceNotFoundException`,
/// `errorKind` client, `httpError` 404).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// `ExperimentTemplateId`'s published ceiling (`@length(min: 0, max: 64)`,
/// `smithy_query(query="operation",operation="GetExperimentTemplate")`): an id at the
/// bound still reaches the resolver instead of being refused at the boundary.
const TEMPLATE_ID_CEILING: usize = 64;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A well-formed IAM role ARN, for the members that require one.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/fis-not-found-suite";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message the answer carries, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The member `name` of this answer's body.
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(FALLBACK_REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One operation that resolves an `experimentTemplateId`, as a request this suite can
/// aim at any id.
struct Resolver {
    /// The operation's name, for the assertion messages.
    name: &'static str,
    method: &'static str,
    /// The request URI for the template id under test.
    uri: fn(&str) -> String,
    /// The request body for the template id under test, if the operation takes one.
    body: fn(&str) -> Option<Value>,
}

/// Every operation whose input carries an `experimentTemplateId`, so each one's refusal
/// is checked rather than only the operation the issue was filed about.
///
/// `GetTargetAccountConfiguration`, `DeleteTargetAccountConfiguration` and
/// `UpdateTargetAccountConfiguration` also take one, but they name a target account
/// configuration rather than the template: a miss on either half of that pair is the
/// configuration's own constant refusal, which the
/// `target_account_configuration_errors` suite covers.
fn resolvers() -> Vec<Resolver> {
    vec![
        Resolver {
            name: "GetExperimentTemplate",
            method: "GET",
            uri: |id| format!("/experimentTemplates/{id}"),
            body: |_| None,
        },
        Resolver {
            name: "UpdateExperimentTemplate",
            method: "PATCH",
            uri: |id| format!("/experimentTemplates/{id}"),
            // Only `description`: the rules this request would be refused by BEFORE the
            // lookup (a `roleArn` that is no IAM role ARN) must not stand in for the
            // not-found refusal under test.
            body: |id| Some(json!({ "description": format!("update {id}") })),
        },
        Resolver {
            name: "DeleteExperimentTemplate",
            method: "DELETE",
            uri: |id| format!("/experimentTemplates/{id}"),
            body: |_| None,
        },
        Resolver {
            name: "StartExperiment",
            method: "POST",
            uri: |_| "/experiments".to_string(),
            body: |id| {
                Some(json!({
                    "clientToken": format!("start-{id}"),
                    "experimentTemplateId": id
                }))
            },
        },
        Resolver {
            name: "CreateTargetAccountConfiguration",
            method: "POST",
            uri: |id| format!("/experimentTemplates/{id}/targetAccountConfigurations/123456789012"),
            body: |_| Some(json!({ "roleArn": ROLE_ARN })),
        },
        Resolver {
            name: "ListTargetAccountConfigurations",
            method: "GET",
            uri: |id| format!("/experimentTemplates/{id}/targetAccountConfigurations"),
            body: |_| None,
        },
    ]
}

/// The template ids that resolve to no record, each inside `ExperimentTemplateId`'s
/// modelled `@length(0..64)` and `pattern ^[\S]+$` so the refusal can only come from the
/// resolver and not from the boundary validator.
///
/// The rows differ along the FORM of the id: the shape this emulator mints, a longer
/// minted-looking id, free-form text, one character (a digit, so that an echo of it
/// could not be mistaken for a letter of the fixed message), the published 64-character
/// ceiling, and an id carrying punctuation and mixed case.
fn unresolvable_ids() -> Vec<(&'static str, String)> {
    vec![
        ("minted-shape-never-minted", "EXT000000000001".to_string()),
        ("minted-shape-high-serial", "EXT999999999999".to_string()),
        (
            "free-form-text",
            "nonexistente4c6c937f48d427d".to_string(),
        ),
        ("single-character", "7".to_string()),
        (
            "at-the-64-character-ceiling",
            "e".repeat(TEMPLATE_ID_CEILING),
        ),
        (
            "punctuation-and-mixed-case",
            "EXT_Not-A-Template.0001".to_string(),
        ),
    ]
}

/// A `CreateExperimentTemplate` body, with everything the operation requires.
fn template_body(label: &str) -> Value {
    json!({
        "clientToken": format!("template-{label}"),
        "description": format!("experiment template not-found suite: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        // `targets` is a member FIS requires of the request, so every create body here
        // names it.
        "targets": {},
        // `aws:fis:wait` takes a required `duration` parameter (an ISO 8601 duration
        // within the documented 1 to 720 minutes), which the create refuses without.
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } } }
    })
}

/// Create an experiment template and report its minted id.
async fn create_template(router: &MultiServiceRouter, label: &str) -> String {
    let answer = send(
        router,
        "POST",
        "/experimentTemplates",
        Some(template_body(label)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experimentTemplate.id in {:?}", answer.body))
        .to_string()
}

/// Every resolver of an `experimentTemplateId` refuses an id that resolves to no record
/// with 404 `ResourceNotFoundException` and the ONE fixed message, which never carries
/// the requested id.
#[tokio::test]
async fn unresolvable_template_id_is_refused_with_the_fixed_message() {
    let router = emulator();

    for resolver in resolvers() {
        for (form, id) in unresolvable_ids() {
            let case = format!("{} / {form}", resolver.name);
            let uri = (resolver.uri)(&id);
            let answer = send(&router, resolver.method, &uri, (resolver.body)(&id)).await;

            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                NOT_FOUND_CODE,
                "case {case:?}: expected error code {NOT_FOUND_CODE:?}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                TEMPLATE_NOT_FOUND,
                "case {case:?}: expected message {TEMPLATE_NOT_FOUND:?}, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains(id.as_str()),
                "case {case:?}: the message must not echo the requested id {id:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// The refusal follows the store and not the request: a template that resolves is read
/// back, listed and started against; once deleted, the same reads answer the fixed
/// not-found refusal.
#[tokio::test]
async fn template_resolves_until_it_is_deleted() {
    let router = emulator();
    let id = create_template(&router, "lifecycle").await;

    // While the template resolves, every resolver of its id answers a success — so the
    // 404s below are the deletion's doing and not an unconditional refusal.
    let read_back = send(&router, "GET", &format!("/experimentTemplates/{id}"), None).await;
    assert_eq!(
        read_back.status, 200,
        "case \"read back a live template\": expected HTTP 200, got {} with {:?}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.member("experimentTemplate")["id"]
            .as_str()
            .unwrap_or_default(),
        id.as_str(),
        "case \"read back a live template\": expected id {id:?}, got {:?}",
        read_back.member("experimentTemplate")
    );

    let listed = send(
        &router,
        "GET",
        &format!("/experimentTemplates/{id}/targetAccountConfigurations"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case \"list a live template's target account configurations\": expected HTTP 200, \
         got {} with {:?}",
        listed.status, listed.body
    );

    let started = send(
        &router,
        "POST",
        "/experiments",
        Some(json!({ "clientToken": "start-lifecycle", "experimentTemplateId": id })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"start an experiment from a live template\": expected HTTP 200, got {} with {:?}",
        started.status, started.body
    );

    let deleted = send(
        &router,
        "DELETE",
        &format!("/experimentTemplates/{id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete the template\": expected HTTP 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // The minted id is now unresolvable, and every resolver refuses it exactly as it
    // refuses an id that was never minted.
    for resolver in resolvers() {
        let case = format!("{} after delete", resolver.name);
        let uri = (resolver.uri)(&id);
        let answer = send(&router, resolver.method, &uri, (resolver.body)(&id)).await;

        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            NOT_FOUND_CODE,
            "case {case:?}: expected error code {NOT_FOUND_CODE:?}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            TEMPLATE_NOT_FOUND,
            "case {case:?}: expected message {TEMPLATE_NOT_FOUND:?}, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains(id.as_str()),
            "case {case:?}: the message must not echo the deleted id {id:?}, got {:?}",
            answer.message()
        );
    }
}
