// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Lambda::Function` really creates that
//! function in AWS Lambda, and a function Lambda refuses fails the stack.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, the stack reports the
//! operation complete only once that service HOLDS the resource, and whatever that service
//! answers short of success fails the resource and the stack — so the identifier a completed
//! stack publishes for an `AWS::Lambda::Function` ("Ref returns the function name") is one
//! `lambda get-function` resolves, and a template Lambda will not accept never reports a
//! clean deploy.** Three crates have to agree for that to hold (CloudFormation makes the
//! call, Lambda serves it, IAM answers for the execution role), so the suite lives here
//! rather than in any one crate.
//!
//! A function Lambda holds is one `GetFunction` resolves from the moment its create answers;
//! the `Pending` span before Lambda reports it `Active` is that service's own transition, so
//! the state is polled for here rather than demanded of the instant the stack completed.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! function or leaves CloudFormation to generate its name, whether the deployment package is
//! INLINE source or an Amazon S3 object, which runtime (and so which handler file an inline
//! package carries), whether the execution role is declared in the same template or was
//! provisioned outside it, which of the properties that are other Lambda calls'
//! members it declares, and whether the template is written in JSON or YAML.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none, which
//! is CloudFormation's to compute: what is checked about it is that Lambda holds a function
//! under it and that the stack's outputs name the same one.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use crate::query_wire::{elements, form_encode, opt_element};
// Polling a stack until it first reports a status is what every suite deploying a resource
// its owning service reports as still coming up needs, and this crate already has it: a
// Lambda function is `Pending` when its create answers, so the stack completes only once
// Lambda reports it `Active`.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls
/// in — and so the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way
/// (the credential scope is what routes it).
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
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
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

