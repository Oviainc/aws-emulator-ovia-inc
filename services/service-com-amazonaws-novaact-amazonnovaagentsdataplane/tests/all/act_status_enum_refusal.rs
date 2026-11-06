// SPDX-License-Identifier: CC0-1.0

//! How a value outside `ActStatus`'s value set is refused, and in which ORDER that set is
//! listed back.
//!
//! The rule under test: a status value the model's enum does not declare is refused with
//! HTTP 400 `ValidationException` whose message is the counted envelope over one clause
//! naming the member path and the value set the model declares — every value of it, in the
//! order the model declares them, and no echo of what the caller sent. The order is read
//! off the model, so two requests that break the same value set are refused with the very
//! same sentence.
//!
//! The table varies what the rule generalises over: the form of the rejected value (empty,
//! a declared value in the wrong case, a value belonging to ANOTHER enum of this model, a
//! prefix of a declared value, one with surrounding whitespace) and how often the same
//! request is repeated.

use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::ops::types::ACT_STATUS_VALUES;

use crate::workflow_lifecycle::{text, Emulator};

/// `ActStatus`' values in the order the MODEL declares them
/// (`smithy_query(query="shape",shape="ActStatus")` reports
/// `RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT`),
/// which is the order a clause listing the set states them in. The crate's own
/// `ACT_STATUS_VALUES` holds the same values sorted, so the two are cross-checked below
/// rather than one being derived from the other.
const DECLARED_ACT_STATUSES: [&str; 6] = [
    "RUNNING",
    "PENDING_CLIENT_ACTION",
    "PENDING_HUMAN_ACTION",
    "SUCCEEDED",
    "FAILED",
    "TIMED_OUT",
];

/// The definition the act under test lives under.
const DEFINITION: &str = "act-status-enum";

/// Where the act under test sits, so every row is refused on its status and not on a
/// missing parent.
struct Placed {
    run_id: String,
    session_id: String,
    act_id: String,
}

/// A definition -> run -> session -> act chain, every link stored.
async fn placed_act(emulator: &Emulator) -> Placed {
    let (status, body) = emulator.create_definition(DEFINITION).await;
    assert_eq!(
        status, 201,
        "the definition must be created, got {status} {body}"
    );
    let (status, body) = emulator.create_run(DEFINITION).await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");
    let (status, body) = emulator.create_session(DEFINITION, &run_id).await;
    assert_eq!(
        status, 201,
        "the session must be created, got {status} {body}"
    );
    let session_id = text(&body, "sessionId");
    let (status, body) = emulator
        .create_act(DEFINITION, &run_id, &session_id, "hold a status")
        .await;
    assert_eq!(status, 201, "the act must be created, got {status} {body}");
    let act_id = text(&body, "actId");
    Placed {
        run_id,
        session_id,
        act_id,
    }
}

/// One `UpdateAct` naming `status`.
async fn update(emulator: &Emulator, placed: &Placed, status: &str) -> (u16, Value) {
    emulator
        .call(
            "PUT",
            &format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{}/sessions/{}/acts/{}",
                placed.run_id, placed.session_id, placed.act_id
            ),
            Some(json!({ "status": status })),
        )
        .await
}

#[tokio::test]
async fn a_status_outside_the_value_set_is_refused_with_the_set_as_the_model_declares_it() {
    let emulator = Emulator::new("us-east-1");
    let placed = placed_act(&emulator).await;

    // The two lists name the same statuses, so the order below is the model's and not a
    // second value set written out beside it.
    let mut sorted = DECLARED_ACT_STATUSES;
    sorted.sort_unstable();
    assert_eq!(
        sorted.as_slice(),
        ACT_STATUS_VALUES,
        "the declared order must list exactly the statuses the model declares"
    );

    // The one sentence every row must read, built from the model's own value list.
    let expected = format!(
        "1 validation error detected: Value at 'status' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        DECLARED_ACT_STATUSES.join(", ")
    );

    let rows = [
        "",
        "succeeded",
        // `WorkflowRunStatus` declares `DELETING` and `WorkflowDefinitionStatus` `ACTIVE`;
        // `ActStatus` declares neither.
        "DELETING",
        "ACTIVE",
        "PENDING",
        "SUCCEEDED ",
        " RUNNING",
    ];
    for value in rows {
        // Repeated, because a sentence that listed the set in the iteration order of a
        // hashed container could differ between two refusals of the same request.
        for attempt in 1..=2 {
            let (status, answer) = update(&emulator, &placed, value).await;
            assert_eq!(
                status, 400,
                "case {value:?} attempt {attempt}: expected HTTP 400, got {status} {answer}"
            );
            assert_eq!(
                answer["__type"], "ValidationException",
                "case {value:?} attempt {attempt}: expected ValidationException, got {answer}"
            );
            let message = text(&answer, "message");
            assert_eq!(
                message, expected,
                "case {value:?} attempt {attempt}: expected message {expected:?}, \
                 got {message:?}"
            );
        }
    }

    // The act kept the status it was created with: a refused update writes nothing.
    let (status, body) = emulator
        .call(
            "POST",
            &format!(
                "/workflow-definitions/{DEFINITION}/acts?sessionId={}",
                placed.session_id
            ),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200, "the listing must answer, got {status} {body}");
    let listed = body["actSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("actSummaries must be a list in {body}"))
        .clone();
    assert_eq!(listed.len(), 1, "one act is stored, got {body}");
    assert_eq!(
        listed[0]["status"], "RUNNING",
        "the act must still be RUNNING, got {body}"
    );
}
