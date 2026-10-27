// SPDX-License-Identifier: CC0-1.0

//! A DNS Firewall domain list the service publishes itself is not the caller's
//! to write, and its domains are not the caller's to read.
//!
//! The rule, stated once and then exercised over a set of inputs: a domain list
//! whose `ManagedOwnerName` is set belongs to Route 53 Resolver DNS Firewall and
//! not to the account that can see it, so
//!
//! * every operation that would WRITE it — `UpdateFirewallDomains`,
//!   `DeleteFirewallDomainList` — answers exactly as it answers for a list that
//!   does not exist: HTTP 400 `ResourceNotFoundException` carrying the service's
//!   "can't find the resource with ID" body for the id the request named, so a
//!   list the caller may not write is indistinguishable from one it does not
//!   have;
//! * `ListFirewallDomains` answers HTTP 400 `AccessDeniedException` — the
//!   domains inside a published list are the service's own threat intelligence,
//!   so the list is visible while its contents are not;
//! * the readers of the list itself keep serving it: `GetFirewallDomainList`
//!   reports the same list, with the same `Status` and `DomainCount`, after
//!   every one of those refusals;
//! * and the caller's OWN list takes the very same write, so the refusals are
//!   about who owns the list and not about rejecting the request.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member};

/// The message Route 53 Resolver answers for a resource the caller named but
/// cannot address: the bracketed service error code, the fixed sentence (with
/// the typographic apostrophe AWS writes it with) and the id from the request.
fn cannot_find(resource_id: &str) -> String {
    format!(r#"[RSLVR-02025] Can’t find the resource with ID "{resource_id}"."#)
}

/// The fixed message `ListFirewallDomains` refuses a published list with.
const MANAGED_LIST_DENIED: &str = "You don't have a permission to list the domains inside a \
                                   Route 53 Resolver DNS Firewall managed list.";

/// Every domain list the service publishes, as `(name, id)` — the lists
/// `ListFirewallDomainLists` reports a `ManagedOwnerName` for, which is the
/// member used "only for lists that are not managed by you"
/// (`smithy_query(query="doc",shape="FirewallDomainList",member="ManagedOwnerName")`).
async fn published_lists(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let listing = ok(router, "ListFirewallDomainLists", json!({ "MaxResults": 100 })).await;
    let published: Vec<(String, String)> = listing
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the domain lists")
        .iter()
        .filter(|item| item.get("ManagedOwnerName").is_some())
        .map(|item| (string_member(item, "Name"), string_member(item, "Id")))
        .collect();
    assert!(
        !published.is_empty(),
        "the listing must report the lists DNS Firewall publishes, got {listing}"
    );
    published
}

/// The `FirewallDomainList` `GetFirewallDomainList` reports for `id`.
async fn read_list(router: &MultiServiceRouter, id: &str) -> Value {
    let answered = ok(
        router,
        "GetFirewallDomainList",
        json!({ "FirewallDomainListId": id }),
    )
    .await;
    answered
        .get("FirewallDomainList")
        .unwrap_or_else(|| panic!("the read must answer a FirewallDomainList, got {answered}"))
        .clone()
}

/// Assert `operation` refuses `request` with HTTP 400, the error shape
/// `expected_error` and exactly `expected_message`.
async fn expect_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    request: Value,
    case: &str,
    expected_error: &str,
    expected_message: &str,
) {
    let (status, answered) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} must answer HTTP 400, got {status}: {answered}"
    );
    assert!(
        error_type(&answered).contains(expected_error),
        "case {case:?}: {operation} must answer {expected_error}, got {answered}"
    );
    assert_eq!(
        string_member(&answered, "Message"),
        expected_message,
        "case {case:?}: {operation} must state the fault as AWS words it, got {answered}"
    );
}

