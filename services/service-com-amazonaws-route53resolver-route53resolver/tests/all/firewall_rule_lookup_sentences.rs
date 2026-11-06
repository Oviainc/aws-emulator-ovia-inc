// SPDX-License-Identifier: CC0-1.0

//! The sentence Route 53 Resolver reports when no rule of the DNS Firewall rule
//! group a request named matches the source it named the rule by.
//!
//! RULE: a DNS Firewall rule is not addressed by an id of its own but INSIDE a
//! rule group, by the source it matches on — so a rule the group holds none of
//! is reported with the service's own error code for that fault, naming both
//! halves of what the request named: the rule group, and the member the rule was
//! named by with the id carried there (`FirewallDomainListId` for a domain-list
//! rule, `FirewallThreatProtectionId` for a DNS Firewall Advanced one).
//!
//! Every operation that addresses a stored rule resolves it through the one
//! lookup, so this suite asserts the SAME sentence across the operations that
//! name one — the single delete, one entry of a batch delete, and one entry of a
//! batch update — in both member forms. A rule named the same way is refused on
//! the same grounds whichever operation named it, which is what keeps a caller
//! able to read the refusal of one call the way it reads the other's.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts. The transport helpers are the shared ones in
//! [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53resolver_route53resolver::state::{
    FIREWALL_RULE_DOMAIN_LIST_MEMBER, FIREWALL_RULE_THREAT_PROTECTION_MEMBER,
    RSLVR_CODE_FIREWALL_RULE_NOT_FOUND,
};

use super::harness::{call, emulator, error_type, ok, string_member};

/// Ids of the shape this service mints for a domain list and for a DNS Firewall
/// Advanced rule that the store holds nothing under, so a request naming either
/// matches no rule of any group.
const UNHELD_DOMAIN_LIST_ID: &str = "rslvr-fdl-eeeeeeeeeeeeeeee";
const UNHELD_THREAT_PROTECTION_ID: &str = "rslvr-ftp-eeeeeeeeeeeeeeee";

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

/// The sentence a refusal states, read from either casing of the member the
/// awsJson error body carries it under.
fn refusal_message(body: &Value) -> String {
    string_member(body, "Message") + &string_member(body, "message")
}

/// The message the service reports for a rule the named group holds none of.
fn rule_not_found_message(group_id: &str, member: &str, match_source_id: &str) -> String {
    format!(
        r#"[{RSLVR_CODE_FIREWALL_RULE_NOT_FOUND}] Can’t locate the rule for the Firewall rule group ID: "{group_id}" and {member} "{match_source_id}"."#
    )
}

/// The one error a batch operation reports for the single entry it carried, as
/// `(Code, Message)`.
fn only_batch_error(case: &str, body: &Value, member: &str) -> (String, String) {
    let errors = body
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry {member} as a list, got {body}")
        });
    assert_eq!(
        errors.len(),
        1,
        "case {case:?}: the batch must report exactly one error for its one entry, got {body}"
    );
    (
        string_member(&errors[0], "Code"),
        refusal_message(&errors[0]),
    )
}

/// Every operation that names one stored rule reports the same sentence for a
/// rule the group holds none of, naming the group and the member the request
/// named the rule by.
///
/// The table spans the two match-source members a rule is named by and the three
/// operations that name one — the single delete, which reports the refusal as the
/// modelled exception, and the two batch operations, which report that same code
/// and sentence in the response body for the entry that caused it.
#[tokio::test]
async fn every_operation_naming_a_rule_reports_the_same_not_found_sentence() {
    let router = emulator();
    let group = rule_group(&router, "lookup-sentence-group").await;

    for (source_case, member, match_source) in [
        (
            "a domain list no rule of the group matches",
            FIREWALL_RULE_DOMAIN_LIST_MEMBER,
            json!({ "FirewallDomainListId": UNHELD_DOMAIN_LIST_ID }),
        ),
        (
            "a DNS Firewall Advanced rule id no rule of the group carries",
            FIREWALL_RULE_THREAT_PROTECTION_MEMBER,
            json!({ "FirewallThreatProtectionId": UNHELD_THREAT_PROTECTION_ID }),
        ),
    ] {
        let match_source_id = match_source
            .get("FirewallDomainListId")
            .or_else(|| match_source.get("FirewallThreatProtectionId"))
            .and_then(Value::as_str)
            .expect("the row names its rule by one of the two members")
            .to_string();
        let expected = rule_not_found_message(&group, member, &match_source_id);

        // The rule the entry names, as every one of the three operations states
        // it: the group, and the source the rule matches on.
        let mut entry = json!({ "FirewallRuleGroupId": group });
        let object = entry.as_object_mut().expect("the entry is an object");
        for (name, value) in match_source
            .as_object()
            .expect("the match source is an object")
        {
            object.insert(name.clone(), value.clone());
        }

        // The single delete: the refusal IS the call's answer, as the modelled
        // exception the operation declares for it.
        let case = format!("{source_case}, named by the single delete");
        let (status, refusal) = call(&router, "DeleteFirewallRule", entry.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the delete must answer HTTP 400, got {status}: {refusal}"
        );
        assert!(
            error_type(&refusal).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {refusal}"
        );
        assert_eq!(
            refusal_message(&refusal),
            expected,
            "case {case:?}: the refusal must state the sentence for a rule the group holds \
             none of, got {refusal}"
        );

        // The batch delete: the same code and the same sentence, reported for
        // the entry that caused them, with the call itself answering HTTP 200.
        let case = format!("{source_case}, named by one entry of a batch delete");
        let body = ok(
            &router,
            "BatchDeleteFirewallRule",
            json!({ "DeleteFirewallRuleEntries": [entry.clone()] }),
        )
        .await;
        let (code, message) = only_batch_error(&case, &body, "DeleteErrors");
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected Code ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: the entry's error must state the sentence the single delete \
             answers for it, got {body}"
        );

        // The batch update names its rule the same way, so it is refused on the
        // same grounds — the member and the id the entry named, not an id of a
        // rule's own.
        let case = format!("{source_case}, named by one entry of a batch update");
        let mut update_entry = entry.clone();
        update_entry
            .as_object_mut()
            .expect("the entry is an object")
            .insert("Priority".to_string(), json!(500));
        let body = ok(
            &router,
            "BatchUpdateFirewallRule",
            json!({ "UpdateFirewallRuleEntries": [update_entry] }),
        )
        .await;
        let (code, message) = only_batch_error(&case, &body, "UpdateErrors");
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected Code ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: the entry's error must state the same sentence the deletes \
             answer for the same rule, got {body}"
        );
    }
}

/// The sentence names the group the request named, so naming the same absent
/// rule under two different groups is refused with two different sentences — a
/// rule is addressed inside ONE group.
#[tokio::test]
async fn the_sentence_names_the_group_the_request_named() {
    let router = emulator();
    let first = rule_group(&router, "lookup-sentence-first-group").await;
    let second = rule_group(&router, "lookup-sentence-second-group").await;

    for (case, group) in [("the first group", &first), ("the second group", &second)] {
        let (status, refusal) = call(
            &router,
            "DeleteFirewallRule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": UNHELD_DOMAIN_LIST_ID,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the delete must answer HTTP 400, got {status}: {refusal}"
        );
        assert_eq!(
            refusal_message(&refusal),
            rule_not_found_message(group, FIREWALL_RULE_DOMAIN_LIST_MEMBER, UNHELD_DOMAIN_LIST_ID),
            "case {case:?}: the refusal must name the group the request named, got {refusal}"
        );
    }
}
