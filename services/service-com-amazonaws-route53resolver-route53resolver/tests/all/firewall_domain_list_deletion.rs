// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFirewallDomainList`, the operation that retires a
//! DNS Firewall domain list, and for the readers its removal is observed
//! through: `GetFirewallDomainList`, `ListFirewallDomains` and
//! `ListFirewallDomainLists`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! What these cases pin is the rule the operation implements: a delete reports
//! the stored record one last time, in the status that says it is going away,
//! and drops it from the store, so every later read of that id reports it
//! missing and the identifiers it held are free again; a delete this account may
//! not make — a list the service itself publishes, or one whose last change has
//! not settled — is refused and changes nothing.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::PROVISIONING_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, expect_error, ok, parse_time, string_member, ACCOUNT, REGION};

/// The `FirewallDomainListStatus` values the model declares
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus")`), which any
/// status a response reports must be one of.
const DOMAIN_LIST_STATUSES: &[&str] = &[
    "COMPLETE",
    "COMPLETE_IMPORT_FAILED",
    "IMPORTING",
    "DELETING",
    "UPDATING",
];

/// A domain list DNS Firewall publishes itself. The first is named in
/// `FirewallDomainList$ManagedOwnerName`'s doc
/// (`smithy_query(query="doc",shape="FirewallDomainList",member="ManagedOwnerName")`);
/// the second is another entry of the same published catalog, which the issue
/// behind these cases never cited.
const PUBLISHED_LISTS: &[&str] = &[
    "AWSManagedDomainsMalwareDomainList",
    "AWSManagedDomainsAggregateThreatList",
];

/// The `FirewallDomainList` of a create/get/delete response.
fn domain_list(body: &Value) -> &Value {
    body.get("FirewallDomainList")
        .expect("the response carries a FirewallDomainList")
}

/// Create a domain list and return its `FirewallDomainList`.
async fn create(router: &MultiServiceRouter, request: Value) -> Value {
    domain_list(&ok(router, "CreateFirewallDomainList", request).await).clone()
}

/// Read one list back, or `None` once nothing is stored under that id.
async fn read(router: &MultiServiceRouter, id: &str) -> Value {
    domain_list(
        &ok(
            router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
        )
        .await,
    )
    .clone()
}

/// The ids `ListFirewallDomainLists` reports, paging until it reports no cursor.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": 100 });
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListFirewallDomainLists", request).await;
        if let Some(items) = page.get("FirewallDomainLists").and_then(Value::as_array) {
            ids.extend(items.iter().map(|item| string_member(item, "Id")));
        }
        next = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next.is_none() {
            return ids;
        }
    }
}

/// Wait out the span the clock owes a list whose change has not settled, so the
/// settled status is due.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// One row of the delete table: how the list was set up before it was deleted.
struct DeleteCase {
    case: &'static str,
    request: Value,
    /// Domains written into the list before the delete, through
    /// `UpdateFirewallDomains`; the delete must report the count they leave.
    domains: &'static [&'static str],
}

