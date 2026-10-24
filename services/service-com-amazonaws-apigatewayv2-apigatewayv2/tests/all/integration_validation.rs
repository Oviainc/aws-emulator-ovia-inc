// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rules `CreateIntegration` holds its input to.
//!
//! Three rules, each asserted over a table rather than over one request:
//!
//! * **An integration needs an `IntegrationUri` only when the URI is what names what it
//!   invokes.** `MOCK` invokes nothing, and an integration carrying an
//!   `IntegrationSubtype` names the AWS service action to invoke there
//!   (`CreateIntegration$IntegrationSubtype`: "Specifies the AWS service action to
//!   invoke"); `IntegrationUri` is not a required member of the input shape (only
//!   `ApiId` and `IntegrationType` are, `smithy_query(query="operation",
//!   operation="CreateIntegration")`), so neither form is incomplete and both are
//!   created.
//! * **`PayloadFormatVersion` is constrained by the integration type**: "Supported
//!   values for Lambda proxy integrations are 1.0 and 2.0. For all other integrations,
//!   1.0 is the only supported value" (`Integration$PayloadFormatVersion`).
//! * **A template selection expression may only name variables the service evaluates**:
//!   an unescaped `$` begins a variable reference, so the literal selection key
//!   `$default` has to be written escaped (`\$default`).
//!
//! The rows span what each rule generalises over: the integration type, whether the
//! request carries a subtype, a URI, or both, the parent api's protocol, the payload
//! format version (each published value, one outside them), and the form of the
//! selection expression (a bare variable, a braced one, an escaped literal, a plain
//! literal, a variable the service does not define). The recorded case that filed the
//! issue — an `SQS-SendMessage` subtype integration with no URI — is ONE row of the
//! first table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The subtypes AWS names for first-class service integrations, as a request spells
/// them (`<Service>-<Action>`). `IntegrationSubtype` is an unconstrained string in the
/// model, so these are VALUES a caller sends, and the rule under test does not depend on
/// which one.
const SERVICE_ACTION_SUBTYPES: &[&str] = &[
    "SQS-SendMessage",
    "EventBridge-PutEvents",
    "StepFunctions-StartExecution",
    "StepFunctions-StartSyncExecution",
    "SQS-ReceiveMessage",
];

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Send one CreateIntegration and answer `(status, body)`.
async fn create_integration(
    emulator: &MultiServiceRouter,
    api_id: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(request.clone()),
    )
    .await
}

/// How many integrations the api holds, so a refused create can be shown to have stored
/// nothing.
async fn integration_count(emulator: &MultiServiceRouter, api_id: &str) -> usize {
    let (status, listed) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/integrations"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetIntegrations, got {status} {listed}"
    );
    listed["items"].as_array().map_or(0, Vec::len)
}

