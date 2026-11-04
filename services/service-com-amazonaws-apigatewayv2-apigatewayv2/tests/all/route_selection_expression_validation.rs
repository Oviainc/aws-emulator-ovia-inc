// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `RouteSelectionExpression` a `CreateApi` or an
//! `UpdateApi` may store on an api.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees.
//!
//! The rule these cases assert, one level above the recorded case (one WEBSOCKET api
//! patched from `$request.body.action` to a bare `$request.body`):
//!
//! > A `RouteSelectionExpression` has to name a variable the api's `ProtocolType` selects
//! > a route key with. On a WEBSOCKET api, whose route keys are matched against the
//! > message body, that is a FIELD of the body (`$request.body.<path>`); an expression
//! > naming anything else — the whole body, another part of the request, a namespace the
//! > service does not evaluate — selects no route key, so it is refused with HTTP 400 /
//! > `BadRequestException` and the message `Unexpected variable in selection expression: `
//! > plus the variable reference as the request wrote it, and NOTHING is written: the
//! > create mints no api and the patch leaves the stored expression exactly as it was.
//!
//! The rows span what the rule generalises over: the form of the reference (bare,
//! braced, dotted path, trailing dot, mixed case), which part of the request it names,
//! how many references the expression makes, the api's `ProtocolType`, and whether the
//! expression arrives on the create or on a later patch.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The sentence API Gateway refuses an unselectable variable with — fixed text; the
/// variable reference the request wrote follows it.
const UNEXPECTED_VARIABLE: &str = "Unexpected variable in selection expression: ";

/// The expression a WEBSOCKET api is created with before a patch is weighed, and the one
/// a refused patch must leave in place.
const STORED_EXPRESSION: &str = "$request.body.action";

/// The expression an HTTP api carries: "For HTTP APIs, the routeSelectionExpression must
/// be ${request.method} ${request.path}" (`CreateApi$RouteSelectionExpression`).
const HTTP_EXPRESSION: &str = "$request.method $request.path";

/// One expression a WEBSOCKET api may carry, and what each is answered with.
struct Row {
    case: &'static str,
    expression: &'static str,
    /// The variable reference the refusal names, or `None` for an expression that is
    /// accepted and stored.
    refused_reference: Option<&'static str>,
}

/// The table: every row is a `RouteSelectionExpression` on a WEBSOCKET api.
///
/// The accepted rows are the forms a body field is written in; the refused ones name a
/// part of the request a route key cannot be selected from. The recorded case is the
/// second refused row (`$request.body`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a body field, the documented form",
            expression: "$request.body.action",
            refused_reference: None,
        },
        Row {
            case: "a differently named body field",
            expression: "$request.body.kind",
            refused_reference: None,
        },
        Row {
            case: "a nested body field",
            expression: "$request.body.message.action",
            refused_reference: None,
        },
        Row {
            case: "the braced form of a body field",
            expression: "${request.body.action}",
            refused_reference: None,
        },
        Row {
            case: "a body field named in another case",
            expression: "$Request.Body.Action",
            refused_reference: None,
        },
        Row {
            case: "the whole body, with no field selector",
            expression: "$request.body",
            refused_reference: Some("$request.body"),
        },
        Row {
            case: "the braced whole body",
            expression: "${request.body}",
            refused_reference: Some("$request.body"),
        },
        Row {
            case: "a body path with no field after the dot",
            expression: "$request.body.",
            refused_reference: Some("$request.body."),
        },
        Row {
            case: "the method, which a frame does not carry",
            expression: "$request.method",
            refused_reference: Some("$request.method"),
        },
        Row {
            case: "a request header",
            expression: "$request.header.x-action",
            refused_reference: Some("$request.header.x-action"),
        },
        Row {
            case: "a stage variable",
            expression: "$stageVariables.action",
            refused_reference: Some("$stageVariables.action"),
        },
        Row {
            case: "a context key",
            expression: "$context.routeKey",
            refused_reference: Some("$context.routeKey"),
        },
        Row {
            case: "two references, the second unselectable",
            expression: "$request.body.action $request.path",
            refused_reference: Some("$request.path"),
        },
        Row {
            case: "a namespace the service evaluates nothing from",
            expression: "$message.action",
            refused_reference: Some("$message"),
        },
    ]
}

/// The CreateApi request for a WEBSOCKET api carrying `expression`.
fn websocket_api(name: &str, expression: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": expression
    })
}

