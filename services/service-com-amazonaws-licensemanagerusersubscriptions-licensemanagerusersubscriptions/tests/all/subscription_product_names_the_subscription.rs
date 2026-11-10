// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `Product` half of a product user's identity on
//! `StopProductSubscription`, driven over the real wire path: every call goes through the
//! router's `dispatch`, so the request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! `Product` is part of which subscription a request names, so a request that addresses one
//! by `ProductUserArn` and ALSO names a `Product` addresses only the subscription whose
//! stored product that is — a request naming another product names no subscription state
//! holds and is refused, with nothing torn down.
//!
//! `arn_and_members_address_one_resource` pins that rule for every identity member at once,
//! each contradicted by a resource in ANOTHER directory. This suite pins the one
//! configuration that rules out the other reading of those refusals, where the product
//! named is refused because no registration holds it: here EVERY documented product is
//! registered for the SAME directory and the SAME user holds a live subscription to each,
//! so the only thing that can tell the subscriptions apart is the product itself. Every
//! ordered pair of distinct products is a row, and the diagonal — the product the stored
//! subscription actually carries — is the row that is served, which is what makes the
//! refusals about the contradiction rather than about naming `Product` at all.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::no_such_subscription;

// ONE wire harness per service: the dispatch plumbing and the provider-registration
// helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, harness, list_subscriptions, provider_union, register, start_subscription,
    stop_subscription, Harness, PRODUCTS,
};
use super::resource_resolution_refusals::registration;

/// The ONE directory every registration in this suite is made for: what the rule is about
/// is the product, so everything else about the subscriptions is held equal.
const DIRECTORY: &str = "d-0123456789";

/// The ONE user every subscription in this suite is for, for the same reason.
const USERNAME: &str = "u1";

/// The subnet every registration in this suite is made with — one of the account's.
const SUBNET: &str = "subnet-0123456789abcdef0";

/// The `Domain` each row's subscription is stored with, by the row's index into
/// [`PRODUCTS`]: the rule holds for a domain-qualified user and for an unqualified one
/// alike, so the table spans both rather than only the form the report happened to use.
fn domain_of(index: usize) -> Option<&'static str> {
    if index.is_multiple_of(2) {
        None
    } else {
        Some("corp.example")
    }
}

/// A harness whose account holds a registration for `(DIRECTORY, product)` for EVERY
/// documented product, and a live subscription of [`USERNAME`] to each one.
///
/// Answers the `ProductUserArn` each subscription minted, indexed like [`PRODUCTS`].
async fn setup() -> (Harness, Vec<String>) {
    let h = harness();
    let mut arns = Vec::new();
    for (index, product) in PRODUCTS.iter().enumerate() {
        let case = format!("registering {product} for {DIRECTORY}");
        register(&h, &registration(DIRECTORY, product, &[SUBNET], None))
            .await
            .ok(&case);

        let mut body = json!({
            "Username": USERNAME,
            "Product": product,
            "IdentityProvider": provider_union(DIRECTORY),
        });
        if let Some(domain) = domain_of(index) {
            body["Domain"] = json!(domain);
        }
        let case = format!("subscribing {USERNAME} to {product}");
        let reply = start_subscription(&h, &body).await;
        let summary = reply.ok(&case)["ProductUserSummary"].clone();
        assert_eq!(
            as_str(&summary, "Product", &case),
            *product,
            "case {case:?}: the subscription must be stored for the product named, got \
             {summary:?}"
        );
        arns.push(as_str(&summary, "ProductUserArn", &case).to_string());
    }
    (h, arns)
}

