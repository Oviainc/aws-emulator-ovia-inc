// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the series a client reads to learn that a function is being invoked
//! at all: `Invocations`, read back where a client reads it — the `AWS/Lambda` series
//! CloudWatch answers for, with CloudWatch mounted beside Lambda on one engine.
//!
//! The RULE asserted over the table below: every invocation Lambda ADMITS to a function —
//! one that got past resolution, the function's lifecycle state, the payload limits and the
//! recursive-loop guardrail, whatever the execution it started then comes to — is published
//! as one `Invocations` datapoint of 1 in the `AWS/Lambda` namespace, counted in `Count`,
//! twice: once in the series dimensioned on the function (`FunctionName`) and once in the
//! series dimensioned on the function and the resource the call addressed (`FunctionName`
//! plus `Resource`, the function for an unqualified invocation and `name:qualifier` for an
//! alias or a published version). A request Lambda refuses before the function is reached is
//! no invocation of it and is counted in neither series, and so is the dry run, which
//! "validate[s] parameter values" and invokes nothing. Both series are therefore discoverable
//! through `ListMetrics`, which is how a client finds what it can alarm or chart on, and the
//! SUM over a window is the number of invocations in it.
//!
//! The rows differ along what the rule generalises over: the invocation surface
//! (`Invoke`, the deprecated `InvokeAsync`, `InvokeWithResponseStream`), the
//! `InvocationType` (synchronous, asynchronous, the dry run), the resource addressed (the
//! function, an explicit `$LATEST`, a published version through the `Qualifier` parameter, an
//! alias through the `name:qualifier` form of `FunctionName`), how many invocations are made
//! (one, two, three, four, five, and a mixture split across three resources), and whether the
//! invocation is admitted at all.
//!
//! Every metric, dimension and unit name here is written as a literal, taken from the AWS
//! Lambda metric reference rather than from this crate's constants, so a series published
//! under a name no AWS reader would ask for fails here instead of agreeing with itself. The
//! Lambda-side harness (the `Answer` wrapper, `CreateFunction` and the activation window) is
//! the one `function_lifecycle.rs` publishes, and the CloudWatch call is the one
//! `kinesis_event_source_delivery.rs` publishes.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{create_active_function, json_call, package_bytes, send, Answer};
use crate::kinesis_event_source_delivery::cloudwatch;

/// The namespace Lambda's own metrics live in (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";

/// The name that reference gives the number of times a function is invoked, and the unit it
/// counts them in.
const INVOCATIONS_SERIES: &str = "Invocations";
/// See [`INVOCATIONS_SERIES`].
const COUNT_UNIT: &str = "Count";

/// The dimensions that reference reports invocations per: the function, and the resource of
/// it an invocation addressed.
const FUNCTION_DIMENSION: &str = "FunctionName";
/// See [`FUNCTION_DIMENSION`].
const RESOURCE_DIMENSION: &str = "Resource";

/// The function every row invokes.
const FUNCTION: &str = "metered";

/// The alias every row can address, and the published version it names.
const ALIAS: &str = "blue";
/// See [`ALIAS`].
const VERSION: &str = "1";

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's own
/// series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds. Wide enough that the
/// minutes the invocations were published in are inside it however a row straddles a minute
/// boundary.
const WINDOW_SECS: f64 = 600.0;

/// Where `Invoke` is bound, from `smithy_query(query="operation", operation="Invoke")`
/// (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// Where `InvokeAsync` is bound (`POST /2014-11-13/functions/{FunctionName}/invoke-async`).
const INVOKE_ASYNC_URI: &str = "/2014-11-13/functions";

/// Where `InvokeWithResponseStream` is bound (`POST
/// /2021-11-15/functions/{FunctionName}/response-streaming-invocations`).
const RESPONSE_STREAMING_URI: &str = "/2021-11-15/functions";

/// The request header `Invoke$InvocationType` is bound to.
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";

/// The `InvocationType` values the rows drive, from the model's enum
/// (`Event | RequestResponse | DryRun`).
const EVENT: &str = "Event";
/// See [`EVENT`].
const REQUEST_RESPONSE: &str = "RequestResponse";
/// See [`EVENT`].
const DRY_RUN: &str = "DryRun";

/// The statuses an invocation answers: "For the Event invocation type, this status code is
/// 202", "For the DryRun invocation type, the status code is 204", and the 501 this emulator
/// answers for a run it has no in-process runtime for.
const ACCEPTED: u16 = 202;
/// See [`ACCEPTED`].
const DRY_RUN_STATUS: u16 = 204;
/// See [`ACCEPTED`].
const RUN_NOT_IMPLEMENTED: u16 = 501;

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

