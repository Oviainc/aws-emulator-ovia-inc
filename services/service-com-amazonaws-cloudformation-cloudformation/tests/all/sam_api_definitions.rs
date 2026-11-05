// SPDX-License-Identifier: CC0-1.0

//! The API DEFINITION the `AWS::Serverless-2016-10-31` (AWS SAM) transform generates, read
//! off the processed template over the awsQuery wire.
//!
//! The rule every row here holds the service to: **the transform gives every API it emits
//! the OpenAPI definition that names it — "if neither `DefinitionUri` nor `DefinitionBody`
//! are specified, AWS SAM generates [it] based on your template configuration" — built from
//! the routes of the function events routed through that API, whether the API is the
//! implicit one the macro adds or one the template declares, with what the declaration (or
//! the `Globals` section) says about the api itself carried INSIDE that document for an HTTP
//! api, which is created by importing it.** So no API the transform emits is an
//! `AWS::ApiGatewayV2::Api` carrying neither a definition nor a `Name`, which is the one
//! shape API Gateway v2's create refuses for a null name.
//!
//! The rows differ along what the rule generalises over: the API type (`AWS::Serverless::Api`
//! and `AWS::Serverless::HttpApi`), whether the API is implicit or declared, whether the
//! declaration carries a `DefinitionBody` of its own (whose title and whose own path item
//! both survive), whether it states a definition held in Amazon S3 (the one case nothing can
//! be written into), how many routes and methods are routed through it, where the CORS
//! configuration is stated (on the declaration or in `Globals`), and whether the declaration
//! names the api.
//!
//! `sam_transform.rs` owns the LOGICAL IDS the macro synthesizes and what a change set plans;
//! `core/router/tests/all/cloudformation_sam_http_api_definitions.rs` owns what API Gateway
//! v2 then HOLDS for a deployed stack. This suite is about the document the transform writes.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{call, member, router_with_engine, xml_text, xml_unescape};

/// What the api's name comes from in one row.
#[derive(Debug, Clone, Copy)]
enum Title {
    /// The generated definition's own `info.title`: the stack's name, which the expansion
    /// writes as the expression for it.
    StackName,
    /// The title the row states — a `Name` the declaration carries, or the one its own
    /// `DefinitionBody` was written with.
    Declared(&'static str),
}

impl Title {
    fn expected(self) -> Value {
        match self {
            Title::StackName => json!({ "Fn::Sub": "${AWS::StackName}" }),
            Title::Declared(title) => json!(title),
        }
    }
}

/// One route a row's API definition must declare: the path, the key its method is spelled
/// under, the integration type behind it and the payload format version that integration
/// carries (`None` where the definition declares none, which is every REST path item).
struct Route {
    path: &'static str,
    method: &'static str,
    integration_type: &'static str,
    payload_format_version: Option<&'static str>,
}

/// One row of the definition table.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The logical id of the API resource the transform emits, and the type it emits it as.
    api: &'static str,
    api_type: &'static str,
    /// The property the definition travels in: `Body` for a definition in the template,
    /// `BodyS3Location` for one held in Amazon S3 — whose document nothing here can read,
    /// so the row asserts no definition members for it.
    definition_property: &'static str,
    /// The api's `info.title`, for a row whose definition is in the template.
    title: Option<Title>,
    /// Every route the definition must declare, and nothing else.
    routes: Vec<Route>,
    /// The `allowOrigins` the definition's CORS extension must carry.
    allow_origins: Vec<&'static str>,
    /// The tags the definition must carry, as `(name, value)` pairs.
    tags: Vec<(&'static str, &'static str)>,
    /// Properties that must NOT sit beside the definition: an HTTP api is created by
    /// IMPORTING its document, and an import takes no member of the create beside it.
    absent_properties: Vec<&'static str>,
}

/// The `AWS::Serverless::Function` declaration a row routes its events through.
fn function(logical_id: &str, events: Value) -> Value {
    json!({
        logical_id: {
            "Type": "AWS::Serverless::Function",
            "Properties": {
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "CodeUri": format!("s3://code-bucket/{logical_id}.zip"),
                "Events": events
            }
        }
    })
}

/// One API event: its type, the path and method it routes, and the API it names, if any.
fn event(kind: &str, path: &str, method: &str, api: Option<&str>) -> Value {
    let mut properties = json!({ "Path": path, "Method": method });
    if let (Some(api), Some(members)) = (api, properties.as_object_mut()) {
        let id = if kind == "Api" { "RestApiId" } else { "ApiId" };
        members.insert(id.to_string(), json!({ "Ref": api }));
    }
    json!({ "Type": kind, "Properties": properties })
}

