// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCOUNT-level concurrency metering Lambda publishes about the
//! Region it is running in, read back where a client reads it: `GetMetricData` over the
//! `AWS/Lambda` series, with CloudWatch mounted beside Lambda on one engine.
//!
//! The rule under test: at every instant it is sampled, Lambda's own concurrency metering
//! reports the concurrency the account has CLAIMED — the executions its unreserved pool is
//! processing, plus every allocation committed to an individual function (each function's
//! reserved concurrency, or the provisioned concurrency of a function that reserves none) —
//! where an execution of a reserving function is charged to that reservation and never to the
//! unreserved pool. So `UnreservedConcurrentExecutions` counts only the executions of the
//! functions with no per-function limit, `ClaimedAccountConcurrency` less
//! `UnreservedConcurrentExecutions` at one instant is exactly the allocated concurrency the
//! control plane reports through `GetAccountSettings`, and a function that is processing
//! executions carries its own `ConcurrentExecutions` observation while one processing none
//! has no datapoint in the window at all.
//!
//! Every row makes its allocations over the wire (`CreateFunction`, `PublishVersion`,
//! `CreateAlias`, `PutFunctionConcurrency`, `PutProvisionedConcurrencyConfig`) and reads the
//! series back over the wire, so metering that was computed but never published, or published
//! into a series no reader can name, fails here. The rows differ along what the rule
//! generalises over: which kind of allocation claims the capacity (none, a reservation,
//! provisioned concurrency, both on one function, both across several), the qualifier a
//! provisioned allocation sits on (a published version, an alias), how many functions share
//! the account, which of them are processing executions (none, only a reserving one, only an
//! unreserved one, several at once), and how large the burst is.
//!
//! The executions a row is processing are seeded on the stored function records, the way the
//! clock-driven suites move a stored due instant: no invocation in this emulator RUNS a
//! deployment package (`Invoke` refuses the run), so the metering is driven against the
//! executions the store holds rather than against a burst that nothing performed. The sample
//! itself is taken at a chosen instant through `publish_account_concurrency`, which is also
//! the whole of one wake-up of the background sampler — so nothing here sleeps except the one
//! test that asserts the sampler publishes with no client call at all.
//!
//! The Lambda-side harness (the router, the `Answer` wrapper, `CreateFunction` and the
//! allocation helpers) is the one `function_concurrency.rs` and
//! `account_allocated_concurrency.rs` publish, so every suite over this surface drives the
//! same wire.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    publish_account_concurrency, ComAmazonawsLambdaAwsgirapiserviceState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_allocated_concurrency::{create_alias, make_activation_due, put_provisioned};
use crate::function_concurrency::{call, create_function, put_concurrency, REGION};

/// The namespace Lambda publishes its own metrics in, and the three series this suite reads,
/// written as AWS publishes them (the AWS Lambda metric reference: `ConcurrentExecutions`
/// dimensioned by `FunctionName`, and the account-level `UnreservedConcurrentExecutions` and
/// `ClaimedAccountConcurrency`, which carry no dimension) rather than as the crate's own
/// constants — so a series published under a name no AWS reader would ask for fails here.
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const UNRESERVED_SERIES: &str = "UnreservedConcurrentExecutions";
/// See [`NAMESPACE`].
const CLAIMED_SERIES: &str = "ClaimedAccountConcurrency";
/// See [`NAMESPACE`].
const CONCURRENT_SERIES: &str = "ConcurrentExecutions";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The `StandardUnit` a concurrency is measured in: it is a count of executions. Named in
/// every query, so a datapoint published under any other unit is not selected and the row
/// fails.
const COUNT_UNIT: &str = "Count";

/// The `Period` every query reads at: the one-minute granularity these series are stored at.
const PERIOD_SECS: i64 = 60;

/// One minute, in the epoch seconds a stored datapoint is stamped with.
const MINUTE_SECS: u64 = 60;

/// How far back the sampled minute sits: far enough that the background sampler, which
/// samples at the CURRENT instant, cannot publish into the minute under test.
const SAMPLE_AGE: u64 = 5 * MINUTE_SECS;

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// How long the one test that waits for the background sampler gives it. Several times the
/// sampler's own interval, so that test fails only when nothing is published at all.
const SAMPLER_BUDGET: Duration = Duration::from_secs(40);

/// How long between two looks while waiting for it.
const LOOK_AGAIN: Duration = Duration::from_millis(200);

