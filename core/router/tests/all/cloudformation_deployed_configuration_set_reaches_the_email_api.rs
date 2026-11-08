// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed configuration set is the one the EMAIL API answers for — and
//! the one the deployed function reads through the SDK its runtime ships.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns the type created it, and the handle the stack publishes for it (`Ref`, and the ARN an
//! intrinsic builds from it) addresses the resource that service really holds — so a function
//! the same stack deploys, handed that handle in its environment, reads the resource back
//! through its own API rather than a refusal for a resource nothing created.** A
//! configuration set is one account resource behind both of Amazon SES's APIs
//! (`service_common::ses_configuration_sets`), so the set CloudFormation created is the set
//! the v2 API reports, whichever API the deployment itself called.
//!
//! Five crates have to agree for that to hold — CloudFormation deploys, IAM answers for the
//! execution role, Amazon SES holds the set, AWS Lambda runs the handler, and the SES v2 API
//! serves the handler's own call — so the suite lives here: a crate-local test can only ever
//! see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! configuration set or leaves the name to CloudFormation, and which intrinsic the function's
//! environment resolves it through (`Ref` for the name, `Fn::Sub` for the ARN built from it).
//! Each row also asserts the other half of the rule: the same handler asking the email API
//! for a set the stack did NOT create is refused with that service's modelled
//! `NotFoundException`, so what the positive leg proves is a set in the owning service and
//! not an answer the runtime made up.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, outputs_of};

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls in
/// — and so the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, and the partition those ARNs name:
/// what `${AWS::AccountId}` and `${AWS::Partition}` resolve to in a template.
const ACCOUNT: &str = "000000000000";
const PARTITION: &str = "aws";

/// The `StackStatus` the rule is stated about.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The environment variables each row's template resolves the configuration set into: the
/// set's name, which `Ref` returns for the type, and the ARN an `Fn::Sub` builds from it.
const NAME_VARIABLE: &str = "CONFIG_SET";
const ARN_VARIABLE: &str = "CONFIG_SET_ARN";

/// The modelled refusal the email API answers for a configuration set the account does not
/// hold (`NotFoundException`, HTTP 404), which the negative leg of every row expects.
const NOT_FOUND: &str = "NotFoundException";

/// The handler every row deploys: it asks the email API about the configuration set its
/// ENVIRONMENT names — or the one the event names, which is the negative leg — and reports
/// what that service answered, so the invocation's payload is the service's own answer and
/// never something the runtime composed.
const HANDLER_SOURCE: &str = r#"
import os

import boto3
import botocore.exceptions


def handler(event, context):
    client = boto3.client("sesv2")
    asked = event.get("set") or os.environ["CONFIG_SET"]
    try:
        answered = client.get_configuration_set(ConfigurationSetName=asked)
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "asked": asked,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
        }
    listed = client.list_configuration_sets().get("ConfigurationSets", [])
    return {
        "ok": True,
        "asked": asked,
        "name": answered.get("ConfigurationSetName"),
        "arn": os.environ["CONFIG_SET_ARN"],
        "listed": listed,
    }
"#;

/// One row: a stack declaring a configuration set and a function whose environment resolves
/// it.
struct Row {
    case: &'static str,
    /// The stack's name.
    stack: &'static str,
    /// The function's name, which the row invokes it by.
    function: &'static str,
    /// The name the template gives the configuration set, or `None` for a template that
    /// names none — whose set carries the name CloudFormation generated.
    declared_set: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a configuration set the template names",
        stack: "notices-named",
        function: "notices-named-fn",
        declared_set: Some("notices-named-set"),
    },
    Row {
        case: "a configuration set whose name CloudFormation generates",
        stack: "notices-generated",
        function: "notices-generated-fn",
        declared_set: None,
    },
];

/// The emulator with the five services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
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

/// Call one Lambda operation over its restJson1 wire, as `(status, headers, body)`.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, http::HeaderMap, Value) {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let answered: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (parts.status.as_u16(), parts.headers, answered)
}

/// Call one SES v2 operation over its restJson1 wire, as `(status, body)`.
async fn sesv2(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("email.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("ses"))
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// The template a row deploys: the configuration set, the function's execution role, and the
/// function whose environment resolves the set through `Ref` and `Fn::Sub`.
fn template(row: &Row) -> String {
    let mut set_properties = json!({});
    if let Some(name) = row.declared_set {
        set_properties["Name"] = json!(name);
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ExecRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": format!("{}-exec", row.function),
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
            "ConfigSet": {
                "Type": "AWS::SES::ConfigurationSet",
                "Properties": set_properties
            },
            "Notifier": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": row.function,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["ExecRole", "Arn"] },
                    "Timeout": 15,
                    "Environment": { "Variables": {
                        NAME_VARIABLE: { "Ref": "ConfigSet" },
                        ARN_VARIABLE: { "Fn::Sub":
                            "arn:${AWS::Partition}:ses:${AWS::Region}:${AWS::AccountId}:\
                             configuration-set/${ConfigSet}"
                        }
                    } },
                    "Code": { "ZipFile": HANDLER_SOURCE }
                }
            }
        },
        "Outputs": {
            "SetName": { "Value": { "Ref": "ConfigSet" } }
        }
    })
    .to_string()
}

