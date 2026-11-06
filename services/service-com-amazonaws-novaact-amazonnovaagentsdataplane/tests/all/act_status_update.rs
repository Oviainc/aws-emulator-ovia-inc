// SPDX-License-Identifier: CC0-1.0

//! `UpdateAct`: the status and error information of one STORED act, written through and
//! read back.
//!
//! The rule these cases assert is not "this one request answers these bytes": the update
//! resolves the act by walking the chain the path names (definition -> run -> session ->
//! act) and refusing the first link the store does not hold with 404, moves the act to
//! whichever status the model's enum declares, keeps `endedAt` in step with that status
//! (reported exactly when the act has reached a status that finishes it, absent otherwise),
//! records the error information the request carries, and refuses with 400 `InvalidStatus`
//! a request that would move an act out of a status it has already finished in — while
//! re-sending the status the act already holds changes nothing, as an `@idempotent`
//! operation's retry must.
//!
//! So the tables below vary what that rule generalises over: which of the six statuses
//! `ActStatus` declares is asked for, whether it is a finishing one, the status the act
//! starts from, how many updates run in sequence, which members of `ActError` a request
//! carries, and which link of the chain is missing. The recorded "running to succeeded"
//! case is one row among them.

use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::ops::types::ACT_STATUS_VALUES;

use crate::workflow_lifecycle::{text, Emulator};

/// The sentence this service states for an act whose current status does not admit the one
/// a request named, spelled out here rather than taken from the crate so a rewording of the
/// handler's constant cannot change both sides of the comparison at once.
const ACT_IN_UNEXPECTED_STATE: &str = "Act in unexpected state";

/// The definition each case builds its chain under.
const DEFINITION: &str = "act-status-update";

/// A well-formed `UuidString`
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) no record was ever
/// minted for.
const ABSENT_UUID: &str = "00000000-0000-4000-8000-000000000000";

/// The values of `ActStatus` that FINISH an act: it ran to the end, it failed, or it ran
/// out of time. An act in one of them reports an `endedAt`; the other three
/// (`RUNNING`, `PENDING_CLIENT_ACTION`, `PENDING_HUMAN_ACTION`) each describe an act still
/// under way.
const FINISHED_STATUSES: [&str; 3] = ["SUCCEEDED", "FAILED", "TIMED_OUT"];

/// Whether `status` is one the act has finished in.
fn finishes(status: &str) -> bool {
    FINISHED_STATUSES.contains(&status)
}

/// Where in the chain the acts of one case live.
#[derive(Debug, Clone)]
struct Chain {
    definition: String,
    run_id: String,
    session_id: String,
}

/// A fresh definition -> run -> session chain to create acts under.
async fn chain(emulator: &Emulator, definition: &str) -> Chain {
    let (status, body) = emulator.create_definition(definition).await;
    assert_eq!(
        status, 201,
        "the definition must be created, got {status} {body}"
    );
    let (status, body) = emulator.create_run(definition).await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");
    let (status, body) = emulator.create_session(definition, &run_id).await;
    assert_eq!(
        status, 201,
        "the session must be created, got {status} {body}"
    );
    let session_id = text(&body, "sessionId");
    Chain {
        definition: definition.to_string(),
        run_id,
        session_id,
    }
}

/// One new act under `chain`, created `RUNNING` as `CreateAct` reports it.
async fn new_act(emulator: &Emulator, chain: &Chain, task: &str) -> String {
    let (status, body) = emulator
        .create_act(&chain.definition, &chain.run_id, &chain.session_id, task)
        .await;
    assert_eq!(status, 201, "the act must be created, got {status} {body}");
    assert_eq!(body["status"], "RUNNING", "a new act is RUNNING, got {body}");
    text(&body, "actId")
}

/// The `UpdateAct` path: every identifier is an `@httpLabel`
/// (`smithy_query(query="operation",operation="UpdateAct")`).
fn update_path(definition: &str, run_id: &str, session_id: &str, act_id: &str) -> String {
    format!(
        "/workflow-definitions/{definition}/workflow-runs/{run_id}/sessions/{session_id}/acts/{act_id}"
    )
}

