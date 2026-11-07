// SPDX-License-Identifier: CC0-1.0

//! The PATH a Lambda proxy event reports for an invoke of a deployed HTTP api stage.
//!
//! The rule every row below holds the data plane to: **the path a proxy event reports —
//! `rawPath` and `requestContext.http.path` under payload format `2.0`, `path` and
//! `requestContext.path` under `1.0` — is the path the api's OWN endpoint was addressed at:
//! the stage's name as the first segment for a NAMED stage, because that stage's invoke URL
//! carries it ("the stage name is typically appended to this URI to form a complete path to
//! a deployed API stage", `Api$ApiEndpoint`), and no stage segment at all for the `$default`
//! stage, which serves the api's root — whichever of the addressings a client on this
//! machine reached the emulator by.**
//!
//! A client cannot resolve `{api-id}.execute-api.{region}.amazonaws.com` on the machine this
//! emulator runs on, so it carries in the PATH what it cannot carry in the host: the api id
//! ahead of the stage, the local endpoint marker, or the root stage's own name — and none of
//! those is part of the path the api was addressed at. An event that reported one would hand
//! the function a path no AWS deployment produces, so a handler that dispatches on it, or
//! rebuilds a URL or a signature from it, would route differently here than at AWS.
//!
//! The rows differ along what the rule generalises over: which stage serves the request (the
//! one that serves the api's root and a named one), HOW the request addresses the api (the
//! api's own `execute-api` host, that host with the root stage's name spelled out, the local
//! endpoint marker with and without that name, the api id as the first path segment, and the
//! stage segment alone), the SHAPE of the route key the path matches (a literal path, a path
//! variable, the greedy remainder and the `$default` catch-all), the number of path segments,
//! and the payload format version the event is built in (both values the model admits).
//!
//! The event is read off what the FUNCTION returned — the echo handler
//! `http_api_backend_dispatch` shares — so nothing asserted here is a value this emulator
//! filled in.

use serde_json::Value;

use crate::http_api_backend_dispatch::{
    emulator_with_lambda, given_api, given_function, lambda_integration, ECHO_EVENT,
};
use crate::websocket_handshake_data_plane::send;

/// The stage that serves an api's root, reached with no stage segment in the path at all.
const ROOT_STAGE: &str = "$default";

/// The marker a rewritten endpoint keeps the api's own endpoint behind, as
/// `service::data_plane::LOCAL_ENDPOINT_PREFIX` spells it.
const LOCAL_MARKER: &str = "/_aws/execute-api";

/// An address of the emulator's own endpoint, which names no service: what a client that
/// cannot resolve the api's host sends to.
const LOCAL_HOST: &str = "127.0.0.1:9297";

/// How the request reaches the emulator — the addressings `service::invoke` resolves.
#[derive(Debug, Clone, Copy)]
enum Addressing {
    /// The api's own `ApiEndpoint` host, as `Api$ApiEndpoint` documents it: the invoke URL
    /// path and nothing else.
    ApiHost,
    /// The api's own host with the stage's name spelled out as the first segment, which is
    /// how a client that appends every stage name reaches the root stage here.
    ApiHostNamingTheStage,
    /// The emulator's own endpoint, with the api's endpoint behind the local marker.
    LocalMarker,
    /// The same, with the stage's name spelled out too.
    LocalMarkerNamingTheStage,
    /// The emulator's own endpoint, with the api id as the first path segment.
    ApiIdFirst,
    /// The emulator's own endpoint, with the stage segment alone naming the deployment.
    StageOnly,
}

/// One row: the stage the request is served by, the route the api declares, the path within
/// that stage the request asks for, the addressing it arrives by, and the payload format the
/// event is built in.
struct Row {
    case: &'static str,
    stage: &'static str,
    route_key: &'static str,
    /// The path within the stage — what the route key matches.
    route_path: &'static str,
    addressing: Addressing,
    payload_format: &'static str,
}

/// The path the api's own endpoint is addressed at for `route_path` of `stage`: the rule the
/// rows are held to, stated once — a named stage's name ahead of the path, and nothing ahead
/// of it for the stage that serves the api's root.
fn endpoint_path(stage: &str, route_path: &str) -> String {
    if stage == ROOT_STAGE {
        route_path.to_string()
    } else {
        format!("/{stage}{route_path}")
    }
}

