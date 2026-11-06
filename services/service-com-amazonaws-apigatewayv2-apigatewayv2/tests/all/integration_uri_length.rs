// SPDX-License-Identifier: CC0-1.0

//! The LENGTH window an `IntegrationUri` falls in, enforced where the member is written.
//!
//! The rule these cases assert: **`Integration$IntegrationUri` targets
//! `UriWithLengthBetween1And2048`, "a string representation of a URI with a length between
//! [1-2048]" (`smithy_query(query="shape", shape="UriWithLengthBetween1And2048")`), and the
//! shape carries no `@length` trait — so the operations that WRITE the member hold it to
//! that window: a URI at the bound is accepted and read back whole, one past the bound is
//! refused 400 `BadRequestException` naming the member and the window, and nothing is
//! stored.** It is the same bound the data plane resolves a stage variable into that URI
//! under (`stage_variable_backend_selection.rs`), so a template that is admitted here
//! cannot grow an upstream past what the member may hold.
//!
//! The rows span what the rule generalises over: the integration TYPE (an `HTTP_PROXY`
//! upstream, whose URI this service leaves in the form it came, and an `AWS_PROXY` Lambda
//! ARN, whose form is checked as well), the length relative to the window (the published
//! bound, one character past it, and a short one), and the OPERATION that writes the member
//! (`CreateIntegration` and `UpdateIntegration`).
//!
//! Which URIs are refused for their SHAPE rather than their length — an `AWS_PROXY` URI that
//! is no ARN, a Lambda ARN naming no function — is `route_target_forms.rs`'s rule and is not
//! restated here.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::INTEGRATION_URI_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// Create an HTTP api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {api}"
    );
    as_str(&api, "apiId")
}

/// An `HTTP_PROXY` upstream whose URI is exactly `length` characters long: the scheme and
/// host, then as much path as the length asks for.
fn http_upstream_of_length(length: usize) -> String {
    let head = "https://backend.example/";
    format!("{head}{}", "a".repeat(length.saturating_sub(head.len())))
}

/// A Lambda invoking URI whose ARN is exactly `length` characters long, padded in the
/// function NAME so the ARN keeps the `function:<name>` resource this service requires of
/// one.
fn lambda_uri_of_length(length: usize) -> String {
    let head = "arn:aws:lambda:eu-west-1:000000000000:function:f";
    format!("{head}{}", "n".repeat(length.saturating_sub(head.len())))
}

/// One row: the case, the `CreateIntegration` body, and whether the member's window admits
/// the URI it carries.
struct Row {
    case: String,
    request: Value,
    accepted: bool,
}

fn rows() -> Vec<Row> {
    let (_, max) = INTEGRATION_URI_LEN;
    vec![
        Row {
            case: "an HTTP_PROXY upstream well inside the window".to_string(),
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://backend.example/items",
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            }),
            accepted: true,
        },
        Row {
            case: format!("an HTTP_PROXY upstream at the published {max}-character bound"),
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": http_upstream_of_length(max),
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            }),
            accepted: true,
        },
        Row {
            case: "an HTTP_PROXY upstream one character past the bound".to_string(),
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": http_upstream_of_length(max + 1),
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            }),
            accepted: false,
        },
        Row {
            case: format!("a Lambda AWS_PROXY ARN at the {max}-character bound"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": lambda_uri_of_length(max),
                "payloadFormatVersion": "2.0"
            }),
            accepted: true,
        },
        Row {
            case: "a Lambda AWS_PROXY ARN one character past the bound".to_string(),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": lambda_uri_of_length(max + 1),
                "payloadFormatVersion": "2.0"
            }),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn an_integration_uri_is_held_to_the_window_its_own_shape_documents() {
    let emulator = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = &row.case;
        let api_id = create_api(&emulator, case, &format!("uri-length-{index}")).await;
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(row.request.clone()),
        )
        .await;

        if row.accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateIntegration, got {status} {body}"
            );
            // The whole URI is stored, not a truncation of it.
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
                "case {case:?}: expected 200 from GetIntegration, got {status} {fetched}"
            );
            assert_eq!(
                fetched.get("integrationUri"),
                row.request.get("integrationUri"),
                "case {case:?}: expected the URI the request named, got {fetched}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateIntegration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let message = as_str(&body, "message");
        assert!(
            message.contains("IntegrationUri") && message.contains(&INTEGRATION_URI_LEN.1.to_string()),
            "case {case:?}: expected the refusal to name the member and the window, got {body}"
        );
        // Nothing was stored: the api holds no integration at all.
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegrations, got {status} {listed}"
        );
        assert_eq!(
            listed
                .get("items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            0,
            "case {case:?}: a refused create must store no integration, got {listed}"
        );
    }
}

#[tokio::test]
async fn updating_an_integration_uri_is_held_to_the_same_window() {
    let emulator = emulator();
    let case = "an update past the window";
    let api_id = create_api(&emulator, case, "uri-length-update").await;
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://backend.example/items",
            "integrationMethod": "ANY",
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    let integration_id = as_str(&created, "integrationId");
    let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}");

    let (status, refused) = call(
        &emulator,
        "PATCH",
        &uri,
        Some(json!({ "integrationUri": http_upstream_of_length(INTEGRATION_URI_LEN.1 + 1) })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from UpdateIntegration, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {refused}"
    );

    // The refused update changed nothing: the integration still reports the URI it was
    // created with.
    let (status, read) = call(&emulator, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetIntegration, got {status} {read}"
    );
    assert_eq!(
        as_str(&read, "integrationUri"),
        "https://backend.example/items",
        "case {case:?}: expected the refused update to leave the URI as it was, got {read}"
    );

    // And an update AT the bound is accepted and read back whole.
    let at_the_bound = http_upstream_of_length(INTEGRATION_URI_LEN.1);
    let (status, updated) = call(
        &emulator,
        "PATCH",
        &uri,
        Some(json!({ "integrationUri": at_the_bound })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an update at the bound\": expected 200 from UpdateIntegration, got {status} \
         {updated}"
    );
    assert_eq!(
        as_str(&updated, "integrationUri"),
        at_the_bound,
        "case \"an update at the bound\": expected the URI the request named, got {updated}"
    );
}
