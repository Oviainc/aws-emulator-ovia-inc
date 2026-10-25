// SPDX-License-Identifier: CC0-1.0

//! `UpdateRuleGroup` and the activations it writes, end to end over the wire:
//! `CreateRule`, `CreateRateBasedRule`, `CreateRuleGroup`, `UpdateRuleGroup`,
//! `ListActivatedRulesInRuleGroup`.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each response is built from the persisted
//! record, not from the request.
//!
//! The rules under test are `UpdateRuleGroup`'s own documentation — "Inserts or deletes
//! ActivatedRule objects in a RuleGroup. You can only insert REGULAR rules into a rule
//! group. You can have a maximum of ten rules per rule group" and "If you want to
//! replace one Rule with another, you delete the existing one and add the new one" —
//! `ActivatedRule`'s, whose `Priority` "must be a unique integer" and whose
//! `OverrideAction` "applies only when updating or adding a RuleGroup to a WebACL. In
//! this case you do not use ActivatedRule|Action. For all other update requests,
//! ActivatedRule|Action is used instead of ActivatedRule|OverrideAction", and the
//! change-token protocol every mutating call follows.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    WAF_ACTION_TYPE_VALUES, WAF_RULE_TYPE_VALUES,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_PAGE_SIZE, MAX_RULE_GROUP_RULES, MAX_RULE_GROUP_UPDATES_PER_REQUEST,
    MESSAGE_INVALID_OPERATION, MESSAGE_LIMITS_EXCEEDED, MESSAGE_NONEXISTENT_CONTAINER,
    MESSAGE_NONEXISTENT_ITEM, MESSAGE_STALE_DATA, WAF_RULE_TYPE_REGULAR,
};

use crate::wire::{call, change_token, error_message, expect_error, ok_call, router, token_status};

/// A UUID-shaped id this service never minted.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Create a regular `Rule` and return its `RuleId`.
async fn create_rule(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRule",
        &json!({"Name": name, "MetricName": "ruleGroupMember", "ChangeToken": token}),
    )
    .await;
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRule returned no RuleId: {body}"))
        .to_string()
}

/// Create a `RateBasedRule` and return its `RuleId` — a rule that is NOT a REGULAR one.
async fn create_rate_based_rule(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRateBasedRule",
        &json!({
            "Name": name,
            "MetricName": "ruleGroupRateBased",
            "RateKey": "IP",
            "RateLimit": 2000,
            "ChangeToken": token,
        }),
    )
    .await;
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRateBasedRule returned no RuleId: {body}"))
        .to_string()
}

/// Create a `RuleGroup` and return its `RuleGroupId`.
async fn create_rule_group(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateRuleGroup",
        &json!({"Name": name, "MetricName": "ruleGroupUpdates", "ChangeToken": token}),
    )
    .await;
    body["RuleGroup"]["RuleGroupId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRuleGroup returned no RuleGroupId: {body}"))
        .to_string()
}

/// One `RuleGroupUpdate` carrying an activation with an `Action` — what this operation
/// uses — and nothing else.
fn update(action: &str, rule_id: &str, priority: i32, waf_action: &str) -> Value {
    json!({
        "Action": action,
        "ActivatedRule": {
            "RuleId": rule_id,
            "Priority": priority,
            "Action": {"Type": waf_action},
        },
    })
}

/// Apply `updates` to `rule_group_id` with a fresh change token, answering
/// `(http status, body, the token the request carried)`.
async fn apply_updates(
    router: &MultiServiceRouter,
    rule_group_id: &str,
    updates: Vec<Value>,
) -> (u16, Value, String) {
    let token = change_token(router).await;
    let (status, body) = call(
        router,
        "UpdateRuleGroup",
        &json!({
            "RuleGroupId": rule_group_id,
            "ChangeToken": token,
            "Updates": updates,
        }),
    )
    .await;
    (status, body, token)
}

