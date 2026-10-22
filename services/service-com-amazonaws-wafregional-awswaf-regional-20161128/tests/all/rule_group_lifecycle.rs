// SPDX-License-Identifier: CC0-1.0

//! The AWS WAF Classic (regional) RuleGroup lifecycle, end to end over the wire:
//! `GetChangeToken`, `CreateRuleGroup`, `GetRuleGroup`.
//!
//! Every call goes through the real router (the shared harness in `tests/all/wire.rs`),
//! so what these tests assert is what an SDK client sees: the `RuleGroup` each response
//! carries is built from the persisted record, not echoed from the request.
//!
//! `CreateRuleGroup` publishes a small per-account quota (`state::MAX_RULE_GROUPS`), so
//! a table spanning more rows than the quota gives each row its own router — ids are
//! minted per store, so cross-store ids are never compared.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_RULE_GROUPS, MAX_TAGS_PER_RESOURCE, RESERVED_METRIC_NAMES,
};

use crate::wire::{call, change_token, expect_error, is_resource_id, ok_call, router};

/// Create a rule group with a fresh change token; answers `(token, response body)`.
async fn create_rule_group(
    router: &MultiServiceRouter,
    name: &str,
    metric_name: &str,
) -> (String, Value) {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRuleGroup",
        &json!({"Name": name, "MetricName": metric_name, "ChangeToken": token}),
    )
    .await;
    (token, body)
}

/// The `RuleGroup` `GetRuleGroup` reports for `rule_group_id`.
async fn get_rule_group(router: &MultiServiceRouter, rule_group_id: &str) -> Value {
    let body = ok_call(
        router,
        "GetRuleGroup",
        &json!({"RuleGroupId": rule_group_id}),
    )
    .await;
    body["RuleGroup"].clone()
}

/// `CreateRuleGroup` stores a new, EMPTY rule group under a minted `RuleGroupId` and
/// `GetRuleGroup` reads that record back — across a table of (Name, MetricName) pairs
/// differing in length, case and character set.
#[tokio::test]
async fn create_then_get_reads_back_the_stored_rule_group() {
    // `ResourceName` and `MetricName` are both len 1..128, pattern \S; `MetricName` is
    // additionally restricted to A-Z, a-z and 0-9 by its own documentation.
    let max_length_name = "n".repeat(128);
    let max_length_metric = "m".repeat(128);
    let cases: &[(&str, &str, &str)] = &[
        // (label, Name, MetricName)
        ("a plain name", "bootstrap-rule-group", "bootstrapRuleGroup"),
        ("one character (the minimum length)", "x", "x"),
        (
            "128 characters (the maximum length)",
            &max_length_name,
            &max_length_metric,
        ),
        ("mixed case and digits", "RuleGroup-2026_v2", "RuleGroup2026v2"),
        (
            "punctuation the name allows",
            "rg.with+punctuation!",
            "rgWithPunctuation",
        ),
        ("non-ASCII in the name", "règle-groupe-日本", "regleGroupe0"),
    ];

    for (label, name, metric_name) in cases {
        // One store per row: the per-account quota is smaller than this table.
        let router = router();
        let (token, created) = create_rule_group(&router, name, metric_name).await;
        let rule_group = &created["RuleGroup"];
        let id = rule_group["RuleGroupId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        assert!(
            is_resource_id(&id),
            "case {label:?}: CreateRuleGroup must mint a ResourceId, got {id:?} ({created})"
        );
        assert_eq!(
            created["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label:?}: CreateRuleGroup must report the change token it consumed, \
             got {created}"
        );
        assert_eq!(
            rule_group["Name"].as_str(),
            Some(*name),
            "case {label:?}: expected Name {name:?}, got {created}"
        );
        assert_eq!(
            rule_group["MetricName"].as_str(),
            Some(*metric_name),
            "case {label:?}: expected MetricName {metric_name:?}, got {created}"
        );

        // The round trip: the record that was persisted is what a reader sees.
        let stored = get_rule_group(&router, &id).await;
        assert_eq!(
            stored, *rule_group,
            "case {label:?}: GetRuleGroup must report the stored record, got {stored} \
             after {rule_group}"
        );

        // A created rule group is EMPTY: the modelled `RuleGroup` carries only the id,
        // the name and the metric name, and holds no rules until `UpdateRuleGroup` adds
        // them.
        let members: BTreeSet<&str> = stored
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: GetRuleGroup returned no RuleGroup object"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            BTreeSet::from(["RuleGroupId", "Name", "MetricName"]),
            "case {label:?}: an empty RuleGroup carries exactly the modelled members, \
             got {stored}"
        );

        // A second read is identical: reading changes nothing a later caller sees.
        let reread = get_rule_group(&router, &id).await;
        assert_eq!(
            reread, stored,
            "case {label:?}: a second GetRuleGroup must report the same record, got {reread}"
        );
    }
}

