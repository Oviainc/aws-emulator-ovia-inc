// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFunctionConcurrency`: "Removes a concurrent execution
//! limit from a function."
//!
//! The rule under test: `DeleteFunctionConcurrency` clears the ONE function-level reserved
//! concurrency value of the function the request names, so afterwards every reader reports
//! that function exactly as it reports one nobody ever configured (`GetFunctionConcurrency`
//! and `GetFunction`'s `Concurrency` report no value, and the capacity it held is back in
//! the account's `UnreservedConcurrentExecutions`), and a later reservation can be made
//! again; the call answers the modelled 204 with no body whether or not a limit was held,
//! so removing one twice is the same as removing it once; it touches no other function's
//! reservation; and a name that qualifies a version or an alias — a resource this
//! function-level setting does not live on — is refused with the modelled 400, while a name
//! resolving to no function at all is refused with the modelled 404, neither of them
//! clearing anything.
//!
//! Every row drives real HTTP requests through the router and reads the value back through
//! `GetFunctionConcurrency`, `GetFunction` and `GetAccountSettings`, so a removal that
//! answered 204 without clearing the stored value — or that cleared more than the function
//! it was asked about — fails here. The rows differ along what the rule generalises over:
//! the value held before the removal (none at all, each published bound of
//! `ReservedConcurrentExecutions` on this account and values between), how many times the
//! removal is called (once, twice, three times), the documented `FunctionName` form the
//! removal addresses the function by, and whether a fresh reservation follows it.
//!
//! The harness (the router, the `Answer` wrapper, the concurrency request helpers and the
//! published concurrency numbers) is the one `function_concurrency.rs` publishes, so both
//! suites drive the same wire.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_concurrency::{
    call, create_function, get_concurrency, put_concurrency, reported, router_on, unreserved,
    Answer, ACCOUNT, CONCURRENT_EXECUTIONS_LIMIT, MAX_RESERVABLE, REGION,
};

/// `DeleteFunctionConcurrency` (`DELETE /2017-10-31/functions/{FunctionName}/concurrency`).
async fn delete_concurrency(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2017-10-31/functions/{reference}/concurrency"),
        &Value::Null,
    )
    .await
}

/// Assert one removal answered the modelled success: 204, with no body of its own (the
/// operation's output shape is `Unit`).
fn assert_removed(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: expected 204 from DeleteFunctionConcurrency, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: a 204 removal carries no body, got {:?}",
        answer.text()
    );
}

/// One row of the removal table: the function, the `FunctionName` form the removal
/// addresses it by, what is reserved on it before the removal, and how many removals
/// follow.
struct Case {
    name: &'static str,
    function: &'static str,
    /// The documented `FunctionName` form the removal uses; `None` means the bare name.
    reference: Option<String>,
    /// The value reserved before the removal; `None` means the function is never
    /// configured at all, so the removal has nothing to clear.
    reserved: Option<i64>,
    /// How many times the removal is called. More than one proves it is idempotent.
    removals: usize,
    /// A reservation made after the removals, which must then be reported.
    reserved_after: Option<i64>,
}

