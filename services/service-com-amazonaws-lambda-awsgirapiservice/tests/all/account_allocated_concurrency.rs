// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the concurrency an account has ALLOCATED to individual functions,
//! which is what `GetAccountSettings` reports the account's
//! `AccountLimit$UnreservedConcurrentExecutions` as the complement of.
//!
//! The rule under test: the concurrency an account has committed to individual functions is
//! every function's reserved concurrency plus the provisioned concurrency allocated on the
//! functions that reserve none — a reserving function's provisioned concurrency is allocated
//! out of that reservation, so it is committed once, not twice — and the account's
//! `UnreservedConcurrentExecutions` is its concurrency limit less exactly that sum; the same
//! sum is what an operation committing more capacity weighs the documented
//! minimum-unreserved floor against, so a reservation that would take the account past it is
//! refused whether the capacity already committed was reserved or provisioned.
//!
//! Every row drives real HTTP requests through the router and reads the account back through
//! `GetAccountSettings`, so an allocation that was accepted but never counted fails here.
//! The rows differ along what the rule generalises over: which kind of allocation commits
//! the capacity (a reservation, provisioned concurrency, both), the kind of qualifier the
//! provisioned concurrency is added to (a published version, an alias, several of each), how
//! many functions share the account, whether the reservation was made before or after the
//! provisioned allocation, and whether the allocation has finished warming up. The
//! clock-driven half is exercised by moving the stored due instants into the past, so
//! nothing here sleeps.
//!
//! The harness (the router, the `Answer` wrapper, `CreateFunction` and the reservation
//! helpers) is the one `function_concurrency.rs` publishes, so every suite over this surface
//! drives the same wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_concurrency::{
    call, create_function, get_concurrency, put_concurrency, router_on, Answer,
    CONCURRENT_EXECUTIONS_LIMIT, MIN_UNRESERVED,
};

/// Where `CreateFunction`, `PublishVersion` and `CreateAlias` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// `PublishVersion` (`POST /2015-03-31/functions/{FunctionName}/versions`), once per
/// version the row needs.
async fn publish_version(router: &MultiServiceRouter, function: &str, ordinal: u32) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/versions"),
        &json!({ "Description": format!("snapshot {ordinal}") }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "PublishVersion {ordinal} of {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `CreateAlias` (`POST /2015-03-31/functions/{FunctionName}/aliases`).
async fn create_alias(router: &MultiServiceRouter, function: &str, name: &str, version: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/aliases"),
        &json!({ "Name": name, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {name:?} on {function:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `PutProvisionedConcurrencyConfig`
/// (`PUT /2019-09-30/functions/{FunctionName}/provisioned-concurrency?Qualifier=`).
async fn put_provisioned(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: &str,
    requested: i64,
) -> Answer {
    call(
        router,
        "PUT",
        &format!(
            "/2019-09-30/functions/{function}/provisioned-concurrency?Qualifier={qualifier}"
        ),
        &json!({ "ProvisionedConcurrentExecutions": requested }),
    )
    .await
}

/// `GetProvisionedConcurrencyConfig` (`GET` of the same resource).
async fn get_provisioned(router: &MultiServiceRouter, function: &str, qualifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/2019-09-30/functions/{function}/provisioned-concurrency?Qualifier={qualifier}"
        ),
        &Value::Null,
    )
    .await
}

/// `DeleteProvisionedConcurrencyConfig` (`DELETE` of the same resource).
async fn delete_provisioned(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/2019-09-30/functions/{function}/provisioned-concurrency?Qualifier={qualifier}"
        ),
        &Value::Null,
    )
    .await
}

/// `DeleteFunctionConcurrency` (`DELETE /2017-10-31/functions/{FunctionName}/concurrency`).
async fn delete_reservation(router: &MultiServiceRouter, function: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2017-10-31/functions/{function}/concurrency"),
        &Value::Null,
    )
    .await
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes: a function is not
/// modifiable until it is active. This is what waiting would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every stored allocation's warm-up instant into the past, so the next read
/// materialises the `IN_PROGRESS` -> `READY` transition the clock owes.
fn make_allocation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        for config in function.provisioned_concurrency_configs.values_mut() {
            config.ready_due = Some(past);
        }
    }
}

/// The concurrency the account has allocated to individual functions, as a client reads it:
/// the `ConcurrentExecutions` quota less the `UnreservedConcurrentExecutions` left of it.
async fn allocated(case: &str, router: &MultiServiceRouter) -> i64 {
    let answer = call(router, "GET", "/2016-08-19/account-settings", &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetAccountSettings must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    let limit = answer
        .number("/AccountLimit/ConcurrentExecutions")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: GetAccountSettings must report ConcurrentExecutions, got {}",
                answer.text()
            )
        });
    assert_eq!(
        limit, CONCURRENT_EXECUTIONS_LIMIT,
        "case {case:?}: the account's concurrency quota is {CONCURRENT_EXECUTIONS_LIMIT}, got \
         {limit:?}"
    );
    let free = answer
        .number("/AccountLimit/UnreservedConcurrentExecutions")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: GetAccountSettings must report \
                 UnreservedConcurrentExecutions, got {}",
                answer.text()
            )
        });
    limit - free
}

