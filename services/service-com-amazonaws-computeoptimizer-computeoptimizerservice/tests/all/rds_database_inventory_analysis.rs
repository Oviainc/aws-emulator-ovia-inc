// SPDX-License-Identifier: CC0-1.0

//! `GetRDSDatabaseRecommendations` over the account's OWN Amazon RDS inventory, end to end.
//!
//! Compute Optimizer "generates recommendations for Amazon Aurora and RDS databases that meet
//! a specific set of requirements": a database is a DB instance the account OWNS, and a
//! finding is computed from the utilization accumulated for it. So the rule this suite asserts
//! is that a recommendation exists for exactly the DB instances the Amazon RDS emulator holds
//! for this account, in this region, whose engine is up and for which utilization has been
//! published — and that every member of one is computed from that instance's own stored
//! configuration and those observations. An account that owns no DB instance reads the empty
//! list.
//!
//! Compute Optimizer, Amazon RDS and CloudWatch are mounted on ONE engine, exactly as
//! `core/router` mounts them, and every call goes through the front door a client uses: the
//! Compute Optimizer calls as awsJson `POST /` with `X-Amz-Target`, the Amazon RDS calls as
//! the `awsQuery` form posts an SDK sends. Each DB instance is read back through Amazon RDS's
//! own `DescribeDBInstances` before it is analysed, so what the analysis reads is a resource
//! the owning service itself reports.
//!
//! The `AWS/RDS` utilization is seeded into the metric store the way Amazon RDS writes it: a
//! client cannot publish into the AWS-reserved namespace space, so a suite that needs observed
//! utilization records it there and then reads the recommendation computed from it over the
//! wire, exactly as the Amazon ECS suites of this crate do.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ANALYSIS_DELAY, EXPORT_JOB_STEP, HOURS_PER_MONTH, RDS_PRICE_PER_VCPU_HOUR,
    RECOMMENDATION_HEADROOM, SOURCE_TYPE_RDS_DB_INSTANCE, UTILIZATION_PERCENTAGE_CEILING,
};
use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in_region, call_rds, call_rds_in, cell, create_bucket, csv_rows,
    emulator_with_analysed_databases, export_destination, get_object, metrics, opt_in,
    seed_cloudwatch_series, strings, ACCOUNT, REGION,
};

/// Enough slack over one of the services' own spans for the clock to have made it due
/// whatever the test scheduler is doing; the spans themselves are the services'.
const SLACK: Duration = Duration::from_millis(200);

/// The CloudWatch namespace Amazon RDS publishes a DB instance's utilization in, and the
/// dimension that identifies one instance's series in it (the Amazon RDS metrics reference).
const RDS_NAMESPACE: &str = "AWS/RDS";
/// See [`RDS_NAMESPACE`].
const DB_INSTANCE_DIMENSION: &str = "DBInstanceIdentifier";

/// Bytes in one GiB: `AllocatedStorage` is in GiB and `FreeStorageSpace` in bytes.
const BYTES_PER_GIB: f64 = 1_073_741_824.0;

/// One DB instance ARN as Amazon RDS mints it:
/// `arn:aws:rds:{region}:{accountId}:db:{resourceName}`.
fn db_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:{identifier}")
}

/// What one vCPU of a DB instance costs per month, from the price this analysis states.
fn monthly_cost_per_vcpu() -> f64 {
    RDS_PRICE_PER_VCPU_HOUR * HOURS_PER_MONTH
}

/// Whether two money or percentage figures agree to the hundredth every reported figure is
/// rounded to.
fn close(left: f64, right: f64) -> bool {
    (left - right).abs() < 0.01
}

