// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the `AWS::ApiGatewayV2::*` resources its template
//! declares, in API Gateway v2 — and publishes identifiers a client can call that service
//! with.
//!
//! The rule under test: **a stack that reaches `CREATE_COMPLETE` has created each
//! `AWS::ApiGatewayV2::Api`, `::DomainName` and `::ApiMapping` its template declares in
//! API Gateway v2 through that service's own create API, and every `Outputs` entry over
//! one of them carries an `OutputValue` that is the identifier that service really minted:
//! `Ref` of an api is its `ApiId` and of a domain name the domain, `Fn::GetAtt` reads an
//! api's `ApiEndpoint`, a mapping's `ApiMappingId` and a domain's `RegionalDomainName`, and
//! each of those values resolves through the matching read (`GetApi`, `GetDomainName`,
//! `GetApiMapping`).** Two crates have to agree for that to hold (CloudFormation makes the
//! calls, API Gateway v2 serves them), so the suite lives here rather than in either of
//! them.
//!
//! The rows differ along what the rule generalises over: which of the three types the
//! template declares (a domain alone, an api alone, all three together), the api's
//! `ProtocolType` (and so the scheme its endpoint carries), whether the api is quick-created
//! from a `Target`, the stage a mapping names, whether the mapping carries an
//! `ApiMappingKey`, and the language the template is written in (JSON and YAML, with the
//! short-form intrinsic tags). Every expected value is computed here from what the row
//! declares or read from the owning service, never from the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_stack_resources::{outputs_of, xml_text, xml_texts};

/// The region every request here is signed for, and so the region the stack's resources
/// and their endpoints belong to.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due.
pub(crate) const PAST_THE_STACK: Duration = Duration::from_millis(1_200);

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
pub(crate) fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
pub(crate) async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one API Gateway v2 operation over its restJson1 wire: this service signs as
/// `apigateway` and binds every operation under `/v2/…`.
pub(crate) async fn apigatewayv2(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(document) => Bytes::from(document.to_string()),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// Create a stack from `template` and answer its id.
pub(crate) async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    xml_text(&body, "StackId").unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {body}"))
}

/// The one stack `stack_name` names, as the response body.
pub(crate) async fn describe_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// A string member of an answered document.
pub(crate) fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} in {body}"))
        .to_string()
}

/// The certificate a custom domain's endpoint presents. A literal ARN: the stack under test
/// is about the domain the template declares, and API Gateway takes the certificate ARN as
/// the configuration it reports back.
fn certificate_arn() -> String {
    format!(
        "arn:aws:acm:{REGION}:{ACCOUNT_ID}:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"
    )
}

/// What one row declares and what the stack must then have created.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name the row runs under (which is what
    /// makes the declared domain name unique per row).
    template: fn(&str) -> String,
    /// The `ProtocolType` the row's api declares, and so the scheme its endpoint carries;
    /// `None` for a row declaring no api.
    protocol_type: Option<&'static str>,
    /// The `Name` the row's api declares, after the stack name: `{stack}-{suffix}`.
    api_name_suffix: &'static str,
    /// Whether the row declares a domain name.
    domain: bool,
    /// One entry per api mapping the row declares: the output its `ApiMappingId` is
    /// published under, the stage it maps and the `ApiMappingKey` it carries.
    mappings: &'static [(&'static str, &'static str, Option<&'static str>)],
}

/// The domain name a row declares: one DNS name per stack, so two rows never collide.
fn domain_of(stack: &str) -> String {
    format!("{stack}.example.com")
}

