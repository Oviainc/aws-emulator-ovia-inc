// SPDX-License-Identifier: CC0-1.0

//! `GetFunctionConcurrency` reports the reserved-concurrency setting whether or not the
//! function holds one.
//!
//! The rule under test: this response IS the setting it reports, so the member is present
//! in every answer — carrying the reserved value when the function reserves one, and JSON
//! `null` when it reserves nothing — rather than being dropped from the body, which would
//! leave a client reading the member off the response with a missing key.
//!
//! The rows differ along what the rule generalises over: the value held (none at all, the
//! modelled minimum `0`, a value in between, the largest the account rule admits) and how
//! the function came to hold it (never configured, configured, configured and then
//! cleared again). Each row asserts the KEY's presence and its JSON form, which is what a
//! body of `{}` fails and a numeric reading cannot tell apart from a rendered `null`.
//!
//! The harness (the router and the concurrency request helpers) is the one
//! `function_concurrency.rs` publishes, so both suites drive the same wire.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_concurrency::{
    call, create_function, get_concurrency, put_concurrency, router_on, Answer, MAX_RESERVABLE,
};

/// The member this response reports.
const MEMBER: &str = "ReservedConcurrentExecutions";

/// `DeleteFunctionConcurrency` (`DELETE /2017-10-31/functions/{FunctionName}/concurrency`),
/// which "Removes a concurrency configuration from a function".
async fn delete_concurrency(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2017-10-31/functions/{reference}/concurrency"),
        &Value::Null,
    )
    .await
}

/// The reserved-concurrency member of one `GetFunctionConcurrency` answer, as JSON.
///
/// A missing key and a rendered `null` are different answers, so the member is read out
/// of the object rather than coerced to a number: `None` here means the body carried no
/// such key at all.
async fn reported_member(case: &str, router: &MultiServiceRouter, reference: &str) -> Option<Value> {
    let answer = get_concurrency(router, reference).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200 from GetFunctionConcurrency, got {} {}",
        answer.status,
        answer.text()
    );
    let body: Value = serde_json::from_slice(&answer.body).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: GetFunctionConcurrency must answer a JSON object ({error}), got {:?}",
            answer.text()
        )
    });
    let object = body.as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected a JSON object, got {body}");
    });
    object.get(MEMBER).cloned()
}

/// One row: the function, the values reserved on it in order, whether the reservation is
/// then removed, and the JSON the member must carry afterwards.
struct Case {
    name: &'static str,
    function: &'static str,
    puts: &'static [i64],
    delete: bool,
    expected: Value,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a function nobody configured renders the member as null",
            function: "reported-unset",
            puts: &[],
            delete: false,
            expected: Value::Null,
        },
        Case {
            name: "the modelled minimum reservation renders as the number zero",
            function: "reported-zero",
            puts: &[0],
            delete: false,
            expected: Value::from(0),
        },
        Case {
            name: "a reservation in between renders as its own number",
            function: "reported-middle",
            puts: &[137],
            delete: false,
            expected: Value::from(137),
        },
        Case {
            name: "the largest reservation the account rule admits renders as its number",
            function: "reported-max",
            puts: &[MAX_RESERVABLE],
            delete: false,
            expected: Value::from(MAX_RESERVABLE),
        },
        Case {
            name: "a reservation replaced by another renders the last one",
            function: "reported-replaced",
            puts: &[500, 12],
            delete: false,
            expected: Value::from(12),
        },
        Case {
            name: "a reservation that was removed renders the member as null again",
            function: "reported-cleared",
            puts: &[64],
            delete: true,
            expected: Value::Null,
        },
        Case {
            name: "a zero reservation that was removed renders the member as null",
            function: "reported-cleared-zero",
            puts: &[0],
            delete: true,
            expected: Value::Null,
        },
    ]
}

#[tokio::test]
async fn the_reserved_concurrency_member_is_reported_set_or_unset() {
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        create_function(&router, case.function).await;

        // Before anything is reserved the member is already there, carrying null: the
        // body is never an object with no members.
        let initial = reported_member(label, &router, case.function).await;
        assert_eq!(
            initial,
            Some(Value::Null),
            "case {label:?}: a function reserving nothing must still render {MEMBER}, got \
             {initial:?}"
        );

        for value in case.puts {
            let put = put_concurrency(&router, case.function, *value).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 reserving {value}, got {} {}",
                put.status,
                put.text()
            );
        }
        if case.delete {
            let removed = delete_concurrency(&router, case.function).await;
            assert_eq!(
                removed.status,
                204,
                "case {label:?}: expected 204 from DeleteFunctionConcurrency, got {} {}",
                removed.status,
                removed.text()
            );
        }

        let actual = reported_member(label, &router, case.function).await;
        assert_eq!(
            actual,
            Some(case.expected.clone()),
            "case {label:?}: expected {MEMBER} to render as {}, got {actual:?}",
            case.expected
        );

        // Reading it changes nothing a later caller sees.
        let again = reported_member(label, &router, case.function).await;
        assert_eq!(
            again,
            Some(case.expected.clone()),
            "case {label:?}: a second read must render {MEMBER} as {} unchanged, got {again:?}",
            case.expected
        );
    }
}