/// Create one DB instance over Amazon RDS's own wire, then read it back through
/// `DescribeDBInstances` — which both proves the owning service holds it and lets its own
/// clock settle the `creating` status the create answered.
async fn create_db_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    engine: &str,
    class: &str,
    multi_az: bool,
    allocated_storage: i32,
) {
    let params = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("DBInstanceClass", class.to_string()),
        ("MultiAZ", multi_az.to_string()),
        ("AllocatedStorage", allocated_storage.to_string()),
    ];
    let (status, body) = call_rds_in(router, region, "CreateDBInstance", &params).await;
    assert_eq!(
        status, 200,
        "creating {identifier}: expected 200, got {status}: {body}"
    );
}

/// Read one DB instance back through Amazon RDS's own `DescribeDBInstances`, answering the
/// response document: the resource the analysis reports on is a resource its owner reports.
async fn describe_db_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> String {
    let (status, body) = call_rds_in(
        router,
        region,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", identifier.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "describing {identifier}: expected 200, got {status}: {body}"
    );
    body
}

/// The text of the first `<tag>` element of an `awsQuery` response document.
fn element(document: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = document.find(&open).map(|at| at + open.len()) else {
        return String::new();
    };
    let rest = &document[start..];
    let Some(end) = rest.find(&close) else {
        return String::new();
    };
    rest[..end].to_string()
}

/// Publish `values` of one `AWS/RDS` series of one DB instance, one hour ago — inside the
/// look-back window every read below analyses.
fn publish(
    engine: &Arc<engine::InMemoryEngine>,
    identifier: &str,
    metric_name: &str,
    unit: &str,
    values: &[f64],
) {
    let at = ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::now_secs()
        - 3_600.0;
    seed_cloudwatch_series(
        engine,
        RDS_NAMESPACE,
        metric_name,
        &[(DB_INSTANCE_DIMENSION, identifier)],
        Some(unit),
        at,
        values,
    );
}

/// Opt the account in and let the analysis its enrollment starts come due — the gate every
/// recommendation reader of this service applies.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + SLACK).await;
}

/// Read the recommendations, asserting the call was accepted, and answer the body.
async fn get_rds(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, "GetRDSDatabaseRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// Every reported recommendation by the `resourceArn` it names.
fn reported_by_arn(body: &Value) -> BTreeMap<String, Value> {
    body["rdsDBRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of rdsDBRecommendations, got {body}"))
        .iter()
        .map(|recommendation| {
            (
                recommendation["resourceArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                recommendation.clone(),
            )
        })
        .collect()
}

/// One row of the analysis table: a DB instance of the account's inventory, the utilization
/// published for it, and the recommendation that follows from both.
struct Case {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The DB instance's identifier, its engine and the class it runs on.
    identifier: &'static str,
    /// See [`Case::identifier`].
    engine: &'static str,
    /// See [`Case::identifier`].
    class: &'static str,
    /// The vCPUs that class provides: AWS's instance-size naming gives an `xlarge` 4 vCPUs,
    /// each `<N>xlarge` N times that, and each named size below `xlarge` half the one above.
    vcpus: f64,
    /// Whether the instance is a Multi-AZ deployment, which runs a standby of the same class.
    multi_az: bool,
    /// Its allocated storage, in GiB.
    allocated_storage: i32,
    /// The peak `CPUUtilization` published for it, as a percentage.
    cpu_peak: f64,
    /// The `RDSInstanceFinding` the analysis must reach, and the
    /// `RDSInstanceFindingReasonCode`s behind it.
    finding: &'static str,
    /// See [`Case::finding`].
    reason_codes: &'static [&'static str],
    /// The DB instance class the analysis must recommend, or `None` where it would keep the
    /// current one and so reports no recommendation option at all.
    recommended: Option<&'static str>,
    /// The vCPUs that recommended class provides.
    recommended_vcpus: f64,
}

