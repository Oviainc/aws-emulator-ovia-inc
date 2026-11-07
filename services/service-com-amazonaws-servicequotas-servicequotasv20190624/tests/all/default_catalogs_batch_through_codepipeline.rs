// SPDX-License-Identifier: CC0-1.0

//! What `ListAWSDefaultServiceQuotas` publishes for the eight service codes whose whole
//! default-quota catalog sits in AWS's own `QuotaCode` namespace, over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target: ServiceQuotasV20190624.<Op>`
//! dispatched through the same `MultiServiceRouter` the emulator serves with, over ONE
//! shared engine, so what is exercised is the wire behaviour.
//!
//! `QuotaCode` is the only key Service Quotas exposes — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it — so the rules asserted
//! here are about the IDENTITY and the COMPLETENESS of a service's published catalog rather
//! than about one recorded listing:
//!
//! * a service's whole published catalog is served, every row keyed on the code AWS mints
//!   for that quota, with the service's own `ServiceCode`/`ServiceName`, a description on
//!   every row and one key per quota;
//! * a code built out of a quota's NAME is published nowhere and resolves nowhere, so a
//!   client cannot read back a quota this emulator invented, while the service code itself
//!   still resolves;
//! * a published row reports the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`,
//!   `Period` and `UsageMetric` AWS publishes for the quota, with a `QuotaArn` naming the
//!   Region it was read at and no account, identically through the listing and through
//!   `GetAWSDefaultServiceQuota`, and the same key resolves in the APPLIED plane.
//!
//! Each rule is driven over a TABLE of the eight `ServiceCode`s, whose catalogs differ in
//! length (5 rows to 72) and in what they publish (job, keyspace, development-environment,
//! HSM-cluster, trail, package-repository, deployment and pipeline planes, per-API
//! throttles, measured sizes and time limits); the pinned rows span counts and measured
//! quotas (`None`, `Bytes`, `Seconds`, `Megabytes`, `Minutes`, `Days`), adjustable and
//! fixed rows, no window / a one-second rate window / the five-minute window a resource
//! count is published over, and unmetered rows beside both `AWS/Usage` metric shapes
//! (`CallCount` and `ResourceCount`). Ten of the sixteen pinned rows name a quota the issue
//! did not cite, so the rule is asserted beyond the reported instances.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `walk_pages`) is the one
//! `quota_workflow.rs` owns, and "is this a code AWS mints" is asked through
//! `catalog_identity.rs`'s single copy of that question.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory, walk_pages, Client,
};

/// One service whose published default-quota catalog is keyed entirely on AWS's codes.
struct Catalog {
    service_code: &'static str,
    /// The display name AWS's own Service Quotas data reports for the code, which every
    /// row of the service repeats as its `ServiceName`.
    service_name: &'static str,
    /// How many quotas the service's published catalog holds.
    quota_count: usize,
    /// Codes no quota of this service is published under: the descriptive stand-ins a
    /// quota name yields (`src/state.rs::descriptive_quota_code`), which AWS publishes
    /// nothing under.
    withdrawn: &'static [&'static str],
}

/// The services this suite covers.
///
/// SOURCE: AWS's published Service Quotas data for each of these service codes (what `aws
/// service-quotas list-aws-default-service-quotas --service-code <code>` reports) and the
/// recorded listing of each — every row of it carries an `L-XXXXXXXX` `QuotaCode`, and the
/// count is the number of rows the service publishes. `withdrawn` names the codes that
/// recording reports as surplus entries, which are exactly the stand-ins this catalog used
/// to key these services' rows on.
const CATALOGS: &[Catalog] = &[
    Catalog {
        service_code: "batch",
        service_name: "AWS Batch",
        quota_count: 15,
        withdrawn: &["JobQueues", "ComputeEnvironments"],
    },
    Catalog {
        service_code: "cassandra",
        service_name: "Amazon Keyspaces (for Apache Cassandra)",
        quota_count: 27,
        withdrawn: &["TablesPerRegion"],
    },
    Catalog {
        service_code: "cloud9",
        service_name: "AWS Cloud9",
        quota_count: 5,
        withdrawn: &["Environments"],
    },
    Catalog {
        service_code: "cloudhsm",
        service_name: "AWS CloudHSM",
        quota_count: 9,
        withdrawn: &["ClustersPerRegion"],
    },
    Catalog {
        service_code: "cloudtrail",
        service_name: "AWS CloudTrail",
        quota_count: 72,
        withdrawn: &["TrailsPerRegion"],
    },
    Catalog {
        service_code: "codeartifact",
        service_name: "AWS CodeArtifact",
        quota_count: 17,
        withdrawn: &["DomainsPerAccount"],
    },
    Catalog {
        service_code: "codedeploy",
        service_name: "AWS CodeDeploy",
        quota_count: 28,
        withdrawn: &["ApplicationsPerRegion", "DeploymentGroupsPerApplication"],
    },
    Catalog {
        service_code: "codepipeline",
        service_name: "AWS CodePipeline",
        quota_count: 29,
        withdrawn: &["PipelinesPerRegion"],
    },
];