/// Lambda and CloudWatch on one engine: the series Lambda publishes is CloudWatch's, so it is
/// read in-process through CloudWatch's own surface — the shape `core/router/src/lib.rs`
/// mounts them in.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// One CloudWatch call, over its awsJson1.0 surface.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/monitoring/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One `MetricDataQuery` over one of the series above: `dimensions` is empty for an
/// account-level series and carries the function for a per-function one.
fn query(id: &str, metric: &str, dimensions: Value) -> Value {
    json!({
        "Id": id,
        "MetricStat": {
            "Metric": {
                "Namespace": NAMESPACE,
                "MetricName": metric,
                "Dimensions": dimensions,
            },
            "Period": PERIOD_SECS,
            "Stat": "Maximum",
            "Unit": COUNT_UNIT,
        },
        "ReturnData": true,
    })
}

/// The values the result `id` carries, in the order the response listed them.
fn values_of(case: &str, body: &Value, id: &str) -> Vec<f64> {
    let results = body
        .get("MetricDataResults")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: GetMetricData answers MetricDataResults, got {body}"));
    let result = results
        .iter()
        .find(|result| result.get("Id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("case {case:?}: no result for query {id:?} in {body}"));
    result
        .get("Values")
        .and_then(Value::as_array)
        .map(|listed| listed.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Read every series this suite asserts over one window, answering the whole response.
async fn read_series(
    router: &MultiServiceRouter,
    case: &str,
    functions: &[&str],
    start_secs: u64,
    end_secs: u64,
) -> Value {
    let mut queries = vec![
        query("unreserved", UNRESERVED_SERIES, json!([])),
        query("claimed", CLAIMED_SERIES, json!([])),
    ];
    for (index, function) in functions.iter().enumerate() {
        queries.push(query(
            &format!("running{index}"),
            CONCURRENT_SERIES,
            json!([{ "Name": FUNCTION_DIMENSION, "Value": function }]),
        ));
    }
    let (status, body) = cloudwatch(
        router,
        "GetMetricData",
        &json!({
            "StartTime": start_secs,
            "EndTime": end_secs,
            "MetricDataQueries": queries,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricData answered {status} {body}"
    );
    body
}

/// The account's allocated concurrency as the CONTROL plane reports it: the
/// `ConcurrentExecutions` quota less the `UnreservedConcurrentExecutions` left of it.
async fn allocated(case: &str, router: &MultiServiceRouter) -> f64 {
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
        .unwrap_or_else(|| panic!("case {case:?}: no ConcurrentExecutions in {}", answer.text()));
    let free = answer
        .number("/AccountLimit/UnreservedConcurrentExecutions")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no UnreservedConcurrentExecutions in {}",
                answer.text()
            )
        });
    #[allow(clippy::cast_precision_loss)]
    let allocated = (limit - free) as f64;
    allocated
}

/// Seed the executions the account is processing on one function: the state an invocation
/// that actually ran the function's deployment package would leave, which this emulator's
/// `Invoke` refuses to produce.
fn given_running(engine: &Arc<engine::InMemoryEngine>, case: &str, name: &str, running: i32) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let function = store
        .functions
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {case:?}: the store must hold the function {name:?}"));
    function.concurrent_executions = running;
}

/// One function of a row: what it allocates, and what it is processing.
struct Subject {
    name: &'static str,
    /// How many versions it publishes, so a qualifier a provisioned configuration can sit on
    /// exists.
    versions: u32,
    /// The aliases to create, each with the version it invokes.
    aliases: &'static [(&'static str, &'static str)],
    /// What `PutFunctionConcurrency` reserves, when the row reserves anything.
    reserved: Option<i64>,
    /// What `PutProvisionedConcurrencyConfig` allocates, per qualifier.
    provisioned: &'static [(&'static str, i64)],
    /// How many executions it is processing when the sample is taken.
    running: i32,
}

/// The default subject: one function publishing one version, allocating nothing, idle.
const IDLE: Subject = Subject {
    name: "metered-subject",
    versions: 1,
    aliases: &[],
    reserved: None,
    provisioned: &[],
    running: 0,
};

/// One row: the account the sample is taken over, and what the two account-level series must
/// then report.
struct Case {
    name: &'static str,
    subjects: &'static [Subject],
    /// `UnreservedConcurrentExecutions` at the sampled instant.
    unreserved: f64,
    /// `ClaimedAccountConcurrency` at the same instant.
    claimed: f64,
}

