// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a request that addresses a resource by its ARN and ALSO names the
//! members that identify it, driven over the real wire path: every call goes through the
//! router's `dispatch`, so the request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! the members that identify a resource are what SAY which resource a request means, so a
//! request that supplies an ARN and one of those members too addresses only the resource
//! whose stored value that member is — one naming a member of another resource names no
//! resource state holds and is refused, by the refusal that addressing already has, with
//! nothing written. A member the request leaves out names nothing and so narrows nothing.
//!
//! What this suite pins, over each plane of this service that admits both addressings and
//! over every identity member of it:
//!
//! * `DisassociateUser` by `InstanceUserArn` beside a contradicting `InstanceId`,
//!   `Username`, `IdentityProvider` or `Domain` fails closed and tears nothing down — the
//!   association the ARN names is left alone, and so is the association the contradicting
//!   member names, on an instance whose ownership this call's gate was never asked about;
//! * `StopProductSubscription` by `ProductUserArn` beside a contradicting `Username`,
//!   `Product`, `IdentityProvider` or `Domain` is told that user has no such subscription,
//!   and both subscriptions stay subscribed;
//! * `DeregisterIdentityProvider` and `UpdateIdentityProviderSettings` by
//!   `IdentityProviderArn` beside a contradicting `IdentityProvider` or `Product` are told
//!   the identity provider is not valid, and the registration stays live;
//! * the same request with every member AGREEING with the stored resource is served, which
//!   is what makes the refusals above about the contradiction rather than about supplying
//!   the members at all;
//! * the empty `Domain` names the unqualified user, so it contradicts nothing when the
//!   stored resource carries no domain.
//!
//! The read-back of what each refusal did NOT change is `ListUserAssociations`,
//! `ListProductSubscriptions` and `ListIdentityProviders`.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    no_such_subscription, IDENTITY_PROVIDER_NOT_VALID,
};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, deregister, harness, list_providers, list_subscriptions, provider_union, register,
    start_subscription, stop_subscription, Harness, Reply, PRODUCTS,
};
use super::resource_resolution_refusals::{
    assert_fails_closed, associate, disassociate, held_instance, registration,
};

/// One resource of each plane, and the members that identify it.
///
/// The suite registers, associates and subscribes TWO of these whose every identity member
/// differs, so a request naming one member of the one beside the other's ARN contradicts
/// itself in exactly that member and in no other.
#[derive(Debug, Clone, Copy)]
struct Party {
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; the two parties use the two
    /// spellings, so the rule is not keyed to one form of naming a directory.
    directory_id: &'static str,
    product: &'static str,
    username: &'static str,
    domain: Option<&'static str>,
    /// An index into the account's seeded EC2 fleet, so an association on it is authorized.
    instance: usize,
}

const LEFT: Party = Party {
    directory_id: "d-0123456789",
    product: PRODUCTS[0],
    username: "a.user",
    domain: Some("a.example"),
    instance: 0,
};

const RIGHT: Party = Party {
    directory_id: "sd-abcdef0123",
    product: PRODUCTS[4],
    username: "b.user",
    domain: Some("b.example"),
    instance: 1,
};

/// A third association, of a user with NO domain, on LEFT's registration: the unqualified
/// user the empty `Domain` names.
const UNQUALIFIED: Party = Party {
    directory_id: LEFT.directory_id,
    product: LEFT.product,
    username: "c.user",
    domain: None,
    instance: 2,
};

/// The subnet every registration in this suite is made with — one of the account's.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// One member of a resource's identity, as a request spells it. What the rule generalises
/// over: it is about every member that says which resource a request means, not about one
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Member {
    InstanceId,
    Username,
    IdentityProvider,
    Product,
    Domain,
}

impl Member {
    /// The input member's name, as the model spells it (smithy_query: operation
    /// DisassociateUser / StopProductSubscription / DeregisterIdentityProvider, input).
    fn name(self) -> &'static str {
        match self {
            Member::InstanceId => "InstanceId",
            Member::Username => "Username",
            Member::IdentityProvider => "IdentityProvider",
            Member::Product => "Product",
            Member::Domain => "Domain",
        }
    }

    /// `party`'s value for this member, as a request body carries it.
    fn value(self, party: &Party) -> Value {
        match self {
            Member::InstanceId => json!(held_instance(party.instance)),
            Member::Username => json!(party.username),
            Member::IdentityProvider => provider_union(party.directory_id),
            Member::Product => json!(party.product),
            Member::Domain => json!(party
                .domain
                .expect("a row that names a Domain is a party that carries one")),
        }
    }
}

