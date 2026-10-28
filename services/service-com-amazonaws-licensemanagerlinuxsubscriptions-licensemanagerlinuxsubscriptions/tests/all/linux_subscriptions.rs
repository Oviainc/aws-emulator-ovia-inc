// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two discovery lists — `ListLinuxSubscriptions` and
//! `ListLinuxSubscriptionInstances` — driven over the real wire path: every call goes
//! through the shared harness's `dispatch`, so the request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be, and this
//! suite states no second copy of what an SDK client's request looks like on the wire.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * both lists answer from the single shared store of discovered instances: an account
//!   that has discovered nothing reports nothing, and what is in the store is what both
//!   lists report;
//! * `ListLinuxSubscriptions` reports one `Subscription` per distinct (name, type) pair
//!   the discovered instances carry, with `InstanceCount` counting the instances using it
//!   that are running;
//! * a `Filter` selects on the member its `Name` addresses, with the comparison its
//!   `Operator` names (`Equal`, `NotEqual`, `Contains`, and equality when absent), and
//!   several filters select what satisfies all of them;
//! * a filter compares against the values the addressed member HOLDS, so an instance that
//!   holds none of them — an unset scalar member, an empty list member — satisfies no
//!   filter whatever its operator, and `Equal`/`NotEqual` partition exactly the instances
//!   that do hold the member;
//! * `MaxResults`/`NextToken` page each list exactly once over every element, with no
//!   token on the last page, and a cursor minted for one selection is not honoured for
//!   another;
//! * a filter a list cannot apply — a `Name` it does not document, or one carrying no
//!   value — is refused with `ValidationException` (HTTP 400) rather than silently
//!   answering an empty page.

use serde_json::{json, Value};

use crate::harness::{call, enable_discovery, harness, seed_discovered_instance, Harness, Reply};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::DiscoveredInstanceRecord;

/// `POST /subscription/ListLinuxSubscriptions` (`OPERATIONS.md`).
const SUBSCRIPTIONS: &str = "/subscription/ListLinuxSubscriptions";
/// `POST /subscription/ListLinuxSubscriptionInstances` (`OPERATIONS.md`).
const INSTANCES: &str = "/subscription/ListLinuxSubscriptionInstances";

/// The three ways a subscription can be licensed, from the model's documentation for
/// `Subscription$Type`: "subscription-included with Amazon EC2, Bring Your Own
/// Subscription model (BYOS), or from the Amazon Web Services Marketplace".
const TYPE_INCLUDED: &str = "Subscription-included (Amazon EC2)";
const TYPE_BYOS: &str = "Bring Your Own Subscription model (BYOS)";
const TYPE_MARKETPLACE: &str = "Amazon Web Services Marketplace";

/// One discovered instance for the fixture store: instance id, account, AMI, instance
/// type, status, Region, usage operation, product codes, `DualSubscription` (empty =
/// unset), subscription name, subscription type.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
    &'static str,
);

/// The discovered instances every case below is asserted against: a table spanning what
/// the rules generalise over — several subscriptions, ONE subscription name licensed two
/// different ways, several instance states (so the running count is not just the record
/// count), several Regions, accounts, AMIs and instance types, an instance carrying two
/// product codes and one carrying none, and `DualSubscription` set on some records and
/// unset on others.
const DISCOVERED: &[Row] = &[
    (
        "i-0a1",
        "000000000000",
        "ami-0001",
        "t3.micro",
        "running",
        "us-west-2",
        "RunInstances:0010",
        &["pc-rhel"],
        "No",
        "RHEL",
        TYPE_INCLUDED,
    ),
    (
        "i-0b2",
        "000000000000",
        "ami-0001",
        "t3.small",
        "running",
        "us-west-1",
        "RunInstances:0010",
        &["pc-rhel"],
        "",
        "RHEL",
        TYPE_INCLUDED,
    ),
    (
        "i-0c3",
        "000000000000",
        "ami-0001",
        "m5.large",
        "stopped",
        "us-west-2",
        "RunInstances:0010",
        &["pc-rhel"],
        "",
        "RHEL",
        TYPE_INCLUDED,
    ),
    (
        "i-0d4",
        "111111111111",
        "ami-0002",
        "t3.micro",
        "running",
        "us-east-1",
        "RunInstances",
        &[],
        "Yes",
        "RHEL",
        TYPE_BYOS,
    ),
    (
        "i-0e5",
        "000000000000",
        "ami-0003",
        "c6g.medium",
        "running",
        "eu-west-1",
        "RunInstances:000g",
        &["pc-suse", "pc-extra"],
        "",
        "SUSE Linux Enterprise Server",
        TYPE_INCLUDED,
    ),
    (
        "i-0f6",
        "222222222222",
        "ami-0004",
        "t4g.nano",
        "terminated",
        "ap-south-1",
        "RunInstances:0g00",
        &["pc-ubuntu"],
        "",
        "Ubuntu Pro",
        TYPE_MARKETPLACE,
    ),
];

