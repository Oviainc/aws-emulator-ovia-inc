// SPDX-License-Identifier: CC0-1.0

//! `PutActionRevision`, end to end over the wire the service speaks, against the
//! shared store every other operation of this crate reads.
//!
//! The rule under test: a revision reported for an action of a stage of a stored
//! pipeline is NEW exactly when it is not the `revisionId` that action already
//! carries; a new revision starts one pipeline execution carrying that revision, and
//! a revision that is not new starts nothing and reports the execution the pipeline
//! is already on. The tables below vary what the rule generalises over — the
//! revision's value and length, which action of the stage it is reported for, the
//! pipeline's execution mode, how many revisions are reported and in what order —
//! so no row is the case the recording happened to use.

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{declaration, Emulator, ROLE_ARN};

/// `PipelineExecutionId`'s modelled pattern, as
/// `smithy_query(query="operation",operation="PutActionRevision")` reports it for
/// `PutActionRevisionOutput$pipelineExecutionId`:
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
fn is_execution_id(value: &Value) -> bool {
    let Some(id) = value.as_str() else {
        return false;
    };
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(length, group)| {
                group.len() == *length
                    // The pattern's hex digits are lowercase.
                    && group
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            })
}

/// An `ActionRevision`: `revisionId`, `revisionChangeId` and `created` are all
/// `@required` (`smithy_query(query="shape",shape="ActionRevision")`).
fn revision(revision_id: &str, revision_change_id: &str) -> Value {
    json!({
        "revisionId": revision_id,
        "revisionChangeId": revision_change_id,
        // Epoch seconds: the awsJson1_1 wire form of a `Timestamp` member.
        "created": 1_760_000_000_i64,
    })
}

/// A pipeline whose first stage carries TWO source actions, so a revision can be
/// reported for either of them, plus a build stage that carries none.
fn two_source_actions(name: &str, execution_mode: Option<&str>) -> Value {
    let mut declaration = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [
            {
                "name": "Source",
                "actions": [
                    source_action("SrcA", "a.zip"),
                    source_action("SrcB", "b.zip"),
                ],
            },
            {
                "name": "Build",
                "actions": [{
                    "name": "Build1",
                    "actionTypeId": {
                        "category": "Build",
                        "owner": "AWS",
                        "provider": "CodeBuild",
                        "version": "1",
                    },
                    "configuration": {"ProjectName": "project"},
                    "inputArtifacts": [{"name": "SrcAOut"}],
                }],
            },
        ],
    });
    if let Some(execution_mode) = execution_mode {
        declaration["executionMode"] = json!(execution_mode);
    }
    declaration
}

/// One source action of the fixture pipeline.
///
/// `Custom`-owned, so an execution that reaches it waits for the job worker that
/// reports its outcome rather than being carried on by the clock: the executions this
/// suite counts against the concurrency bound stay non-terminal for as long as the
/// bound is being asserted.
fn source_action(name: &str, key: &str) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": "Source",
            "owner": "Custom",
            "provider": "MySource",
            "version": "1",
        },
        "configuration": {"S3Bucket": "my-bucket", "S3ObjectKey": key},
        "outputArtifacts": [{"name": format!("{name}Out")}],
    })
}