/// A template body from its sections: `Transform` plus whatever the row declares.
fn template(sections: Value) -> String {
    let mut document = json!({ "Transform": "AWS::Serverless-2016-10-31" });
    if let (Some(document), Some(sections)) = (document.as_object_mut(), sections.as_object()) {
        for (name, value) in sections {
            document.insert(name.clone(), value.clone());
        }
    }
    document.to_string()
}

/// One generated HTTP route: the Lambda proxy integration an `HttpApi` event becomes.
fn http_route(path: &'static str, method: &'static str) -> Route {
    Route {
        path,
        method,
        integration_type: "aws_proxy",
        payload_format_version: Some("2.0"),
    }
}

/// One generated REST route: the same proxy integration, which a REST definition carries no
/// payload format version on.
fn rest_route(path: &'static str, method: &'static str) -> Route {
    Route {
        path,
        method,
        integration_type: "aws_proxy",
        payload_format_version: None,
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the implicit HTTP api: one HttpApi event that names no ApiId",
            stack: "sam-definition-implicit-http",
            template: template(json!({
                "Resources": function("Greeter", json!({
                    "Hello": event("HttpApi", "/hello", "get", None)
                }))
            })),
            api: "ServerlessHttpApi",
            api_type: "AWS::ApiGatewayV2::Api",
            definition_property: "Body",
            title: Some(Title::StackName),
            routes: vec![http_route("/hello", "get")],
            allow_origins: vec![],
            tags: vec![],
            absent_properties: vec!["Name"],
        },
        Row {
            case: "a declared AWS::Serverless::HttpApi with no definition of its own, whose \
                   CORS configuration the Globals section states",
            stack: "sam-definition-declared-http",
            template: template(json!({
                "Globals": {
                    "HttpApi": {
                        "CorsConfiguration": {
                            "AllowOrigins": ["https://ui.test"],
                            "AllowMethods": ["GET", "POST"]
                        }
                    }
                },
                "Resources": {
                    "Api": {
                        "Type": "AWS::Serverless::HttpApi",
                        "Properties": { "StageName": "$default" }
                    },
                    "Greeter": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "CodeUri": "s3://code-bucket/greeter.zip",
                            "Events": {
                                "Hello": event("HttpApi", "/hello", "get", Some("Api")),
                                "Place": event("HttpApi", "/orders", "post", Some("Api"))
                            }
                        }
                    }
                }
            })),
            api: "Api",
            api_type: "AWS::ApiGatewayV2::Api",
            definition_property: "Body",
            title: Some(Title::StackName),
            routes: vec![http_route("/hello", "get"), http_route("/orders", "post")],
            allow_origins: vec!["https://ui.test"],
            tags: vec![],
            absent_properties: vec!["Name", "CorsConfiguration", "Tags"],
        },
        Row {
            case: "a declared HTTP api that states a Name, a description and tags, which an \
                   imported api carries inside its definition",
            stack: "sam-definition-named-http",
            template: template(json!({
                "Resources": {
                    "Api": {
                        "Type": "AWS::Serverless::HttpApi",
                        "Properties": {
                            "Name": "the-named-api",
                            "Description": "named by its declaration",
                            "Tags": { "Owner": "orders" },
                            "CorsConfiguration": "https://single.origin"
                        }
                    },
                    "Greeter": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "CodeUri": "s3://code-bucket/greeter.zip",
                            "Events": {
                                "Any": event("HttpApi", "/{proxy+}", "any", Some("Api"))
                            }
                        }
                    }
                }
            })),
            api: "Api",
            api_type: "AWS::ApiGatewayV2::Api",
            definition_property: "Body",
            title: Some(Title::Declared("the-named-api")),
            // "any" is the method that answers every verb, which a definition spells with
            // API Gateway's own extension.
            routes: vec![http_route("/{proxy+}", "x-amazon-apigateway-any-method")],
            allow_origins: vec!["https://single.origin"],
            tags: vec![("Owner", "orders")],
            absent_properties: vec!["Name", "Description", "Tags", "CorsConfiguration"],
        },
        Row {
            case: "a declared HTTP api whose own DefinitionBody keeps its title and its path \
                   item while the event's route is added beside it",
            stack: "sam-definition-merged-http",
            template: template(json!({
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
                    "Greeter": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "CodeUri": "s3://code-bucket/greeter.zip",
                            "Events": {
                                "Hello": event("HttpApi", "/hello", "get", Some("Api"))
                            }
                        }
                    }
                }
            })),
            api: "Api",
            api_type: "AWS::ApiGatewayV2::Api",
            definition_property: "Body",
            title: Some(Title::Declared("written-by-the-template")),
            routes: vec![
                http_route("/hello", "get"),
                Route {
                    path: "/ping",
                    method: "get",
                    integration_type: "http_proxy",
                    payload_format_version: Some("1.0"),
                },
            ],
            allow_origins: vec![],
            tags: vec![],
            absent_properties: vec!["Name", "DefinitionBody"],
        },
        Row {
            case: "an HTTP api whose definition is held in Amazon S3: the one case nothing \
                   can be written into the document",
            stack: "sam-definition-s3-http",
            template: template(json!({
                "Resources": {
                    "Api": {
                        "Type": "AWS::Serverless::HttpApi",
                        "Properties": {
                            "DefinitionUri": "s3://definitions/api.json",
                            "Name": "defined-in-s3"
                        }
                    }
                }
            })),
            api: "Api",
            api_type: "AWS::ApiGatewayV2::Api",
            definition_property: "BodyS3Location",
            title: None,
            routes: vec![],
            allow_origins: vec![],
            tags: vec![],
            absent_properties: vec!["Body", "DefinitionUri"],
        },
        Row {
            case: "the implicit REST api: two Api events through it, one of them an ANY \
                   method",
            stack: "sam-definition-implicit-rest",
            template: template(json!({
                "Resources": function("Greeter", json!({
                    "Hello": event("Api", "/hello", "get", None),
                    "Any": event("Api", "/{proxy+}", "any", None)
                }))
            })),
            api: "ServerlessRestApi",
            api_type: "AWS::ApiGateway::RestApi",
            definition_property: "Body",
            title: Some(Title::StackName),
            routes: vec![
                rest_route("/hello", "get"),
                rest_route("/{proxy+}", "x-amazon-apigateway-any-method"),
            ],
            allow_origins: vec![],
            tags: vec![],
            absent_properties: vec![],
        },
        Row {
            case: "a declared AWS::Serverless::Api with no definition of its own, which the \
                   event routed to it generates",
            stack: "sam-definition-declared-rest",
            template: template(json!({
                "Resources": {
                    "Web": {
                        "Type": "AWS::Serverless::Api",
                        "Properties": { "StageName": "live" }
                    },
                    "Placer": {
                        "Type": "AWS::Serverless::Function",
                        "Properties": {
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "CodeUri": "s3://code-bucket/placer.zip",
                            "Events": {
                                "Place": event("Api", "/orders", "post", Some("Web"))
                            }
                        }
                    }
                }
            })),
            api: "Web",
            api_type: "AWS::ApiGateway::RestApi",
            definition_property: "Body",
            title: Some(Title::StackName),
            routes: vec![rest_route("/orders", "post")],
            allow_origins: vec![],
            tags: vec![],
            absent_properties: vec![],
        },
    ]
}

