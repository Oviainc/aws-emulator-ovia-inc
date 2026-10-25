// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH rule a concurrency or runtime-management refusal diagnoses.
//!
//! The rules under test. Lambda has three separate quota rules over provisioned
//! concurrency, and each refusal states the one the request actually violated — a caller's
//! only signal for what to change:
//!
//! * the capacity a function that reserves nothing asks to keep initialised is drawn from
//!   the account's UNRESERVED concurrency, and a request for more than that pool holds is
//!   refused with the sentence naming the pool, carrying the account's own
//!   `AccountLimit$UnreservedConcurrentExecutions` as its only variable part;
//! * a request the pool does hold, but that would empty it past the floor
//!   `PutFunctionConcurrency` documents ("you leave at least 100 simultaneous executions
//!   unreserved for functions that aren't configured with a per-function limit"), is
//!   refused with the floor sentence instead — the same call, a different rule, a
//!   different sentence;
//! * a reserving function allocates its provisioned concurrency OUT OF its reservation, so
//!   the total across its aliases and versions has to fit inside the reservation. That is
//!   ONE rule violable from either side — by allocating more than the reservation admits,
//!   or by shrinking the reservation below what is already allocated — and both sides are
//!   refused with the single constant sentence that states it, with nothing of the request
//!   interpolated.
//!
//! And one conditional-requirement rule: `RuntimeVersionArn` is "only required if you're
//! using the Manual runtime update mode", a condition the model cannot express as a
//! required member, so `PutRuntimeManagementConfig` diagnoses the missing member itself
//! with its own fixed sentence — the member, the condition, and the remedy.
//!
//! Every row drives real HTTP requests through the router and asserts the wire body, and
//! reads the resource back afterwards, so a refusal that stated the wrong rule or that
//! stored something anyway fails here. The rows differ along what the rules generalise
//! over: how far past the pool or the reservation the request reaches (one unit, the whole
//! account quota, the modelled maximum of the member), what other functions have already
//! committed (nothing, a reservation, provisioned concurrency), which side of the
//! per-function rule is violated, the kind of qualifier the capacity is attached to (a
//! published version, an alias, two of them at once), the version a runtime-management
//! write addresses, and the documented `FunctionName` form the request carries — the
//! recorded cases are one row each.
//!
//! The harness is the one `function_concurrency.rs` and `account_allocated_concurrency.rs`
//! publish, so every suite over this surface drives the same wire.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_allocated_concurrency::{
    create_alias, get_provisioned, make_activation_due, publish_version, put_provisioned,
};
use crate::account_concurrency_floor_message::message;
use crate::function_concurrency::{
    call, create_function, get_concurrency, put_concurrency, router_on, unreserved, Answer,
    ACCOUNT, CONCURRENT_EXECUTIONS_LIMIT, MAX_RESERVABLE, MIN_UNRESERVED, REGION,
};

/// The published version every row attaches capacity to first: `PublishVersion` numbers
/// versions from `1`.
const FIRST_VERSION: &str = "1";

/// The alias every row that needs a second qualifier creates.
const ALIAS: &str = "live";

/// The function under test in every table.
const SUBJECT: &str = "quota-rule-subject";

/// The second function a row uses to commit capacity elsewhere first.
const NEIGHBOUR: &str = "quota-rule-neighbour";

/// The sentence that states the ACCOUNT pool rule: the allocation asks for more
/// simultaneous executions than the account has left unreserved. Its only variable part is
/// the account's own unreserved concurrency, which every row reads back out of
/// `GetAccountSettings` rather than writing as a literal.
fn pool_sentence(unreserved: i64) -> String {
    format!(
        "Specified ConcurrentExecutions for function is greater than account's unreserved \
         concurrency [{unreserved}]."
    )
}

/// The sentence that states the account FLOOR rule, whose only variable part is the
/// capacity an account keeps unreserved.
fn floor_sentence(floor: i64) -> String {
    format!(
        "Specified ConcurrentExecutions for function decreases account's \
         UnreservedConcurrentExecution below its minimum value of [{floor}]."
    )
}

