// SPDX-License-Identifier: CC0-1.0

//! What the two discovery lists report is the inventory of the Regions the account's
//! settings discover NOW.
//!
//! The rule under test: an account discovers exactly the `SourceRegions` its settings name
//! while `LinuxSubscriptionsDiscovery` is `Enabled`, and NO Region while it is not — so
//! once the switch is not `Enabled`, whatever a previous pass recorded while it was stops
//! being reported by both lists and stops being held, rather than staying behind as an
//! inventory no later pass can refresh. The same rule the settings' Region set is narrowed
//! under: a Region the settings no longer name is one nothing reads any more, and turning
//! the switch off removes every Region from discovery at once.
//!
//! The rows span what the rule generalises over:
//!
//! * the inventory held when the switch moves — how many instances, how many Regions they
//!   were found in, how many distinct subscriptions they use and which of them are running;
//! * which `SourceRegions` the settings carry when the switch is no longer `Enabled` (the
//!   same set, a subset, a set naming none of the Regions the records are held for, a
//!   wider one), because what decides is the switch and not that set;
//! * the account that holds records and has no settings at all.
//!
//! Each row also reads the lists BEFORE the switch moves, so an empty answer after it is
//! the switch's doing and not an inventory that was never there, and reads them again
//! after discovery is switched back on: the records are GONE rather than hidden, because
//! the next pass reports the inventory as it is then.
//!
//! The records are placed through the harness's seeding seam — discovery reads the account's
//! EC2 inventory, and no operation of this service writes a discovered instance, so a suite
//! whose rule is about what the SWITCH does to that store places the records itself: a row
//! here states the inventory a previous pass left, several subscriptions licensed several
//! ways among them, without launching and settling one EC2 instance per row.
//! `tests/all/discovery_from_ec2.rs` is the suite that drives the pass against EC2 itself,
//! and the narrowing of `SourceRegions` under an enabled switch — the neighbouring case of
//! this rule — is pinned there.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::discovery::{
    SUBSCRIPTION_TYPE_BYOS, SUBSCRIPTION_TYPE_INCLUDED, SUBSCRIPTION_TYPE_MARKETPLACE,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    DiscoveredInstanceRecord, INSTANCE_STATUS_RUNNING,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState;

use crate::harness::{
    call, get_settings, harness, seed_discovered_instance, update_settings, Harness, Reply, REGION,
};

/// `POST /subscription/ListLinuxSubscriptions` and its sibling list (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// Two more Regions, so a row can hold records for more than one Region and name Regions
/// the records were not found in. `REGION` is the Region every call here is signed for.
const OTHER_REGION: &str = "us-east-1";
const THIRD_REGION: &str = "eu-west-1";

/// The two values of the account's discovery switch
/// (`smithy_query(query="shape",shape="LinuxSubscriptionsDiscovery")`: Enabled | Disabled).
const ENABLED: &str = "Enabled";
const DISABLED: &str = "Disabled";

/// An `Instance$Status` of an instance that is not running, so its subscription reports it
/// as discovered without counting it (`Subscription$InstanceCount`: "The total amount of
/// running instances using this subscription").
const STATUS_STOPPED: &str = "stopped";

/// One discovered instance a row holds: the members the two lists report it under.
#[derive(Debug, Clone, Copy)]
struct Found {
    instance_id: &'static str,
    region: &'static str,
    status: &'static str,
    subscription_name: &'static str,
    subscription_type: &'static str,
}

impl Found {
    /// The record a previous pass would have left for it; every other member is what
    /// discovery leaves for an instance no third-party provider was involved in.
    fn record(&self) -> DiscoveredInstanceRecord {
        DiscoveredInstanceRecord {
            instance_id: self.instance_id.to_string(),
            account_id: crate::harness::ACCOUNT.to_string(),
            ami_id: "ami-00000000000000001".to_string(),
            instance_type: "t3.micro".to_string(),
            status: self.status.to_string(),
            region: self.region.to_string(),
            usage_operation: "RunInstances:0010".to_string(),
            subscription_name: self.subscription_name.to_string(),
            subscription_type: self.subscription_type.to_string(),
            ..DiscoveredInstanceRecord::default()
        }
    }
}