/// The analysis table: one row per case a database recommendation distinguishes, each
/// differing in the class, the engine, the deployment, the storage and the observed peak — so
/// what is asserted is the rule over the class of inputs rather than one recorded database.
///
/// The classes are the ones Amazon RDS publishes as orderable for the row's engine
/// (`DescribeOrderableDBInstanceOptions`): the burstable `t4g` family in its `micro` …
/// `2xlarge` sizes and the standard families (`m5`, `r6g`, …) in `large` … `16xlarge`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "an over-provisioned memory-optimised instance drops to the smallest class \
                   that holds its peak",
            identifier: "orders-primary",
            engine: "postgres",
            class: "db.r6g.2xlarge",
            vcpus: 8.0,
            multi_az: false,
            allocated_storage: 500,
            // 8 vCPUs at a peak of 17.5% with 20% headroom needs 1.68 vCPUs, which the
            // family's smallest class (2 vCPUs) holds.
            cpu_peak: 17.5,
            finding: "Overprovisioned",
            reason_codes: &["CPUOverprovisioned"],
            recommended: Some("db.r6g.large"),
            recommended_vcpus: 2.0,
        },
        Case {
            case: "an under-provisioned burstable instance grows to the next size up",
            identifier: "reports-primary",
            engine: "mysql",
            class: "db.t4g.medium",
            vcpus: 1.0,
            multi_az: false,
            allocated_storage: 100,
            // 1 vCPU at a peak of 95% with headroom needs 1.14, which only the next size up
            // (2 vCPUs) holds.
            cpu_peak: 95.0,
            finding: "Underprovisioned",
            reason_codes: &["CPUUnderprovisioned"],
            recommended: Some("db.t4g.large"),
            recommended_vcpus: 2.0,
        },
        Case {
            case: "an instance whose peak fits the class it runs on is optimized and has no \
                   option",
            identifier: "audit-archive",
            engine: "mariadb",
            class: "db.m5.large",
            vcpus: 2.0,
            multi_az: false,
            allocated_storage: 50,
            // 2 vCPUs at a peak of 80% with headroom needs 1.92: the class it runs on.
            cpu_peak: 80.0,
            finding: "Optimized",
            reason_codes: &[],
            recommended: None,
            recommended_vcpus: 2.0,
        },
        Case {
            case: "a Multi-AZ instance is priced for its standby too",
            identifier: "ledger-primary",
            engine: "postgres",
            class: "db.m5.xlarge",
            vcpus: 4.0,
            multi_az: true,
            allocated_storage: 200,
            cpu_peak: 10.0,
            finding: "Overprovisioned",
            reason_codes: &["CPUOverprovisioned"],
            recommended: Some("db.m5.large"),
            recommended_vcpus: 2.0,
        },
        Case {
            case: "a far over-provisioned instance drops several sizes at once",
            identifier: "hot-shard",
            engine: "postgres",
            class: "db.r6g.8xlarge",
            vcpus: 32.0,
            multi_az: false,
            allocated_storage: 1_000,
            // 32 vCPUs at a peak of 5% with headroom needs 1.92 vCPUs.
            cpu_peak: 5.0,
            finding: "Overprovisioned",
            reason_codes: &["CPUOverprovisioned"],
            recommended: Some("db.r6g.large"),
            recommended_vcpus: 2.0,
        },
        Case {
            case: "an instance whose demand passes the largest published class stays on it",
            identifier: "mega-primary",
            engine: "postgres",
            class: "db.r6g.16xlarge",
            vcpus: 64.0,
            multi_az: false,
            allocated_storage: 2_000,
            // 64 vCPUs at a peak of 99% needs 76.03 with headroom, which no published class of
            // the family holds: the finding still says so, and there is no larger class to
            // recommend.
            cpu_peak: 99.0,
            finding: "Underprovisioned",
            reason_codes: &["CPUUnderprovisioned"],
            recommended: None,
            recommended_vcpus: 64.0,
        },
    ]
}