/// The rows. Between them they cover a function that holds no reservation, the published
/// bounds of `ReservedConcurrentExecutions` on an account nothing else reserves on (its
/// modelled minimum `0` and the maximum the documented 100-unreserved rule admits) and
/// values between, each documented unqualified `FunctionName` form, one to three removals,
/// and a fresh reservation after the removal.
fn cases() -> Vec<Case> {
    vec![
        // Nothing to clear: the removal describes the state it leaves behind, so a
        // function that reserves nothing is left reserving nothing and still succeeds.
        Case {
            name: "removing the limit of a function that holds none",
            function: "removal-unset",
            reference: None,
            reserved: None,
            removals: 1,
            reserved_after: None,
        },
        // The modelled minimum (`@range(min: 0)`): a reservation of zero is a value held,
        // distinct from holding none, so the removal has to clear it.
        Case {
            name: "removing the modelled minimum reservation of zero",
            function: "removal-zero",
            reference: None,
            reserved: Some(0),
            removals: 1,
            reserved_after: None,
        },
        // One unit above the minimum, removed by the `function:` prefixed form.
        Case {
            name: "removing one unit, addressed by the function: form",
            function: "removal-one",
            reference: Some("function:removal-one".to_string()),
            reserved: Some(1),
            removals: 1,
            reserved_after: None,
        },
        // A value in between, removed by the partial ARN and then reserved again: the
        // removal clears the value without taking the function's ability to hold one.
        Case {
            name: "removing a mid-range reservation and reserving again",
            function: "removal-middle",
            reference: Some(format!("{ACCOUNT}:function:removal-middle")),
            reserved: Some(512),
            removals: 1,
            reserved_after: Some(37),
        },
        // The largest reservation the documented rule admits here, removed by the full
        // function ARN: its whole capacity has to go back to the account.
        Case {
            name: "removing the largest admitted reservation, by function ARN",
            function: "removal-max",
            reference: Some(format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:removal-max"
            )),
            reserved: Some(MAX_RESERVABLE),
            removals: 1,
            reserved_after: None,
        },
        // Twice: the second removal finds nothing left and answers the same way.
        Case {
            name: "removing the same limit twice",
            function: "removal-twice",
            reference: None,
            reserved: Some(250),
            removals: 2,
            reserved_after: None,
        },
        // Three times, then a fresh reservation at the ceiling: the capacity released by
        // the first removal is fully available again.
        Case {
            name: "removing three times, then reserving the whole capacity",
            function: "removal-thrice",
            reference: None,
            reserved: Some(MAX_RESERVABLE),
            removals: 3,
            reserved_after: Some(MAX_RESERVABLE),
        },
    ]
}

#[tokio::test]
async fn removing_a_limit_leaves_the_function_holding_none() {
    for case in cases() {
        // One account per row, so the account-wide capacity assertions below are about
        // this row's reservations only.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        create_function(&router, case.function).await;
        let reference = case
            .reference
            .clone()
            .unwrap_or_else(|| case.function.to_string());

        if let Some(value) = case.reserved {
            let put = put_concurrency(&router, &reference, value).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 reserving {value}, got {} {}",
                put.status,
                put.text()
            );
            let held = reported(label, &router, case.function).await;
            assert_eq!(
                held,
                Some(value),
                "case {label:?}: the function must hold {value} before the removal, got \
                 {held:?}"
            );
        }

        for round in 1..=case.removals {
            let removed = delete_concurrency(&router, &reference).await;
            assert_removed(label, &removed);

            // After every removal the function reads exactly like one nobody configured,
            // through both readers of the value.
            let after = reported(label, &router, case.function).await;
            assert_eq!(
                after, None,
                "case {label:?}: after removal {round} the function must hold no \
                 reservation, got {after:?}"
            );
            let free = unreserved(&router).await;
            assert_eq!(
                free, CONCURRENT_EXECUTIONS_LIMIT,
                "case {label:?}: after removal {round} the whole limit \
                 ({CONCURRENT_EXECUTIONS_LIMIT}) is unreserved again, got {free:?}"
            );
        }

        if let Some(value) = case.reserved_after {
            let put = put_concurrency(&router, &reference, value).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 reserving {value} after the removal, got {} {}",
                put.status,
                put.text()
            );
            let held = reported(label, &router, case.function).await;
            assert_eq!(
                held,
                Some(value),
                "case {label:?}: a reservation made after the removal must be reported, got \
                 {held:?}"
            );
            let free = unreserved(&router).await;
            assert_eq!(
                free,
                CONCURRENT_EXECUTIONS_LIMIT - value,
                "case {label:?}: reserving {value} again must leave {} unreserved, got \
                 {free:?}",
                CONCURRENT_EXECUTIONS_LIMIT - value
            );
        }
    }
}

