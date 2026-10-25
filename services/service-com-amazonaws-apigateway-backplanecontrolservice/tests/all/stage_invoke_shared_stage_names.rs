// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the addressing of a deployed REST API stage that carries the api
//! id IN THE PATH — the one a client that cannot reach
//! `{rest-api-id}.execute-api.{region}.amazonaws.com` has.
//!
//! One endpoint serves this emulator's whole surface, so the two things AWS puts in an
//! invoke URL's HOST — that the request is for the `execute-api` data plane, and which api
//! — have to travel in the path. The rules asserted here are:
//!
//! * a request whose path names the data plane and then the api
//!   (`/_aws/execute-api/{api-id}/{stage}/{resource-path}`) is answered by exactly that
//!   api's stage, on whatever host it arrives at, and answers the same as the api's own
//!   `execute-api` host does;
//! * that addressing is what makes every api reachable when SEVERAL of them publish a stage
//!   of the same name — a stage name is unique within an api, not across an account's apis,
//!   and the invoke path alone names no one of them.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service — so what is asserted is what a client sees, not what a handler
//! returns.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the prefixed
//! addressing over a table of api ids (held, another api's, none at all), stages (deployed,
//! not deployed), resource paths (declared, the stage root, undeclared) and verbs; and the
//! shared-stage-name case over a table that varies the resource path, the verb and the
//! number of apis sharing one stage name.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};

/// The region an api's own invoke host is addressed in; any region is served.
const REGION: &str = "us-east-1";

/// The emulator's own endpoint, as a client on this machine reaches it: a loopback address,
/// which names no service to the front door.
const OWN_ENDPOINT: &str = "127.0.0.1:9297";

/// This service's control-plane host, which the front door DOES resolve to a service: the
/// prefixed addressing is read on it too, since the path names the data plane itself.
const CONTROL_PLANE_HOST: &str = "apigateway.us-east-1.amazonaws.com";

/// The reserved path prefix that names the data plane ahead of the api id.
const DATA_PLANE_PREFIX: &str = "/_aws/execute-api";

/// One answered invoke: the status line and the body verbatim.
struct Invoked {
    status: u16,
    body: String,
}

/// Issue one invoke of `path` with `verb`, addressed at `host`.
async fn invoke(gateway: &Gateway, host: &str, verb: &str, path: &str) -> Invoked {
    let request = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .body(Bytes::from_static(b"{\"sent\":true}"))
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Invoked { status, body }
}

