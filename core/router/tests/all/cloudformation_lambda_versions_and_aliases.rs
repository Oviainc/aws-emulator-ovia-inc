// SPDX-License-Identifier: CC0-1.0

//! The `AWS::Lambda::Version` and `AWS::Lambda::Alias` resources a CloudFormation stack
//! declares are created in AWS Lambda itself, and what Lambda refuses about one fails the
//! resource and the stack.
//!
//! The rule under test: **a published version and an alias a stack declares are made by AWS
//! Lambda's own API, so the identifiers the stack publishes for them are the ones that
//! service minted — `Ref` resolves to the ARN it answered, `Fn::GetAtt … Version` to the
//! number it allocated and `Fn::GetAtt … AliasArn` to the alias's ARN — and an alias whose
//! `FunctionVersion` names a version the function does not have is REFUSED by Lambda, which
//! fails that resource with Lambda's own message quoted in its status reason and leaves the
//! stack in `UPDATE_FAILED` where the update asked for its state to be preserved and in
//! `UPDATE_ROLLBACK_COMPLETE` where it did not.** Two crates have to agree for that to hold
//! (CloudFormation makes the call, Lambda serves it, IAM answers for the execution role), so
//! the suite lives here rather than in any one crate.
//!
//! Three more consequences of that rule are pinned here, because they are only reachable
//! once an alias can fail at all: a stack whose update failed and was left standing takes a
//! further update only from a caller that asks for that state to go on being preserved,
//! resubmitting the configuration such an update could not apply is a RETRY rather than a
//! no-op, and the `AWS::Serverless::Function` transform's `AutoPublishAlias` expands into
//! exactly these two resources — the version under a hashed logical id, retained, and the
//! alias pointing at the version that hash names.
//!
//! The rows differ along what the rule generalises over: how the declaration names the
//! function (`Ref` to it, or its ARN through `Fn::GetAtt`), whether the version and the alias
//! carry a description, which intrinsic each output reads the identifier through, whether the
//! alias is broken on the CREATE or on an UPDATE, and whether the update that breaks it asks
//! for the failed state to be preserved.
//!
//! Every expected value is computed here from what the row declares and from the account and
//! Region every ARN this emulator mints carries — never read back out of the answer being
//! checked.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use crate::query_wire::{elements, form_encode, members_of, opt_element, outputs_of};
// Polling a stack until it first reports a status is what every suite deploying a resource
// its owning service reports as still coming up needs, and this crate already has it.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;

/// The Region every request here is signed for, and so the Region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const UPDATE_FAILED: &str = "UPDATE_FAILED";
const UPDATE_ROLLBACK_COMPLETE: &str = "UPDATE_ROLLBACK_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The alias name every row declares, which is the qualifier its ARN carries.
const ALIAS: &str = "live";

/// A version number no function of these stacks ever published, which is what an alias
/// pointing at it is refused for.
const MISSING_VERSION: &str = "593";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
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
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
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

/// Call one Lambda operation over its restJson1 wire.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// The ARN a function of `name` carries in this suite's Region: the same total function of
/// the Region, the account and the name both services compute it from.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The execution role every template declares, with the trust policy Lambda has to be able
/// to assume.
fn execution_role() -> Value {
    json!({
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
    })
}