/// The `ActivatedRules` `ListActivatedRulesInRuleGroup` reports for `rule_group_id`, in
/// stored order, asking for the whole set.
async fn activated_rules(router: &MultiServiceRouter, rule_group_id: &str) -> Vec<Value> {
    let body = ok_call(
        router,
        "ListActivatedRulesInRuleGroup",
        &json!({"RuleGroupId": rule_group_id}),
    )
    .await;
    assert!(
        body.get("NextMarker").is_none(),
        "a page holding every activation carries no NextMarker, got {body}"
    );
    body["ActivatedRules"]
        .as_array()
        .unwrap_or_else(|| panic!("ListActivatedRulesInRuleGroup returned no array: {body}"))
        .clone()
}

/// The activation a read reports for an insert of `rule_id` at `priority` with
/// `waf_action`: the stored record, which carries the activation as it was accepted.
fn activation(rule_id: &str, priority: i32, waf_action: &str) -> Value {
    json!({
        "RuleId": rule_id,
        "Priority": priority,
        "Action": {"Type": waf_action},
    })
}

/// An `INSERT` activates the named rule in the stored rule group and a `DELETE`
/// deactivates it — across every `WafActionType` the model declares, priorities spanning
/// the integer `RulePriority` takes, and an activation that names its REGULAR type as
/// well as one that leaves it to the documented default.
#[tokio::test]
async fn insert_then_delete_round_trips_an_activation() {
    let router = router();
    let rule = create_rule(&router, "round-trip-rule").await;
    let group = create_rule_group(&router, "round-trip-group").await;

    // (label, WafActionType, Priority, the Type the activation names) — the action
    // values are the model's own `WAF_ACTION_TYPE_VALUES`, so every one is covered.
    let mut cases: Vec<(String, &str, i32, Option<&str>)> = WAF_ACTION_TYPE_VALUES
        .iter()
        .map(|action| (format!("the {action} action"), *action, 1, None))
        .collect();
    cases.extend([
        (
            "the REGULAR type named outright".to_string(),
            "BLOCK",
            1,
            Some(WAF_RULE_TYPE_REGULAR),
        ),
        ("priority zero".to_string(), "COUNT", 0, None),
        ("a negative priority".to_string(), "ALLOW", -7, None),
        (
            "the largest priority an integer carries".to_string(),
            "BLOCK",
            i32::MAX,
            None,
        ),
        (
            "the smallest priority an integer carries".to_string(),
            "BLOCK",
            i32::MIN,
            None,
        ),
    ]);

    for (label, waf_action, priority, rule_type) in &cases {
        let mut activated = json!({
            "RuleId": rule,
            "Priority": priority,
            "Action": {"Type": waf_action},
        });
        if let Some(rule_type) = rule_type {
            activated["Type"] = json!(rule_type);
        }
        let token = change_token(&router).await;
        let (status, body) = call(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [{"Action": "INSERT", "ActivatedRule": activated}],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateRuleGroup must accept the insert, got {status}: {body}"
        );
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label:?}: UpdateRuleGroup must report the change token it consumed, \
             got {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PENDING",
            "case {label:?}: an applied update consumes the change token"
        );

        // The round trip: the activation that was persisted is what a reader sees.
        let mut expected = activation(&rule, *priority, waf_action);
        if let Some(rule_type) = rule_type {
            expected["Type"] = json!(rule_type);
        }
        assert_eq!(
            activated_rules(&router, &group).await,
            vec![expected],
            "case {label:?}: the read must report the inserted activation"
        );

        let (status, body, _) = apply_updates(
            &router,
            &group,
            vec![update("DELETE", &rule, *priority, waf_action)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateRuleGroup must accept the delete, got {status}: {body}"
        );
        let remaining = activated_rules(&router, &group).await;
        assert!(
            remaining.is_empty(),
            "case {label:?}: the deleted activation must be gone, got {remaining:?}"
        );
    }
}

