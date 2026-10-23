// SPDX-License-Identifier: CC0-1.0

//! `DeleteRule`, end to end over the wire: `CreateRule`, `UpdateRule`, `GetRule`,
//! `ListRules`, `DeleteRule`.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees: each answer is computed from the persisted
//! record, not from the request.
//!
//! The rule under test is `DeleteRule`'s own documentation: "Permanently deletes a
//! Rule. You can't delete a Rule if it's still used in any WebACL objects or if it
//! still includes any predicates, such as ByteMatchSet objects. If you just want to
//! remove a Rule from a WebACL, use UpdateWebACL. To permanently delete a Rule from AWS
//! WAF, perform the following steps: Update the Rule to remove predicates, if any. […]
//! Use GetChangeToken to get the change token that you provide in the ChangeToken
//! parameter of a DeleteRule request."

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::{
    ActivatedRule, WafAction,
};
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    ComAmazonawsWafregionalAwswafRegional20161128State, MESSAGE_NONEXISTENT_ITEM,
    MESSAGE_NON_EMPTY_ENTITY, MESSAGE_REFERENCED_ITEM, MESSAGE_STALE_DATA,
};

use crate::wire::{
    call, change_token, error_message, expect_error, ok_call, router, router_on, token_status,
};

/// A UUID-shaped id this service never minted.
const UNKNOWN_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Create a rule, optionally tagged, and return its `RuleId`.
async fn create_rule(
    router: &MultiServiceRouter,
    name: &str,
    metric_name: &str,
    tags: &Value,
) -> String {
    let token = change_token(router).await;
    let mut request = json!({"Name": name, "MetricName": metric_name, "ChangeToken": token});
    if !tags.is_null() {
        request["Tags"] = tags.clone();
    }
    let body = ok_call(router, "CreateRule", &request).await;
    body["Rule"]["RuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRule returned no RuleId: {body}"))
        .to_string()
}

/// Create an `IPSet` and return its `IPSetId`.
async fn create_ip_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateIPSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {body}"))
        .to_string()
}

/// Create an `XssMatchSet` and return its `XssMatchSetId`.
async fn create_xss_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateXssMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["XssMatchSet"]["XssMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateXssMatchSet returned no XssMatchSetId: {body}"))
        .to_string()
}

/// Create a `ByteMatchSet` and return its `ByteMatchSetId`.
async fn create_byte_match_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateByteMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["ByteMatchSet"]["ByteMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateByteMatchSet returned no ByteMatchSetId: {body}"))
        .to_string()
}

/// Create a `GeoMatchSet` and return its `GeoMatchSetId`.
async fn create_geo_match_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateGeoMatchSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["GeoMatchSet"]["GeoMatchSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGeoMatchSet returned no GeoMatchSetId: {body}"))
        .to_string()
}

/// Create a `WebACL` and return its `WebACLId`.
async fn create_web_acl(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateWebACL",
        &json!({
            "Name": name,
            "MetricName": "ruleDeletionWebAcl",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
        }),
    )
    .await;
    body["WebACL"]["WebACLId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebACL returned no WebACLId: {body}"))
        .to_string()
}