/// `CreateDeployment` of `stage` on `api`, as a developer publishes one.
async fn deploy(gateway: &Gateway, api: &str, stage: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment({api}, {stage:?}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// Put a `MOCK` `verb` method on `resource` of `api` answering `body`, through the control
/// plane, exactly as a developer builds one.
async fn mock_method(gateway: &Gateway, api: &str, resource: &str, verb: &str, body: &str) {
    let answers: &[(&str, String, Value)] = &[
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
            json!({ "authorizationType": "NONE" }),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": "{\"statusCode\": 200}" },
            }),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/{verb}/responses/200"),
            json!({}),
        ),
        (
            "PUT",
            format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/200"
            ),
            json!({ "responseTemplates": { "application/json": body } }),
        ),
    ];
    for (method, uri, request) in answers {
        let answer = gateway.call(method, uri, Some(request.clone())).await;
        assert!(
            (200..300).contains(&answer.status),
            "case {uri}: expected the control-plane call to be accepted, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// One api with a `MOCK` `GET` on its root and a `MOCK` `verb` method on `/{resource}`,
/// deployed to `stage`. Each method answers a body naming the api and the resource.
///
/// That body is what tells WHICH api of WHICH resource answered, so a row cannot pass by
/// being served by the wrong api in the same status class.
async fn api_serving(
    gateway: &Gateway,
    name: &str,
    resource: &str,
    verb: &str,
    stage: &str,
) -> String {
    let (api, root) = create_api(gateway, name).await;
    mock_method(gateway, &api, &root, "GET", &format!("{{\"api\":\"{name}\",\"at\":\"root\"}}")).await;
    let child = create_resource(gateway, &api, &root, resource).await;
    let child = string_member(&child, "id", &format!("CreateResource({resource:?})"));
    let body = format!("{{\"api\":\"{name}\",\"at\":\"{resource}\"}}");
    mock_method(gateway, &api, &child, verb, &body).await;
    deploy(gateway, &api, stage).await;
    api
}

#[tokio::test]
async fn a_path_that_names_the_data_plane_and_the_api_reaches_that_api_s_stage() {
    let gateway = Gateway::new();
    let pets = api_serving(&gateway, "prefixed-pets", "pets", "GET", "prod").await;
    let items = api_serving(&gateway, "prefixed-items", "items", "POST", "prod").await;
    let staged = api_serving(&gateway, "prefixed-staged", "orders", "GET", "staging").await;

    // Rows differ along every part of the addressing: which api id the path names, which
    // stage, which resource path (a declared one, the stage root, one the deployment does
    // not hold) and which verb. The body proves which api answered.
    let cases: &[(&str, &str, String, u16, &str)] = &[
        (
            "the api and stage of a declared resource",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets}/prod/pets"),
            200,
            r#"{"api":"prefixed-pets","at":"pets"}"#,
        ),
        (
            "another api, named by the same prefix",
            "POST",
            format!("{DATA_PLANE_PREFIX}/{items}/prod/items"),
            200,
            r#"{"api":"prefixed-items","at":"items"}"#,
        ),
        (
            "a stage name only this api holds",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{staged}/staging/orders"),
            200,
            r#"{"api":"prefixed-staged","at":"orders"}"#,
        ),
        (
            "the stage root, with no resource path after it",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets}/prod"),
            200,
            r#"{"api":"prefixed-pets","at":"root"}"#,
        ),
        (
            "a resource path another api declares, not this one",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets}/prod/items"),
            403,
            "Missing Authentication Token",
        ),
        (
            "a verb the named api does not declare for that path",
            "DELETE",
            format!("{DATA_PLANE_PREFIX}/{pets}/prod/pets"),
            403,
            "Missing Authentication Token",
        ),
        (
            "a stage the named api has not deployed",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets}/staging/pets"),
            403,
            "Forbidden",
        ),
        (
            "no stage at all after the api id",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets}"),
            403,
            "Forbidden",
        ),
    ];

    for (case, verb, path, status, expected) in cases {
        for host in [OWN_ENDPOINT, CONTROL_PLANE_HOST] {
            let answered = invoke(&gateway, host, verb, path).await;
            assert_eq!(
                answered.status, *status,
                "case {case:?} ({verb} {path} at {host}): expected {status}, got {} with {:?}",
                answered.status, answered.body
            );
            assert!(
                answered.body.contains(expected),
                "case {case:?} ({verb} {path} at {host}): expected a body carrying \
                 {expected:?}, got {:?}",
                answered.body
            );
            assert!(
                !answered.body.contains("UnknownService"),
                "case {case:?} ({verb} {path} at {host}): a stage this service published \
                 must never be answered by the front door's own refusal, got {:?}",
                answered.body
            );
        }
    }

    // The prefix is an ADDRESSING, not a second behaviour: the api's own `execute-api`
    // host answers each row the same way.
    let both_ways: &[(&str, &str, String, String)] = &[
        (
            "prefixed-pets",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets}/prod/pets"),
            "/prod/pets".to_string(),
        ),
        (
            "prefixed-items",
            "POST",
            format!("{DATA_PLANE_PREFIX}/{items}/prod/items"),
            "/prod/items".to_string(),
        ),
        (
            "prefixed-staged",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{staged}/staging/orders"),
            "/staging/orders".to_string(),
        ),
    ];
    for (name, verb, prefixed, on_host) in both_ways {
        let api = match *name {
            "prefixed-pets" => &pets,
            "prefixed-items" => &items,
            _ => &staged,
        };
        let through_prefix = invoke(&gateway, OWN_ENDPOINT, verb, prefixed).await;
        let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
        let through_host = invoke(&gateway, &host, verb, on_host).await;
        assert_eq!(
            (through_prefix.status, &through_prefix.body),
            (through_host.status, &through_host.body),
            "case {name:?}: the prefixed path and the api's own host must answer the same, \
             got {} {:?} and {} {:?}",
            through_prefix.status,
            through_prefix.body,
            through_host.status,
            through_host.body
        );
    }
}