/// The identity members of each plane: the association is identified by the directory, the
/// instance, the user name and the domain; the subscription by the directory, the product,
/// the user name and the domain; the registration by the directory and the product.
const ASSOCIATION_MEMBERS: &[Member] = &[
    Member::InstanceId,
    Member::Username,
    Member::IdentityProvider,
    Member::Domain,
];
const SUBSCRIPTION_MEMBERS: &[Member] = &[
    Member::Username,
    Member::Product,
    Member::IdentityProvider,
    Member::Domain,
];
const REGISTRATION_MEMBERS: &[Member] = &[Member::IdentityProvider, Member::Product];

/// The body that addresses `arn` through `arn_member` and names nothing else.
fn addressed_by_arn(arn_member: &str, arn: &str) -> Value {
    let mut body = json!({});
    body[arn_member] = json!(arn);
    body
}

/// The body that addresses `arn` through `arn_member` and ALSO names `member` with
/// `party`'s value.
fn addressed_by_arn_and(arn_member: &str, arn: &str, member: Member, party: &Party) -> Value {
    let mut body = addressed_by_arn(arn_member, arn);
    body[member.name()] = member.value(party);
    body
}

impl Party {
    /// The `AssociateUser` body for this party.
    fn association(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "InstanceId": held_instance(self.instance),
            "IdentityProvider": provider_union(self.directory_id),
        });
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// The `StartProductSubscription` body for this party.
    fn subscription(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "Product": self.product,
            "IdentityProvider": provider_union(self.directory_id),
        });
        if let Some(domain) = self.domain {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// The `ListUserAssociations` body that reports this party's instance.
    fn list_associations(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory_id),
            "InstanceId": held_instance(self.instance),
        })
    }

    /// The `ListProductSubscriptions` body that reports this party's registration.
    fn list_subscriptions(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
        })
    }
}

/// A harness whose account holds a registration for each party's `(directory, product)`,
/// answered with the `IdentityProviderArn` each one minted.
async fn setup() -> (Harness, String, String) {
    let h = harness();
    let mut minted = Vec::new();
    for party in [LEFT, RIGHT] {
        let reply = register(
            &h,
            &registration(party.directory_id, party.product, &[SUBNET], None),
        )
        .await;
        let summary = reply.ok(party.directory_id)["IdentityProviderSummary"].clone();
        minted.push(as_str(&summary, "IdentityProviderArn", party.directory_id).to_string());
    }
    let right = minted.pop().expect("both registrations were minted");
    let left = minted.pop().expect("both registrations were minted");
    (h, left, right)
}

/// Associate `party`'s user and answer the `InstanceUserArn` it minted.
async fn associate_party(h: &Harness, party: &Party) -> String {
    let reply = associate(h, &party.association()).await;
    let summary = reply.ok(party.username)["InstanceUserSummary"].clone();
    as_str(&summary, "InstanceUserArn", party.username).to_string()
}

/// Subscribe `party`'s user and answer the `ProductUserArn` it minted.
async fn subscribe_party(h: &Harness, party: &Party) -> String {
    let reply = start_subscription(h, &party.subscription()).await;
    let summary = reply.ok(party.username)["ProductUserSummary"].clone();
    as_str(&summary, "ProductUserArn", party.username).to_string()
}

/// The summary `reply`'s list reports for `arn`, or `None` when the list does not report
/// it at all.
fn reported(reply: &Reply, items: &str, member: &str, arn: &str, case: &str) -> Option<Value> {
    let answered = reply.ok(case);
    answered[items]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {items} in {answered:?}"))
        .iter()
        .find(|summary| summary.get(member).and_then(Value::as_str) == Some(arn))
        .cloned()
}

/// Assert the resource `arn` names is still reported, in a status that is not `forbidden`
/// (the status the refused call would have moved it to).
fn assert_untouched(case: &str, who: &str, arn: &str, forbidden: &str, summary: Option<Value>) {
    let summary = summary.unwrap_or_else(|| {
        panic!("case {case:?}: the {who} resource {arn} must still be reported, got nothing")
    });
    let status = as_str(&summary, "Status", case);
    assert_ne!(
        status, forbidden,
        "case {case:?}: a refused call must not move the {who} resource, got {summary:?}"
    );
}

