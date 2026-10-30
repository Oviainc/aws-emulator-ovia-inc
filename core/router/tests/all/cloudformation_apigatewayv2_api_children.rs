// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack creates the resources that live INSIDE an `AWS::ApiGatewayV2::Api`
//! — its integrations, routes, stages, deployments and authorizers — in API Gateway v2, and
//! publishes for each of them the identifier that service assigned.
//!
//! The rule under test: **the physical resource id CloudFormation records for a resource —
//! what `Ref` resolves to, what `DescribeStackResources` reports, and what a later
//! resource's property referencing it is resolved to — is the identifier the service that
//! OWNS the type assigned when the stack created the resource there, never a name taken
//! from the declaration; and a stack that reaches `CREATE_COMPLETE` really has created every
//! declared resource of a type this emulator serves, so each published identifier resolves
//! through that service's own read.** Two crates have to agree for that to hold
//! (CloudFormation makes the calls, API Gateway v2 serves them), so the suite lives here
//! rather than in either of them.
//!
//! `cloudformation_apigatewayv2_resources.rs` owns the api, the custom domain name and the
//! api mapping; this suite owns the resources that hang off an api, which is what a deployed
//! api needs before it holds any route at all. Together they are every
//! `AWS::ApiGatewayV2::*` type whose create AND delete API Gateway v2 serves: an
//! `::IntegrationResponse`, a `::Model`, a `::RouteResponse` and a `::RoutingRule` are left
//! declared-only on purpose, because that service's own delete of each is still a 501 stub,
//! and a stack must not create a resource it could then not delete.
//!
//! The rows differ along what the rule generalises over: which of the types the template
//! declares, the api's `ProtocolType` (and so which integration types and route keys are
//! open to it), the FORM the template composes the integration reference in (`Fn::Join` and
//! `Fn::Sub`, the two spellings of `integrations/<IntegrationId>`), whether the identifier
//! is one that service mints (an integration, a route, a deployment, an authorizer) or the
//! name the template states (a stage), whether the route is authorized by an authorizer
//! resource, and whether the stage carries maps keyed by the caller's own data (route
//! settings keyed by route key, stage variables, request templates keyed by content type).
//! Every expected value is computed here from what the row declares or read from the owning
//! service, never from the answer being checked.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_apigatewayv2_resources::{
    apigatewayv2, build_router, cloudformation, create_stack, describe_stack,
    every_output_has_a_value, member, PAST_THE_STACK, REGION,
};
use crate::cloudformation_provisions_stack_resources::{xml_text, xml_texts};

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// How many characters an identifier API Gateway v2 mints for an API itself carries, and
/// the alphabet every minted id draws from: ten lowercase alphanumerics (`api_lifecycle.rs`
/// in that service's own suite pins it for `CreateApi`).
const API_ID_LEN: usize = 10;

/// The status a stack that created everything it declared reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The error code API Gateway v2 answers for a resource it does not hold, the modelled 404
/// of every read here.
const NOT_FOUND: &str = "NotFoundException";

/// One resource of a row: what the template declares, where its own read lives, and what
/// that read must report.
struct Expected {
    /// The logical id the template declares the resource under.
    logical_id: &'static str,
    /// The `Outputs` entry the stack publishes its `Ref` under.
    output: &'static str,
    /// The collection of its api the resource's own read is in, as that read's `@http` uri
    /// spells it (`GET /v2/apis/{ApiId}/<collection>/<id>`).
    collection: &'static str,
    /// Whether API Gateway v2 ASSIGNS this resource's identifier. Where it does, the value
    /// the stack publishes must be an id of that service's minting and nothing the template
    /// states; where it does not, it is the name the declaration asked for — which `name`
    /// carries.
    name: Option<String>,
    /// The members the resource's own read must report, each with the value the declaration
    /// asked for.
    members: Vec<(&'static str, Value)>,
    /// The output naming the INTEGRATION this route is wired to, where the resource is a
    /// route: its `Target` must be `integrations/<that integration's own id>`, which is the
    /// only form that service resolves.
    target_integration: Option<&'static str>,
    /// The output naming the AUTHORIZER this route hands its authorization to, where it
    /// does: its `AuthorizerId` must be the id that service minted for it.
    authorizer: Option<&'static str>,
}

impl Expected {
    /// One resource whose identifier API Gateway v2 assigns.
    fn minted(logical_id: &'static str, output: &'static str, collection: &'static str) -> Self {
        Self {
            logical_id,
            output,
            collection,
            name: None,
            members: Vec::new(),
            target_integration: None,
            authorizer: None,
        }
    }