/// Each create is its own resource: `ResourceName` carries no uniqueness constraint, so
/// rule groups created with the same `Name` are distinct records under distinct ids, and
/// each one reads back its own fields.
#[tokio::test]
async fn every_create_is_a_distinct_record_even_under_one_name() {
    let router = router();
    // As many rows as the account quota allows, the first two sharing a Name.
    let cases: Vec<(String, String)> = (0..MAX_RULE_GROUPS)
        .map(|index| {
            let name = if index < 2 {
                "twice-over".to_string()
            } else {
                format!("distinct-{index}")
            };
            (name, format!("distinctMetric{index}"))
        })
        .collect();

    let mut created: Vec<(String, String, String)> = Vec::new();
    for (name, metric_name) in &cases {
        let (_, body) = create_rule_group(&router, name, metric_name).await;
        created.push((
            body["RuleGroup"]["RuleGroupId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            name.clone(),
            metric_name.clone(),
        ));
    }

    let ids: BTreeSet<&str> = created.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "every CreateRuleGroup must mint a distinct id, got {ids:?}"
    );

    for (id, name, metric_name) in &created {
        let stored = get_rule_group(&router, id).await;
        assert_eq!(
            stored["Name"].as_str(),
            Some(name.as_str()),
            "case {id:?}: expected Name {name:?}, got {stored}"
        );
        assert_eq!(
            stored["MetricName"].as_str(),
            Some(metric_name.as_str()),
            "case {id:?}: expected MetricName {metric_name:?}, got {stored}"
        );
    }
}