#[tokio::test]
async fn an_api_id_this_service_does_not_hold_is_not_claimed_by_it() {
    let gateway = Gateway::new();
    // One deployed api, so the rows below are refused for the id they name and not because
    // the service holds nothing at all.
    let held = api_serving(&gateway, "prefix-held", "pets", "GET", "prod").await;

    // An api id of another account's — or another service's — api is not this service's to
    // answer: an API Gateway V2 api is reached at the same shape and its own service claims
    // it, so nothing here is claimed for an id this store does not hold.
    let cases: &[(&str, String)] = &[
        (
            "an api id no api of this service holds",
            format!("{DATA_PLANE_PREFIX}/zzz9999999/prod/pets"),
        ),
        (
            "the prefix with no api id after it",
            DATA_PLANE_PREFIX.to_string(),
        ),
        (
            "the prefix's first segment alone",
            "/_aws/prod/pets".to_string(),
        ),
    ];
    for (case, path) in cases {
        let answered = invoke(&gateway, OWN_ENDPOINT, "GET", path).await;
        assert_eq!(
            answered.status, 404,
            "case {case:?} ({path}): expected the front door's own refusal, got {} with {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body.contains("UnknownService"),
            "case {case:?} ({path}): expected the front door's own refusal body, got {:?}",
            answered.body
        );
    }

    // The held api still answers through the same prefix, so the rows above are refused for
    // the id they name.
    let answered = invoke(
        &gateway,
        OWN_ENDPOINT,
        "GET",
        &format!("{DATA_PLANE_PREFIX}/{held}/prod/pets"),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case \"the api this service holds\": expected 200, got {} with {:?}",
        answered.status, answered.body
    );
}

#[tokio::test]
async fn apis_sharing_one_stage_name_are_each_reachable_by_the_addressing_that_names_them() {
    let gateway = Gateway::new();
    // FOUR apis publishing the SAME stage name, which is what a client that deploys more
    // than one api has, plus one holding a stage name of its own — the degenerate count.
    // `pets` is declared twice, by one api for `GET` and by another for `POST`, so the rows
    // below vary the resource path and the verb as well as the count.
    let pets_get = api_serving(&gateway, "shared-pets-get", "pets", "GET", "prod").await;
    let items = api_serving(&gateway, "shared-items", "items", "GET", "prod").await;
    let pets_post = api_serving(&gateway, "shared-pets-post", "pets", "POST", "prod").await;
    let carts = api_serving(&gateway, "shared-carts", "carts", "GET", "prod").await;
    let sole = api_serving(&gateway, "sole-stage-name", "users", "GET", "staging").await;

    // Each api answers its own route through the addressing that NAMES it, whatever other
    // apis publish a stage of the same name: the rows differ along the api, the stage name,
    // the resource path (a declared one and the stage root) and the verb.
    let addressed: &[(&str, &str, String, &str)] = &[
        (
            "shared-pets-get",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{pets_get}/prod/pets"),
            r#"{"api":"shared-pets-get","at":"pets"}"#,
        ),
        (
            "shared-items",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{items}/prod/items"),
            r#"{"api":"shared-items","at":"items"}"#,
        ),
        (
            "shared-pets-post",
            "POST",
            format!("{DATA_PLANE_PREFIX}/{pets_post}/prod/pets"),
            r#"{"api":"shared-pets-post","at":"pets"}"#,
        ),
        (
            "shared-carts, at its stage root",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{carts}/prod"),
            r#"{"api":"shared-carts","at":"root"}"#,
        ),
        (
            "sole-stage-name, the one unshared stage name",
            "GET",
            format!("{DATA_PLANE_PREFIX}/{sole}/staging/users"),
            r#"{"api":"sole-stage-name","at":"users"}"#,
        ),
    ];
    for (case, verb, path, expected) in addressed {
        let answered = invoke(&gateway, OWN_ENDPOINT, verb, path).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?} ({verb} {path}): expected 200, got {} with {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body.contains(expected),
            "case {case:?} ({verb} {path}): expected a body carrying {expected:?}, got {:?}",
            answered.body
        );
    }

    // The invoke path ALONE names one of them only while one api holds that stage name: the
    // shared one names no ONE api and is left to the front door, which is why the addressing
    // above exists.
    let stage_only: &[(&str, &str, &str, u16, &str)] = &[
        (
            "a stage name four apis hold",
            "GET",
            "/prod/pets",
            404,
            "UnknownService",
        ),
        (
            "the same stage name under another verb",
            "POST",
            "/prod/pets",
            404,
            "UnknownService",
        ),
        (
            "a stage name one api alone holds",
            "GET",
            "/staging/users",
            200,
            r#"{"api":"sole-stage-name","at":"users"}"#,
        ),
        (
            "that api's stage root, which the stage name still places",
            "GET",
            "/staging",
            200,
            r#"{"api":"sole-stage-name","at":"root"}"#,
        ),
    ];
    for (case, verb, path, status, expected) in stage_only {
        let answered = invoke(&gateway, OWN_ENDPOINT, verb, path).await;
        assert_eq!(
            answered.status, *status,
            "case {case:?} ({verb} {path}): expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        assert!(
            answered.body.contains(expected),
            "case {case:?} ({verb} {path}): expected a body carrying {expected:?}, got {:?}",
            answered.body
        );
    }
}
