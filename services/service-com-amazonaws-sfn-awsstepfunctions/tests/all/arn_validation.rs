// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ARN validation stage every Step Functions operation
//! runs BEFORE it resolves a resource, driven over this service's wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`, so what is asserted is what a
//! signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. an ARN member is VALIDATED before the lookup: text that is not a well-formed
//!    ARN of this service, in the region the request's endpoint serves, is the
//!    modelled `InvalidArn` (client, HTTP 400) whose message wraps a diagnosis of
//!    the first component that failed — `Invalid Arn: '<diagnosis>'` — and never a
//!    resource-absent error, whatever the operation and whichever ARN member it
//!    reads;
//! 2. the region an ARN is checked against is the ENDPOINT's, so the same ARN is
//!    served by the endpoint of its own region and refused by every other;
//! 3. a map-run LABELED state machine ARN (`<stateMachineArn>/<label>`) on an
//!    operation that documents refusing one is the modelled `ValidationException`
//!    carrying `reason` `API_DOES_NOT_SUPPORT_LABELED_ARNS`, while an operation that
//!    accepts a labeled ARN resolves it like any other.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, ACCOUNT, HELLO_DEFINITION, REGION,
};

/// Region codes other than the harness's endpoint region, from AWS's published
/// region list. Rows vary along the region's partition and distance, because the
/// rule is on the ARN's region FIELD and not on which region it names.
const FOREIGN_REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];

/// The `__type` discriminator of an awsJson error body, without its namespace.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` member of an error body.
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// `arn` with its region field replaced by `region`.
fn with_region(arn: &str, region: &str) -> String {
    let mut fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {arn:?}: expected an ARN of six fields to re-region"
    );
    fields[3] = region;
    fields.join(":")
}

/// The request body that puts `arn` in `member`, plus any other member the
/// operation models as required.
fn request(operation: &str, member: &str, arn: &str) -> Value {
    let mut body = json!({ member: arn });
    if operation == "TagResource" {
        body["tags"] = json!([{ "key": "k", "value": "v" }]);
    }
    body
}

/// Start an execution that must succeed, and return its ARN.
async fn start(emulator: &Emulator, state_machine_arn: &str, name: &str) -> String {
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": state_machine_arn, "name": name }),
        )
        .await;
    string(&started, "executionArn")
}

/// Assert that `operation` refuses `arn` in `member` with `InvalidArn` and exactly
/// `expected_message`.
async fn expect_invalid_arn(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    member: &str,
    arn: &str,
    expected_message: &str,
) {
    let (status, body) = emulator
        .call(operation, &request(operation, member, arn))
        .await;
    let actual_type = error_type(&body);
    let actual_message = error_message(&body);
    assert_eq!(
        (status, actual_type.as_str()),
        (400, "InvalidArn"),
        "case {case:?} on {operation}.{member}: expected 400 InvalidArn, got {status} \
         {actual_type} with body {body:?}"
    );
    assert_eq!(
        actual_message, expected_message,
        "case {case:?} on {operation}.{member}: expected message {expected_message:?}, got \
         {actual_message:?}"
    );
}

// ---------------------------------------------------------------------------
// An ARN of another region
// ---------------------------------------------------------------------------