/// Now, in the epoch seconds a stored datapoint is stamped with.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Which resource of the function a call addresses, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// `FunctionName` alone, with no qualifier anywhere: the function itself.
    Function,
    /// The `Qualifier` parameter naming the unpublished version the function is addressed
    /// by, explicitly.
    LatestQualifier,
    /// The `Qualifier` parameter naming the published version [`VERSION`].
    VersionQualifier,
    /// The `name:qualifier` form of `FunctionName`, naming the alias [`ALIAS`].
    AliasInName,
}

impl Target {
    /// The `FunctionName` path label a call to this target carries.
    fn reference(self) -> String {
        match self {
            Self::AliasInName => format!("{FUNCTION}:{ALIAS}"),
            _ => FUNCTION.to_string(),
        }
    }

    /// The query string a call to this target carries.
    fn query(self) -> &'static str {
        match self {
            Self::LatestQualifier => "?Qualifier=$LATEST",
            Self::VersionQualifier => "?Qualifier=1",
            _ => "",
        }
    }
}

/// Which invocation surface a call drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    /// `Invoke`.
    Invoke,
    /// `InvokeAsync`, which takes no `InvocationType` at all.
    InvokeAsync,
    /// `InvokeWithResponseStream`.
    ResponseStream,
}

/// One group of identical calls a row makes.
#[derive(Clone, Copy, Debug)]
struct Calls {
    surface: Surface,
    /// The `InvocationType` the call carries, ignored by [`Surface::InvokeAsync`].
    invocation_type: &'static str,
    target: Target,
    /// How many of these calls are made.
    times: u32,
}

/// One row: the calls it makes, and the invocations each series must then report.
struct Row {
    case: &'static str,
    calls: &'static [Calls],
    /// What the series dimensioned on the function alone must sum to.
    function_sum: f64,
    /// What the series of each addressed resource must sum to: `(Resource, sum)`.
    resource_sums: &'static [(&'static str, f64)],
}

/// The rows. Every expected count is written out rather than computed from the calls, so a
/// row that publishes once per CALL and one that publishes once per INVOCATION cannot both
/// pass.
const ROWS: &[Row] = &[
    Row {
        case: "one unqualified synchronous invocation",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: REQUEST_RESPONSE,
            target: Target::Function,
            times: 1,
        }],
        function_sum: 1.0,
        resource_sums: &[("metered", 1.0)],
    },
    Row {
        case: "five unqualified synchronous invocations, the magnitude the recorded case drives",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: REQUEST_RESPONSE,
            target: Target::Function,
            times: 5,
        }],
        function_sum: 5.0,
        resource_sums: &[("metered", 5.0)],
    },
    Row {
        case: "three asynchronous invocations",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: EVENT,
            target: Target::Function,
            times: 3,
        }],
        function_sum: 3.0,
        resource_sums: &[("metered", 3.0)],
    },
    Row {
        case: "an explicit $LATEST qualifier, which addresses the function itself",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: REQUEST_RESPONSE,
            target: Target::LatestQualifier,
            times: 2,
        }],
        function_sum: 2.0,
        resource_sums: &[("metered", 2.0)],
    },
    Row {
        case: "a published version addressed by the Qualifier parameter",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: REQUEST_RESPONSE,
            target: Target::VersionQualifier,
            times: 4,
        }],
        function_sum: 4.0,
        resource_sums: &[("metered:1", 4.0)],
    },
    Row {
        case: "an alias addressed in the name:qualifier form",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: EVENT,
            target: Target::AliasInName,
            times: 2,
        }],
        function_sum: 2.0,
        resource_sums: &[("metered:blue", 2.0)],
    },
    Row {
        case: "invocations split across the function and two of its qualifiers",
        calls: &[
            Calls {
                surface: Surface::Invoke,
                invocation_type: REQUEST_RESPONSE,
                target: Target::Function,
                times: 1,
            },
            Calls {
                surface: Surface::Invoke,
                invocation_type: REQUEST_RESPONSE,
                target: Target::VersionQualifier,
                times: 2,
            },
            Calls {
                surface: Surface::Invoke,
                invocation_type: EVENT,
                target: Target::AliasInName,
                times: 3,
            },
        ],
        function_sum: 6.0,
        resource_sums: &[("metered", 1.0), ("metered:1", 2.0), ("metered:blue", 3.0)],
    },
    Row {
        case: "the deprecated asynchronous invocation surface",
        calls: &[Calls {
            surface: Surface::InvokeAsync,
            invocation_type: EVENT,
            target: Target::Function,
            times: 3,
        }],
        function_sum: 3.0,
        resource_sums: &[("metered", 3.0)],
    },
    Row {
        case: "response-streaming invocations",
        calls: &[Calls {
            surface: Surface::ResponseStream,
            invocation_type: REQUEST_RESPONSE,
            target: Target::Function,
            times: 2,
        }],
        function_sum: 2.0,
        resource_sums: &[("metered", 2.0)],
    },
    Row {
        case: "dry runs, which invoke nothing",
        calls: &[Calls {
            surface: Surface::Invoke,
            invocation_type: DRY_RUN,
            target: Target::Function,
            times: 4,
        }],
        function_sum: 0.0,
        resource_sums: &[],
    },
    Row {
        case: "response-streaming dry runs, which invoke nothing either",
        calls: &[Calls {
            surface: Surface::ResponseStream,
            invocation_type: DRY_RUN,
            target: Target::VersionQualifier,
            times: 3,
        }],
        function_sum: 0.0,
        resource_sums: &[],
    },
    Row {
        case: "every surface at once, beside dry runs that count for nothing",
        calls: &[
            Calls {
                surface: Surface::Invoke,
                invocation_type: REQUEST_RESPONSE,
                target: Target::Function,
                times: 1,
            },
            Calls {
                surface: Surface::Invoke,
                invocation_type: EVENT,
                target: Target::Function,
                times: 1,
            },
            Calls {
                surface: Surface::InvokeAsync,
                invocation_type: EVENT,
                target: Target::Function,
                times: 1,
            },
            Calls {
                surface: Surface::ResponseStream,
                invocation_type: REQUEST_RESPONSE,
                target: Target::Function,
                times: 1,
            },
            Calls {
                surface: Surface::Invoke,
                invocation_type: DRY_RUN,
                target: Target::Function,
                times: 2,
            },
        ],
        function_sum: 4.0,
        resource_sums: &[("metered", 4.0)],
    },
];

