// SPDX-License-Identifier: CC0-1.0

//! What `ListAWSDefaultServiceQuotas` may publish as a service's DEFAULT-QUOTA SET,
//! over the wire.
//!
//! The rule: for a `ServiceCode`, the listing is the set of default quotas AWS
//! publishes for that service, and every row is keyed on the `QuotaCode` AWS assigns
//! the quota — a code in AWS's minted `L-` namespace — so a client that enumerates a
//! service to resolve a code, read its default `Value` or test its `Adjustable` flag
//! reaches the quota under the identifier AWS gave it, and no row answers to a code
//! AWS does not publish.
//!
//! The suites drive a TABLE of `ServiceCode`s through the listing, the single read and
//! a paged walk. Rows differ in the SIZE of the published set (from a handful to
//! several dozen), in the SHAPE of the quotas in it (countable, measured in a unit
//! other than `None`, not adjustable, a rate with a `Period` and a `UsageMetric`) and
//! in the naming form the service publishes ("… per Region", "Maximum number of …",
//! a bare resource name), and the spot-checked rows are quotas the issue behind this
//! suite never cited.
//!
//! SOURCE for every expectation below: AWS's published Service Quotas data for the
//! service code (what `aws service-quotas list-aws-default-service-quotas
//! --service-code <code>` reports), which is the same source
//! `src/state.rs::seed_quota_catalog` carries.
//!
//! The harness (`client`, `emulator`, `walk_pages`) is the one `quota_workflow.rs`
//! owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::descriptive_quota_code;

// The minted-code rule and the catalog walkers are stated ONCE for this target:
// `catalog_identity.rs` owns the rule, `quota_workflow.rs` owns the harness.
use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{client, default_quotas, emulator, walk_pages, Client};

/// One spot-checked row of a service's published set: the quota as AWS publishes it.
struct Published {
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
}

/// One row of the table: a service code, the number of default quotas AWS publishes
/// for it, and the rows spot-checked field for field.
struct ServiceRow {
    service_code: &'static str,
    service_name: &'static str,
    published_quotas: usize,
    spot_checks: &'static [Published],
}

/// The table: the eight service codes whose published sets this suite pins, with the
/// size of each set and the rows checked field for field.
const SERVICES: &[ServiceRow] = &[
    ServiceRow {
        service_code: "apigateway",
        service_name: "Amazon API Gateway",
        published_quotas: 51,
        spot_checks: &[
            // A plain countable quota, and a per-key one, neither cited by the issue.
            Published {
                quota_code: "L-65B5C802",
                quota_name: "Usage plans per API key",
                value: 10.0,
                unit: "None",
                adjustable: true,
            },
            // A quota measured in a unit other than `None`, published as fixed.
            Published {
                quota_code: "L-8E6A5A87",
                quota_name: "Integration timeout",
                value: 29_000.0,
                unit: "Milliseconds",
                adjustable: false,
            },
        ],
    },
    ServiceRow {
        service_code: "appconfig",
        service_name: "AWS AppConfig",
        published_quotas: 8,
        spot_checks: &[Published {
            quota_code: "L-EEB0151E",
            quota_name: "Maximum number of environments per application",
            value: 100.0,
            unit: "None",
            adjustable: true,
        }],
    },
    ServiceRow {
        service_code: "application-autoscaling",
        service_name: "Application Auto Scaling",
        published_quotas: 16,
        spot_checks: &[
            Published {
                quota_code: "L-782A3EE2",
                quota_name: "Scalable targets for ECS",
                value: 500.0,
                unit: "None",
                adjustable: true,
            },
            Published {
                quota_code: "L-D75CA9D2",
                quota_name: "Scheduled actions per scalable target",
                value: 200.0,
                unit: "None",
                adjustable: false,
            },
        ],
    },
    ServiceRow {
        service_code: "appmesh",
        service_name: "AWS App Mesh",
        published_quotas: 11,
        spot_checks: &[Published {
            quota_code: "L-BB90B7FF",
            quota_name: "Routes per virtual router",
            value: 50.0,
            unit: "None",
            adjustable: true,
        }],
    },
    ServiceRow {
        service_code: "apprunner",
        service_name: "AWS App Runner",
        published_quotas: 6,
        spot_checks: &[Published {
            quota_code: "L-1D8C5BDD",
            quota_name: "Auto scaling configurations",
            value: 10.0,
            unit: "None",
            adjustable: true,
        }],
    },
    ServiceRow {
        service_code: "athena",
        service_name: "Amazon Athena",
        published_quotas: 30,
        spot_checks: &[
            // The replenishment rate of one API's calls, and the burst multiplier on
            // it, which AWS publishes as fixed.
            Published {
                quota_code: "L-076245C0",
                quota_name: "Replenishment rate of CreateNamedQuery API calls",
                value: 5.0,
                unit: "None",
                adjustable: true,
            },
            Published {
                quota_code: "L-5A8D5237",
                quota_name: "Burst multiplier quota for CreateNamedQuery API",
                value: 2.0,
                unit: "None",
                adjustable: false,
            },
            Published {
                quota_code: "L-0F303755",
                quota_name: "DDL query timeout",
                value: 600.0,
                unit: "Minutes",
                adjustable: false,
            },
        ],
    },
    ServiceRow {
        service_code: "autoscaling",
        service_name: "Amazon EC2 Auto Scaling",
        published_quotas: 9,
        spot_checks: &[Published {
            quota_code: "L-05CB8B12",
            quota_name: "Target groups per Auto Scaling group",
            value: 50.0,
            unit: "None",
            adjustable: false,
        }],
    },
    ServiceRow {
        service_code: "backup",
        service_name: "AWS Backup",
        published_quotas: 12,
        spot_checks: &[
            Published {
                quota_code: "L-7705D2CB",
                quota_name: "Controls per framework",
                value: 50.0,
                unit: "None",
                adjustable: true,
            },
            // The largest published default in the table: a count in the millions.
            Published {
                quota_code: "L-514878B6",
                quota_name: "Recovery points per backup vault",
                value: 1_000_000.0,
                unit: "None",
                adjustable: true,
            },
        ],
    },
];

