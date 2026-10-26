// SPDX-License-Identifier: CC0-1.0

//! The ACTIVE receipt rule set, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE each operation follows over
//! a SET of inputs, not one recorded request:
//!
//! * the account has at most ONE active receipt rule set, and the reader of it
//!   reports the metadata and rules of whichever set the last accepted
//!   activation named;
//! * an activation naming a set the account does not hold is refused with the
//!   modelled `RuleSetDoesNotExist` and changes nothing;
//! * an activation naming NO set leaves none active, and leaves every stored set
//!   in place.

use crate::harness::{call, element, elements, emulator_over, field};
use service_com_amazonaws_ses_simpleemailservice::ops::types::ReceiptRule;
use service_com_amazonaws_ses_simpleemailservice::state::{
    ComAmazonawsSesSimpleemailserviceState, ReceiptRuleSet, MAX_RECEIPT_RULE_SET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::SystemTime;

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

/// `SetActiveReceiptRuleSet` naming `name`.
async fn set_active(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "SetActiveReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `SetActiveReceiptRuleSet` carrying no rule set name at all, which is how the
/// operation is called to leave no set active.
async fn set_active_none(router: &MultiServiceRouter) -> (u16, String) {
    call(router, "SetActiveReceiptRuleSet", &[]).await
}

/// `DescribeActiveReceiptRuleSet`.
async fn describe_active(router: &MultiServiceRouter) -> (u16, String) {
    call(router, "DescribeActiveReceiptRuleSet", &[]).await
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

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// A rule to seed a set with, so an active set can hold rules.
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

/// Store a receipt rule set holding `rules` directly in the service's state.
fn seed_rule_set(engine: &engine::InMemoryEngine, name: &str, rules: Vec<ReceiptRule>) {
    engine
        .state::<ComAmazonawsSesSimpleemailserviceState>()
        .lock()
        .receipt_rule_sets
        .insert(
            name.to_string(),
            ReceiptRuleSet {
                name: name.to_string(),
                created_at: SystemTime::now(),
                rules,
            },
        );
}

/// The `<Name>` the active-set reader reports, if it reports a set at all.
fn active_name(body: &str) -> Option<String> {
    element(body, "Metadata").and_then(|metadata| element(&metadata, "Name"))
}

/// A fresh account has no active receipt rule set: the reader answers 200 and
/// names none, and it stays that way while sets are merely CREATED — only an
/// activation makes one active.
#[tokio::test]
async fn an_account_starts_with_no_active_rule_set() {
    let (router, _engine) = emulator_with_engine();

    let (status, body) = describe_active(&router).await;
    assert_eq!(
        status, 200,
        "case fresh account: expected 200 from DescribeActiveReceiptRuleSet, got {status} \
         with {body}"
    );
    assert_eq!(
        active_name(&body),
        None,
        "case fresh account: no set is active, got {body}"
    );

    for name in ["first-set", "second_set", "Third9"] {
        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: expected 200, got {status} with {body}"
        );
        let (status, body) = describe_active(&router).await;
        assert_eq!(
            status, 200,
            "case after creating {name:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            active_name(&body),
            None,
            "case after creating {name:?}: creating a set must not activate it, got {body}"
        );
    }
}

/// Activating a set the account holds round-trips: the reader reports that set's
/// name, the creation timestamp the set itself reports, and the rules it holds —
/// over sets differing in name form and in how many rules they carry. Activating
/// another set REPLACES the active one, and activating the same set twice is the
/// same state as activating it once.
#[tokio::test]
async fn an_activated_rule_set_round_trips_through_the_active_reader() {
    let (router, engine) = emulator_with_engine();

    // The rows differ along the two things the rule generalises over: the form
    // and length of the name (`CreateReceiptRuleSetRequest$RuleSetName`: ASCII
    // letters, numbers, underscores or dashes, starting and ending with a letter
    // or number, 64 characters or fewer) and the number of rules the set holds.
    let rows: Vec<(String, Vec<ReceiptRule>)> = vec![
        ("a".to_string(), Vec::new()),
        ("9".to_string(), vec![seeded_rule("only", &[], true)]),
        (
            "inbound-rules_2024".to_string(),
            vec![
                seeded_rule("catch-all", &["example.com"], true),
                seeded_rule("sales", &["sales@example.com"], false),
            ],
        ),
        (
            "x".repeat(MAX_RECEIPT_RULE_SET_NAME),
            vec![
                seeded_rule("one", &["a@example.com"], true),
                seeded_rule("two", &["b@example.net", "c@example.org"], true),
                seeded_rule("three", &[], false),
            ],
        ),
    ];
    for (name, rules) in &rows {
        seed_rule_set(&engine, name, rules.clone());
    }

    for (name, rules) in &rows {
        let (status, body) = set_active(&router, name).await;
        assert_eq!(
            status, 200,
            "case activate {name:?}: expected 200 from SetActiveReceiptRuleSet, got {status} \
             with {body}"
        );

        // Activated twice is the same state as activated once: the second call
        // is accepted and the reader still reports exactly this one set.
        let (status, body) = set_active(&router, name).await;
        assert_eq!(
            status, 200,
            "case re-activate {name:?}: expected 200, got {status} with {body}"
        );

        let (status, body) = describe_active(&router).await;
        assert_eq!(
            status, 200,
            "case active {name:?}: expected 200 from DescribeActiveReceiptRuleSet, got \
             {status} with {body}"
        );
        let metadata = element(&body, "Metadata").unwrap_or_default();
        assert_eq!(
            element(&metadata, "Name").as_deref(),
            Some(name.as_str()),
            "case active {name:?}: the last activated set is the active one, got {body}"
        );
        // One set is active, not a collection of them: the reader carries a
        // single `Metadata`.
        assert_eq!(
            elements(&body, "Metadata").len(),
            1,
            "case active {name:?}: exactly one set is active, got {body}"
        );

        // The metadata is the stored set's own, so it agrees with what the
        // per-set reader reports for the same set.
        let (status, by_name) = describe_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case active {name:?}: expected 200 from DescribeReceiptRuleSet, got {status} \
             with {by_name}"
        );
        let by_name_metadata = element(&by_name, "Metadata").unwrap_or_default();
        assert_eq!(
            element(&metadata, "CreatedTimestamp"),
            element(&by_name_metadata, "CreatedTimestamp"),
            "case active {name:?}: the active reader reports the set's own creation \
             timestamp, got {body}"
        );
        assert_eq!(
            element(&body, "Rules").unwrap_or_default(),
            element(&by_name, "Rules").unwrap_or_default(),
            "case active {name:?}: the active reader reports the set's {} stored rule(s), \
             got {body}",
            rules.len()
        );
        assert_eq!(
            elements(&element(&body, "Rules").unwrap_or_default(), "Name").len(),
            rules.len(),
            "case active {name:?}: expected {} rule(s) in the active set, got {body}",
            rules.len()
        );
    }

    // The pointer resolves on every read: a rule added to the active set after
    // it was activated is part of what the reader reports.
    let (last_name, last_rules) = rows.last().expect("the table has rows");
    engine
        .state::<ComAmazonawsSesSimpleemailserviceState>()
        .lock()
        .receipt_rule_sets
        .get_mut(last_name)
        .expect("the seeded set is stored")
        .rules
        .push(seeded_rule("added-later", &["late@example.com"], true));
    let (status, body) = describe_active(&router).await;
    assert_eq!(
        status, 200,
        "case rules changed under the active set: expected 200, got {status} with {body}"
    );
    assert_eq!(
        elements(&element(&body, "Rules").unwrap_or_default(), "Name").len(),
        last_rules.len() + 1,
        "case rules changed under the active set: expected {} rule(s), got {body}",
        last_rules.len() + 1
    );
}

/// Activating a set the account does not hold is refused with the modelled
/// `RuleSetDoesNotExist`, and leaves the set that was active active — whatever
/// the form of the absent name.
#[tokio::test]
async fn activating_an_absent_rule_set_is_refused_and_changes_nothing() {
    let (router, _engine) = emulator_with_engine();
    let (status, body) = create_rule_set(&router, "held-rule-set").await;
    assert_eq!(
        status, 200,
        "CreateReceiptRuleSet(held-rule-set): expected 200, got {status} with {body}"
    );
    let (status, body) = set_active(&router, "held-rule-set").await;
    assert_eq!(
        status, 200,
        "SetActiveReceiptRuleSet(held-rule-set): expected 200, got {status} with {body}"
    );

    // The rows differ in the form and size of the ABSENT name, including names
    // no account could hold at all: a name the account cannot hold is a name it
    // does not hold.
    let absent = [
        "absent".to_string(),
        "a".to_string(),
        "Absent_Rule-Set_2024".to_string(),
        "held-rule-se".to_string(),
        "held-rule-sets".to_string(),
        "n".repeat(MAX_RECEIPT_RULE_SET_NAME),
        "y".repeat(MAX_RECEIPT_RULE_SET_NAME + 1),
        "not a legal name!".to_string(),
    ];
    for name in &absent {
        let (status, body) = set_active(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
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
            "case {name:?}: expected the absent set named in the message, got {body}"
        );

        // The refusal wrote nothing: the set activated before it is still the
        // active one.
        let (status, body) = describe_active(&router).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from DescribeActiveReceiptRuleSet, got {status} \
             with {body}"
        );
        assert_eq!(
            active_name(&body).as_deref(),
            Some("held-rule-set"),
            "case {name:?}: a refused activation must leave the active set alone, got {body}"
        );
    }
}

