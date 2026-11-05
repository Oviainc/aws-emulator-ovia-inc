// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteFirewallRule`: the delete of ONE DNS Firewall
//! rule of one rule group.
//!
//! The rule under test is the one the operation documents: a request names one
//! rule of the group `FirewallRuleGroupId` states by the source that rule
//! matches on — `FirewallDomainListId` for a domain-list rule,
//! `FirewallThreatProtectionId` for a DNS Firewall Advanced one, with `Qtype`
//! telling apart the rules of one group that match the same source — and the
//! call removes exactly that rule from that group and answers with the rule as
//! the group held it. A request that names no single rule of the group is
//! refused and removes nothing, and no lifecycle state refuses the delete: it
//! "is the only operation that succeeds against a rule whose Status is
//! CREATION_FAILED".
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53resolver_route53resolver::state::{
    FIREWALL_RULE_DOMAIN_LIST_MEMBER, FIREWALL_RULE_STATUS_CREATING,
    FIREWALL_RULE_STATUS_CREATION_FAILED, FIREWALL_RULE_THREAT_PROTECTION_MEMBER,
    PROVISIONING_DELAY, RSLVR_CODE_FIREWALL_RULE_NOT_FOUND,
};

use super::harness::{call, emulator, error_type, ok, string_member, without_status};

/// An id of the shape this service mints for a rule group or a domain list that
/// the store holds nothing under, so a request naming it resolves to nothing.
const UNRESOLVABLE_RULE_GROUP_ID: &str = "rslvr-frg-ffffffffffffffff";
const UNRESOLVABLE_DOMAIN_LIST_ID: &str = "rslvr-fdl-ffffffffffffffff";

/// One Amazon Web Services-managed advanced threat category, as
/// `FirewallRuleType$FirewallAdvancedThreatCategory`'s doc names one
/// (`smithy_query(query="doc",shape="FirewallRuleType",member="FirewallAdvancedThreatCategory")`).
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

/// Create one rule from the members `members` states and answer it as the
/// create reported it.
async fn create_rule(router: &MultiServiceRouter, name: &str, members: Value) -> Value {
    let mut request = members;
    let object = request
        .as_object_mut()
        .expect("a rule's members are stated as an object");
    object.insert("Name".to_string(), json!(name));
    object.insert("CreatorRequestId".to_string(), json!(format!("rule-{name}")));
    let created = ok(router, "CreateFirewallRule", request).await;
    created
        .get("FirewallRule")
        .cloned()
        .unwrap_or_else(|| panic!("the create of {name:?} answers with the FirewallRule"))
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

/// The names of the rules the named group holds, sorted so a comparison does
/// not depend on the list order.
async fn listed_rule_names(router: &MultiServiceRouter, group_id: &str) -> Vec<String> {
    let mut names: Vec<String> = listed_rules(router, group_id)
        .await
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    names.sort();
    names
}

/// The rule the group holds under `name`, as `ListFirewallRules` reports it.
async fn held_rule(router: &MultiServiceRouter, group_id: &str, case: &str, name: &str) -> Value {
    listed_rules(router, group_id)
        .await
        .into_iter()
        .find(|rule| string_member(rule, "Name") == name)
        .unwrap_or_else(|| panic!("case {case:?}: the group must hold the rule {name:?}"))
}

/// The `RuleCount` the group reports, as `GetFirewallRuleGroup` reports it.
async fn rule_count(router: &MultiServiceRouter, group_id: &str) -> i64 {
    let group = ok(
        router,
        "GetFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": group_id }),
    )
    .await;
    group
        .get("FirewallRuleGroup")
        .and_then(|group| group.get("RuleCount"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("GetFirewallRuleGroup must report a RuleCount, got {group}"))
}

