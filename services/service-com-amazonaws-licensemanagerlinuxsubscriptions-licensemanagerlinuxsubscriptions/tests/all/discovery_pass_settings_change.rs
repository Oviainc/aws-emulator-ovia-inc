// SPDX-License-Identifier: CC0-1.0

//! A discovery pass answers to the settings as they are WHILE IT RUNS, not to the ones it
//! started from.
//!
//! The rule under test: every hop a discovery pass makes and every write it lands is
//! decided by the account's `LinuxSubscriptionsDiscovery` and `SourceRegions` AT THAT
//! INSTANT, so a settings change a client persists while a pass is in flight decides what
//! the store holds when the pass ends — no data is read, written or kept for a Region the
//! settings no longer name, nor for an account whose discovery is no longer enabled, even
//! when the pass was already running when the change landed. A pass reads the account's EC2
//! inventory over several in-process hops and the settings singleton is one
//! `UpdateServiceSettings` call away throughout, so "the settings the pass began with" and
//! "the settings the account has" are two different things.
//!
//! How a row lands a change mid-pass, without one wall-clock guess:
//!
//! * the fleet runs one instance in each of several Regions, and the pass asks about them in
//!   a stable order ([`FLEET_REGIONS`], written in that order), so the record of the FIRST
//!   Region appearing in the store means the pass has started and read the settings it began
//!   with, and the record of the LAST Region still being absent means its remaining writes —
//!   and the pruning step that ends the pass — are still to come;
//! * the row persists its change only once the store shows that window, and then checks the
//!   LAST Region's record is STILL absent now that `UpdateServiceSettings` has answered: the
//!   pass therefore reached neither that write nor its pruning step before the change was
//!   persisted, which is what makes the attempt a test of this rule rather than of a
//!   settings change that landed before or after a pass;
//! * an attempt that misses that window asserts nothing and is retried, so the probe losing
//!   a race is never read as a verdict on the service.
//!
//! The rows span what the rule generalises over: a Region dropped from `SourceRegions`, the
//! set narrowed to the one Region the pass reads last, the set replaced by a Region the
//! account runs nothing in, and the discovery switch itself turned off with the Region set
//! left alone.
//!
//! The fleet, the AMI and the two services on one engine come from
//! `tests/all/discovery_from_ec2.rs`, which is the suite that drives discovery against EC2
//! itself; this one only adds the concurrent settings change.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState;

use crate::discovery_from_ec2::{Fleet, DISABLED, ENABLED, RHEL};
use crate::harness::{Harness, Reply, REGION};

/// The Regions the fleet runs an instance in, IN THE ORDER a pass asks about them: the pass
/// walks the distinct source Regions in sorted order, so the first entry here is the Region
/// whose record appears first and the last entry the one whose record appears last.
///
/// `REGION` is the Region every call is signed for and sorts last of these, which is why it
/// is the entry a row watches for.
const FLEET_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-south-1",
    "eu-central-1",
    "eu-west-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    REGION,
];

/// A Region the fleet runs nothing in, for the row whose change points `SourceRegions`
/// somewhere else entirely.
const SPARE_REGION: &str = "ca-central-1";

/// How many times a row re-runs its race before giving up on catching a pass in flight.
///
/// One attempt costs one pass over the fleet's Regions, so a row that keeps missing the
/// window still finishes quickly.
const ATTEMPTS: usize = 25;

/// How long a row watches the store for the window it lands its change in. Generous: it is
/// only ever reached when a pass is already running, and the attempt is abandoned rather
/// than failed when it elapses.
const WINDOW_BUDGET: Duration = Duration::from_secs(10);

/// One settings change a client persists while a pass is in flight.
#[derive(Debug, Clone, Copy)]
struct Change {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// The `LinuxSubscriptionsDiscovery` the change persists.
    discovery: &'static str,
    /// The `SourceRegions` it persists beside that switch.
    source_regions: &'static [&'static str],
}

const CHANGES: &[Change] = &[
    Change {
        case: "the Region the pass reads first is taken out of SourceRegions",
        discovery: ENABLED,
        source_regions: &[
            "ap-south-1",
            "eu-central-1",
            "eu-west-1",
            "sa-east-1",
            "us-east-1",
            "us-east-2",
            REGION,
        ],
    },
    Change {
        case: "SourceRegions narrowed to the one Region the pass reads last",
        discovery: ENABLED,
        source_regions: &[REGION],
    },
    Change {
        case: "SourceRegions replaced by a Region the account runs nothing in",
        discovery: ENABLED,
        source_regions: &[SPARE_REGION],
    },
    Change {
        case: "discovery disabled, with the same SourceRegions left beside the switch",
        discovery: DISABLED,
        source_regions: FLEET_REGIONS,
    },
];