/// The executions a pipeline has, newest first, as `ListPipelineExecutions` reports
/// them.
async fn executions(emulator: &Emulator, pipeline: &str) -> Vec<Value> {
    let listed = emulator
        .ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    listed["pipelineExecutionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// A revision no action carries yet is new, is recorded, and starts exactly one
/// execution that carries it; reporting the SAME revision again is not new and starts
/// nothing.
#[tokio::test]
async fn a_new_revision_is_recorded_and_starts_one_execution() {
    // (case, action, revisionId, revisionChangeId). The bounds are the modelled ones:
    // `Revision` is len 1..1500 and `RevisionChangeIdentifier` len 1..100
    // (`smithy_query(query="shape",shape="ActionRevision")`).
    let long_revision = "r".repeat(1500);
    let long_change_id = "c".repeat(100);
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "minimum-length revision id",
            "SrcA",
            "1",
            "change-1",
        ),
        (
            "a commit sha",
            "SrcA",
            "0b9c1f2e5a4d3c8b7a6f5e4d3c2b1a0998877665",
            "deployment-7",
        ),
        (
            "maximum-length revision id",
            "SrcA",
            long_revision.as_str(),
            "change-long",
        ),
        (
            "maximum-length revision change id",
            "SrcA",
            "rev-with-max-change-id",
            long_change_id.as_str(),
        ),
        (
            "an object version with punctuation",
            "SrcA",
            "3HL4kqtJlcpXroDTDmJ+rmSpXd3dIbrHY+MTRCxf3vjVBH40Nr8X8gdRQBpUMLUo",
            "2026-10-22T17:35:20Z",
        ),
        (
            "the other source action of the same stage",
            "SrcB",
            "rev-for-b",
            "change-b",
        ),
    ];

    for (case, action, revision_id, revision_change_id) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": two_source_actions("demo", None)}),
            )
            .await;

        let body = json!({
            "pipelineName": "demo",
            "stageName": "Source",
            "actionName": action,
            "actionRevision": revision(revision_id, revision_change_id),
        });
        let put = emulator.ok("PutActionRevision", &body).await;
        assert_eq!(
            put["newRevision"],
            json!(true),
            "case {case:?}: a revision the action does not carry is new, got {put}"
        );
        assert!(
            is_execution_id(&put["pipelineExecutionId"]),
            "case {case:?}: a new revision reports the execution it started, got {put}"
        );
        let execution_id = put["pipelineExecutionId"].as_str().expect("an id").to_string();

        // The execution is in the shared store, carries the reported revision, and
        // records what started it.
        let summaries = executions(&emulator, "demo").await;
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: one new revision starts one execution, got {summaries:?}"
        );
        assert_eq!(
            summaries[0]["pipelineExecutionId"],
            json!(execution_id),
            "case {case:?}: the listed execution is the one the put minted, got {summaries:?}"
        );
        assert_eq!(
            summaries[0]["sourceRevisions"],
            json!([{"actionName": action, "revisionId": revision_id}]),
            "case {case:?}: the execution carries the reported revision, got {summaries:?}"
        );
        assert_eq!(
            summaries[0]["trigger"]["triggerType"],
            json!("PutActionRevision"),
            "case {case:?}: the trigger records what started it, got {summaries:?}"
        );

        // And the implemented reader of an action's state reports it as that action's
        // `currentRevision`, while the actions no revision was reported for carry
        // none.
        let state = emulator.ok("GetPipelineState", &json!({"name": "demo"})).await;
        let action_states: Vec<&Value> = state["stageStates"]
            .as_array()
            .expect("stage states")
            .iter()
            .flat_map(|stage| {
                stage["actionStates"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
            })
            .collect();
        for reported in &action_states {
            let is_the_action = reported["actionName"] == json!(action);
            let current = reported.get("currentRevision");
            if is_the_action {
                let current = current.unwrap_or_else(|| {
                    panic!("case {case:?}: the action carries a revision, got {reported}")
                });
                assert_eq!(
                    (
                        current["revisionId"].clone(),
                        current["revisionChangeId"].clone()
                    ),
                    (json!(revision_id), json!(revision_change_id)),
                    "case {case:?}: GetPipelineState must report the revision as stored, \
                     got {current}"
                );
                assert!(
                    current["created"].is_number(),
                    "case {case:?}: the stored revision keeps its created instant, got {current}"
                );
            } else {
                assert!(
                    current.is_none(),
                    "case {case:?}: no revision was reported for this action, got {reported}"
                );
            }
        }

        let fetched = emulator
            .ok(
                "GetPipelineExecution",
                &json!({"pipelineName": "demo", "pipelineExecutionId": execution_id}),
            )
            .await;
        let fetched = &fetched["pipelineExecution"];
        assert_eq!(
            (
                fetched["status"].clone(),
                fetched["pipelineName"].clone(),
                fetched["pipelineVersion"].clone(),
                fetched["trigger"]["triggerType"].clone()
            ),
            (
                json!("InProgress"),
                json!("demo"),
                json!(1),
                json!("PutActionRevision")
            ),
            "case {case:?}: unexpected stored execution, got {fetched}"
        );

        // The same revision again: not new, nothing started, and the pipeline's
        // current workflow state is still that one execution.
        let again = emulator.ok("PutActionRevision", &body).await;
        assert_eq!(
            (again["newRevision"].clone(), again["pipelineExecutionId"].clone()),
            (json!(false), json!(execution_id)),
            "case {case:?}: a revision the action already carries is not new, got {again}"
        );
        let summaries = executions(&emulator, "demo").await;
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: a repeated revision starts no execution, got {summaries:?}"
        );
    }
}

