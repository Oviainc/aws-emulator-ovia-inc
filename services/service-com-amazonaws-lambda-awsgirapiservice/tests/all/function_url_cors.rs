// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CROSS-ORIGIN half of a FUNCTION URL: what the endpoint
//! `CreateFunctionUrlConfig` hands out does with the `Cors` settings it was configured with.
//!
//! The rule under test: `FunctionUrlConfig$Cors` is the ENDPOINT's setting, so the endpoint
//! applies it. A CORS PREFLIGHT — an `OPTIONS` request carrying `Origin` and
//! `Access-Control-Request-Method` — is answered by the endpoint ITSELF, HTTP 200 stating the
//! permission the configuration grants (the admitted origin, `AllowMethods`, `AllowHeaders`,
//! `MaxAge`, `AllowCredentials`) with the function NOT invoked, and is refused 403 when the
//! configuration admits neither the origin it came from nor the method it asks about. Every
//! other request is served exactly as a request carrying no `Origin` is — the function runs
//! and its own response is answered — with the permission ADDED for an admitted origin (the
//! origin, `ExposeHeaders`, `AllowCredentials`), and with nothing added, and nothing
//! withheld, for an origin the configuration does not admit: the headers are what a BROWSER
//! enforces with, so this endpoint never refuses a request over them. A URL configured with
//! no `Cors` states no permission at all and its preflight reaches the function like any
//! other request.
//!
//! Every row drives a real HTTP request through the router against a function whose
//! deployment package RUNS, and the handler answers `418` to any `OPTIONS` request that
//! reaches it — so a preflight the endpoint answered itself is told apart from one the
//! function answered, rather than inferred. The rows differ along what the rule generalises
//! over: whether the request is a preflight, the origin it carries (admitted, the second
//! admitted one, not admitted, absent), the method it asks about (admitted, admitted through
//! the wildcard, not admitted, absent so not a preflight), and which collections the
//! configuration holds (all of them, the wildcards alone, none at all).
//!
//! The last two rows are about this EMULATOR rather than about Lambda, and are here so this
//! suite cannot be read as having moved them: the front door refuses a request whose
//! browser-set headers say a browser sent it from another site before any endpoint of this
//! service sees it, whatever the stored CORS settings grant.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_url_data_plane::{
    given_configured_url, given_function, grant, router_on, send, Endpoint,
};

/// The origin the configured URL below admits by name.
const ADMITTED_ORIGIN: &str = "https://allowed.example";

/// An origin no configuration below admits by name.
const OTHER_ORIGIN: &str = "https://other.example";

/// The entry either CORS collection carries to stand for every value
/// (`smithy_query(query="shape", shape="Cors")`: "you can grant access to all origins using
/// the wildcard character (`*`)").
const WILDCARD: &str = "*";

/// The status the handler answers an `OPTIONS` request with, so a preflight that REACHED it
/// is visible as such. Nothing of CORS is in it: it is the function's own answer.
const HANDLER_OPTIONS_STATUS: u16 = 418;

/// What the handler puts in the body of every other answer, so a row can tell a run's answer
/// from the endpoint's own.
const MARKER: &str = "ran";

/// What the ENDPOINT's own refusal of a preflight its configuration does not admit says.
const ENDPOINT_REFUSAL: &str = "Forbidden";

/// What the emulator's FRONT DOOR says when it refuses a request whose browser-set headers
/// say a browser sent it from another site — the guard that stands in front of every endpoint
/// this emulator serves, before any of this service's own code runs.
const FRONT_DOOR_REFUSAL: &str = "AccessDenied";

/// The handler every row runs.
fn handler() -> String {
    r#"
import json


def lambda_handler(event, context):
    method = event["requestContext"]["http"]["method"]
    if method == "OPTIONS":
        return {"statusCode": 418, "body": "the function answered this OPTIONS request"}
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json", "x-made": "by-the-handler"},
        "body": json.dumps({"marker": "ran", "origin": event["headers"].get("origin")}),
    }
"#
    .to_string()
}

/// `CreateFunctionUrlConfig` on `function` with the `Cors` settings `cors` names (absent for
/// `Value::Null`), granting the public the invocation that makes a `NONE` URL public.
async fn given_url(router: &MultiServiceRouter, function: &str, cors: &Value) -> Endpoint {
    let endpoint = given_configured_url(router, function, None, "NONE", cors).await;
    grant(
        router,
        function,
        None,
        &json!({
            "StatementId": "public-url",
            "Action": "lambda:InvokeFunctionUrl",
            "Principal": "*",
            "FunctionUrlAuthType": "NONE",
        }),
    )
    .await;
    endpoint
}

