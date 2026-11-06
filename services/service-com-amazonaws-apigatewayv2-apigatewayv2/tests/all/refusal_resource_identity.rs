// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH RESOURCE an ApiGatewayV2 refusal names.
//!
//! A client reads a refusal to learn which resource of its request the service could not
//! resolve, or which resources block an operation, so the rules below are about the
//! IDENTITY the message conveys rather than its wording alone:
//!
//! * **A `NotFoundException` names the KIND of resource the call addressed, with that
//!   kind's own sentence**: the kinds addressed WITHIN an api whose sentence is a
//!   constant for their class (a route, a route response, an integration, a stage, a
//!   deployment, an authorizer, a VPC link) get one sentence whatever identifier the
//!   request carried; the kinds whose sentence is BUILT from the request (a model, an
//!   integration response) name the identifiers they were addressed with; and the
//!   ACCOUNT-level kinds (the api itself, a portal product) are reported
//!   account-qualified.
//! * **A minted identifier denotes ONE kind of resource**: no two kinds mint the same
//!   id, so an id minted for one kind addresses nothing of another and reads back as that
//!   other kind's `NotFoundException`.
//! * **A conflict refusal is built from STATE**: `DeleteIntegration` refused by the
//!   routes wired to the integration names the `RouteId` of every one of them, which is
//!   the only thing that tells a caller which routes to retarget.
//! * **A collection reader addresses its parent the same way**: `GetRouteResponses`
//!   reports the route responses its route holds, a page at a time, and names the route
//!   (or the api) that resolved to nothing.
//!
//! The rows span what the rules generalise over: the resource kind, the operation that
//! addresses it (read, write, delete, and a create whose PARENT is missing), the form of
//! the identifier that resolves to nothing (length, alphabet, case, an id minted for
//! another kind, an id of another api) and, for the conflict, the NUMBER of blocking
//! routes. The recorded calls that filed the issue — one `GetDeployment` after its api
//! was deleted, one `CreateRouteResponse` on an unknown route, one `DeleteIntegration`
//! blocked by a single route — are one row each.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    integration_response_not_found_message, invalid_api_identifier_message,
    invalid_identifier_message, invalid_model_identifier_message,
    portal_product_not_found_message, AUTHORIZER_RESOURCE, DEPLOYMENT_RESOURCE,
    INTEGRATION_RESOURCE, INVALID_NEXT_TOKEN_MESSAGE, MAX_PAGE_SIZE, ROUTE_RESOURCE,
    ROUTE_RESPONSE_NOT_FOUND_MESSAGE, STAGE_RESOURCE, VPC_LINK_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// The stage name the fixture's api carries. "Stage names can only contain alphanumeric
/// characters, hyphens, and underscores" (`CreateStage$StageName`).
const STAGE_NAME: &str = "prod";

/// The sentence a paginated reader refuses a `MaxResults` that is no positive integer
/// with. Built from the request, so it quotes the value the caller sent back at it.
fn max_results_refusal(raw: &str) -> String {
    format!("Invalid value for MaxResults: '{raw}'. MaxResults must be a positive integer.")
}

/// Identifiers nothing minted, spanning the forms a client can send: an eleven-character
/// lowercase-alphanumeric one, a single character, the longest id any member of this model
/// admits (`__stringMin10Max30PatternAZ09`), and a mixed-case one.
fn unminted_identifiers() -> Vec<String> {
    vec![
        "nosuchid000".to_string(),
        "z".to_string(),
        "9".repeat(30),
        "MixedCase1".to_string(),
    ]
}

/// Send one request that must be accepted, and answer the created resource.
async fn created(
    emulator: &MultiServiceRouter,
    case: &str,
    uri: &str,
    request: Value,
) -> Value {
    let (status, body) = call(emulator, "POST", uri, Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from POST {uri}, got {status} {body}"
    );
    body
}

/// Create one WebSocket api — the protocol that carries route responses and models — and
/// answer its `ApiId`.
async fn websocket_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let api = created(
        emulator,
        name,
        "/v2/apis",
        json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            // "This property is required for WebSocket APIs"
            // (`CreateApi$RouteSelectionExpression`).
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    as_str(&api, "apiId")
}

