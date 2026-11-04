// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a route may TARGET, what an integration's
//! `IntegrationUri` may NAME, and which route quick create manages.
//!
//! Five rules, each asserted over a table rather than over one request:
//!
//! * **A route's `Target` addresses an integration of the route's own api as
//!   `integrations/{IntegrationId}`.** That is the form `CreateRoute$Target` takes and
//!   the one every route reader reports back, so a value of any other FORM names nothing
//!   this service could route to and is refused with `BadRequestException` (400,
//!   `smithy_query(query="operation", operation="CreateRoute")`) naming the route and the
//!   form a target takes; a value of the right form naming an integration the api does
//!   not hold is refused by the same error naming the type and the id the request
//!   carried. Either refusal persists no route, which is what keeps the route key free
//!   for the create that gets it right.
//! * **A target is reported in the form it was supplied**, `integrations/{IntegrationId}`
//!   and not the bare id, by every reader of the route and by the writers.
//! * **An `IntegrationUri` of a type that integrates "with an AWS service action" names
//!   the resource that action runs on**, so it has to be a well-formed ARN ("For a Lambda
//!   integration, specify the URI of a Lambda function", `Integration$IntegrationUri`);
//!   one that is not is refused with the sentence API Gateway answers an unreadable ARN
//!   with. The types that name "a fully-qualified URL" instead are left as they came.
//! * **An `IntegrationType` outside the enum the member targets is no type the service
//!   can read**, so the request is refused exactly as one that left the `@required` member
//!   out: the one sentence naming the member and what it is required for.
//! * **Every resource quick create mints with the api is managed by API Gateway**,
//!   whatever `RouteKey` the request named — the route included
//!   (`Route$ApiGatewayManaged`) — while a route the caller asked for through
//!   `CreateRoute` is its own.
//!
//! The rows span what each rule generalises over: the form of the target (the right form,
//! the prefix alone, a bare id, a trailing or leading separator, another resource type, an
//! ARN, a URL, a selection key), the id a well-formed target names, the integration type
//! and the form of its URI (a function ARN, a qualified one, another vendor's resource, a
//! malformed ARN, a URL), how an `IntegrationType` is unreadable, the api's protocol, and
//! the route key quick create mints under.
//! The recorded cases that filed the issue — a `CreateIntegration` with no
//! `PayloadFormatVersion`, one whose `IntegrationUri` is not an ARN, a route target of
//! `integrations/abc1234`, and a quick-create api's listed route — are ONE row each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, call_reading_headers, emulator, error_code};

/// The sentence API Gateway refuses a `CreateIntegration` that leaves
/// `PayloadFormatVersion` out for an HTTP api with (the recorded 400 of the fixture that
/// filed this issue): fixed text, naming the member and the resource it is required for.
const PAYLOAD_FORMAT_VERSION_REQUIRED: &str =
    "PayloadFormatVersion is a required parameter for integration";

/// The sentence API Gateway refuses a caller-supplied ARN it cannot read with: fixed
/// text, naming neither the member nor the value.
const INVALID_ARN: &str = "Invalid ARN specified in the request";

/// The sentence API Gateway refuses a `CreateIntegration` that names no readable
/// `IntegrationType` with: fixed text naming the member and what it is required for.
const INTEGRATION_TYPE_REQUIRED: &str =
    "IntegrationType is a required parameter for creating integration.";

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

/// The sentence a malformed target is refused with, as the service builds it: the route
/// the target was sent for, and the form a target takes. A create has minted no `RouteId`
/// yet, so `route` is the absent identifier API Gateway reports there.
fn malformed_target(route: &str) -> String {
    format!(
        "Unexpected or malformed target in route {route}. Correct format should be \
         integrations/<integration_id>."
    )
}

