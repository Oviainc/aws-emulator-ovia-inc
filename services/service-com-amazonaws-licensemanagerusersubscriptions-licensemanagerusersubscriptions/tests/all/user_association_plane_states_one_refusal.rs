// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE sentence the user-association plane states an identity
//! provider with no live registration with, driven over the real wire path: every call
//! goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one recorded request:
//! an operation of the user-association plane (`AssociateUser`, `ListUserAssociations`)
//! that names an identity provider no LIVE registration of the account is for answers its
//! own modelled `ResourceNotFoundException` (HTTP 404) carrying that plane's one sentence
//! — whichever path inside the handler establishes the fact, the branch that resolves the
//! registration itself or a state-layer lookup that reports it — and never the sentence
//! the REGISTRATION plane states an unresolvable provider reference with
//! (`ValidationException` / "Identity provider is not valid", which
//! `tests/all/resource_resolution_refusals.rs` pins for that plane).
//!
//! `DisassociateUser` is not in the table: it resolves the association a request
//! addresses, not the registration of a directory, so it never establishes this fact (see
//! `src/ops/disassociate_user.rs`).
//!
//! The rows differ along what the rule generalises over: the form the provider union
//! addresses the directory by, why no live registration is for it (never registered, its
//! only registration deregistered, another directory registered instead) and the operation
//! that is refused. The harness (router + engine + the account's directory, network and
//! instance fleets) is the one in [`super::identity_provider_subscriptions`]; only the
//! rule is new here, and so are the request builders and the directory forms: those come
//! from [`super::associations_follow_their_registration`], the suite that already drives
//! this plane over every provider form.

use serde_json::json;

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    IDENTITY_PROVIDER_NOT_REGISTERED, IDENTITY_PROVIDER_NOT_VALID,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service, and one copy of each request builder: these come from the
// sibling suites rather than a second copy of the same plumbing.
use crate::associations_follow_their_registration::{
    deregistration, list_associations, registration, Directory,
};
use crate::identity_provider_subscriptions::{
    deregister, harness, register, Harness, Reply, PRODUCTS,
};
use crate::resource_resolution_refusals::{associate, held_instance};

/// Why the directory a row names has no live registration.
#[derive(Debug, Clone, Copy)]
enum NoRegistration {
    /// Nothing was ever registered for the directory.
    NeverRegistered,
    /// The directory's only registration was deregistered, so it is on its way out and no
    /// new work may be built on it.
    Deregistered(&'static str),
    /// Another directory is registered, and this one is not: the resolution is scoped to
    /// the directory the request names.
    AnotherDirectoryRegistered {
        other: Directory,
        product: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    directory: Directory,
    reason: NoRegistration,
    /// Index into the account's seeded EC2 fleet, so a row names an instance the
    /// authorization gate holds without hard-coding its id: the refusal under test is the
    /// one that comes AFTER the instance is authorized.
    instance: usize,
    /// The user the `AssociateUser` row would associate.
    username: &'static str,
    /// The Active Directory domain the user's information comes from, where the row names
    /// one: part of which user is named, so it is varied too.
    domain: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "an AWS-managed directory of the account that was never registered",
        directory: Directory::AwsManaged("d-123456789a"),
        reason: NoRegistration::NeverRegistered,
        instance: 0,
        username: "alice",
        domain: None,
    },
    Row {
        case: "a directory shared into the account that was never registered",
        directory: Directory::SharedIntoAccount("sd-abcdef0123"),
        reason: NoRegistration::NeverRegistered,
        instance: 1,
        username: "bob.admin",
        domain: Some("shared.example"),
    },
    Row {
        case: "a self-managed directory named by its domain that was never registered",
        directory: Directory::SelfManaged("self.managed.example"),
        reason: NoRegistration::NeverRegistered,
        instance: 2,
        username: "CAROL",
        domain: Some("self.managed.example"),
    },
    Row {
        case: "an AWS-managed directory whose only registration was deregistered",
        directory: Directory::AwsManaged("d-0123456789"),
        reason: NoRegistration::Deregistered(PRODUCTS[0]),
        instance: 0,
        username: "dave",
        domain: None,
    },
    Row {
        case: "a self-managed directory whose only registration was deregistered",
        directory: Directory::SelfManaged("torn.down.example"),
        reason: NoRegistration::Deregistered(PRODUCTS[4]),
        instance: 1,
        username: "erin",
        domain: Some("torn.down.example"),
    },
    Row {
        case: "a directory that is not the one registered",
        directory: Directory::AwsManaged("d-123456789a"),
        reason: NoRegistration::AnotherDirectoryRegistered {
            other: Directory::AwsManaged("d-00000000ff"),
            product: PRODUCTS[2],
        },
        instance: 2,
        username: "frank",
        domain: Some("other.example"),
    },
];

/// The refusal every operation of the user-association plane answers a directory with no
/// live registration with: the modelled `ResourceNotFoundException` at HTTP 404, carrying
/// that plane's one sentence and not the registration plane's.
fn assert_provider_not_registered(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected the modelled ResourceNotFoundException at HTTP 404, got \
         {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.message(),
        Some(IDENTITY_PROVIDER_NOT_REGISTERED),
        "case {case:?}: expected {IDENTITY_PROVIDER_NOT_REGISTERED:?}, got {:?}",
        reply.message()
    );
}

/// The two planes state this refusal with DIFFERENT sentences, which is the whole point of
/// asserting on the user-association plane's one: a change that collapsed them would make
/// every assertion above vacuous.
#[test]
fn the_two_planes_state_this_refusal_differently() {
    assert_ne!(
        IDENTITY_PROVIDER_NOT_REGISTERED, IDENTITY_PROVIDER_NOT_VALID,
        "the user-association plane's sentence is not the registration plane's, got \
         {IDENTITY_PROVIDER_NOT_REGISTERED:?} and {IDENTITY_PROVIDER_NOT_VALID:?}"
    );
}

/// How many user associations the STORE holds, read through the engine the handlers write:
/// a refused association must mint nothing.
fn held_association_count(h: &Harness) -> usize {
    let store = h.engine.state::<ServiceState>();
    let guard = store.lock();
    guard.instance_users.len()
}

/// RULE: an operation of the user-association plane that names an identity provider no
/// live registration of the account is for answers its own modelled
/// `ResourceNotFoundException` carrying that plane's one sentence, whichever path in the
/// handler establishes it, and mints nothing.
#[tokio::test]
async fn the_user_association_plane_words_an_unregistered_directory_one_way() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        let provider = row.directory.union();
        let instance_id = held_instance(row.instance);

