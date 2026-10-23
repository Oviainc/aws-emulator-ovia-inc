// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Authorizer` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateAuthorizer` persists an authorizer under the api that owns it and answers
//!   201 with the `AuthorizerId` it minted**, reporting the configuration the record
//!   holds; `GetAuthorizer` reports that same record, member for member. Nothing is
//!   invented for a member the request left out.
//! * **A configuration its own `AuthorizerType` — or its api's `ProtocolType` — does not
//!   admit is refused 400 `BadRequestException`**, and nothing is persisted:
//!   `AuthorizerType` `JWT` is "supported only for HTTP APIs"
//!   (`CreateAuthorizer$AuthorizerType`), `JwtConfiguration` is "Required for the JWT
//!   authorizer type" and its `Issuer` is the identity provider that issues the tokens,
//!   `AuthorizerUri` is the Lambda a `REQUEST` authorizer invokes, and
//!   `AuthorizerPayloadFormatVersion` is "Required for HTTP API Lambda authorizers"
//!   with "Supported values … 1.0 and 2.0".
//! * **`AuthorizerResultTtlInSeconds` takes a value in the modelled range 0..3600**:
//!   both published bounds are accepted and a value past either is refused 400 by the
//!   boundary validator, before the handler runs.
//! * **An api holds at most `MAX_AUTHORIZERS_PER_API` authorizers**, after which a
//!   create is refused 429 `TooManyRequestsException`; the bound is per api.
//! * **An authorizer belongs to the ONE api that owns it**: an api that does not exist
//!   holds none to create or to read, and an `AuthorizerId` of another api is not found.
//!
//! The rows vary what each rule generalises over: the authorizer type (`JWT`,
//! `REQUEST`), the owning api's protocol (`HTTP`, `WEBSOCKET`), which optional members
//! the request carries, the number of identity sources (none, one, two), the payload
//! format version, and the TTL at each published bound.
//!
//! One row is deliberately NOT refused: a `REQUEST` authorizer that sets
//! `EnableSimpleResponses` under payload format version 1.0. That combination is a
//! configuration the service stores; it is the authorizer's INVOCATION that fails, which
//! belongs to the authorizer invoke path rather than to this create.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// A well-formed Lambda authorizer URI, the form `AuthorizerUri` documents: "For REQUEST
/// authorizers, this must be a well-formed Lambda function URI, for example,
/// arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function-arn}/invocations".
fn lambda_uri(function: &str) -> String {
    format!(
        "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
         arn:aws:lambda:us-east-1:123456789012:function:{function}/invocations"
    )
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

/// One accepted-create row: the owning api's protocol, the request, and the members the
/// stored authorizer must report.
struct Row {
    case: &'static str,
    protocol: &'static str,
    request: Value,
    expected: Vec<(&'static str, Value)>,
}

fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a JWT authorizer with one audience",
            protocol: "HTTP",
            request: json!({
                "name": "jwt-one-audience",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": {
                    "issuer": "https://example-issuer.test/oauth2",
                    "audience": ["first-audience"]
                }
            }),
            expected: vec![
                ("name", json!("jwt-one-audience")),
                ("authorizerType", json!("JWT")),
                ("identitySource", json!(["$request.header.Authorization"])),
                (
                    "jwtConfiguration",
                    json!({
                        "issuer": "https://example-issuer.test/oauth2",
                        "audience": ["first-audience"]
                    }),
                ),
            ],
        },
        Row {
            case: "a JWT authorizer with three audiences and a non-standard header",
            protocol: "HTTP",
            request: json!({
                "name": "jwt-many-audiences",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.X-Access-Token"],
                "jwtConfiguration": {
                    "issuer": "https://other-issuer.test/",
                    "audience": ["one", "two", "three"]
                }
            }),
            expected: vec![
                ("identitySource", json!(["$request.header.X-Access-Token"])),
                (
                    "jwtConfiguration",
                    json!({
                        "issuer": "https://other-issuer.test/",
                        "audience": ["one", "two", "three"]
                    }),
                ),
            ],
        },
        Row {
            case: "a JWT authorizer whose configuration names no audience",
            protocol: "HTTP",
            request: json!({
                "name": "jwt-no-audience",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": { "issuer": "https://issuer-only.test/" }
            }),
            expected: vec![(
                "jwtConfiguration",
                json!({ "issuer": "https://issuer-only.test/" }),
            )],
        },
        Row {
            case: "a REQUEST authorizer under payload format 2.0 with caching disabled",
            protocol: "HTTP",
            request: json!({
                "name": "request-simple",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("simple-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "enableSimpleResponses": true,
                // "If it equals 0, authorization caching is disabled" — the lower
                // published bound of the modelled range.
                "authorizerResultTtlInSeconds": 0,
                "identitySource": ["$request.header.Authorization"]
            }),
            expected: vec![
                ("authorizerType", json!("REQUEST")),
                ("authorizerUri", json!(lambda_uri("simple-auth"))),
                ("authorizerPayloadFormatVersion", json!("2.0")),
                ("enableSimpleResponses", json!(true)),
                ("authorizerResultTtlInSeconds", json!(0)),
            ],
        },
        Row {
            case: "a REQUEST authorizer under payload format 1.0 at the maximum TTL",
            protocol: "HTTP",
            request: json!({
                "name": "request-iam-policy",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("policy-auth"),
                "authorizerPayloadFormatVersion": "1.0",
                // "The maximum value is 3600, or 1 hour" — the upper published bound.
                "authorizerResultTtlInSeconds": 3600,
                "authorizerCredentialsArn": "arn:aws:iam::123456789012:role/apigw-invoke",
                "identityValidationExpression": "^Bearer .*$",
                "identitySource": ["$request.header.Authorization"]
            }),
            expected: vec![
                ("authorizerPayloadFormatVersion", json!("1.0")),
                ("authorizerResultTtlInSeconds", json!(3600)),
                (
                    "authorizerCredentialsArn",
                    json!("arn:aws:iam::123456789012:role/apigw-invoke"),
                ),
                ("identityValidationExpression", json!("^Bearer .*$")),
            ],
        },
        Row {
            case: "a REQUEST authorizer keyed on two identity sources",
            protocol: "HTTP",
            request: json!({
                "name": "request-two-sources",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("two-source-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "authorizerResultTtlInSeconds": 300,
                "identitySource": [
                    "$request.header.Authorization",
                    "$request.querystring.tenant"
                ]
            }),
            expected: vec![
                (
                    "identitySource",
                    json!([
                        "$request.header.Authorization",
                        "$request.querystring.tenant"
                    ]),
                ),
                ("authorizerResultTtlInSeconds", json!(300)),
            ],
        },
        Row {
            // NOT refused: the combination is stored, and it is the INVOCATION of such an
            // authorizer that fails — not its creation.
            case: "a REQUEST authorizer asking for simple responses under payload format 1.0",
            protocol: "HTTP",
            request: json!({
                "name": "request-simple-under-1-0",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("mismatched-auth"),
                "authorizerPayloadFormatVersion": "1.0",
                "enableSimpleResponses": true,
                "identitySource": ["$request.header.Authorization"]
            }),
            expected: vec![
                ("authorizerPayloadFormatVersion", json!("1.0")),
                ("enableSimpleResponses", json!(true)),
            ],
        },
        Row {
            // `AuthorizerPayloadFormatVersion` is required only for HTTP API Lambda
            // authorizers, so a WebSocket api's REQUEST authorizer carries none.
            case: "a WebSocket REQUEST authorizer with no payload format version",
            protocol: "WEBSOCKET",
            request: json!({
                "name": "websocket-token-gate",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("connect-auth"),
                "identitySource": ["route.request.header.Auth"]
            }),
            expected: vec![
                ("authorizerType", json!("REQUEST")),
                ("identitySource", json!(["route.request.header.Auth"])),
            ],
        },
        Row {
            case: "a WebSocket REQUEST authorizer with an empty identity source list",
            protocol: "WEBSOCKET",
            request: json!({
                "name": "websocket-no-identity-source",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("open-auth"),
                "identitySource": []
            }),
            // The member is required on the request, so the resource always reports the
            // list it holds — empty here rather than absent.
            expected: vec![("identitySource", json!([]))],
        },
        Row {
            case: "a REQUEST authorizer at the smallest TTL that enables caching",
            protocol: "HTTP",
            request: json!({
                "name": "request-ttl-one",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("ttl-one-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "authorizerResultTtlInSeconds": 1,
                "identitySource": ["$request.header.Authorization"]
            }),
            expected: vec![("authorizerResultTtlInSeconds", json!(1))],
        },
    ]
}

/// Every optional member of the `Authorizer` shape, so a row can assert that the one it
/// did not request is reported by neither reader.
const OPTIONAL_MEMBERS: &[&str] = &[
    "authorizerCredentialsArn",
    "authorizerPayloadFormatVersion",
    "authorizerResultTtlInSeconds",
    "authorizerUri",
    "enableSimpleResponses",
    "identityValidationExpression",
    "jwtConfiguration",
];

/// The two `AuthorizerType` names this suite and the service's validator spell out are
/// exactly the generated catalog of the modelled enum, so the hand-written constants in
/// `state.rs` cannot drift from the model's own list.
#[test]
fn the_authorizer_type_names_are_the_modelled_catalog() {
    use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::AUTHORIZER_TYPE_VALUES;
    use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
        AUTHORIZER_TYPE_JWT, AUTHORIZER_TYPE_REQUEST,
    };

    let mut modelled: Vec<&str> = AUTHORIZER_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    let mut used = vec![AUTHORIZER_TYPE_JWT, AUTHORIZER_TYPE_REQUEST];
    used.sort_unstable();
    assert_eq!(
        used, modelled,
        "expected the service's AuthorizerType constants to be exactly the modelled \
         catalog, got {used:?} against {modelled:?}"
    );
}

