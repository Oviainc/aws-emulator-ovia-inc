// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the AWS SAM transform really creates the HTTP api
//! that transform synthesizes, in API Gateway v2.
//!
//! The rule under test: **the `AWS::Serverless-2016-10-31` transform gives every API it
//! emits the OpenAPI definition that names it — "if neither `DefinitionUri` nor
//! `DefinitionBody` are specified, AWS SAM generates [it] based on your template
//! configuration" — built from the routes of the function events routed through that API,
//! whether the API is the implicit one the macro adds or one the template declares, with
//! what the declaration (or the `Globals` section) says about the api itself carried inside
//! that document: so the `AWS::ApiGatewayV2::Api` the stack creates is IMPORTED from a
//! definition, and API Gateway v2 holds an api whose name, routes, Lambda proxy
//! integrations and CORS configuration are the ones the template asked for.** Three crates
//! have to agree for that to hold (CloudFormation expands the macro and makes the calls,
//! API Gateway v2 serves the import, Lambda and IAM answer for the function the routes
//! integrate), so the suite lives here rather than in any one of them, and it drives the
//! emulator's own assembled service set.
//!
//! The rows differ along what the rule generalises over: whether the API is implicit or
//! declared, how many routes and how many functions are routed through it, whether the
//! declaration carries a `DefinitionBody` of its own (whose own route and own title both
//! survive), whether it states a `Name`, whether CORS comes from the declaration or from
//! `Globals`, and the language the template is written in.
//!
//! Every expected value is computed here from what the row declares — the api's name is the
//! stack's own, which is what a generated definition's `info.title` carries — never read
//! back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, outputs_of, physical_ids_of};

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The `StackStatus` the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The emulator as it is ASSEMBLED, on one engine: the production service set
/// (`aws_emulator_router::build_router`), so what a row drives is the emulator a client
/// talks to rather than a hand-picked pair of services — CloudFormation expands the macro
/// and makes the calls, API Gateway v2 serves the import, and Lambda and IAM answer for the
/// function the generated routes integrate.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
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
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
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
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one API Gateway v2 operation over its restJson1 wire: that service signs as
/// `apigateway` and binds every operation under `/v2/…`.
async fn apigatewayv2(router: &MultiServiceRouter, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// One row of the table: the template it deploys and what the api the stack creates must
/// then be in API Gateway v2.
struct Row {
    case: &'static str,
    /// The logical id of the API resource the transform emits — the implicit one's, or the
    /// one the template declares.
    api: &'static str,
    /// The template body, built from the stack name it is deployed under.
    template: fn(&str) -> String,
    /// The api's name, where the row declares one; `None` for a row whose generated
    /// definition names the api after the stack.
    declared_name: Option<&'static str>,
    /// Every route key the api must hold, sorted.
    route_keys: &'static [&'static str],
    /// The `AllowOrigins` the api's CORS configuration must report, for a row that states
    /// one.
    allow_origins: &'static [&'static str],
    /// The function logical ids whose ARNs the api's Lambda proxy integrations must name.
    integrated_functions: &'static [&'static str],
}

/// The `AWS::Serverless::Function` declaration every row routes its events through, with
/// its `Events` map spelled by the row.
fn function(events: Value) -> Value {
    json!({
        "Type": "AWS::Serverless::Function",
        "Properties": {
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "InlineCode": "def handler(event, context):\n    return {'statusCode': 200}\n",
            "Events": events
        }
    })
}

/// One `HttpApi` event: the path and method it routes, and the API it names, if any.
fn http_event(path: &str, method: &str, api: Option<&str>) -> Value {
    let mut properties = json!({ "Path": path, "Method": method });
    if let (Some(api), Some(members)) = (api, properties.as_object_mut()) {
        members.insert("ApiId".to_string(), json!({ "Ref": api }));
    }
    json!({ "Type": "HttpApi", "Properties": properties })
}

