// SPDX-License-Identifier: CC0-1.0

//! Which `QuotaCode` a default-quota row may be published under, over the wire.
//!
//! `QuotaCode` is the only key Service Quotas exposes: it is what
//! `GetAWSDefaultServiceQuota`, `GetServiceQuota` and `RequestServiceQuotaIncrease`
//! take, so a row published under a code the real service answers to nothing under
//! hands a client a key no other API accepts. The rules asserted here, each over a
//! TABLE of inputs rather than one case:
//!
//! * a service's quotas are published ONLY under the code the service answers to —
//!   AWS's minted `L-XXXXXXXX` code for every row of the services below — and a code
//!   this catalog derived from a quota NAME for one of them is published nowhere: not
//!   in either listing, and not resolvable by either single read;
//! * dropping those rows does not drop the SERVICE: each still resolves both listings
//!   with HTTP 200 and a `Quotas` array, which is what a client enumerating
//!   `ListServices` and calling per code sees;
//! * a row this catalog re-keyed onto AWS's own code reports the facts AWS publishes
//!   for that quota, through the listing and the single read alike, and the whole
//!   class of row it belongs to is built the one way (KMS's per-operation request
//!   rates: the name, the one-second window, the `AWS/Usage` `CallCount` metric of the
//!   operation and the published sentence).
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns, so this suite speaks to the emulator the way the other catalog suites do.

use std::collections::BTreeSet;

use serde_json::json;

// `is_aws_minted_shape` is the ONE statement of what AWS's `QuotaCode` namespace is:
// it is owned by `catalog_identity.rs`, which asserts the catalog-wide rule that a code
// in that namespace is published only where this catalog cites it, so the two suites
// cannot disagree about what the namespace is or carry a second copy of the test.
use crate::catalog_identity::is_aws_minted_shape;
use crate::quota_workflow::{client, default_quotas as defaults, emulator, walk_pages, Client};

/// The services whose published default quotas AWS keys ENTIRELY on its own `L-` codes,
/// each with the code this catalog used to derive from a quota name for it.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response for each of these
/// service codes — every row of it carries an `L-XXXXXXXX` `QuotaCode`, and the derived
/// code below appears in none of them (the recording reports the emulator's row as a
/// surplus entry).
///
/// The rows span the shapes the rule has to hold over: a service that now publishes no
/// default quota at all (most of them), one that publishes a counted resource quota
/// (`kafka`), one that publishes metered request rates (`kms`), a service with one
/// derived code and services with two or three, and codes derived from a resource name
/// as well as from a throttle name.
const DERIVED_CODES: &[(&str, &str)] = &[
    ("iotevents", "DetectorModelsPerAccount"),
    ("kafka", "BrokersPerCluster"),
    ("kafka", "ClustersPerAccount"),
    ("kendra", "IndexesPerAccount"),
    ("kms", "CustomerManagedKMSKeys"),
    ("kms", "AliasesPerKMSKey"),
    ("kms", "GrantsPerKMSKey"),
    ("lakeformation", "DataLakeAdministratorsPerAccount"),
    ("lex", "BotsPerAccount"),
    ("license-manager", "LicenseConfigurationsPerAccount"),
    ("logs", "MetricFiltersPerLogGroup"),
    ("logs", "SubscriptionFiltersPerLogGroup"),
    ("logs", "DescribeLogStreamsThrottleLimitInTransactionsPerSecond"),
    ("medialive", "ChannelsPerRegion"),
    ("mediapackage", "ChannelsPerRegion"),
];

/// Every `ServiceCode` [`DERIVED_CODES`] covers, once.
fn covered_services() -> BTreeSet<&'static str> {
    DERIVED_CODES.iter().map(|(code, _)| *code).collect()
}

/// One KMS per-operation request-rate quota, as AWS publishes it.
struct RequestRate {
    quota_code: &'static str,
    /// The API operation the rate throttles — the variable part of every member.
    api_name: &'static str,
    requests_per_second: f64,
}

