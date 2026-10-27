// SPDX-License-Identifier: CC0-1.0

//! Deleting a receipt rule set, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs, not one recorded request:
//!
//! * a stored set that is not the active one is removed with the rules it holds,
//!   so neither the per-set reader nor the listing reports it afterwards, and
//!   every other set is left where it was;
//! * the set email receiving is currently served by is NOT removed: the delete is
//!   refused with the modelled `CannotDelete`, the set stays readable, listed and
//!   active, and the same delete succeeds once another set (or none) is active;
//! * a name the account does not hold is already in the state the caller asked
//!   for, so the delete succeeds and changes nothing — however often it is
//!   repeated.

use crate::harness::{call, element, elements, emulator_over, field};
use service_com_amazonaws_ses_simpleemailservice::ops::types::ReceiptRule;
use service_com_amazonaws_ses_simpleemailservice::state::{
    ComAmazonawsSesSimpleemailserviceState, MAX_RECEIPT_RULE_SET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A router holding only this service, beside the engine it was built over, so a
/// suite can seed a rule set holding RULES — `CreateReceiptRule` is still a
/// scaffold stub, so the rules of a set cannot be written over the wire yet.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `DeleteReceiptRuleSet`.
async fn delete_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DeleteReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `DescribeReceiptRuleSet`.
async fn describe_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DescribeReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `SetActiveReceiptRuleSet` naming `name`.
async fn set_active(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "SetActiveReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `SetActiveReceiptRuleSet` carrying no name, which leaves no set active.
async fn set_active_none(router: &MultiServiceRouter) -> (u16, String) {
    call(router, "SetActiveReceiptRuleSet", &[]).await
}

/// `DescribeActiveReceiptRuleSet`, as the name of the set it reports (if any).
async fn active_rule_set_name(router: &MultiServiceRouter) -> Option<String> {
    let (status, body) = call(router, "DescribeActiveReceiptRuleSet", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeActiveReceiptRuleSet: expected 200, got {status} with {body}"
    );
    element(&body, "Metadata").and_then(|metadata| element(&metadata, "Name"))
}

/// The names `ListReceiptRuleSets` reports for the account, in listing order.
async fn listed_rule_set_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListReceiptRuleSets", &[]).await;
    assert_eq!(
        status, 200,
        "ListReceiptRuleSets: expected 200, got {status} with {body}"
    );
    elements(&element(&body, "RuleSets").unwrap_or_default(), "Name")
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// A rule to seed a set with, so a deleted set has rules to lose.
fn seeded_rule(name: &str, recipients: &[&str], enabled: bool) -> ReceiptRule {
    ReceiptRule {
        name: name.to_string(),
        enabled,
        tls_policy: Some("Optional".to_string()),
        recipients: Some(recipients.iter().map(|at| (*at).to_string()).collect()),
        actions: None,
        scan_enabled: true,
    }
}

/// Give the stored set `name` the rules `rules`, which no implemented operation
/// can write yet.
fn seed_rules(engine: &engine::InMemoryEngine, name: &str, rules: Vec<ReceiptRule>) {
    engine
        .state::<ComAmazonawsSesSimpleemailserviceState>()
        .lock()
        .receipt_rule_sets
        .get_mut(name)
        .expect("the set was created over the wire")
        .rules = rules;
}

/// `n` rules, so a row can differ in how many rules the deleted set held.
fn rules(count: usize) -> Vec<ReceiptRule> {
    (0..count)
        .map(|index| {
            seeded_rule(
                &format!("rule-{index}"),
                &["sales@example.com", "example.net"][..index.min(2)],
                index % 2 == 0,
            )
        })
        .collect()
}

/// Deleting a set the account holds removes it and the rules it contained: the
/// per-set reader refuses the name afterwards, the listing no longer carries it,
/// and the name is free again — holding no rules when it is re-created. The rows
/// differ along the name's form and length and the number of rules the set held.
#[tokio::test]
async fn a_deleted_rule_set_and_its_rules_are_gone() {
    let (router, engine) = emulator_with_engine();
    // The documented name form (`DeleteReceiptRuleSetRequest$RuleSetName`): ASCII
    // letters, numbers, underscores or dashes, starting and ending with a letter
    // or number, 64 characters or fewer.
    let rows: Vec<(String, usize)> = vec![
        ("a".to_string(), 0),
        ("9".to_string(), 1),
        ("inbound-rules_2024".to_string(), 2),
        ("x".repeat(MAX_RECEIPT_RULE_SET_NAME), 3),
    ];
    for (name, rule_count) in &rows {
        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from CreateReceiptRuleSet, got {status} with {body}"
        );
        seed_rules(&engine, name, rules(*rule_count));
    }

    let mut remaining: Vec<String> = rows.iter().map(|(name, _)| name.clone()).collect();
    remaining.sort();
    for (name, rule_count) in &rows {
        let (status, body) = delete_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?} holding {rule_count} rule(s): expected 200 from \
             DeleteReceiptRuleSet, got {status} with {body}"
        );

        let (status, body) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a deleted set must not be readable, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {name:?}: expected the modelled RuleSetDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set does not exist: {name}").as_str()),
            "case {name:?}: expected the deleted set named in the message, got {body}"
        );

        // The listing is the account's record of which sets exist: the deleted
        // one is gone from it and every other one is still there.
        remaining.retain(|stored| stored != name);
        let listed = listed_rule_set_names(&router).await;
        assert_eq!(
            listed, remaining,
            "case {name:?}: expected the remaining sets {remaining:?} listed, got {listed:?}"
        );

        // The name is free again, and the set it names is empty: the rules went
        // with the set rather than surviving it.
        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the deleted name is free again, got {status} with {body}"
        );
        let (status, body) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 re-reading the re-created set, got {status} \
             with {body}"
        );
        assert_eq!(
            element(&body, "Rules").as_deref(),
            Some(""),
            "case {name:?}: the {rule_count} rule(s) of the deleted set must not survive it, \
             got {body}"
        );
        let (status, body) = delete_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 deleting the re-created set, got {status} with {body}"
        );
    }

    assert_eq!(
        listed_rule_set_names(&router).await,
        Vec::<String>::new(),
        "case every set deleted: the account holds no receipt rule sets"
    );
}