/// The `UsageMetric` a pinned row publishes, where it publishes one.
enum Metric {
    /// No usage metric at all.
    None,
    /// The `AWS/Usage` `CallCount` of the API a request rate throttles, under the
    /// `Service` dimension that count is published with.
    Calls {
        usage_service: &'static str,
        api: &'static str,
    },
    /// The `AWS/Usage` `ResourceCount` of a counted plane, under the `Service` dimension
    /// and the `Resource` series that count is published with.
    Resources {
        usage_service: &'static str,
        resource: &'static str,
    },
}

/// One published row, with every member AWS publishes for it.
struct Row {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// `(PeriodValue, PeriodUnit)` — the window the quota is published over, for a row
    /// that is published over one.
    period: Option<(i32, &'static str)>,
    metric: Metric,
}

/// The rows this suite pins, two per service so the table spans the eight catalogs rather
/// than repeating one.
///
/// SOURCE: AWS's published Service Quotas data for each `ServiceCode` and the recorded
/// default-quota listing of each code for which `QuotaCode` names which quota. Only six of
/// the sixteen rows name a quota the issue cited.
const ROWS: &[Row] = &[
    Row {
        service_code: "batch",
        quota_code: "L-61E3E54E",
        quota_name: "Array size limit",
        value: 10_000.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "batch",
        quota_code: "L-E985971C",
        quota_name: "Transactions per second for SubmitJob limit",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "cassandra",
        quota_code: "L-0659E12E",
        quota_name: "Max clustering key size",
        value: 850.0,
        unit: "Bytes",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "cassandra",
        quota_code: "L-677FFD22",
        quota_name: "Max TTL",
        value: 630_720_000.0,
        unit: "Seconds",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "cloud9",
        quota_code: "L-65F36924",
        quota_name: "Shared environment members",
        value: 8.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "cloud9",
        quota_code: "L-A29EDECE",
        quota_name: "SSH development environments for this account",
        value: 200.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "cloudhsm",
        quota_code: "L-4B16B391",
        quota_name: "Clusters per AWS Region and AWS account",
        value: 4.0,
        unit: "None",
        adjustable: true,
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "CloudHSM",
            resource: "Clusters",
        },
    },
    Row {
        service_code: "cloudhsm",
        quota_code: "L-F847CBBF",
        quota_name: "client connections per HSM",
        value: 900.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "cloudtrail",
        quota_code: "L-9387CED7",
        quota_name: "Transactions per second (TPS) for the LookupEvents API",
        value: 2.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "CloudTrail",
            api: "LookupEvents",
        },
    },
    Row {
        service_code: "cloudtrail",
        quota_code: "L-3BB16389",
        quota_name: "Summarized query results size",
        value: 3.0,
        unit: "Megabytes",
        adjustable: true,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "codeartifact",
        quota_code: "L-37894301",
        quota_name: "GetAuthorizationToken requests per second",
        value: 40.0,
        unit: "None",
        adjustable: true,
        period: Some((1, "SECOND")),
        metric: Metric::None,
    },
    Row {
        service_code: "codeartifact",
        quota_code: "L-5D176989",
        quota_name: "Upstream repositories searched",
        value: 25.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "codedeploy",
        quota_code: "L-86A1A2E1",
        quota_name: "Minutes a deployment lifecycle event can run",
        value: 60.0,
        unit: "Minutes",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "codedeploy",
        quota_code: "L-877B748B",
        quota_name: "Event notification triggers in a deployment group",
        value: 10.0,
        unit: "None",
        adjustable: true,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "codepipeline",
        quota_code: "L-8DF1BAAD",
        quota_name: "Maximum timeout for a manual approval action",
        value: 7.0,
        unit: "Days",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "codepipeline",
        quota_code: "L-0097A9B4",
        quota_name: "Active executions per pipeline",
        value: 50.0,
        unit: "None",
        adjustable: false,
        period: None,
        metric: Metric::None,
    },
];