/// RULE: every member of an association's identity a `DisassociateUser` names says which
/// association it means, so an `InstanceUserArn` beside a contradicting `InstanceId`,
/// `Username`, `IdentityProvider` or `Domain` addresses no association state holds: the
/// call fails closed and tears nothing down, on either instance.
#[tokio::test]
async fn an_association_addressed_by_arn_is_only_the_one_its_other_members_name() {
    let (h, _, _) = setup().await;
    let left = associate_party(&h, &LEFT).await;
    let right = associate_party(&h, &RIGHT).await;

    for member in ASSOCIATION_MEMBERS {
        let case = format!(
            "the left association's InstanceUserArn beside the right association's {}",
            member.name()
        );
        let refused = disassociate(
            &h,
            &addressed_by_arn_and("InstanceUserArn", &left, *member, &RIGHT),
        )
        .await;
        assert_fails_closed(&case, &left, &refused);

        // Neither association was touched: not the one the ARN names, and not the one the
        // contradicting member names — whose instance this call's authorization gate was
        // never asked about.
        for (who, party, arn) in [("left", &LEFT, &left), ("right", &RIGHT, &right)] {
            let listed = call(&h, "/user/ListUserAssociations", &party.list_associations()).await;
            let summary = reported(&listed, "InstanceUserSummaries", "InstanceUserArn", arn, &case);
            assert!(
                summary
                    .as_ref()
                    .is_some_and(|summary| summary.get("DisassociationDate").is_none()),
                "case {case:?}: the {who} association must carry no disassociation date, got \
                 {summary:?}"
            );
            assert_untouched(&case, who, arn, "DISASSOCIATING", summary);
        }
    }

    // The same request with every member AGREEING with the stored association is served:
    // what the refusals above answer is the contradiction, not the members themselves.
    let case = "every member of the left association, beside its ARN";
    let mut body = addressed_by_arn("InstanceUserArn", &left);
    for member in ASSOCIATION_MEMBERS {
        body[member.name()] = member.value(&LEFT);
    }
    let accepted = disassociate(&h, &body).await;
    let summary = accepted.ok(case)["InstanceUserSummary"].clone();
    assert_eq!(
        as_str(&summary, "InstanceUserArn", case),
        left,
        "case {case:?}: the agreeing request must end the association it names, got {summary:?}"
    );
    assert_eq!(
        as_str(&summary, "Status", case),
        "DISASSOCIATING",
        "case {case:?}: expected the non-terminal teardown status, got {summary:?}"
    );

    // And the right association, on the instance none of those calls named, is still live.
    let listed = call(&h, "/user/ListUserAssociations", &RIGHT.list_associations()).await;
    let still = reported(&listed, "InstanceUserSummaries", "InstanceUserArn", &right, case);
    assert_untouched(case, "right", &right, "DISASSOCIATING", still);
}

/// RULE: the empty `Domain` names the unqualified user, so it contradicts nothing when the
/// association the ARN names carries no domain — while a domain that association's user is
/// not in does, exactly as any other contradicting member.
#[tokio::test]
async fn an_unqualified_association_is_contradicted_by_a_domain_and_not_by_the_empty_one() {
    let (h, _, _) = setup().await;
    let arn = associate_party(&h, &UNQUALIFIED).await;

    let case = "an association with no domain, beside a Domain its user is not in";
    let mut body = addressed_by_arn("InstanceUserArn", &arn);
    body["Domain"] = json!(LEFT
        .domain
        .expect("the left party carries a domain to contradict with"));
    assert_fails_closed(case, &arn, &disassociate(&h, &body).await);

    let case = "an association with no domain, beside the empty Domain";
    let mut body = addressed_by_arn("InstanceUserArn", &arn);
    body["Domain"] = json!("");
    let accepted = disassociate(&h, &body).await;
    let summary = accepted.ok(case)["InstanceUserSummary"].clone();
    assert_eq!(
        as_str(&summary, "InstanceUserArn", case),
        arn,
        "case {case:?}: the empty Domain names the unqualified user, so it narrows nothing, \
         got {summary:?}"
    );
}

