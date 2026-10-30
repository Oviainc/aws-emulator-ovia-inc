// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack provisions the `AWS::ApiGateway::DomainName` resources its
//! template declares, in the Amazon API Gateway REST control plane that owns them.
//!
//! The rule under test: **a stack reports a resource created only once the service that
//! owns its type really created it, through that service's own create API, and never while
//! that service still reports the resource short of the state it is usable in — so a client
//! that deployed a template declaring a custom domain name reads that domain back with
//! `apigateway GetDomainName`, under the name the template stated, carrying the
//! configuration the template declared and the endpoint names only API Gateway assigns,
//! and the domain stops existing when the stack is deleted.** Two crates have to agree for
//! that to hold (CloudFormation makes the calls, API Gateway serves them), so the suite
//! lives here rather than in either of them.
//!
//! The rows differ along what the rule generalises over: the endpoint type the template
//! declares (`REGIONAL`, `EDGE`, and none at all, so the service's own completion applies),
//! which certificate member carries it, the `securityPolicy` declared (two spellings out of
//! that enum, and none), whether the declaration carries tags, a resource policy document
//! or a mutual-TLS truststore, which endpoint attributes the stack publishes, and the
//! language the template is written in (JSON, and YAML with the short-form intrinsic tags).
//! Every expected value is computed here from what the row declares or read from the owning
//! service, never from the answer being checked.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery call, the stack readers and the output readers this test crate writes once
// (`tests/all/cloudformation_apigatewayv2_resources.rs` for the API Gateway v2 half of the
// same rule, `tests/all/cloudformation_provisions_stack_resources.rs` for the XML readers).
use crate::cloudformation_apigatewayv2_resources::{
    cloudformation, create_stack, describe_stack, every_output_has_a_value, member, REGION,
};
use crate::cloudformation_provisions_stack_resources::xml_text;
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// How often the suite asks the stack whether it has completed, and how long it goes on
/// asking. An edge-optimized domain is not in the state it is usable in when its create
/// answers, so the stack stays `CREATE_IN_PROGRESS` until API Gateway reports it ready: the
/// FIRST observation that reports the stack complete is the one every assertion is made
/// about. The budget is long enough that a loaded machine is slower than the spans these
/// services choose without the rule being any different.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The `StackStatus` values the rule is stated in.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The `DomainNameStatus` a custom domain name is usable in, out of the enum API Gateway's
/// model declares for that member.
const AVAILABLE: &str = "AVAILABLE";

/// The prefix of the tag keys AWS reserves for itself, which API Gateway refuses on every
/// call that takes tags — so the three `aws:cloudformation:*` records CloudFormation keeps
/// of its own accord must not be sent to it.
const RESERVED_TAG_PREFIX: &str = "aws:";

/// What API Gateway answers for a custom domain name it does not hold: the modelled 404,
/// whose message is a sentence of its own with nothing interpolated from the request.
const NOT_FOUND: &str = "NotFoundException";
const NO_SUCH_DOMAIN_NAME: &str = "Invalid domain name identifier specified";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// Call one Amazon API Gateway operation over its restJson1 wire: this service signs as
/// `apigateway` and binds its custom-domain operations under `/domainnames`.
///
/// Answers the HTTP status, the `x-amzn-errortype` a refusal carries (the wire error code a
/// client reads) and the answered document.
async fn apigateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Option<String>, Value) {
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
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        });
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, error_type, answered)
}

/// The certificate a custom domain's endpoint presents. A literal ARN: the stack under test
/// is about the domain the template declares, and API Gateway takes the certificate ARN as
/// the configuration it reports back.
fn certificate_arn() -> String {
    format!(
        "arn:aws:acm:{REGION}:000000000000:certificate/8f4c1b1e-2a1d-4a57-9c0e-1f7b2a3d4e5f"
    )
}

/// The domain name a row declares: one DNS name per stack, so two rows never collide.
fn domain_of(stack: &str) -> String {
    format!("{stack}.example.com")
}

