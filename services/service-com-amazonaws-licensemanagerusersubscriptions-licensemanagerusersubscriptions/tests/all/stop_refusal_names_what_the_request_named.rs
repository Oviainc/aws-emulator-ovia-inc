// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT a `StopProductSubscription` refusal says when the request
//! addresses no live subscription, driven over the real wire path: every call goes through
//! the router's `dispatch`, so each request is deserialized, constraint-checked, handled
//! and serialized exactly as a client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! a refusal is worded from the identifiers the REQUEST named and no others, so one
//! logical refusal has one sentence per ADDRESSING — a request that named the members that
//! identify a user is told that user has no subscription to that product, and a request
//! that supplied a `ProductUserArn` is told that ARN has no subscription — and neither
//! sentence depends on how far a lookup inside the service got, so neither changes with
//! how much of the state the clock owes has been materialised, and neither reports an
//! identifier this service minted and the caller never supplied.
//!
//! The three states a stop can meet are driven for every row, because the rule says the
//! answer does not distinguish them: the identity was never subscribed, its subscription
//! has ENDED and is still being torn down (the identity stays occupied, so a lookup inside
//! the service still reaches the ending subscription), and the teardown the clock owed has
//! completed (nothing is held for the identity at all).
//!
//! The expected sentences are spelled out here rather than read from the production
//! helpers, so a change to either of them fails a test. The wire harness, the signing
//! identity and the registration helper are the shared ones.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    arn_prefix, as_str, harness, provider_union, register, start_subscription, stop_subscription,
    Harness, Reply, PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use super::resource_resolution_refusals::registration;

/// The directories the shared harness puts in the account's fleet; a row runs through the
/// `(directory, product)` of its own index, so no row's outcome is keyed to one
/// registration.
const DIRECTORIES: &[&str] = &["d-0123456789", "d-00000000ff", "d-123456789a"];

/// One subscription to end, and the `Domain` the member-addressed stop that follows names.
///
/// `Username` and `Domain` are plain `String`s in this service's model (no pattern to
/// honour), so the rows vary in FORM: a plain name, a upn-shaped one, a punctuated one and
/// an upper-case one, against a stored domain, none, a sibling domain and the empty value.
/// Whether the named identity is the stored subscription's varies too, because the rule is
/// the same sentence either way.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    username: &'static str,
    /// The `Domain` the `StartProductSubscription` carries, or `None` when it carries none.
    stored: Option<&'static str>,
    /// The `Domain` the member-addressed stops carry, or `None` when they carry none.
    named: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "no domain stored, none named: the identity the index still holds",
        username: "alice",
        stored: None,
        named: None,
    },
    Row {
        case: "a domain stored and named again: the qualified identity it holds",
        username: "bob",
        stored: Some("corp.example"),
        named: Some("corp.example"),
    },
    Row {
        case: "a domain stored, none named: an addressing that narrows nothing",
        username: "carol@corp.example",
        stored: Some("example.com"),
        named: None,
    },
    Row {
        case: "no domain stored, a domain named: another user of that name",
        username: "dave.o-brien",
        stored: None,
        named: Some("ad.example.internal"),
    },
    Row {
        case: "a domain stored, a sibling domain named: another user again",
        username: "ERIN",
        stored: Some("a.example"),
        named: Some("b.example"),
    },
    Row {
        case: "a domain stored, the empty value named, which names the unqualified user",
        username: "frank",
        stored: Some("corp.example"),
        named: Some(""),
    },
];

impl Row {
    fn index(&self) -> usize {
        ROWS.iter()
            .position(|row| row.case == self.case)
            .unwrap_or_else(|| panic!("case {:?}: every row is in the table", self.case))
    }

    fn directory(&self) -> &'static str {
        DIRECTORIES[self.index() % DIRECTORIES.len()]
    }

    fn product(&self) -> &'static str {
        PRODUCTS[self.index() % PRODUCTS.len()]
    }

    /// The `StartProductSubscription` body: the user as STORED, with the row's stored
    /// `Domain` left out of the body entirely when the row stores none.
    fn start_request(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "IdentityProvider": provider_union(self.directory()),
            "Product": self.product(),
        });
        if let Some(domain) = self.stored {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// A `StopProductSubscription` body addressed by the members that identify a user,
    /// carrying the `Domain` this row NAMES.
    fn member_stop_request(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "IdentityProvider": provider_union(self.directory()),
            "Product": self.product(),
        });
        if let Some(domain) = self.named {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// The sentence a member-addressed stop is refused with: the user and product the
    /// REQUEST named, qualified by the domain it named when that names a domain at all (the
    /// empty value names the unqualified user, so it qualifies nothing).
    fn expected_sentence(&self) -> String {
        match self.named.filter(|domain| !domain.is_empty()) {
            Some(domain) => format!(
                "user {} has no subscription to product {} in domain {domain}",
                self.username,
                self.product()
            ),
            None => format!(
                "user {} has no subscription to product {}",
                self.username,
                self.product()
            ),
        }
    }
}

/// The sentence a stop addressed by a `ProductUserArn` is refused with: the ARN the
/// REQUEST supplied.
fn expected_arn_sentence(product_user_arn: &str) -> String {
    format!("product user {product_user_arn} has no subscription")
}

