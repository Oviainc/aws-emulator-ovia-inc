// SPDX-License-Identifier: CC0-1.0

//! WHERE the `State` view of an enumerated IAM Identity Center instance is read FROM, over
//! the `awsJson1_1` wire.
//!
//! THE RULE: the `State` structure an enumeration serves on an instance is read from THAT
//! instance's own lifecycle record at the instant of the read, so `State.Name` is the same
//! value the entry's flat `Status` carries — never a Region's status (`RegionStatus`:
//! `ACTIVE | ADDING | REMOVING`, a different enum from the instance-level `InstanceStatus`:
//! `CREATE_IN_PROGRESS | CREATE_FAILED | DELETE_IN_PROGRESS | ACTIVE`), never another
//! entry's status, and never keyed by the instance's name, which is not part of an
//! instance's identity.
//!
//! `instance_state_view.rs` pins that the member is SERVED and mirrors the status through
//! the lifecycle; this suite pins what it is read from, over the records a serializer could
//! wrongly read it from. The rows span what the rule generalises over:
//!
//! * the instance status × the status of a Region BESIDE it — the full cross product of the
//!   two model-generated value lists `ops::types::INSTANCE_STATUS_VALUES` and
//!   `ops::types::REGION_STATUS_VALUES`, so the Region's value varies independently of the
//!   instance's while `State.Name` stays the instance's;
//! * two instances that share one `Name` and differ in status, matched by `InstanceArn`, so
//!   a per-run instance name cannot be what the view is keyed on;
//! * a transition the clock owes falling due BETWEEN two pages of one enumeration, so the
//!   view is read as of the page that serves it rather than fixed when the walk began.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; the harness is the one
//! `ssoadmin_permission_sets.rs` owns, and the enumeration and the rule assertion are the
//! ones `instance_state_view.rs` owns. Where a phase lives on the far side of a due instant
//! the test moves the STORED instant rather than sleeping — the transition is still applied
//! by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::ops::types::{
    INSTANCE_STATUS_VALUES, REGION_STATUS_VALUES,
};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_state_view::{assert_state_mirrors_status, listed};
use crate::ssoadmin_permission_sets::{call_in_region, router_over, with_store};

/// The Region every instance below is enabled in, and the Region its calls are signed for.
/// A `RegionName` the recorded case did not use.
const REGION: &str = "eu-west-1";

/// An ADDITIONAL Region of those instances, the one whose own `RegionStatus` the rows vary.
const ADDITIONAL_REGION: &str = "eu-west-3";

/// A router over an account that holds exactly `instances`, and the engine behind it (for
/// the stored due instants a phase has to be moved past).
///
/// An organization instance is enabled through AWS Organizations rather than through this
/// API, and an account holds one instance, so the records are seeded through the service's
/// own `state::Instance` constructors; everything asserted then goes over the wire.
fn account_holding(
    instances: Vec<state::Instance>,
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    with_store(&engine, |store| {
        store.instances.clear();
        for instance in instances {
            store.instances.insert(instance.instance_arn.clone(), instance);
        }
        // The account's ambient instance has been enabled already, so no later request
        // enables one beside the records seeded here.
        store.ambient_instance_enabled = true;
    });
    (router_over(Arc::clone(&engine)), engine)
}

/// One stored instance whose own status is `status`, enabled in [`REGION`], carrying one
/// ADDITIONAL Region whose `RegionStatus` is `region_status`.
///
/// Nothing is owed on the record: the status seeded here IS the instance's status, so no
/// transition the clock owes can move it out from under a read.
fn instance_with_region_status(
    instance_id: &str,
    identity_store_id: &str,
    name: &str,
    status: &str,
    region_status: &str,
) -> state::Instance {
    let now = SystemTime::now();
    let mut instance = state::Instance::active(
        instance_id,
        identity_store_id,
        state::InstanceType::Organization,
        Some(name.to_string()),
        DateTime::from(now),
        REGION,
    );
    instance.status = status.to_string();
    instance.active_due = None;
    instance.deleted_due = None;
    let mut additional = state::Region::active(ADDITIONAL_REGION, DateTime::from(now));
    additional.status = region_status.to_string();
    instance.regions.push(additional);
    instance
}

