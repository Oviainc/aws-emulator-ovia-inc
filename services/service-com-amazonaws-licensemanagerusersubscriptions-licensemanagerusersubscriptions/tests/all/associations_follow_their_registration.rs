// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule a user association's LIFETIME follows, driven over the
//! real wire path: every call goes through the router's `dispatch`, so each request is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would
//! be.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded request:
//! a user association exists only through a live identity-provider registration of its
//! directory — that is the resolution both `AssociateUser` and `ListUserAssociations` run
//! before they report anything — so
//!
//! * a deregistration that leaves the directory another live registration leaves its
//!   associations exactly as they stood (an association names a directory and no product);
//! * the LAST live registration of a directory takes the associations made through it with
//!   it: the store holds no association the service can no longer report, the
//!   `(directory, instance, user)` triple each occupied is free again once the directory
//!   is registered anew, and the instance leaves this service's view with the last
//!   association naming it instead of being reported providing no product at all;
//! * an instance reported by `ListInstances` therefore always provides at least one
//!   product (`InstanceSummary.Products` is required and documented as "A list of provided
//!   user-based subscription products").
//!
//! `tests/all/instance_products_follow_the_registrations.rs` covers which products an
//! instance in view provides; this suite covers whether the association — and so the
//! instance — is there at all.
//!
//! The harness (router + engine + the account's directory fleet the registrations resolve
//! against) is the one in [`super::identity_provider_subscriptions`]; only the rule is new
//! here.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::IDENTITY_PROVIDER_NOT_REGISTERED;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service, and one copy of each request builder: these come from the
// sibling suites rather than a second copy of the same plumbing.
use crate::identity_provider_subscriptions::{
    as_str, call, deregister, harness, list_providers, provider_union, register, Harness, Reply,
    PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use crate::resource_resolution_refusals::{associate, held_instance};

/// The subnet every registration in this suite is settled in: `Settings` is required and
/// carries no part of the rule under test, so one value is used throughout.
const SUBNETS: &[&str] = &["subnet-0123456789abcdef0"];

/// How a row addresses its directory. All three forms the model admits appear in the
/// table: both `ActiveDirectoryIdentityProvider.DirectoryId` forms its `@pattern`
/// (`^(d|sd)-[0-9a-f]{10}$`) allows — a directory of the account and one shared into it —
/// and a self-managed directory, which carries no `DirectoryId` and is named by
/// `ActiveDirectorySettings.DomainName` instead (smithy_query shape
/// ActiveDirectoryIdentityProvider).
#[derive(Debug, Clone, Copy)]
enum Directory {
    /// An AWS-managed directory of the account's fleet.
    AwsManaged(&'static str),
    /// A directory shared INTO the account, which this service does not resolve.
    SharedIntoAccount(&'static str),
    /// A self-managed Active Directory, named by its domain.
    SelfManaged(&'static str),
}

impl Directory {
    fn union(self) -> Value {
        match self {
            Directory::AwsManaged(id) | Directory::SharedIntoAccount(id) => provider_union(id),
            Directory::SelfManaged(domain) => json!({
                "ActiveDirectoryIdentityProvider": {
                    "ActiveDirectorySettings": { "DomainName": domain }
                }
            }),
        }
    }
}

/// One directory's registrations and the users associated through it. The rows differ
/// along what the rule generalises over: the form the directory is addressed by, which
/// instance of the account's fleet the users are on, how many products the directory is
/// registered for, the order those registrations are deregistered in, and how many users
/// are associated through it.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    directory: Directory,
    /// Index into the account's seeded EC2 fleet, so a row names an instance the
    /// authorization gate holds without hard-coding its id.
    instance: usize,
    /// The products the directory is registered for, in registration order. At least one:
    /// `AssociateUser` requires the directory to have a live registration.
    products: &'static [&'static str],
    /// The order the registrations are deregistered in, as indices into `products`.
    deregistration_order: &'static [usize],
    /// The users associated through the directory on that instance, in association order.
    usernames: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "one product, one user",
        directory: Directory::AwsManaged("d-0123456789"),
        instance: 0,
        products: &[PRODUCTS[0]],
        deregistration_order: &[0],
        usernames: &["alice"],
    },
    Row {
        case: "three products deregistered in registration order, two users",
        directory: Directory::AwsManaged("d-00000000ff"),
        instance: 1,
        products: &[PRODUCTS[1], PRODUCTS[2], PRODUCTS[3]],
        deregistration_order: &[0, 1, 2],
        usernames: &["bob", "CAROL"],
    },
    Row {
        case: "a directory shared into the account, two products deregistered in reverse",
        directory: Directory::SharedIntoAccount("sd-abcdef0123"),
        instance: 2,
        products: &[PRODUCTS[4], PRODUCTS[0]],
        deregistration_order: &[1, 0],
        usernames: &["dave", "erin", "frank.admin"],
    },
    Row {
        case: "a self-managed directory named by its domain, middle product first",
        directory: Directory::SelfManaged("self.managed.example"),
        instance: 0,
        products: &[PRODUCTS[0], PRODUCTS[2], PRODUCTS[4]],
        deregistration_order: &[1, 2, 0],
        usernames: &["gina"],
    },
];

/// `ListUserAssociations` is bound to POST /user/ListUserAssociations (smithy_query:
/// operation ListUserAssociations, http.uri).
async fn list_associations(h: &Harness, provider: &Value, instance_id: &str) -> Reply {
    call(
        h,
        "/user/ListUserAssociations",
        &json!({ "InstanceId": instance_id, "IdentityProvider": provider }),
    )
    .await
}

/// A `RegisterIdentityProvider` body for any of the provider forms, which is why this
/// suite builds its own rather than the `DirectoryId`-only one its siblings share.
fn registration(provider: &Value, product: &str) -> Value {
    json!({
        "IdentityProvider": provider,
        "Product": product,
        "Settings": { "Subnets": SUBNETS, "SecurityGroupId": "sg-0123abcd" },
    })
}

fn deregistration(provider: &Value, product: &str) -> Value {
    json!({ "IdentityProvider": provider, "Product": product })
}

/// The user names `ListUserAssociations` reports for one scope.
async fn listed_usernames(
    h: &Harness,
    provider: &Value,
    instance_id: &str,
    case: &str,
) -> Vec<String> {
    let reply = list_associations(h, provider, instance_id).await;
    reply.ok(case)["InstanceUserSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: no InstanceUserSummaries in {:?}", reply.body)
        })
        .iter()
        .map(|summary| as_str(summary, "Username", case).to_string())
        .collect()
}