/// How a caller NAMES the stored rule `rule` of the group `group_id`: by the
/// source that rule matches on, plus the query type it evaluates where it
/// evaluates one.
fn naming(group_id: &str, rule: &Value) -> Value {
    let mut request = json!({ "FirewallRuleGroupId": group_id });
    let object = request.as_object_mut().expect("the request is an object");
    match rule.get("FirewallDomainListId").and_then(Value::as_str) {
        Some(domain_list_id) => {
            object.insert("FirewallDomainListId".to_string(), json!(domain_list_id));
        }
        None => {
            let threat_protection_id = rule
                .get("FirewallThreatProtectionId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("a rule matching no domain list must report a FirewallThreatProtectionId, got {rule}")
                });
            object.insert(
                "FirewallThreatProtectionId".to_string(),
                json!(threat_protection_id),
            );
        }
    }
    if let Some(qtype) = rule.get("Qtype").and_then(Value::as_str) {
        object.insert("Qtype".to_string(), json!(qtype));
    }
    request
}

/// The sentence a refusal states, read from either casing of the member the
/// awsJson error body carries it under.
fn refusal_message(body: &Value) -> String {
    string_member(body, "Message") + &string_member(body, "message")
}

/// The message Route 53 Resolver answers when no rule of the group a request
/// named matches the source it named the rule by: the service error code for
/// that fault, and both halves of what the request named — the rule group, and
/// the member it named the rule by with the id it carried there.
fn rule_not_found_message(group_id: &str, member: &str, match_source_id: &str) -> String {
    format!(
        r#"[{RSLVR_CODE_FIREWALL_RULE_NOT_FOUND}] Can’t locate the rule for the Firewall rule group ID: "{group_id}" and {member} "{match_source_id}"."#
    )
}

/// A delete removes the one rule the request names and answers with that rule
/// exactly as the group held it; the rule is then gone from every later read,
/// and naming it again resolves to nothing.
///
/// The table spans what the rule generalises over — the match source naming the
/// rule (a domain list, the id of a DNS Firewall Advanced rule), the kind of
/// source the rule matches on (a domain list, a built-in detector, a managed
/// threat category), the query types it evaluates (one, all of them), and the
/// set of members it carries (a block with an override, a bare alert).
#[tokio::test]
async fn a_delete_removes_the_rule_it_names_and_reports_it_as_held() {
    let router = emulator();
    let group = rule_group(&router, "delete-rule-group").await;
    let override_list = domain_list(&router, "delete-rule-override-list").await;
    let alert_list = domain_list(&router, "delete-rule-alert-list").await;

    let rows = vec![
        (
            "a blocking domain-list rule with an override, evaluating one query type",
            "override-rule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": override_list,
                "Priority": 100,
                "Action": "BLOCK",
                "BlockResponse": "OVERRIDE",
                "BlockOverrideDomain": "example.test",
                "BlockOverrideDnsType": "CNAME",
                "BlockOverrideTtl": 60,
                "FirewallDomainRedirectionAction": "TRUST_REDIRECTION_DOMAIN",
                "Qtype": "AAAA",
            }),
        ),
        (
            "an alerting domain-list rule evaluating every query type",
            "alert-rule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": alert_list,
                "Priority": 200,
                "Action": "ALERT",
            }),
        ),
        (
            "a rule matching a built-in detector",
            "detector-rule",
            json!({
                "FirewallRuleGroupId": group,
                "DnsThreatProtection": "DGA",
                "ConfidenceThreshold": "HIGH",
                "Priority": 300,
                "Action": "ALERT",
            }),
        ),
        (
            "a rule matching a managed threat category",
            "category-rule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallRuleType": {
                    "FirewallAdvancedThreatCategory": { "Category": MANAGED_THREAT_CATEGORY },
                },
                "Priority": 400,
                "Action": "BLOCK",
                "BlockResponse": "NODATA",
            }),
        ),
    ];

    for (case, name, members) in &rows {
        let created = create_rule(&router, name, members.clone()).await;
        let held = held_rule(&router, &group, case, name).await;
        assert_eq!(
            without_status(&held),
            without_status(&created),
            "case {case:?}: the group must report the rule the create answered with"
        );
    }

    let mut remaining = rows.len() as i64;
    assert_eq!(
        rule_count(&router, &group).await,
        remaining,
        "the group's RuleCount must count every rule created into it"
    );

    for (case, name, _) in &rows {
        let held = held_rule(&router, &group, case, name).await;
        let request = naming(&group, &held);

        let (status, body) = call(&router, "DeleteFirewallRule", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete naming one rule of the group must answer HTTP 200, \
             got {status}: {body}"
        );
        let reported = body
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {case:?}: the delete must answer with the rule it \
                                      deleted, got {body}"));
        assert_eq!(
            without_status(reported),
            without_status(&held),
            "case {case:?}: the deleted rule must be reported as the group held it"
        );

        // The one removal is what every later read follows from: the rule is
        // gone from the group's listing and out of its `RuleCount`.
        assert!(
            !listed_rule_names(&router, &group)
                .await
                .contains(&name.to_string()),
            "case {case:?}: the group must no longer hold the rule the delete removed"
        );
        remaining -= 1;
        assert_eq!(
            rule_count(&router, &group).await,
            remaining,
            "case {case:?}: the group's RuleCount must count exactly the rules left"
        );

        // A rule is deleted once: the same request again names no rule at all,
        // and the refusal states both halves of what the request named.
        let (match_source_member, match_source_id) = match request
            .get("FirewallDomainListId")
            .and_then(Value::as_str)
        {
            Some(domain_list_id) => (FIREWALL_RULE_DOMAIN_LIST_MEMBER, domain_list_id.to_string()),
            None => (
                FIREWALL_RULE_THREAT_PROTECTION_MEMBER,
                string_member(&request, "FirewallThreatProtectionId"),
            ),
        };
        let (status, refusal) = call(&router, "DeleteFirewallRule", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a repeat of the same delete must answer HTTP 400, got {status}: \
             {refusal}"
        );
        assert!(
            error_type(&refusal).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {refusal}"
        );
        assert_eq!(
            refusal_message(&refusal),
            rule_not_found_message(&group, match_source_member, &match_source_id),
            "case {case:?}: the refusal must state the sentence for a rule the group holds \
             none of, got {refusal}"
        );
    }

    assert_eq!(
        listed_rule_names(&router, &group).await,
        Vec::<String>::new(),
        "every rule the group held was deleted, so it lists none"
    );
}