/// Apply one `RuleUpdate` to `rule_id` with a fresh change token.
async fn apply_update(
    router: &MultiServiceRouter,
    rule_id: &str,
    action: &str,
    kind: &str,
    data_id: &str,
    label: &str,
) {
    let token = change_token(router).await;
    let (status, body) = call(
        router,
        "UpdateRule",
        &json!({
            "RuleId": rule_id,
            "ChangeToken": token,
            "Updates": [{
                "Action": action,
                "Predicate": {"Negated": false, "Type": kind, "DataId": data_id},
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label}: UpdateRule must accept the {action}, got {status}: {body}"
    );
}

/// Submit `DeleteRule` for `rule_id` with a fresh change token, answering
/// `(http status, body, the token the request carried)`.
async fn delete_rule(router: &MultiServiceRouter, rule_id: &str) -> (u16, Value, String) {
    let token = change_token(router).await;
    let (status, body) = call(
        router,
        "DeleteRule",
        &json!({"RuleId": rule_id, "ChangeToken": token}),
    )
    .await;
    (status, body, token)
}

/// The `RuleId`s `ListRules` reports.
async fn listed_rule_ids(router: &MultiServiceRouter) -> Vec<String> {
    let body = ok_call(router, "ListRules", &json!({"Limit": 100})).await;
    body["Rules"]
        .as_array()
        .unwrap_or_else(|| panic!("ListRules answered no Rules array: {body}"))
        .iter()
        .map(|summary| summary["RuleId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `DeleteRule` permanently removes an empty, unreferenced rule: it answers the change
/// token it consumed, `GetRule` then answers the modelled not-found, `ListRules` stops
/// listing it and the stored record (with its tags) is gone — across a table of rules
/// differing in name, metric name, tag set and the predicate history they went through.
#[tokio::test]
async fn delete_removes_the_stored_rule_and_its_record() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());
    let ip_set = create_ip_set(&router, "deletion-ip-set").await;
    let max_length = "d".repeat(128);

    // (label, Name, MetricName, Tags, whether the rule held a predicate that was then
    // removed with UpdateRule) — `ResourceName` and `MetricName` are len 1..128, and a
    // rule's tag set and predicate history are both part of the record a delete must
    // take with it.
    type Case<'a> = (&'a str, &'a str, &'a str, Value, bool);
    let cases: Vec<Case<'_>> = vec![
        ("a plain untagged rule", "delete-me", "deleteMe", Value::Null, false),
        (
            "one character (the minimum name length)",
            "x",
            "x",
            Value::Null,
            false,
        ),
        (
            "128 characters (the maximum name length)",
            max_length.as_str(),
            max_length.as_str(),
            Value::Null,
            false,
        ),
        (
            "whitespace inside the resource name",
            "two words",
            "twoWords",
            Value::Null,
            false,
        ),
        (
            "a tagged rule",
            "tagged-delete",
            "taggedDelete",
            json!([{"Key": "env", "Value": "prod"}, {"Key": "team", "Value": "waf"}]),
            false,
        ),
        (
            "a rule emptied again with UpdateRule",
            "emptied-delete",
            "emptiedDelete",
            Value::Null,
            true,
        ),
    ];

    for (label, name, metric_name, tags, had_predicate) in &cases {
        let rule = create_rule(&router, name, metric_name, tags).await;
        if *had_predicate {
            apply_update(&router, &rule, "INSERT", "IPMatch", &ip_set, label).await;
            apply_update(&router, &rule, "DELETE", "IPMatch", &ip_set, label).await;
        }

        let (status, body, token) = delete_rule(&router, &rule).await;
        assert_eq!(
            status, 200,
            "case {label}: DeleteRule must answer 200, got {status}: {body}"
        );
        assert_eq!(
            body["ChangeToken"].as_str(),
            Some(token.as_str()),
            "case {label}: DeleteRule must report the change token it consumed, got {body}"
        );
        // A token a mutating call consumed is never PROVISIONED again.
        let status_after = token_status(&router, &token).await;
        assert!(
            status_after == "PENDING" || status_after == "INSYNC",
            "case {label}: the consumed token must be PENDING or INSYNC, got {status_after:?}"
        );

        // The record is gone: the reader, the listing and the store all agree.
        let not_found = expect_error(
            &router,
            "GetRule",
            &json!({"RuleId": rule}),
            "WAFNonexistentItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&not_found),
            MESSAGE_NONEXISTENT_ITEM,
            "case {label}: GetRule after the delete carries the fixed not-found sentence, \
             got {:?}",
            error_message(&not_found)
        );
        let listed = listed_rule_ids(&router).await;
        assert!(
            !listed.contains(&rule),
            "case {label}: ListRules must not list the deleted rule {rule:?}, got {listed:?}"
        );
        assert!(
            !engine
                .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
                .lock()
                .rules
                .contains_key(&rule),
            "case {label}: the deleted rule must leave no stored record"
        );
    }

    // Every rule this test created was deleted, so nothing is left to list: the delete
    // removed records rather than hiding them.
    let listed = listed_rule_ids(&router).await;
    assert!(
        listed.is_empty(),
        "every created rule was deleted, so ListRules must list none, got {listed:?}"
    );
}

/// A rule that still holds a predicate is not empty, so the delete is refused with the
/// modelled `WAFNonEmptyEntityException` and changes nothing — for a predicate of every
/// condition kind this service stores, and whatever the number of predicates. Removing
/// them with `UpdateRule` is what makes the rule deletable.
#[tokio::test]
async fn delete_refuses_a_rule_that_still_holds_predicates() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());
    let ip_set = create_ip_set(&router, "non-empty-ip-set").await;
    let xss_set = create_xss_set(&router, "non-empty-xss-set").await;
    let byte_set = create_byte_match_set(&router, "non-empty-byte-set").await;
    let geo_set = create_geo_match_set(&router, "non-empty-geo-set").await;

    // (label, the predicates the rule holds as (PredicateType, DataId)) — one of each
    // condition kind, and a rule holding several at once.
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("one IPSet predicate", &[("IPMatch", ip_set.as_str())]),
        ("one XssMatchSet predicate", &[("XssMatch", xss_set.as_str())]),
        (
            "one ByteMatchSet predicate",
            &[("ByteMatch", byte_set.as_str())],
        ),
        ("one GeoMatchSet predicate", &[("GeoMatch", geo_set.as_str())]),
        (
            "several predicates of different kinds",
            &[
                ("IPMatch", ip_set.as_str()),
                ("XssMatch", xss_set.as_str()),
                ("GeoMatch", geo_set.as_str()),
            ],
        ),
    ];

    for (label, predicates) in cases {
        let rule = create_rule(&router, "non-empty-rule", "nonEmptyRule", &Value::Null).await;
        for (kind, data_id) in *predicates {
            apply_update(&router, &rule, "INSERT", kind, data_id, label).await;
        }

        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "DeleteRule",
            &json!({"RuleId": rule, "ChangeToken": token}),
            "WAFNonEmptyEntityException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_NON_EMPTY_ENTITY,
            "case {label}: the refusal carries WAF Classic's fixed non-empty sentence, \
             got {:?}",
            error_message(&body)
        );
        // The refused request applied nothing: the token is unspent and the rule, with
        // every predicate, is still there.
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused DeleteRule must not consume the change token"
        );
        let stored = engine
            .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
            .lock()
            .rules
            .get(&rule)
            .map(|entry| entry.predicates.len());
        assert_eq!(
            stored,
            Some(predicates.len()),
            "case {label}: the refused rule must keep every predicate, got {stored:?}"
        );

        // Emptying the rule, as the documented procedure says, makes it deletable.
        for (kind, data_id) in *predicates {
            apply_update(&router, &rule, "DELETE", kind, data_id, label).await;
        }
        let (status, body, _) = delete_rule(&router, &rule).await;
        assert_eq!(
            status, 200,
            "case {label}: an emptied rule must delete, got {status}: {body}"
        );
    }
}

