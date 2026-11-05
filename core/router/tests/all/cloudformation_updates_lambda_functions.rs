// SPDX-License-Identifier: CC0-1.0

//! An `UpdateStack` keeps the `AWS::Lambda::Function` a stack declares in step with the
//! declaration, in AWS Lambda itself.
//!
//! The rule under test: **a resource of a type this emulator provisions stays owned by the
//! service that owns it across EVERY deployment of the stack, not only the first — an update
//! applies the changed declaration to that service through its own update calls, leaves a
//! resource the declaration did not change alone, creates what the update ADDS and replaces
//! the resource whose identity the declaration moved — so the physical id a completed update
//! publishes for an `AWS::Lambda::Function` is still one AWS Lambda resolves, and what that
//! service reports for it is what the latest template declared.**
//!
//! `cloudformation_provisions_lambda_functions.rs` states the same rule for the FIRST
//! deployment; this suite is the update half, which is a different code path: the create
//! calls `CreateFunction`, while an update calls `UpdateFunctionConfiguration` and
//! `UpdateFunctionCode` on the function the stack already has — and a stack whose function
//! survived its create but vanished on an update is exactly as broken for a client that
//! configures, waits on or invokes it.
//!
//! The rows differ along WHAT the update changes: nothing about the function at all (only a
//! sibling resource it depends on), the function's own configuration members, its deployment
//! package, its NAME — which is its identity, so the resource is replaced rather than edited
//! — and the number of functions the template declares. Every expected value is computed
//! here from what the row declares, never read back out of the answer being checked; the one
//! value read before the update is the package digest, because what the rule says about it is
//! that it MOVED (or did not), not what it is.
//!
//! Whether the deployment package then runs to completion is AWS Lambda's own runtime, whose
//! gaps are their own issues: what is asserted of an invocation here is that the published id
//! RESOLVES to a function — a `DryRun` is accepted and a `RequestResponse` is served — rather
//! than being refused as a function nothing created.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// Polling a stack until it first reports a status, and the awsQuery form encoder and the
// answer readers, are written ONCE in this crate.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, outputs_of, physical_ids_of};

/// The Region every request here is signed for — the Region `wait_for_stack_status` polls in
/// — and so the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The logical id every row declares its function under, and the one its execution role
/// carries.
const FUNCTION_LOGICAL_ID: &str = "Fn";
const ROLE_LOGICAL_ID: &str = "ExecRole";

/// The logical id a row whose update ADDS a function declares the new one under.
const SECOND_LOGICAL_ID: &str = "Second";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them: CloudFormation makes the calls, Lambda serves
/// them, IAM answers for the execution role.
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

/// Call one Lambda operation over its restJson1 wire, with the headers an invocation needs.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let payload = match body {
        Some(document) => serde_json::to_vec(document).expect("the request body serializes"),
        None => Vec::new(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// What `GetFunctionConfiguration` reports for the function `name`, with the status it
/// answered: the reader this suite states every expectation about the function through, so
/// nothing is read out of the deployment's own answer.
async fn reported_configuration(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        None,
        &[],
    )
    .await
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, and
/// answer the configuration it reported then.
///
/// The span a function spends `Pending` is AWS Lambda's own, so no fixed sleep names the
/// instant it ends; the budget is long enough that a loaded machine is slower than the span
/// that service chooses without the rule being any different.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        let (_, configuration) = reported_configuration(router, name).await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        last = configuration;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    last
}

/// A trust policy that lets the principal `service` assume the role, which is what an
/// execution role has to carry for Lambda to be handed it.
fn trust_policy(service: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// An inline policy document granting `action` on every resource, which is what a stack
/// update adding a permission to the function's execution role declares.
fn permission_policy(name: &str, action: &str) -> Value {
    json!([{
        "PolicyName": name,
        "PolicyDocument": {
            "Version": "2012-10-17",
            "Statement": [{ "Effect": "Allow", "Action": [action], "Resource": "*" }]
        }
    }])
}

/// The template a row deploys: the execution role the function runs as — carrying the inline
/// policies this stage declares — the function itself, and optionally a SECOND function the
/// stage adds; publishing the function's `Ref` and its ARN as the stack's outputs.
fn template(
    role_name: &str,
    policies: Option<Value>,
    function: Value,
    second: Option<Value>,
) -> String {
    let mut role_properties = json!({
        "RoleName": role_name,
        "AssumeRolePolicyDocument": trust_policy("lambda.amazonaws.com"),
    });
    if let Some(policies) = policies {
        role_properties["Policies"] = policies;
    }
    let mut resources = json!({
        ROLE_LOGICAL_ID: { "Type": "AWS::IAM::Role", "Properties": role_properties },
        FUNCTION_LOGICAL_ID: { "Type": "AWS::Lambda::Function", "Properties": function },
    });
    if let Some(properties) = second {
        resources[SECOND_LOGICAL_ID] =
            json!({ "Type": "AWS::Lambda::Function", "Properties": properties });
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": resources,
        "Outputs": {
            "FunctionRef": { "Value": { "Ref": FUNCTION_LOGICAL_ID } },
            "FunctionArn": { "Value": { "Fn::GetAtt": [FUNCTION_LOGICAL_ID, "Arn"] } }
        }
    })
    .to_string()
}

