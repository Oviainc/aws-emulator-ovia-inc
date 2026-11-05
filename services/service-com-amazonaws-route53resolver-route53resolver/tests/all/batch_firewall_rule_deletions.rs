// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchDeleteFirewallRule`: the batch delete of DNS
//! Firewall rules, which decides EVERY entry on its own.
//!
//! The rule under test is the one the operation's two output members state: an
//! entry names one rule of a rule group by the source that rule matches on —
//! `FirewallDomainListId` for a domain-list rule, `FirewallThreatProtectionId`
//! for a DNS Firewall Advanced one, with `Qtype` telling apart the rules of one
//! group that match the same source — and the call removes the rule it names
//! from that group, reporting the rules it removed in `DeletedFirewallRules`
//! and the entries it could not remove in `DeleteErrors`. So one call can
//! succeed in part, an entry that is refused neither fails the call nor stops
//! the entries around it from being applied, and a batch carrying more entries
//! than one request may is refused WHOLE.
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
    FIREWALL_RULE_STATUS_CREATING, FIREWALL_RULE_STATUS_CREATION_FAILED,
    MAX_FIREWALL_RULE_ENTRIES_PER_BATCH, PROVISIONING_DELAY,
};

use super::harness::{call, emulator, error_type, ok, string_member, without_status};

/// An id of the shape this service mints for a rule group or a domain list that
/// the store holds nothing under, so a request naming it resolves to nothing.
const UNRESOLVABLE_RULE_GROUP_ID: &str = "rslvr-frg-ffffffffffffffff";
const UNRESOLVABLE_DOMAIN_LIST_ID: &str = "rslvr-fdl-ffffffffffffffff";

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

/// The list member `member` of a response, which the batch delete reports for
/// every call — empty or not.
fn list_member(case: &str, body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry {member} as a list, got {body}")
        })
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

