// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH USER a product subscription and a user association belong
//! to, driven over the real wire path: every call goes through the router's `dispatch`, so
//! each request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The RULE under test, asserted over a TABLE of inputs rather than one recorded request:
//! the `Domain` a user-plane request carries is part of WHICH USER it names — it is "the
//! domain name of the Active Directory that contains the user" the call acts on, and the
//! call that ends one carries it BESIDE `Username` and the provider reference
//! (smithy_query: operation StartProductSubscription / StopProductSubscription /
//! AssociateUser / DisassociateUser, input `Domain`) — so a request that names a domain
//! addresses only the user of that domain, and a request that names none does not narrow:
//! it addresses the user record already held for that user name. Every path that matches a
//! user does so the same way, so what the creating call treats as already held is exactly
//! what the ending call resolves, and no user name is left unsubscribable-but-unresolvable.
//!
//! What this suite pins, over the same rows on both halves of the user plane:
//!
//! * `StartProductSubscription` refuses a second subscription only for the user its
//!   `Domain` names, and `StopProductSubscription` given the same members resolves exactly
//!   the subscription whose existence that refusal (or its absence) reported;
//! * `AssociateUser`, which is `@idempotent`, answers the association already held only for
//!   the user its `Domain` names and mints one for a user name in another domain, and
//!   `DisassociateUser` given the same members resolves exactly that association;
//! * the empty `Domain` names the same (unqualified) user as no `Domain` at all, in every
//!   one of those paths.
//!
//! The read-back of what was written is `ListProductSubscriptions` and
//! `ListUserAssociations`, the operations that report those records besides the ones that
//! write them.

use serde_json::{json, Value};

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// provider-registration helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, harness, list_subscriptions, provider_union, register, start_subscription,
    stop_subscription, Harness, Reply, PRODUCTS,
};
use super::resource_resolution_refusals::{associate, disassociate, held_instance, registration};

/// `ListUserAssociations` is bound to POST /user/ListUserAssociations (smithy_query:
/// operation ListUserAssociations, http.uri).
async fn list_associations(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListUserAssociations", body).await
}

/// Whether the second call of a row names the user the first call already created a record
/// for, or another user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    /// The same user: the record the first call wrote is the one the second call's members
    /// name, so a start conflicts, an associate answers that association, and the ending
    /// call resolves that same record.
    TheSameUser,
    /// Another user: the second call writes a record of its own, which is the one the
    /// ending call with those members resolves.
    AnotherUser,
}

/// Two user-plane calls for ONE user name, differing in the `Domain` each one carries.
///
/// `Username` and `Domain` are plain `String`s in this service's model (no pattern to
/// honour), so the rows vary in FORM — a domain and none, two sibling domains, a domain and
/// a sub-domain of it, the empty value — rather than against a constraint, and each row
/// runs through its own registration (directory, product) so no rule is keyed to one.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    username: &'static str,
    /// The `Domain` the first call carries, or `None` when it carries none.
    first: Option<&'static str>,
    /// The `Domain` the second call carries.
    second: Option<&'static str>,
    names: Names,
}

const ROWS: &[Row] = &[
    Row {
        case: "two sibling Active Directory domains",
        username: "alice",
        first: Some("a.example"),
        second: Some("b.example"),
        names: Names::AnotherUser,
    },
    Row {
        case: "a user with no domain, then one in a domain",
        username: "bob",
        first: None,
        second: Some("corp.example"),
        names: Names::AnotherUser,
    },
    Row {
        case: "a domain and a sub-domain of it, upn-shaped user name",
        username: "carol@corp.example",
        first: Some("corp.example"),
        second: Some("eu.corp.example"),
        names: Names::AnotherUser,
    },
    Row {
        case: "the same domain twice",
        username: "dave.o-brien",
        first: Some("example.com"),
        second: Some("example.com"),
        names: Names::TheSameUser,
    },
    Row {
        case: "no domain twice",
        username: "erin",
        first: None,
        second: None,
        names: Names::TheSameUser,
    },
    Row {
        case: "no domain, then the empty Domain, which names the same unqualified user",
        username: "FRANK",
        first: None,
        second: Some(""),
        names: Names::TheSameUser,
    },
    Row {
        case: "a domain, then no domain at all, which narrows nothing",
        username: "grace",
        first: Some("corp.example"),
        second: None,
        names: Names::TheSameUser,
    },
];

