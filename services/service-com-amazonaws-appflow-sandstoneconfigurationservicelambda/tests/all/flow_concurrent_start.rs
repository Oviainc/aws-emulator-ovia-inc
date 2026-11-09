// SPDX-License-Identifier: CC0-1.0

//! `StartFlow` against a flow whose run is still IN FLIGHT, driven over the wire.
//!
//! The rule under test: a `StartFlow` refused because a run of that flow has not finished
//! is a conflict between the two CALLS rather than with the state of a resource, so its
//! `ConflictException` message is one fixed sentence with no variable part at all — it
//! names neither the flow the request addressed nor the run already in flight — while the
//! conflicts AppFlow raises about a flow's stored state keep naming the flow. The rows
//! differ along the flow name, the connector the flow moves data from, and the idempotency
//! tokens the two calls carry; the sentence is the same for every one of them.
//!
//! The refusal is also only as durable as the run: once the clock has finished that run,
//! the same request is served, and the refused call left no run behind.

use serde_json::{json, Value};

use crate::flow_lifecycle::{
    appflow, call, filter_task, map_task, on_demand_trigger, s3_destination, s3_source,
    salesforce_source, AFTER_RUN_SETTLES,
};

/// The message AppFlow refuses a `StartFlow` with while a run of that flow is still in
/// flight. The whole message is fixed text: no flow name, no execution id, no trailing
/// period. The model does not state it (`smithy_query(query="shape",
/// shape="ConflictException")` documents only the conflict class), so it is pinned here as
/// the sentence the service sends.
const RUN_IN_FLIGHT_CONFLICT: &str = "Conflicts detected while executing request";

/// One row of the in-flight-conflict table.
struct Race {
    case: &'static str,
    /// The name both calls address the flow by — the value a message that interpolated
    /// the request would carry.
    flow_name: &'static str,
    source: Value,
    destination: Value,
    tasks: Value,
    /// The idempotency token of the call that is accepted, if it carries one.
    winner_token: Option<&'static str>,
    /// The idempotency token of the call that is refused. Always different from the
    /// winner's, so the refusal is the conflict rather than a retry of the first call.
    loser_token: Option<&'static str>,
}

/// The table: what differs between rows is the flow name, where the run reads from, and
/// which of the two calls names an idempotency token.
fn races() -> Vec<Race> {
    vec![
        Race {
            case: "a hyphenated name, neither call naming a token",
            flow_name: "runs-race-flow",
            source: s3_source("runs-in"),
            destination: s3_destination("runs-out"),
            tasks: json!([map_task("id", "id")]),
            winner_token: None,
            loser_token: None,
        },
        Race {
            case: "a name carrying an underscore and a digit, the refused call naming a token",
            flow_name: "Race_Flow_2",
            source: s3_source("orders-in"),
            destination: s3_destination("orders-out"),
            tasks: json!([map_task("id", "id")]),
            winner_token: None,
            loser_token: Some("second-start-token"),
        },
        Race {
            case: "a dotted name, both calls naming a token of their own",
            flow_name: "a.b-c_d9",
            source: s3_source("maps-in"),
            destination: s3_destination("maps-out"),
            tasks: json!([map_task("id", "id")]),
            winner_token: Some("first-start-token"),
            loser_token: Some("second-start-token"),
        },
        Race {
            case: "a flow reading from a connector this emulator performs no transfer for",
            flow_name: "salesforce-race-flow",
            source: salesforce_source("Contact"),
            destination: s3_destination("contacts"),
            tasks: json!([filter_task(&["Id"])]),
            winner_token: Some("first-start-token"),
            loser_token: None,
        },
    ]
}

/// A `StartFlow` request, with the idempotency token the row names.
fn start_request(flow_name: &str, token: Option<&str>) -> Value {
    let mut request = json!({ "flowName": flow_name });
    if let Some(token) = token {
        request["clientToken"] = json!(token);
    }
    request
}

