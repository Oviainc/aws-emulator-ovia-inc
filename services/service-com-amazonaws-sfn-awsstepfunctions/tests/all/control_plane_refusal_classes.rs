// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two refusal CLASSIFICATIONS of the control plane, driven over
//! this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes.
//!
//! The rules under test:
//!
//! 1. `UpdateStateMachine` can only address the state machine RECORD, which an
//!    unqualified ARN names: an ARN qualified with a published version or an alias
//!    names no record this call could update, so it is refused as a state machine
//!    that does not exist — `StateMachineDoesNotExist`, carrying this service's one
//!    resource-refusal sentence for the ARN the request sent. The ARN qualified with
//!    a Distributed Map LABEL keeps its own documented classification
//!    (`ValidationException`, `API_DOES_NOT_SUPPORT_LABELED_ARNS`), and the
//!    unqualified ARN of the same state machine still updates — so the rule is about
//!    which resource the ARN names, not about rejecting qualifiers.
//! 2. `ListExecutions` models both of its selectors as optional, so the one-of rule
//!    is the service's own: a request naming NEITHER is `ValidationException` with
//!    the service's fixed sentence, whatever else the request carries.
//!
//! The harness (`Emulator`, `string`, `error_code`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_aliases::route;
use crate::state_machine_lifecycle::{
    error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION, ROLE_ARN,
};

/// A second definition, so an update that names one asks for a real change.
const GOODBYE_DEFINITION: &str =
    r#"{"StartAt":"Bye","States":{"Bye":{"Type":"Pass","Result":{"msg":"bye"},"End":true}}}"#;

/// One row of the qualified-update table: the case, the ARN the update addresses, and
/// the HTTP status, error code, message and `reason` the refusal must carry.
type QualifiedRow = (String, String, u16, &'static str, String, Option<&'static str>);

/// Create a state machine publishing version 1, and answer
/// `(unqualified ARN, version 1 ARN)`.
async fn published(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "publish": true,
            }),
        )
        .await;
    (
        string(&created, "stateMachineArn"),
        string(&created, "stateMachineVersionArn"),
    )
}

/// The `UpdateStateMachine` body that addresses `arn` and names `member` as the one
/// change it asks for.
fn update_naming(arn: &str, member: &str) -> Value {
    let mut body = json!({ "stateMachineArn": arn });
    match member {
        "definition" => body["definition"] = json!(GOODBYE_DEFINITION),
        "roleArn" => body["roleArn"] = json!(ROLE_ARN),
        // Both at once: a request may name any set of the members an update changes.
        _ => {
            body["definition"] = json!(GOODBYE_DEFINITION);
            body["roleArn"] = json!(ROLE_ARN);
        }
    }
    body
}

/// This service's one resource-refusal sentence for a state machine that does not
/// exist (`state::resource_error_message` over `state::STATE_MACHINE_DOES_NOT_EXIST`).
fn does_not_exist(arn: &str) -> String {
    format!("State Machine Does Not Exist: '{arn}'")
}

/// What a labeled (Distributed Map) ARN is refused with: this service's own fixed
/// sentence for the rule, which names no value of the request
/// (`state::LABELED_ARN_MESSAGE`), under the modelled `ValidationExceptionReason`
/// (`state::VALIDATION_REASON_LABELED_ARN`).
const LABELED_ARN_MESSAGE: &str = "Labeled State Machine ARNs are not supported for this API";
const LABELED_ARN_REASON: &str = "API_DOES_NOT_SUPPORT_LABELED_ARNS";

// ---------------------------------------------------------------------------
// 1. Which resource an UpdateStateMachine ARN names decides the refusal class
// ---------------------------------------------------------------------------

