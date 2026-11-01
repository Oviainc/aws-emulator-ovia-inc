// SPDX-License-Identifier: CC0-1.0

//! The `Filters` and the `MaxResults` of the two discovery lists select and page over the
//! records a discovery pass wrote from the account's EC2 inventory.
//!
//! The rule under test: each `Filter$Name` a discovery list documents addresses the member
//! of the discovered record carrying ONE fact about the account's inventory, compared
//! against the filter's `Values` the way its `Operator` says, and `MaxResults` pages
//! whatever that selection holds — so a filtered or a paged request reports a subset of
//! what the account really runs in the Regions its settings discover, and nothing a
//! discovered record does not hold can be reached through a filter.
//!
//! `tests/all/linux_subscriptions.rs` asserts the same filter vocabulary over records
//! placed in the store directly. What is asserted HERE is that the values those filters
//! match are the ones EC2 reported — the AMI an instance was launched from and that AMI's
//! billing facts (the usage operation it bills under, the Marketplace product code it
//! carries), the instance type the launch asked for, the Region the instance runs in, the
//! state EC2 keeps it in and the account EC2 says owns it — and that an instance no pass
//! records, because its AMI bills no Linux subscription, is out of reach of every filter.
//!
//! The rows span what the rule generalises over: every `Filter$Name` the model documents
//! for the instance list and the one it documents for the subscription list, each
//! `Operator` value and an absent one, a filter selecting one record, several, all of them
//! and none, a member no discovered record holds at all, several filters at once, and a
//! page size that truncates a filtered selection.
//!
//! The fleet, the AMIs and the two services on one engine come from
//! `tests/all/discovery_from_ec2.rs`, the suite that drives the pass against EC2 itself.

use serde_json::{json, Value};

use crate::discovery_from_ec2::{
    reported_instance_ids, reported_subscriptions, Ami, Fleet, AMIS, ENABLED, OTHER_REGION, RHEL,
    STATE_RUNNING, STATE_STOPPED,
};
use crate::harness::REGION;

/// The instance type a launch of this suite ASKS for
/// (`RunInstancesRequest$InstanceType`), so the `InstanceType` rows select by a type the
/// account named rather than by the one EC2 picks for a launch that names none.
const ASKED_INSTANCE_TYPE: &str = "t3.micro";

/// A `DualSubscription` value, the member nothing EC2 reports establishes
/// (`Instance$DualSubscription` is "indicates that you have two different license
/// subscriptions for the same software on your instance").
const A_DUAL_SUBSCRIPTION: &str = "Yes";

/// The AMI row of the fleet suite's table whose `Image$PlatformDetails` is
/// `platform_details`.
fn ami(platform_details: &str) -> &'static Ami {
    AMIS.iter()
        .find(|row| row.platform_details == platform_details)
        .unwrap_or_else(|| panic!("the fleet table publishes an AMI billing {platform_details:?}"))
}

/// One discovered fleet: the emulator it runs in, and each instance EC2 holds named by the
/// inventory fact a case selects it on.
struct Inventory {
    fleet: Fleet,
    /// The AMI billing the Red Hat Enterprise Linux subscription, which three instances of
    /// the fleet run.
    rhel_image: String,
    /// The AMI billing no Linux subscription at all, whose instance no pass records.
    unbilled_image: String,
    /// The Red Hat instance launched as [`ASKED_INSTANCE_TYPE`], running in `REGION`.
    typed: String,
    /// The Red Hat instance EC2 holds `stopped` in `REGION`.
    stopped: String,
    /// The Red Hat instance running in [`OTHER_REGION`].
    elsewhere: String,
    /// The SUSE instance, running in `REGION`.
    suse: String,
    /// The instance whose AMI is licensed through the Marketplace, running in `REGION`.
    marketplace: String,
    /// The instance whose AMI bills nothing, which discovery does not record.
    unbilled: String,
    /// The account EC2 says owns them (`Reservation$OwnerId`), read off the owner rather
    /// than written here.
    account: String,
}

impl Inventory {
    /// Every instance discovery records, in the instance-id order both lists report.
    fn discovered(&self) -> Vec<String> {
        sorted(vec![
            self.typed.clone(),
            self.stopped.clone(),
            self.elsewhere.clone(),
            self.suse.clone(),
            self.marketplace.clone(),
        ])
    }

    /// The discovered instances other than `instance_id`, in the same order.
    fn discovered_except(&self, instance_id: &str) -> Vec<String> {
        self.discovered()
            .into_iter()
            .filter(|held| held != instance_id)
            .collect()
    }
}