/// The rows. Between them: an account that claims nothing, each kind of allocation on its own
/// and both together, each qualifier a provisioned allocation sits on, executions on a
/// reserving function only, on an unreserved function only, on several at once, the recorded
/// shape (a reserved burst beside an unreserved one and a provisioned alias), and a burst far
/// larger than any allocation.
const CASES: &[Case] = &[
    Case {
        name: "an idle account that has allocated nothing",
        subjects: &[IDLE],
        unreserved: 0.0,
        claimed: 0.0,
    },
    Case {
        name: "a reservation of the modelled minimum claims nothing",
        subjects: &[Subject {
            reserved: Some(0),
            ..IDLE
        }],
        unreserved: 0.0,
        claimed: 0.0,
    },
    Case {
        name: "a reservation on an idle function is claimed in full",
        subjects: &[Subject {
            reserved: Some(6),
            ..IDLE
        }],
        unreserved: 0.0,
        claimed: 6.0,
    },
    Case {
        name: "provisioned concurrency on a published version is claimed",
        subjects: &[Subject {
            provisioned: &[("1", 1)],
            ..IDLE
        }],
        unreserved: 0.0,
        claimed: 1.0,
    },
    Case {
        name: "provisioned concurrency on an alias is claimed",
        subjects: &[Subject {
            aliases: &[("warm", "1")],
            provisioned: &[("warm", 4)],
            ..IDLE
        }],
        unreserved: 0.0,
        claimed: 4.0,
    },
    Case {
        name: "a reserving function's executions are charged to its reservation",
        subjects: &[Subject {
            reserved: Some(6),
            running: 4,
            ..IDLE
        }],
        unreserved: 0.0,
        claimed: 6.0,
    },
    Case {
        name: "an unreserved function's executions come out of the unreserved pool",
        subjects: &[Subject {
            running: 3,
            ..IDLE
        }],
        unreserved: 3.0,
        claimed: 3.0,
    },
    Case {
        name: "an unreserved function's executions beside its own provisioned allocation",
        subjects: &[Subject {
            provisioned: &[("1", 5)],
            running: 2,
            ..IDLE
        }],
        unreserved: 2.0,
        claimed: 7.0,
    },
    Case {
        name: "provisioned concurrency inside a reservation is claimed once",
        subjects: &[Subject {
            reserved: Some(50),
            provisioned: &[("1", 20)],
            running: 7,
            ..IDLE
        }],
        unreserved: 0.0,
        claimed: 50.0,
    },
    Case {
        name: "two unreserved functions' executions add up in the pool",
        subjects: &[
            Subject {
                name: "metered-pool-first",
                running: 3,
                ..IDLE
            },
            Subject {
                name: "metered-pool-second",
                running: 2,
                ..IDLE
            },
        ],
        unreserved: 5.0,
        claimed: 5.0,
    },
    Case {
        name: "a reserved burst beside an unreserved burst and a provisioned alias",
        subjects: &[
            Subject {
                name: "metered-reserved",
                reserved: Some(6),
                running: 4,
                ..IDLE
            },
            Subject {
                name: "metered-unreserved",
                running: 3,
                ..IDLE
            },
            Subject {
                name: "metered-warm",
                aliases: &[("warm", "1")],
                provisioned: &[("warm", 1)],
                ..IDLE
            },
        ],
        unreserved: 3.0,
        claimed: 10.0,
    },
    Case {
        name: "a burst far larger than anything the account allocated",
        subjects: &[
            Subject {
                name: "metered-burst",
                running: 100,
                ..IDLE
            },
            Subject {
                name: "metered-reserved",
                reserved: Some(6),
                ..IDLE
            },
        ],
        unreserved: 100.0,
        claimed: 106.0,
    },
];

/// Make every allocation one row asks for, over an account of its own, and seed what each of
/// its functions is processing.
async fn given(case: &Case, engine: &Arc<engine::InMemoryEngine>, router: &MultiServiceRouter) {
    let label = case.name;
    for subject in case.subjects {
        create_function(router, subject.name).await;
        make_activation_due(engine);
        for ordinal in 1..=subject.versions {
            crate::account_allocated_concurrency::publish_version(router, subject.name, ordinal)
                .await;
        }
        for (alias, version) in subject.aliases {
            create_alias(router, subject.name, alias, version).await;
        }
        if let Some(reserved) = subject.reserved {
            let put = put_concurrency(router, subject.name, reserved).await;
            assert_eq!(
                put.status,
                200,
                "case {label:?}: reserving {reserved} on {:?} must succeed, got {} {}",
                subject.name,
                put.status,
                put.text()
            );
        }
        for (qualifier, amount) in subject.provisioned {
            let put = put_provisioned(router, subject.name, qualifier, *amount).await;
            assert_eq!(
                put.status,
                202,
                "case {label:?}: allocating {amount} on {:?} {qualifier:?} must succeed, got {} {}",
                subject.name,
                put.status,
                put.text()
            );
        }
        if subject.running > 0 {
            given_running(engine, label, subject.name, subject.running);
        }
    }
}

