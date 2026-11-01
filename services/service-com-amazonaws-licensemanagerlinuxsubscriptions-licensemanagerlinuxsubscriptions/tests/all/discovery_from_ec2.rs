// SPDX-License-Identifier: CC0-1.0

//! Linux subscription discovery reads the account's EC2 inventory, so what a client runs
//! is what the two discovery lists report.
//!
//! The rule under test: once `LinuxSubscriptionsDiscovery` is enabled, the instances the
//! account runs in the configured `SourceRegions` are discovered, each reporting the
//! subscription its AMI's billing platform names (`Image$PlatformDetails` is "the platform
//! details associated with the billing code of the AMI"), and `ListLinuxSubscriptions`
//! reports one `Subscription` per distinct (name, type) pair with `InstanceCount` — "the
//! total amount of running instances using this subscription".
//!
//! The instances and the AMIs are EC2's resources, so EC2 is mounted BESIDE this service on
//! ONE engine and driven through the AWS SDK over the same in-process callback this
//! service's own discovery reaches it through. An instance is launched with
//! `ec2:RunInstances` and believed to exist only because EC2 answered for it; nothing here
//! reaches into this service's own store.
//!
//! The rows span what the rule generalises over: the billing platform of the AMI (several
//! Linux subscription platforms, the plain Linux platform that bills none, a composite
//! platform whose operating system IS that plain one, and a Windows one), how the
//! subscription is licensed (included with Amazon EC2, brought by the account, bought
//! through the Marketplace), how many instances share one subscription, the state EC2
//! reports them in (`running`, `stopped`, `terminated`), and the Region they run in against
//! the Regions the settings name.
//!
//! What both lists report is therefore the inventory of the Regions the account's CURRENT
//! settings discover — the `SourceRegions` they name while `LinuxSubscriptionsDiscovery` is
//! enabled, and no Region at all while it is not — so the rows also walk that switch back
//! and forth with the fleet changing under it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::discovery::{
    SUBSCRIPTION_TYPE_BYOS, SUBSCRIPTION_TYPE_INCLUDED, SUBSCRIPTION_TYPE_MARKETPLACE,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::EmulatedService;

use crate::harness::{call_in_region, is_iso8601_utc, Harness, Reply, REGION};

/// `POST /subscription/ListLinuxSubscriptions` and its sibling list (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// A second Region, so the `SourceRegions` rule is asserted over more than one Region's
/// inventory. `REGION` is the one every call here is signed for, which makes it the home
/// Region; this is only a Region instances run in.
pub const OTHER_REGION: &str = "us-east-1";

/// How long EC2 is given to bring an instance to a state its own clock owes it — a launch
/// to `running`, a stop to `stopped`, a termination to `terminated`. Nothing in the emulator
/// sleeps for those; this is the test waiting for that clock.
const EC2_BUDGET: Duration = Duration::from_secs(15);

/// The `InstanceState$Name` values this suite names.
pub const STATE_RUNNING: &str = "running";
pub const STATE_STOPPED: &str = "stopped";
const STATE_TERMINATED: &str = "terminated";

/// `ProductCode$ProductCodeType` of a code bought through the Marketplace
/// (`ProductCodeValues`: devpay | marketplace —
/// `smithy_query(query="shape",shape="ProductCodeValues")` on EC2's model).
const PRODUCT_CODE_TYPE_MARKETPLACE: &str = "marketplace";

/// The two values of the account's discovery switch
/// (`smithy_query(query="shape",shape="LinuxSubscriptionsDiscovery")`: Enabled | Disabled).
pub const ENABLED: &str = "Enabled";
pub const DISABLED: &str = "Disabled";

