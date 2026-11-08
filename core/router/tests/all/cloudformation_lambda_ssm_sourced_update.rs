// SPDX-License-Identifier: CC0-1.0

//! What a stack UPDATE deploys into an `AWS::Lambda::Function` whose property values live in
//! AWS Systems Manager Parameter Store.
//!
//! RULE: CloudFormation applies a changed `AWS::Lambda::Function` declaration to the function
//! AWS Lambda holds through that service's own update calls, each made only once Lambda
//! reports the function out of the window in which it refuses a modification ("During this
//! time, you can't modify the function"), so the stack reaches `UPDATE_COMPLETE` rather than
//! failing the resource on a conflict of the deployment's own making; and every property
//! value the declaration sources from Parameter Store — a `{{resolve:ssm:…}}` dynamic
//! reference, or a parameter declared `AWS::SSM::Parameter::Value<…>`, which "corresponds to
//! an existing Systems Manager parameter" — is resolved against that store BY THE DEPLOYMENT
//! THAT RUNS, so the updated function reports and SERVES the value the store holds then
//! rather than the one it held when the stack was created.
//!
//! Four services have to agree for that to hold, so this is the suite that can pin it:
//! CloudFormation resolves the declaration and makes the calls, Lambda applies them and
//! serves the invocation, Systems Manager holds the values, and IAM holds the execution role.
//! `cloudformation_updates_lambda_functions.rs` states the update rule for declarations whose
//! values the template itself carries; this suite is the half whose values another service
//! holds, which is a different code path on both sides — the deployment re-reads the store,
//! and what it then applies has to reach the function through calls Lambda accepts.
//!
//! The rows differ along the FORM a value is sourced through and what moves under it: a plain
//! dynamic reference, one pinned to a parameter version, an SSM-typed parameter whose name the
//! request supplies, one taking the template's `Default`, one the update re-points at another
//! parameter, and a list-typed parameter joined into a single value — each beside a literal,
//! which moves only when the template says so. Every expected value is computed from what the
//! row wrote into Parameter Store, never read back out of the answer being checked.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// Polling a stack until it first reports a status, and the awsQuery form encoder and the
// answer readers, are written ONCE in this crate.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{elements, form_encode, physical_ids_of};

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls in
/// — and so the Region the stack, its function and its parameters belong to.
const REGION: &str = "us-east-1";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The logical ids every row declares its two resources under.
const FUNCTION_LOGICAL_ID: &str = "Fn";
const ROLE_LOGICAL_ID: &str = "ExecRole";

/// The two environment variables every row declares, whose SOURCES are what the rows vary.
const FIRST_VARIABLE: &str = "FIRST";
const SECOND_VARIABLE: &str = "SECOND";

/// The runtime every row declares, and the handler the inline package's archived file
/// resolves to (`index` plus the runtime's extension).
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";

/// The `State` a function takes invocations in, and the `LastUpdateStatus` of a function whose
/// last modification has been applied — the two members that say Lambda is no longer refusing
/// modifications of it.
const STATE_ACTIVE: &str = "Active";
const LAST_UPDATE_SUCCESSFUL: &str = "Successful";

/// The parameter-store names the rows address. One per row, because every row deploys into the
/// one account this suite's engine holds.
const BOTH_DYNAMIC: &str = "/probe/cfn-lambda-update/both-dynamic";
const BOTH_TYPED: &str = "/probe/cfn-lambda-update/both-typed";
const PINNED: &str = "/probe/cfn-lambda-update/pinned";
const DEFAULTED: &str = "/probe/cfn-lambda-update/defaulted";
const LEFT: &str = "/probe/cfn-lambda-update/left";
const RIGHT: &str = "/probe/cfn-lambda-update/right";
const LISTED: &str = "/probe/cfn-lambda-update/listed";

/// The handler every row declares: it needs nothing of its runtime beyond the language and its
/// own environment, so what an invocation of it answers is the two values the deployment
/// resolved into the function.
const HANDLER_SOURCE: &str = r#"
import os


def handler(event, context):
    return {"first": os.environ["FIRST"], "second": os.environ["SECOND"]}
"#;

/// The four services this rule spans, over ONE engine exactly as `core/router/src/lib.rs`
/// mounts them: CloudFormation makes the calls, Lambda serves them, Systems Manager holds the
/// values the declaration is built from, IAM holds the execution role.
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
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request and what each handler reads its Region from. The signature is never checked:
/// this router runs with request authentication off.
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

