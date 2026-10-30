// SPDX-License-Identifier: CC0-1.0

//! The Region restriction on `GetCostEstimate` — the one door that admits every
//! kind of record and so learns which kind a name addresses only after resolving
//! it — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is `instance_lifecycle`'s and the Region table and
//! refusal text are `domain_region_restriction`'s, so this file stands up no
//! second emulator and keeps no second copy of either.
//!
//! The rule under test: a record of a kind whose API Lightsail serves from the
//! N. Virginia (`us-east-1`) endpoint ALONE is reported only through that Region
//! WHICHEVER call reports it, so a cost estimate that resolves to one is refused
//! in every other Region with the modelled `InvalidInputException` (HTTP 400)
//! carrying the documented restriction — while a record of any other kind is
//! estimated from every Region its own reader serves it in.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::domain_region_restriction::{other_regions, DOMAIN_REGION, REFUSAL};
use crate::instance_lifecycle::{error_code, Emulator};

/// The hours AWS prices one Lightsail billing month at, so a window of this many
/// hours is exactly one plan's worth.
const BILLING_MONTH_HOURS: f64 = 730.0;

/// How far after the call the estimated window starts: every resource under test
/// is created before the call, so a window that opens later is billed whole and
/// the expected figures do not depend on how long the setup took.
const WINDOW_STARTS_IN_SECONDS: f64 = 86_400.0;

/// The instance the distribution originates at and the alarm monitors.
const ORIGIN_INSTANCE: &str = "cost-region-instance";

/// The CDN distribution — a record of a kind the restriction covers.
const DISTRIBUTION: &str = "cost-region-distribution";

/// The 50 GB CDN plan the distribution runs on.
const DISTRIBUTION_BUNDLE: &str = "small_1_0";

/// What one billing month of [`DISTRIBUTION_BUNDLE`] costs, in USD.
///
/// SOURCE: the CDN line of the Lightsail price list — $2.50 a month for the
/// 50 GB plan.
const DISTRIBUTION_USD_PER_MONTH: f64 = 2.5;

/// A DNS zone of this account: the second kind the restriction covers, whose
/// dotted name no cost-estimate request can carry (see
/// [`a_zone_name_never_reaches_the_cost_estimate_door`]).
const ZONE: &str = "cost-region.example";

/// A record addressed by its name alone whose own reader serves it from every
/// Region, with the modelled `ResourceType` an estimate of it reports.
///
/// The rows differ along the kind of record a name resolves to — a virtual
/// private server, a block storage disk, a snapshot of one, a reserved address,
/// a metric alarm — which is the one thing the rule turns on.
const UNRESTRICTED: &[(&str, &str)] = &[
    (ORIGIN_INSTANCE, "Instance"),
    ("cost-region-disk", "Disk"),
    ("cost-region-disk-snapshot", "DiskSnapshot"),
    ("cost-region-static-ip", "StaticIp"),
    ("cost-region-alarm", "Alarm"),
];

/// The bucket each Region gets one of: a record keyed by the Region TOGETHER
/// with its name, so the Region a request is made in is what resolves it at all.
///
/// A Lightsail bucket name is unique across the account's buckets whichever
/// Region holds them, so each Region's bucket is named for its Region.
fn bucket_in(region: &str) -> String {
    format!("cost-region-bucket-{region}")
}

/// Now, in epoch seconds.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The window every case asks about: one whole billing month, opening after the
/// setup is done.
fn window() -> (f64, f64) {
    let start = now_epoch_seconds() + WINDOW_STARTS_IN_SECONDS;
    (start, start + BILLING_MONTH_HOURS * 3_600.0)
}