/// The template a row deploys: a function, one published version of it and one alias of
/// that version, with the outputs that publish each identifier.
///
/// `function_reference` is how the version's and the alias's declarations NAME the function
/// — `Ref` to it (its name) or its ARN through `Fn::GetAtt` — and `alias_version` is what
/// the alias's `FunctionVersion` states: the version the stack publishes, or a number the
/// function does not have.
fn template(
    function_name: &str,
    function_reference: &Value,
    alias_version: &Value,
    described: bool,
) -> String {
    let mut version = json!({
        "Type": "AWS::Lambda::Version",
        "Properties": { "FunctionName": function_reference }
    });
    let mut alias = json!({
        "Type": "AWS::Lambda::Alias",
        "Properties": {
            "FunctionName": function_reference,
            "Name": ALIAS,
            "FunctionVersion": alias_version
        }
    });
    if described {
        version["Properties"]["Description"] = json!("the first cut");
        alias["Properties"]["Description"] = json!("what clients invoke");
    }
    json!({
        "Resources": {
            "ExecRole": execution_role(),
            "Fn": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function_name,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["ExecRole", "Arn"] },
                    "Code": { "ZipFile": "def handler(event, context):\n    return 1\n" }
                }
            },
            "FnVersion": version,
            "FnAlias": alias
        },
        "Outputs": {
            "VersionNumber": { "Value": { "Fn::GetAtt": ["FnVersion", "Version"] } },
            "VersionRef": { "Value": { "Ref": "FnVersion" } },
            "VersionArnAttribute": { "Value": { "Fn::GetAtt": ["FnVersion", "FunctionArn"] } },
            "AliasRef": { "Value": { "Ref": "FnAlias" } },
            "AliasArnAttribute": { "Value": { "Fn::GetAtt": ["FnAlias", "AliasArn"] } }
        }
    })
    .to_string()
}

/// The `ResourceStatusReason` the stack's events carry for one resource's own FAILURE, or
/// `None` where that resource failed in none of them.
///
/// Read from the `<member>` that carries BOTH the logical id and the reason, so a reason is
/// never read off a different resource's event.
fn failure_reason(events: &str, logical_id: &str) -> Option<String> {
    members_of(events).into_iter().find_map(|member| {
        let named = elements(member, "LogicalResourceId")
            .into_iter()
            .next()
            .is_some_and(|named| named == logical_id);
        let failed = elements(member, "ResourceStatus")
            .into_iter()
            .next()
            .is_some_and(|status| status.ends_with("_FAILED"));
        (named && failed)
            .then(|| elements(member, "ResourceStatusReason").into_iter().next())
            .flatten()
    })
}

