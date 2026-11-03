// SPDX-License-Identifier: CC0-1.0

//! What a service's DEFAULT-QUOTA row is keyed on, and what it carries, over the wire.
//!
//! The rule these suites assert: for a `ServiceCode`,
//! `ListAWSDefaultServiceQuotas`/`GetAWSDefaultServiceQuota` publish the quotas AWS
//! publishes for that service, each under the `QuotaCode` AWS assigns it — a code in
//! AWS's minted `L-` namespace — carrying that quota's own `QuotaName`, `Value`,
//! `Unit`, `Adjustable`, `Description` and, where AWS publishes them, the `Period` it
//! is measured over and the `AWS/Usage` `UsageMetric` it is metered by; and NO row
//! answers to a code built out of a quota's name, because a code AWS never minted
//! resolves a quota AWS never published.
//!
//! The table spans eight `ServiceCode`s and every shape the rule generalises over: a
//! plain count plane (`ram`), a metered plane published over a ONE-HOUR window
//! (`rds`), a concurrency plane published as fixed (`rds`), a per-database plane in the
//! thousands and a count in the hundreds of thousands (`redshift`), planes published in
//! the `Count` unit and metered by a resource count over five minutes
//! (`redshift-serverless`), a per-VPC plane published as fixed and a domain-list plane
//! in the hundreds of thousands (`route53resolver`), planes measured in `Terabytes` and
//! a rate measured in `Gigabits` per second (`s3`), per-API request rates metered by an
//! `AWS/Usage` `CallCount` over one second (`scheduler`, `secretsmanager`) and one rate
//! published over FOUR operations at once, which no single call count meters
//! (`secretsmanager`).
//!
//! SOURCE for every expectation: the recorded `ListAWSDefaultServiceQuotas` response of
//! each of these service codes, which states for each code the quota published under it
//! — its name, default value, unit, adjustability, window and usage metric — beside
//! AWS's published Service Quotas data for the service, which is the source
//! `src/state.rs::seed_quota_catalog` carries. The spot-checked rows include quotas the
//! issue behind this suite never cited.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns, and the minted-code question is `catalog_identity.rs`'s one copy.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::descriptive_quota_code;

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{client, default_quotas, emulator, walk_pages};

/// One published row, as the recorded default-quota listing of its `ServiceCode`
/// reports it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// `(PeriodValue, PeriodUnit)` for a quota AWS publishes over a window.
    period: Option<(i32, &'static str)>,
    /// `(MetricName, MetricStatisticRecommendation, Service, Resource, Type)` of the
    /// `AWS/Usage` metric, for a quota CloudWatch publishes usage for.
    metric: Option<(
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )>,
}

/// The `AWS/Usage` `CallCount` metric of one API: the shape every per-API request-rate
/// row is metered by.
const fn call_count(
    usage_service: &'static str,
    api: &'static str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    Some(("CallCount", "Sum", usage_service, api, "API"))
}

/// The `AWS/Usage` `ResourceCount` metric of one resource plane.
const fn resource_count(
    usage_service: &'static str,
    resource: &'static str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    Some(("ResourceCount", "Maximum", usage_service, resource, "Resource"))
}