    /// One resource created under a name the template states, which is its identifier.
    fn named(
        logical_id: &'static str,
        output: &'static str,
        collection: &'static str,
        name: &str,
    ) -> Self {
        Self {
            name: Some(name.to_string()),
            ..Self::minted(logical_id, output, collection)
        }
    }

    fn reporting(mut self, members: Vec<(&'static str, Value)>) -> Self {
        self.members = members;
        self
    }

    fn wired_to(mut self, integration_output: &'static str) -> Self {
        self.target_integration = Some(integration_output);
        self
    }

    fn authorized_by(mut self, authorizer_output: &'static str) -> Self {
        self.authorizer = Some(authorizer_output);
        self
    }
}

/// One row: the template a stack is created from, and what that stack must then have
/// created in API Gateway v2.
struct Row {
    case: &'static str,
    /// The stack name the row runs under, which every declared name is built from so two
    /// rows never collide.
    stack: &'static str,
    /// The template body, built from that stack name.
    template: fn(&str) -> String,
    /// The resources the row declares inside its api.
    expected: fn(&str) -> Vec<Expected>,
}

/// The ARN of the function a Lambda proxy integration invokes. A literal: this suite is
/// about the integration record API Gateway v2 holds, and that service takes the URI as the
/// configuration it reports back.
fn function_arn(stack: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{stack}-handler")
}

/// An HTTP api serving one route through an HTTP proxy integration, with a `$default` stage
/// over it and a deployment released to that stage — the shape a hand-written template takes,
/// with the integration reference composed by `Fn::Join`.
fn http_proxy_template(stack: &str) -> String {
    json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": { "Name": format!("{stack}-api"), "ProtocolType": "HTTP" }
            },
            "Integration": {
                "Type": "AWS::ApiGatewayV2::Integration",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "IntegrationType": "HTTP_PROXY",
                    "IntegrationMethod": "GET",
                    "IntegrationUri": "https://upstream.example.com/items",
                    "PayloadFormatVersion": "1.0",
                    "TimeoutInMillis": 5000
                }
            },
            "Route": {
                "Type": "AWS::ApiGatewayV2::Route",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "RouteKey": "GET /items",
                    "Target": { "Fn::Join": ["/", ["integrations", { "Ref": "Integration" }]] }
                }
            },
            "Stage": {
                "Type": "AWS::ApiGatewayV2::Stage",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "StageName": "$default",
                    "AutoDeploy": true,
                    "Description": "the default stage"
                }
            },
            "Deployment": {
                "Type": "AWS::ApiGatewayV2::Deployment",
                "DependsOn": ["Route"],
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "StageName": { "Ref": "Stage" },
                    "Description": "the first deployment"
                }
            }
        },
        "Outputs": {
            "ApiRef": { "Value": { "Ref": "Api" } },
            "IntegrationRef": { "Value": { "Ref": "Integration" } },
            "RouteRef": { "Value": { "Ref": "Route" } },
            "StageRef": { "Value": { "Ref": "Stage" } },
            "DeploymentRef": { "Value": { "Ref": "Deployment" } }
        }
    })
    .to_string()
}

fn http_proxy_expected(_stack: &str) -> Vec<Expected> {
    vec![
        Expected::minted("Integration", "IntegrationRef", "integrations").reporting(vec![
            ("integrationType", json!("HTTP_PROXY")),
            ("integrationMethod", json!("GET")),
            ("integrationUri", json!("https://upstream.example.com/items")),
            ("payloadFormatVersion", json!("1.0")),
            ("timeoutInMillis", json!(5000)),
        ]),
        Expected::minted("Route", "RouteRef", "routes")
            .reporting(vec![("routeKey", json!("GET /items"))])
            .wired_to("IntegrationRef"),
        Expected::named("Stage", "StageRef", "stages", "$default").reporting(vec![
            ("stageName", json!("$default")),
            ("autoDeploy", json!(true)),
            ("description", json!("the default stage")),
        ]),
        Expected::minted("Deployment", "DeploymentRef", "deployments")
            .reporting(vec![("description", json!("the first deployment"))]),
    ]
}

