// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares a REST API really creates it in Amazon API Gateway,
//! and the stage the stack deployed SERVES the integration the template declared.
//!
//! The rule under test: **CloudFormation creates every resource a template declares in the
//! service that OWNS that resource type, through that service's own public API, and the
//! identity in force is the one that service assigned — so a REST API, its resources, its
//! methods, the deployment of them and the stage that serves it exist in API Gateway under
//! the ids it minted, and a request to the deployed stage's invoke URL is answered by that
//! api's data plane rather than by the emulator's generic AWS-API front door.** Five crates
//! have to agree for that to hold (CloudFormation expands the macro and makes the calls, API
//! Gateway serves the import and then the invoke, Lambda runs the function behind the
//! integration, IAM answers for the role the transform synthesized and DynamoDB serves the
//! table the handler writes), so the suite lives here rather than in any one of them, and it
//! drives the emulator's own assembled service set.
//!
//! The rows differ along what the rule generalises over: whether the api comes from the AWS
//! SAM transform's IMPLICIT api, from an `AWS::Serverless::Api` the template declares, or
//! from `AWS::ApiGateway::*` resources written out one by one; whether it is created by
//! IMPORTING an OpenAPI definition or by `CreateRestApi` with a resource and a method put on
//! it afterwards; the stage's name; the verb and the path depth of the method; and whether
//! the request carries a body.
//!
//! Every expected value is computed here from what the row declares — the api id and the
//! invoke URL come out of the stack's own outputs, which is what a client deploying the
//! stack reads — never out of the answer being checked.

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

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long to wait between polls of a function's state, and how many polls to make.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The emulator as it is ASSEMBLED, on one engine (`aws_emulator_router::build_router`): the
/// api CloudFormation creates is the api this suite then invokes, and the function behind its
/// integration is the one AWS Lambda runs.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from.
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

/// Read one API Gateway control-plane resource over its restJson1 wire: that service signs as
/// `apigateway` and binds every REST operation under `/restapis/…`.
async fn apigateway(router: &MultiServiceRouter, uri: &str) -> (u16, Value) {
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

/// Invoke a deployed stage at the invoke URL the stack published, exactly as a client reaches
/// AWS: the request's host is the api's own `{rest-api-id}.execute-api.{region}.amazonaws.com`
/// endpoint and its path is the stage and the resource path.
///
/// The URL is taken apart rather than rebuilt here, so what is driven is the value the stack
/// told the caller to use.
async fn invoke(
    router: &MultiServiceRouter,
    invoke_url: &str,
    method: &str,
    body: &str,
) -> (u16, String) {
    let without_scheme = invoke_url
        .strip_prefix("https://")
        .or_else(|| invoke_url.strip_prefix("http://"))
        .unwrap_or(invoke_url);
    let (host, path) = match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One Lambda `restJson1` read, for the wait below.
async fn lambda(router: &MultiServiceRouter, uri: &str) -> Value {
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
///
/// The span a function spends `Pending` is that service's own, so no fixed sleep names the
/// instant it ends; an invoke of the stage before then is an invoke of a function Lambda is
/// still bringing up.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLLS {
        let configuration =
            lambda(router, &format!("/2015-03-31/functions/{name}/configuration")).await;
        let state = configuration
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if state == "Active" {
            return state;
        }
        last = state;
        tokio::time::sleep(POLL).await;
    }
    last
}

/// The handler every row's function runs: a Lambda proxy integration's own answer shape,
/// carrying what the event said about the request so the response proves the integration was
/// really reached.
const PROXY_HANDLER: &str = r#"
import json


def handler(event, context):
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json"},
        "body": json.dumps(
            {
                "path": event.get("path"),
                "method": event.get("httpMethod"),
                "stage": (event.get("requestContext") or {}).get("stage"),
                "body": event.get("body"),
            }
        ),
    }
"#;