/// One inventory a previous pass left behind before the switch moves.
#[derive(Debug, Clone, Copy)]
struct Inventory {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    found: &'static [Found],
}

const INVENTORIES: &[Inventory] = &[
    Inventory {
        case: "one running instance in one Region",
        found: &[Found {
            instance_id: "i-0000000000000001",
            region: REGION,
            status: INSTANCE_STATUS_RUNNING,
            subscription_name: "Red Hat Enterprise Linux",
            subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
        }],
    },
    Inventory {
        case: "three instances of one subscription in one Region, one of them stopped",
        found: &[
            Found {
                instance_id: "i-0000000000000011",
                region: REGION,
                status: INSTANCE_STATUS_RUNNING,
                subscription_name: "SUSE Linux",
                subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
            },
            Found {
                instance_id: "i-0000000000000012",
                region: REGION,
                status: INSTANCE_STATUS_RUNNING,
                subscription_name: "SUSE Linux",
                subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
            },
            Found {
                instance_id: "i-0000000000000013",
                region: REGION,
                status: STATUS_STOPPED,
                subscription_name: "SUSE Linux",
                subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
            },
        ],
    },
    Inventory {
        case: "instances of three differently licensed subscriptions across two Regions",
        found: &[
            Found {
                instance_id: "i-0000000000000021",
                region: REGION,
                status: INSTANCE_STATUS_RUNNING,
                subscription_name: "Ubuntu Pro",
                subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
            },
            Found {
                instance_id: "i-0000000000000022",
                region: OTHER_REGION,
                status: INSTANCE_STATUS_RUNNING,
                subscription_name: "RHEL with HA",
                subscription_type: SUBSCRIPTION_TYPE_MARKETPLACE,
            },
            Found {
                instance_id: "i-0000000000000023",
                region: OTHER_REGION,
                status: STATUS_STOPPED,
                subscription_name: "Red Hat BYOL Linux",
                subscription_type: SUBSCRIPTION_TYPE_BYOS,
            },
        ],
    },
    Inventory {
        case: "a single stopped instance, whose subscription counts none",
        found: &[Found {
            instance_id: "i-0000000000000031",
            region: OTHER_REGION,
            status: STATUS_STOPPED,
            subscription_name: "Red Hat Enterprise Linux",
            subscription_type: SUBSCRIPTION_TYPE_INCLUDED,
        }],
    },
];

/// The `SourceRegions` a row leaves beside the switch once it is no longer `Enabled`,
/// relative to the Regions the held records were found in. `SourceRegions` is a required
/// list of at least one Region
/// (`smithy_query(query="shape",shape="LinuxSubscriptionsDiscoverySettings")`), so a
/// disable still names one: what decides is the switch, not the set.
#[derive(Debug, Clone, Copy)]
enum LeftNaming {
    /// Exactly the Regions the records were found in.
    TheSameRegions,
    /// Only the first of them.
    OneOfThem,
    /// A Region none of the records were found in.
    SomewhereElse,
    /// Those Regions and one more.
    ThoseAndMore,
}

impl LeftNaming {
    fn label(self) -> &'static str {
        match self {
            Self::TheSameRegions => "the same SourceRegions",
            Self::OneOfThem => "one of those SourceRegions",
            Self::SomewhereElse => "SourceRegions naming none of them",
            Self::ThoseAndMore => "those SourceRegions and one more",
        }
    }

    fn regions(self, discovered: &[String]) -> Vec<String> {
        match self {
            Self::TheSameRegions => discovered.to_vec(),
            Self::OneOfThem => vec![discovered[0].clone()],
            Self::SomewhereElse => vec![THIRD_REGION.to_string()],
            Self::ThoseAndMore => {
                let mut regions = discovered.to_vec();
                regions.push(THIRD_REGION.to_string());
                regions
            }
        }
    }
}

