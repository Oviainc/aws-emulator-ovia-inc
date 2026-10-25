// SPDX-License-Identifier: CC0-1.0

//! `GetCostEstimate`: what the resource a request names will cost over the window
//! it asks about, driven over the same front door a client uses.
//!
//! The rule under test: for the resource the account holds under the requested
//! name, the estimate reports that resource's modelled `resourceType`, the window
//! the request asked about, and one usage type per metered dimension the stored
//! record carries — the monthly plan it runs on, the storage it holds, or the flat
//! hourly price of holding it — each with the count of pricing units the window
//! consumes (`unit`) and that count times the published price (`usageCost`) in
//! USD. The window is billed only from the instant the resource was created, and a
//! name the account holds no resource under is refused rather than estimated at
//! zero.
//!
//! So the cases are TABLES: one row per resource type the store holds (and so per
//! pricing basis), and one row per window length and position relative to the
//! resource's creation. The expected figures are this suite's own reading of the
//! published Lightsail price list, not the service's constants.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::get_cost_estimate::billed_resources;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    ComAmazonawsLightsailLightsail20161128State as State, StoredDomain, StoredLocation,
    StoredStaticIp,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// The hours AWS prices one Lightsail billing month at.
///
/// SOURCE: AWS bills a monthly Lightsail plan by the hour at `price / 730`, so a
/// window of 730 hours is exactly one plan's worth. Every expected figure below
/// is stated per such a window.
const BILLING_MONTH_HOURS: f64 = 730.0;

/// How far after the call the estimated window starts.
///
/// Every resource under test is created before the call, so a window that starts
/// later is billed whole: that makes the expected unit counts exact rather than
/// dependent on how long the setup took.
const WINDOW_STARTS_IN_SECONDS: f64 = 86_400.0;

/// Now, in epoch seconds.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// One usage type an estimate must carry, with the figures one billing month of
/// it comes to.
#[derive(Debug)]
struct Charge {
    /// `CostEstimate$usageType`.
    usage_type: &'static str,
    /// The modelled `PricingUnit` value.
    pricing_unit: &'static str,
    /// Pricing units consumed over ONE billing month.
    unit: f64,
    /// USD over ONE billing month.
    cost: f64,
}

/// One resource under test: its name, the modelled `ResourceType` it reports, and
/// what it is charged for.
#[derive(Debug)]
struct Row {
    case: &'static str,
    name: &'static str,
    resource_type: &'static str,
    charges: &'static [Charge],
}

