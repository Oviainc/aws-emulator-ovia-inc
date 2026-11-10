// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an api's default `ApiEndpoint`.
//!
//! The rule: an api's `ApiEndpoint` is the URI a caller hands to a client library to
//! reach that api, so its scheme names the protocol the api speaks — `wss` for a
//! WebSocket API (the RFC 6455 handshake scheme), `https` for an HTTP API
//! (`CreateApiResponse$ApiEndpoint`) — and its AUTHORITY is the address that api is
//! REACHED at: for an HTTP api created by a client that resolves AWS's own hosts, that
//! api's own `{api-id}.execute-api.{region}.amazonaws.com` host, which this emulator's
//! front door claims and serves, and for a WEBSOCKET api the `host:port` of
//! the socket the core opened for that api's data plane, which is the one address a
//! WebSocket client on this machine can complete a handshake against. Whichever operation
//! reports the api reports the same value.
//!
//! The other authority an HTTP api is announced on — the EMULATOR'S OWN endpoint, for a
//! client configured with it, which resolves no AWS host — is
//! `http_api_published_invoke_address`'s: every row here drives a client that addresses this
//! service's own endpoint host, which is the case the documented form applies to.
//!
//! The table spans the whole `ProtocolType` enum (`HTTP | WEBSOCKET`, from
//! `smithy_query(query="shape", shape="ProtocolType")`) crossed with the regions a
//! request can be signed for, and every row is read back through CreateApi, GetApi
//! AND GetApis, so the value is asserted on each reporter rather than only on the
//! one the recorded case used.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call_in_region, emulator};

/// The scheme an api of `protocol_type` must be reported on. The source is the
/// `ProtocolType` enum (`HTTP | WEBSOCKET`): a WebSocket API is opened with an RFC
/// 6455 handshake, whose secure URI scheme is `wss`; an HTTP API is invoked over
/// `https` (`Api$DisableExecuteApiEndpoint`: "clients can invoke your API with the
/// default https://{api_id}.execute-api.{region}.amazonaws.com endpoint").
pub(crate) fn expected_scheme(protocol_type: &str) -> &'static str {
    match protocol_type {
        "WEBSOCKET" => "wss",
        "HTTP" => "https",
        other => panic!("unmodelled ProtocolType {other:?}: the enum is HTTP | WEBSOCKET"),
    }
}

/// One row: the protocol type, the region the request is signed for, and the rest of
/// the CreateApi body that protocol type needs.
struct EndpointCase {
    label: &'static str,
    protocol_type: &'static str,
    region: &'static str,
    extra: Value,
}

/// Assert `announced` is the endpoint an api of `protocol_type` is reached at, for a case
/// whose control-plane request addressed one of AWS's own hosts; see the module docs for the
/// rule, and `http_api_published_invoke_address` for the form an api created through the
/// emulator's own endpoint is announced on.
///
/// `reporter` names the operation that reported it, so a row that disagrees says WHICH
/// reader disagreed.
pub(crate) fn assert_announced_endpoint(
    case: &str,
    reporter: &str,
    protocol_type: &str,
    api_id: &str,
    region: &str,
    announced: &str,
) {
    let scheme = expected_scheme(protocol_type);
    let Some(authority) = announced.strip_prefix(&format!("{scheme}://")) else {
        panic!(
            "case {case:?}: {reporter} must report a {scheme}:// endpoint, got {announced:?}"
        );
    };
    if protocol_type == "HTTP" {
        assert_eq!(
            authority,
            format!("{api_id}.execute-api.{region}.amazonaws.com"),
            "case {case:?}: {reporter} must report the api's own execute-api host, got \
             {announced:?}"
        );
        return;
    }
    // A WEBSOCKET api announces the socket the core opened for it: a `host:port` on the
    // interface the emulator is reached on — loopback unless the operator bound another —
    // with the port the core assigned, which is the one address a WebSocket client on this
    // machine can complete a handshake against.
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: {reporter} must report a host:port the \
                                  client dials, got {announced:?}"));
    let address: std::net::IpAddr = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .unwrap_or_else(|_| {
            panic!(
                "case {case:?}: {reporter} must report the address the socket is bound to, \
                 got {announced:?}"
            )
        });
    assert!(
        address.is_loopback(),
        "case {case:?}: {reporter} must report the interface the core binds, got {announced:?}"
    );
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {reporter} must report a port, got {announced:?}"));
    assert!(
        port > 0,
        "case {case:?}: {reporter} must report the port the socket landed on, got \
         {announced:?}"
    );
}