/// Every AWS default quota of `service_code`, walked at `page_size`: the shared
/// harness walks the whole set ([`default_quotas`]), and a named page size goes
/// through the one page walker beside it.
async fn defaults(client: &Client, service_code: &str, page_size: Option<i64>) -> Vec<Value> {
    match page_size {
        None => default_quotas(client, service_code).await,
        Some(size) => {
            walk_pages(
                client,
                "ListAWSDefaultServiceQuotas",
                &json!({"ServiceCode": service_code}),
                "Quotas",
                Some(size),
            )
            .await
        }
    }
}

/// Every published row is keyed on the code AWS assigns the quota, the set is the one
/// AWS publishes for the service, and each row reads back field for field through
/// `GetAWSDefaultServiceQuota`.
#[tokio::test]
async fn a_service_publishes_its_aws_default_quotas_under_the_codes_aws_assigns() {
    let engine = emulator();
    let region = "us-west-2";
    let client = client(&engine, region);

    for row in SERVICES {
        let case = row.service_code;
        let listed = defaults(&client, row.service_code, None).await;

        assert_eq!(
            listed.len(),
            row.published_quotas,
            "case {case}: expected the {} default quotas AWS publishes for the service, got {}",
            row.published_quotas,
            listed.len()
        );

        let mut codes: BTreeSet<String> = BTreeSet::new();
        for quota in &listed {
            let code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a quota without a code: {quota}"))
                .to_string();
            let name = quota["QuotaName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a quota without a name: {quota}"))
                .to_string();

            assert!(
                is_aws_minted_shape(&code),
                "case {case}/{name}: every published row is keyed on the code AWS assigns the \
                 quota, got {code:?}"
            );
            assert!(
                codes.insert(code.clone()),
                "case {case}: a QuotaCode identifies one quota, got {code:?} twice"
            );
            assert_eq!(
                (
                    quota["ServiceCode"].as_str(),
                    quota["ServiceName"].as_str(),
                    quota["GlobalQuota"].as_bool(),
                    quota["QuotaArn"].as_str(),
                ),
                (
                    Some(row.service_code),
                    Some(row.service_name),
                    Some(false),
                    Some(
                        format!(
                            "arn:aws:servicequotas:{region}::{}/{code}",
                            row.service_code
                        )
                        .as_str()
                    ),
                ),
                "case {case}/{code}: expected the service's own regional default row, got {quota}"
            );

            // The single read of the pair is the listed row, so a client that resolved
            // a code out of the listing reads the same quota back by it.
            let single = client
                .ok(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": row.service_code, "QuotaCode": code.clone()}),
                )
                .await;
            assert_eq!(
                single["Quota"], *quota,
                "case {case}/{code}: the single default read must be the listed row, got {single}"
            );

            // And nothing answers to a code built from the quota's NAME: a code AWS
            // does not publish resolves no quota of this service.
            let invented = descriptive_quota_code(&name);
            let (status, error) = client
                .refused(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": row.service_code, "QuotaCode": invented.clone()}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case}/{invented}: a code AWS does not publish must resolve nothing"
            );
        }

        for spot in row.spot_checks {
            let quota = listed
                .iter()
                .find(|quota| quota["QuotaCode"] == json!(spot.quota_code))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}/{}: expected the published quota, got the codes {codes:?}",
                        spot.quota_code
                    )
                });
            assert_eq!(
                (
                    quota["QuotaName"].as_str(),
                    quota["Value"].as_f64(),
                    quota["Unit"].as_str(),
                    quota["Adjustable"].as_bool(),
                ),
                (
                    Some(spot.quota_name),
                    Some(spot.value),
                    Some(spot.unit),
                    Some(spot.adjustable),
                ),
                "case {case}/{}: expected the quota as AWS publishes it, got {quota}",
                spot.quota_code
            );
        }
    }
}