/// What `DISCOVERED` aggregates to: one subscription per distinct (name, type) pair, in
/// (name, type) order, counting only the RUNNING instances that use it. `Ubuntu Pro` was
/// discovered but its only instance is terminated, so it reports a count of zero.
const EXPECTED_SUBSCRIPTIONS: &[(&str, &str, i64)] = &[
    ("RHEL", TYPE_BYOS, 1),
    ("RHEL", TYPE_INCLUDED, 2),
    ("SUSE Linux Enterprise Server", TYPE_INCLUDED, 1),
    ("Ubuntu Pro", TYPE_MARKETPLACE, 0),
];

/// Put `DISCOVERED` in the store directly, through the shared harness's seeding seam:
/// discovery reads the account's EC2 inventory, which no operation of THIS service writes,
/// so the store is pre-filled to exercise the projection the two list operations perform
/// over it.
///
/// The account's settings are the ones that make those records discovered data — discovery
/// enabled for exactly the Regions the rows were found in — because what the two lists
/// report is the inventory of the Regions the settings discover NOW.
async fn discover(h: &Harness, rows: &[Row]) {
    let mut regions: Vec<&str> = rows.iter().map(|row| row.5).collect();
    regions.sort_unstable();
    regions.dedup();
    enable_discovery(h, &regions).await;
    for row in rows {
        let (
            instance_id,
            account_id,
            ami_id,
            instance_type,
            status,
            region,
            usage_operation,
            product_codes,
            dual_subscription,
            subscription_name,
            subscription_type,
        ) = *row;
        seed_discovered_instance(
            h,
            DiscoveredInstanceRecord {
                instance_id: instance_id.to_string(),
                account_id: account_id.to_string(),
                ami_id: ami_id.to_string(),
                instance_type: instance_type.to_string(),
                status: status.to_string(),
                region: region.to_string(),
                usage_operation: usage_operation.to_string(),
                product_codes: product_codes.iter().map(|code| code.to_string()).collect(),
                subscription_name: subscription_name.to_string(),
                subscription_type: subscription_type.to_string(),
                dual_subscription: Some(dual_subscription.to_string())
                    .filter(|value| !value.is_empty()),
                ..DiscoveredInstanceRecord::default()
            },
        );
    }
}

/// A member of a list entry, as a string.
fn member(entry: &Value, name: &str, case: &str) -> String {
    entry
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
        .to_string()
}

/// The entries a list body reports under `key`.
fn entries<'a>(reply: &'a Reply, key: &str, case: &str) -> &'a Vec<Value> {
    reply
        .ok(case)
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no {key} in {:?}", reply.body))
}