/// A minute-aligned instant [`SAMPLE_AGE`] in the past, as epoch seconds: aligned so the one
/// sample taken at it falls in exactly one whole-minute period, and old enough that the
/// background sampler cannot publish into that minute.
fn sampled_minute() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now - now % MINUTE_SECS - SAMPLE_AGE
}

/// RULE: Lambda's own concurrency metering reports the concurrency the account has claimed —
/// the executions of the functions with no per-function limit, plus every allocation
/// committed to an individual function — so a reserving function's executions are charged to
/// its reservation, `ClaimedAccountConcurrency` less `UnreservedConcurrentExecutions` is the
/// allocated concurrency the control plane reports, and only a function that is processing
/// executions carries a `ConcurrentExecutions` observation.
#[tokio::test]
async fn the_account_concurrency_metering_reports_what_the_account_has_claimed() {
    for case in CASES {
        // One account per row, so each row's own allocations and executions are all it meters.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let label = case.name;
        given(case, &engine, &router).await;

        let minute = sampled_minute();
        publish_account_concurrency(&*engine, UNIX_EPOCH + Duration::from_secs(minute));

        let names: Vec<&str> = case.subjects.iter().map(|subject| subject.name).collect();
        let body = read_series(&router, label, &names, minute, minute + MINUTE_SECS).await;

        let unreserved = values_of(label, &body, "unreserved");
        assert_eq!(
            unreserved,
            vec![case.unreserved],
            "case {label:?}: UnreservedConcurrentExecutions over the sampled minute must be \
             [{}], got {unreserved:?}",
            case.unreserved
        );
        let claimed = values_of(label, &body, "claimed");
        assert_eq!(
            claimed,
            vec![case.claimed],
            "case {label:?}: ClaimedAccountConcurrency over the sampled minute must be [{}], got \
             {claimed:?}",
            case.claimed
        );

        // The two series are read at the SAME instant, so their difference is exactly what
        // the control plane says the account has allocated to individual functions.
        let committed = allocated(label, &router).await;
        assert_eq!(
            case.claimed - case.unreserved,
            committed,
            "case {label:?}: ClaimedAccountConcurrency ({}) less UnreservedConcurrentExecutions \
             ({}) must be the allocated concurrency the account reports ({committed}), got {}",
            case.claimed,
            case.unreserved,
            case.claimed - case.unreserved
        );

        // A function processing executions is metered per function; one processing none has no
        // datapoint in the window rather than a zero.
        for (index, subject) in case.subjects.iter().enumerate() {
            let running = values_of(label, &body, &format!("running{index}"));
            let expected: Vec<f64> = if subject.running > 0 {
                vec![f64::from(subject.running)]
            } else {
                Vec::new()
            };
            assert_eq!(
                running, expected,
                "case {label:?}: ConcurrentExecutions for {:?} must be {expected:?}, got \
                 {running:?}",
                subject.name
            );
        }

        // Reading the series changes nothing a later reader sees.
        let again = read_series(&router, label, &names, minute, minute + MINUTE_SECS).await;
        assert_eq!(
            values_of(label, &again, "claimed"),
            claimed,
            "case {label:?}: a second read of ClaimedAccountConcurrency must report {claimed:?}, \
             got {:?}",
            values_of(label, &again, "claimed")
        );
    }
}

/// RULE (the other half): this metering is published OFF the response path — an operator
/// alarming on the Region's headroom reads it without calling Lambda at all — so an account
/// that holds a function is sampled with no client action whatsoever.
#[tokio::test]
async fn the_metering_is_published_with_no_client_call_at_all() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let case = "published with no client call";
    let function = "metered-unattended";

    create_function(&router, function).await;
    make_activation_due(&engine);
    let put = put_concurrency(&router, function, 6).await;
    assert_eq!(
        put.status,
        200,
        "case {case:?}: the reservation under test must be stored, got {} {}",
        put.status,
        put.text()
    );

    // Nothing below calls Lambda again: the only thing that can publish into the window is
    // the sampler the account's own function armed.
    let start = {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now - now % MINUTE_SECS
    };
    let deadline = Instant::now() + SAMPLER_BUDGET;
    loop {
        let end = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + MINUTE_SECS;
        let body = read_series(&router, case, &[], start, end).await;
        let claimed = values_of(case, &body, "claimed");
        if claimed.contains(&6.0) {
            let unreserved = values_of(case, &body, "unreserved");
            assert!(
                unreserved.iter().all(|value| *value == 0.0),
                "case {case:?}: no execution is running, so every published \
                 UnreservedConcurrentExecutions must be 0, got {unreserved:?}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: nothing published the account's reserved 6 as \
             ClaimedAccountConcurrency without a client call, got {claimed:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