/// The rows this catalog publishes under AWS's own code for a service
/// [`DERIVED_CODES`] covers, beside the KMS request rates below.
///
/// `(ServiceCode, QuotaCode, QuotaName, Value, Adjustable)`.
const RE_KEYED_ROWS: &[(&str, &str, &str, f64, bool)] =
    &[("kafka", "L-FAB9E493", "Number of brokers per cluster", 30.0, true)];

/// The KMS request rates this catalog publishes.
const KMS_REQUEST_RATES: &[RequestRate] = &[
    RequestRate {
        quota_code: "L-8A08DAEA",
        api_name: "ListKeys",
        requests_per_second: 500.0,
    },
    RequestRate {
        quota_code: "L-275D92F3",
        api_name: "UpdateCustomKeyStore",
        requests_per_second: 5.0,
    },
];

/// The `QuotaCode`s a service's AWS-default listing reports.
async fn listed_codes(client: &Client, service_code: &str) -> BTreeSet<String> {
    defaults(client, service_code)
        .await
        .iter()
        .filter_map(|quota| quota["QuotaCode"].as_str().map(str::to_string))
        .collect()
}

/// A code this catalog derived from a quota name for one of these services is published
/// nowhere: neither listing carries it and neither single read resolves it.
#[tokio::test]
async fn a_code_derived_from_a_quota_name_is_not_published() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    for (service_code, derived) in DERIVED_CODES {
        let case = format!("{service_code}/{derived}");
        let listed = listed_codes(&client, service_code).await;
        assert!(
            !listed.contains(*derived),
            "case {case}: a code derived from a quota name must not be published, got {listed:?}"
        );

        // The applied listing resolves the same keys, so the derived code is absent
        // there too.
        let applied: BTreeSet<String> = walk_pages(
            &client,
            "ListServiceQuotas",
            &json!({"ServiceCode": service_code}),
            "Quotas",
            None,
        )
        .await
        .iter()
        .filter_map(|quota| quota["QuotaCode"].as_str().map(str::to_string))
        .collect();
        assert!(
            !applied.contains(*derived),
            "case {case}: the applied listing publishes the same keys as the defaults, got \
             {applied:?}"
        );

        for operation in [
            "GetAWSDefaultServiceQuota",
            "GetServiceQuota",
            "RequestServiceQuotaIncrease",
        ] {
            let mut body = json!({"ServiceCode": service_code, "QuotaCode": derived});
            if operation == "RequestServiceQuotaIncrease" {
                body["DesiredValue"] = json!(1_000.0);
            }
            let (status, error) = client.refused(operation, body).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case}: {operation} must refuse a code this service publishes nothing under"
            );
        }
    }
}

/// Every code these services DO publish is one of AWS's own, and the service itself
/// still resolves both listings.
#[tokio::test]
async fn every_published_code_of_these_services_is_an_aws_minted_code() {
    let engine = emulator();

    // Two endpoints, because the catalog is published per Region and the rule is about
    // the catalog, not about one region's view of it.
    for region in ["us-west-2", "eu-west-1"] {
        let client = client(&engine, region);

        for service_code in covered_services() {
            let case = format!("{region} {service_code}");
            for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
                let response = client
                    .ok(operation, json!({"ServiceCode": service_code}))
                    .await;
                let quotas = response["Quotas"].as_array().unwrap_or_else(|| {
                    panic!("case {case}: {operation} must resolve a Quotas list, got {response}")
                });
                for quota in quotas {
                    let code = quota["QuotaCode"].as_str().unwrap_or_default();
                    assert!(
                        is_aws_minted_shape(code),
                        "case {case}: {operation} may publish only AWS's own quota codes, got \
                         {code:?} in {quota}"
                    );
                }
            }
        }
    }
}

