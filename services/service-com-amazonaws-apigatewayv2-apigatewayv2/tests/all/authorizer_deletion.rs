// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteAuthorizer`.
//!
//! The rules these cases assert:
//!
//! * **An authorizer no route of its api names is removed**: `DeleteAuthorizer` answers
//!   204 with an empty body, `GetAuthorizer` then reports 404 `NotFoundException`, and a
//!   second delete of the same id is a 404 rather than a second removal. Every other
//!   authorizer of the api, and every route, survives.
//! * **An authorizer a route of the same api still names cannot be removed**: the delete
//!   is refused 409 `ConflictException` and persists nothing, so the authorizer is still
//!   readable afterwards. A route references an authorizer through its `AuthorizerId`
//!   (`Route$AuthorizerId` is "The identifier of the Authorizer resource to be associated
//!   with this route"), so the reference is what blocks the delete: re-point or clear the
//!   route's `AuthorizerId` and the same delete then succeeds.
//! * **An identifier that resolves to nothing is a 404**: an api that does not exist, an
//!   authorizer id nothing minted, and an authorizer addressed through another api — and
//!   an `AuthorizerId` resolves only through its own api, so a route of ANOTHER api
//!   naming an authorizer of that api blocks nothing here.
//!
//! The rows vary what the rules generalise over: the owning api's protocol type (`HTTP`,
//! `WEBSOCKET`), the authorizer type (`JWT`, `REQUEST`), how many routes reference the
//! authorizer, the `AuthorizationType` the referencing route carries (`JWT`, `CUSTOM`),
//! and the FORM of a route's reference (absent, another authorizer of the same api, the
//! empty string, which names no authorizer). The recorded case that filed the issue — one
//! route naming the authorizer being deleted — is one row of the refusal table.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The sentence API Gateway answers an `AuthorizerId` that resolves to nothing with. It
/// names neither the account nor the identifier the request carried — unlike the
/// api-level "Invalid API identifier specified {account}:{apiId}".
const INVALID_AUTHORIZER_IDENTIFIER: &str = "Invalid Authorizer identifier specified";

/// The `message` an error response carries.
fn error_message(body: &Value) -> String {
    as_str(body, "message")
}

/// The message API Gateway refuses the delete of an authorizer `name` with: it names the
/// authorizer and ONE of the routes whose `AuthorizerId` names it ("… is referenced in
/// route: <RouteKey>" is singular), so the route named must be one of `route_keys` — which
/// one depends on the store's own `RouteId` order, not on the order they were created in.
fn assert_in_use_message(case: &str, body: &Value, name: &str, route_keys: &[&str]) {
    let reported = error_message(body);
    let prefix = format!("Cannot delete authorizer '{name}', is referenced in route: ");
    let listed = reported.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected a message starting {prefix:?}, got {reported:?}")
    });
    assert!(
        route_keys.contains(&listed),
        "case {case:?}: expected one of {route_keys:?} to be named, got {reported:?}"
    );
}

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI, for example,
/// arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations".
fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:us-east-1:123456789012:function:{function}/invocations"
    )
}

/// A `JWT` authorizer request. "Specify JWT to use JSON Web Tokens (supported only for
/// HTTP APIs)" (`CreateAuthorizer$AuthorizerType`), so only an HTTP api takes one.
fn jwt_authorizer(name: &str) -> Value {
    json!({
        "name": name,
        "authorizerType": "JWT",
        "identitySource": ["$request.header.Authorization"],
        "jwtConfiguration": {
            "issuer": "https://example-issuer.test/oauth2",
            "audience": ["an-audience"]
        }
    })
}

/// A `REQUEST` authorizer request for an api of `protocol`.
/// `AuthorizerPayloadFormatVersion` is "Required for HTTP API Lambda authorizers", so a
/// WebSocket api's authorizer carries none.
fn request_authorizer(name: &str, protocol: &str) -> Value {
    let mut request = json!({
        "name": name,
        "authorizerType": "REQUEST",
        "identitySource": ["$request.header.Authorization"],
        "authorizerUri": lambda_uri(name)
    });
    if protocol != "WEBSOCKET" {
        request["authorizerPayloadFormatVersion"] = json!("2.0");
    }
    request
}

