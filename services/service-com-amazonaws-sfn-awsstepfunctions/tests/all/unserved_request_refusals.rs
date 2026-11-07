// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two refusals this service answers rather than serving a request
//! it does not serve, driven over its own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! 1. **a listing that combines the two filters.** `ListExecutions`' own `redriveFilter`
//!    documentation states the rule and the outcome: "If you provide a state machine ARN
//!    in redriveFilter, the API returns a validation exception" — the filter selects the
//!    child workflow executions a Map Run redrove, so it holds together only with
//!    `mapRunArn`. The combination is a defect in the REQUEST, so it is refused with HTTP
//!    400 `ValidationException` and the service's fixed sentence before the state machine
//!    the request names is resolved at all — never with an authoritative-looking empty
//!    page.
//! 2. **an execution ARN an operation does not serve.** An operation that documents it
//!    does not serve EXPRESS executions refuses the ARN by its RESOURCE TYPE: HTTP 400
//!    `InvalidArn`, carrying the one ARN diagnosis this service words for a resource type
//!    that is not valid where it was handed in, with the type an Express execution's ARN
//!    carries named in it and nothing else — no operation name and no ARN quoted back.
//!
//! The rows differ along what each rule generalises over: the workflow type of the state
//! machine the listing names (STANDARD and EXPRESS), the value of `redriveFilter`, what
//! else the request carries (a page size, a status filter), a state machine ARN nothing
//! answers to, and — for the ARN rule — each operation that reads an execution ARN and
//! excludes Express.
//!
//! The harness (`Emulator`, `create_machine`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` builds; `tests/all.rs` is this binary's crate root, so it is
//! reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EXPRESS_EXECUTION_RESOURCE;

use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION, ROLE_ARN,
};

/// The modelled refusal class a request whose members cannot hold together answers, and
/// the status it carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";
const CLIENT_REFUSAL_STATUS: u16 = 400;

/// The modelled refusal class an ARN this service does not serve answers.
const INVALID_ARN: &str = "InvalidArn";

/// The sentence `ListExecutions` refuses `redriveFilter` beside `stateMachineArn` with:
/// fixed text, naming the two members in the service's own words.
const REDRIVE_FILTER_WITH_STATE_MACHINE: &str =
    "RedriveFilter cannot be applied to ListExecutions call with StateMachine ARN";

/// What the one ARN diagnosis says about a resource type that is not valid where the ARN
/// was handed in.
fn resource_type_refusal(kind: &str) -> String {
    format!("Invalid Arn: 'Resource type not valid in this context: {kind}'")
}