/// The constant sentence that states the PER-FUNCTION rule, from whichever side it was
/// violated. It has no slot: neither the reservation nor the requested amount appears in
/// it.
const FUNCTION_CAP_SENTENCE: &str =
    "Requested Provisioned Concurrency should not be greater than the \
     reservedConcurrentExecution for function";

/// The constant sentence `PutRuntimeManagementConfig` states for the manual mode without
/// the runtime version ARN that mode requires.
const MISSING_RUNTIME_VERSION_ARN_SENTENCE: &str =
    "Missing runtime version ARN. A runtime version ARN is required when \
     UpdateRuntimeVersionOn=Manual. Provide a valid runtime version ARN and retry the \
     request.";

/// Where both runtime-management operations are bound, optionally with the `Qualifier`
/// query parameter.
fn runtime_management_uri(reference: &str, qualifier: Option<&str>) -> String {
    let base = format!("/2021-07-20/functions/{reference}/runtime-management-config");
    match qualifier {
        Some(qualifier) => format!("{base}?Qualifier={qualifier}"),
        None => base,
    }
}

/// `PutRuntimeManagementConfig`, with the `RuntimeVersionArn` member sent only when the
/// caller names one.
async fn put_runtime_management(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
    update_runtime_on: &str,
    runtime_version_arn: Option<&str>,
) -> Answer {
    let mut body = json!({ "UpdateRuntimeOn": update_runtime_on });
    if let Some(arn) = runtime_version_arn {
        body["RuntimeVersionArn"] = json!(arn);
    }
    call(
        router,
        "PUT",
        &runtime_management_uri(reference, qualifier),
        &body,
    )
    .await
}

/// `GetRuntimeManagementConfig`, which every runtime-management row reads the refusal's
/// effect (none) back through.
async fn get_runtime_management(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> Answer {
    call(
        router,
        "GET",
        &runtime_management_uri(reference, qualifier),
        &Value::Null,
    )
    .await
}

/// Assert that `answer` is the modelled 400 for an invalid parameter value carrying
/// exactly `expected`.
fn assert_refused_with(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected 400, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains("InvalidParameterValueException"),
        "case {case:?}: expected InvalidParameterValueException, got {:?}",
        answer.error_code()
    );
    let actual = message(answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
}

/// The qualifier a row attaches capacity to: the published version, or an alias of it.
#[derive(Clone, Copy, Debug)]
enum Qualifier {
    Version,
    Alias,
}

impl Qualifier {
    /// The label this qualifier is addressed by on the wire.
    fn label(self) -> &'static str {
        match self {
            Self::Version => FIRST_VERSION,
            Self::Alias => ALIAS,
        }
    }
}

/// Which of the two account-level rules a row violates.
#[derive(Clone, Copy, Debug)]
enum AccountRule {
    /// More than the unreserved pool holds at all.
    PastThePool,
    /// Inside the pool, but it may not be emptied that far.
    PastTheFloor,
}

/// One row of the account-rule table.
#[derive(Debug)]
struct AccountCase {
    name: &'static str,
    /// What a second function reserves before the call.
    neighbour_reserves: Option<i64>,
    /// What a second function allocates on a version of its own before the call.
    neighbour_provisions: Option<i64>,
    qualifier: Qualifier,
    /// The documented `FunctionName` form the request carries, built from the name.
    reference: fn(&str) -> String,
    requested: i64,
    violates: AccountRule,
}

