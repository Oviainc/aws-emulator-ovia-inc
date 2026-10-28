// SPDX-License-Identifier: CC0-1.0

//! `SetReceiptRulePosition`, end to end over the wire.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request against one shared engine. Nothing is seeded into the store:
//! each set is made with `CreateReceiptRuleSet`, filled with `CreateReceiptRule`
//! and read back through `DescribeReceiptRuleSet`, so what is asserted is the
//! RULE the operation follows over a SET of inputs rather than one recorded
//! request:
//!
//! * the named rule is moved to the position directly after the rule `After`
//!   names, and to the first position when the request names no `After`, with
//!   every other rule keeping its relative order and the set keeping its size;
//! * the rule travels WHOLE: everything but its position is unchanged;
//! * a `RuleName` the set does not hold, and an `After` the set does not hold,
//!   are each refused with the modelled `RuleDoesNotExist` naming that value;
//! * a rule set the account does not hold is refused with the modelled
//!   `RuleSetDoesNotExist`;
//! * every refusal leaves the set's order exactly as it was.

use crate::harness::{call, element, elements, emulator, error_envelope, field, Field};
use service_com_amazonaws_ses_simpleemailservice::state::MAX_RECEIPT_RULE_NAME;
use smithy_server_runtime::MultiServiceRouter;

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "CreateReceiptRuleSet", &[field("RuleSetName", name)]).await
}

/// `CreateReceiptRule`, placing a rule named `name` after `after` (at the
/// beginning of the rule list when `after` is `None`).
///
/// The rule carries a recipient derived from its name, so a later read can tell
/// that a move carried the rule WHOLE rather than rebuilding it from its name.
async fn create_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    after: Option<&str>,
    name: &str,
) -> (u16, String) {
    let mut params: Vec<Field> = vec![
        field("RuleSetName", rule_set_name),
        field("Rule.Name", name),
        field("Rule.Enabled", "true"),
        field("Rule.ScanEnabled", "false"),
        field("Rule.TlsPolicy", "Require"),
        field("Rule.Recipients.member.1", recipient_of(name)),
    ];
    if let Some(after) = after {
        params.push(field("After", after));
    }
    call(router, "CreateReceiptRule", &params).await
}