/// Every ARN member of every implemented operation (13 pairs) refuses an ARN whose region is
/// not the endpoint's, with `InvalidArn` and the region-mismatch message — not with
/// the resource-absent error, and not with a 404.
#[tokio::test]
async fn every_arn_member_refuses_an_arn_from_another_region() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "arn-rule", HELLO_DEFINITION).await;
    let execution = start(&emulator, &machine, "run-1").await;
    // Neither a map run nor an activity can exist in this emulator, so these two are
    // built from the ARN forms the model documents; the region check does not depend
    // on the resource being there.
    let map_run = format!(
        "arn:aws:states:{REGION}:{ACCOUNT}:mapRun:arn-rule/mapLabel:\
         0e6e0d0e-0000-4000-8000-00000000abcd"
    );
    let activity = format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:an-activity");

    // (operation, ARN member, an ARN of this region the member accepts) — every ARN
    // member of every implemented operation.
    let members: Vec<(&str, &str, &String)> = vec![
        ("StartExecution", "stateMachineArn", &machine),
        ("DescribeStateMachine", "stateMachineArn", &machine),
        ("ListExecutions", "stateMachineArn", &machine),
        ("DeleteStateMachine", "stateMachineArn", &machine),
        ("UpdateStateMachine", "stateMachineArn", &machine),
        ("TagResource", "resourceArn", &machine),
        ("ListTagsForResource", "resourceArn", &machine),
        ("TagResource", "resourceArn", &activity),
        ("ListTagsForResource", "resourceArn", &activity),
        ("DescribeExecution", "executionArn", &execution),
        ("StopExecution", "executionArn", &execution),
        ("RedriveExecution", "executionArn", &execution),
        ("ListExecutions", "mapRunArn", &map_run),
    ];

    for foreign in FOREIGN_REGIONS {
        for (operation, member, base) in &members {
            let arn = with_region(base, foreign);
            let expected =
                format!("Invalid Arn: 'Expected the ARN {arn} to be within region ({REGION}).'");
            expect_invalid_arn(
                &emulator,
                &format!("an ARN of {foreign}"),
                operation,
                member,
                &arn,
                &expected,
            )
            .await;
        }
    }

    // No refusal above touched the resources this region does hold.
    let described = emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    assert_eq!(
        string(&described, "stateMachineArn"),
        machine,
        "the in-region state machine must still be served, got {described:?}"
    );
    let described = emulator
        .ok("DescribeExecution", &json!({ "executionArn": execution }))
        .await;
    assert_eq!(
        string(&described, "executionArn"),
        execution,
        "the in-region execution must still be served, got {described:?}"
    );
}

