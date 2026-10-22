// SPDX-License-Identifier: CC0-1.0

//! The workflow STEP contract: what a created step reads back as, whatever the step
//! list it sits in.
//!
//! `workflow_lifecycle` pins the four lifecycle rules. This suite pins the two step
//! properties its rows sample but do not span, each over a table of its own, and keeps
//! its helpers local so it shares no mutable surface with that suite:
//!
//! 1. ORDER, over EVERY list length the model allows: `Steps` and `OnExceptionSteps` are
//!    modelled `len 0..8`, so the rows run 1..=8 steps — and, at each length, a list
//!    whose types are the model's declaration order rotated and a list that repeats one
//!    type — and `DescribeWorkflow` reports the steps in the order `CreateWorkflow` was
//!    given them, in both members.
//! 2. VALUES the caller NAMED for a member that has a documented default: the value is
//!    kept rather than replaced by the default (`SourceFileLocation` named as
//!    `${original.file}`, `OverwriteExisting` as `TRUE`), while `TimeoutSeconds`, which
//!    has NO documented default, stays absent rather than being invented.
//!
//! Plus the minted identifier: `CreateWorkflow` mints a `WorkflowId` of the modelled
//! shape, no two creates mint the same one, and the ARN is built with the region the
//! caller signed for rather than a fixed one.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{assert_members, member_of, Emulator, ACCOUNT};

/// The `SourceFileLocation` value these rows NAME: the other of the two values the
/// member's documentation gives ("To use the originally uploaded file location as input
/// for this step, enter `${original.file}`"), the default being `${previous.file}`.
/// Naming it is what shows the value is kept rather than defaulted over.
const ORIGINAL_SOURCE: &str = "${original.file}";

/// `OverwriteExisting`'s non-default value (the enum is `[FALSE, TRUE]` and the
/// documented default is `FALSE`).
const OVERWRITE_TRUE: &str = "TRUE";

/// Every `WorkflowStepType` value, in the order the model declares it, with the ONE
/// `WorkflowStep` member that configures it ("Each step type has its own `StepDetails`
/// structure"). Source: `smithy_query(query="shape",
/// shape="com.amazonaws.transfer#WorkflowStep")`.
const STEP_TYPES: [(&str, &str); 5] = [
    ("COPY", "CopyStepDetails"),
    ("CUSTOM", "CustomStepDetails"),
    ("TAG", "TagStepDetails"),
    ("DELETE", "DeleteStepDetails"),
    ("DECRYPT", "DecryptStepDetails"),
];

/// Satisfies `CustomStepTarget`'s modelled pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA: &str = "arn:aws:lambda:us-east-1:000000000000:function:on-upload";

/// The `*StepDetails` member `step_type` carries.
fn details_member(step_type: &str) -> &'static str {
    STEP_TYPES
        .iter()
        .find(|(known, _)| *known == step_type)
        .map(|(_, member)| *member)
        .unwrap_or_else(|| panic!("{step_type} is not a modelled WorkflowStepType"))
}

/// A step of `step_type` named `name` that satisfies every documented requirement and
/// names nothing a default would supply.
fn step_of(step_type: &str, name: &str) -> Value {
    let details = match step_type {
        "COPY" => json!({
            "Name": name,
            "DestinationFileLocation": {"S3FileLocation": {"Bucket": "step-target", "Key": "out/"}},
        }),
        "CUSTOM" => json!({"Name": name, "Target": LAMBDA, "TimeoutSeconds": 60}),
        "TAG" => json!({"Name": name, "Tags": [{"Key": "scanned", "Value": "yes"}]}),
        "DELETE" => json!({"Name": name}),
        "DECRYPT" => json!({
            "Name": name,
            "Type": "PGP",
            "DestinationFileLocation": {"S3FileLocation": {"Bucket": "step-clear", "Key": "out/"}},
        }),
        other => panic!("{other} is not a modelled WorkflowStepType"),
    };
    json!({"Type": step_type, details_member(step_type): details})
}

/// Create the workflow `request` asks for and return its `WorkflowId`.
async fn create(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateWorkflow", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWorkflow 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WorkflowId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a WorkflowId, got {}", answer.body))
        .to_string()
}