/// One request carries several updates: the rule group holds the activations it
/// inserted, in priority order whatever order the request listed them in, and a later
/// request deleting some of them leaves the rest in that order.
#[tokio::test]
async fn one_request_applies_several_updates_and_the_group_keeps_priority_order() {
    let router = router();
    let group = create_rule_group(&router, "ordering-group").await;
    let mut rules = Vec::new();
    for index in 0..4 {
        rules.push(create_rule(&router, &format!("ordering-rule-{index}")).await);
    }

    // Priorities out of order in the request, and not consecutive: "If you add multiple
    // Rules to a WebACL, the values don't need to be consecutive."
    let inserts = vec![
        update("INSERT", &rules[0], 30, "BLOCK"),
        update("INSERT", &rules[1], 10, "ALLOW"),
        update("INSERT", &rules[2], 20, "COUNT"),
        update("INSERT", &rules[3], -1, "BLOCK"),
    ];
    let (status, body, _) = apply_updates(&router, &group, inserts).await;
    assert_eq!(
        status, 200,
        "a request carrying four inserts must be accepted, got {status}: {body}"
    );
    assert_eq!(
        activated_rules(&router, &group).await,
        vec![
            activation(&rules[3], -1, "BLOCK"),
            activation(&rules[1], 10, "ALLOW"),
            activation(&rules[2], 20, "COUNT"),
            activation(&rules[0], 30, "BLOCK"),
        ],
        "the rule group must report every inserted activation, in priority order"
    );

    // A mixed request: deactivate two of them and activate one again at another
    // priority, which is how "you delete the existing one and add the new one".
    let mixed = vec![
        update("DELETE", &rules[1], 10, "ALLOW"),
        update("DELETE", &rules[0], 30, "BLOCK"),
        update("INSERT", &rules[1], 25, "COUNT"),
    ];
    let (status, body, _) = apply_updates(&router, &group, mixed).await;
    assert_eq!(
        status, 200,
        "a mixed delete/insert request must be accepted, got {status}: {body}"
    );
    assert_eq!(
        activated_rules(&router, &group).await,
        vec![
            activation(&rules[3], -1, "BLOCK"),
            activation(&rules[2], 20, "COUNT"),
            activation(&rules[1], 25, "COUNT"),
        ],
        "the surviving activations keep priority order and the re-inserted one takes its \
         new priority"
    );
}

/// The read is paginated: `Limit` bounds the page, the `NextMarker` it hands back
/// resumes at the activation after it, and walking the pages reports every activation
/// exactly once, in the stored order — for every page size from one to past the whole
/// set.
#[tokio::test]
async fn the_read_pages_over_the_activations_in_stored_order() {
    let router = router();
    let group = create_rule_group(&router, "paging-group").await;
    let mut expected = Vec::new();
    for index in 0..MAX_RULE_GROUP_RULES {
        let rule = create_rule(&router, &format!("paging-rule-{index}")).await;
        let priority = i32::try_from(index).expect("the quota fits an i32") * 5;
        let (status, body, _) = apply_updates(
            &router,
            &group,
            vec![update("INSERT", &rule, priority, "BLOCK")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case rule {index}: the setup insert must be accepted, got {status}: {body}"
        );
        expected.push(activation(&rule, priority, "BLOCK"));
    }

    // (label, Limit) — 1 and `MAX_PAGE_SIZE` are the published bounds of
    // `PaginationLimit`'s @range above its default, and 0 is that default, which asks
    // for the service's own page size.
    let page_sizes: &[(&str, usize)] = &[
        ("one activation a page (the smallest page)", 1),
        ("a page smaller than the set", 3),
        ("a page the size of the set", MAX_RULE_GROUP_RULES),
        ("a page larger than the set", MAX_RULE_GROUP_RULES + 1),
        ("the largest page the model allows", MAX_PAGE_SIZE),
        ("the default Limit", 0),
    ];

    for (label, limit) in page_sizes {
        let mut seen: Vec<Value> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"RuleGroupId": group, "Limit": limit});
            if let Some(marker) = &marker {
                request["NextMarker"] = json!(marker);
            }
            let body = ok_call(&router, "ListActivatedRulesInRuleGroup", &request).await;
            let page = body["ActivatedRules"]
                .as_array()
                .unwrap_or_else(|| panic!("case {label:?}: the read returned no array: {body}"))
                .clone();
            if *limit > 0 {
                assert!(
                    page.len() <= *limit,
                    "case {label:?}: a page must hold at most Limit activations, got {}",
                    page.len()
                );
            }
            seen.extend(page);
            marker = body["NextMarker"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= MAX_RULE_GROUP_RULES + 1,
                "case {label:?}: the walk must terminate, saw {pages} pages"
            );
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {label:?}: walking the pages must report every activation once, in \
             stored order"
        );
    }

    // A marker that resumes at no activation of this rule group is not one a page handed
    // back: "You specified an invalid value."
    for (label, bad_marker) in [
        ("an id this service never minted", UNKNOWN_ID),
        ("the rule group's own id", group.as_str()),
    ] {
        expect_error(
            &router,
            "ListActivatedRulesInRuleGroup",
            &json!({"RuleGroupId": group, "NextMarker": bad_marker}),
            "WAFInvalidParameterException",
            label,
        )
        .await;
    }
}