/// The region an ARN is checked against is the ENDPOINT's: over one store, each
/// regional endpoint serves the ARNs it minted and refuses every other endpoint's,
/// even though those resources DO exist — which is what shows the validation runs
/// before the lookup.
#[tokio::test]
async fn the_endpoint_region_is_what_an_arn_is_checked_against() {
    let regions = ["us-west-2", "eu-west-1", "ap-southeast-2"];
    let first = Emulator::in_region(regions[0]);
    let mut endpoints: Vec<(&str, Emulator)> = vec![(regions[0], first)];
    for region in &regions[1..] {
        let endpoint = Emulator::in_region_on(&endpoints[0].1, region);
        endpoints.push((region, endpoint));
    }

    // One state machine and one execution per endpoint, under the SAME names, so the
    // only thing that differs between the rows is the region field.
    let mut minted: Vec<(&str, String, String)> = Vec::new();
    for (region, endpoint) in &endpoints {
        let machine = create_machine(endpoint, "regional", HELLO_DEFINITION).await;
        assert_eq!(
            machine,
            format!("arn:aws:states:{region}:{ACCOUNT}:stateMachine:regional"),
            "case {region:?}: the endpoint must mint its own region's ARN, got {machine:?}"
        );
        let execution = start(endpoint, &machine, "run-1").await;
        minted.push((region, machine, execution));
    }

    for (endpoint_region, endpoint) in &endpoints {
        for (arn_region, machine, execution) in &minted {
            for (operation, member, arn) in [
                ("DescribeStateMachine", "stateMachineArn", machine),
                ("DescribeExecution", "executionArn", execution),
            ] {
                let case = format!("the {endpoint_region} endpoint reading a {arn_region} ARN");
                if endpoint_region == arn_region {
                    let body = endpoint.ok(operation, &request(operation, member, arn)).await;
                    assert_eq!(
                        &string(&body, member),
                        arn,
                        "case {case:?} on {operation}: the endpoint must serve its own region's \
                         ARN, got {body:?}"
                    );
                } else {
                    let expected = format!(
                        "Invalid Arn: 'Expected the ARN {arn} to be within region \
                         ({endpoint_region}).'"
                    );
                    expect_invalid_arn(endpoint, &case, operation, member, arn, &expected).await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A malformed ARN
// ---------------------------------------------------------------------------

/// A malformed ARN is `InvalidArn` on every ARN member, and the message names the
/// first component that failed inside the service's `Invalid Arn: '...'` envelope.
#[tokio::test]
async fn a_malformed_arn_reports_the_component_that_failed() {
    let emulator = Emulator::new();
    create_machine(&emulator, "arn-shape", HELLO_DEFINITION).await;

    // Rows vary along WHICH component of the ARN is wrong. These ones are
    // independent of the resource type the member expects, so each is asserted on
    // every ARN member below.
    let shared: Vec<(&str, String, String)> = vec![
        (
            "text that is not an ARN",
            "not-a-valid-arn".to_string(),
            "Invalid ARN prefix: not-a-valid-arn".to_string(),
        ),
        (
            "a bare resource name",
            "arn-shape".to_string(),
            "Invalid ARN prefix: arn-shape".to_string(),
        ),
        (
            "an ARN with no resource field",
            format!("arn:aws:states:{REGION}:{ACCOUNT}"),
            format!("Invalid ARN: expected 6 colon-delimited fields in arn:aws:states:{REGION}:{ACCOUNT}"),
        ),
        (
            "an empty partition",
            format!("arn::states:{REGION}:{ACCOUNT}:stateMachine:arn-shape"),
            format!(
                "Invalid ARN partition: arn::states:{REGION}:{ACCOUNT}:stateMachine:arn-shape"
            ),
        ),
        (
            // The components are judged in the order they are written, so the
            // partition is diagnosed even though the ARN also stops short.
            "an empty partition on a truncated ARN",
            "arn::states".to_string(),
            "Invalid ARN partition: arn::states".to_string(),
        ),
        (
            "another service's ARN",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:arn-shape"),
            "Invalid ARN service: lambda, expected states".to_string(),
        ),
    ];

    // Every (operation, ARN member) pair an implemented operation reads.
    let members = [
        ("StartExecution", "stateMachineArn"),
        ("DescribeStateMachine", "stateMachineArn"),
        ("ListExecutions", "stateMachineArn"),
        ("DeleteStateMachine", "stateMachineArn"),
        ("UpdateStateMachine", "stateMachineArn"),
        ("TagResource", "resourceArn"),
        ("ListTagsForResource", "resourceArn"),
        ("DescribeExecution", "executionArn"),
        ("StopExecution", "executionArn"),
        ("RedriveExecution", "executionArn"),
        ("ListExecutions", "mapRunArn"),
    ];

    for (case, value, diagnosis) in &shared {
        for (operation, member) in members {
            expect_invalid_arn(
                &emulator,
                case,
                operation,
                member,
                value,
                &format!("Invalid Arn: '{diagnosis}'"),
            )
            .await;
        }
    }

    // And the rows whose diagnosis depends on the resource type the member reads:
    // (case, operation, member, ARN, diagnosis)
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");
    let typed: Vec<(&str, &str, &str, String, String)> = vec![
        (
            "an activity ARN where an execution ARN is required",
            "DescribeExecution",
            "executionArn",
            format!("{prefix}:activity:an-activity"),
            "Invalid ARN resource type: activity, expected execution".to_string(),
        ),
        (
            "an execution ARN where a state machine ARN is required",
            "StartExecution",
            "stateMachineArn",
            format!("{prefix}:execution:arn-shape:run-1"),
            "Invalid ARN resource type: execution, expected stateMachine".to_string(),
        ),
        (
            "a map run ARN where a taggable ARN is required",
            "ListTagsForResource",
            "resourceArn",
            format!("{prefix}:mapRun:arn-shape/mapLabel:abcd"),
            "Invalid ARN resource type: mapRun, expected stateMachine or activity".to_string(),
        ),
        (
            "a state machine ARN where a map run ARN is required",
            "ListExecutions",
            "mapRunArn",
            format!("{prefix}:stateMachine:arn-shape"),
            "Invalid ARN resource type: stateMachine, expected mapRun".to_string(),
        ),
        (
            "a state machine ARN with no name",
            "DescribeStateMachine",
            "stateMachineArn",
            format!("{prefix}:stateMachine:"),
            format!("Invalid ARN stateMachine name: {prefix}:stateMachine:"),
        ),
        (
            "an execution ARN with no execution name",
            "StopExecution",
            "executionArn",
            format!("{prefix}:execution:arn-shape"),
            "Invalid ARN execution resource: execution:arn-shape".to_string(),
        ),
        (
            "a map run ARN with no resource",
            "ListExecutions",
            "mapRunArn",
            format!("{prefix}:mapRun"),
            "Invalid ARN mapRun resource: mapRun".to_string(),
        ),
    ];

    for (case, operation, member, arn, diagnosis) in &typed {
        expect_invalid_arn(
            &emulator,
            case,
            operation,
            member,
            arn,
            &format!("Invalid Arn: '{diagnosis}'"),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// A map-run labeled ARN
// ---------------------------------------------------------------------------

/// A labeled (Distributed Map) state machine ARN is refused by the operations that
/// document refusing one, with `ValidationException` and the modelled
/// `API_DOES_NOT_SUPPORT_LABELED_ARNS` reason (3 operations × 5 label forms ×
/// {existing, absent} state machine).
#[tokio::test]
async fn a_labeled_state_machine_arn_is_refused_with_the_modelled_reason() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "mapper", HELLO_DEFINITION).await;
    let absent = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:never-created");

    // `StartExecution`, `DeleteStateMachine` and `UpdateStateMachine` are the
    // implemented operations whose own documentation says a qualified ARN naming a
    // Distributed Map state "fails with ValidationException";
    // `ValidationExceptionReason` names the class.
    let operations = ["StartExecution", "DeleteStateMachine", "UpdateStateMachine"];
    // Rows vary along the label's own form, and along whether the state machine the
    // ARN labels exists at all — the ARN is judged before anything is looked up.
    let labels = ["mapStateLabel", "Map", "inner-map_1", "a.b", "0"];

    for base in [&machine, &absent] {
        for label in labels {
            let arn = format!("{base}/{label}");
            for operation in operations {
                let (status, body) = emulator
                    .call(operation, &request(operation, "stateMachineArn", &arn))
                    .await;
                let actual_type = error_type(&body);
                assert_eq!(
                    (status, actual_type.as_str()),
                    (400, "ValidationException"),
                    "case {arn:?} on {operation}: expected 400 ValidationException, got {status} \
                     {actual_type} with body {body:?}"
                );
                assert_eq!(
                    body.get("reason").and_then(Value::as_str),
                    Some("API_DOES_NOT_SUPPORT_LABELED_ARNS"),
                    "case {arn:?} on {operation}: expected the modelled labeled-ARN reason, got \
                     {body:?}"
                );
                let message = error_message(&body);
                assert_eq!(
                    message, "Labeled State Machine ARNs are not supported for this API",
                    "case {arn:?} on {operation}: unexpected message {message:?}"
                );
            }
        }
    }

    // `DescribeStateMachine` reports a `label` member, so it ACCEPTS a labeled ARN
    // and resolves it against the store: nothing holds one here, which is its own
    // resource-absent error rather than a validation refusal.
    let labeled = format!("{machine}/mapStateLabel");
    let (status, body) = emulator
        .call(
            "DescribeStateMachine",
            &json!({ "stateMachineArn": labeled }),
        )
        .await;
    let actual_type = error_type(&body);
    assert_eq!(
        (status, actual_type.as_str()),
        (400, "StateMachineDoesNotExist"),
        "a labeled ARN on DescribeStateMachine must be resolved, not refused, got {status} \
         {actual_type} with body {body:?}"
    );

    // And the UNLABELED ARN of the same state machine still starts an execution.
    let started = emulator
        .ok(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "unlabeled" }),
        )
        .await;
    assert_eq!(
        string(&started, "executionArn"),
        format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:mapper:unlabeled"),
        "the unlabeled ARN must still start an execution, got {started:?}"
    );
}