/// What one function of a row commits, and how.
struct Allocation {
    function: &'static str,
    /// How many versions the function publishes, so a qualifier a configuration can be
    /// added to exists.
    versions: u32,
    /// The aliases to create, each with the version it invokes.
    aliases: &'static [(&'static str, &'static str)],
    /// What `PutFunctionConcurrency` reserves, when the row reserves anything.
    reserved: Option<i64>,
    /// Whether the reservation is made before the provisioned allocations below (the
    /// capacity is committed either way, so the order must not change the answer).
    reserve_first: bool,
    /// What `PutProvisionedConcurrencyConfig` allocates, per qualifier.
    provisioned: &'static [(&'static str, i64)],
}

/// The default row: one function publishing one version, committing nothing.
const NOTHING: Allocation = Allocation {
    function: "allocated-subject",
    versions: 1,
    aliases: &[],
    reserved: None,
    reserve_first: true,
    provisioned: &[],
};

/// One row of the accounting table.
struct Case {
    name: &'static str,
    functions: &'static [Allocation],
    /// Whether the clock has finished every allocation's warm-up before the account is
    /// read.
    ready: bool,
    /// The concurrency the account must then report as allocated.
    expected: i64,
}

/// The rows. Between them they cover an account that commits nothing, each kind of
/// allocation on its own, both on one function, both across several functions, each kind of
/// qualifier a provisioned allocation is added to, both orders the two calls can be made in,
/// and an allocation whose warm-up is still under way.
const CASES: &[Case] = &[
    Case {
        name: "an account that has committed nothing",
        functions: &[NOTHING],
        ready: true,
        expected: 0,
    },
    Case {
        name: "a reservation of the modelled minimum commits nothing",
        functions: &[Allocation {
            reserved: Some(0),
            ..NOTHING
        }],
        ready: true,
        expected: 0,
    },
    Case {
        name: "one reservation",
        functions: &[Allocation {
            reserved: Some(6),
            ..NOTHING
        }],
        ready: true,
        expected: 6,
    },
    Case {
        name: "provisioned concurrency on a published version",
        functions: &[Allocation {
            provisioned: &[("1", 1)],
            ..NOTHING
        }],
        ready: true,
        expected: 1,
    },
    Case {
        name: "provisioned concurrency on an alias",
        functions: &[Allocation {
            aliases: &[("live", "1")],
            provisioned: &[("live", 4)],
            ..NOTHING
        }],
        ready: true,
        expected: 4,
    },
    Case {
        name: "an alias and the version it invokes each hold their own",
        functions: &[Allocation {
            aliases: &[("live", "1")],
            provisioned: &[("live", 4), ("1", 3)],
            ..NOTHING
        }],
        ready: true,
        expected: 7,
    },
    Case {
        name: "provisioned concurrency across two published versions",
        functions: &[Allocation {
            versions: 2,
            provisioned: &[("1", 2), ("2", 9)],
            ..NOTHING
        }],
        ready: true,
        expected: 11,
    },
    Case {
        name: "an allocation still warming up is already committed",
        functions: &[Allocation {
            provisioned: &[("1", 5)],
            ..NOTHING
        }],
        ready: false,
        expected: 5,
    },
    Case {
        name: "provisioned concurrency inside a reservation is committed once",
        functions: &[Allocation {
            reserved: Some(50),
            provisioned: &[("1", 20)],
            ..NOTHING
        }],
        ready: true,
        expected: 50,
    },
    Case {
        name: "a reservation made after the allocation absorbs it",
        functions: &[Allocation {
            reserved: Some(50),
            reserve_first: false,
            provisioned: &[("1", 20)],
            ..NOTHING
        }],
        ready: true,
        expected: 50,
    },
    Case {
        name: "a reservation on one function and a provisioned alias on another",
        functions: &[
            Allocation {
                function: "allocated-reserved",
                reserved: Some(6),
                ..NOTHING
            },
            Allocation {
                function: "allocated-provisioned",
                aliases: &[("live", "1")],
                provisioned: &[("live", 1)],
                ..NOTHING
            },
        ],
        ready: true,
        expected: 7,
    },
    Case {
        name: "every kind of allocation across three functions",
        functions: &[
            Allocation {
                function: "allocated-reserved",
                reserved: Some(6),
                provisioned: &[("1", 5)],
                ..NOTHING
            },
            Allocation {
                function: "allocated-provisioned",
                versions: 2,
                aliases: &[("live", "2")],
                provisioned: &[("live", 1), ("1", 2)],
                ..NOTHING
            },
            Allocation {
                function: "allocated-idle",
                ..NOTHING
            },
        ],
        ready: true,
        expected: 9,
    },
];