/// The currently ACTIVE set cannot be deleted: the delete is refused with the
/// modelled `CannotDelete`, naming the set, and the set stays stored, readable,
/// listed and active. Once another set — or none — is active, the same delete
/// succeeds. The rows differ along the name's form and length and the number of
/// rules the active set holds.
#[tokio::test]
async fn deleting_the_active_rule_set_is_refused_and_leaves_it_in_place() {
    // The second stage each row ends with: what the account is left active on
    // before the delete is retried.
    let rows: Vec<(String, usize, &str)> = vec![
        ("b".to_string(), 0, "another set active"),
        ("7".to_string(), 1, "no set active"),
        ("Prod-Inbound_2024".to_string(), 2, "another set active"),
        ("y".repeat(MAX_RECEIPT_RULE_SET_NAME), 3, "no set active"),
    ];
    for (name, rule_count, deactivation) in &rows {
        let (router, engine) = emulator_with_engine();
        // A second set, so a refusal below is about the ACTIVE set and not about
        // an account holding exactly one.
        for stored in [name.as_str(), "bystander-set"] {
            let (status, body) = create_rule_set(&router, stored).await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected 200 creating {stored:?}, got {status} with {body}"
            );
        }
        seed_rules(&engine, name, rules(*rule_count));
        let (status, body) = set_active(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from SetActiveReceiptRuleSet, got {status} with {body}"
        );

        // Refused, twice over: a refusal that wrote nothing is refused again the
        // same way.
        for attempt in 1..=2 {
            let (status, body) = delete_rule_set(&router, name).await;
            assert_eq!(
                status, 400,
                "case {name:?} attempt {attempt}: expected HTTP 400 deleting the active set, \
                 got {status} with {body}"
            );
            let (code, message) = error(&body);
            assert_eq!(
                code.as_deref(),
                Some("CannotDelete"),
                "case {name:?} attempt {attempt}: expected the modelled CannotDelete, got {body}"
            );
            assert_eq!(
                message.as_deref(),
                Some(format!("Cannot delete active rule set: {name}").as_str()),
                "case {name:?} attempt {attempt}: expected the active set named in the \
                 message, got {body}"
            );
        }

        // The refusal left the set in place: still readable with its rules, still
        // listed, and still the active one.
        let (status, body) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the refused delete must leave the set readable, got {status} \
             with {body}"
        );
        assert_eq!(
            elements(&element(&body, "Rules").unwrap_or_default(), "Name").len(),
            *rule_count,
            "case {name:?}: expected the set's {rule_count} rule(s) intact, got {body}"
        );
        let listed = listed_rule_set_names(&router).await;
        assert!(
            listed.contains(name),
            "case {name:?}: the refused delete must leave the set listed, got {listed:?}"
        );
        assert_eq!(
            active_rule_set_name(&router).await.as_deref(),
            Some(name.as_str()),
            "case {name:?}: the refused delete must leave the set active"
        );

        // A set that is NOT active is deletable while another one is: the guard
        // is about the active set, not about deleting at all.
        let (status, body) = delete_rule_set(&router, "bystander-set").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 deleting the set that is not active, got {status} \
             with {body}"
        );

        // Once the set is no longer the active one, the same delete succeeds.
        match *deactivation {
            "another set active" => {
                let (status, body) = create_rule_set(&router, "successor-set").await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected 200 creating the successor, got {status} \
                     with {body}"
                );
                let (status, body) = set_active(&router, "successor-set").await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected 200 activating the successor, got {status} \
                     with {body}"
                );
            }
            _ => {
                let (status, body) = set_active_none(&router).await;
                assert_eq!(
                    status, 200,
                    "case {name:?}: expected 200 leaving no set active, got {status} \
                     with {body}"
                );
            }
        }
        let (status, body) = delete_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?} with {deactivation}: expected 200 deleting the set once it is not \
             active, got {status} with {body}"
        );
        let (status, body) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?} with {deactivation}: the deleted set must not be readable, got \
             {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {name:?} with {deactivation}: expected RuleSetDoesNotExist, got {body}"
        );
        let (status, body) = set_active(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?} with {deactivation}: the deleted set must not be activatable, got \
             {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {name:?} with {deactivation}: expected RuleSetDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set does not exist: {name}").as_str()),
            "case {name:?} with {deactivation}: expected the dead name in the message, \
             got {body}"
        );
    }
}