/// Create one api of `protocol` and answer its `ApiId`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
    protocol: &str,
) -> String {
    let mut request = json!({ "name": name, "protocolType": protocol });
    if protocol == "WEBSOCKET" {
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one authorizer under `api_id` and answer its `AuthorizerId`.
async fn create_authorizer(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAuthorizer, got {status} {created}"
    );
    as_str(&created, "authorizerId")
}

/// Create one route under `api_id` and answer its `RouteId`. `authorizer` is the route's
/// `AuthorizerId`, and `authorization_type` the `AuthorizationType` that goes with it.
async fn create_route(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    authorization_type: &str,
    authorizer: Option<String>,
) -> String {
    let mut request = json!({ "routeKey": route_key, "authorizationType": authorization_type });
    if let Some(authorizer) = authorizer {
        request["authorizerId"] = json!(authorizer);
    }
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
    );
    as_str(&created, "routeId")
}

/// `(status, body)` of `GetAuthorizer`.
async fn get_authorizer(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    authorizer_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
        None,
    )
    .await
}

/// `(status, body)` of `DeleteAuthorizer`.
async fn delete_authorizer(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    authorizer_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
        None,
    )
    .await
}

/// One row of the "the delete goes through" table: the api the authorizer lives under,
/// the authorizer to create, and a route whose `AuthorizerId` does NOT name it.
struct Removable {
    case: &'static str,
    protocol: &'static str,
    /// The authorizer the row deletes, built for the row's protocol.
    authorizer: fn(&str) -> Value,
    /// The route key of a route created beside the authorizer.
    route_key: &'static str,
    /// That route's `AuthorizationType`.
    authorization_type: &'static str,
    /// That route's `AuthorizerId`, built from the ids the row minted: the authorizer
    /// being deleted (never, by construction), another authorizer of the same api, or the
    /// empty string. `None` leaves the route with no `AuthorizerId` at all.
    authorizer_reference: fn(&Context) -> Option<String>,
}

/// The ids one row minted, for a row's reference to be built from.
struct Context {
    /// The authorizer the row deletes.
    authorizer_id: String,
    /// A second authorizer of the same api, which the row never deletes.
    other_authorizer_id: String,
}

fn removable_rows() -> Vec<Removable> {
    vec![
        Removable {
            case: "an HTTP api JWT authorizer no route names",
            protocol: "HTTP",
            authorizer: |_| jwt_authorizer("delete-unused-jwt"),
            route_key: "GET /orders",
            authorization_type: "NONE",
            authorizer_reference: |_| None,
        },
        Removable {
            case: "an authorizer whose api's route names ANOTHER authorizer",
            protocol: "HTTP",
            authorizer: |protocol| request_authorizer("delete-sibling-request", protocol),
            route_key: "POST /orders",
            authorization_type: "CUSTOM",
            authorizer_reference: |context| Some(context.other_authorizer_id.clone()),
        },
        Removable {
            case: "a WebSocket api REQUEST authorizer beside a route naming none",
            protocol: "WEBSOCKET",
            authorizer: |protocol| request_authorizer("delete-socket-request", protocol),
            route_key: "$connect",
            authorization_type: "NONE",
            // An empty `AuthorizerId` names no authorizer of this api.
            authorizer_reference: |_| Some(String::new()),
        },
    ]
}

#[tokio::test]
async fn delete_authorizer_removes_the_record_and_a_later_read_reports_not_found() {
    let emulator = emulator();

    for (index, row) in removable_rows().into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            row.case,
            &format!("delete-authorizer-api-{index}"),
            row.protocol,
        )
        .await;
        let authorizer_id =
            create_authorizer(&emulator, row.case, &api_id, (row.authorizer)(row.protocol)).await;
        let other_authorizer_id = create_authorizer(
            &emulator,
            row.case,
            &api_id,
            request_authorizer(&format!("spare-authorizer-{index}"), row.protocol),
        )
        .await;
        let context = Context {
            authorizer_id: authorizer_id.clone(),
            other_authorizer_id: other_authorizer_id.clone(),
        };
        assert_ne!(
            (row.authorizer_reference)(&context),
            Some(context.authorizer_id.clone()),
            "case {:?}: this row's route must not name the authorizer it deletes",
            row.case
        );
        let route_id = create_route(
            &emulator,
            row.case,
            &api_id,
            row.route_key,
            row.authorization_type,
            (row.authorizer_reference)(&context),
        )
        .await;

        let (status, body) = delete_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteAuthorizer, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty DeleteAuthorizer body, got {body}",
            row.case
        );

        // Read back through the implemented reader: the record is gone.
        let (status, body) = get_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetAuthorizer after the delete, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );
        assert_eq!(
            error_message(&body),
            INVALID_AUTHORIZER_IDENTIFIER,
            "case {:?}: expected the authorizer-not-found sentence, got {body}",
            row.case
        );

        // A second delete removes nothing: the id now resolves to nothing.
        let (status, body) = delete_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from the second DeleteAuthorizer, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );
        assert_eq!(
            error_message(&body),
            INVALID_AUTHORIZER_IDENTIFIER,
            "case {:?}: expected the authorizer-not-found sentence, got {body}",
            row.case
        );

        // The api's other authorizer survives the delete of its sibling.
        let (status, fetched) = get_authorizer(&emulator, &api_id, &other_authorizer_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected {other_authorizer_id} to survive, got {status} {fetched}",
            row.case
        );

        // And the routes of the api are untouched by an authorizer delete.
        let (status, route) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the route to survive the delete, got {status} {route}",
            row.case
        );
    }
}

