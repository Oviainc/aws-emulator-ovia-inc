// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE cross-check a record reached by its ARN is put through
//! when the request also names a `Domain`, driven over the real wire path: every call goes
//! through the router's `dispatch`, so each request is deserialized, constraint-checked,
//! handled and serialized exactly as a client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! `Domain` is part of WHICH USER a user-plane request names — it is "the domain name of
//! the Active Directory that contains information for the user" the call acts on
//! (smithy_query: operation StopProductSubscription / DisassociateUser, input `Domain`) —
//! so a record a request reached by ARN is the one the caller asked for only when the
//! domain the request names is the stored user's, while a request that names no domain (or
//! the empty value, which names the same unqualified user) narrows nothing and takes
//! whatever the ARN reached.
//!
//! Both halves of the user plane address a record by ARN, and both put the record through
//! that same comparison: `StopProductSubscription` (which resolves a subscription by
//! `ProductUserArn`) and `DisassociateUser` (which resolves an association by
//! `InstanceUserArn`) are driven over the SAME rows here, so a change to how `Domain` is
//! matched cannot move one without the other. What differs, and is asserted per operation,
//! is only the refusal each surface gives a record of another user: the subscription plane
//! reports the user has no such subscription, the association plane fails CLOSED.
//!
//! The rows are the FORM of the `Domain` value, over both planes, which is why they are
//! their own table beside the two sibling suites that span a different axis of the same
//! rule: `arn_and_members_address_one_resource` varies WHICH identity member an
//! ARN-addressed request contradicts, and `user_domain_names_the_user` names the user by
//! the identity members on both calls, where the question is which record each matches.
//! Here the second call carries only an ARN and a `Domain` whose form varies — a domain,
//! a sibling, a sub-domain, an unrelated one, none, the empty value — and the question is
//! whether the record it reached is the user's. The wire harness, the registration helper
//! and the signing identity are the shared ones.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, harness, provider_union, register, start_subscription, stop_subscription, Harness,
    Reply, PRODUCTS,
};
use super::resource_resolution_refusals::{
    assert_fails_closed, associate, disassociate, held_instance, registration,
};

/// Whether the `Domain` the second (ARN-addressed) call names is the stored record's, so
/// that call resolves the record the first one created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    /// The user the stored record is for: the ARN resolves and the call is served.
    TheStoredUser,
    /// Another user: the ARN reaches a record of a different user, so the call is refused.
    AnotherUser,
}

/// One record to store with a `Domain`, then address by ARN while naming another.
///
/// `Username` and `Domain` are plain `String`s in this service's model (no pattern to
/// honour), so the rows vary in FORM — a domain and none, two sibling domains, a domain and
/// a sub-domain of it, two unrelated domains, the empty value — and each row runs through
/// its own registration `(directory, product)` and instance, so no row's outcome is keyed
/// to one.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    username: &'static str,
    /// The `Domain` the creating call carries, or `None` when it carries none.
    stored: Option<&'static str>,
    /// The `Domain` the ARN-addressed call carries.
    named: Option<&'static str>,
    names: Names,
}

const ROWS: &[Row] = &[
    Row {
        case: "the stored domain, named again",
        username: "alice",
        stored: Some("corp.example"),
        named: Some("corp.example"),
        names: Names::TheStoredUser,
    },
    Row {
        case: "a sibling Active Directory domain",
        username: "bob",
        stored: Some("a.example"),
        named: Some("b.example"),
        names: Names::AnotherUser,
    },
    Row {
        case: "a sub-domain of the stored domain, upn-shaped user name",
        username: "carol@corp.example",
        stored: Some("corp.example"),
        named: Some("eu.corp.example"),
        names: Names::AnotherUser,
    },
    Row {
        case: "an unrelated domain, hyphenated user name",
        username: "dave.o-brien",
        stored: Some("example.com"),
        named: Some("ad.example.internal"),
        names: Names::AnotherUser,
    },
    Row {
        case: "no domain named against a stored domain, which narrows nothing",
        username: "erin",
        stored: Some("corp.example"),
        named: None,
        names: Names::TheStoredUser,
    },
    Row {
        case: "the empty Domain against a stored domain, which narrows nothing either",
        username: "FRANK",
        stored: Some("corp.example"),
        named: Some(""),
        names: Names::TheStoredUser,
    },
    Row {
        case: "a domain named against an unqualified stored user",
        username: "grace",
        stored: None,
        named: Some("corp.example"),
        names: Names::AnotherUser,
    },
    Row {
        case: "the empty Domain against an unqualified stored user, the same user",
        username: "heidi",
        stored: None,
        named: Some(""),
        names: Names::TheStoredUser,
    },
    Row {
        case: "no domain on either call",
        username: "ivan",
        stored: None,
        named: None,
        names: Names::TheStoredUser,
    },
];