const ROWS: &[Row] = &[
    Row {
        case: "a domain name alone, which a client then maps an api onto itself",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Domain": {
                        "Type": "AWS::ApiGatewayV2::DomainName",
                        "Properties": {
                            "DomainName": domain_of(stack),
                            "DomainNameConfigurations": [{
                                "EndpointType": "REGIONAL",
                                "CertificateArn": certificate_arn(),
                                "SecurityPolicy": "TLS_1_2"
                            }]
                        }
                    }
                },
                "Outputs": {
                    "DomainName": { "Value": { "Ref": "Domain" } },
                    "RegionalDomainName": {
                        "Value": { "Fn::GetAtt": ["Domain", "RegionalDomainName"] }
                    }
                }
            })
            .to_string()
        },
        protocol_type: None,
        api_name_suffix: "",
        domain: true,
        mappings: &[],
    },
    Row {
        case: "an HTTP api quick-created from a Target, with a domain and two mappings — \
               one at the domain's root and one under a key",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Domain": {
                        "Type": "AWS::ApiGatewayV2::DomainName",
                        "Properties": {
                            "DomainName": domain_of(stack),
                            "DomainNameConfigurations": [{
                                "EndpointType": "REGIONAL",
                                "CertificateArn": certificate_arn(),
                                "SecurityPolicy": "TLS_1_2"
                            }]
                        }
                    },
                    "Api": {
                        "Type": "AWS::ApiGatewayV2::Api",
                        "Properties": {
                            "Name": format!("{stack}-api"),
                            "ProtocolType": "HTTP",
                            "Target": "https://example.com/backend",
                            "RouteKey": "ANY /"
                        }
                    },
                    "Mapping": {
                        "Type": "AWS::ApiGatewayV2::ApiMapping",
                        "Properties": {
                            "ApiId": { "Ref": "Api" },
                            "DomainName": { "Ref": "Domain" },
                            "Stage": "$default"
                        }
                    },
                    "KeyedMapping": {
                        "Type": "AWS::ApiGatewayV2::ApiMapping",
                        "Properties": {
                            "ApiId": { "Ref": "Api" },
                            "DomainName": { "Ref": "Domain" },
                            "Stage": "$default",
                            "ApiMappingKey": "orders"
                        }
                    }
                },
                "Outputs": {
                    "ApiId": { "Value": { "Ref": "Api" } },
                    "ApiEndpoint": { "Value": { "Fn::GetAtt": ["Api", "ApiEndpoint"] } },
                    "ApiMappingId": {
                        "Value": { "Fn::GetAtt": ["Mapping", "ApiMappingId"] }
                    },
                    "KeyedApiMappingId": { "Value": { "Ref": "KeyedMapping" } },
                    "DomainName": { "Value": { "Ref": "Domain" } },
                    "RegionalDomainName": {
                        "Value": { "Fn::GetAtt": ["Domain", "RegionalDomainName"] }
                    }
                }
            })
            .to_string()
        },
        protocol_type: Some("HTTP"),
        api_name_suffix: "api",
        domain: true,
        // The `Ref` of a mapping and its `ApiMappingId` attribute are the same id, so the
        // keyed row's output is written with `Ref` and the other with `Fn::GetAtt`.
        mappings: &[
            ("ApiMappingId", "$default", None),
            ("KeyedApiMappingId", "$default", Some("orders")),
        ],
    },
    Row {
        case: "a WEBSOCKET api beside a domain, in YAML with the short-form tags",
        template: |stack| {
            format!(
                "\
AWSTemplateFormatVersion: '2010-09-09'
Description: a WebSocket api and a custom domain
Resources:
  Domain:
    Type: AWS::ApiGatewayV2::DomainName
    Properties:
      DomainName: {domain}
      DomainNameConfigurations:
        - EndpointType: REGIONAL
          CertificateArn: {certificate}
          SecurityPolicy: TLS_1_2
  Api:
    Type: AWS::ApiGatewayV2::Api
    Properties:
      Name: {stack}-socket
      ProtocolType: WEBSOCKET
      RouteSelectionExpression: $request.body.action
      Description: the socket api
Outputs:
  ApiId:
    Value: !Ref Api
  ApiEndpoint:
    Value: !GetAtt Api.ApiEndpoint
  DomainName:
    Value: !Ref Domain
  RegionalDomainName:
    Value: !GetAtt Domain.RegionalDomainName
",
                domain = domain_of(stack),
                certificate = certificate_arn(),
            )
        },
        protocol_type: Some("WEBSOCKET"),
        api_name_suffix: "socket",
        domain: true,
        // A mapping names a STAGE of its api, and a WebSocket api holds none until
        // something creates one — this row's template declares no
        // `AWS::ApiGatewayV2::Stage` (the stages a template declares are provisioned, and
        // `cloudformation_apigatewayv2_api_children` owns those), so it declares no
        // mapping either.
        mappings: &[],
    },
];