/// The recipient the rule named `name` is seeded with.
fn recipient_of(name: &str) -> String {
    format!("{name}@example.com")
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

/// `SetReceiptRulePosition`, moving `rule_name` to the position directly after
/// `after` (to the first position when `after` is `None`).
async fn set_position(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    rule_name: &str,
    after: Option<&str>,
) -> (u16, String) {
    let mut params = vec![
        field("RuleSetName", rule_set_name),
        field("RuleName", rule_name),
    ];
    if let Some(after) = after {
        params.push(field("After", after));
    }
    call(router, "SetReceiptRulePosition", &params).await
}

/// The names of the rules a `DescribeReceiptRuleSet` body reports, in the order
/// the set holds them. `<Name>` is the rule's own member, so reading the names
/// in document order reads the rule list's order.
fn reported_names(body: &str) -> Vec<String> {
    elements(&element(body, "Rules").unwrap_or_default(), "Name")
}

/// The one recipient each reported rule carries, in the order the rules are
/// reported.
///
/// Every rule here is seeded with exactly one recipient ([`create_rule`]), so
/// each `<Recipients>` element of the reported list holds one `<member>`, and
/// reading them in document order reads one value per rule in the order the set
/// holds them.
fn reported_recipients(body: &str) -> Vec<String> {
    elements(&element(body, "Rules").unwrap_or_default(), "Recipients")
        .iter()
        .map(|list| element(list, "member").unwrap_or_default())
        .collect()
}

/// The order a set holds its rules in, read back over the wire.
async fn held_order(router: &MultiServiceRouter, rule_set_name: &str) -> Vec<String> {
    let (status, body) = describe_rule_set(router, rule_set_name).await;
    assert_eq!(
        status, 200,
        "reading {rule_set_name:?} back: expected 200, got {status} with {body}"
    );
    reported_names(&body)
}

/// Make the set `rule_set_name` holding `rules` in exactly that order, each rule
/// appended after the one before it.
async fn seed(router: &MultiServiceRouter, rule_set_name: &str, rules: &[String]) {
    let (status, body) = create_rule_set(router, rule_set_name).await;
    assert_eq!(
        status, 200,
        "seeding {rule_set_name:?}: expected 200 from CreateReceiptRuleSet, got {status} with {body}"
    );
    let mut previous: Option<String> = None;
    for name in rules {
        let (status, body) = create_rule(router, rule_set_name, previous.as_deref(), name).await;
        assert_eq!(
            status, 200,
            "seeding rule {name:?} of {rule_set_name:?}: expected 200, got {status} with {body}"
        );
        previous = Some(name.clone());
    }
    assert_eq!(
        held_order(router, rule_set_name).await,
        rules,
        "seeding {rule_set_name:?}: the seeded order must be the one asked for"
    );
}

/// `count` rule names, `rule-1` … `rule-<count>`.
fn names(count: usize) -> Vec<String> {
    (1..=count).map(|index| format!("rule-{index}")).collect()
}

/// `names` as owned strings, for comparing against a reported order.
fn owned(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

/// An accepted request makes the moved rule sit directly after the rule `After`
/// names — first in the order when it names none — and leaves every other rule's
/// relative order, and the set's size, alone.
#[tokio::test]
async fn a_rule_lands_directly_after_its_anchor() {
    let router = emulator();

    // The rows differ along what the rule generalises over: how many rules the
    // set holds (one, two, three, several), where the moved rule starts (first,
    // middle, last), where its anchor sits (before it, after it, directly
    // before it, the rule itself), and whether an anchor was named at all. The
    // expected order is written out rather than computed, so the assertion does
    // not restate the handler's own arithmetic.
    let rows: [(usize, &str, Option<&str>, Vec<&str>); 13] = [
        // One rule: the only order there is, with and without an anchor.
        (1, "rule-1", None, vec!["rule-1"]),
        (1, "rule-1", Some("rule-1"), vec!["rule-1"]),
        // Two rules: a swap in each direction.
        (2, "rule-1", Some("rule-2"), vec!["rule-2", "rule-1"]),
        (2, "rule-2", None, vec!["rule-2", "rule-1"]),
        // Three rules: the anchor after the moved rule, before it, and the
        // front of the list.
        (
            3,
            "rule-1",
            Some("rule-3"),
            vec!["rule-2", "rule-3", "rule-1"],
        ),
        (
            3,
            "rule-3",
            Some("rule-1"),
            vec!["rule-1", "rule-3", "rule-2"],
        ),
        (
            3,
            "rule-2",
            Some("rule-3"),
            vec!["rule-1", "rule-3", "rule-2"],
        ),
        (
            3,
            "rule-3",
            None,
            vec!["rule-3", "rule-1", "rule-2"],
        ),
        // A rule asked for the position it already occupies: already first and
        // asked to be first, anchored to the rule directly before it, and
        // anchored to itself. Each one is accepted and changes nothing.
        (3, "rule-1", None, vec!["rule-1", "rule-2", "rule-3"]),
        (
            3,
            "rule-2",
            Some("rule-1"),
            vec!["rule-1", "rule-2", "rule-3"],
        ),
        (
            4,
            "rule-3",
            Some("rule-3"),
            vec!["rule-1", "rule-2", "rule-3", "rule-4"],
        ),
        // Longer sets: a rule moved from the end towards the front and one from
        // the front towards the end.
        (
            5,
            "rule-5",
            Some("rule-1"),
            vec!["rule-1", "rule-5", "rule-2", "rule-3", "rule-4"],
        ),
        (
            5,
            "rule-2",
            Some("rule-5"),
            vec!["rule-1", "rule-3", "rule-4", "rule-5", "rule-2"],
        ),
    ];
    for (index, (count, rule_name, after, expected)) in rows.into_iter().enumerate() {
        let case = format!("{count} rule(s), move {rule_name} after {after:?}");
        let rule_set_name = format!("set-{index}");
        let seeded = names(count);
        seed(&router, &rule_set_name, &seeded).await;

        let (status, body) = set_position(&router, &rule_set_name, rule_name, after).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from SetReceiptRulePosition, got {status} with {body}"
        );

        let (status, body) = describe_rule_set(&router, &rule_set_name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        let reported = reported_names(&body);
        assert_eq!(
            reported,
            owned(&expected),
            "case {case}: expected the order {expected:?}, got {reported:?} in {body}"
        );
        // The rule moved WHOLE: the recipient every rule was seeded with
        // travels with it, so the recipients follow the new order rather than
        // staying with the positions.
        let recipients = reported_recipients(&body);
        assert_eq!(
            recipients,
            expected
                .iter()
                .map(|name| recipient_of(name))
                .collect::<Vec<_>>(),
            "case {case}: expected each rule's recipient to move with it, got {recipients:?} in {body}"
        );
    }
}

/// Setting a position is not a one-shot: each accepted request moves the rule it
/// names within the order the one before it left.
#[tokio::test]
async fn positions_apply_one_after_another() {
    let router = emulator();
    let seeded = names(4);
    seed(&router, "repeated", &seeded).await;

    // Each row is (rule moved, anchor, the order that leaves), applied in
    // sequence to the SAME set.
    let steps: [(&str, Option<&str>, Vec<&str>); 4] = [
        (
            "rule-4",
            None,
            vec!["rule-4", "rule-1", "rule-2", "rule-3"],
        ),
        (
            "rule-1",
            Some("rule-3"),
            vec!["rule-4", "rule-2", "rule-3", "rule-1"],
        ),
        (
            "rule-2",
            Some("rule-1"),
            vec!["rule-4", "rule-3", "rule-1", "rule-2"],
        ),
        (
            "rule-4",
            Some("rule-2"),
            vec!["rule-3", "rule-1", "rule-2", "rule-4"],
        ),
    ];
    for (rule_name, after, expected) in steps {
        let case = format!("move {rule_name} after {after:?}");
        let (status, body) = set_position(&router, "repeated", rule_name, after).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let held = held_order(&router, "repeated").await;
        assert_eq!(
            held,
            owned(&expected),
            "case {case}: expected {expected:?}, got {held:?}"
        );
    }
}

/// A `RuleName` the set does not hold names no rule to reposition, so the
/// request is refused with the modelled `RuleDoesNotExist` naming it, and the
/// set keeps the order it had.
#[tokio::test]
async fn moving_a_rule_the_set_does_not_hold_is_refused() {
    let router = emulator();
    let seeded = names(3);
    seed(&router, "unknown-rule", &seeded).await;
    // A rule of ANOTHER set is still a rule this set does not hold.
    seed(&router, "other-set", &["only-in-other-set".to_string()]).await;

    // The rows differ in the form and size of the absent name: a plain one, one
    // differing from a held rule only in case, one carrying a period (legal in a
    // rule name), a rule of another set, the longest name `ReceiptRule$Name`
    // documents, and one that is not even a legal rule name — a name SES cannot
    // hold is a name the set does not hold.
    let absent = [
        "absent-rule".to_string(),
        "RULE-1".to_string(),
        "rule.1".to_string(),
        "only-in-other-set".to_string(),
        "n".repeat(MAX_RECEIPT_RULE_NAME),
        "not a legal name!".to_string(),
    ];
    for name in &absent {
        // The anchor is a rule the set DOES hold, so the absent `RuleName` is
        // the only defect in the request.
        for after in [None, Some("rule-2")] {
            let case = format!("absent rule {name:?} after {after:?}");
            let (status, body) = set_position(&router, "unknown-rule", name, after).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {body}"
            );
            let (code, message) = error_envelope(&body);
            assert_eq!(
                code.as_deref(),
                Some("RuleDoesNotExist"),
                "case {case}: expected the modelled RuleDoesNotExist, got {body}"
            );
            assert_eq!(
                message.as_deref(),
                Some(format!("Rule does not exist: {name}").as_str()),
                "case {case}: expected the absent rule named in the message, got {body}"
            );
            let held = held_order(&router, "unknown-rule").await;
            assert_eq!(
                held, seeded,
                "case {case}: a refused request must leave the order alone, got {held:?}"
            );
        }
    }
}

/// An `After` the set does not hold anchors the rule to a rule that does not
/// exist, so the request is refused with the modelled `RuleDoesNotExist` naming
/// that value, and the set keeps the order it had.
#[tokio::test]
async fn anchoring_to_a_rule_the_set_does_not_hold_is_refused() {
    let router = emulator();
    let seeded = names(3);
    seed(&router, "unknown-anchor", &seeded).await;
    seed(&router, "anchor-elsewhere", &["only-in-other-set".to_string()]).await;

    // Same class of absent names as the moved rule's, for the same reason: the
    // anchor is read out of the same set.
    let absent = [
        "absent-rule".to_string(),
        "RULE-2".to_string(),
        "rule.2".to_string(),
        "only-in-other-set".to_string(),
        "a".repeat(MAX_RECEIPT_RULE_NAME),
        "not a legal name!".to_string(),
    ];
    for after in &absent {
        // The moved rule is one the set DOES hold, so the absent anchor is the
        // only defect in the request; it is varied over the set's positions so
        // the refusal does not depend on where the rule sits.
        for rule_name in &seeded {
            let case = format!("move {rule_name} after absent {after:?}");
            let (status, body) =
                set_position(&router, "unknown-anchor", rule_name, Some(after)).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} with {body}"
            );
            let (code, message) = error_envelope(&body);
            assert_eq!(
                code.as_deref(),
                Some("RuleDoesNotExist"),
                "case {case}: expected the modelled RuleDoesNotExist, got {body}"
            );
            assert_eq!(
                message.as_deref(),
                Some(format!("Rule does not exist: {after}").as_str()),
                "case {case}: expected the absent anchor named in the message, got {body}"
            );
            let held = held_order(&router, "unknown-anchor").await;
            assert_eq!(
                held, seeded,
                "case {case}: a refused request must leave the order alone, got {held:?}"
            );
        }
    }
}