/// Each change of the reported `revisionId` is a new revision and starts its own
/// execution, in the order the reports arrive — including going back to a revision
/// the action carried before, which is not the one it carries now.
#[tokio::test]
async fn every_change_of_revision_starts_its_own_execution() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreatePipeline",
            // PARALLEL mode: the pipeline holds more than one execution at a time, so
            // a run of reports is accepted rather than refused on the second.
            &json!({"pipeline": two_source_actions("parallel", Some("PARALLEL"))}),
        )
        .await;

    // (case, revisionId, expected newRevision)
    let reports: Vec<(&str, &str, bool)> = vec![
        ("first revision", "rev-1", true),
        ("the same revision again", "rev-1", false),
        ("a second revision", "rev-2", true),
        ("the same second revision again", "rev-2", false),
        ("back to the first revision", "rev-1", true),
    ];

    let mut started: Vec<String> = Vec::new();
    for (case, revision_id, expected_new) in reports {
        let put = emulator
            .ok(
                "PutActionRevision",
                &json!({
                    "pipelineName": "parallel",
                    "stageName": "Source",
                    "actionName": "SrcA",
                    "actionRevision": revision(revision_id, "change"),
                }),
            )
            .await;
        assert_eq!(
            put["newRevision"],
            json!(expected_new),
            "case {case:?}: unexpected newRevision, got {put}"
        );
        let id = put["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an execution id, got {put}"))
            .to_string();
        if expected_new {
            assert!(
                !started.contains(&id),
                "case {case:?}: a new revision starts its own execution, got {id} in {started:?}"
            );
            started.push(id);
        } else {
            assert_eq!(
                Some(&id),
                started.last(),
                "case {case:?}: a revision that is not new reports the current execution"
            );
        }
    }

    let summaries = executions(&emulator, "parallel").await;
    let listed: Vec<String> = summaries
        .iter()
        .map(|summary| summary["pipelineExecutionId"].as_str().unwrap_or_default().to_string())
        .collect();
    let newest_first: Vec<String> = started.iter().rev().cloned().collect();
    assert_eq!(
        listed, newest_first,
        "the store holds one execution per new revision, newest first, got {summaries:?}"
    );
}