/// An HTTP api of the shape a CDK-synthesised stack takes: CORS on the api, a Lambda proxy
/// integration, a `$default` route authorized by a JWT authorizer of the same stack, and a
/// NAMED stage carrying maps keyed by the caller's own data. The integration reference is
/// composed by `Fn::Sub` here, the other spelling of the same value.
fn cdk_shaped_template(stack: &str) -> String {
    json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": {
                    "Name": format!("{stack}-api"),
                    "ProtocolType": "HTTP",
                    "CorsConfiguration": {
                        "AllowOrigins": ["https://app.example.com"],
                        "AllowMethods": ["GET", "POST"],
                        "AllowHeaders": ["authorization"],
                        "MaxAge": 300
                    }
                }
            },
            "Authorizer": {
                "Type": "AWS::ApiGatewayV2::Authorizer",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "Name": format!("{stack}-jwt"),
                    "AuthorizerType": "JWT",
                    "IdentitySource": ["$request.header.Authorization"],
                    "JwtConfiguration": {
                        "Audience": ["an-audience"],
                        "Issuer": "https://issuer.example.com/"
                    }
                }
            },
            "Integration": {
                "Type": "AWS::ApiGatewayV2::Integration",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "IntegrationType": "AWS_PROXY",
                    "IntegrationUri": function_arn(stack),
                    "PayloadFormatVersion": "2.0"
                }
            },
            "Route": {
                "Type": "AWS::ApiGatewayV2::Route",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "RouteKey": "$default",
                    "Target": { "Fn::Sub": "integrations/${Integration}" },
                    "AuthorizationType": "JWT",
                    "AuthorizerId": { "Ref": "Authorizer" }
                }
            },
            "Stage": {
                "Type": "AWS::ApiGatewayV2::Stage",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "StageName": "prod",
                    "AutoDeploy": true,
                    "StageVariables": { "TableName": "orders" },
                    "RouteSettings": {
                        "$default": {
                            "DetailedMetricsEnabled": true,
                            "ThrottlingBurstLimit": 100
                        }
                    }
                }
            }
        },
        "Outputs": {
            "ApiRef": { "Value": { "Ref": "Api" } },
            "AuthorizerRef": { "Value": { "Ref": "Authorizer" } },
            "IntegrationRef": { "Value": { "Ref": "Integration" } },
            "RouteRef": { "Value": { "Ref": "Route" } },
            "StageRef": { "Value": { "Ref": "Stage" } }
        }
    })
    .to_string()
}

fn cdk_shaped_expected(stack: &str) -> Vec<Expected> {
    vec![
        Expected::minted("Authorizer", "AuthorizerRef", "authorizers").reporting(vec![
            ("name", json!(format!("{stack}-jwt"))),
            ("authorizerType", json!("JWT")),
            ("identitySource", json!(["$request.header.Authorization"])),
            (
                "jwtConfiguration",
                json!({"audience": ["an-audience"], "issuer": "https://issuer.example.com/"}),
            ),
        ]),
        Expected::minted("Integration", "IntegrationRef", "integrations").reporting(vec![
            ("integrationType", json!("AWS_PROXY")),
            ("integrationUri", json!(function_arn(stack))),
            ("payloadFormatVersion", json!("2.0")),
        ]),
        Expected::minted("Route", "RouteRef", "routes")
            .reporting(vec![
                ("routeKey", json!("$default")),
                ("authorizationType", json!("JWT")),
            ])
            .wired_to("IntegrationRef")
            .authorized_by("AuthorizerRef"),
        // The maps keyed by the caller's own data keep those keys exactly as the template
        // spells them — a route key is not a member name to re-case.
        Expected::named("Stage", "StageRef", "stages", "prod").reporting(vec![
            ("stageName", json!("prod")),
            ("stageVariables", json!({"TableName": "orders"})),
            (
                "routeSettings",
                json!({"$default": {"detailedMetricsEnabled": true, "throttlingBurstLimit": 100}}),
            ),
        ]),
    ]
}