/// A harness whose account holds a registration for this row's `(directory, product)`.
async fn setup(row: &Row) -> Harness {
    let h = harness();
    register(
        &h,
        &registration(
            row.directory(),
            row.product(),
            &["subnet-0123456789abcdef0"],
            None,
        ),
    )
    .await
    .ok(row.case);
    h
}

/// Assert one refusal: `ResourceNotFoundException` at HTTP 404 carrying exactly
/// `expected`, and naming no identifier beyond the ones `named` lists.
fn assert_refusal(case: &str, reply: &Reply, expected: &str, named: &[&str]) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: StopProductSubscription models ResourceNotFoundException at HTTP \
         404, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(expected),
        "case {case:?}: expected the refusal {expected:?}, got {reply:?}"
    );
    let message = reply.message().unwrap_or_default();
    if let Some(fragment) = message.split_whitespace().find(|word| {
        word.starts_with("arn:") && !named.iter().any(|allowed| word.contains(allowed))
    }) {
        panic!(
            "case {case:?}: the refusal names {fragment:?}, an identifier the request never \
             supplied, in {message:?}"
        );
    }
}

/// The `ProductUserArn` a start minted, as a client reads it off the answer.
fn minted_arn(reply: &Reply, case: &str) -> String {
    as_str(&reply.ok(case)["ProductUserSummary"], "ProductUserArn", case).to_string()
}

/// RULE: a stop addressed by the members that identify a user is refused with the ONE
/// sentence for those members — the user and product the request named, qualified by the
/// domain it named — whether state holds nothing for that identity, holds a subscription
/// whose teardown is still running, or has completed that teardown; the ARN a lookup
/// inside the service reached on the way is not an identifier the request supplied, so no
/// refusal reports it.
#[tokio::test]
async fn a_member_addressed_stop_is_refused_with_one_sentence() {
    for row in ROWS {
        let h = setup(row).await;
        let expected = row.expected_sentence();

        // Nothing is subscribed for this identity yet.
        let case = format!("{}: never subscribed", row.case);
        let never = stop_subscription(&h, &row.member_stop_request()).await;
        assert_refusal(&case, &never, &expected, &[]);

        // One subscription, ended through the ARN it was minted under, so every row
        // reaches the teardown window whether or not the members it names are the stored
        // subscription's.
        let started = start_subscription(&h, &row.start_request()).await;
        let arn = minted_arn(&started, row.case);
        stop_subscription(&h, &json!({ "ProductUserArn": arn }))
            .await
            .ok(row.case);

        // Inside the teardown window: the identity stays occupied, so a lookup inside the
        // service still reaches the ending subscription — which changes nothing about the
        // sentence, and must not put that subscription's ARN in it.
        let case = format!("{}: inside the teardown window", row.case);
        let ending = stop_subscription(&h, &row.member_stop_request()).await;
        assert_refusal(&case, &ending, &expected, &[]);
        assert!(
            !ending.message().unwrap_or_default().contains(&arn),
            "case {case:?}: the refusal must not report {arn:?}, which the request never \
             named, got {ending:?}"
        );

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        // And past the teardown the clock owed, when nothing is held for the identity at
        // all: the same answer, to the byte.
        let case = format!("{}: past the owed teardown", row.case);
        let gone = stop_subscription(&h, &row.member_stop_request()).await;
        assert_refusal(&case, &gone, &expected, &[]);
        assert_eq!(
            (gone.status, gone.error_type.as_deref(), gone.message()),
            (
                ending.status,
                ending.error_type.as_deref(),
                ending.message()
            ),
            "case {case:?}: the refusal cannot depend on how much of the teardown the clock \
             owed has been materialised, got {gone:?} after {ending:?}"
        );
    }
}

/// RULE: the other addressing has its own one sentence, naming the `ProductUserArn` the
/// REQUEST supplied — for an ARN this service never minted, for one whose subscription is
/// still being torn down, and for one whose teardown has completed.
#[tokio::test]
async fn an_arn_addressed_stop_names_the_arn_the_request_supplied() {
    for row in ROWS {
        let h = setup(row).await;

        // An ARN of the modelled form that nothing was ever subscribed under.
        let unknown = format!("{}pu-000000009999", arn_prefix("user"));
        let case = format!("{}: an ARN that was never subscribed", row.case);
        let never = stop_subscription(&h, &json!({ "ProductUserArn": unknown })).await;
        assert_refusal(&case, &never, &expected_arn_sentence(&unknown), &[&unknown]);

        let started = start_subscription(&h, &row.start_request()).await;
        let arn = minted_arn(&started, row.case);
        stop_subscription(&h, &json!({ "ProductUserArn": arn }))
            .await
            .ok(row.case);

        let case = format!("{}: the same ARN inside the teardown window", row.case);
        let ending = stop_subscription(&h, &json!({ "ProductUserArn": arn })).await;
        assert_refusal(&case, &ending, &expected_arn_sentence(&arn), &[&arn]);

        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

        let case = format!("{}: the same ARN past the owed teardown", row.case);
        let gone = stop_subscription(&h, &json!({ "ProductUserArn": arn })).await;
        assert_refusal(&case, &gone, &expected_arn_sentence(&arn), &[&arn]);
        assert_eq!(
            (gone.status, gone.error_type.as_deref(), gone.message()),
            (
                ending.status,
                ending.error_type.as_deref(),
                ending.message()
            ),
            "case {case:?}: the refusal cannot depend on how much of the teardown the clock \
             owed has been materialised, got {gone:?} after {ending:?}"
        );
    }
}
