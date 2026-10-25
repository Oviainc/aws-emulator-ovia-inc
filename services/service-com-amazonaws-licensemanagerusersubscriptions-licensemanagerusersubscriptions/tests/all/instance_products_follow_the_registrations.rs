// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule `InstanceSummary.Products` follows, driven over the real
//! wire path: every call goes through the router's `dispatch`, so each request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded request:
//!
//! * what `ListInstances` reports an instance PROVIDES is the products the directories of
//!   the user associations naming that instance are registered for, as those registrations
//!   stand at the READ — so a registration added for such a directory is reported by the
//!   very next list, with no other write in between, and a registration removed stops
//!   being reported the same way;
//! * a registration for a directory no association on an instance names changes nothing
//!   about that instance, and brings no instance into the view.
//!
//! `tests/all/resource_resolution_refusals.rs` covers the other half of the same rule
//! (the ASSOCIATIONS behind the products coming and going); these tests cover the
//! registration side, in both orders of register and associate.

use serde_json::{json, Value};

// ONE wire harness per service: these come from the sibling suites rather than a second
// copy of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    call, deregister, harness, provider_union, register, Harness, PRODUCTS,
};
use crate::resource_resolution_refusals::{associate, held_instance, registration};

/// The subnet every registration in this suite is settled in: `Settings` is required and
/// carries no part of the rule under test, so one value is used throughout.
const SUBNETS: &[&str] = &["subnet-0123456789abcdef0"];

/// The `InstanceSummary` `ListInstances` currently reports for `instance_id`, or `None`
/// when no summary names it.
///
/// Shared with `tests/all/instance_status_check_date.rs`, which reads the same view
/// through the same single reader rather than growing a second copy of it.
pub(crate) async fn summary_of(h: &Harness, instance_id: &str, case: &str) -> Option<Value> {
    let reply = call(h, "/instance/ListInstances", &json!({})).await;
    reply.ok(case)["InstanceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries in {:?}", reply.body))
        .iter()
        .find(|summary| summary.get("InstanceId").and_then(Value::as_str) == Some(instance_id))
        .cloned()
}

/// The `Products` `ListInstances` reports for `instance_id`, which has to be in view.
async fn products_of(h: &Harness, instance_id: &str, case: &str) -> Vec<String> {
    let summary = summary_of(h, instance_id, case)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: {instance_id} is not in the instance view"));
    summary["Products"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Products is required, got {summary:?}"))
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// How many instances `ListInstances` reports.
async fn instances_in_view(h: &Harness, case: &str) -> usize {
    let reply = call(h, "/instance/ListInstances", &json!({})).await;
    reply.ok(case)["InstanceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries in {:?}", reply.body))
        .len()
}

/// One ordering of registrations and one association to exercise. The rows differ along
/// what the rule generalises over: the directory-id form, the instance, how many products
/// the directory is registered for BEFORE the association, and how many are registered
/// AFTER it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// Both forms `ActiveDirectoryIdentityProvider.DirectoryId`'s `@pattern`
    /// (`^(d|sd)-[0-9a-f]{10}$`) admits appear below: a directory this account owns and
    /// one shared into it.
    directory_id: &'static str,
    /// Which instance of the account's EC2 fleet the user is associated with.
    instance: usize,
    /// The products the directory is registered for BEFORE the user is associated. At
    /// least one: `AssociateUser` requires the directory to be registered.
    before: &'static [&'static str],
    /// The products registered AFTER the association, in the order they are registered.
    after: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "one product before the association, one after",
        directory_id: "d-0123456789",
        instance: 0,
        before: &[PRODUCTS[0]],
        after: &[PRODUCTS[3]],
    },
    Row {
        case: "one before, three after",
        directory_id: "d-00000000ff",
        instance: 1,
        before: &[PRODUCTS[4]],
        after: &[PRODUCTS[0], PRODUCTS[1], PRODUCTS[2]],
    },
    Row {
        case: "two before, two after",
        directory_id: "d-123456789a",
        instance: 2,
        before: &[PRODUCTS[1], PRODUCTS[2]],
        after: &[PRODUCTS[3], PRODUCTS[0]],
    },
    Row {
        case: "a directory shared into the account, one before and one after",
        directory_id: "sd-abcdef0123",
        instance: 0,
        before: &[PRODUCTS[2]],
        after: &[PRODUCTS[4]],
    },
    Row {
        case: "every product the service admits, all but the first registered after",
        directory_id: "d-0123456789",
        instance: 1,
        before: &[PRODUCTS[0]],
        after: &[PRODUCTS[1], PRODUCTS[2], PRODUCTS[3], PRODUCTS[4]],
    },
];