/// The rows pinned field for field, across the eight service codes.
const PUBLISHED_ROWS: &[Row] = &[
    // ram — the sharing planes, all published at the same large count.
    Row {
        service_code: "ram",
        quota_code: "L-595828F9",
        quota_name: "Number of resource shares",
        value: 25_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "ram",
        quota_code: "L-8491BF81",
        quota_name: "Number of principal associations",
        value: 25_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // A plane two orders of magnitude smaller, in the same service.
    Row {
        service_code: "ram",
        quota_code: "L-2870BE9D",
        quota_name: "Number of custom permissions per resource type",
        value: 10.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // rds — a metered plane published over a one-hour window, and a concurrency plane
    // published as fixed.
    Row {
        service_code: "rds",
        quota_code: "L-9372BAB3",
        quota_name: "Custom endpoints per DB cluster",
        value: 5.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "HOUR")),
        metric: resource_count("RDS", "CustomEndpoints"),
    },
    Row {
        service_code: "rds",
        quota_code: "L-E3721453",
        quota_name: "Data API maximum concurrent cluster-secret pairs",
        value: 30.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    // redshift — a per-database plane published as fixed, and a count in the hundreds
    // of thousands.
    Row {
        service_code: "redshift",
        quota_code: "L-4E1FE923",
        quota_name: "Schemas in each database per cluster",
        value: 9900.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-99F53FC8",
        quota_name: "Tables for 4xlarge cluster node type",
        value: 200_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "redshift",
        quota_code: "L-BB966966",
        quota_name: "Nodes in a cluster",
        value: 128.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // redshift-serverless — planes published in the `Count` unit and metered by a
    // resource count over five minutes.
    Row {
        service_code: "redshift-serverless",
        quota_code: "L-96772D74",
        quota_name: "Serverless Namespaces",
        value: 25.0,
        unit: "Count",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: resource_count("RedshiftServerless", "ServerlessNamespaces"),
    },
    Row {
        service_code: "redshift-serverless",
        quota_code: "L-1854027F",
        quota_name: "Redshift Processing Units (RPUs)",
        value: 3200.0,
        unit: "Count",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: resource_count("RedshiftServerless", "RedshiftProcessingUnits"),
    },
    // route53resolver — a per-VPC plane published as fixed, and a domain-list plane in
    // the hundreds of thousands.
    Row {
        service_code: "route53resolver",
        quota_code: "L-15219E1D",
        quota_name: "DNS Firewall rule group associations per VPC",
        value: 5.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-1B2BDF0A",
        quota_name: "Domains in a file imported from S3",
        value: 250_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "route53resolver",
        quota_code: "L-D74B6237",
        quota_name: "Target IP addresses per resolver rule",
        value: 6.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    // s3 — a plane measured in `Terabytes` (and in a FRACTION of one), a rate measured
    // in `Gigabits` per second, and the bucket plane, which is counted per Region like
    // the rest of this service's rows.
    Row {
        service_code: "s3",
        quota_code: "L-89BABEE8",
        quota_name: "Object size",
        value: 48.828_125,
        unit: "Terabytes",
        adjustable: true,
        period: None,
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-349AD9CA",
        quota_name: "Replication transfer rate",
        value: 1.0,
        unit: "Gigabits",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-B461D596",
        quota_name: "Replication rules",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: None,
    },
    Row {
        service_code: "s3",
        quota_code: "L-DC2B2D3D",
        quota_name: "General purpose buckets",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // scheduler — a per-API request rate metered by a call count, beside the account's
    // own schedule plane, whose default is in the tens of millions.
    Row {
        service_code: "scheduler",
        quota_code: "L-6F4B17DE",
        quota_name: "CreateSchedule request rate",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: call_count("EventBridge Scheduler", "CreateSchedule"),
    },
    Row {
        service_code: "scheduler",
        quota_code: "L-EE5D6FF0",
        quota_name: "Number of schedules",
        value: 10_000_000.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: None,
    },
    // secretsmanager — request rates this service publishes as NOT adjustable, one of
    // them over four operations at once, which no single call count meters.
    Row {
        service_code: "secretsmanager",
        quota_code: "L-385CE7B7",
        quota_name: "Rate of DescribeSecret API requests",
        value: 40_000.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: call_count("Secrets Manager", "DescribeSecret"),
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-2D877201",
        quota_name: "Rate of GetSecretValue API requests",
        value: 10_000.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: call_count("Secrets Manager", "GetSecretValue"),
    },
    Row {
        service_code: "secretsmanager",
        quota_code: "L-8D82A813",
        quota_name: "Combined rate of DeleteResourcePolicy, GetResourcePolicy, \
                     PutResourcePolicy and ValidateResourcePolicy API requests",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: Some((1, "SECOND")),
        metric: None,
    },
];

/// The eight service codes, each with the `ServiceName` Service Quotas reports it under
/// and the `AWS/Usage` display name its metered quotas carry (`None` for a service that
/// publishes no metered row here).
const COVERED_SERVICES: &[(&str, &str, Option<&str>)] = &[
    ("ram", "AWS Resource Access Manager", None),
    (
        "rds",
        "Amazon Relational Database Service (Amazon RDS)",
        Some("RDS"),
    ),
    ("redshift", "Amazon Redshift", None),
    (
        "redshift-serverless",
        "Amazon Redshift Serverless",
        Some("RedshiftServerless"),
    ),
    ("route53resolver", "Route 53 Resolver", None),
    ("s3", "Amazon Simple Storage Service (Amazon S3)", None),
    (
        "scheduler",
        "Amazon EventBridge Scheduler",
        Some("EventBridge Scheduler"),
    ),
    ("secretsmanager", "AWS Secrets Manager", Some("Secrets Manager")),
];

/// Every pinned code is bound to the quota AWS publishes under it: the row reports that
/// quota's name, value, unit, adjustability, window and usage metric, through the
/// listing, through the single default read and through the applied read alike.
#[tokio::test]
async fn a_published_row_carries_the_quota_aws_publishes_under_its_code() {
    let engine = emulator();
    let region = "us-west-2";
    let caller = client(&engine, region);

    let mut listings: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for row in PUBLISHED_ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            (
                quota["QuotaName"].clone(),
                quota["Value"].clone(),
                quota["Unit"].clone(),
                quota["Adjustable"].clone(),
                quota["GlobalQuota"].clone(),
                quota["QuotaArn"].clone(),
            ),
            (
                json!(row.quota_name),
                json!(row.value),
                json!(row.unit),
                json!(row.adjustable),
                json!(false),
                json!(format!(
                    "arn:aws:servicequotas:{region}::{}/{}",
                    row.service_code, row.quota_code
                )),
            ),
            "case {case}: expected the quota AWS publishes under this code, got {quota}"
        );
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes this quota over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        match row.metric {
            Some((metric_name, statistic, service, resource, usage_type)) => assert_eq!(
                quota["UsageMetric"],
                json!({
                    "MetricNamespace": "AWS/Usage",
                    "MetricName": metric_name,
                    "MetricStatisticRecommendation": statistic,
                    "MetricDimensions": {
                        "Service": service,
                        "Resource": resource,
                        "Type": usage_type,
                        "Class": "None",
                    },
                }),
                "case {case}: expected the usage metric AWS meters this quota with, got {quota}"
            ),
            None => assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {quota}"
            ),
        }

        let listed = match listings.get(row.service_code) {
            Some(listed) => listed,
            None => {
                let listed = default_quotas(&caller, row.service_code).await;
                listings.entry(row.service_code).or_insert(listed)
            }
        };
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );

        // The applied plane is keyed on the same code, so a client that read the code
        // out of the default catalog resolves the same quota's value in force.
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (applied["QuotaName"].clone(), applied["Value"].clone()),
            (json!(row.quota_name), json!(row.value)),
            "case {case}: the applied read must resolve the same quota, got {applied}"
        );
    }
}

