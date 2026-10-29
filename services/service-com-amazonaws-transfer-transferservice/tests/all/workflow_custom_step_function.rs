// SPDX-License-Identifier: CC0-1.0

//! The refusal `Message` RULE for a workflow step that names no function to invoke.
//!
//! A `CUSTOM` step is "a custom step with an Lambda function target", and its
//! `CustomStepDetails` "consists of the Lambda function's name, target, and timeout",
//! with `Target` "The ARN for the Lambda function that is being called". A step that
//! names no such function cannot run, so a workflow definition carrying one is refused
//! with the modelled `InvalidRequestException` (HTTP 400) carrying ONE fixed sentence
//! about the defect — `Invalid function provided` — and nothing from the request: no
//! step index, no member path, no step name. Two tables assert that:
//!
//! 1. every way a definition can name no function is refused with exactly that text,
//!    whichever step list the step sits in, wherever in it, however many steps the list
//!    holds and whichever of the details members it did send — and persists nothing;
//! 2. the complement: a `CUSTOM` step that DOES name a function is accepted and reads
//!    back through `DescribeWorkflow` with the target it named, over targets differing
//!    in partition, length and function name, including `CustomStepTarget`'s published
//!    maximum length — while one character past that maximum is refused for the length,
//!    not for the function.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{drain_pages, member_of, Emulator};

/// The fixed text the service answers for a `CUSTOM` step that names no function.
const INVALID_FUNCTION: &str = "Invalid function provided";

/// `CustomStepTarget`'s modelled maximum length (`len 0..170`,
/// `smithy_query(query="shape", shape="com.amazonaws.transfer#CustomStepDetails")`).
const TARGET_MAX_LEN: usize = 170;

/// A `SourceFileLocation` satisfying its modelled pattern `^\$\{(\w+.)+\w+\}$` — the
/// non-default one the member documents.
const ORIGINAL_SOURCE: &str = "${original.file}";

/// A step that satisfies every documented requirement, so a table row is refused for
/// the function it lacks and not for a neighbouring step.
fn valid_step(step_type: &str) -> Value {
    match step_type {
        "DELETE" => json!({"Type": "DELETE", "DeleteStepDetails": {"Name": "delete-step"}}),
        "TAG" => json!({
            "Type": "TAG",
            "TagStepDetails": {"Name": "tag-step", "Tags": [{"Key": "scan", "Value": "clean"}]},
        }),
        other => panic!("{other} is not a step type this suite builds"),
    }
}

/// A `CUSTOM` step whose `CustomStepDetails` are exactly `details`; `None` sends the
/// step with no details member at all.
fn custom_step(details: Option<Value>) -> Value {
    match details {
        Some(details) => json!({"Type": "CUSTOM", "CustomStepDetails": details}),
        None => json!({"Type": "CUSTOM"}),
    }
}

/// A Lambda function ARN of exactly `len` characters, satisfying `CustomStepTarget`'s
/// modelled pattern `^arn:[a-z-]+:lambda:.*$`, so a bound can be tested without a
/// literal of that width in the source.
fn target_of_length(len: usize) -> String {
    let prefix = "arn:aws:lambda:us-east-1:000000000000:function:";
    assert!(len > prefix.len(), "a target of {len} characters needs a longer suffix");
    format!("{prefix}{}", "f".repeat(len - prefix.len()))
}