/// One row of the table: the template it deploys and what must then be true of the api in
/// API Gateway and of its deployed stage.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name it is deployed under.
    template: fn(&str) -> String,
    /// The logical id of the function the api's integration invokes, whose physical id is the
    /// function name this suite waits on.
    function: &'static str,
    /// The stage the api is deployed to.
    stage: &'static str,
    /// The resource path the method is on, as `Resource$path` spells one.
    path: &'static str,
    /// The verb the method answers, and the verb the invoke is made with.
    method: &'static str,
    /// The body the invoke carries.
    body: &'static str,
}

/// The `AWS::Serverless::Function` declaration a SAM row routes its api event through.
fn serverless_function(events: Value) -> Value {
    json!({
        "Type": "AWS::Serverless::Function",
        "Properties": {
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "InlineCode": PROXY_HANDLER,
            "Timeout": 30,
            "Events": events
        }
    })
}

/// A template whose api is the one the SAM transform synthesizes for an `Api` event that
/// names no `RestApiId`: the implicit `ServerlessRestApi`, deployed to its implicit stage.
fn implicit_api(_stack: &str) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Items": serverless_function(json!({
                "Post": { "Type": "Api", "Properties": { "Path": "/items", "Method": "post" } }
            }))
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "ServerlessRestApi" } },
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub": "https://${ServerlessRestApi}.execute-api.${AWS::Region}.amazonaws.com/Prod/items"
                }
            }
        }
    })
    .to_string()
}

/// A template whose api is an `AWS::Serverless::Api` the template DECLARES, with its own
/// stage name, and whose event routes a nested path through it.
fn declared_api(_stack: &str) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Orders": serverless_function(json!({
                "Get": {
                    "Type": "Api",
                    "Properties": {
                        "Path": "/orders/open",
                        "Method": "get",
                        "RestApiId": { "Ref": "Api" }
                    }
                }
            })),
            "Api": {
                "Type": "AWS::Serverless::Api",
                "Properties": {
                    "StageName": "beta",
                    "Name": "declared-orders-api",
                    "Variables": { "Backend": "primary" }
                }
            }
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub": "https://${Api}.execute-api.${AWS::Region}.amazonaws.com/beta/orders/open"
                }
            }
        }
    })
    .to_string()
}

/// A template that writes the api out one `AWS::ApiGateway::*` resource at a time: the api
/// itself with no definition, a resource under its root, a method with a Lambda proxy
/// integration on it, a deployment of them and a stage serving that deployment.
fn written_out_api(stack: &str) -> String {
    json!({
        "Resources": {
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "lambda.amazonaws.com" },
                            "Action": "sts:AssumeRole"
                        }]
                    }
                }
            },
            "Fn": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                    "Code": { "ZipFile": PROXY_HANDLER }
                }
            },
            "Api": {
                "Type": "AWS::ApiGateway::RestApi",
                "Properties": {
                    "Name": format!("{stack}-written-out"),
                    "Description": "written out resource by resource",
                    "ApiKeySourceType": "HEADER",
                    "EndpointConfiguration": { "Types": ["REGIONAL"] }
                }
            },
            "Reports": {
                "Type": "AWS::ApiGateway::Resource",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ParentId": { "Fn::GetAtt": ["Api", "RootResourceId"] },
                    "PathPart": "reports"
                }
            },
            "Method": {
                "Type": "AWS::ApiGateway::Method",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "ResourceId": { "Ref": "Reports" },
                    "HttpMethod": "PUT",
                    "AuthorizationType": "NONE",
                    "OperationName": "putReport",
                    "MethodResponses": [{ "StatusCode": "200" }],
                    "Integration": {
                        "Type": "AWS_PROXY",
                        "IntegrationHttpMethod": "POST",
                        "Uri": {
                            "Fn::Sub": "arn:aws:apigateway:${AWS::Region}:lambda:path/2015-03-31/functions/${Fn.Arn}/invocations"
                        }
                    }
                }
            },
            "Permission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": ["Fn", "Arn"] },
                    "Principal": "apigateway.amazonaws.com"
                }
            },
            "Deployment": {
                "Type": "AWS::ApiGateway::Deployment",
                "DependsOn": ["Method"],
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "Description": "the first deployment"
                }
            },
            "Stage": {
                "Type": "AWS::ApiGateway::Stage",
                "Properties": {
                    "RestApiId": { "Ref": "Api" },
                    "DeploymentId": { "Ref": "Deployment" },
                    "StageName": "live",
                    "Description": "the live stage",
                    "Variables": { "Backend": "primary" },
                    "MethodSettings": [{
                        "ResourcePath": "/reports",
                        "HttpMethod": "PUT",
                        "MetricsEnabled": true
                    }]
                }
            }
        },
        "Outputs": {
            "ApiId": { "Value": { "Ref": "Api" } },
            "InvokeUrl": {
                "Value": {
                    "Fn::Sub": "https://${Api}.execute-api.${AWS::Region}.amazonaws.com/live/reports"
                }
            }
        }
    })
    .to_string()
}