/// Every row of these services is keyed on a code in AWS's minted namespace, described
/// once, named once, and reported under the service's own `ServiceCode`/`ServiceName`;
/// and a code built out of a quota's NAME resolves nothing, in the default plane and in
/// the applied plane alike.
#[tokio::test]
async fn no_row_answers_to_a_code_aws_never_minted() {
    let engine = emulator();
    let region = "eu-west-1";
    let caller = client(&engine, region);

    for (service_code, service_name, _) in COVERED_SERVICES {
        let listed = default_quotas(&caller, service_code).await;
        assert!(
            !listed.is_empty(),
            "case {service_code}: the service must publish its default-quota set, got {listed:?}"
        );

        let mut names: BTreeSet<&str> = BTreeSet::new();
        for quota in &listed {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            assert!(
                is_aws_minted_shape(code),
                "case {case}: every published row is keyed on the code AWS assigns the quota, \
                 got {code:?}"
            );
            assert_eq!(
                (
                    quota["ServiceCode"].as_str(),
                    quota["ServiceName"].as_str(),
                ),
                (Some(*service_code), Some(*service_name)),
                "case {case}: expected the service's own code and published name, got {quota}"
            );
            let description = quota["Description"].as_str().unwrap_or_default();
            assert!(
                !description.is_empty(),
                "case {case}: every published row carries a description, got {quota}"
            );
            let name = quota["QuotaName"].as_str().unwrap_or_default();
            assert!(
                names.insert(name),
                "case {case}: a quota name identifies one quota of a service, got a second row \
                 named {name:?} in {listed:?}"
            );

            // The code a quota's NAME would produce is not a code AWS publishes, so
            // nothing of this service answers to it.
            let invented = descriptive_quota_code(name);
            for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
                let (status, error) = caller
                    .refused(
                        operation,
                        json!({"ServiceCode": service_code, "QuotaCode": invented.clone()}),
                    )
                    .await;
                assert_eq!(
                    (status, error.as_str()),
                    (400, "NoSuchResourceException"),
                    "case {case}/{invented}: {operation} must resolve no quota for a code AWS \
                     does not publish"
                );
            }
        }
    }
}