/// One route of `api_id` wired to `target`, answering its `RouteId`.
async fn create_route(
    emulator: &MultiServiceRouter,
    api_id: &str,
    route_key: &str,
    target: Option<&str>,
) -> String {
    let mut request = json!({ "routeKey": route_key });
    if let Some(target) = target {
        request["target"] = json!(target);
    }
    let route = created(
        emulator,
        route_key,
        &format!("/v2/apis/{api_id}/routes"),
        request,
    )
    .await;
    as_str(&route, "routeId")
}

/// Every resource kind this suite mints an id for, in one api: the kind's name (for the
/// case label) and the id minted for it.
struct Fixture {
    api_id: String,
    /// `kind -> minted id`, for the kinds whose id this service mints.
    minted: BTreeMap<&'static str, String>,
    /// The `RouteId` the fixture's route response hangs off.
    route_id: String,
    /// The `IntegrationId` the fixture's integration response hangs off.
    integration_id: String,
}

/// One api of every child resource this service mints an id for, plus the two
/// account-level kinds (a VPC link and a portal product).
async fn fixture(emulator: &MultiServiceRouter, name: &str) -> Fixture {
    let api_id = websocket_api(emulator, name).await;

    let integration = created(
        emulator,
        "the integration",
        &format!("/v2/apis/{api_id}/integrations"),
        // "MOCK: for integrating the route or method request with API Gateway as a
        // 'loopback' endpoint without invoking any backend"
        // (`CreateIntegration$IntegrationType`), so it names no endpoint.
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let integration_id = as_str(&integration, "integrationId");

    let integration_response = created(
        emulator,
        "the integration response",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
        json!({ "integrationResponseKey": "$default" }),
    )
    .await;

    let route_id = create_route(
        emulator,
        &api_id,
        "$default",
        Some(&format!("integrations/{integration_id}")),
    )
    .await;

    let route_response = created(
        emulator,
        "the route response",
        &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
        json!({ "routeResponseKey": "$default" }),
    )
    .await;

    let model = created(
        emulator,
        "the model",
        &format!("/v2/apis/{api_id}/models"),
        json!({
            "name": "Order",
            "contentType": "application/json",
            "schema": "{\"type\":\"object\"}"
        }),
    )
    .await;

    created(
        emulator,
        "the stage",
        &format!("/v2/apis/{api_id}/stages"),
        json!({ "stageName": STAGE_NAME }),
    )
    .await;

    let deployment = created(
        emulator,
        "the deployment",
        &format!("/v2/apis/{api_id}/deployments"),
        json!({ "stageName": STAGE_NAME }),
    )
    .await;

    let authorizer = created(
        emulator,
        "the authorizer",
        &format!("/v2/apis/{api_id}/authorizers"),
        json!({
            "name": "the-authorizer",
            "authorizerType": "REQUEST",
            "identitySource": ["$request.header.Authorization"],
            "authorizerUri": "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                              arn:aws:lambda:us-east-1:123456789012:function:auth/invocations"
        }),
    )
    .await;

    let vpc_link = created(
        emulator,
        "the vpc link",
        "/v2/vpclinks",
        json!({ "name": "the-link", "subnetIds": ["subnet-0a1b2c3d"] }),
    )
    .await;

    let portal_product = created(
        emulator,
        "the portal product",
        "/v2/portalproducts",
        json!({ "displayName": "Payments" }),
    )
    .await;

    let minted = BTreeMap::from([
        ("Api", api_id.clone()),
        ("Integration", integration_id.clone()),
        (
            "IntegrationResponse",
            as_str(&integration_response, "integrationResponseId"),
        ),
        ("Route", route_id.clone()),
        ("RouteResponse", as_str(&route_response, "routeResponseId")),
        ("Model", as_str(&model, "modelId")),
        ("Deployment", as_str(&deployment, "deploymentId")),
        ("Authorizer", as_str(&authorizer, "authorizerId")),
        ("VpcLink", as_str(&vpc_link, "vpcLinkId")),
        ("PortalProduct", as_str(&portal_product, "portalProductId")),
    ]);
    for (kind, id) in &minted {
        assert!(
            !id.is_empty(),
            "case {kind:?}: expected the create to report a minted id, got {minted:?}"
        );
    }

    Fixture {
        api_id,
        minted,
        route_id,
        integration_id,
    }
}

/// What a refusal must answer: the HTTP status, the error code and the message.
struct Refusal<'a> {
    status: u16,
    code: &'a str,
    message: &'a str,
}

