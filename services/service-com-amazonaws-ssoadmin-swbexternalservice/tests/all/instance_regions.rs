// SPDX-License-Identifier: CC0-1.0

//! The REGION an IAM Identity Center instance reports, end to end over the `awsJson1_1`
//! wire.
//!
//! THE RULE: an instance is enabled in exactly one Region — the Region of the request that
//! enabled it — and every operation that reports a Region reports THAT Region, never a
//! constant the emulator was built with. `DescribeInstance$PrimaryRegion` is documented as
//! "The primary Region where the IAM Identity Center instance was originally enabled" and
//! `Regions` as "The list of Regions enabled in the IAM Identity Center instance", so the
//! account's instance reports, in `DescribeInstance` and in the `InstanceMetadata` of
//! `ListInstances` alike, the Region it was enabled in, flagged `IsPrimaryRegion`.
//!
//! The rows span what the rule generalises over: the Region of the request, over the
//! partitions and the shapes the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits
//! (two and three hyphen-separated prefixes, `us-`/`eu-`/`ap-`, a GovCloud and a China
//! Region), for BOTH instances that exist in this emulator — the account's ambient
//! instance, which the first request enables, and an instance `CreateInstance` mints.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call_in_region, router_over, with_store};

/// The Regions the rows drive, each inside the model's `RegionName` pattern
/// `^([a-z]+-){2,3}\d$` (`smithy_query(query="shape",shape="RegionName")`): two-prefix and
/// three-prefix spellings, from four geographies and from the GovCloud and China
/// partitions. None of them is the emulator's own fallback Region
/// (`state::DEFAULT_REGION`), so a reported Region that happens to equal that fallback
/// would fail every row.
const REGIONS: &[&str] = &[
    "us-west-2",
    "eu-central-1",
    "ap-southeast-4",
    "us-gov-west-1",
    "cn-north-1",
    "me-central-1",
];

/// A router over a fresh engine, and that engine.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// The one instance `ListInstances` reports, read in `region`.
async fn listed_instance(router: &MultiServiceRouter, region: &str) -> Value {
    let (status, body) = call_in_region(router, region, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "region {region}: ListInstances expected 200, got {status} {body}"
    );
    let instances = body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("region {region}: no Instances in {body}"));
    assert_eq!(
        instances.len(),
        1,
        "region {region}: expected the account's one instance, got {instances:?}"
    );
    instances[0].clone()
}

/// `DescribeInstance` for `instance_arn`, read in `region`, as a 200 body.
async fn describe(router: &MultiServiceRouter, region: &str, instance_arn: &str) -> Value {
    let (status, body) = call_in_region(
        router,
        region,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "region {region}: DescribeInstance expected 200, got {status} {body}"
    );
    body
}

/// Assert that `instance` — a `DescribeInstance` body or an `InstanceMetadata` — reports
/// `region` as its one enabled, primary Region.
fn assert_enabled_in(case: &str, instance: &Value, region: &str) {
    assert_eq!(
        instance["PrimaryRegion"].as_str(),
        Some(region),
        "case {case:?}: PrimaryRegion must be the Region the instance was enabled in, got \
         {:?}",
        instance["PrimaryRegion"]
    );
    let regions = instance["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Regions in {instance}"));
    assert_eq!(
        regions.len(),
        1,
        "case {case:?}: the instance is enabled in exactly the one Region, got {regions:?}"
    );
    assert_eq!(
        regions[0]["RegionName"].as_str(),
        Some(region),
        "case {case:?}: the enabled Region must be the one the instance was enabled in, \
         got {:?}",
        regions[0]["RegionName"]
    );
    assert_eq!(
        regions[0]["IsPrimaryRegion"],
        Value::Bool(true),
        "case {case:?}: the Region the instance was enabled in is its primary one, got {:?}",
        regions[0]
    );
    assert_eq!(
        regions[0]["RegionName"], instance["PrimaryRegion"],
        "case {case:?}: PrimaryRegion and the primary entry of Regions are one value, got \
         {:?} vs {:?}",
        regions[0]["RegionName"], instance["PrimaryRegion"]
    );
}

/// THE RULE on the account's AMBIENT instance: the instance the account already holds is
/// enabled in the Region the emulator is addressed in, and `DescribeInstance` and
/// `ListInstances` report that Region.
#[tokio::test]
async fn the_accounts_instance_reports_the_region_it_was_enabled_in() {
    for region in REGIONS {
        // One account per row: the Region an instance is enabled in is decided once, by
        // the request that enables it.
        let (router, _engine) = router_and_engine();

        let listed = listed_instance(&router, region).await;
        assert_enabled_in(&format!("the listed instance in {region}"), &listed, region);

        let instance_arn = listed["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no InstanceArn in {listed}"))
            .to_string();
        let described = describe(&router, region, &instance_arn).await;
        assert_enabled_in(
            &format!("the described instance in {region}"),
            &described,
            region,
        );
        // One record, two projections: the describe cannot disagree with the list.
        for member in ["PrimaryRegion", "Regions"] {
            assert_eq!(
                described[member], listed[member],
                "region {region}: member {member} must match the listed instance, got {:?} \
                 vs {:?}",
                described[member], listed[member]
            );
        }
    }
}

/// THE RULE on a CREATED instance: `CreateInstance` enables the instance in the Region its
/// own request was made in, whatever Region a later reader asks from — the Region is a
/// property of the record, not of the read.
#[tokio::test]
async fn a_created_instance_reports_the_region_the_create_was_made_in() {
    for region in REGIONS {
        let (router, engine) = router_and_engine();
        // An account that holds no instance, which is what `CreateInstance` is documented
        // for: the state a completed `DeleteInstance` leaves behind (driven over the wire
        // in `instance_delete.rs`).
        with_store(&engine, |store| {
            store.instances.clear();
            store.ambient_instance_enabled = true;
        });

        let (status, created) = call_in_region(
            &router,
            region,
            "CreateInstance",
            &json!({"Name": "RegionUnderTest"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: CreateInstance expected 200, got {status} {created}"
        );
        let instance_arn = created["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no InstanceArn in {created}"))
            .to_string();

        let described = describe(&router, region, &instance_arn).await;
        assert_enabled_in(&format!("created in {region}"), &described, region);

        // A read from ANOTHER Region reports the same record: the Region the create was
        // made in, not the Region of the reader.
        let elsewhere = REGIONS
            .iter()
            .find(|candidate| *candidate != region)
            .expect("the table holds more than one Region");
        let read_elsewhere = describe(&router, elsewhere, &instance_arn).await;
        assert_enabled_in(
            &format!("created in {region}, read from {elsewhere}"),
            &read_elsewhere,
            region,
        );
        let listed = listed_instance(&router, elsewhere).await;
        assert_enabled_in(
            &format!("created in {region}, listed from {elsewhere}"),
            &listed,
            region,
        );
    }
}