/// The `Message` of an answer, or `""` when it carries none.
fn message_of(body: &Value) -> String {
    member_of(body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a workflow definition carrying a `CUSTOM` step that names no Lambda function is
/// refused with `InvalidRequestException` (HTTP 400) whose `Message` is exactly
/// `Invalid function provided`, carries nothing the request named, and persists nothing.
///
/// The rows differ along how the function is missing (no details at all, empty details,
/// details naming only other members), which of those other members the step did send,
/// which of the two step lists the step sits in, its index in that list, how many steps
/// the list holds (up to `WorkflowSteps`' modelled maximum of 8) and the step's own name.
#[tokio::test]
async fn a_custom_step_naming_no_function_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();

    // One workflow that IS valid, so the table cannot pass by refusing everything.
    let baseline = emulator
        .call(
            "CreateWorkflow",
            json!({"Description": "valid-before", "Steps": [valid_step("TAG")]}),
        )
        .await;
    assert_eq!(
        baseline.status, 200,
        "case \"a valid workflow before the table\": expected 200, got {} with body {}",
        baseline.status, baseline.body
    );
    let kept = baseline.get("WorkflowId").as_str().unwrap_or_default().to_string();

    // A list of 8 steps — `WorkflowSteps`' modelled maximum (`len 0..8`) — whose LAST
    // one names no function, so the deepest index the model admits is covered.
    let mut longest_list: Vec<Value> = (0..7).map(|_| valid_step("DELETE")).collect();
    longest_list.push(custom_step(Some(json!({"Name": "last-step"}))));

    // (case, request, the step name the request carried)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no details at all",
            json!({"Steps": [custom_step(None)]}),
            "",
        ),
        (
            "details carrying nothing",
            json!({"Steps": [custom_step(Some(json!({})))]}),
            "",
        ),
        (
            "details naming only the step",
            json!({"Steps": [custom_step(Some(json!({"Name": "step-custom"})))]}),
            "step-custom",
        ),
        (
            "details naming a differently named step",
            json!({"Steps": [custom_step(Some(json!({"Name": "invoke-scanner"})))]}),
            "invoke-scanner",
        ),
        (
            "details naming a timeout but no function",
            // `TimeoutSeconds` is modelled `range 1..1800`; this is its maximum.
            json!({"Steps": [custom_step(Some(json!({"Name": "waits", "TimeoutSeconds": 1800})))]}),
            "waits",
        ),
        (
            "details naming a source file but no function",
            json!({"Steps": [custom_step(Some(
                json!({"Name": "reads-original", "SourceFileLocation": ORIGINAL_SOURCE}),
            ))]}),
            "reads-original",
        ),
        (
            "the SECOND of three steps",
            json!({"Steps": [
                valid_step("DELETE"),
                custom_step(Some(json!({"Name": "middle-step"}))),
                valid_step("TAG"),
            ]}),
            "middle-step",
        ),
        (
            "the LAST step of a list at the modelled maximum length",
            json!({"Steps": longest_list}),
            "last-step",
        ),
        (
            "the only exception step",
            json!({
                "Steps": [valid_step("DELETE")],
                "OnExceptionSteps": [custom_step(None)],
            }),
            "",
        ),
        (
            "the SECOND exception step",
            json!({
                "Steps": [valid_step("TAG")],
                "OnExceptionSteps": [
                    valid_step("DELETE"),
                    custom_step(Some(json!({"Name": "exception-step"}))),
                ],
            }),
            "exception-step",
        ),
        (
            "a targetless step in BOTH lists",
            json!({
                "Steps": [custom_step(Some(json!({"Name": "first-list"})))],
                "OnExceptionSteps": [custom_step(Some(json!({"Name": "second-list"})))],
            }),
            "first-list",
        ),
    ];

    for (case, request, step_name) in cases {
        let answer = emulator.call("CreateWorkflow", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            answer.body
        );
        let message = message_of(&answer.body);
        assert_eq!(
            message, INVALID_FUNCTION,
            "case {case:?}: expected the fixed text {INVALID_FUNCTION:?}, got {message:?}"
        );
        // Nothing the request named reaches the text: not the member path, not the
        // list the step sat in, not its index, not its name.
        let mut absent = vec![
            "Steps",
            "OnExceptionSteps",
            "CustomStepDetails",
            "Target",
            "[0]",
            "[1]",
            "[7]",
        ];
        if !step_name.is_empty() {
            absent.push(step_name);
        }
        for fragment in absent {
            assert!(
                !message.contains(fragment),
                "case {case:?}: expected no request data in Message, got {message:?} \
                 carrying {fragment:?}"
            );
        }
        assert!(
            answer.body.get("WorkflowId").is_none(),
            "case {case:?}: a refused create must answer no identifier, got {}",
            answer.body
        );
    }

    // Nothing the table submitted was persisted: the store still holds only the
    // workflow created before it.
    let (_, ids) = drain_pages(
        &emulator,
        "ListWorkflows",
        &json!({}),
        "Workflows",
        "WorkflowId",
        None,
    )
    .await;
    assert_eq!(
        ids,
        vec![kept],
        "case \"the store after the whole table\": expected only the accepted workflow, \
         got {ids:?}"
    );
}