fn endpoint_cases() -> Vec<EndpointCase> {
    vec![
        EndpointCase {
            label: "websocket, required members only",
            protocol_type: "WEBSOCKET",
            region: "eu-west-1",
            // "This property is required for WebSocket APIs"
            // (`CreateApi$RouteSelectionExpression`).
            extra: json!({ "routeSelectionExpression": "$request.body.action" }),
        },
        EndpointCase {
            label: "websocket, another region",
            protocol_type: "WEBSOCKET",
            region: "ap-southeast-2",
            extra: json!({ "routeSelectionExpression": "$request.body.message" }),
        },
        EndpointCase {
            label: "websocket, default endpoint disabled",
            protocol_type: "WEBSOCKET",
            region: "us-west-2",
            extra: json!({
                "routeSelectionExpression": "$request.body.action",
                "disableExecuteApiEndpoint": true,
                "apiKeySelectionExpression": "$request.querystring.apiKey"
            }),
        },
        EndpointCase {
            label: "http, required members only",
            protocol_type: "HTTP",
            region: "eu-west-1",
            extra: json!({}),
        },
        EndpointCase {
            label: "http, another region and a quick-create target",
            protocol_type: "HTTP",
            region: "us-west-2",
            extra: json!({ "target": "https://example.com/upstream" }),
        },
        EndpointCase {
            label: "http, dualstack and an explicit route selection expression",
            protocol_type: "HTTP",
            region: "ap-southeast-2",
            extra: json!({
                "ipAddressType": "dualstack",
                "routeSelectionExpression": "$request.method $request.path"
            }),
        },
    ]
}

/// The `ApiEndpoint` CreateApi mints, GetApi reports and GetApis lists all carry the
/// scheme the api's `ProtocolType` names, on the api's own host in the request's region.
#[tokio::test]
async fn api_endpoint_scheme_follows_the_protocol_type_on_every_reporter() {
    for (index, case) in endpoint_cases().into_iter().enumerate() {
        // One emulator per row, so GetApis lists exactly this row's api.
        let emulator = emulator();

        let mut request = json!({
            "name": format!("endpoint-scheme-{index}"),
            "protocolType": case.protocol_type,
        });
        for (member, value) in case.extra.as_object().expect("extra is an object") {
            request[member] = value.clone();
        }

        let (status, created) =
            call_in_region(&emulator, case.region, "POST", "/v2/apis", Some(request)).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateApi, got {status} {created}",
            case.label
        );

        let api_id = as_str(&created, "apiId");
        let announced = as_str(&created, "apiEndpoint");
        assert_announced_endpoint(
            case.label,
            "CreateApi",
            case.protocol_type,
            &api_id,
            case.region,
            &announced,
        );

        let (status, fetched) =
            call_in_region(&emulator, case.region, "GET", &format!("/v2/apis/{api_id}"), None)
                .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApi, got {status} {fetched}",
            case.label
        );
        assert_eq!(
            as_str(&fetched, "apiEndpoint"),
            announced,
            "case {:?}: GetApi reported the wrong ApiEndpoint, got {fetched}",
            case.label
        );

        let (status, listed) =
            call_in_region(&emulator, case.region, "GET", "/v2/apis", None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApis, got {status} {listed}",
            case.label
        );
        let items = listed["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: GetApis served no items, got {listed}", case.label));
        let endpoints: Vec<String> = items
            .iter()
            .map(|item| as_str(item, "apiEndpoint"))
            .collect();
        assert_eq!(
            endpoints,
            vec![announced.clone()],
            "case {:?}: GetApis listed the wrong ApiEndpoint, got {listed}",
            case.label
        );
    }
}

/// Two apis that differ ONLY in `ProtocolType` differ in their endpoint: each is reported
/// at the address that api is reached at, so the endpoint is not a property of the store or
/// of the region but of the api.
#[tokio::test]
async fn the_scheme_is_the_only_difference_between_the_two_protocol_types() {
    let emulator = emulator();
    let region = "eu-central-1";

    let mut endpoints = Vec::new();
    for (protocol_type, route_selection) in [
        // The expression each protocol type documents for itself; it is the only member
        // that has to differ with the protocol type.
        ("HTTP", "$request.method $request.path"),
        ("WEBSOCKET", "$request.body.action"),
    ] {
        let (status, created) = call_in_region(
            &emulator,
            region,
            "POST",
            "/v2/apis",
            Some(json!({
                "name": "same-but-for-the-protocol",
                "protocolType": protocol_type,
                "routeSelectionExpression": route_selection,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {protocol_type:?}: expected 201 from CreateApi, got {status} {created}"
        );
        let api_id = as_str(&created, "apiId");
        let endpoint = as_str(&created, "apiEndpoint");
        assert_announced_endpoint(
            protocol_type,
            "CreateApi",
            protocol_type,
            &api_id,
            region,
            &endpoint,
        );
        endpoints.push(endpoint);
    }

    assert_ne!(
        endpoints[0], endpoints[1],
        "two apis differing only in ProtocolType must not share one ApiEndpoint, got {endpoints:?}"
    );
}
