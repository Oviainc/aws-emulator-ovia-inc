// SPDX-License-Identifier: CC0-1.0

//! Discovery picks its plan up where the previous pass stopped, so EVERY configured source
//! Region is discovered and not only the prefix of them one pass reaches.
//!
//! The rule under test: one pass reads a bounded amount of the owner's inventory, so a plan
//! it cannot read to the end leaves a remainder — and that remainder is where the next pass
//! begins, which makes every source Region the account's settings name discovered within a
//! bounded number of reports however many of them there are (the modelled
//! `LinuxSubscriptionsDiscoverySettings$SourceRegions` admits 1..100 entries:
//! `smithy_query(query="shape",shape="LinuxSubscriptionsDiscoverySettings")`). No Region of
//! the plan is therefore left holding nothing while another is re-read, and what an earlier
//! pass discovered elsewhere in the plan survives a later one.
//!
//! The rows span what the rule generalises over: HOW MANY source Regions the account
//! configures — fewer than one pass reads, as many as it reads, one more than that, well
//! past it, and a set narrowed again afterwards — each asserted over BOTH discovery lists,
//! which project the one store a pass fills. A second table spans the stopping points a pass
//! can leave behind — a Region's inventory finished, the last Region's finished, a Region
//! stopped inside with and without a cursor into it, and a stopping point the settings have
//! since dropped — and asserts the walk the next pass lays out from each.
//!
//! The fleet, the AMI and the two services on one engine come from
//! `tests/all/discovery_from_ec2.rs`, which is the suite that drives discovery against EC2
//! itself; this one only varies the length of the plan.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::discovery::SUBSCRIPTION_TYPE_INCLUDED;
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    is_known_region, DiscoveryResume,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState;

use crate::discovery_from_ec2::{reported_subscriptions, Fleet, ENABLED, RHEL};
use crate::harness::{enable_discovery, harness, Reply};

/// The source Regions the rows configure, in the order the plan sorts them, so a row asking
/// for `n` of them takes the first `n`.
///
/// Every entry is a code of the ONE Region catalog this service holds a Region-valued member
/// to (`crate::state::is_known_region`, asserted below), because `UpdateServiceSettings`
/// refuses a `SourceRegions` entry that names no AWS Region.
const SOURCE_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-northeast-1",
    "ap-south-1",
    "ap-southeast-1",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-west-1",
    "eu-west-2",
    "il-central-1",
    "me-south-1",
    "sa-east-1",
    "us-east-1",
];

/// How many of those Regions each row configures: a plan one pass reads whole, one as long
/// as a pass's own bound on the pages it reads, one entry past it, one far past it, and a
/// plan narrowed again after the long ones.
const PLAN_LENGTHS: &[usize] = &[2, 8, 9, 14, 3];

/// The instances a `ListLinuxSubscriptionInstances` body reports, by the `Instance$Region`
/// each runs in: one record per discovered Region, which is what says which Regions the
/// passes so far have reached.
fn reported_by_region(reply: &Reply, case: &str) -> BTreeMap<String, String> {
    reply
        .ok(case)
        .get("Instances")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Instances in {:?}", reply.body))
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
                    .to_string()
            };
            (member("Region"), member("InstanceID"))
        })
        .collect()
}

#[tokio::test]
async fn every_configured_source_region_is_discovered_over_successive_reports() {
    for region in SOURCE_REGIONS {
        assert!(
            is_known_region(region),
            "setup: {region} must be a code of the Region catalog this service holds a \
             Region-valued member to"
        );
    }

    let fleet = Fleet::new();
    let image_id = fleet.publish_image(RHEL);
    // One running instance per Region, launched through EC2 itself, so the Region a reported
    // record carries says which Region a pass reached — and the subscription's
    // `InstanceCount` ("the total amount of running instances using this subscription")
    // counts one per discovered Region.
    let mut launched: BTreeMap<String, String> = BTreeMap::new();
    for region in SOURCE_REGIONS {
        let ids = fleet.launch(region, &image_id, 1).await;
        launched.insert((*region).to_string(), ids[0].clone());
    }

    for length in PLAN_LENGTHS {
        let regions: Vec<&str> = SOURCE_REGIONS[..*length].to_vec();
        let case = &format!("a plan of {length} source Regions");
        fleet.set_discovery(ENABLED, &regions).await;
        let expected: BTreeMap<String, String> = regions
            .iter()
            .map(|region| ((*region).to_string(), launched[*region].clone()))
            .collect();

        // Every report runs one pass, and a pass reads a bounded part of the plan, so the
        // Regions of a long plan are discovered over successive reports. The bound asserted
        // is ONE report per configured Region — a plan cannot need more than that however
        // little of it one pass reads — while a pass that never resumed would stop at the
        // same prefix of the plan on every one of them.
        let mut reported = BTreeMap::new();
        let mut reports = 0;
        while reports < regions.len() {
            reports += 1;
            let reply = fleet.instances(&json!({})).await;
            reported = reported_by_region(&reply, case);
            let unasked: Vec<&String> = reported
                .keys()
                .filter(|region| !expected.contains_key(*region))
                .collect();
            assert!(
                unasked.is_empty(),
                "case {case:?}: report {reports} must discover only the configured Regions, \
                 got records for {unasked:?}"
            );
            if reported == expected {
                break;
            }
        }
        assert_eq!(
            reported, expected,
            "case {case:?}: every configured Region must be discovered within \
             {} reports, after {reports} the store holds {reported:?}",
            regions.len()
        );

        // And a further pass keeps them: resuming must not drop what an earlier pass
        // discovered in the part of the plan this one does not reach.
        let again = reported_by_region(&fleet.instances(&json!({})).await, case);
        assert_eq!(
            again, expected,
            "case {case:?}: a later report must still hold every discovered Region, got \
             {again:?}"
        );

        // The other discovery list projects the same store, so it counts one running
        // instance per discovered Region.
        let subscriptions = reported_subscriptions(&fleet.subscriptions(&json!({})).await, case);
        let counted = vec![(
            RHEL.platform_details.to_string(),
            SUBSCRIPTION_TYPE_INCLUDED.to_string(),
            i64::try_from(regions.len()).expect("a small Region count"),
        )];
        assert_eq!(
            subscriptions, counted,
            "case {case:?}: expected {counted:?}, got {subscriptions:?}"
        );
    }
}