/// The scheme an api of `protocol_type` is invoked over: a WEBSOCKET api is reached on
/// `wss`, an HTTP api on `https` (`Api$ApiEndpoint`).
fn expected_scheme(protocol_type: &str) -> &'static str {
    if protocol_type == "WEBSOCKET" {
        "wss"
    } else {
        "https"
    }
}

/// Every output of a `DescribeStacks` answer carries BOTH its key and its value: an entry
/// with a key and no value is what makes a stack's outputs unreadable, so the two lists
/// are compared before anything is read out of them.
pub(crate) fn every_output_has_a_value(case: &str, body: &str) -> BTreeMap<String, String> {
    let keys = xml_texts(body, "OutputKey");
    let values = xml_texts(body, "OutputValue");
    assert_eq!(
        keys.len(),
        values.len(),
        "case {case:?}: every Outputs entry must carry an OutputValue; got {} keys \
         ({keys:?}) and {} values ({values:?})",
        keys.len(),
        values.len()
    );
    let outputs = outputs_of(body);
    for (key, value) in &outputs {
        assert!(
            !value.is_empty(),
            "case {case:?}: output {key} must carry a resolved value, got {value:?}"
        );
    }
    outputs
}

#[tokio::test]
async fn a_stack_creates_the_api_gateway_v2_resources_it_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("apigwv2-{index}");
        let template = (row.template)(&stack_name);
        create_stack(&router, case, &stack_name, &template).await;

        tokio::time::sleep(PAST_THE_STACK).await;
        let body = describe_stack(&router, case, &stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );
        let outputs = every_output_has_a_value(case, &body);

        // The domain the template declared exists in API Gateway v2, with the
        // configuration the template gave it — and the stack's `Ref` output is the name a
        // client reads it back by.
        if row.domain {
            let declared = domain_of(&stack_name);
            assert_eq!(
                outputs.get("DomainName"),
                Some(&declared),
                "case {case:?}: expected the Ref of the domain to be the declared name, \
                 got {outputs:?}"
            );
            let (status, domain) = apigatewayv2(
                &router,
                "GET",
                &format!("/v2/domainnames/{declared}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetDomainName for {declared:?}, got \
                 {status} {domain}"
            );
            assert_eq!(
                member(case, &domain, "domainName"),
                declared,
                "case {case:?}: expected the declared domain name, got {domain}"
            );
            let endpoint = domain["domainNameConfigurations"]
                .as_array()
                .and_then(|entries| entries.first())
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the declared endpoint, got {domain}")
                });
            for (field, expected) in [
                ("endpointType", "REGIONAL".to_string()),
                ("securityPolicy", "TLS_1_2".to_string()),
                ("certificateArn", certificate_arn()),
                // A domain this service holds is serving the endpoints its configuration
                // describes, so it reports its own status as available.
                ("domainNameStatus", "AVAILABLE".to_string()),
            ] {
                assert_eq!(
                    member(case, &endpoint, field),
                    expected,
                    "case {case:?}: expected the declared {field}, got {endpoint}"
                );
            }
            // The output's `RegionalDomainName` is the host that service really serves the
            // domain on, not a value CloudFormation guessed.
            assert_eq!(
                outputs.get("RegionalDomainName").map(String::as_str),
                endpoint.get("apiGatewayDomainName").and_then(Value::as_str),
                "case {case:?}: expected the RegionalDomainName output to be the host API \
                 Gateway serves the domain on, got {outputs:?} and {endpoint}"
            );
        }

        // The api the template declared exists, under the id the stack published — which
        // is what every later call takes as its `ApiId`.
        if let Some(protocol_type) = row.protocol_type {
            let api_id = outputs
                .get("ApiId")
                .unwrap_or_else(|| panic!("case {case:?}: no ApiId output in {outputs:?}"))
                .clone();
            let (status, api) =
                apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetApi for the published ApiId \
                 {api_id:?}, got {status} {api}"
            );
            assert_eq!(
                member(case, &api, "name"),
                format!("{stack_name}-{}", row.api_name_suffix),
                "case {case:?}: expected the api the template declared, got {api}"
            );
            assert_eq!(
                member(case, &api, "protocolType"),
                protocol_type,
                "case {case:?}: expected the declared ProtocolType, got {api}"
            );
            // The `ApiEndpoint` output is the endpoint that service reports for the api,
            // under the scheme its protocol speaks.
            let endpoint = member(case, &api, "apiEndpoint");
            assert_eq!(
                outputs.get("ApiEndpoint"),
                Some(&endpoint),
                "case {case:?}: expected the ApiEndpoint output to be the endpoint API \
                 Gateway reports, got {outputs:?}"
            );
            // The endpoint names the API GATEWAY address the api is reached at: an HTTP
            // api's own `execute-api` host, and for a WEBSOCKET api the socket that api's
            // data plane is served on, which carries the core's assigned port rather than
            // the api id (the apigatewayv2 crate's `api_endpoint_scheme` suite owns that
            // whole rule).
            assert!(
                endpoint.starts_with(&format!("{}://", expected_scheme(protocol_type))),
                "case {case:?}: expected a {protocol_type} endpoint, got {endpoint:?}"
            );
            if protocol_type == "HTTP" {
                assert!(
                    endpoint.starts_with(&format!("https://{api_id}.")),
                    "case {case:?}: expected an HTTP endpoint naming the api, got {endpoint:?}"
                );
            }

            // Each mapping the template declared, read back by the id the stack published
            // — the id a client calls `GetApiMapping` with.
            let mut published_ids: Vec<String> = Vec::new();
            for (output, stage, mapping_key) in row.mappings {
                let mapping_id = outputs
                    .get(*output)
                    .unwrap_or_else(|| panic!("case {case:?}: no {output} output in {outputs:?}"))
                    .clone();
                let declared = domain_of(&stack_name);
                let (status, mapping) = apigatewayv2(
                    &router,
                    "GET",
                    &format!("/v2/domainnames/{declared}/apimappings/{mapping_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from GetApiMapping for the published \
                     {output} {mapping_id:?}, got {status} {mapping}"
                );
                assert_eq!(
                    member(case, &mapping, "apiId"),
                    api_id,
                    "case {case:?}: expected the mapping to name the api the template \
                     mapped, got {mapping}"
                );
                assert_eq!(
                    member(case, &mapping, "stage"),
                    *stage,
                    "case {case:?}: expected the declared Stage, got {mapping}"
                );
                assert_eq!(
                    mapping.get("apiMappingKey").and_then(Value::as_str),
                    *mapping_key,
                    "case {case:?}: expected the declared ApiMappingKey, got {mapping}"
                );
                assert!(
                    !published_ids.contains(&mapping_id),
                    "case {case:?}: two declared mappings must not publish one id, got \
                     {outputs:?}"
                );
                published_ids.push(mapping_id);
            }
        }
    }
}