/// A quota AWS publishes as a RATE carries the window it is measured over and the
/// `AWS/Usage` metric that meters it; a quota AWS publishes usage counts for carries
/// its resource-count metric. Checked on rows the issue behind this suite did not
/// cite.
#[tokio::test]
async fn a_measured_default_quota_publishes_the_metric_aws_meters_it_with() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    // Rows: a per-API call-rate quota (metered by `CallCount` over one second) and a
    // resource-count quota (metered by `ResourceCount`), each read out of the listing
    // it belongs to.
    for (service_code, quota_code, metric_name, statistic, period) in [
        (
            "athena",
            "L-C60BEAE0",
            "CallCount",
            "Sum",
            json!({"PeriodValue": 1, "PeriodUnit": "SECOND"}),
        ),
        (
            "autoscaling",
            "L-CDE20ADC",
            "ResourceCount",
            "Maximum",
            json!({"PeriodValue": 5, "PeriodUnit": "MINUTE"}),
        ),
    ] {
        let case = format!("{service_code}/{quota_code}");
        let listed = defaults(&client, service_code, None).await;
        let quota = listed
            .iter()
            .find(|quota| quota["QuotaCode"] == json!(quota_code))
            .unwrap_or_else(|| panic!("case {case}: expected the published quota"));

        assert_eq!(
            (
                quota["UsageMetric"]["MetricNamespace"].as_str(),
                quota["UsageMetric"]["MetricName"].as_str(),
                quota["UsageMetric"]["MetricStatisticRecommendation"].as_str(),
            ),
            (Some("AWS/Usage"), Some(metric_name), Some(statistic)),
            "case {case}: expected the AWS/Usage metric AWS meters the quota with, got {quota}"
        );
        assert_eq!(
            quota["Period"], period,
            "case {case}: expected the window the quota is measured over, got {quota}"
        );
        assert!(
            quota["UsageMetric"]["MetricDimensions"]["Service"]
                .as_str()
                .is_some(),
            "case {case}: every AWS/Usage metric names the service its usage belongs to, got \
             {quota}"
        );
    }
}

/// The published set does not depend on how it is paged: the same rows, in the same
/// order, whatever page size the walk asks for — including a set far longer than the
/// page sizes used here.
#[tokio::test]
async fn the_published_set_is_the_same_however_it_is_paged() {
    let engine = emulator();
    let client = client(&engine, "ap-northeast-1");

    // Rows: the largest published set in the table and the smallest, walked whole and
    // then at page sizes above and below their lengths.
    for service_code in ["apigateway", "apprunner"] {
        let whole = defaults(&client, service_code, None).await;
        for size in [1_i64, 7, 100] {
            let paged = defaults(&client, service_code, Some(size)).await;
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