/// The fleet every case here selects over: instances of four AMIs, in two discovered
/// Regions, of two instance types, one of them stopped and one of them launched from an AMI
/// that bills no Linux subscription.
///
/// Everything is launched through EC2 itself, so each fact a filter matches below is one
/// EC2 reported rather than one this suite wrote into the store.
async fn inventory() -> Inventory {
    let fleet = Fleet::new();
    // Both Regions the fleet runs in are discovered, so a `Region` row partitions the
    // inventory rather than hiding half of it.
    fleet.set_discovery(ENABLED, &[REGION, OTHER_REGION]).await;

    let rhel_image = fleet.publish_image(RHEL);
    let suse_image = fleet.publish_image(ami("SUSE Linux"));
    let marketplace_image = fleet.publish_image(ami("RHEL with HA"));
    let unbilled_image = fleet.publish_image(ami("Linux/UNIX"));

    let typed = one(
        fleet
            .launch_as(REGION, &rhel_image, ASKED_INSTANCE_TYPE, 1)
            .await,
    );
    let stopped = one(fleet.launch(REGION, &rhel_image, 1).await);
    fleet.stop(REGION, &stopped).await;
    let elsewhere = one(fleet.launch(OTHER_REGION, &rhel_image, 1).await);
    let suse = one(fleet.launch(REGION, &suse_image, 1).await);
    let marketplace = one(fleet.launch(REGION, &marketplace_image, 1).await);
    let unbilled = one(fleet.launch(REGION, &unbilled_image, 1).await);
    let account = fleet.owner_of(REGION, &typed).await;

    Inventory {
        fleet,
        rhel_image,
        unbilled_image,
        typed,
        stopped,
        elsewhere,
        suse,
        marketplace,
        unbilled,
        account,
    }
}

/// The one id a launch of a single instance minted.
fn one(ids: Vec<String>) -> String {
    let mut ids = ids.into_iter();
    let id = ids.next().expect("a launch of one instance mints one id");
    assert!(
        ids.next().is_none(),
        "setup: a launch of one instance mints exactly one id"
    );
    id
}

/// `ids` in the instance-id order both discovery lists report their records in.
fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// One filter case over the discovered inventory: the `Filter$Name`, its `Operator`
/// (`None` is an absent one), its `Values`, and the instance ids it must select in the
/// order the list reports them.
type InstanceFilterCase = (
    &'static str,
    Option<&'static str>,
    Vec<String>,
    Vec<String>,
);

/// One `Filter`. An `operator` of `None` leaves `Operator` out, which is the equality an
/// absent one means.
fn filter(name: &str, operator: Option<&str>, values: &[String]) -> Value {
    let mut filter = json!({ "Name": name, "Values": values });
    if let Some(operator) = operator {
        filter["Operator"] = json!(operator);
    }
    filter
}

/// The `(Subscription$Name, Subscription$Type, InstanceCount)` the instances of the AMI
/// `row` aggregate to when `running` of them are running.
fn subscription(row: &Ami, running: i64) -> (String, String, i64) {
    let (name, subscription_type) = row
        .subscription
        .expect("the row bills a Linux subscription");
    (name.to_string(), subscription_type.to_string(), running)
}

/// The entries of `full` whose `Subscription$Name` is one of `names`, in the list's order —
/// how a row states which subscriptions it expects without restating their type or count.
fn rows(full: &[(String, String, i64)], names: &[&str]) -> Vec<(String, String, i64)> {
    full.iter()
        .filter(|(name, _, _)| names.contains(&name.as_str()))
        .cloned()
        .collect()
}