/// Make every allocation one row asks for, over an account of its own.
async fn given(case: &Case, engine: &Arc<engine::InMemoryEngine>, router: &MultiServiceRouter) {
    let label = case.name;
    for allocation in case.functions {
        create_function(router, allocation.function).await;
        make_activation_due(engine);
        for ordinal in 1..=allocation.versions {
            publish_version(router, allocation.function, ordinal).await;
        }
        for (alias, version) in allocation.aliases {
            create_alias(router, allocation.function, alias, version).await;
        }

        let reserve = async {
            if let Some(reserved) = allocation.reserved {
                let put = put_concurrency(router, allocation.function, reserved).await;
                assert_eq!(
                    put.status,
                    200,
                    "case {label:?}: reserving {reserved} on {:?} must succeed, got {} {}",
                    allocation.function,
                    put.status,
                    put.text()
                );
            }
        };
        let provision = async {
            for (qualifier, amount) in allocation.provisioned {
                let put = put_provisioned(router, allocation.function, qualifier, *amount).await;
                assert_eq!(
                    put.status,
                    202,
                    "case {label:?}: allocating {amount} on {:?} {qualifier:?} must succeed, \
                     got {} {}",
                    allocation.function,
                    put.status,
                    put.text()
                );
            }
        };
        if allocation.reserve_first {
            reserve.await;
            provision.await;
        } else {
            provision.await;
            reserve.await;
        }
    }
    if case.ready {
        make_allocation_due(engine);
    }
}

#[tokio::test]
async fn the_account_reports_the_concurrency_it_has_allocated_to_its_functions() {
    for case in CASES {
        // One account per row, so each row's allocations are the only ones it reports.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        given(case, &engine, &router).await;

        let committed = allocated(label, &router).await;
        assert_eq!(
            committed, case.expected,
            "case {label:?}: the account must report {} allocated, got {committed:?}",
            case.expected
        );

        // Reading the account does not change what the next caller sees.
        let again = allocated(label, &router).await;
        assert_eq!(
            again, case.expected,
            "case {label:?}: a second read must report {} allocated, got {again:?}",
            case.expected
        );
    }
}

#[tokio::test]
async fn removing_an_allocation_returns_its_capacity_to_the_account() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let reserving = "allocated-removal-reserved";
    let provisioning = "allocated-removal-provisioned";

    for function in [reserving, provisioning] {
        create_function(&router, function).await;
        make_activation_due(&engine);
        publish_version(&router, function, 1).await;
    }
    create_alias(&router, provisioning, "live", "1").await;
    let put = put_concurrency(&router, reserving, 40).await;
    assert_eq!(
        put.status,
        200,
        "the reservation under test must be stored, got {} {}",
        put.status,
        put.text()
    );
    for qualifier in ["1", "live"] {
        let put = put_provisioned(&router, provisioning, qualifier, 10).await;
        assert_eq!(
            put.status,
            202,
            "case {qualifier:?}: the allocation under test must be stored, got {} {}",
            put.status,
            put.text()
        );
    }
    make_allocation_due(&engine);
    let committed = allocated("every allocation held", &router).await;
    assert_eq!(
        committed, 60,
        "the account must report the reservation and both allocations (60), got {committed:?}"
    );

    // Removing one provisioned allocation returns exactly its capacity, and the other
    // allocations are untouched — read back through their own readers.
    let deleted = delete_provisioned(&router, provisioning, "live").await;
    assert_eq!(
        deleted.status,
        204,
        "DeleteProvisionedConcurrencyConfig must answer 204, got {} {}",
        deleted.status,
        deleted.text()
    );
    let gone = get_provisioned(&router, provisioning, "live").await;
    assert_eq!(
        gone.status,
        404,
        "the deleted configuration must be gone, got {} {}",
        gone.status,
        gone.text()
    );
    let kept = get_provisioned(&router, provisioning, "1").await;
    assert_eq!(
        kept.number("/RequestedProvisionedConcurrentExecutions"),
        Some(10),
        "the other qualifier's allocation must be untouched, got {}",
        kept.text()
    );
    let committed = allocated("one allocation removed", &router).await;
    assert_eq!(
        committed, 50,
        "removing an allocation of 10 must leave 50 allocated, got {committed:?}"
    );

    // And removing the reservation returns its capacity too.
    let removed = delete_reservation(&router, reserving).await;
    assert_eq!(
        removed.status,
        204,
        "DeleteFunctionConcurrency must answer 204, got {} {}",
        removed.status,
        removed.text()
    );
    let cleared = get_concurrency(&router, reserving).await;
    assert_eq!(
        cleared.number("/ReservedConcurrentExecutions"),
        None,
        "the removed reservation must be gone, got {}",
        cleared.text()
    );
    let committed = allocated("the reservation removed too", &router).await;
    assert_eq!(
        committed, 10,
        "only the remaining allocation of 10 is still committed, got {committed:?}"
    );

    let last = delete_provisioned(&router, provisioning, "1").await;
    assert_eq!(
        last.status,
        204,
        "DeleteProvisionedConcurrencyConfig must answer 204, got {} {}",
        last.status,
        last.text()
    );
    let committed = allocated("nothing left committed", &router).await;
    assert_eq!(
        committed, 0,
        "an account whose allocations are all removed has committed nothing, got {committed:?}"
    );
}