/// The resource policy one row declares for its domain, as the document a template carries
/// and the API takes the TEXT of.
fn declared_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "execute-api:Invoke",
            "Resource": "execute-api:/*"
        }]
    })
}

/// What one row declares and what the stack must then have created.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name the row runs under (which is what
    /// makes the declared domain name unique per row).
    template: fn(&str) -> String,
    /// The endpoint type the row declares, or `None` where it declares no
    /// `EndpointConfiguration` at all.
    endpoint_type: Option<&'static str>,
    /// The `securityPolicy` the row declares, or `None` where it declares none.
    security_policy: Option<&'static str>,
    /// The member `GetDomainName` must report the row's certificate ARN under, which is the
    /// property the row declared it as with a lower-cased initial.
    certificate_member: &'static str,
    /// The `routingMode` the row declares, or `None` where it declares none.
    routing_mode: Option<&'static str>,
    /// The tags the row declares, each of which the created domain must carry.
    tags: &'static [(&'static str, &'static str)],
    /// Whether the row declares [`declared_policy`] for its domain.
    policy: bool,
    /// The mutual-TLS truststore the row declares, as `(uri, version)`.
    truststore: Option<(&'static str, &'static str)>,
    /// The endpoint attributes the row publishes as outputs, as `(output key, the member
    /// API Gateway reports that value under)`.
    endpoint_outputs: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a regional domain with its regional certificate, a security policy and a tag",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Domain": {
                        "Type": "AWS::ApiGateway::DomainName",
                        "Properties": {
                            "DomainName": domain_of(stack),
                            "RegionalCertificateArn": certificate_arn(),
                            "SecurityPolicy": "TLS_1_2",
                            "EndpointConfiguration": { "Types": ["REGIONAL"] },
                            "Tags": [{ "Key": "tier", "Value": "edge-api" }]
                        }
                    }
                },
                "Outputs": {
                    "DomainName": { "Value": { "Ref": "Domain" } },
                    "RegionalDomainName": {
                        "Value": { "Fn::GetAtt": ["Domain", "RegionalDomainName"] }
                    },
                    "DomainNameArn": { "Value": { "Fn::GetAtt": ["Domain", "DomainNameArn"] } }
                }
            })
            .to_string()
        },
        endpoint_type: Some("REGIONAL"),
        security_policy: Some("TLS_1_2"),
        certificate_member: "regionalCertificateArn",
        routing_mode: None,
        tags: &[("tier", "edge-api")],
        policy: false,
        truststore: None,
        endpoint_outputs: &[
            ("RegionalDomainName", "regionalDomainName"),
            ("DomainNameArn", "domainNameArn"),
        ],
    },
    Row {
        case: "an edge-optimized domain, whose CloudFront distribution is not in place when \
               the create answers",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Domain": {
                        "Type": "AWS::ApiGateway::DomainName",
                        "Properties": {
                            "DomainName": domain_of(stack),
                            "CertificateArn": certificate_arn(),
                            "SecurityPolicy": "SecurityPolicy_TLS12_2018_EDGE",
                            "EndpointConfiguration": { "Types": ["EDGE"] }
                        }
                    }
                },
                "Outputs": {
                    "DomainName": { "Value": { "Ref": "Domain" } },
                    "DistributionDomainName": {
                        "Value": { "Fn::GetAtt": ["Domain", "DistributionDomainName"] }
                    },
                    "DistributionHostedZoneId": {
                        "Value": { "Fn::GetAtt": ["Domain", "DistributionHostedZoneId"] }
                    }
                }
            })
            .to_string()
        },
        endpoint_type: Some("EDGE"),
        security_policy: Some("SecurityPolicy_TLS12_2018_EDGE"),
        certificate_member: "certificateArn",
        routing_mode: None,
        tags: &[],
        policy: false,
        truststore: None,
        endpoint_outputs: &[
            ("DistributionDomainName", "distributionDomainName"),
            ("DistributionHostedZoneId", "distributionHostedZoneId"),
        ],
    },
    Row {
        case: "a domain declaring no endpoint configuration, a resource policy and a \
               mutual-TLS truststore, in YAML with the short-form tags",
        template: |stack| {
            format!(
                "\
AWSTemplateFormatVersion: '2010-09-09'
Description: a custom domain whose endpoint type the service completes
Resources:
  Domain:
    Type: AWS::ApiGateway::DomainName
    Properties:
      DomainName: {domain}
      CertificateArn: {certificate}
      MutualTlsAuthentication:
        TruststoreUri: s3://truststores/clients.pem
        TruststoreVersion: '4fbd2a'
      Policy:
        Version: '2012-10-17'
        Statement:
          - Effect: Allow
            Principal: '*'
            Action: execute-api:Invoke
            Resource: execute-api:/*
Outputs:
  DomainName:
    Value: !Ref Domain
  DomainNameArn:
    Value: !GetAtt Domain.DomainNameArn
",
                domain = domain_of(stack),
                certificate = certificate_arn(),
            )
        },
        endpoint_type: None,
        security_policy: None,
        certificate_member: "certificateArn",
        routing_mode: None,
        tags: &[],
        policy: true,
        truststore: Some(("s3://truststores/clients.pem", "4fbd2a")),
        endpoint_outputs: &[("DomainNameArn", "domainNameArn")],
    },
    Row {
        case: "a regional domain with a routing mode, a newer security policy and two tags",
        template: |stack| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Domain": {
                        "Type": "AWS::ApiGateway::DomainName",
                        "Properties": {
                            "DomainName": domain_of(stack),
                            "RegionalCertificateArn": certificate_arn(),
                            "RegionalCertificateName": "orders-regional",
                            "SecurityPolicy": "SecurityPolicy_TLS13_1_2_2021_06",
                            "RoutingMode": "BASE_PATH_MAPPING_ONLY",
                            "EndpointConfiguration": { "Types": ["REGIONAL"] },
                            "Tags": [
                                { "Key": "team", "Value": "payments" },
                                { "Key": "env", "Value": "staging" }
                            ]
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
        endpoint_type: Some("REGIONAL"),
        security_policy: Some("SecurityPolicy_TLS13_1_2_2021_06"),
        certificate_member: "regionalCertificateArn",
        routing_mode: Some("BASE_PATH_MAPPING_ONLY"),
        tags: &[("team", "payments"), ("env", "staging")],
        policy: false,
        truststore: None,
        endpoint_outputs: &[("RegionalDomainName", "regionalDomainName")],
    },
];

/// The stack's body at the FIRST observation that reports it complete, which is what every
/// assertion about a provisioned resource is made against.
async fn awaited_complete(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLLS {
        let body = describe_stack(router, case, stack).await;
        match xml_text(&body, "StackStatus").as_deref() {
            Some(CREATE_COMPLETE) => return body,
            Some(CREATE_IN_PROGRESS) => {}
            other => panic!(
                "case {case:?}: expected the stack to be creating or complete, got {other:?} \
                 in {body}"
            ),
        }
        last = body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the stack never completed; the last answer was {last}")
}

/// The custom domain name API Gateway holds under `domain_name`, read the way a client
/// reads it.
async fn get_domain_name(
    router: &MultiServiceRouter,
    domain_name: &str,
) -> (u16, Option<String>, Value) {
    apigateway(router, "GET", &format!("/domainnames/{domain_name}"), None).await
}

#[tokio::test]
async fn a_stack_creates_the_custom_domain_names_it_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("apigw-domain-{index}");
        let declared = domain_of(&stack_name);
        create_stack(&router, case, &stack_name, &(row.template)(&stack_name)).await;

        let body = awaited_complete(&router, case, &stack_name).await;
        let outputs = every_output_has_a_value(case, &body);

        // The domain the template declared exists in API Gateway, and the stack's `Ref`
        // output is the name a client reads it back by.
        assert_eq!(
            outputs.get("DomainName"),
            Some(&declared),
            "case {case:?}: expected the Ref of the domain to be the declared name, got \
             {outputs:?}"
        );
        let (status, _, domain) = get_domain_name(&router, &declared).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetDomainName for {declared:?}, got {status} \
             {domain}"
        );
        assert_eq!(
            member(case, &domain, "domainName"),
            declared,
            "case {case:?}: expected the declared domain name, got {domain}"
        );
        // The stack was not reported complete before API Gateway had the domain in the
        // state it is usable in, so the first completed observation reads it available.
        assert_eq!(
            member(case, &domain, "domainNameStatus"),
            AVAILABLE,
            "case {case:?}: expected the domain to be available once the stack completed, \
             got {domain}"
        );

        // Every value the declaration carried reached API Gateway.
        assert_eq!(
            member(case, &domain, row.certificate_member),
            certificate_arn(),
            "case {case:?}: expected the declared certificate under {}, got {domain}",
            row.certificate_member
        );
        // A row stating no `SecurityPolicy` leaves the service's own default in place,
        // which is API Gateway's to choose and nothing this rule is about.
        if let Some(security_policy) = row.security_policy {
            assert_eq!(
                domain.get("securityPolicy").and_then(Value::as_str),
                Some(security_policy),
                "case {case:?}: expected the declared securityPolicy, got {domain}"
            );
        }
        assert_eq!(
            domain.get("routingMode").and_then(Value::as_str),
            row.routing_mode,
            "case {case:?}: expected the declared routingMode, got {domain}"
        );
        let types = domain["endpointConfiguration"]["types"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an endpoint configuration, got {domain}")
            });
        match row.endpoint_type {
            Some(endpoint_type) => assert_eq!(
                types,
                vec![Value::String(endpoint_type.to_string())],
                "case {case:?}: expected the declared endpoint type, got {domain}"
            ),
            // A declaration stating no endpoint configuration is completed by API Gateway
            // itself, so what the row asserts is that the created domain is serving ONE
            // endpoint type rather than which one it chose.
            None => assert_eq!(
                types.len(),
                1,
                "case {case:?}: expected the service to complete the endpoint \
                 configuration with one endpoint type, got {domain}"
            ),
        }
        if row.policy {
            let reported = member(case, &domain, "policy");
            let parsed: Value = serde_json::from_str(&reported).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected the policy to be the declared document's \
                     text, got {reported:?} ({error})"
                )
            });
            assert_eq!(
                parsed,
                declared_policy(),
                "case {case:?}: expected the declared policy document, got {reported:?}"
            );
        }
        if let Some((uri, version)) = row.truststore {
            let authentication = &domain["mutualTlsAuthentication"];
            assert_eq!(
                member(case, authentication, "truststoreUri"),
                uri,
                "case {case:?}: expected the declared truststore, got {domain}"
            );
            assert_eq!(
                member(case, authentication, "truststoreVersion"),
                version,
                "case {case:?}: expected the declared truststore version, got {domain}"
            );
        }

        // The tags the declaration asked for are on the domain, and the records
        // CloudFormation keeps of its own accord — which API Gateway refuses, the `aws:`
        // namespace being AWS's own — are not sent to it.
        let tags = domain["tags"].as_object().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: expected the collection of tags, got {domain}")
        });
        for (key, value) in row.tags {
            assert_eq!(
                tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected the declared tag {key}, got {tags:?}"
            );
        }
        for key in tags.keys() {
            assert!(
                !key.to_ascii_lowercase().starts_with(RESERVED_TAG_PREFIX),
                "case {case:?}: expected no reserved tag key on the domain, got {tags:?}"
            );
        }

        // Each endpoint attribute the stack published is the value API Gateway really
        // assigned, not one CloudFormation computed. The output has to BE there before it
        // can agree with anything: an absent one and an unreported member would otherwise
        // agree by both being nothing.
        for (output, reported) in row.endpoint_outputs {
            let published = outputs
                .get(*output)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {output} output in {outputs:?}")
                });
            assert_eq!(
                Some(published.as_str()),
                domain.get(*reported).and_then(Value::as_str),
                "case {case:?}: expected the {output} output to be the {reported} API \
                 Gateway reports, got {outputs:?} and {domain}"
            );
        }
    }
}

/// A domain a stack created is an ordinary custom domain of the account: a client can map a
/// base path of it onto a REST API with a direct, non-CloudFormation
/// `CreateBasePathMapping`, and read that mapping back.
#[tokio::test]
async fn a_stack_created_domain_takes_a_direct_base_path_mapping() {
    let router = build_router();
    let case = "a direct CreateBasePathMapping onto a stack-created domain";
    let stack_name = "apigw-domain-mapped";
    let declared = domain_of(stack_name);
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Domain": {
                "Type": "AWS::ApiGateway::DomainName",
                "Properties": {
                    "DomainName": declared,
                    "RegionalCertificateArn": certificate_arn(),
                    "SecurityPolicy": "TLS_1_2",
                    "EndpointConfiguration": { "Types": ["REGIONAL"] }
                }
            }
        },
        "Outputs": { "DomainName": { "Value": { "Ref": "Domain" } } }
    })
    .to_string();
    create_stack(&router, case, stack_name, &template).await;
    awaited_complete(&router, case, stack_name).await;

    // A REST API of the client's own, created straight through API Gateway.
    let (status, _, api) = apigateway(
        &router,
        "POST",
        "/restapis",
        Some(json!({ "name": "mapped-api" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRestApi, got {status} {api}"
    );
    let api_id = member(case, &api, "id");

    let (status, _, mapping) = apigateway(
        &router,
        "POST",
        &format!("/domainnames/{declared}/basepathmappings"),
        Some(json!({ "basePath": "v1", "restApiId": api_id })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateBasePathMapping onto the stack's domain, \
         got {status} {mapping}"
    );

    let (status, _, read) = apigateway(
        &router,
        "GET",
        &format!("/domainnames/{declared}/basepathmappings/v1"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetBasePathMapping on the stack's domain, got \
         {status} {read}"
    );
    assert_eq!(
        member(case, &read, "restApiId"),
        api_id,
        "case {case:?}: expected the mapping to name the api it was created for, got {read}"
    );
    assert_eq!(
        member(case, &read, "basePath"),
        "v1",
        "case {case:?}: expected the base path the mapping was created under, got {read}"
    );
}

/// Deleting the stack deletes what it created: the domain stops existing in API Gateway,
/// which is how a client can tell the stack really owned it — and the read that no longer
/// resolves is the service's own modelled refusal.
#[tokio::test]
async fn deleting_the_stack_deletes_the_domain_it_created() {
    let router = build_router();
    let case = "a deleted api gateway custom-domain stack";
    let stack_name = "apigw-domain-deleted";
    let declared = domain_of(stack_name);
    let template = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "Domain": {
                "Type": "AWS::ApiGateway::DomainName",
                "Properties": {
                    "DomainName": declared,
                    "RegionalCertificateArn": certificate_arn(),
                    "EndpointConfiguration": { "Types": ["REGIONAL"] }
                }
            }
        },
        "Outputs": { "DomainName": { "Value": { "Ref": "Domain" } } }
    })
    .to_string();
    create_stack(&router, case, stack_name, &template).await;
    awaited_complete(&router, case, stack_name).await;

    let (status, _, domain) = get_domain_name(&router, &declared).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the stack's domain must exist before the stack is deleted, got \
         {status} {domain}"
    );

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    // The stack's deletion is the clock's to make due, so the read a client makes next is
    // asked until it answers for a domain nothing holds.
    let mut answer = get_domain_name(&router, &declared).await;
    for _ in 0..POLLS {
        if answer.0 != 200 {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        answer = get_domain_name(&router, &declared).await;
    }
    let (status, error_type, gone) = answer;
    assert_eq!(
        status, 404,
        "case {case:?}: expected GetDomainName to report the deleted domain gone, got \
         {status} {gone}"
    );
    assert_eq!(
        error_type.as_deref(),
        Some(NOT_FOUND),
        "case {case:?}: expected the modelled not-found code, got {error_type:?} {gone}"
    );
    assert_eq!(
        gone.get("message").and_then(Value::as_str),
        Some(NO_SUCH_DOMAIN_NAME),
        "case {case:?}: expected the service's own not-found message, got {gone}"
    );
}