/// The sentence a well-formed target naming an integration the api does not hold is
/// refused with: the type the target addresses and the id it carried.
fn unresolved_target(integration_id: &str) -> String {
    format!("Unable to find route target of type integrations with ID {integration_id}")
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

/// Create one HTTP_PROXY integration under `api_id` and answer its `IntegrationId`.
async fn create_integration(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/upstream",
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    as_str(&created, "integrationId")
}

/// The routes the api holds, in the order `GetRoutes` lists them.
async fn routes(emulator: &MultiServiceRouter, api_id: &str) -> Vec<Value> {
    let (status, listed) = call(emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetRoutes, got {status} {listed}"
    );
    listed["items"].as_array().cloned().unwrap_or_default()
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

/// One row of the target-form table: the `Target` a request sends, and what the service
/// must answer for it. `expect_message` is `None` for a target the service accepts, and
/// otherwise the sentence the refusal carries, built from the request.
struct TargetRow {
    case: &'static str,
    /// The target the request carries, built from the api's own `IntegrationId` where the
    /// row needs a resolvable one.
    target: fn(&str) -> String,
    expect_message: Option<fn(&str) -> String>,
}

/// The target forms the rule spans. The id the row's closure receives is the
/// `IntegrationId` of the api under test, so a row that needs a RESOLVABLE target builds
/// one and the rest build a value that is not one.
fn target_rows() -> Vec<TargetRow> {
    vec![
        TargetRow {
            case: "the form a target takes, naming this api's integration",
            target: |integration_id| format!("integrations/{integration_id}"),
            expect_message: None,
        },
        TargetRow {
            case: "the right form naming an integration no api ever minted",
            target: |_| "integrations/abc1234".to_string(),
            expect_message: Some(|_| unresolved_target("abc1234")),
        },
        TargetRow {
            case: "the right form naming another unminted id, so the id in the sentence \
                   is the request's",
            target: |_| "integrations/zzz9999".to_string(),
            expect_message: Some(|_| unresolved_target("zzz9999")),
        },
        TargetRow {
            case: "the type prefix with no id at all",
            target: |_| "integrations".to_string(),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "the type prefix with an empty id",
            target: |_| "integrations/".to_string(),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "the bare IntegrationId, with the type prefix left off",
            target: |integration_id| integration_id.to_string(),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "a leading separator before the type",
            target: |integration_id| format!("/integrations/{integration_id}"),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "a segment after the id",
            target: |integration_id| format!("integrations/{integration_id}/invoke"),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "the type spelled with a capital",
            target: |integration_id| format!("Integrations/{integration_id}"),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "another resource type of the same api",
            target: |integration_id| format!("routes/{integration_id}"),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "the ARN of a Lambda function, which is an integration's URI and not a \
                   route's target",
            target: |_| "arn:aws:lambda:eu-west-1:000000000000:function:handler".to_string(),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "a fully qualified URL",
            target: |_| "https://example.com/upstream".to_string(),
            expect_message: Some(|_| malformed_target("null")),
        },
        TargetRow {
            case: "a selection key",
            target: |_| "$default".to_string(),
            expect_message: Some(|_| malformed_target("null")),
        },
    ]
}

#[tokio::test]
async fn a_route_target_that_is_not_an_integration_reference_is_refused_and_stores_nothing() {
    let emulator = emulator();

    for (index, row) in target_rows().into_iter().enumerate() {
        // One api per row, so "the api holds no route" is about this row's request alone.
        let api_id = create_api(
            &emulator,
            row.case,
            &http_api(&format!("target-form-{index}")),
        )
        .await;
        let integration_id = create_integration(&emulator, row.case, &api_id).await;
        let target = (row.target)(&integration_id);

        // Every row sends the SAME route key, so a row that stored a route it should have
        // refused would be visible as a conflict on the next row's api too.
        let request = json!({ "routeKey": "GET /capture", "target": target });
        let (status, headers, body) = call_reading_headers(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(request),
        )
        .await;

        let Some(expect_message) = row.expect_message else {
            assert_eq!(
                status, 201,
                "case {:?} ({target:?}): expected 201 from CreateRoute, got {status} {body}",
                row.case
            );
            assert_eq!(
                as_str(&body, "target"),
                target,
                "case {:?}: the stored Target must be the one sent, got {body}",
                row.case
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {:?} ({target:?}): expected 400 from CreateRoute, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {:?} ({target:?}): expected BadRequestException, got {body}",
            row.case
        );
        assert_eq!(
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok()),
            Some("BadRequestException"),
            "case {:?}: expected the error type on the response headers, got {headers:?}",
            row.case
        );
        assert_eq!(
            as_str(&body, "message"),
            expect_message(&integration_id),
            "case {:?} ({target:?}): wrong refusal sentence, got {body}",
            row.case
        );
        assert!(
            routes(&emulator, &api_id).await.is_empty(),
            "case {:?} ({target:?}): a refused CreateRoute must store no route",
            row.case
        );

        // And the route key the refused request asked for is still free: the create that
        // names a target this api CAN resolve gets it.
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": "GET /capture",
                "target": format!("integrations/{integration_id}")
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: the key a refused create asked for must still be free, got \
             {status} {created}",
            row.case
        );
        assert_eq!(
            created.get("authorizationType"),
            Some(&json!("NONE")),
            "case {:?}: a route the request gave no authorization type is open, got {created}",
            row.case
        );
        assert_eq!(
            created.get("apiKeyRequired"),
            Some(&json!(false)),
            "case {:?}: a route no request asked an API key for requires none, got {created}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_route_reports_its_target_in_the_form_it_was_supplied() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "target round trip", &http_api("target-round-trip")).await;

    // Several integrations, so the id in the target varies from row to row and the
    // assertion cannot pass by carrying one id.
    for (index, route_key) in ["GET /first", "POST /second", "ANY /{proxy+}", "$default"]
        .into_iter()
        .enumerate()
    {
        let case = format!("route {route_key:?}");
        let integration_id = create_integration(&emulator, &case, &api_id).await;
        let target = format!("integrations/{integration_id}");

        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": route_key, "target": target })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
        );
        assert_eq!(
            as_str(&created, "target"),
            target,
            "case {case:?}: CreateRoute must report the target in the form it was sent, \
             got {created}"
        );
        let route_id = as_str(&created, "routeId");

        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoute, got {status} {fetched}"
        );
        assert_eq!(
            fetched, created,
            "case {case:?}: GetRoute must report what CreateRoute stored"
        );

        let listed = routes(&emulator, &api_id).await;
        assert_eq!(
            listed.len(),
            index + 1,
            "case {case:?}: GetRoutes must list every route the api holds, got {listed:?}"
        );
        let item = listed
            .iter()
            .find(|route| as_str(route, "routeId") == route_id)
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            as_str(&item, "target"),
            target,
            "case {case:?}: GetRoutes must report the target in the form it was sent, \
             got {item}"
        );

        // A patch onto another integration reports the new target in the same form.
        let moved = create_integration(&emulator, &case, &api_id).await;
        let (status, patched) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            Some(json!({ "target": format!("integrations/{moved}") })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateRoute, got {status} {patched}"
        );
        assert_eq!(
            as_str(&patched, "target"),
            format!("integrations/{moved}"),
            "case {case:?}: UpdateRoute must report the patched target in the form it \
             was sent, got {patched}"
        );
    }
}