/// Among the rules of one group that match the SAME source, the `Qtype` the
/// request names is what tells them apart — and a rule that evaluates every
/// query type stores none, so a request naming no `Qtype` names that rule.
#[tokio::test]
async fn qtype_tells_apart_the_rules_matching_one_source() {
    let router = emulator();
    let group = rule_group(&router, "delete-rule-qtype-group").await;
    let shared = domain_list(&router, "delete-rule-shared-list").await;

    // One rule per query type the group evaluates that source for, plus the one
    // that evaluates every query type: each row deletes exactly one of them,
    // named by the `Qtype` it evaluates. The query types are values
    // `DeleteFirewallRule$Qtype`'s doc lists
    // (`smithy_query(query="doc",shape="DeleteFirewallRuleRequest",member="Qtype")`).
    for (name, priority, qtype) in [
        ("shared-aaaa", 100, Some("AAAA")),
        ("shared-mx", 200, Some("MX")),
        ("shared-txt", 300, Some("TXT")),
        ("shared-any", 400, None),
    ] {
        let mut members = json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared,
            "Priority": priority,
            "Action": "ALERT",
        });
        if let Some(qtype) = qtype {
            members
                .as_object_mut()
                .expect("the members are an object")
                .insert("Qtype".to_string(), json!(qtype));
        }
        create_rule(&router, name, members).await;
    }

    let mut left = vec![
        "shared-aaaa".to_string(),
        "shared-any".to_string(),
        "shared-mx".to_string(),
        "shared-txt".to_string(),
    ];
    for (case, qtype, deletes) in [
        ("the rule evaluating MX", Some("MX"), "shared-mx"),
        ("the rule evaluating AAAA", Some("AAAA"), "shared-aaaa"),
        ("the rule evaluating TXT", Some("TXT"), "shared-txt"),
        (
            "the rule evaluating every query type, named by no Qtype",
            None,
            "shared-any",
        ),
    ] {
        let mut request = json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared,
        });
        if let Some(qtype) = qtype {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert("Qtype".to_string(), json!(qtype));
        }

        let body = ok(&router, "DeleteFirewallRule", request).await;
        let deleted = body
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {case:?}: the delete must answer with a rule, got {body}"));
        assert_eq!(
            string_member(deleted, "Name"),
            deletes,
            "case {case:?}: expected the delete to remove {deletes}, got {deleted}"
        );

        left.retain(|name| name != deletes);
        assert_eq!(
            listed_rule_names(&router, &group).await,
            left,
            "case {case:?}: the group must still hold every other rule of that source"
        );
    }
}