/// The directories the harness puts in the account's fleet, and the products of this
/// service: a row runs through `(directory, product)` of its own index, so the rule is
/// exercised through more than one registration.
const DIRECTORIES: &[&str] = &["d-0123456789", "d-00000000ff"];

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

    /// The members that name a user of this row's registration, with `Domain` as supplied
    /// (left out of the body entirely when the row carries none).
    fn subscription_members(&self, domain: Option<&str>) -> Value {
        let mut body = json!({
            "Username": self.username,
            "IdentityProvider": provider_union(self.directory()),
            "Product": self.product(),
        });
        if let Some(domain) = domain {
            body["Domain"] = json!(domain);
        }
        body
    }

    fn association_members(&self, domain: Option<&str>) -> Value {
        let mut body = json!({
            "Username": self.username,
            "InstanceId": self.instance(),
            "IdentityProvider": provider_union(self.directory()),
        });
        if let Some(domain) = domain {
            body["Domain"] = json!(domain);
        }
        body
    }

    fn list_subscriptions_request(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory()),
            "Product": self.product(),
        })
    }

    fn list_associations_request(&self) -> Value {
        json!({
            "IdentityProvider": provider_union(self.directory()),
            "InstanceId": self.instance(),
        })
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

/// The `Domain` a summary reports, as a client reads it off the body (absent reads as
/// `None`).
fn reported_domain(summary: &Value) -> Option<&str> {
    summary.get("Domain").and_then(Value::as_str)
}

/// The ARNs one list reports, in list order.
fn listed(body: &Value, list: &str, items: &str, member: &str, case: &str) -> Vec<String> {
    body[items]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {items} in the {list} answer {body:?}"))
        .iter()
        .map(|summary| as_str(summary, member, case).to_string())
        .collect()
}

/// RULE: `Domain` names which user a product subscription belongs to — a second start for
/// the user a stored subscription belongs to is refused as a conflict, one for the same user
/// name in another Active Directory domain is a subscription of its own, and
/// `StopProductSubscription` given those same members resolves exactly the subscription the
/// start reported.
#[tokio::test]
async fn a_domain_names_which_user_a_product_subscription_belongs_to() {
    for row in ROWS {
        let case = row.case;
        let h = setup(row).await;

        let first = start_subscription(&h, &row.subscription_members(row.first)).await;
        let first_summary = first.ok(case)["ProductUserSummary"].clone();
        let first_arn = as_str(&first_summary, "ProductUserArn", case).to_string();
        assert_eq!(
            reported_domain(&first_summary),
            row.first,
            "case {case:?}: the summary must carry the domain the start named, got \
             {first_summary:?}"
        );

        let second = start_subscription(&h, &row.subscription_members(row.second)).await;
        // Whichever user the second call named, the subscription the matching stop resolves
        // is the one this start reported — a user a start refuses as already subscribed is
        // never a user a stop says has no subscription.
        let expected_arn = match row.names {
            Names::TheSameUser => {
                // `ConflictException` carries `@error("server")` and no `@httpError` in this
                // service's model, so the framework serializes it at HTTP 500.
                assert_eq!(
                    second.status, 500,
                    "case {case:?}: a second subscription for the same user is the modelled \
                     ConflictException at HTTP 500, got {second:?}"
                );
                assert_eq!(
                    second.error_type.as_deref(),
                    Some("ConflictException"),
                    "case {case:?}: expected ConflictException, got {second:?}"
                );
                first_arn.clone()
            }
            Names::AnotherUser => {
                let summary = second.ok(case)["ProductUserSummary"].clone();
                let arn = as_str(&summary, "ProductUserArn", case).to_string();
                assert_ne!(
                    arn, first_arn,
                    "case {case:?}: a user name in another domain is another user, which \
                     carries its own subscription, got {summary:?}"
                );
                assert_eq!(
                    reported_domain(&summary),
                    row.second,
                    "case {case:?}: the summary must carry the domain the start named, got \
                     {summary:?}"
                );
                arn
            }
        };

        // Every subscription the two starts stored is reported, each one once.
        let reported = list_subscriptions(&h, &row.list_subscriptions_request()).await;
        let reported = listed(
            reported.ok(case),
            "ListProductSubscriptions",
            "ProductUserSummaries",
            "ProductUserArn",
            case,
        );
        let mut expected_list = vec![first_arn.clone()];
        if row.names == Names::AnotherUser {
            expected_list.push(expected_arn.clone());
        }
        assert_eq!(
            reported, expected_list,
            "case {case:?}: the list must report exactly the subscriptions the starts \
             stored, in subscription order"
        );

        // The stop addressed by the SECOND call's members resolves the subscription that
        // call reported: the one it created, or the one it was refused for.
        let stopped = stop_subscription(&h, &row.subscription_members(row.second)).await;
        let stopped_summary = stopped.ok(case)["ProductUserSummary"].clone();
        assert_eq!(
            as_str(&stopped_summary, "ProductUserArn", case),
            expected_arn,
            "case {case:?}: the stop must resolve the subscription of the user its members \
             name, got {stopped_summary:?}"
        );

        // And the FIRST call's user is untouched by that stop unless it was the same user.
        let still_subscribed = list_subscriptions(&h, &row.list_subscriptions_request()).await;
        let body = still_subscribed.ok(case);
        let live: Vec<String> = body["ProductUserSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no ProductUserSummaries in {body:?}"))
            .iter()
            .filter(|summary| as_str(summary, "Status", case) == "SUBSCRIBED")
            .map(|summary| as_str(summary, "ProductUserArn", case).to_string())
            .collect();
        let expected_live = match row.names {
            Names::TheSameUser => Vec::new(),
            Names::AnotherUser => vec![first_arn.clone()],
        };
        assert_eq!(
            live, expected_live,
            "case {case:?}: ending one user's subscription must leave the other user's \
             subscribed"
        );
    }
}