/// The user names the STORE holds for `instance_id`, read through the engine the handlers
/// write: an association the service cannot report must not be sitting in state either,
/// which is the whole point of the rule.
fn held_usernames(h: &Harness, instance_id: &str) -> Vec<String> {
    let store = h.engine.state::<ServiceState>();
    let guard = store.lock();
    guard
        .instance_user_order
        .iter()
        .filter_map(|arn| guard.instance_users.get(arn))
        .filter(|record| record.instance_id == instance_id)
        .map(|record| record.username.clone())
        .collect()
}

/// What `ListInstances` reports, as `(InstanceId, Products)` pairs.
async fn instances_in_view(h: &Harness, case: &str) -> Vec<(String, Vec<String>)> {
    let reply = call(h, "/instance/ListInstances", &json!({})).await;
    reply.ok(case)["InstanceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InstanceSummaries in {:?}", reply.body))
        .iter()
        .map(|summary| {
            let products = summary["Products"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Products is required, got {summary:?}"))
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            (as_str(summary, "InstanceId", case).to_string(), products)
        })
        .collect()
}

/// Every instance `ListInstances` reports provides at least one product: an instance is in
/// this service's view because it provides user-based subscriptions, so one providing none
/// is a view no state implies.
async fn every_instance_in_view_provides_a_product(h: &Harness, case: &str) {
    for (instance_id, products) in instances_in_view(h, case).await {
        assert!(
            !products.is_empty(),
            "case {case:?}: instance {instance_id} is in the view providing no product"
        );
    }
}