/// Create the function every row invokes, bring it to `Active`, publish version [`VERSION`]
/// of it and give it the alias [`ALIAS`], so every resource form is addressable.
async fn create_function_with_qualifiers(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) {
    create_active_function(router, engine, FUNCTION, &package_bytes(64)).await;
    let published = json_call(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{FUNCTION}/versions"),
        &json!({}),
    )
    .await;
    assert_eq!(
        published.status,
        201,
        "case {case:?}: PublishVersion must succeed, got {} {}",
        published.status,
        String::from_utf8_lossy(&published.body)
    );
    let alias = json_call(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{FUNCTION}/aliases"),
        &json!({ "Name": ALIAS, "FunctionVersion": VERSION }),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "case {case:?}: CreateAlias must succeed, got {} {}",
        alias.status,
        String::from_utf8_lossy(&alias.body)
    );
}

/// Drive one call of `group` and answer what it replied.
async fn drive(router: &MultiServiceRouter, group: &Calls) -> Answer {
    let reference = group.target.reference();
    let query = group.target.query();
    match group.surface {
        Surface::Invoke => {
            send(
                router,
                "POST",
                &format!("{INVOCATIONS_URI}/{reference}/invocations{query}"),
                "application/octet-stream",
                b"{}".to_vec(),
                &[(INVOCATION_TYPE_HEADER, group.invocation_type.to_string())],
            )
            .await
        }
        Surface::InvokeAsync => {
            send(
                router,
                "POST",
                &format!("{INVOKE_ASYNC_URI}/{reference}/invoke-async"),
                "application/octet-stream",
                b"{}".to_vec(),
                &[],
            )
            .await
        }
        Surface::ResponseStream => {
            send(
                router,
                "POST",
                &format!("{RESPONSE_STREAMING_URI}/{reference}/response-streaming-invocations{query}"),
                "application/octet-stream",
                b"{}".to_vec(),
                &[(INVOCATION_TYPE_HEADER, group.invocation_type.to_string())],
            )
            .await
        }
    }
}

/// The status a call of `group` must answer: the accepted status for an asynchronous
/// invocation, the dry-run status for one that invokes nothing, and this emulator's refusal
/// of the run itself for a synchronous one.
fn expected_status(group: &Calls) -> u16 {
    match (group.surface, group.invocation_type) {
        (Surface::InvokeAsync, _) => ACCEPTED,
        (_, EVENT) => ACCEPTED,
        (_, DRY_RUN) => DRY_RUN_STATUS,
        _ => RUN_NOT_IMPLEMENTED,
    }
}