/// The PROCESSED template of a `CREATE` change set over `template`: the document the macro
/// produced, which is what CloudFormation goes on to plan and deploy. A change set provisions
/// nothing, so this reads the transform's output alone.
async fn processed_template(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack: &str,
    body: &str,
) -> Value {
    let (code, answer) = call(
        router,
        "CreateChangeSet",
        &[
            member("StackName", stack),
            member("ChangeSetName", "plan"),
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", body),
            member("Capabilities.member.1", "CAPABILITY_IAM"),
            member("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected CreateChangeSet to be accepted, got {code} {answer}"
    );
    settle_the_store(router, engine).await;

    let (code, answer) = call(
        router,
        "GetTemplate",
        &[
            member("StackName", stack),
            member("ChangeSetName", "plan"),
            member("TemplateStage", "Processed"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected the processed template to be readable, got {code} {answer}"
    );
    let processed = xml_text(&answer, "TemplateBody")
        .map(|body| xml_unescape(&body))
        .unwrap_or_else(|| panic!("case {case:?}: no TemplateBody in {answer}"));
    ::serde_json::from_str(&processed)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a JSON document, got {error}"))
}

/// One resource of a processed template, by logical id.
fn resource<'a>(case: &str, processed: &'a Value, logical_id: &str) -> &'a Value {
    processed
        .get("Resources")
        .and_then(|resources| resources.get(logical_id))
        .unwrap_or_else(|| panic!("case {case:?}: no {logical_id} resource in {processed}"))
}

/// The paths a definition declares, as `(path, method)` pairs sorted the way a row states
/// them.
fn declared_routes(definition: &Value) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = definition
        .get("paths")
        .and_then(Value::as_object)
        .map(|paths| {
            paths
                .iter()
                .flat_map(|(path, item)| {
                    item.as_object()
                        .map(|methods| {
                            methods
                                .keys()
                                .map(|method| (path.clone(), method.clone()))
                                .collect::<Vec<(String, String)>>()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

#[tokio::test]
async fn every_api_the_transform_emits_carries_the_definition_that_names_it() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let case = row.case;
        let processed =
            processed_template(&router, &engine, case, row.stack, &row.template).await;
        let api = resource(case, &processed, row.api);
        assert_eq!(
            api.get("Type").and_then(Value::as_str),
            Some(row.api_type),
            "case {case:?}: expected {} to be a {}, got {api}",
            row.api,
            row.api_type
        );
        let properties = api
            .get("Properties")
            .unwrap_or_else(|| panic!("case {case:?}: no Properties on {api}"));
        let definition = properties
            .get(row.definition_property)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the api to carry its definition under {}, got \
                     {properties}",
                    row.definition_property
                )
            });
        for absent in &row.absent_properties {
            assert!(
                properties.get(*absent).is_none(),
                "case {case:?}: expected no {absent} beside the api's definition, got \
                 {properties}"
            );
        }

        if let Some(title) = row.title {
            // The api's NAME: an imported api takes it from `info.title`, so a definition
            // the transform generates always states one.
            assert_eq!(
                definition.get("info").and_then(|info| info.get("title")),
                Some(&title.expected()),
                "case {case:?}: expected the definition's info.title to be {}, got \
                 {definition}",
                title.expected()
            );

            let expected: Vec<(String, String)> = {
                let mut routes: Vec<(String, String)> = row
                    .routes
                    .iter()
                    .map(|route| (route.path.to_string(), route.method.to_string()))
                    .collect();
                routes.sort();
                routes
            };
            let declared = declared_routes(definition);
            assert_eq!(
                declared, expected,
                "case {case:?}: expected the definition to declare the routes {expected:?}, \
                 got {declared:?} in {definition}"
            );
            for route in &row.routes {
                let integration = definition
                    .get("paths")
                    .and_then(|paths| paths.get(route.path))
                    .and_then(|item| item.get(route.method))
                    .and_then(|operation| operation.get("x-amazon-apigateway-integration"))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: no integration on {} {} in {definition}",
                            route.method, route.path
                        )
                    });
                assert_eq!(
                    integration.get("type").and_then(Value::as_str),
                    Some(route.integration_type),
                    "case {case:?}: expected a {} integration on {} {}, got {integration}",
                    route.integration_type,
                    route.method,
                    route.path
                );
                assert_eq!(
                    integration
                        .get("payloadFormatVersion")
                        .and_then(Value::as_str),
                    route.payload_format_version,
                    "case {case:?}: expected the payload format version {:?} on {} {}, got \
                     {integration}",
                    route.payload_format_version,
                    route.method,
                    route.path
                );
            }
        }

        // The CORS configuration an HTTP api is created with travels in its definition.
        let origins: Vec<String> = definition
            .get("x-amazon-apigateway-cors")
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
            "case {case:?}: expected the definition's CORS allowOrigins to be \
             {expected_origins:?}, got {origins:?} in {definition}"
        );

        for (name, value) in &row.tags {
            let entry = json!({ "name": name, "x-amazon-apigateway-tag-value": value });
            let declared = definition.get("tags").cloned().unwrap_or(Value::Null);
            assert!(
                declared
                    .as_array()
                    .is_some_and(|entries| entries.contains(&entry)),
                "case {case:?}: expected the definition to carry the tag {entry}, got \
                 {declared}"
            );
        }

        // Whatever the row declared, the processed template declares no macro INPUT type:
        // every API the transform emitted is a concrete resource.
        let declared = processed
            .get("Resources")
            .and_then(Value::as_object)
            .map(|resources| {
                resources
                    .values()
                    .filter_map(|resource| resource.get("Type").and_then(Value::as_str))
                    .filter(|resource_type| resource_type.starts_with("AWS::Serverless::"))
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        assert!(
            declared.is_empty(),
            "case {case:?}: expected no AWS::Serverless::* type in the processed template, got \
             {declared:?}"
        );
    }
}