/// One row of the `IntegrationUri` table.
struct UriRow {
    case: String,
    /// `true` for an HTTP api, `false` for a WebSocket one.
    http: bool,
    request: Value,
    /// The members the stored record must report back, once created.
    expect_members: Vec<(&'static str, Value)>,
    /// `None` when the create must be accepted.
    expect_refused: bool,
}

fn uri_rows() -> Vec<UriRow> {
    let mut rows = Vec::new();

    // Every first-class service integration: AWS_PROXY plus a subtype, credentials and
    // request parameters, and NO IntegrationUri.
    for subtype in SERVICE_ACTION_SUBTYPES {
        rows.push(UriRow {
            case: format!("AWS_PROXY with the {subtype} subtype and no IntegrationUri"),
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationSubtype": subtype,
                "payloadFormatVersion": "1.0",
                "credentialsArn": "arn:aws:iam::000000000000:role/apigw",
                "requestParameters": {
                    "QueueUrl": "$request.header.queue",
                    "MessageBody": "$request.body.message"
                }
            }),
            expect_members: vec![
                ("integrationType", json!("AWS_PROXY")),
                ("integrationSubtype", json!(*subtype)),
                (
                    "credentialsArn",
                    json!("arn:aws:iam::000000000000:role/apigw"),
                ),
                (
                    "requestParameters",
                    json!({
                        "QueueUrl": "$request.header.queue",
                        "MessageBody": "$request.body.message"
                    }),
                ),
            ],
            expect_refused: false,
        });
    }

    rows.push(UriRow {
        case: "a subtype integration carrying a URI as well".to_string(),
        http: true,
        request: json!({
            "integrationType": "AWS_PROXY",
            "integrationSubtype": "SQS-SendMessage",
            "integrationUri": "arn:aws:sqs:eu-west-1:000000000000:a-queue",
            "payloadFormatVersion": "1.0",
            "credentialsArn": "arn:aws:iam::000000000000:role/apigw"
        }),
        expect_members: vec![
            ("integrationSubtype", json!("SQS-SendMessage")),
            (
                "integrationUri",
                json!("arn:aws:sqs:eu-west-1:000000000000:a-queue"),
            ),
        ],
        expect_refused: false,
    });
    rows.push(UriRow {
        case: "a subtype integration with no credentials, which are optional".to_string(),
        http: true,
        request: json!({
            "integrationType": "AWS_PROXY",
            "integrationSubtype": "EventBridge-PutEvents",
            "payloadFormatVersion": "1.0"
        }),
        expect_members: vec![("integrationSubtype", json!("EventBridge-PutEvents"))],
        expect_refused: false,
    });
    rows.push(UriRow {
        case: "MOCK, which invokes nothing, on a WebSocket api".to_string(),
        http: false,
        request: json!({ "integrationType": "MOCK" }),
        expect_members: vec![("integrationType", json!("MOCK"))],
        expect_refused: false,
    });
    rows.push(UriRow {
        case: "AWS_PROXY naming neither a URI nor a subtype".to_string(),
        http: true,
        request: json!({ "integrationType": "AWS_PROXY", "payloadFormatVersion": "1.0" }),
        expect_members: Vec::new(),
        expect_refused: true,
    });
    rows.push(UriRow {
        case: "HTTP_PROXY naming no URI".to_string(),
        http: true,
        request: json!({ "integrationType": "HTTP_PROXY", "payloadFormatVersion": "1.0" }),
        expect_members: Vec::new(),
        expect_refused: true,
    });
    rows.push(UriRow {
        case: "a WebSocket AWS integration naming no URI".to_string(),
        http: false,
        request: json!({ "integrationType": "AWS" }),
        expect_members: Vec::new(),
        expect_refused: true,
    });
    rows.push(UriRow {
        case: "a subtype on an integration that is not AWS_PROXY".to_string(),
        http: true,
        request: json!({
            "integrationType": "HTTP_PROXY",
            "integrationSubtype": "SQS-SendMessage",
            "payloadFormatVersion": "1.0"
        }),
        expect_members: Vec::new(),
        expect_refused: true,
    });

    rows
}

#[tokio::test]
async fn an_integration_needs_a_uri_only_when_the_uri_names_what_it_invokes() {
    let emulator = emulator();

    for (index, row) in uri_rows().into_iter().enumerate() {
        let request = if row.http {
            http_api(&format!("uri-rule-http-{index}"))
        } else {
            websocket_api(&format!("uri-rule-socket-{index}"))
        };
        let api_id = create_api(&emulator, &row.case, &request).await;

        let (status, body) = create_integration(&emulator, &api_id, &row.request).await;
        if row.expect_refused {
            assert_eq!(
                status, 400,
                "case {:?}: expected 400 from CreateIntegration, got {status} {body}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {:?}: expected BadRequestException, got {body}",
                row.case
            );
            assert_eq!(
                integration_count(&emulator, &api_id).await,
                0,
                "case {:?}: a refused create must store no integration",
                row.case
            );
            continue;
        }

        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateIntegration, got {status} {body}",
            row.case
        );
        let integration_id = as_str(&body, "integrationId");
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetIntegration, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, body,
            "case {:?}: GetIntegration must report what CreateIntegration stored",
            row.case
        );
        for (member, expected) in &row.expect_members {
            assert_eq!(
                fetched.get(*member),
                Some(expected),
                "case {:?}: wrong {member}, got {fetched}",
                row.case
            );
        }
        // An integration created from a subtype alone names no URI, and reports none.
        if row.request.get("integrationUri").is_none() {
            assert_eq!(
                fetched.get("integrationUri"),
                None,
                "case {:?}: an integration created with no URI must report none, got {fetched}",
                row.case
            );
        }
    }
}

/// One row of the payload-format-version table.
struct PayloadRow {
    case: String,
    http: bool,
    request: Value,
    accepted: bool,
}