/// A rule group is a stored record under a published per-account quota, so the
/// (quota + 1)-th create is refused with the modelled `WAFLimitsExceededException` and
/// stores nothing — whatever the names it carries.
#[tokio::test]
async fn create_refuses_past_the_rule_group_quota() {
    let router = router();
    let mut ids = Vec::new();
    for index in 0..MAX_RULE_GROUPS {
        let (_, created) =
            create_rule_group(&router, &format!("quota-{index}"), &format!("quota{index}")).await;
        ids.push(
            created["RuleGroup"]["RuleGroupId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    for (label, name, metric) in [
        ("one past the quota", "quota-overflow", "quotaOverflow"),
        (
            "a second attempt past the quota",
            "quota-overflow-2",
            "quotaOverflow2",
        ),
        ("reusing a stored name", "quota-0", "quota0again"),
    ] {
        let token = change_token(&router).await;
        expect_error(
            &router,
            "CreateRuleGroup",
            &json!({"Name": name, "MetricName": metric, "ChangeToken": token}),
            "WAFLimitsExceededException",
            label,
        )
        .await;
        // The quota is checked before the change token is consumed, so the refused
        // call left the token exactly as `GetChangeToken` minted it.
        let status = ok_call(
            &router,
            "GetChangeTokenStatus",
            &json!({"ChangeToken": token}),
        )
        .await;
        assert_eq!(
            status["ChangeTokenStatus"].as_str(),
            Some("PROVISIONED"),
            "case {label:?}: a create refused for the quota must not consume the change \
             token, got {status}"
        );
    }

    // The refusals stored nothing and removed nothing: every rule group under the quota
    // is still readable.
    for id in &ids {
        let stored = get_rule_group(&router, id).await;
        assert_eq!(
            stored["RuleGroupId"].as_str(),
            Some(id.as_str()),
            "case {id:?}: a refused create must leave the stored rule groups intact, \
             got {stored}"
        );
    }
}

/// `MetricName` "can contain only alphanumeric characters (A-Z, a-z, 0-9) ... It can't
/// contain whitespace or metric names reserved for AWS WAF, including "All" and
/// "Default_Action."" — so a name outside that character set, or one AWS reserves, is
/// refused with the modelled `WAFDisallowedNameException`, and anything inside it is
/// stored as given.
#[tokio::test]
async fn create_refuses_metric_names_outside_the_documented_character_set() {
    // Accepted rows, including each published bound of `MetricName`'s modelled
    // `@length` of 1..128.
    let longest_metric = "m".repeat(128);
    for (label, metric) in [
        ("a single letter (the minimum length)", "a"),
        ("a single digit", "7"),
        ("letters and digits", "RuleGroupMetric42"),
        ("128 characters (the maximum length)", longest_metric.as_str()),
        // The reserved names are reserved exactly: neither a longer name containing
        // one nor a differently-cased spelling is reserved.
        ("a reserved name as a prefix", "AllRequests"),
        ("a reserved name in a different case", "all"),
    ] {
        let router = router();
        let created = ok_call(
            &router,
            "CreateRuleGroup",
            &json!({
                "Name": "allowed-metric",
                "MetricName": metric,
                "ChangeToken": change_token(&router).await,
            }),
        )
        .await;
        assert_eq!(
            created["RuleGroup"]["MetricName"].as_str(),
            Some(metric),
            "case {label:?}: an allowed MetricName must be stored as given, got {created}"
        );
    }

    let mut refused: Vec<(String, String)> = [
        ("whitespace inside the name", "Rule Group"),
        ("a leading space", " RuleGroup"),
        ("a hyphen", "rule-group"),
        ("an underscore", "rule_group"),
        ("a dot", "rule.group"),
        ("a non-ASCII letter", "règle"),
        ("an emoji", "metric🙂"),
    ]
    .iter()
    .map(|(label, metric)| ((*label).to_string(), (*metric).to_string()))
    .collect();
    // The reserved names themselves come from `MetricName`'s own documentation, via
    // `state::RESERVED_METRIC_NAMES`, so every entry is covered — including any this
    // test does not spell out.
    for reserved in RESERVED_METRIC_NAMES {
        refused.push((
            format!("the reserved name {reserved}"),
            (*reserved).to_string(),
        ));
    }

    for (label, metric) in &refused {
        let router = router();
        let token = change_token(&router).await;
        expect_error(
            &router,
            "CreateRuleGroup",
            &json!({"Name": "disallowed-metric", "MetricName": metric, "ChangeToken": token}),
            "WAFDisallowedNameException",
            label,
        )
        .await;
        // The refusal stored nothing and consumed nothing, so the change token it
        // carried is still the one an allowed create can use.
        let created = ok_call(
            &router,
            "CreateRuleGroup",
            &json!({"Name": "after-refusal", "MetricName": "afterRefusal", "ChangeToken": token}),
        )
        .await;
        assert!(
            is_resource_id(
                created["RuleGroup"]["RuleGroupId"]
                    .as_str()
                    .unwrap_or_default()
            ),
            "case {label:?}: a create refused for its MetricName must not consume the \
             change token, got {created}"
        );
    }
}

/// The change-token protocol: "each create, update, or delete request must use a unique
/// change token", so a token a mutating call already consumed — and one this service
/// never minted — is refused with the modelled `WAFStaleDataException`.
#[tokio::test]
async fn create_refuses_a_change_token_it_cannot_consume() {
    let router = router();
    let (token, created) = create_rule_group(&router, "token-probe", "tokenProbe").await;
    let id = created["RuleGroup"]["RuleGroupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (label, stale) in [
        ("already consumed", token.clone()),
        (
            "never minted",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
    ] {
        expect_error(
            &router,
            "CreateRuleGroup",
            &json!({"Name": "stale-token", "MetricName": "staleToken", "ChangeToken": stale}),
            "WAFStaleDataException",
            label,
        )
        .await;
    }

    // Only the first create stored anything: the refusals persisted nothing.
    let stored = get_rule_group(&router, &id).await;
    assert_eq!(
        stored["Name"].as_str(),
        Some("token-probe"),
        "a refused create must store nothing, got {stored}"
    );
}

/// `GetRuleGroup` for an id the service holds nothing for is the modelled
/// `WAFNonexistentItemException`, whatever the id's shape — and the refusal carries one
/// request-independent sentence, never the requested id.
#[tokio::test]
async fn an_unknown_rule_group_id_is_a_modelled_not_found() {
    let router = router();
    let (_, created) = create_rule_group(&router, "present", "present").await;
    let known = created["RuleGroup"]["RuleGroupId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let longest_id = "z".repeat(128);
    // An id of the right shape that was never minted: the stored id with its last hex
    // digit swapped for a different one, whatever that digit is.
    let last = known.chars().next_back().unwrap_or('0');
    let near_miss = format!(
        "{}{}",
        &known[..known.len() - 1],
        if last == 'a' { 'b' } else { 'a' }
    );
    assert_ne!(
        near_miss, known,
        "the near-miss id must differ from the stored one, got {near_miss:?}"
    );
    let unknown: Vec<(&str, &str)> = vec![
        (
            "a UUID-shaped id nothing was stored under",
            "00000000-0000-0000-0000-000000000000",
        ),
        ("one character (the minimum length)", "x"),
        ("128 characters (the maximum length)", longest_id.as_str()),
        ("one character off a stored id", near_miss.as_str()),
    ];

    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (label, id) in &unknown {
        let body = expect_error(
            &router,
            "GetRuleGroup",
            &json!({"RuleGroupId": id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default()
            .to_string();
        // Only the distinguishing ids are checked for an echo: a one-character id
        // occurs in ordinary English prose ("x" is a letter of "exist").
        assert!(
            id.len() < 8 || !message.contains(id),
            "case {label:?}: the refusal must not echo the requested id, got {message:?}"
        );
        messages.insert(message);
    }
    assert_eq!(
        messages.len(),
        1,
        "the not-found sentence must not vary with the request, saw {messages:?}"
    );

    // The id that WAS stored still reads back.
    let stored = get_rule_group(&router, &known).await;
    assert_eq!(
        stored["RuleGroupId"].as_str(),
        Some(known.as_str()),
        "a refused read must leave the stored record readable, got {stored}"
    );
}

/// The tags a create request carries are accepted up to the per-resource tag quota and
/// do not change the stored `RuleGroup` projection, which the model limits to the id,
/// the name and the metric name.
#[tokio::test]
async fn tags_on_create_are_accepted_and_the_rule_group_projection_is_unchanged() {
    let at_the_quota: Value = Value::Array(
        (0..MAX_TAGS_PER_RESOURCE)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect(),
    );
    let cases: &[(&str, Value)] = &[
        ("one tag", json!([{"Key": "team", "Value": "security"}])),
        (
            "several tags, one with an empty value",
            json!([
                {"Key": "team", "Value": "security"},
                {"Key": "env", "Value": "prod"},
                {"Key": "empty-value", "Value": ""}
            ]),
        ),
        ("a repeated key is accepted", json!([
            {"Key": "team", "Value": "first"},
            {"Key": "team", "Value": "last"}
        ])),
        ("the per-resource tag quota exactly", at_the_quota),
    ];

    for (index, (label, tags)) in cases.iter().enumerate() {
        let name = format!("tagged-{index}");
        // One store per row: the rule-group quota is smaller than this table.
        let router = router();
        let token = change_token(&router).await;
        let created = ok_call(
            &router,
            "CreateRuleGroup",
            &json!({
                "Name": name,
                "MetricName": format!("tagged{index}"),
                "ChangeToken": token,
                "Tags": tags,
            }),
        )
        .await;
        let id = created["RuleGroup"]["RuleGroupId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stored = get_rule_group(&router, &id).await;
        assert_eq!(
            stored["Name"].as_str(),
            Some(name.as_str()),
            "case {label:?}: a tagged create must store the rule group, got {stored}"
        );
        assert!(
            stored.get("Tags").is_none(),
            "case {label:?}: the modelled RuleGroup carries no Tags member, got {stored}"
        );
    }

    // One tag past the per-resource quota is refused, and the modelled `@length` of
    // `TagList` (1..) refuses an empty list before the handler runs.
    let over_the_quota: Value = Value::Array(
        (0..=MAX_TAGS_PER_RESOURCE)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
            .collect(),
    );
    let router = router();
    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateRuleGroup",
        &json!({
            "Name": "too-many-tags",
            "MetricName": "tooManyTags",
            "ChangeToken": token,
            "Tags": over_the_quota,
        }),
        "WAFLimitsExceededException",
        "one tag past the per-resource quota",
    )
    .await;

    let (status, body) = call(
        &router,
        "CreateRuleGroup",
        &json!({
            "Name": "empty-tags",
            "MetricName": "emptyTags",
            "ChangeToken": token,
            "Tags": [],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case empty Tags: a list outside the modelled length must be refused, \
         got {status}: {body}"
    );
}