#[tokio::test]
async fn a_patch_onto_a_malformed_target_names_the_route_and_changes_nothing() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "malformed patch", &http_api("target-patch")).await;
    let integration_id = create_integration(&emulator, "the api's integration", &api_id).await;

    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": "GET /patched",
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateRoute, got {status} {created}"
    );
    let route_id = as_str(&created, "routeId");
    let uri = format!("/v2/apis/{api_id}/routes/{route_id}");

    // The same forms the create refuses, on a route that EXISTS: the sentence names the
    // route being patched, because the service has an id for it here.
    for target in [
        "integrations",
        "integrations/",
        integration_id.as_str(),
        "https://example.com/upstream",
    ] {
        let (status, body) = call(&emulator, "PATCH", &uri, Some(json!({ "target": target }))).await;
        assert_eq!(
            status, 400,
            "case {target:?}: expected 400 from UpdateRoute, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {target:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            malformed_target(&route_id),
            "case {target:?}: the sentence must name the route being patched, got {body}"
        );
        let (_, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            fetched, created,
            "case {target:?}: a refused UpdateRoute must leave the record as it was"
        );
    }
}

/// One row of the `IntegrationUri` table: the api's protocol, the integration the request
/// asks for, and whether the URI it carries names what that type invokes.
struct UriRow {
    case: &'static str,
    /// `true` for an HTTP api, `false` for a WebSocket one.
    http: bool,
    request: Value,
    accepted: bool,
}

