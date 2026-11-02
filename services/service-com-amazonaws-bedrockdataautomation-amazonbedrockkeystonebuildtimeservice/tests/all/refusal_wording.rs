// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of HOW this service WORDS the client errors it refuses a
//! request with.
//!
//! Every case drives the real `restJson1` wire path — `MultiServiceRouter::dispatch`
//! with a signed-looking request — so routing, HTTP binding, constraint validation, the
//! refusal seam, the handler and response serialization are all exercised, exactly as an
//! AWS SDK `bedrock-data-automation` client would.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over (the operation, the resource kind, the ARN's partition, region,
//! account and id length, the name's character class and length, the constraint class
//! broken, and whether the member's value is one the model withholds) rather than over
//! one recorded request:
//!
//! 1. a lookup that resolves to no resource this store holds is refused with ONE FIXED
//!    sentence per refusal class — one per resource kind for an ARN, and one more for a
//!    blueprint lookup the request scoped to a version — taking nothing from the request,
//!    whichever operation was handed it;
//! 2. a create whose name some live resource already holds is refused with ONE FIXED
//!    sentence per resource kind, which does not carry the requested name;
//! 3. a value outside a constraint its member declares is stated with the member path and
//!    the requirement, with a `@pattern` quoted WITHOUT the `^`/`$` a whole-value pattern
//!    is written with, and with NO value at all for a member the model marks
//!    `@sensitive`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`): `can_handle` resolves the route key from it.
fn authorization() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef"
        .to_string()
}

/// The emulator under test: one shared engine, one mounted service, request
/// authentication off so the suite signs nothing for real.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return its status, the raw body text (so a case can assert
/// that a value is nowhere in the answer) and the decoded JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let decoded = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, text, decoded)
}

/// The one sentence this service answers for a `blueprintArn` that resolves to nothing.
const BLUEPRINT_ARN_NOT_FOUND: &str = "Blueprint Arn not found.";

/// The one sentence this service answers when the request SCOPED a blueprint lookup to a
/// version and the ARN-and-version pair resolves to nothing.
const BLUEPRINT_ARN_OR_VERSION_INVALID: &str =
    "Incorrect Blueprint Arn or Version provided in the request.";

/// The one sentence this service answers for a `projectArn` that resolves to nothing.
const PROJECT_ARN_NOT_FOUND: &str = "Project Arn not found.";

/// The one sentence `CreateBlueprint` answers for a `blueprintName` already taken.
const BLUEPRINT_NAME_CONFLICT: &str = "Blueprint name already exists.";

/// The one sentence `CreateDataAutomationProject` answers for a `projectName` already
/// taken.
const PROJECT_NAME_CONFLICT: &str = "Project name already exists.";