/// Call one IAM operation over its awsQuery wire. IAM is global, so the request is signed
/// for this suite's Region only because that is what routes it.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Lambda operation over its restJson1 wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
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
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// A trust policy that lets the principal `service` assume the role, which is what an
/// execution role has to carry for Lambda to be handed it.
fn trust_policy(service: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The ARN a function of `name` carries in this suite's Region: the same total function of
/// the Region and the name both services compute it from.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN a role of `name` carries: the value an execution role is named by.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// Poll `GetFunctionConfiguration` until Lambda first reports `wanted` as the function's
/// `State`, and answer the last state it reported.
///
/// The span a function spends `Pending` is AWS Lambda's own, so no fixed sleep names the
/// instant it ends; the budget is long enough that a loaded machine is slower than the span
/// that service chooses without the rule being any different.
async fn wait_for_function_state(
    router: &MultiServiceRouter,
    function_name: &str,
    wanted: &str,
) -> String {
    let uri = format!("/2015-03-31/functions/{function_name}/configuration");
    let mut last = String::new();
    for _ in 0..200 {
        let (_, configuration) = lambda(router, "GET", &uri, None).await;
        last = configuration
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if last == wanted {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    last
}

/// One row of the table: a template, and what the rule says the function it declares must
/// then be in Lambda.
struct Row {
    case: &'static str,
    /// The logical id the row declares its function under, which is the resource the stack
    /// is read by and the one the `aws:cloudformation:logical-id` tag names.
    logical_id: &'static str,
    /// The role this row creates in IAM BEFORE the stack, for a row whose template names an
    /// execution role provisioned outside it.
    external_role: Option<&'static str>,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The name the template declares, or `None` for a template that names none — whose
    /// function carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The configuration members `GetFunction` must report for the function, each as the
    /// declaration states it.
    expected_configuration: &'static [(&'static str, &'static str)],
    /// Whether the package was declared INLINE, so Lambda holds bytes for it and reports a
    /// code size and digest of its own.
    inline_package: bool,
    /// The reserved concurrency `GetFunctionConcurrency` must report, for a row that declares
    /// it — the property that is `PutFunctionConcurrency`'s member rather than the create's.
    expected_reserved_concurrency: Option<i64>,
    /// The recursive-loop configuration `GetFunctionRecursionConfig` must report, for a row
    /// that declares it.
    expected_recursive_loop: Option<&'static str>,
    /// The tags `lambda list-tags` must report for the function, beside the three
    /// CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named Python function with inline code and a role the template declares",
        logical_id: "Handler",
        external_role: None,
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "ExecRole": {
                        "Type": "AWS::IAM::Role",
                        "Properties": {
                            "RoleName": "orders-exec",
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
                    "Handler": {
                        "Type": "AWS::Lambda::Function",
                        "Properties": {
                            "FunctionName": "orders-handler",
                            "Runtime": "python3.12",
                            "Handler": "index.handler",
                            "Role": { "Fn::GetAtt": ["ExecRole", "Arn"] },
                            "Timeout": 15,
                            "MemorySize": 256,
                            "Description": "answers an order",
                            "Environment": { "Variables": { "TABLE": "orders" } },
                            "Code": {
                                "ZipFile": "def handler(event, context):\n    return {'ok': True}\n"
                            },
                            "Tags": [{ "Key": "Owner", "Value": "orders" }]
                        }
                    }
                },
                "Outputs": {
                    "FunctionRef": { "Value": { "Ref": "Handler" } },
                    "FunctionArn": { "Value": { "Fn::GetAtt": ["Handler", "Arn"] } }
                }
            })
            .to_string()
        },
        declared_name: Some("orders-handler"),
        expected_configuration: &[
            ("Runtime", "python3.12"),
            ("Handler", "index.handler"),
            ("Description", "answers an order"),
        ],
        inline_package: true,
        expected_reserved_concurrency: None,
        expected_recursive_loop: None,
        expected_tags: &[("Owner", "orders")],
    },
    Row {
        case: "a YAML template naming no function, an S3 package, and the two properties \
               that are other calls' members",
        logical_id: "Worker",
        external_role: None,
        template: || {
            [
                "Resources:",
                "  ExecRole:",
                "    Type: AWS::IAM::Role",
                "    Properties:",
                "      RoleName: billing-exec",
                "      AssumeRolePolicyDocument:",
                "        Version: '2012-10-17'",
                "        Statement:",
                "          - Effect: Allow",
                "            Principal:",
                "              Service: lambda.amazonaws.com",
                "            Action: sts:AssumeRole",
                "  Worker:",
                "    Type: AWS::Lambda::Function",
                "    Properties:",
                "      Runtime: nodejs20.x",
                "      Handler: index.handler",
                "      Role: !GetAtt ExecRole.Arn",
                "      ReservedConcurrentExecutions: 5",
                "      RecursiveLoop: Allow",
                "      Code:",
                "        S3Bucket: deployments",
                "        S3Key: functions/worker.zip",
                "Outputs:",
                "  FunctionRef:",
                "    Value: !Ref Worker",
                "  FunctionArn:",
                "    Value: !GetAtt Worker.Arn",
                "",
            ]
            .join("\n")
        },
        declared_name: None,
        expected_configuration: &[("Runtime", "nodejs20.x"), ("Handler", "index.handler")],
        inline_package: false,
        expected_reserved_concurrency: Some(5),
        expected_recursive_loop: Some("Allow"),
        expected_tags: &[],
    },
    Row {
        case: "a Ruby function on arm64 whose execution role was provisioned outside the \
               template",
        logical_id: "Reporter",
        external_role: Some("reports-exec"),
        template: || {
            json!({
                "Resources": {
                    "Reporter": {
                        "Type": "AWS::Lambda::Function",
                        "Properties": {
                            "FunctionName": "reports-handler",
                            "Runtime": "ruby3.3",
                            "Handler": "index.handler",
                            "PackageType": "Zip",
                            "Role": "arn:aws:iam::000000000000:role/reports-exec",
                            "Architectures": ["arm64"],
                            "EphemeralStorage": { "Size": 1024 },
                            "MemorySize": 512,
                            "Timeout": 30,
                            "Code": {
                                "ZipFile": "def handler(event:, context:)\n  { ok: true }\nend\n"
                            },
                            "Tags": [
                                { "Key": "Owner", "Value": "reports" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "FunctionRef": { "Value": { "Ref": "Reporter" } },
                    "FunctionArn": { "Value": { "Fn::GetAtt": ["Reporter", "Arn"] } }
                }
            })
            .to_string()
        },
        declared_name: Some("reports-handler"),
        expected_configuration: &[("Runtime", "ruby3.3"), ("Handler", "index.handler")],
        inline_package: true,
        expected_reserved_concurrency: None,
        expected_recursive_loop: None,
        expected_tags: &[("Owner", "reports"), ("Tier", "prod")],
    },
];

/// Every row: the stack completes with the function CREATED in Lambda, the name the stack
/// publishes is a function `GetFunction` resolves with the configuration the template
/// declared, the properties that are other Lambda calls' members were applied through those
/// calls, Lambda reports the function `Active` once its own activation span is due, the tags
/// CloudFormation gives the resource are on it, and deleting the stack deletes the function
/// there.
#[tokio::test]
async fn a_stack_provisions_the_lambda_function_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("lambda-function-stack-{index}");

        if let Some(role) = row.external_role {
            let (status, body) = iam(
                &router,
                "CreateRole",
                &[
                    ("RoleName", role),
                    ("AssumeRolePolicyDocument", &trust_policy("lambda.amazonaws.com")),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the execution role to be created in IAM, got {status} \
                 with body {body}"
            );
        }

        let template = (row.template)();
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                // A template that declares an IAM resource with a NAME of its own is
                // acknowledged with this capability, which is what CloudFormation requires
                // of the caller before it creates one.
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (status, described) =
            wait_for_stack_status(&router, &stack_name, CREATE_COMPLETE).await;
        // A resource that failed carries the owning service's own refusal in the stack's
        // event stream, which is what says WHY a stack that did not complete did not.
        let events = if status == CREATE_COMPLETE {
            Vec::new()
        } else {
            let (_, body) =
                cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack_name)]).await;
            elements(&body, "ResourceStatusReason")
        };
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {described} with \
             resource failures {events:?}"
        );

        let (status, resources) = cloudformation(
            &router,
            "DescribeStackResources",
            &[
                ("StackName", &stack_name),
                ("LogicalResourceId", row.logical_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));

        // "Ref returns the function name": the physical id is that name, which is the one
        // the template asked for — or, for a template that named none, the one
        // CloudFormation generated.
        if let Some(declared) = row.declared_name {
            assert_eq!(
                physical_id, declared,
                "case {case:?}: expected the physical id to be the declared function name \
                 {declared:?}, got {physical_id:?}"
            );
        } else {
            assert!(
                !physical_id.is_empty() && physical_id.len() <= 64,
                "case {case:?}: expected a generated name inside the type's 64-character \
                 ceiling, got {physical_id:?}"
            );
        }
        let arn = function_arn(&physical_id);

        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("FunctionRef"),
            Some(&physical_id),
            "case {case:?}: expected the Ref output to be the function name {physical_id:?}, \
             got {:?}",
            outputs.get("FunctionRef")
        );
        assert_eq!(
            outputs.get("FunctionArn"),
            Some(&arn),
            "case {case:?}: expected the Arn output to be {arn:?}, got {:?}",
            outputs.get("FunctionArn")
        );

        // And Lambda itself resolves that name to the function the template declared.
        let (status, function) =
            lambda(&router, "GET", &format!("/2015-03-31/functions/{physical_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFunction on the name the stack \
             published, got {status} with body {function}"
        );
        let configuration = function
            .get("Configuration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            configuration.get("FunctionArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the function's ARN to be {arn}, got {configuration}"
        );
        // Lambda's own activation span is that service's, not the stack's: the function
        // exists from the moment the create answers, and `State` moves to `Active` once
        // Lambda's clock makes the move due — so the state is polled for rather than
        // demanded of the instant the stack completed.
        let state = wait_for_function_state(&router, &physical_id, "Active").await;
        assert_eq!(
            state, "Active",
            "case {case:?}: expected Lambda to report the created function Active once its \
             own activation span was due, got {state:?}"
        );
        for (member, expected) in row.expected_configuration {
            assert_eq!(
                configuration.get(*member).and_then(Value::as_str),
                Some(*expected),
                "case {case:?}: expected {member} to be {expected:?}, got {configuration}"
            );
        }
        let role = configuration
            .get("Role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            role.starts_with(&format!("arn:aws:iam::{ACCOUNT}:role/")),
            "case {case:?}: expected the stored execution role to be a role ARN of this \
             account, got {role:?}"
        );
        if let Some(external) = row.external_role {
            assert_eq!(
                role,
                role_arn(external),
                "case {case:?}: expected the declared role ARN to be stored as written, got \
                 {role:?}"
            );
        }
        if row.inline_package {
            // The inline source reached Lambda as a package it holds bytes for, so it
            // reports a size and a digest it computed over them.
            let code_size = configuration
                .get("CodeSize")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            assert!(
                code_size > 0,
                "case {case:?}: expected a code size over the archived inline source, got \
                 {code_size}"
            );
            assert!(
                configuration
                    .get("CodeSha256")
                    .and_then(Value::as_str)
                    .is_some_and(|digest| !digest.is_empty()),
                "case {case:?}: expected a CodeSha256 over the archived inline source, got \
                 {configuration}"
            );
        }
        if let Some(expected) = row.expected_reserved_concurrency {
            let (status, concurrency) = lambda(
                &router,
                "GET",
                &format!("/2019-09-30/functions/{physical_id}/concurrency"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetFunctionConcurrency, got {status} \
                 with body {concurrency}"
            );
            assert_eq!(
                concurrency
                    .get("ReservedConcurrentExecutions")
                    .and_then(Value::as_i64),
                Some(expected),
                "case {case:?}: expected the declared reserved concurrency {expected}, got \
                 {concurrency}"
            );
        }
        if let Some(expected) = row.expected_recursive_loop {
            let (status, recursion) = lambda(
                &router,
                "GET",
                &format!("/2024-08-31/functions/{physical_id}/recursion-config"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from GetFunctionRecursionConfig, got \
                 {status} with body {recursion}"
            );
            assert_eq!(
                recursion.get("RecursiveLoop").and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected the declared recursive-loop configuration \
                 {expected:?}, got {recursion}"
            );
        }

        // The tags the declaration asked for, read through Lambda's own tagging call.
        // Lambda refuses a tag key beginning with `aws:` from any caller, so the three
        // records CloudFormation keeps about a resource it provisioned never reach the
        // function and nothing in the reserved namespace is on it.
        let (status, listed) =
            lambda(&router, "GET", &format!("/2017-03-31/tags/{arn}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTags, got {status} with body {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no 'Tags' map in {listed}"))
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.len(),
            row.expected_tags.len(),
            "case {case:?}: expected exactly the {} tag(s) the template declared, got \
             {reported:?}",
            row.expected_tags.len()
        );

        // Deleting the stack deletes the function Lambda holds for it.
        let (status, body) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) =
            lambda(&router, "GET", &format!("/2015-03-31/functions/{physical_id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetFunction once the stack deleted the \
             function, got {status} with body {gone}"
        );
    }
}

/// One row of the execution-role table: the role a template names, and whether the rule
/// says the stack's function can be created with it.
struct RoleRow {
    case: &'static str,
    /// The role this row creates in IAM beforehand, with the service principal its trust
    /// policy admits — `None` for a row whose ARN names a role IAM holds nothing under.
    held_role: Option<(&'static str, &'static str)>,
    /// The `Role` the template declares.
    declared_role: &'static str,
    /// Whether the stack reaches `CREATE_COMPLETE` (so the function exists) or is rolled
    /// back (so nothing was created under the name the template asked for).
    created: bool,
}

/// The rows differ along WHAT IAM answers about the declared role: a role it holds whose
/// trust admits the service principal, one whose trust admits another principal, a name it
/// holds no role under, and a role of another account — which this emulator's IAM cannot be
/// asked about at all.
const ROLE_ROWS: &[RoleRow] = &[
    RoleRow {
        case: "a role this account holds whose trust policy admits Lambda",
        held_role: Some(("assumable-exec", "lambda.amazonaws.com")),
        declared_role: "arn:aws:iam::000000000000:role/assumable-exec",
        created: true,
    },
    RoleRow {
        case: "a role this account holds whose trust policy admits another service",
        held_role: Some(("ec2-only-exec", "ec2.amazonaws.com")),
        declared_role: "arn:aws:iam::000000000000:role/ec2-only-exec",
        created: false,
    },
    RoleRow {
        case: "an ARN naming a role of this account that IAM holds nothing under",
        held_role: None,
        declared_role: "arn:aws:iam::000000000000:role/never-created-exec",
        created: false,
    },
    RoleRow {
        case: "a role of ANOTHER account, which this emulator's IAM cannot be asked about",
        held_role: None,
        declared_role: "arn:aws:iam::123456789012:role/remote-exec",
        created: true,
    },
];

/// A function whose declared execution role cannot be handed to Lambda does not leave a
/// stack reporting a resource nothing created: the stack fails and is rolled back, carrying
/// the refusal, and no function exists under the name the template asked for — while a role
/// nothing is known about is handed over and the function is created with it.
#[tokio::test]
async fn a_function_whose_execution_role_cannot_be_passed_fails_the_stack() {
    let router = build_router();

    for (index, row) in ROLE_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("lambda-function-role-{index}");
        let function_name = format!("role-case-{index}-handler");

        if let Some((role, principal)) = row.held_role {
            let (status, body) = iam(
                &router,
                "CreateRole",
                &[
                    ("RoleName", role),
                    ("AssumeRolePolicyDocument", &trust_policy(principal)),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the role to be created in IAM, got {status} with body \
                 {body}"
            );
        }

        let template = json!({
            "Resources": {
                "Handler": {
                    "Type": "AWS::Lambda::Function",
                    "Properties": {
                        "FunctionName": function_name,
                        "Runtime": "python3.12",
                        "Handler": "index.handler",
                        "Role": row.declared_role,
                        "Code": { "ZipFile": "def handler(event, context):\n    return 1\n" }
                    }
                }
            }
        })
        .to_string();
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let wanted = if row.created {
            CREATE_COMPLETE
        } else {
            ROLLBACK_COMPLETE
        };
        let (status, described) = wait_for_stack_status(&router, &stack_name, wanted).await;
        assert_eq!(
            status, wanted,
            "case {case:?}: expected the stack to reach {wanted}, got {described}"
        );

        let (status, read) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{function_name}"),
            None,
        )
        .await;
        if row.created {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the function to exist once the stack completed, got \
                 {status} with body {read}"
            );
        } else {
            let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
            assert!(
                reason.contains("Handler"),
                "case {case:?}: expected the failure to name the resource that failed, got \
                 {reason:?}"
            );
            assert_eq!(
                status, 404,
                "case {case:?}: expected no function to exist for the failed resource, got \
                 {status} with body {read}"
            );
        }
    }
}