/// A revision reported against a name the stored declaration does not carry is
/// refused with the error the model declares for that name, and nothing is recorded.
#[tokio::test]
async fn a_revision_for_an_unknown_name_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreatePipeline",
            &json!({"pipeline": two_source_actions("demo", None)}),
        )
        .await;

    // (case, pipelineName, stageName, actionName, expected error code)
    let cases: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "unknown pipeline",
            "missing",
            "Source",
            "SrcA",
            "PipelineNotFoundException",
        ),
        (
            "unknown stage",
            "demo",
            "Missing",
            "SrcA",
            "StageNotFoundException",
        ),
        (
            "unknown action",
            "demo",
            "Source",
            "Missing",
            "ActionNotFoundException",
        ),
        (
            "an action of another stage of the same pipeline",
            "demo",
            "Source",
            "Build1",
            "ActionNotFoundException",
        ),
        (
            "a stage name that is a pipeline name",
            "demo",
            "demo",
            "SrcA",
            "StageNotFoundException",
        ),
    ];

    for (case, pipeline_name, stage_name, action_name, expected) in cases {
        let (status, code) = emulator
            .expect_error(
                "PutActionRevision",
                &json!({
                    "pipelineName": pipeline_name,
                    "stageName": stage_name,
                    "actionName": action_name,
                    "actionRevision": revision("rev", "change"),
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: unexpected refusal"
        );
    }

    // No refused report started anything, and none of them was recorded: the first
    // accepted report of a revision is still new.
    let summaries = executions(&emulator, "demo").await;
    assert!(
        summaries.is_empty(),
        "a refused report starts no execution, got {summaries:?}"
    );
    let put = emulator
        .ok(
            "PutActionRevision",
            &json!({
                "pipelineName": "demo",
                "stageName": "Source",
                "actionName": "SrcA",
                "actionRevision": revision("rev", "change"),
            }),
        )
        .await;
    assert_eq!(
        put["newRevision"],
        json!(true),
        "a refused report records no revision, got {put}"
    );
}

/// A revision whose members break a modelled constraint is refused at the boundary
/// with `ValidationException` and reaches no handler: the bounds are
/// `Revision` len 1..1500 and `RevisionChangeIdentifier` len 1..100
/// (`smithy_query(query="shape",shape="ActionRevision")`).
#[tokio::test]
async fn a_revision_the_model_rejects_never_reaches_the_store() {
    // (case, actionRevision, accepted)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("revision id at its minimum length", revision("r", "c"), true),
        (
            "revision id at its maximum length",
            revision(&"r".repeat(1500), "c"),
            true,
        ),
        (
            "revision id one past its maximum length",
            revision(&"r".repeat(1501), "c"),
            false,
        ),
        ("an empty revision id", revision("", "c"), false),
        (
            "revision change id at its maximum length",
            revision("r-100", &"c".repeat(100)),
            true,
        ),
        (
            "revision change id one past its maximum length",
            revision("r-101", &"c".repeat(101)),
            false,
        ),
        ("an empty revision change id", revision("r-empty", ""), false),
        (
            "no revisionId at all",
            json!({"revisionChangeId": "c", "created": 1_760_000_000_i64}),
            false,
        ),
        (
            "no created at all",
            json!({"revisionId": "r", "revisionChangeId": "c"}),
            false,
        ),
    ];

    for (case, action_revision, accepted) in cases {
        // One pipeline per row, so an accepted row's execution cannot change what the
        // next row sees.
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": two_source_actions("demo", None)}),
            )
            .await;
        let body = json!({
            "pipelineName": "demo",
            "stageName": "Source",
            "actionName": "SrcA",
            "actionRevision": action_revision,
        });
        let (status, response) = emulator.call("PutActionRevision", &body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the revision satisfies every modelled constraint, got {response}"
            );
            continue;
        }
        assert_eq!(
            (status, response["__type"].as_str().unwrap_or_default()),
            (400, "ValidationException"),
            "case {case:?}: unexpected refusal, got {response}"
        );
        let summaries = executions(&emulator, "demo").await;
        assert!(
            summaries.is_empty(),
            "case {case:?}: a refused revision starts no execution, got {summaries:?}"
        );
    }
}

