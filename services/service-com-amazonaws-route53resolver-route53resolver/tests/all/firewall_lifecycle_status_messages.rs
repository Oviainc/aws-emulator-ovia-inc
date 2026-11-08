// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the lifecycle state the DNS Firewall domain-list and
//! rule-group projections report: the pair of `Status` and `StatusMessage`
//! `CreateFirewallDomainList`, `GetFirewallDomainList`, `UpdateFirewallDomains`,
//! `DeleteFirewallDomainList`, `CreateFirewallRuleGroup`,
//! `GetFirewallRuleGroup` and `DeleteFirewallRuleGroup` answer with.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! What these cases pin is the rule the slice implements: a domain list and a
//! rule group each stand in a lifecycle step, and every projection of one
//! reports that step BOTH as its `Status` and as the sentence the step states in
//! `StatusMessage` — the step it LAST reached, so a resource a caller has
//! written states the step that change reached and not the one its create did;
//! a change the service applies inside the call it was asked for leaves the
//! resource in its settled step, so no response reports a step that is still in
//! flight for work that is already done.

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::{
    FIREWALL_DOMAIN_LIST_STATUS_VALUES, FIREWALL_DOMAIN_UPDATE_OPERATION_VALUES,
    FIREWALL_RULE_GROUP_STATUS_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, expect_error, ok, string_member};

/// The sentence DNS Firewall states for a domain list that stands in its
/// created, settled step, the one it states once a write of its domains has
/// landed, and the one it states once a delete has been accepted for the list.
/// All three are fixed text for the step, with no part of the request in them.
const DOMAIN_LIST_CREATED_MESSAGE: &str = "Created Firewall Domain List";
const DOMAIN_LIST_CHANGED_MESSAGE: &str = "Finished domain list update";
const DOMAIN_LIST_DELETING_MESSAGE: &str = "Deleting the Firewall Domain List";

/// The sentence DNS Firewall states for a rule group that stands in its
/// created, settled step, and the one it states once a write of its rules has
/// landed.
const RULE_GROUP_CREATED_MESSAGE: &str = "Created Firewall Rule Group";
const RULE_GROUP_CHANGED_MESSAGE: &str = "Finished rule group update";

/// The sentence DNS Firewall states for a rule group a delete has been
/// accepted for — the group's own delete-step sentence, as
/// [`DOMAIN_LIST_DELETING_MESSAGE`] is the list's.
const RULE_GROUP_DELETING_MESSAGE: &str = "Deleting the Firewall Rule Group";

/// The settled `FirewallDomainListStatus`/`FirewallRuleGroupStatus` both
/// families report once nothing is in flight, out of the model's enums
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus,FirewallRuleGroupStatus")`,
/// re-exported as the `*_STATUS_VALUES` catalogs this suite checks against).
const SETTLED: &str = "COMPLETE";

/// The step a domain list stands in once a delete has been accepted for it.
const DELETING: &str = "DELETING";

/// `StatusMessage`'s modelled maximum length, from the member's `length` trait
/// (0..255) on both `FirewallDomainList$StatusMessage` and
/// `FirewallRuleGroup$StatusMessage`.
const STATUS_MESSAGE_MAX: usize = 255;

/// One row of the create table: the name the case goes by, and what the call
/// hands over.
struct ResourceCase {
    case: &'static str,
    request: Value,
}