/// A WebSocket api, whose protocol opens integration types and route keys an HTTP api has
/// none of: a `MOCK` integration carrying request templates keyed by content type, behind
/// the `$connect` route.
fn websocket_template(stack: &str) -> String {
    json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": {
                    "Name": format!("{stack}-api"),
                    "ProtocolType": "WEBSOCKET",
                    "RouteSelectionExpression": "$request.body.action"
                }
            },
            "Integration": {
                "Type": "AWS::ApiGatewayV2::Integration",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "IntegrationType": "MOCK",
                    "RequestTemplates": { "application/json": "{\"statusCode\": 200}" }
                }
            },
            "Route": {
                "Type": "AWS::ApiGatewayV2::Route",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "RouteKey": "$connect",
                    "Target": { "Fn::Join": ["/", ["integrations", { "Ref": "Integration" }]] }
                }
            },
            "Stage": {
                "Type": "AWS::ApiGatewayV2::Stage",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "StageName": "dev",
                    "AutoDeploy": true
                }
            }
        },
        "Outputs": {
            "ApiRef": { "Value": { "Ref": "Api" } },
            "IntegrationRef": { "Value": { "Ref": "Integration" } },
            "RouteRef": { "Value": { "Ref": "Route" } },
            "StageRef": { "Value": { "Ref": "Stage" } }
        }
    })
    .to_string()
}

fn websocket_expected(_stack: &str) -> Vec<Expected> {
    vec![
        Expected::minted("Integration", "IntegrationRef", "integrations").reporting(vec![
            ("integrationType", json!("MOCK")),
            (
                "requestTemplates",
                json!({"application/json": "{\"statusCode\": 200}"}),
            ),
        ]),
        Expected::minted("Route", "RouteRef", "routes")
            .reporting(vec![("routeKey", json!("$connect"))])
            .wired_to("IntegrationRef"),
        Expected::named("Stage", "StageRef", "stages", "dev")
            .reporting(vec![("stageName", json!("dev"))]),
    ]
}

const ROWS: &[Row] = &[
    Row {
        case: "an HTTP api with an HTTP proxy integration, a route, a stage and a deployment",
        stack: "cfn-apigwv2-http-proxy",
        template: http_proxy_template,
        expected: http_proxy_expected,
    },
    Row {
        case: "a CDK-shaped HTTP api with CORS, a Lambda integration, a JWT authorizer and a \
               named stage",
        stack: "cfn-apigwv2-cdk-shaped",
        template: cdk_shaped_template,
        expected: cdk_shaped_expected,
    },
    Row {
        case: "a WebSocket api with a MOCK integration behind $connect",
        stack: "cfn-apigwv2-websocket",
        template: websocket_template,
        expected: websocket_expected,
    },
];

/// How many characters the identifier of a resource nested INSIDE an api carries, by the
/// collection it lives in: the short opaque form API Gateway v2 answers for those
/// (`minted_id_shape.rs` in that service's own suite pins the shape per kind).
fn nested_id_len(collection: &str) -> usize {
    match collection {
        "routes" | "integrations" => 7,
        "authorizers" | "deployments" | "models" => 6,
        other => panic!("this suite knows no minted-id length for the {other:?} collection"),
    }
}