/// Every analysed database is a DB instance of the account's own Amazon RDS inventory, and
/// every member reported for it is computed from that instance's stored configuration and from
/// the utilization published for it: the class it runs on, the finding and reason code its peak
/// implies, the class the analysis would move it to, and what that move saves per month.
#[tokio::test]
async fn each_analysed_database_is_computed_from_the_db_instance_and_its_utilization() {
    let (router, engine) = emulator_with_analysed_databases();
    let cases = cases();
    for case in &cases {
        create_db_instance(
            &router,
            REGION,
            case.identifier,
            case.engine,
            case.class,
            case.multi_az,
            case.allocated_storage,
        )
        .await;
        // The peak is the Maximum of the window, so each series carries a lower observation
        // too.
        publish(
            &engine,
            case.identifier,
            "CPUUtilization",
            "Percent",
            &[case.cpu_peak / 2.0, case.cpu_peak],
        );
    }
    analysed(&router).await;
    // Amazon RDS owes each instance its `available` status one span after the create; the
    // owning service settles it on its own next call, which is the read-back below.
    tokio::time::sleep(STATUS_TRANSITION_DELAY + SLACK).await;
    for case in &cases {
        let described = describe_db_instance(&router, REGION, case.identifier).await;
        assert_eq!(
            element(&described, "DBInstanceStatus"),
            "available",
            "case {:?}: Amazon RDS reports the instance it holds, got {described}",
            case.case
        );
    }

    let body = get_rds(&router, "the account's own databases", json!({})).await;
    let reported = reported_by_arn(&body);
    let wanted: Vec<String> = cases
        .iter()
        .map(|case| db_arn(REGION, case.identifier))
        .collect();
    let mut found: Vec<String> = reported.keys().cloned().collect();
    found.sort();
    let mut expected = wanted.clone();
    expected.sort();
    assert_eq!(
        found, expected,
        "a recommendation is reported for exactly the account's analysed DB instances, got \
         {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "no resource was named, so nothing is missing a recommendation, got {body}"
    );

    for case in &cases {
        let arn = db_arn(REGION, case.identifier);
        let recommendation = reported
            .get(&arn)
            .unwrap_or_else(|| panic!("case {:?}: a recommendation for {arn}, got {body}", case.case));
        assert_eq!(
            recommendation["accountId"].as_str(),
            Some(ACCOUNT),
            "case {:?}: the account that owns the DB instance, got {recommendation}",
            case.case
        );
        assert_eq!(
            recommendation["engine"].as_str(),
            Some(case.engine),
            "case {:?}: the engine Amazon RDS holds for the instance, got {recommendation}",
            case.case
        );
        assert_eq!(
            recommendation["currentDBInstanceClass"].as_str(),
            Some(case.class),
            "case {:?}: the class Amazon RDS holds for the instance, got {recommendation}",
            case.case
        );
        assert_eq!(
            recommendation["currentStorageConfiguration"]["allocatedStorage"].as_i64(),
            Some(i64::from(case.allocated_storage)),
            "case {:?}: the storage Amazon RDS holds for the instance, got {recommendation}",
            case.case
        );
        assert_eq!(
            recommendation["instanceFinding"].as_str(),
            Some(case.finding),
            "case {:?}: the finding the observed peak implies, got {recommendation}",
            case.case
        );
        assert_eq!(
            strings(&recommendation["instanceFindingReasonCodes"]),
            case.reason_codes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {:?}: the reason codes behind the finding, got {recommendation}",
            case.case
        );
        // The observed utilization is the Maximum of the published window, under the one
        // statistic this API reports.
        let observed = metrics(&recommendation["utilizationMetrics"]);
        assert!(
            observed
                .get("CPU")
                .is_some_and(|value| close(*value, case.cpu_peak)),
            "case {:?}: CPU carries the peak published for the instance ({}), got {observed:?}",
            case.case,
            case.cpu_peak
        );

        // The capacity the peak needs, which is what decides the class and the finding.
        let required = case.vcpus * case.cpu_peak / UTILIZATION_PERCENTAGE_CEILING
            * (1.0 + RECOMMENDATION_HEADROOM);
        let options = recommendation["instanceRecommendationOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let Some(recommended) = case.recommended else {
            assert!(
                options.is_empty(),
                "case {:?}: the analysis would keep the class, so there is no option to \
                 report, got {recommendation}",
                case.case
            );
            assert!(
                recommendation["recommendedDBInstanceClass"].is_null(),
                "case {:?}: no class is recommended, got {recommendation}",
                case.case
            );
            continue;
        };
        assert!(
            case.recommended_vcpus >= required || case.finding == "Underprovisioned",
            "case {:?}: the recommended class must hold the {required} vCPUs the peak needs",
            case.case
        );
        assert_eq!(
            options.len(),
            1,
            "case {:?}: the analysis records one class it would move the database to, got \
             {recommendation}",
            case.case
        );
        let option = &options[0];
        assert_eq!(
            option["dbInstanceClass"].as_str(),
            Some(recommended),
            "case {:?}: the smallest class of the family that holds the peak, got {option}",
            case.case
        );
        assert_eq!(
            option["rank"].as_i64(),
            Some(1),
            "case {:?}: the one option is the first-ranked one, got {option}",
            case.case
        );
        // Each class is priced per vCPU, a Multi-AZ deployment for the standby as well, so
        // what the move saves is the capacity it gives up at that price — and nothing at all
        // where the analysis would GROW the database, which saves no money.
        let instances = if case.multi_az { 2.0 } else { 1.0 };
        let current_cost = case.vcpus * monthly_cost_per_vcpu() * instances;
        let saved =
            ((case.vcpus - case.recommended_vcpus) * monthly_cost_per_vcpu() * instances).max(0.0);
        let percentage = saved / current_cost * UTILIZATION_PERCENTAGE_CEILING;
        let reported_saved = option["savingsOpportunity"]["estimatedMonthlySavings"]["value"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            close(reported_saved, saved),
            "case {:?}: expected a monthly saving of {saved}, got {reported_saved} in {option}",
            case.case
        );
        let reported_percentage = option["savingsOpportunity"]["savingsOpportunityPercentage"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            close(reported_percentage, percentage),
            "case {:?}: expected {percentage}% of the current cost, got {reported_percentage} \
             in {option}",
            case.case
        );
    }
}