/// A request that names no single rule of the group is refused, with the error
/// the model declares for that class of fault — and it removes nothing.
///
/// The table spans the kinds of fault: an unresolvable rule group, a source the
/// group holds no rule for, a rule of ANOTHER group, a source several rules
/// match that no `Qtype` tells apart, both match-source ids at once, and
/// neither of them.
#[tokio::test]
async fn a_delete_naming_no_single_rule_is_refused_and_removes_nothing() {
    let router = emulator();
    let group = rule_group(&router, "delete-rule-refusal-group").await;
    let other_group = rule_group(&router, "delete-rule-other-group").await;
    let shared = domain_list(&router, "delete-rule-refusal-shared-list").await;
    let elsewhere = domain_list(&router, "delete-rule-elsewhere-list").await;

    // Two rules of the group match `shared`, which is what makes `Qtype` the
    // member that tells them apart.
    create_rule(
        &router,
        "refusal-any-qtype",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared,
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;
    create_rule(
        &router,
        "refusal-aaaa",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared,
            "Priority": 110,
            "Action": "ALERT",
            "Qtype": "AAAA",
        }),
    )
    .await;
    let detector = create_rule(
        &router,
        "refusal-detector",
        json!({
            "FirewallRuleGroupId": group,
            "DnsThreatProtection": "DNS_TUNNELING",
            "ConfidenceThreshold": "LOW",
            "Priority": 120,
            "Action": "ALERT",
        }),
    )
    .await;
    let detector_id = string_member(&detector, "FirewallThreatProtectionId");
    // A rule of ANOTHER group: the group a request names is the only group its
    // rule is looked for in.
    create_rule(
        &router,
        "elsewhere-rule",
        json!({
            "FirewallRuleGroupId": other_group,
            "FirewallDomainListId": elsewhere,
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;

    // The codes are the modelled shape names of this service's exceptions
    // (`smithy_query(query="operation",operation="DeleteFirewallRule")`), and a
    // refusal states the sentence Route 53 Resolver words that fault with.
    for (case, request, expected, states) in [
        (
            "an unresolvable rule group",
            json!({
                "FirewallRuleGroupId": UNRESOLVABLE_RULE_GROUP_ID,
                "FirewallDomainListId": shared,
            }),
            "ResourceNotFoundException",
            Some(UNRESOLVABLE_RULE_GROUP_ID.to_string()),
        ),
        (
            "a domain list the group holds no rule for",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": UNRESOLVABLE_DOMAIN_LIST_ID,
            }),
            "ResourceNotFoundException",
            Some(rule_not_found_message(
                &group,
                FIREWALL_RULE_DOMAIN_LIST_MEMBER,
                UNRESOLVABLE_DOMAIN_LIST_ID,
            )),
        ),
        (
            "a rule of another group",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": elsewhere,
            }),
            "ResourceNotFoundException",
            Some(rule_not_found_message(
                &group,
                FIREWALL_RULE_DOMAIN_LIST_MEMBER,
                &elsewhere,
            )),
        ),
        (
            "a DNS Firewall Advanced rule of another group",
            json!({
                "FirewallRuleGroupId": other_group,
                "FirewallThreatProtectionId": detector_id,
            }),
            "ResourceNotFoundException",
            Some(rule_not_found_message(
                &other_group,
                FIREWALL_RULE_THREAT_PROTECTION_MEMBER,
                &detector_id,
            )),
        ),
        (
            "a source several rules match, with a Qtype none of them evaluates",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared,
                "Qtype": "SOA",
            }),
            "ValidationException",
            Some(shared.clone()),
        ),
        (
            "both match-source ids at once",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared,
                "FirewallThreatProtectionId": detector_id,
            }),
            "ValidationException",
            None,
        ),
        (
            "neither match-source id",
            json!({ "FirewallRuleGroupId": group }),
            "ValidationException",
            None,
        ),
    ] {
        let (status, refusal) = call(&router, "DeleteFirewallRule", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the delete must answer HTTP 400, got {status}: {refusal}"
        );
        assert!(
            error_type(&refusal).contains(expected),
            "case {case:?}: expected {expected}, got {refusal}"
        );
        let message = refusal_message(&refusal);
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must state why the request named no rule, got {refusal}"
        );
        if let Some(states) = states {
            assert!(
                message.contains(&states),
                "case {case:?}: the refusal must state {states:?}, got {message:?}"
            );
        }
    }

    // A refusal writes nothing: every rule either group held is still there.
    assert_eq!(
        listed_rule_names(&router, &group).await,
        vec![
            "refusal-aaaa".to_string(),
            "refusal-any-qtype".to_string(),
            "refusal-detector".to_string(),
        ],
        "a refused delete must remove no rule of the group it named"
    );
    assert_eq!(
        rule_count(&router, &group).await,
        3,
        "the group's RuleCount must be unchanged by refused deletes"
    );
    assert_eq!(
        listed_rule_names(&router, &other_group).await,
        vec!["elsewhere-rule".to_string()],
        "a delete naming one group must remove no rule of another group"
    );
}

