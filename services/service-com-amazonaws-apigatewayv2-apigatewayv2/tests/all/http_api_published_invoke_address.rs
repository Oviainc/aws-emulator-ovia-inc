// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ADDRESS an HTTP api's `ApiEndpoint` publishes.
//!
//! The rule, one level above the recorded case that filed the issue (an HTTP api with a
//! JWT authorizer whose published endpoint a client could not reach):
//!
//! > The address this service publishes for an api's data plane is one this emulator
//! > itself answers on, so the invoke URL a client builds from `ApiEndpoint` —
//! > that value with the stage name appended ("the stage name is typically appended to
//! > this URI to form a complete path to a deployed API stage", `Api$ApiEndpoint`) —
//! > reaches the api's route. A client configured with the emulator's own endpoint is
//! > announced that endpoint with the api behind the reserved data-plane path; a client
//! > that resolves AWS's own hosts is announced the `{api-id}.execute-api.{region}
//! > .amazonaws.com` host that member documents, which the front door claims host-style.
//!
//! The rows span what the rule generalises over: the endpoint the control-plane request
//! was addressed at (an IPv4 loopback literal, a loopback NAME, a bracketed IPv6 literal,
//! each with a port, and one of AWS's own service hosts), the Region the request is signed
//! for, the stage the api is deployed to (the reserved `$default` stage that serves the
//! api's root and a named one), and the operation that published the value (`CreateApi`,
//! `GetApi`, `GetApis`, `ImportApi`). The recorded case — one HTTP api read back from a
//! client pointed at the emulator's loopback endpoint — is ONE row.
//!
//! `api_endpoint_scheme` holds the SCHEME half of the same member's rule (the scheme
//! follows the `ProtocolType`, and a WEBSOCKET api announces the socket the core opened
//! for it); this suite holds the AUTHORITY half: whatever is announced is served.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host, emulator};
use crate::websocket_handshake_data_plane::{send, Answer};

/// The status an `HTTP_PROXY` route answers: the upstream is a URI the CALLER can reach, so
/// the caller is sent there — `307`, with that integration's own `IntegrationUri` in
/// `Location`. Reaching it proves the request was resolved all the way to the route's
/// integration, which is what an address being SERVED means.
const SERVED: u16 = 307;

/// The front door's envelope for a request no service claimed. The defect this suite pins
/// is an address nothing serves, so this is never a legitimate answer here.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The upstream the row's `HTTP_PROXY` integration fronts.
const UPSTREAM: &str = "https://upstream.example.com/items";

/// The route every row deploys, and the path within the stage that matches it.
const ROUTE_KEY: &str = "GET /items";
const ROUTE_PATH: &str = "/items";

/// The reserved stage "that serves an api's root" (`$default`), reached with no stage
/// segment in the path at all.
const DEFAULT_STAGE: &str = "$default";

/// Which address a row must be announced on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Announced {
    /// The emulator's own endpoint, with the api behind the reserved data-plane path.
    EmulatorEndpoint,
    /// The `{api-id}.execute-api.{region}.amazonaws.com` host `Api$ApiEndpoint` documents.
    ExecuteApiHost,
}

/// One row: the endpoint the control-plane calls are addressed at, the Region they are
/// signed for, the stage the api is deployed to, and the address the api must be announced
/// on.
struct Row {
    case: &'static str,
    /// The `Host` the control-plane request carries — the endpoint the client is
    /// configured with.
    host: &'static str,
    region: &'static str,
    stage_name: &'static str,
    announced: Announced,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a client configured with the emulator's loopback \
                   endpoint, deployed to the stage that serves the api's root",
            host: "127.0.0.1:45923",
            region: "us-west-2",
            stage_name: DEFAULT_STAGE,
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by its loopback NAME, deployed to a named stage",
            host: "localhost:9297",
            region: "eu-west-1",
            stage_name: "prod",
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by a bracketed IPv6 literal",
            host: "[::1]:9297",
            region: "ap-southeast-2",
            stage_name: "live",
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "a client that resolves AWS's own hosts: the documented execute-api host",
            host: "apigateway.us-east-1.amazonaws.com",
            region: "us-east-1",
            stage_name: DEFAULT_STAGE,
            announced: Announced::ExecuteApiHost,
        },
        Row {
            case: "a client that resolves AWS's own hosts, a named stage in another Region",
            host: "apigateway.eu-central-1.amazonaws.com",
            region: "eu-central-1",
            stage_name: "staging",
            announced: Announced::ExecuteApiHost,
        },
    ]
}

