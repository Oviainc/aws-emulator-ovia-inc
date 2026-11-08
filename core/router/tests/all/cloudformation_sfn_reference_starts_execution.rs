// SPDX-License-Identifier: CC0-1.0

//! The reference a CloudFormation stack publishes for an `AWS::StepFunctions::StateMachine`
//! can be handed STRAIGHT to `states StartExecution`.
//!
//! The rule under test: **the physical resource id of a resource whose published return
//! values document `Ref` as an ARN is that ARN — the one the owning service minted — so every
//! way a template resolves the reference (`Ref`, `Fn::GetAtt … Arn`, `Fn::Sub "${LogicalId}"`,
//! and the `Outputs` carrying any of them) yields a value that service accepts wherever it
//! takes that resource's ARN.** For a state machine that is `StartExecution`'s
//! `stateMachineArn`: the stack output starts an execution and the call answers with the
//! `executionArn` and `startDate` of a run Step Functions holds — while the machine's NAME,
//! which is a separate `Fn::GetAtt` attribute and never the reference, is refused `InvalidArn`
//! by that same operation.
//!
//! Two crates have to agree for that to hold (CloudFormation computes the identity and makes
//! the create, Step Functions serves the start), so the suite lives here rather than in
//! either. `cloudformation_provisions_sfn_state_machines.rs` is the sibling suite over the
//! machine the stack creates and the definition it carries; this one is over what a CLIENT can
//! then do with the handle the stack published.
//!
//! The rows differ along what the rule generalises over: which intrinsic the stack resolves
//! the reference through, whether the template NAMES the machine or leaves CloudFormation to
//! generate its name, which `StateMachineType` it declares, which Region it deploys in, and
//! whether the start names the execution or leaves Step Functions to mint that name. Every row
//! publishes the machine's `Name` attribute as well, so the name half of the rule is known
//! independently of the reference under test rather than read back out of it.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{form_encode, opt_element, outputs_of};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the outputs of the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The execution role every row's machine declares. No IAM is mounted here, so nothing is
/// known about it and the create is served — the documented reading of a role provisioned
/// outside the client that creates the resource.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/StatesExecutionRole";