/// One `UpdateAct` call carrying `body`.
async fn update_act(emulator: &Emulator, chain: &Chain, act_id: &str, body: Value) -> (u16, Value) {
    emulator
        .call(
            "PUT",
            &update_path(&chain.definition, &chain.run_id, &chain.session_id, act_id),
            Some(body),
        )
        .await
}

/// One `UpdateAct` call that only moves the act's status.
async fn move_to(emulator: &Emulator, chain: &Chain, act_id: &str, status: &str) -> (u16, Value) {
    update_act(emulator, chain, act_id, json!({ "status": status })).await
}

/// The stored act read back through `ListActs` — the implemented reader of an act — so
/// nothing below can pass on what the update itself answered.
async fn summary(emulator: &Emulator, chain: &Chain, act_id: &str) -> Value {
    let (status, body) = emulator
        .call(
            "POST",
            &format!(
                "/workflow-definitions/{}/acts?workflowRunId={}&sessionId={}",
                chain.definition, chain.run_id, chain.session_id
            ),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200, "the listing must answer, got {status} {body}");
    body["actSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("actSummaries must be a list in {body}"))
        .iter()
        .find(|summary| summary["actId"] == act_id)
        .unwrap_or_else(|| panic!("act {act_id} must be listed in {body}"))
        .clone()
}

/// The error information one act holds, read straight out of the store: `ActError` is
/// reported by no operation this model declares.
fn stored_error(
    emulator: &Emulator,
    chain: &Chain,
    act_id: &str,
    label: &str,
) -> Option<(String, Option<String>)> {
    let store = emulator.store();
    let state = store.lock();
    let act = state
        .definition(&chain.definition)
        .and_then(|definition| definition.run(&chain.run_id))
        .and_then(|run| run.session(&chain.session_id))
        .unwrap_or_else(|missing| panic!("case {label:?}: the chain must resolve: {missing:?}"))
        .acts
        .get(act_id)
        .unwrap_or_else(|| panic!("case {label:?}: the act must be stored"));
    act.error
        .as_ref()
        .map(|error| (error.message.clone(), error.r#type.clone()))
}

/// Every status the model's enum declares is stored, and `endedAt` is reported exactly for
/// an act that has finished.
#[tokio::test]
async fn every_modeled_status_is_stored_and_ended_at_follows_it() {
    let emulator = Emulator::new("us-east-1");
    let chain = chain(&emulator, DEFINITION).await;
    assert_eq!(
        ACT_STATUS_VALUES.len(),
        6,
        "ActStatus declares six values, got {ACT_STATUS_VALUES:?}"
    );

    for requested in ACT_STATUS_VALUES {
        let act_id = new_act(&emulator, &chain, &format!("become {requested}")).await;
        let (status, answer) = move_to(&emulator, &chain, &act_id, requested).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: expected HTTP 200, got {status} {answer}"
        );

        let stored = summary(&emulator, &chain, &act_id).await;
        assert_eq!(
            stored["status"], *requested,
            "case {requested:?}: expected the listing to report {requested:?}, got {stored}"
        );
        assert!(
            !stored["startedAt"].is_null(),
            "case {requested:?}: startedAt must stay reported, got {stored}"
        );
        if finishes(requested) {
            assert!(
                !stored["endedAt"].is_null(),
                "case {requested:?}: a finished act reports endedAt, got {stored}"
            );
        } else {
            assert!(
                stored["endedAt"].is_null(),
                "case {requested:?}: an unfinished act reports no endedAt, got {stored}"
            );
        }
    }
}

/// An act moves freely between the statuses that leave it under way, and never reports an
/// `endedAt` while it does.
#[tokio::test]
async fn an_act_still_under_way_moves_between_unfinished_statuses() {
    let emulator = Emulator::new("eu-west-2");
    let chain = chain(&emulator, DEFINITION).await;
    let act_id = new_act(&emulator, &chain, "wait on the client, then the human").await;

    // One sequence per row, so the rule is asserted over an order of transitions rather
    // than over a single step.
    let sequences: [&[&str]; 3] = [
        &["PENDING_CLIENT_ACTION", "RUNNING"],
        &["PENDING_HUMAN_ACTION", "PENDING_CLIENT_ACTION", "RUNNING"],
        &["RUNNING", "RUNNING", "PENDING_HUMAN_ACTION"],
    ];
    for sequence in sequences {
        for requested in sequence {
            let (status, answer) = move_to(&emulator, &chain, &act_id, requested).await;
            assert_eq!(
                status, 200,
                "case {sequence:?} at {requested:?}: expected HTTP 200, got {status} {answer}"
            );
            let stored = summary(&emulator, &chain, &act_id).await;
            assert_eq!(
                stored["status"], *requested,
                "case {sequence:?} at {requested:?}: expected {requested:?}, got {stored}"
            );
            assert!(
                stored["endedAt"].is_null(),
                "case {sequence:?} at {requested:?}: an unfinished act reports no endedAt, \
                 got {stored}"
            );
        }
    }
}

/// An act that has finished keeps the status and the instant it finished with: a request
/// naming any OTHER status names a status its current one does not admit.
#[tokio::test]
async fn a_finished_act_refuses_a_move_to_another_status() {
    let emulator = Emulator::new("us-east-1");
    let chain = chain(&emulator, DEFINITION).await;

    for finished in FINISHED_STATUSES {
        for requested in ACT_STATUS_VALUES.iter().filter(|value| **value != finished) {
            let label = format!("{finished} then {requested}");
            let act_id = new_act(&emulator, &chain, &label).await;
            let (status, answer) = move_to(&emulator, &chain, &act_id, finished).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the act must finish first, got {status} {answer}"
            );
            let finished_summary = summary(&emulator, &chain, &act_id).await;

            let (status, answer) = move_to(&emulator, &chain, &act_id, requested).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400, got {status} {answer}"
            );
            assert_eq!(
                answer["__type"], "ValidationException",
                "case {label:?}: expected ValidationException, got {answer}"
            );
            // `ValidationExceptionReason` declares `InvalidStatus`
            // (`smithy_query(query="shape",shape="ValidationExceptionReason")`), which is
            // what a status the act's current one does not admit is.
            assert_eq!(
                answer["reason"], "InvalidStatus",
                "case {label:?}: expected reason \"InvalidStatus\", got {answer}"
            );
            let message = text(&answer, "message");
            assert_eq!(
                message, ACT_IN_UNEXPECTED_STATE,
                "case {label:?}: expected message {ACT_IN_UNEXPECTED_STATE:?}, got {message:?}"
            );

            // The refusal left the act exactly as it was, instants included.
            let after = summary(&emulator, &chain, &act_id).await;
            assert_eq!(
                after, finished_summary,
                "case {label:?}: the refused update must change nothing, got {after}"
            );
        }
    }
}

