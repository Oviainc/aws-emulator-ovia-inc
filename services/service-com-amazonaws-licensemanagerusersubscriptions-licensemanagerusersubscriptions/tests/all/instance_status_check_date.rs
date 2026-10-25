// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the status-check date `ListInstances` reports, driven over the
//! real wire path: every call goes through the router's `dispatch`, so each request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded request:
//!
//! * an instance this service tracks reports WHEN its status was last checked
//!   (`InstanceSummary.LastStatusCheckDate`): the instant the service established the
//!   status it reports for that instance, which is the write that brought the instance
//!   into the service's view;
//! * that instant is persisted, so every read answers the same value, whatever reading
//!   path it comes through, and a read neither checks a status nor invents a date;
//! * a later write about what the instance PROVIDES (another registration for its
//!   directory, one deregistered, another user associated, one disassociated) says nothing
//!   new about the instance, so the recorded instant stands;
//! * an instance that LEFT the view and is brought back in was established again, so it
//!   reports the later instant.
//!
//! The dates are compared as strings: this service formats an instant as ISO-8601 UTC at
//! whole-second precision (`crate::state::iso8601`), a fixed-width form that orders
//! exactly as the instants it encodes.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::iso8601;

// ONE wire harness per service: these come from the sibling suites rather than a second
// copy of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    call, deregister, harness, provider_union, register, Harness, PRODUCTS,
};
use crate::instance_products_follow_the_registrations::summary_of;
use crate::resource_resolution_refusals::{associate, disassociate, held_instance, registration};

/// The subnet every registration in this suite is settled in: `Settings` is required and
/// carries no part of the rule under test, so one value is used throughout.
const SUBNETS: &[&str] = &["subnet-0123456789abcdef0"];

/// Long enough for every span this service owes the clock to have fallen due (an
/// association's `ASSOCIATED` transition and a disassociation's teardown are both well
/// under a second), and long enough for the whole second a date is reported at to have
/// advanced — so an instant recorded after this wait is strictly later than one recorded
/// before it.
const PAST_EVERY_SPAN: Duration = Duration::from_millis(1_100);

/// The `LastStatusCheckDate` `ListInstances` reports for `instance_id`, which has to be in
/// the service's view.
async fn check_date(h: &Harness, instance_id: &str, case: &str) -> String {
    let summary = summary_of(h, instance_id, case)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: {instance_id} is not in the instance view"));
    summary
        .get("LastStatusCheckDate")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: an instance in view reports when its status was checked, got \
                 {summary:?}"
            )
        })
        .to_string()
}

/// One instance to bring into the service's view. The rows differ along what the rule
/// generalises over: the directory-id form, which instance of the account's fleet it is,
/// how many products its directory is registered for, and how many users are associated
/// with it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// Both forms `ActiveDirectoryIdentityProvider.DirectoryId`'s `@pattern`
    /// (`^(d|sd)-[0-9a-f]{10}$`) admits appear below: a directory this account owns and one
    /// shared into it.
    directory_id: &'static str,
    /// Which instance of the account's EC2 fleet the users are associated with.
    instance: usize,
    /// The products the directory is registered for; at least one, because `AssociateUser`
    /// requires the directory to be registered.
    products: &'static [&'static str],
    /// The users associated with the instance, in association order.
    users: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "one product, one user, aws-managed directory",
        directory_id: "d-0123456789",
        instance: 0,
        products: &[PRODUCTS[0]],
        users: &["checked.user"],
    },
    Row {
        case: "two products, two users, another instance of the fleet",
        directory_id: "d-00000000ff",
        instance: 1,
        products: &[PRODUCTS[1], PRODUCTS[2]],
        users: &["checked.user", "checked.other"],
    },
    Row {
        case: "a directory shared into the account, three users",
        directory_id: "sd-abcdef0123",
        instance: 2,
        products: &[PRODUCTS[4]],
        users: &["one.user", "another.user", "a.third.user"],
    },
    Row {
        case: "every product the service admits, one user",
        directory_id: "d-123456789a",
        instance: 0,
        products: PRODUCTS,
        users: &["every.product.user"],
    },
];

/// Register `row`'s products and associate `row`'s users, and answer the window
/// `[before, after]` the instance entered the view in.
async fn bring_into_view(h: &Harness, row: &Row) -> (String, String) {
    for product in row.products {
        register(h, &registration(row.directory_id, product, SUBNETS, None))
            .await
            .ok(&format!("{}: registering {product}", row.case));
    }
    let before = iso8601(SystemTime::now());
    for user in row.users {
        associate(
            h,
            &json!({
                "Username": user,
                "InstanceId": held_instance(row.instance),
                "IdentityProvider": provider_union(row.directory_id),
            }),
        )
        .await
        .ok(&format!("{}: associating {user}", row.case));
    }
    (before, iso8601(SystemTime::now()))
}