/// The names of the rules the named group holds, as `ListFirewallRules` reports
/// them, sorted so a comparison does not depend on the list order.
async fn listed_rule_names(router: &MultiServiceRouter, group_id: &str) -> Vec<String> {
    let mut names: Vec<String> = listed_rules(router, group_id)
        .await
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    names.sort();
    names
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

/// One row of the entry table: the entry a batch carries, and what the call must
/// answer for it — either the rule it deleted or the error code it was refused
/// with.
struct EntryCase {
    case: &'static str,
    /// Everything the entry carries.
    members: Value,
    /// The `Name` of the rule this entry must delete, or `None` for an entry the
    /// call must refuse.
    deletes: Option<&'static str>,
    /// The `Code` the refusal must report, or `None` for an entry the call must
    /// apply.
    ///
    /// The codes are the modelled shape names of this service's exceptions
    /// (`smithy_query(query="closure",operation="DeleteFirewallRule")`), which
    /// is what the single delete answers with on the wire for the same entry.
    code: Option<&'static str>,
}

/// A batch delete applies each entry on its own: it removes every rule it can
/// resolve, refuses the rest with the code and the sentence the single delete
/// answers for that entry, and answers HTTP 200 either way.
///
/// The table spans what the rule generalises over — the entry's outcome
/// (deleted or refused), the match source naming the rule (a domain list, a
/// threat-protection id), how many rules of the group match that source (one,
/// several told apart by `Qtype`, none), and the kind of fault (an unresolvable
/// rule group, a source the group holds no rule for, a source several rules
/// match that no `Qtype` tells apart, both ids at once, neither id, a rule an
/// earlier entry of the same call already removed).
#[tokio::test]
async fn batch_delete_decides_every_entry_on_its_own() {
    let router = emulator();
    let group = rule_group(&router, "batch-delete-group").await;
    let shared_list = domain_list(&router, "batch-delete-shared-list").await;
    let other_list = domain_list(&router, "batch-delete-other-list").await;
    let spare_list = domain_list(&router, "batch-delete-spare-list").await;

    // Two rules of the group match `shared_list`, which is what makes `Qtype`
    // the member that tells them apart: one evaluates every query type and
    // stores none, the other evaluates AAAA alone.
    create_rule(
        &router,
        "alpha-any-qtype",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared_list,
            "Priority": 100,
            "Action": "ALERT",
        }),
    )
    .await;
    create_rule(
        &router,
        "alpha-aaaa",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": shared_list,
            "Priority": 110,
            "Action": "ALERT",
            "Qtype": "AAAA",
        }),
    )
    .await;
    create_rule(
        &router,
        "beta-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": other_list,
            "Priority": 120,
            "Action": "BLOCK",
            "BlockResponse": "NXDOMAIN",
        }),
    )
    .await;
    // A DNS Firewall Advanced rule is not a domain-list rule: it carries a
    // `FirewallThreatProtectionId` of its own, which is what names it.
    let detector_rule = create_rule(
        &router,
        "detector-rule",
        json!({
            "FirewallRuleGroupId": group,
            "DnsThreatProtection": "DGA",
            "ConfidenceThreshold": "HIGH",
            "Priority": 130,
            "Action": "ALERT",
        }),
    )
    .await;
    let detector_id = string_member(&detector_rule, "FirewallThreatProtectionId");
    assert!(
        !detector_id.is_empty(),
        "a DNS Firewall Advanced rule must report a FirewallThreatProtectionId, got \
         {detector_rule}"
    );
    create_rule(
        &router,
        "untouched-rule",
        json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": spare_list,
            "Priority": 140,
            "Action": "ALERT",
        }),
    )
    .await;

    let cases = vec![
        EntryCase {
            case: "an unresolvable rule group",
            members: json!({
                "FirewallRuleGroupId": UNRESOLVABLE_RULE_GROUP_ID,
                "FirewallDomainListId": shared_list,
            }),
            deletes: None,
            code: Some("ResourceNotFoundException"),
        },
        EntryCase {
            case: "a source several rules match, with a Qtype none of them evaluates",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared_list,
                "Qtype": "TXT",
            }),
            deletes: None,
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "the rule of that source evaluating the Qtype the entry names",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared_list,
                "Qtype": "AAAA",
            }),
            deletes: Some("alpha-aaaa"),
            code: None,
        },
        EntryCase {
            case: "the one rule of that source left, naming no Qtype",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared_list,
            }),
            deletes: Some("alpha-any-qtype"),
            code: None,
        },
        EntryCase {
            case: "a rule an earlier entry of the same call already removed",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": shared_list,
            }),
            deletes: None,
            code: Some("ResourceNotFoundException"),
        },
        EntryCase {
            case: "both match-source ids at once",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": other_list,
                "FirewallThreatProtectionId": detector_id,
            }),
            deletes: None,
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "neither match-source id",
            members: json!({ "FirewallRuleGroupId": group }),
            deletes: None,
            code: Some("ValidationException"),
        },
        EntryCase {
            case: "a DNS Firewall Advanced rule by its threat-protection id",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": detector_id,
            }),
            deletes: Some("detector-rule"),
            code: None,
        },
        EntryCase {
            case: "a domain list the group holds no rule for",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": UNRESOLVABLE_DOMAIN_LIST_ID,
            }),
            deletes: None,
            code: Some("ResourceNotFoundException"),
        },
        EntryCase {
            case: "the one rule matching another domain list",
            members: json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": other_list,
            }),
            deletes: Some("beta-rule"),
            code: None,
        },
    ];

    let entries: Vec<Value> = cases.iter().map(|case| case.members.clone()).collect();
    let (status, body) = call(
        &router,
        "BatchDeleteFirewallRule",
        json!({ "DeleteFirewallRuleEntries": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a batch that deletes some entries and refuses others must answer HTTP 200, \
         got {status}: {body}"
    );

    let deleted = list_member("the mixed batch", &body, "DeletedFirewallRules");
    let errors = list_member("the mixed batch", &body, "DeleteErrors");

    let expected_deleted: Vec<&str> = cases.iter().filter_map(|case| case.deletes).collect();
    let reported_deleted: Vec<String> = deleted
        .iter()
        .map(|rule| string_member(rule, "Name"))
        .collect();
    assert_eq!(
        reported_deleted, expected_deleted,
        "the batch must report exactly the rules it deleted, in the order the request \
         stated their entries: expected {expected_deleted:?}, got {reported_deleted:?} \
         from {body}"
    );

    let expected_refused: Vec<(&str, &str, &Value)> = cases
        .iter()
        .filter_map(|case| {
            case.code
                .map(|code| (case.case, code, &case.members))
        })
        .collect();
    assert_eq!(
        errors.len(),
        expected_refused.len(),
        "the batch must report one error per entry it could not apply: expected \
         {expected_refused:?}, got {errors:?}"
    );
    for ((case, code, entry), error) in expected_refused.iter().zip(errors.iter()) {
        // "The firewall rule entry that caused the error": the entry the request
        // stated, so a caller can tell which of its entries this error is about.
        assert_eq!(
            error.get("FirewallRule"),
            Some(*entry),
            "case {case:?}: the error must echo the entry that caused it, got {error}"
        );
        assert_eq!(
            &string_member(error, "Code"),
            code,
            "case {case:?}: expected Code {code}, got {error}"
        );
        assert!(
            !string_member(error, "Message").is_empty(),
            "case {case:?}: the error must state why the entry was refused, got {error}"
        );
    }

    // Exactly the rules the batch deleted are gone: a refused entry removed
    // nothing, and the group still holds the rule no entry ever named.
    assert_eq!(
        listed_rule_names(&router, &group).await,
        vec!["untouched-rule".to_string()],
        "the group must hold exactly the rules the batch did not delete"
    );
    assert_eq!(
        rule_count(&router, &group).await,
        1,
        "the group's RuleCount must count exactly the rules the batch left behind"
    );
}

/// The rule a batch delete reports is the rule the group held: every member it
/// carried, as the group's own reader reported it the moment before — and it is
/// gone from that reader afterwards, so a repeat of the same entry resolves to
/// nothing.
#[tokio::test]
async fn deleted_rules_are_reported_as_the_group_held_them() {
    let router = emulator();
    let group = rule_group(&router, "batch-delete-roundtrip-group").await;
    let list = domain_list(&router, "batch-delete-roundtrip-list").await;

    // One rule per row, each stating a different set of the members a rule may
    // carry, so what is asserted is the whole rule and not one member of it.
    for (case, name, members) in [
        (
            "a rule that blocks with an override",
            "override-rule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
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
            "a rule that alerts on every query type",
            "alert-rule",
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 200,
                "Action": "ALERT",
            }),
        ),
    ] {
        let created = create_rule(&router, name, members).await;
        let held = listed_rules(&router, &group)
            .await
            .into_iter()
            .find(|rule| string_member(rule, "Name") == name)
            .unwrap_or_else(|| panic!("case {case:?}: the created rule must be listed"));
        assert_eq!(
            without_status(&held),
            without_status(&created),
            "case {case:?}: the group must report the rule the create answered with"
        );

        // The entry names the rule the way a caller does: its group, the source
        // it matches, and the query type it evaluates where it evaluates one.
        let mut entry = json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
        });
        if let Some(qtype) = created.get("Qtype").and_then(Value::as_str) {
            entry
                .as_object_mut()
                .expect("the entry is an object")
                .insert("Qtype".to_string(), json!(qtype));
        }
        let body = ok(
            &router,
            "BatchDeleteFirewallRule",
            json!({ "DeleteFirewallRuleEntries": [entry.clone()] }),
        )
        .await;
        let deleted = list_member(case, &body, "DeletedFirewallRules");
        assert!(
            list_member(case, &body, "DeleteErrors").is_empty(),
            "case {case:?}: an entry naming a rule of the group must be applied, got {body}"
        );
        assert_eq!(
            deleted.len(),
            1,
            "case {case:?}: the entry must delete exactly one rule, got {body}"
        );
        assert_eq!(
            without_status(&deleted[0]),
            without_status(&held),
            "case {case:?}: the deleted rule must be reported as the group held it"
        );

        assert!(
            !listed_rule_names(&router, &group).await.contains(&name.to_string()),
            "case {case:?}: the group must no longer hold the rule the batch deleted"
        );

        // A rule is deleted once: the same entry again names no rule at all.
        let repeat = ok(
            &router,
            "BatchDeleteFirewallRule",
            json!({ "DeleteFirewallRuleEntries": [entry] }),
        )
        .await;
        let repeat_errors = list_member(case, &repeat, "DeleteErrors");
        assert!(
            list_member(case, &repeat, "DeletedFirewallRules").is_empty(),
            "case {case:?}: a repeat must delete nothing more, got {repeat}"
        );
        assert_eq!(
            repeat_errors.len(),
            1,
            "case {case:?}: a repeat must report the entry it could not apply, got {repeat}"
        );
        assert_eq!(
            string_member(&repeat_errors[0], "Code"),
            "ResourceNotFoundException",
            "case {case:?}: expected Code ResourceNotFoundException, got {repeat}"
        );
    }

    assert_eq!(
        rule_count(&router, &group).await,
        0,
        "every rule the group held was deleted, so it holds none"
    );
}

