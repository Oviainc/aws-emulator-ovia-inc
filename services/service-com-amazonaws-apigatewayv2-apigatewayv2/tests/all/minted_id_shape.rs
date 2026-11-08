// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SHAPE of the identifiers this service MINTS.
//!
//! The rules these cases assert:
//!
//! * **A created resource answers an identifier of the family API Gateway v2 answers for
//!   that kind of resource**: lowercase alphanumerics, the short opaque form for a
//!   resource nested inside an api (an authorizer, a model, a deployment, a route
//!   response, an integration response, a route, an integration) and the longer form for
//!   a resource the account holds. `Id` is an unconstrained string in the model
//!   (`smithy_query(query="shape", shape="Id")`), so the per-kind length is this service's
//!   convention, stated once in `api_lifecycle::minted_id_len` and asserted here against
//!   every create that mints one.
//! * **A minted identifier denotes ONE resource**: no two resources of a run carry the
//!   same id, so a client that caches, indexes or round-trips ids never collides two of
//!   them — not even the first child of one api against the first child of another, which
//!   a counter shared by every collection would answer identically.
//! * **No character position of a minted id is stuck on one character**: the ids of one
//!   collection vary in every position, so none of them carries a constant run that the
//!   id family has no room for.
//! * Every minted id RESOLVES: the resource reads back under it through its own reader.
//!
//! The tables span what the rules generalise over: the kind of resource (every create
//! that mints an id), the scope the id is minted in (several apis, several owning routes
//! and integrations), and the number of ids drawn from one collection. The recorded
//! `CreateAuthorizer` / `CreateModel` / `CreateRouteResponse` calls that filed the issue
//! are three rows of the first table.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, minted_id_len};

/// How many ids one collection is asked for when checking that no position of an id is
/// stuck. Eight stays inside the tightest per-api quota any of these kinds has
/// (`MAX_AUTHORIZERS_PER_API` is ten) and makes a position that varies at all almost
/// certain to be seen varying.
const SAMPLE: usize = 8;

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI".
fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:us-east-1:123456789012:function:{function}/invocations"
    )
}

/// POST one request that must be accepted with 201 and answer the created body.
async fn created(emulator: &MultiServiceRouter, case: &str, uri: &str, request: Value) -> Value {
    let (status, body) = call(emulator, "POST", uri, Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from POST {uri}, got {status} {body}"
    );
    body
}

/// One minted id: the member it is reported under, the id itself, and the reader that
/// must resolve the resource back under it.
struct Minted {
    member: &'static str,
    id: String,
    reader: String,
}

/// Create one WebSocket api named `name` and one resource of every kind nested in it,
/// answering each minted id with the reader that resolves it.
///
/// WebSocket, because a model and a route response are WebSocket-only resources
/// (`RouteResponse$ModelSelectionExpression`: "Supported only for WebSocket APIs") while
/// every other kind here exists under either protocol.
async fn mint_family(emulator: &MultiServiceRouter, case: &str, name: &str) -> Vec<Minted> {
    let api = created(
        emulator,
        case,
        "/v2/apis",
        json!({
            "name": name,
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    let api_id = as_str(&api, "apiId");

    let route = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/routes"),
        json!({ "routeKey": "$connect" }),
    )
    .await;
    let route_id = as_str(&route, "routeId");

    let integration = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/integrations"),
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let integration_id = as_str(&integration, "integrationId");

    let authorizer = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/authorizers"),
        json!({
            "name": "connect-auth",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("connect-auth"),
            "identitySource": ["route.request.header.Auth"]
        }),
    )
    .await;

    let model = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/models"),
        json!({ "name": "Empty", "schema": "{}" }),
    )
    .await;

    let deployment = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/deployments"),
        json!({}),
    )
    .await;

    let route_response = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
        json!({ "routeResponseKey": "$default" }),
    )
    .await;

    let integration_response = created(
        emulator,
        case,
        &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
        json!({ "integrationResponseKey": "$default" }),
    )
    .await;

    let mut minted = vec![
        Minted {
            member: "apiId",
            id: api_id.clone(),
            reader: format!("/v2/apis/{api_id}"),
        },
        Minted {
            member: "routeId",
            id: route_id.clone(),
            reader: format!("/v2/apis/{api_id}/routes/{route_id}"),
        },
        Minted {
            member: "integrationId",
            id: integration_id.clone(),
            reader: format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        },
    ];
    for (member, body) in [
        ("authorizerId", &authorizer),
        ("modelId", &model),
        ("deploymentId", &deployment),
    ] {
        let id = as_str(body, member);
        let collection = match member {
            "authorizerId" => "authorizers",
            "modelId" => "models",
            _ => "deployments",
        };
        minted.push(Minted {
            member,
            reader: format!("/v2/apis/{api_id}/{collection}/{id}"),
            id,
        });
    }
    let route_response_id = as_str(&route_response, "routeResponseId");
    minted.push(Minted {
        member: "routeResponseId",
        reader: format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/{route_response_id}"),
        id: route_response_id,
    });
    let integration_response_id = as_str(&integration_response, "integrationResponseId");
    minted.push(Minted {
        member: "integrationResponseId",
        reader: format!(
            "/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses/\
             {integration_response_id}"
        ),
        id: integration_response_id,
    });
    minted
}

#[tokio::test]
async fn every_create_answers_an_id_of_its_kinds_family_and_the_resource_reads_back_under_it() {
    let emulator = emulator();

    for minted in mint_family(&emulator, "one api of every child kind", "id-family").await {
        let case = format!("the minted {}", minted.member);
        assert_minted_id(&case, minted.member, &minted.id);

        let (status, fetched) = call(&emulator, "GET", &minted.reader, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GET {}, got {status} {fetched}",
            minted.reader
        );
        assert_eq!(
            as_str(&fetched, minted.member),
            minted.id,
            "case {case:?}: the reader must report the id it was addressed by, got {fetched}"
        );
    }
}