const ROWS: &[Row] = &[
    Row {
        case: "the implicit api the SAM transform synthesizes, deployed to its implicit stage",
        template: implicit_api,
        function: "Items",
        stage: "Prod",
        path: "/items",
        method: "POST",
        body: r#"{"id":"one"}"#,
    },
    Row {
        case: "an AWS::Serverless::Api the template declares, on its own stage name",
        template: declared_api,
        function: "Orders",
        stage: "beta",
        path: "/orders/open",
        method: "GET",
        body: "",
    },
    Row {
        case: "an api written out as AWS::ApiGateway::* resources, one call each",
        template: written_out_api,
        function: "Fn",
        stage: "live",
        path: "/reports",
        method: "PUT",
        body: r#"{"report":"two"}"#,
    },
];

#[tokio::test]
async fn a_stack_s_rest_api_is_created_in_api_gateway_and_its_stage_serves() {
    let router = build_router();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("rest-api-stage-{index}");
        let template = (row.template)(&stack);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &template),
                // A template that declares a macro is acknowledged with
                // CAPABILITY_AUTO_EXPAND, and the role a function needs with CAPABILITY_IAM.
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

        let outputs = outputs_of(&described);
        let api_id = outputs.get("ApiId").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no ApiId output in {described}")
        });
        let invoke_url = outputs.get("InvokeUrl").cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no InvokeUrl output in {described}")
        });

        // The id the stack published is one API Gateway HOLDS an api under: the stack's own
        // records are not evidence of anything, the owning service's read is.
        let (status, api) = apigateway(&router, &format!("/restapis/{api_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRestApi for the api the stack published \
             ({api_id}), got {status} with body {api}"
        );
        assert_eq!(
            api.get("id").and_then(Value::as_str),
            Some(api_id.as_str()),
            "case {case:?}: expected the api to report the id the stack published, got {api}"
        );

        // It holds the resource the row's method is on, and that resource holds the method.
        let (status, resources) =
            apigateway(&router, &format!("/restapis/{api_id}/resources")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetResources, got {status} with body \
             {resources}"
        );
        let items: Vec<&Value> = resources
            // `GetResources` answers its list under the member name the model's `jsonName`
            // gives it (`item`), which is what a client of this service reads.
            .get("item")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default();
        let paths: Vec<&str> = items
            .iter()
            .filter_map(|resource| resource.get("path").and_then(Value::as_str))
            .collect();
        assert!(
            paths.contains(&row.path),
            "case {case:?}: expected the api to hold the resource {:?}, got {paths:?}",
            row.path
        );
        let on_the_path = items
            .iter()
            .find(|resource| resource.get("path").and_then(Value::as_str) == Some(row.path))
            .expect("the resource asserted above");
        let resource_id = on_the_path
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (status, method) = apigateway(
            &router,
            &format!(
                "/restapis/{api_id}/resources/{resource_id}/methods/{}",
                row.method
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetMethod for {} {}, got {status} with body \
             {method}",
            row.method, row.path
        );
        let integration_uri = method
            .get("methodIntegration")
            .and_then(|integration| integration.get("uri"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (_, stack_resources) =
            cloudformation(&router, "DescribeStackResources", &[("StackName", &stack)]).await;
        let physical: BTreeMap<String, String> = physical_ids_of(&stack_resources);
        let function = physical.get(row.function).cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: no physical id for {} in {stack_resources}",
                row.function
            )
        });
        assert!(
            integration_uri.contains(&format!("function:{function}")),
            "case {case:?}: expected the method's integration to name the function \
             {function:?}, got {integration_uri:?}"
        );

        // The stage the stack deployed points at a deployment that service holds, so what it
        // serves is a snapshot of those methods.
        let (status, stage) =
            apigateway(&router, &format!("/restapis/{api_id}/stages/{}", row.stage)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetStage for {}, got {status} with body \
             {stage}",
            row.stage
        );
        let deployment_id = stage
            .get("deploymentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (status, deployment) = apigateway(
            &router,
            &format!("/restapis/{api_id}/deployments/{deployment_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stage's deployment ({deployment_id}) to be one API \
             Gateway holds, got {status} with body {deployment}"
        );

        // And the invoke URL the stack published is served by that stage: the answer is the
        // integration's, carrying what the proxy event said about the request.
        let state = wait_until_active(&router, &function).await;
        assert_eq!(
            state, "Active",
            "case {case:?}: expected Lambda to report {function} Active before the invoke, got \
             {state:?}"
        );
        let (status, answered) = invoke(&router, &invoke_url, row.method, row.body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deployed stage to answer HTTP 200 at {invoke_url}, got \
             {status} with body {answered}"
        );
        let payload: Value = serde_json::from_str(&answered).unwrap_or(Value::Null);
        assert_eq!(
            payload.get("path").and_then(Value::as_str),
            Some(row.path),
            "case {case:?}: expected the integration to see the path {:?}, got {answered}",
            row.path
        );
        assert_eq!(
            payload.get("method").and_then(Value::as_str),
            Some(row.method),
            "case {case:?}: expected the integration to see the verb {:?}, got {answered}",
            row.method
        );
        assert_eq!(
            payload.get("stage").and_then(Value::as_str),
            Some(row.stage),
            "case {case:?}: expected the integration to see the stage {:?}, got {answered}",
            row.stage
        );
    }
}