/// An activation carrying no rule set name leaves NO set active — and leaves
/// every stored set where it was, so a later activation can name one again.
#[tokio::test]
async fn activating_no_rule_set_leaves_none_active() {
    let (router, _engine) = emulator_with_engine();
    let names = ["alpha", "beta_2", "Gamma-3"];
    for name in names {
        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case create {name:?}: expected 200, got {status} with {body}"
        );
    }

    for name in names {
        let (status, body) = set_active(&router, name).await;
        assert_eq!(
            status, 200,
            "case activate {name:?}: expected 200, got {status} with {body}"
        );

        let (status, body) = set_active_none(&router).await;
        assert_eq!(
            status, 200,
            "case disable after {name:?}: expected 200 from SetActiveReceiptRuleSet with no \
             RuleSetName, got {status} with {body}"
        );
        let (status, body) = describe_active(&router).await;
        assert_eq!(
            status, 200,
            "case disabled after {name:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            active_name(&body),
            None,
            "case disabled after {name:?}: no set is active, got {body}"
        );
        assert_eq!(
            element(&body, "Rules"),
            None,
            "case disabled after {name:?}: no rules are reported, got {body}"
        );

        // Disabling twice over is still disabled, and never an error.
        let (status, body) = set_active_none(&router).await;
        assert_eq!(
            status, 200,
            "case disable twice after {name:?}: expected 200, got {status} with {body}"
        );
        let (status, body) = describe_active(&router).await;
        assert_eq!(
            active_name(&body),
            None,
            "case disabled twice after {name:?}: no set is active, got {status} with {body}"
        );

        // Every stored set survived the disable: each is still readable, and
        // still activatable.
        for stored in names {
            let (status, body) = describe_rule_set(&router, stored).await;
            assert_eq!(
                status, 200,
                "case {stored:?} after disabling: expected 200, got {status} with {body}"
            );
        }
    }
}
