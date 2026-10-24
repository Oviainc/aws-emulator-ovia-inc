// SPDX-License-Identifier: CC0-1.0

//! The ENUM `RegionMetadata$Status` carries, over the `awsJson1_1` wire.
//!
//! THE RULE: a member typed by an enum reports only values that enum declares, and
//! `RegionMetadata$Status` is typed by `RegionStatus` — `ACTIVE | ADDING | REMOVING`
//! (`smithy_query(query="shape",shape="RegionStatus")`, whose documentation spells them
//! out: "ACTIVE (Region is operational), ADDING (Region extension workflow is in
//! progress), or REMOVING (Region removal workflow is in progress)"). That is a DIFFERENT
//! enum from the instance-level `InstanceStatus` (`CREATE_IN_PROGRESS | CREATE_FAILED |
//! DELETE_IN_PROGRESS | ACTIVE`), so no Region of an instance ever reports an
//! instance-level value, whatever the instance's own lifecycle is doing: a Region whose
//! enablement is in flight is `ADDING` and an operational one is `ACTIVE`, while the
//! instance beside it is `CREATE_IN_PROGRESS`, `ACTIVE` or `DELETE_IN_PROGRESS`
//! independently.
//!
//! The rows span what the rule generalises over: the lifecycle PHASE the instance is in
//! (creation in flight, activation due, deletion in flight), the READER that projects
//! `RegionMetadata` (`DescribeInstance` and `ListInstances` both carry `Regions`), and the
//! Region the instance is enabled in — the names the model's `RegionName` pattern
//! `^([a-z]+-){2,3}\d$` admits, in two- and three-prefix spellings and across partitions.
//! The admissible status values themselves are not written out by hand here: they are the
//! model-generated `ops::types::REGION_STATUS_VALUES` and `INSTANCE_STATUS_VALUES`.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Where a phase lives on the far side of a due
//! instant the test moves the STORED instant rather than sleeping — the transition is
//! still applied by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::{
    INSTANCE_STATUS_VALUES, REGION_STATUS_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{call_in_region, router_over, with_store};

/// A router over an account that holds NO instance yet, so `CreateInstance` is the call
/// that enables the account's one instance — and the engine behind it, for the stored due
/// instants a phase has to be moved past.
fn empty_account() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// Move the stored activation instant of every instance to `due_at`.
fn hold_activation(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            instance.active_due = Some(due_at);
        }
    });
}

/// Move the stored removal instant of every instance being deleted to `due_at`.
fn hold_deletion(engine: &Arc<engine::InMemoryEngine>, due_at: SystemTime) {
    with_store(engine, |store| {
        for instance in store.instances.values_mut() {
            if instance.deleted_due.is_some() {
                instance.deleted_due = Some(due_at);
            }
        }
    });
}