/// Assert every subscription this suite started is still reported and still live: no
/// `SubscriptionEndDate`, and a status that is not the ended one.
async fn assert_all_still_subscribed(h: &Harness, case: &str) {
    for (index, product) in PRODUCTS.iter().enumerate() {
        let listed = list_subscriptions(
            h,
            &json!({ "IdentityProvider": provider_union(DIRECTORY), "Product": product }),
        )
        .await;
        let answered = listed.ok(case);
        let summaries = answered["ProductUserSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no ProductUserSummaries in {answered:?}"))
            .clone();
        let summary = summaries
            .iter()
            .find(|summary| summary.get("Username").and_then(Value::as_str) == Some(USERNAME))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the {product} subscription of {USERNAME} must still be \
                     reported, got {summaries:?}"
                )
            });
        assert_eq!(
            summary.get("Domain").and_then(Value::as_str),
            domain_of(index),
            "case {case:?}: the {product} subscription must keep the domain it was started \
             in, got {summary:?}"
        );
        assert!(
            summary.get("SubscriptionEndDate").is_none(),
            "case {case:?}: the {product} subscription must carry no end date, got {summary:?}"
        );
        assert_ne!(
            as_str(summary, "Status", case),
            "UNSUBSCRIBED",
            "case {case:?}: a refused stop must not end the {product} subscription, got \
             {summary:?}"
        );
    }
}

/// RULE: `Product` is part of which subscription a `StopProductSubscription` names, so a
/// `ProductUserArn` beside a `Product` that is not the stored subscription's addresses no
/// subscription state holds — even when that product is registered for the same directory
/// and the same user holds a live subscription to it — and nothing is torn down.
#[tokio::test]
async fn a_subscription_addressed_by_arn_is_only_the_one_the_named_product_is_stored_for() {
    let (h, arns) = setup().await;

    for (stored, stored_product) in PRODUCTS.iter().enumerate() {
        for named_product in PRODUCTS.iter().filter(|p| *p != stored_product) {
            let case = format!(
                "the {stored_product} subscription's ProductUserArn beside Product \
                 {named_product}"
            );
            let refused = stop_subscription(
                &h,
                &json!({ "ProductUserArn": arns[stored], "Product": named_product }),
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
            // The sentence is about what the request ASKED FOR: the product it named, and
            // the stored subscription's values for the members it left out.
            let expected =
                no_such_subscription(USERNAME, named_product, domain_of(stored));
            assert_eq!(
                refused.message(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {refused:?}"
            );

            assert_all_still_subscribed(&h, &case).await;
        }
    }
}

/// RULE, the other way round: the same request whose `Product` AGREES with the stored
/// subscription is served, so what the refusals above answer is the contradiction and not
/// the presence of `Product` beside the ARN.
#[tokio::test]
async fn a_subscription_addressed_by_arn_and_its_own_product_is_the_one_that_is_ended() {
    let (h, arns) = setup().await;

    for (stored, stored_product) in PRODUCTS.iter().enumerate() {
        let case = format!("the {stored_product} subscription's ProductUserArn beside its own Product");
        let accepted = stop_subscription(
            &h,
            &json!({ "ProductUserArn": arns[stored], "Product": stored_product }),
        )
        .await;
        let summary = accepted.ok(&case)["ProductUserSummary"].clone();
        assert_eq!(
            as_str(&summary, "ProductUserArn", &case),
            arns[stored],
            "case {case:?}: the agreeing request must end the subscription it names, got \
             {summary:?}"
        );
        assert_eq!(
            as_str(&summary, "Product", &case),
            *stored_product,
            "case {case:?}: the answer must report the product the subscription is stored \
             for, got {summary:?}"
        );
        assert_eq!(
            as_str(&summary, "Status", &case),
            "UNSUBSCRIBED",
            "case {case:?}: expected the ended status, got {summary:?}"
        );
        assert_eq!(
            summary.get("Domain").and_then(Value::as_str),
            domain_of(stored),
            "case {case:?}: the answer must report the domain the subscription was started \
             in, got {summary:?}"
        );
        assert!(
            summary.get("SubscriptionEndDate").is_some(),
            "case {case:?}: an ended subscription carries the end date of this call, got \
             {summary:?}"
        );

        // A second stop of the same subscription resolves nothing: the end was already
        // accepted, so the ARN names no live subscription of this account and the call
        // fails closed rather than disclosing that it resolves to nothing.
        let case = format!("stopping the {stored_product} subscription twice");
        let repeated = stop_subscription(
            &h,
            &json!({ "ProductUserArn": arns[stored], "Product": stored_product }),
        )
        .await;
        assert_eq!(
            (repeated.status, repeated.error_type.as_deref()),
            (401, Some("UnauthorizedException")),
            "case {case:?}: expected the fail-closed refusal for a subscription already \
             ended, got {repeated:?}"
        );
    }
}
