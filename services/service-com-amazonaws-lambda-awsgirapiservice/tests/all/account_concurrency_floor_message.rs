// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MESSAGE a write refused by the account's unreserved-concurrency
//! floor carries.
//!
//! The rule under test: when a write would leave the account's
//! `AccountLimit$UnreservedConcurrentExecutions` below the floor
//! `PutFunctionConcurrency` documents ("you leave at least 100 simultaneous executions
//! unreserved for functions that aren't configured with a per-function limit"), the
//! refusal is the service's one complete sentence for that rule — terminated by a period,
//! with the account's floor in brackets as its ONLY variable part and nothing of the
//! request in it.
//!
//! Every row drives real HTTP requests through the router and asserts the wire body, so a
//! sentence that is built anywhere but from the rule fails here. The rows differ along
//! what the rule generalises over: by how much the request overshoots the floor (one past
//! the ceiling, the whole account limit, the modelled maximum of the member), how much of
//! the account other functions already reserve (nothing, part of it, all of it), and the
//! documented `FunctionName` form the request addresses the function by — the recorded
//! case is one row.
//!
//! The floor the expected sentence states is not written as a literal: it is read back out
//! of `GetAccountSettings` at the boundary of the rule (the account reserving everything
//! it may leaves exactly the floor unreserved), so the sentence is checked against the
//! account's own limits rather than against a number copied from one run.

use std::sync::Arc;

use serde_json::Value;

use crate::function_concurrency::{
    create_function, put_concurrency, reported, router_on, unreserved, Answer, ACCOUNT,
    CONCURRENT_EXECUTIONS_LIMIT, MAX_RESERVABLE, REGION,
};

/// The `Message` of a wire error, under either casing the JSON envelopes use.
fn message(answer: &Answer) -> String {
    let body: Value = serde_json::from_slice(&answer.body).unwrap_or_else(|error| {
        panic!("expected a JSON error body, got {error}: {}", answer.text())
    });
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("expected an error Message, got {}", answer.text()))
}

/// The sentence the floor rule refuses with, for an account whose floor is `floor`.
fn floor_sentence(floor: i64) -> String {
    format!(
        "Specified ReservedConcurrentExecutions for function decreases account's \
         UnreservedConcurrentExecution below its minimum value of [{floor}]."
    )
}

/// One row: what another function already reserves, the reservation under test, and the
/// `FunctionName` form it is requested with.
struct Case {
    name: &'static str,
    /// What a second function reserves before the call under test.
    reserved_elsewhere: Option<i64>,
    requested: i64,
    /// The documented `FunctionName` form; `None` means the bare name.
    reference: Option<String>,
}

/// The name of the function every row reserves on.
const SUBJECT: &str = "floor-message-subject";

/// The rows, spanning each side of the rule's boundary from above and every documented
/// name form.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one past the reservable capacity of an empty account",
            reserved_elsewhere: None,
            requested: MAX_RESERVABLE + 1,
            reference: None,
        },
        Case {
            name: "the whole account concurrency limit, by the function: form",
            reserved_elsewhere: None,
            requested: CONCURRENT_EXECUTIONS_LIMIT,
            reference: Some(format!("function:{SUBJECT}")),
        },
        Case {
            name: "the modelled maximum of the member, by the partial ARN",
            reserved_elsewhere: None,
            requested: i64::from(i32::MAX),
            reference: Some(format!("{ACCOUNT}:function:{SUBJECT}")),
        },
        Case {
            name: "one past what is left beside a neighbour's reservation, by the full ARN",
            reserved_elsewhere: Some(500),
            requested: MAX_RESERVABLE - 500 + 1,
            reference: Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{SUBJECT}"
            )),
        },
        Case {
            name: "a single unit beside a neighbour holding the whole reservable capacity",
            reserved_elsewhere: Some(MAX_RESERVABLE),
            requested: 1,
            reference: None,
        },
    ]
}

/// The floor the account's own limits hold: the unreserved capacity left when one function
/// reserves everything the rule admits.
async fn account_floor(router: &smithy_server_runtime::MultiServiceRouter) -> i64 {
    create_function(router, "floor-message-probe").await;
    let put = put_concurrency(router, "floor-message-probe", MAX_RESERVABLE).await;
    assert_eq!(
        put.status,
        200,
        "the reservation at the rule's boundary ({MAX_RESERVABLE}) must be admitted, got {} {}",
        put.status,
        put.text()
    );
    unreserved(router).await
}

#[tokio::test]
async fn the_unreserved_floor_refusal_states_the_accounts_floor_as_one_sentence() {
    // The floor is derived once, on its own account, from the boundary of the rule.
    let probe_engine = Arc::new(engine::InMemoryEngine::new());
    let probe_router = router_on(&probe_engine);
    let floor = account_floor(&probe_router).await;
    let expected = floor_sentence(floor);

    for case in cases() {
        // One account per row, so each row's neighbour is the only other commitment the
        // rule has to leave room for.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        create_function(&router, SUBJECT).await;
        if let Some(elsewhere) = case.reserved_elsewhere {
            create_function(&router, "floor-message-neighbour").await;
            let put = put_concurrency(&router, "floor-message-neighbour", elsewhere).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: the neighbour's reservation of {elsewhere} must succeed, got \
                 {} {}",
                put.status,
                put.text()
            );
        }

        let reference = case.reference.clone().unwrap_or_else(|| SUBJECT.to_string());
        let refused = put_concurrency(&router, &reference, case.requested).await;
        assert_eq!(
            refused.status,
            400,
            "case {label:?}: expected 400 reserving {}, got {} {}",
            case.requested,
            refused.status,
            refused.text()
        );
        assert!(
            refused
                .error_code()
                .contains("InvalidParameterValueException"),
            "case {label:?}: expected InvalidParameterValueException, got {:?}",
            refused.error_code()
        );

        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected message {expected:?}, got {actual:?}"
        );
        assert!(
            actual.ends_with('.'),
            "case {label:?}: the refusal is a complete sentence ending in a period, got \
             {actual:?}"
        );
        assert!(
            actual.contains(&format!("[{floor}]")) && !actual.contains(SUBJECT),
            "case {label:?}: the bracketed value is the account's floor ({floor}) and the \
             sentence carries nothing of the request, got {actual:?}"
        );

        // A refused call stores nothing, so the floor the next refusal states is the same.
        let stored = reported(label, &router, SUBJECT).await;
        assert_eq!(
            stored, None,
            "case {label:?}: a refused reservation must store nothing, got {stored:?}"
        );
    }
}