/// ARNs that satisfy the modelled `BlueprintArn` pattern
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):blueprint/(bedrock-data-automation-public-[a-zA-Z0-9-_]{1,30}|[a-zA-Z0-9-]{12,36})$`
/// (source: `smithy_query(shape="BlueprintArn")`) and name no blueprint this store holds.
///
/// The rows span the pattern: every partition it admits, a short and a long region, and
/// the resource id at both published bounds of `{12,36}`.
fn unknown_blueprint_arns() -> Vec<String> {
    vec![
        "arn:aws:bedrock:us-east-1:000000000000:blueprint/aaaaaaaaaaaa".to_string(),
        "arn:aws-cn:bedrock:cn-north-1:111111111111:blueprint/0c579b95-0000000000000000".to_string(),
        format!(
            "arn:aws-us-gov:bedrock:eu-central-1:222222222222:blueprint/{}",
            "abcdefghijklmnopqrstuvwxyz0123456789"
        ),
    ]
}

/// ARNs that satisfy the modelled `DataAutomationProjectArn` pattern
/// `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$`
/// (source: `smithy_query(shape="DataAutomationProjectArn")`) and name no project this
/// store holds. Spans the same axes as [`unknown_blueprint_arns`].
fn unknown_project_arns() -> Vec<String> {
    vec![
        "arn:aws:bedrock:us-east-1:000000000000:data-automation-project/bbbbbbbbbbbb".to_string(),
        "arn:aws-cn:bedrock:cn-north-1:111111111111:data-automation-project/7f3c-1111-2222-3333"
            .to_string(),
        format!(
            "arn:aws-us-gov:bedrock:eu-central-1:222222222222:data-automation-project/{}",
            "abcdefghijklmnopqrstuvwxyz0123456789"
        ),
    ]
}

/// `/blueprints/{blueprintArn}/`, the URI `GetBlueprint`, `UpdateBlueprint` and
/// `DeleteBlueprint` bind.
fn blueprint_uri(arn: &str) -> String {
    format!("/blueprints/{}/", encode_label(arn))
}

/// `/data-automation-projects/{projectArn}/`, the URI `GetDataAutomationProject`,
/// `UpdateDataAutomationProject` and `DeleteDataAutomationProject` bind.
fn project_uri(arn: &str) -> String {
    format!("/data-automation-projects/{}/", encode_label(arn))
}

/// One request whose ARN names no resource this store holds.
struct NotFoundCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    /// The ARN the request named, which the refusal must not carry.
    named: String,
    expected: &'static str,
}

/// Every implemented operation of this service that resolves an ARN to a stored
/// resource, against an ARN that resolves to none — the blueprint ones and the project
/// ones, including the two listings that resolve an ARN out of a FILTER rather than out
/// of the URI.
fn not_found_cases() -> Vec<NotFoundCase> {
    let blueprints = unknown_blueprint_arns();
    let projects = unknown_project_arns();
    vec![
        NotFoundCase {
            case: "GetBlueprint, resource id at the 12-character lower bound",
            method: "POST",
            uri: blueprint_uri(&blueprints[0]),
            body: json!({}),
            named: blueprints[0].clone(),
            expected: BLUEPRINT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "GetBlueprint scoped to a version, aws-cn partition",
            method: "POST",
            uri: blueprint_uri(&blueprints[1]),
            body: json!({ "blueprintVersion": "7" }),
            named: blueprints[1].clone(),
            expected: BLUEPRINT_ARN_OR_VERSION_INVALID,
        },
        NotFoundCase {
            case: "DeleteBlueprint scoped to a version",
            method: "DELETE",
            uri: format!("{}?blueprintVersion=3", blueprint_uri(&blueprints[1])),
            body: json!({}),
            named: blueprints[1].clone(),
            expected: BLUEPRINT_ARN_OR_VERSION_INVALID,
        },
        NotFoundCase {
            case: "DeleteBlueprint, resource id at the 36-character upper bound",
            method: "DELETE",
            uri: blueprint_uri(&blueprints[2]),
            body: json!({}),
            named: blueprints[2].clone(),
            expected: BLUEPRINT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "UpdateBlueprint",
            method: "PUT",
            uri: blueprint_uri(&blueprints[0]),
            body: json!({ "schema": "{\"description\":\"a schema\"}" }),
            named: blueprints[0].clone(),
            expected: BLUEPRINT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "CreateBlueprintVersion",
            method: "POST",
            uri: format!("/blueprints/{}/versions/", encode_label(&blueprints[1])),
            body: json!({}),
            named: blueprints[1].clone(),
            expected: BLUEPRINT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "ListDataAutomationProjects resolving a blueprintFilter",
            method: "POST",
            uri: "/data-automation-projects/".to_string(),
            body: json!({ "blueprintFilter": { "blueprintArn": blueprints[2] } }),
            named: blueprints[2].clone(),
            expected: BLUEPRINT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "GetDataAutomationProject, resource id at the 12-character lower bound",
            method: "POST",
            uri: project_uri(&projects[0]),
            body: json!({}),
            named: projects[0].clone(),
            expected: PROJECT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "GetDataAutomationProject naming a stage too, aws-cn partition",
            method: "POST",
            uri: project_uri(&projects[1]),
            body: json!({ "projectStage": "LIVE" }),
            named: projects[1].clone(),
            expected: PROJECT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "DeleteDataAutomationProject, resource id at the 36-character upper bound",
            method: "DELETE",
            uri: project_uri(&projects[2]),
            body: json!({}),
            named: projects[2].clone(),
            expected: PROJECT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "UpdateDataAutomationProject",
            method: "PUT",
            uri: project_uri(&projects[0]),
            body: json!({ "standardOutputConfiguration": {} }),
            named: projects[0].clone(),
            expected: PROJECT_ARN_NOT_FOUND,
        },
        NotFoundCase {
            case: "ListBlueprints resolving a projectFilter",
            method: "POST",
            uri: "/blueprints/".to_string(),
            body: json!({ "projectFilter": { "projectArn": projects[1] } }),
            named: projects[1].clone(),
            expected: PROJECT_ARN_NOT_FOUND,
        },
    ]
}

/// RULE: a lookup that resolves to no resource this store holds is refused `404`
/// `ResourceNotFoundException` with ONE FIXED sentence per refusal class — the resource
/// kind's ARN sentence, or the ARN-or-version sentence when the request scoped a
/// blueprint lookup to a version — and the sentence carries nothing from the request, so
/// it is the same whichever operation was handed the ARN and whatever partition, region,
/// account or id the ARN carried.
#[tokio::test]
async fn a_lookup_that_resolves_to_nothing_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    // A store that is NOT empty: one live blueprint and one live project, so every row
    // below is answered about the ARN it named rather than about an empty store.
    let (status, _, payload) = call(
        &router,
        "PUT",
        "/blueprints/",
        &json!({ "blueprintName": "live-blueprint", "type": "DOCUMENT", "schema": "{}" }),
    )
    .await;
    assert_eq!(status, 201, "expected the seed CreateBlueprint to be 201");
    let live_blueprint = string_at(
        payload
            .get("blueprint")
            .unwrap_or_else(|| panic!("the seed create returned no blueprint: {payload:?}")),
        "blueprintArn",
        "seed blueprint",
    );
    let (status, _, _) = call(
        &router,
        "PUT",
        "/data-automation-projects/",
        &json!({ "projectName": "live-project", "standardOutputConfiguration": {} }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected the seed CreateDataAutomationProject to be 201"
    );

    // The pair refusal with its ARN half RESOLVING and only its version half missing:
    // the live blueprint, read at a version nothing ever cut. Same sentence as a
    // version-scoped lookup whose ARN is unknown, which is what makes it a sentence
    // about the pair.
    let mut rows = not_found_cases();
    rows.push(NotFoundCase {
        case: "GetBlueprint on a live blueprint at a version nothing ever cut",
        method: "POST",
        uri: blueprint_uri(&live_blueprint),
        body: json!({ "blueprintVersion": "9" }),
        named: live_blueprint.clone(),
        expected: BLUEPRINT_ARN_OR_VERSION_INVALID,
    });
    rows.push(NotFoundCase {
        case: "DeleteBlueprint on a live blueprint at a version nothing ever cut",
        method: "DELETE",
        uri: format!("{}?blueprintVersion=9", blueprint_uri(&live_blueprint)),
        body: json!({}),
        named: live_blueprint.clone(),
        expected: BLUEPRINT_ARN_OR_VERSION_INVALID,
    });

    for row in rows {
        let NotFoundCase {
            case,
            method,
            uri,
            body,
            named,
            expected,
        } = row;
        let (status, text, payload) = call(&router, method, &uri, &body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled ResourceNotFoundException status 404, got \
             {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !text.contains(&named),
            "case {case:?}: expected the refusal to carry nothing from the request, but it \
             echoed {named:?}: {text}"
        );
    }
}

/// The names a conflict case takes: every character class the modelled
/// `^[a-zA-Z0-9-_]+$` admits (source: `smithy_query(shape="BlueprintName")` and
/// `smithy_query(shape="DataAutomationProjectName")`, the same expression on both), at
/// the lower bound of `@length(1..128)`, in the middle, and at the upper bound.
fn taken_names() -> Vec<(&'static str, String)> {
    vec![
        ("one character, the length lower bound", "b".to_string()),
        ("hyphens", "name-with-hyphens".to_string()),
        ("underscores", "name_with_underscores".to_string()),
        ("digits only", "20261029".to_string()),
        ("mixed case", "NameWithMixedCase".to_string()),
        ("128 characters, the length upper bound", "a".repeat(128)),
    ]
}

/// RULE: a create whose name some live resource of the same kind already holds is refused
/// `409` `ConflictException` with ONE FIXED sentence per resource kind, which does NOT
/// carry the requested name — the two name shapes are modelled `@sensitive`, so the
/// refusal names the member class and discloses no value.
#[tokio::test]
async fn a_name_already_taken_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    for (case, name) in taken_names() {
        // Blueprints and projects hold their names in separate name spaces, so the same
        // name is free on each and each row exercises both creates.
        let blueprint =
            json!({ "blueprintName": name, "type": "IMAGE", "schema": "{\"k\":\"v\"}" });
        let (status, _, payload) = call(&router, "PUT", "/blueprints/", &blueprint).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first CreateBlueprint to be 201, got {status} with \
             {payload:?}"
        );
        let (status, text, payload) = call(&router, "PUT", "/blueprints/", &blueprint).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the modelled ConflictException status 409, got {status} \
             with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        assert_eq!(
            message, BLUEPRINT_NAME_CONFLICT,
            "case {case:?}: expected {BLUEPRINT_NAME_CONFLICT:?}, got {message:?}"
        );
        assert!(
            !text.contains(&name),
            "case {case:?}: expected the conflict to disclose no name, but it echoed {name:?}: \
             {text}"
        );

        let project = json!({ "projectName": name, "standardOutputConfiguration": {} });
        let (status, _, payload) =
            call(&router, "PUT", "/data-automation-projects/", &project).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first CreateDataAutomationProject to be 201, got \
             {status} with {payload:?}"
        );
        let (status, text, payload) =
            call(&router, "PUT", "/data-automation-projects/", &project).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the modelled ConflictException status 409, got {status} \
             with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        assert_eq!(
            message, PROJECT_NAME_CONFLICT,
            "case {case:?}: expected {PROJECT_NAME_CONFLICT:?}, got {message:?}"
        );
        assert!(
            !text.contains(&name),
            "case {case:?}: expected the conflict to disclose no name, but it echoed {name:?}: \
             {text}"
        );
    }
}

/// One request carrying a value outside a constraint its member declares.
struct ValidationCase {
    case: &'static str,
    uri: &'static str,
    body: Value,
    /// A value the answer must NOT carry, for a member whose value this service
    /// withholds; `None` where the member's value is one the refusal does echo.
    withheld: Option<String>,
    expected: String,
}

/// The unanchored form of the one expression both name shapes declare
/// (`^[a-zA-Z0-9-_]+$`, source: `smithy_query(shape="BlueprintName")`).
const NAME_PATTERN: &str = "[a-zA-Z0-9-_]+";

/// The unanchored form of the expression `ClientToken` declares
/// (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, source: `smithy_query(shape="ClientToken")`).
/// `ClientToken` is NOT `@sensitive`, so a clause about it still echoes its value.
const CLIENT_TOKEN_PATTERN: &str = "[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}";

/// A clause as this service states one: the member path, the requirement, and the value
/// half only where the member's value is not withheld.
fn one_error(value: Option<&str>, member: &str, requirement: &str) -> String {
    match value {
        Some(value) => format!(
            "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
             constraint: {requirement}"
        ),
        None => format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
             {requirement}"
        ),
    }
}

/// The constraint-refusal rows: both creates, both constraint classes a string member can
/// break, members whose value is withheld and one whose value is not, and names drawn
/// from outside the character class as well as past the length bound.
fn validation_cases() -> Vec<ValidationCase> {
    // A token that satisfies `@length(33..256)` and breaks only the pattern, so exactly
    // one clause is stated about it.
    let spaced_token = format!("{} {}", "a".repeat(20), "b".repeat(20));
    let long_name = "a".repeat(129);
    let long_description = "d".repeat(301);
    vec![
        ValidationCase {
            case: "blueprintName carrying a space",
            uri: "/blueprints/",
            body: json!({ "blueprintName": "has space", "type": "DOCUMENT", "schema": "{}" }),
            withheld: Some("has space".to_string()),
            expected: one_error(
                None,
                "blueprintName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        },
        ValidationCase {
            case: "blueprintName carrying a slash",
            uri: "/blueprints/",
            body: json!({ "blueprintName": "bad/name", "type": "VIDEO", "schema": "{}" }),
            withheld: Some("bad/name".to_string()),
            expected: one_error(
                None,
                "blueprintName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        },
        ValidationCase {
            case: "blueprintName one character past the length upper bound",
            uri: "/blueprints/",
            body: json!({ "blueprintName": long_name, "type": "AUDIO", "schema": "{}" }),
            withheld: Some(long_name.clone()),
            expected: one_error(
                None,
                "blueprintName",
                "Member must have length less than or equal to 128",
            ),
        },
        ValidationCase {
            case: "schema under its length lower bound: a withheld member that is not a name",
            uri: "/blueprints/",
            body: json!({ "blueprintName": "well-formed", "type": "IMAGE", "schema": "" }),
            withheld: None,
            expected: one_error(
                None,
                "schema",
                "Member must have length greater than or equal to 1",
            ),
        },
        ValidationCase {
            case: "clientToken outside its pattern: a member whose value is NOT withheld",
            uri: "/blueprints/",
            body: json!({
                "blueprintName": "well-formed",
                "type": "DOCUMENT",
                "schema": "{}",
                "clientToken": spaced_token,
            }),
            withheld: None,
            expected: one_error(
                Some(&spaced_token),
                "clientToken",
                &format!(
                    "Member must satisfy regular expression pattern: {CLIENT_TOKEN_PATTERN}"
                ),
            ),
        },
        ValidationCase {
            case: "projectName carrying a slash",
            uri: "/data-automation-projects/",
            body: json!({
                "projectName": "bad/name",
                "standardOutputConfiguration": {},
            }),
            withheld: Some("bad/name".to_string()),
            expected: one_error(
                None,
                "projectName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        },
        ValidationCase {
            case: "projectName carrying a dot",
            uri: "/data-automation-projects/",
            body: json!({
                "projectName": "bad.name",
                "standardOutputConfiguration": {},
            }),
            withheld: Some("bad.name".to_string()),
            expected: one_error(
                None,
                "projectName",
                &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
            ),
        },
        ValidationCase {
            case: "projectName one character past the length upper bound",
            uri: "/data-automation-projects/",
            body: json!({
                "projectName": long_name,
                "standardOutputConfiguration": {},
            }),
            withheld: Some(long_name.clone()),
            expected: one_error(
                None,
                "projectName",
                "Member must have length less than or equal to 128",
            ),
        },
        ValidationCase {
            case: "projectDescription past its length upper bound: another withheld member",
            uri: "/data-automation-projects/",
            body: json!({
                "projectName": "well-formed",
                "projectDescription": long_description,
                "standardOutputConfiguration": {},
            }),
            withheld: Some(long_description.clone()),
            expected: one_error(
                None,
                "projectDescription",
                "Member must have length less than or equal to 300",
            ),
        },
    ]
}

/// RULE: a value outside a constraint its member declares is refused `400`
/// `ValidationException` naming the member path and the requirement, with a `@pattern`
/// quoted WITHOUT the `^`/`$` a whole-value pattern wraps it in, and with no value at all
/// for a member the model marks `@sensitive` — while a member the model does not mark
/// keeps the value half.
#[tokio::test]
async fn a_constraint_refusal_withholds_a_sensitive_value_and_drops_the_pattern_anchors() {
    let router = emulator();
    for row in validation_cases() {
        let ValidationCase {
            case,
            uri,
            body,
            withheld,
            expected,
        } = row;
        let (status, text, payload) = call(&router, "PUT", uri, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled ValidationException status 400, got {status} \
             with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload:?}"
        );
        let message = string_at(&payload, "message", case);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        if let Some(value) = withheld {
            assert!(
                !text.contains(&value),
                "case {case:?}: expected the refusal to disclose no value, but it echoed \
                 {value:?}: {text}"
            );
        }
    }
}