/// The properties of one function declaration: the members every row carries, plus the ones
/// its stage varies.
fn function_properties(
    name: &str,
    description: &str,
    timeout: i64,
    memory: i64,
    environment: &[(&str, &str)],
    source: &str,
) -> Value {
    let variables: BTreeMap<&str, &str> = environment.iter().copied().collect();
    json!({
        "FunctionName": name,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Role": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] },
        "Description": description,
        "Timeout": timeout,
        "MemorySize": memory,
        "Environment": { "Variables": variables },
        "Code": { "ZipFile": source }
    })
}

/// The ARN a function of `name` carries in this suite's Region: the same total function of
/// the Region and the name both services compute it from.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The ARN a role of `name` carries, which is the value an execution role is named by.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// A handler that needs nothing of its runtime beyond the language itself, so what an
/// invocation of it says is whether the function RESOLVED.
const FIRST_SOURCE: &str = "def handler(event, context):\n    return {'stage': 'first'}\n";

/// A different package: the row that changes the deployment package declares this one, so
/// the digest Lambda computes over it has to move.
const SECOND_SOURCE: &str =
    "def handler(event, context):\n    reply = {'stage': 'second'}\n    return reply\n";

/// One row of the table: the stack the row creates, the update it then deploys, and what the
/// rule says Lambda must report once that update completed.
struct Row {
    case: &'static str,
    /// The role name both of this row's templates declare. Distinct per row: every row
    /// deploys into the one account this suite's engine holds.
    role: &'static str,
    /// The template the row creates the stack with.
    create: fn() -> String,
    /// The template the row then updates the stack with.
    update: fn() -> String,
    /// The function name the create declares, which is the physical id the stack publishes
    /// for it.
    created_name: &'static str,
    /// The name the UPDATE declares for the same logical resource: the same name for an
    /// in-place update, a different one for the row whose update moves the function's
    /// identity and so replaces it.
    updated_name: &'static str,
    /// The text configuration members Lambda must report after the update, each as the
    /// updated declaration states it.
    expected_text: &'static [(&'static str, &'static str)],
    /// The numeric ones, likewise.
    expected_numbers: &'static [(&'static str, i64)],
    /// The environment variables it must report after the update.
    expected_environment: &'static [(&'static str, &'static str)],
    /// Whether the update declared a DIFFERENT deployment package, so the digest Lambda
    /// reports has to have moved — and, for a row that did not, has to be the one it was.
    code_changed: bool,
    /// A function the update ADDS, which Lambda must then hold as well.
    added_function: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        // The recorded case: the update adds a permission to the role the function runs as
        // and touches the function's own declaration not at all.
        case: "an update that changes only the sibling execution role the function runs as",
        role: "upd-sibling-exec",
        create: || {
            template(
                "upd-sibling-exec",
                None,
                function_properties(
                    "upd-sibling-fn",
                    "reads the index",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        update: || {
            template(
                "upd-sibling-exec",
                Some(permission_policy("allow-retrieve", "bedrock:Retrieve")),
                function_properties(
                    "upd-sibling-fn",
                    "reads the index",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        created_name: "upd-sibling-fn",
        updated_name: "upd-sibling-fn",
        expected_text: &[
            ("Runtime", "python3.12"),
            ("Handler", "index.handler"),
            ("Description", "reads the index"),
        ],
        expected_numbers: &[("Timeout", 10), ("MemorySize", 192)],
        expected_environment: &[("STAGE", "one")],
        code_changed: false,
        added_function: None,
    },
    Row {
        case: "an update that changes the function's own configuration members",
        role: "upd-config-exec",
        create: || {
            template(
                "upd-config-exec",
                None,
                function_properties(
                    "upd-config-fn",
                    "the first description",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        update: || {
            template(
                "upd-config-exec",
                None,
                function_properties(
                    "upd-config-fn",
                    "the second description",
                    45,
                    512,
                    &[("STAGE", "two"), ("SHARDS", "4")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        created_name: "upd-config-fn",
        updated_name: "upd-config-fn",
        expected_text: &[
            ("Runtime", "python3.12"),
            ("Handler", "index.handler"),
            ("Description", "the second description"),
        ],
        expected_numbers: &[("Timeout", 45), ("MemorySize", 512)],
        expected_environment: &[("STAGE", "two"), ("SHARDS", "4")],
        code_changed: false,
        added_function: None,
    },
    Row {
        case: "an update that changes the inline deployment package",
        role: "upd-code-exec",
        create: || {
            template(
                "upd-code-exec",
                None,
                function_properties(
                    "upd-code-fn",
                    "archives the package",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        update: || {
            template(
                "upd-code-exec",
                None,
                function_properties(
                    "upd-code-fn",
                    "archives the package",
                    10,
                    192,
                    &[("STAGE", "one")],
                    SECOND_SOURCE,
                ),
                None,
            )
        },
        created_name: "upd-code-fn",
        updated_name: "upd-code-fn",
        expected_text: &[
            ("Runtime", "python3.12"),
            ("Handler", "index.handler"),
            ("Description", "archives the package"),
        ],
        expected_numbers: &[("Timeout", 10), ("MemorySize", 192)],
        expected_environment: &[("STAGE", "one")],
        code_changed: true,
        added_function: None,
    },
    Row {
        case: "an update that renames the function, which is the resource's identity",
        role: "upd-renamed-exec",
        create: || {
            template(
                "upd-renamed-exec",
                None,
                function_properties(
                    "upd-renamed-fn-one",
                    "carries the name",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        update: || {
            template(
                "upd-renamed-exec",
                None,
                function_properties(
                    "upd-renamed-fn-two",
                    "carries the name",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        created_name: "upd-renamed-fn-one",
        updated_name: "upd-renamed-fn-two",
        expected_text: &[
            ("Runtime", "python3.12"),
            ("Handler", "index.handler"),
            ("Description", "carries the name"),
        ],
        expected_numbers: &[("Timeout", 10), ("MemorySize", 192)],
        expected_environment: &[("STAGE", "one")],
        code_changed: false,
        added_function: None,
    },
    Row {
        case: "an update that adds a second function to the same stack",
        role: "upd-added-exec",
        create: || {
            template(
                "upd-added-exec",
                None,
                function_properties(
                    "upd-added-fn",
                    "the one the create declared",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                None,
            )
        },
        update: || {
            template(
                "upd-added-exec",
                None,
                function_properties(
                    "upd-added-fn",
                    "the one the create declared",
                    10,
                    192,
                    &[("STAGE", "one")],
                    FIRST_SOURCE,
                ),
                Some(function_properties(
                    "upd-added-second-fn",
                    "the one the update added",
                    20,
                    256,
                    &[("STAGE", "two")],
                    SECOND_SOURCE,
                )),
            )
        },
        created_name: "upd-added-fn",
        updated_name: "upd-added-fn",
        expected_text: &[
            ("Runtime", "python3.12"),
            ("Handler", "index.handler"),
            ("Description", "the one the create declared"),
        ],
        expected_numbers: &[("Timeout", 10), ("MemorySize", 192)],
        expected_environment: &[("STAGE", "one")],
        code_changed: false,
        added_function: Some("upd-added-second-fn"),
    },
];

/// Every row: the update completes, the physical id the stack publishes for the function is
/// one Lambda resolves, what Lambda reports for it is what the updated template declared, a
/// renamed function is a new one with the old gone, and a function the update added is held
/// too.
#[tokio::test]
async fn an_update_keeps_the_declared_lambda_function_in_step() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("lambda-function-update-stack-{index}");

        // ---- The create: the stack the update is deployed against. ----
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &(row.create)()),
                // The template declares a NAMED IAM role, which is what this capability
                // acknowledges.
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        let (status, described) = wait_for_stack_status(&router, &stack_name, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE} before the update, \
             got {described}"
        );

        let created_id = published_id(&router, case, &stack_name, FUNCTION_LOGICAL_ID).await;
        assert_eq!(
            created_id, row.created_name,
            "case {case:?}: expected the create to publish the declared function name {:?}, \
             got {created_id:?}",
            row.created_name
        );
        let before = wait_until_active(&router, &created_id).await;
        let digest_before = before
            .get("CodeSha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !digest_before.is_empty(),
            "case {case:?}: expected Lambda to report a digest over the created package, got \
             {before}"
        );

        // ---- The update. ----
        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &(row.update)()),
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );
        let (status, described) = wait_for_stack_status(&router, &stack_name, UPDATE_COMPLETE).await;
        let reasons = if status == UPDATE_COMPLETE {
            Vec::new()
        } else {
            let (_, events) =
                cloudformation(&router, "DescribeStackEvents", &[("StackName", &stack_name)]).await;
            crate::query_wire::elements(&events, "ResourceStatusReason")
        };
        assert_eq!(
            status, UPDATE_COMPLETE,
            "case {case:?}: expected the stack to reach {UPDATE_COMPLETE}, got {described} with \
             resource failures {reasons:?}"
        );

        // ---- What the completed update publishes, and what Lambda holds for it. ----
        let published = published_id(&router, case, &stack_name, FUNCTION_LOGICAL_ID).await;
        assert_eq!(
            published, row.updated_name,
            "case {case:?}: expected the update to publish the name the template now declares \
             ({:?}), got {published:?}",
            row.updated_name
        );
        let arn = function_arn(&published);
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("FunctionRef"),
            Some(&published),
            "case {case:?}: expected the Ref output to be {published:?}, got {:?}",
            outputs.get("FunctionRef")
        );
        assert_eq!(
            outputs.get("FunctionArn"),
            Some(&arn),
            "case {case:?}: expected the Arn output to be {arn:?}, got {:?}",
            outputs.get("FunctionArn")
        );

        let (status, configuration) = reported_configuration(&router, &published).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Lambda to resolve the function the completed update \
             published ({published:?}), got {status} with body {configuration}"
        );
        assert_eq!(
            configuration.get("FunctionArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the function's ARN to be {arn}, got {configuration}"
        );
        for (member, expected) in row.expected_text {
            assert_eq!(
                configuration.get(*member).and_then(Value::as_str),
                Some(*expected),
                "case {case:?}: expected {member} to be {expected:?} after the update, got \
                 {configuration}"
            );
        }
        for (member, expected) in row.expected_numbers {
            assert_eq!(
                configuration.get(*member).and_then(Value::as_i64),
                Some(*expected),
                "case {case:?}: expected {member} to be {expected} after the update, got \
                 {configuration}"
            );
        }
        let reported_environment: BTreeMap<String, String> = configuration
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
            .unwrap_or_default();
        let expected_environment: BTreeMap<String, String> = row
            .expected_environment
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported_environment, expected_environment,
            "case {case:?}: expected the environment {expected_environment:?} after the update, \
             got {reported_environment:?}"
        );
        // The role is the one the template's own role resource carries, resolved through
        // `Fn::GetAtt`.
        assert_eq!(
            configuration.get("Role").and_then(Value::as_str),
            Some(role_arn(row.role).as_str()),
            "case {case:?}: expected the execution role {:?}, got {configuration}",
            role_arn(row.role)
        );

        // The package: a declaration that changed it moved the digest Lambda computes over
        // the archive; one that did not leaves it where it was.
        let active = wait_until_active(&router, &published).await;
        let digest_after = active
            .get("CodeSha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if row.code_changed {
            assert!(
                !digest_after.is_empty() && digest_after != digest_before,
                "case {case:?}: expected the digest over the updated package to differ from \
                 {digest_before:?}, got {digest_after:?}"
            );
        } else {
            assert_eq!(
                digest_after, digest_before,
                "case {case:?}: expected the digest to stay {digest_before:?} for an update \
                 that declared the same package, got {digest_after:?}"
            );
        }

        // A function whose NAME the update moved is a different resource: the stack holds
        // the new one and the old one is gone from Lambda.
        if published != created_id {
            let (status, answered) = reported_configuration(&router, &created_id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the replaced function {created_id:?} to be gone from \
                 Lambda, got {status} with body {answered}"
            );
        }

        // A function the update ADDED is created in Lambda by the same deployment.
        if let Some(added) = row.added_function {
            let published_second =
                published_id(&router, case, &stack_name, SECOND_LOGICAL_ID).await;
            assert_eq!(
                published_second, added,
                "case {case:?}: expected the added resource to publish {added:?}, got \
                 {published_second:?}"
            );
            let (status, configuration) = reported_configuration(&router, added).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Lambda to hold the function the update added \
                 ({added:?}), got {status} with body {configuration}"
            );
        }

        // And the id the completed update published is one an invocation resolves: the
        // request-validating `DryRun` is accepted, and a `RequestResponse` is served by
        // Lambda rather than refused as a function nothing created.
        let invocations = format!("/2015-03-31/functions/{published}/invocations");
        let (status, answered) = lambda(
            &router,
            "POST",
            &invocations,
            Some(&json!({ "probe": true })),
            &[("x-amz-invocation-type", "DryRun")],
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected a DryRun invocation of {published:?} to be accepted, got \
             {status} with body {answered}"
        );
        let (status, answered) = lambda(
            &router,
            "POST",
            &invocations,
            Some(&json!({ "probe": true })),
            &[("x-amz-invocation-type", "RequestResponse")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Lambda to serve an invocation of {published:?}, got \
             {status} with body {answered}"
        );
        assert!(
            !answered.to_string().contains("Function not found"),
            "case {case:?}: expected the invocation to reach the function the update \
             published, got {answered}"
        );
    }
}

/// The physical id the stack publishes for ONE of its resources, read through
/// `DescribeStackResources` — the hand-off a client takes the function's name from.
async fn published_id(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    logical_id: &str,
) -> String {
    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    physical_ids_of(&resources)
        .remove(logical_id)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no physical id for {logical_id:?} in {resources}")
        })
}