/// A rule is deletable in every lifecycle state it can report: the delete is
/// "the only operation that succeeds against a rule whose Status is
/// CREATION_FAILED", and it succeeds against one still CREATING too — the two
/// states an update of the same rule is refused in.
#[tokio::test]
async fn a_rule_is_deleted_in_every_lifecycle_state_it_reports() {
    let router = emulator();
    let group = rule_group(&router, "batch-delete-lifecycle-group").await;

    // A rule matching a partner threat feed is the one match source whose
    // provisioning is asynchronous, so such a rule begins in CREATING and
    // settles on the state the clock owes it.
    for (case, name, partner, priority, settle, expected_status) in [
        (
            "a rule whose provisioning has not settled",
            "creating-rule",
            "example-threat-feed",
            100,
            false,
            FIREWALL_RULE_STATUS_CREATING,
        ),
        (
            "a rule whose provisioning failed",
            "failed-rule",
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
            "BatchDeleteFirewallRule",
            json!({ "DeleteFirewallRuleEntries": [{
                "FirewallRuleGroupId": group,
                "FirewallThreatProtectionId": threat_protection_id,
            }] }),
        )
        .await;
        assert!(
            list_member(case, &body, "DeleteErrors").is_empty(),
            "case {case:?}: no lifecycle state refuses a delete, got {body}"
        );
        let deleted = list_member(case, &body, "DeletedFirewallRules");
        assert_eq!(
            deleted.len(),
            1,
            "case {case:?}: the entry must delete exactly one rule, got {body}"
        );
        assert_eq!(
            deleted[0].get("Status"),
            Some(&json!(expected_status)),
            "case {case:?}: expected Status {expected_status}, got {}",
            deleted[0]
        );
        assert!(
            !listed_rule_names(&router, &group).await.contains(&name.to_string()),
            "case {case:?}: the group must no longer hold the rule the batch deleted"
        );
    }
}

