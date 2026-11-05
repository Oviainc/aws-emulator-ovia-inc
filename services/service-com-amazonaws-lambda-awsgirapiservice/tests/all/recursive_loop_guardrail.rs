// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Lambda's RECURSIVE-LOOP GUARDRAIL, read back where a client reads
//! it: the `AWS/Lambda` series CloudWatch answers for, with CloudWatch mounted beside
//! Lambda on one engine.
//!
//! The RULE asserted over the table below: an invocation that arrives as part of a request
//! chain which has already invoked the SAME function the published number of times (16) is
//! one Lambda has detected a recursive loop in, so a function whose recursive loop
//! detection configuration is `Terminate` has that invocation STOPPED — nothing is queued
//! for it and nothing runs — and every stopped invocation is published as one
//! `RecursiveInvocationsDropped` datapoint in the `AWS/Lambda` namespace, dimensioned on
//! `FunctionName` and counted in `Count`. A function configured `Allow` has nothing stopped
//! and NO such series at all, which is the only thing that makes the setting observable: an
//! emulator publishing for both arms, or for neither, would look identical to a client.
//!
//! The chain a request belongs to is the one its own tracing header names, which is what a
//! caller propagates when it invokes a function from inside another call. So the rows differ
//! along what the rule generalises over: the configuration the function holds (the
//! documented default, `Terminate` set explicitly, `Allow`), the invocation type each hop
//! uses (asynchronous, synchronous, the dry run that invokes nothing), how deep the chain is
//! driven (one hop, exactly the published limit, one past it, far past it), whether the hops
//! belong to ONE chain or to a fresh chain each, and whether they carry a tracing header at
//! all.
//!
//! Every name here is written as a literal, taken from the AWS Lambda metric reference
//! rather than from this crate's constants, so a series published under a name no AWS reader
//! would ask for fails here instead of agreeing with itself. The Lambda-side harness (the
//! router, the `Answer` wrapper and `CreateFunction`) is the one `function_lifecycle.rs`
//! publishes.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ComAmazonawsLambdaAwsgirapiserviceState,
    ASYNC_RECURSION_STOPPED_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_active_function, json_call, package_bytes, send, Answer, REGION,
};

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";

/// The name that reference gives the invocations the guardrail dropped, and the unit it
/// counts them in.
const DROPPED_SERIES: &str = "RecursiveInvocationsDropped";
/// See [`DROPPED_SERIES`].
const COUNT_UNIT: &str = "Count";

/// The dimension that reference reports it per: the function whose invocation was stopped.
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's own
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds. Wide enough that the
/// minutes the drops were published in are inside it however the row straddles a minute
/// boundary.
const WINDOW_SECS: f64 = 600.0;

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// Where `Invoke` is bound, from `smithy_query(query="operation", operation="Invoke")`
/// (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// Where `PutFunctionRecursionConfig` is bound (`PUT
/// /2024-08-31/functions/{FunctionName}/recursion-config`).
const RECURSION_CONFIG_URI: &str = "/2024-08-31/functions";

/// The request header `Invoke$InvocationType` is bound to.
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";

/// The request header a tracing header arrives on, as X-Ray spells it.
const TRACE_HEADER: &str = "x-amzn-trace-id";

/// The `InvocationType` values the rows drive, from the model's enum
/// (`Event | RequestResponse | DryRun`).
const EVENT: &str = "Event";
/// See [`EVENT`].
const REQUEST_RESPONSE: &str = "RequestResponse";
/// See [`EVENT`].
const DRY_RUN: &str = "DryRun";

/// The `RecursiveLoop` values, from the model's enum (`Allow | Terminate`).
const TERMINATE: &str = "Terminate";
/// See [`TERMINATE`].
const ALLOW: &str = "Allow";

/// The status an accepted asynchronous invocation answers, and the one a dry run answers
/// ("For the Event invocation type, this status code is 202"; "For the DryRun invocation
/// type, the status code is 204").
const ACCEPTED: u16 = 202;
/// See [`ACCEPTED`].
const DRY_RUN_STATUS: u16 = 204;

/// The modelled status and wire code of an invocation the guardrail stopped
/// (`smithy_query(query="shape", shape="RecursiveInvocationException")`: a client error at
/// HTTP 400).
const STOPPED_STATUS: u16 = 400;
/// See [`STOPPED_STATUS`].
const STOPPED_CODE: &str = "RecursiveInvocationException";

