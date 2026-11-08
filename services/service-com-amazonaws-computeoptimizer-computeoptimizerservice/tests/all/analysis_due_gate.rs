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
//! type it reports on, in whether it lists an account's analysis, rolls it up per resource
//! type or reports ONE named resource's, and so in how "no analysis yet" reaches a client —
//! an empty list from a lister, counts of zero from the roll-up, whose entries are the
//! resource types the service reports recommendations for and so exist as soon as the
//! account is enrolled, and an empty list of projections from a reader of one resource.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    reports_idle_recommendations, service_state, AnalysedResource, RdsDatabaseDetail,
    ANALYSIS_DELAY, SOURCE_TYPE_RDS_DB_INSTANCE,
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
    /// The reader ROLLS the account's analysis UP per resource type the service reports
    /// recommendations for, so its entries are that vocabulary whatever the analysis holds:
    /// while the analysis is not due it reports an entry per resource type with every count
    /// at zero, which is how a caller tells an enrolled account with nothing analysed apart
    /// from an account with no such surface.
    ZeroCounts(&'static str),
    /// The reader reports ONE named resource's analysis, so a resource with no analysis has
    /// no projection to report: its `recommendedOptionProjectedMetrics` is an optional list
    /// with no required member, so the answer is that list, empty — a successful response
    /// rather than a failure, which is how a caller tells "nothing analysed yet" apart from
    /// a request it got wrong.
    EmptyProjection,
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
/// resource of the store's own analysis.
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
        Reader {
            operation: "GetRecommendationSummaries",
            request: json!({}),
            no_analysis: NoAnalysis::ZeroCounts("recommendationSummaries"),
        },
        Reader {
            operation: "GetEC2RecommendationProjectedMetrics",
            request: json!({
                "instanceArn": instance_arn,
                "stat": "Maximum",
                "period": PERIOD,
                "startTime": WINDOW_START,
                "endTime": WINDOW_START + PERIOD,
            }),
            no_analysis: NoAnalysis::EmptyProjection,
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
            no_analysis: NoAnalysis::EmptyProjection,
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

/// Every count one roll-up entry carries is zero: the account has nothing analysed yet, so
/// each `Summary` count, each `IdleSummary` count of a resource type the idle analysis
/// covers, each `CurrentPerformanceRisk` rating and each saving it states is nothing — and
/// a resource type that analysis does not cover reports no idle member at all.
fn assert_counts_are_zero(case: &str, entry: &Value) {
    let resource_type = entry["recommendationResourceType"]
        .as_str()
        .unwrap_or_default();
    for count in entry["summaries"].as_array().cloned().unwrap_or_default() {
        assert_eq!(
            count["value"].as_f64(),
            Some(0.0),
            "case {case:?}/{resource_type}: nothing is analysed yet, so every finding counts \
             zero resources, got {count}"
        );
    }
    for rating in ["high", "medium", "low", "veryLow"] {
        assert_eq!(
            entry["currentPerformanceRiskRatings"][rating].as_f64(),
            Some(0.0),
            "case {case:?}/{resource_type}: nothing is analysed yet, so no resource runs the \
             {rating} risk, got {entry}"
        );
    }
    assert_eq!(
        entry["savingsOpportunity"]["estimatedMonthlySavings"]["value"].as_f64(),
        Some(0.0),
        "case {case:?}/{resource_type}: nothing is analysed yet, so there is nothing to save, \
         got {entry}"
    );
    let idle = entry["idleSummaries"].as_array().cloned();
    assert_eq!(
        idle.is_some(),
        reports_idle_recommendations(resource_type),
        "case {case:?}/{resource_type}: the idle counts are reported exactly where the idle \
         analysis covers the resource type, got {entry}"
    );
    for count in idle.unwrap_or_default() {
        assert_eq!(
            count["value"].as_f64(),
            Some(0.0),
            "case {case:?}/{resource_type}: nothing was found idle yet, so every idle finding \
             counts zero resources, got {count}"
        );
    }
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

/// The ARNs of two resources the store's analysis holds, as the endpoint in [`REGION`]
/// reports them: an Amazon EC2 instance the store observes, and an Amazon Aurora or RDS
/// database recorded here the way Compute Optimizer's own analysis of an observed workload
/// would record it — no operation of this API creates either.
///
/// Taken from the stored analysis rather than written out here, so these rows name whatever
/// that analysis holds rather than one memorised identifier.
fn analysed_resources(engine: &Arc<engine::InMemoryEngine>) -> (String, String) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    let instance = guard
        .instances
        .values()
        .next()
        .expect("the observed analysis holds an Amazon EC2 instance")
        .instance_arn(REGION);
    guard.analysed_resources.push(AnalysedResource {
        account_id: ::engine::emulator_account_id().to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        // The catalog records a resource's REGION-FREE identity; the ARN reported for it
        // names the region of the endpoint that reports it.
        arn_service: "rds".to_string(),
        arn_resource: "db:ledger-primary".to_string(),
        // An over-provisioned database: the analysis would move it to a smaller DB instance
        // class, which is what gives it a recommendation option to project metrics onto.
        finding: "Overprovisioned".to_string(),
        finding_reason_codes: Vec::new(),
        current_performance_risk: "Low".to_string(),
        current_monthly_cost: 200.0,
        recommended_monthly_cost: 150.0,
        inferred_workload_types: Vec::new(),
        rds_database: Some(RdsDatabaseDetail {
            engine: Some("postgres".to_string()),
            current_db_instance_class: Some("db.r6g.2xlarge".to_string()),
            recommended_db_instance_class: Some("db.r6g.xlarge".to_string()),
            ..RdsDatabaseDetail::default()
        }),
    });
    let database = guard
        .analysed_resources
        .iter()
        .find(|resource| resource.resource_type == SOURCE_TYPE_RDS_DB_INSTANCE)
        .expect("the findings catalog holds an Amazon Aurora or RDS database")
        .arn_in(REGION);
    (instance, database)
}

/// One enrollment, four phases, every reader of the account's analysis: never opted in reads
/// nothing at all; just opted in reads no analysis yet; once the analysis its enrollment
/// started is due the analysis is served; opted back out reads nothing again.
#[tokio::test]
async fn every_reader_of_the_analysis_waits_for_the_enrollment_and_the_analysis() {
    let (router, engine) = emulator_with_engine();
    let (instance_arn, database_arn) = analysed_resources(&engine);
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
            NoAnalysis::ZeroCounts(member) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an account with nothing analysed is not a failed request, \
                     expected 200, got {status}: {body}"
                );
                let rolled_up = items(&case, member, &body);
                assert!(
                    !rolled_up.is_empty(),
                    "case {case:?}: a roll-up of the analysis per resource type is reported for \
                     an enrolled account whether or not the analysis is due, got {body}"
                );
                for entry in &rolled_up {
                    assert_counts_are_zero(&case, entry);
                }
            }
            NoAnalysis::EmptyProjection => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a resource with nothing analysed is not a failed \
                     request, expected 200, got {status}: {body}"
                );
                assert_eq!(
                    items(&case, "recommendedOptionProjectedMetrics", &body),
                    Vec::<Value>::new(),
                    "case {case:?}: the analysis is not due yet, so there is nothing to \
                     project, got {body}"
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
            NoAnalysis::ZeroCounts(member) => {
                let rolled_up = items(&case, member, &body);
                assert!(
                    !rolled_up.is_empty(),
                    "case {case:?}: once the analysis is due {member} carries it, got {body}"
                );
                let counted: f64 = rolled_up
                    .iter()
                    .flat_map(|entry| entry["summaries"].as_array().cloned().unwrap_or_default())
                    .map(|count| count["value"].as_f64().unwrap_or_default())
                    .sum();
                assert!(
                    counted > 0.0,
                    "case {case:?}: once the analysis is due the roll-up counts it, got {body}"
                );
            }
            NoAnalysis::EmptyProjection => assert!(
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
/// the analysis is not due the resource is absent from its lister and its projected-metrics
/// reader reports no projection for it, and once it is due both report it. A reader that
/// answered from the store without the gate would disagree with its own lister.
#[tokio::test]
async fn the_two_readers_of_one_resource_agree_about_whether_its_analysis_exists() {
    let (router, engine) = emulator_with_engine();
    let (instance_arn, database_arn) = analysed_resources(&engine);
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
            status, 200,
            "case {case:?}: a resource whose analysis is not due is no failed request, got \
             {status}: {metrics}"
        );
        assert_eq!(
            items(&case, "recommendedOptionProjectedMetrics", &metrics),
            Vec::<Value>::new(),
            "case {case:?}: a resource whose analysis is not due has no projection, got \
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