/// An inserted activation names "the RuleId for a Rule […] returned by CreateRule and by
/// ListRules", so an id no stored REGULAR rule carries — including a rate-based rule's
/// and another resource kind's — is the modelled "referenced object doesn't exist", and
/// the refused request changes nothing.
#[tokio::test]
async fn insert_refuses_a_rule_that_does_not_exist() {
    let router = router();
    let group = create_rule_group(&router, "resolution-group").await;
    let rate_based = create_rate_based_rule(&router, "resolution-rate-based").await;
    let stored_rule = create_rule(&router, "resolution-rule").await;

    let cases: &[(&str, &str)] = &[
        ("a UUID-shaped id that was never minted", UNKNOWN_ID),
        ("the zero id", "00000000-0000-0000-0000-000000000000"),
        ("a one-character id", "x"),
        // A RateBasedRule is not a REGULAR rule, so its id names no rule a group may
        // activate.
        ("the id of a stored RateBasedRule", rate_based.as_str()),
        // An id this service minted for another resource kind is not a RuleId.
        ("the rule group's own id", group.as_str()),
    ];

    for (label, rule_id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [update("INSERT", rule_id, 1, "BLOCK")],
            }),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: the refusal carries WAF Classic's fixed not-found sentence, \
             got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );
        let stored = activated_rules(&router, &group).await;
        assert!(
            stored.is_empty(),
            "case {label}: a refused UpdateRuleGroup must persist no activation, \
             got {stored:?}"
        );
    }

    // The rule that DOES exist is accepted, so the refusals above are about the id and
    // not about the request's shape.
    let (status, body, _) = apply_updates(
        &router,
        &group,
        vec![update("INSERT", &stored_rule, 1, "BLOCK")],
    )
    .await;
    assert_eq!(
        status, 200,
        "an insert naming a stored Rule must be accepted, got {status}: {body}"
    );
}

