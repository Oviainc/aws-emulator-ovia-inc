// SPDX-License-Identifier: CC0-1.0

//! `UpdateInvestigationState` resolves the record it writes by the (`GraphArn`,
//! `InvestigationId`) PAIR, and writes only that record's `State`.
//!
//! The rules under test:
//!
//! 1. "Updates the state of an investigation": the stored record the pair names is the
//!    one that changes, so a write against one behavior graph leaves every investigation
//!    of every OTHER graph where it was — an investigation id is meaningful only within
//!    the graph that minted it, so two graphs can each hold a record under the same id.
//! 2. The write touches the `State` member and nothing else: every other member
//!    `GetInvestigation` projects for that record reads back unchanged after the update,
//!    for either value of the modelled `State` enum, in any order.
//! 3. A pair whose id names no investigation OF THAT GRAPH is the modelled
//!    `ResourceNotFoundException` (404) — the investigation surface's refusal, not the
//!    graph-not-found one, because the graph itself resolved — and stores nothing.
//!
//! The cases differ along the graph the pair names, the state asked for and the order the
//! states are asked for in. Every case drives the real wire path: the production
//! `MultiServiceRouter`, restJson1 serialization, the modelled-constraint walk and the
//! handlers.

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE, INVESTIGATION_ID_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    call_in, create_graph_ok, emulator, error_code, error_message, ACCOUNT, REGION,
};

/// The URIs of the three investigation operations these cases drive, from
/// `smithy_query(query="operation",operation="StartInvestigation,GetInvestigation,UpdateInvestigationState")`.
const START_URI: &str = "/investigations/startInvestigation";
const GET_URI: &str = "/investigations/getInvestigation";
const UPDATE_STATE_URI: &str = "/investigations/updateInvestigationState";

/// Every value the modelled `State` enum accepts
/// (`smithy_query(query="shape",shape="State")`).
const STATES: [&str; 2] = ["ACTIVE", "ARCHIVED"];

/// The state an investigation nothing has archived reports, from the same enum.
const STATE_ACTIVE: &str = "ACTIVE";

/// The state an archived investigation reports, from the same enum.
const STATE_ARCHIVED: &str = "ARCHIVED";

/// A second Region, so one store holds two behavior graphs: an account administers one
/// graph per Region, so a second graph is reached by creating it in another Region.
const OTHER_REGION: &str = "eu-central-1";

/// The members `GetInvestigation` projects for a record BESIDE its lifecycle state and
/// its run status, from
/// `smithy_query(query="operation",operation="GetInvestigation")`'s output: none of them
/// may move when an update stores a state.
const MEMBERS_AN_UPDATE_MUST_NOT_TOUCH: [&str; 8] = [
    "GraphArn",
    "InvestigationId",
    "EntityArn",
    "EntityType",
    "CreatedTime",
    "ScopeStartTime",
    "ScopeEndTime",
    "Severity",
];

/// One stored investigation, named the way the operations name it: the behavior graph's
/// ARN and the id minted within that graph, plus the Region its graph lives in.
#[derive(Debug, Clone, Copy)]
struct Pair<'a> {
    region: &'a str,
    graph: &'a str,
    id: &'a str,
}