/// One AMI the account launches from, and the subscription discovering its instances must
/// report.
#[derive(Debug, Clone, Copy)]
pub struct Ami {
    /// What this row is about, named in every assertion it drives.
    case: &'static str,
    /// The billing platform the AMI declares (`Image$PlatformDetails`). The Linux values
    /// here are platforms EC2 itself publishes
    /// (`smithy_query(query="shape",shape="CapacityReservationInstancePlatform")`: `Red Hat
    /// Enterprise Linux`, `SUSE Linux`, `Ubuntu Pro`, `RHEL with HA`, `Linux with SQL Server
    /// Standard`, `Linux/UNIX`, `Windows`), plus the BYOL platform of a Red Hat image an
    /// account brings its own subscription for (Amazon EC2 User Guide, "Understand AMI
    /// billing information").
    pub platform_details: &'static str,
    /// The billing code that platform bills under (`Image$UsageOperation`). Only that the
    /// discovered instance reports its AMI's OWN value is asserted, never a mapping from
    /// it, so these only have to differ from one another.
    pub usage_operation: &'static str,
    /// Whether this is a Windows AMI — "this value is set to `windows` for Windows AMIs;
    /// otherwise, it is blank" (`Image$Platform`).
    windows: bool,
    /// The Marketplace product code the AMI carries, for an image whose licensing is bought
    /// through the Marketplace (`Image$ProductCodes`).
    pub product_code: Option<&'static str>,
    /// How many instances the account launches from it.
    instances: usize,
    /// The `(Subscription$Name, Subscription$Type)` discovery must report for those
    /// instances — `None` for an AMI that bills no Linux subscription at all.
    pub subscription: Option<(&'static str, &'static str)>,
}

/// The AMIs the fleet of the first case is launched from.
pub const AMIS: &[Ami] = &[
    Ami {
        case: "Red Hat Enterprise Linux, two instances",
        platform_details: "Red Hat Enterprise Linux",
        usage_operation: "RunInstances:0010",
        windows: false,
        product_code: None,
        instances: 2,
        subscription: Some(("Red Hat Enterprise Linux", SUBSCRIPTION_TYPE_INCLUDED)),
    },
    Ami {
        case: "SUSE Linux",
        platform_details: "SUSE Linux",
        usage_operation: "RunInstances:000g",
        windows: false,
        product_code: None,
        instances: 1,
        subscription: Some(("SUSE Linux", SUBSCRIPTION_TYPE_INCLUDED)),
    },
    Ami {
        case: "Ubuntu Pro",
        platform_details: "Ubuntu Pro",
        usage_operation: "RunInstances:0g00",
        windows: false,
        product_code: None,
        instances: 1,
        subscription: Some(("Ubuntu Pro", SUBSCRIPTION_TYPE_INCLUDED)),
    },
    Ami {
        case: "RHEL with HA, licensed through the Marketplace",
        platform_details: "RHEL with HA",
        usage_operation: "RunInstances:1010",
        windows: false,
        product_code: Some("aw0evgkw8e5c1q413zgy5pjce"),
        instances: 1,
        subscription: Some(("RHEL with HA", SUBSCRIPTION_TYPE_MARKETPLACE)),
    },
    Ami {
        case: "a Red Hat subscription the account brings itself",
        platform_details: "Red Hat BYOL Linux",
        usage_operation: "RunInstances:0210",
        windows: false,
        product_code: None,
        instances: 1,
        subscription: Some(("Red Hat BYOL Linux", SUBSCRIPTION_TYPE_BYOS)),
    },
    Ami {
        case: "the plain Linux platform, which bills no subscription",
        platform_details: "Linux/UNIX",
        usage_operation: "RunInstances",
        windows: false,
        product_code: None,
        instances: 1,
        subscription: None,
    },
    Ami {
        case: "a composite platform whose operating system is the plain Linux one",
        platform_details: "Linux with SQL Server Standard",
        usage_operation: "RunInstances:0004",
        windows: false,
        product_code: None,
        instances: 1,
        subscription: None,
    },
    Ami {
        case: "a Windows AMI, which is no Linux subscription",
        platform_details: "Windows",
        usage_operation: "RunInstances:0002",
        windows: true,
        product_code: None,
        instances: 1,
        subscription: None,
    },
];

/// The AMI row every case that needs just one Linux subscription launches from.
pub const RHEL: &Ami = &AMIS[0];