/// One row of the floor table: what another function has already provisioned, what this
/// reservation asks for, and whether the documented minimum-unreserved rule admits it.
struct FloorCase {
    name: &'static str,
    /// What a second function allocates on a version of its own before the call.
    provisioned_elsewhere: i64,
    requested: i64,
    admitted: bool,
}

/// The rows: the published bound of the rule and one past it, beside each amount another
/// function has provisioned.
const FLOOR_CASES: &[FloorCase] = &[
    FloorCase {
        name: "the whole reservable capacity beside no other allocation",
        provisioned_elsewhere: 0,
        requested: CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED,
        admitted: true,
    },
    FloorCase {
        name: "one past the reservable capacity beside no other allocation",
        provisioned_elsewhere: 0,
        requested: CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED + 1,
        admitted: false,
    },
    FloorCase {
        name: "all that is left beside another function's provisioned concurrency",
        provisioned_elsewhere: 400,
        requested: CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED - 400,
        admitted: true,
    },
    FloorCase {
        name: "one past what is left beside another function's provisioned concurrency",
        provisioned_elsewhere: 400,
        requested: CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED - 400 + 1,
        admitted: false,
    },
    FloorCase {
        name: "nothing is left beside a function provisioning the whole capacity",
        provisioned_elsewhere: CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED,
        requested: 1,
        admitted: false,
    },
    FloorCase {
        name: "the modelled minimum reservation is admitted with nothing left",
        provisioned_elsewhere: CONCURRENT_EXECUTIONS_LIMIT - MIN_UNRESERVED,
        requested: 0,
        admitted: true,
    },
];

#[tokio::test]
async fn a_reservation_weighs_the_floor_against_provisioned_concurrency_too() {
    for case in FLOOR_CASES {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        let subject = "allocated-floor-subject";
        let neighbour = "allocated-floor-neighbour";

        create_function(&router, subject).await;
        if case.provisioned_elsewhere > 0 {
            create_function(&router, neighbour).await;
            make_activation_due(&engine);
            publish_version(&router, neighbour, 1).await;
            let put =
                put_provisioned(&router, neighbour, "1", case.provisioned_elsewhere).await;
            assert_eq!(
                put.status,
                202,
                "case {label:?}: the neighbour's allocation of {} must succeed, got {} {}",
                case.provisioned_elsewhere,
                put.status,
                put.text()
            );
        }

        let put = put_concurrency(&router, subject, case.requested).await;
        let committed = allocated(label, &router).await;
        if case.admitted {
            assert_eq!(
                put.status,
                200,
                "case {label:?}: expected 200 reserving {}, got {} {}",
                case.requested,
                put.status,
                put.text()
            );
            assert_eq!(
                committed,
                case.provisioned_elsewhere + case.requested,
                "case {label:?}: an admitted reservation of {} must be committed beside the \
                 {} provisioned elsewhere, got {committed:?}",
                case.requested,
                case.provisioned_elsewhere
            );
        } else {
            assert_eq!(
                put.status,
                400,
                "case {label:?}: expected 400 reserving {}, got {} {}",
                case.requested,
                put.status,
                put.text()
            );
            assert!(
                put.error_code().contains("InvalidParameterValueException"),
                "case {label:?}: expected InvalidParameterValueException, got {:?}",
                put.error_code()
            );
            // A refused call commits nothing: only the neighbour's allocation is held, and
            // the function under test still reserves nothing.
            assert_eq!(
                committed, case.provisioned_elsewhere,
                "case {label:?}: a refused reservation must leave {} allocated, got \
                 {committed:?}",
                case.provisioned_elsewhere
            );
            let stored = get_concurrency(&router, subject).await;
            assert_eq!(
                stored.number("/ReservedConcurrentExecutions"),
                None,
                "case {label:?}: a refused reservation must store nothing, got {}",
                stored.text()
            );
        }
    }
}