/// Whether `value` is an identifier of the shape API Gateway v2 mints one in, `expected_len`
/// characters long.
fn is_minted_id(value: &str, expected_len: usize) -> bool {
    value.chars().count() == expected_len
        && value
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

/// The `(logical id, physical id)` pairs a stack reports, read as the two lists
/// `DescribeStackResources` carries in document order.
async fn reported_resources(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> BTreeMap<String, String> {
    let (status, body) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {body}"
    );
    let logical = xml_texts(&body, "LogicalResourceId");
    let physical = xml_texts(&body, "PhysicalResourceId");
    assert_eq!(
        logical.len(),
        physical.len(),
        "case {case:?}: every reported resource must carry a physical id, got {logical:?} \
         against {physical:?}"
    );
    logical.into_iter().zip(physical).collect()
}

/// Read one resource of an api back through API Gateway v2's own read.
async fn read_child(
    router: &MultiServiceRouter,
    api_id: &str,
    collection: &str,
    identifier: &str,
) -> (u16, Value) {
    apigatewayv2(
        router,
        "GET",
        &format!("/v2/apis/{api_id}/{collection}/{identifier}"),
        None,
    )
    .await
}

#[tokio::test]
async fn a_stack_creates_the_resources_that_live_inside_the_api_it_declares() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let template = (row.template)(row.stack);
        create_stack(&router, case, row.stack, &template).await;
        tokio::time::sleep(PAST_THE_STACK).await;

        let described = describe_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(CREATE_COMPLETE),
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got body {described}"
        );
        let outputs = every_output_has_a_value(case, &described);
        let reported = reported_resources(&router, case, row.stack).await;

        let api_id = outputs
            .get("ApiRef")
            .unwrap_or_else(|| panic!("case {case:?}: expected an ApiRef output in {outputs:?}"))
            .clone();
        assert!(
            is_minted_id(&api_id, API_ID_LEN),
            "case {case:?}: expected the api's Ref to be an id API Gateway v2 minted, got \
             {api_id:?}"
        );

        for expected in (row.expected)(row.stack) {
            let logical_id = expected.logical_id;
            let published = outputs.get(expected.output).unwrap_or_else(|| {
                panic!(
                    "case {case:?}/{logical_id}: expected an {} output in {outputs:?}",
                    expected.output
                )
            });

            // The identifier the stack publishes for the resource IS the identifier the
            // owning service knows it by: the one it assigned, or — where CloudFormation
            // decides the name — the name the declaration asked for.
            match &expected.name {
                None => {
                    assert!(
                        is_minted_id(published, nested_id_len(expected.collection)),
                        "case {case:?}/{logical_id}: expected the Ref to be an id API Gateway \
                         v2 minted, got {published:?}"
                    );
                    assert!(
                        !published.contains(row.stack) && published != logical_id,
                        "case {case:?}/{logical_id}: expected the Ref not to be a name built \
                         from the template, got {published:?}"
                    );
                }
                Some(name) => assert_eq!(
                    published, name,
                    "case {case:?}/{logical_id}: expected the Ref to be the declared name \
                     {name:?}, got {published:?}"
                ),
            }

            // What the stack REPORTS for the resource is the same identifier, so a client
            // reading the stack rather than its outputs gets a usable handle too.
            assert_eq!(
                reported.get(logical_id),
                Some(published),
                "case {case:?}/{logical_id}: expected DescribeStackResources to report \
                 {published:?}, got {:?}",
                reported.get(logical_id)
            );

            // And the resource really is there, under that identifier, configured as the
            // template declared it.
            let (status, answered) =
                read_child(&router, &api_id, expected.collection, published).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{logical_id}: expected 200 reading {}/{published} back, got \
                 {status} {answered}",
                expected.collection
            );
            for (name, value) in &expected.members {
                assert_eq!(
                    answered.get(name),
                    Some(value),
                    "case {case:?}/{logical_id}: expected the read to report {name} as \
                     {value}, got {answered}"
                );
            }

            // A route reaches its integration by the id that service minted for it, in the
            // `integrations/<IntegrationId>` form it resolves — whichever intrinsic the
            // template composed that value with.
            if let Some(integration_output) = expected.target_integration {
                let integration_id = outputs.get(integration_output).unwrap_or_else(|| {
                    panic!("case {case:?}/{logical_id}: expected an {integration_output} output")
                });
                assert_eq!(
                    member(case, &answered, "target"),
                    format!("integrations/{integration_id}"),
                    "case {case:?}/{logical_id}: expected the route to target the integration \
                     the stack created, got {answered}"
                );
            }
            if let Some(authorizer_output) = expected.authorizer {
                let authorizer_id = outputs.get(authorizer_output).unwrap_or_else(|| {
                    panic!("case {case:?}/{logical_id}: expected an {authorizer_output} output")
                });
                assert_eq!(
                    member(case, &answered, "authorizerId"),
                    *authorizer_id,
                    "case {case:?}/{logical_id}: expected the route to name the authorizer the \
                     stack created, got {answered}"
                );
            }
        }

        // A READ changes nothing: the identifiers a second DescribeStackResources reports
        // are the ones the first did.
        assert_eq!(
            reported_resources(&router, case, row.stack).await,
            reported,
            "case {case:?}: the reported identifiers must be stable across reads"
        );
    }
}