/// The integration types and URI forms the rule spans. The ARNs are of the resources an
/// AWS integration invokes — a Lambda function, an Application Load Balancer listener, a
/// Cloud Map service, and API Gateway's own Lambda-invoking path
/// (`Integration$IntegrationUri`: "For a Lambda integration, specify the URI of a Lambda
/// function … For private integrations, all resources must be owned by the same AWS
/// account") — beside values that are not ARNs at all.
fn uri_rows() -> Vec<UriRow> {
    vec![
        UriRow {
            case: "AWS_PROXY naming a Lambda function",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:handler",
                "payloadFormatVersion": "2.0"
            }),
            accepted: true,
        },
        UriRow {
            case: "AWS_PROXY naming a qualified Lambda function",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:handler:3",
                "payloadFormatVersion": "2.0"
            }),
            accepted: true,
        },
        UriRow {
            case: "AWS_PROXY naming a load balancer listener, another vendor's resource",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri":
                    "arn:aws:elasticloadbalancing:eu-west-1:000000000000:listener/app/\
                     orders/6d0ecf831eec9f09/9f4c4b2b1f1f4a21",
                "payloadFormatVersion": "1.0"
            }),
            accepted: true,
        },
        UriRow {
            case: "AWS_PROXY naming a Cloud Map service",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:servicediscovery:eu-west-1:000000000000:service/srv-abc",
                "payloadFormatVersion": "1.0"
            }),
            accepted: true,
        },
        UriRow {
            case: "AWS_PROXY naming a value that is not an ARN at all",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "handler",
                "payloadFormatVersion": "2.0"
            }),
            accepted: false,
        },
        UriRow {
            case: "AWS_PROXY naming an ARN that stops before its resource",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000",
                "payloadFormatVersion": "2.0"
            }),
            accepted: false,
        },
        UriRow {
            case: "AWS_PROXY naming an ARN with no partition",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn::lambda:eu-west-1:000000000000:function:handler",
                "payloadFormatVersion": "2.0"
            }),
            accepted: false,
        },
        UriRow {
            case: "AWS_PROXY naming a Lambda ARN with no function",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function",
                "payloadFormatVersion": "2.0"
            }),
            accepted: false,
        },
        UriRow {
            case: "AWS_PROXY naming a URL, which is an HTTP integration's endpoint",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "2.0"
            }),
            accepted: false,
        },
        UriRow {
            case: "a WebSocket AWS integration naming API Gateway's Lambda-invoking path",
            http: false,
            request: json!({
                "integrationType": "AWS",
                "integrationUri":
                    "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/\
                     arn:aws:lambda:eu-west-1:000000000000:function:ws/invocations",
                "integrationMethod": "POST"
            }),
            accepted: true,
        },
        UriRow {
            case: "a WebSocket AWS integration naming a Lambda function",
            http: false,
            request: json!({
                "integrationType": "AWS",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:ws",
                "integrationMethod": "POST"
            }),
            accepted: true,
        },
        UriRow {
            case: "a WebSocket AWS integration naming a URL",
            http: false,
            request: json!({
                "integrationType": "AWS",
                "integrationUri": "https://example.com/upstream",
                "integrationMethod": "POST"
            }),
            accepted: false,
        },
        UriRow {
            case: "HTTP_PROXY naming a fully qualified URL, which is its endpoint",
            http: true,
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0"
            }),
            accepted: true,
        },
        UriRow {
            case: "a WebSocket HTTP integration naming a URL",
            http: false,
            request: json!({
                "integrationType": "HTTP",
                "integrationUri": "http://example.com/legacy",
                "integrationMethod": "GET"
            }),
            accepted: true,
        },
        UriRow {
            case: "a subtype integration, which names the service action and no URI",
            http: true,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationSubtype": "SQS-SendMessage",
                "payloadFormatVersion": "1.0"
            }),
            accepted: true,
        },
        UriRow {
            case: "a WebSocket MOCK integration, which invokes nothing",
            http: false,
            request: json!({ "integrationType": "MOCK" }),
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn an_integration_uri_that_must_name_an_aws_resource_is_refused_unless_it_is_an_arn() {
    let emulator = emulator();

    for (index, row) in uri_rows().into_iter().enumerate() {
        let api = if row.http {
            http_api(&format!("uri-arn-http-{index}"))
        } else {
            websocket_api(&format!("uri-arn-socket-{index}"))
        };
        let api_id = create_api(&emulator, row.case, &api).await;

        let (status, headers, body) = call_reading_headers(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(row.request.clone()),
        )
        .await;

        if row.accepted {
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from CreateIntegration, got {status} {body}",
                row.case
            );
            // Read back through the implemented reader: the URI the request named is the
            // one the stored record reports.
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
                fetched.get("integrationUri"),
                row.request.get("integrationUri"),
                "case {:?}: the stored IntegrationUri must be the one sent, got {fetched}",
                row.case
            );
            continue;
        }

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
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok()),
            Some("BadRequestException"),
            "case {:?}: expected the error type on the response headers, got {headers:?}",
            row.case
        );
        assert_eq!(
            as_str(&body, "message"),
            INVALID_ARN,
            "case {:?}: wrong refusal sentence, got {body}",
            row.case
        );
        assert_eq!(
            integration_count(&emulator, &api_id).await,
            0,
            "case {:?}: a refused CreateIntegration must store no integration",
            row.case
        );

        // The same rule holds for the PATCH: an integration the api DOES hold cannot be
        // moved onto a URI that names nothing it could invoke.
        let mut accepted = row.request.clone();
        accepted["integrationUri"] = if row.http {
            json!("arn:aws:lambda:eu-west-1:000000000000:function:handler")
        } else {
            json!("arn:aws:lambda:eu-west-1:000000000000:function:ws")
        };
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(accepted),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 creating the integration to patch, got {status} {created}",
            row.case
        );
        let integration_id = as_str(&created, "integrationId");
        let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}");
        let (status, patched) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "integrationUri": row.request.get("integrationUri") })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 from UpdateIntegration, got {status} {patched}",
            row.case
        );
        assert_eq!(
            as_str(&patched, "message"),
            INVALID_ARN,
            "case {:?}: wrong refusal sentence from UpdateIntegration, got {patched}",
            row.case
        );
        let (_, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            fetched, created,
            "case {:?}: a refused UpdateIntegration must leave the record as it was",
            row.case
        );
    }
}

