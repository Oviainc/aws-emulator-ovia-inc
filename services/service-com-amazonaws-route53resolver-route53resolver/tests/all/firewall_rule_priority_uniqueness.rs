// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the uniqueness of a DNS Firewall rule's `Priority`
//! inside its rule group, and for the BODY of the refusal a create that breaks
//! it is answered with.
//!
//! The rule under test: `CreateFirewallRule$Priority`'s doc states "you must
//! specify a unique priority for each rule in a rule group", so a priority is
//! unique over the whole GROUP and over nothing narrower — no other member of
//! the rule (the query type it evaluates, its action, its block response, its
//! match source, its name) scopes that uniqueness — and a create that asks for a
//! priority a rule of the group already holds is refused with the one
//! service-coded message for that fault, naming the rule group, while a priority
//! no rule of THAT group holds is created even when a rule of another group
//! holds it.
//!
//! The table spans what the rule generalises over: the member by which the
//! second rule differs from the first (nothing, a query type present on one side
//! only, two different query types, the same query type, the action, the block
//! response, the match source, the domain list, the name) and the priority
//! itself, from zero through the integer the model's `Priority` admits at either
//! extreme.
//!
//! The message is asserted in FULL: the service answers this refusal with the
//! code, the fault sentence and the sentence that says what to send instead, so
//! no part of it is cut before the comparison.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, string_member};

/// The whole message the service answers for a rule whose `Priority` another
/// rule of its group already holds, written out here rather than built from the
/// emulator's own constants so the wording itself is what this suite pins: the
/// service error code in square brackets, the fault sentence with the rule group
/// double-quoted inside it, and the sentence that states what to send instead.
fn expected_message(firewall_rule_group_id: &str) -> String {
    format!(
        "[RSLVR-02203] The DNS Firewall rule group already has a rule associated with the same \
         priority: \"{firewall_rule_group_id}\". Provide another priority."
    )
}

/// An Amazon Web Services-managed advanced threat category, as the model's
/// `FirewallRuleType$FirewallAdvancedThreatCategory` doc names one ("for
/// example, PHISHING"). One category is enough here: what this suite varies is
/// the rule's match SOURCE, not the catalog behind one source.
const MANAGED_THREAT_CATEGORY: &str = "PHISHING";

/// Create one rule group and answer the id Route 53 Resolver minted for it.
async fn rule_group(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": format!("group-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallRuleGroup")
            .expect("the create answers with the FirewallRuleGroup"),
        "Id",
    )
}

/// Create one domain list and answer the id Route 53 Resolver minted for it.
async fn domain_list(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallDomainList",
        json!({ "CreatorRequestId": format!("list-{name}"), "Name": name }),
    )
    .await;
    string_member(
        created
            .get("FirewallDomainList")
            .expect("the create answers with the FirewallDomainList"),
        "Id",
    )
}