/// The `(instance Status, [(RegionName, Region Status)])` each reader reports for
/// `instance_arn`, read in `region`: `DescribeInstance` first, then the entry
/// `ListInstances` projects for the same instance.
async fn reported(
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
) -> Vec<(&'static str, Option<String>, Vec<(String, String)>)> {
    let (status, described) = call_in_region(
        router,
        region,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "region {region}: DescribeInstance expected 200, got {status} {described}"
    );

    let (status, listed) = call_in_region(router, region, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "region {region}: ListInstances expected 200, got {status} {listed}"
    );
    let entry = listed["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("no Instances in {listed}"))
        .iter()
        .find(|instance| instance["InstanceArn"].as_str() == Some(instance_arn))
        .unwrap_or_else(|| panic!("instance {instance_arn} is not listed in {listed}"))
        .clone();

    vec![
        ("DescribeInstance", instance_status(&described), regions_of(&described)),
        ("ListInstances", instance_status(&entry), regions_of(&entry)),
    ]
}

/// The instance-level `Status` a projection reports.
fn instance_status(instance: &Value) -> Option<String> {
    instance["Status"].as_str().map(str::to_string)
}

/// The `(RegionName, Status)` pairs of a projection's `Regions` list.
fn regions_of(instance: &Value) -> Vec<(String, String)> {
    instance["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("no Regions in {instance}"))
        .iter()
        .map(|entry| {
            (
                entry["RegionName"].as_str().unwrap_or_default().to_string(),
                entry["Status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every `RegionMetadata$Status` of `regions` is one the model's `RegionStatus` enum
/// declares, and in particular is never one of the values only `InstanceStatus` declares.
fn assert_region_statuses_are_modeled(case: &str, regions: &[(String, String)]) {
    // The values ONLY `InstanceStatus` declares — the instance-level enum's members that
    // `RegionStatus` does not share. Derived from the two model-generated value lists, so
    // nothing here is a hand-written catalog.
    let instance_only: Vec<&str> = INSTANCE_STATUS_VALUES
        .iter()
        .copied()
        .filter(|value| !REGION_STATUS_VALUES.contains(value))
        .collect();
    assert!(
        !regions.is_empty(),
        "case {case:?}: an instance is enabled in at least its primary Region"
    );
    for (region_name, status) in regions {
        assert!(
            REGION_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: Region {region_name} must report one of the modeled \
             RegionStatus values {REGION_STATUS_VALUES:?}, got {status:?}"
        );
        assert!(
            !instance_only.contains(&status.as_str()),
            "case {case:?}: Region {region_name} must not report one of the \
             InstanceStatus-only values {instance_only:?}, got {status:?}"
        );
    }
}

/// THE RULE, across the instance's lifecycle phases, both readers and several primary
/// Regions: the Region statuses are `RegionStatus` values throughout, `ADDING` while the
/// enablement is in flight and `ACTIVE` once it is operational, while the instance's own
/// `Status` moves through `InstanceStatus` independently.
#[tokio::test]
async fn region_status_is_never_an_instance_status_through_the_lifecycle() {
    // The Region the instance is enabled in: the `RegionName` pattern's two- and
    // three-prefix spellings and three partitions. None of them is the `us-west-2` the
    // recorded case used.
    let regions = [
        "us-east-1",
        "eu-central-1",
        "ap-southeast-3",
        "us-gov-west-1",
        "cn-north-1",
    ];

    for region in regions {
        let (router, engine) = empty_account();
        let (status, created) =
            call_in_region(&router, region, "CreateInstance", &json!({"Name": "Lifecycle"})).await;
        assert_eq!(
            status, 200,
            "region {region}: CreateInstance expected 200, got {status} {created}"
        );
        let instance_arn = created["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no InstanceArn in {created}"))
            .to_string();

        // PHASE 1 — the creation is in flight: the instance is CREATE_IN_PROGRESS and its
        // Regions' enablement is ADDING, which is what `RegionStatus` has for a workflow
        // in progress.
        hold_activation(&engine, SystemTime::now() + Duration::from_secs(3_600));
        for (reader, status, region_statuses) in reported(&router, region, &instance_arn).await {
            let case = format!("{region} / {reader} / creation in flight");
            assert_region_statuses_are_modeled(&case, &region_statuses);
            assert_eq!(
                status.as_deref(),
                Some("CREATE_IN_PROGRESS"),
                "case {case:?}: the instance itself is CREATE_IN_PROGRESS, got {status:?}"
            );
            for (region_name, region_status) in &region_statuses {
                assert_eq!(
                    region_status, "ADDING",
                    "case {case:?}: Region {region_name}'s enablement is in flight, so it \
                     is ADDING, got {region_status:?}"
                );
            }
            assert!(
                region_statuses
                    .iter()
                    .any(|(region_name, _)| region_name == region),
                "case {case:?}: the instance is enabled in the Region it was created in, \
                 got {region_statuses:?}"
            );
        }

        // PHASE 2 — the activation is due: the instance and every Region of it are ACTIVE,
        // and stay there across repeated reads.
        hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));
        for read in 0..2 {
            for (reader, status, region_statuses) in reported(&router, region, &instance_arn).await
            {
                let case = format!("{region} / {reader} / activation due / read {read}");
                assert_region_statuses_are_modeled(&case, &region_statuses);
                assert_eq!(
                    status.as_deref(),
                    Some("ACTIVE"),
                    "case {case:?}: the activation was due, got {status:?}"
                );
                for (region_name, region_status) in &region_statuses {
                    assert_eq!(
                        region_status, "ACTIVE",
                        "case {case:?}: Region {region_name} is operational, got \
                         {region_status:?}"
                    );
                }
            }
        }

        // PHASE 3 — the deletion is in flight: the instance carries the instance-level
        // DELETE_IN_PROGRESS while its Regions keep a `RegionStatus` value, since that
        // enum has no deleting member.
        let (status, deleted) = call_in_region(
            &router,
            region,
            "DeleteInstance",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: DeleteInstance expected 200, got {status} {deleted}"
        );
        hold_deletion(&engine, SystemTime::now() + Duration::from_secs(3_600));
        for (reader, status, region_statuses) in reported(&router, region, &instance_arn).await {
            let case = format!("{region} / {reader} / deletion in flight");
            assert_region_statuses_are_modeled(&case, &region_statuses);
            assert_eq!(
                status.as_deref(),
                Some("DELETE_IN_PROGRESS"),
                "case {case:?}: the instance is being deleted, got {status:?}"
            );
            for (region_name, region_status) in &region_statuses {
                assert_eq!(
                    region_status, "ACTIVE",
                    "case {case:?}: Region {region_name} keeps the status it had, got \
                     {region_status:?}"
                );
            }
        }
    }
}

/// The same rule where the Region is one a REMOVAL workflow is running on: `RemoveRegion`
/// reports `REMOVING`, the third `RegionStatus` value, and the primary Region beside it is
/// unaffected — so the enum's whole value set is exercised over the wire.
#[tokio::test]
async fn a_removing_region_reports_the_third_region_status_value() {
    use aws_smithy_types::DateTime;
    use service_com_amazonaws_ssoadmin_swbexternalservice::state;

    let region = "eu-west-2";
    let additional = "eu-west-3";
    let (router, engine) = empty_account();
    let (status, created) =
        call_in_region(&router, region, "CreateInstance", &json!({"Name": "Removal"})).await;
    assert_eq!(
        status, 200,
        "CreateInstance expected 200, got {status} {created}"
    );
    let instance_arn = created["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no InstanceArn in {created}"))
        .to_string();
    // The activation is due, so the instance is operational; `AddRegion` is still a
    // scaffold stub with an issue of its own, so the additional Region is seeded through
    // the service's own `state::Region` constructor and everything asserted then goes over
    // the wire.
    hold_activation(&engine, SystemTime::now() - Duration::from_secs(1));
    with_store(&engine, |store| {
        store
            .instance_mut(&instance_arn)
            .expect("the instance the create just stored")
            .regions
            .push(state::Region::active(
                additional,
                DateTime::from(SystemTime::now()),
            ));
    });

    let (status, removed) = call_in_region(
        &router,
        region,
        "RemoveRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": additional}),
    )
    .await;
    assert_eq!(
        status, 200,
        "RemoveRegion expected 200, got {status} {removed}"
    );
    assert_eq!(
        removed["Status"].as_str(),
        Some("REMOVING"),
        "a removal in flight reports REMOVING, got {removed}"
    );

    for (reader, _, region_statuses) in reported(&router, region, &instance_arn).await {
        let case = format!("{reader} / removal in flight");
        assert_region_statuses_are_modeled(&case, &region_statuses);
        for (region_name, region_status) in &region_statuses {
            let expected = if region_name == additional {
                "REMOVING"
            } else {
                "ACTIVE"
            };
            assert_eq!(
                region_status, expected,
                "case {case:?}: Region {region_name} expected {expected}, got \
                 {region_status:?}"
            );
        }
    }
}