/// "You can only insert REGULAR rules into a rule group": an activation naming any other
/// `WafRuleType` is the modelled invalid parameter, naming the rule type as the field it
/// did not accept.
#[tokio::test]
async fn insert_refuses_a_rule_type_other_than_regular() {
    let router = router();
    let group = create_rule_group(&router, "rule-type-group").await;
    let rule = create_rule(&router, "rule-type-rule").await;

    // Every `WafRuleType` the model declares except the one a rule group may hold, from
    // the model's own `WAF_RULE_TYPE_VALUES` (REGULAR, RATE_BASED, GROUP).
    for rule_type in WAF_RULE_TYPE_VALUES
        .iter()
        .filter(|rule_type| **rule_type != WAF_RULE_TYPE_REGULAR)
    {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [{
                    "Action": "INSERT",
                    "ActivatedRule": {
                        "RuleId": rule,
                        "Priority": 1,
                        "Action": {"Type": "BLOCK"},
                        "Type": rule_type,
                    },
                }],
            }),
            "WAFInvalidParameterException",
            rule_type,
        )
        .await;
        assert_eq!(
            body["field"].as_str(),
            Some("RULE_TYPE"),
            "case {rule_type}: the refusal names the rule type as the field, got {body}"
        );
        assert_eq!(
            body["reason"].as_str(),
            Some("INVALID_OPTION"),
            "case {rule_type}: the refusal names the reason the model declares, got {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {rule_type}: a refused UpdateRuleGroup must not consume the change token"
        );
        let stored = activated_rules(&router, &group).await;
        assert!(
            stored.is_empty(),
            "case {rule_type}: a refused UpdateRuleGroup must persist nothing, got {stored:?}"
        );
    }
}

/// "ActivatedRule|OverrideAction applies only when updating or adding a RuleGroup to a
/// WebACL. In this case you do not use ActivatedRule|Action. For all other update
/// requests, ActivatedRule|Action is used instead of ActivatedRule|OverrideAction" — so
/// an activation inserted here carries an `Action`, and one carrying an `OverrideAction`
/// instead, or as well, is the modelled invalid parameter.
#[tokio::test]
async fn insert_uses_the_action_and_not_the_override_action() {
    let router = router();
    let group = create_rule_group(&router, "action-group").await;
    let rule = create_rule(&router, "action-rule").await;

    // (label, the ActivatedRule the request carries, the field the refusal names)
    let cases: &[(&str, Value, &str)] = &[
        (
            "no action at all",
            json!({"RuleId": rule, "Priority": 1}),
            "WAF_ACTION",
        ),
        (
            "an OverrideAction of NONE instead of an Action",
            json!({"RuleId": rule, "Priority": 1, "OverrideAction": {"Type": "NONE"}}),
            "WAF_OVERRIDE_ACTION",
        ),
        (
            "an OverrideAction of COUNT instead of an Action",
            json!({"RuleId": rule, "Priority": 1, "OverrideAction": {"Type": "COUNT"}}),
            "WAF_OVERRIDE_ACTION",
        ),
        (
            "an OverrideAction beside an Action",
            json!({
                "RuleId": rule,
                "Priority": 1,
                "Action": {"Type": "BLOCK"},
                "OverrideAction": {"Type": "COUNT"},
            }),
            "WAF_OVERRIDE_ACTION",
        ),
    ];

    for (label, activated, field) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [{"Action": "INSERT", "ActivatedRule": activated}],
            }),
            "WAFInvalidParameterException",
            label,
        )
        .await;
        assert_eq!(
            body["field"].as_str(),
            Some(*field),
            "case {label}: the refusal names the field it did not accept, got {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );
        let stored = activated_rules(&router, &group).await;
        assert!(
            stored.is_empty(),
            "case {label}: a refused UpdateRuleGroup must persist nothing, got {stored:?}"
        );
    }
}