/// Call one Systems Manager operation over its awsJson1_1 wire.
async fn ssm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ssm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonSSM.{operation}"))
        .header("authorization", authorization("ssm"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Lambda restJson1 call, as its status, its headers and its parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::from(body.to_vec()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// What `GetFunctionConfiguration` reports for one function, with the status it answered.
async fn reported_configuration(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    let (status, _, configuration) = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        &[],
    )
    .await;
    (status, configuration)
}

/// The environment variables one reported configuration carries, as the map a client reads.
fn reported_environment(configuration: &Value) -> BTreeMap<String, String> {
    configuration
        .get("Environment")
        .and_then(|environment| environment.get("Variables"))
        .and_then(Value::as_object)
        .map(|variables| {
            variables
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A trust policy letting Lambda assume the role, which is what an execution role has to carry
/// for that service to be handed it.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// Where ONE of the function's two environment values comes from in a template — the axis the
/// rows differ along.
#[derive(Debug)]
enum Source {
    /// A value the template itself carries, which moves only when the template does.
    Literal(&'static str),
    /// `{{resolve:ssm:<name>}}`: "the dynamic reference pattern for plaintext Systems Manager
    /// parameters", resolved by the deployment that reads it.
    Dynamic(&'static str),
    /// `{{resolve:ssm:<name>:<version>}}`: the same reference pinned to one version of the
    /// parameter, which Parameter Store resolves through its `name:version` selector.
    DynamicVersion(&'static str, u32),
    /// `{"Ref": <key>}` on a template parameter declared with an SSM-typed `Type`, whose value
    /// is the NAME of a parameter and whose resolution is the value the store holds for it.
    Reference(&'static str),
    /// `Fn::Join` over such a parameter whose inner type is a list, so what the function
    /// carries is built from the resolved list rather than from one string.
    JoinedReference(&'static str, &'static str),
}

impl Source {
    /// The value a template declares for this source.
    fn declared(&self) -> Value {
        match self {
            Self::Literal(text) => json!(text),
            Self::Dynamic(name) => json!(format!("{{{{resolve:ssm:{name}}}}}")),
            Self::DynamicVersion(name, version) => {
                json!(format!("{{{{resolve:ssm:{name}:{version}}}}}"))
            }
            Self::Reference(key) => json!({ "Ref": key }),
            Self::JoinedReference(delimiter, key) => {
                json!({ "Fn::Join": [delimiter, { "Ref": key }] })
            }
        }
    }
}

/// A template parameter one row declares.
struct Declared {
    key: &'static str,
    /// The declared `Type`, which is what decides whether the value in force is a parameter
    /// NAME to read or the value itself.
    declared_type: &'static str,
    /// The `Default` the template declares for it, for a row whose request passes no value.
    default: Option<&'static str>,
}

/// A parameter one row writes into Parameter Store, with the type that store holds it as.
struct Stored {
    name: &'static str,
    parameter_type: &'static str,
    value: &'static str,
}

/// One deployment of a row: the declaration it submits, the parameter values its request
/// carries, and what the rule says the function must then report and serve.
struct Stage {
    description: &'static str,
    first: Source,
    second: Source,
    expected_first: &'static str,
    expected_second: &'static str,
    request_parameters: &'static [(&'static str, &'static str)],
}

/// One row of the table: the stack it creates, what it then changes in Parameter Store, and the
/// update it deploys.
struct Row {
    case: &'static str,
    /// The role and function names this row's templates declare. Distinct per row: every row
    /// deploys into the one account this suite's engine holds.
    role: &'static str,
    function: &'static str,
    declared: &'static [Declared],
    /// What the store holds before the create.
    held: &'static [Stored],
    /// What is written over it before the update — each write adding a version, so a pinned
    /// reference still resolves the earlier one.
    overwritten: &'static [Stored],
    create: Stage,
    update: Stage,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded shape: both environment values come from the store, and both parameters
        // are overwritten between the two deployments.
        case: "a dynamic reference and an SSM-typed parameter, both overwritten before the \
               update",
        role: "cfn-ssm-upd-both-role",
        function: "cfn-ssm-upd-both-fn",
        declared: &[Declared {
            key: "TypedName",
            declared_type: "AWS::SSM::Parameter::Value<String>",
            default: None,
        }],
        held: &[
            Stored {
                name: BOTH_DYNAMIC,
                parameter_type: "String",
                value: "dyn-one",
            },
            Stored {
                name: BOTH_TYPED,
                parameter_type: "String",
                value: "typed-one",
            },
        ],
        overwritten: &[
            Stored {
                name: BOTH_DYNAMIC,
                parameter_type: "String",
                value: "dyn-two",
            },
            Stored {
                name: BOTH_TYPED,
                parameter_type: "String",
                value: "typed-two",
            },
        ],
        create: Stage {
            description: "reads both sources",
            first: Source::Dynamic(BOTH_DYNAMIC),
            second: Source::Reference("TypedName"),
            expected_first: "dyn-one",
            expected_second: "typed-one",
            request_parameters: &[("TypedName", BOTH_TYPED)],
        },
        update: Stage {
            description: "reads both sources again",
            first: Source::Dynamic(BOTH_DYNAMIC),
            second: Source::Reference("TypedName"),
            expected_first: "dyn-two",
            expected_second: "typed-two",
            request_parameters: &[("TypedName", BOTH_TYPED)],
        },
    },
    Row {
        // A reference pinned to a VERSION is resolved at deploy time too, and what it resolves
        // to is that version — beside the unpinned reference to the same parameter, which moves.
        case: "a dynamic reference pinned to a parameter version beside the same parameter \
               unpinned",
        role: "cfn-ssm-upd-pinned-role",
        function: "cfn-ssm-upd-pinned-fn",
        declared: &[],
        held: &[Stored {
            name: PINNED,
            parameter_type: "String",
            value: "pin-one",
        }],
        overwritten: &[Stored {
            name: PINNED,
            parameter_type: "String",
            value: "pin-two",
        }],
        create: Stage {
            description: "reads the pinned and the unpinned reference",
            first: Source::DynamicVersion(PINNED, 1),
            second: Source::Dynamic(PINNED),
            expected_first: "pin-one",
            expected_second: "pin-one",
            request_parameters: &[],
        },
        update: Stage {
            description: "reads the pinned and the unpinned reference once more",
            first: Source::DynamicVersion(PINNED, 1),
            second: Source::Dynamic(PINNED),
            expected_first: "pin-one",
            expected_second: "pin-two",
            request_parameters: &[],
        },
    },
    Row {
        // An SSM-typed parameter taking the template's own `Default`, beside the literal the
        // update changes: the resolved value is re-read and stays what the store holds.
        case: "an SSM-typed parameter taking the template default beside a literal the update \
               changes",
        role: "cfn-ssm-upd-default-role",
        function: "cfn-ssm-upd-default-fn",
        declared: &[Declared {
            key: "HeldName",
            declared_type: "AWS::SSM::Parameter::Value<String>",
            default: Some(DEFAULTED),
        }],
        held: &[Stored {
            name: DEFAULTED,
            parameter_type: "String",
            value: "held-value",
        }],
        overwritten: &[],
        create: Stage {
            description: "carries the first literal",
            first: Source::Reference("HeldName"),
            second: Source::Literal("literal-one"),
            expected_first: "held-value",
            expected_second: "literal-one",
            request_parameters: &[],
        },
        update: Stage {
            description: "carries the second literal",
            first: Source::Reference("HeldName"),
            second: Source::Literal("literal-two"),
            expected_first: "held-value",
            expected_second: "literal-two",
            request_parameters: &[],
        },
    },
    Row {
        // Nothing in the store moves: the update re-points the parameter at ANOTHER name, so the
        // value in force is the other parameter's.
        case: "an SSM-typed parameter the update points at a different parameter",
        role: "cfn-ssm-upd-repoint-role",
        function: "cfn-ssm-upd-repoint-fn",
        declared: &[Declared {
            key: "Which",
            declared_type: "AWS::SSM::Parameter::Value<String>",
            default: None,
        }],
        held: &[
            Stored {
                name: LEFT,
                parameter_type: "String",
                value: "left-value",
            },
            Stored {
                name: RIGHT,
                parameter_type: "String",
                value: "right-value",
            },
        ],
        overwritten: &[],
        create: Stage {
            description: "addresses the left parameter",
            first: Source::Reference("Which"),
            second: Source::Literal("unchanged"),
            expected_first: "left-value",
            expected_second: "unchanged",
            request_parameters: &[("Which", LEFT)],
        },
        update: Stage {
            description: "addresses the right parameter",
            first: Source::Reference("Which"),
            second: Source::Literal("unchanged"),
            expected_first: "right-value",
            expected_second: "unchanged",
            request_parameters: &[("Which", RIGHT)],
        },
    },
    Row {
        // The inner type is what the resolved value is read AS: a list-typed parameter is a list
        // to the function that joins it, on the update as on the create.
        case: "a list-typed SSM parameter joined into one value, overwritten before the update",
        role: "cfn-ssm-upd-listed-role",
        function: "cfn-ssm-upd-listed-fn",
        declared: &[Declared {
            key: "Shards",
            declared_type: "AWS::SSM::Parameter::Value<List<String>>",
            default: Some(LISTED),
        }],
        held: &[Stored {
            name: LISTED,
            parameter_type: "StringList",
            value: "a-one,b-one",
        }],
        overwritten: &[Stored {
            name: LISTED,
            parameter_type: "StringList",
            value: "a-two,b-two",
        }],
        create: Stage {
            description: "joins the first list",
            first: Source::JoinedReference("|", "Shards"),
            second: Source::Literal("joined"),
            expected_first: "a-one|b-one",
            expected_second: "joined",
            request_parameters: &[],
        },
        update: Stage {
            description: "joins the list the store holds now",
            first: Source::JoinedReference("|", "Shards"),
            second: Source::Literal("joined"),
            expected_first: "a-two|b-two",
            expected_second: "joined",
            request_parameters: &[],
        },
    },
];

/// The template one stage of a row deploys: the execution role the function runs as, the
/// function itself with the two environment values this stage sources, and the parameters the
/// row declares; publishing the function's `Ref` as the stack's output.
fn template(row: &Row, stage: &Stage) -> String {
    let mut document = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            ROLE_LOGICAL_ID: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": row.role,
                    "AssumeRolePolicyDocument": trust_policy(),
                }
            },
            FUNCTION_LOGICAL_ID: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": row.function,
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Role": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] },
                    "Description": stage.description,
                    "Environment": {
                        "Variables": {
                            FIRST_VARIABLE: stage.first.declared(),
                            SECOND_VARIABLE: stage.second.declared(),
                        }
                    },
                    "Code": { "ZipFile": HANDLER_SOURCE },
                }
            }
        },
        "Outputs": {
            "FunctionRef": { "Value": { "Ref": FUNCTION_LOGICAL_ID } }
        }
    });
    if !row.declared.is_empty() {
        let mut parameters = Map::new();
        for parameter in row.declared {
            let mut declaration = json!({ "Type": parameter.declared_type });
            if let Some(default) = parameter.default {
                declaration["Default"] = json!(default);
            }
            parameters.insert(parameter.key.to_string(), declaration);
        }
        document["Parameters"] = Value::Object(parameters);
    }
    document.to_string()
}

/// The awsQuery members one deployment of a stage carries: the stack, the template, the
/// acknowledgement the declared named role needs, and the stage's own parameter values.
fn deployment_members(stack: &str, body: &str, stage: &Stage) -> Vec<(String, String)> {
    let mut members = vec![
        ("StackName".to_string(), stack.to_string()),
        ("TemplateBody".to_string(), body.to_string()),
        (
            "Capabilities.member.1".to_string(),
            "CAPABILITY_NAMED_IAM".to_string(),
        ),
    ];
    for (index, (key, value)) in stage.request_parameters.iter().enumerate() {
        let position = index + 1;
        members.push((
            format!("Parameters.member.{position}.ParameterKey"),
            (*key).to_string(),
        ));
        members.push((
            format!("Parameters.member.{position}.ParameterValue"),
            (*value).to_string(),
        ));
    }
    members
}

/// Write one parameter into Parameter Store, overwriting a value already held under the name —
/// which adds a version rather than replacing the old one.
async fn put_parameter(router: &MultiServiceRouter, case: &str, parameter: &Stored) {
    let (status, body) = ssm(
        router,
        "PutParameter",
        &json!({
            "Name": parameter.name,
            "Type": parameter.parameter_type,
            "Value": parameter.value,
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from PutParameter for {:?}, got {status} with body \
         {body}",
        parameter.name
    );
}

/// What Lambda reports for the function one completed deployment published, and what it serves
/// when invoked: the two values the deployment resolved, and a function this service is no
/// longer refusing modifications of.
async fn assert_function_carries(
    router: &MultiServiceRouter,
    case: &str,
    phase: &str,
    row: &Row,
    stage: &Stage,
) {
    // ONE read, taken the moment the stack reported the deployment complete and never
    // retried: the deployment does not report the resource done while its own service is
    // still refusing calls on it, so a wait here would hide exactly the failure the rule
    // forbids — a stack complete over a function Lambda has not finished with.
    let (status, configuration) = reported_configuration(router, row.function).await;
    assert_eq!(
        status, 200,
        "case {case:?} after the {phase}: expected Lambda to resolve the function the \
         completed deployment published, got {status} with body {configuration}"
    );
    assert_eq!(
        configuration.get("State").and_then(Value::as_str),
        Some(STATE_ACTIVE),
        "case {case:?} after the {phase}: expected Lambda to report the function \
         {STATE_ACTIVE}, got {configuration}"
    );
    assert_eq!(
        configuration
            .get("LastUpdateStatus")
            .and_then(Value::as_str),
        Some(LAST_UPDATE_SUCCESSFUL),
        "case {case:?} after the {phase}: expected LastUpdateStatus {LAST_UPDATE_SUCCESSFUL}, \
         got {configuration}"
    );
    assert_eq!(
        configuration.get("Description").and_then(Value::as_str),
        Some(stage.description),
        "case {case:?} after the {phase}: expected the Description {:?}, got {configuration}",
        stage.description
    );

    let expected: BTreeMap<String, String> = [
        (FIRST_VARIABLE.to_string(), stage.expected_first.to_string()),
        (
            SECOND_VARIABLE.to_string(),
            stage.expected_second.to_string(),
        ),
    ]
    .into_iter()
    .collect();
    let reported = reported_environment(&configuration);
    assert_eq!(
        reported, expected,
        "case {case:?} after the {phase}: expected the environment {expected:?}, got {reported:?}"
    );

    // And what the function SERVES is those values: the resolution reached the function's own
    // environment, not only the record the stack keeps of it.
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{}/invocations", row.function),
        json!({ "probe": true }).to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} after the {phase}: expected HTTP 200 from Invoke, got {status} with \
         payload {payload}"
    );
    let failure = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        failure, None,
        "case {case:?} after the {phase}: expected no function error, got {failure:?} with \
         payload {payload}"
    );
    let wanted = json!({ "first": stage.expected_first, "second": stage.expected_second });
    assert_eq!(
        payload, wanted,
        "case {case:?} after the {phase}: expected the handler to answer {wanted}, got {payload}"
    );
}