/// A row re-keyed onto AWS's own code reports the facts AWS publishes for that quota,
/// through the listing and the single read alike, with the default ARN naming the
/// Region and no account.
#[tokio::test]
async fn a_re_keyed_row_reports_the_facts_aws_publishes() {
    let engine = emulator();
    let region = "ap-northeast-1";
    let client = client(&engine, region);

    for (service_code, quota_code, quota_name, value, adjustable) in RE_KEYED_ROWS {
        let case = format!("{service_code}/{quota_code}");
        let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!("None"),
            "case {case}: a counted quota reports no unit, got {quota}"
        );
        assert_eq!(
            quota["GlobalQuota"],
            json!(false),
            "case {case}: this quota is counted per Region, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!(
                "arn:aws:servicequotas:{region}::{service_code}/{quota_code}"
            )),
            "case {case}: an AWS default quota's ARN names the Region and no account, got {quota}"
        );

        let listed = defaults(&client, service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }
}

/// Every KMS per-operation request rate is published the one way its class is: the
/// operation's name, a one-second window, that operation's `AWS/Usage` call count, and
/// the sentence KMS publishes for the class with the operation named in it.
#[tokio::test]
async fn a_kms_request_rate_is_published_as_its_whole_class_is() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    let listed = defaults(&client, "kms").await;

    for rate in KMS_REQUEST_RATES {
        let case = format!("kms/{}", rate.quota_code);
        let pair = json!({"ServiceCode": "kms", "QuotaCode": rate.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(format!("{} request rate", rate.api_name)),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(rate.requests_per_second),
            "case {case}: expected the published requests per second, got {quota}"
        );
        assert_eq!(
            quota["Period"],
            json!({"PeriodValue": 1, "PeriodUnit": "SECOND"}),
            "case {case}: a request rate is published over a one-second window, got {quota}"
        );
        assert_eq!(
            quota["Description"],
            json!(format!(
                "Maximum {} requests per second. When you reach this quota, KMS rejects requests \
                 for this operation for the remainder of the interval.",
                rate.api_name
            )),
            "case {case}: expected the published description of the class, got {quota}"
        );
        assert_eq!(
            quota["UsageMetric"],
            json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Service": "KMS",
                    "Resource": rate.api_name,
                    "Type": "API",
                    "Class": "None",
                },
            }),
            "case {case}: a request rate is metered by that operation's call count, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(true),
            "case {case}: KMS publishes its request rates as adjustable, got {quota}"
        );

        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );
    }

    // The two rows differ in the operation they throttle and in their value, so the
    // class is asserted over more than one shape of row.
    let values: BTreeSet<String> = listed
        .iter()
        .map(|quota| quota["Value"].to_string())
        .collect();
    assert_eq!(
        values.len(),
        KMS_REQUEST_RATES.len(),
        "case kms: the rows must span more than one published value, got {values:?}"
    );
    let names: BTreeSet<&str> = listed
        .iter()
        .filter_map(|quota| quota["QuotaName"].as_str())
        .collect();
    assert_eq!(
        names.len(),
        KMS_REQUEST_RATES.len(),
        "case kms: each row names its own operation, got {names:?}"
    );
}

/// A value that is not a published quota record is never fabricated by the reads this
/// suite drives: the derived codes are gone and a well-formed code nothing publishes is
/// still refused, so no listing is padded to look complete.
#[tokio::test]
async fn a_code_nothing_publishes_is_refused_for_every_covered_service() {
    let engine = emulator();
    let client = client(&engine, "eu-central-1");

    // Rows: a well-formed AWS code no service publishes, and one service's code read
    // against another service — both outside every covered service's published set.
    for service_code in covered_services() {
        for (quota_code, why) in [
            ("L-00000000", "a well-formed code nothing publishes"),
            ("L-F678F1CE", "a code another service publishes"),
        ] {
            if defaults(&client, service_code)
                .await
                .iter()
                .any(|quota| quota["QuotaCode"] == json!(quota_code))
            {
                continue;
            }
            let (status, error) = client
                .refused(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {service_code}/{quota_code} ({why}): expected a refusal"
            );
        }
    }
}