/// A batch past the entries one call may carry is refused WHOLE: nothing is
/// deleted for any of its entries.
#[tokio::test]
async fn a_batch_past_the_entry_bound_deletes_nothing() {
    let router = emulator();
    let list = domain_list(&router, "batch-delete-bound-list").await;

    // The bound itself, one under it, and one past it: the bound is a bound, so
    // the call it refuses is the one PAST it.
    for (case, count, expected_status) in [
        (
            "one under the bound",
            MAX_FIREWALL_RULE_ENTRIES_PER_BATCH - 1,
            200,
        ),
        ("the bound itself", MAX_FIREWALL_RULE_ENTRIES_PER_BATCH, 200),
        (
            "one past the bound",
            MAX_FIREWALL_RULE_ENTRIES_PER_BATCH + 1,
            400,
        ),
    ] {
        let group = rule_group(&router, &format!("batch-delete-bound-{count}")).await;
        create_rule(
            &router,
            &format!("bound-survivor-{count}"),
            json!({
                "FirewallRuleGroupId": group,
                "FirewallDomainListId": list,
                "Priority": 100,
                "Action": "ALERT",
            }),
        )
        .await;

        // The first entry names the one rule the group holds; every other entry
        // names a rule group that resolves to nothing, so a batch within the
        // bound deletes that rule and refuses the rest.
        let mut entries = vec![json!({
            "FirewallRuleGroupId": group,
            "FirewallDomainListId": list,
        })];
        entries.extend((1..count).map(|_| {
            json!({
                "FirewallRuleGroupId": UNRESOLVABLE_RULE_GROUP_ID,
                "FirewallDomainListId": list,
            })
        }));

        let (status, body) = call(
            &router,
            "BatchDeleteFirewallRule",
            json!({ "DeleteFirewallRuleEntries": entries }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({count} entries): expected HTTP {expected_status}, got {status} \
             with {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                list_member(case, &body, "DeletedFirewallRules").len(),
                1,
                "case {case:?}: a batch within the bound must apply the entry that names a \
                 rule, got {body}"
            );
            assert_eq!(
                list_member(case, &body, "DeleteErrors").len(),
                count - 1,
                "case {case:?}: a batch within the bound must report one error per entry it \
                 could not apply, got {body}"
            );
            assert_eq!(
                rule_count(&router, &group).await,
                0,
                "case {case:?}: the rule the batch deleted is gone from the group"
            );
        } else {
            assert!(
                error_type(&body).contains("LimitExceededException"),
                "case {case:?}: expected LimitExceededException, got {body}"
            );
            assert_eq!(
                rule_count(&router, &group).await,
                1,
                "case {case:?}: a refused batch must delete nothing at all"
            );
        }
    }
}
