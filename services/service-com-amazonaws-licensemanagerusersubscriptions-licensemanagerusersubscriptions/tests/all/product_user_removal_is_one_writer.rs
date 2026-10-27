// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE rule every path that stops reporting a product user obeys,
//! driven over the real wire path: every call goes through the router's `dispatch`, so the
//! request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The RULE, asserted over a TABLE of inputs rather than one request: removing a product
//! user takes the record, the place it held in the subscription order, the
//! `(IdentityProvider, Product, Username)` index entry that reached it and the tags it
//! carried — all four, together — and takes nothing belonging to a subscription that was
//! not removed, however the removal was reached. The rows differ along the path that
//! reaches the removal (the caller ending ONE subscription and the clock completing its
//! teardown, the caller deregistering the registration the subscriptions hang off, and
//! that deregistration carried through to the registration's own clock-owed drop), the
//! number of subscriptions removed at once, the directory form, the product and whether
//! the subscriptions carried tags.
//!
//! The store is read directly, after each removal, because the four things a removal has to
//! take away are not all reportable: a list reports the record, but the order it is listed
//! in and the triple index that reaches it are only observable as an inconsistency later,
//! and the tags of a removed subscription cannot be read back through `ListTagsForResource`
//! at all (the ARN no longer resolves).

use std::collections::HashMap;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState;

// ONE wire harness per service: the dispatch plumbing and the register/subscribe helpers
// come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, deregister, harness, list_providers, list_subscriptions, provider_union, register,
    start_subscription, stop_subscription, Harness, PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use super::resource_resolution_refusals::registration;

/// How the removal of a product user is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// The caller ends each subscription and the teardown the clock owes completes it.
    StopThenOwedTeardown,
    /// The caller deregisters the registration the subscriptions hang off: nothing is
    /// subscribed through a registration that is being torn down.
    Deregister,
    /// That deregistration, carried through to the registration's own clock-owed drop.
    DeregisterThenOwedDrop,
}

/// One set of subscriptions to remove.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; both spellings appear below.
    directory_id: &'static str,
    product: &'static str,
    /// The users subscribed through this row's registration — one row removes a single
    /// subscription, another several at once.
    usernames: &'static [&'static str],
    tagged: bool,
    removal: Removal,
}

const ROWS: &[Row] = &[
    Row {
        case: "one subscription, ended by its caller and torn down by the clock",
        directory_id: "d-0123456789",
        product: PRODUCTS[0],
        usernames: &["alice"],
        tagged: false,
        removal: Removal::StopThenOwedTeardown,
    },
    Row {
        case: "three tagged subscriptions, all ended by their caller and torn down together",
        directory_id: "d-0123456789",
        product: PRODUCTS[2],
        usernames: &["bob", "carol@example.com", "dave.o-brien"],
        tagged: true,
        removal: Removal::StopThenOwedTeardown,
    },
    Row {
        case: "two tagged subscriptions dropped by the deregistration of their registration",
        directory_id: "d-00000000ff",
        product: PRODUCTS[3],
        usernames: &["erin", "frank"],
        tagged: true,
        removal: Removal::Deregister,
    },
    Row {
        case: "a shared directory's single subscription, dropped by its deregistration",
        directory_id: "sd-abcdef0123",
        product: PRODUCTS[1],
        usernames: &["grace"],
        tagged: false,
        removal: Removal::Deregister,
    },
    Row {
        case: "three subscriptions, through to the registration's own clock-owed drop",
        directory_id: "d-123456789a",
        product: PRODUCTS[4],
        usernames: &["heidi", "ivan", "judy"],
        tagged: false,
        removal: Removal::DeregisterThenOwedDrop,
    },
];

/// The registration every row's subscriptions keep running beside, so each removal is
/// checked for what it must NOT take as well as for what it must.
const BYSTANDER_DIRECTORY: &str = "d-0123456789";
const BYSTANDER_PRODUCT: &str = PRODUCTS[3];
const BYSTANDER_USERS: &[&str] = &["bystander.one", "bystander.two"];

/// The tags a subscription carries when its row is tagged.
fn row_tags() -> Value {
    json!({ "team": "licensing" })
}

