// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the scheme of an api's default `ApiEndpoint`.
//!
//! The rule: an api's `ApiEndpoint` is the URI a caller hands to a client library to
//! reach that api, so its scheme names the protocol the api speaks — `wss` for a
//! WebSocket API (the RFC 6455 handshake scheme), `https` for an HTTP API — on the
//! `{api-id}.execute-api.{region}.amazonaws.com` host
//! (`CreateApiResponse$ApiEndpoint`), whichever operation reports the api.
//!
//! The table spans the whole `ProtocolType` enum (`HTTP | WEBSOCKET`, from
//! `smithy_query(query="shape", shape="ProtocolType")`) crossed with the regions a
//! request can be signed for, and every row is read back through CreateApi, GetApi
//! AND GetApis, so the scheme is asserted on each reporter rather than only on the
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
        let expected = format!(
            "{}://{api_id}.execute-api.{}.amazonaws.com",
            expected_scheme(case.protocol_type),
            case.region
        );

        assert_eq!(
            as_str(&created, "apiEndpoint"),
            expected,
            "case {:?}: CreateApi reported the wrong ApiEndpoint, got {created}",
            case.label
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
            expected,
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
            vec![expected.clone()],
            "case {:?}: GetApis listed the wrong ApiEndpoint, got {listed}",
            case.label
        );
    }
}

/// Two apis that differ ONLY in `ProtocolType` differ in their endpoint scheme and in
/// nothing else about the endpoint: the host is each api's own, so the scheme is not a
/// property of the store or of the region but of the api.
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
        assert_eq!(
            endpoint,
            format!(
                "{}://{api_id}.execute-api.{region}.amazonaws.com",
                expected_scheme(protocol_type)
            ),
            "case {protocol_type:?}: wrong ApiEndpoint, got {created}"
        );
        endpoints.push(endpoint);
    }

    assert_ne!(
        endpoints[0], endpoints[1],
        "two apis differing only in ProtocolType must not share one ApiEndpoint, got {endpoints:?}"
    );
}
