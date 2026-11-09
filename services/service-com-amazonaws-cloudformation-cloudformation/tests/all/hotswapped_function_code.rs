// SPDX-License-Identifier: CC0-1.0

//! A deployment package uploaded DIRECTLY to AWS Lambda for a function a stack provisioned —
//! the hotswap a deployment tool performs when a change needs no new template — and the stack
//! update that carries a change no such upload can apply.
//!
//! The rule every row here holds the emulator to: **a data-plane write to a resource a stack
//! provisioned is applied by the service that OWNS the resource and changes nothing about the
//! stack that provisioned it — so the next read and the next run of the function see the
//! package `UpdateFunctionCode` stored, while the stack keeps its status, its template, its
//! resource records and its event timeline — and a later `UpdateStack` whose template states a
//! different value for a property of that same resource applies it through the owning
//! service's own update API and reaches `UPDATE_COMPLETE`, with the resource keeping its
//! identity.**
//!
//! That is one rule in two halves, which is how the workflow this suite was written for uses
//! it: the code-only change goes straight to `lambda:UpdateFunctionCode` and never touches
//! CloudFormation, and the change that is not a code change (a `Timeout`, say) falls back to a
//! full stack update. A hotswap whose bytes never become the running code leaves a developer
//! watching stale code answer a successful upload; a fallback that does not reach
//! `UPDATE_COMPLETE` leaves a configuration change silently unapplied.
//!
//! The rows differ along what the rule generalises over: what the hotswapped handler COMPUTES
//! (a re-valued object, a list, a value derived from the invocation's event, a bare string),
//! and which non-code property the fallback update changes and to what — `Timeout` at the
//! bottom of its published range (1 second), at the top of AWS's published function-timeout
//! quota (900 seconds), at a value in between, and `Timeout` together with `MemorySize`. Every
//! expected value is computed here from what the row declares; the one value read before the
//! hotswap is the package digest, because what the rule says about it is that it MOVED.

use std::io::Write as _;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, router_with_engine,
    wait_for_the_transition, xml_blocks, xml_text, REGION,
};

/// The runtime every row declares, and so the extension of the one source file in the
/// deployment package: a Python handler of `index.handler` is `index.py`.
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";
const SOURCE_FILE: &str = "index.py";

/// The logical ids every row's template declares its two resources under.
const FUNCTION_LOGICAL_ID: &str = "Fn";
const ROLE_LOGICAL_ID: &str = "ExecRole";

/// The event every invocation here carries, so a handler that computes its answer FROM the
/// event has one to compute from.
fn event() -> Value {
    json!({ "n": 7, "probe": "hotswap" })
}

/// What one Lambda request answered: the status, the headers (the function-error signal lives
/// there) and the body as JSON.
///
/// `pub(crate)` with [`lambda`] because more than one suite of this target reads a function a
/// stack provisioned back through Lambda's own wire, and there is one spelling of that call:
/// `cdk_asset_function_writes_its_granted_table.rs` invokes the function a CDK-shaped deploy
/// created through it.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: Value,
}