/// Every resource type this store holds, with the published price of what each
/// one is on.
///
/// SOURCE of the figures: the Lightsail price list — instance plans ($3.50 a month
/// for the Linux/Unix nano plan, $8 for the Windows plan of the same size, whose
/// system disk is 20 GB), block storage ($0.10 per GB-month), snapshots ($0.05 per
/// GB-month), object storage ($1 a month for the 5 GB plan), managed databases
/// ($15 a month for the micro plan, whose disk is 40 GB), container services ($7 a
/// month per nano node), CDN distributions ($2.50 a month for the 50 GB plan),
/// load balancers ($18 a month) and static IPs ($0.005 an hour while unattached).
const ROWS: &[Row] = &[
    Row {
        case: "a Linux instance on the nano plan",
        name: "cost-instance",
        resource_type: "Instance",
        charges: &[Charge {
            usage_type: "BundleUsage:nano_3_0",
            pricing_unit: "Bundles",
            unit: 1.0,
            cost: 3.5,
        }],
    },
    Row {
        case: "a Windows instance on the nano plan",
        name: "cost-windows-instance",
        resource_type: "Instance",
        charges: &[Charge {
            usage_type: "BundleUsage:nano_win_3_0",
            pricing_unit: "Bundles",
            unit: 1.0,
            cost: 8.0,
        }],
    },
    Row {
        case: "a 32 GB block storage disk",
        name: "cost-disk",
        resource_type: "Disk",
        charges: &[Charge {
            usage_type: "DiskUsage",
            pricing_unit: "GB-Mo",
            unit: 32.0,
            cost: 3.2,
        }],
    },
    Row {
        case: "a snapshot of that 32 GB disk",
        name: "cost-disk-snapshot",
        resource_type: "DiskSnapshot",
        charges: &[Charge {
            usage_type: "SnapshotUsage",
            pricing_unit: "GB-Mo",
            unit: 32.0,
            cost: 1.6,
        }],
    },
    Row {
        case: "a snapshot of the nano instance, whose system disk is 20 GB",
        name: "cost-instance-snapshot",
        resource_type: "InstanceSnapshot",
        charges: &[Charge {
            usage_type: "SnapshotUsage",
            pricing_unit: "GB-Mo",
            unit: 20.0,
            cost: 1.0,
        }],
    },
    Row {
        case: "a managed database on the micro plan",
        name: "cost-database",
        resource_type: "RelationalDatabase",
        charges: &[Charge {
            usage_type: "BundleUsage:micro_1_0",
            pricing_unit: "Bundles",
            unit: 1.0,
            cost: 15.0,
        }],
    },
    Row {
        case: "a snapshot of that database, whose disk is 40 GB",
        name: "cost-database-snapshot",
        resource_type: "RelationalDatabaseSnapshot",
        charges: &[Charge {
            usage_type: "SnapshotUsage",
            pricing_unit: "GB-Mo",
            unit: 40.0,
            cost: 2.0,
        }],
    },
    Row {
        case: "a bucket on the 5 GB object storage plan",
        name: "cost-bucket",
        resource_type: "Bucket",
        charges: &[Charge {
            usage_type: "BundleUsage:small_1_0",
            pricing_unit: "Bundles",
            unit: 1.0,
            cost: 1.0,
        }],
    },
    Row {
        case: "a container service of two nano nodes",
        name: "cost-containers",
        resource_type: "ContainerService",
        charges: &[Charge {
            usage_type: "PowerUsage:nano-1",
            pricing_unit: "Bundles",
            unit: 2.0,
            cost: 14.0,
        }],
    },
    Row {
        case: "a CDN distribution on the 50 GB plan",
        name: "cost-distribution",
        resource_type: "Distribution",
        charges: &[Charge {
            usage_type: "BundleUsage:small_1_0",
            pricing_unit: "Bundles",
            unit: 1.0,
            cost: 2.5,
        }],
    },
    Row {
        case: "a load balancer, which is on no plan",
        name: "cost-balancer",
        resource_type: "LoadBalancer",
        charges: &[Charge {
            usage_type: "LoadBalancerUsage",
            pricing_unit: "Hrs",
            unit: BILLING_MONTH_HOURS,
            cost: 18.0,
        }],
    },
    Row {
        case: "an unattached static IP",
        name: "cost-static-ip",
        resource_type: "StaticIp",
        charges: &[Charge {
            usage_type: "StaticIpUsage",
            pricing_unit: "Hrs",
            unit: BILLING_MONTH_HOURS,
            cost: 3.65,
        }],
    },
    Row {
        // Lightsail charges for an alarm by neither a plan nor a metered
        // dimension, so the resource exists and carries no usage type at all.
        case: "an alarm, which is charged for nothing",
        name: "cost-alarm",
        resource_type: "Alarm",
        charges: &[],
    },
];