/// A rule set the account does not hold is refused with the modelled
/// `RuleSetDoesNotExist` naming it, and no set is created for it.
#[tokio::test]
async fn setting_a_position_in_a_set_that_does_not_exist_is_refused() {
    let router = emulator();
    let seeded = names(2);
    seed(&router, "present-set", &seeded).await;

    // The rows differ in the form and size of the absent set name, including one
    // that differs from the present set only in case, one carrying a period —
    // which a rule set name, unlike a rule name, may not — and one that is not
    // even a legal rule set name.
    let absent = [
        "absent-set".to_string(),
        "a".to_string(),
        "PRESENT-SET".to_string(),
        "present.set".to_string(),
        "x".repeat(MAX_RECEIPT_RULE_NAME),
        "not a legal name!".to_string(),
    ];
    for rule_set_name in &absent {
        let case = format!("absent rule set {rule_set_name:?}");
        let (status, body) = set_position(&router, rule_set_name, "rule-1", Some("rule-2")).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {case}: expected the modelled RuleSetDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set does not exist: {rule_set_name}").as_str()),
            "case {case}: expected the absent rule set named in the message, got {body}"
        );

        // The refused request made no set under that name, and left the present
        // set's order alone.
        let (status, body) = describe_rule_set(&router, rule_set_name).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused request must make no rule set, got {status} with {body}"
        );
        let held = held_order(&router, "present-set").await;
        assert_eq!(
            held, seeded,
            "case {case}: a refused request must leave the present set's order alone, got {held:?}"
        );
    }
}