/// RULE: a user association exists only through a live registration of its directory — a
/// deregistration that leaves another live registration for that directory leaves the
/// associations as they stood, and the last one takes them with it, freeing the triple
/// each occupied and taking the instance out of the view with the last association naming
/// it.
#[tokio::test]
async fn associations_outlive_every_registration_of_their_directory_but_the_last() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        let provider = row.directory.union();
        let instance_id = held_instance(row.instance);

        for product in row.products {
            register(&h, &registration(&provider, product))
                .await
                .ok(&format!("{case}: registering {product}"));
        }
        for username in row.usernames {
            associate(
                &h,
                &json!({
                    "Username": username,
                    "InstanceId": instance_id,
                    "IdentityProvider": provider,
                }),
            )
            .await
            .ok(&format!("{case}: associating {username}"));
        }

        let expected: Vec<String> = row.usernames.iter().map(|u| (*u).to_string()).collect();
        let listed = listed_usernames(&h, &provider, &instance_id, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the associations just made, got {listed:?}"
        );

        // Each deregistration but the last leaves the directory another live registration,
        // so it leaves every association exactly as it stood: an association names a
        // directory and no product.
        let last = row.deregistration_order.len() - 1;
        for (step, index) in row.deregistration_order.iter().enumerate() {
            let product = row.products[*index];
            let step_case = format!("{case}: deregistering {product}");
            deregister(&h, &deregistration(&provider, product))
                .await
                .ok(&step_case);

            if step < last {
                let listed = listed_usernames(&h, &provider, &instance_id, &step_case).await;
                assert_eq!(
                    listed, expected,
                    "case {step_case:?}: a directory with a live registration left keeps \
                     its associations, got {listed:?}"
                );
                assert_eq!(
                    held_usernames(&h, &instance_id),
                    expected,
                    "case {step_case:?}: the store must hold exactly those associations"
                );
                assert!(
                    instances_in_view(&h, &step_case)
                        .await
                        .iter()
                        .any(|(id, _)| id == &instance_id),
                    "case {step_case:?}: the instance stays in view while an association \
                     names it"
                );
            }
            every_instance_in_view_provides_a_product(&h, &step_case).await;
        }

        // The last live registration of the directory is gone, so nothing is associated
        // through it any more: not in the list, and not in the store behind it.
        let reply = list_associations(&h, &provider, &instance_id).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: a directory with no live registration is the modelled \
             ResourceNotFoundException at HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {reply:?}"
        );
        assert_eq!(
            reply.message(),
            Some(IDENTITY_PROVIDER_NOT_REGISTERED),
            "case {case:?}: the user-association plane states this refusal in one \
             sentence, got {reply:?}"
        );
        assert_eq!(
            held_usernames(&h, &instance_id),
            Vec::<String>::new(),
            "case {case:?}: no association may outlive the last registration of its \
             directory"
        );
        let in_view = instances_in_view(&h, case).await;
        assert!(
            !in_view.iter().any(|(id, _)| id == &instance_id),
            "case {case:?}: the instance must leave the view with its last association, \
             got {in_view:?}"
        );

        // Once the teardown the clock owes has completed, the registration is gone from the
        // list too, and nothing came back.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let listed = list_providers(&h, &json!({})).await;
        assert_eq!(
            listed.ok(case)["IdentityProviderSummaries"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {case:?}: every registration of this row was deregistered, got {:?}",
            listed.body
        );
        assert_eq!(
            held_usernames(&h, &instance_id),
            Vec::<String>::new(),
            "case {case:?}: the completed teardown must not resurrect an association"
        );

        // The triples the dropped associations occupied are free: registering the
        // directory anew gives an empty list to page, and the same users can be
        // associated again, each as a NEW association.
        register(&h, &registration(&provider, row.products[0]))
            .await
            .ok(&format!("{case}: registering the directory anew"));
        let listed = listed_usernames(&h, &provider, &instance_id, case).await;
        assert_eq!(
            listed,
            Vec::<String>::new(),
            "case {case:?}: a directory registered anew starts with no association, got \
             {listed:?}"
        );
        for username in row.usernames {
            let reply = associate(
                &h,
                &json!({
                    "Username": username,
                    "InstanceId": instance_id,
                    "IdentityProvider": provider,
                }),
            )
            .await;
            let created = reply.ok(&format!("{case}: re-associating {username}"));
            let summary = created["InstanceUserSummary"].clone();
            assert_eq!(
                as_str(&summary, "Username", case),
                *username,
                "case {case:?}: expected the re-associated user, got {summary:?}"
            );
        }
        let listed = listed_usernames(&h, &provider, &instance_id, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the re-made associations are the ones reported, got {listed:?}"
        );
        every_instance_in_view_provides_a_product(&h, case).await;
    }
}