/// `DescribeWorkflow`'s `Workflow` structure, asserting the read succeeded.
async fn describe(emulator: &Emulator, case: &str, workflow_id: &str) -> Value {
    let answer = emulator
        .call("DescribeWorkflow", json!({"WorkflowId": workflow_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWorkflow 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("Workflow").clone()
}

/// The `Name` of every step in a described step list, in list order.
fn step_names(case: &str, steps: &Value) -> Vec<String> {
    steps
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a step list, got {steps:?}"))
        .iter()
        .map(|step| {
            let step_type = member_of(step, "Type")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a step carries no Type: {step:?}"));
            member_of(member_of(step, details_member(step_type)), "Name")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a step carries no Name: {step:?}"))
                .to_string()
        })
        .collect()
}

/// RULE 1: `DescribeWorkflow` reports a workflow's steps in the order `CreateWorkflow`
/// was given them, for every list length the model allows and whatever the types in it.
///
/// The rows span the list LENGTH (1 to 8, `WorkflowSteps` being `len 0..8`) and, at each
/// length, the type sequence: one row rotates the model's declaration order by the
/// length, so no two rows send the same sequence, and one repeats a single type, so the
/// order cannot be recovered from the types. Each row is asserted in `Steps` and in
/// `OnExceptionSteps`, which hold steps of the same shape.
#[tokio::test]
async fn described_step_order_is_the_created_step_order() {
    let emulator = Emulator::new();

    for length in 1..=8usize {
        for (shape, rotate) in [("rotated types", true), ("one repeated type", false)] {
            let case = format!("{length} step(s), {shape}");
            let names: Vec<String> =
                (0..length).map(|index| format!("step-{index}")).collect();
            let steps: Vec<Value> = names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let step_type = if rotate {
                        STEP_TYPES[(index + length) % STEP_TYPES.len()].0
                    } else {
                        STEP_TYPES[length % STEP_TYPES.len()].0
                    };
                    step_of(step_type, name)
                })
                .collect();

            let workflow_id = create(
                &emulator,
                &case,
                json!({"Steps": steps.clone(), "OnExceptionSteps": steps}),
            )
            .await;
            let workflow = describe(&emulator, &case, &workflow_id).await;

            for list in ["Steps", "OnExceptionSteps"] {
                let described = step_names(&case, member_of(&workflow, list));
                assert_eq!(
                    described, names,
                    "case {case:?} ({list}): expected the created order {names:?}, got \
                     {described:?}"
                );
            }
        }
    }
}

/// One row of the "the caller named it" table.
struct StepRow {
    case: &'static str,
    /// The step as `CreateWorkflow` is given it.
    step: Value,
    /// The members the described step's detail structure must carry; a `null`
    /// expectation means the member must be absent.
    expected: Value,
}

/// RULE 2: a value the request names for a member that HAS a documented default is kept,
/// and a member with NO documented default stays absent.
///
/// One row per step type that carries `SourceFileLocation`, each naming the non-default
/// value; the two types that also carry `OverwriteExisting` name its non-default value
/// too; and the `CUSTOM` row drops `TimeoutSeconds`, which the model documents no default
/// for.
#[tokio::test]
async fn a_step_keeps_the_values_the_request_named_and_defaults_nothing_else() {
    let rows = vec![
        StepRow {
            case: "COPY names both defaulted members",
            step: json!({"Type": "COPY", "CopyStepDetails": {
                "Name": "copy-named",
                "DestinationFileLocation": {"S3FileLocation": {"Bucket": "step-target", "Key": "out/"}},
                "SourceFileLocation": ORIGINAL_SOURCE,
                "OverwriteExisting": OVERWRITE_TRUE,
            }}),
            expected: json!({
                "Name": "copy-named",
                "SourceFileLocation": ORIGINAL_SOURCE,
                "OverwriteExisting": OVERWRITE_TRUE,
            }),
        },
        StepRow {
            case: "DECRYPT names both defaulted members",
            step: json!({"Type": "DECRYPT", "DecryptStepDetails": {
                "Name": "decrypt-named",
                "Type": "PGP",
                "DestinationFileLocation": {"S3FileLocation": {"Bucket": "step-clear", "Key": "out/"}},
                "SourceFileLocation": ORIGINAL_SOURCE,
                "OverwriteExisting": OVERWRITE_TRUE,
            }}),
            expected: json!({
                "Name": "decrypt-named",
                "SourceFileLocation": ORIGINAL_SOURCE,
                "OverwriteExisting": OVERWRITE_TRUE,
                "Type": "PGP",
            }),
        },
        StepRow {
            case: "CUSTOM names SourceFileLocation and no TimeoutSeconds",
            step: json!({"Type": "CUSTOM", "CustomStepDetails": {
                "Name": "custom-named",
                "Target": LAMBDA,
                "SourceFileLocation": ORIGINAL_SOURCE,
            }}),
            expected: json!({
                "Name": "custom-named",
                "SourceFileLocation": ORIGINAL_SOURCE,
                "Target": LAMBDA,
                // No documented default, so an absent one is reported absent rather
                // than invented.
                "TimeoutSeconds": null,
            }),
        },
        StepRow {
            case: "DELETE names SourceFileLocation",
            step: json!({"Type": "DELETE", "DeleteStepDetails": {
                "Name": "delete-named",
                "SourceFileLocation": ORIGINAL_SOURCE,
            }}),
            expected: json!({
                "Name": "delete-named",
                "SourceFileLocation": ORIGINAL_SOURCE,
            }),
        },
        StepRow {
            case: "TAG names SourceFileLocation",
            step: json!({"Type": "TAG", "TagStepDetails": {
                "Name": "tag-named",
                "Tags": [{"Key": "scanned", "Value": "yes"}],
                "SourceFileLocation": ORIGINAL_SOURCE,
            }}),
            expected: json!({
                "Name": "tag-named",
                "SourceFileLocation": ORIGINAL_SOURCE,
                "Tags": [{"Key": "scanned", "Value": "yes"}],
            }),
        },
    ];

    let emulator = Emulator::new();
    for row in rows {
        let step_type = member_of(&row.step, "Type")
            .as_str()
            .expect("a row's step carries a Type")
            .to_string();

        // Sent as a workflow step AND as an exception step: one shape, one rule.
        let workflow_id = create(
            &emulator,
            row.case,
            json!({
                "Steps": [row.step.clone()],
                "OnExceptionSteps": [row.step.clone()],
            }),
        )
        .await;
        let workflow = describe(&emulator, row.case, &workflow_id).await;

        for list in ["Steps", "OnExceptionSteps"] {
            let steps = member_of(&workflow, list);
            let described = steps
                .as_array()
                .and_then(|steps| steps.first())
                .unwrap_or_else(|| panic!("case {:?}: {list} holds no step: {steps:?}", row.case));
            assert_eq!(
                member_of(described, "Type").as_str(),
                Some(step_type.as_str()),
                "case {:?} ({list}): expected Type {step_type:?}, got {described:?}",
                row.case
            );
            assert_members(
                &format!("{} ({list})", row.case),
                member_of(described, details_member(&step_type)),
                &row.expected,
            );
        }
    }
}

/// RULE 3: every create mints a `WorkflowId` of the modelled shape, no two creates mint
/// the same one, and the ARN carries the region the caller signed for.
///
/// The rows differ along the signed region and along what the workflow holds — one step,
/// several steps with metadata, and the empty step list `WorkflowSteps`' `len 0..8`
/// allows.
#[tokio::test]
async fn created_workflow_ids_and_arns_are_modelled_and_unique() {
    let cases: [(&str, &str, Value); 3] = [
        (
            "us-east-1, one step",
            "us-east-1",
            json!({"Steps": [step_of("DELETE", "only")]}),
        ),
        (
            "eu-west-1, described and tagged",
            "eu-west-1",
            json!({
                "Description": "nightly ingest",
                "Steps": [step_of("TAG", "tag"), step_of("DELETE", "rm")],
                "Tags": [{"Key": "team", "Value": "data"}],
            }),
        ),
        (
            "ap-southeast-2, no steps at all",
            "ap-southeast-2",
            json!({"Steps": []}),
        ),
    ];

    let emulator = Emulator::new();
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for (case, region, request) in cases {
        let answer = emulator.call_in(region, "CreateWorkflow", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let workflow_id = answer
            .get("WorkflowId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no WorkflowId in {}", answer.body))
            .to_string();
        // `WorkflowId` is modelled `len 19..19`, pattern `^w-([a-z0-9]{17})$`.
        assert!(
            workflow_id.len() == 19
                && workflow_id.starts_with("w-")
                && workflow_id[2..]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
            "case {case:?}: expected a WorkflowId matching ^w-([a-z0-9]{{17}})$, got \
             {workflow_id:?}"
        );
        if let Some(previous) = seen.insert(workflow_id.clone(), case) {
            panic!("case {case:?}: WorkflowId {workflow_id:?} was already minted for {previous:?}");
        }

        let workflow = describe(&emulator, case, &workflow_id).await;
        assert_members(
            case,
            &workflow,
            &json!({
                "WorkflowId": &workflow_id,
                "Arn": format!("arn:aws:transfer:{region}:{ACCOUNT}:workflow/{workflow_id}"),
            }),
        );
    }
}