/// This service and EC2 — the owner of the instances and the AMIs — over ONE shared engine,
/// with an EC2 client per Region over the router's own in-process callback: the very seam
/// this service's discovery reaches EC2 through.
///
/// `pub` so the suite whose rule is about a settings change landing WHILE a pass is in
/// flight (`tests/all/discovery_pass_settings_change.rs`) drives the pass through this one
/// fleet rather than assembling a second copy of the same two services.
pub struct Fleet {
    pub h: Harness,
    callback: EmbeddedEmulator,
}

impl Fleet {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing; authentication is the framework's concern, not this
        // service's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self {
            h: Harness { router, engine },
            callback,
        }
    }

    /// An EC2 client addressed at `region`, over the in-process callback.
    fn ec2(&self, region: &str) -> ::aws_sdk_ec2::Client {
        let config = ::aws_sdk_ec2::Config::builder()
            .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
            .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
            .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
            .credentials_provider(self.callback.aws_credentials_provider())
            .http_client(self.callback.http_client())
            .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        ::aws_sdk_ec2::Client::from_conf(config)
    }

    /// Publish the AMI `row` describes in EC2's own image store and answer its `ImageId`.
    ///
    /// The billing facts of an AMI are set when the image is created, and no operation of
    /// the EC2 emulator takes them from a request, so the image is placed through EC2's own
    /// store seam — the one its handlers read — rather than through a second copy of it.
    pub fn publish_image(&self, row: &Ami) -> String {
        let store = ec2::state::service_state(&*self.h.engine).expect("EC2 holds its store");
        let mut store = store.lock();
        store.insert_image(ec2::state::ImageSpec {
            name: format!("discovery-{}", row.platform_details),
            description: row.case.to_string(),
            platform: row
                .windows
                .then(|| ec2::state::IMAGE_PLATFORM_WINDOWS.to_string()),
            platform_details: row.platform_details.to_string(),
            usage_operation: row.usage_operation.to_string(),
            product_codes: row
                .product_code
                .map(|code| {
                    vec![ec2::ops::types::ProductCode {
                        product_code_id: Some(code.to_string()),
                        product_code_type: Some(PRODUCT_CODE_TYPE_MARKETPLACE.to_string()),
                    }]
                })
                .unwrap_or_default(),
            ..ec2::state::ImageSpec::default()
        })
    }

    /// Launch `count` instances from `image_id` in `region`, through EC2 itself, and answer
    /// the ids it minted once EC2 reports every one of them `running`.
    pub async fn launch(&self, region: &str, image_id: &str, count: usize) -> Vec<String> {
        let ids = self.launch_now(region, image_id, count).await;
        for id in &ids {
            self.settled_in(region, id, STATE_RUNNING).await;
        }
        ids
    }

    /// Launch `count` instances from `image_id` in `region` AS `instance_type`
    /// (`RunInstancesRequest$InstanceType`) and answer the ids EC2 minted once it reports
    /// every one of them `running`.
    ///
    /// The form a case whose rule is about the instance TYPE a discovered record carries
    /// uses: a launch that names none leaves the type to EC2, so a case filtering on one
    /// asks for the type it then selects by.
    pub async fn launch_as(
        &self,
        region: &str,
        image_id: &str,
        instance_type: &str,
        count: usize,
    ) -> Vec<String> {
        let ids = self
            .run(region, image_id, Some(instance_type), count)
            .await;
        for id in &ids {
            self.settled_in(region, id, STATE_RUNNING).await;
        }
        ids
    }

    /// Launch `count` instances from `image_id` in `region` and answer the ids EC2 minted,
    /// WITHOUT waiting for the state EC2's clock owes them.
    ///
    /// Discovery records an instance in whatever state EC2 reports it in short of
    /// `terminated`, so a case whose rule is about WHICH instances a pass writes — not what
    /// `Status` they report — does not have to wait that span out per Region.
    pub async fn launch_now(&self, region: &str, image_id: &str, count: usize) -> Vec<String> {
        self.run(region, image_id, None, count).await
    }

    /// Launch `count` instances from `image_id` in `region`, as `instance_type` when the
    /// caller asked for one, and answer the ids EC2 minted — the ONE launch every form
    /// above goes through, so a case cannot reach EC2 a second way.
    async fn run(
        &self,
        region: &str,
        image_id: &str,
        instance_type: Option<&str>,
        count: usize,
    ) -> Vec<String> {
        let requested = i32::try_from(count).expect("a small instance count");
        let launched = self
            .ec2(region)
            .run_instances()
            .image_id(image_id)
            .set_instance_type(instance_type.map(::aws_sdk_ec2::types::InstanceType::from))
            .min_count(requested)
            .max_count(requested)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup: EC2 must launch {count} instances of {image_id}: {error:?}")
            });
        let ids: Vec<String> = launched
            .instances()
            .iter()
            .filter_map(|instance| instance.instance_id().map(str::to_string))
            .collect();
        assert_eq!(
            ids.len(),
            count,
            "setup: expected {count} launched instance ids, got {ids:?}"
        );
        ids
    }

    /// Stop `instance_id` through EC2 itself and wait until it reports it stopped.
    pub async fn stop(&self, region: &str, instance_id: &str) {
        self.ec2(region)
            .stop_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must stop {instance_id}: {error:?}"));
        self.settled_in(region, instance_id, STATE_STOPPED).await;
    }

    /// Terminate `instance_id` through EC2 itself and wait until it reports it terminated.
    async fn terminate(&self, region: &str, instance_id: &str) {
        self.ec2(region)
            .terminate_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: EC2 must terminate {instance_id}: {error:?}"));
        self.settled_in(region, instance_id, STATE_TERMINATED).await;
    }

    /// Wait until EC2 reports `instance_id` in `state`.
    async fn settled_in(&self, region: &str, instance_id: &str, state: &str) {
        let deadline = Instant::now() + EC2_BUDGET;
        loop {
            let reported = self.instance_state(region, instance_id).await;
            if reported == state {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "setup: {instance_id} must reach {state}, EC2 reports {reported}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The `InstanceState$Name` EC2 reports for `instance_id`.
    async fn instance_state(&self, region: &str, instance_id: &str) -> String {
        self.ec2(region)
            .describe_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must describe {instance_id}: {error:?}"))
            .reservations()
            .iter()
            .flat_map(::aws_sdk_ec2::types::Reservation::instances)
            .find(|instance| instance.instance_id() == Some(instance_id))
            .and_then(|instance| instance.state()?.name().map(|name| name.as_str().to_string()))
            .unwrap_or_else(|| panic!("EC2 must report a state for {instance_id}"))
    }

    /// The account EC2 says owns `instance_id` (`Reservation$OwnerId`) — what a discovered
    /// instance's `AccountID` has to agree with, read from the owner rather than written
    /// here.
    pub async fn owner_of(&self, region: &str, instance_id: &str) -> String {
        self.ec2(region)
            .describe_instances()
            .instance_ids(instance_id)
            .send()
            .await
            .unwrap_or_else(|error| panic!("EC2 must describe {instance_id}: {error:?}"))
            .reservations()
            .iter()
            .find(|reservation| {
                reservation
                    .instances()
                    .iter()
                    .any(|instance| instance.instance_id() == Some(instance_id))
            })
            .and_then(|reservation| reservation.owner_id().map(str::to_string))
            .unwrap_or_else(|| panic!("EC2 must report an owner for {instance_id}"))
    }

    /// Enable discovery for `source_regions`, the way a client onboards the account.
    async fn enable_discovery(&self, source_regions: &[&str]) {
        self.set_discovery(ENABLED, source_regions).await;
    }

    /// Persist `discovery` as this account's `LinuxSubscriptionsDiscovery`, with
    /// `source_regions` as the `SourceRegions` beside it: the one call a client onboards
    /// the account with and the one it later turns discovery off again with.
    pub async fn set_discovery(&self, discovery: &str, source_regions: &[&str]) {
        let case = &format!("UpdateServiceSettings {discovery} for {source_regions:?}");
        let reply = call_in_region(
            &self.h,
            REGION,
            "/subscription/UpdateServiceSettings",
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
        let settings = reply.ok(case);
        assert_eq!(
            settings.get("LinuxSubscriptionsDiscovery"),
            Some(&json!(discovery)),
            "case {case:?}: the persisted discovery switch must be {discovery:?}, got \
             {settings:?}"
        );
    }

    /// `ListLinuxSubscriptions` with `body`.
    pub async fn subscriptions(&self, body: &Value) -> Reply {
        call_in_region(&self.h, REGION, SUBSCRIPTIONS, body).await
    }

    /// `ListLinuxSubscriptionInstances` with `body`.
    pub async fn instances(&self, body: &Value) -> Reply {
        call_in_region(&self.h, REGION, INSTANCES, body).await
    }
}

/// The entries a list body reports under `key`.
fn entries<'a>(reply: &'a Reply, key: &str, case: &str) -> &'a Vec<Value> {
    reply
        .ok(case)
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} in {:?}", reply.body))
}