/// The host and path the row's addressing sends the request to.
fn addressed(row: &Row, api_id: &str, endpoint_host: &str) -> (String, String) {
    let aws_path = endpoint_path(row.stage, row.route_path);
    let staged = format!("/{}{}", row.stage, row.route_path);
    match row.addressing {
        Addressing::ApiHost => (endpoint_host.to_string(), aws_path),
        Addressing::ApiHostNamingTheStage => (endpoint_host.to_string(), staged),
        Addressing::LocalMarker => (
            LOCAL_HOST.to_string(),
            format!("{LOCAL_MARKER}/{api_id}{aws_path}"),
        ),
        Addressing::LocalMarkerNamingTheStage => (
            LOCAL_HOST.to_string(),
            format!("{LOCAL_MARKER}/{api_id}{staged}"),
        ),
        Addressing::ApiIdFirst => (LOCAL_HOST.to_string(), format!("/{api_id}{aws_path}")),
        Addressing::StageOnly => (LOCAL_HOST.to_string(), staged),
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the root stage on the api's own host",
            stage: ROOT_STAGE,
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::ApiHost,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage reached with its own name as a path segment",
            stage: ROOT_STAGE,
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::ApiHostNamingTheStage,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage behind the local endpoint marker",
            stage: ROOT_STAGE,
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::LocalMarker,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage behind the marker, with its own name too",
            stage: ROOT_STAGE,
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::LocalMarkerNamingTheStage,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage with the api id as the first segment",
            stage: ROOT_STAGE,
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::ApiIdFirst,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage named by its segment alone",
            stage: ROOT_STAGE,
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::StageOnly,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage, a path variable and several segments",
            stage: ROOT_STAGE,
            route_key: "GET /orders/{id}/items",
            route_path: "/orders/7/items",
            addressing: Addressing::StageOnly,
            payload_format: "2.0",
        },
        Row {
            case: "the root stage and the greedy remainder, under 1.0",
            stage: ROOT_STAGE,
            route_key: "ANY /files/{proxy+}",
            route_path: "/files/a/b/c",
            addressing: Addressing::LocalMarkerNamingTheStage,
            payload_format: "1.0",
        },
        Row {
            case: "the root stage and the $default catch-all route, under 1.0",
            stage: ROOT_STAGE,
            route_key: "$default",
            route_path: "/anything/at/all",
            addressing: Addressing::StageOnly,
            payload_format: "1.0",
        },
        Row {
            case: "the root stage at the api's root path",
            stage: ROOT_STAGE,
            route_key: "$default",
            route_path: "/",
            addressing: Addressing::ApiHostNamingTheStage,
            payload_format: "2.0",
        },
        Row {
            case: "a named stage on the api's own host",
            stage: "prod",
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::ApiHost,
            payload_format: "2.0",
        },
        Row {
            case: "a named stage behind the local endpoint marker",
            stage: "prod",
            route_key: "GET /orders/{id}",
            route_path: "/orders/7",
            addressing: Addressing::LocalMarker,
            payload_format: "2.0",
        },
        Row {
            case: "a named stage with the api id as the first segment, under 1.0",
            stage: "prod",
            route_key: "GET /alpha",
            route_path: "/alpha",
            addressing: Addressing::ApiIdFirst,
            payload_format: "1.0",
        },
        Row {
            case: "a named stage named by its segment alone",
            stage: "staging",
            route_key: "ANY /files/{proxy+}",
            route_path: "/files/a/b",
            addressing: Addressing::StageOnly,
            payload_format: "2.0",
        },
    ]
}

/// The two members of the event that carry the path, in the payload format's own spelling.
fn path_members(payload_format: &str) -> [(&'static str, &'static str); 2] {
    if payload_format == "2.0" {
        [
            ("the raw path", "/rawPath"),
            ("the request context's path", "/requestContext/http/path"),
        ]
    } else {
        [
            ("the path", "/path"),
            ("the request context's path", "/requestContext/path"),
        ]
    }
}

#[tokio::test]
async fn the_event_reports_the_path_the_apis_own_endpoint_was_addressed_at() {
    for row in rows() {
        let case = row.case;
        let (emulator, engine) = emulator_with_lambda();
        let function = given_function(&emulator, &engine, case, "event-path-function", ECHO_EVENT)
            .await;
        let (api_id, endpoint_host) = given_api(
            &emulator,
            case,
            lambda_integration(&function, row.payload_format),
            row.route_key,
            serde_json::json!({ "stageName": row.stage, "autoDeploy": true }),
        )
        .await;
        let (host, path) = addressed(&row, &api_id, &endpoint_host);
        let answer = send(&emulator, &host, "GET", &path, &[]).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the function's 200 for {path:?}, got {}",
            answer.summary()
        );
        let event: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
        let expected = endpoint_path(row.stage, row.route_path);
        for (member, pointer) in path_members(row.payload_format) {
            let actual = event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual,
                serde_json::json!(expected),
                "case {case:?}: expected {member} ({pointer}) of the event for {path:?} to be \
                 {expected:?}, got {actual:?}"
            );
        }
        // Whatever the addressing carried, the event's own path members name neither the
        // root stage nor anything the emulator's addressing added: the stage name belongs on
        // `requestContext.stage`, and the api id nowhere in a path.
        if row.stage == ROOT_STAGE {
            for pointer in path_members(row.payload_format).map(|(_, pointer)| pointer) {
                let actual = event
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !actual.contains(ROOT_STAGE) && !actual.contains(&api_id),
                    "case {case:?}: expected {pointer} to carry neither the stage name nor the \
                     api id, got {actual:?}"
                );
            }
        }
    }
}