/// RULE: an instance in this service's view reports the instant its status was established
/// — an instant inside the window the association that brought it into view fell in — and
/// every read answers that same persisted instant, whatever path it is read through.
#[tokio::test]
async fn an_instance_in_view_reports_when_its_status_was_checked() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        let instance_id = held_instance(row.instance);

        let (before, after) = bring_into_view(&h, row).await;
        let reported = check_date(&h, &instance_id, case).await;
        assert!(
            reported >= before && reported <= after,
            "case {case:?}: expected a check inside [{before}, {after}], got {reported:?}"
        );

        // The same instant through every other reading path: the same list again, a
        // filtered list, and a single-element page.
        for (path, request) in [
            ("read again", json!({})),
            (
                "filtered on the status the instance is in",
                json!({ "Filters": [{ "Attribute": "Status", "Value": "ACTIVE" }] }),
            ),
            ("one instance per page", json!({ "MaxResults": 1 })),
            (
                "filtered on the instance id",
                json!({ "Filters": [{ "Attribute": "InstanceId", "Value": instance_id }] }),
            ),
        ] {
            let reply = call(&h, "/instance/ListInstances", &request).await;
            let again = reply.ok(case)["InstanceSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries in {:?}", reply.body))
                .iter()
                .find(|summary| {
                    summary.get("InstanceId").and_then(Value::as_str) == Some(instance_id.as_str())
                })
                .and_then(|summary| {
                    summary
                        .get("LastStatusCheckDate")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            assert_eq!(
                again.as_deref(),
                Some(reported.as_str()),
                "case {case:?} ({path}): every read answers the persisted instant, got {again:?}"
            );
        }

        // Time passing is not a status check: the clock-owed `ASSOCIATED` transition
        // materialises on the next read, and the recorded instant is still the one the
        // association left behind.
        tokio::time::sleep(PAST_EVERY_SPAN).await;
        let later = check_date(&h, &instance_id, case).await;
        assert_eq!(
            later, reported,
            "case {case:?}: a read performs no status check, expected {reported:?}, got {later:?}"
        );
    }
}

/// RULE: a write about what an instance PROVIDES establishes nothing new about the
/// instance, so the instant its status was checked at is left exactly as it was.
#[tokio::test]
async fn a_write_about_what_an_instance_provides_leaves_its_status_check_alone() {
    let row = ROWS[1];
    let case = row.case;
    let h = harness();
    let instance_id = held_instance(row.instance);
    bring_into_view(&h, &row).await;
    let checked = check_date(&h, &instance_id, case).await;

    // Far enough past the entry that a re-stamped instant would be a DIFFERENT whole
    // second, so each write below is a real test of "unchanged".
    tokio::time::sleep(PAST_EVERY_SPAN).await;

    // Every kind of write this service has that recomputes what an instance provides.
    let one_more_product = PRODUCTS[3];
    for write in [
        Write::RegisterAnotherProduct,
        Write::DeregisterThatProduct,
        Write::AssociateAnotherUser,
        Write::DisassociateAUser,
    ] {
        let what = format!("{write:?}");
        match write {
            Write::RegisterAnotherProduct => {
                register(
                    &h,
                    &registration(row.directory_id, one_more_product, SUBNETS, None),
                )
                .await
                .ok(&what);
            }
            Write::DeregisterThatProduct => {
                deregister(
                    &h,
                    &json!({
                        "IdentityProvider": provider_union(row.directory_id),
                        "Product": one_more_product,
                    }),
                )
                .await
                .ok(&what);
            }
            Write::AssociateAnotherUser => {
                associate(
                    &h,
                    &json!({
                        "Username": "one.more.user",
                        "InstanceId": instance_id,
                        "IdentityProvider": provider_union(row.directory_id),
                    }),
                )
                .await
                .ok(&what);
            }
            Write::DisassociateAUser => {
                disassociate(
                    &h,
                    &json!({
                        "Username": row.users[0],
                        "InstanceId": instance_id,
                        "IdentityProvider": provider_union(row.directory_id),
                    }),
                )
                .await
                .ok(&what);
            }
        }
        let reported = check_date(&h, &instance_id, case).await;
        assert_eq!(
            reported, checked,
            "case {what:?}: the recorded status check stands, expected {checked:?}, got \
             {reported:?}"
        );
    }
}

/// A write that recomputes what an instance in the view provides, without saying anything
/// new about the instance itself.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// One more product registered for the directory the instance's users came through.
    RegisterAnotherProduct,
    /// That registration torn down again.
    DeregisterThatProduct,
    /// One more user associated with the instance.
    AssociateAnotherUser,
    /// One of the instance's users disassociated, leaving others.
    DisassociateAUser,
}

/// RULE: an instance that left the service's view and is brought back into it had its
/// status established again, so it reports the LATER instant.
#[tokio::test]
async fn an_instance_brought_back_into_the_view_reports_the_later_check() {
    for row in [ROWS[0], ROWS[2]] {
        let case = row.case;
        let h = harness();
        let instance_id = held_instance(row.instance);

        bring_into_view(&h, &row).await;
        let first = check_date(&h, &instance_id, case).await;

        // Every association removed takes the instance out of the view, once the teardown
        // each disassociation starts has completed.
        for user in row.users {
            disassociate(
                &h,
                &json!({
                    "Username": user,
                    "InstanceId": instance_id,
                    "IdentityProvider": provider_union(row.directory_id),
                }),
            )
            .await
            .ok(&format!("{case}: disassociating {user}"));
        }
        tokio::time::sleep(PAST_EVERY_SPAN).await;
        assert!(
            summary_of(&h, &instance_id, case).await.is_none(),
            "case {case:?}: an instance leaves the view with its last association"
        );

        let before = iso8601(SystemTime::now());
        associate(
            &h,
            &json!({
                "Username": row.users[0],
                "InstanceId": instance_id,
                "IdentityProvider": provider_union(row.directory_id),
            }),
        )
        .await
        .ok(&format!("{case}: associating the user again"));
        let after = iso8601(SystemTime::now());

        let second = check_date(&h, &instance_id, case).await;
        assert!(
            second >= before && second <= after,
            "case {case:?}: expected a check inside [{before}, {after}], got {second:?}"
        );
        assert!(
            second > first,
            "case {case:?}: an instance checked again reports the later instant, got {second:?} \
             after {first:?}"
        );
    }
}