/// A domain a stack created is an ordinary custom domain of the account: a client can map
/// an api onto it with a direct, non-CloudFormation `CreateApiMapping`.
#[tokio::test]
async fn a_stack_created_domain_takes_a_direct_api_mapping() {
    let router = build_router();
    let case = "a direct CreateApiMapping onto a stack-created domain";
    let stack_name = "apigwv2-direct";
    let declared = domain_of(stack_name);
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Domain": {
                "Type": "AWS::ApiGatewayV2::DomainName",
                "Properties": {
                    "DomainName": declared,
                    "DomainNameConfigurations": [{
                        "EndpointType": "REGIONAL",
                        "CertificateArn": certificate_arn(),
                        "SecurityPolicy": "TLS_1_2"
                    }]
                }
            }
        },
        "Outputs": { "DomainName": { "Value": { "Ref": "Domain" } } }
    })
    .to_string();
    create_stack(&router, case, stack_name, &template).await;
    tokio::time::sleep(PAST_THE_STACK).await;
    let body = describe_stack(&router, case, stack_name).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );

    // An api of the client's own, quick-created so it holds the `$default` stage.
    let (status, api) = apigatewayv2(
        &router,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "direct-api",
            "protocolType": "HTTP",
            "target": "https://example.com/backend"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {api}"
    );
    let api_id = member(case, &api, "apiId");

    let (status, mapping) = apigatewayv2(
        &router,
        "POST",
        &format!("/v2/domainnames/{declared}/apimappings"),
        Some(json!({ "apiId": api_id, "stage": "$default" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateApiMapping onto the stack's domain, got \
         {status} {mapping}"
    );
    assert_eq!(
        member(case, &mapping, "apiId"),
        api_id,
        "case {case:?}: expected the mapping to name the api, got {mapping}"
    );
}

/// Deleting the stack deletes what it created: the api and the domain stop existing in API
/// Gateway v2, which is how a client can tell the stack really owned them.
#[tokio::test]
async fn deleting_the_stack_deletes_the_resources_it_created() {
    let router = build_router();
    let case = "a deleted apigatewayv2 stack";
    let stack_name = "apigwv2-deleted";
    let declared = domain_of(stack_name);
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Domain": {
                "Type": "AWS::ApiGatewayV2::DomainName",
                "Properties": {
                    "DomainName": declared,
                    "DomainNameConfigurations": [{
                        "EndpointType": "REGIONAL",
                        "CertificateArn": certificate_arn(),
                        "SecurityPolicy": "TLS_1_2"
                    }]
                }
            },
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": {
                    "Name": "deleted-api",
                    "ProtocolType": "HTTP",
                    "Target": "https://example.com/backend"
                }
            },
            "Mapping": {
                "Type": "AWS::ApiGatewayV2::ApiMapping",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "DomainName": { "Ref": "Domain" },
                    "Stage": "$default"
                }
            }
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "ApiMappingId": { "Value": { "Fn::GetAtt": ["Mapping", "ApiMappingId"] } }
        }
    })
    .to_string();
    create_stack(&router, case, stack_name, &template).await;
    tokio::time::sleep(PAST_THE_STACK).await;
    let body = describe_stack(&router, case, stack_name).await;
    let outputs = every_output_has_a_value(case, &body);
    let api_id = outputs
        .get("ApiId")
        .unwrap_or_else(|| panic!("case {case:?}: no ApiId output in {outputs:?}"))
        .clone();
    let mapping_id = outputs
        .get("ApiMappingId")
        .unwrap_or_else(|| panic!("case {case:?}: no ApiMappingId output in {outputs:?}"))
        .clone();
    let (status, _) = apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the stack's api must exist before the stack is deleted"
    );

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    tokio::time::sleep(PAST_THE_STACK).await;

    for (step, uri) in [
        ("GetApi", format!("/v2/apis/{api_id}")),
        (
            "GetApiMapping",
            format!("/v2/domainnames/{declared}/apimappings/{mapping_id}"),
        ),
        ("GetDomainName", format!("/v2/domainnames/{declared}")),
    ] {
        let (status, body) = apigatewayv2(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {step} to report the deleted resource gone, got \
             {status} {body}"
        );
    }
}