/// The stack's events, as the answer a client reads them from.
async fn stack_events(router: &MultiServiceRouter, stack: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStackEvents {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// One row of the first table: how the declaration names the function, and whether the
/// version and the alias carry a description.
struct Row {
    case: &'static str,
    function: &'static str,
    /// `true` for a declaration that names the function through `Fn::GetAtt … Arn`, `false`
    /// for one that names it with `Ref` (which "returns the function name").
    by_arn: bool,
    described: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "the function named by Ref, both resources described",
        function: "orders-handler",
        by_arn: false,
        described: true,
    },
    Row {
        case: "the function named by its ARN, neither resource described",
        function: "billing-handler",
        by_arn: true,
        described: false,
    },
];

/// Every row: the stack completes, AWS Lambda holds the published version and the alias, the
/// identifiers the stack publishes for them are the ones Lambda minted, and each of those
/// identifiers resolves back through Lambda's own API.
#[tokio::test]
async fn a_stack_publishes_the_lambda_version_and_alias_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("lambda-alias-stack-{index}");
        let reference = if row.by_arn {
            json!({ "Fn::GetAtt": ["Fn", "Arn"] })
        } else {
            json!({ "Ref": "Fn" })
        };
        let body = template(
            row.function,
            &reference,
            &json!({ "Fn::GetAtt": ["FnVersion", "Version"] }),
            row.described,
        );
        let (status, answered) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &body),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answered}"
        );

        let (status, _) = wait_for_stack_status(&router, &stack_name, CREATE_COMPLETE).await;
        let events = stack_events(&router, &stack_name).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {status} with \
             the failure reasons {:?}",
            elements(&events, "ResourceStatusReason")
        );

        // AWS Lambda holds the version the stack published: the number it allocated is the
        // first one of the function.
        let (status, listed) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{}/versions", row.function),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListVersionsByFunction, got {status} with \
             {listed}"
        );
        let published: Vec<String> = listed
            .get("Versions")
            .and_then(Value::as_array)
            .map(|versions| {
                versions
                    .iter()
                    .filter_map(|version| version.get("Version").and_then(Value::as_str))
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            published,
            vec!["$LATEST".to_string(), "1".to_string()],
            "case {case:?}: expected the unpublished version and the one the stack published, \
             got {published:?}"
        );

        // ...and the alias, invoking exactly that version.
        let (status, alias) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{}/aliases/{ALIAS}", row.function),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetAlias, got {status} with {alias}"
        );
        assert_eq!(
            alias.get("FunctionVersion").and_then(Value::as_str),
            Some("1"),
            "case {case:?}: expected the alias to invoke the version the stack published, got \
             {alias}"
        );
        assert_eq!(
            alias.get("AliasArn").and_then(Value::as_str),
            Some(format!("{}:{ALIAS}", function_arn(row.function)).as_str()),
            "case {case:?}: expected the alias's own ARN, got {alias}"
        );

        // The identifiers the stack PUBLISHES are those: the version's number and ARN, and
        // the alias's ARN, whichever intrinsic the output reads them through.
        let (status, described) =
            cloudformation(&router, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} {described}"
        );
        let outputs = outputs_of(&described);
        let version_arn = format!("{}:1", function_arn(row.function));
        let alias_arn = format!("{}:{ALIAS}", function_arn(row.function));
        let expected: BTreeMap<String, String> = [
            ("VersionNumber", "1".to_string()),
            ("VersionRef", version_arn.clone()),
            ("VersionArnAttribute", version_arn.clone()),
            ("AliasRef", alias_arn.clone()),
            ("AliasArnAttribute", alias_arn.clone()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
        assert_eq!(
            outputs, expected,
            "case {case:?}: expected the outputs to publish the identifiers Lambda minted, \
             got {outputs:?}"
        );

        // ...and each of them resolves back through Lambda's own API, which is what a client
        // reading an identifier out of a stack output does with it.
        let (status, read) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{version_arn}/configuration"),
        )
        .await;
        assert_eq!(
            (status, read.get("Version").and_then(Value::as_str)),
            (200, Some("1")),
            "case {case:?}: expected the published version's ARN to resolve to that version, \
             got {status} with {read}"
        );
        let (status, read) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{alias_arn}/configuration"),
        )
        .await;
        assert_eq!(
            (status, read.get("Version").and_then(Value::as_str)),
            (200, Some("1")),
            "case {case:?}: expected the alias's ARN to resolve to the version it invokes, \
             got {status} with {read}"
        );
    }
}

/// One row of the second table: when the alias is pointed at a version the function does not
/// have, and what the stack is then in.
struct FailureRow {
    case: &'static str,
    function: &'static str,
    /// `true` for a row that declares the missing version on the CREATE, `false` for one
    /// that deploys a sound stack and then UPDATES the alias onto it.
    on_create: bool,
    /// Whether the failing operation asks for the state it provisioned to be preserved.
    disable_rollback: bool,
    settles_to: &'static str,
}

const FAILURE_ROWS: &[FailureRow] = &[
    FailureRow {
        case: "a create whose alias names a version the function never published",
        function: "create-break-handler",
        on_create: true,
        disable_rollback: false,
        settles_to: ROLLBACK_COMPLETE,
    },
    FailureRow {
        case: "an update onto a missing version, asked to preserve what it provisioned",
        function: "update-break-handler",
        on_create: false,
        disable_rollback: true,
        settles_to: UPDATE_FAILED,
    },
    FailureRow {
        case: "an update onto a missing version, with nothing asked about preserving it",
        function: "rollback-break-handler",
        on_create: false,
        disable_rollback: false,
        settles_to: UPDATE_ROLLBACK_COMPLETE,
    },
];