/// The `Datapoints` `GetMetricStatistics` answers for the [`INVOCATIONS_SERIES`] series the
/// dimensions `dimensions` identify, selected by the unit the AWS Lambda metric reference
/// gives it.
async fn invocation_datapoints(
    router: &MultiServiceRouter,
    case: &str,
    dimensions: &Value,
) -> Vec<Value> {
    let now = now_secs();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": INVOCATIONS_SERIES,
            "Dimensions": dimensions,
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
        "case {case:?}: GetMetricStatistics({INVOCATIONS_SERIES}, {dimensions}) answered \
         {status} {stats}"
    );
    assert_eq!(
        stats["Label"].as_str(),
        Some(INVOCATIONS_SERIES),
        "case {case:?}: the answer must be labelled {INVOCATIONS_SERIES:?}, got {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The invocations the series `dimensions` identify reports over the whole window: the SUM,
/// which is the magnitude a client asserts on.
async fn invocation_sum(router: &MultiServiceRouter, case: &str, dimensions: &Value) -> f64 {
    invocation_datapoints(router, case, dimensions)
        .await
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// The dimension set of the series reported per function.
fn function_dimensions() -> Value {
    json!([{ "Name": FUNCTION_DIMENSION, "Value": FUNCTION }])
}

/// The dimension set of the series reported per invoked resource.
fn resource_dimensions(resource: &str) -> Value {
    json!([
        { "Name": FUNCTION_DIMENSION, "Value": FUNCTION },
        { "Name": RESOURCE_DIMENSION, "Value": resource },
    ])
}

/// Every series `ListMetrics` discovers in [`NAMESPACE`], as `(MetricName, dimension pairs)`
/// with the dimensions ordered by name — which is how a client enumerating what it can alarm
/// on reads them.
async fn listed_series(
    router: &MultiServiceRouter,
    case: &str,
) -> Vec<(String, Vec<(String, String)>)> {
    let (status, listed) = cloudwatch(
        router,
        "ListMetrics",
        &json!({ "Namespace": NAMESPACE }),
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
        .map(|series| {
            let mut dimensions: Vec<(String, String)> = series["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|dimension| {
                    (
                        dimension["Name"].as_str().unwrap_or_default().to_string(),
                        dimension["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            dimensions.sort();
            (
                series["MetricName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                dimensions,
            )
        })
        .collect()
}

/// RULE: every invocation Lambda admits to a function is published as one `Invocations`
/// `Count` datapoint in BOTH the series dimensioned on the function and the series
/// dimensioned on the function and the resource the call addressed; a call that invokes
/// nothing is published in neither.
#[tokio::test]
async fn every_admitted_invocation_is_counted_in_the_function_and_the_resource_series() {
    for row in ROWS {
        // One account per row, so the only invocations in the window are this row's.
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        let case = row.case;
        create_function_with_qualifiers(&router, &engine, case).await;

        for group in row.calls {
            for nth in 1..=group.times {
                let answer = drive(&router, group).await;
                let expected = expected_status(group);
                assert_eq!(
                    answer.status,
                    expected,
                    "case {case:?}: call {nth} of {group:?} must answer {expected}, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }
        }

        // The function's own series carries every invocation of it, whichever resource each
        // one addressed.
        let dimensions = function_dimensions();
        let points = invocation_datapoints(&router, case, &dimensions).await;
        let sum: f64 = points
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum();
        if row.function_sum == 0.0 {
            assert!(
                points.is_empty(),
                "case {case:?}: nothing was invoked, so {INVOCATIONS_SERIES} must have no \
                 datapoint at all, got {points:?}"
            );
        } else {
            assert_eq!(
                sum, row.function_sum,
                "case {case:?}: {INVOCATIONS_SERIES} of the function must sum to {}, got {sum} \
                 over {points:?}",
                row.function_sum
            );
            assert!(
                points
                    .iter()
                    .all(|point| point["Unit"].as_str() == Some(COUNT_UNIT)),
                "case {case:?}: every datapoint must carry the unit {COUNT_UNIT:?}, got \
                 {points:?}"
            );
        }

        // And each addressed resource's series carries the invocations of that resource.
        for (resource, expected) in row.resource_sums {
            let dimensions = resource_dimensions(resource);
            let resource_sum = invocation_sum(&router, case, &dimensions).await;
            assert_eq!(
                resource_sum, *expected,
                "case {case:?}: {INVOCATIONS_SERIES} of {resource:?} must sum to {expected}, \
                 got {resource_sum}"
            );
        }

        // Both series are DISCOVERABLE, which is the read a client makes before alarming on
        // them, and a row that invoked nothing registers neither.
        let listed = listed_series(&router, case).await;
        let function_series = (
            INVOCATIONS_SERIES.to_string(),
            vec![(FUNCTION_DIMENSION.to_string(), FUNCTION.to_string())],
        );
        if row.function_sum == 0.0 {
            assert!(
                !listed.iter().any(|series| series.0 == INVOCATIONS_SERIES),
                "case {case:?}: nothing was invoked, so no {INVOCATIONS_SERIES} series must be \
                 discoverable, got {listed:?}"
            );
        } else {
            assert!(
                listed.contains(&function_series),
                "case {case:?}: ListMetrics must discover {function_series:?}, got {listed:?}"
            );
            for (resource, _) in row.resource_sums {
                let expected = (
                    INVOCATIONS_SERIES.to_string(),
                    vec![
                        (FUNCTION_DIMENSION.to_string(), FUNCTION.to_string()),
                        (RESOURCE_DIMENSION.to_string(), (*resource).to_string()),
                    ],
                );
                assert!(
                    listed.contains(&expected),
                    "case {case:?}: ListMetrics must discover {expected:?}, got {listed:?}"
                );
            }
        }

        // Reading the series changes nothing a later reader sees.
        let again = invocation_sum(&router, case, &function_dimensions()).await;
        assert_eq!(
            again, sum,
            "case {case:?}: a second read of {INVOCATIONS_SERIES} must report {sum}, got {again}"
        );
    }
}

/// One refusal row: a request Lambda turns down before the function is reached, and the
/// status it turns it down with.
struct RefusedRow {
    case: &'static str,
    /// The `FunctionName` path label the request carries.
    reference: &'static str,
    /// The body the request carries.
    body: &'static [u8],
    status: u16,
}

/// The refusals: a name no function answers to, a qualifier the function does not hold, and
/// a body that is not the JSON an invocation's input must be.
const REFUSED: &[RefusedRow] = &[
    RefusedRow {
        case: "a function name nothing resolves to",
        reference: "absent",
        body: b"{}",
        status: 404,
    },
    RefusedRow {
        case: "a qualifier the function does not hold",
        reference: "metered:9",
        body: b"{}",
        status: 404,
    },
    RefusedRow {
        case: "a payload that is not JSON",
        reference: "metered",
        body: b"not json",
        status: 400,
    },
];

/// RULE (the other half): `Invocations` counts the invocations that REACHED the function, so
/// a request refused before it did is counted in neither series — and a function nobody
/// invoked has no `Invocations` series at all rather than a zero-valued one.
#[tokio::test]
async fn a_request_refused_before_the_function_is_never_counted_as_an_invocation() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let case = "refusals before the function";
    create_function_with_qualifiers(&router, &engine, case).await;

    // A function nobody has invoked yet: an absence, not a zero.
    let before = invocation_datapoints(&router, case, &function_dimensions()).await;
    assert!(
        before.is_empty(),
        "case {case:?}: a function nobody invoked must have no {INVOCATIONS_SERIES} datapoint, \
         got {before:?}"
    );
    let listed = listed_series(&router, case).await;
    assert!(
        !listed.iter().any(|series| series.0 == INVOCATIONS_SERIES),
        "case {case:?}: a function nobody invoked must have no {INVOCATIONS_SERIES} series, got \
         {listed:?}"
    );

    for row in REFUSED {
        let answer = send(
            &router,
            "POST",
            &format!("{INVOCATIONS_URI}/{}/invocations", row.reference),
            "application/octet-stream",
            row.body.to_vec(),
            &[(INVOCATION_TYPE_HEADER, REQUEST_RESPONSE.to_string())],
        )
        .await;
        assert_eq!(
            answer.status,
            row.status,
            "case {:?}: the request must be refused with {}, got {} {}",
            row.case,
            row.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let points = invocation_datapoints(&router, row.case, &function_dimensions()).await;
        assert!(
            points.is_empty(),
            "case {:?}: a refused request is no invocation, so {INVOCATIONS_SERIES} must still \
             have no datapoint, got {points:?}",
            row.case
        );
    }

    // One admitted invocation after them, so the absences above are the refusals' and not the
    // harness's.
    let answer = send(
        &router,
        "POST",
        &format!("{INVOCATIONS_URI}/{FUNCTION}/invocations"),
        "application/octet-stream",
        b"{}".to_vec(),
        &[(INVOCATION_TYPE_HEADER, EVENT.to_string())],
    )
    .await;
    assert_eq!(
        answer.status,
        ACCEPTED,
        "case {case:?}: the invocation must be accepted, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let sum = invocation_sum(&router, case, &function_dimensions()).await;
    assert_eq!(
        sum, 1.0,
        "case {case:?}: the one admitted invocation must be the only one counted, got {sum}"
    );
}