/// The Regions one row of the plan table configures, sorted, so a row states the walk it
/// expects against a known order.
const PLANNED: &[&str] = &["ap-south-1", "eu-west-1", "us-east-1", "us-west-2"];

/// A Region ordered between two of those, which the settings do NOT name: the stopping point
/// of a pass whose Region the account has since dropped.
const DROPPED: &str = "eu-central-1";

/// The inventory cursor a stopping point inside a Region carries. Opaque to this service —
/// it is the owner's own pagination token — so only that it is handed back to the Region it
/// was reached in, and to no other, is asserted.
const CURSOR: &str = "an-inventory-cursor";

/// One row: the point the previous pass stopped at, the walk the next one must lay out, and
/// the inventory cursor its first Region must resume from.
struct Row {
    case: &'static str,
    stopped: Option<DiscoveryResume>,
    regions: &'static [&'static str],
    cursor: Option<&'static str>,
}

#[tokio::test]
async fn the_next_pass_plans_its_walk_from_where_the_previous_one_stopped() {
    let rows = vec![
        Row {
            case: "no pass has stopped anywhere yet",
            stopped: None,
            regions: PLANNED,
            cursor: None,
        },
        Row {
            case: "a pass that finished the first Region's inventory",
            stopped: Some(DiscoveryResume::After("ap-south-1".to_string())),
            regions: &["eu-west-1", "us-east-1", "us-west-2", "ap-south-1"],
            cursor: None,
        },
        Row {
            case: "a pass that finished the last Region's inventory",
            stopped: Some(DiscoveryResume::After("us-west-2".to_string())),
            regions: PLANNED,
            cursor: None,
        },
        Row {
            case: "a pass that stopped inside a Region's inventory",
            stopped: Some(DiscoveryResume::Inside(
                "us-east-1".to_string(),
                Some(CURSOR.to_string()),
            )),
            regions: &["us-east-1", "us-west-2", "ap-south-1", "eu-west-1"],
            cursor: Some(CURSOR),
        },
        Row {
            case: "a pass that stopped inside a Region before recording a page of it",
            stopped: Some(DiscoveryResume::Inside("us-west-2".to_string(), None)),
            regions: &["us-west-2", "ap-south-1", "eu-west-1", "us-east-1"],
            cursor: None,
        },
        Row {
            case: "a stopping point inside a Region the settings no longer name",
            stopped: Some(DiscoveryResume::Inside(
                DROPPED.to_string(),
                Some(CURSOR.to_string()),
            )),
            regions: &["eu-west-1", "us-east-1", "us-west-2", "ap-south-1"],
            cursor: None,
        },
        Row {
            case: "a stopping point after a Region the settings no longer name",
            stopped: Some(DiscoveryResume::After(DROPPED.to_string())),
            regions: &["eu-west-1", "us-east-1", "us-west-2", "ap-south-1"],
            cursor: None,
        },
    ];

    let h = harness();
    enable_discovery(&h, PLANNED).await;
    let state =
        h.engine
            .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>();

    for row in rows {
        let case = row.case;
        // Recorded through the same seam a pass records it through, so a row states a
        // stopping point rather than racing a pass into leaving one.
        state.lock().record_discovery_resume(row.stopped.clone());
        let plan = state.lock().discovery_plan();
        let expected: Vec<String> = row.regions.iter().map(|region| (*region).to_string()).collect();
        assert_eq!(
            plan.regions, expected,
            "case {case:?}: expected the walk {expected:?}, got {:?}",
            plan.regions
        );
        let expected_cursor = row.cursor.map(str::to_string);
        assert_eq!(
            plan.resume_cursor, expected_cursor,
            "case {case:?}: expected the first Region to resume from {expected_cursor:?}, got \
             {:?}",
            plan.resume_cursor
        );
        // Whatever the stopping point, one pass still asks about every discovered Region
        // exactly once: the walk is the configured set, only rotated.
        let mut walked = plan.regions.clone();
        walked.sort();
        let configured: Vec<String> = PLANNED.iter().map(|region| (*region).to_string()).collect();
        assert_eq!(
            walked, configured,
            "case {case:?}: the walk must cover the configured Regions once each, got {walked:?}"
        );
    }
}