#[tokio::test]
async fn create_authorizer_persists_the_record_and_get_authorizer_reports_it_back() {
    let emulator = emulator();

    for (index, row) in accepted_rows().into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            row.case,
            &format!("authorizer-api-{index}"),
            row.protocol,
        )
        .await;

        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateAuthorizer, got {status} {created}",
            row.case
        );
        let authorizer_id = as_str(&created, "authorizerId");
        assert_minted_id(row.case, "AuthorizerId", &authorizer_id);

        // The same record, read back through the api that owns it.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetAuthorizer, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            created, fetched,
            "case {:?}: expected GetAuthorizer to report the record CreateAuthorizer \
             persisted, got {fetched} after {created}",
            row.case
        );

        for (member, value) in &row.expected {
            assert_eq!(
                created.get(member),
                Some(value),
                "case {:?}: expected CreateAuthorizer to report {member}={value}, got {created}",
                row.case
            );
        }

        // Nothing is invented for a member the request left out.
        for member in OPTIONAL_MEMBERS {
            if row.request.get(member).is_none() {
                assert!(
                    fetched.get(member).is_none_or(Value::is_null),
                    "case {:?}: expected GetAuthorizer to report no {member}, got {fetched}",
                    row.case
                );
            }
        }
    }

    // Several authorizers under one api each carry their own minted id.
    let api_id = create_api(&emulator, "the shared api", "authorizer-shared-api", "HTTP").await;
    let mut minted = Vec::new();
    for index in 0..4 {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(json!({
                "name": format!("shared-{index}"),
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("shared-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "authorizer {index}: expected 201 from CreateAuthorizer, got {status} {created}"
        );
        minted.push(as_str(&created, "authorizerId"));
    }
    for (index, id) in minted.iter().enumerate() {
        assert_eq!(
            minted.iter().filter(|other| *other == id).count(),
            1,
            "authorizer {index}: expected distinct ids within one api, got {minted:?}"
        );
    }
}

/// One refused-create row: the owning api's protocol and the request.
struct Refusal {
    case: &'static str,
    protocol: &'static str,
    request: Value,
}

fn config_refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            // "Specify JWT to use JSON Web Tokens (supported only for HTTP APIs)".
            case: "a JWT authorizer on a WebSocket api",
            protocol: "WEBSOCKET",
            request: json!({
                "name": "jwt-on-websocket",
                "authorizerType": "JWT",
                "identitySource": ["route.request.header.Authorization"],
                "jwtConfiguration": { "issuer": "https://example-issuer.test/" }
            }),
        },
        Refusal {
            // "Required for the JWT authorizer type."
            case: "a JWT authorizer carrying no JwtConfiguration",
            protocol: "HTTP",
            request: json!({
                "name": "jwt-no-configuration",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        Refusal {
            case: "a JWT authorizer whose JwtConfiguration names no issuer",
            protocol: "HTTP",
            request: json!({
                "name": "jwt-no-issuer",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": { "audience": ["an-audience"] }
            }),
        },
        Refusal {
            case: "a JWT authorizer whose issuer is empty",
            protocol: "HTTP",
            request: json!({
                "name": "jwt-empty-issuer",
                "authorizerType": "JWT",
                "identitySource": ["$request.header.Authorization"],
                "jwtConfiguration": { "issuer": "" }
            }),
        },
        Refusal {
            // The Lambda a REQUEST authorizer invokes.
            case: "a REQUEST authorizer carrying no AuthorizerUri",
            protocol: "HTTP",
            request: json!({
                "name": "request-no-uri",
                "authorizerType": "REQUEST",
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        Refusal {
            case: "a WebSocket REQUEST authorizer carrying no AuthorizerUri",
            protocol: "WEBSOCKET",
            request: json!({
                "name": "websocket-request-no-uri",
                "authorizerType": "REQUEST",
                "identitySource": ["route.request.header.Auth"]
            }),
        },
        Refusal {
            // "Required for HTTP API Lambda authorizers."
            case: "an HTTP REQUEST authorizer carrying no payload format version",
            protocol: "HTTP",
            request: json!({
                "name": "request-no-payload-format",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("no-format-auth"),
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        Refusal {
            // "Supported values are 1.0 and 2.0."
            case: "a payload format version past the supported ones",
            protocol: "HTTP",
            request: json!({
                "name": "request-format-3-0",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("format-auth"),
                "authorizerPayloadFormatVersion": "3.0",
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        Refusal {
            case: "a payload format version written without its minor part",
            protocol: "HTTP",
            request: json!({
                "name": "request-format-2",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("format-auth"),
                "authorizerPayloadFormatVersion": "2",
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        Refusal {
            case: "a payload format version carrying a version prefix",
            protocol: "HTTP",
            request: json!({
                "name": "request-format-v2-0",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("format-auth"),
                "authorizerPayloadFormatVersion": "v2.0",
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        Refusal {
            case: "a payload format version supported on a WebSocket api's authorizer either",
            protocol: "WEBSOCKET",
            request: json!({
                "name": "websocket-format-1-1",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("format-auth"),
                "authorizerPayloadFormatVersion": "1.1",
                "identitySource": ["route.request.header.Auth"]
            }),
        },
    ]
}

#[tokio::test]
async fn a_configuration_its_type_or_protocol_does_not_admit_is_refused() {
    let emulator = emulator();

    for (index, refusal) in config_refusals().into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            refusal.case,
            &format!("authorizer-refusal-api-{index}"),
            refusal.protocol,
        )
        .await;
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(refusal.request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 from CreateAuthorizer, got {status} {body}",
            refusal.case
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {:?}: expected BadRequestException, got {body}",
            refusal.case
        );
    }

    // Every create above was refused under its own api. Repeating them all against ONE
    // api persists nothing and mints nothing there: the first authorizer that api
    // accepts carries the FIRST id of its own id bijection, which is the id a fresh
    // api's first authorizer carries too.
    let shared = create_api(&emulator, "the shared api", "authorizer-refusal-shared", "HTTP").await;
    for refusal in config_refusals() {
        if refusal.protocol != "HTTP" {
            continue;
        }
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{shared}/authorizers"),
            Some(refusal.request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 from CreateAuthorizer, got {status} {body}",
            refusal.case
        );
    }
    let accepted = json!({
        "name": "after-the-refusals",
        "authorizerType": "REQUEST",
        "authorizerUri": lambda_uri("after-auth"),
        "authorizerPayloadFormatVersion": "2.0",
        "identitySource": ["$request.header.Authorization"]
    });
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{shared}/authorizers"),
        Some(accepted.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for a well-formed authorizer after the refusals, got {status} {created}"
    );
    let after_refusals = as_str(&created, "authorizerId");

    let fresh = create_api(&emulator, "a fresh api", "authorizer-refusal-fresh", "HTTP").await;
    let (status, first) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{fresh}/authorizers"),
        Some(accepted),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for the fresh api's first authorizer, got {status} {first}"
    );
    assert_eq!(
        after_refusals,
        as_str(&first, "authorizerId"),
        "expected the refused creates to have minted nothing, so the api's first accepted \
         authorizer carries the same id a fresh api's first authorizer does, got \
         {after_refusals:?} and {first}"
    );
}

#[tokio::test]
async fn the_result_ttl_is_accepted_at_each_published_bound_and_refused_past_it() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the ttl api", "authorizer-ttl-api", "HTTP").await;

    // The modelled range of `AuthorizerResultTtlInSeconds` is 0..3600
    // (`smithy_query(query="operation", operation="CreateAuthorizer")`), so both bounds
    // are accepted and round-trip.
    for (case, ttl) in [
        ("the minimum, which disables caching", 0),
        ("one second, the smallest TTL that caches", 1),
        ("the documented default of other API Gateway authorizers", 300),
        ("the maximum, one hour", 3600),
    ] {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(json!({
                "name": format!("ttl-{ttl}"),
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("ttl-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "authorizerResultTtlInSeconds": ttl,
                "identitySource": ["$request.header.Authorization"]
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAuthorizer, got {status} {created}"
        );
        let authorizer_id = as_str(&created, "authorizerId");
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAuthorizer, got {status} {fetched}"
        );
        assert_eq!(
            fetched.get("authorizerResultTtlInSeconds"),
            Some(&json!(ttl)),
            "case {case:?}: expected the stored TTL {ttl}, got {fetched}"
        );
    }

    // One past either bound is refused by the boundary validator, before the handler
    // runs, so no authorizer is stored for it.
    for (case, ttl) in [
        ("one below the minimum", -1),
        ("one above the maximum", 3601),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(json!({
                "name": "ttl-out-of-range",
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("ttl-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "authorizerResultTtlInSeconds": ttl,
                "identitySource": ["$request.header.Authorization"]
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a TTL of {ttl}, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_api_holds_at_most_the_authorizers_its_quota_allows() {
    // The per-api bound the service enforces, read from the service itself rather than
    // copied: the create past it is refused with the modelled `TooManyRequestsException`
    // ("a limit has been exceeded").
    const LIMIT: usize =
        service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_AUTHORIZERS_PER_API;

    let emulator = emulator();
    let api_id = create_api(&emulator, "the quota api", "authorizer-quota-api", "HTTP").await;

    for index in 0..LIMIT {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/authorizers"),
            Some(json!({
                "name": format!("quota-{index}"),
                "authorizerType": "REQUEST",
                "authorizerUri": lambda_uri("quota-auth"),
                "authorizerPayloadFormatVersion": "2.0",
                "identitySource": ["$request.header.Authorization"]
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "authorizer {index} of {LIMIT}: expected 201 from CreateAuthorizer, \
             got {status} {created}"
        );
    }

    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "one-too-many",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("quota-auth"),
            "authorizerPayloadFormatVersion": "2.0",
            "identitySource": ["$request.header.Authorization"]
        })),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 from the authorizer past the limit of {LIMIT}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );

    // Another api is bounded on its own: the full api does not exhaust it.
    let other = create_api(&emulator, "another api", "authorizer-quota-other", "HTTP").await;
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{other}/authorizers"),
        Some(json!({
            "name": "room",
            "authorizerType": "REQUEST",
            "authorizerUri": lambda_uri("quota-auth"),
            "authorizerPayloadFormatVersion": "2.0",
            "identitySource": ["$request.header.Authorization"]
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 under another api, got {status} {created}"
    );
}

#[tokio::test]
async fn an_authorizer_belongs_to_the_one_api_that_owns_it() {
    let emulator = emulator();
    let first = create_api(&emulator, "the owning api", "authorizer-owner-api", "HTTP").await;
    let second = create_api(&emulator, "another api", "authorizer-other-api", "HTTP").await;

    let request = json!({
        "name": "owned",
        "authorizerType": "REQUEST",
        "authorizerUri": lambda_uri("owned-auth"),
        "authorizerPayloadFormatVersion": "2.0",
        "identitySource": ["$request.header.Authorization"]
    });
    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{first}/authorizers"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateAuthorizer, got {status} {created}"
    );
    let authorizer_id = as_str(&created, "authorizerId");

    for (case, uri) in [
        (
            "an authorizer of another api",
            format!("/v2/apis/{second}/authorizers/{authorizer_id}"),
        ),
        (
            "an authorizer id nothing minted",
            format!("/v2/apis/{first}/authorizers/nosuchauth0"),
        ),
        (
            "an api that does not exist",
            format!("/v2/apis/nosuchapi00/authorizers/{authorizer_id}"),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetAuthorizer, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // Creating under an api that does not exist creates nothing.
    let (status, body) = call(
        &emulator,
        "POST",
        "/v2/apis/nosuchapi00/authorizers",
        Some(request),
    )
    .await;
    assert_eq!(
        status, 404,
        "expected 404 from CreateAuthorizer on an unknown api, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "expected NotFoundException, got {body}"
    );
}