/// The rows: each side of the boundary between the two rules, beside each kind of capacity
/// another function has already committed, on both kinds of qualifier and through the
/// documented name forms.
fn account_cases() -> Vec<AccountCase> {
    vec![
        AccountCase {
            name: "one past the whole pool of an account committing nothing else",
            neighbour_reserves: None,
            neighbour_provisions: None,
            qualifier: Qualifier::Version,
            reference: |name| name.to_string(),
            requested: CONCURRENT_EXECUTIONS_LIMIT + 1,
            violates: AccountRule::PastThePool,
        },
        AccountCase {
            name: "ten million past the account quota, by the full ARN",
            neighbour_reserves: None,
            neighbour_provisions: None,
            qualifier: Qualifier::Version,
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
            requested: CONCURRENT_EXECUTIONS_LIMIT + 10_000_000,
            violates: AccountRule::PastThePool,
        },
        AccountCase {
            name: "the modelled maximum of the member, on an alias",
            neighbour_reserves: None,
            neighbour_provisions: None,
            qualifier: Qualifier::Alias,
            reference: |name| name.to_string(),
            requested: i64::from(i32::MAX),
            violates: AccountRule::PastThePool,
        },
        AccountCase {
            name: "one past the pool a neighbour's reservation leaves, by the partial ARN",
            neighbour_reserves: Some(300),
            neighbour_provisions: None,
            qualifier: Qualifier::Version,
            reference: |name| format!("{ACCOUNT}:function:{name}"),
            requested: CONCURRENT_EXECUTIONS_LIMIT - 300 + 1,
            violates: AccountRule::PastThePool,
        },
        AccountCase {
            name: "one past the pool a neighbour's allocation leaves, on an alias",
            neighbour_reserves: None,
            neighbour_provisions: Some(400),
            qualifier: Qualifier::Alias,
            reference: |name| format!("function:{name}"),
            requested: CONCURRENT_EXECUTIONS_LIMIT - 400 + 1,
            violates: AccountRule::PastThePool,
        },
        AccountCase {
            name: "inside the pool, one past what the floor admits",
            neighbour_reserves: None,
            neighbour_provisions: None,
            qualifier: Qualifier::Version,
            reference: |name| name.to_string(),
            requested: MAX_RESERVABLE + 1,
            violates: AccountRule::PastTheFloor,
        },
        AccountCase {
            name: "inside the pool a neighbour's reservation leaves, one past the floor",
            neighbour_reserves: Some(300),
            neighbour_provisions: None,
            qualifier: Qualifier::Alias,
            reference: |name| name.to_string(),
            requested: CONCURRENT_EXECUTIONS_LIMIT - 300 - MIN_UNRESERVED + 1,
            violates: AccountRule::PastTheFloor,
        },
    ]
}

/// Create the subject function with a published version, and the alias rows that need one
/// address it by.
async fn subject_with_qualifiers(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    create_function(router, name).await;
    make_activation_due(engine);
    publish_version(router, name, 1).await;
    create_alias(router, name, ALIAS, FIRST_VERSION).await;
    make_activation_due(engine);
}

#[tokio::test]
async fn an_allocation_the_account_cannot_cover_names_the_rule_it_violated() {
    for case in account_cases() {
        // One account per row, so what the row commits elsewhere is the only other
        // commitment the rules have to leave room for.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        subject_with_qualifiers(&router, &engine, SUBJECT).await;
        if let Some(reserved) = case.neighbour_reserves {
            create_function(&router, NEIGHBOUR).await;
            let put = put_concurrency(&router, NEIGHBOUR, reserved).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: the neighbour's reservation of {reserved} must succeed, got \
                 {} {}",
                put.status,
                put.text()
            );
        }
        if let Some(provisioned) = case.neighbour_provisions {
            subject_with_qualifiers(&router, &engine, NEIGHBOUR).await;
            let put = put_provisioned(&router, NEIGHBOUR, FIRST_VERSION, provisioned).await;
            assert_eq!(
                put.status,
                202,
                "case {label:?}: the neighbour's allocation of {provisioned} must succeed, got \
                 {} {}",
                put.status,
                put.text()
            );
        }

        // The pool the sentence states is the account's own, read through the API rather
        // than written as a literal.
        let pool = unreserved(&router).await;
        let expected = match case.violates {
            AccountRule::PastThePool => pool_sentence(pool),
            AccountRule::PastTheFloor => floor_sentence(MIN_UNRESERVED),
        };
        assert!(
            match case.violates {
                AccountRule::PastThePool => case.requested > pool,
                AccountRule::PastTheFloor => {
                    case.requested <= pool && pool - case.requested < MIN_UNRESERVED
                }
            },
            "case {label:?}: the row must violate {:?} against a pool of {pool}, asking for {}",
            case.violates,
            case.requested
        );

        let qualifier = case.qualifier.label();
        let refused = put_provisioned(
            &router,
            &(case.reference)(SUBJECT),
            qualifier,
            case.requested,
        )
        .await;
        assert_refused_with(label, &refused, &expected);

        // A refused allocation stores nothing, so the pool the next refusal states is the
        // same one.
        let stored = get_provisioned(&router, SUBJECT, qualifier).await;
        assert_eq!(
            stored.status,
            404,
            "case {label:?}: a refused allocation must store nothing, got {} {}",
            stored.status,
            stored.text()
        );
        let after = unreserved(&router).await;
        assert_eq!(
            after, pool,
            "case {label:?}: a refused allocation must leave the account's unreserved \
             concurrency at {pool}, got {after:?}"
        );
    }
}

