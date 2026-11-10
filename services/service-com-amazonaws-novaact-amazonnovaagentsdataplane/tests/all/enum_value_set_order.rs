// SPDX-License-Identifier: CC0-1.0

//! WHICH ORDER a refusal of a value outside an enumerated type lists that type's
//! admissible values in, over the real wire path: every call goes through the router's
//! `dispatch`, so the clause is read exactly as a client reads it.
//!
//! The rule: a value outside an enumerated type is refused HTTP 400 `ValidationException`
//! whose clause names the values that type admits, and those values are listed in the order
//! this front door walks its own table of them — a table keyed on each value's own text,
//! walked bucket by bucket (`service_common::validation_message::hashed_enum_order`) —
//! rather than in the order the model declares them.
//!
//! The order is derived from the values themselves, so it belongs to the enumerated TYPE:
//! one type is listed alike whichever member carries it, in whatever operation, and whatever
//! the request's shape. That is what the rows below walk — the enum, the member, the
//! operation, and the form of the rejected value — while the two guards at the end state the
//! derivation independently of any request.

use serde_json::{json, Value};

use service_common::validation_message::{enum_value_bucket, hashed_enum_order};

use crate::workflow_lifecycle::{text, Emulator};

/// Every enumerated type a REQUEST member of this model carries, as the values it declares,
/// in declaration order.
///
/// Source: `smithy_query(query="shape", shape="<Enum>")` for each of `ActStatus`
/// (`UpdateAct$status`), `WorkflowRunStatus` (`UpdateWorkflowRun$status`) and `SortOrder`
/// (the `sortOrder` of every listing).
const MODELLED_ENUMS: &[(&str, &[&str])] = &[
    (
        "ActStatus",
        &[
            "RUNNING",
            "PENDING_CLIENT_ACTION",
            "PENDING_HUMAN_ACTION",
            "SUCCEEDED",
            "FAILED",
            "TIMED_OUT",
        ],
    ),
    (
        "WorkflowRunStatus",
        &["RUNNING", "SUCCEEDED", "FAILED", "TIMED_OUT", "DELETING"],
    ),
    ("SortOrder", &["Ascending", "Descending"]),
];

/// The values `name` declares, read out of [`MODELLED_ENUMS`] so each value set is written
/// in ONE place: a row names the enum, never a second copy of its values.
fn declared(name: &str) -> &'static [&'static str] {
    MODELLED_ENUMS
        .iter()
        .find(|(enumerated, _)| *enumerated == name)
        .map(|(_, values)| *values)
        .unwrap_or_else(|| panic!("case {name:?}: this model declares no such enum"))
}

/// The definition every row is refused under.
const DEFINITION: &str = "enum-value-set-order";

/// The whole message a refused row must carry: the counted envelope over one clause naming
/// the member and the value set, with the set put through the SHARED derivation rather than
/// transcribed — so a service that grew a second copy of the order and drifted from it fails
/// here.
fn expected_message(member: &str, enumerated: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        hashed_enum_order(declared(enumerated)).join(", ")
    )
}

/// Where the act and the run under test sit, so every row is refused on its enum member and
/// not on a missing parent.
struct Placed {
    run_id: String,
    session_id: String,
    act_id: String,
}