/// Every row: AWS Lambda refuses the alias, so the resource FAILS with that service's own
/// message quoted in its status reason, the stack settles in the status the operation's own
/// rollback setting leaves, and the alias Lambda holds still invokes the version it did.
#[tokio::test]
async fn an_alias_pointed_at_a_version_that_does_not_exist_fails_the_stack() {
    let router = build_router();

    for (index, row) in FAILURE_ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("lambda-alias-failure-{index}");
        let reference = json!({ "Ref": "Fn" });
        let sound = template(
            row.function,
            &reference,
            &json!({ "Fn::GetAtt": ["FnVersion", "Version"] }),
            false,
        );
        let broken = template(
            row.function,
            &reference,
            &json!(MISSING_VERSION),
            false,
        );

        let created = if row.on_create { &broken } else { &sound };
        let (status, answered) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", created),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {answered}"
        );

        if !row.on_create {
            let (status, _) = wait_for_stack_status(&router, &stack_name, CREATE_COMPLETE).await;
            assert_eq!(
                status, CREATE_COMPLETE,
                "case {case:?}: the sound stack must complete first, got {status} with the \
                 failure reasons {:?}",
                elements(&stack_events(&router, &stack_name).await, "ResourceStatusReason")
            );
            let mut members = vec![
                ("StackName", stack_name.as_str()),
                ("TemplateBody", broken.as_str()),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ];
            if row.disable_rollback {
                members.push(("DisableRollback", "true"));
            }
            let (status, answered) = cloudformation(&router, "UpdateStack", &members).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with \
                 {answered}"
            );
        }

        let (status, _) = wait_for_stack_status(&router, &stack_name, row.settles_to).await;
        assert_eq!(
            status, row.settles_to,
            "case {case:?}: expected the stack to settle in {}, got {status}",
            row.settles_to
        );

        // The resource's reason is Lambda's own refusal, quoted the way CloudFormation
        // quotes a resource handler's message, and it names the function qualified by the
        // version that does not exist. What follows the message inside the quotes is that
        // service's own diagnostic of the call, which carries per-call identifiers.
        let events = stack_events(&router, &stack_name).await;
        let reason = failure_reason(&events, "FnAlias").unwrap_or_default();
        let quoted = format!(
            "Resource handler returned message: \"Function not found: {}:{MISSING_VERSION}",
            function_arn(row.function)
        );
        assert!(
            reason.starts_with(&quoted),
            "case {case:?}: expected the alias's failure to open with {quoted:?}, got \
             {reason:?}"
        );

        // ...and nothing was changed in Lambda: the alias of a stack that had one still
        // invokes the version it did, and a create that failed left no alias at all.
        let (status, alias) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{}/aliases/{ALIAS}", row.function),
        )
        .await;
        let held = (status, alias.get("FunctionVersion").and_then(Value::as_str));
        let expected = if row.on_create {
            (404, None)
        } else {
            (200, Some("1"))
        };
        assert_eq!(
            held, expected,
            "case {case:?}: expected Lambda to report {expected:?} for the alias, got \
             {status} with {alias}"
        );
    }
}