/// An `UpdateStateMachine` whose ARN is qualified with a VERSION or an ALIAS is
/// refused as a state machine that does not exist, while a LABEL-qualified ARN keeps
/// the documented `ValidationException` and the unqualified ARN still updates.
///
/// The rows vary along the qualifier's CLASS (version number, alias name, Distributed
/// Map label), along whether the resource the qualifier names exists at all, and
/// along which member the update asks to change — the rule is about the ARN, so none
/// of those changes the answer within a class.
#[tokio::test]
async fn an_update_on_a_qualified_arn_names_no_state_machine_to_update() {
    let emulator = Emulator::new();
    let (machine, version_one) = published(&emulator, "qualified-update").await;
    // A second version and an alias that routes to it, so the rows span qualifiers a
    // record really answers for as well as ones nothing does.
    let updated = emulator
        .ok(
            "UpdateStateMachine",
            &json!({
                "stateMachineArn": &machine,
                "definition": GOODBYE_DEFINITION,
                "publish": true,
            }),
        )
        .await;
    let version_two = string(&updated, "stateMachineVersionArn");
    let alias = emulator
        .ok(
            "CreateStateMachineAlias",
            &json!({
                "name": "live",
                "routingConfiguration": [route(&version_two, 100)],
            }),
        )
        .await;
    let alias_arn = string(&alias, "stateMachineAliasArn");
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    let mut rows: Vec<QualifiedRow> = Vec::new();
    for arn in [
        version_one.clone(),
        version_two.clone(),
        // A version number nothing was published for, and an alias name no alias
        // answers for: the ARN still names a version or an alias rather than the
        // state machine, so the class is the same.
        format!("{machine}:99"),
        alias_arn.clone(),
        format!("{machine}:no-such-alias"),
        // And the same two qualifier classes on a state machine that was never
        // created at all.
        format!("{absent}:1"),
        format!("{absent}:live"),
    ] {
        rows.push((
            format!("the qualified ARN {arn:?}"),
            arn.clone(),
            400,
            "StateMachineDoesNotExist",
            does_not_exist(&arn),
            None,
        ));
    }
    // "If you provide a qualified state machine ARN that refers to a Distributed Map
    // state, the request fails with ValidationException": the label is its own class.
    for label in ["mapStateLabel", "inner-map_1"] {
        let arn = format!("{machine}/{label}");
        rows.push((
            format!("the labeled ARN {arn:?}"),
            arn,
            400,
            "ValidationException",
            LABELED_ARN_MESSAGE.to_string(),
            Some(LABELED_ARN_REASON),
        ));
    }

    for (case, arn, status, code, message, reason) in &rows {
        for member in ["definition", "roleArn", "both"] {
            let (actual_status, body) = emulator
                .call("UpdateStateMachine", &update_naming(arn, member))
                .await;
            let actual_code = error_code(&body);
            assert_eq!(
                (actual_status, actual_code.as_str()),
                (*status, *code),
                "case {case:?} naming {member}: expected {status} {code}, got {actual_status} \
                 {actual_code} with body {body:?}"
            );
            let actual_message = body.get("message").and_then(Value::as_str);
            assert_eq!(
                actual_message,
                Some(message.as_str()),
                "case {case:?} naming {member}: expected the message {message:?}, got \
                 {actual_message:?}"
            );
            let actual_reason = body.get("reason").and_then(Value::as_str);
            assert_eq!(
                actual_reason, *reason,
                "case {case:?} naming {member}: expected the reason {reason:?}, got \
                 {actual_reason:?}"
            );
        }
    }

    // The same call on the UNQUALIFIED ARN is served: the refusals above are about
    // the resource the ARN named, not about the request's members.
    let served = emulator
        .ok("UpdateStateMachine", &update_naming(&machine, "definition"))
        .await;
    assert!(
        served.get("updateDate").is_some(),
        "the unqualified ARN must still update, got {served:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. ListExecutions with neither selector
// ---------------------------------------------------------------------------

/// `ListExecutions` naming neither `stateMachineArn` nor `mapRunArn` is the modelled
/// `ValidationException`, in the service's own fixed sentence, whatever else the
/// request carries.
///
/// The rows vary along the other members a listing may name — the page size, the page
/// token, the status filter and the redrive filter — because the selector rule is
/// judged before any of them.
#[tokio::test]
async fn a_listing_that_names_no_selector_is_refused_in_one_fixed_sentence() {
    let emulator = Emulator::new();
    // A state machine with an execution exists, so the refusal is not an artefact of
    // an empty store.
    let machine = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "listing-selector",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    let machine = string(&machine, "stateMachineArn");
    emulator
        .ok("StartExecution", &json!({ "stateMachineArn": &machine }))
        .await;

    let rows: Vec<(&str, Value)> = vec![
        ("no other member at all", json!({})),
        ("a page size", json!({ "maxResults": 10 })),
        ("the default page size", json!({ "maxResults": 0 })),
        ("a status filter", json!({ "statusFilter": "RUNNING" })),
        (
            "a redrive filter",
            json!({ "redriveFilter": "NOT_REDRIVEN" }),
        ),
        (
            "every other member at once",
            json!({ "maxResults": 1, "statusFilter": "SUCCEEDED", "redriveFilter": "REDRIVEN" }),
        ),
    ];

    for (case, body) in &rows {
        let (status, answered) = emulator.call("ListExecutions", body).await;
        let code = error_code(&answered);
        let message = answered.get("message").and_then(Value::as_str);
        assert_eq!(
            (status, code.as_str(), message),
            (
                400,
                "ValidationException",
                Some("Must provide a StateMachine ARN or MapRun ARN")
            ),
            "case {case:?}: expected the fixed one-of refusal, got {status} {code} with body \
             {answered:?}"
        );
    }

    // And a listing that DOES name a selector is served, so the refusal is about the
    // selector being absent rather than about the operation.
    let listed = emulator
        .ok("ListExecutions", &json!({ "stateMachineArn": &machine }))
        .await;
    assert_eq!(
        listed.get("executions").and_then(Value::as_array).map(Vec::len),
        Some(1),
        "a listing naming the state machine must report its one execution, got {listed:?}"
    );
}