/// Create every resource [`ROWS`] names, each in the default Region the suite
/// signs nothing for (`us-east-1`).
async fn create_resources(emulator: &Emulator) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["cost-instance"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["cost-windows-instance"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "windows_server_2022",
                "bundleId": "nano_win_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDisk",
            json!({
                "diskName": "cost-disk",
                "availabilityZone": "us-east-1a",
                "sizeInGb": 32,
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDiskSnapshot",
            json!({"diskName": "cost-disk", "diskSnapshotName": "cost-disk-snapshot"}),
        )
        .await;
    emulator
        .ok(
            "CreateInstanceSnapshot",
            json!({
                "instanceName": "cost-instance",
                "instanceSnapshotName": "cost-instance-snapshot",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateRelationalDatabase",
            json!({
                "relationalDatabaseName": "cost-database",
                "availabilityZone": "us-east-1a",
                "relationalDatabaseBlueprintId": "mysql_8_4",
                "relationalDatabaseBundleId": "micro_1_0",
                "masterDatabaseName": "appdata",
                "masterUsername": "dbmaster",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateRelationalDatabaseSnapshot",
            json!({
                "relationalDatabaseName": "cost-database",
                "relationalDatabaseSnapshotName": "cost-database-snapshot",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": "cost-bucket", "bundleId": "small_1_0"}),
        )
        .await;
    // `CreateContainerService` answers the modelled 201 for the resource it
    // created, so it cannot go through `ok`.
    let (status, created) = emulator
        .call(
            "CreateContainerService",
            json!({"serviceName": "cost-containers", "power": "nano", "scale": 2}),
        )
        .await;
    assert_eq!(
        status, 201,
        "the container service must be created, got {status} {created:?}"
    );
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "cost-distribution",
                "origin": {"name": "cost-instance"},
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": "small_1_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateLoadBalancer",
            json!({"loadBalancerName": "cost-balancer", "instancePort": 80}),
        )
        .await;
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "cost-static-ip"}))
        .await;
    emulator
        .ok(
            "PutAlarm",
            json!({
                "alarmName": "cost-alarm",
                "monitoredResourceName": "cost-instance",
                "metricName": "CPUUtilization",
                "comparisonOperator": "GreaterThanThreshold",
                "threshold": 80,
                "evaluationPeriods": 2,
            }),
        )
        .await;
}

/// The one estimate `GetCostEstimate` answers for `name` over `[start, end)`.
async fn estimate(emulator: &Emulator, name: &str, start: f64, end: f64) -> Value {
    let body = emulator
        .ok(
            "GetCostEstimate",
            json!({"resourceName": name, "startTime": start, "endTime": end}),
        )
        .await;
    let estimates = body["resourcesBudgetEstimate"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name}: expected a resourcesBudgetEstimate array, got {body:?}"))
        .clone();
    assert_eq!(
        estimates.len(),
        1,
        "case {name}: expected the one named resource to be estimated, got {estimates:?}"
    );
    estimates[0].clone()
}

/// Whether two figures agree to the millionth, which is the precision an estimate
/// is reported at.
fn same(left: Option<f64>, right: f64) -> bool {
    left.is_some_and(|left| (left - right).abs() < 1e-6)
}

/// Whether two instants agree to the second.
fn same_instant(left: Option<f64>, right: f64) -> bool {
    left.is_some_and(|left| (left - right).abs() < 1.0)
}

/// The `resultsByTime` entries of `estimate`, keyed by usage type.
fn charges_of(case: &str, estimate: &Value) -> Vec<(String, Value)> {
    estimate["costEstimates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a costEstimates array, got {estimate:?}"))
        .iter()
        .map(|charge| {
            let usage_type = charge["usageType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: every charge names a usageType, got {charge:?}"))
                .to_string();
            let results = charge["resultsByTime"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a resultsByTime array, got {charge:?}")
                })
                .clone();
            assert_eq!(
                results.len(),
                1,
                "case {case}: expected one result for the one window asked about, got {results:?}"
            );
            (usage_type, results[0].clone())
        })
        .collect()
}

/// RULE: every resource type the store holds is estimated from its own stored
/// pricing basis — its plan, its size or its flat hourly price — over the window
/// the request asked about.
#[tokio::test(flavor = "current_thread")]
async fn every_resource_type_is_estimated_from_its_own_pricing_basis() {
    let emulator = Emulator::new();
    create_resources(&emulator).await;

    // One whole billing month, beginning after every resource above exists, so
    // each row's figures are exactly one month of its plan.
    let start = now_epoch_seconds() + WINDOW_STARTS_IN_SECONDS;
    let end = start + BILLING_MONTH_HOURS * 3_600.0;

    for row in ROWS {
        let case = row.case;
        let estimate = estimate(&emulator, row.name, start, end).await;

        assert_eq!(
            estimate["resourceName"].as_str(),
            Some(row.name),
            "case {case}: expected the estimate to name the resource, got {estimate:?}"
        );
        assert_eq!(
            estimate["resourceType"].as_str(),
            Some(row.resource_type),
            "case {case}: expected the modelled resource type, got {estimate:?}"
        );
        assert!(
            same_instant(estimate["startTime"].as_f64(), start)
                && same_instant(estimate["endTime"].as_f64(), end),
            "case {case}: expected the requested window to be reported back, got {estimate:?}"
        );

        let charges = charges_of(case, &estimate);
        let reported: Vec<&str> = charges.iter().map(|(usage, _)| usage.as_str()).collect();
        let expected: Vec<&str> = row.charges.iter().map(|charge| charge.usage_type).collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected exactly the usage types this resource is charged for, \
             got {reported:?}"
        );

        for (expected, (usage_type, result)) in row.charges.iter().zip(charges.iter()) {
            assert_eq!(
                result["pricingUnit"].as_str(),
                Some(expected.pricing_unit),
                "case {case} ({usage_type}): expected the pricing unit \
                 {:?}, got {result:?}",
                expected.pricing_unit
            );
            assert_eq!(
                result["currency"].as_str(),
                Some("USD"),
                "case {case} ({usage_type}): every figure is in US dollars, got {result:?}"
            );
            assert!(
                same(result["unit"].as_f64(), expected.unit),
                "case {case} ({usage_type}): expected {} pricing units over a billing month, \
                 got {result:?}",
                expected.unit
            );
            assert!(
                same(result["usageCost"].as_f64(), expected.cost),
                "case {case} ({usage_type}): expected ${} over a billing month, got {result:?}",
                expected.cost
            );
            assert!(
                same_instant(result["timePeriod"]["start"].as_f64(), start)
                    && same_instant(result["timePeriod"]["end"].as_f64(), end),
                "case {case} ({usage_type}): expected the billed period to be the whole \
                 requested window, got {result:?}"
            );
        }
    }
}