/// A rule a `WebACL` activates is still in use, so the delete is refused with the
/// modelled `WAFReferencedItemException` and changes nothing; removing the activation
/// ("If you just want to remove a Rule from a WebACL, use UpdateWebACL") makes it
/// deletable. A rule no web ACL activates is deletable while another rule is activated.
#[tokio::test]
async fn delete_refuses_a_rule_a_web_acl_still_activates() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());

    // `UpdateWebACL` is still a scaffold in this emulator (its own stub issue), so the
    // activation is written straight onto the stored web ACL — the same record
    // `UpdateWebACL` will write and `GetWebACL` reads.
    fn activate(engine: &engine::InMemoryEngine, web_acl: &str, rule: &str, priority: i32) {
        let state = engine.state::<ComAmazonawsWafregionalAwswafRegional20161128State>();
        let mut state = state.lock();
        let entry = state
            .web_acls
            .get_mut(web_acl)
            .unwrap_or_else(|| panic!("the web ACL {web_acl} must be stored"));
        entry.rules.push(ActivatedRule {
            priority,
            rule_id: rule.to_string(),
            action: Some(WafAction {
                r#type: "BLOCK".to_string(),
            }),
            override_action: None,
            r#type: Some("REGULAR".to_string()),
            excluded_rules: None,
        });
    }

    /// Drop every activation of `rule` from every stored web ACL.
    fn deactivate(engine: &engine::InMemoryEngine, rule: &str) {
        let state = engine.state::<ComAmazonawsWafregionalAwswafRegional20161128State>();
        let mut state = state.lock();
        for entry in state.web_acls.values_mut() {
            entry.rules.retain(|activated| activated.rule_id != rule);
        }
    }

    let first_acl = create_web_acl(&router, "deletion-web-acl-one").await;
    let second_acl = create_web_acl(&router, "deletion-web-acl-two").await;

    // (label, the web ACLs that activate the rule, the priority each activation carries)
    let cases: &[(&str, &[&str], i32)] = &[
        ("the only web ACL activates it", &[first_acl.as_str()], 0),
        (
            "the second web ACL activates it",
            &[second_acl.as_str()],
            7,
        ),
        (
            "both web ACLs activate it",
            &[first_acl.as_str(), second_acl.as_str()],
            3,
        ),
    ];

    for (label, acls, priority) in cases {
        let rule = create_rule(&router, "referenced-rule", "referencedRule", &Value::Null).await;
        for acl in *acls {
            activate(&engine, acl, &rule, *priority);
        }

        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "DeleteRule",
            &json!({"RuleId": rule, "ChangeToken": token}),
            "WAFReferencedItemException",
            label,
        )
        .await;
        assert_eq!(
            error_message(&body),
            MESSAGE_REFERENCED_ITEM,
            "case {label}: the refusal carries WAF Classic's fixed in-use sentence, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            token_status(&router, &token).await,
            "PROVISIONED",
            "case {label}: a refused DeleteRule must not consume the change token"
        );
        let fetched = ok_call(&router, "GetRule", &json!({"RuleId": rule})).await;
        assert_eq!(
            fetched["Rule"]["RuleId"].as_str(),
            Some(rule.as_str()),
            "case {label}: the refused rule must still be readable, got {fetched}"
        );

        // Taken out of every web ACL, the same rule deletes.
        deactivate(&engine, &rule);
        let (status, body, _) = delete_rule(&router, &rule).await;
        assert_eq!(
            status, 200,
            "case {label}: a rule no web ACL activates must delete, got {status}: {body}"
        );
    }

    // A rule another rule's activation names is not itself referenced: only the rule a
    // web ACL activates is.
    let activated = create_rule(&router, "activated-rule", "activatedRule", &Value::Null).await;
    let bystander = create_rule(&router, "bystander-rule", "bystanderRule", &Value::Null).await;
    activate(&engine, &first_acl, &activated, 1);
    let (status, body, _) = delete_rule(&router, &bystander).await;
    assert_eq!(
        status, 200,
        "case a rule no web ACL activates: the delete must succeed, got {status}: {body}"
    );
    let token = change_token(&router).await;
    expect_error(
        &router,
        "DeleteRule",
        &json!({"RuleId": activated, "ChangeToken": token}),
        "WAFReferencedItemException",
        "the activated rule beside it",
    )
    .await;
}