/// `Priority` "must be a unique integer", so an insert taking the priority another
/// activation of the same rule group already holds is the modelled invalid parameter —
/// while the same priority in another rule group is free.
#[tokio::test]
async fn insert_refuses_a_priority_another_activation_holds() {
    let router = router();
    let group = create_rule_group(&router, "priority-group").await;
    let other_group = create_rule_group(&router, "priority-other-group").await;
    let held = create_rule(&router, "priority-held-rule").await;
    let candidate = create_rule(&router, "priority-candidate-rule").await;

    for (label, priority) in [
        ("a positive priority", 4),
        ("priority zero", 0),
        ("a negative priority", -4),
    ] {
        let (status, body, _) = apply_updates(
            &router,
            &group,
            vec![update("INSERT", &held, priority, "BLOCK")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: the setup insert must be accepted, got {status}: {body}"
        );

        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [update("INSERT", &candidate, priority, "COUNT")],
            }),
            "WAFInvalidParameterException",
            label,
        )
        .await;
        assert_eq!(
            body["reason"].as_str(),
            Some("ILLEGAL_ARGUMENT"),
            "case {label}: the refusal names the reason the model declares, got {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );
        assert_eq!(
            activated_rules(&router, &group).await,
            vec![activation(&held, priority, "BLOCK")],
            "case {label}: the stored rule group must be unchanged by the refusal"
        );

        // Another rule group's activations are its own, so the priority is free there.
        let (status, body, _) = apply_updates(
            &router,
            &other_group,
            vec![update("INSERT", &candidate, priority, "COUNT")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: the same priority in another rule group must be accepted, \
             got {status}: {body}"
        );

        // Leave both groups empty for the next row.
        for (group_id, rule_id, waf_action) in [
            (&group, &held, "BLOCK"),
            (&other_group, &candidate, "COUNT"),
        ] {
            let (status, body, _) = apply_updates(
                &router,
                group_id,
                vec![update("DELETE", rule_id, priority, waf_action)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label}: the teardown delete must be accepted, got {status}: {body}"
            );
        }
    }
}

/// An update that would leave the rule group exactly as it was is the modelled "there
/// was nothing to do": an `INSERT` for a rule it already activates, and a `DELETE` of
/// one it does not activate — the activation's `RuleId` being its identity.
#[tokio::test]
async fn an_update_that_changes_nothing_is_refused() {
    let router = router();
    let group = create_rule_group(&router, "nothing-to-do-group").await;
    let held = create_rule(&router, "nothing-to-do-held").await;
    let absent = create_rule(&router, "nothing-to-do-absent").await;
    let (status, body, _) = apply_updates(
        &router,
        &group,
        vec![update("INSERT", &held, 7, "BLOCK")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    // (label, the update that must change nothing)
    let cases: &[(&str, Value)] = &[
        (
            "inserting the rule the group already activates",
            update("INSERT", &held, 7, "BLOCK"),
        ),
        (
            "inserting it again under another action",
            update("INSERT", &held, 7, "COUNT"),
        ),
        (
            "deleting a rule the group never activated",
            update("DELETE", &absent, 7, "BLOCK"),
        ),
        (
            "deleting an id that was never minted",
            update("DELETE", UNKNOWN_ID, 1, "BLOCK"),
        ),
    ];

    for (label, candidate) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [candidate],
            }),
            "WAFInvalidOperationException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_INVALID_OPERATION,
            "case {label}: the refusal carries WAF Classic's fixed nothing-to-do \
             sentence, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );
        assert_eq!(
            activated_rules(&router, &group).await,
            vec![activation(&held, 7, "BLOCK")],
            "case {label}: the stored rule group must be unchanged by the refusal"
        );
    }
}

/// A request is applied whole or not at all: when a later update is refused, the earlier
/// ones in the same request are not persisted and the change token is still unspent.
#[tokio::test]
async fn a_request_refused_part_way_through_persists_nothing() {
    let router = router();
    let group = create_rule_group(&router, "atomic-group").await;
    let first = create_rule(&router, "atomic-rule-one").await;
    let second = create_rule(&router, "atomic-rule-two").await;

    // (label, the updates one request carries) — each row's first update is applicable
    // and a later one is not.
    let cases: &[(&str, Vec<Value>)] = &[
        (
            "a second update naming a rule that does not exist",
            vec![
                update("INSERT", &first, 1, "BLOCK"),
                update("INSERT", UNKNOWN_ID, 2, "BLOCK"),
            ],
        ),
        (
            "a second update that changes nothing",
            vec![
                update("INSERT", &first, 1, "BLOCK"),
                update("DELETE", &second, 2, "BLOCK"),
            ],
        ),
        (
            "an insert repeated inside one request",
            vec![
                update("INSERT", &first, 1, "BLOCK"),
                update("INSERT", &first, 2, "BLOCK"),
            ],
        ),
        (
            "a second update taking the priority the first took",
            vec![
                update("INSERT", &first, 1, "BLOCK"),
                update("INSERT", &second, 1, "COUNT"),
            ],
        ),
    ];

    for (label, updates) in cases {
        let (status, body, token) = apply_updates(&router, &group, updates.clone()).await;
        assert_eq!(
            status, 400,
            "case {label}: the request must be refused, got {status}: {body}"
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );
        let stored = activated_rules(&router, &group).await;
        assert!(
            stored.is_empty(),
            "case {label}: no update of a refused request may be persisted, got {stored:?}"
        );
    }
}