/// RULE: the same `Domain` matching names which user a user association belongs to — the
/// `@idempotent` `AssociateUser` answers the association already held only for the user its
/// `Domain` names, mints one for the same user name in another Active Directory domain, and
/// `DisassociateUser` given those same members resolves exactly the association the
/// associate reported.
#[tokio::test]
async fn a_domain_names_which_user_an_association_belongs_to() {
    for row in ROWS {
        let case = row.case;
        let h = setup(row).await;

        let first = associate(&h, &row.association_members(row.first)).await;
        let first_summary = first.ok(case)["InstanceUserSummary"].clone();
        let first_arn = as_str(&first_summary, "InstanceUserArn", case).to_string();
        assert_eq!(
            reported_domain(&first_summary),
            row.first,
            "case {case:?}: the summary must carry the domain the associate named, got \
             {first_summary:?}"
        );

        // `AssociateUser` is `@idempotent`, so naming the user that is already associated
        // answers THAT association rather than minting a second one; naming a user name in
        // another domain names another user, which gets its own.
        let second = associate(&h, &row.association_members(row.second)).await;
        let second_summary = second.ok(case)["InstanceUserSummary"].clone();
        let second_arn = as_str(&second_summary, "InstanceUserArn", case).to_string();
        match row.names {
            Names::TheSameUser => assert_eq!(
                second_arn, first_arn,
                "case {case:?}: the association already held for that user is the one \
                 answered, got {second_summary:?}"
            ),
            Names::AnotherUser => {
                assert_ne!(
                    second_arn, first_arn,
                    "case {case:?}: a user name in another domain is another user, which \
                     carries its own association, got {second_summary:?}"
                );
                assert_eq!(
                    reported_domain(&second_summary),
                    row.second,
                    "case {case:?}: the summary must carry the domain the associate named, \
                     got {second_summary:?}"
                );
            }
        }

        let reported = list_associations(&h, &row.list_associations_request()).await;
        let reported = listed(
            reported.ok(case),
            "ListUserAssociations",
            "InstanceUserSummaries",
            "InstanceUserArn",
            case,
        );
        let mut expected_list = vec![first_arn.clone()];
        if row.names == Names::AnotherUser {
            expected_list.push(second_arn.clone());
        }
        assert_eq!(
            reported, expected_list,
            "case {case:?}: the list must report exactly the associations the calls stored, \
             in association order"
        );

        // The disassociation addressed by the SECOND call's members resolves the association
        // that call answered.
        let ended = disassociate(&h, &row.association_members(row.second)).await;
        let ended_summary = ended.ok(case)["InstanceUserSummary"].clone();
        assert_eq!(
            as_str(&ended_summary, "InstanceUserArn", case),
            second_arn,
            "case {case:?}: the disassociation must resolve the association of the user its \
             members name, got {ended_summary:?}"
        );
    }
}