/// How many apis the collision case mints a whole family under: three, so a first, a
/// second and a third collection of each kind are compared, not just two.
const SCOPES: usize = 3;

#[tokio::test]
async fn no_two_resources_of_one_run_carry_the_same_minted_id() {
    let emulator = emulator();

    // id -> the case that already minted it. Every collection counts its own ids from its
    // own seed, so the n-th resource of one api and the n-th of another are the pair a
    // shared counter answers identically.
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut minted_count = 0usize;
    for scope in 0..SCOPES {
        let case = format!("api {scope}");
        for minted in mint_family(&emulator, &case, &format!("id-collision-{scope}")).await {
            minted_count += 1;
            let owner = format!("{case}'s {}", minted.member);
            if let Some(other) = seen.insert(minted.id.clone(), owner.clone()) {
                panic!(
                    "case {owner:?}: the id {:?} was already minted for {other:?} — a minted \
                     identifier must denote one resource",
                    minted.id
                );
            }
        }
    }
    assert_eq!(
        seen.len(),
        minted_count,
        "case {SCOPES:?} apis: expected every one of the {minted_count} minted ids to be \
         distinct, got {} distinct",
        seen.len()
    );
}

/// Mint `SAMPLE` ids of `member`'s kind within ONE collection, in the order they were
/// minted.
async fn mint_many(emulator: &MultiServiceRouter, member: &str) -> Vec<String> {
    let case = format!("{SAMPLE} {member}s of one collection");
    if member == "apiId" {
        // The account's own collection of apis, which needs no api to hold it.
        let mut ids = Vec::with_capacity(SAMPLE);
        for index in 0..SAMPLE {
            let api = created(
                emulator,
                &case,
                "/v2/apis",
                json!({
                    "name": format!("stuck-position-api-{index}"),
                    "protocolType": "HTTP"
                }),
            )
            .await;
            ids.push(as_str(&api, "apiId"));
        }
        return ids;
    }
    let api = created(
        emulator,
        &case,
        "/v2/apis",
        json!({
            "name": format!("stuck-position-{member}"),
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    let api_id = as_str(&api, "apiId");
    let host_route = created(
        emulator,
        &case,
        &format!("/v2/apis/{api_id}/routes"),
        json!({ "routeKey": "$connect" }),
    )
    .await;
    let host_route_id = as_str(&host_route, "routeId");
    let host_integration = created(
        emulator,
        &case,
        &format!("/v2/apis/{api_id}/integrations"),
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let host_integration_id = as_str(&host_integration, "integrationId");

    let mut ids = Vec::with_capacity(SAMPLE);
    for index in 0..SAMPLE {
        let (uri, request) = match member {
            "routeId" => (
                format!("/v2/apis/{api_id}/routes"),
                json!({ "routeKey": format!("action{index}") }),
            ),
            "integrationId" => (
                format!("/v2/apis/{api_id}/integrations"),
                json!({ "integrationType": "MOCK" }),
            ),
            "authorizerId" => (
                format!("/v2/apis/{api_id}/authorizers"),
                json!({
                    "name": format!("auth{index}"),
                    "authorizerType": "REQUEST",
                    "authorizerUri": lambda_uri("stuck-auth"),
                    "identitySource": ["route.request.header.Auth"]
                }),
            ),
            "modelId" => (
                format!("/v2/apis/{api_id}/models"),
                json!({ "name": format!("Model{index}"), "schema": "{}" }),
            ),
            "deploymentId" => (format!("/v2/apis/{api_id}/deployments"), json!({})),
            "routeResponseId" => (
                format!("/v2/apis/{api_id}/routes/{host_route_id}/routeresponses"),
                json!({ "routeResponseKey": format!("key{index}") }),
            ),
            "integrationResponseId" => (
                format!(
                    "/v2/apis/{api_id}/integrations/{host_integration_id}/integrationresponses"
                ),
                json!({ "integrationResponseKey": format!("/{}/", 200 + index) }),
            ),
            other => panic!("this suite mints no collection of {other:?}"),
        };
        let body = created(emulator, &case, &uri, request).await;
        ids.push(as_str(&body, member));
    }
    ids
}

#[tokio::test]
async fn no_character_position_of_a_minted_id_is_stuck_on_one_character() {
    let emulator = emulator();

    // Every kind whose ids one collection can be asked for in bulk, so both id lengths
    // this service mints are covered: the account's own apis carry the ten-character form
    // and the rest the short one.
    for member in [
        "apiId",
        "routeId",
        "integrationId",
        "authorizerId",
        "modelId",
        "deploymentId",
        "routeResponseId",
        "integrationResponseId",
    ] {
        let ids = mint_many(&emulator, member).await;
        let case = format!("{SAMPLE} minted {member}s");
        for (index, id) in ids.iter().enumerate() {
            assert_minted_id(&format!("{case}, number {index}"), member, id);
        }
        for position in 0..minted_id_len(member) {
            let characters: Vec<char> = ids
                .iter()
                .filter_map(|id| id.chars().nth(position))
                .collect();
            let first = characters.first().copied().unwrap_or_default();
            assert!(
                characters.iter().any(|character| *character != first),
                "case {case:?}: every id carries {first:?} at position {position} \
                 — that position is stuck; got {ids:?}"
            );
        }
    }
}
