// SPDX-License-Identifier: CC0-1.0

//! What `ListAWSDefaultServiceQuotas` publishes for a service whose whole default-quota
//! catalog sits in AWS's own `QuotaCode` namespace, over the wire.
//!
//! `QuotaCode` is the only stable key Service Quotas exposes — `GetAWSDefaultServiceQuota`,
//! `GetServiceQuota` and `RequestServiceQuotaIncrease` all take it — so the rules asserted
//! here are about the IDENTITY and the CONTENT of a service's published catalog rather than
//! about one recorded listing:
//!
//! * a service's whole published catalog is served, every row keyed on the `L-XXXXXXXX`
//!   code AWS mints for that quota, with the service's own `ServiceCode`/`ServiceName` and
//!   a description on every row, and one key per quota;
//! * a code built out of a quota's NAME is published nowhere and resolves nowhere, so a
//!   client cannot read back a quota this emulator invented;
//! * a published row reports the `QuotaName`, `Value`, `Unit`, `Adjustable`, `GlobalQuota`,
//!   `Period` and `UsageMetric` AWS publishes for the quota, with a `QuotaArn` naming the
//!   Region it was read at and no account, identically through the listing and through
//!   `GetAWSDefaultServiceQuota`, and the same key resolves in the APPLIED plane.
//!
//! Each rule is driven over a TABLE of ten `ServiceCode`s whose catalogs differ in length
//! (5 rows to 43), in what they count (directory, table, volume, image, cluster, cache,
//! application, file system, load balancer and search-domain planes), and in which members
//! their rows carry; the pinned rows span counts and measured quotas (`None`, `TiB`,
//! `Bytes`, `Gigabytes`, `Megabytes/Second`), adjustable and fixed rows, no window / a
//! one-second rate window / the five-minute window a resource count is published over, and
//! unmetered rows beside both `AWS/Usage` metric shapes (`CallCount` and `ResourceCount`).
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
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response for each of these service
/// codes — every row of it carries an `L-XXXXXXXX` `QuotaCode`, and the count is the
/// number of rows that response holds. `withdrawn` names the codes that response reports
/// as surplus entries, which are exactly the stand-ins this catalog used to key these
/// services' rows on.
const CATALOGS: &[Catalog] = &[
    Catalog {
        service_code: "ds",
        service_name: "AWS Directory Service",
        quota_count: 6,
        withdrawn: &["Directories"],
    },
    Catalog {
        service_code: "dynamodb",
        service_name: "Amazon DynamoDB",
        quota_count: 13,
        withdrawn: &[
            "AccountLevelWriteThroughputLimit",
            "GlobalSecondaryIndexesPerTable",
        ],
    },
    Catalog {
        service_code: "ebs",
        service_name: "Amazon Elastic Block Store (Amazon EBS)",
        quota_count: 43,
        withdrawn: &["SnapshotsPerRegion", "StorageForGeneralPurposeSSDGp3VolumesInTiB"],
    },
    Catalog {
        service_code: "ecr",
        service_name: "Amazon Elastic Container Registry (Amazon ECR)",
        quota_count: 23,
        withdrawn: &["Repositories", "ImagesPerRepository"],
    },
    Catalog {
        service_code: "eks",
        service_name: "Amazon Elastic Kubernetes Service (Amazon EKS)",
        quota_count: 13,
        withdrawn: &[
            "Clusters",
            "ManagedNodeGroupsPerCluster",
            "NodesPerManagedNodeGroup",
        ],
    },
    Catalog {
        service_code: "elasticache",
        service_name: "Amazon ElastiCache",
        quota_count: 12,
        withdrawn: &["NodesPerRegion", "ParameterGroupsPerRegion"],
    },
    Catalog {
        service_code: "elasticbeanstalk",
        service_name: "AWS Elastic Beanstalk",
        quota_count: 5,
        withdrawn: &["Applications", "Environments"],
    },
    Catalog {
        service_code: "elasticfilesystem",
        service_name: "Amazon Elastic File System (Amazon EFS)",
        quota_count: 23,
        withdrawn: &["FileSystemsPerAccount", "AccessPointsPerFileSystem"],
    },
    Catalog {
        service_code: "elasticloadbalancing",
        service_name: "Elastic Load Balancing (ELB)",
        quota_count: 31,
        withdrawn: &[
            "ApplicationLoadBalancersPerRegion",
            "ClassicLoadBalancersPerRegion",
            "TargetGroupsPerRegion",
        ],
    },
    Catalog {
        service_code: "es",
        service_name: "Amazon OpenSearch Service",
        quota_count: 5,
        withdrawn: &["DomainsPerRegion", "InstancesPerDomain"],
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
    description: &'static str,
    /// `(PeriodValue, PeriodUnit)` — the window the quota is published over, for a row
    /// that is published over one.
    period: Option<(i32, &'static str)>,
    metric: Metric,
}

/// The rows this suite pins, at most two per service so the table spans the ten catalogs
/// rather than repeating one.
///
/// SOURCE: AWS's published Service Quotas data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>` reports),
/// and the recorded default-quota listing of each code for which `QuotaCode` names which
/// quota. Only two of the twenty rows name a quota the issue cited, so the rule is
/// asserted on quotas beyond the reported instances.
const ROWS: &[Row] = &[
    Row {
        service_code: "ds",
        quota_code: "L-A18F1612",
        quota_name: "Simple AD manual snapshots",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of manual snapshots that you can take for your Simple \
                      AD directory.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "ds",
        quota_code: "L-092080C3",
        quota_name: "AD Connector directories",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of AD Connector directories that you can create in \
                      this account in the current Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "dynamodb",
        quota_code: "L-AB614373",
        quota_name: "Table-level write throughput limit",
        value: 40_000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of write capacity units that can be provisioned for a \
                      single table.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "dynamodb",
        quota_code: "L-F98FE922",
        quota_name: "Maximum number of tables",
        value: 2500.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of tables that can be created in this account in the \
                      current Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "DynamoDB",
            resource: "TableCount",
        },
    },
    Row {
        service_code: "ebs",
        quota_code: "L-D18FCD1D",
        quota_name: "Storage for General Purpose SSD (gp2) volumes, in TiB",
        value: 50.0,
        unit: "TiB",
        adjustable: true,
        description: "The maximum aggregated storage, in TiB, of all General Purpose SSD (gp2) \
                      volumes in this account in the current Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "ebs",
        quota_code: "L-835364B2",
        quota_name: "Rate of GetSnapshotBlock requests",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of GetSnapshotBlock requests you can make per second \
                      in this account in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "EBS",
            api: "GetSnapshotBlock",
        },
    },
    Row {
        service_code: "ecr",
        quota_code: "L-241DEEBA",
        quota_name: "Repository policy length",
        value: 10_240.0,
        unit: "Bytes",
        adjustable: false,
        description: "The maximum length, in bytes, of one repository's policy document.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "ecr",
        quota_code: "L-343F3D94",
        quota_name: "Maximum image layer size",
        value: 52.0,
        unit: "Gigabytes",
        adjustable: false,
        description: "The maximum size, in GiB, of one image layer.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-6D54EA21",
        quota_name: "Control plane security groups per cluster",
        value: 4.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of control plane security groups per cluster.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "eks",
        quota_code: "L-C56B9FC3",
        quota_name: "Pod identity associations per cluster",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of EKS Pod Identity associations per cluster.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "elasticache",
        quota_code: "L-3E7F7726",
        quota_name: "Nodes per Region",
        value: 300.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of nodes in this account in the current Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "ElastiCache",
            resource: "NodesPerRegion",
        },
    },
    Row {
        service_code: "elasticache",
        quota_code: "L-A87EE522",
        quota_name: "Subnets per subnet group",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of subnets in one subnet group.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "elasticbeanstalk",
        quota_code: "L-8EFC1C51",
        quota_name: "Application versions",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of application versions in this account in the \
                      current Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "elasticbeanstalk",
        quota_code: "L-E593A077",
        quota_name: "Custom platform versions",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of custom platform versions in this account in the \
                      current Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-4CCB99B3",
        quota_name: "Total provisioned throughput per Region",
        value: 3072.0,
        unit: "Megabytes/Second",
        adjustable: true,
        description: "The maximum throughput, in MiB/s, that can be provisioned across all \
                      file systems in this account in the current Region.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "elasticfilesystem",
        quota_code: "L-BFB6FB19",
        quota_name: "Rate of DescribeFileSystems requests",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of DescribeFileSystems requests you can make per \
                      second in this account in the current Region.",
        period: Some((1, "SECOND")),
        metric: Metric::Calls {
            usage_service: "EFS",
            api: "DescribeFileSystems",
        },
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-057ECE54",
        quota_name: "Application Load Balancers per Region",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of Application Load Balancers in this account in the \
                      current Region.",
        period: Some((5, "MINUTE")),
        metric: Metric::Resources {
            usage_service: "Elastic Load Balancing",
            resource: "ApplicationLoadBalancersPerRegion",
        },
    },
    Row {
        service_code: "elasticloadbalancing",
        quota_code: "L-B22855CB",
        quota_name: "Load balancers per target group",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of load balancers one target group can be used by.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "es",
        quota_code: "L-1F053E6F",
        quota_name: "Warm instances per domain",
        value: 150.0,
        unit: "None",
        adjustable: false,
        description: "The maximum number of UltraWarm instances per domain.",
        period: None,
        metric: Metric::None,
    },
    Row {
        service_code: "es",
        quota_code: "L-E9BC8C95",
        quota_name: "Packages per domain",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: "The maximum number of packages that can be associated with one domain.",
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
    for region in ["us-west-2", "eu-central-1"] {
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
    let region = "ap-northeast-1";
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
        assert_eq!(
            quota["Description"],
            json!(row.description),
            "case {case}: a description is the service's own published text, got {quota}"
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