/// A path the deployed api has no method for is answered by the STAGE — the data plane's own
/// refusal for a request that resolves to no method — and never by the front door's
/// service-resolution error, which is no answer an api endpoint can give.
#[tokio::test]
async fn a_path_the_deployed_api_has_no_method_for_is_refused_by_the_stage() {
    let router = build_router();
    let stack = "rest-api-stage-unknown-path";
    let template = implicit_api(stack);

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let (status, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "expected the stack to reach {CREATE_COMPLETE}, got {described}"
    );
    let api_id = outputs_of(&described)
        .get("ApiId")
        .cloned()
        .unwrap_or_else(|| panic!("the stack published no ApiId output in {described}"));

    // Two addressings of the same stage, both of which a client on this machine uses: the
    // api's own invoke host, and the emulator's reserved data-plane path.
    let cases: [(&str, String); 2] = [
        (
            "the api's own invoke host",
            format!("https://{api_id}.execute-api.{REGION}.amazonaws.com/Prod/absent"),
        ),
        (
            "the emulator's reserved data-plane path",
            format!("https://localhost/_aws/execute-api/{api_id}/Prod/absent"),
        ),
    ];
    for (case, url) in cases {
        let (status, answered) = invoke(&router, &url, "POST", "{}").await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the stage to refuse a path it has no method for with HTTP \
             403, got {status} with body {answered}"
        );
        assert!(
            !answered.contains("UnknownService"),
            "case {case:?}: expected an answer from the api's data plane, got the front door's \
             service-resolution error {answered}"
        );
    }
}