/// `UpdateRuleGroup` for a rule group this service holds nothing for is the modelled
/// nonexistent-container refusal — the rule group is the container the activations go
/// into — and the read of such an id, or of a request naming none, is the modelled
/// nonexistent item.
#[tokio::test]
async fn an_unknown_rule_group_id_is_refused_by_both_the_update_and_the_read() {
    let router = router();
    let rule = create_rule(&router, "container-rule").await;
    let group = create_rule_group(&router, "container-group").await;

    let cases: &[(&str, &str)] = &[
        ("a UUID-shaped id that was never minted", UNKNOWN_ID),
        ("the zero id", "00000000-0000-0000-0000-000000000000"),
        ("a one-character id", "x"),
        // An id this service minted for another resource kind is not a RuleGroupId.
        ("the id of a stored Rule", rule.as_str()),
    ];

    for (label, rule_group_id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": rule_group_id,
                "ChangeToken": token,
                "Updates": [update("INSERT", &rule, 1, "BLOCK")],
            }),
            "WAFNonexistentContainerException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_CONTAINER,
            "case {label}: the refusal carries WAF Classic's fixed container sentence, \
             got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );

        let body = expect_error(
            &router,
            "ListActivatedRulesInRuleGroup",
            &json!({"RuleGroupId": rule_group_id}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: the read's refusal carries WAF Classic's fixed not-found \
             sentence, got {:?}",
            error_message(&body)
        );
    }

    // A read naming no rule group at all asks for the activations of a rule group that
    // does not exist either.
    expect_error(
        &router,
        "ListActivatedRulesInRuleGroup",
        &json!({}),
        "WAFNonexistentItemException",
        "a read naming no rule group",
    )
    .await;

    // The rule group that EXISTS still reads back, empty.
    let stored = activated_rules(&router, &group).await;
    assert!(
        stored.is_empty(),
        "a created rule group activates no rule, got {stored:?}"
    );
}