/// The modelled 404 `NotFoundException` ("The resource specified in the request was not
/// found"), carrying `message`.
fn not_found(message: &str) -> Refusal<'_> {
    Refusal {
        status: 404,
        code: "NotFoundException",
        message,
    }
}

/// Assert one refusal end to end: the status, the error code and the message.
async fn assert_refusal(
    emulator: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected: Refusal<'_>,
) {
    let (status, answered) = call(emulator, method, uri, body).await;
    assert_eq!(
        status, expected.status,
        "case {case:?}: expected {} from {method} {uri}, got {status} {answered}",
        expected.status
    );
    assert_eq!(
        error_code(&answered),
        expected.code,
        "case {case:?}: expected {} from {method} {uri}, got {answered}",
        expected.code
    );
    assert_eq!(
        as_str(&answered, "message"),
        expected.message,
        "case {case:?}: expected the refusal {:?} from {method} {uri}, got {answered}",
        expected.message
    );
}

/// How a single resource of one kind is addressed, and the sentence a request that
/// resolves to nothing there must be refused with.
struct Reader {
    kind: &'static str,
    /// The URI that addresses one resource of this kind by `id`.
    uri: Box<dyn Fn(&str) -> String>,
    /// The sentence this kind's unknown identifier is refused with, for `id`.
    expected: Box<dyn Fn(&str) -> String>,
    /// What this kind's sentence is built from.
    sentence: Sentence,
}

/// What a kind's `NotFoundException` sentence is built from.
#[derive(Debug, PartialEq, Eq)]
enum Sentence {
    /// A CONSTANT for the whole class: it carries neither the identifier the request
    /// addressed nor the account, so nothing in it varies with the request.
    Constant,
    /// Built from the identifier(s) the request addressed, which is what a caller can
    /// act on.
    FromRequest,
    /// Built from the identifier the request addressed AND the account the lookup was
    /// made in — how API Gateway reports an identifier of an ACCOUNT-level resource.
    AccountQualified,
}

/// Every kind's reader, over the fixture's api, route and integration.
fn readers(fixture: &Fixture) -> Vec<Reader> {
    let api_id = fixture.api_id.clone();
    let route_id = fixture.route_id.clone();
    let integration_id = fixture.integration_id.clone();
    vec![
        Reader {
            kind: "Api",
            uri: Box::new(|id| format!("/v2/apis/{id}")),
            // API Gateway reports an unknown API identifier as `{account}:{ApiId}`.
            expected: Box::new(invalid_api_identifier_message),
            sentence: Sentence::AccountQualified,
        },
        Reader {
            kind: "Route",
            uri: Box::new({
                let api_id = api_id.clone();
                move |id| format!("/v2/apis/{api_id}/routes/{id}")
            }),
            expected: Box::new(|_| invalid_identifier_message(ROUTE_RESOURCE)),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "RouteResponse",
            uri: Box::new({
                let api_id = api_id.clone();
                move |id| format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/{id}")
            }),
            expected: Box::new(|_| ROUTE_RESPONSE_NOT_FOUND_MESSAGE.to_string()),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "Integration",
            uri: Box::new({
                let api_id = api_id.clone();
                move |id| format!("/v2/apis/{api_id}/integrations/{id}")
            }),
            expected: Box::new(|_| invalid_identifier_message(INTEGRATION_RESOURCE)),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "IntegrationResponse",
            uri: Box::new({
                let api_id = api_id.clone();
                let integration_id = integration_id.clone();
                move |id| {
                    format!(
                        "/v2/apis/{api_id}/integrations/{integration_id}/\
                         integrationresponses/{id}"
                    )
                }
            }),
            // An integration response is reported by BOTH ids it was addressed with.
            expected: Box::new(move |id| {
                integration_response_not_found_message(id, &integration_id)
            }),
            sentence: Sentence::FromRequest,
        },
        Reader {
            kind: "Model",
            uri: Box::new({
                let api_id = api_id.clone();
                move |id| format!("/v2/apis/{api_id}/models/{id}")
            }),
            expected: Box::new(invalid_model_identifier_message),
            sentence: Sentence::FromRequest,
        },
        Reader {
            kind: "Stage",
            uri: Box::new({
                let api_id = api_id.clone();
                move |id| format!("/v2/apis/{api_id}/stages/{id}")
            }),
            expected: Box::new(|_| invalid_identifier_message(STAGE_RESOURCE)),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "Deployment",
            uri: Box::new({
                let api_id = api_id.clone();
                move |id| format!("/v2/apis/{api_id}/deployments/{id}")
            }),
            expected: Box::new(|_| invalid_identifier_message(DEPLOYMENT_RESOURCE)),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "Authorizer",
            uri: Box::new(move |id| format!("/v2/apis/{api_id}/authorizers/{id}")),
            expected: Box::new(|_| invalid_identifier_message(AUTHORIZER_RESOURCE)),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "VpcLink",
            uri: Box::new(|id| format!("/v2/vpclinks/{id}")),
            expected: Box::new(|_| invalid_identifier_message(VPC_LINK_RESOURCE)),
            sentence: Sentence::Constant,
        },
        Reader {
            kind: "PortalProduct",
            // A portal product is an ACCOUNT-level resource, so its sentence names the
            // account the lookup was made in beside the id, as the api's does.
            uri: Box::new(|id| format!("/v2/portalproducts/{id}")),
            expected: Box::new(portal_product_not_found_message),
            sentence: Sentence::AccountQualified,
        },
    ]
}