/// The ARN a configuration set of `name` carries in this suite's Region: the same total
/// function of the partition, Region, account and name the template's `Fn::Sub` builds.
fn configuration_set_arn(name: &str) -> String {
    format!("arn:{PARTITION}:ses:{REGION}:{ACCOUNT}:configuration-set/{name}")
}

/// `Invoke` the function synchronously with `event`, as `(the FunctionError signal, the
/// payload)`.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{function}/invocations"),
        Some(event),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} with {payload}"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

#[tokio::test]
async fn a_deployed_configuration_set_is_the_one_the_email_api_answers_for() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row)),
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );

        let (reported, described) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            reported, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {reported} with \
             {described}"
        );

        // The handle the stack publishes for the set: the name `Ref` returns for the type,
        // which the template that names none leaves to CloudFormation.
        let outputs = outputs_of(&described);
        let set = outputs
            .get("SetName")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the stack publishes SetName, got {outputs:?}"));
        if let Some(declared) = row.declared_set {
            assert_eq!(
                set, declared,
                "case {case:?}: expected the declared set name, got {set:?}"
            );
        } else {
            assert!(
                !set.is_empty() && set.len() <= 64,
                "case {case:?}: expected a generated configuration-set name within the type's \
                 64-character bound, got {set:?}"
            );
        }

        // The service that OWNS the type holds a set under exactly that name — asked through
        // the email API a client uses, not through the API the deployment happened to call.
        let (status, held) = sesv2(
            &router,
            "GET",
            &format!("/v2/email/configuration-sets/{set}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the email API must hold the set the stack published, got {status} \
             with {held}"
        );
        assert_eq!(
            held.get("ConfigurationSetName").and_then(Value::as_str),
            Some(set.as_str()),
            "case {case:?}: the email API must report the set under the published name, got \
             {held}"
        );

        // The function's environment carries the RESOLVED handle: the set's name, and the ARN
        // the template's intrinsic built from it.
        let (status, _headers, configuration) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{}/configuration", row.function),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFunctionConfiguration, got {status} with \
             {configuration}"
        );
        let variables = configuration
            .get("Environment")
            .and_then(|environment| environment.get("Variables"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            variables.get(NAME_VARIABLE).and_then(Value::as_str),
            Some(set.as_str()),
            "case {case:?}: expected the environment to resolve the set's name, got {variables}"
        );
        assert_eq!(
            variables.get(ARN_VARIABLE).and_then(Value::as_str),
            Some(configuration_set_arn(&set).as_str()),
            "case {case:?}: expected the environment to resolve the set's ARN, got {variables}"
        );

        // And the function READS the set back through the SDK its runtime ships, which is
        // what a client deploying this stack does with the handle it was given.
        let (signal, payload) = invoke(&router, case, row.function, &json!({})).await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler's own call must succeed, got {payload}"
        );
        assert_eq!(
            payload.get("ok").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the handler to read the set back, got {payload}"
        );
        assert_eq!(
            payload.get("name").and_then(Value::as_str),
            Some(set.as_str()),
            "case {case:?}: expected the handler to read the published set, got {payload}"
        );
        assert_eq!(
            payload.get("arn").and_then(Value::as_str),
            Some(configuration_set_arn(&set).as_str()),
            "case {case:?}: expected the handler's environment ARN, got {payload}"
        );
        let listed: Vec<String> = payload
            .get("listed")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|name| name.as_str().map(str::to_string))
            .collect();
        assert!(
            listed.contains(&set),
            "case {case:?}: expected the handler's listing to carry the published set, got \
             {listed:?}"
        );

        // The other half of the rule: a set the stack did NOT create is the email API's own
        // modelled refusal, so the leg above is a set that service holds and not an answer
        // the runtime composed.
        let absent = format!("{set}-absent");
        let (signal, payload) =
            invoke(&router, case, row.function, &json!({ "set": absent })).await;
        assert_eq!(
            signal, None,
            "case {case:?}: the handler reports the refusal rather than failing, got {payload}"
        );
        assert_eq!(
            payload.get("ok").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected the absent set to be refused, got {payload}"
        );
        assert_eq!(
            payload.get("code").and_then(Value::as_str),
            Some(NOT_FOUND),
            "case {case:?}: expected the email API's modelled not-found code, got {payload}"
        );
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(format!("Configuration set <{absent}> does not exist.").as_str()),
            "case {case:?}: expected the email API's own wording for a set it does not hold, \
             got {payload}"
        );
    }
}