/// A string member of a list entry.
fn member(entry: &Value, name: &str, case: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
        .to_string()
}

/// The subscriptions a `ListLinuxSubscriptions` body reports, as `(name, type, count)`.
pub fn reported_subscriptions(reply: &Reply, case: &str) -> Vec<(String, String, i64)> {
    entries(reply, "Subscriptions", case)
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
        .collect()
}

/// The instances a `ListLinuxSubscriptionInstances` body reports, by `InstanceID`.
pub fn reported_instance_ids(reply: &Reply, case: &str) -> Vec<String> {
    entries(reply, "Instances", case)
        .iter()
        .map(|entry| member(entry, "InstanceID", case))
        .collect()
}

/// The reported instance entry for `instance_id`.
fn reported_instance(reply: &Reply, instance_id: &str, case: &str) -> Value {
    entries(reply, "Instances", case)
        .iter()
        .find(|entry| member(entry, "InstanceID", case) == instance_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {instance_id} is missing from {:?}", reply.body))
}

#[tokio::test]
async fn one_subscription_per_billing_platform_of_the_discovered_fleet() {
    let fleet = Fleet::new();
    fleet.enable_discovery(&[REGION]).await;

    // Every row's fleet, launched through EC2 itself in the one discovered Region.
    let mut launched: Vec<(&Ami, Vec<String>)> = Vec::new();
    for row in AMIS {
        let image_id = fleet.publish_image(row);
        let ids = fleet.launch(REGION, &image_id, row.instances).await;
        launched.push((row, ids));
    }

    let case = "the discovered fleet";
    let mut expected: Vec<(String, String, i64)> = launched
        .iter()
        .filter_map(|(row, ids)| {
            let (name, subscription_type) = row.subscription?;
            Some((
                name.to_string(),
                subscription_type.to_string(),
                i64::try_from(ids.len()).expect("a small instance count"),
            ))
        })
        .collect();
    // The list reports its subscriptions in (name, type) order.
    expected.sort();
    let actual = reported_subscriptions(&fleet.subscriptions(&json!({})).await, case);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );

    // The same pass feeds the instance list, so exactly the instances of the AMIs that bill
    // a Linux subscription are reported there — and each one reports its own AMI's facts.
    let reply = fleet.instances(&json!({})).await;
    let listed: BTreeSet<String> = reported_instance_ids(&reply, case).into_iter().collect();
    let mut expected_ids: BTreeSet<String> = BTreeSet::new();
    for (row, ids) in &launched {
        for id in ids {
            let case = &format!("{} instance {id}", row.case);
            if row.subscription.is_none() {
                assert!(
                    !listed.contains(id),
                    "case {case:?}: an AMI that bills no Linux subscription must not be \
                     discovered, got {:?}",
                    reply.body
                );
                continue;
            }
            expected_ids.insert(id.clone());
            let entry = reported_instance(&reply, id, case);
            let expected_owner = fleet.owner_of(REGION, id).await;
            for (name, expected) in [
                ("UsageOperation", row.usage_operation.to_string()),
                ("Region", REGION.to_string()),
                ("Status", STATE_RUNNING.to_string()),
                ("AccountID", expected_owner),
                (
                    "SubscriptionName",
                    row.subscription
                        .expect("a row with a subscription")
                        .0
                        .to_string(),
                ),
            ] {
                let actual = member(&entry, name, case);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: {name} expected {expected:?}, got {actual:?}"
                );
            }
            let updated = member(&entry, "LastUpdatedTime", case);
            assert!(
                is_iso8601_utc(&updated),
                "case {case:?}: LastUpdatedTime must be an ISO-8601 UTC instant, got \
                 {updated:?}"
            );
            let codes: Vec<String> = entry
                .get("ProductCode")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
                .iter()
                .map(|code| code.as_str().unwrap_or_default().to_string())
                .collect();
            let expected_codes: Vec<String> =
                row.product_code.map(str::to_string).into_iter().collect();
            assert_eq!(
                codes, expected_codes,
                "case {case:?}: ProductCode expected {expected_codes:?}, got {codes:?}"
            );
        }
    }
    let expected_ids: Vec<String> = expected_ids.into_iter().collect();
    let listed: Vec<String> = listed.into_iter().collect();
    assert_eq!(
        listed, expected_ids,
        "case {case:?}: expected the discovered instances {expected_ids:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn an_instance_launched_after_discovery_was_enabled_is_discovered() {
    let fleet = Fleet::new();
    fleet.enable_discovery(&[REGION]).await;

    let before = "an enabled account with nothing running";
    let empty = reported_subscriptions(&fleet.subscriptions(&json!({})).await, before);
    assert!(
        empty.is_empty(),
        "case {before:?}: expected no subscriptions, got {empty:?}"
    );

    // One instance launched now, with discovery already on: the next read discovers it.
    let image_id = fleet.publish_image(RHEL);
    let ids = fleet.launch(REGION, &image_id, 1).await;

    let case = "one instance launched after onboarding";
    let (name, subscription_type) = RHEL.subscription.expect("the row carries a subscription");
    let expected = vec![(name.to_string(), subscription_type.to_string(), 1)];
    let actual = reported_subscriptions(&fleet.subscriptions(&json!({})).await, case);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    let listed = reported_instance_ids(&fleet.instances(&json!({})).await, case);
    assert_eq!(
        listed, ids,
        "case {case:?}: expected the launched instance {ids:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn the_count_is_the_running_instances_and_a_terminated_one_is_gone() {
    let fleet = Fleet::new();
    fleet.enable_discovery(&[REGION]).await;
    let image_id = fleet.publish_image(RHEL);
    let ids = fleet.launch(REGION, &image_id, 3).await;
    fleet.stop(REGION, &ids[1]).await;
    fleet.terminate(REGION, &ids[2]).await;

    let case = "one running, one stopped, one terminated";
    let (name, subscription_type) = RHEL.subscription.expect("the row carries a subscription");
    // `InstanceCount` is "the total amount of RUNNING instances using this subscription",
    // so the stopped instance is discovered without being counted, and the terminated one
    // is not discovered at all.
    let expected = vec![(name.to_string(), subscription_type.to_string(), 1)];
    let actual = reported_subscriptions(&fleet.subscriptions(&json!({})).await, case);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );

    let reply = fleet.instances(&json!({})).await;
    let listed = reported_instance_ids(&reply, case);
    let expected_ids = vec![ids[0].clone(), ids[1].clone()];
    assert_eq!(
        listed, expected_ids,
        "case {case:?}: expected {expected_ids:?}, got {listed:?}"
    );
    for (instance_id, expected_status) in [(&ids[0], STATE_RUNNING), (&ids[1], STATE_STOPPED)] {
        let entry = reported_instance(&reply, instance_id, case);
        let actual = member(&entry, "Status", case);
        assert_eq!(
            actual, expected_status,
            "case {case:?}: {instance_id} Status expected {expected_status:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn discovery_covers_exactly_the_configured_source_regions() {
    let fleet = Fleet::new();
    let image_id = fleet.publish_image(RHEL);
    let here = fleet.launch(REGION, &image_id, 1).await;
    let elsewhere = fleet.launch(OTHER_REGION, &image_id, 1).await;

    // (the source Regions configured, the instances discovery must report)
    let cases: &[(&[&str], Vec<String>)] = &[
        (&[REGION], here.clone()),
        (
            &[REGION, OTHER_REGION],
            here.iter().chain(elsewhere.iter()).cloned().collect(),
        ),
        (&[OTHER_REGION], elsewhere.clone()),
        (&[REGION], here.clone()),
    ];
    for (source_regions, expected) in cases {
        let case = &format!("SourceRegions {source_regions:?}");
        fleet.enable_discovery(source_regions).await;
        let reply = fleet.instances(&json!({})).await;
        let mut listed = reported_instance_ids(&reply, case);
        listed.sort();
        let mut expected = expected.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
        let subscriptions = reported_subscriptions(&fleet.subscriptions(&json!({})).await, case);
        let counted: i64 = subscriptions.iter().map(|(_, _, count)| count).sum();
        let running = i64::try_from(expected.len()).expect("a small instance count");
        assert_eq!(
            counted, running,
            "case {case:?}: the counts ({counted}) must add up to the discovered running \
             instances ({running}), got {subscriptions:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_list_leaves_the_next_reader_the_same_answer() {
    let fleet = Fleet::new();
    fleet.enable_discovery(&[REGION]).await;
    let image_id = fleet.publish_image(RHEL);
    fleet.launch(REGION, &image_id, 2).await;

    for (path, case) in [
        (SUBSCRIPTIONS, "ListLinuxSubscriptions twice"),
        (INSTANCES, "ListLinuxSubscriptionInstances twice"),
    ] {
        let first = call_in_region(&fleet.h, REGION, path, &json!({})).await;
        let second = call_in_region(&fleet.h, REGION, path, &json!({})).await;
        assert_eq!(
            first.ok(case),
            second.ok(case),
            "case {case:?}: a read must leave the next reader the same answer"
        );
    }
}

#[tokio::test]
async fn a_walk_of_the_discovered_list_covers_every_record_once() {
    let fleet = Fleet::new();
    fleet.enable_discovery(&[REGION]).await;
    let image_id = fleet.publish_image(RHEL);
    let mut ids = fleet.launch(REGION, &image_id, 3).await;
    // The store reports its records in instance-id order, which is the order a cursor names
    // a position in.
    ids.sort();

    let case = "a paginated walk with the fleet changing under it";
    let first = fleet.instances(&json!({ "MaxResults": 2 })).await;
    let mut walked = reported_instance_ids(&first, case);
    let token = first
        .ok(case)
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a truncated page carries a NextToken, got {:?}", first.body))
        .to_string();

    // The account's inventory changes between the two pages: the record the first page
    // started at is gone. The cursor is a position in the list the first page reported, so
    // the walk must still cover each of its records exactly once rather than skipping the
    // one that would have moved across the cursor.
    fleet.terminate(REGION, &ids[0]).await;

    let second = fleet
        .instances(&json!({ "MaxResults": 2, "NextToken": token }))
        .await;
    walked.extend(reported_instance_ids(&second, case));
    assert_eq!(
        walked, ids,
        "case {case:?}: expected the walk to cover {ids:?} once each, got {walked:?}"
    );

    // A fresh walk runs a pass again, so it reports the inventory as it is now.
    let after = "a fresh walk after the termination";
    let listed = reported_instance_ids(&fleet.instances(&json!({})).await, after);
    let expected = vec![ids[1].clone(), ids[2].clone()];
    assert_eq!(
        listed, expected,
        "case {after:?}: expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn nothing_is_discovered_until_the_account_enables_discovery() {
    let fleet = Fleet::new();
    let image_id = fleet.publish_image(RHEL);
    fleet.launch(REGION, &image_id, 2).await;

    for (path, key) in [(SUBSCRIPTIONS, "Subscriptions"), (INSTANCES, "Instances")] {
        let case = &format!("{path} with discovery never enabled");
        let reply = call_in_region(&fleet.h, REGION, path, &json!({})).await;
        let listed = entries(&reply, key, case);
        assert!(
            listed.is_empty(),
            "case {case:?}: an account that did not enable discovery discovers nothing, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn both_lists_report_the_inventory_of_the_regions_the_settings_discover_now() {
    let fleet = Fleet::new();
    let image_id = fleet.publish_image(RHEL);
    let here = fleet.launch(REGION, &image_id, 1).await;
    let elsewhere = fleet.launch(OTHER_REGION, &image_id, 1).await;
    let both: Vec<String> = here.iter().chain(elsewhere.iter()).cloned().collect();
    let nothing: Vec<String> = Vec::new();

    // (the discovery switch the settings now carry, their `SourceRegions`, the instances
    // both lists must then report). The rows walk the switch back and forth and change the
    // Regions under each value, so every answer is read against settings the account
    // reached from the other value: what decides is the setting the account holds now, not
    // what some earlier pass found while it held another.
    let cases: &[(&str, &[&str], &Vec<String>)] = &[
        (ENABLED, &[REGION, OTHER_REGION], &both),
        (DISABLED, &[REGION, OTHER_REGION], &nothing),
        (ENABLED, &[REGION], &here),
        (DISABLED, &[REGION], &nothing),
        (DISABLED, &[OTHER_REGION], &nothing),
        (ENABLED, &[OTHER_REGION], &elsewhere),
        (DISABLED, &[REGION, OTHER_REGION], &nothing),
        (ENABLED, &[REGION, OTHER_REGION], &both),
    ];
    for (discovery, source_regions, expected) in cases {
        let case = &format!("{discovery} with SourceRegions {source_regions:?}");
        fleet.set_discovery(discovery, source_regions).await;

        let reply = fleet.instances(&json!({})).await;
        let mut listed = reported_instance_ids(&reply, case);
        listed.sort();
        let mut expected_ids = (*expected).clone();
        expected_ids.sort();
        assert_eq!(
            listed, expected_ids,
            "case {case:?}: expected the discovered instances {expected_ids:?}, got {listed:?}"
        );

        // The sibling list aggregates the same store, so it reports the subscription of
        // exactly those instances, counting the running ones.
        let (name, subscription_type) = RHEL.subscription.expect("the row carries a subscription");
        let expected_subscriptions: Vec<(String, String, i64)> = if expected_ids.is_empty() {
            Vec::new()
        } else {
            vec![(
                name.to_string(),
                subscription_type.to_string(),
                i64::try_from(expected_ids.len()).expect("a small instance count"),
            )]
        };
        let subscriptions = reported_subscriptions(&fleet.subscriptions(&json!({})).await, case);
        assert_eq!(
            subscriptions, expected_subscriptions,
            "case {case:?}: expected {expected_subscriptions:?}, got {subscriptions:?}"
        );
    }
}

#[tokio::test]
async fn a_fleet_that_changed_while_discovery_was_off_is_reported_as_it_is_now() {
    let fleet = Fleet::new();
    fleet.set_discovery(ENABLED, &[REGION]).await;
    let image_id = fleet.publish_image(RHEL);
    let mut ids = fleet.launch(REGION, &image_id, 2).await;
    ids.sort();

    let discovered = "the fleet of an enabled account";
    let mut listed = reported_instance_ids(&fleet.instances(&json!({})).await, discovered);
    listed.sort();
    assert_eq!(
        listed, ids,
        "case {discovered:?}: expected {ids:?}, got {listed:?}"
    );

    fleet.set_discovery(DISABLED, &[REGION]).await;
    for (path, key) in [(SUBSCRIPTIONS, "Subscriptions"), (INSTANCES, "Instances")] {
        let case = &format!("{path} after the account disabled discovery");
        let reply = call_in_region(&fleet.h, REGION, path, &json!({})).await;
        let listed = entries(&reply, key, case);
        assert!(
            listed.is_empty(),
            "case {case:?}: an account whose discovery is not enabled discovers nothing, got \
             {listed:?}"
        );
    }

    // The account's fleet changes while no pass runs, so nothing refreshed what an earlier
    // pass had found: an instance that is gone must not come back when discovery is
    // enabled again, because the next pass reports the inventory as it is then.
    fleet.terminate(REGION, &ids[0]).await;
    fleet.set_discovery(ENABLED, &[REGION]).await;

    let again = "the fleet after discovery was enabled again";
    let listed = reported_instance_ids(&fleet.instances(&json!({})).await, again);
    let expected = vec![ids[1].clone()];
    assert_eq!(
        listed, expected,
        "case {again:?}: expected {expected:?}, got {listed:?}"
    );
}