/// Every `AWS/RDS` series the analysis reads is reported under the `RDSDBMetricName` that
/// measures the same thing, and the storage-space utilization is derived from the free space
/// published for the instance against the storage allocated to it.
#[tokio::test]
async fn the_reported_utilization_is_the_published_series() {
    let (router, engine) = emulator_with_analysed_databases();
    let identifier = "metrics-primary";
    let allocated_storage = 200;
    create_db_instance(
        &router,
        REGION,
        identifier,
        "postgres",
        "db.m5.xlarge",
        false,
        allocated_storage,
    )
    .await;

    // (the series Amazon RDS publishes, its unit, the peak published, the member it is
    // reported under) — the mapping the analysis reads, each row a different unit.
    let series: Vec<(&str, &str, f64, &str)> = vec![
        ("CPUUtilization", "Percent", 40.0, "CPU"),
        ("DatabaseConnections", "Count", 12.0, "DatabaseConnections"),
        (
            "NetworkReceiveThroughput",
            "Bytes/Second",
            1_250_000.0,
            "NetworkReceiveThroughput",
        ),
        (
            "NetworkTransmitThroughput",
            "Bytes/Second",
            640_000.0,
            "NetworkTransmitThroughput",
        ),
        ("ReadIOPS", "Count/Second", 820.0, "EBSVolumeReadIOPS"),
        ("WriteIOPS", "Count/Second", 310.0, "EBSVolumeWriteIOPS"),
        (
            "ReadThroughput",
            "Bytes/Second",
            4_500_000.0,
            "EBSVolumeReadThroughput",
        ),
        (
            "WriteThroughput",
            "Bytes/Second",
            900_000.0,
            "EBSVolumeWriteThroughput",
        ),
    ];
    for (published, unit, peak, _) in &series {
        publish(&engine, identifier, published, unit, &[peak / 2.0, *peak]);
    }
    // The free space left on the instance's storage: the LEAST of the window is the most
    // storage in use, which is what the reported utilization is of.
    let free_gib = 50.0;
    publish(
        &engine,
        identifier,
        "FreeStorageSpace",
        "Bytes",
        &[free_gib * BYTES_PER_GIB, 150.0 * BYTES_PER_GIB],
    );

    analysed(&router).await;
    tokio::time::sleep(STATUS_TRANSITION_DELAY + SLACK).await;
    describe_db_instance(&router, REGION, identifier).await;

    let arn = db_arn(REGION, identifier);
    let body = get_rds(&router, "the published series", json!({"resourceArns": [arn.clone()]})).await;
    let reported = reported_by_arn(&body);
    let recommendation = reported
        .get(&arn)
        .unwrap_or_else(|| panic!("a recommendation for {arn}, got {body}"));
    let observed = metrics(&recommendation["utilizationMetrics"]);
    for (published, _, peak, member) in &series {
        let value = observed
            .get(*member)
            .unwrap_or_else(|| panic!("{published} is reported as {member}, got {observed:?}"));
        assert!(
            close(*value, *peak),
            "{published}: expected the published peak {peak} under {member}, got {value}"
        );
    }
    let used_percentage = (f64::from(allocated_storage) - free_gib)
        / f64::from(allocated_storage)
        * UTILIZATION_PERCENTAGE_CEILING;
    let storage = observed
        .get("EBSVolumeStorageSpaceUtilization")
        .unwrap_or_else(|| panic!("the storage-space utilization, got {observed:?}"));
    assert!(
        close(*storage, used_percentage),
        "the storage in use is the allocated storage less the free space: expected \
         {used_percentage}, got {storage}"
    );
    // Amazon RDS publishes `FreeableMemory` in bytes rather than a percentage of the class's
    // memory, which this emulator publishes no capacity for, so nothing is reported for the
    // `Memory` member rather than a figure nothing observed.
    assert!(
        !observed.contains_key("Memory"),
        "no memory utilization was observed, so none is reported, got {observed:?}"
    );
}