#[tokio::test]
async fn an_http_api_integration_with_no_payload_format_version_is_refused_with_aws_s_sentence() {
    let emulator = emulator();

    // The rule is the HTTP api's, whatever the integration invokes, so the rows span the
    // integration type and what names its backend.
    let http_rows = [
        (
            "AWS_PROXY naming a Lambda function",
            json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
            }),
        ),
        (
            "HTTP_PROXY naming a URL",
            json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream"
            }),
        ),
        (
            "AWS_PROXY naming a service action through its subtype",
            json!({
                "integrationType": "AWS_PROXY",
                "integrationSubtype": "SQS-SendMessage"
            }),
        ),
    ];

    for (index, (case, request)) in http_rows.into_iter().enumerate() {
        let api_id = create_api(&emulator, case, &http_api(&format!("payload-text-{index}"))).await;
        let (status, headers, body) = call_reading_headers(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateIntegration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok()),
            Some("BadRequestException"),
            "case {case:?}: expected the error type on the response headers, got {headers:?}"
        );
        assert_eq!(
            as_str(&body, "message"),
            PAYLOAD_FORMAT_VERSION_REQUIRED,
            "case {case:?}: wrong refusal sentence, got {body}"
        );
        assert_eq!(
            integration_count(&emulator, &api_id).await,
            0,
            "case {case:?}: a refused CreateIntegration must store no integration"
        );

        // The same request carrying the member is created, so the refusal is about the
        // member and nothing else.
        let mut complete = request;
        complete["payloadFormatVersion"] = json!("1.0");
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(complete),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 once PayloadFormatVersion is sent, got {status} {created}"
        );
    }

    // "Required for HTTP APIs" (`CreateIntegration$PayloadFormatVersion`), so an api of
    // the other protocol needs none: its integration is created and reports the one
    // version every type supports.
    let socket = create_api(
        &emulator,
        "a websocket api's integration",
        &websocket_api("payload-text-socket"),
    )
    .await;
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{socket}/integrations"),
        Some(json!({ "integrationType": "MOCK" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "a WebSocket api's integration needs no PayloadFormatVersion, got {status} {created}"
    );
    assert_eq!(
        created.get("payloadFormatVersion"),
        Some(&json!("1.0")),
        "the integration must report the version every type supports, got {created}"
    );
}

#[tokio::test]
async fn an_integration_type_the_service_cannot_read_is_refused_as_a_missing_one() {
    let emulator = emulator();

    // The rule generalises over HOW the type is unreadable: left out entirely, or a value
    // outside the enum `IntegrationType` declares — another service's spelling of a proxy
    // integration, a published value in the wrong case, the empty string, a protocol that
    // is no integration type at all.
    let unreadable: [(&str, Option<&str>); 6] = [
        ("no IntegrationType at all", None),
        ("another spelling of the Lambda proxy", Some("LAMBDA_PROXY")),
        ("a published value in the wrong case", Some("aws_proxy")),
        ("the empty string", Some("")),
        ("a value of no integration type", Some("FTP")),
        ("a value of the right shape but no enum", Some("SQS_PROXY")),
    ];

    for (index, (case, integration_type)) in unreadable.into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            case,
            &http_api(&format!("integration-type-{index}")),
        )
        .await;
        let mut request = json!({
            "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:handler",
            "payloadFormatVersion": "2.0"
        });
        if let Some(value) = integration_type {
            request["integrationType"] = json!(value);
        }

        let (status, headers, body) = call_reading_headers(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateIntegration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok()),
            Some("BadRequestException"),
            "case {case:?}: expected the error type on the response headers, got {headers:?}"
        );
        assert_eq!(
            as_str(&body, "message"),
            INTEGRATION_TYPE_REQUIRED,
            "case {case:?}: an unreadable IntegrationType is refused as a missing one, \
             got {body}"
        );
        assert_eq!(
            integration_count(&emulator, &api_id).await,
            0,
            "case {case:?}: a refused CreateIntegration must store no integration"
        );
    }
}