#[tokio::test]
async fn every_documented_filter_name_selects_on_the_fact_ec2_reported() {
    let inventory = inventory().await;
    let discovered = inventory.discovered();
    let nothing: Vec<String> = Vec::new();
    let red_hat = sorted(vec![
        inventory.typed.clone(),
        inventory.stopped.clone(),
        inventory.elsewhere.clone(),
    ]);
    let marketplace_code = ami("RHEL with HA")
        .product_code
        .expect("the Marketplace row carries a product code")
        .to_string();
    let suse_usage_operation = ami("SUSE Linux").usage_operation.to_string();

    // (the `Filter$Name`, its `Operator` — `None` is an absent one, its `Values`, the
    // instance ids it must select in the list's order). Every `Name` the model documents
    // for `ListLinuxSubscriptionInstancesRequest$Filters` appears, matched against the fact
    // EC2 reported for the instance.
    let cases: Vec<InstanceFilterCase> = vec![
        (
            "AccountID",
            Some("Equal"),
            vec![inventory.account.clone()],
            discovered.clone(),
        ),
        (
            "AccountID",
            Some("NotEqual"),
            vec![inventory.account.clone()],
            nothing.clone(),
        ),
        (
            "AmiID",
            Some("Equal"),
            vec![inventory.rhel_image.clone()],
            red_hat.clone(),
        ),
        (
            "AmiID",
            Some("NotEqual"),
            vec![inventory.rhel_image.clone()],
            sorted(vec![inventory.suse.clone(), inventory.marketplace.clone()]),
        ),
        // The instance whose AMI bills no Linux subscription is not in the store at all, so
        // neither its AMI nor its own id can reach it.
        (
            "AmiID",
            Some("Equal"),
            vec![inventory.unbilled_image.clone()],
            nothing.clone(),
        ),
        (
            "InstanceID",
            Some("Equal"),
            vec![inventory.unbilled.clone()],
            nothing.clone(),
        ),
        (
            "InstanceID",
            Some("Equal"),
            vec![inventory.suse.clone()],
            vec![inventory.suse.clone()],
        ),
        (
            "InstanceID",
            None,
            vec![inventory.suse.clone()],
            vec![inventory.suse.clone()],
        ),
        (
            "InstanceID",
            Some("NotEqual"),
            vec![inventory.suse.clone()],
            inventory.discovered_except(&inventory.suse),
        ),
        (
            "InstanceType",
            Some("Equal"),
            vec![ASKED_INSTANCE_TYPE.to_string()],
            vec![inventory.typed.clone()],
        ),
        (
            "InstanceType",
            Some("Contains"),
            vec!["t3.".to_string()],
            vec![inventory.typed.clone()],
        ),
        (
            "InstanceType",
            Some("NotEqual"),
            vec![ASKED_INSTANCE_TYPE.to_string()],
            inventory.discovered_except(&inventory.typed),
        ),
        (
            "ProductCode",
            Some("Equal"),
            vec![marketplace_code.clone()],
            vec![inventory.marketplace.clone()],
        ),
        // `Instance$ProductCode` is a LIST member, and only the AMI bought through the
        // Marketplace reported one: an instance holding no value for the member differs
        // from nothing, so the negated filter selects neither it nor the record that
        // matches.
        (
            "ProductCode",
            Some("NotEqual"),
            vec![marketplace_code],
            nothing.clone(),
        ),
        (
            "Region",
            Some("Equal"),
            vec![OTHER_REGION.to_string()],
            vec![inventory.elsewhere.clone()],
        ),
        (
            "Region",
            Some("NotEqual"),
            vec![OTHER_REGION.to_string()],
            inventory.discovered_except(&inventory.elsewhere),
        ),
        (
            "Region",
            Some("Contains"),
            vec!["us-".to_string()],
            discovered.clone(),
        ),
        (
            "Status",
            Some("Equal"),
            vec![STATE_RUNNING.to_string()],
            inventory.discovered_except(&inventory.stopped),
        ),
        (
            "Status",
            Some("Equal"),
            vec![STATE_STOPPED.to_string()],
            vec![inventory.stopped.clone()],
        ),
        (
            "Status",
            Some("NotEqual"),
            vec![STATE_RUNNING.to_string()],
            vec![inventory.stopped.clone()],
        ),
        (
            "UsageOperation",
            Some("Equal"),
            vec![suse_usage_operation],
            vec![inventory.suse.clone()],
        ),
        // Nothing EC2 reports establishes a dual subscription, so no discovered record
        // holds the member and no value of the filter can select one — the negated filter
        // included.
        (
            "DualSubscription",
            Some("Equal"),
            vec![A_DUAL_SUBSCRIPTION.to_string()],
            nothing.clone(),
        ),
        (
            "DualSubscription",
            Some("NotEqual"),
            vec![A_DUAL_SUBSCRIPTION.to_string()],
            nothing.clone(),
        ),
    ];

    for (name, operator, values, expected) in cases {
        let case = &format!("{name} {operator:?} {values:?}");
        let reply = inventory
            .fleet
            .instances(&json!({ "Filters": [filter(name, operator, &values)] }))
            .await;
        let listed = reported_instance_ids(&reply, case);
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // Several filters select what satisfies ALL of them, over that same real inventory.
    let case = "the running instances of the Red Hat AMI";
    let reply = inventory
        .fleet
        .instances(&json!({ "Filters": [
            filter("AmiID", Some("Equal"), std::slice::from_ref(&inventory.rhel_image)),
            filter("Status", Some("Equal"), &[STATE_RUNNING.to_string()]),
        ] }))
        .await;
    let listed = reported_instance_ids(&reply, case);
    let expected = sorted(vec![inventory.typed.clone(), inventory.elsewhere.clone()]);
    assert_eq!(
        listed, expected,
        "case {case:?}: expected {expected:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_subscription_filter_selects_over_what_the_discovered_fleet_aggregates_to() {
    let inventory = inventory().await;

    // What the fleet aggregates to: one `Subscription` per distinct (name, type) pair the
    // discovered instances carry, counting the RUNNING ones — `Subscription$InstanceCount`
    // is "the total amount of running instances using this subscription", so the stopped
    // Red Hat instance is discovered without being counted.
    let mut full: Vec<(String, String, i64)> = vec![
        subscription(RHEL, 2),
        subscription(ami("SUSE Linux"), 1),
        subscription(ami("RHEL with HA"), 1),
    ];
    // The list reports its subscriptions in (name, type) order.
    full.sort();

    let unfiltered = "every discovered subscription";
    let actual = reported_subscriptions(
        &inventory.fleet.subscriptions(&json!({})).await,
        unfiltered,
    );
    assert_eq!(
        actual, full,
        "case {unfiltered:?}: expected {full:?}, got {actual:?}"
    );

    let red_hat = subscription(RHEL, 2).0;
    let suse = subscription(ami("SUSE Linux"), 1).0;
    let high_availability = subscription(ami("RHEL with HA"), 1).0;

    // (the `Operator` — `None` is an absent one, the `Values`, and the subscriptions of the
    // fleet the filter must select, by name). `Subscription` is the only `Name` this list
    // documents and it addresses the subscription's name.
    let cases: Vec<(Option<&str>, Vec<String>, Vec<&str>)> = vec![
        (Some("Equal"), vec![suse.clone()], vec![suse.as_str()]),
        (None, vec![suse.clone()], vec![suse.as_str()]),
        (
            Some("Equal"),
            vec![suse.clone(), high_availability.clone()],
            vec![suse.as_str(), high_availability.as_str()],
        ),
        // The Red Hat subscription's name carries those words; the high-availability one's
        // does not, so a partial match is not the same selection as equality.
        (
            Some("Contains"),
            vec!["Red Hat".to_string()],
            vec![red_hat.as_str()],
        ),
        (
            Some("Contains"),
            vec!["RHEL".to_string(), "SUSE".to_string()],
            vec![high_availability.as_str(), suse.as_str()],
        ),
        (
            Some("NotEqual"),
            vec![red_hat.clone()],
            vec![suse.as_str(), high_availability.as_str()],
        ),
        (
            Some("Equal"),
            vec!["no-such-subscription".to_string()],
            Vec::new(),
        ),
        (
            Some("NotEqual"),
            vec!["no-such-subscription".to_string()],
            vec![red_hat.as_str(), suse.as_str(), high_availability.as_str()],
        ),
    ];

    for (operator, values, names) in cases {
        let case = &format!("Subscription {operator:?} {values:?}");
        let reply = inventory
            .fleet
            .subscriptions(&json!({ "Filters": [filter("Subscription", operator, &values)] }))
            .await;
        let actual = reported_subscriptions(&reply, case);
        let expected = rows(&full, &names);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_paged_walk_of_a_filtered_discovery_list_covers_that_selection_once() {
    let inventory = inventory().await;
    let expected = sorted(vec![
        inventory.typed.clone(),
        inventory.stopped.clone(),
        inventory.elsewhere.clone(),
    ]);

    // One record per page, over the selection the filter makes rather than over the whole
    // inventory: the walk covers exactly the instances of the Red Hat AMI, once each, in
    // the list's order.
    let case = "a one-at-a-time walk of the Red Hat instances";
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=expected.len() {
        let mut body = json!({
            "MaxResults": 1,
            "Filters": [
                filter("AmiID", Some("Equal"), std::slice::from_ref(&inventory.rhel_image)),
            ],
        });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = inventory.fleet.instances(&body).await;
        let listed = reported_instance_ids(&reply, case);
        assert!(
            listed.len() <= 1,
            "case {case:?}: page {page} must hold at most the one record MaxResults asked \
             for, got {listed:?}"
        );
        walked.extend(listed);
        token = reply
            .ok(case)
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case:?}: the walk must end without a NextToken, got {token:?}"
    );
    assert_eq!(
        walked, expected,
        "case {case:?}: expected {expected:?}, got {walked:?}"
    );
}