/// The rules the named group holds, as `ListFirewallRules` reports them.
async fn listed_rules(router: &MultiServiceRouter, group_id: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group_id }),
    )
    .await;
    listed
        .get("FirewallRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One `CreateFirewallRule` request body: the members every rule needs, plus
/// whatever the row states.
fn rule_body(group_id: &str, name: &str, priority: i64, members: &Value) -> Value {
    let mut body = json!({
        "CreatorRequestId": name,
        "FirewallRuleGroupId": group_id,
        "Name": name,
        "Priority": priority,
    });
    let object = body.as_object_mut().expect("the request body is an object");
    for (member, value) in members
        .as_object()
        .expect("the row states its members as an object")
        .clone()
    {
        object.insert(member, value);
    }
    body
}

/// One row of the clash table: a rule the group holds, and a second rule asking
/// for the same priority while differing from the first in the member the row
/// names.
struct ClashCase {
    case: &'static str,
    /// The priority BOTH rules ask for.
    priority: i64,
    /// What the rule already in the group carries beside `CreatorRequestId`,
    /// `FirewallRuleGroupId`, `Name` and `Priority`.
    held: Value,
    /// The same, for the rule whose create must be refused.
    asked: Value,
}

/// The rows, built against the two domain lists the suite created.
fn clash_cases(own_list: &str, other_list: &str) -> Vec<ClashCase> {
    vec![
        ClashCase {
            case: "two rules alike but for their name",
            priority: 100,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT" }),
            asked: json!({ "FirewallDomainListId": own_list, "Action": "ALERT" }),
        },
        ClashCase {
            case: "a query type on the held rule alone",
            priority: 710,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "A" }),
            asked: json!({ "FirewallDomainListId": own_list, "Action": "ALERT" }),
        },
        ClashCase {
            case: "a query type on the asked rule alone",
            priority: 1,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT" }),
            asked: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "AAAA" }),
        },
        ClashCase {
            case: "two different query types",
            priority: 2_500,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "A" }),
            asked: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "MX" }),
        },
        ClashCase {
            case: "the same query type on both",
            priority: 9_900,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "CNAME" }),
            asked: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "CNAME" }),
        },
        ClashCase {
            case: "a different action and block response",
            priority: 0,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT" }),
            asked: json!({
                "FirewallDomainListId": own_list,
                "Action": "BLOCK",
                "BlockResponse": "NXDOMAIN",
            }),
        },
        ClashCase {
            case: "two block responses that differ",
            priority: -1,
            held: json!({
                "FirewallDomainListId": own_list,
                "Action": "BLOCK",
                "BlockResponse": "NODATA",
            }),
            asked: json!({
                "FirewallDomainListId": own_list,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "safe.example.com",
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
            }),
        },
        ClashCase {
            case: "another domain list as the match source",
            priority: 2_147_483_647,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT", "Qtype": "TXT" }),
            asked: json!({ "FirewallDomainListId": other_list, "Action": "ALERT" }),
        },
        ClashCase {
            case: "a threat detector as the match source",
            priority: -2_147_483_648,
            held: json!({ "FirewallDomainListId": own_list, "Action": "ALERT" }),
            asked: json!({
                "DnsThreatProtection": "DNS_TUNNELING",
                "ConfidenceThreshold": "MEDIUM",
                "Action": "ALERT",
            }),
        },
        ClashCase {
            case: "a managed threat category as the match source",
            priority: 4_242,
            held: json!({
                "DnsThreatProtection": "DGA",
                "ConfidenceThreshold": "LOW",
                "Action": "ALERT",
            }),
            asked: json!({
                "FirewallRuleType": {
                    "FirewallAdvancedThreatCategory": { "Category": MANAGED_THREAT_CATEGORY },
                },
                "Action": "BLOCK",
                "BlockResponse": "NODATA",
            }),
        },
    ]
}

/// Read the refusal `CreateFirewallRule` answers for `body`, requiring the
/// status and the error shape the service refuses a clashing priority with, and
/// answer its `Message`.
async fn refusal_message(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, refusal) = call(router, "CreateFirewallRule", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: the create must answer HTTP 400, got {status}: {refusal}"
    );
    let answered = error_type(&refusal);
    assert!(
        answered.contains("ValidationException"),
        "case {case:?}: the create must answer ValidationException, got {answered:?}: {refusal}"
    );
    string_member(&refusal, "Message")
}

/// A priority a rule of the group already holds is refused whatever else the
/// asked rule states, with the whole service-coded message naming the group, and
/// the refused create writes nothing.
#[tokio::test]
async fn a_priority_the_group_holds_is_refused_whatever_else_the_rule_states() {
    let router = emulator();
    let own_list = domain_list(&router, "priority-clash-list").await;
    let other_list = domain_list(&router, "priority-clash-other-list").await;

    for ClashCase {
        case,
        priority,
        held,
        asked,
    } in clash_cases(&own_list, &other_list)
    {
        // A group per row, so what the row asserts is the clash it set up and
        // not a rule some earlier row left behind.
        let group = rule_group(&router, &format!("clash-{}", case.replace(' ', "-"))).await;
        let held_name = format!("held-{}", case.replace(' ', "-"));
        let created = ok(
            &router,
            "CreateFirewallRule",
            rule_body(&group, &held_name, priority, &held),
        )
        .await;
        assert_eq!(
            created
                .get("FirewallRule")
                .and_then(|rule| rule.get("Priority"))
                .and_then(Value::as_i64),
            Some(priority),
            "case {case:?}: the held rule must report the priority it was created with, \
             got {created}"
        );

        let asked_name = format!("asked-{}", case.replace(' ', "-"));
        let message = refusal_message(
            &router,
            case,
            rule_body(&group, &asked_name, priority, &asked),
        )
        .await;
        let expected = expected_message(&group);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        let held_rules = listed_rules(&router, &group).await;
        let names: Vec<String> = held_rules
            .iter()
            .map(|rule| string_member(rule, "Name"))
            .collect();
        assert_eq!(
            names,
            vec![held_name.clone()],
            "case {case:?}: a refused create must leave the group holding only the rule it \
             already had, got {names:?}"
        );
    }
}