/// A per-API request-rate row is published over one second, metered by that operation's
/// `AWS/Usage` `CallCount` under the service's display name, and described by the
/// sentence AWS publishes for the class with the operation named in it.
#[tokio::test]
async fn a_request_rate_row_names_the_operation_it_throttles() {
    let engine = emulator();
    let caller = client(&engine, "ap-northeast-1");

    let mut rate_rows = 0_usize;
    for (service_code, _, usage_service) in COVERED_SERVICES {
        for quota in default_quotas(&caller, service_code).await {
            if quota["UsageMetric"]["MetricName"] != json!("CallCount") {
                continue;
            }
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            let api = quota["UsageMetric"]["MetricDimensions"]["Resource"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !api.is_empty(),
                "case {case}: a call count names the API it meters, got {quota}"
            );
            assert_eq!(
                quota["UsageMetric"]["MetricDimensions"]["Service"],
                json!(usage_service.expect("a metered service has an AWS/Usage display name")),
                "case {case}: expected the AWS/Usage display name of the service, got {quota}"
            );
            assert_eq!(
                quota["Period"],
                json!({"PeriodValue": 1, "PeriodUnit": "SECOND"}),
                "case {case}: a request rate is published over one second, got {quota}"
            );
            assert_eq!(
                quota["Description"],
                json!(format!(
                    "The maximum number of {api} requests you can make per second in this \
                     account in the current Region."
                )),
                "case {case}: expected the sentence AWS publishes for a request-rate quota, got \
                 {quota}"
            );
            rate_rows += 1;
        }
    }

    assert!(
        rate_rows >= 6,
        "case catalog: the rule must be asserted over the request-rate rows of these catalogs, \
         got {rate_rows} rows"
    );
}

/// The published set of each of these services does not depend on how it is paged, and
/// a `ServiceCode` the catalog knows but publishes no row for is still a 200 with an
/// empty set rather than a refusal.
#[tokio::test]
async fn the_published_set_is_the_same_however_it_is_paged() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for (service_code, _, _) in COVERED_SERVICES {
        let whole = default_quotas(&caller, service_code).await;
        for size in [1_i64, 3, 100] {
            let paged = walk_pages(
                &caller,
                "ListAWSDefaultServiceQuotas",
                &json!({"ServiceCode": service_code}),
                "Quotas",
                Some(size),
            )
            .await;
            assert_eq!(
                paged, whole,
                "case {service_code} MaxResults={size}: paging must yield the one published set \
                 of {} quotas, got {}",
                whole.len(),
                paged.len()
            );
        }
    }
}