/// The write path hides every published list, the domain listing refuses it,
/// and the list itself is unchanged and still readable afterwards.
///
/// The table spans what the rule generalises over: every published list rather
/// than one, and each `FirewallDomainUpdateOperation` the model declares, so the
/// answer is about the list and not about the kind of write.
#[tokio::test]
async fn a_published_domain_list_is_hidden_from_the_write_path() {
    let router = emulator();

    for (name, id) in published_lists(&router).await {
        let before = read_list(&router, &id).await;

        for operation in ["ADD", "REMOVE", "REPLACE"] {
            let case = format!("{name} / {operation}");
            expect_refusal(
                &router,
                "UpdateFirewallDomains",
                json!({
                    "FirewallDomainListId": id.clone(),
                    "Operation": operation,
                    "Domains": ["sim2real-should-not-land.example."],
                }),
                &case,
                "ResourceNotFoundException",
                &cannot_find(&id),
            )
            .await;
        }

        expect_refusal(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": id.clone() }),
            &name,
            "ResourceNotFoundException",
            &cannot_find(&id),
        )
        .await;

        expect_refusal(
            &router,
            "ListFirewallDomains",
            json!({ "FirewallDomainListId": id.clone() }),
            &name,
            "AccessDeniedException",
            MANAGED_LIST_DENIED,
        )
        .await;

        // The list itself is still served, and nothing about it moved.
        let after = read_list(&router, &id).await;
        assert_eq!(
            after, before,
            "case {name:?}: the published list must be reported exactly as before the refusals"
        );
        assert_eq!(
            string_member(&after, "Name"),
            name,
            "case {name:?}: the published list must still be readable under its own id, got {after}"
        );
        assert!(
            after.get("ManagedOwnerName").is_some(),
            "case {name:?}: the list the refusals are about is the service's own, got {after}"
        );
        assert_eq!(
            after.get("DomainCount").and_then(Value::as_i64),
            Some(0),
            "case {name:?}: the read must report the DomainCount this store holds for a published \
             list, whose domains are the service's own and are held nowhere here, got {after}"
        );
        assert_eq!(
            string_member(&after, "Status"),
            string_member(&before, "Status"),
            "case {name:?}: the refusals must not move the list's Status, got {after}"
        );
    }
}

/// The same write the published lists hide is served against the caller's own
/// list, and an id nothing minted is refused with the same body a published list
/// is — so the refusal is about ownership, not about the request, and a list the
/// caller may not write cannot be told apart from one that is not there.
#[tokio::test]
async fn the_callers_own_list_takes_the_write_a_published_list_hides() {
    let router = emulator();
    let published = published_lists(&router).await;
    let (_, published_id) = published
        .first()
        .cloned()
        .expect("at least one published list");

    let own = ok(
        &router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": "req-own", "Name": "my-own-list" }),
    )
    .await;
    let own_id = string_member(
        own.get("FirewallDomainList")
            .unwrap_or_else(|| panic!("the create must answer a FirewallDomainList, got {own}")),
        "Id",
    );

    let updated = ok(
        &router,
        "UpdateFirewallDomains",
        json!({
            "FirewallDomainListId": own_id.clone(),
            "Operation": "ADD",
            "Domains": ["sim2real-should-not-land.example."],
        }),
    )
    .await;
    assert_eq!(
        string_member(&updated, "Id"),
        own_id,
        "the caller's own list must take the write, got {updated}"
    );
    let listed = ok(
        &router,
        "ListFirewallDomains",
        json!({ "FirewallDomainListId": own_id.clone() }),
    )
    .await;
    assert_eq!(
        listed.get("Domains"),
        Some(&json!(["sim2real-should-not-land.example."])),
        "the caller's own list must report the domain the write added, got {listed}"
    );

    // An id nothing minted, and the published list, answer the same body.
    let absent = format!("{own_id}x");
    for (case, id) in [
        ("an id nothing minted", absent),
        ("a published list", published_id),
    ] {
        expect_refusal(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": "ADD",
                "Domains": ["sim2real-should-not-land.example."],
            }),
            case,
            "ResourceNotFoundException",
            &cannot_find(&id),
        )
        .await;
    }
}