/// Re-sending the status an act already holds is the retry this `@idempotent` operation
/// promises: it succeeds and moves neither the status nor the instant the act finished.
#[tokio::test]
async fn repeating_the_status_an_act_holds_changes_nothing() {
    let emulator = Emulator::new("ap-southeast-1");
    let chain = chain(&emulator, DEFINITION).await;

    for requested in ACT_STATUS_VALUES {
        let act_id = new_act(&emulator, &chain, &format!("repeat {requested}")).await;
        let (status, answer) = move_to(&emulator, &chain, &act_id, requested).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: the first update must be accepted, got {status} {answer}"
        );
        let first = summary(&emulator, &chain, &act_id).await;

        for attempt in 1..=2 {
            let (status, answer) = move_to(&emulator, &chain, &act_id, requested).await;
            assert_eq!(
                status, 200,
                "case {requested:?} retry {attempt}: expected HTTP 200, got {status} {answer}"
            );
            let again = summary(&emulator, &chain, &act_id).await;
            assert_eq!(
                again, first,
                "case {requested:?} retry {attempt}: the act must be unchanged, got {again}"
            );
        }
    }
}

/// The error information a request carries is recorded on the act, and an update that
/// carries none leaves what the act already holds.
#[tokio::test]
async fn error_information_is_recorded_on_the_act() {
    let emulator = Emulator::new("us-east-1");
    let chain = chain(&emulator, DEFINITION).await;

    // Rows differ in which members of `ActError` are present and in the length of the
    // message: `ActError$message` is len 1..10000 and `type` len 1..100
    // (`smithy_query(query="shape",shape="ActError")`), so one row sits on the message's
    // upper bound.
    let rows: [(&str, String, Option<&str>); 3] = [
        (
            "message only",
            "the page never loaded".to_string(),
            None,
        ),
        (
            "message and type",
            "x".to_string(),
            Some("ToolInvocationFailed"),
        ),
        ("a message at its maximum length", "e".repeat(10_000), Some("T")),
    ];

    for (label, message, kind) in rows {
        let act_id = new_act(&emulator, &chain, label).await;
        let error = match kind {
            Some(kind) => json!({ "message": message, "type": kind }),
            None => json!({ "message": message }),
        };
        let (status, answer) = update_act(
            &emulator,
            &chain,
            &act_id,
            json!({ "status": "FAILED", "error": error }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} {answer}"
        );
        let recorded = stored_error(&emulator, &chain, &act_id, label)
            .unwrap_or_else(|| panic!("case {label:?}: the error must be recorded"));
        assert_eq!(
            recorded,
            (message.clone(), kind.map(str::to_string)),
            "case {label:?}: expected the submitted error, got {recorded:?}"
        );

        // The same status again, carrying no error: the recorded error stays.
        let (status, answer) = move_to(&emulator, &chain, &act_id, "FAILED").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the retry must be accepted, got {status} {answer}"
        );
        let after = stored_error(&emulator, &chain, &act_id, label);
        assert_eq!(
            after,
            Some((message, kind.map(str::to_string))),
            "case {label:?}: an update carrying no error must leave the recorded one, \
             got {after:?}"
        );
    }
}