/// The definition an imported api is declared from: an OpenAPI 3 document whose paths carry
/// the API Gateway integration extension, which is what makes an imported api serve
/// anything.
fn inline_definition(stack: &str) -> Value {
    json!({
        "openapi": "3.0.1",
        "info": { "title": format!("{stack}-imported"), "version": "1.0" },
        "paths": {
            "/items": {
                "get": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "GET",
                        "uri": "https://upstream.example.com/items",
                        "payloadFormatVersion": "1.0"
                    }
                }
            },
            "/{proxy+}": {
                "x-amazon-apigateway-any-method": {
                    "x-amazon-apigateway-integration": {
                        "type": "http_proxy",
                        "httpMethod": "ANY",
                        "uri": "https://upstream.example.com/",
                        "payloadFormatVersion": "1.0"
                    }
                }
            }
        }
    })
}

/// An `AWS::ApiGatewayV2::Api` declared with an inline OpenAPI definition is IMPORTED from
/// it: "to import an HTTP API, you must specify a Body or BodyS3Location", and the api the
/// stack publishes is the one that import minted — readable, and holding the routes the
/// definition declared.
#[tokio::test]
async fn an_api_declared_with_an_inline_definition_is_imported_and_readable() {
    let router = build_router();
    let case = "an api imported from an inline OpenAPI definition";
    let stack = "cfn-apigwv2-body-import";
    let definition = inline_definition(stack);
    let template = json!({
        "Resources": {
            // No `Name` and no `ProtocolType`: "if you specify a Body or BodyS3Location,
            // don't specify ... Name, ProtocolType ..." — the definition carries both, so
            // the physical id of this resource is one nothing in the template states.
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": { "Body": definition.clone(), "FailOnWarnings": true }
            },
            "Stage": {
                "Type": "AWS::ApiGatewayV2::Stage",
                "Properties": {
                    "ApiId": { "Ref": "Api" },
                    "StageName": "$default",
                    "AutoDeploy": true
                }
            }
        },
        "Outputs": {
            "ApiRef": { "Value": { "Ref": "Api" } },
            "ApiEndpoint": { "Value": { "Fn::GetAtt": ["Api", "ApiEndpoint"] } },
            "StageRef": { "Value": { "Ref": "Stage" } }
        }
    })
    .to_string();

    create_stack(&router, case, stack, &template).await;
    tokio::time::sleep(PAST_THE_STACK).await;

    let described = describe_stack(&router, case, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got body {described}"
    );
    let outputs = every_output_has_a_value(case, &described);
    let api_id = outputs
        .get("ApiRef")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ApiRef output in {outputs:?}"))
        .clone();
    assert!(
        is_minted_id(&api_id, API_ID_LEN),
        "case {case:?}: expected the api's Ref to be an id API Gateway v2 minted, got {api_id:?}"
    );

    // The api is there, named by its DEFINITION — the import is what decided the name, so a
    // stack that published the name instead could not have imported anything.
    let (status, api) = apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetApi for the published ApiId {api_id:?}, got \
         {status} {api}"
    );
    assert_eq!(
        member(case, &api, "name"),
        format!("{stack}-imported"),
        "case {case:?}: expected the api to carry the name its definition states, got {api}"
    );
    assert_eq!(
        member(case, &api, "protocolType"),
        "HTTP",
        "case {case:?}: expected an imported api to be an HTTP api, got {api}"
    );
    assert_eq!(
        outputs.get("ApiEndpoint").map(String::as_str),
        Some(member(case, &api, "apiEndpoint").as_str()),
        "case {case:?}: expected the published endpoint to be the one API Gateway v2 serves \
         the api on, got {outputs:?} against {api}"
    );

    // And it holds the routes the definition declared, each wired to an integration of the
    // same api — which is what an import is for.
    let (status, routes) =
        apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetRoutes, got {status} {routes}"
    );
    let keys: Vec<String> = routes
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|route| member(case, route, "routeKey"))
        .collect();
    for declared in ["GET /items", "ANY /{proxy+}"] {
        assert!(
            keys.iter().any(|key| key == declared),
            "case {case:?}: expected the imported api to hold the route {declared:?} its \
             definition declares, got {keys:?}"
        );
    }

    // The stage the template declared over that imported api is there too, under the name it
    // states.
    let (status, stage) = read_child(&router, &api_id, "stages", "$default").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 reading the declared stage back, got {status} {stage}"
    );
}