/// The instances that must be discovered once `change` is the account's settings: the fleet's
/// instances in the Regions those settings name, and none at all while the switch is not
/// `ENABLED` — whatever the Region set beside it says.
fn expected_ids(change: &Change, fleet: &BTreeMap<String, String>) -> Vec<String> {
    if change.discovery != ENABLED {
        return Vec::new();
    }
    let mut ids: Vec<String> = fleet
        .iter()
        .filter(|(region, _)| change.source_regions.contains(&region.as_str()))
        .map(|(_, instance_id)| instance_id.clone())
        .collect();
    ids.sort();
    ids
}

/// The instances a `ListLinuxSubscriptionInstances` body reports, by `InstanceID`, sorted.
fn reported_instance_ids(reply: &Reply, case: &str) -> Vec<String> {
    let mut ids: Vec<String> = reply
        .ok(case)
        .get("Instances")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Instances in {:?}", reply.body))
        .iter()
        .map(|entry| {
            entry
                .get("InstanceID")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no InstanceID in {entry:?}"))
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The discovered instances the store holds, read through the same seam the handlers use —
/// which is how a row watches a pass that is still running instead of running a second one.
fn stored_instance_ids(h: &Harness) -> BTreeSet<String> {
    h.engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .discovered_instances
        .keys()
        .cloned()
        .collect()
}

/// Forget everything an earlier attempt's pass discovered, so the records a row watches for
/// are the ones the pass it is racing wrote.
fn forget_what_was_discovered(h: &Harness) {
    h.engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .discovered_instances
        .clear();
}

/// Watch the store until the running pass has recorded `first` and not yet `last`, which
/// places it past the settings it began with and short of both its last write and the
/// pruning step after it.
///
/// `false` when the pass got through that window unobserved: the attempt lands no change and
/// is retried.
fn caught_mid_pass(h: &Harness, first: &str, last: &str) -> bool {
    let deadline = Instant::now() + WINDOW_BUDGET;
    while Instant::now() < deadline {
        let stored = stored_instance_ids(h);
        if stored.contains(last) {
            return false;
        }
        if stored.contains(first) {
            return true;
        }
        std::thread::yield_now();
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_settings_change_landing_mid_pass_decides_what_that_pass_writes_and_keeps() {
    let fleet = Arc::new(Fleet::new());
    let image_id = fleet.publish_image(RHEL);
    // One instance per Region, launched through EC2 itself, so each Region the pass asks
    // about has exactly one record to discover and the store says which Region the pass has
    // reached. Discovery records an instance in whatever state short of `terminated` EC2
    // reports it in, so nothing here waits out EC2's own clock.
    let mut launched: BTreeMap<String, String> = BTreeMap::new();
    for region in FLEET_REGIONS {
        let ids = fleet.launch_now(region, &image_id, 1).await;
        launched.insert((*region).to_string(), ids[0].clone());
    }
    let first = launched[FLEET_REGIONS[0]].clone();
    let last = launched[FLEET_REGIONS[FLEET_REGIONS.len() - 1]].clone();

    for change in CHANGES {
        let case = change.case;
        let expected = expected_ids(change, &launched);
        let mut landed_mid_pass = false;
        for _ in 0..ATTEMPTS {
            // The pass this attempt races begins from settings naming every Region of the
            // fleet, over a store holding nothing.
            fleet.set_discovery(ENABLED, FLEET_REGIONS).await;
            forget_what_was_discovered(&fleet.h);

            let report = tokio::spawn({
                let fleet = Arc::clone(&fleet);
                async move { fleet.instances(&json!({})).await }
            });
            if !caught_mid_pass(&fleet.h, &first, &last) {
                report.await.expect("the report must not panic");
                continue;
            }
            // The change is persisted over the wire, the way a client makes it, while the
            // pass is between two of its writes.
            fleet
                .set_discovery(change.discovery, change.source_regions)
                .await;
            // …and the last Region's record is still absent now that the change HAS been
            // persisted, so that write and the pruning step after it are still ahead of the
            // pass: this attempt really is a change landing mid-pass.
            let raced = !stored_instance_ids(&fleet.h).contains(&last);
            let reply = report.await.expect("the report must not panic");
            if !raced {
                continue;
            }
            landed_mid_pass = true;

            let listed = reported_instance_ids(&reply, case);
            assert_eq!(
                listed, expected,
                "case {case:?}: the report of a pass the change landed inside must answer \
                 from the settings as they are now, expected {expected:?}, got {listed:?}"
            );
            let held: Vec<String> = stored_instance_ids(&fleet.h).into_iter().collect();
            assert_eq!(
                held, expected,
                "case {case:?}: the pass must leave the store holding exactly what those \
                 settings discover, expected {expected:?}, got {held:?}"
            );
            break;
        }
        assert!(
            landed_mid_pass,
            "case {case:?}: no attempt of {ATTEMPTS} landed the change while a pass was in \
             flight, so the rule was never exercised"
        );
    }
}
