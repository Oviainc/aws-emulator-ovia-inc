// SPDX-License-Identifier: CC0-1.0

//! `UpdateInvestigationState` and the lifecycle `GetInvestigation` reports.
//!
//! The rules under test:
//!
//! 1. `UpdateInvestigationState` stores the `State` it is given on the investigation it
//!    names ("an archived investigation indicates you have completed reviewing the
//!    investigation"), and every later read of that investigation reports it — for every
//!    value of the modelled `State` enum, in any order, including the state the
//!    investigation is already in. It changes no other investigation, and no other
//!    member of the one it changes.
//! 2. `Status` is "the status based on the completion status of the investigation": the
//!    non-terminal `RUNNING` while the run is outstanding, and the outcome of the run
//!    once the clock has made it due. No activity is ever ingested for an entity, so a
//!    run has nothing to assemble a report from and does not complete: the outcome is
//!    `FAILED`. Archiving an investigation does not touch it.
//! 3. An investigation the graph does not hold, and a graph this service does not hold,
//!    are each the modelled `ResourceNotFoundException` (404) with that surface's fixed
//!    sentence, and the write stores nothing.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_ID_LEN, INVESTIGATION_RUN_MILLIS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call, create_graph_ok, emulator, error_code, error_message, ACCOUNT, REGION,
};

/// The URIs of the investigation operations, from
/// `smithy_query(query="operation",operation="StartInvestigation,GetInvestigation,UpdateInvestigationState")`.
const START_URI: &str = "/investigations/startInvestigation";
const GET_URI: &str = "/investigations/getInvestigation";
const UPDATE_STATE_URI: &str = "/investigations/updateInvestigationState";

/// Every value the modelled `State` enum accepts
/// (`smithy_query(query="shape",shape="State")`).
const STATES: [&str; 2] = ["ACTIVE", "ARCHIVED"];

/// The state an investigation is in before anything archives it, from the same enum.
const STATE_ACTIVE: &str = "ACTIVE";

/// The state an archived investigation reports, from the same enum.
const STATE_ARCHIVED: &str = "ARCHIVED";