/// A rule is deletable in every lifecycle state it can report: the delete is
/// "the only operation that succeeds against a rule whose Status is
/// CREATION_FAILED", and it succeeds against one still CREATING too.
#[tokio::test]
async fn a_rule_is_deleted_in_every_lifecycle_state_it_reports() {
    let router = emulator();
    let group = rule_group(&router, "delete-rule-lifecycle-group").await;

    // A rule matching a partner threat feed is the one match source whose
    // provisioning is asynchronous, so such a rule begins in CREATING and
    // settles on the state the clock owes it.
    for (case, name, partner, priority, settle, expected_status) in [
        (
            "a rule whose provisioning has not settled",
            "lifecycle-creating-rule",
            "example-threat-feed",
            100,
            false,
            FIREWALL_RULE_STATUS_CREATING,
        ),
        (
            "a rule whose provisioning failed",
            "lifecycle-failed-rule",
            "other-threat-feed",
            200,
            true,
            FIREWALL_RULE_STATUS_CREATION_FAILED,
        ),
    ] {
        let created = create_rule(
            &router,
            name,
            json!({
                "FirewallRuleGroupId": group,
                "FirewallRuleType": { "PartnerThreatProtection": { "Partner": partner } },
                "Priority": priority,
                "Action": "ALERT",
            }),
        )
        .await;
        let threat_protection_id = string_member(&created, "FirewallThreatProtectionId");
        if settle {
            tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
        }

        let body = ok(
            &router,
            "DeleteFirewallRule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": threat_protection_id,
            }),
        )
        .await;
        let deleted = body
            .get("FirewallRule")
            .unwrap_or_else(|| panic!("case {case:?}: the delete must answer with a rule, got {body}"));
        assert_eq!(
            deleted.get("Status"),
            Some(&json!(expected_status)),
            "case {case:?}: expected Status {expected_status}, got {deleted}"
        );
        assert!(
            !listed_rule_names(&router, &group)
                .await
                .contains(&name.to_string()),
            "case {case:?}: the group must no longer hold the rule the delete removed"
        );
    }

    assert_eq!(
        rule_count(&router, &group).await,
        0,
        "every rule the group held was deleted, so it holds none"
    );
}