/// Stand up an emulator holding one record of every kind this suite drives, each
/// created in the domain Region — the one Region every kind here can be created
/// in — plus one bucket per Region, which only its own Region resolves.
async fn emulator_with_every_kind() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "CreateDisk",
            json!({
                "diskName": "cost-region-disk",
                "availabilityZone": "us-east-1a",
                "sizeInGb": 32,
            }),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "CreateDiskSnapshot",
            json!({
                "diskName": "cost-region-disk",
                "diskSnapshotName": "cost-region-disk-snapshot",
            }),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "AllocateStaticIp",
            json!({"staticIpName": "cost-region-static-ip"}),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "PutAlarm",
            json!({
                "alarmName": "cost-region-alarm",
                "monitoredResourceName": ORIGIN_INSTANCE,
                "metricName": "CPUUtilization",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 80,
                "evaluationPeriods": 2,
            }),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "CreateDistribution",
            json!({
                "distributionName": DISTRIBUTION,
                "origin": {"name": ORIGIN_INSTANCE},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": DISTRIBUTION_BUNDLE,
            }),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(DOMAIN_REGION),
            "CreateDomain",
            json!({"domainName": ZONE}),
        )
        .await;
    // One bucket per Region: a bucket is keyed by its Region together with its
    // name, so the one a Region holds is the one estimated there.
    let mut regions: Vec<&str> = vec![DOMAIN_REGION];
    regions.extend(other_regions());
    for region in regions {
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateBucket",
                json!({"bucketName": bucket_in(region), "bundleId": "small_1_0"}),
            )
            .await;
    }
    emulator
}

