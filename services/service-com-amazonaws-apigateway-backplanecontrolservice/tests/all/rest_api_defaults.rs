// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `RestApi` members the service fills in itself:
//! `apiKeySource`, `endpointConfiguration` (its `types` and its `ipAddressType`),
//! `securityPolicy` and the `tags` collection.
//!
//! The rule under test: a created API carries a value for every member the service
//! defaults, the request's own value where it set one, and every operation that
//! serializes a `RestApi` — `CreateRestApi`, `GetRestApi` and the `GetRestApis` list
//! element — reports the SAME stored values.
//!
//! The rows of [`default_cases`] differ along what the rule generalises over: which of
//! those members the request sets, the endpoint type (which decides the defaulted IP
//! address type), the number of endpoint types and VPC endpoint ids, and whether the
//! API is tagged. The enum catalogs are the crate's own model-derived constants
//! (`ops::types::*_VALUES`, generated from the Smithy enums), so they cannot drift from
//! the model, and they include values the recorded case never used.
//!
//! The emulator, the request builder and the page helper are this crate's existing wire
//! harness, reused from `rest_api_lifecycle`.

use serde_json::{json, Map, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::{
    API_KEY_SOURCE_TYPE_VALUES, ENDPOINT_TYPE_VALUES, IP_ADDRESS_TYPE_VALUES,
    SECURITY_POLICY_VALUES,
};
// The endpoint type whose only supported IP address type is `dualstack`, named by the
// crate's own constant so a second definition cannot drift from it. The VALUES this
// suite expects stay literal below, so no assertion can be satisfied by the
// implementation's own constants.
use service_com_amazonaws_apigateway_backplanecontrolservice::state::PRIVATE_ENDPOINT_TYPE;

use crate::rest_api_lifecycle::{page_items, string_member, Answer, Gateway};

/// What a created API must report for the defaulted members, for one request shape.
struct Expectation {
    /// Names the row in every assertion.
    case: &'static str,
    /// The `CreateRestApi` body, `name` included.
    request: Value,
    api_key_source: &'static str,
    endpoint_types: Vec<&'static str>,
    ip_address_type: &'static str,
    vpc_endpoint_ids: Option<Vec<&'static str>>,
    security_policy: &'static str,
    /// The tag pairs the API must report; empty for an untagged API, which must still
    /// report a (then empty) `tags` map.
    tags: Vec<(&'static str, &'static str)>,
}

/// The request shapes the rule has to hold over. Every row omits at least one defaulted
/// member and sets at least one other, except the first (which sets none) and the last
/// (which sets them all).
fn default_cases() -> Vec<Expectation> {
    vec![
        Expectation {
            case: "nothing but a name",
            request: json!({ "name": "defaults-bare" }),
            api_key_source: "HEADER",
            endpoint_types: vec!["EDGE"],
            ip_address_type: "ipv4",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "endpoint types only",
            request: json!({
                "name": "defaults-regional",
                "endpointConfiguration": { "types": ["REGIONAL"] },
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["REGIONAL"],
            ip_address_type: "ipv4",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "a private API takes the only address type it supports",
            request: json!({
                "name": "defaults-private",
                "endpointConfiguration": { "types": ["PRIVATE"] },
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["PRIVATE"],
            ip_address_type: "dualstack",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "an empty endpoint types list",
            request: json!({
                "name": "defaults-empty-types",
                "endpointConfiguration": { "types": [] },
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["EDGE"],
            ip_address_type: "ipv4",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "an address type without endpoint types",
            request: json!({
                "name": "defaults-address-only",
                "endpointConfiguration": { "ipAddressType": "dualstack" },
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["EDGE"],
            ip_address_type: "dualstack",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "vpc endpoint ids kept beside the defaulted address type",
            request: json!({
                "name": "defaults-vpce",
                "endpointConfiguration": {
                    "types": ["PRIVATE"],
                    "vpcEndpointIds": ["vpce-0a1b2c3d", "vpce-4e5f6a7b"],
                },
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["PRIVATE"],
            ip_address_type: "dualstack",
            vpc_endpoint_ids: Some(vec!["vpce-0a1b2c3d", "vpce-4e5f6a7b"]),
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "an api key source the request chose",
            request: json!({
                "name": "defaults-authorizer",
                "apiKeySource": "AUTHORIZER",
            }),
            api_key_source: "AUTHORIZER",
            endpoint_types: vec!["EDGE"],
            ip_address_type: "ipv4",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![],
        },
        Expectation {
            case: "a security policy the request chose",
            request: json!({
                "name": "defaults-tls13",
                "securityPolicy": "SecurityPolicy_TLS13_1_2_2021_06",
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["EDGE"],
            ip_address_type: "ipv4",
            vpc_endpoint_ids: None,
            security_policy: "SecurityPolicy_TLS13_1_2_2021_06",
            tags: vec![],
        },
        Expectation {
            case: "a tagged API",
            request: json!({
                "name": "defaults-tagged",
                "tags": { "team": "api", "env": "test" },
            }),
            api_key_source: "HEADER",
            endpoint_types: vec!["EDGE"],
            ip_address_type: "ipv4",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_0",
            tags: vec![("env", "test"), ("team", "api")],
        },
        Expectation {
            case: "every defaulted member set by the request",
            request: json!({
                "name": "defaults-none-needed",
                "apiKeySource": "AUTHORIZER",
                "securityPolicy": "TLS_1_2",
                "endpointConfiguration": {
                    "types": ["REGIONAL", "EDGE"],
                    "ipAddressType": "dualstack",
                },
                "tags": { "owner": "platform" },
            }),
            api_key_source: "AUTHORIZER",
            endpoint_types: vec!["REGIONAL", "EDGE"],
            ip_address_type: "dualstack",
            vpc_endpoint_ids: None,
            security_policy: "TLS_1_2",
            tags: vec![("owner", "platform")],
        },
    ]
}

/// `CreateRestApi` with `request`, asserting the modelled 201 and returning the body.
async fn create(gateway: &Gateway, case: &str, request: Value) -> Value {
    let answer = gateway.call("POST", "/restapis", Some(request)).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateRestApi, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// `GetRestApi` for `id`, asserting the modelled 200 and returning the body.
async fn get(gateway: &Gateway, case: &str, id: &str) -> Value {
    let answer: Answer = gateway.call("GET", &format!("/restapis/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetRestApi, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// The `GetRestApis` element for `id`, asserting it is listed exactly once.
async fn listed(gateway: &Gateway, case: &str, id: &str) -> Value {
    let answer = gateway.call("GET", "/restapis?limit=500", None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetRestApis, got {} with {:?}",
        answer.status, answer.body
    );
    let mut matched: Vec<Value> = page_items(&answer.body, case)
        .into_iter()
        .filter(|item| item["id"].as_str() == Some(id))
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "case {case}: expected the API {id:?} listed exactly once, got {matched:?}"
    );
    matched.remove(0)
}

/// The string members a `RestApi` body must carry, as `(member, expected)` pairs.
fn assert_members(body: &Value, expectation: &Expectation, reported_by: &str) {
    let case = expectation.case;
    for (member, expected) in [
        ("apiKeySource", expectation.api_key_source),
        ("securityPolicy", expectation.security_policy),
    ] {
        assert_eq!(
            body[member].as_str(),
            Some(expected),
            "case {case} ({reported_by}): expected {member} {expected:?}, got {:?}",
            body[member]
        );
    }

    let configuration = &body["endpointConfiguration"];
    assert!(
        configuration.is_object(),
        "case {case} ({reported_by}): expected an endpointConfiguration object, got {configuration:?}"
    );
    let types: Vec<&str> = configuration["types"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case} ({reported_by}): expected endpointConfiguration.types, got {configuration:?}"
            )
        })
        .iter()
        .map(|value| value.as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        types, expectation.endpoint_types,
        "case {case} ({reported_by}): expected endpointConfiguration.types {:?}, got {types:?}",
        expectation.endpoint_types
    );
    assert_eq!(
        configuration["ipAddressType"].as_str(),
        Some(expectation.ip_address_type),
        "case {case} ({reported_by}): expected endpointConfiguration.ipAddressType {:?}, got {:?}",
        expectation.ip_address_type,
        configuration["ipAddressType"]
    );
    match &expectation.vpc_endpoint_ids {
        None => assert!(
            configuration["vpcEndpointIds"].is_null(),
            "case {case} ({reported_by}): expected no vpcEndpointIds, got {:?}",
            configuration["vpcEndpointIds"]
        ),
        Some(expected) => {
            let ids: Vec<&str> = configuration["vpcEndpointIds"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case} ({reported_by}): expected vpcEndpointIds, got {configuration:?}"
                    )
                })
                .iter()
                .map(|value| value.as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                &ids, expected,
                "case {case} ({reported_by}): expected vpcEndpointIds {expected:?}, got {ids:?}"
            );
        }
    }

    let expected_tags: Map<String, Value> = expectation
        .tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
        .collect();
    assert_eq!(
        body["tags"].as_object(),
        Some(&expected_tags),
        "case {case} ({reported_by}): expected tags {expected_tags:?}, got {:?}",
        body["tags"]
    );
}

#[tokio::test]
async fn a_created_rest_api_carries_the_service_defaulted_members_on_every_read_path() {
    let gateway = Gateway::new();
    for expectation in default_cases() {
        let created = create(&gateway, expectation.case, expectation.request.clone()).await;
        assert_members(&created, &expectation, "CreateRestApi");

        let id = string_member(&created, "id", expectation.case);
        let fetched = get(&gateway, expectation.case, &id).await;
        assert_members(&fetched, &expectation, "GetRestApi");

        let element = listed(&gateway, expectation.case, &id).await;
        assert_members(&element, &expectation, "GetRestApis");
    }
}

#[tokio::test]
async fn every_endpoint_type_keeps_its_types_and_takes_the_address_type_it_supports() {
    let gateway = Gateway::new();
    // The whole EndpointType catalog, PRIVATE — which supports only dualstack —
    // included; the recorded case used EDGE alone.
    for endpoint_type in ENDPOINT_TYPE_VALUES {
        let case = format!("endpointConfiguration.types [{endpoint_type}]");
        let created = create(
            &gateway,
            &case,
            json!({
                "name": format!("endpoint-{}", endpoint_type.to_lowercase()),
                "endpointConfiguration": { "types": [endpoint_type] },
            }),
        )
        .await;
        let expected_address_type = if *endpoint_type == PRIVATE_ENDPOINT_TYPE {
            "dualstack"
        } else {
            "ipv4"
        };
        let configuration = &created["endpointConfiguration"];
        assert_eq!(
            configuration["types"].as_array().map(Vec::len),
            Some(1),
            "case {case}: expected exactly the requested endpoint type, got {configuration:?}"
        );
        assert_eq!(
            configuration["types"][0].as_str(),
            Some(*endpoint_type),
            "case {case}: expected the endpoint type kept as given, got {configuration:?}"
        );
        assert_eq!(
            configuration["ipAddressType"].as_str(),
            Some(expected_address_type),
            "case {case}: expected ipAddressType {expected_address_type:?}, got {:?}",
            configuration["ipAddressType"]
        );

        // And the stored record, not the request, is what a read reports.
        let id = string_member(&created, "id", &case);
        let fetched = get(&gateway, &case, &id).await;
        assert_eq!(
            fetched["endpointConfiguration"], created["endpointConfiguration"],
            "case {case}: GetRestApi must report the stored configuration {:?}, got {:?}",
            created["endpointConfiguration"], fetched["endpointConfiguration"]
        );
    }
}

#[tokio::test]
async fn every_modelled_enum_value_a_request_sets_is_stored_and_reported_unchanged() {
    let gateway = Gateway::new();
    for (index, source) in API_KEY_SOURCE_TYPE_VALUES.iter().enumerate() {
        let case = format!("apiKeySource {source}");
        let created = create(
            &gateway,
            &case,
            json!({ "name": format!("keysource-{index}"), "apiKeySource": source }),
        )
        .await;
        let id = string_member(&created, "id", &case);
        let fetched = get(&gateway, &case, &id).await;
        assert_eq!(
            fetched["apiKeySource"].as_str(),
            Some(*source),
            "case {case}: expected the stored apiKeySource {source:?}, got {:?}",
            fetched["apiKeySource"]
        );
    }
    for (index, policy) in SECURITY_POLICY_VALUES.iter().enumerate() {
        let case = format!("securityPolicy {policy}");
        let created = create(
            &gateway,
            &case,
            json!({ "name": format!("policy-{index}"), "securityPolicy": policy }),
        )
        .await;
        let id = string_member(&created, "id", &case);
        let fetched = get(&gateway, &case, &id).await;
        assert_eq!(
            fetched["securityPolicy"].as_str(),
            Some(*policy),
            "case {case}: expected the stored securityPolicy {policy:?}, got {:?}",
            fetched["securityPolicy"]
        );
    }
    for (index, address_type) in IP_ADDRESS_TYPE_VALUES.iter().enumerate() {
        let case = format!("ipAddressType {address_type}");
        let created = create(
            &gateway,
            &case,
            json!({
                "name": format!("address-{index}"),
                "endpointConfiguration": { "ipAddressType": address_type },
            }),
        )
        .await;
        let id = string_member(&created, "id", &case);
        let fetched = get(&gateway, &case, &id).await;
        assert_eq!(
            fetched["endpointConfiguration"]["ipAddressType"].as_str(),
            Some(*address_type),
            "case {case}: expected the stored ipAddressType {address_type:?}, got {:?}",
            fetched["endpointConfiguration"]["ipAddressType"]
        );
    }
}