#[tokio::test]
async fn a_not_found_names_the_kind_it_addressed_with_that_kinds_own_sentence() {
    let emulator = emulator();
    let fixture = fixture(&emulator, "refusal-identity").await;
    let account = engine::emulator_account_id();

    for reader in readers(&fixture) {
        // What a kind's sentence must NOT vary with: where the sentence is a constant for
        // its class, one sentence for every identifier of that kind.
        let mut constant: Option<String> = None;
        for id in unminted_identifiers() {
            let uri = (reader.uri)(&id);
            let expected = (reader.expected)(&id);
            let case = format!("{} addressed as {id:?}", reader.kind);
            assert_refusal(
                &emulator,
                &case,
                "GET",
                &uri,
                None,
                not_found(&expected),
            )
            .await;

            match reader.sentence {
                // An account-level resource is reported account-qualified: the account
                // the lookup was made in, and the identifier the request addressed.
                Sentence::AccountQualified => assert!(
                    expected.contains(account) && expected.contains(&id),
                    "case {case:?}: expected the sentence to report the account \
                     {account:?} and the identifier, got {expected:?}"
                ),
                // A sentence built from the request names what the caller can act on:
                // the identifier it addressed, and no account.
                Sentence::FromRequest => assert!(
                    expected.contains(&id) && !expected.contains(account),
                    "case {case:?}: expected the sentence to name the identifier the \
                     request addressed and no account, got {expected:?}"
                ),
                Sentence::Constant => {
                    assert!(
                        !expected.contains(account),
                        "case {case:?}: expected a sentence carrying no account, \
                         got {expected:?}"
                    );
                    match &constant {
                        None => constant = Some(expected),
                        Some(first) => assert_eq!(
                            &expected, first,
                            "case {case:?}: expected every identifier of this kind to be \
                             refused with one sentence, got {expected:?} beside {first:?}"
                        ),
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_create_whose_parent_is_missing_names_the_parent_that_resolved_to_nothing() {
    let emulator = emulator();
    let fixture = fixture(&emulator, "missing-parent").await;
    let api_id = &fixture.api_id;

    for id in unminted_identifiers() {
        // CreateRouteResponse on a `RouteId` the api does not hold: the route is what
        // resolved to nothing, so the route is what the refusal names. This is the
        // recorded call that filed the issue.
        assert_refusal(
            &emulator,
            &format!("CreateRouteResponse under route {id:?}"),
            "POST",
            &format!("/v2/apis/{api_id}/routes/{id}/routeresponses"),
            Some(json!({ "routeResponseKey": "$default" })),
            not_found(&invalid_identifier_message(ROUTE_RESOURCE)),
        )
        .await;

        // The same rule one level across: the integration under a create of an
        // integration response, and the api under a create of a route.
        assert_refusal(
            &emulator,
            &format!("CreateIntegrationResponse under integration {id:?}"),
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{id}/integrationresponses"),
            Some(json!({ "integrationResponseKey": "$default" })),
            not_found(&invalid_identifier_message(INTEGRATION_RESOURCE)),
        )
        .await;

        assert_refusal(
            &emulator,
            &format!("CreateRoute under api {id:?}"),
            "POST",
            &format!("/v2/apis/{id}/routes"),
            Some(json!({ "routeKey": "GET /items" })),
            not_found(&invalid_api_identifier_message(&id)),
        )
        .await;

        // A member of the BODY that names a resource follows the same rule: the stage a
        // deployment is released to, and the deployment a stage is to serve.
        assert_refusal(
            &emulator,
            &format!("CreateDeployment to stage {id:?}"),
            "POST",
            &format!("/v2/apis/{api_id}/deployments"),
            Some(json!({ "stageName": id })),
            not_found(&invalid_identifier_message(STAGE_RESOURCE)),
        )
        .await;

        assert_refusal(
            &emulator,
            &format!("CreateStage serving deployment {id:?}"),
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": format!("s{}", id.to_lowercase()), "deploymentId": id })),
            not_found(&invalid_identifier_message(DEPLOYMENT_RESOURCE)),
        )
        .await;
    }
}

#[tokio::test]
async fn a_deployment_read_names_the_deployment_even_when_its_api_is_gone() {
    let emulator = emulator();
    let fixture = fixture(&emulator, "deployment-identity").await;
    let api_id = &fixture.api_id;
    let deployment_id = fixture.minted["Deployment"].clone();

    // While the api is there, the deployment reads back: the fixture is addressable.
    let (status, read) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetDeployment before the api is deleted, got {status} {read}"
    );

    let (status, deleted) = call(&emulator, "DELETE", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 204,
        "expected 204 from DeleteApi, got {status} {deleted}"
    );

    // A deployment is addressed WITHIN its api, so the pair is one lookup: a deployment
    // whose api is gone is a missing DEPLOYMENT, on the read and on the delete. This is
    // the recorded call that filed the issue.
    for (case, method) in [
        ("GetDeployment after DeleteApi", "GET"),
        ("DeleteDeployment after DeleteApi", "DELETE"),
    ] {
        assert_refusal(
            &emulator,
            case,
            method,
            &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
            None,
            not_found(&invalid_identifier_message(DEPLOYMENT_RESOURCE)),
        )
        .await;
    }

    // And the api's own readers still name the api, so the two refusals stay distinct.
    assert_refusal(
        &emulator,
        "GetApi after DeleteApi",
        "GET",
        &format!("/v2/apis/{api_id}"),
        None,
        not_found(&invalid_api_identifier_message(api_id)),
    )
    .await;
}

/// `GetRouteResponses` reports the route responses its route holds — the reader the
/// refused-create's positive control reads back through — and answers the route's own
/// unknown-identifier sentence for a `RouteId` the api does not hold.
#[tokio::test]
async fn the_route_response_list_reports_the_routes_responses_and_names_what_it_addressed() {
    let emulator = emulator();
    let api_id = websocket_api(&emulator, "route-response-list").await;
    let route_id = create_route(&emulator, &api_id, "$default", None).await;
    let collection = format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses");

    // The rows span the number of responses one route holds, and the key's form.
    let mut minted: Vec<String> = Vec::new();
    for key in ["$default", "first", "second-response", "$disconnect"] {
        let response = created(
            &emulator,
            key,
            &collection,
            json!({ "routeResponseKey": key }),
        )
        .await;
        minted.push(as_str(&response, "routeResponseId"));
        minted.sort();

        let (status, listed) = call(&emulator, "GET", &collection, None).await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected 200 from GetRouteResponses, got {status} {listed}"
        );
        let mut reported: Vec<String> = listed["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| as_str(item, "routeResponseId"))
            .collect();
        reported.sort();
        assert_eq!(
            reported, minted,
            "case {key:?}: expected the list to report every route response the route \
             holds, got {listed}"
        );
    }

    // One page at a time, in the store's own `RouteResponseId` order, resumed by the
    // token the previous page answered with.
    let (status, first) = call(&emulator, "GET", &format!("{collection}?maxResults=2"), None).await;
    assert_eq!(
        status, 200,
        "expected 200 from the first page, got {status} {first}"
    );
    assert_eq!(
        first["items"].as_array().map(Vec::len),
        Some(2),
        "expected a page of two, got {first}"
    );
    let token = as_str(&first, "nextToken");
    assert!(!token.is_empty(), "expected a nextToken, got {first}");
    let (status, second) = call(
        &emulator,
        "GET",
        &format!("{collection}?maxResults=2&nextToken={token}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from the resumed page, got {status} {second}"
    );
    assert_eq!(
        second["items"].as_array().map(Vec::len),
        Some(2),
        "expected the rest of the collection, got {second}"
    );

    // A page size the caller asks for is clamped server-side: a huge `MaxResults`
    // reports no more than `MAX_PAGE_SIZE` records, so one small request cannot make the
    // emulator project an unbounded page.
    let (status, asked_for_many) = call(
        &emulator,
        "GET",
        &format!("{collection}?maxResults=100000000"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 for a huge maxResults, got {status} {asked_for_many}"
    );
    let reported = asked_for_many["items"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert!(
        reported <= MAX_PAGE_SIZE,
        "expected at most the server-side page size {MAX_PAGE_SIZE}, got {reported}"
    );

    // A pagination input a caller chooses can only page the collection or be refused:
    // the rows span the forms neither input takes, and every one is a 400 that leaves
    // the collection readable afterwards.
    let mut hostile: Vec<(String, String)> = vec![
        ("maxResults=0".to_string(), max_results_refusal("0")),
        ("maxResults=-1".to_string(), max_results_refusal("-1")),
        ("maxResults=1.5".to_string(), max_results_refusal("1.5")),
        ("maxResults=abc".to_string(), max_results_refusal("abc")),
        (
            format!("maxResults={}", "9".repeat(40)),
            max_results_refusal(&"9".repeat(40)),
        ),
        (
            "nextToken=not-a-token".to_string(),
            INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        ),
        (
            "nextToken=ff".to_string(),
            INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        ),
        (
            format!("nextToken={}", "61".repeat(4096)),
            INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        ),
    ];
    // A token another collection issued is one this reader never minted either.
    let (_, other) = call(&emulator, "GET", "/v2/apis?maxResults=1", None).await;
    let other_token = as_str(&other, "nextToken");
    if !other_token.is_empty() {
        hostile.push((
            format!("nextToken={other_token}"),
            INVALID_NEXT_TOKEN_MESSAGE.to_string(),
        ));
    }
    for (query, expected) in hostile {
        let (status, refused) = call(&emulator, "GET", &format!("{collection}?{query}"), None).await;
        assert_eq!(
            status, 400,
            "case {query:?}: expected 400 from GetRouteResponses, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "BadRequestException",
            "case {query:?}: expected BadRequestException, got {refused}"
        );
        assert_eq!(
            as_str(&refused, "message"),
            expected,
            "case {query:?}: expected the refusal {expected:?}, got {refused}"
        );
    }
    // The refused reads changed nothing: the collection still reports its responses.
    let (status, still_there) = call(&emulator, "GET", &collection, None).await;
    assert_eq!(
        status, 200,
        "expected the collection to be readable after the refusals, got {status} \
         {still_there}"
    );

    // And the same addressing every route reader applies: the route this call addressed
    // is what an unknown `RouteId` is refused for, the api for an unknown `ApiId`.
    for id in unminted_identifiers() {
        assert_refusal(
            &emulator,
            &format!("GetRouteResponses of route {id:?}"),
            "GET",
            &format!("/v2/apis/{api_id}/routes/{id}/routeresponses"),
            None,
            not_found(&invalid_identifier_message(ROUTE_RESOURCE)),
        )
        .await;
        assert_refusal(
            &emulator,
            &format!("GetRouteResponses of api {id:?}"),
            "GET",
            &format!("/v2/apis/{id}/routes/{route_id}/routeresponses"),
            None,
            not_found(&invalid_api_identifier_message(&id)),
        )
        .await;
    }
}

#[tokio::test]
async fn an_identifier_minted_for_one_kind_addresses_nothing_of_another_kind() {
    let emulator = emulator();
    let fixture = fixture(&emulator, "kind-distinct").await;

    // No two kinds mint the same id, so one string never denotes two kinds of resource.
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (kind, id) in &fixture.minted {
        // Folding the kind into the id changes WHICH id each kind gets, not its shape:
        // every minted id is still lowercase alphanumerics, as many of them as its kind's
        // convention gives it (`minted_id_len`) — and the account-level ids still satisfy
        // the `^[a-z0-9]+$`, len 10..30 the portal-product and portal members are
        // modelled with.
        assert_minted_id(kind, kind, id);
        if let Some(other) = seen.insert(id.as_str(), kind.as_ref()) {
            panic!(
                "case {kind:?}: the id {id:?} is also the {other:?} id — a minted \
                 identifier must denote one kind of resource; minted {:?}",
                fixture.minted
            );
        }
    }

    // And the consequence a client sees: an id minted for one kind resolves to nothing
    // when it addresses another, with that other kind's own sentence.
    for reader in readers(&fixture) {
        for (kind, id) in &fixture.minted {
            if *kind == reader.kind {
                continue;
            }
            let case = format!("the {kind} id {id:?} addressed as a {}", reader.kind);
            assert_refusal(
                &emulator,
                &case,
                "GET",
                &(reader.uri)(id),
                None,
                not_found(&(reader.expected)(id)),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn deleting_an_integration_routes_target_names_every_blocking_route() {
    let emulator = emulator();

    // The rows span the NUMBER of routes that block the delete, which is what the
    // message's list generalises over.
    for blocking in [1_usize, 2, 3] {
        let case = format!("{blocking} blocking route(s)");
        let api_id = websocket_api(&emulator, &format!("in-use-{blocking}")).await;
        let integration = created(
            &emulator,
            &case,
            &format!("/v2/apis/{api_id}/integrations"),
            json!({ "integrationType": "MOCK" }),
        )
        .await;
        let integration_id = as_str(&integration, "integrationId");
        let target = format!("integrations/{integration_id}");

        let mut blockers: Vec<String> = Vec::new();
        for index in 0..blocking {
            blockers.push(
                create_route(
                    &emulator,
                    &api_id,
                    &format!("blocking-{index}"),
                    Some(&target),
                )
                .await,
            );
        }
        // A route of the same api wired to nothing, and one wired elsewhere, block
        // nothing: only the routes that TARGET this integration are named.
        create_route(&emulator, &api_id, "untargeted", None).await;
        let other = created(
            &emulator,
            &case,
            &format!("/v2/apis/{api_id}/integrations"),
            json!({ "integrationType": "MOCK" }),
        )
        .await;
        create_route(
            &emulator,
            &api_id,
            "elsewhere",
            Some(&format!("integrations/{}", as_str(&other, "integrationId"))),
        )
        .await;

        // The list is in the store's own `RouteId` order, which is what a reader pages
        // the api's routes in.
        blockers.sort();
        let expected = format!(
            "Cannot delete Integration because it is referenced by the following \
             Routes with Ids: [{}]",
            blockers.join(", ")
        );
        let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}");
        assert_refusal(
            &emulator,
            &case,
            "DELETE",
            &uri,
            None,
            Refusal {
                status: 409,
                code: "ConflictException",
                message: &expected,
            },
        )
        .await;

        // Every id the refusal names is a ROUTE of this api wired to this integration —
        // not the integration's own id — so a caller can act on the message.
        for route_id in &blockers {
            assert_ne!(
                route_id, &integration_id,
                "case {case:?}: the refusal must name the blocking RouteId, not the \
                 IntegrationId ({integration_id:?})"
            );
            let (status, route) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the named route {route_id:?} to be readable, \
                 got {status} {route}"
            );
            assert_eq!(
                as_str(&route, "target"),
                target,
                "case {case:?}: expected the named route {route_id:?} to target the \
                 integration, got {route}"
            );
        }

        // The refused delete persisted nothing: the integration is still there.
        let (status, still_there) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the integration readable, \
             got {status} {still_there}"
        );

        // Acting on the message resolves the conflict: retarget one of the named routes
        // and delete the rest, and the same delete then succeeds.
        let (status, retargeted) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/routes/{}", blockers[0]),
            Some(json!({ "target": format!("integrations/{}", as_str(&other, "integrationId")) })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from UpdateRoute, got {status} {retargeted}"
        );
        for route_id in blockers.iter().skip(1) {
            let (status, removed) = call(
                &emulator,
                "DELETE",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected 204 from DeleteRoute, got {status} {removed}"
            );
        }

        let (status, removed) = call(&emulator, "DELETE", &uri, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteIntegration once no route targets \
             it, got {status} {removed}"
        );
        assert_refusal(
            &emulator,
            &case,
            "GET",
            &uri,
            None,
            not_found(&invalid_identifier_message(INTEGRATION_RESOURCE)),
        )
        .await;
    }
}