        match row.reason {
            NoRegistration::NeverRegistered => {}
            NoRegistration::Deregistered(product) => {
                register(&h, &registration(&provider, product))
                    .await
                    .ok(&format!("{case}: registering {product}"));
                deregister(&h, &deregistration(&provider, product))
                    .await
                    .ok(&format!("{case}: deregistering {product}"));
            }
            NoRegistration::AnotherDirectoryRegistered { other, product } => {
                register(&h, &registration(&other.union(), product))
                    .await
                    .ok(&format!("{case}: registering another directory"));
            }
        }

        let mut body = json!({
            "Username": row.username,
            "InstanceId": instance_id,
            "IdentityProvider": provider,
        });
        if let Some(domain) = row.domain {
            body["Domain"] = json!(domain);
        }
        let reply = associate(&h, &body).await;
        assert_provider_not_registered(&format!("AssociateUser: {case}"), &reply);
        assert_eq!(
            held_association_count(&h),
            0,
            "case {case:?}: a refused AssociateUser must mint no association"
        );

        let reply = list_associations(&h, &provider, &instance_id).await;
        assert_provider_not_registered(&format!("ListUserAssociations: {case}"), &reply);
    }
}

/// RULE, from the other side: once the directory the request names DOES have a live
/// registration, the same two calls are served — so the refusal above reports the absence
/// of a registration rather than anything about the request's shape.
#[tokio::test]
async fn the_same_calls_are_served_once_the_directory_is_registered() {
    for row in ROWS {
        let case = row.case;
        let h = harness();
        let provider = row.directory.union();
        let instance_id = held_instance(row.instance);
        // One registration of the directory the row names is all this plane resolves: an
        // association carries no `Product`.
        register(&h, &registration(&provider, PRODUCTS[1]))
            .await
            .ok(&format!("{case}: registering the row's directory"));

        let mut body = json!({
            "Username": row.username,
            "InstanceId": instance_id,
            "IdentityProvider": provider,
        });
        if let Some(domain) = row.domain {
            body["Domain"] = json!(domain);
        }
        let reply = associate(&h, &body).await;
        let created = reply.ok(&format!("AssociateUser: {case}"));
        assert_eq!(
            created["InstanceUserSummary"]["Username"].as_str(),
            Some(row.username),
            "case {case:?}: expected the associated user, got {:?}",
            created["InstanceUserSummary"]
        );

        let reply = list_associations(&h, &provider, &instance_id).await;
        let listed = reply.ok(&format!("ListUserAssociations: {case}"));
        let usernames: Vec<&str> = listed["InstanceUserSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no InstanceUserSummaries in {listed:?}"))
            .iter()
            .filter_map(|summary| summary["Username"].as_str())
            .collect();
        assert_eq!(
            usernames,
            vec![row.username],
            "case {case:?}: expected the association just made, got {usernames:?}"
        );
    }
}