/// RULE (the complement): a `CUSTOM` step that DOES name a Lambda function is accepted
/// in either step list and reads back through `DescribeWorkflow` with the target it
/// named — so the refusal above is the function requirement and not a blanket refusal of
/// `CUSTOM` steps.
///
/// The rows differ along the ARN's partition, its function name and its length,
/// including `CustomStepTarget`'s published maximum; the last row is one character past
/// that maximum, which is refused for the length rather than for the function.
#[tokio::test]
async fn a_custom_step_naming_a_function_is_accepted_and_reads_back() {
    let emulator = Emulator::new();

    let accepted: Vec<(&str, String)> = vec![
        (
            "a commercial-partition function",
            "arn:aws:lambda:us-east-1:000000000000:function:custom-step".to_string(),
        ),
        (
            "another partition and another function name",
            "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:scan_file".to_string(),
        ),
        (
            "a qualified function version",
            "arn:aws:lambda:eu-west-1:000000000000:function:custom-step:7".to_string(),
        ),
        (
            "a target at the modelled maximum length",
            target_of_length(TARGET_MAX_LEN),
        ),
    ];

    for (case, target) in accepted {
        let created = emulator
            .call(
                "CreateWorkflow",
                json!({
                    "Description": "names a function",
                    "Steps": [custom_step(Some(json!({"Name": "custom-step", "Target": target})))],
                    "OnExceptionSteps": [custom_step(Some(
                        json!({"Name": "on-exception", "Target": target}),
                    ))],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            created.status, created.body
        );
        let workflow_id = created.get("WorkflowId").as_str().unwrap_or_default().to_string();

        let described = emulator
            .call("DescribeWorkflow", json!({"WorkflowId": &workflow_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeWorkflow 200, got {} with body {}",
            described.status, described.body
        );
        let workflow = described.get("Workflow").clone();
        for list in ["Steps", "OnExceptionSteps"] {
            let step = member_of(&workflow, list)
                .as_array()
                .and_then(|steps| steps.first())
                .cloned()
                .unwrap_or(Value::Null);
            let read_back = member_of(&member_of(&step, "CustomStepDetails").clone(), "Target")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                read_back, target,
                "case {case:?}: expected {list} to report the target {target:?}, got \
                 {read_back:?}"
            );
        }
    }

    // One character past `CustomStepTarget`'s maximum: still refused, but for the
    // length the model states, not for a missing function.
    let too_long = target_of_length(TARGET_MAX_LEN + 1);
    let answer = emulator
        .call(
            "CreateWorkflow",
            json!({
                "Steps": [custom_step(Some(json!({"Name": "too-long", "Target": too_long})))],
            }),
        )
        .await;
    let case = "a target one character past the modelled maximum";
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    let message = message_of(&answer.body);
    assert_ne!(
        message, INVALID_FUNCTION,
        "case {case:?}: a function WAS named, so the refusal must not be the \
         missing-function text, got {message:?}"
    );
    assert!(
        answer.body.get("WorkflowId").is_none(),
        "case {case:?}: a refused create must answer no identifier, got {}",
        answer.body
    );
}