impl Answer {
    /// The `X-Amz-Function-Error` signal, which an invocation that RAN to completion carries
    /// none of.
    pub(crate) fn function_error(&self) -> Option<String> {
        self.headers
            .get("x-amz-function-error")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// Call one AWS Lambda operation over its restJson1 wire, signed for this suite's Region.
pub(crate) async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("a well-formed Lambda request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: parsed,
    }
}

/// A .zip deployment package holding `source` as the one entry `file` names, written in memory
/// with the maintained `zip` implementation the way a caller uploads one.
///
/// `pub(crate)` because more than one suite of this target archives a package: the suite over
/// a CDK-shaped deploy stages one as an Amazon S3 asset object
/// (`cdk_asset_function_writes_its_granted_table.rs`), so there is ONE archive writer here.
pub(crate) fn package(file: &str, source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file(file, options)
            .expect("the archive entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the archive entry's bytes are written");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// [`package`] holding the one file the declared handler resolves, base64-encoded — because
/// `ZipFile` is a blob and a JSON request carries a blob encoded.
fn encoded_package(source: &str) -> String {
    aws_smithy_types::base64::encode(package(SOURCE_FILE, source))
}

/// What `GetFunctionConfiguration` reports for `name` right now.
async fn reported_configuration(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Value {
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        &[],
        &[],
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected AWS Lambda to hold the function {name:?}, got {} with body {}",
        answered.status, answered.body
    );
    answered.body
}

/// The `CodeSha256` AWS Lambda reports for `name`: the digest over the package it holds.
async fn reported_digest(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    reported_configuration(router, case, name)
        .await
        .get("CodeSha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Invoke `name` synchronously with [`event`], asserting only that the invocation was SERVED,
/// and answer the payload the run produced.
async fn invoked(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let answered = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/invocations"),
        event().to_string().as_bytes(),
        &[("x-amz-invocation-type", "RequestResponse")],
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected AWS Lambda to serve an invocation of {name:?}, got {} with \
         body {}",
        answered.status, answered.body
    );
    assert_eq!(
        answered.function_error(),
        None,
        "case {case:?}: expected the handler of {name:?} to run to completion, got the error \
         document {}",
        answered.body
    );
    answered.body
}

/// The capabilities a template declaring a NAMED IAM role has to be submitted with.
fn capabilities() -> Vec<(String, String)> {
    vec![member("Capabilities.member.1", "CAPABILITY_NAMED_IAM")]
}

/// Every `(logical id, resource status, physical id)` `DescribeStackResources` reports for a
/// stack, in wire order: the stack's record of what it provisioned.
async fn stack_resources(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String, String)> {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "ResourceStatus").unwrap_or_default(),
                xml_text(entry, "PhysicalResourceId").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every `(logical id, resource status)` the stack's event timeline holds, in wire order.
async fn stack_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String)> {
    let (status_code, body) =
        call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackEvents to answer, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "ResourceStatus").unwrap_or_default(),
            )
        })
        .collect()
}

/// The template body the stack is in force with, read through `GetTemplate` — the stack's own
/// record of what it was asked to deploy.
async fn stack_template(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status_code, body) = get_template(router, stack, None).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected GetTemplate to answer, got {status_code} {body}"
    );
    xml_text(&body, "TemplateBody").unwrap_or_default()
}

/// The `StackStatus` a stack reports right now.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status_code, body) = describe_stack(router, stack).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The physical id the stack published for one of its logical resources.
fn physical_id(
    case: &str,
    resources: &[(String, String, String)],
    logical_id: &str,
) -> String {
    resources
        .iter()
        .find(|(id, _, _)| id == logical_id)
        .map(|(_, _, physical)| physical.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to hold {logical_id:?}, got {resources:?}")
        })
}

/// One row: the package the template deploys, the package the hotswap uploads instead, and the
/// non-code change the fallback stack update carries.
struct Row {
    case: &'static str,
    /// The stack name, which the role and function names are derived from so one row's
    /// resources are never another's.
    stack: &'static str,
    /// The handler the TEMPLATE declares, deployed by the create.
    deployed_source: &'static str,
    /// What an invocation of the deployed handler answers, as JSON.
    deployed_answer: &'static str,
    /// The handler the hotswap uploads straight to AWS Lambda.
    hotswapped_source: &'static str,
    /// What an invocation of the HOTSWAPPED handler must answer, as JSON.
    hotswapped_answer: &'static str,
    /// The `Timeout` and `MemorySize` the create declares, and the ones the fallback update
    /// declares instead.
    created_timeout: i64,
    updated_timeout: i64,
    created_memory: i64,
    updated_memory: i64,
}

/// The rows. `Timeout` is varied across the range AWS publishes for it — 1 second is the
/// bottom of the API model's `Timeout` range (`smithy_query(query="shape",shape="Timeout")`
/// gives 1..5400) and 900 is the top of AWS's published Lambda quota "Function timeout: 900
/// seconds (15 minutes)", which is the narrower of the two.
const ROWS: &[Row] = &[
    Row {
        case: "a hotswap that re-values the object the handler returns, then a Timeout change",
        stack: "hotswap-object",
        deployed_source: "def handler(event, context):\n    return {'stage': 'deployed'}\n",
        deployed_answer: r#"{"stage":"deployed"}"#,
        hotswapped_source: "def handler(event, context):\n    \
                            return {'stage': 'hotswapped', 'build': 2}\n",
        hotswapped_answer: r#"{"stage":"hotswapped","build":2}"#,
        created_timeout: 10,
        updated_timeout: 73,
        created_memory: 192,
        updated_memory: 192,
    },
    Row {
        case: "a hotswap whose handler returns a LIST, then a Timeout change to the bottom of \
               the modelled range",
        stack: "hotswap-list",
        deployed_source: "def handler(event, context):\n    return {'items': []}\n",
        deployed_answer: r#"{"items":[]}"#,
        hotswapped_source: "def handler(event, context):\n    return [1, 2, 3]\n",
        hotswapped_answer: "[1,2,3]",
        created_timeout: 10,
        updated_timeout: 1,
        created_memory: 192,
        updated_memory: 192,
    },
    Row {
        case: "a hotswap whose handler computes its answer from the event, then a Timeout \
               change to the top of the published quota",
        stack: "hotswap-computed",
        deployed_source: "def handler(event, context):\n    return {'n': 0}\n",
        deployed_answer: r#"{"n":0}"#,
        hotswapped_source: "def handler(event, context):\n    return {'n': event['n'] * 3}\n",
        hotswapped_answer: r#"{"n":21}"#,
        created_timeout: 10,
        updated_timeout: 900,
        created_memory: 192,
        updated_memory: 192,
    },
    Row {
        case: "a hotswap whose handler returns a bare string, then a Timeout and MemorySize \
               change together",
        stack: "hotswap-scalar",
        deployed_source: "def handler(event, context):\n    return 'deployed'\n",
        deployed_answer: r#""deployed""#,
        hotswapped_source: "def handler(event, context):\n    return 'hotswapped'\n",
        hotswapped_answer: r#""hotswapped""#,
        created_timeout: 10,
        updated_timeout: 42,
        created_memory: 192,
        updated_memory: 320,
    },
];