/// How many apis the account holds.
async fn api_count(emulator: &smithy_server_runtime::MultiServiceRouter) -> usize {
    let (status, body) = call(emulator, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "GetApis must answer 200, got {status} {body}");
    body.get("items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[tokio::test]
async fn create_api_stores_only_a_route_selection_expression_a_websocket_api_can_route_with() {
    for (index, row) in rows().into_iter().enumerate() {
        // One emulator per row, so the api count below counts only this row's create.
        let emulator = emulator();
        let (status, body) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(websocket_api(
                &format!("selection-{index}"),
                row.expression,
            )),
        )
        .await;

        match row.refused_reference {
            None => {
                assert_eq!(
                    status, 201,
                    "case {:?}: expected 201 for {:?}, got {status} {body}",
                    row.case, row.expression
                );
                assert_eq!(
                    as_str(&body, "routeSelectionExpression"),
                    row.expression,
                    "case {:?}: the created api must report the expression it was given, \
                     got {body}",
                    row.case
                );
                assert_eq!(
                    api_count(&emulator).await,
                    1,
                    "case {:?}: the accepted create must have minted one api",
                    row.case
                );
            }
            Some(reference) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected 400 for {:?}, got {status} {body}",
                    row.case, row.expression
                );
                assert_eq!(
                    error_code(&body),
                    "BadRequestException",
                    "case {:?}: expected BadRequestException, got {body}",
                    row.case
                );
                assert_eq!(
                    as_str(&body, "message"),
                    format!("{UNEXPECTED_VARIABLE}{reference}"),
                    "case {:?}: expected the refusal to name {reference:?}, got {body}",
                    row.case
                );
                assert_eq!(
                    api_count(&emulator).await,
                    0,
                    "case {:?}: the refused create must have minted nothing",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn update_api_keeps_the_stored_expression_when_the_patch_cannot_route() {
    for (index, row) in rows().into_iter().enumerate() {
        let emulator = emulator();
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(websocket_api(
                &format!("selection-patch-{index}"),
                STORED_EXPRESSION,
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: the api under patch must be created, got {status} {created}",
            row.case
        );
        let api_id = as_str(&created, "apiId");

        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}"),
            Some(json!({ "routeSelectionExpression": row.expression })),
        )
        .await;

        let expected_after = match row.refused_reference {
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected 200 patching to {:?}, got {status} {body}",
                    row.case, row.expression
                );
                row.expression
            }
            Some(reference) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected 400 patching to {:?}, got {status} {body}",
                    row.case, row.expression
                );
                assert_eq!(
                    error_code(&body),
                    "BadRequestException",
                    "case {:?}: expected BadRequestException, got {body}",
                    row.case
                );
                assert_eq!(
                    as_str(&body, "message"),
                    format!("{UNEXPECTED_VARIABLE}{reference}"),
                    "case {:?}: expected the refusal to name {reference:?}, got {body}",
                    row.case
                );
                STORED_EXPRESSION
            }
        };

        // The api reports what it CARRIES: the patched expression when the patch was
        // taken, the one it already had when it was refused.
        let (status, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetApi must answer 200, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "routeSelectionExpression"),
            expected_after,
            "case {:?}: expected the api to carry {expected_after:?}, got {fetched}",
            row.case
        );
    }
}

/// An HTTP api selects a route by method and path, so the body-field rule is not its:
/// its documented expression is accepted on the create and on a patch, while a reference
/// to a namespace the service evaluates nothing from is refused for either protocol.
#[tokio::test]
async fn an_http_api_keeps_its_own_route_selection_expression_rule() {
    // (case, the expression, the refusal's variable reference or None)
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("the documented HTTP expression", HTTP_EXPRESSION, None),
        (
            "the braced HTTP expression",
            "${request.method} ${request.path}",
            None,
        ),
        (
            "a namespace the service evaluates nothing from",
            "$message.action",
            Some("$message"),
        ),
    ];

    for (index, (case, expression, refused_reference)) in rows.into_iter().enumerate() {
        let emulator = emulator();
        let (status, body) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({
                "name": format!("http-selection-{index}"),
                "protocolType": "HTTP",
                "routeSelectionExpression": expression
            })),
        )
        .await;

        match refused_reference {
            None => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 for {expression:?}, got {status} {body}"
                );
                assert_eq!(
                    as_str(&body, "routeSelectionExpression"),
                    expression,
                    "case {case:?}: the created api must report {expression:?}, got {body}"
                );
            }
            Some(reference) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 for {expression:?}, got {status} {body}"
                );
                assert_eq!(
                    as_str(&body, "message"),
                    format!("{UNEXPECTED_VARIABLE}{reference}"),
                    "case {case:?}: expected the refusal to name {reference:?}, got {body}"
                );
            }
        }
    }
}