/// Every row: the create resolves its sources, the update completes rather than failing the
/// function on a conflict, and what the function then reports and serves is what the store
/// holds at update time.
#[tokio::test]
async fn an_update_re_resolves_the_store_into_the_function_it_changes() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack = format!("cfn-lambda-ssm-update-{index}");

        for parameter in row.held {
            put_parameter(&router, case, parameter).await;
        }

        // ---- The create: the stack the update is deployed against. ----
        let body = template(row, &row.create);
        let owned = deployment_members(&stack, &body, &row.create);
        let members: Vec<(&str, &str)> = owned
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, answered) = cloudformation(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
        );
        let (status, described) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE} before the update, \
             got {described} with resource failures {:?}",
            failure_reasons(&router, &stack).await
        );
        let published = published_id(&router, case, &stack).await;
        assert_eq!(
            published, row.function,
            "case {case:?}: expected the create to publish the declared function name {:?}, got \
             {published:?}",
            row.function
        );
        assert_function_carries(&router, case, "create", row, &row.create).await;

        // ---- What the store holds changes under the stack. ----
        for parameter in row.overwritten {
            put_parameter(&router, case, parameter).await;
        }

        // ---- The update. ----
        let body = template(row, &row.update);
        let owned = deployment_members(&stack, &body, &row.update);
        let members: Vec<(&str, &str)> = owned
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, answered) = cloudformation(&router, "UpdateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {answered}"
        );
        let (status, described) = wait_for_stack_status(&router, &stack, UPDATE_COMPLETE).await;
        assert_eq!(
            status, UPDATE_COMPLETE,
            "case {case:?}: expected the stack to reach {UPDATE_COMPLETE}, got {described} with \
             resource failures {:?}",
            failure_reasons(&router, &stack).await
        );
        let published = published_id(&router, case, &stack).await;
        assert_eq!(
            published, row.function,
            "case {case:?}: expected the completed update to publish {:?}, got {published:?}",
            row.function
        );
        assert_function_carries(&router, case, "update", row, &row.update).await;
    }
}

/// The physical id the stack publishes for its function, read through
/// `DescribeStackResources` — the hand-off a client takes the function's name from.
async fn published_id(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    physical_ids_of(&resources)
        .remove(FUNCTION_LOGICAL_ID)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no physical id for {FUNCTION_LOGICAL_ID:?} in {resources}")
        })
}

/// Every `ResourceStatusReason` the stack has recorded, so a deployment that did NOT reach the
/// status the rule states reports why rather than only that it did not.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}