/// The one estimate `GetCostEstimate` answers for `name` in `region` over
/// `[start, end)`, requiring the call to be served.
async fn estimate_in(
    emulator: &Emulator,
    region: Option<&str>,
    name: &str,
    start: f64,
    end: f64,
) -> Value {
    let case = format!("estimating {name} in {region:?}");
    let (status, body) = emulator
        .call_in(
            region,
            "GetCostEstimate",
            json!({"resourceName": name, "startTime": start, "endTime": end}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected the estimate to be served, got {status} {body:?}"
    );
    let estimates = body["resourcesBudgetEstimate"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a resourcesBudgetEstimate array, got {body:?}"))
        .clone();
    assert_eq!(
        estimates.len(),
        1,
        "case {case}: expected the one resource held under that name, got {estimates:?}"
    );
    estimates[0].clone()
}

/// RULE: a cost estimate that resolves to a record of a kind served from the
/// domain Region alone is refused in every other Region with the modelled
/// `InvalidInputException` (HTTP 400) carrying the documented restriction, and
/// the record is still estimated through its own Region afterwards.
#[tokio::test(flavor = "current_thread")]
async fn an_estimate_of_a_domain_region_record_is_refused_outside_that_region() {
    let emulator = emulator_with_every_kind().await;
    let (start, end) = window();

    for region in other_regions() {
        let case = format!("estimating the distribution in {region}");
        let (status, body) = emulator
            .call_in(
                Some(region),
                "GetCostEstimate",
                json!({"resourceName": DISTRIBUTION, "startTime": start, "endTime": end}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(REFUSAL),
            "case {case}: expected the documented restriction, got {:?}",
            body["message"]
        );

        // The refusal took nothing away: the same estimate is still served
        // through the Region the distribution's own API is served in.
        let served = estimate_in(&emulator, Some(DOMAIN_REGION), DISTRIBUTION, start, end).await;
        assert_eq!(
            served["resourceType"].as_str(),
            Some("Distribution"),
            "case {case}: expected the distribution to be estimable in {DOMAIN_REGION} after \
             the refusal, got {served:?}"
        );
    }
}

/// RULE: the same call IS served for that record through the domain Region — and
/// through a request carrying no credential scope at all, which the service
/// serves in its default Region, the domain Region — so the estimate reports the
/// record's own modelled `resourceType` and the plan it is stored on.
#[tokio::test(flavor = "current_thread")]
async fn an_estimate_of_a_domain_region_record_is_served_in_that_region() {
    let emulator = emulator_with_every_kind().await;
    let (start, end) = window();

    // The two ways a request names the domain Region: the scope says so, or
    // there is no scope to say otherwise.
    for (case, region) in [
        ("the us-east-1 credential scope", Some(DOMAIN_REGION)),
        ("no credential scope at all", None),
    ] {
        let estimate = estimate_in(&emulator, region, DISTRIBUTION, start, end).await;
        assert_eq!(
            estimate["resourceName"].as_str(),
            Some(DISTRIBUTION),
            "case {case}: expected the named distribution, got {estimate:?}"
        );
        assert_eq!(
            estimate["resourceType"].as_str(),
            Some("Distribution"),
            "case {case}: expected the modelled Distribution resource type, got {estimate:?}"
        );
        let charges = estimate["costEstimates"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a costEstimates array, got {estimate:?}"))
            .clone();
        assert_eq!(
            charges.len(),
            1,
            "case {case}: expected the one plan charge a distribution carries, got {charges:?}"
        );
        assert_eq!(
            charges[0]["usageType"].as_str(),
            Some(format!("BundleUsage:{DISTRIBUTION_BUNDLE}").as_str()),
            "case {case}: expected the charge to name the stored plan, got {:?}",
            charges[0]
        );
        let cost = charges[0]["resultsByTime"][0]["usageCost"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a usageCost, got {:?}", charges[0]));
        assert!(
            (cost - DISTRIBUTION_USD_PER_MONTH).abs() < 1e-6,
            "case {case}: expected ${DISTRIBUTION_USD_PER_MONTH} over the billing month, \
             got ${cost}"
        );
    }
}

/// RULE: a record of a kind whose own reader serves it from every Region is
/// estimated from every Region too — the restriction narrows exactly the kinds
/// served from the domain Region alone and nothing else.
///
/// The rows are the kinds held under a name alone, which every Region resolves,
/// plus the bucket — a record keyed by the Region together with its name, so the
/// one each Region holds is estimated there.
#[tokio::test(flavor = "current_thread")]
async fn an_estimate_of_every_other_kind_is_served_in_every_region() {
    let emulator = emulator_with_every_kind().await;
    let (start, end) = window();

    for region in other_regions() {
        let bucket = bucket_in(region);
        let rows: Vec<(&str, &str)> = UNRESTRICTED
            .iter()
            .copied()
            .chain([(bucket.as_str(), "Bucket")])
            .collect();
        for (name, resource_type) in rows {
            let case = format!("estimating {name} in {region}");
            let estimate = estimate_in(&emulator, Some(region), name, start, end).await;
            assert_eq!(
                estimate["resourceName"].as_str(),
                Some(name),
                "case {case}: expected the named resource, got {estimate:?}"
            );
            assert_eq!(
                estimate["resourceType"].as_str(),
                Some(resource_type),
                "case {case}: expected the modelled {resource_type} resource type, \
                 got {estimate:?}"
            );
        }
    }
}

/// RULE: the DNS zone is the other kind the restriction covers that this store
/// holds under a name alone, and no cost-estimate request reaches one at all: a
/// zone is named by a dotted domain name, which the modelled
/// `GetCostEstimate$resourceName` pattern (`^\w[\w\-]*\w$`) does not admit, so
/// the centralized constraint walk refuses the request with its own
/// `ValidationException` (HTTP 400) in every Region — the domain Region
/// included — and the zone is still the account's afterwards.
#[tokio::test(flavor = "current_thread")]
async fn a_zone_name_never_reaches_the_cost_estimate_door() {
    let emulator = emulator_with_every_kind().await;
    let (start, end) = window();

    let mut regions: Vec<&str> = vec![DOMAIN_REGION];
    regions.extend(other_regions());
    for region in regions {
        let case = format!("estimating the zone in {region}");
        let (status, body) = emulator
            .call_in(
                Some(region),
                "GetCostEstimate",
                json!({"resourceName": ZONE, "startTime": start, "endTime": end}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the walk's refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected the boundary walk's ValidationException, got {body:?}"
        );

        let zone = emulator
            .ok_at(
                None,
                Some(DOMAIN_REGION),
                "GetDomain",
                json!({"domainName": ZONE}),
            )
            .await["domain"]
            .clone();
        assert_eq!(
            zone["name"].as_str(),
            Some(ZONE),
            "case {case}: expected the zone to be untouched, got {zone:?}"
        );
    }
}