#[tokio::test]
async fn removing_one_functions_limit_leaves_every_other_one_alone() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Four functions on one account: three reserving different amounts and one left
    // unconfigured. Removing the middle one's limit may touch neither of the others nor
    // the account's view of what they reserve.
    let reservations: &[(&str, i64)] = &[
        ("removal-neighbour-a", 120),
        ("removal-subject", 300),
        ("removal-neighbour-b", 0),
    ];
    for (name, _) in reservations {
        create_function(&router, name).await;
    }
    create_function(&router, "removal-neighbour-none").await;
    for (name, value) in reservations {
        let put = put_concurrency(&router, name, *value).await;
        assert_eq!(
            put.status,
            200,
            "case {name:?}: expected 200 reserving {value}, got {} {}",
            put.status,
            put.text()
        );
    }

    let removed = delete_concurrency(&router, "removal-subject").await;
    assert_removed("removal-subject", &removed);

    for (name, value) in reservations {
        let expected = if *name == "removal-subject" {
            None
        } else {
            Some(*value)
        };
        let held = reported(name, &router, name).await;
        assert_eq!(
            held, expected,
            "case {name:?}: expected {expected:?} after removing one function's limit, got \
             {held:?}"
        );
    }
    let untouched = reported("removal-neighbour-none", &router, "removal-neighbour-none").await;
    assert_eq!(
        untouched, None,
        "a function nobody configured must still hold no reservation, got {untouched:?}"
    );

    let standing: i64 = reservations
        .iter()
        .filter(|(name, _)| *name != "removal-subject")
        .map(|(_, value)| *value)
        .sum();
    let free = unreserved(&router).await;
    assert_eq!(
        free,
        CONCURRENT_EXECUTIONS_LIMIT - standing,
        "the account's unreserved capacity is the limit less the reservations that still \
         stand ({}), got {free:?}",
        CONCURRENT_EXECUTIONS_LIMIT - standing
    );
}

#[tokio::test]
async fn a_removal_that_names_no_reservable_resource_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "removal-present").await;
    let put = put_concurrency(&router, "removal-present", 42).await;
    assert_eq!(
        put.status,
        200,
        "the reservation under test must succeed, got {} {}",
        put.status,
        put.text()
    );
    // Publishing a version gives the function a qualifier that does exist, so the refusal
    // below is about the setting being function-level and not about a missing version.
    let published = call(
        &router,
        "POST",
        "/2015-03-31/functions/removal-present/versions",
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "PublishVersion must succeed, got {} {}",
        published.status,
        published.text()
    );

    // Every `FunctionName` naming no function, in each documented unqualified form.
    let missing = [
        "removal-absent".to_string(),
        format!("{ACCOUNT}:function:removal-absent"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:removal-absent"),
    ];
    for reference in &missing {
        let answer = delete_concurrency(&router, reference).await;
        assert_eq!(
            answer.status,
            404,
            "case {reference:?}: expected 404 removing the limit of a function that does not \
             exist, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {reference:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }

    // A qualified name: the reservation "appl[ies] to the function as a whole, including
    // all published versions and the unpublished version", so a version or an alias is not
    // a resource whose limit can be removed — including the version just published.
    let qualified = [
        "removal-present:1".to_string(),
        "removal-present:live".to_string(),
        format!("{ACCOUNT}:function:removal-present:1"),
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:removal-present:1"),
    ];
    for reference in &qualified {
        let answer = delete_concurrency(&router, reference).await;
        assert_eq!(
            answer.status,
            400,
            "case {reference:?}: expected 400 removing the limit of a qualified name, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer
                .error_code()
                .contains("InvalidParameterValueException"),
            "case {reference:?}: expected InvalidParameterValueException, got {:?}",
            answer.error_code()
        );
    }

    // None of the refusals cleared the reservation the function does hold.
    let held = reported("removal-present", &router, "removal-present").await;
    assert_eq!(
        held,
        Some(42),
        "a refused removal must leave the stored reservation alone, got {held:?}"
    );

    // And the unqualified removal of that same function still works, so the refusals above
    // are about the resource named and not about the operation.
    let removed = delete_concurrency(&router, "removal-present").await;
    assert_removed("removal-present", &removed);
    let cleared = get_concurrency(&router, "removal-present").await;
    assert_eq!(
        cleared.status,
        200,
        "GetFunctionConcurrency must succeed after the removal, got {} {}",
        cleared.status,
        cleared.text()
    );
    let value = cleared.number("/ReservedConcurrentExecutions");
    assert_eq!(
        value, None,
        "after the removal the function must report no ReservedConcurrentExecutions, got \
         {value:?}"
    );
}