/// A delete reports the stored list in `DELETING` and removes it: afterwards
/// every reader reports that id missing, the listing no longer carries it, and a
/// second delete of the same id is a miss too.
///
/// The table spans what the rule generalises over: a list the create left empty
/// and lists carrying one and several domains (so the reported `DomainCount` is
/// computed, not a constant), a list with tags and one without, and names of
/// different shapes.
#[tokio::test]
async fn a_deleted_domain_list_is_removed_from_state() {
    let cases = vec![
        DeleteCase {
            case: "the empty list a create makes",
            request: json!({ "CreatorRequestId": "req-empty", "Name": "empty-list" }),
            domains: &[],
        },
        DeleteCase {
            case: "one domain, and tags",
            request: json!({
                "CreatorRequestId": "req-one-domain",
                "Name": "one_domain-1",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
            domains: &["blocked.example.com"],
        },
        DeleteCase {
            case: "several domains, a name with spaces",
            request: json!({ "CreatorRequestId": "req-many", "Name": "my blocked list" }),
            domains: &["a.example.com", "b.example.net", "*.example.org"],
        },
    ];

    let router = emulator();
    for DeleteCase {
        case,
        request,
        domains,
    } in cases
    {
        let made = create(&router, request).await;
        let id = string_member(&made, "Id");
        if !domains.is_empty() {
            ok(
                &router,
                "UpdateFirewallDomains",
                json!({
                    "FirewallDomainListId": id.clone(),
                    "Operation": "ADD",
                    "Domains": domains,
                }),
            )
            .await;
            // The change the clock owes falls due first, so the delete below is
            // about deleting a settled list and not about a transition.
            settle().await;
        }

        let before = read(&router, &id).await;
        let deleted = domain_list(
            &ok(
                &router,
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            )
            .await,
        )
        .clone();

        // The delete answers "the domain list that you just deleted": the
        // record as it stood, not a fresh one.
        for member in ["Id", "Arn", "Name", "CreatorRequestId", "CreationTime"] {
            assert_eq!(
                deleted.get(member),
                before.get(member),
                "case {case:?}: {member} must come from the stored list, got {deleted}"
            );
        }
        assert_eq!(
            string_member(&deleted, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:firewall-domain-list/{id}"),
            "case {case:?}: the Arn must be the stored one, rooted in the signed region, got {deleted}"
        );
        assert_eq!(
            deleted.get("DomainCount"),
            Some(&json!(domains.len())),
            "case {case:?}: DomainCount must be the domains the list held, got {deleted}"
        );
        let status = string_member(&deleted, "Status");
        assert_eq!(
            status, "DELETING",
            "case {case:?}: the list the delete reports is on its way out, got {deleted}"
        );
        assert!(
            DOMAIN_LIST_STATUSES.contains(&status.as_str()),
            "case {case:?}: Status must be a modelled FirewallDomainListStatus, got {status:?}"
        );
        let created_at = parse_time(case, "CreationTime", &string_member(&deleted, "CreationTime"));
        let modified_at = parse_time(
            case,
            "ModificationTime",
            &string_member(&deleted, "ModificationTime"),
        );
        assert!(
            modified_at >= created_at,
            "case {case:?}: ModificationTime must not precede CreationTime, got {deleted}"
        );

        // Every reader of that id now reports it missing, and the listing has
        // dropped it.
        for (operation, request) in [
            (
                "GetFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            ),
            (
                "ListFirewallDomains",
                json!({ "FirewallDomainListId": id.clone() }),
            ),
            (
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            ),
        ] {
            expect_error(
                &router,
                operation,
                request,
                case,
                "ResourceNotFoundException",
            )
            .await;
        }
        let ids = listed_ids(&router).await;
        assert!(
            !ids.contains(&id),
            "case {case:?}: the listing must no longer report {id:?}, got {ids:?}"
        );
    }
}

/// A delete frees the `CreatorRequestId` the list was made with: a later create
/// reusing that string makes a NEW list rather than resolving to the deleted one
/// (which the retry rule would do while the list still existed).
#[tokio::test]
async fn the_creator_request_id_is_free_again_after_the_delete() {
    let router = emulator();

    for (case, creator_request_id) in [
        ("a plain request id", "req-reused"),
        ("a request id that reads like a date/time stamp", "2026-10-24T15:07:03Z"),
    ] {
        let request = json!({ "CreatorRequestId": creator_request_id, "Name": "reused-id" });
        let first = create(&router, request.clone()).await;
        let first_id = string_member(&first, "Id");

        // While the list exists, the same request id resolves to it.
        let retried = create(&router, request.clone()).await;
        assert_eq!(
            string_member(&retried, "Id"),
            first_id,
            "case {case:?}: a retry must resolve to the list that request made, got {retried}"
        );

        ok(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": first_id.clone() }),
        )
        .await;

        let remade = create(&router, request).await;
        let remade_id = string_member(&remade, "Id");
        assert_ne!(
            remade_id, first_id,
            "case {case:?}: the deleted list is gone, so the create must mint a new Id, got {remade}"
        );
        assert_eq!(
            string_member(&read(&router, &remade_id).await, "Id"),
            remade_id,
            "case {case:?}: the new list must be readable under its own id"
        );
    }
}

/// An id the store holds no list under is reported with the
/// `ResourceNotFoundException` the operation models — never an empty success,
/// and never a list it invented. The rows walk the forms a wrong id takes: one
/// nothing minted, another resource's id, the shortest `ResourceId` the model
/// allows, and a near miss of a stored id.
#[tokio::test]
async fn an_unknown_domain_list_id_is_reported_as_missing() {
    let router = emulator();
    let stored = string_member(
        &create(
            &router,
            json!({ "CreatorRequestId": "req-kept", "Name": "kept" }),
        )
        .await,
        "Id",
    );

    for (case, id) in [
        (
            "an id nothing minted",
            "rslvr-fdl-0123456789abcdef".to_string(),
        ),
        ("a Resolver rule id", "rslvr-rr-0123456789abcdef".to_string()),
        ("one character, the shortest ResourceId", "x".to_string()),
        (
            "a near miss of a stored id",
            stored[..stored.len() - 1].to_string(),
        ),
    ] {
        expect_error(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // None of the refusals touched the stored list.
    assert_eq!(
        string_member(&read(&router, &stored).await, "Id"),
        stored,
        "case \"the stored list after the refusals\": it must still be readable"
    );
}

/// A domain list DNS Firewall publishes itself belongs to the service, not to
/// the account reading it: a delete that names one is refused, and the published
/// list is still there afterwards.
#[tokio::test]
async fn a_published_domain_list_is_not_this_accounts_to_delete() {
    let router = emulator();
    let listing = ok(&router, "ListFirewallDomainLists", json!({ "MaxResults": 100 })).await;
    let items = listing
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the published lists")
        .clone();

    for name in PUBLISHED_LISTS {
        let id = items
            .iter()
            .find(|item| string_member(item, "Name") == *name)
            .map(|item| string_member(item, "Id"))
            .unwrap_or_else(|| panic!("case {name:?}: the published list must be reported"));

        expect_error(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": id.clone() }),
            name,
            "ValidationException",
        )
        .await;

        assert_eq!(
            string_member(&read(&router, &id).await, "Name"),
            *name,
            "case {name:?}: the published list must still be readable after the refusal"
        );
    }
}

/// A list whose last change has not settled yet has a state transition in
/// flight, so the delete is refused with the `ConflictException` the operation
/// models and the list keeps its domains. Once the clock has made the settled
/// status due, the very same delete is served.
#[tokio::test]
async fn a_list_whose_change_has_not_settled_cannot_be_deleted_yet() {
    let router = emulator();

    for (case, operation, domains) in [
        ("an ADD that has not settled", "ADD", vec!["a.example.com"]),
        (
            "a REPLACE that has not settled",
            "REPLACE",
            vec!["b.example.com", "c.example.com"],
        ),
    ] {
        let id = string_member(
            &create(
                &router,
                json!({
                    "CreatorRequestId": format!("req-{operation}"),
                    "Name": format!("settling-{operation}"),
                }),
            )
            .await,
            "Id",
        );

        let updated = ok(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": id.clone(),
                "Operation": operation,
                "Domains": domains.clone(),
            }),
        )
        .await;
        assert_eq!(
            string_member(&updated, "Status"),
            "UPDATING",
            "case {case:?}: the change has not settled yet, got {updated}"
        );

        expect_error(
            &router,
            "DeleteFirewallDomainList",
            json!({ "FirewallDomainListId": id.clone() }),
            case,
            "ConflictException",
        )
        .await;

        // The refusal wrote nothing: the list is still there, still carrying the
        // domains the update wrote.
        let kept = read(&router, &id).await;
        assert_eq!(
            kept.get("DomainCount"),
            Some(&json!(domains.len())),
            "case {case:?}: the refused delete must leave the list as it was, got {kept}"
        );

        settle().await;

        let deleted = domain_list(
            &ok(
                &router,
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            )
            .await,
        )
        .clone();
        assert_eq!(
            string_member(&deleted, "Status"),
            "DELETING",
            "case {case:?}: once the change has settled the delete is served, got {deleted}"
        );
        expect_error(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }
}