/// One row: a request to a function URL, and the whole answer the endpoint owes it.
struct Row<'r> {
    /// What this row is about.
    case: &'r str,
    /// Which of the three URLs it addresses.
    endpoint: &'r Endpoint,
    /// The request's method.
    method: &'r str,
    /// The headers the client sets on it.
    headers: &'r [(&'r str, &'r str)],
    /// The status the endpoint answers.
    status: u16,
    /// The headers the answer must carry, with these values.
    stated: &'r [(&'r str, &'r str)],
    /// The headers the answer must not carry at all.
    absent: &'r [&'r str],
    /// What the answer's body must name, where the row is about WHICH refusal it is.
    names: Option<&'r str>,
    /// Whether the FUNCTION produced the answer.
    ran: bool,
}

/// The whole rule, over every class of request a browser makes of a function URL.
#[tokio::test]
async fn a_function_urls_cors_settings_decide_its_preflight_and_decorate_its_answers() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_function(&router, "cors-configured", &handler()).await;
    given_function(&router, "cors-wildcard", &handler()).await;
    given_function(&router, "cors-absent", &handler()).await;

    // Every collection the model declares, so each one's reading is asserted.
    let configured = given_url(
        &router,
        "cors-configured",
        &json!({
            "AllowOrigins": [ADMITTED_ORIGIN, "https://second.example"],
            "AllowMethods": ["GET", "POST"],
            "AllowHeaders": ["x-asked"],
            "ExposeHeaders": ["x-made", "x-also"],
            "AllowCredentials": true,
            "MaxAge": 600,
        }),
    )
    .await;
    // The wildcard in both collections, and nothing else configured.
    let wildcard = given_url(
        &router,
        "cors-wildcard",
        &json!({ "AllowOrigins": [WILDCARD], "AllowMethods": [WILDCARD] }),
    )
    .await;
    // No CORS settings at all.
    let absent = given_url(&router, "cors-absent", &Value::Null).await;

    let origin = ("origin", ADMITTED_ORIGIN);
    let other_origin = ("origin", OTHER_ORIGIN);
    let asks_get = ("access-control-request-method", "GET");
    let asks_post = ("access-control-request-method", "POST");
    let asks_delete = ("access-control-request-method", "DELETE");
    // What a browser sets on a request it sends from ANOTHER site, and on one it sends from
    // the endpoint's own.
    let cross_site = ("sec-fetch-site", "cross-site");
    let same_origin = ("sec-fetch-site", "same-origin");
    let allow_origin = "access-control-allow-origin";
    let allow_methods = "access-control-allow-methods";
    let allow_headers = "access-control-allow-headers";
    let expose_headers = "access-control-expose-headers";
    let allow_credentials = "access-control-allow-credentials";
    let max_age = "access-control-max-age";

    let rows: &[Row<'_>] = &[
        Row {
            case: "a preflight from an admitted origin, asking about an admitted method",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[origin, asks_get, same_origin],
            status: 200,
            stated: &[
                (allow_origin, ADMITTED_ORIGIN),
                (allow_methods, "GET,POST"),
                (allow_headers, "x-asked"),
                (max_age, "600"),
                (allow_credentials, "true"),
            ],
            // The exposed headers answer the question an ACTUAL request's answer is about,
            // not the one a preflight asks.
            absent: &[expose_headers],
            names: None,
            ran: false,
        },
        Row {
            case: "a preflight asking about another admitted method",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[origin, asks_post, same_origin],
            status: 200,
            stated: &[(allow_origin, ADMITTED_ORIGIN), (allow_methods, "GET,POST")],
            absent: &[],
            names: None,
            ran: false,
        },
        Row {
            case: "a preflight from the second admitted origin",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[("origin", "https://second.example"), asks_get, same_origin],
            status: 200,
            stated: &[(allow_origin, "https://second.example")],
            absent: &[],
            names: None,
            ran: false,
        },
        Row {
            case: "a preflight asking about a method the configuration does not list",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[origin, asks_delete, same_origin],
            status: 403,
            stated: &[],
            absent: &[allow_origin, allow_methods],
            names: Some(ENDPOINT_REFUSAL),
            ran: false,
        },
        Row {
            case: "a preflight from an origin the configuration does not list",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[other_origin, asks_get, same_origin],
            status: 403,
            stated: &[],
            absent: &[allow_origin],
            names: Some(ENDPOINT_REFUSAL),
            ran: false,
        },
        Row {
            case: "an OPTIONS request that asks about no method, so is no preflight",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[origin, same_origin],
            status: HANDLER_OPTIONS_STATUS,
            stated: &[(allow_origin, ADMITTED_ORIGIN)],
            absent: &[allow_methods],
            names: None,
            ran: true,
        },
        Row {
            case: "an actual request from an admitted origin",
            endpoint: &configured,
            method: "GET",
            headers: &[origin, same_origin],
            status: 200,
            stated: &[
                (allow_origin, ADMITTED_ORIGIN),
                (expose_headers, "x-made,x-also"),
                (allow_credentials, "true"),
            ],
            // What a preflight is answered is not put on an actual request's answer.
            absent: &[allow_methods, allow_headers, max_age],
            names: None,
            ran: true,
        },
        Row {
            case: "an actual request from an origin the configuration does not admit",
            endpoint: &configured,
            method: "GET",
            headers: &[other_origin, same_origin],
            status: 200,
            stated: &[],
            absent: &[allow_origin, expose_headers, allow_credentials],
            names: None,
            ran: true,
        },
        Row {
            case: "an actual request carrying no origin at all",
            endpoint: &configured,
            method: "GET",
            headers: &[],
            status: 200,
            stated: &[],
            absent: &[allow_origin, expose_headers, allow_credentials],
            names: None,
            ran: true,
        },
        Row {
            case: "a preflight under the wildcard collections, from any origin",
            endpoint: &wildcard,
            method: "OPTIONS",
            headers: &[other_origin, asks_delete, same_origin],
            status: 200,
            stated: &[(allow_origin, WILDCARD), (allow_methods, WILDCARD)],
            // Neither was configured, so neither is stated.
            absent: &[max_age, allow_credentials],
            names: None,
            ran: false,
        },
        Row {
            case: "an actual request under the wildcard collections",
            endpoint: &wildcard,
            method: "GET",
            headers: &[other_origin, same_origin],
            status: 200,
            stated: &[(allow_origin, WILDCARD)],
            absent: &[expose_headers, allow_credentials],
            names: None,
            ran: true,
        },
        Row {
            case: "a preflight to a URL configured with no CORS at all",
            endpoint: &absent,
            method: "OPTIONS",
            headers: &[origin, asks_get, same_origin],
            status: HANDLER_OPTIONS_STATUS,
            stated: &[],
            absent: &[allow_origin, allow_methods, allow_headers, max_age],
            names: None,
            ran: true,
        },
        Row {
            case: "an actual request to a URL configured with no CORS at all",
            endpoint: &absent,
            method: "GET",
            headers: &[origin, same_origin],
            status: 200,
            stated: &[],
            absent: &[allow_origin, expose_headers, allow_credentials],
            names: None,
            ran: true,
        },
        // The emulator's OWN guard, in front of every endpoint it serves and pinned here so
        // this suite cannot be read as having moved it: a request whose browser-set headers
        // say a browser sent it from ANOTHER SITE is refused by the front door before the
        // endpoint sees it, whatever the stored CORS settings grant — so no page a developer
        // visits reads an answer from the emulator, and nothing of the configuration is
        // disclosed to it either.
        Row {
            case: "a preflight a browser sent from another site",
            endpoint: &configured,
            method: "OPTIONS",
            headers: &[origin, asks_get, cross_site],
            status: 403,
            stated: &[],
            absent: &[
                allow_origin,
                allow_methods,
                allow_headers,
                max_age,
                allow_credentials,
            ],
            names: Some(FRONT_DOOR_REFUSAL),
            ran: false,
        },
        Row {
            case: "an actual request a browser sent from another site",
            endpoint: &configured,
            method: "GET",
            headers: &[origin, cross_site],
            status: 403,
            stated: &[],
            absent: &[allow_origin, expose_headers, allow_credentials],
            names: Some(FRONT_DOOR_REFUSAL),
            ran: false,
        },
    ];

    for row in rows {
        let answer = send(
            &router,
            row.method,
            &row.endpoint.host,
            "/",
            &[],
            row.headers,
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected HTTP {}, got {}",
            row.case,
            row.status,
            answer.summary()
        );
        for (name, expected) in row.stated {
            let stated = answer.header(name);
            assert_eq!(
                stated.as_deref(),
                Some(*expected),
                "case {:?}: expected {name} to be {expected:?}, got {stated:?}",
                row.case
            );
        }
        for name in row.absent {
            let stated = answer.header(name);
            assert!(
                stated.is_none(),
                "case {:?}: expected no {name} on the answer, got {stated:?}",
                row.case
            );
        }
        if let Some(named) = row.names {
            assert!(
                answer.text().contains(named),
                "case {:?}: expected the answer to name {named:?}, got {}",
                row.case,
                answer.summary()
            );
        }
        let answered_by_the_function = answer.text().contains(MARKER)
            || answer.status == HANDLER_OPTIONS_STATUS;
        assert_eq!(
            answered_by_the_function, row.ran,
            "case {:?}: expected the function to have answered == {}, got {}",
            row.case,
            row.ran,
            answer.summary()
        );
    }
}
