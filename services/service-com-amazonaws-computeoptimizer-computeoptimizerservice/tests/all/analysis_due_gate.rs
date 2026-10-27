// SPDX-License-Identifier: CC0-1.0

//! The visibility gate every reader of the account's analysis applies, end to end over the
//! wire.
//!
//! Compute Optimizer reports an analysis of an account's resources only to an ENROLLED
//! account, and only once the analysis that enrollment starts is due: an account that never
//! opted in reads nothing at all, one that has JUST opted in reads no analysis yet — rather
//! than one nothing has produced — and an account that opted back out reads nothing again.
//!
//! The rule is about the ANALYSIS, not about one reader, so what is asserted here is the
//! whole class of readers against it: every reader of this service's stored analysis is
//! driven through the same four phases of one enrollment, each row differing in the resource
//! type it reports on, in whether it lists an account's analysis or reports ONE named
//! resource's, and so in how "no analysis yet" reaches a client — an empty list from a
//! lister, the modelled `ResourceNotFoundException` from a reader of one resource.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, ANALYSIS_DELAY, SOURCE_TYPE_RDS_DB_INSTANCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator_with_engine, error_code, opt_in, REGION};

/// The first data point the projected-metrics rows ask for, in epoch seconds, and the
/// granularity and length of their window: one hourly data point.
const WINDOW_START: i64 = 1_761_000_000;
/// See [`WINDOW_START`].
const PERIOD: i64 = 3_600;

/// How a reader answers while the account's analysis does not exist yet.
#[derive(Debug, Clone, Copy)]
enum NoAnalysis {
    /// The reader LISTS an account's analysis, so it reports the empty list under this
    /// member — an account with nothing analysed is not a failed request.
    EmptyList(&'static str),
    /// The reader reports ONE named resource's analysis and models no per-resource error
    /// list, so a resource with no analysis is "a resource that is required for the action
    /// doesn't exist" (HTTP 404 `ResourceNotFoundException`).
    NotFound,
}

/// One reader of the account's analysis: the operation, the request that drives it, and how
/// it answers before the analysis is due.
#[derive(Debug, Clone)]
struct Reader {
    operation: &'static str,
    request: Value,
    no_analysis: NoAnalysis,
}

/// Every reader of this service's stored analysis, with the request that drives it.
///
/// The listers are the ones `tests/all/timestamp_probe.rs` reads one analysis through, plus
/// the two the findings catalog answers (`GetRDSDatabaseRecommendations` and the summary
/// reader); the per-resource rows are the two projected-metrics readers, each naming a
/// resource of the store's own seeded analysis.
fn readers(instance_arn: &str, database_arn: &str) -> Vec<Reader> {
    let lister = |operation: &'static str, items: &'static str| Reader {
        operation,
        request: json!({}),
        no_analysis: NoAnalysis::EmptyList(items),
    };
    vec![
        lister("GetEC2InstanceRecommendations", "instanceRecommendations"),
        lister(
            "GetAutoScalingGroupRecommendations",
            "autoScalingGroupRecommendations",
        ),
        lister("GetEBSVolumeRecommendations", "volumeRecommendations"),
        lister(
            "GetLambdaFunctionRecommendations",
            "lambdaFunctionRecommendations",
        ),
        lister("GetLicenseRecommendations", "licenseRecommendations"),
        lister("GetIdleRecommendations", "idleRecommendations"),
        lister("GetRDSDatabaseRecommendations", "rdsDBRecommendations"),
        lister("GetRecommendationSummaries", "recommendationSummaries"),
        Reader {
            operation: "GetEC2RecommendationProjectedMetrics",
            request: json!({
                "instanceArn": instance_arn,
                "stat": "Maximum",
                "period": PERIOD,
                "startTime": WINDOW_START,
                "endTime": WINDOW_START + PERIOD,
            }),
            no_analysis: NoAnalysis::NotFound,
        },
        Reader {
            operation: "GetRDSDatabaseRecommendationProjectedMetrics",
            request: json!({
                "resourceArn": database_arn,
                "stat": "Maximum",
                "period": PERIOD,
                "startTime": WINDOW_START,
                "endTime": WINDOW_START + PERIOD,
            }),
            no_analysis: NoAnalysis::NotFound,
        },
    ]
}

/// The list a lister reports, as a list.
fn items(case: &str, member: &str, body: &Value) -> Vec<Value> {
    body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be a list, got {body}"))
        .clone()
}

/// Every reader is refused with the modelled `OptInRequiredException` (HTTP 403).
async fn assert_opt_in_required(router: &MultiServiceRouter, phase: &str, readers: &[Reader]) {
    for reader in readers {
        let case = format!("{phase}/{}", reader.operation);
        let (status, body) = call(router, reader.operation, reader.request.clone()).await;
        assert_eq!(
            status, 403,
            "case {case:?}: an account that is not opted in reads no analysis, expected 403, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "OptInRequiredException",
            "case {case:?}: expected OptInRequiredException, got {body}"
        );
    }
}

/// The ARNs of two resources the store's own seeded analysis holds: an Amazon EC2 instance
/// and an Amazon Aurora or RDS database, as the endpoint in [`REGION`] reports them.
///
/// Taken from the stored analysis rather than written out here, so these rows name whatever
/// the seeded analysis holds rather than one memorised identifier.
fn seeded_resources(engine: &Arc<engine::InMemoryEngine>) -> (String, String) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let guard = state.lock();
    let instance = guard
        .instances
        .values()
        .next()
        .expect("the seeded analysis holds an Amazon EC2 instance")
        .instance_arn(REGION);
    let database = guard
        .analysed_resources
        .iter()
        .find(|resource| resource.resource_type == SOURCE_TYPE_RDS_DB_INSTANCE)
        .expect("the seeded findings catalog holds an Amazon Aurora or RDS database")
        .arn_in(REGION);
    (instance, database)
}