/// `StartInvestigation` on `graph_arn`, which must succeed; answers the id it minted.
async fn start_investigation(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    graph_arn: &str,
    entity_arn: &str,
) -> String {
    let (status, started) = call_in(
        router,
        region,
        START_URI,
        &json!({
            "GraphArn": graph_arn,
            "EntityArn": entity_arn,
            "ScopeStartTime": "2024-07-02T08:00:00Z",
            "ScopeEndTime": "2024-07-02T20:00:00Z",
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

/// `GetInvestigation` on one pair, which must succeed.
async fn get_investigation(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    graph_arn: &str,
    investigation_id: &str,
) -> Value {
    let (status, got) = call_in(
        router,
        region,
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

/// `UpdateInvestigationState` on one pair: the raw status and answer, with nothing
/// asserted.
async fn update_state(
    router: &MultiServiceRouter,
    region: &str,
    graph_arn: &str,
    investigation_id: &str,
    state: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        UPDATE_STATE_URI,
        &json!({
            "GraphArn": graph_arn,
            "InvestigationId": investigation_id,
            "State": state,
        }),
    )
    .await
}

/// As [`update_state`], asserting the write was accepted.
async fn update_state_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    graph_arn: &str,
    investigation_id: &str,
    state: &str,
) {
    let (status, answered) = update_state(router, region, graph_arn, investigation_id, state).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateInvestigationState {state} expected 200, got {status} {answered:?}"
    );
}

/// The `State` one read reported.
fn state_of(case: &str, got: &Value) -> String {
    got.get("State")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: read carried no State: {got:?}"))
        .to_string()
}

/// One read's record WITHOUT the two members the lifecycle moves: `State`, which is what
/// the update stores, and `Status`, which is the outcome of the run the clock settles on
/// its own schedule. What is left is every member an update must leave alone.
fn members_an_update_must_not_touch(case: &str, got: &Value) -> Map<String, Value> {
    let mut record = got
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigation record, got {got:?}"))
        .clone();
    record.remove("State");
    record.remove("Status");
    for member in MEMBERS_AN_UPDATE_MUST_NOT_TOUCH {
        assert!(
            record.contains_key(member),
            "case {case:?}: expected the read to project {member}, got {record:?}"
        );
    }
    record
}

#[tokio::test]
async fn an_update_stores_the_state_and_leaves_every_other_member_of_the_record_alone() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let investigation_id = start_investigation(
        &router,
        "seed",
        REGION,
        &graph_arn,
        &format!("arn:aws:iam::{ACCOUNT}:role/incident-responder"),
    )
    .await;

    let before = get_investigation(&router, "started", REGION, &graph_arn, &investigation_id).await;
    let untouched = members_an_update_must_not_touch("started", &before);
    assert_eq!(
        state_of("started", &before),
        STATE_ACTIVE,
        "case \"started\": an investigation starts out not archived, got {before:?}"
    );

    // The rows walk every modelled `State` value twice, in both orders, so each row is
    // either a change or a request for the state the record already carries.
    let rows: Vec<(String, &str)> = STATES
        .iter()
        .chain(STATES.iter().rev())
        .enumerate()
        .map(|(n, state)| (format!("{n}-{state}"), *state))
        .collect();
    for (case, state) in &rows {
        update_state_ok(&router, case, REGION, &graph_arn, &investigation_id, state).await;

        let got = get_investigation(&router, case, REGION, &graph_arn, &investigation_id).await;
        assert_eq!(
            &state_of(case, &got),
            state,
            "case {case:?}: expected the state the update stored, got {got:?}"
        );
        assert_eq!(
            members_an_update_must_not_touch(case, &got),
            untouched,
            "case {case:?}: expected every other member of the record unchanged, got {got:?}"
        );
    }
}

#[tokio::test]
async fn an_update_against_one_graph_leaves_the_investigations_of_another_graph_alone() {
    let router = emulator();
    // Two graphs in one store. An investigation id is minted within a graph, so the pair
    // — not the id alone — is what names a record.
    let here = create_graph_ok(&router, REGION, &json!({})).await;
    let there = create_graph_ok(&router, OTHER_REGION, &json!({})).await;
    assert_ne!(here, there, "the two Regions must hold two graphs");

    let here_id = start_investigation(
        &router,
        "seed-here",
        REGION,
        &here,
        &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
    )
    .await;
    let there_id = start_investigation(
        &router,
        "seed-there",
        OTHER_REGION,
        &there,
        &format!("arn:aws:iam::{ACCOUNT}:role/auditor"),
    )
    .await;

    let first = Pair {
        region: REGION,
        graph: &here,
        id: &here_id,
    };
    let second = Pair {
        region: OTHER_REGION,
        graph: &there,
        id: &there_id,
    };

    // Each row writes against one of the two pairs and checks the other, so both
    // directions are covered; the state asked for differs per row as well.
    let rows = [
        ("write-first", &first, &second, STATE_ARCHIVED),
        ("write-second", &second, &first, STATE_ARCHIVED),
        ("reactivate-first", &first, &second, STATE_ACTIVE),
    ];
    for (case, written, other, state) in rows {
        let before = get_investigation(&router, case, other.region, other.graph, other.id).await;
        update_state_ok(&router, case, written.region, written.graph, written.id, state).await;

        let got = get_investigation(&router, case, written.region, written.graph, written.id).await;
        assert_eq!(
            state_of(case, &got),
            state,
            "case {case:?}: expected the state the update stored, got {got:?}"
        );
        let after = get_investigation(&router, case, other.region, other.graph, other.id).await;
        assert_eq!(
            state_of(case, &after),
            state_of(case, &before),
            "case {case:?}: expected the other graph's investigation untouched, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_id_the_named_graph_does_not_hold_is_refused_even_when_another_graph_holds_it() {
    let router = emulator();
    let here = create_graph_ok(&router, REGION, &json!({})).await;
    let empty = create_graph_ok(&router, OTHER_REGION, &json!({})).await;
    let here_id = start_investigation(
        &router,
        "seed",
        REGION,
        &here,
        &format!("arn:aws:iam::{ACCOUNT}:user/alice"),
    )
    .await;

    // The rows differ along the pair: an id another graph holds, and ids inside the
    // modelled length and pattern that no graph holds at all. The graph in every row
    // resolves, so the refusal is the investigation's rather than the graph's.
    let rows = [
        ("held-by-another-graph", OTHER_REGION, &empty, here_id.clone()),
        (
            "all-nines",
            OTHER_REGION,
            &empty,
            "9".repeat(INVESTIGATION_ID_LEN),
        ),
        (
            "unminted-here",
            REGION,
            &here,
            format!("7{}", "3".repeat(INVESTIGATION_ID_LEN - 1)),
        ),
    ];
    for (case, region, graph, id) in rows {
        assert!(
            graph != &here || id != here_id,
            "case {case:?}: the row itself must name no stored investigation"
        );
        let (status, answered) = update_state(&router, region, graph, &id, STATE_ARCHIVED).await;
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
        assert_ne!(
            message, INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE,
            "case {case:?}: the graph resolved, so the refusal is about the investigation"
        );
        assert!(
            !message.contains(&id) && !message.contains(graph.as_str()),
            "case {case:?}: the refusal echoes neither input back, got {message:?}"
        );
    }

    // Every refusal above stored nothing: the one investigation the store holds is still
    // the state it was in, and the empty graph still holds nothing under that id.
    let got = get_investigation(&router, "after-the-refusals", REGION, &here, &here_id).await;
    assert_eq!(
        state_of("after-the-refusals", &got),
        STATE_ACTIVE,
        "expected a refused update to store nothing, got {got:?}"
    );
    let (status, answered) = call_in(
        &router,
        OTHER_REGION,
        GET_URI,
        &json!({ "GraphArn": empty, "InvestigationId": here_id }),
    )
    .await;
    assert_eq!(
        status, 404,
        "expected a refused update to mint nothing on the other graph, got {status} {answered:?}"
    );
}