/// A DB instance the account does not have an analysis for is absent from the answer, which
/// stays a successful empty result: an account that owns none reads the empty list, and so
/// does one whose instances this endpoint's region does not hold, whose engine is not up yet,
/// which nothing has published utilization for, or which has been deleted.
#[tokio::test]
async fn a_db_instance_with_no_analysis_is_absent_from_the_answer() {
    let (router, engine) = emulator_with_analysed_databases();
    analysed(&router).await;

    let body = get_rds(&router, "an account that owns no database", json!({})).await;
    assert_eq!(
        body["rdsDBRecommendations"],
        json!([]),
        "an account with no DB instance has no database recommendation, got {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "nothing is missing a recommendation either, got {body}"
    );

    // An instance of another region: Compute Optimizer is regional, so it belongs to that
    // region's endpoint and not to this one.
    create_db_instance(
        &router,
        "eu-central-1",
        "elsewhere-primary",
        "postgres",
        "db.m5.large",
        false,
        100,
    )
    .await;
    publish(&engine, "elsewhere-primary", "CPUUtilization", "Percent", &[20.0]);
    // An instance nothing published utilization for: Compute Optimizer needs accumulated
    // metrics before it reaches a finding.
    create_db_instance(&router, REGION, "silent-primary", "postgres", "db.m5.large", false, 100)
        .await;
    // An instance whose engine is not up yet: its create was accepted a moment ago, so the
    // workload the analysis reads has not run. Once Amazon RDS's own clock brings it up it IS
    // analysed, which is what the last read below shows.
    create_db_instance(&router, REGION, "warming-primary", "postgres", "db.m5.large", false, 100)
        .await;
    publish(&engine, "warming-primary", "CPUUtilization", "Percent", &[30.0]);
    // An instance that was deleted: it is gone from the inventory, so it is gone from the
    // answer.
    create_db_instance(&router, REGION, "retired-primary", "postgres", "db.m5.large", false, 100)
        .await;
    publish(&engine, "retired-primary", "CPUUtilization", "Percent", &[25.0]);

    let body = get_rds(&router, "before the engines are up", json!({})).await;
    assert_eq!(
        body["rdsDBRecommendations"],
        json!([]),
        "no instance is analysed yet, got {body}"
    );

    tokio::time::sleep(STATUS_TRANSITION_DELAY + SLACK).await;
    describe_db_instance(&router, REGION, "retired-primary").await;
    let (status, deleted) = call_rds(
        &router,
        "DeleteDBInstance",
        &[
            ("DBInstanceIdentifier", "retired-primary".to_string()),
            ("SkipFinalSnapshot", "true".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "deleting retired-primary: expected 200, got {status}: {deleted}"
    );
    // The delete is owed one span, after which the instance is gone from the store.
    tokio::time::sleep(STATUS_TRANSITION_DELAY + SLACK).await;
    describe_db_instance(&router, REGION, "silent-primary").await;

    let body = get_rds(&router, "after the engines are up", json!({})).await;
    let reported = reported_by_arn(&body);
    assert_eq!(
        reported.keys().cloned().collect::<Vec<String>>(),
        vec![db_arn(REGION, "warming-primary")],
        "only the instance whose engine came up with published utilization is analysed: of \
         the others one is another region's, one has no published utilization and one was \
         deleted, got {body}"
    );

    // The one of them this endpoint's region does hold IS reported at the endpoint it belongs
    // to, once its own engine is up there — the same rule, read at the other endpoint.
    describe_db_instance(&router, "eu-central-1", "elsewhere-primary").await;
    let (status, elsewhere) = call_in_region(
        &router,
        "eu-central-1",
        "GetRDSDatabaseRecommendations",
        json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the other region: expected 200, got {status}: {elsewhere}"
    );
    assert_eq!(
        reported_by_arn(&elsewhere)
            .keys()
            .cloned()
            .collect::<Vec<String>>(),
        vec![db_arn("eu-central-1", "elsewhere-primary")],
        "the instance of that region is analysed at that region's endpoint, got {elsewhere}"
    );
}

/// Every surface that serves a database analysis serves the SAME analysed databases: the
/// recommendation reader, the projection of its recommended option, the summary that counts it
/// and the export of the same rows.
#[tokio::test]
async fn every_database_surface_reads_the_same_analysed_databases() {
    let bucket = "compute-optimizer-database-inventory";
    let (router, engine) = emulator_with_analysed_databases();
    let identifier = "orders-primary";
    create_db_instance(&router, REGION, identifier, "postgres", "db.r6g.2xlarge", false, 500)
        .await;
    publish(&engine, identifier, "CPUUtilization", "Percent", &[17.5]);
    create_bucket(&router, bucket).await;
    analysed(&router).await;
    tokio::time::sleep(STATUS_TRANSITION_DELAY + SLACK).await;
    describe_db_instance(&router, REGION, identifier).await;

    let arn = db_arn(REGION, identifier);
    let body = get_rds(&router, "the reader", json!({})).await;
    let reported = reported_by_arn(&body);
    let recommendation = reported
        .get(&arn)
        .unwrap_or_else(|| panic!("a recommendation for {arn}, got {body}"));
    let option = &recommendation["instanceRecommendationOptions"][0];
    let recommended_class = option["dbInstanceClass"].clone();
    let saved = option["savingsOpportunity"]["estimatedMonthlySavings"]["value"]
        .as_f64()
        .unwrap_or_default();

    // The projection is onto the option the reader reports, and the utilization it projects is
    // the observed one on the recommended class's capacity: the same work on a quarter of the
    // vCPUs runs at four times the percentage.
    let (status, projected) = call(
        &router,
        "GetRDSDatabaseRecommendationProjectedMetrics",
        json!({
            "resourceArn": arn,
            "stat": "Maximum",
            "period": 3_600,
            "startTime": 1_761_000_000,
            "endTime": 1_761_000_000 + 2 * 3_600,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the projection: expected 200, got {status}: {projected}"
    );
    let projection = &projected["recommendedOptionProjectedMetrics"][0];
    assert_eq!(
        projection["recommendedDBInstanceClass"], recommended_class,
        "the projection is onto the recommended option: {projection} vs {recommendation}"
    );
    let cpu = projection["projectedMetrics"]
        .as_array()
        .and_then(|series| {
            series
                .iter()
                .find(|metric| metric["name"].as_str() == Some("CPU"))
                .cloned()
        })
        .unwrap_or_else(|| panic!("a projected CPU series, got {projected}"));
    let projected_cpu = cpu["values"][0].as_f64().unwrap_or_default();
    assert!(
        close(projected_cpu, 70.0),
        "17.5% of 8 vCPUs is 70% of the 2 vCPUs recommended, got {projected_cpu} in {cpu}"
    );

    // The summary counts the database the reader reports, and reports the same saving.
    let (status, summaries) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(
        status, 200,
        "the summary: expected 200, got {status}: {summaries}"
    );
    let summary = summaries["recommendationSummaries"]
        .as_array()
        .expect("a list of summaries")
        .iter()
        .find(|summary| summary["recommendationResourceType"] == json!(SOURCE_TYPE_RDS_DB_INSTANCE))
        .cloned()
        .unwrap_or_else(|| panic!("a summary for {SOURCE_TYPE_RDS_DB_INSTANCE}, got {summaries}"));
    let counted: f64 = summary["summaries"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["value"].as_f64().unwrap_or_default())
                .sum()
        })
        .unwrap_or_default();
    assert!(
        close(counted, 1.0),
        "the summary counts the one analysed database, got {summary}"
    );
    let summarised = summary["savingsOpportunity"]["estimatedMonthlySavings"]["value"]
        .as_f64()
        .unwrap_or_default();
    assert!(
        close(summarised, saved),
        "the summarised saving is the saving the reader reports ({saved}), got {summary}"
    );

    // The export holds the database the reader reports, with the class and the finding the
    // reader reported for it.
    let columns = vec![
        "AccountId".to_string(),
        "ResourceArn".to_string(),
        "InstanceFinding".to_string(),
        "CurrentDBInstanceClass".to_string(),
    ];
    let (status, exported) = call(
        &router,
        "ExportRDSDatabaseRecommendations",
        json!({
            "s3DestinationConfig": {"bucket": bucket},
            "fieldsToExport": columns,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the export: expected 200, got {status}: {exported}"
    );
    let (bucket_name, key, _) = export_destination("the export", &exported);
    // A created job is `Queued` for one step and `InProgress` for the next, then `Complete` —
    // which is when the objects it answered are in the bucket.
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let (status, document) = get_object(&router, &bucket_name, &key).await;
    assert_eq!(
        status, 200,
        "downloading {key}: got {status}: {}",
        String::from_utf8_lossy(&document)
    );
    let document = String::from_utf8(document).expect("the export file is text");
    let rows = csv_rows(&document);
    let header = rows.first().cloned().unwrap_or_default();
    let data = rows.get(1..).unwrap_or_default();
    assert_eq!(
        data.len(),
        1,
        "the export holds one row per analysed database, got {document:?}"
    );
    let row = &data[0];
    assert_eq!(
        cell(&header, row, "ResourceArn"),
        arn,
        "the exported row is about the database the reader reports, got {row:?}"
    );
    assert_eq!(
        cell(&header, row, "InstanceFinding"),
        recommendation["instanceFinding"].as_str().unwrap_or_default(),
        "the exported finding is the finding the reader reports, got {row:?}"
    );
    assert_eq!(
        cell(&header, row, "CurrentDBInstanceClass"),
        recommendation["currentDBInstanceClass"]
            .as_str()
            .unwrap_or_default(),
        "the exported class is the class the reader reports, got {row:?}"
    );
}