/// The status a synchronous invocation the guardrail ADMITTED is answered with here.
///
/// The invocation reaches the run, and this suite's deployment package is 64 bytes that are
/// not a readable archive — "Lambda could not unzip the deployment package", the modelled
/// `InvalidZipFileException` at HTTP 502. What this suite asserts with it is that the
/// guardrail let the invocation through to the run at all.
const RUN_REACHED_STATUS: u16 = 502;

/// Lambda and CloudWatch on one engine: the series Lambda publishes is CloudWatch's, so it
/// is read in-process through CloudWatch's own surface — the shape `core/router/src/lib.rs`
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
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
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

/// Now, in the epoch seconds a stored datapoint is stamped with.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// A well-formed X-Ray trace id, which is what identifies one request chain: the version,
/// the time in 8 hexadecimal digits and a 96-bit unique part in 24 (the X-Ray tracing
/// header's documented form). `nth` makes each chain of a row a different one.
fn chain(nth: u32) -> String {
    format!("1-5759e988-{nth:024x}")
}

/// `Invoke` on `reference` with `invocation_type`, carrying the tracing header of the chain
/// `chain` names when the hop belongs to one.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    invocation_type: &str,
    chain: Option<&str>,
    payload: &[u8],
) -> Answer {
    let mut headers = vec![(INVOCATION_TYPE_HEADER, invocation_type.to_string())];
    if let Some(root) = chain {
        headers.push((TRACE_HEADER, format!("Root={root};Sampled=1")));
    }
    send(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{reference}/invocations"),
        "application/octet-stream",
        payload.to_vec(),
        &headers,
    )
    .await
}

/// `PutFunctionRecursionConfig` on `name`.
async fn put_recursion_config(router: &MultiServiceRouter, name: &str, value: &str) -> Answer {
    json_call(
        router,
        "PUT",
        &format!("{RECURSION_CONFIG_URI}/{name}/recursion-config"),
        &json!({ "RecursiveLoop": value }),
    )
    .await
}