/// "Each create, update, or delete request must use a unique change token": a token
/// already spent and one no `GetChangeToken` minted are both stale data, and such a
/// request changes nothing.
#[tokio::test]
async fn update_refuses_a_stale_or_unknown_change_token() {
    let router = router();
    let group = create_rule_group(&router, "token-group").await;
    let held = create_rule(&router, "token-held-rule").await;
    let other = create_rule(&router, "token-other-rule").await;
    let (status, body, spent) = apply_updates(
        &router,
        &group,
        vec![update("INSERT", &held, 1, "BLOCK")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    for (label, token) in [
        ("a token this UpdateRuleGroup already spent", spent.as_str()),
        ("a token no GetChangeToken minted", UNKNOWN_ID),
    ] {
        let body = expect_error(
            &router,
            "UpdateRuleGroup",
            &json!({
                "RuleGroupId": group,
                "ChangeToken": token,
                "Updates": [update("INSERT", &other, 2, "COUNT")],
            }),
            "WAFStaleDataException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_STALE_DATA,
            "case {label}: the refusal carries WAF Classic's fixed stale-token sentence, \
             got {:?}",
            error_message(&body)
        );
        assert_eq!(
            activated_rules(&router, &group).await,
            vec![activation(&held, 1, "BLOCK")],
            "case {label}: a refused UpdateRuleGroup must persist nothing"
        );
    }
}

/// "You can have a maximum of ten rules per rule group": inserts are accepted up to that
/// quota and refused past it, and a request carrying more updates than one request may
/// carry is refused before any of them is applied.
#[tokio::test]
async fn update_refuses_past_the_rules_per_group_quota() {
    let router = router();
    let group = create_rule_group(&router, "quota-group").await;

    // One rule per activation the group may hold, plus one more to insert past the
    // quota.
    let mut rules = Vec::new();
    for index in 0..=MAX_RULE_GROUP_RULES {
        rules.push(create_rule(&router, &format!("quota-rule-{index}")).await);
    }

    for (index, rule) in rules.iter().take(MAX_RULE_GROUP_RULES).enumerate() {
        let priority = i32::try_from(index).expect("the quota fits an i32");
        let (status, body, _) = apply_updates(
            &router,
            &group,
            vec![update("INSERT", rule, priority, "BLOCK")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case activation {index}: every insert up to the quota must be accepted, \
             got {status}: {body}"
        );
    }
    assert_eq!(
        activated_rules(&router, &group).await.len(),
        MAX_RULE_GROUP_RULES,
        "the rule group must hold every activation up to the quota"
    );

    let past_quota = vec![update(
        "INSERT",
        rules.last().expect("the table has a last row"),
        i32::try_from(MAX_RULE_GROUP_RULES).expect("the quota fits an i32"),
        "BLOCK",
    )];
    // One update past the per-request bound: the deletes and inserts that would empty
    // the group and fill it again, plus one more.
    let mut past_request_bound: Vec<Value> = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let priority = i32::try_from(index).expect("the quota fits an i32");
        past_request_bound.push(update("DELETE", rule, priority, "BLOCK"));
        past_request_bound.push(update("INSERT", rule, priority, "BLOCK"));
    }
    assert!(
        past_request_bound.len() > MAX_RULE_GROUP_UPDATES_PER_REQUEST,
        "the per-request row must carry more updates than one request may carry, got {}",
        past_request_bound.len()
    );

    for (label, updates) in [
        ("one insert past the per-group quota", past_quota),
        (
            "a request past the per-request update bound",
            past_request_bound,
        ),
    ] {
        let (status, body, token) = apply_updates(&router, &group, updates).await;
        assert_eq!(
            status, 400,
            "case {label}: the request must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_message(&body),
            MESSAGE_LIMITS_EXCEEDED,
            "case {label}: the refusal carries WAF Classic's fixed limit sentence, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused UpdateRuleGroup must not consume the change token"
        );
        assert_eq!(
            activated_rules(&router, &group).await.len(),
            MAX_RULE_GROUP_RULES,
            "case {label}: the stored rule group must be unchanged by the refusal"
        );
    }
}

/// A read changes nothing a later caller sees: two reads of one rule group report the
/// same activations, and a read between two updates does not disturb what the next
/// update applies.
#[tokio::test]
async fn reading_the_activations_changes_nothing() {
    let router = router();
    let group = create_rule_group(&router, "read-only-group").await;
    let rule = create_rule(&router, "read-only-rule").await;
    let (status, body, _) = apply_updates(
        &router,
        &group,
        vec![update("INSERT", &rule, 3, "COUNT")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup insert must be accepted, got {status}: {body}"
    );

    let first = activated_rules(&router, &group).await;
    let second = activated_rules(&router, &group).await;
    assert_eq!(
        first, second,
        "a second read must report the same activations, got {second:?} after {first:?}"
    );
    assert_eq!(
        first,
        vec![activation(&rule, 3, "COUNT")],
        "the read must report the stored activation"
    );

    // The delete still applies after the reads, so neither read consumed anything.
    let (status, body, _) = apply_updates(
        &router,
        &group,
        vec![update("DELETE", &rule, 3, "COUNT")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete after the reads must be accepted, got {status}: {body}"
    );
    let remaining = activated_rules(&router, &group).await;
    assert!(
        remaining.is_empty(),
        "the rule group must hold no activation after the delete, got {remaining:?}"
    );
}