/// A stack whose update FAILED and was left standing takes a further update only from a
/// caller that asks for that state to go on being preserved — and resubmitting the
/// configuration the failed update could not apply is a RETRY of it rather than a no-op.
#[tokio::test]
async fn an_update_from_update_failed_asks_for_the_failed_state_to_be_preserved() {
    let router = build_router();
    let stack_name = "lambda-alias-fix-forward";
    let function = "fix-forward-handler";
    let reference = json!({ "Ref": "Fn" });
    let sound = template(
        function,
        &reference,
        &json!({ "Fn::GetAtt": ["FnVersion", "Version"] }),
        false,
    );
    let broken = template(function, &reference, &json!(MISSING_VERSION), false);

    let (status, answered) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &sound),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the stack to break\": expected HTTP 200 from CreateStack, got {status} \
         {answered}"
    );
    let (status, _) = wait_for_stack_status(&router, stack_name, CREATE_COMPLETE).await;
    assert_eq!(
        status, CREATE_COMPLETE,
        "case \"the stack to break\": expected {CREATE_COMPLETE}, got {status}"
    );

    let (status, answered) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &broken),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the update that fails\": expected HTTP 200, got {status} {answered}"
    );
    let (status, _) = wait_for_stack_status(&router, stack_name, UPDATE_FAILED).await;
    assert_eq!(
        status, UPDATE_FAILED,
        "case \"the update that fails\": expected {UPDATE_FAILED}, got {status}"
    );
    let stack_id = {
        let (_, described) =
            cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
        opt_element(&described, "StackId").unwrap_or_default()
    };
    assert!(
        !stack_id.is_empty(),
        "case \"the update that fails\": expected the stack to report its id"
    );

    // An update that asks nothing about preserving the failed state is refused: that state
    // is not a terminal one, and the refusal names the two ways on from it.
    let (status, refused) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &sound),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an update that asks nothing about the failed state\": expected HTTP 400, got \
         {status} {refused}"
    );
    assert_eq!(
        opt_element(&refused, "Code").as_deref(),
        Some("ValidationError"),
        "case \"an update that asks nothing about the failed state\": expected a \
         ValidationError, got {refused}"
    );
    let expected = service_com_amazonaws_cloudformation_cloudformation::state::stack_is_in_a_non_terminal_state(
        UPDATE_FAILED,
    );
    assert_eq!(
        opt_element(&refused, "Message").as_deref(),
        Some(expected.as_str()),
        "case \"an update that asks nothing about the failed state\": expected the refusal to \
         read {expected:?}, got {refused}"
    );

    // Resubmitting the very configuration the failed update could not apply is not a no-op:
    // the stack's resources do not carry it, so the update is the RETRY of that work — and
    // it fails the same way.
    let (status, answered) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &broken),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"resubmitting the failing template\": expected HTTP 200, got {status} \
         {answered}"
    );
    let (status, _) = wait_for_stack_status(&router, stack_name, UPDATE_FAILED).await;
    assert_eq!(
        status, UPDATE_FAILED,
        "case \"resubmitting the failing template\": expected {UPDATE_FAILED}, got {status}"
    );

    // ...and the fix-forward, which asks for the same preservation, completes.
    let (status, answered) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &sound),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the fix-forward\": expected HTTP 200, got {status} {answered}"
    );
    let (status, _) = wait_for_stack_status(&router, stack_name, UPDATE_COMPLETE).await;
    assert_eq!(
        status, UPDATE_COMPLETE,
        "case \"the fix-forward\": expected {UPDATE_COMPLETE}, got {status}"
    );
}

/// One row of the SAM table: how the declaration states the value that governs the
/// publication, and the logical id the expansion is then to give the version.
struct SamRow {
    case: &'static str,
    stack: &'static str,
    function: &'static str,
    /// The `AutoPublishCodeSha256` the row states, as the template writes it, and the
    /// parameter value in force where it states one.
    sha: &'static str,
    by_parameter: bool,
}

/// Two code digests a caller states to govern the publication. Any value does: the property
/// is "a string that, when changed, triggers a new version to be published", so what is
/// asserted about it is that its first ten characters are the hash the generated logical id
/// carries.
const SHA_A: &str = "9f2c3b7a1d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8";
const SHA_B: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f809";

const SAM_ROWS: &[SamRow] = &[
    SamRow {
        case: "the code digest written out in the declaration",
        stack: "sam-autopublish-literal",
        function: "sam-literal-handler",
        sha: SHA_A,
        by_parameter: false,
    },
    SamRow {
        case: "the code digest stated as a parameter of the template",
        stack: "sam-autopublish-parameter",
        function: "sam-parameter-handler",
        sha: SHA_B,
        by_parameter: true,
    },
];