/// The runs the flow's history holds, read ONCE so an assertion and the diagnostic it
/// prints judge the very same answer.
async fn recorded_runs(
    router: &smithy_server_runtime::MultiServiceRouter,
    flow_name: &str,
) -> Vec<Value> {
    let records = call(
        router,
        "/describe-flow-execution-records",
        &json!({ "flowName": flow_name }),
    )
    .await;
    records
        .get("flowExecutions")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Starting a flow whose run has not finished is refused with the parameter-free conflict
/// sentence, records no second run, and is served again once that run is over.
#[tokio::test]
async fn starting_a_flow_whose_run_is_in_flight_is_refused_with_a_parameter_free_sentence() {
    for row in races() {
        let case = row.case;
        let flow_name = row.flow_name;
        let router = appflow();
        let created = call(
            &router,
            "/create-flow",
            &json!({
                "flowName": flow_name,
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": row.source,
                "destinationFlowConfigList": [row.destination],
                "tasks": row.tasks,
            }),
        )
        .await;
        assert!(
            created.is_ok(),
            "case {case:?}: expected CreateFlow to succeed, got {} {:?}",
            created.status,
            created.body
        );

        let accepted = call(
            &router,
            "/start-flow",
            &start_request(flow_name, row.winner_token),
        )
        .await;
        let execution_id = accepted.string("executionId").to_string();
        assert!(
            accepted.is_ok() && !execution_id.is_empty(),
            "case {case:?}: expected StartFlow to start a run, got {} {:?}",
            accepted.status,
            accepted.body
        );

        let refused = call(
            &router,
            "/start-flow",
            &start_request(flow_name, row.loser_token),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected a second start to conflict, got {} {:?}",
            refused.status,
            refused.body
        );
        let message = refused.string("message").to_string();
        assert_eq!(
            message, RUN_IN_FLIGHT_CONFLICT,
            "case {case:?}: expected AppFlow's conflict sentence, got {message:?}"
        );
        assert!(
            !message.contains(flow_name),
            "case {case:?}: expected no flow name in the message, got {message:?}"
        );
        assert!(
            !message.contains(&execution_id),
            "case {case:?}: expected no run id in the message, got {message:?}"
        );
        let after_refusal = recorded_runs(&router, flow_name).await;
        assert_eq!(
            after_refusal.len(),
            1,
            "case {case:?}: expected the refused call to record no run, got {after_refusal:?}"
        );

        // The refusal lasts only as long as the run does: once the clock has finished it,
        // the very same request starts a new run.
        std::thread::sleep(AFTER_RUN_SETTLES);
        let restarted = call(
            &router,
            "/start-flow",
            &start_request(flow_name, row.loser_token),
        )
        .await;
        assert!(
            restarted.is_ok(),
            "case {case:?}: expected a start after the run finished to succeed, got {} {:?}",
            restarted.status,
            restarted.body
        );
        assert_ne!(
            restarted.string("executionId"),
            execution_id.as_str(),
            "case {case:?}: expected a run of its own, got {:?}",
            restarted.body
        );
        let after_restart = recorded_runs(&router, flow_name).await;
        assert_eq!(
            after_restart.len(),
            2,
            "case {case:?}: expected the history to hold both runs, got {after_restart:?}"
        );
    }
}

/// Two `StartFlow` calls issued together on one on-demand flow: exactly one is accepted
/// and reports a run, exactly one is refused as a conflict with the same fixed sentence,
/// and the flow's history holds the single run that was started.
#[tokio::test]
async fn two_starts_issued_together_accept_one_run_and_refuse_the_other() {
    let router = appflow();
    let flow_name = "together-race-flow";
    let created = call(
        &router,
        "/create-flow",
        &json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("fanout-in"),
            "destinationFlowConfigList": [s3_destination("fanout-out")],
            "tasks": [map_task("id", "id")],
        }),
    )
    .await;
    assert!(
        created.is_ok(),
        "expected CreateFlow to succeed, got {} {:?}",
        created.status,
        created.body
    );

    // Each call carries its OWN idempotency token, so neither is a retry of the other.
    let one = start_request(flow_name, Some("racer-one"));
    let two = start_request(flow_name, Some("racer-two"));
    let (first, second) = tokio::join!(
        call(&router, "/start-flow", &one),
        call(&router, "/start-flow", &two),
    );

    let answers = [("first", &first), ("second", &second)];
    let served: Vec<&str> = answers
        .iter()
        .filter(|(_, answer)| answer.is_ok())
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        served.len(),
        1,
        "expected exactly one of the two starts to be served, got {:?} and {:?}",
        first.body,
        second.body
    );
    let (loser, refused) = answers
        .into_iter()
        .find(|(_, answer)| !answer.is_ok())
        .expect("one of the two starts is refused");
    assert_eq!(
        (
            refused.status,
            refused.error.as_str(),
            refused.string("message")
        ),
        (409, "ConflictException", RUN_IN_FLIGHT_CONFLICT),
        "case {loser:?}: expected the refused start to carry AppFlow's conflict sentence, \
         got {} {:?}",
        refused.status,
        refused.body
    );
    let history = recorded_runs(&router, flow_name).await;
    assert_eq!(
        history.len(),
        1,
        "expected one run for two racing starts, got {history:?}"
    );
}
