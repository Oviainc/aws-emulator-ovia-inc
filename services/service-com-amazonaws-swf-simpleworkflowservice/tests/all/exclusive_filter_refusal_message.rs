// SPDX-License-Identifier: CC0-1.0

//! How the visibility operations refuse a request that names more than one of their
//! mutually exclusive filters, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: a request naming two or more of the exclusive
//! filters is refused with HTTP 400, and the refusal names the conflicting filters by
//! the SHAPE each input member targets, as a bracketed list in the order the operation
//! declares those members:
//! `Cannot specify more than one exclusive filters in the same query: [<Shape>, ...]`.
//! Only the filters the request actually supplied appear in the list, and a request
//! naming at most one of them is not refused at all.
//!
//! Rows vary what the rule generalises over: which operation is asked (the two listings
//! and the open-execution counter), which filters are supplied (every pair, every
//! triple, and all four where the operation has four), and how many of them there are.
//! The filter-member-to-shape mapping is read off the model:
//! `smithy_query(query="operation", operation="ListClosedWorkflowExecutions,ListOpenWorkflowExecutions")`
//! reports `executionFilter -> WorkflowExecutionFilter`, `typeFilter ->
//! WorkflowTypeFilter`, `tagFilter -> TagFilter` and `closeStatusFilter ->
//! CloseStatusFilter`.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{error_code, full_defaults, seeded, Emulator};

/// The domain and workflow type every request below is scoped to, so a refusal is about
/// the filters and never about an unresolved resource.
const DOMAIN: &str = "filter-refusals";
const TYPE_NAME: &str = "orders";
const TYPE_VERSION: &str = "1.0";

/// One exclusive filter: the input member that carries it, the shape that member
/// targets (what the refusal names it by), and a well-formed value for it.
struct Filter {
    member: &'static str,
    shape: &'static str,
    value: fn() -> Value,
}

/// The three filters every visibility operation of this service shares, in the order
/// the inputs declare them.
static SHARED: [Filter; 3] = [
    Filter {
        member: "executionFilter",
        shape: "WorkflowExecutionFilter",
        value: || json!({ "workflowId": "wf-1" }),
    },
    Filter {
        member: "typeFilter",
        shape: "WorkflowTypeFilter",
        value: || json!({ "name": TYPE_NAME, "version": TYPE_VERSION }),
    },
    Filter {
        member: "tagFilter",
        shape: "TagFilter",
        value: || json!({ "tag": "shared" }),
    },
];

/// `ListClosedWorkflowExecutions` adds a fourth, declared before the shared three.
static CLOSE_STATUS: Filter = Filter {
    member: "closeStatusFilter",
    shape: "CloseStatusFilter",
    value: || json!({ "status": "COMPLETED" }),
};

/// The filters one operation declares, in declaration order, with the request body it
/// needs before any filter is added.
fn operations() -> Vec<(&'static str, Vec<&'static Filter>, Value)> {
    let shared: Vec<&'static Filter> = SHARED.iter().collect();
    let mut closed: Vec<&'static Filter> = vec![&CLOSE_STATUS];
    closed.extend(SHARED.iter());
    vec![
        (
            "ListOpenWorkflowExecutions",
            shared.clone(),
            json!({ "domain": DOMAIN, "startTimeFilter": { "oldestDate": 0 } }),
        ),
        (
            "CountOpenWorkflowExecutions",
            shared,
            json!({ "domain": DOMAIN, "startTimeFilter": { "oldestDate": 0 } }),
        ),
        (
            "ListClosedWorkflowExecutions",
            closed,
            json!({ "domain": DOMAIN, "startTimeFilter": { "oldestDate": 0 } }),
        ),
    ]
}

/// Every selection of `declared` holding two or more filters, each as the subset in
/// declaration order.
fn conflicting_selections(declared: &[&'static Filter]) -> Vec<Vec<&'static Filter>> {
    let mut selections = Vec::new();
    for mask in 0u32..(1 << declared.len()) {
        let chosen: Vec<&'static Filter> = declared
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, filter)| *filter)
            .collect();
        if chosen.len() > 1 {
            selections.push(chosen);
        }
    }
    selections
}

/// `body` with `filter` added to it.
fn with_filter(body: &Value, filter: &Filter) -> Value {
    let mut extended = body.clone();
    let Some(members) = extended.as_object_mut() else {
        panic!("a request body is an object, got {body:?}");
    };
    members.insert(filter.member.to_string(), (filter.value)());
    extended
}

/// The message AWS words this refusal with, for the filters `chosen` supplies.
fn expected_message(chosen: &[&'static Filter]) -> String {
    let shapes: Vec<&str> = chosen.iter().map(|filter| filter.shape).collect();
    format!(
        "Cannot specify more than one exclusive filters in the same query: [{}]",
        shapes.join(", ")
    )
}

#[tokio::test]
async fn naming_two_exclusive_filters_is_refused_with_their_shape_names() {
    let emulator = Emulator::new();
    seeded(
        &emulator,
        DOMAIN,
        DOMAIN,
        (TYPE_NAME, TYPE_VERSION),
        full_defaults("tasks", "TERMINATE"),
    )
    .await;

    for (operation, declared, base) in operations() {
        for chosen in conflicting_selections(&declared) {
            let members: Vec<&str> = chosen.iter().map(|filter| filter.member).collect();
            let label = format!("{operation} / {}", members.join("+"));
            let mut body = base.clone();
            for filter in &chosen {
                body = with_filter(&body, filter);
            }

            let (status, refused) = emulator.call(operation, &body).await;
            let code = error_code(&refused);
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {label:?}: naming {} exclusive filters is refused as a bad \
                 request, got {status} {code} with {refused:?}",
                chosen.len()
            );
            let message = refused
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {label:?}: expected a message, got {refused:?}"));
            assert_eq!(
                message,
                expected_message(&chosen),
                "case {label:?}: the refusal names the supplied filters' shapes in a \
                 bracketed list, got {message:?}"
            );
            for filter in &declared {
                let named = members.contains(&filter.member);
                assert_eq!(
                    message.contains(filter.shape),
                    named,
                    "case {label:?}: the list holds exactly the supplied filters, so \
                     {} must{} appear, got {message:?}",
                    filter.shape,
                    if named { "" } else { " not" }
                );
            }
        }
    }
}

#[tokio::test]
async fn naming_at_most_one_exclusive_filter_is_not_refused() {
    // The control: the same requests carrying ONE filter, or none, are answered — so
    // the refusal above is about the combination and not about the filters themselves.
    let emulator = Emulator::new();
    seeded(
        &emulator,
        DOMAIN,
        DOMAIN,
        (TYPE_NAME, TYPE_VERSION),
        full_defaults("tasks", "TERMINATE"),
    )
    .await;

    for (operation, declared, base) in operations() {
        emulator.ok("no filter", operation, &base).await;
        for filter in declared {
            let label = format!("{operation} / {} alone", filter.member);
            let body = with_filter(&base, filter);
            let (status, answered) = emulator.call(operation, &body).await;
            assert_eq!(
                status, 200,
                "case {label:?}: one exclusive filter is answered, got {status} \
                 with {answered:?}"
            );
        }
    }
}