/// A pipeline holds only so many executions that have not completed: a new revision
/// that would start one past that bound is refused with
/// `ConcurrentPipelineExecutionsLimitExceededException`, whatever started the
/// executions already running, and the refusal records nothing.
#[tokio::test]
async fn a_new_revision_past_the_concurrency_bound_is_refused() {
    // The bounds are the service crate's own, read from `crate::state` so this test
    // cannot drift from them.
    let superseded_limit =
        service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_CONCURRENT_EXECUTIONS_SUPERSEDED;
    let parallel_limit =
        service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_CONCURRENT_EXECUTIONS_PARALLEL;
    assert!(
        superseded_limit < parallel_limit,
        "a mode that holds several executions must allow more than SUPERSEDED, got \
         {superseded_limit} and {parallel_limit}"
    );

    // (case, executionMode, the bound that mode allows)
    let cases: Vec<(&str, Option<&str>, usize)> = vec![
        ("the default mode", None, superseded_limit),
        ("SUPERSEDED", Some("SUPERSEDED"), superseded_limit),
        ("QUEUED", Some("QUEUED"), parallel_limit),
        ("PARALLEL", Some("PARALLEL"), parallel_limit),
    ];

    for (case, execution_mode, limit) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": two_source_actions("demo", execution_mode)}),
            )
            .await;

        for index in 0..limit {
            let put = emulator
                .ok(
                    "PutActionRevision",
                    &json!({
                        "pipelineName": "demo",
                        "stageName": "Source",
                        "actionName": "SrcA",
                        "actionRevision": revision(&format!("rev-{index}"), "change"),
                    }),
                )
                .await;
            assert_eq!(
                put["newRevision"],
                json!(true),
                "case {case:?}: revision {index} is inside the bound of {limit}, got {put}"
            );
        }

        let (status, code) = emulator
            .expect_error(
                "PutActionRevision",
                &json!({
                    "pipelineName": "demo",
                    "stageName": "Source",
                    "actionName": "SrcA",
                    "actionRevision": revision("one-too-many", "change"),
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConcurrentPipelineExecutionsLimitExceededException"),
            "case {case:?}: revision {} is past the bound of {limit}",
            limit + 1
        );

        // The refusal started nothing and recorded nothing: the action still carries
        // the last accepted revision.
        let summaries = executions(&emulator, "demo").await;
        assert_eq!(
            summaries.len(),
            limit,
            "case {case:?}: a refused revision starts no execution, got {} summaries",
            summaries.len()
        );
        let again = emulator
            .ok(
                "PutActionRevision",
                &json!({
                    "pipelineName": "demo",
                    "stageName": "Source",
                    "actionName": "SrcA",
                    "actionRevision": revision(&format!("rev-{}", limit - 1), "change"),
                }),
            )
            .await;
        assert_eq!(
            again["newRevision"],
            json!(false),
            "case {case:?}: the refused revision was not recorded, got {again}"
        );
    }
}

/// The bound counts every execution that has not completed, whichever call started
/// it: an execution `StartPipelineExecution` began occupies the same slot.
#[tokio::test]
async fn an_execution_started_by_another_call_occupies_the_bound() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration("demo")}))
        .await;
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": "demo"}))
        .await;
    let execution_id = started["pipelineExecutionId"]
        .as_str()
        .expect("an execution id")
        .to_string();

    let (status, code) = emulator
        .expect_error(
            "PutActionRevision",
            &json!({
                // `declaration` builds a one-stage pipeline whose source action is
                // named `Src`.
                "pipelineName": "demo",
                "stageName": "Source",
                "actionName": "Src",
                "actionRevision": revision("rev-1", "change"),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConcurrentPipelineExecutionsLimitExceededException"),
        "a SUPERSEDED pipeline already running an execution holds no second one"
    );

    let summaries = executions(&emulator, "demo").await;
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary["pipelineExecutionId"].clone())
            .collect::<Vec<Value>>(),
        vec![json!(execution_id)],
        "the refused revision started nothing, got {summaries:?}"
    );
}

/// Deleting the pipeline forgets the revisions its actions carried, so re-creating it
/// starts from no revision at all rather than from the deleted pipeline's.
#[tokio::test]
async fn a_deleted_pipelines_revisions_go_with_it() {
    let emulator = Emulator::new();
    let body = json!({
        "pipelineName": "demo",
        "stageName": "Source",
        "actionName": "SrcA",
        "actionRevision": revision("rev-1", "change"),
    });

    for round in 1..=2 {
        emulator
            .ok(
                "CreatePipeline",
                &json!({"pipeline": two_source_actions("demo", None)}),
            )
            .await;
        let put = emulator.ok("PutActionRevision", &body).await;
        assert_eq!(
            put["newRevision"],
            json!(true),
            "round {round}: a pipeline that carries no revision yet, got {put}"
        );
        emulator.ok("DeletePipeline", &json!({"name": "demo"})).await;
    }
}