/// The uniqueness is scoped to the rule group: the same priority is created in
/// another group, and a priority no rule of the group holds is created in it —
/// so the refusal above is the clash and not the number.
#[tokio::test]
async fn a_priority_is_unique_within_one_group_and_not_across_groups() {
    let router = emulator();
    let list = domain_list(&router, "priority-scope-list").await;
    let first_group = rule_group(&router, "priority-scope-a").await;
    let second_group = rule_group(&router, "priority-scope-b").await;

    // The same priorities, asked for in two groups: every one of them is
    // created in both, because neither group holds it yet.
    for priority in [0_i64, 100, 710, 9_900, 2_147_483_647] {
        for (index, group) in [&first_group, &second_group].into_iter().enumerate() {
            let name = format!("scoped-{priority}-{index}");
            let created = ok(
                &router,
                "CreateFirewallRule",
                rule_body(
                    group,
                    &name,
                    priority,
                    &json!({ "FirewallDomainListId": list, "Action": "ALERT" }),
                ),
            )
            .await;
            let rule = created
                .get("FirewallRule")
                .unwrap_or_else(|| panic!("case {name:?}: the create answers a rule, {created}"));
            assert_eq!(
                rule.get("FirewallRuleGroupId").and_then(Value::as_str),
                Some(group.as_str()),
                "case {name:?}: the rule belongs to the group it was created in, got {rule}"
            );
            assert_eq!(
                rule.get("Priority").and_then(Value::as_i64),
                Some(priority),
                "case {name:?}: the rule reports the priority it asked for, got {rule}"
            );
        }
    }

    for group in [&first_group, &second_group] {
        let priorities: Vec<i64> = listed_rules(&router, group)
            .await
            .iter()
            .filter_map(|rule| rule.get("Priority").and_then(Value::as_i64))
            .collect();
        assert_eq!(
            priorities,
            vec![0, 100, 710, 9_900, 2_147_483_647],
            "group {group:?}: every distinct priority asked for must be held, got {priorities:?}"
        );
    }
}

/// A batch create decides each entry through the same rule, so an entry whose
/// priority an earlier entry — or a rule already in the group — holds is
/// reported with the code and the very message the single create answers for
/// it, while the entries around it are created.
#[tokio::test]
async fn a_batch_entry_with_a_held_priority_reports_the_same_message() {
    let router = emulator();
    let list = domain_list(&router, "batch-priority-list").await;
    let group = rule_group(&router, "batch-priority-group").await;

    ok(
        &router,
        "CreateFirewallRule",
        rule_body(
            &group,
            "batch-priority-held",
            300,
            &json!({ "FirewallDomainListId": list, "Action": "ALERT", "Qtype": "A" }),
        ),
    )
    .await;

    let entries = vec![
        // A priority the group already holds, asked for with another query type.
        rule_body(
            &group,
            "batch-priority-clash-stored",
            300,
            &json!({ "FirewallDomainListId": list, "Action": "ALERT", "Qtype": "SRV" }),
        ),
        // A priority no rule holds, which this entry takes.
        rule_body(
            &group,
            "batch-priority-fresh",
            400,
            &json!({ "FirewallDomainListId": list, "Action": "ALERT" }),
        ),
        // The priority the entry before it just took.
        rule_body(
            &group,
            "batch-priority-clash-entry",
            400,
            &json!({ "FirewallDomainListId": list, "Action": "BLOCK", "BlockResponse": "NODATA" }),
        ),
    ];

    let body = ok(
        &router,
        "BatchCreateFirewallRule",
        json!({ "CreateFirewallRuleEntries": entries }),
    )
    .await;

    let created: Vec<String> = body
        .get("CreatedFirewallRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    assert_eq!(
        created,
        vec!["batch-priority-fresh".to_string()],
        "the batch must create exactly the entry whose priority no rule held, got {created:?} \
         from {body}"
    );

    let errors = body
        .get("CreateErrors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let expected = expected_message(&group);
    let reported: Vec<(String, String, String)> = errors
        .iter()
        .map(|error| {
            (
                error
                    .get("FirewallRule")
                    .map(|entry| string_member(entry, "Name"))
                    .unwrap_or_default(),
                string_member(error, "Code"),
                string_member(error, "Message"),
            )
        })
        .collect();
    let want: Vec<(String, String, String)> = [
        "batch-priority-clash-stored",
        "batch-priority-clash-entry",
    ]
    .into_iter()
    .map(|name| {
        (
            name.to_string(),
            "ValidationException".to_string(),
            expected.clone(),
        )
    })
    .collect();
    assert_eq!(
        reported, want,
        "the batch must report each clashing entry with the code and the message the single \
         create answers for it: expected {want:?}, got {reported:?}"
    );

    let held: Vec<i64> = listed_rules(&router, &group)
        .await
        .iter()
        .filter_map(|rule| rule.get("Priority").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        held,
        vec![300, 400],
        "the group must hold one rule per distinct priority, got {held:?}"
    );
}