/// The rows both families are driven over: the shapes a `Name` the model's
/// pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits takes (letters, digits
/// beside letters, underscore, hyphen, apostrophe, space, the published
/// 64-character maximum and the one-character minimum), with and without
/// `Tags`, and `CreatorRequestId`s of different forms — the inputs the rule has
/// to answer the same way, because the step a resource stands in is not
/// something the request chooses.
fn resource_cases() -> Vec<ResourceCase> {
    vec![
        ResourceCase {
            case: "letters only, no Tags",
            request: json!({ "CreatorRequestId": "req-letters", "Name": "lifecycle-plain" }),
        },
        ResourceCase {
            case: "digits beside letters and underscores, one tag",
            request: json!({
                "CreatorRequestId": "req-mixed-2",
                "Name": "lifecycle_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
        },
        ResourceCase {
            case: "spaces and an apostrophe, several tags",
            request: json!({
                "CreatorRequestId": "2026-10-29T23:44:50Z",
                "Name": "ops' lifecycle list",
                "Tags": [
                    { "Key": "team", "Value": "net" },
                    { "Key": "env", "Value": "prod" },
                ],
            }),
        },
        ResourceCase {
            case: "one character, an empty tag list",
            request: json!({ "CreatorRequestId": "r", "Name": "z", "Tags": [] }),
        },
        ResourceCase {
            case: "the longest Name the model allows",
            request: json!({ "CreatorRequestId": "req-longest", "Name": "n".repeat(64) }),
        },
    ]
}

/// Assert the projection `resource` reports `status` and the sentence that step
/// states, within the member's modelled length, and that the sentence quotes no
/// part of the request.
fn assert_step(resource: &Value, status: &str, message: &str, statuses: &[&str], case: &str) {
    let reported = string_member(resource, "Status");
    assert_eq!(
        reported, status,
        "case {case:?}: expected Status {status:?}, got {resource}"
    );
    assert!(
        statuses.contains(&reported.as_str()),
        "case {case:?}: Status must be one of the modelled {statuses:?}, got {reported:?}"
    );
    let reported_message = string_member(resource, "StatusMessage");
    assert_eq!(
        reported_message, message,
        "case {case:?}: expected StatusMessage {message:?}, got {resource}"
    );
    assert!(
        !reported_message.is_empty() && reported_message.len() <= STATUS_MESSAGE_MAX,
        "case {case:?}: StatusMessage must satisfy its modelled length 0..{STATUS_MESSAGE_MAX}, got {reported_message:?}"
    );
    // The step is the service's, so its sentence carries nothing the caller
    // chose: not the resource's own name, nor the request id, nor the minted id.
    // A value of one or two characters is skipped — it can sit inside any
    // English sentence by chance, so it identifies nothing.
    for member in ["Name", "CreatorRequestId", "Id"] {
        let value = string_member(resource, member);
        assert!(
            value.len() < 3 || !reported_message.contains(&value),
            "case {case:?}: StatusMessage must not quote {member} ({value:?}), got {reported_message:?}"
        );
    }
}

/// The `FirewallDomainList` of a domain-list response.
fn domain_list(body: &Value) -> Value {
    body.get("FirewallDomainList")
        .expect("the response carries a FirewallDomainList")
        .clone()
}

/// The `FirewallRuleGroup` of a rule-group response.
fn rule_group(body: &Value) -> Value {
    body.get("FirewallRuleGroup")
        .expect("the response carries a FirewallRuleGroup")
        .clone()
}

/// Read one domain list back over the wire.
async fn read_list(router: &MultiServiceRouter, id: &str) -> Value {
    domain_list(
        &ok(
            router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
        )
        .await,
    )
}

/// Every projection of a domain list — the create, the read, each `Operation`
/// of an update, and the delete — reports the step the list stands in as both
/// `Status` and `StatusMessage`.
///
/// The update rows are every value the model's `FirewallDomainUpdateOperation`
/// declares (re-exported as `FIREWALL_DOMAIN_UPDATE_OPERATION_VALUES`), each
/// carrying a different number of domains, so the settled step is asserted for
/// a list that grows, one that shrinks and one that is rewritten.
#[tokio::test]
async fn every_domain_list_projection_states_the_step_the_list_stands_in() {
    let router = emulator();

    for ResourceCase { case, request } in resource_cases() {
        let made = domain_list(&ok(&router, "CreateFirewallDomainList", request).await);
        let id = string_member(&made, "Id");

        // A list is "created empty", which needs no import, so the create
        // answers the settled step.
        assert_step(
            &made,
            SETTLED,
            DOMAIN_LIST_CREATED_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("{case}: the create"),
        );
        assert_step(
            &read_list(&router, &id).await,
            SETTLED,
            DOMAIN_LIST_CREATED_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("{case}: the read"),
        );

        for (index, operation) in FIREWALL_DOMAIN_UPDATE_OPERATION_VALUES.iter().enumerate() {
            // A different number of domains per row: one, several, and none.
            let domains: Vec<String> = (0..index)
                .map(|slot| format!("host-{slot}.example.com"))
                .chain(std::iter::once(format!("{operation}.example.com")))
                .collect();
            // The acknowledgement states the step the list stood in as the
            // update was accepted: the step the change reaches is what the read
            // after it reports. So the first update answers the creation step
            // and every later one the finished-change step.
            let accepted = if index == 0 {
                DOMAIN_LIST_CREATED_MESSAGE
            } else {
                DOMAIN_LIST_CHANGED_MESSAGE
            };
            let answered = ok(
                &router,
                "UpdateFirewallDomains",
                json!({
                    "FirewallDomainListId": id.clone(),
                    "Operation": operation,
                    "Domains": domains,
                }),
            )
            .await;
            assert_step(
                &answered,
                SETTLED,
                accepted,
                FIREWALL_DOMAIN_LIST_STATUS_VALUES,
                &format!("{case}: {operation}"),
            );
            // The whole set operation is applied inside the call, so the list
            // is settled by the time it answers — in the step that write
            // reached, which every later read reports.
            assert_step(
                &read_list(&router, &id).await,
                SETTLED,
                DOMAIN_LIST_CHANGED_MESSAGE,
                FIREWALL_DOMAIN_LIST_STATUS_VALUES,
                &format!("{case}: the read after {operation}"),
            );
        }

        // The delete has been accepted, so the list stands in the deleting step
        // and states that step's own sentence.
        let deleted = domain_list(
            &ok(
                &router,
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            )
            .await,
        );
        assert_step(
            &deleted,
            DELETING,
            DOMAIN_LIST_DELETING_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("{case}: the delete"),
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

/// Every projection of a rule group — the create, the read and the delete —
/// reports the step the group stands in as both `Status` and `StatusMessage`.
/// The delete has been accepted when it answers, so the group it answers with
/// stands in its deleting step.
#[tokio::test]
async fn every_rule_group_projection_states_the_step_the_group_stands_in() {
    let router = emulator();

    for ResourceCase { case, request } in resource_cases() {
        let made = rule_group(&ok(&router, "CreateFirewallRuleGroup", request).await);
        let id = string_member(&made, "Id");

        assert_step(
            &made,
            SETTLED,
            RULE_GROUP_CREATED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            &format!("{case}: the create"),
        );

        let read = rule_group(
            &ok(
                &router,
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            )
            .await,
        );
        assert_step(
            &read,
            SETTLED,
            RULE_GROUP_CREATED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            &format!("{case}: the read"),
        );

        let deleted = rule_group(
            &ok(
                &router,
                "DeleteFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            )
            .await,
        );
        assert_step(
            &deleted,
            DELETING,
            RULE_GROUP_DELETING_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            &format!("{case}: the delete"),
        );
        expect_error(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }
}

/// A rule group whose rules a caller has written states the step that change
/// reached and no longer the one its create did — the domain list's rule
/// ([`every_domain_list_projection_states_the_step_the_list_stands_in`]) over
/// the other resource that carries rules.
///
/// The rows are every write that changes what a group holds: a rule added to
/// it, that rule changed in it, and the rule removed from it again.
#[tokio::test]
async fn a_rule_group_states_the_step_its_last_rule_write_reached() {
    let router = emulator();
    let list = string_member(
        &domain_list(
            &ok(
                &router,
                "CreateFirewallDomainList",
                json!({ "CreatorRequestId": "req-rule-source", "Name": "rule-source" }),
            )
            .await,
        ),
        "Id",
    );
    let group = string_member(
        &rule_group(
            &ok(
                &router,
                "CreateFirewallRuleGroup",
                json!({ "CreatorRequestId": "req-written-group", "Name": "written-group" }),
            )
            .await,
        ),
        "Id",
    );

    let writes = vec![
        (
            "a rule added to the group",
            "CreateFirewallRule",
            json!({
                "CreatorRequestId": "req-written-rule",
                "FirewallRuleGroupId": group.clone(),
                "FirewallDomainListId": list.clone(),
                "Name": "written-rule",
                "Priority": 100,
                "Action": "ALERT",
            }),
        ),
        (
            "that rule changed in the group",
            "UpdateFirewallRule",
            json!({
                "FirewallRuleGroupId": group.clone(),
                "FirewallDomainListId": list.clone(),
                "Priority": 200,
            }),
        ),
        (
            "the rule removed from the group again",
            "DeleteFirewallRule",
            json!({
                "FirewallRuleGroupId": group.clone(),
                "FirewallDomainListId": list.clone(),
            }),
        ),
    ];

    for (case, operation, request) in writes {
        ok(&router, operation, request).await;
        let read = rule_group(
            &ok(
                &router,
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": group.clone() }),
            )
            .await,
        );
        assert_step(
            &read,
            SETTLED,
            RULE_GROUP_CHANGED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            case,
        );
    }
}

/// Every domain list DNS Firewall publishes itself stands in the settled step,
/// so a read of one states that step as well: the rule covers the whole
/// catalog, including the entries the lists this account creates never touch.
#[tokio::test]
async fn every_published_domain_list_states_the_step_it_stands_in() {
    let router = emulator();
    let listing = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 100 }),
    )
    .await;
    let published: Vec<(String, String)> = listing
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the domain lists this account is served")
        .iter()
        // "The owner of the list, used only for lists that are NOT managed by
        // you": the member is what tells a published entry from an own one.
        .filter(|item| item.get("ManagedOwnerName").is_some())
        .map(|item| (string_member(item, "Name"), string_member(item, "Id")))
        .collect();
    assert!(
        !published.is_empty(),
        "case \"the catalog\": the listing must report the lists DNS Firewall publishes, got {listing}"
    );

    for (name, id) in published {
        assert_step(
            &read_list(&router, &id).await,
            SETTLED,
            DOMAIN_LIST_CREATED_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("the published list {name}"),
        );
    }
}

/// The step is the resource's own: lists and groups created by different
/// requests, and read at different points in their lives, all state the
/// sentence of the step they stand in — so a list's message is never a group's,
/// and never a value the request supplied.
#[tokio::test]
async fn the_sentence_belongs_to_the_step_and_not_to_one_request() {
    let router = emulator();

    // (what the message was read from, the message) for four lists and four
    // groups, each made by its own request.
    let mut seen: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        let list = domain_list(
            &ok(
                &router,
                "CreateFirewallDomainList",
                json!({
                    "CreatorRequestId": format!("req-list-{index}"),
                    "Name": format!("list-{index}"),
                }),
            )
            .await,
        );
        let list_id = string_member(&list, "Id");
        let group = rule_group(
            &ok(
                &router,
                "CreateFirewallRuleGroup",
                json!({
                    "CreatorRequestId": format!("req-group-{index}"),
                    "Name": format!("group-{index}"),
                }),
            )
            .await,
        );
        seen.push((
            format!("the create of list {index}"),
            string_member(&list, "StatusMessage"),
        ));
        seen.push((
            format!("the create of group {index}"),
            string_member(&group, "StatusMessage"),
        ));

        // A list read back after a write to it still stands in a settled step,
        // and states the step that write reached.
        ok(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": list_id.clone(),
                "Operation": "ADD",
                "Domains": [format!("d-{index}.example.com")],
            }),
        )
        .await;
        seen.push((
            format!("the read of list {index} after a write"),
            string_member(&read_list(&router, &list_id).await, "StatusMessage"),
        ));
    }

    for (case, message) in &seen {
        let expected = if case.contains("group") {
            RULE_GROUP_CREATED_MESSAGE
        } else if case.contains("after a write") {
            DOMAIN_LIST_CHANGED_MESSAGE
        } else {
            DOMAIN_LIST_CREATED_MESSAGE
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected the settled step's sentence {expected:?}, got {message:?}"
        );
    }
    assert_eq!(
        seen.len(),
        12,
        "case \"four lists and four groups\": every projection was read, got {seen:?}"
    );
}