/// RULE: the same matching on the subscription plane — a `ProductUserArn` beside a
/// contradicting `Username`, `Product`, `IdentityProvider` or `Domain` addresses no
/// subscription state holds, so the caller is told the user it named has none and both
/// subscriptions stay subscribed.
#[tokio::test]
async fn a_subscription_addressed_by_arn_is_only_the_one_its_other_members_name() {
    let (h, _, _) = setup().await;
    let left = subscribe_party(&h, &LEFT).await;
    let right = subscribe_party(&h, &RIGHT).await;

    for member in SUBSCRIPTION_MEMBERS {
        let case = format!(
            "the left subscription's ProductUserArn beside the right subscription's {}",
            member.name()
        );
        let refused = stop_subscription(
            &h,
            &addressed_by_arn_and("ProductUserArn", &left, *member, &RIGHT),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: ResourceNotFoundException is modelled at HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        // The sentence is about what the request ASKED FOR: the members it named, and the
        // stored subscription's values for the members it left out.
        let expected = no_such_subscription(
            if *member == Member::Username {
                RIGHT.username
            } else {
                LEFT.username
            },
            if *member == Member::Product {
                RIGHT.product
            } else {
                LEFT.product
            },
            if *member == Member::Domain {
                RIGHT.domain
            } else {
                LEFT.domain
            },
        );
        assert_eq!(
            refused.message(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {refused:?}"
        );

        for (who, party, arn) in [("left", &LEFT, &left), ("right", &RIGHT, &right)] {
            let listed = list_subscriptions(&h, &party.list_subscriptions()).await;
            let summary = reported(&listed, "ProductUserSummaries", "ProductUserArn", arn, &case);
            assert!(
                summary
                    .as_ref()
                    .is_some_and(|summary| summary.get("SubscriptionEndDate").is_none()),
                "case {case:?}: the {who} subscription must carry no end date, got {summary:?}"
            );
            assert_untouched(&case, who, arn, "UNSUBSCRIBED", summary);
        }
    }

    let case = "every member of the left subscription, beside its ARN";
    let mut body = addressed_by_arn("ProductUserArn", &left);
    for member in SUBSCRIPTION_MEMBERS {
        body[member.name()] = member.value(&LEFT);
    }
    let accepted = stop_subscription(&h, &body).await;
    let summary = accepted.ok(case)["ProductUserSummary"].clone();
    assert_eq!(
        as_str(&summary, "ProductUserArn", case),
        left,
        "case {case:?}: the agreeing request must end the subscription it names, got {summary:?}"
    );
    assert_eq!(
        as_str(&summary, "Status", case),
        "UNSUBSCRIBED",
        "case {case:?}: expected the ended status, got {summary:?}"
    );

    let listed = list_subscriptions(&h, &RIGHT.list_subscriptions()).await;
    let still = reported(&listed, "ProductUserSummaries", "ProductUserArn", &right, case);
    assert_untouched(case, "right", &right, "UNSUBSCRIBED", still);
}

/// RULE: the same matching on the registration plane, for every operation that addresses a
/// registration — an `IdentityProviderArn` beside a contradicting `IdentityProvider` or
/// `Product` is not one of the account's registrations, so `DeregisterIdentityProvider` and
/// `UpdateIdentityProviderSettings` both answer that the identity provider is not valid and
/// the registration stays live.
#[tokio::test]
async fn a_registration_addressed_by_arn_is_only_the_one_its_other_members_name() {
    let (h, left, right) = setup().await;

    for member in REGISTRATION_MEMBERS {
        let case = format!(
            "the left registration's IdentityProviderArn beside the right registration's {}",
            member.name()
        );
        let deregistered = deregister(
            &h,
            &addressed_by_arn_and("IdentityProviderArn", &left, *member, &RIGHT),
        )
        .await;
        let mut update = addressed_by_arn_and("IdentityProviderArn", &left, *member, &RIGHT);
        update["UpdateSettings"] = json!({ "AddSubnets": [SUBNET], "RemoveSubnets": [] });
        let updated = call(
            &h,
            "/identity-provider/UpdateIdentityProviderSettings",
            &update,
        )
        .await;

        for (operation, reply) in [
            ("DeregisterIdentityProvider", &deregistered),
            ("UpdateIdentityProviderSettings", &updated),
        ] {
            assert_eq!(
                reply.status, 400,
                "case {case:?}: {operation} models ValidationException at HTTP 400, got {reply:?}"
            );
            assert_eq!(
                reply.error_type.as_deref(),
                Some("ValidationException"),
                "case {case:?}: {operation} must answer ValidationException, got {reply:?}"
            );
            assert_eq!(
                reply.message(),
                Some(IDENTITY_PROVIDER_NOT_VALID),
                "case {case:?}: {operation} must carry the service's sentence for an \
                 unresolvable identity provider, got {reply:?}"
            );
        }

        let listed = list_providers(&h, &json!({})).await;
        for (who, arn) in [("left", &left), ("right", &right)] {
            let summary = reported(
                &listed,
                "IdentityProviderSummaries",
                "IdentityProviderArn",
                arn,
                &case,
            );
            assert_untouched(&case, who, arn, "DEREGISTERING", summary);
        }
    }

    let case = "every member of the left registration, beside its ARN";
    let mut body = addressed_by_arn("IdentityProviderArn", &left);
    for member in REGISTRATION_MEMBERS {
        body[member.name()] = member.value(&LEFT);
    }
    let accepted = deregister(&h, &body).await;
    let summary = accepted.ok(case)["IdentityProviderSummary"].clone();
    assert_eq!(
        as_str(&summary, "IdentityProviderArn", case),
        left,
        "case {case:?}: the agreeing request must deregister the registration it names, got \
         {summary:?}"
    );
    assert_eq!(
        as_str(&summary, "Status", case),
        "DEREGISTERING",
        "case {case:?}: expected the non-terminal teardown status, got {summary:?}"
    );

    let listed = list_providers(&h, &json!({})).await;
    let still = reported(
        &listed,
        "IdentityProviderSummaries",
        "IdentityProviderArn",
        &right,
        case,
    );
    assert_untouched(case, "right", &right, "DEREGISTERING", still);
}