/// The directories the shared harness puts in the account's fleet: a row runs through the
/// `(directory, product)` of its own index, so the rule is exercised through more than one
/// registration.
const DIRECTORIES: &[&str] = &["d-0123456789", "d-00000000ff", "d-123456789a"];

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

    /// The instance this row's associations are made on — one of the account's fleet, so
    /// the association is authorized.
    fn instance(&self) -> String {
        held_instance(self.index() % 3)
    }

    /// The members a creating call names the user by, with `Domain` as stored (left out of
    /// the body entirely when the row stores none).
    fn subscription_members(&self) -> Value {
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

    fn association_members(&self) -> Value {
        let mut body = json!({
            "Username": self.username,
            "InstanceId": self.instance(),
            "IdentityProvider": provider_union(self.directory()),
        });
        if let Some(domain) = self.stored {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// A request that addresses the record `arn` names, under the member `identifier`, and
    /// carries the `Domain` this row names (left out when the row names none).
    fn addressed_by_arn(&self, identifier: &str, arn: &str) -> Value {
        let mut body = json!({ identifier: arn });
        if let Some(domain) = self.named {
            body["Domain"] = json!(domain);
        }
        body
    }

    /// The sentence a `StopProductSubscription` is refused with when the ARN reaches a
    /// subscription of another user: the service's "no subscription" wording, with the
    /// domain the REQUEST named, spelled out here rather than read from the production
    /// helper so a change to it fails a test.
    fn no_such_subscription(&self) -> String {
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

/// The ARN a summary carries, as a client reads it off the body.
fn summary_arn(reply: &Reply, summary: &str, member: &str, case: &str) -> String {
    as_str(&reply.ok(case)[summary], member, case).to_string()
}

/// RULE: a subscription reached by `ProductUserArn` is the one the caller asked for only
/// when the `Domain` the request names is the stored user's — a request naming none, or the
/// empty value, narrows nothing — and a subscription of another user is reported as no
/// subscription of the user the request named (`ResourceNotFoundException`, HTTP 404).
#[tokio::test]
async fn a_product_user_arn_resolves_only_the_user_the_domain_names() {
    for row in ROWS {
        let case = row.case;
        let h = setup(row).await;

        let started = start_subscription(&h, &row.subscription_members()).await;
        let arn = summary_arn(&started, "ProductUserSummary", "ProductUserArn", case);

        let stopped = stop_subscription(&h, &row.addressed_by_arn("ProductUserArn", &arn)).await;
        match row.names {
            Names::TheStoredUser => {
                let resolved =
                    summary_arn(&stopped, "ProductUserSummary", "ProductUserArn", case);
                assert_eq!(
                    resolved, arn,
                    "case {case:?}: the ARN names the stored user, so the stop must resolve \
                     that subscription, got {stopped:?}"
                );
            }
            Names::AnotherUser => {
                assert_eq!(
                    stopped.status, 404,
                    "case {case:?}: StopProductSubscription models \
                     ResourceNotFoundException at HTTP 404, got {stopped:?}"
                );
                assert_eq!(
                    stopped.error_type.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: expected ResourceNotFoundException, got {stopped:?}"
                );
                assert_eq!(
                    stopped.message(),
                    Some(row.no_such_subscription().as_str()),
                    "case {case:?}: the refusal must report no subscription for the user the \
                     request named, got {stopped:?}"
                );
                // And the subscription the ARN reached is untouched: the refused call ended
                // nothing, so addressing it as the user it is stored for still resolves it.
                let its_own_user = stop_subscription(
                    &h,
                    &json!({ "ProductUserArn": arn }),
                )
                .await;
                assert_eq!(
                    summary_arn(&its_own_user, "ProductUserSummary", "ProductUserArn", case),
                    arn,
                    "case {case:?}: a refused stop must leave the stored subscription \
                     resolvable, got {its_own_user:?}"
                );
            }
        }
    }
}

/// RULE: the SAME comparison decides an association reached by `InstanceUserArn` — only the
/// user the request's `Domain` names resolves — with this surface's own refusal: an
/// association of another user cannot be shown to be the one the caller asked for, so the
/// call fails CLOSED rather than reporting a not-found.
#[tokio::test]
async fn an_instance_user_arn_resolves_only_the_user_the_domain_names() {
    for row in ROWS {
        let case = row.case;
        let h = setup(row).await;

        let associated = associate(&h, &row.association_members()).await;
        let arn = summary_arn(&associated, "InstanceUserSummary", "InstanceUserArn", case);

        let ended = disassociate(&h, &row.addressed_by_arn("InstanceUserArn", &arn)).await;
        match row.names {
            Names::TheStoredUser => {
                let resolved = summary_arn(&ended, "InstanceUserSummary", "InstanceUserArn", case);
                assert_eq!(
                    resolved, arn,
                    "case {case:?}: the ARN names the stored user, so the disassociation \
                     must resolve that association, got {ended:?}"
                );
            }
            Names::AnotherUser => {
                assert_fails_closed(case, &arn, &ended);
                // The association the ARN reached is untouched: addressing it as the user it
                // is stored for still resolves it.
                let its_own_user = disassociate(&h, &json!({ "InstanceUserArn": arn })).await;
                assert_eq!(
                    summary_arn(&its_own_user, "InstanceUserSummary", "InstanceUserArn", case),
                    arn,
                    "case {case:?}: a refused disassociation must leave the stored \
                     association resolvable, got {its_own_user:?}"
                );
            }
        }
    }
}