const LEFT_NAMINGS: &[LeftNaming] = &[
    LeftNaming::TheSameRegions,
    LeftNaming::OneOfThem,
    LeftNaming::SomewhereElse,
    LeftNaming::ThoseAndMore,
];

/// The Regions an inventory's records were found in, each once, in a stable order.
fn regions_of(inventory: &Inventory) -> Vec<String> {
    inventory
        .found
        .iter()
        .map(|found| found.region.to_string())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// The entries a list body reports under `key`.
fn entries(reply: &Reply, key: &str, case: &str) -> Vec<Value> {
    reply
        .ok(case)
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} in {:?}", reply.body))
        .clone()
}

/// A string member of a reported entry.
fn member(entry: &Value, name: &str, case: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
        .to_string()
}

/// The instances `ListLinuxSubscriptionInstances` reports, by `InstanceID`, sorted.
async fn reported_instances(h: &Harness, case: &str) -> Vec<String> {
    let reply = call(h, INSTANCES, &json!({})).await;
    let mut ids: Vec<String> = entries(&reply, "Instances", case)
        .iter()
        .map(|entry| member(entry, "InstanceID", case))
        .collect();
    ids.sort();
    ids
}

/// The subscriptions `ListLinuxSubscriptions` reports, as sorted `(name, type, count)`.
async fn reported_subscriptions(h: &Harness, case: &str) -> Vec<(String, String, i64)> {
    let reply = call(h, SUBSCRIPTIONS, &json!({})).await;
    let mut reported: Vec<(String, String, i64)> = entries(&reply, "Subscriptions", case)
        .iter()
        .map(|entry| {
            (
                member(entry, "Name", case),
                member(entry, "Type", case),
                entry
                    .get("InstanceCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(|| panic!("case {case:?}: no InstanceCount in {entry:?}")),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// What an inventory's records must be reported as while the account discovers their
/// Regions: every instance, and one subscription per distinct (name, type) pair counting
/// the running instances using it.
fn expected_of(inventory: &Inventory) -> (Vec<String>, Vec<(String, String, i64)>) {
    let mut ids: Vec<String> = inventory
        .found
        .iter()
        .map(|found| found.instance_id.to_string())
        .collect();
    ids.sort();
    let mut subscriptions: Vec<(String, String, i64)> = inventory
        .found
        .iter()
        .map(|found| {
            (
                found.subscription_name.to_string(),
                found.subscription_type.to_string(),
            )
        })
        .collect::<BTreeSet<(String, String)>>()
        .into_iter()
        .map(|(name, subscription_type)| {
            let count = inventory
                .found
                .iter()
                .filter(|found| {
                    found.subscription_name == name
                        && found.subscription_type == subscription_type
                        && found.status == INSTANCE_STATUS_RUNNING
                })
                .count();
            (
                name,
                subscription_type,
                i64::try_from(count).expect("a small instance count"),
            )
        })
        .collect();
    subscriptions.sort();
    (ids, subscriptions)
}

/// How many discovered instances the store holds — the inventory a later pass would have
/// to refresh, read through the same seam the handlers use.
fn held(h: &Harness) -> usize {
    h.engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .discovered_instances
        .len()
}

/// Persist the account's settings with `discovery` as the switch and `source_regions`
/// beside it, and assert the switch the settings now report.
async fn set_discovery(h: &Harness, discovery: &str, source_regions: &[String], case: &str) {
    let reply = update_settings(
        h,
        &json!({
            "LinuxSubscriptionsDiscovery": discovery,
            "LinuxSubscriptionsDiscoverySettings": {
                "OrganizationIntegration": "Disabled",
                "SourceRegions": source_regions,
            },
            "AllowUpdate": true,
        }),
    )
    .await;
    let body = reply.ok(case);
    assert_eq!(
        body.get("LinuxSubscriptionsDiscovery"),
        Some(&json!(discovery)),
        "case {case:?}: the persisted switch must be {discovery:?}, got {body:?}"
    );
}

/// Assert both lists report nothing and the store holds no record.
async fn reports_nothing(h: &Harness, case: &str) {
    let instances = reported_instances(h, case).await;
    assert!(
        instances.is_empty(),
        "case {case:?}: an account that discovers no Region reports no discovered instance, \
         got {instances:?}"
    );
    let subscriptions = reported_subscriptions(h, case).await;
    assert!(
        subscriptions.is_empty(),
        "case {case:?}: an account that discovers no Region reports no subscription, got \
         {subscriptions:?}"
    );
    let remaining = held(h);
    assert_eq!(
        remaining, 0,
        "case {case:?}: nothing a pass can no longer refresh may stay in the store, {remaining} \
         record(s) left"
    );
}

#[tokio::test]
async fn switching_discovery_off_stops_both_lists_reporting_what_a_pass_had_found() {
    for inventory in INVENTORIES {
        let discovered = regions_of(inventory);
        let discovered_named: Vec<&str> = discovered.iter().map(String::as_str).collect();
        let (expected_ids, expected_subscriptions) = expected_of(inventory);

        for naming in LEFT_NAMINGS {
            let case = &format!("{}, disabled with {}", inventory.case, naming.label());
            let h = harness();
            set_discovery(&h, ENABLED, &discovered, case).await;
            for found in inventory.found {
                seed_discovered_instance(&h, found.record());
            }

            // Before the switch moves: the account discovers those Regions, so both lists
            // report what the pass found there — which is what makes the empty answers
            // below the switch's doing.
            let reported = reported_instances(&h, case).await;
            assert_eq!(
                reported, expected_ids,
                "case {case:?}: while discovery is enabled for {discovered_named:?} the \
                 instances {expected_ids:?} must be reported, got {reported:?}"
            );
            let reported = reported_subscriptions(&h, case).await;
            assert_eq!(
                reported, expected_subscriptions,
                "case {case:?}: while discovery is enabled the subscriptions \
                 {expected_subscriptions:?} must be reported, got {reported:?}"
            );

            // The switch moves. The settings answer for themselves, and the two lists
            // answer from the Regions those settings discover now: none.
            let left = naming.regions(&discovered);
            set_discovery(&h, DISABLED, &left, case).await;
            let settings = get_settings(&h).await;
            let body = settings.ok(case);
            assert_eq!(
                body.get("LinuxSubscriptionsDiscovery"),
                Some(&json!(DISABLED)),
                "case {case:?}: the settings must report the switch as {DISABLED:?}, got {body:?}"
            );
            reports_nothing(&h, case).await;

            // And it stays that way: a second pair of reports answers the same, because
            // the switch is what decides and no pass put anything back.
            reports_nothing(&h, &format!("{case}, read again")).await;

            // Switched back on, the records are GONE rather than hidden: the next pass
            // reports the inventory as it is then, and nothing here runs an instance.
            set_discovery(&h, ENABLED, &discovered, case).await;
            reports_nothing(&h, &format!("{case}, after discovery was enabled again")).await;
        }
    }
}

#[tokio::test]
async fn an_account_with_no_settings_at_all_discovers_no_region() {
    for inventory in INVENTORIES {
        let case = &format!("{}, held by an account with no settings", inventory.case);
        let h = harness();
        for found in inventory.found {
            seed_discovered_instance(&h, found.record());
        }
        // No `UpdateServiceSettings` call has been made, so the account names no source
        // Region at all — the same thing the disabled switch amounts to.
        reports_nothing(&h, case).await;
    }
}