/// An id this service holds no rule for is the modelled not-found, and a rule is
/// deleted exactly once: the second delete of the same id is a not-found too. Neither
/// refusal spends the change token.
#[tokio::test]
async fn delete_refuses_an_id_no_rule_is_stored_under() {
    let router = router();
    let kept = create_rule(&router, "kept-rule", "keptRule", &Value::Null).await;
    let deleted = create_rule(&router, "deleted-rule", "deletedRule", &Value::Null).await;
    let (status, body, _) = delete_rule(&router, &deleted).await;
    assert_eq!(
        status, 200,
        "the first delete must succeed, got {status}: {body}"
    );

    let cases: &[(&str, &str)] = &[
        ("a UUID-shaped id that was never minted", UNKNOWN_ID),
        ("the zero id", "00000000-0000-0000-0000-000000000000"),
        ("a one-character id", "x"),
        ("the id of an already deleted rule", deleted.as_str()),
    ];
    for (label, id) in cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "DeleteRule",
            &json!({"RuleId": id, "ChangeToken": token}),
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
            "case {label}: a refused DeleteRule must not consume the change token"
        );
    }

    // The rule that does exist survived every refusal.
    let fetched = ok_call(&router, "GetRule", &json!({"RuleId": kept})).await;
    assert_eq!(
        fetched["Rule"]["RuleId"].as_str(),
        Some(kept.as_str()),
        "the stored rule must survive every refusal, got {fetched}"
    );
}

/// "Each create, update, or delete request must use a unique change token": a token no
/// `GetChangeToken` minted, and one another mutating call already spent, are both stale
/// data, and the rule survives.
#[tokio::test]
async fn delete_refuses_a_stale_or_unknown_change_token() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(engine.clone());
    let rule = create_rule(&router, "token-probe-rule", "tokenProbeRule", &Value::Null).await;

    // A token the CreateRule above spent, plus one this service never minted.
    let spent = change_token(&router).await;
    ok_call(
        &router,
        "CreateRule",
        &json!({"Name": "spender", "MetricName": "spender", "ChangeToken": spent}),
    )
    .await;

    for (label, token) in [
        ("already consumed by another call", spent.clone()),
        ("never minted", UNKNOWN_ID.to_string()),
    ] {
        let body = expect_error(
            &router,
            "DeleteRule",
            &json!({"RuleId": rule, "ChangeToken": token}),
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
        assert!(
            engine
                .state::<ComAmazonawsWafregionalAwswafRegional20161128State>()
                .lock()
                .rules
                .contains_key(&rule),
            "case {label}: a refused DeleteRule must leave the rule stored"
        );
    }

    // And a fresh token deletes it.
    let (status, body, _) = delete_rule(&router, &rule).await;
    assert_eq!(
        status, 200,
        "a fresh token must delete the rule, got {status}: {body}"
    );
}