/// `StartInvestigation` over `entity_arn`, which must succeed; answers the id it minted.
async fn start_investigation(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    entity_arn: &str,
) -> String {
    let (status, started) = call(
        router,
        START_URI,
        &json!({
            "GraphArn": graph_arn,
            "EntityArn": entity_arn,
            "ScopeStartTime": "2024-05-01T08:00:00Z",
            "ScopeEndTime": "2024-05-01T12:00:00Z",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartInvestigation expected 200, got {status} {started:?}"
    );
    let id = started
        .get("InvestigationId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no InvestigationId in {started:?}"))
        .to_string();
    assert_eq!(
        id.len(),
        INVESTIGATION_ID_LEN,
        "case {case:?}: expected a modelled InvestigationId, got {id:?}"
    );
    id
}

/// `GetInvestigation`, which must succeed.
async fn get_investigation(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    investigation_id: &str,
) -> Value {
    let (status, got) = call(
        router,
        GET_URI,
        &json!({ "GraphArn": graph_arn, "InvestigationId": investigation_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetInvestigation expected 200, got {status} {got:?}"
    );
    got
}

/// `UpdateInvestigationState`, which must be accepted and answer an empty body.
async fn update_state(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    investigation_id: &str,
    state: &str,
) {
    let (status, answered) = call(
        router,
        UPDATE_STATE_URI,
        &json!({
            "GraphArn": graph_arn,
            "InvestigationId": investigation_id,
            "State": state,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateInvestigationState {state} expected 200, got {status} {answered:?}"
    );
    assert!(
        answered.is_null() || answered == json!({}),
        "case {case:?}: expected an empty body, got {answered:?}"
    );
}

/// The `State` one read reported.
fn state_of(case: &str, got: &Value) -> String {
    got.get("State")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: read carried no State: {got:?}"))
        .to_string()
}

#[tokio::test]
async fn an_update_stores_the_state_every_later_read_of_that_investigation_reports() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    // Two investigations, so each write is checked to be scoped to the one it names.
    let archived = start_investigation(
        &router,
        "seed",
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
    )
    .await;
    let untouched = start_investigation(
        &router,
        "seed",
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:role/auditor"),
    )
    .await;

    // An investigation nothing has archived is ACTIVE.
    let case = "started";
    assert_eq!(
        state_of(case, &get_investigation(&router, case, &graph_arn, &archived).await),
        STATE_ACTIVE,
        "case {case:?}: an investigation starts out not archived"
    );

    // The rows walk the modelled `State` values: archiving it, asking for the state it
    // is already in (a no-change update, not a refusal), activating it again, and
    // archiving it once more. Each row is read back through `GetInvestigation`.
    let rows = [
        ("archive", STATE_ARCHIVED),
        ("archive-again", STATE_ARCHIVED),
        ("activate", STATE_ACTIVE),
        ("activate-again", STATE_ACTIVE),
        ("archive-last", STATE_ARCHIVED),
    ];
    for (case, state) in rows {
        assert!(
            STATES.contains(&state),
            "case {case:?}: the row itself must be a modelled State, got {state:?}"
        );
        update_state(&router, case, &graph_arn, &archived, state).await;

        let got = get_investigation(&router, case, &graph_arn, &archived).await;
        assert_eq!(
            state_of(case, &got),
            state,
            "case {case:?}: expected the state the update stored, got {got:?}"
        );
        // A read does not change what a later caller sees.
        let again = get_investigation(&router, case, &graph_arn, &archived).await;
        assert_eq!(
            again, got,
            "case {case:?}: expected a repeated read to answer identically, got {again:?}"
        );
        // And the write is scoped to the investigation it named.
        let other = get_investigation(&router, case, &graph_arn, &untouched).await;
        assert_eq!(
            state_of(case, &other),
            STATE_ACTIVE,
            "case {case:?}: expected the other investigation untouched, got {other:?}"
        );
    }

    // Nothing else about the archived investigation moved: the identity and window the
    // start stored read back unchanged beside the new state.
    let got = get_investigation(&router, "after-every-update", &graph_arn, &archived).await;
    assert_eq!(
        got.get("InvestigationId").and_then(Value::as_str),
        Some(archived.as_str()),
        "expected the investigation it was asked about, got {got:?}"
    );
    assert_eq!(
        got.get("EntityArn").and_then(Value::as_str),
        Some(format!("arn:aws:iam::{ACCOUNT}:user/alice").as_str()),
        "expected the entity the start named, got {got:?}"
    );
    assert_eq!(
        got.get("EntityType").and_then(Value::as_str),
        Some("IAM_USER"),
        "expected the entity kind the ARN names, got {got:?}"
    );
}

#[tokio::test]
async fn the_status_a_read_reports_is_the_outcome_of_the_run_whatever_the_state_is() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;

    // The rows differ along the state the investigation is put in before the run comes
    // due: the lifecycle state and the completion status are independent members.
    let rows: [(&str, Option<&str>); 3] = [
        ("left-active", None),
        ("archived-while-running", Some(STATE_ARCHIVED)),
        ("archived-then-activated", Some(STATE_ACTIVE)),
    ];
    for (case, archive_as) in rows {
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
        let investigation_id = start_investigation(
            &router,
            case,
            &graph_arn,
            &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
        )
        .await;

        // The run's span is never zero, so a caller reading straight back still observes
        // the non-terminal status.
        let running = get_investigation(&router, case, &graph_arn, &investigation_id).await;
        assert_eq!(
            running.get("Status").and_then(Value::as_str),
            Some("RUNNING"),
            "case {case:?}: expected the transient status straight after the start, got {running:?}"
        );

        if let Some(state) = archive_as {
            update_state(&router, case, &graph_arn, &investigation_id, STATE_ARCHIVED).await;
            update_state(&router, case, &graph_arn, &investigation_id, state).await;
        }

        tokio::time::sleep(std::time::Duration::from_millis(
            (INVESTIGATION_RUN_MILLIS as u64) + 250,
        ))
        .await;

        // The graph ingested no activity for the entity over the scope window, so the
        // run had nothing to assemble a report from: its completion status is FAILED,
        // and it is the same on every later read.
        for read in 0..2 {
            let got = get_investigation(&router, case, &graph_arn, &investigation_id).await;
            assert_eq!(
                got.get("Status").and_then(Value::as_str),
                Some("FAILED"),
                "case {case:?}: read {read} expected the run's outcome, got {got:?}"
            );
            assert_eq!(
                state_of(case, &got),
                archive_as.unwrap_or(STATE_ACTIVE),
                "case {case:?}: read {read} expected the state the update stored, got {got:?}"
            );
        }
    }

    // And archiving an investigation whose run has already finished leaves that outcome
    // where it is.
    let investigation_id = start_investigation(
        &router,
        "finished-then-archived",
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:role/auditor"),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(
        (INVESTIGATION_RUN_MILLIS as u64) + 250,
    ))
    .await;
    let case = "finished-then-archived";
    update_state(&router, case, &graph_arn, &investigation_id, STATE_ARCHIVED).await;
    let got = get_investigation(&router, case, &graph_arn, &investigation_id).await;
    assert_eq!(
        got.get("Status").and_then(Value::as_str),
        Some("FAILED"),
        "case {case:?}: expected the status the run reached, got {got:?}"
    );
    assert_eq!(
        state_of(case, &got),
        STATE_ARCHIVED,
        "case {case:?}: expected the archived state, got {got:?}"
    );
}

#[tokio::test]
async fn an_update_naming_something_this_service_does_not_hold_is_refused_and_stores_nothing() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let investigation_id = start_investigation(
        &router,
        "seed",
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
    )
    .await;

    // A well-formed `GraphArn` the service never minted: the rows differ along which
    // field of the ARN names nothing, so each one satisfies the modelled pattern and the
    // refusal is this operation's rather than the constraint walk's.
    let absent_graphs = [
        (
            "unknown-id",
            format!("arn:aws:detective:{REGION}:{ACCOUNT}:graph:{}", "a".repeat(32)),
        ),
        ("another-region", graph_arn.replace(REGION, "eu-central-1")),
        ("another-account", graph_arn.replace(ACCOUNT, "123456789012")),
    ];
    for (case, absent) in absent_graphs {
        let (status, answered) = call(
            &router,
            UPDATE_STATE_URI,
            &json!({
                "GraphArn": absent,
                "InvestigationId": investigation_id,
                "State": STATE_ARCHIVED,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: got {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the investigation surface's fixed graph-not-found \
             message, got {message:?}"
        );
    }

    // A well-formed `InvestigationId` no investigation of the graph carries: the rows
    // differ along the id, each one inside the modelled length and pattern.
    let absent_ids = [
        ("all-nines", "9".repeat(INVESTIGATION_ID_LEN)),
        ("a-leading-one", format!("1{}", "0".repeat(INVESTIGATION_ID_LEN - 1))),
    ];
    for (case, absent) in absent_ids {
        assert_ne!(
            absent, investigation_id,
            "case {case:?}: the row itself must name no stored investigation"
        );
        let (status, answered) = call(
            &router,
            UPDATE_STATE_URI,
            &json!({
                "GraphArn": graph_arn,
                "InvestigationId": absent,
                "State": STATE_ARCHIVED,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: got {answered:?}"
        );
        let message = error_message(&answered);
        assert!(
            !message.contains(&absent) && !message.contains(graph_arn.as_str()),
            "case {case:?}: the refusal echoes neither input back, got {message:?}"
        );
    }

    // Every refusal above stored nothing: the investigation that does exist is still
    // the state it was in.
    let got = get_investigation(&router, "after-the-refusals", &graph_arn, &investigation_id).await;
    assert_eq!(
        state_of("after-the-refusals", &got),
        STATE_ACTIVE,
        "expected a refused update to store nothing, got {got:?}"
    );
}