/// Deleting a name the account does not hold is the state the caller asked for:
/// the call succeeds, removes nothing, and leaves the active set alone — whatever
/// the form of the absent name, and however often it is repeated.
#[tokio::test]
async fn deleting_a_rule_set_the_account_does_not_hold_changes_nothing() {
    let (router, _engine) = emulator_with_engine();
    for stored in ["held-set", "other-held-set"] {
        let (status, body) = create_rule_set(&router, stored).await;
        assert_eq!(
            status, 200,
            "expected 200 creating {stored:?}, got {status} with {body}"
        );
    }
    let (status, body) = set_active(&router, "held-set").await;
    assert_eq!(
        status, 200,
        "SetActiveReceiptRuleSet(held-set): expected 200, got {status} with {body}"
    );
    let before = listed_rule_set_names(&router).await;

    // The rows differ in the form and length of the ABSENT name, including names
    // one character either side of a name the account does hold.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "a".to_string(),
        "0".to_string(),
        "Absent_Rule-Set_2024".to_string(),
        "held-se".to_string(),
        "held-sets".to_string(),
        "n".repeat(MAX_RECEIPT_RULE_SET_NAME),
    ];
    for name in &absent {
        for attempt in 1..=2 {
            let (status, body) = delete_rule_set(&router, name).await;
            assert_eq!(
                status, 200,
                "case {name:?} attempt {attempt}: expected 200 deleting a set the account \
                 does not hold, got {status} with {body}"
            );
        }

        let listed = listed_rule_set_names(&router).await;
        assert_eq!(
            listed, before,
            "case {name:?}: the delete must leave the stored sets {before:?} alone, \
             got {listed:?}"
        );
        assert_eq!(
            active_rule_set_name(&router).await.as_deref(),
            Some("held-set"),
            "case {name:?}: the delete must leave the active set alone"
        );
    }
}