#[tokio::test]
async fn the_route_quick_create_mints_is_api_gateway_managed_whatever_key_it_carries() {
    // (case, the RouteKey the request names, the key the route carries)
    let rows = [
        ("no RouteKey, so the catch-all", None, "$default"),
        ("the catch-all named explicitly", Some("$default"), "$default"),
        ("a method-and-path key", Some("GET /items"), "GET /items"),
        ("a greedy key", Some("ANY /{proxy+}"), "ANY /{proxy+}"),
    ];

    for (index, (case, route_key, expect_key)) in rows.into_iter().enumerate() {
        // Both target forms quick create builds an integration from, so the rule does not
        // depend on which backend it proxies to.
        for target in [
            "https://example.com/upstream",
            "arn:aws:lambda:eu-west-1:000000000000:function:handler",
        ] {
            let emulator = emulator();
            let mut request = http_api(&format!("managed-route-{index}"));
            request["target"] = json!(target);
            if let Some(key) = route_key {
                request["routeKey"] = json!(key);
            }
            let api_id = create_api(&emulator, case, &request).await;

            let listed = routes(&emulator, &api_id).await;
            assert_eq!(
                listed.len(),
                1,
                "case {case:?} ({target:?}): quick create mints exactly one route, got {listed:?}"
            );
            assert_eq!(
                as_str(&listed[0], "routeKey"),
                expect_key,
                "case {case:?} ({target:?}): wrong RouteKey on the quick-create route, got {listed:?}"
            );
            assert_eq!(
                listed[0].get("apiGatewayManaged"),
                Some(&json!(true)),
                "case {case:?} ({target:?}): the route quick create mints is \
                 API-Gateway-managed, got {listed:?}"
            );

            // The single-route reader reports the same flag.
            let route_id = as_str(&listed[0], "routeId");
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} ({target:?}): expected 200 from GetRoute, got {status} {fetched}"
            );
            assert_eq!(
                fetched.get("apiGatewayManaged"),
                Some(&json!(true)),
                "case {case:?} ({target:?}): GetRoute must report the flag GetRoutes \
                 reports, got {fetched}"
            );

            // A route the CALLER asked for on the same api is its own, so the flag marks
            // what quick create made rather than every route of a quick-created api.
            let integration_id = as_str(&listed[0], "target")
                .strip_prefix("integrations/")
                .unwrap_or_default()
                .to_string();
            let (status, created) = call(
                &emulator,
                "POST",
                &format!("/v2/apis/{api_id}/routes"),
                Some(json!({
                    "routeKey": "GET /mine",
                    "target": format!("integrations/{integration_id}")
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?} ({target:?}): expected 201 from CreateRoute, got {status} {created}"
            );
            assert_eq!(
                created.get("apiGatewayManaged"),
                Some(&json!(false)),
                "case {case:?} ({target:?}): a route the caller created is not \
                 API-Gateway-managed, got {created}"
            );
        }
    }
}