fn payload_rows() -> Vec<PayloadRow> {
    let lambda = "arn:aws:lambda:eu-west-1:000000000000:function:handler";
    let mut rows = Vec::new();

    // A Lambda proxy integration: both published values, and one outside them.
    for (version, accepted) in [("1.0", true), ("2.0", true), ("3.0", false), ("2", false)] {
        rows.push(PayloadRow {
            case: format!("AWS_PROXY invoking a function with PayloadFormatVersion {version}"),
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": lambda,
                "payloadFormatVersion": version
            }),
            accepted,
        });
    }
    // Every other integration: 1.0 only. An AWS_PROXY integration naming a service
    // action through its subtype is one of them — it invokes no function.
    for (version, accepted) in [("1.0", true), ("2.0", false)] {
        rows.push(PayloadRow {
            case: format!("HTTP_PROXY with PayloadFormatVersion {version}"),
            http: true,
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": version
            }),
            accepted,
        });
        rows.push(PayloadRow {
            case: format!("an SQS-SendMessage subtype with PayloadFormatVersion {version}"),
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationSubtype": "SQS-SendMessage",
                "payloadFormatVersion": version,
                "credentialsArn": "arn:aws:iam::000000000000:role/apigw"
            }),
            accepted,
        });
        rows.push(PayloadRow {
            case: format!("a WebSocket AWS integration with PayloadFormatVersion {version}"),
            http: false,
            request: json!({
                "integrationType": "AWS",
                "integrationUri": lambda,
                "payloadFormatVersion": version
            }),
            accepted,
        });
    }

    rows
}

#[tokio::test]
async fn the_payload_format_version_an_integration_may_carry_follows_its_type() {
    let emulator = emulator();

    for (index, row) in payload_rows().into_iter().enumerate() {
        let request = if row.http {
            http_api(&format!("payload-http-{index}"))
        } else {
            websocket_api(&format!("payload-socket-{index}"))
        };
        let api_id = create_api(&emulator, &row.case, &request).await;

        let (status, body) = create_integration(&emulator, &api_id, &row.request).await;
        if row.accepted {
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from CreateIntegration, got {status} {body}",
                row.case
            );
            assert_eq!(
                as_str(&body, "payloadFormatVersion"),
                as_str(&row.request, "payloadFormatVersion"),
                "case {:?}: the stored PayloadFormatVersion must be the one sent, got {body}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected 400 from CreateIntegration, got {status} {body}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {:?}: expected BadRequestException, got {body}",
                row.case
            );
            assert_eq!(
                integration_count(&emulator, &api_id).await,
                0,
                "case {:?}: a refused create must store no integration",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_template_selection_expression_may_only_name_variables_the_service_evaluates() {
    let emulator = emulator();

    // Every row is a WebSocket `AWS` integration: "The template selection expression for
    // the integration. Supported only for WebSocket APIs"
    // (`Integration$TemplateSelectionExpression`).
    let rows = [
        ("a bare request variable", "$request.body.action", true),
        ("a braced request variable", "${request.body.action}", true),
        ("a context variable", "$context.routeKey", true),
        ("a stage variable", "$stageVariables.stage", true),
        ("the escaped selection key", "\\$default", true),
        ("a plain literal naming no variable", "action", true),
        ("the unescaped selection key", "$default", false),
        ("a variable the service does not define", "$notAVariable", false),
        ("an unescaped key beside a variable", "$request.body.x $default", false),
    ];

    for (index, (case, expression, accepted)) in rows.into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            case,
            &websocket_api(&format!("template-expression-{index}")),
        )
        .await;
        let request = json!({
            "integrationType": "AWS",
            "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:ws",
            "integrationMethod": "POST",
            "requestTemplates": { "application/json": "{\"body\": $input.json('$')}" },
            "templateSelectionExpression": expression
        });

        let (status, body) = create_integration(&emulator, &api_id, &request).await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?} ({expression:?}): expected 201 from CreateIntegration, got {status} {body}"
            );
            assert_eq!(
                as_str(&body, "templateSelectionExpression"),
                expression,
                "case {case:?}: the stored expression must be the one sent, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} ({expression:?}): expected 400 from CreateIntegration, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            assert!(
                as_str(&body, "message").contains("selection expression"),
                "case {case:?}: the refusal must say what it refused, got {body}"
            );
            assert_eq!(
                integration_count(&emulator, &api_id).await,
                0,
                "case {case:?}: a refused create must store no integration"
            );
        }
    }
}