/// Each link of the chain is resolved before the act is written: the first one the store
/// does not hold is reported as the missing resource, by its ARN.
#[tokio::test]
async fn an_update_naming_a_resource_the_store_does_not_hold_is_a_404() {
    let region = "us-west-2";
    let emulator = Emulator::new(region);
    let chain = chain(&emulator, DEFINITION).await;
    let act_id = new_act(&emulator, &chain, "present").await;

    let rows: [(&str, String, &str, String); 4] = [
        (
            "an absent definition",
            update_path("absent", &chain.run_id, &chain.session_id, &act_id),
            "workflow-definition",
            "workflow-definition/absent".to_string(),
        ),
        (
            "an absent run under a present definition",
            update_path(DEFINITION, ABSENT_UUID, &chain.session_id, &act_id),
            "workflow-run",
            format!("workflow-definition/{DEFINITION}/workflow-run/{ABSENT_UUID}"),
        ),
        (
            "an absent session under a present run",
            update_path(DEFINITION, &chain.run_id, ABSENT_UUID, &act_id),
            "session",
            format!(
                "workflow-definition/{DEFINITION}/workflow-run/{}/session/{ABSENT_UUID}",
                chain.run_id
            ),
        ),
        (
            "an absent act under a present session",
            update_path(DEFINITION, &chain.run_id, &chain.session_id, ABSENT_UUID),
            "act",
            format!(
                "workflow-definition/{DEFINITION}/workflow-run/{}/session/{}/act/{ABSENT_UUID}",
                chain.run_id, chain.session_id
            ),
        ),
    ];

    for (label, path, resource_type, resource_path) in rows {
        let (status, answer) = emulator
            .call("PUT", &path, Some(json!({ "status": "SUCCEEDED" })))
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404, got {status} {answer}"
        );
        assert_eq!(
            answer["__type"], "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer}"
        );
        assert_eq!(
            answer["resourceType"], resource_type,
            "case {label:?}: expected resourceType {resource_type:?}, got {answer}"
        );
        let expected = format!(
            "arn:aws:nova-act:{region}:{}:{resource_path}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            answer["resourceId"], expected,
            "case {label:?}: expected resourceId {expected:?}, got {answer}"
        );
    }

    // The act the rows above named is still RUNNING: nothing was written down a chain that
    // did not resolve.
    let stored = summary(&emulator, &chain, &act_id).await;
    assert_eq!(
        stored["status"], "RUNNING",
        "the present act must be untouched, got {stored}"
    );
}