/// The announced endpoint split into the authority a client dials and the path it carries
/// ahead of the stage — empty for an endpoint that is a bare host.
fn announced_parts(announced: &str) -> (String, String) {
    let without_scheme = announced
        .split_once("://")
        .map_or(announced, |(_scheme, rest)| rest);
    match without_scheme.find('/') {
        Some(at) => (
            without_scheme[..at].to_string(),
            without_scheme[at..].to_string(),
        ),
        None => (without_scheme.to_string(), String::new()),
    }
}

/// Assert `announced` is an address this emulator answers on, in the form this row's
/// client is announced, and answer the authority and base path a client builds its invoke
/// URL from.
fn assert_announced(row: &Row, reporter: &str, api_id: &str, announced: &str) -> (String, String) {
    let scheme = "https://";
    assert!(
        announced.starts_with(scheme),
        "case {:?}: {reporter} must announce an {scheme} endpoint for an HTTP api, got \
         {announced:?}",
        row.case
    );
    let (authority, base_path) = announced_parts(announced);
    match row.announced {
        Announced::EmulatorEndpoint => {
            assert_eq!(
                authority, row.host,
                "case {:?}: {reporter} must announce the endpoint the request reached, got \
                 {announced:?}",
                row.case
            );
            assert!(
                !authority.ends_with("amazonaws.com"),
                "case {:?}: {reporter} must not announce an AWS host no client of this \
                 emulator resolves, got {announced:?}",
                row.case
            );
            assert_eq!(
                base_path,
                format!("/_aws/execute-api/{api_id}"),
                "case {:?}: {reporter} must name the api in the reserved data-plane path, \
                 got {announced:?}",
                row.case
            );
        }
        Announced::ExecuteApiHost => {
            assert_eq!(
                authority,
                format!("{api_id}.execute-api.{}.amazonaws.com", row.region),
                "case {:?}: {reporter} must announce the api's own execute-api host, got \
                 {announced:?}",
                row.case
            );
            assert!(
                base_path.is_empty(),
                "case {:?}: the documented endpoint carries no path of its own, got \
                 {announced:?}",
                row.case
            );
        }
    }
    (authority, base_path)
}