impl Row {
    /// The `StartProductSubscription` body for `username` through this row's registration.
    fn start_request(&self, username: &str) -> Value {
        let mut body = json!({
            "Username": username,
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        });
        if self.tagged {
            body["Tags"] = row_tags();
        }
        body
    }
}

/// Subscribe `usernames` through `(directory_id, product)` and answer the
/// `ProductUserArn` each start minted, in the order they were subscribed.
async fn subscribe(h: &Harness, row: &Row) -> Vec<String> {
    let mut minted = Vec::new();
    for username in row.usernames {
        let reply = start_subscription(h, &row.start_request(username)).await;
        let summary = reply.ok(row.case)["ProductUserSummary"].clone();
        minted.push(as_str(&summary, "ProductUserArn", row.case).to_string());
    }
    minted
}

/// Everything the store keeps about the product users it is reporting: the ARNs it holds
/// records for, the subscription order, the ARNs the triple index reaches, and the ARNs
/// that carry tags.
#[derive(Debug)]
struct Snapshot {
    records: Vec<String>,
    order: Vec<String>,
    indexed: Vec<String>,
    tagged: Vec<String>,
}

fn snapshot(h: &Harness) -> Snapshot {
    let store = h
        .engine
        .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let guard = store.lock();
    let mut records: Vec<String> = guard.product_users.keys().cloned().collect();
    records.sort();
    let mut indexed: Vec<String> = guard.product_user_index.values().cloned().collect();
    indexed.sort();
    let mut tagged: Vec<String> = guard
        .tags
        .keys()
        .filter(|arn| arn.contains(":user/"))
        .cloned()
        .collect();
    tagged.sort();
    Snapshot {
        records,
        order: guard.product_user_order.clone(),
        indexed,
        tagged,
    }
}

impl Snapshot {
    /// Assert `arn` is in none of the four views a product user is reachable through.
    fn assert_absent(&self, case: &str, arn: &str) {
        assert!(
            !self.records.contains(&arn.to_string()),
            "case {case:?}: a removed subscription keeps no record, got {:?}",
            self.records
        );
        assert!(
            !self.order.contains(&arn.to_string()),
            "case {case:?}: a removed subscription keeps no place in the subscription order, \
             got {:?}",
            self.order
        );
        assert!(
            !self.indexed.contains(&arn.to_string()),
            "case {case:?}: a removed subscription is reachable through no index entry, got \
             {:?}",
            self.indexed
        );
        assert!(
            !self.tagged.contains(&arn.to_string()),
            "case {case:?}: a removed subscription carries no tags, got {:?}",
            self.tagged
        );
    }

    /// Assert `arn` is still in the record store, the order and the index — what a removal
    /// of OTHER subscriptions must leave alone.
    fn assert_present(&self, case: &str, arn: &str) {
        assert!(
            self.records.contains(&arn.to_string())
                && self.order.contains(&arn.to_string())
                && self.indexed.contains(&arn.to_string()),
            "case {case:?}: a subscription nothing removed must stay reachable through every \
             view, got records={:?}, order={:?}, index={:?}",
            self.records,
            self.order,
            self.indexed
        );
    }
}

/// A read that lets the service materialise whatever the clock has made due, so a sweep
/// that completes a teardown is reached the way a client reaches it.
async fn read_through(h: &Harness) {
    list_providers(h, &json!({})).await.ok("a plain list reads");
}

/// Carry out one row's removal over the wire.
async fn remove(h: &Harness, row: &Row, provider_arn: &str, product_users: &[String]) {
    match row.removal {
        Removal::StopThenOwedTeardown => {
            for arn in product_users {
                stop_subscription(h, &json!({ "ProductUserArn": arn }))
                    .await
                    .ok(row.case);
            }
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
            // The teardown the stop announced is now due; a read is what materialises it.
            list_subscriptions(
                h,
                &json!({
                    "IdentityProvider": provider_union(row.directory_id),
                    "Product": row.product,
                }),
            )
            .await
            .ok(row.case);
        }
        Removal::Deregister => {
            deregister(h, &json!({ "IdentityProviderArn": provider_arn }))
                .await
                .ok(row.case);
        }
        Removal::DeregisterThenOwedDrop => {
            deregister(h, &json!({ "IdentityProviderArn": provider_arn }))
                .await
                .ok(row.case);
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
            read_through(h).await;
        }
    }
}