/// The subscriptions a `ListLinuxSubscriptions` body reports, as (name, type, count).
fn subscriptions(reply: &Reply, case: &str) -> Vec<(String, String, i64)> {
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

/// The `InstanceID`s a `ListLinuxSubscriptionInstances` body reports.
fn instance_ids(reply: &Reply, case: &str) -> Vec<String> {
    entries(reply, "Instances", case)
        .iter()
        .map(|entry| member(entry, "InstanceID", case))
        .collect()
}

/// The `NextToken` of a 200 body, if it carries one.
fn next_token(reply: &Reply, case: &str) -> Option<String> {
    reply
        .ok(case)
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn an_account_that_discovered_nothing_reports_nothing() {
    let h = harness();
    for (path, key) in [(SUBSCRIPTIONS, "Subscriptions"), (INSTANCES, "Instances")] {
        let case = &format!("{path} before any discovery");
        let reply = call(&h, path, &json!({})).await;
        assert!(
            entries(&reply, key, case).is_empty(),
            "case {case:?}: expected an empty {key}, got {:?}",
            reply.body
        );
        assert_eq!(
            next_token(&reply, case),
            None,
            "case {case:?}: a complete page carries no NextToken, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn subscriptions_aggregate_the_discovered_instances() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    let case = "every discovered subscription";
    let reply = call(&h, SUBSCRIPTIONS, &json!({})).await;
    let actual = subscriptions(&reply, case);
    let expected: Vec<(String, String, i64)> = EXPECTED_SUBSCRIPTIONS
        .iter()
        .map(|(name, kind, count)| (name.to_string(), kind.to_string(), *count))
        .collect();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );

    // The same store answers the instance list, so the two cannot disagree: every
    // discovered record is reported, and the counts above are exactly its running rows.
    let instance_case = "every discovered instance";
    let reply = call(&h, INSTANCES, &json!({})).await;
    let listed = instance_ids(&reply, instance_case);
    let mut expected_ids: Vec<String> = DISCOVERED.iter().map(|row| row.0.to_string()).collect();
    expected_ids.sort();
    assert_eq!(
        listed, expected_ids,
        "case {instance_case:?}: expected {expected_ids:?}, got {listed:?}"
    );
    let running = DISCOVERED.iter().filter(|row| row.4 == "running").count() as i64;
    let counted: i64 = actual.iter().map(|(_, _, count)| count).sum();
    assert_eq!(
        counted, running,
        "case {instance_case:?}: the subscription counts ({counted}) must add up to the \
         running discovered instances ({running})"
    );
}

#[tokio::test]
async fn every_member_of_a_discovered_instance_is_reported_from_state() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    let case = "instance members";
    let reply = call(&h, INSTANCES, &json!({})).await;
    let listed = entries(&reply, "Instances", case).clone();
    for row in DISCOVERED {
        let (
            instance_id,
            account_id,
            ami_id,
            instance_type,
            status,
            region,
            usage_operation,
            product_codes,
            dual_subscription,
            subscription_name,
            subscription_type,
        ) = *row;
        let _ = subscription_type;
        let entry = listed
            .iter()
            .find(|entry| member(entry, "InstanceID", case) == instance_id)
            .unwrap_or_else(|| panic!("case {case:?}: {instance_id} is missing from {listed:?}"));
        for (name, expected) in [
            ("AccountID", account_id),
            ("AmiId", ami_id),
            ("InstanceType", instance_type),
            ("Status", status),
            ("Region", region),
            ("UsageOperation", usage_operation),
            ("SubscriptionName", subscription_name),
        ] {
            let actual = member(entry, name, case);
            assert_eq!(
                actual, expected,
                "case {case:?}: {instance_id} {name} expected {expected:?}, got {actual:?}"
            );
        }
        let codes: Vec<String> = entry
            .get("ProductCode")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no ProductCode in {entry:?}"))
            .iter()
            .map(|code| code.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            codes, product_codes,
            "case {case:?}: {instance_id} ProductCode expected {product_codes:?}, got {codes:?}"
        );
        let dual = entry.get("DualSubscription").and_then(Value::as_str);
        let expected_dual = Some(dual_subscription).filter(|value| !value.is_empty());
        assert_eq!(
            dual, expected_dual,
            "case {case:?}: {instance_id} DualSubscription expected {expected_dual:?}, got {dual:?}"
        );
    }
}

#[tokio::test]
async fn a_subscription_filter_selects_along_its_operator() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    // (operator — `None` is an absent `Operator`, values, the subscription names expected
    // in (name, type) order). `Subscription` is the only `Name` this list documents, and
    // it addresses the subscription's name.
    let cases: &[(Option<&str>, &[&str], &[&str])] = &[
        (None, &["RHEL"], &["RHEL", "RHEL"]),
        (Some("Equal"), &["RHEL"], &["RHEL", "RHEL"]),
        (
            Some("Equal"),
            &["RHEL", "Ubuntu Pro"],
            &["RHEL", "RHEL", "Ubuntu Pro"],
        ),
        (Some("Equal"), &["rhel"], &[]),
        (Some("Equal"), &["RH"], &[]),
        (
            Some("Contains"),
            &["Linux"],
            &["SUSE Linux Enterprise Server"],
        ),
        (
            Some("Contains"),
            &["RH", "SUSE"],
            &["RHEL", "RHEL", "SUSE Linux Enterprise Server"],
        ),
        (
            Some("NotEqual"),
            &["RHEL"],
            &["SUSE Linux Enterprise Server", "Ubuntu Pro"],
        ),
        (
            Some("NotEqual"),
            &["nothing-like-this"],
            &["RHEL", "RHEL", "SUSE Linux Enterprise Server", "Ubuntu Pro"],
        ),
    ];

    for (operator, values, expected) in cases {
        let case = &format!("Subscription {operator:?} {values:?}");
        let mut filter = json!({ "Name": "Subscription", "Values": values });
        if let Some(operator) = operator {
            filter["Operator"] = json!(operator);
        }
        let reply = call(&h, SUBSCRIPTIONS, &json!({ "Filters": [filter] })).await;
        let names: Vec<String> = subscriptions(&reply, case)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(
            names, *expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn an_instance_filter_selects_on_the_member_it_names() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    // Every `Filter$Name` the model documents for this list, with the instance ids it
    // selects (in instance-id order).
    let cases: &[(&str, &str, &[&str], &[&str])] = &[
        ("AccountID", "Equal", &["111111111111"], &["i-0d4"]),
        ("AmiID", "Equal", &["ami-0003"], &["i-0e5"]),
        ("DualSubscription", "Equal", &["Yes"], &["i-0d4"]),
        ("DualSubscription", "Equal", &["No"], &["i-0a1"]),
        ("InstanceID", "Equal", &["i-0c3"], &["i-0c3"]),
        (
            "InstanceType",
            "Contains",
            &["t3."],
            &["i-0a1", "i-0b2", "i-0d4"],
        ),
        ("ProductCode", "Equal", &["pc-extra"], &["i-0e5"]),
        (
            "Region",
            "Contains",
            &["us-west"],
            &["i-0a1", "i-0b2", "i-0c3"],
        ),
        ("Status", "NotEqual", &["running"], &["i-0c3", "i-0f6"]),
        (
            "UsageOperation",
            "Equal",
            &["RunInstances:0010"],
            &["i-0a1", "i-0b2", "i-0c3"],
        ),
    ];

    for (name, operator, values, expected) in cases {
        let case = &format!("{name} {operator} {values:?}");
        let reply = call(
            &h,
            INSTANCES,
            &json!({ "Filters": [{ "Name": name, "Values": values, "Operator": operator }] }),
        )
        .await;
        let listed = instance_ids(&reply, case);
        assert_eq!(
            listed, *expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // Several filters select what satisfies ALL of them.
    let case = "Region us-west AND Status running";
    let reply = call(
        &h,
        INSTANCES,
        &json!({ "Filters": [
            { "Name": "Region", "Values": ["us-west"], "Operator": "Contains" },
            { "Name": "Status", "Values": ["running"], "Operator": "Equal" },
        ] }),
    )
    .await;
    let listed = instance_ids(&reply, case);
    assert_eq!(
        listed,
        vec!["i-0a1".to_string(), "i-0b2".to_string()],
        "case {case:?}: expected the running us-west instances, got {listed:?}"
    );
}

/// Every `Filter$Name` this list documents, with the `Instance` member it addresses.
///
/// The names are the model's documentation for
/// `ListLinuxSubscriptionInstancesRequest$Filters` ("you can specify one of the following
/// values for the `Name` key"); the member each addresses is the `Instance` member of that
/// name — note that the `AmiID` filter addresses `Instance$AmiId`, and that `ProductCode`
/// addresses a LIST member (`Instance$ProductCode`), so a record can hold several values
/// for it or none.
const FILTERED_MEMBERS: &[(&str, &str)] = &[
    ("AccountID", "AccountID"),
    ("AmiID", "AmiId"),
    ("DualSubscription", "DualSubscription"),
    ("InstanceID", "InstanceID"),
    ("InstanceType", "InstanceType"),
    ("ProductCode", "ProductCode"),
    ("Region", "Region"),
    ("Status", "Status"),
    ("UsageOperation", "UsageOperation"),
];

/// One filter case: the `Filter$Name`, its `Operator` (`None` is an absent one), its
/// `Values`, and the instance ids the filter must select, in instance-id order.
type FilterCase = (
    &'static str,
    Option<&'static str>,
    &'static [&'static str],
    &'static [&'static str],
);

/// Build one filtered `ListLinuxSubscriptionInstances` request body. `operator` of `None`
/// leaves `Operator` out, which is the equality an absent operator means.
fn instance_filter(name: &str, operator: Option<&str>, values: &[&str]) -> Value {
    let mut filter = json!({ "Name": name, "Values": values });
    if let Some(operator) = operator {
        filter["Operator"] = json!(operator);
    }
    json!({ "Filters": [filter] })
}

/// What the entry for one instance holds for the `Instance` member `name`: the string it
/// carries, every element of the list member `ProductCode`, and nothing at all for a
/// member the entry does not report.
fn held_values(entry: &Value, name: &str) -> Vec<String> {
    match entry.get(name) {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn a_filter_cannot_select_an_instance_whose_member_holds_nothing() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    // (the filter's Name, its Operator — `None` is an absent one, its Values, the
    // instance ids it selects in instance-id order). Only the records that HOLD a value
    // for the addressed member can be selected, whatever the operator: `i-0d4` is the
    // only record carrying a `DualSubscription` of "Yes" and the only one whose AMI
    // reported no product code at all, while `i-0b2`, `i-0c3`, `i-0e5` and `i-0f6` hold
    // no `DualSubscription`.
    let cases: &[FilterCase] = &[
        // A record holding nothing for the member is out of every answer, including the
        // negated one — the defect this pins: it used to be in all of them.
        ("DualSubscription", Some("NotEqual"), &["Yes"], &["i-0a1"]),
        ("DualSubscription", Some("NotEqual"), &["No"], &["i-0d4"]),
        ("DualSubscription", Some("NotEqual"), &["Yes", "No"], &[]),
        (
            "DualSubscription",
            Some("NotEqual"),
            &["neither-value"],
            &["i-0a1", "i-0d4"],
        ),
        ("DualSubscription", Some("Contains"), &["o"], &["i-0a1"]),
        ("DualSubscription", Some("Equal"), &["No"], &["i-0a1"]),
        ("DualSubscription", None, &["No"], &["i-0a1"]),
        // The same for the list member: an instance whose AMI reported no product code
        // holds no value, so it differs from nothing.
        (
            "ProductCode",
            Some("NotEqual"),
            &["pc-rhel"],
            &["i-0e5", "i-0f6"],
        ),
        (
            "ProductCode",
            Some("NotEqual"),
            &["no-such-code"],
            &["i-0a1", "i-0b2", "i-0c3", "i-0e5", "i-0f6"],
        ),
        (
            "ProductCode",
            Some("Contains"),
            &["pc-"],
            &["i-0a1", "i-0b2", "i-0c3", "i-0e5", "i-0f6"],
        ),
        (
            "ProductCode",
            Some("Equal"),
            &["pc-rhel"],
            &["i-0a1", "i-0b2", "i-0c3"],
        ),
        // A member every record holds is unaffected: a negated filter there still selects
        // every record whose value differs.
        (
            "Status",
            Some("NotEqual"),
            &["running"],
            &["i-0c3", "i-0f6"],
        ),
        (
            "Region",
            Some("NotEqual"),
            &["us-west-2"],
            &["i-0b2", "i-0d4", "i-0e5", "i-0f6"],
        ),
    ];

    for (name, operator, values, expected) in cases {
        let case = &format!("{name} {operator:?} {values:?}");
        let reply = call(&h, INSTANCES, &instance_filter(name, *operator, values)).await;
        let listed = instance_ids(&reply, case);
        assert_eq!(
            listed, *expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
        // Whatever an answer holds was selected on a value the client can see in it.
        for entry in entries(&reply, "Instances", case) {
            let held = held_values(entry, member_of(name));
            assert!(
                !held.is_empty(),
                "case {case:?}: {entry:?} was selected on a member it reports no value for"
            );
        }
    }
}

/// The `Instance` member the filter `name` addresses, from [`FILTERED_MEMBERS`].
fn member_of(name: &str) -> &'static str {
    FILTERED_MEMBERS
        .iter()
        .find(|(filter_name, _)| *filter_name == name)
        .map(|(_, member)| *member)
        .unwrap_or_else(|| panic!("{name} is a documented Filter$Name"))
}

#[tokio::test]
async fn equal_and_not_equal_partition_the_instances_that_hold_the_member() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    let unfiltered = call(&h, INSTANCES, &json!({})).await;
    let all = entries(&unfiltered, "Instances", "every discovered instance").clone();

    // Asserted for EVERY documented filter name, not only the two whose members a record
    // can leave unset: for one set of values, the instances `Equal` selects and the ones
    // `NotEqual` selects are disjoint, and together they are exactly the instances that
    // hold a value for the addressed member.
    for (name, member_name) in FILTERED_MEMBERS {
        let holders: Vec<String> = all
            .iter()
            .filter(|entry| !held_values(entry, member_name).is_empty())
            .map(|entry| member(entry, "InstanceID", "holders"))
            .collect();

        // One value some instance holds, and one no instance holds: the two sides of the
        // partition are non-trivial for the first and the whole of it for the second.
        let held = all
            .iter()
            .find_map(|entry| held_values(entry, member_name).into_iter().next())
            .unwrap_or_else(|| panic!("{member_name} is held by some discovered instance"));
        for values in [vec![held.as_str()], vec!["no-instance-holds-this-value"]] {
            let case = &format!("{name} partition on {values:?}");
            let mut equal = instance_ids(
                &call(&h, INSTANCES, &instance_filter(name, Some("Equal"), &values)).await,
                case,
            );
            let not_equal = instance_ids(
                &call(
                    &h,
                    INSTANCES,
                    &instance_filter(name, Some("NotEqual"), &values),
                )
                .await,
                case,
            );
            let overlap: Vec<&String> = equal.iter().filter(|id| not_equal.contains(id)).collect();
            assert!(
                overlap.is_empty(),
                "case {case:?}: {overlap:?} are selected by both Equal and NotEqual"
            );
            equal.extend(not_equal);
            equal.sort();
            assert_eq!(
                equal, holders,
                "case {case:?}: Equal and NotEqual together must select exactly the \
                 instances holding {member_name}, expected {holders:?}, got {equal:?}"
            );
        }
    }
}

#[tokio::test]
async fn max_results_pages_each_list_exactly_once() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    for (path, key, total) in [
        (SUBSCRIPTIONS, "Subscriptions", EXPECTED_SUBSCRIPTIONS.len()),
        (INSTANCES, "Instances", DISCOVERED.len()),
    ] {
        // One page of everything is the reference the paged walks must reproduce.
        let case = &format!("{path} unpaged");
        let reply = call(&h, path, &json!({})).await;
        let whole = entries(&reply, key, case).clone();
        assert_eq!(
            whole.len(),
            total,
            "case {case:?}: expected {total} entries, got {:?}",
            reply.body
        );

        for page_size in 1..=total + 1 {
            let case = &format!("{path} MaxResults={page_size}");
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut body = json!({ "MaxResults": page_size });
                if let Some(token) = &token {
                    body["NextToken"] = json!(token);
                }
                let reply = call(&h, path, &body).await;
                let page = entries(&reply, key, case).clone();
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: page {pages} holds {} entries, more than MaxResults",
                    page.len()
                );
                walked.extend(page);
                token = next_token(&reply, case);
                pages += 1;
                assert!(
                    pages <= total + 1,
                    "case {case:?}: the walk did not terminate after {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                walked, whole,
                "case {case:?}: the paged walk must visit every entry exactly once, in order"
            );
            let expected_pages = total.div_ceil(page_size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, took {pages}"
            );
        }

        // A page size below 1 makes no progress, and a cursor that is not one this list
        // minted is not a client-chosen value: both are refused.
        for (label, body) in [
            ("MaxResults=0", json!({ "MaxResults": 0 })),
            ("NextToken=not-a-token", json!({ "NextToken": "zzz" })),
        ] {
            let case = &format!("{path} {label}");
            call(&h, path, &body).await.error(case, 400, "ValidationException");
        }
    }

    // A cursor is a position in ONE list: the one the filter selection produced. The
    // token minted by a filtered first page is not honoured for a different selection.
    let case = "a cursor from another selection";
    let first = call(
        &h,
        INSTANCES,
        &json!({ "MaxResults": 1, "Filters": [{ "Name": "Region", "Values": ["us-west"], "Operator": "Contains" }] }),
    )
    .await;
    let token = next_token(&first, case).unwrap_or_else(|| {
        panic!("case {case:?}: expected a NextToken on a truncated page, got {first:?}")
    });
    call(
        &h,
        INSTANCES,
        &json!({ "MaxResults": 1, "NextToken": token, "Filters": [{ "Name": "Status", "Values": ["running"], "Operator": "Equal" }] }),
    )
    .await
    .error(case, 400, "ValidationException");
}

#[tokio::test]
async fn a_filter_a_list_cannot_apply_is_refused() {
    let h = harness();
    discover(&h, DISCOVERED).await;

    // (the list, the filter, why it cannot be applied).
    let cases: &[(&str, Value, &str)] = &[
        (
            SUBSCRIPTIONS,
            json!({ "Name": "Region", "Values": ["us-west-2"] }),
            "Region is documented for the instance list, not for this one",
        ),
        (
            SUBSCRIPTIONS,
            json!({ "Values": ["RHEL"] }),
            "a filter with no Name addresses no member",
        ),
        (
            SUBSCRIPTIONS,
            json!({ "Name": "Subscription" }),
            "a filter with no Values has nothing to compare",
        ),
        (
            SUBSCRIPTIONS,
            json!({ "Name": "subscription", "Values": ["RHEL"] }),
            "the documented Name is case-sensitive",
        ),
        (
            INSTANCES,
            json!({ "Name": "Subscription", "Values": ["RHEL"] }),
            "Subscription is documented for the subscription list, not for this one",
        ),
        (
            INSTANCES,
            json!({ "Name": "Status", "Values": ["running"], "Operator": "Equals" }),
            "Equals is outside the modelled Operator set",
        ),
    ];

    for (path, filter, why) in cases {
        let case = &format!("{path} {filter}: {why}");
        call(&h, path, &json!({ "Filters": [filter] }))
            .await
            .error(case, 400, "ValidationException");
    }

    // A refused request changes nothing: the next caller still sees the whole store.
    let case = "the store after the refusals";
    let reply = call(&h, SUBSCRIPTIONS, &json!({})).await;
    assert_eq!(
        subscriptions(&reply, case).len(),
        EXPECTED_SUBSCRIPTIONS.len(),
        "case {case:?}: expected the store untouched, got {:?}",
        reply.body
    );
}