/// One row of the refusal table: how many routes of the api name the authorizer, under
/// which `AuthorizationType`, and which route keys they carry.
struct InUse {
    case: &'static str,
    protocol: &'static str,
    authorizer: fn(&str) -> Value,
    /// The `AuthorizationType` every referencing route of this row carries.
    authorization_type: &'static str,
    /// The route keys of the routes that name the authorizer. Non-empty by construction.
    route_keys: &'static [&'static str],
}

fn in_use_rows() -> Vec<InUse> {
    vec![
        InUse {
            case: "one JWT route names the authorizer",
            protocol: "HTTP",
            authorizer: |_| jwt_authorizer("in-use-single-jwt"),
            authorization_type: "JWT",
            route_keys: &["GET /recorded"],
        },
        InUse {
            case: "three routes of the api name the one authorizer",
            protocol: "HTTP",
            authorizer: |protocol| request_authorizer("in-use-fanin-request", protocol),
            authorization_type: "CUSTOM",
            route_keys: &["GET /items", "POST /items", "DELETE /items/{id}"],
        },
        InUse {
            case: "a WebSocket api's $connect route names the authorizer",
            protocol: "WEBSOCKET",
            authorizer: |protocol| request_authorizer("in-use-socket-request", protocol),
            authorization_type: "CUSTOM",
            route_keys: &["$connect"],
        },
    ]
}

#[tokio::test]
async fn delete_authorizer_is_refused_while_a_route_of_its_api_names_it() {
    let emulator = emulator();

    for (index, row) in in_use_rows().into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            row.case,
            &format!("in-use-authorizer-api-{index}"),
            row.protocol,
        )
        .await;
        let authorizer_request = (row.authorizer)(row.protocol);
        let authorizer_name = as_str(&authorizer_request, "name");
        let spare_name = format!("in-use-spare-{index}");
        let authorizer_id =
            create_authorizer(&emulator, row.case, &api_id, authorizer_request).await;
        // A second authorizer for the routes to be re-pointed at below.
        let spare_authorizer_id = create_authorizer(
            &emulator,
            row.case,
            &api_id,
            request_authorizer(&spare_name, row.protocol),
        )
        .await;

        let mut route_ids = Vec::new();
        for route_key in row.route_keys {
            route_ids.push(
                create_route(
                    &emulator,
                    row.case,
                    &api_id,
                    route_key,
                    row.authorization_type,
                    Some(authorizer_id.clone()),
                )
                .await,
            );
        }

        let (status, body) = delete_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 from DeleteAuthorizer while {} route(s) name it, got {status} {body}",
            row.case,
            route_ids.len()
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {:?}: expected ConflictException, got {body}",
            row.case
        );
        // The refusal names the authorizer and every route key that references it.
        assert_in_use_message(row.case, &body, &authorizer_name, row.route_keys);

        // The refusal persisted nothing: the authorizer is still there to read.
        let (status, fetched) = get_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the refused authorizer to survive, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "authorizerId"),
            authorizer_id,
            "case {:?}: expected the same authorizer back, got {fetched}",
            row.case
        );

        // What blocks the delete is the REFERENCE, not the authorizer: re-point every
        // route at the spare authorizer and the very same delete goes through.
        for route_id in &route_ids {
            let (status, updated) = call(
                &emulator,
                "PATCH",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                Some(json!({ "authorizerId": spare_authorizer_id })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from UpdateRoute, got {status} {updated}",
                row.case
            );
        }

        let (status, body) = delete_authorizer(&emulator, &api_id, &authorizer_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 once no route names the authorizer, got {status} {body}",
            row.case
        );

        // And the spare, which every route now names, is refused in its turn — until the
        // routes give up their reference altogether: an empty `AuthorizerId` names no
        // authorizer, so the open route releases it.
        let (status, body) = delete_authorizer(&emulator, &api_id, &spare_authorizer_id).await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 for the authorizer the routes now name, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {:?}: expected ConflictException, got {body}",
            row.case
        );
        assert_in_use_message(row.case, &body, &spare_name, row.route_keys);
        for route_id in &route_ids {
            let (status, updated) = call(
                &emulator,
                "PATCH",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                Some(json!({ "authorizationType": "NONE", "authorizerId": "" })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from UpdateRoute clearing the reference, got {status} {updated}",
                row.case
            );
        }
        let (status, body) = delete_authorizer(&emulator, &api_id, &spare_authorizer_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 once every route has cleared its AuthorizerId, got {status} {body}",
            row.case
        );
    }
}