/// RULE: the teardown is scoped to the directory that lost its registrations — an instance
/// associated through two directories keeps the associations (and the products) of the
/// directory that is still registered, and stays in the view on them.
#[tokio::test]
async fn only_the_associations_of_the_deregistered_directory_are_torn_down() {
    let h = harness();
    let instance_id = held_instance(0);
    // Two directories, each registered for a product of its own, each with a user
    // associated through it on the SAME instance, so a teardown that took too much would
    // show up as a missing association and a missing product.
    let rows = [
        ("d-0123456789", PRODUCTS[0], "alice"),
        ("d-00000000ff", PRODUCTS[1], "bob"),
    ];
    for (directory_id, product, username) in rows {
        register(&h, &registration(&provider_union(directory_id), product))
            .await
            .ok(directory_id);
        associate(
            &h,
            &json!({
                "Username": username,
                "InstanceId": instance_id,
                "IdentityProvider": provider_union(directory_id),
            }),
        )
        .await
        .ok(username);
    }

    // Deregistering the FIRST directory's only registration takes its association and
    // nothing else.
    deregister(&h, &deregistration(&provider_union(rows[0].0), rows[0].1))
        .await
        .ok("deregistering the first directory");

    let case = "one directory of two deregistered";
    let reply = list_associations(&h, &provider_union(rows[0].0), &instance_id).await;
    assert_eq!(
        reply.status, 404,
        "case {case:?}: the deregistered directory has no associations to report, got {reply:?}"
    );
    let surviving = listed_usernames(&h, &provider_union(rows[1].0), &instance_id, case).await;
    assert_eq!(
        surviving,
        vec![rows[1].2.to_string()],
        "case {case:?}: the registered directory keeps its association, got {surviving:?}"
    );
    assert_eq!(
        held_usernames(&h, &instance_id),
        vec![rows[1].2.to_string()],
        "case {case:?}: the store holds exactly the surviving association"
    );
    let in_view = instances_in_view(&h, case).await;
    assert_eq!(
        in_view,
        vec![(instance_id.clone(), vec![rows[1].1.to_string()])],
        "case {case:?}: the instance stays in view providing the surviving directory's \
         product, got {in_view:?}"
    );

    // And the second directory's deregistration takes the last association with it, so the
    // instance leaves the view.
    deregister(&h, &deregistration(&provider_union(rows[1].0), rows[1].1))
        .await
        .ok("deregistering the second directory");
    let case = "both directories deregistered";
    assert_eq!(
        held_usernames(&h, &instance_id),
        Vec::<String>::new(),
        "case {case:?}: no association outlives the last registration of its directory"
    );
    let in_view = instances_in_view(&h, case).await;
    assert!(
        in_view.is_empty(),
        "case {case:?}: the instance must leave the view with its last association, got \
         {in_view:?}"
    );
}