/// RULE: removing a product user takes the record, its place in the subscription order, the
/// `(IdentityProvider, Product, Username)` index entry that reaches it and its tags — all
/// four, together — and takes nothing belonging to a subscription that was not removed,
/// whichever path reached the removal.
#[tokio::test]
async fn every_removal_path_unindexes_a_product_user_the_same_way() {
    let h = harness();

    // The registration whose subscriptions survive every removal below.
    let bystander = Row {
        case: "the subscriptions nothing removes",
        directory_id: BYSTANDER_DIRECTORY,
        product: BYSTANDER_PRODUCT,
        usernames: BYSTANDER_USERS,
        tagged: true,
        removal: Removal::Deregister,
    };
    register(
        &h,
        &registration(
            bystander.directory_id,
            bystander.product,
            &["subnet-0123456789abcdef0"],
            None,
        ),
    )
    .await
    .ok(bystander.case);
    let untouched = subscribe(&h, &bystander).await;

    for row in ROWS {
        let reply = register(
            &h,
            &registration(
                row.directory_id,
                row.product,
                &["subnet-0123456789abcdef0"],
                None,
            ),
        )
        .await;
        let provider_arn = as_str(
            &reply.ok(row.case)["IdentityProviderSummary"],
            "IdentityProviderArn",
            row.case,
        )
        .to_string();
        let minted = subscribe(&h, row).await;

        // The precondition: every view the store keeps reaches each new subscription, and
        // a tagged row's subscriptions carry their tags.
        let before = snapshot(&h);
        for arn in &minted {
            before.assert_present(row.case, arn);
            assert_eq!(
                before.tagged.contains(arn),
                row.tagged,
                "case {:?}: a subscription carries exactly the tags its start supplied, got \
                 {:?}",
                row.case,
                before.tagged
            );
        }
        let order_before = before.order.clone();

        remove(&h, row, &provider_arn, &minted).await;

        let after = snapshot(&h);
        for arn in &minted {
            after.assert_absent(row.case, arn);
        }
        for arn in &untouched {
            after.assert_present(row.case, arn);
        }
        // The removal rebuilt the order by dropping exactly the removed ARNs: the ones that
        // stayed keep the order they were subscribed in.
        let expected_order: Vec<String> = order_before
            .into_iter()
            .filter(|arn| !minted.contains(arn))
            .collect();
        assert_eq!(
            after.order, expected_order,
            "case {:?}: a removal must drop exactly the removed subscriptions from the \
             subscription order and reorder nothing",
            row.case
        );
    }

    // Nothing but the bystander's subscriptions is left, and each of them is still reported
    // over the wire with the tags its start stored.
    let left = snapshot(&h);
    let mut expected = untouched.clone();
    expected.sort();
    assert_eq!(
        left.records, expected,
        "once every removal path has run, exactly the subscriptions nothing removed are \
         still held"
    );
    assert_eq!(
        left.indexed, expected,
        "the triple index reaches exactly the subscriptions that are still held"
    );
    assert_eq!(
        left.tagged, expected,
        "exactly the subscriptions that are still held carry tags"
    );
    assert_eq!(
        left.order, untouched,
        "the surviving subscriptions keep the order they were subscribed in"
    );

    let reported = list_subscriptions(
        &h,
        &json!({
            "IdentityProvider": provider_union(BYSTANDER_DIRECTORY),
            "Product": BYSTANDER_PRODUCT,
        }),
    )
    .await;
    let summaries = reported.ok("the surviving registration")["ProductUserSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let names: Vec<&str> = summaries
        .iter()
        .map(|summary| as_str(summary, "Username", "the surviving registration"))
        .collect();
    assert_eq!(
        names, BYSTANDER_USERS,
        "the registration nothing deregistered still reports its own subscribers, got \
         {summaries:?}"
    );
    let statuses: HashMap<&str, &str> = summaries
        .iter()
        .map(|summary| {
            (
                as_str(summary, "Username", "the surviving registration"),
                as_str(summary, "Status", "the surviving registration"),
            )
        })
        .collect();
    for username in BYSTANDER_USERS {
        assert_eq!(
            statuses.get(username).copied(),
            Some("SUBSCRIBED"),
            "a subscription nothing ended is still subscribed, got {statuses:?}"
        );
    }
}