/// The `UsageMetric` a row's [`Metric`] publishes, as the wire reports it.
fn metric_json(metric: &Metric) -> Value {
    match metric {
        Metric::None => Value::Null,
        Metric::Calls { usage_service, api } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "CallCount",
            "MetricStatisticRecommendation": "Sum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": api,
                "Type": "API",
            },
        }),
        Metric::Resources {
            usage_service,
            resource,
        } => json!({
            "MetricNamespace": "AWS/Usage",
            "MetricName": "ResourceCount",
            "MetricStatisticRecommendation": "Maximum",
            "MetricDimensions": {
                "Service": usage_service,
                "Class": "None",
                "Resource": resource,
                "Type": "Resource",
            },
        }),
    }
}

/// Every `QuotaCode` a listing of `service_code` reports.
async fn listed_codes(caller: &Client, operation: &str, service_code: &str) -> BTreeSet<String> {
    walk_pages(
        caller,
        operation,
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
    .iter()
    .filter_map(|quota| quota["QuotaCode"].as_str().map(str::to_string))
    .collect()
}

/// Every covered service serves its whole published catalog, keyed only on codes AWS
/// mints, each row described and reachable by a key of its own.
#[tokio::test]
async fn every_covered_service_serves_its_published_catalog() {
    let engine = emulator();

    // Two endpoints, because the catalog is published per Region and the rule is about
    // the catalog rather than about one Region's view of it.
    for region in ["us-east-2", "eu-west-3"] {
        let caller = client(&engine, region);
        let inventory: BTreeMap<String, String> = service_inventory(&caller)
            .await
            .iter()
            .filter_map(|service| {
                Some((
                    service["ServiceCode"].as_str()?.to_string(),
                    service["ServiceName"].as_str()?.to_string(),
                ))
            })
            .collect();

        for catalog in CATALOGS {
            let case = format!("{region} {}", catalog.service_code);
            let quotas = defaults(&caller, catalog.service_code).await;

            assert_eq!(
                quotas.len(),
                catalog.quota_count,
                "case {case}: expected the whole published catalog ({} quotas), got {} ({:?})",
                catalog.quota_count,
                quotas.len(),
                quotas
                    .iter()
                    .filter_map(|quota| quota["QuotaCode"].as_str())
                    .collect::<Vec<_>>()
            );

            let mut names: BTreeSet<&str> = BTreeSet::new();
            for quota in &quotas {
                let code = quota["QuotaCode"].as_str().unwrap_or_default();
                let row = format!("{case} {code}");
                assert!(
                    is_aws_minted_shape(code),
                    "case {row}: every published row is keyed on the code AWS assigns the \
                     quota, got {code:?} in {quota}"
                );
                assert_eq!(
                    (
                        quota["ServiceCode"].as_str(),
                        quota["ServiceName"].as_str()
                    ),
                    (Some(catalog.service_code), Some(catalog.service_name)),
                    "case {row}: expected the service's own code and published name, got {quota}"
                );
                assert!(
                    !quota["Description"].as_str().unwrap_or_default().is_empty(),
                    "case {row}: every published row carries the quota's description, got \
                     {quota}"
                );
                let name = quota["QuotaName"].as_str().unwrap_or_default();
                assert!(
                    names.insert(name),
                    "case {row}: a quota name identifies one quota of a service, got a second \
                     row named {name:?}"
                );
                assert_eq!(
                    quota["QuotaArn"],
                    json!(format!(
                        "arn:aws:servicequotas:{region}::{}/{code}",
                        catalog.service_code
                    )),
                    "case {row}: a default quota's ARN names the Region and no account, got \
                     {quota}"
                );
            }

            let codes: BTreeSet<&str> = quotas
                .iter()
                .filter_map(|quota| quota["QuotaCode"].as_str())
                .collect();
            assert_eq!(
                codes.len(),
                quotas.len(),
                "case {case}: every published quota has a key of its own, got {codes:?}"
            );

            assert_eq!(
                inventory.get(catalog.service_code).map(String::as_str),
                Some(catalog.service_name),
                "case {case}: ListServices must report the same display name the quotas carry, \
                 got {:?}",
                inventory.get(catalog.service_code)
            );
        }
    }
}

/// A code built out of a quota NAME is published nowhere and resolves nowhere, while the
/// service itself still resolves.
#[tokio::test]
async fn a_code_derived_from_a_quota_name_resolves_nowhere() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for catalog in CATALOGS {
        for withdrawn in catalog.withdrawn {
            let case = format!("{}/{withdrawn}", catalog.service_code);

            for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
                let listed = listed_codes(&caller, operation, catalog.service_code).await;
                assert!(
                    !listed.contains(*withdrawn),
                    "case {case}: {operation} must not publish a code derived from a quota \
                     name, got {listed:?}"
                );
            }

            for operation in [
                "GetAWSDefaultServiceQuota",
                "GetServiceQuota",
                "RequestServiceQuotaIncrease",
            ] {
                let mut body =
                    json!({"ServiceCode": catalog.service_code, "QuotaCode": withdrawn});
                if operation == "RequestServiceQuotaIncrease" {
                    body["DesiredValue"] = json!(10_000.0);
                }
                let (status, error) = caller.refused(operation, body).await;
                assert_eq!(
                    (status, error.as_str()),
                    (400, "NoSuchResourceException"),
                    "case {case}: {operation} must refuse a code this service publishes \
                     nothing under"
                );
            }
        }

        // The SERVICE still resolves: it is the invented quota that is unpublished.
        let response = caller
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": catalog.service_code}),
            )
            .await;
        assert!(
            response["Quotas"].is_array(),
            "case {}: the service code must still resolve its listing, got {response}",
            catalog.service_code
        );
    }
}