/// RULE: an instance provides the products the directories of the associations naming it
/// are registered for AT THE READ — so a registration added after the association is
/// reported by the next list, with no other write in between, and the products are the
/// registrations in registration order.
#[tokio::test]
async fn a_registration_added_after_an_association_is_reported_at_once() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        let instance_id = held_instance(row.instance);

        for product in row.before {
            register(&h, &registration(row.directory_id, product, SUBNETS, None))
                .await
                .ok(&format!("{case}: registering {product} before the association"));
        }
        associate(
            &h,
            &json!({
                "Username": "products.user",
                "InstanceId": instance_id,
                "IdentityProvider": provider_union(row.directory_id),
            }),
        )
        .await
        .ok(&format!("{case}: associating the user"));

        let mut expected: Vec<String> = row.before.iter().map(|p| (*p).to_string()).collect();
        let reported = products_of(&h, &instance_id, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: an association reports the products its directory is already \
             registered for, expected {expected:?}, got {reported:?}"
        );

        // Each registration added AFTER the association is reported by the very next
        // list: no other write happens in between.
        for product in row.after {
            register(&h, &registration(row.directory_id, product, SUBNETS, None))
                .await
                .ok(&format!("{case}: registering {product} after the association"));
            expected.push((*product).to_string());
            let reported = products_of(&h, &instance_id, case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: after registering {product} the instance must provide \
                 {expected:?}, got {reported:?}"
            );
        }

        // A read answers the same twice: the list is computed from state, not corrected by
        // being read.
        let first = call(&h, "/instance/ListInstances", &json!({})).await;
        let second = call(&h, "/instance/ListInstances", &json!({})).await;
        assert_eq!(
            first.ok(case),
            second.ok(case),
            "case {case:?}: a read must answer the same twice, got {:?} then {:?}",
            first.body,
            second.body
        );

        // The same rule by the other route: the product goes with its registration, and
        // the products left are still the remaining registrations in registration order.
        let dropped = row.before[0];
        deregister(
            &h,
            &json!({
                "IdentityProvider": provider_union(row.directory_id),
                "Product": dropped,
            }),
        )
        .await
        .ok(&format!("{case}: deregistering {dropped}"));
        expected.retain(|product| product != dropped);
        let reported = products_of(&h, &instance_id, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: after deregistering {dropped} the instance must provide \
             {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: only the instances whose associations resolve through a directory see that
/// directory's registrations — a registration for any other directory changes no instance
/// summary, and brings no instance into the view (an instance enters the view through an
/// association, never through a registration).
#[tokio::test]
async fn a_registration_for_another_directory_changes_no_other_instance() {
    let h = harness();
    // Two instances, each associated through its own directory, each registered for a
    // different product — so a stale value could not be mistaken for the right one.
    let rows = [
        (0usize, "d-0123456789", PRODUCTS[0]),
        (1usize, "d-00000000ff", PRODUCTS[1]),
    ];
    for (instance, directory_id, product) in rows {
        register(&h, &registration(directory_id, product, SUBNETS, None))
            .await
            .ok(directory_id);
        associate(
            &h,
            &json!({
                "Username": format!("user.on{instance}"),
                "InstanceId": held_instance(instance),
                "IdentityProvider": provider_union(directory_id),
            }),
        )
        .await
        .ok(directory_id);
    }

    // (case, the directory registered for one more product, the instance that must change)
    let cases: [(&str, &str, Option<usize>); 3] = [
        (
            "a directory the second instance is associated through",
            "d-00000000ff",
            Some(1),
        ),
        (
            "a directory the first instance is associated through",
            "d-0123456789",
            Some(0),
        ),
        (
            "a directory no association names",
            "d-123456789a",
            None,
        ),
    ];
    // What each instance must provide, carried across the cases: a registration adds its
    // product to the instances reached through ITS directory and to no other.
    let mut expected: Vec<Vec<String>> = rows
        .iter()
        .map(|(_, _, product)| vec![(*product).to_string()])
        .collect();
    let added = PRODUCTS[2];
    for (case, directory_id, changes) in cases {
        let mut before: Vec<Value> = Vec::new();
        for (instance, _, _) in rows.iter() {
            before.push(
                summary_of(&h, &held_instance(*instance), case)
                    .await
                    .unwrap_or_else(|| panic!("case {case:?}: an instance left the view")),
            );
        }

        register(&h, &registration(directory_id, added, SUBNETS, None))
            .await
            .ok(case);
        if let Some(index) = changes {
            expected[index].push(added.to_string());
        }

        for (index, (instance, _, _)) in rows.iter().enumerate() {
            let instance_id = held_instance(*instance);
            let reported = products_of(&h, &instance_id, case).await;
            let expected = &expected[index];
            assert_eq!(
                &reported, expected,
                "case {case:?}: instance {instance_id} must provide {expected:?}, got \
                 {reported:?}"
            );

            // Nothing else about an instance is a function of the registrations, so no
            // other member moves.
            let now = summary_of(&h, &instance_id, case)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: {instance_id} left the view"));
            let mut was = before[index].clone();
            was["Products"] = now["Products"].clone();
            assert_eq!(
                now, was,
                "case {case:?}: only Products may change for {instance_id}, got {now:?} from \
                 {:?}",
                before[index]
            );
        }

        assert_eq!(
            instances_in_view(&h, case).await,
            rows.len(),
            "case {case:?}: a registration brings no instance into the view"
        );
    }
}