/// The `(RegionName, Status)` pairs of an entry's `Regions` list.
fn region_statuses(case: &str, entry: &Value) -> Vec<(String, String)> {
    entry["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the entry reports no Regions: {entry}"))
        .iter()
        .map(|region| {
            (
                region["RegionName"].as_str().unwrap_or_default().to_string(),
                region["Status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// THE RULE where a Region of the instance carries a DIFFERENT status from the instance
/// itself: over the cross product of the two enums' value sets, every entry's `State.Name`
/// is the instance's own status and follows no Region's.
#[tokio::test]
async fn state_name_is_the_instance_status_whatever_its_regions_carry() {
    for status in INSTANCE_STATUS_VALUES {
        for region_status in REGION_STATUS_VALUES {
            let case = format!("instance {status} / additional Region {region_status}");
            let (router, _engine) = account_holding(vec![instance_with_region_status(
                "ssoins-aaaaaaaaaaaaaaa1",
                "d-1a2b3c4d5e",
                "RegionIndependent",
                status,
                region_status,
            )]);

            let entries = listed(&case, &router, REGION, None).await;
            assert_eq!(
                entries.len(),
                1,
                "case {case:?}: the account's one instance is enumerated, got {entries:?}"
            );
            for entry in &entries {
                assert_state_mirrors_status(&case, entry);
                assert_eq!(
                    entry["Status"].as_str(),
                    Some(*status),
                    "case {case:?}: expected the stored instance status, got {entry}"
                );
                assert_eq!(
                    entry["State"]["Name"].as_str(),
                    Some(*status),
                    "case {case:?}: expected State.Name {status:?}, got {entry}"
                );

                let regions = region_statuses(&case, entry);
                let additional = regions
                    .iter()
                    .find(|(region_name, _)| region_name == ADDITIONAL_REGION)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the additional Region is not reported: {regions:?}")
                    });
                assert_eq!(
                    additional.1, *region_status,
                    "case {case:?}: expected the additional Region to report \
                     {region_status:?}, got {:?}",
                    additional.1
                );
                // The row is only a test of INDEPENDENCE where the two values differ; where
                // they do differ, `State.Name` has to be the instance's.
                if *region_status != *status {
                    assert_ne!(
                        entry["State"]["Name"].as_str(),
                        Some(*region_status),
                        "case {case:?}: State.Name must not be read off a Region, got {entry}"
                    );
                }
            }
        }
    }
}

/// THE RULE where two instances share one `Name` and differ in status: each entry's
/// `State.Name` is the status of the record that entry's `InstanceArn` names, so the view is
/// per record and not keyed on a name (the name a recording happens to generate is no part
/// of an instance's identity).
#[tokio::test]
async fn each_entry_state_is_its_own_record_under_a_shared_name() {
    let shared_name = "Shared";
    let (router, _engine) = account_holding(vec![
        instance_with_region_status(
            "ssoins-aaaaaaaaaaaaaaa1",
            "d-1a2b3c4d5e",
            shared_name,
            "ACTIVE",
            "ACTIVE",
        ),
        instance_with_region_status(
            "ssoins-bbbbbbbbbbbbbbb2",
            "d-1a2b3c4d5f",
            shared_name,
            "CREATE_IN_PROGRESS",
            "ADDING",
        ),
    ]);

    // `(InstanceArn suffix, the status the record holds)`, which is what each entry's
    // `State.Name` has to report.
    let expected = [
        ("ssoins-aaaaaaaaaaaaaaa1", "ACTIVE"),
        ("ssoins-bbbbbbbbbbbbbbb2", "CREATE_IN_PROGRESS"),
    ];

    for max_results in [Some(1), Some(2), None] {
        let case = format!("shared name / MaxResults {max_results:?}");
        let entries = listed(&case, &router, REGION, max_results).await;
        assert_eq!(
            entries.len(),
            2,
            "case {case:?}: both instances are enumerated, got {entries:?}"
        );
        for (instance_id, status) in expected {
            let entry = entries
                .iter()
                .find(|entry| {
                    entry["InstanceArn"]
                        .as_str()
                        .is_some_and(|arn| arn.ends_with(instance_id))
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: instance {instance_id} is not enumerated: {entries:?}")
                });
            assert_state_mirrors_status(&case, entry);
            assert_eq!(
                entry["Name"].as_str(),
                Some(shared_name),
                "case {case:?}: both records carry the shared name, got {entry}"
            );
            assert_eq!(
                entry["State"]["Name"].as_str(),
                Some(status),
                "case {case:?}: expected instance {instance_id} to report State.Name \
                 {status:?}, got {entry}"
            );
        }
    }
}

/// THE RULE across a page boundary: when the activation the clock owes falls due between two
/// pages of one enumeration, the entry served on the LATER page reports the status it holds
/// at that page's instant — in `Status` and in `State.Name` alike — while the earlier page
/// reported the status that was current when it was served.
#[tokio::test]
async fn a_later_page_reports_the_state_owed_at_its_own_instant() {
    let now = SystemTime::now();
    // Two instances whose creation is still in flight, so the activation the clock owes them
    // can be made due part-way through the enumeration. Their ARNs order the enumeration, so
    // a one-entry page size serves them one at a time.
    let (router, engine) = account_holding(vec![
        state::Instance::creating(
            "ssoins-aaaaaaaaaaaaaaa1",
            "d-1a2b3c4d5e",
            state::InstanceType::Organization,
            Some("First".to_string()),
            now + Duration::from_secs(3_600),
            REGION,
        ),
        state::Instance::creating(
            "ssoins-bbbbbbbbbbbbbbb2",
            "d-1a2b3c4d5f",
            state::InstanceType::Organization,
            Some("Second".to_string()),
            now + Duration::from_secs(3_600),
            REGION,
        ),
    ]);

    let case = "page served while the creation is in flight";
    let (first_page, token) = page(case, &router, None).await;
    assert_eq!(
        first_page.len(),
        1,
        "case {case:?}: a one-entry page size serves one instance, got {first_page:?}"
    );
    for entry in &first_page {
        assert_state_mirrors_status(case, entry);
        assert_eq!(
            entry["State"]["Name"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {case:?}: the creation is still in flight, got {entry}"
        );
    }
    let token = token.unwrap_or_else(|| panic!("case {case:?}: the page carries no NextToken"));

    // The clock has moved past the activation of every instance that was owed one.
    with_store(&engine, |store| {
        for instance in store.instances.values_mut() {
            if instance.active_due.is_some() {
                instance.active_due = Some(SystemTime::now() - Duration::from_secs(1));
            }
        }
    });

    let case = "page served after the activation fell due";
    let (second_page, _) = page(case, &router, Some(&token)).await;
    assert_eq!(
        second_page.len(),
        1,
        "case {case:?}: the enumeration resumes with the second instance, got {second_page:?}"
    );
    for entry in &second_page {
        assert_state_mirrors_status(case, entry);
        assert_eq!(
            entry["State"]["Name"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: the activation was due before this page was served, got {entry}"
        );
    }
}

/// One page of `ListInstances` at the smallest page size the model's `MaxResults` admits,
/// resuming from `next_token`: its entries and the token it hands out.
async fn page(
    case: &str,
    router: &MultiServiceRouter,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = json!({"MaxResults": 1});
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    let (status, body) = call_in_region(router, REGION, "ListInstances", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListInstances expected 200, got {status} {body}"
    );
    let entries = body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Instances in {body}"))
        .clone();
    (entries, body["NextToken"].as_str().map(str::to_string))
}