/// An EXPRESS state machine, as its ARN.
async fn create_express(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "type": "EXPRESS",
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// `redriveFilter` beside `stateMachineArn` is refused with the one fixed sentence,
/// whatever else the request carries and whichever state machine it names — including one
/// this account does not hold, since the combination is judged before the lookup.
#[tokio::test]
async fn a_listing_that_combines_the_redrive_filter_with_a_state_machine_is_refused() {
    let emulator = Emulator::new();
    let standard = create_machine(&emulator, "redrive-filter-standard", HELLO_DEFINITION).await;
    let express = create_express(&emulator, "redrive-filter-express").await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:nothing-here");

    // (case, the state machine ARN the listing names, the rest of the request)
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a STANDARD state machine and NOT_REDRIVEN",
            standard.as_str(),
            json!({ "redriveFilter": "NOT_REDRIVEN" }),
        ),
        (
            "a STANDARD state machine and REDRIVEN",
            standard.as_str(),
            json!({ "redriveFilter": "REDRIVEN" }),
        ),
        (
            "an EXPRESS state machine, which the listing does not serve either",
            express.as_str(),
            json!({ "redriveFilter": "NOT_REDRIVEN" }),
        ),
        (
            "every other member of the listing beside it",
            standard.as_str(),
            json!({ "redriveFilter": "REDRIVEN", "maxResults": 1, "statusFilter": "SUCCEEDED" }),
        ),
        (
            "a state machine this account does not hold",
            absent.as_str(),
            json!({ "redriveFilter": "NOT_REDRIVEN" }),
        ),
    ];

    for (case, machine, rest) in rows {
        let mut body = rest;
        body["stateMachineArn"] = json!(machine);
        let (status, answered) = emulator.call("ListExecutions", &body).await;
        let code = error_code(&answered);
        let message = answered.get("message").and_then(Value::as_str);
        assert_eq!(
            (status, code.as_str(), message),
            (
                CLIENT_REFUSAL_STATUS,
                VALIDATION_EXCEPTION,
                Some(REDRIVE_FILTER_WITH_STATE_MACHINE)
            ),
            "case {case:?}: expected the fixed redrive-filter refusal, got {status} {code} with \
             {answered:?}"
        );
    }

    // The control: the same listing WITHOUT the filter is served, so the refusal is about
    // the two members together and not about the operation.
    let listed = emulator
        .ok("ListExecutions", &json!({ "stateMachineArn": &standard }))
        .await;
    assert_eq!(
        listed
            .get("executions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a listing naming no redrive filter must be served, got {listed:?}"
    );

    // ...and the filter beside the OTHER selector is not refused on this rule at all: a
    // Map Run ARN nothing answers to is reported as the resource it names being absent.
    let (status, answered) = emulator
        .call(
            "ListExecutions",
            &json!({
                "mapRunArn": format!(
                    "arn:aws:states:{REGION}:{ACCOUNT}:mapRun:nothing-here/00000000-0000-0000-0000-000000000000"
                ),
                "redriveFilter": "NOT_REDRIVEN",
            }),
        )
        .await;
    assert_ne!(
        answered.get("message").and_then(Value::as_str),
        Some(REDRIVE_FILTER_WITH_STATE_MACHINE),
        "the filter beside a Map Run ARN is the combination the API serves, got {status} \
         {answered:?}"
    );
}

/// Every operation that reads an execution ARN and excludes EXPRESS refuses one by the
/// resource type an Express execution's ARN carries — one diagnosis, naming that type and
/// nothing else.
#[tokio::test]
async fn an_express_execution_is_refused_by_its_resource_type() {
    let emulator = Emulator::new();
    let express = create_express(&emulator, "express-arn-refusal").await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": express, "name": "quick" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    let expected = resource_type_refusal(EXPRESS_EXECUTION_RESOURCE);

    for operation in ["StopExecution", "GetExecutionHistory"] {
        let case = format!("an EXPRESS execution on {operation}");
        let (status, answered) = emulator
            .call(operation, &json!({ "executionArn": execution.clone() }))
            .await;
        let code = error_code(&answered);
        let message = answered.get("message").and_then(Value::as_str);
        assert_eq!(
            (status, code.as_str(), message),
            (
                CLIENT_REFUSAL_STATUS,
                INVALID_ARN,
                Some(expected.as_str())
            ),
            "case {case:?}: expected the resource-type ARN refusal, got {status} {code} with \
             {answered:?}"
        );
        let reported = message.unwrap_or_default();
        assert!(
            !reported.contains(operation) && !reported.contains(execution.as_str()),
            "case {case:?}: the diagnosis names the resource type alone, got {reported:?}"
        );
    }

    // The same operations serve a STANDARD execution, so the refusal is about the ARN's
    // resource type rather than about the operations.
    let standard = create_machine(&emulator, "express-arn-control", HELLO_DEFINITION).await;
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": standard, "name": "slow" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    let answered = emulator
        .ok("GetExecutionHistory", &json!({ "executionArn": execution }))
        .await;
    assert!(
        answered
            .get("events")
            .and_then(Value::as_array)
            .is_some_and(|events| !events.is_empty()),
        "a STANDARD execution's history is served, got {answered:?}"
    );
}