/// The `Datapoints` `GetMetricStatistics` answers for the [`DROPPED_SERIES`] series of one
/// function, selected by the unit the AWS Lambda metric reference gives it.
async fn dropped_datapoints(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
) -> Vec<Value> {
    let now = now_secs();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": DROPPED_SERIES,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum"],
            "Unit": COUNT_UNIT,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics({DROPPED_SERIES}) answered {status} {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// How many invocations of `function` the guardrail is reported to have dropped: the SUM of
/// the series over a window covering the whole row, which is the magnitude a client asserts
/// on.
async fn dropped_sum(router: &MultiServiceRouter, case: &str, function: &str) -> f64 {
    dropped_datapoints(router, case, function)
        .await
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// The metric names [`NAMESPACE`] reports for the series dimensioned on `function` alone.
async fn series_names_of(router: &MultiServiceRouter, case: &str, function: &str) -> Vec<String> {
    let (status, listed) = cloudwatch(
        router,
        "ListMetrics",
        &json!({
            "Namespace": NAMESPACE,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMetrics answered {status} {listed}"
    );
    listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|series| series["MetricName"].as_str().map(str::to_string))
        .collect()
}

/// What the function `name` holds for the asynchronous invocations it accepted: each
/// entry's outcome, in queue order, read through the service's own state because Lambda
/// exposes no operation that reports the queue.
fn accepted_outcomes(
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> Vec<AsyncInvocationOutcome> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map(|function| {
            function
                .async_invocations
                .iter()
                .map(|invocation| invocation.outcome.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// One row: a function, the chain its hops belong to, and how many of those hops Lambda
/// must stop.
struct Row {
    case: &'static str,
    /// What `PutFunctionRecursionConfig` sets, or `None` to leave the documented default.
    configured: Option<&'static str>,
    /// The `InvocationType` every hop uses.
    invocation_type: &'static str,
    /// How many hops are driven.
    hops: u32,
    /// Whether every hop belongs to the SAME chain — a request coming back round into the
    /// function — or to a fresh chain of its own.
    one_chain: bool,
    /// Whether the hops carry a tracing header at all.
    traced: bool,
    /// How many of the hops the guardrail must stop, which is also how many datapoints the
    /// series must carry.
    stopped: u32,
}

/// The rows. The published limit is 16 invocations of one function in one chain, so a chain
/// of 16 hops is served in full, the 17th is the first one stopped, and a chain driven to 24
/// — past any plausible threshold — has its last 8 stopped. Every count here is written out
/// rather than computed from the crate's own constant.
const ROWS: &[Row] = &[
    Row {
        case: "a single asynchronous hop of a chain",
        configured: None,
        invocation_type: EVENT,
        hops: 1,
        one_chain: true,
        traced: true,
        stopped: 0,
    },
    Row {
        case: "a chain driven to exactly the published limit",
        configured: None,
        invocation_type: EVENT,
        hops: 16,
        one_chain: true,
        traced: true,
        stopped: 0,
    },
    Row {
        case: "a chain driven one hop past the published limit",
        configured: None,
        invocation_type: EVENT,
        hops: 17,
        one_chain: true,
        traced: true,
        stopped: 1,
    },
    Row {
        case: "a chain driven well past the limit, as the recorded case drives it",
        configured: Some(TERMINATE),
        invocation_type: EVENT,
        hops: 24,
        one_chain: true,
        traced: true,
        stopped: 8,
    },
    Row {
        case: "the same chain of synchronous invocations",
        configured: Some(TERMINATE),
        invocation_type: REQUEST_RESPONSE,
        hops: 20,
        one_chain: true,
        traced: true,
        stopped: 4,
    },
    Row {
        case: "a chain of dry runs, which invoke nothing",
        configured: Some(TERMINATE),
        invocation_type: DRY_RUN,
        hops: 24,
        one_chain: true,
        traced: true,
        stopped: 0,
    },
    Row {
        case: "the loop allowed, driven to the same depth",
        configured: Some(ALLOW),
        invocation_type: EVENT,
        hops: 24,
        one_chain: true,
        traced: true,
        stopped: 0,
    },
    Row {
        case: "the loop allowed, driven synchronously to the same depth",
        configured: Some(ALLOW),
        invocation_type: REQUEST_RESPONSE,
        hops: 20,
        one_chain: true,
        traced: true,
        stopped: 0,
    },
    Row {
        case: "as many hops, each its own chain",
        configured: Some(TERMINATE),
        invocation_type: EVENT,
        hops: 24,
        one_chain: false,
        traced: true,
        stopped: 0,
    },
    Row {
        case: "as many hops carrying no tracing header at all",
        configured: Some(TERMINATE),
        invocation_type: EVENT,
        hops: 24,
        one_chain: true,
        traced: false,
        stopped: 0,
    },
];

/// RULE: past the published number of invocations of one function in one request chain,
/// Lambda has detected a recursive loop — so a `Terminate` function has every further
/// invocation of that chain STOPPED and each stop published as one
/// `RecursiveInvocationsDropped` `Count` datapoint dimensioned on `FunctionName`, while an
/// `Allow` function has nothing stopped and no such series at all.
#[tokio::test]
async fn the_guardrail_stops_a_looping_chain_and_meters_every_invocation_it_drops() {
    for row in ROWS {
        // One account per row, so the only drops in the window are this row's.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let case = row.case;
        let function = "guarded";
        create_active_function(&router, &engine, function, &package_bytes(64)).await;
        if let Some(value) = row.configured {
            let put = put_recursion_config(&router, function, value).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: setting RecursiveLoop={value:?} must succeed, got {} {}",
                put.status,
                String::from_utf8_lossy(&put.body)
            );
        }

        let mut stopped_hops = 0u32;
        for hop in 1..=row.hops {
            let root = chain(if row.one_chain { 0 } else { hop });
            let answer = invoke(
                &router,
                function,
                row.invocation_type,
                row.traced.then_some(root.as_str()),
                format!(r#"{{"depth":{hop}}}"#).as_bytes(),
            )
            .await;
            // The hops past the limit are the stopped ones, and only for a configuration
            // that says to stop them.
            let must_stop = row.stopped > 0 && hop > row.hops - row.stopped;
            if must_stop {
                stopped_hops += 1;
            }
            let expected = match (must_stop, row.invocation_type) {
                (true, EVENT) => ACCEPTED,
                (true, _) => STOPPED_STATUS,
                (false, EVENT) => ACCEPTED,
                (false, DRY_RUN) => DRY_RUN_STATUS,
                // A synchronous invocation nothing stopped reaches the run itself. This
                // suite's package is not a readable archive, so the run is refused with
                // the modelled 502 rather than answered with an invented payload — what
                // matters here is that the guardrail let it through.
                (false, _) => RUN_REACHED_STATUS,
            };
            assert_eq!(
                answer.status,
                expected,
                "case {case:?}: hop {hop} of {} must answer {expected}, got {} {}",
                row.hops,
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            if must_stop && row.invocation_type != EVENT {
                assert!(
                    answer.error_code().contains(STOPPED_CODE),
                    "case {case:?}: hop {hop} must be refused as {STOPPED_CODE}, got {}",
                    answer.error_code()
                );
            }
        }
        assert_eq!(
            stopped_hops, row.stopped,
            "case {case:?}: the row must drive {} stopped hops, got {stopped_hops}",
            row.stopped
        );

        // The magnitude a client asserts on: the sum of the series over the window is the
        // number of invocations the guardrail dropped, and a row that dropped none has NO
        // datapoint rather than a zero.
        let points = dropped_datapoints(&router, case, function).await;
        let sum: f64 = points
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum();
        if row.stopped == 0 {
            assert!(
                points.is_empty(),
                "case {case:?}: nothing was stopped, so {DROPPED_SERIES} must have no \
                 datapoint at all, got {points:?}"
            );
        } else {
            assert_eq!(
                sum,
                f64::from(row.stopped),
                "case {case:?}: {DROPPED_SERIES} must sum to the {} invocations the \
                 guardrail dropped, got {sum} over {points:?}",
                row.stopped
            );
            assert!(
                points
                    .iter()
                    .all(|point| point["Unit"].as_str() == Some(COUNT_UNIT)),
                "case {case:?}: every datapoint must carry the unit {COUNT_UNIT:?}, got \
                 {points:?}"
            );
        }

        // An asynchronous invocation the guardrail stopped is accepted and NEVER delivered:
        // its entry records that the guardrail stopped it, and the served ones are queued
        // for delivery as before.
        if row.invocation_type == EVENT {
            let outcomes = accepted_outcomes(&engine, function);
            let stopped_reason =
                AsyncInvocationOutcome::Failed(ASYNC_RECURSION_STOPPED_REASON.to_string());
            let actually_stopped = outcomes
                .iter()
                .filter(|outcome| **outcome == stopped_reason)
                .count();
            assert_eq!(
                actually_stopped,
                usize::try_from(row.stopped).expect("the row's count fits"),
                "case {case:?}: {} of the accepted events must record that the guardrail \
                 stopped them, got {actually_stopped} of {outcomes:?}",
                row.stopped
            );
        }

        // Reading the series changes nothing a later reader sees.
        let again = dropped_sum(&router, case, function).await;
        assert_eq!(
            again, sum,
            "case {case:?}: a second read of {DROPPED_SERIES} must report {sum}, got {again}"
        );
    }
}

/// RULE (the discriminating half): the two configurations are told apart only by the
/// series, so two otherwise identical functions driven through the SAME chain to the same
/// depth must differ exactly there — the `Terminate` one carries the drops, and the `Allow`
/// one has no `RecursiveInvocationsDropped` series at all, an absence rather than a
/// zero-valued series, held for as long as the positive one has been readable.
#[tokio::test]
async fn an_allowed_loop_publishes_no_series_at_all_where_a_terminated_one_publishes_drops() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let case = "the twin arms of one chain";
    let depth = 24u32;

    for (function, configured) in [("terminated", TERMINATE), ("allowed", ALLOW)] {
        create_active_function(&router, &engine, function, &package_bytes(64)).await;
        let put = put_recursion_config(&router, function, configured).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: setting RecursiveLoop={configured:?} on {function:?} must \
             succeed, got {} {}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }

    // ONE chain, hitting both functions at every hop, so neither arm can differ by the
    // chain it was driven through.
    let root = chain(7);
    for hop in 1..=depth {
        for function in ["terminated", "allowed"] {
            let answer = invoke(
                &router,
                function,
                EVENT,
                Some(root.as_str()),
                format!(r#"{{"depth":{hop}}}"#).as_bytes(),
            )
            .await;
            assert_eq!(
                answer.status,
                ACCEPTED,
                "case {case:?}: hop {hop} of {function:?} must be accepted, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }

    let terminated = dropped_sum(&router, case, "terminated").await;
    assert!(
        terminated >= 1.0,
        "case {case:?}: the Terminate arm must publish at least one dropped invocation, got \
         {terminated}"
    );

    let allowed = dropped_datapoints(&router, case, "allowed").await;
    assert!(
        allowed.is_empty(),
        "case {case:?}: the Allow arm must have no {DROPPED_SERIES} datapoint at all, got \
         {allowed:?}"
    );
    let listed = series_names_of(&router, case, "allowed").await;
    assert!(
        !listed.iter().any(|name| name == DROPPED_SERIES),
        "case {case:?}: the Allow arm must have no {DROPPED_SERIES} series at all, got \
         {listed:?}"
    );
    let listed_terminated = series_names_of(&router, case, "terminated").await;
    assert!(
        listed_terminated.iter().any(|name| name == DROPPED_SERIES),
        "case {case:?}: the Terminate arm's {DROPPED_SERIES} series must be reported for its \
         own dimension, got {listed_terminated:?}"
    );

    // The absence is still an absence after the positive arm has been read.
    let allowed_again = dropped_datapoints(&router, case, "allowed").await;
    assert!(
        allowed_again.is_empty(),
        "case {case:?}: the Allow arm must still have no datapoint, got {allowed_again:?}"
    );
}

/// RULE (what is counted): a loop is one request coming back round into the SAME function,
/// so the count is per function — one chain that invokes two functions up to the limit each
/// loops in neither, while one more invocation of either is that function's first stopped
/// one.
#[tokio::test]
async fn the_count_is_kept_per_function_within_one_chain() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let case = "one chain across two functions";
    let root = chain(11);

    for function in ["first", "second"] {
        create_active_function(&router, &engine, function, &package_bytes(64)).await;
    }

    // The published limit of invocations of EACH function, in one chain: 32 hops, and no
    // loop in either.
    for hop in 1..=16 {
        for function in ["first", "second"] {
            let answer = invoke(
                &router,
                function,
                EVENT,
                Some(root.as_str()),
                format!(r#"{{"depth":{hop}}}"#).as_bytes(),
            )
            .await;
            assert_eq!(
                answer.status,
                ACCEPTED,
                "case {case:?}: hop {hop} of {function:?} must be accepted, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
    for function in ["first", "second"] {
        let points = dropped_datapoints(&router, case, function).await;
        assert!(
            points.is_empty(),
            "case {case:?}: {function:?} saw the limit and no more, so it must have no \
             {DROPPED_SERIES} datapoint, got {points:?}"
        );
    }

    // One more invocation of ONE of them is that function's first stopped invocation, and
    // the other function is untouched by it.
    let answer = invoke(
        &router,
        "first",
        REQUEST_RESPONSE,
        Some(root.as_str()),
        b"{}",
    )
    .await;
    assert_eq!(
        answer.status,
        STOPPED_STATUS,
        "case {case:?}: the 17th invocation of first must be stopped, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let first = dropped_sum(&router, case, "first").await;
    assert_eq!(
        first, 1.0,
        "case {case:?}: first must report exactly the one dropped invocation, got {first}"
    );
    let second = dropped_datapoints(&router, case, "second").await;
    assert!(
        second.is_empty(),
        "case {case:?}: second must still have no {DROPPED_SERIES} datapoint, got {second:?}"
    );
}

/// RULE (how long a chain lasts): a chain is live only while an invocation of it could
/// still make the next hop, so a chain whose last hop is further back than the longest one
/// invocation can run is over — a later request that carries the same id is a new chain and
/// is counted from the start, rather than inheriting a count no live chain holds.
#[tokio::test]
async fn a_chain_idle_past_the_longest_invocation_is_counted_as_a_new_one() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let case = "a chain idle past the longest invocation";
    let function = "aged";
    let root = chain(13);
    create_active_function(&router, &engine, function, &package_bytes(64)).await;

    for hop in 1..=16 {
        let answer = invoke(
            &router,
            function,
            EVENT,
            Some(root.as_str()),
            format!(r#"{{"depth":{hop}}}"#).as_bytes(),
        )
        .await;
        assert_eq!(
            answer.status,
            ACCEPTED,
            "case {case:?}: hop {hop} must be accepted, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // What waiting out the window would do, without waiting: the last hop of the chain is
    // moved further into the past than the longest a single invocation of a function can run
    // ("The maximum allowed value is 900 seconds", `FunctionConfiguration$Timeout`).
    {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let held = store
            .functions
            .get_mut(function)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {function:?}"));
        let aged = SystemTime::now() - Duration::from_secs(901);
        for chain in &mut held.request_chains {
            chain.last_seen = aged;
        }
    }

    // The 17th invocation of the id would have been stopped inside the window; after it, it
    // is the first hop of a new chain.
    let answer = invoke(
        &router,
        function,
        REQUEST_RESPONSE,
        Some(root.as_str()),
        b"{}",
    )
    .await;
    assert_eq!(
        answer.status, RUN_REACHED_STATUS,
        "case {case:?}: the invocation must reach the run rather than be stopped, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let points = dropped_datapoints(&router, case, function).await;
    assert!(
        points.is_empty(),
        "case {case:?}: nothing was stopped, so {DROPPED_SERIES} must have no datapoint, got \
         {points:?}"
    );
}