/// RULE: the figures scale with the hours of the window — a charge is the pricing
/// units the window consumes times the price of one unit — so the same resource
/// over a longer window costs proportionally more.
///
/// The rows differ along the one dimension the request has beyond the resource:
/// how long the window is.
#[tokio::test(flavor = "current_thread")]
async fn the_estimate_scales_with_the_length_of_the_window() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["windowed"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "small_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateDisk",
            json!({"diskName": "windowed-disk", "availabilityZone": "us-east-1a", "sizeInGb": 8}),
        )
        .await;

    // SOURCE: the Lightsail price list — the small Linux/Unix instance plan is $10
    // a month, and block storage is $0.10 per GB-month.
    let plan_usd_per_month = 10.0;
    let disk_usd_per_gb_month = 0.10;
    let disk_gigabytes = 8.0;

    // (case, hours of the window)
    let windows: &[(&str, f64)] = &[
        ("a single hour", 1.0),
        ("a day", 24.0),
        ("a week", 168.0),
        ("a whole billing month", BILLING_MONTH_HOURS),
        ("two billing months", 2.0 * BILLING_MONTH_HOURS),
        ("a year", 8_760.0),
    ];

    let start = now_epoch_seconds() + WINDOW_STARTS_IN_SECONDS;
    for (case, hours) in windows {
        let end = start + hours * 3_600.0;
        let months = hours / BILLING_MONTH_HOURS;

        let instance = estimate(&emulator, "windowed", start, end).await;
        let instance_charges = charges_of(case, &instance);
        let (_, result) = &instance_charges[0];
        assert!(
            same(result["unit"].as_f64(), months),
            "case {case}: expected {months} of a plan over {hours} hours, got {result:?}"
        );
        assert!(
            same(result["usageCost"].as_f64(), months * plan_usd_per_month),
            "case {case}: expected ${} for {hours} hours of the plan, got {result:?}",
            months * plan_usd_per_month
        );

        let disk = estimate(&emulator, "windowed-disk", start, end).await;
        let disk_charges = charges_of(case, &disk);
        let (_, result) = &disk_charges[0];
        assert!(
            same(result["unit"].as_f64(), disk_gigabytes * months),
            "case {case}: expected {} GB-months over {hours} hours, got {result:?}",
            disk_gigabytes * months
        );
        assert!(
            same(
                result["usageCost"].as_f64(),
                disk_gigabytes * months * disk_usd_per_gb_month
            ),
            "case {case}: expected ${} for {hours} hours of the disk, got {result:?}",
            disk_gigabytes * months * disk_usd_per_gb_month
        );
    }
}