/// One row of the per-function table: the reservation the subject holds, what one of its
/// qualifiers already keeps initialised, and the call that breaks the rule — an allocation
/// past the reservation, or a reservation below what is allocated.
#[derive(Debug)]
struct FunctionCapCase {
    name: &'static str,
    reserved: i64,
    /// What the subject allocates on its published version before the call, when it does.
    allocated: Option<i64>,
    /// The allocation under test: the qualifier it addresses and the amount it asks for.
    allocate: Option<(Qualifier, i64)>,
    /// The reservation under test, for the rows that violate the rule from that side.
    reserve: Option<i64>,
}

/// The rows: the rule broken from BOTH sides, over reservations at the modelled minimum
/// positive value, in the middle and at the largest the account admits, with the capacity
/// on one qualifier or two.
fn function_cap_cases() -> Vec<FunctionCapCase> {
    vec![
        FunctionCapCase {
            name: "one past a reservation of a single execution",
            reserved: 1,
            allocated: None,
            allocate: Some((Qualifier::Version, 2)),
            reserve: None,
        },
        FunctionCapCase {
            name: "one past a reservation of four, on an alias",
            reserved: 4,
            allocated: None,
            allocate: Some((Qualifier::Alias, 5)),
            reserve: None,
        },
        FunctionCapCase {
            name: "far past a mid-range reservation",
            reserved: 50,
            allocated: None,
            allocate: Some((Qualifier::Version, 550)),
            reserve: None,
        },
        FunctionCapCase {
            name: "one past the largest reservation the account admits",
            reserved: MAX_RESERVABLE,
            allocated: None,
            allocate: Some((Qualifier::Version, MAX_RESERVABLE + 1)),
            reserve: None,
        },
        FunctionCapCase {
            name: "a second qualifier taking the total past the reservation",
            reserved: 10,
            allocated: Some(6),
            allocate: Some((Qualifier::Alias, 5)),
            reserve: None,
        },
        FunctionCapCase {
            name: "a reservation one below what a qualifier already keeps initialised",
            reserved: 10,
            allocated: Some(10),
            allocate: None,
            reserve: Some(9),
        },
        FunctionCapCase {
            name: "a reservation of none below what a qualifier keeps initialised",
            reserved: 10,
            allocated: Some(4),
            allocate: None,
            reserve: Some(0),
        },
    ]
}