/// One enrollment, four phases, every reader of the account's analysis: never opted in reads
/// nothing at all; just opted in reads no analysis yet; once the analysis its enrollment
/// started is due the analysis is served; opted back out reads nothing again.
#[tokio::test]
async fn every_reader_of_the_analysis_waits_for_the_enrollment_and_the_analysis() {
    let (router, engine) = emulator_with_engine();
    let (instance_arn, database_arn) = seeded_resources(&engine);
    let readers = readers(&instance_arn, &database_arn);

    assert_opt_in_required(&router, "never opted in", &readers).await;

    opt_in(&router).await;
    for reader in &readers {
        let case = format!("just opted in/{}", reader.operation);
        let (status, body) = call(&router, reader.operation, reader.request.clone()).await;
        match reader.no_analysis {
            NoAnalysis::EmptyList(member) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an account with nothing analysed is not a failed request, \
                     expected 200, got {status}: {body}"
                );
                assert_eq!(
                    items(&case, member, &body),
                    Vec::<Value>::new(),
                    "case {case:?}: the analysis is not due yet, so {member} is empty, got \
                     {body}"
                );
            }
            NoAnalysis::NotFound => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: the analysis is not due yet, so the resource has none, \
                     expected 404, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
            }
        }
    }

    // The analysis comes due one span after the enrollment record was written
    // (`ANALYSIS_DELAY`); the extra margin covers scheduler jitter.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    for reader in &readers {
        let case = format!("the analysis is due/{}", reader.operation);
        let (status, body) = call(&router, reader.operation, reader.request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the analysis is due it is served, expected 200, got {status}: \
             {body}"
        );
        match reader.no_analysis {
            NoAnalysis::EmptyList(member) => assert!(
                !items(&case, member, &body).is_empty(),
                "case {case:?}: once the analysis is due {member} carries it, got {body}"
            ),
            NoAnalysis::NotFound => assert!(
                !items(&case, "recommendedOptionProjectedMetrics", &body).is_empty(),
                "case {case:?}: once the analysis is due the projection is served, got {body}"
            ),
        }
    }

    // The gate is the STORED enrollment, not whether the account ever enrolled: opting back
    // out takes the analysis away again, however long ago the analysis came due.
    let (status, body) = call(
        &router,
        "UpdateEnrollmentStatus",
        json!({"status": "Inactive"}),
    )
    .await;
    assert_eq!(status, 200, "opting out: expected 200, got {status}: {body}");
    assert_opt_in_required(&router, "opted back out", &readers).await;
}

/// The two readers of ONE analysed resource agree about whether its analysis exists: while
/// the analysis is not due the resource is missing to the projected-metrics reader and absent
/// from its lister, and once it is due both report it. A reader that answered from the store
/// without the gate would disagree with its own lister.
#[tokio::test]
async fn the_two_readers_of_one_resource_agree_about_whether_its_analysis_exists() {
    let (router, engine) = emulator_with_engine();
    let (instance_arn, database_arn) = seeded_resources(&engine);
    let all = readers(&instance_arn, &database_arn);

    // (the resource's ARN, the lister of its analysis, the member it lists, the member the
    // list reports an ARN under, the projected-metrics reader of that one resource)
    let pairs = [
        (
            &instance_arn,
            "GetEC2InstanceRecommendations",
            "instanceRecommendations",
            "instanceArn",
            "GetEC2RecommendationProjectedMetrics",
        ),
        (
            &database_arn,
            "GetRDSDatabaseRecommendations",
            "rdsDBRecommendations",
            "resourceArn",
            "GetRDSDatabaseRecommendationProjectedMetrics",
        ),
    ];

    opt_in(&router).await;
    for (arn, lister, member, identifier, projected) in pairs {
        let case = format!("just opted in/{projected}");
        let body = {
            let (status, body) = call(&router, lister, json!({})).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {body}");
            body
        };
        assert!(
            !items(&case, member, &body)
                .iter()
                .any(|entry| entry[identifier] == json!(arn)),
            "case {case:?}: the analysis is not due, so {arn} is in no list, got {body}"
        );
        let request = all
            .iter()
            .find(|reader| reader.operation == projected)
            .expect("the table holds this reader")
            .request
            .clone();
        let (status, metrics) = call(&router, projected, request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a resource whose analysis is not due is missing, got {status}: \
             {metrics}"
        );
    }

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    for (arn, lister, member, identifier, projected) in pairs {
        let case = format!("the analysis is due/{projected}");
        let body = {
            let (status, body) = call(&router, lister, json!({})).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {body}");
            body
        };
        assert!(
            items(&case, member, &body)
                .iter()
                .any(|entry| entry[identifier] == json!(arn)),
            "case {case:?}: once the analysis is due {arn} is listed, got {body}"
        );
        let request = all
            .iter()
            .find(|reader| reader.operation == projected)
            .expect("the table holds this reader")
            .request
            .clone();
        let (status, metrics) = call(&router, projected, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a listed resource's projection is served, got {status}: {metrics}"
        );
    }
}