const ROWS: &[Row] = &[
    Row {
        case: "an implicit HTTP api: one function with one HttpApi event that names no ApiId",
        api: "ServerlessHttpApi",
        template: |_| {
            json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Resources": {
                    "Greeter": function(json!({
                        "Hello": http_event("/hello", "get", None)
                    }))
                },
                "Outputs": {
                    "Endpoint": {
                        "Value": { "Fn::GetAtt": ["ServerlessHttpApi", "ApiEndpoint"] }
                    }
                }
            })
            .to_string()
        },
        declared_name: None,
        route_keys: &["GET /hello"],
        allow_origins: &[],
        integrated_functions: &["Greeter"],
    },
    Row {
        case: "two functions and three routes through the implicit HTTP api, with the CORS \
               configuration the Globals section states for every HTTP api",
        api: "ServerlessHttpApi",
        template: |_| {
            json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Globals": {
                    "Function": { "MemorySize": 192 },
                    "HttpApi": {
                        "CorsConfiguration": {
                            "AllowOrigins": ["https://orders.example.com"],
                            "AllowMethods": ["GET", "POST"]
                        }
                    }
                },
                "Resources": {
                    "Greeter": function(json!({
                        "Hello": http_event("/hello", "get", None),
                        "Bye": http_event("/bye/{id}", "delete", None)
                    })),
                    "Placer": function(json!({
                        "Place": http_event("/orders", "post", None)
                    }))
                },
                "Outputs": {
                    "Endpoint": {
                        "Value": { "Fn::GetAtt": ["ServerlessHttpApi", "ApiEndpoint"] }
                    }
                }
            })
            .to_string()
        },
        declared_name: None,
        route_keys: &["DELETE /bye/{id}", "GET /hello", "POST /orders"],
        allow_origins: &["https://orders.example.com"],
        integrated_functions: &["Greeter", "Placer"],
    },
    Row {
        case: "an AWS::Serverless::HttpApi the template declares, with no definition of its \
               own, that the function's event names",
        api: "Api",
        template: |_| {
            json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Globals": {
                    "HttpApi": { "CorsConfiguration": { "AllowOrigins": ["https://ui.test"] } }
                },
                "Resources": {
                    "Api": {
                        "Type": "AWS::Serverless::HttpApi",
                        "Properties": { "StageName": "$default" }
                    },
                    "Greeter": function(json!({
                        "Hello": http_event("/hello", "get", Some("Api"))
                    }))
                },
                "Outputs": {
                    "Endpoint": { "Value": { "Fn::GetAtt": ["Api", "ApiEndpoint"] } }
                }
            })
            .to_string()
        },
        declared_name: None,
        route_keys: &["GET /hello"],
        allow_origins: &["https://ui.test"],
        integrated_functions: &["Greeter"],
    },
    Row {
        case: "a declared HTTP api whose own DefinitionBody keeps its title and its route \
               while the event's route is added to it",
        api: "Api",
        template: |_| {
            json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Resources": {
                    "Api": {
                        "Type": "AWS::Serverless::HttpApi",
                        "Properties": {
                            "DefinitionBody": {
                                "openapi": "3.0.1",
                                "info": { "title": "written-by-the-template", "version": "2.0" },
                                "paths": {
                                    "/ping": {
                                        "get": {
                                            "x-amazon-apigateway-integration": {
                                                "type": "http_proxy",
                                                "httpMethod": "GET",
                                                "uri": "https://backend.test/ping",
                                                "payloadFormatVersion": "1.0"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    "Placer": function(json!({
                        "Place": http_event("/orders", "post", Some("Api"))
                    }))
                },
                "Outputs": {
                    "Endpoint": { "Value": { "Fn::GetAtt": ["Api", "ApiEndpoint"] } }
                }
            })
            .to_string()
        },
        declared_name: Some("written-by-the-template"),
        route_keys: &["GET /ping", "POST /orders"],
        allow_origins: &[],
        integrated_functions: &["Placer"],
    },
    Row {
        case: "the YAML spelling of a declared HTTP api that states a Name and a catch-all \
               method",
        api: "Api",
        template: |_| {
            [
                "Transform: AWS::Serverless-2016-10-31",
                "Resources:",
                "  Api:",
                "    Type: AWS::Serverless::HttpApi",
                "    Properties:",
                "      Name: the-named-api",
                "      Description: an api named by its declaration",
                "  Greeter:",
                "    Type: AWS::Serverless::Function",
                "    Properties:",
                "      Runtime: python3.12",
                "      Handler: index.handler",
                "      InlineCode: |",
                "        def handler(event, context):",
                "            return {'statusCode': 200}",
                "      Events:",
                "        Any:",
                "          Type: HttpApi",
                "          Properties:",
                "            Path: /{proxy+}",
                "            Method: any",
                "            ApiId: !Ref Api",
                "Outputs:",
                "  Endpoint:",
                "    Value: !GetAtt Api.ApiEndpoint",
                "",
            ]
            .join("\n")
        },
        declared_name: Some("the-named-api"),
        route_keys: &["ANY /{proxy+}"],
        allow_origins: &[],
        integrated_functions: &["Greeter"],
    },
    Row {
        case: "a parameterised template: the CORS origin and the function's name come from \
               Parameters, and Globals carries the function's memory size",
        api: "ServerlessHttpApi",
        template: |stack| {
            json!({
                "Transform": "AWS::Serverless-2016-10-31",
                "Parameters": {
                    "FunctionName": { "Type": "String", "Default": format!("{stack}-fn") },
                    "AllowedOrigin": { "Type": "String", "Default": "https://sim.example" }
                },
                "Globals": {
                    "Function": { "MemorySize": 192 },
                    "HttpApi": {
                        "CorsConfiguration": { "AllowOrigins": [{ "Ref": "AllowedOrigin" }] }
                    }
                },
                "Resources": {
                    "Greeter": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "FunctionName": { "Ref": "FunctionName" },
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "InlineCode": "def handler(event, context):\n    return {'statusCode': 200}\n",
                            "Events": { "Api": http_event("/hello", "get", None) }
                        }
                    }
                },
                "Outputs": {
                    "Endpoint": {
                        "Value": { "Fn::GetAtt": ["ServerlessHttpApi", "ApiEndpoint"] }
                    }
                }
            })
            .to_string()
        },
        declared_name: None,
        route_keys: &["GET /hello"],
        allow_origins: &["https://sim.example"],
        integrated_functions: &["Greeter"],
    },
];

/// The route keys an api holds, sorted — what `GetRoutes` reports.
async fn route_keys(router: &MultiServiceRouter, case: &str, api_id: &str) -> Vec<String> {
    let (status, answered) = apigatewayv2(router, &format!("/v2/apis/{api_id}/routes")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetRoutes for {api_id}, got {status} with body \
         {answered}"
    );
    let mut keys: Vec<String> = answered
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|route| route.get("routeKey").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    keys.sort();
    keys
}

/// The integrations an api holds, as `(IntegrationType, PayloadFormatVersion, IntegrationUri)`
/// triples sorted by the uri — what `GetIntegrations` reports.
async fn integrations(
    router: &MultiServiceRouter,
    case: &str,
    api_id: &str,
) -> Vec<(String, String, String)> {
    let (status, answered) =
        apigatewayv2(router, &format!("/v2/apis/{api_id}/integrations")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetIntegrations for {api_id}, got {status} with \
         body {answered}"
    );
    let mut found: Vec<(String, String, String)> = answered
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|integration| {
                    let member = |name: &str| {
                        integration
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (
                        member("integrationType"),
                        member("payloadFormatVersion"),
                        member("integrationUri"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Every stage name an api holds — what `GetStages` reports.
async fn stage_names(router: &MultiServiceRouter, case: &str, api_id: &str) -> Vec<String> {
    let (status, answered) = apigatewayv2(router, &format!("/v2/apis/{api_id}/stages")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetStages for {api_id}, got {status} with body \
         {answered}"
    );
    let mut names: Vec<String> = answered
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|stage| stage.get("stageName").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn a_sam_http_api_is_imported_from_the_definition_the_transform_generates() {
    let router = build_router();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("sam-http-api-{index}");
        let template = (row.template)(&stack);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &template),
                // A template that declares a macro is acknowledged with
                // CAPABILITY_AUTO_EXPAND, and the role the transform synthesizes for each
                // function with CAPABILITY_IAM.
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (status, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        let failures = if status == CREATE_COMPLETE {
            Vec::new()
        } else {
            let (_, events) =
                cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack)]).await;
            elements(&events, "ResourceStatusReason")
        };
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
             resource failures {failures:?}"
        );

        let (status, resources) =
            cloudformation(&router, "DescribeStackResources", &[("StackName", &stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical: BTreeMap<String, String> = physical_ids_of(&resources);
        let api_id = physical.get(row.api).cloned().unwrap_or_else(|| {
            panic!("case {case:?}: no physical id for {} in {resources}", row.api)
        });

        // The api the stack created is one API Gateway v2 HOLDS, and the name it holds it
        // under is the one the definition carried: the stack's own name for a generated
        // definition, the declared one where the row states it.
        let (status, api) = apigatewayv2(&router, &format!("/v2/apis/{api_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetApi for the api the stack published \
             ({api_id}), got {status} with body {api}"
        );
        let expected_name = row.declared_name.unwrap_or(stack.as_str());
        assert_eq!(
            api.get("name").and_then(Value::as_str),
            Some(expected_name),
            "case {case:?}: expected the api's name to be {expected_name:?}, got {api}"
        );
        assert_eq!(
            api.get("protocolType").and_then(Value::as_str),
            Some("HTTP"),
            "case {case:?}: expected an HTTP api, got {api}"
        );

        // Its routes are the ones the events (and the declared definition) state.
        let keys = route_keys(&router, case, &api_id).await;
        let expected: Vec<String> = row.route_keys.iter().map(ToString::to_string).collect();
        assert_eq!(
            keys, expected,
            "case {case:?}: expected the api to hold the routes {expected:?}, got {keys:?}"
        );

        // Each generated route integrates the function it was declared on, as a Lambda proxy
        // integration on the HTTP api's own payload format version.
        let held = integrations(&router, case, &api_id).await;
        for logical_id in row.integrated_functions {
            let arn = physical
                .get(*logical_id)
                .map(|name| format!("arn:aws:lambda:{REGION}:000000000000:function:{name}"))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no physical id for {logical_id} in {resources}")
                });
            let uri = format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{arn}/invocations"
            );
            assert!(
                held.contains(&("AWS_PROXY".to_string(), "2.0".to_string(), uri.clone())),
                "case {case:?}: expected a 2.0 AWS_PROXY integration on {uri:?}, got {held:?}"
            );
        }

        // The CORS configuration the declaration (or the Globals section) stated reached the
        // api through its definition.
        let origins: Vec<String> = api
            .get("corsConfiguration")
            .and_then(|cors| cors.get("allowOrigins"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let expected_origins: Vec<String> =
            row.allow_origins.iter().map(ToString::to_string).collect();
        assert_eq!(
            origins, expected_origins,
            "case {case:?}: expected the api's AllowOrigins to be {expected_origins:?}, got \
             {origins:?} in {api}"
        );

        // The api is deployed to the `$default` stage the transform adds for it, and the
        // endpoint the stack published is the one that service serves the api on.
        let stages = stage_names(&router, case, &api_id).await;
        assert_eq!(
            stages,
            vec!["$default".to_string()],
            "case {case:?}: expected the transform's $default stage, got {stages:?}"
        );
        let endpoint = api
            .get("apiEndpoint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            outputs_of(&described).get("Endpoint"),
            Some(&endpoint),
            "case {case:?}: expected the stack's Endpoint output to be the api's own \
             {endpoint:?}, got {described}"
        );
    }
}

/// The span this suite gives a stack to settle, for the one case that asserts a stack
/// NEVER reaches a status: long enough for the stack's own clock-owed transition.
const PAST_THE_STACK: Duration = Duration::from_millis(1_500);

/// An API the transform emits is never one the API Gateway v2 create refuses for a null
/// name: a declared `AWS::Serverless::HttpApi` that no event routes to and that states no
/// definition still deploys, because the transform generates the definition that names it.
#[tokio::test]
async fn a_declared_http_api_with_no_events_still_carries_the_name_its_definition_gives_it() {
    let router = build_router();
    let stack = "sam-http-api-no-events";
    let template = json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Api": { "Type": "AWS::Serverless::HttpApi", "Properties": { "StageName": "live" } }
        },
        "Outputs": { "ApiId": { "Value": { "Ref": "Api" } } }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK).await;

    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    let (_, events) = cloudformation(&router, "DescribeStackEvents", &[("StackName", stack)]).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "expected the stack to reach {CREATE_COMPLETE}, got {described} with resource failures \
         {:?}",
        elements(&events, "ResourceStatusReason")
    );

    let api_id = outputs_of(&described)
        .get("ApiId")
        .cloned()
        .unwrap_or_else(|| panic!("no ApiId output in {described}"));
    let (status, api) = apigatewayv2(&router, &format!("/v2/apis/{api_id}")).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from GetApi for {api_id}, got {status} with body {api}"
    );
    assert_eq!(
        api.get("name").and_then(Value::as_str),
        Some(stack),
        "expected the generated definition to name the api after the stack, got {api}"
    );
    let routes = route_keys(&router, "no events", &api_id).await;
    assert!(
        routes.is_empty(),
        "expected an api no event routes to to hold no route, got {routes:?}"
    );
    let stages = stage_names(&router, "no events", &api_id).await;
    assert_eq!(
        stages,
        vec!["live".to_string()],
        "expected the stage the declaration names, got {stages:?}"
    );
}