/// A definition held in Amazon S3 names an object that has to be FETCHED before any api can
/// be imported from it, which nothing here does — so the declaration is refused and the
/// stack rolls back, rather than completing with an api configured differently from what it
/// declared.
#[tokio::test]
async fn an_api_declared_with_a_definition_in_s3_is_refused() {
    let router = build_router();
    let case = "an api whose definition is an Amazon S3 object";
    let stack = "cfn-apigwv2-body-s3";
    let template = json!({
        "Resources": {
            "Api": {
                "Type": "AWS::ApiGatewayV2::Api",
                "Properties": {
                    "BodyS3Location": { "Bucket": "a-bucket", "Key": "openapi.json" }
                }
            }
        }
    })
    .to_string();

    create_stack(&router, case, stack, &template).await;
    tokio::time::sleep(PAST_THE_STACK).await;

    let described = describe_stack(&router, case, stack).await;
    let status = xml_text(&described, "StackStatus").unwrap_or_default();
    assert!(
        status.contains("ROLLBACK") || status == "CREATE_FAILED",
        "case {case:?}: expected the stack not to complete, got {status:?} in {described}"
    );
}

/// Deleting the stack deletes every resource it created inside the api, each addressed by
/// the identifier the stack holds for it — which is how a client can tell the stack really
/// owned them.
#[tokio::test]
async fn deleting_the_stack_deletes_the_resources_inside_the_api() {
    let router = build_router();
    let case = "a deleted stack whose api held an integration, a route, a stage and an authorizer";
    let stack = "cfn-apigwv2-children-deleted";
    let template = (ROWS[1].template)(stack);

    create_stack(&router, case, stack, &template).await;
    tokio::time::sleep(PAST_THE_STACK).await;
    let described = describe_stack(&router, case, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(CREATE_COMPLETE),
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got body {described}"
    );
    let outputs = every_output_has_a_value(case, &described);
    let api_id = outputs
        .get("ApiRef")
        .unwrap_or_else(|| panic!("case {case:?}: expected an ApiRef output in {outputs:?}"))
        .clone();

    // Each resource is there before the delete, so what the delete changes is unambiguous.
    let addressed: Vec<(&str, String)> = vec![
        ("integrations", outputs["IntegrationRef"].clone()),
        ("routes", outputs["RouteRef"].clone()),
        ("authorizers", outputs["AuthorizerRef"].clone()),
        ("stages", outputs["StageRef"].clone()),
    ];
    for (collection, identifier) in &addressed {
        let (status, body) = read_child(&router, &api_id, collection, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading {collection}/{identifier} before the delete, \
             got {status} {body}"
        );
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK).await;

    for (collection, identifier) in &addressed {
        let (status, body) = read_child(&router, &api_id, collection, identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted {collection}/{identifier} to be gone, got \
             {status} {body}"
        );
        assert_eq!(
            body.get("__type")
                .or_else(|| body.get("code"))
                .and_then(Value::as_str)
                .map(|code| code.rsplit('#').next().unwrap_or(code).to_string()),
            Some(NOT_FOUND.to_string()),
            "case {case:?}: expected {NOT_FOUND} for the deleted {collection}/{identifier}, \
             got {body}"
        );
    }
    let (status, api) = apigatewayv2(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the deleted api to be gone, got {status} {api}"
    );
}