/// RULE: a window is billed only from the instant the resource was created —
/// nothing is charged for an hour before it existed — so a window entirely before
/// the resource existed estimates zero and one that straddles its creation is
/// billed from there.
#[tokio::test(flavor = "current_thread")]
async fn only_the_hours_the_resource_has_existed_for_are_billed() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["clamped"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    let created_at = emulator
        .ok("GetInstance", json!({"instanceName": "clamped"}))
        .await["instance"]["createdAt"]
        .as_f64()
        .expect("a created instance reports when it was created");

    // SOURCE: the Lightsail price list — the nano Linux/Unix plan is $3.50 a month.
    let plan_usd_per_month = 3.5;
    let hour = 3_600.0;

    // (case, window start, window end, the hours of it that are billable)
    let rows: &[(&str, f64, f64, f64)] = &[
        (
            "a window that ended before the resource existed",
            created_at - 48.0 * hour,
            created_at - 24.0 * hour,
            0.0,
        ),
        (
            "a window ending exactly when the resource was created",
            created_at - 10.0 * hour,
            created_at,
            0.0,
        ),
        (
            "a window starting a day before the resource and running a day past it",
            created_at - 24.0 * hour,
            created_at + 24.0 * hour,
            24.0,
        ),
        (
            "a window starting a month before the resource and running a month past it",
            created_at - BILLING_MONTH_HOURS * hour,
            created_at + BILLING_MONTH_HOURS * hour,
            BILLING_MONTH_HOURS,
        ),
        (
            "a window wholly after the resource was created",
            created_at + hour,
            created_at + 25.0 * hour,
            24.0,
        ),
    ];

    for (case, start, end, billable_hours) in rows {
        let estimate = estimate(&emulator, "clamped", *start, *end).await;
        // The window the caller asked about is reported as asked, whatever part of
        // it is billable.
        assert!(
            same_instant(estimate["startTime"].as_f64(), *start)
                && same_instant(estimate["endTime"].as_f64(), *end),
            "case {case}: expected the requested window to be reported back, got {estimate:?}"
        );

        let charges = charges_of(case, &estimate);
        let (_, result) = &charges[0];
        let months = billable_hours / BILLING_MONTH_HOURS;
        assert!(
            same(result["unit"].as_f64(), months),
            "case {case}: expected {billable_hours} billable hours ({months} of a plan), \
             got {result:?}"
        );
        assert!(
            same(result["usageCost"].as_f64(), months * plan_usd_per_month),
            "case {case}: expected ${} for {billable_hours} billable hours, got {result:?}",
            months * plan_usd_per_month
        );

        // The billed period lies inside the window the caller asked about, is as
        // long as the billable hours, and — when anything is billable — starts
        // where the resource and the window both already existed.
        let billed_start = result["timePeriod"]["start"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a billed period, got {result:?}"));
        let billed_end = result["timePeriod"]["end"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a billed period, got {result:?}"));
        assert!(
            billed_start >= start - 1.0 && billed_end <= end + 1.0 && billed_start <= billed_end,
            "case {case}: expected the billed period inside the requested window, got {result:?}"
        );
        assert!(
            ((billed_end - billed_start) - billable_hours * hour).abs() < 1.0,
            "case {case}: expected a billed period of {billable_hours} hours, got {result:?}"
        );
        if *billable_hours > 0.0 {
            assert!(
                same_instant(Some(billed_start), start.max(created_at)),
                "case {case}: expected billing to start where the resource and the window \
                 both already existed, got {result:?}"
            );
        }
    }
}

/// RULE (the half of it the wire cannot reach): a static IP is charged by the hour
/// only while it is idle, and a resource this store holds no metered dimension for
/// carries no usage type at all.
///
/// `AttachStaticIp` is still a scaffold and a DNS zone's name cannot satisfy this
/// operation's `ResourceName` pattern, so these two rows are driven against the
/// store directly — the same lookup the handler runs.
#[test]
fn an_attached_static_ip_is_free_and_an_unpriced_resource_carries_no_usage_type() {
    let created_at_ms = 1_700_000_000_000;
    let hour_ms = 3_600_000;
    // One day of the window, starting when the resource was created.
    let start_ms = created_at_ms;
    let end_ms = created_at_ms + 24 * hour_ms;

    // (case, the record to store, the usage types expected, the USD expected)
    struct Row {
        case: &'static str,
        /// The name the stored record is held under.
        name: &'static str,
        /// Stores that record.
        store_it: fn(&mut State),
        usage_types: &'static [&'static str],
        /// The USD a day of it comes to.
        cost_per_day: f64,
    }

    fn store_idle_static_ip(store: &mut State) {
        store.insert_static_ip(static_ip("idle", None));
    }

    fn store_attached_static_ip(store: &mut State) {
        store.insert_static_ip(static_ip("attached", Some("some-instance".to_string())));
    }

    fn store_zone(store: &mut State) {
        store.insert_domain(zone());
    }

    let rows = [
        Row {
            case: "an idle static IP",
            name: "idle",
            store_it: store_idle_static_ip,
            usage_types: &["StaticIpUsage"],
            // SOURCE: $0.005 an hour while unattached, over 24 hours.
            cost_per_day: 24.0 * 0.005,
        },
        Row {
            case: "a static IP attached to an instance",
            name: "attached",
            store_it: store_attached_static_ip,
            usage_types: &["StaticIpUsage"],
            // An address attached to an instance is free.
            cost_per_day: 0.0,
        },
        Row {
            case: "a DNS zone, which this store holds no metered dimension for",
            name: "example",
            store_it: store_zone,
            usage_types: &[],
            cost_per_day: 0.0,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut store = State::default();
        (row.store_it)(&mut store);
        let billed = billed_resources(&store, "us-east-1", row.name)
            .unwrap_or_else(|error| panic!("case {case}: the stored resource is priced: {error}"));
        assert_eq!(
            billed.len(),
            1,
            "case {case}: expected the one stored resource to be billed, got {billed:?}"
        );
        let estimate = billed
            .into_iter()
            .next()
            .expect("the one billed resource")
            .estimate(start_ms, end_ms);
        let charges = estimate.cost_estimates.unwrap_or_default();
        let usage_types: Vec<String> = charges
            .iter()
            .map(|charge| charge.usage_type.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            usage_types, row.usage_types,
            "case {case}: expected exactly the usage types this resource is charged for, \
             got {usage_types:?}"
        );
        let cost: f64 = charges
            .iter()
            .flat_map(|charge| charge.results_by_time.clone().unwrap_or_default())
            .map(|result| result.usage_cost.unwrap_or_default())
            .sum();
        assert!(
            (cost - row.cost_per_day).abs() < 1e-6,
            "case {case}: expected ${} over the day, got ${cost}",
            row.cost_per_day
        );
    }
}

/// A stored static IP named `name`, attached to `attached_to` when it is attached.
fn static_ip(name: &str, attached_to: Option<String>) -> StoredStaticIp {
    StoredStaticIp {
        name: name.to_string(),
        arn: format!("arn:aws:lightsail:us-east-1:000000000000:StaticIp/{name}"),
        support_code: "000000000000/static-ip".to_string(),
        created_at_ms: 1_700_000_000_000,
        location: StoredLocation::regional("us-east-1"),
        ip_address: "192.0.2.10".to_string(),
        attached_to,
    }
}

/// A stored DNS zone, the resource this service prices by no dimension the store
/// holds.
fn zone() -> StoredDomain {
    StoredDomain {
        name: "example".to_string(),
        arn: "arn:aws:lightsail:global:000000000000:Domain/example".to_string(),
        support_code: "000000000000/domain".to_string(),
        created_at_ms: 1_700_000_000_000,
        location: StoredLocation::domain(),
        tags: Vec::new(),
        entries: Vec::new(),
    }
}

/// RULE: a request the service cannot estimate is refused, never answered with a
/// zeroed estimate — a name the account holds no resource under (including one it
/// has deleted, and one held in another Region) with the service's `DoesNotExist`
/// envelope, and a window that holds no instant with the modelled
/// `InvalidInputException`.
#[tokio::test(flavor = "current_thread")]
async fn a_request_that_cannot_be_estimated_is_refused() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["doomed"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": "east-bucket", "bundleId": "small_1_0"}),
        )
        .await;
    emulator.ok("DeleteInstance", json!({"instanceName": "doomed"})).await;

    let start = now_epoch_seconds() + WINDOW_STARTS_IN_SECONDS;
    let end = start + BILLING_MONTH_HOURS * 3_600.0;

    // Each name is admitted by the modelled `ResourceName` pattern and names no
    // resource of this account in this Region: one never created, one deleted, one
    // differing from a stored name only in case, and one that is a resource of
    // another Region only.
    for (case, name, region) in [
        ("a name never created", "absent", None),
        ("a deleted instance", "doomed", None),
        ("a stored name in another case", "DOOMED", None),
        ("a bucket of another Region", "east-bucket", Some("us-west-2")),
    ] {
        let (status, body) = emulator
            .call_in(
                region,
                "GetCostEstimate",
                json!({"resourceName": name, "startTime": start, "endTime": end}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the service's not-found status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case}: expected the modelled not-found error, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected Lightsail's DoesNotExist code, got {body:?}"
        );
    }

    // A window whose end is not after its start holds no instant: the period is
    // inclusive of its start and exclusive of its end.
    for (case, window_start, window_end) in [
        ("a window of no length", start, start),
        ("a window that ends before it starts", end, start),
    ] {
        let (status, body) = emulator
            .call(
                "GetCostEstimate",
                json!({
                    "resourceName": "east-bucket",
                    "startTime": window_start,
                    "endTime": window_end,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case}: expected the modelled invalid-input error, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("endTime") && message.contains("startTime"),
            "case {case}: expected a message naming the members, got {message:?}"
        );
    }
}

/// RULE: estimating a cost is a READ — two estimates of the same window agree,
/// and the resources they were made from are untouched afterwards.
#[tokio::test(flavor = "current_thread")]
async fn estimating_a_cost_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["read-only"],
                "availabilityZone": "us-east-1a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "micro_3_0",
            }),
        )
        .await;
    emulator
        .ok(
            "AllocateStaticIp",
            json!({"staticIpName": "read-only-ip"}),
        )
        .await;

    let start = now_epoch_seconds() + WINDOW_STARTS_IN_SECONDS;
    let end = start + BILLING_MONTH_HOURS * 3_600.0;

    for name in ["read-only", "read-only-ip"] {
        let first = estimate(&emulator, name, start, end).await;
        let again = estimate(&emulator, name, start, end).await;
        assert_eq!(
            first, again,
            "case {name}: two estimates of the same window must agree, got {again:?} \
             after {first:?}"
        );
    }

    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "read-only"}))
        .await["instance"]
        .clone();
    assert_eq!(
        instance["bundleId"].as_str(),
        Some("micro_3_0"),
        "case store unchanged: expected the instance's plan to survive the reads, \
         got {instance:?}"
    );
    let static_ip = emulator
        .ok("GetStaticIp", json!({"staticIpName": "read-only-ip"}))
        .await["staticIp"]
        .clone();
    assert_eq!(
        static_ip["isAttached"].as_bool(),
        Some(false),
        "case store unchanged: expected the static IP to be untouched, got {static_ip:?}"
    );
}