#[tokio::test]
async fn the_per_function_cap_states_one_sentence_from_either_side() {
    for case in function_cap_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;

        subject_with_qualifiers(&router, &engine, SUBJECT).await;
        let reserved = put_concurrency(&router, SUBJECT, case.reserved).await;
        assert_eq!(
            reserved.status,
            200,
            "case {label:?}: the reservation of {} must succeed, got {} {}",
            case.reserved,
            reserved.status,
            reserved.text()
        );
        if let Some(allocated) = case.allocated {
            let put = put_provisioned(&router, SUBJECT, FIRST_VERSION, allocated).await;
            assert_eq!(
                put.status,
                202,
                "case {label:?}: the allocation of {allocated} inside the reservation must \
                 succeed, got {} {}",
                put.status,
                put.text()
            );
        }

        if let Some((qualifier, amount)) = case.allocate {
            let refused = put_provisioned(&router, SUBJECT, qualifier.label(), amount).await;
            assert_refused_with(label, &refused, FUNCTION_CAP_SENTENCE);
            // The refusal stored nothing: the qualifier it addressed holds no
            // configuration of its own.
            let stored = get_provisioned(&router, SUBJECT, qualifier.label()).await;
            let expected_status = if matches!(qualifier, Qualifier::Version)
                && case.allocated.is_some()
            {
                200
            } else {
                404
            };
            assert_eq!(
                stored.status,
                expected_status,
                "case {label:?}: a refused allocation must leave the qualifier as it was \
                 ({expected_status}), got {} {}",
                stored.status,
                stored.text()
            );
            if expected_status == 200 {
                let held = stored.number("/RequestedProvisionedConcurrentExecutions");
                assert_eq!(
                    held,
                    case.allocated,
                    "case {label:?}: the allocation already held must be untouched, got {held:?}"
                );
            }
        }

        if let Some(amount) = case.reserve {
            let refused = put_concurrency(&router, SUBJECT, amount).await;
            assert_refused_with(label, &refused, FUNCTION_CAP_SENTENCE);
            // The refusal stored nothing: the function still reserves what it did.
            let held = get_concurrency(&router, SUBJECT).await;
            assert_eq!(
                held.number("/ReservedConcurrentExecutions"),
                Some(case.reserved),
                "case {label:?}: a refused reservation must leave {} reserved, got {}",
                case.reserved,
                held.text()
            );
        }
    }
}

/// One row of the runtime-management table: the version the write addresses, and the
/// `FunctionName` form it carries.
#[derive(Debug)]
struct ManualCase {
    name: &'static str,
    /// The `Qualifier` query parameter the write sends, when it sends one.
    qualifier: Option<&'static str>,
    reference: fn(&str) -> String,
}

/// The rows: every documented way of addressing a version, through the documented name
/// forms.
fn manual_cases() -> Vec<ManualCase> {
    vec![
        ManualCase {
            name: "the unpublished version, addressed implicitly",
            qualifier: None,
            reference: |name| name.to_string(),
        },
        ManualCase {
            name: "the unpublished version, addressed by Qualifier",
            qualifier: Some("$LATEST"),
            reference: |name| name.to_string(),
        },
        ManualCase {
            name: "a published version, by the full ARN",
            qualifier: Some(FIRST_VERSION),
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        },
        ManualCase {
            name: "the version an alias invokes, by the partial ARN",
            qualifier: Some(ALIAS),
            reference: |name| format!("{ACCOUNT}:function:{name}"),
        },
        ManualCase {
            name: "a published version, by the qualified name",
            qualifier: None,
            reference: |name| format!("{name}:{FIRST_VERSION}"),
        },
    ]
}

#[tokio::test]
async fn the_manual_runtime_mode_without_an_arn_names_the_member_it_needs() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    subject_with_qualifiers(&router, &engine, SUBJECT).await;

    for case in manual_cases() {
        let label = case.name;
        let reference = (case.reference)(SUBJECT);
        let before = get_runtime_management(&router, &reference, case.qualifier).await;
        assert_eq!(
            before.status,
            200,
            "case {label:?}: GetRuntimeManagementConfig must report the version before the \
             refusal, got {} {}",
            before.status,
            before.text()
        );

        let refused =
            put_runtime_management(&router, &reference, case.qualifier, "Manual", None).await;
        assert_refused_with(label, &refused, MISSING_RUNTIME_VERSION_ARN_SENTENCE);

        // The refusal stored nothing: the version reports exactly what it did before.
        let after = get_runtime_management(&router, &reference, case.qualifier).await;
        assert_eq!(
            after.text(),
            before.text(),
            "case {label:?}: a refused write must leave the configuration at {}, got {}",
            before.text(),
            after.text()
        );
    }
}