/// The template a row deploys: the execution role the function runs as, and the function
/// itself with the source, the timeout and the memory this stage declares.
fn template(row: &Row, source: &str, timeout: i64, memory: i64) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            ROLE_LOGICAL_ID: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role_name(row),
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
            FUNCTION_LOGICAL_ID: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function_name(row),
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Role": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] },
                    "Timeout": timeout,
                    "MemorySize": memory,
                    "Code": { "ZipFile": source }
                }
            }
        }
    })
    .to_string()
}

/// The function name a row's template declares, and so the physical id the stack publishes.
fn function_name(row: &Row) -> String {
    format!("{}-fn", row.stack)
}

/// The execution role name a row's template declares.
fn role_name(row: &Row) -> String {
    format!("{}-exec", row.stack)
}

/// The numeric member `member` of a reported configuration.
fn reported_number(configuration: &Value, member: &str) -> Option<i64> {
    configuration.get(member).and_then(Value::as_i64)
}

/// RULE: a package uploaded straight to AWS Lambda becomes the function's running code and
/// leaves the stack that provisioned the function exactly as it was; the change that is not a
/// code change then falls back to a stack update, which applies it and completes.
#[tokio::test]
async fn a_hotswapped_package_runs_and_a_non_code_change_falls_back_to_a_stack_update() {
    for row in ROWS {
        let case = row.case;
        let (router, _engine) = router_with_engine();
        let function = function_name(row);

        // ---- The deployment the hotswap is performed against. ----
        let deployed = template(
            row,
            row.deployed_source,
            row.created_timeout,
            row.created_memory,
        );
        let (status_code, body) =
            create_stack(&router, row.stack, &deployed, &capabilities()).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let reported = stack_status(&router, case, row.stack).await;
        assert_eq!(
            reported, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to deploy, got {reported:?}"
        );

        let resources_before = stack_resources(&router, case, row.stack).await;
        let published = physical_id(case, &resources_before, FUNCTION_LOGICAL_ID);
        assert_eq!(
            published, function,
            "case {case:?}: expected the stack to publish the declared function name \
             ({function:?}), got {published:?}"
        );
        let configuration = reported_configuration(&router, case, &function).await;
        assert_eq!(
            reported_number(&configuration, "Timeout"),
            Some(row.created_timeout),
            "case {case:?}: expected the created Timeout {}, got {configuration}",
            row.created_timeout
        );

        // The code the TEMPLATE deployed is what runs before the hotswap.
        let answered = invoked(&router, case, &function).await;
        let expected: Value = serde_json::from_str(row.deployed_answer)
            .expect("the row's deployed answer is JSON");
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the deployed handler's own result {expected}, got {answered}"
        );

        // What the stack looks like before anything is uploaded: its template, its resource
        // records and its event timeline, each read through its own operation.
        let template_before = stack_template(&router, case, row.stack).await;
        let events_before = stack_events(&router, case, row.stack).await;
        let digest_before = reported_digest(&router, case, &function).await;
        assert!(
            !digest_before.is_empty(),
            "case {case:?}: expected AWS Lambda to report a digest over the deployed package, \
             got {digest_before:?}"
        );

        // ---- The hotswap: UpdateFunctionCode, straight to AWS Lambda. ----
        let uploaded = lambda(
            &router,
            "PUT",
            &format!("/2015-03-31/functions/{function}/code"),
            serde_json::to_vec(&json!({ "ZipFile": encoded_package(row.hotswapped_source) }))
                .expect("the upload body serializes")
                .as_slice(),
            &[],
        )
        .await;
        assert_eq!(
            uploaded.status, 200,
            "case {case:?}: expected UpdateFunctionCode to be accepted, got {} with body {}",
            uploaded.status, uploaded.body
        );
        let reported_after_upload = uploaded
            .body
            .get("CodeSha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !reported_after_upload.is_empty() && reported_after_upload != digest_before,
            "case {case:?}: expected the upload to report a digest over the NEW package, \
             differing from {digest_before:?}, got {reported_after_upload:?}"
        );
        assert_eq!(
            reported_number(&uploaded.body, "Timeout"),
            Some(row.created_timeout),
            "case {case:?}: expected a code-only upload to leave the Timeout at {}, got {}",
            row.created_timeout,
            uploaded.body
        );

        // The package the upload stored is the one AWS Lambda now holds...
        let digest_after = reported_digest(&router, case, &function).await;
        assert_eq!(
            digest_after, reported_after_upload,
            "case {case:?}: expected the function to hold the uploaded package \
             ({reported_after_upload:?}), got {digest_after:?}"
        );
        // ...and the NEXT invocation runs it.
        let answered = invoked(&router, case, &function).await;
        let expected: Value = serde_json::from_str(row.hotswapped_answer)
            .expect("the row's hotswapped answer is JSON");
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the hotswapped handler's own result {expected}, got \
             {answered}"
        );

        // ---- The stack is untouched by that upload. ----
        let reported = stack_status(&router, case, row.stack).await;
        assert_eq!(
            reported, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to keep the status its create left, got \
             {reported:?}"
        );
        let template_after = stack_template(&router, case, row.stack).await;
        assert_eq!(
            template_after, template_before,
            "case {case:?}: expected the stack's template to be untouched by a data-plane \
             upload, got {template_after}"
        );
        let resources_after = stack_resources(&router, case, row.stack).await;
        assert_eq!(
            resources_after, resources_before,
            "case {case:?}: expected the stack's resource records to be untouched by a \
             data-plane upload, got {resources_after:?}"
        );
        let events_after = stack_events(&router, case, row.stack).await;
        assert_eq!(
            events_after, events_before,
            "case {case:?}: expected the stack's event timeline to be untouched by a \
             data-plane upload, got {events_after:?}"
        );

        // ---- The fallback: the change no upload can carry goes through UpdateStack. ----
        let revised = template(
            row,
            row.deployed_source,
            row.updated_timeout,
            row.updated_memory,
        );
        let (status_code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &revised),
                member("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let reported = stack_status(&router, case, row.stack).await;
        assert_eq!(
            reported,
            "UPDATE_COMPLETE",
            "case {case:?}: expected the fallback update to complete, got {reported:?} with \
             events {:?}",
            stack_events(&router, case, row.stack).await
        );

        // The change the update carried is one AWS Lambda really holds, and the function kept
        // its identity: an in-place update, not a replacement.
        let configuration = reported_configuration(&router, case, &function).await;
        assert_eq!(
            reported_number(&configuration, "Timeout"),
            Some(row.updated_timeout),
            "case {case:?}: expected the updated Timeout {}, got {configuration}",
            row.updated_timeout
        );
        assert_eq!(
            reported_number(&configuration, "MemorySize"),
            Some(row.updated_memory),
            "case {case:?}: expected the updated MemorySize {}, got {configuration}",
            row.updated_memory
        );
        let resources_updated = stack_resources(&router, case, row.stack).await;
        let still_published = physical_id(case, &resources_updated, FUNCTION_LOGICAL_ID);
        assert_eq!(
            still_published, published,
            "case {case:?}: expected the function to keep the physical id {published:?} across \
             the update, got {still_published:?}"
        );

        // ...and the id the completed update still publishes is one an invocation resolves:
        // the request-validating `DryRun` is accepted rather than refused as a function
        // nothing holds.
        let probe = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{still_published}/invocations"),
            event().to_string().as_bytes(),
            &[("x-amz-invocation-type", "DryRun")],
        )
        .await;
        assert_eq!(
            probe.status, 204,
            "case {case:?}: expected a DryRun invocation of {still_published:?} to be accepted, \
             got {} with body {}",
            probe.status, probe.body
        );
    }
}