/// The modelled upper bound on an execution `Name` (`len 1..80`, from
/// `smithy_query(query="operation",operation="StartExecution")`), which the name Step
/// Functions mints for a start that names none must also respect.
const MAX_EXECUTION_NAME: usize = 80;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Step Functions operation (`sigv4Name: states`, service shape `AWSStepFunctions`,
/// `awsJson1_0`) over its own wire, signed for `region`.
async fn sfn(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header("authorization", authorization("states", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a state machine of `name` carries in `region`: the same total function of the
/// Region, the account and the name both services compute it from.
fn state_machine_arn(region: &str, name: &str) -> String {
    format!("arn:aws:states:{region}:{ACCOUNT}:stateMachine:{name}")
}

/// The ARN one execution of that machine carries, which is what a start answers with.
fn execution_arn(region: &str, machine: &str, execution: &str) -> String {
    format!("arn:aws:states:{region}:{ACCOUNT}:execution:{machine}:{execution}")
}

/// A one-state definition that ends immediately, as each row's template states it.
fn pass_definition(result: &str) -> Value {
    json!({
        "StartAt": "Done",
        "States": { "Done": { "Type": "Pass", "Result": result, "End": true } },
    })
}

/// One row of the table: a template that publishes a reference to the machine it declares, and
/// what starting an execution with that reference must then do.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the machine is created in and the one
    /// both its ARN and its executions' ARNs carry.
    region: &'static str,
    /// The template body.
    template: fn() -> String,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// machine carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The output key whose value a client hands to `StartExecution`, and the intrinsic the
    /// template resolved it through.
    reference_output: &'static str,
    /// The output key carrying the machine's NAME, which every row publishes with
    /// `Fn::GetAtt … Name`: the name half of the rule, known INDEPENDENTLY of the reference
    /// rather than read back out of it, and the value `StartExecution` must refuse because a
    /// name is not an ARN.
    name_output: &'static str,
    /// The name the start asks for, or `None` to leave Step Functions to mint one.
    execution_name: Option<&'static str>,
    /// Whether the machine is an EXPRESS workflow: `ListExecutions` answers
    /// `StateMachineTypeNotSupported` for one and `DescribeExecution` answers
    /// `ExecutionDoesNotExist` ("executions of an EXPRESS state machine aren't supported by
    /// DescribeExecution unless a Map Run dispatched them"), so the row asserts those refusals
    /// where a STANDARD row asserts the reads.
    express: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a named STANDARD machine published with Ref",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "CallbackStateMachine": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "StateMachineName": "callback-workflow",
                            "RoleArn": ROLE_ARN,
                            "DefinitionString": pass_definition("called back").to_string(),
                        },
                    },
                },
                "Outputs": {
                    "StateMachine": { "Value": { "Ref": "CallbackStateMachine" } },
                    "StateMachineName": {
                        "Value": { "Fn::GetAtt": ["CallbackStateMachine", "Name"] },
                    },
                },
            })
            .to_string()
        },
        declared_name: Some("callback-workflow"),
        reference_output: "StateMachine",
        name_output: "StateMachineName",
        execution_name: Some("started-from-the-stack-output"),
        express: false,
    },
    Row {
        // The pairing the recorded stack has: nothing names the machine, and the output a
        // client reads is a `Ref` to it.
        case: "an unnamed STANDARD machine published with Ref",
        region: "us-west-2",
        template: || {
            json!({
                "Resources": {
                    "CallbackStateMachine": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "RoleArn": ROLE_ARN,
                            "Definition": pass_definition("generated"),
                        },
                    },
                },
                "Outputs": {
                    "StateMachine": { "Value": { "Ref": "CallbackStateMachine" } },
                    "StateMachineName": {
                        "Value": { "Fn::GetAtt": ["CallbackStateMachine", "Name"] },
                    },
                },
            })
            .to_string()
        },
        declared_name: None,
        reference_output: "StateMachine",
        name_output: "StateMachineName",
        execution_name: None,
        express: false,
    },
    Row {
        case: "an unnamed STANDARD machine published with Fn::GetAtt Arn",
        region: "eu-central-1",
        template: || {
            json!({
                "Resources": {
                    "Workflow": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "RoleArn": ROLE_ARN,
                            "StateMachineType": "STANDARD",
                            "DefinitionString": pass_definition("attributed").to_string(),
                        },
                    },
                },
                "Outputs": {
                    "StateMachineArn": { "Value": { "Fn::GetAtt": ["Workflow", "Arn"] } },
                    "StateMachineName": { "Value": { "Fn::GetAtt": ["Workflow", "Name"] } },
                },
            })
            .to_string()
        },
        declared_name: None,
        reference_output: "StateMachineArn",
        name_output: "StateMachineName",
        execution_name: Some("started-from-the-attribute"),
        express: false,
    },
    Row {
        case: "a named EXPRESS machine published with Fn::Sub over its logical id",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "Workflow": {
                        "Type": "AWS::StepFunctions::StateMachine",
                        "Properties": {
                            "StateMachineName": "express-workflow",
                            "StateMachineType": "EXPRESS",
                            "RoleArn": ROLE_ARN,
                            "DefinitionString": json!({
                                "StartAt": "Done",
                                "States": {
                                    "Done": {
                                        "Type": "Pass",
                                        "Result": "${Outcome}",
                                        "End": true,
                                    },
                                },
                            })
                            .to_string(),
                            "DefinitionSubstitutions": { "Outcome": "expressed" },
                        },
                    },
                },
                "Outputs": {
                    "StateMachine": { "Value": { "Fn::Sub": "${Workflow}" } },
                    "StateMachineName": { "Value": { "Fn::GetAtt": ["Workflow", "Name"] } },
                },
            })
            .to_string()
        },
        declared_name: Some("express-workflow"),
        reference_output: "StateMachine",
        name_output: "StateMachineName",
        execution_name: Some("express-run"),
        express: true,
    },
];