/// Deploy the row's api: an `HTTP_PROXY` integration, the route that fronts it and an
/// auto-deploying stage. Answers the `ApiId` and the `ApiEndpoint` `CreateApi` announced.
async fn deploy(emulator: &MultiServiceRouter, row: &Row, name: &str) -> (String, String) {
    let call = |method: &'static str, uri: String, body: Option<Value>| async move {
        call_at_host(emulator, row.host, row.region, method, &uri, body).await
    };

    let (status, api) = call(
        "POST",
        "/v2/apis".to_string(),
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateApi must answer 201, got {status} {api}",
        row.case
    );
    let api_id = as_str(&api, "apiId");
    let announced = as_str(&api, "apiEndpoint");

    let (status, integration) = call(
        "POST",
        format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationMethod": "GET",
            "integrationUri": UPSTREAM,
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateIntegration must answer 201, got {status} {integration}",
        row.case
    );
    let integration_id = as_str(&integration, "integrationId");

    let (status, route) = call(
        "POST",
        format!("/v2/apis/{api_id}/routes"),
        Some(json!({
            "routeKey": ROUTE_KEY,
            "target": format!("integrations/{integration_id}")
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateRoute must answer 201, got {status} {route}",
        row.case
    );

    let (status, stage) = call(
        "POST",
        format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": row.stage_name, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateStage must answer 201, got {status} {stage}",
        row.case
    );
    assert!(
        stage.get("deploymentId").and_then(Value::as_str).is_some(),
        "case {:?}: an auto-deploying stage must carry a deployment, got {stage}",
        row.case
    );

    (api_id, announced)
}

/// Send one invoke to `authority` at `path` and assert it was SERVED by the api's route —
/// and, first, that the front door did not answer that nothing claims the address.
async fn assert_served(
    emulator: &MultiServiceRouter,
    case: &str,
    form: &str,
    authority: &str,
    path: &str,
) {
    let answer: Answer = send(emulator, authority, "GET", path, &[]).await;
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the announced address must be one this emulator serves, but \
         {form} ({authority}{path}) is claimed by no service: {}",
        answer.summary()
    );
    assert_eq!(
        answer.status,
        SERVED,
        "case {case:?}: {form} ({authority}{path}) must reach the route's integration, got \
         {}",
        answer.summary()
    );
    assert_eq!(
        answer.location.as_deref(),
        Some(UPSTREAM),
        "case {case:?}: {form} must be sent to the upstream the integration names, got {}",
        answer.summary()
    );
}

/// The invoke URL every reporter publishes is an address this emulator serves: the api's
/// route answers on it, in each of the forms a client builds from the announced endpoint.
#[tokio::test]
async fn the_announced_api_endpoint_is_an_address_this_emulator_serves() {
    for (index, row) in rows().into_iter().enumerate() {
        // One emulator per row, so GetApis lists exactly this row's api.
        let emulator = emulator();
        let case = row.case;
        let (api_id, announced) =
            deploy(&emulator, &row, &format!("published-address-{index}")).await;
        let (authority, base_path) = assert_announced(&row, "CreateApi", &api_id, &announced);

        // Every reporter of the api reports the same address.
        let (status, fetched) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "GET",
            &format!("/v2/apis/{api_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApi must answer 200, got {status} {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "apiEndpoint"),
            announced,
            "case {case:?}: GetApi must report the announced ApiEndpoint, got {fetched}"
        );
        let (status, listed) =
            call_at_host(&emulator, row.host, row.region, "GET", "/v2/apis", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApis must answer 200, got {status} {listed}"
        );
        let endpoints: Vec<String> = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: GetApis served no items, got {listed}"))
            .iter()
            .map(|item| as_str(item, "apiEndpoint"))
            .collect();
        assert_eq!(
            endpoints,
            vec![announced.clone()],
            "case {case:?}: GetApis must list the announced ApiEndpoint, got {listed}"
        );

        // The invoke URL a client builds from that endpoint: the stage name appended, and —
        // for the stage that serves the api's root — the endpoint on its own, which is the
        // "typically" of `Api$ApiEndpoint`.
        let mut forms: Vec<(&str, String)> = vec![(
            "the endpoint with the stage name appended",
            format!("{base_path}/{}{ROUTE_PATH}", row.stage_name),
        )];
        if row.stage_name == DEFAULT_STAGE {
            forms.push((
                "the endpoint on its own, for the stage that serves the api's root",
                format!("{base_path}{ROUTE_PATH}"),
            ));
        }
        for (form, path) in forms {
            assert_served(&emulator, case, form, &authority, &path).await;
        }
    }
}

/// `ImportApi` announces the api on the same address `CreateApi` does, and that address is
/// served too: the api is reached through the route the definition declared.
#[tokio::test]
async fn an_imported_api_is_announced_on_an_address_this_emulator_serves() {
    let definition = json!({
        "openapi": "3.0.1",
        "info": { "title": "imported-published-address", "version": "1.0" },
        "paths": {
            "/items": {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": UPSTREAM,
                        "payloadFormatVersion": "1.0"
                    }
                }
            }
        }
    })
    .to_string();

    for row in rows() {
        let emulator = emulator();
        let case = row.case;
        let (status, imported) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "PUT",
            "/v2/apis",
            Some(json!({ "body": definition })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ImportApi must answer 200, got {status} {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let announced = as_str(&imported, "apiEndpoint");
        let (authority, base_path) = assert_announced(&row, "ImportApi", &api_id, &announced);

        // An imported api carries no stage, so one is created over it: the address is
        // served once the api has a deployment released to a stage, exactly as a created
        // api's is.
        let (status, stage) = call_at_host(
            &emulator,
            row.host,
            row.region,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": row.stage_name, "autoDeploy": true })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateStage must answer 201, got {status} {stage}"
        );

        assert_served(
            &emulator,
            case,
            "the imported api's endpoint with the stage name appended",
            &authority,
            &format!("{base_path}/{}{ROUTE_PATH}", row.stage_name),
        )
        .await;
    }
}