/// Every row: the transform expands `AutoPublishAlias` into one `AWS::Lambda::Version` under
/// a logical id carrying the first ten characters of the digest the declaration states, with
/// that version RETAINED, and one `AWS::Lambda::Alias` pointing at it — and the stack really
/// publishes the version and creates the alias in AWS Lambda.
#[tokio::test]
async fn the_serverless_transform_publishes_the_version_its_alias_asks_for() {
    let router = build_router();

    for row in SAM_ROWS {
        let case = row.case;
        let sha = if row.by_parameter {
            json!({ "Ref": "CodeSha" })
        } else {
            json!(row.sha)
        };
        let mut body = json!({
            "Transform": "AWS::Serverless-2016-10-31",
            "Resources": {
                "Fn": {
                    "Type": "AWS::Serverless::Function",
                    "Properties": {
                        "FunctionName": row.function,
                        "Runtime": "python3.12",
                        "Handler": "index.handler",
                        "InlineCode": "def handler(event, context):\n    return 1\n",
                        "AutoPublishAlias": ALIAS,
                        "AutoPublishCodeSha256": sha
                    }
                }
            }
        });
        if row.by_parameter {
            body["Parameters"] = json!({ "CodeSha": { "Type": "String" } });
        }
        let body = body.to_string();

        let mut members = vec![
            ("StackName", row.stack),
            ("TemplateBody", body.as_str()),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
        ];
        if row.by_parameter {
            members.push(("Parameters.member.1.ParameterKey", "CodeSha"));
            members.push(("Parameters.member.1.ParameterValue", row.sha));
        }
        let (status, answered) = cloudformation(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {answered}"
        );
        let (status, _) = wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {status} with \
             the failure reasons {:?}",
            elements(&stack_events(&router, row.stack).await, "ResourceStatusReason")
        );

        // The PROCESSED template — what CloudFormation deploys — carries the two resources
        // the transform generated, the version under the hashed logical id and retained.
        let (status, served) = cloudformation(
            &router,
            "GetTemplate",
            &[("StackName", row.stack), ("TemplateStage", "Processed")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetTemplate, got {status} with {served}"
        );
        // The served body is a DOCUMENT inside an XML element, so it is read through the
        // reader that undoes the serializer's escaping.
        let processed: Value = serde_json::from_str(
            &elements(&served, "TemplateBody").into_iter().next().unwrap_or_default(),
        )
        .unwrap_or(Value::Null);
        let version_id = format!("FnVersion{}", &row.sha[..10]);
        let generated = processed
            .get("Resources")
            .and_then(Value::as_object)
            .map(|resources| {
                resources
                    .iter()
                    .filter(|(_, declared)| {
                        declared.get("Type").and_then(Value::as_str)
                            == Some("AWS::Lambda::Version")
                    })
                    .map(|(logical_id, _)| logical_id.clone())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        assert_eq!(
            generated,
            vec![version_id.clone()],
            "case {case:?}: expected exactly the hashed version {version_id:?}, got \
             {generated:?} in {processed}"
        );
        assert_eq!(
            processed["Resources"][&version_id]
                .get("DeletionPolicy")
                .and_then(Value::as_str),
            Some("Retain"),
            "case {case:?}: expected the generated version to be retained, got {processed}"
        );
        assert_eq!(
            processed["Resources"][format!("FnAlias{ALIAS}")]
                .pointer("/Properties/FunctionVersion")
                .cloned()
                .unwrap_or(Value::Null),
            json!({ "Fn::GetAtt": [version_id.clone(), "Version"] }),
            "case {case:?}: expected the generated alias to invoke the generated version, got \
             {processed}"
        );

        // AWS Lambda holds what that expansion asked for: the published version, and the
        // alias invoking it.
        let (status, alias) = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{}/aliases/{ALIAS}", row.function),
        )
        .await;
        assert_eq!(
            (status, alias.get("FunctionVersion").and_then(Value::as_str)),
            (200, Some("1")),
            "case {case:?}: expected the alias to invoke the published version, got {status} \
             with {alias}"
        );
    }
}