/// Every row: the reference the stack publishes for its state machine is that machine's ARN,
/// `StartExecution` serves a start against it with an `executionArn` and a `startDate`, and
/// Step Functions holds the execution the call reported.
#[tokio::test]
async fn a_stack_published_state_machine_reference_starts_an_execution() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("sfn-reference-stack-{index}");

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &(row.template)()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // The handle a client reads out of the stack, exactly as it reads it.
        let outputs = outputs_of(&described);
        let reference = outputs
            .get(row.reference_output)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an output {:?} in {outputs:?}",
                    row.reference_output
                )
            });

        // The machine's NAME, read through the separate attribute the type publishes it as —
        // so the name half of the rule is known independently of the reference being checked,
        // including for the rows whose name CloudFormation generated.
        let machine_name = outputs.get(row.name_output).cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an output {:?} in {outputs:?}",
                row.name_output
            )
        });
        if let Some(declared) = row.declared_name {
            assert_eq!(
                machine_name, declared,
                "case {case:?}: expected the Name attribute to be the declared name \
                 {declared:?}, got {machine_name:?}"
            );
        }

        // "Ref returns the ARN of the state machine": the published reference is that ARN,
        // whichever intrinsic resolved it, over the name the template asked for or the one
        // CloudFormation generated.
        let expected_reference = state_machine_arn(region, &machine_name);
        assert_eq!(
            reference, expected_reference,
            "case {case:?}: expected the stack's reference to be the machine's ARN \
             {expected_reference:?}, got {reference:?}"
        );

        // And the start a client makes with it is SERVED.
        let mut request = json!({ "stateMachineArn": reference, "input": "{}" });
        if let Some(name) = row.execution_name {
            request["name"] = json!(name);
        }
        let (status, started) = sfn(&router, region, "StartExecution", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartExecution on the reference the stack \
             published, got {status} with body {started}"
        );
        let started_arn = started
            .get("executionArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no 'executionArn' in {started}"));
        let execution_name = match row.execution_name {
            Some(name) => {
                assert_eq!(
                    started_arn,
                    execution_arn(region, &machine_name, name),
                    "case {case:?}: expected the execution ARN of the name the start asked for, \
                     got {started_arn:?}"
                );
                name.to_string()
            }
            None => {
                // A start that names no execution gets the name Step Functions minted, inside
                // an execution ARN of the machine the reference named.
                let prefix = execution_arn(region, &machine_name, "");
                let minted = started_arn.strip_prefix(prefix.as_str()).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected an execution ARN under {prefix:?}, got \
                         {started_arn:?}"
                    )
                });
                assert!(
                    !minted.is_empty() && minted.len() <= MAX_EXECUTION_NAME,
                    "case {case:?}: expected a minted execution name inside the modelled \
                     1..{MAX_EXECUTION_NAME}, got {minted:?}"
                );
                minted.to_string()
            }
        };
        let start_date = started.get("startDate").and_then(Value::as_f64);
        assert!(
            start_date.is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected a startDate in epoch seconds, got {:?}",
            started.get("startDate")
        );

        // Step Functions HOLDS that execution, read back through the machine the stack's
        // reference named and through the execution's own ARN — and each read answers for the
        // reference the way the API documents it for the workflow type. A STANDARD machine
        // reports both; for an EXPRESS one `ListExecutions` answers
        // `StateMachineTypeNotSupported` and `DescribeExecution` answers `ExecutionDoesNotExist`
        // ("executions of an EXPRESS state machine aren't supported by DescribeExecution unless
        // a Map Run dispatched them"), which is itself evidence the reference resolved to a
        // machine of that type rather than to nothing.
        let (status, listed) = sfn(
            &router,
            region,
            "ListExecutions",
            &json!({ "stateMachineArn": reference }),
        )
        .await;
        if row.express {
            assert_eq!(
                status, 400,
                "case {case:?}: expected ListExecutions to be refused for an EXPRESS machine, \
                 got {status} with body {listed}"
            );
            assert!(
                listed
                    .get("__type")
                    .and_then(Value::as_str)
                    .is_some_and(|code| code.ends_with("StateMachineTypeNotSupported")),
                "case {case:?}: expected StateMachineTypeNotSupported, got {listed}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListExecutions on the stack's reference, \
                 got {status} with body {listed}"
            );
            let listed_arns: Vec<String> = listed
                .get("executions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no 'executions' array in {listed}"))
                .iter()
                .filter_map(|execution| {
                    execution
                        .get("executionArn")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            assert_eq!(
                listed_arns,
                vec![started_arn.to_string()],
                "case {case:?}: expected the machine to hold the one execution the start \
                 reported, got {listed:?}"
            );
        }

        let (status, execution) = sfn(
            &router,
            region,
            "DescribeExecution",
            &json!({ "executionArn": started_arn }),
        )
        .await;
        if row.express {
            assert_eq!(
                status, 400,
                "case {case:?}: expected DescribeExecution to be refused for an EXPRESS \
                 execution, got {status} with body {execution}"
            );
            assert!(
                execution
                    .get("__type")
                    .and_then(Value::as_str)
                    .is_some_and(|code| code.ends_with("ExecutionDoesNotExist")),
                "case {case:?}: expected ExecutionDoesNotExist, got {execution}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeExecution on the ARN the start \
                 reported, got {status} with body {execution}"
            );
            assert_eq!(
                execution.get("stateMachineArn").and_then(Value::as_str),
                Some(reference.as_str()),
                "case {case:?}: expected the execution to name the stack's reference as its \
                 state machine, got {execution}"
            );
            assert_eq!(
                execution.get("name").and_then(Value::as_str),
                Some(execution_name.as_str()),
                "case {case:?}: expected the execution to carry the name {execution_name:?}, got \
                 {execution}"
            );
        }

        // The other half of the rule: the machine's NAME is a separate attribute, never the
        // reference, and that operation refuses it — which is what a stack publishing a name
        // instead of an ARN costs every consumer of the machine.
        let (status, refused) = sfn(
            &router,
            region,
            "StartExecution",
            &json!({ "stateMachineArn": machine_name, "input": "{}" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from StartExecution on the bare name \
             {machine_name:?}, got {status} with body {refused}"
        );
        assert!(
            refused
                .get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("InvalidArn")),
            "case {case:?}: expected InvalidArn for a value that is not an ARN, got {refused}"
        );
    }
}