/// An `AuthorizerId` resolves only through the api that owns it, so a route of ANOTHER
/// api naming an authorizer of its own blocks nothing here: only the routes of the
/// authorizer's OWN api can block its delete.
#[tokio::test]
async fn a_route_of_another_api_does_not_block_the_delete() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", "scoped-owner-api", "HTTP").await;
    let authorizer_id = create_authorizer(
        &emulator,
        "the owning api's authorizer",
        &owner,
        jwt_authorizer("scoped-owner-authorizer"),
    )
    .await;

    // A DIFFERENT api, holding an authorizer of its own — a minted `AuthorizerId` denotes
    // ONE authorizer, so the bystander's carries an id of its own — named by a route of
    // that api. A route may only name an authorizer of its own api, so this is the
    // blocking reference that exists anywhere but in the owner's api.
    let bystander = create_api(&emulator, "the bystander api", "scoped-other-api", "HTTP").await;
    let bystander_authorizer = create_authorizer(
        &emulator,
        "the bystander api's authorizer",
        &bystander,
        jwt_authorizer("scoped-bystander-authorizer"),
    )
    .await;
    assert_ne!(
        bystander_authorizer, authorizer_id,
        "a minted AuthorizerId denotes one authorizer, so the bystander api's must differ \
         from the owner's; got {bystander_authorizer} and {authorizer_id}"
    );
    let bystander_route = create_route(
        &emulator,
        "the bystander api's route",
        &bystander,
        "GET /scoped",
        "JWT",
        Some(bystander_authorizer.clone()),
    )
    .await;

    let (status, body) = delete_authorizer(&emulator, &owner, &authorizer_id).await;
    assert_eq!(
        status, 204,
        "expected 204: no route of the OWNING api names the authorizer, got {status} {body}"
    );

    let (status, route) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{bystander}/routes/{bystander_route}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the other api's route to be untouched, got {status} {route}"
    );
    assert_eq!(
        route.get("authorizerId"),
        Some(&json!(bystander_authorizer)),
        "expected the other api's route to keep its AuthorizerId, got {route}"
    );

    // And the bystander's own authorizer, which its route names, is still refused.
    let (status, body) = delete_authorizer(&emulator, &bystander, &bystander_authorizer).await;
    assert_eq!(
        status, 409,
        "expected 409 for the authorizer the bystander's route names, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body}"
    );
    assert_in_use_message(
        "the bystander api's own authorizer",
        &body,
        "scoped-bystander-authorizer",
        &["GET /scoped"],
    );
}

#[tokio::test]
async fn delete_authorizer_answers_not_found_for_an_identifier_that_resolves_to_nothing() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", "delete-owner-api", "HTTP").await;
    let other = create_api(&emulator, "another api", "delete-other-api", "HTTP").await;
    let authorizer_id = create_authorizer(
        &emulator,
        "the owning api's authorizer",
        &owner,
        request_authorizer("not-found-request", "HTTP"),
    )
    .await;

    for (case, api_id, target_id, expected_message) in [
        (
            "an api that does not exist",
            "nosuchapi00",
            &authorizer_id,
            // An api identifier that resolves to nothing is reported the way every other
            // reader of an api reports it, naming the account and the id.
            format!(
                "Invalid API identifier specified {}:nosuchapi00",
                engine::emulator_account_id()
            ),
        ),
        (
            "an authorizer id nothing minted",
            owner.as_str(),
            &"nosuchauth".to_string(),
            INVALID_AUTHORIZER_IDENTIFIER.to_string(),
        ),
        (
            "an authorizer addressed through another api",
            other.as_str(),
            &authorizer_id,
            INVALID_AUTHORIZER_IDENTIFIER.to_string(),
        ),
    ] {
        let (status, body) = delete_authorizer(&emulator, api_id, target_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteAuthorizer, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected the identifier's own refusal sentence, got {body}"
        );
    }

    // None of those refusals removed the authorizer they named.
    let (status, body) = get_authorizer(&emulator, &owner, &authorizer_id).await;
    assert_eq!(
        status, 200,
        "expected the authorizer to survive every refused delete, got {status} {body}"
    );
}