/// A definition -> run -> session -> act chain, every link stored.
async fn placed(emulator: &Emulator) -> Placed {
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

/// One refused request: how to send it, the member its clause is reported at, and the enum
/// whose values that clause has to list.
struct Row {
    case: String,
    method: &'static str,
    path: String,
    body: Value,
    member: &'static str,
    enumerated: &'static str,
}

/// Every row: each request-carried enum of this model, through every operation that carries
/// it, over the forms a rejected value takes.
fn rows(placed: &Placed) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // `UpdateAct$status`, the recorded case's member. The forms walk an empty value, a
    // declared value recased, a value belonging to ANOTHER enum of this model, a prefix of a
    // declared value, and a declared value with surrounding whitespace — none of which may
    // reach the listing.
    let act = format!(
        "/workflow-definitions/{DEFINITION}/workflow-runs/{}/sessions/{}/acts/{}",
        placed.run_id, placed.session_id, placed.act_id
    );
    for rejected in [
        "",
        "succeeded",
        // `WorkflowRunStatus` declares `DELETING`; `ActStatus` does not.
        "DELETING",
        "PENDING",
        "SUCCEEDED ",
        " RUNNING",
    ] {
        rows.push(Row {
            case: format!("UpdateAct with status {rejected:?}"),
            method: "PUT",
            path: act.clone(),
            body: json!({ "status": rejected }),
            member: "status",
            enumerated: "ActStatus",
        });
    }

    // `UpdateWorkflowRun$status`: another operation, the same member NAME, a different enum
    // — so the listing follows the TYPE the member targets and not the path it sits at.
    let run = format!(
        "/workflow-definitions/{DEFINITION}/workflow-runs/{}",
        placed.run_id
    );
    for rejected in [
        "RUNNING_NOW",
        "running",
        // `ActStatus` declares this one; `WorkflowRunStatus` does not.
        "PENDING_CLIENT_ACTION",
    ] {
        rows.push(Row {
            case: format!("UpdateWorkflowRun with status {rejected:?}"),
            method: "PUT",
            path: run.clone(),
            body: json!({ "status": rejected }),
            member: "status",
            enumerated: "WorkflowRunStatus",
        });
    }

    // `sortOrder`, carried by every listing of this model: one enum, four operations.
    let listings: [(&str, String); 4] = [
        ("ListWorkflowDefinitions", "/workflow-definitions".to_string()),
        (
            "ListWorkflowRuns",
            format!("/workflow-definitions/{DEFINITION}/workflow-runs"),
        ),
        (
            "ListSessions",
            format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{}",
                placed.run_id
            ),
        ),
        (
            "ListActs",
            format!(
                "/workflow-definitions/{DEFINITION}/acts?sessionId={}",
                placed.session_id
            ),
        ),
    ];
    for (operation, path) in listings {
        for rejected in ["Sideways", "ascending", "DESCENDING"] {
            rows.push(Row {
                case: format!("{operation} with sortOrder {rejected:?}"),
                method: "POST",
                path: path.clone(),
                body: json!({ "sortOrder": rejected }),
                member: "sortOrder",
                enumerated: "SortOrder",
            });
        }
    }

    rows
}

#[tokio::test]
async fn an_enum_refusal_lists_the_values_in_the_order_the_front_door_states_them() {
    let emulator = Emulator::new("us-east-1");
    let placed = placed(&emulator).await;

    for row in rows(&placed) {
        let case = &row.case;
        // Repeated, because a listing read off a container walked in a per-process order
        // could differ between two refusals of the same request.
        for attempt in 1..=2 {
            let (status, answer) = emulator
                .call(row.method, &row.path, Some(row.body.clone()))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} attempt {attempt}: expected HTTP 400, got {status} {answer}"
            );
            assert_eq!(
                answer["__type"], "ValidationException",
                "case {case:?} attempt {attempt}: expected ValidationException, got {answer}"
            );
            let message = text(&answer, "message");
            // The expected clause is built from the enum's declared values alone, so this
            // equality also states that nothing of the rejected value reaches the message:
            // the requirement is the SET every value of the member must come from, not the
            // one value that was outside it.
            let expected = expected_message(row.member, row.enumerated);
            assert_eq!(
                message, expected,
                "case {case:?} attempt {attempt}: expected message {expected:?}, \
                 got {message:?}"
            );
        }
    }

    // A value inside each set is accepted by the same members, so the rows above are refused
    // for the enum and not for something every request of that shape fails.
    for (case, method, path, body, expected_status) in [
        (
            "ListWorkflowDefinitions with a declared sortOrder",
            "POST",
            "/workflow-definitions".to_string(),
            json!({ "sortOrder": "Descending" }),
            200,
        ),
        (
            "ListActs with a declared sortOrder",
            "POST",
            format!(
                "/workflow-definitions/{DEFINITION}/acts?sessionId={}",
                placed.session_id
            ),
            json!({ "sortOrder": "Ascending" }),
            200,
        ),
        (
            "UpdateAct with a declared status",
            "PUT",
            format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{}/sessions/{}/acts/{}",
                placed.run_id, placed.session_id, placed.act_id
            ),
            json!({ "status": "SUCCEEDED" }),
            200,
        ),
        // `UpdateWorkflowRun` is still scaffolded, so a status INSIDE its set is answered
        // 501 rather than 200 — which is itself the positive control for its rows above: a
        // declared value reaches the (unimplemented) handler, and only a value outside the
        // set is refused at the boundary.
        (
            "UpdateWorkflowRun with a declared status",
            "PUT",
            format!(
                "/workflow-definitions/{DEFINITION}/workflow-runs/{}",
                placed.run_id
            ),
            json!({ "status": "SUCCEEDED" }),
            501,
        ),
    ] {
        let (status, answer) = emulator.call(method, &path, Some(body)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answer}"
        );
    }
}