/// A published row reports the facts AWS publishes for that quota, identically through the
/// listing and the single read, and the applied plane resolves the same key.
#[tokio::test]
async fn a_published_row_reports_the_facts_aws_publishes() {
    let engine = emulator();
    let region = "ap-south-1";
    let caller = client(&engine, region);

    for row in ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = caller.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(row.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!(row.unit),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(row.adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        assert_eq!(
            quota["GlobalQuota"],
            json!(false),
            "case {case}: AWS counts these quotas per Region, got {quota}"
        );
        assert!(
            !quota["Description"].as_str().unwrap_or_default().is_empty(),
            "case {case}: a published row carries the quota's description, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{}/{}",
                row.service_code, row.quota_code
            )),
            "case {case}: an AWS default quota's ARN names the Region and no account, got \
             {quota}"
        );
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes the quota over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        let metric = metric_json(&row.metric);
        if metric.is_null() {
            assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {quota}"
            );
        } else {
            assert_eq!(
                quota["UsageMetric"], metric,
                "case {case}: expected the usage metric the service publishes, got {quota}"
            );
        }

        // The listing carries exactly the record the single read returned.
        let listed = defaults(&caller, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );

        // The applied plane is keyed on the same code, so a client that read a code out of
        // the default catalog resolves its value in force too.
        let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (
                applied["QuotaCode"].clone(),
                applied["QuotaName"].clone(),
                applied["Value"].clone()
            ),
            (
                json!(row.quota_code),
                json!(row.quota_name),
                json!(row.value)
            ),
            "case {case}: the applied read resolves the same quota at its default value, got \
             {applied}"
        );
    }
}