/// GUARD on the expectations above, which read their listing from the shared derivation: the
/// `ActStatus` set is listed `[SUCCEEDED, PENDING_CLIENT_ACTION, TIMED_OUT,
/// PENDING_HUMAN_ACTION, FAILED, RUNNING]`, which is the listing a recorded refusal of an
/// `UpdateAct$status` outside the set carries — and the model declares that set
/// `[RUNNING, PENDING_CLIENT_ACTION, PENDING_HUMAN_ACTION, SUCCEEDED, FAILED, TIMED_OUT]`,
/// so the listing is NOT the declaration.
#[test]
fn the_act_status_value_set_is_listed_as_the_recorded_refusal_lists_it() {
    const RECORDED: &[&str] = &[
        "SUCCEEDED",
        "PENDING_CLIENT_ACTION",
        "TIMED_OUT",
        "PENDING_HUMAN_ACTION",
        "FAILED",
        "RUNNING",
    ];

    // Whichever order the set is presented in — the model's, reversed, or rotated — the
    // listing is the one the front door states, because it is derived from the values.
    let model = declared("ActStatus");
    let reversed: Vec<&str> = model.iter().rev().copied().collect();
    let rotated: Vec<&str> = model[2..].iter().chain(&model[..2]).copied().collect();
    for (case, presented) in [
        ("the model's declaration order", model.to_vec()),
        ("reversed", reversed),
        ("rotated", rotated),
        ("the recorded listing itself", RECORDED.to_vec()),
    ] {
        let listed = hashed_enum_order(&presented);
        assert_eq!(
            listed, RECORDED,
            "case {case:?}: expected {RECORDED:?}, got {listed:?}"
        );
    }
    assert_ne!(
        hashed_enum_order(model),
        model.to_vec(),
        "case the declaration order: the listing must not be the model's declaration, got {:?}",
        model
    );
}

/// GUARD on the derivation itself, over EVERY enumerated type a request member of this model
/// carries: the listing holds exactly the values the type admits — none added, none dropped,
/// none repeated — and, for a type no two of whose values share a bucket of the table, it
/// does not depend on the order the values were presented in.
#[test]
fn every_modelled_value_set_is_listed_whole_and_independently_of_its_presentation() {
    for (name, values) in MODELLED_ENUMS {
        let listed = hashed_enum_order(values);
        let mut listed_sorted = listed.clone();
        listed_sorted.sort_unstable();
        let mut declared_sorted = values.to_vec();
        declared_sorted.sort_unstable();
        assert_eq!(
            listed_sorted, declared_sorted,
            "case {name:?}: the listing must hold exactly the declared values, got {listed:?}"
        );

        // Two values in one bucket are read in the order they were pushed onto it, so the
        // claim below is made only for a type whose values sit in buckets of their own.
        let mut buckets: Vec<u32> = values.iter().map(|value| enum_value_bucket(value)).collect();
        buckets.sort_unstable();
        if buckets.windows(2).any(|pair| pair[0] == pair[1]) {
            continue;
        }
        let reversed: Vec<&str> = values.iter().rev().copied().collect();
        let from_reversed = hashed_enum_order(&reversed);
        assert_eq!(
            from_reversed, listed,
            "case {name:?}: the listing must not depend on the presentation, got \
             {from_reversed:?} from the reversed set and {listed:?} from the declared one"
        );
    }
}
