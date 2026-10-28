// SPDX-License-Identifier: CC0-1.0

//! `ReorderReceiptRuleSet`, end to end over the wire.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so each one is a real
//! awsQuery request against one shared engine. Nothing is seeded into the store:
//! each set is made with `CreateReceiptRuleSet`, filled with `CreateReceiptRule`
//! and read back through `DescribeReceiptRuleSet`, so what is asserted is the
//! RULE the operation follows over a SET of inputs rather than one recorded
//! request:
//!
//! * a request that positions every rule the set holds exactly once makes the
//!   order it names the set's evaluation order, whatever the size of the set and
//!   whatever the permutation, and changes nothing else about any rule;
//! * a rule named more than once is given more than one position, so the request
//!   is refused (`InvalidParameterValue`);
//! * a name the set does not hold positions a rule that does not exist, so the
//!   request is refused with the modelled `RuleDoesNotExist`;
//! * a rule of the set the request leaves out is left unpositioned, so the
//!   request is refused (`InvalidParameterValue`);
//! * a rule set the account does not hold is refused with the modelled
//!   `RuleSetDoesNotExist`;
//! * every refusal leaves the set's order exactly as it was.

use crate::harness::{call, element, elements, emulator, field, xml_text, Field};
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
/// that a reorder moved the rule WHOLE rather than rebuilding it from its name.
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

/// `ReorderReceiptRuleSet`, positioning the rules in the order `rule_names`
/// names them.
async fn reorder(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    rule_names: &[String],
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", rule_set_name)];
    for (index, name) in rule_names.iter().enumerate() {
        params.push(field(
            format!("RuleNames.member.{}", index + 1),
            name.clone(),
        ));
    }
    call(router, "ReorderReceiptRuleSet", &params).await
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope, with
/// the message's XML escaping undone.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code"),
        element(body, "Message").map(|message| xml_text(&message)),
    )
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

/// `order` with the entries at `positions` taken first, in that order, and the
/// rest following — a permutation written as the positions it moves to the
/// front.
fn permuted(order: &[String], positions: &[usize]) -> Vec<String> {
    let mut permutation: Vec<String> = positions
        .iter()
        .map(|at| {
            order
                .get(*at)
                .unwrap_or_else(|| panic!("position {at} is within the seeded order"))
                .clone()
        })
        .collect();
    let rest: Vec<String> = order
        .iter()
        .filter(|name| !permutation.contains(name))
        .cloned()
        .collect();
    permutation.extend(rest);
    permutation
}

/// A request positioning every rule of the set exactly once makes the order it
/// names the set's order, and changes nothing else about any rule.
#[tokio::test]
async fn a_reorder_makes_the_requested_order_the_held_order() {
    let router = emulator();

    // The rows differ along what the rule generalises over: how many rules the
    // set holds (one, two, three, several), and which permutation is asked for —
    // the order the set already has, a swap of the first two, a reversal, a
    // rotation, and one that moves a rule from the end to the front.
    let rows: [(usize, Vec<usize>); 8] = [
        (1, vec![0]),
        (2, vec![0, 1]),
        (2, vec![1, 0]),
        (3, vec![2, 1, 0]),
        (3, vec![1, 2, 0]),
        (5, vec![4, 0, 3, 1, 2]),
        (5, vec![0, 1, 2, 3, 4]),
        (7, vec![6, 5, 4, 3, 2, 1, 0]),
    ];
    for (count, positions) in rows {
        let case = format!("{count} rule(s), permutation {positions:?}");
        let rule_set_name = format!("set-{count}-{}", positions[0]);
        let seeded = names(count);
        seed(&router, &rule_set_name, &seeded).await;
        let wanted = permuted(&seeded, &positions);

        let (status, body) = reorder(&router, &rule_set_name, &wanted).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from ReorderReceiptRuleSet, got {status} with {body}"
        );

        let (status, body) = describe_rule_set(&router, &rule_set_name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert_eq!(
            reported_names(&body),
            wanted,
            "case {case}: expected the requested order {wanted:?}, got {body}"
        );
        // Each rule moved WHOLE: the recipient every rule was seeded with
        // travels with it, so the recipients follow the new order rather than
        // staying with the positions.
        assert_eq!(
            reported_recipients(&body),
            wanted
                .iter()
                .map(|name| recipient_of(name))
                .collect::<Vec<_>>(),
            "case {case}: expected each rule's recipient to move with it, got {body}"
        );
    }
}

/// A reorder is not a one-shot: the order the set holds is the order the LAST
/// accepted request named.
#[tokio::test]
async fn reorders_apply_one_after_another() {
    let router = emulator();
    let seeded = names(4);
    seed(&router, "repeated", &seeded).await;

    let requests = [
        vec![
            "rule-4".to_string(),
            "rule-3".to_string(),
            "rule-2".to_string(),
            "rule-1".to_string(),
        ],
        vec![
            "rule-2".to_string(),
            "rule-4".to_string(),
            "rule-1".to_string(),
            "rule-3".to_string(),
        ],
        seeded.clone(),
    ];
    for wanted in &requests {
        let (status, body) = reorder(&router, "repeated", wanted).await;
        assert_eq!(
            status, 200,
            "case {wanted:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            held_order(&router, "repeated").await,
            *wanted,
            "case {wanted:?}: expected the order the last request named"
        );
    }
}

/// A rule named more than once is given more than one position, so the request
/// is refused with `InvalidParameterValue` naming that rule, and the set keeps
/// the order it had.
#[tokio::test]
async fn a_rule_positioned_twice_is_refused() {
    let router = emulator();
    let seeded = names(3);
    seed(&router, "duplicates", &seeded).await;

    // The rows differ in WHICH rule is repeated (first, middle, last), how many
    // times it is repeated, and whether the repeated request names the other
    // rules at all.
    let rows: [(Vec<&str>, &str); 5] = [
        (vec!["rule-1", "rule-1", "rule-2", "rule-3"], "rule-1"),
        (vec!["rule-1", "rule-2", "rule-2", "rule-3"], "rule-2"),
        (vec!["rule-3", "rule-2", "rule-1", "rule-3"], "rule-3"),
        (vec!["rule-2", "rule-2", "rule-2"], "rule-2"),
        (vec!["rule-1", "rule-1"], "rule-1"),
    ];
    for (request, repeated) in rows {
        let case = format!("{request:?}");
        let request: Vec<String> = request.iter().map(|name| name.to_string()).collect();
        let (status, body) = reorder(&router, "duplicates", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidParameterValue"),
            "case {case}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Multiple positions found for rule: {repeated}").as_str()),
            "case {case}: expected the repeated rule named in the message, got {body}"
        );
        assert_eq!(
            held_order(&router, "duplicates").await,
            seeded,
            "case {case}: a refused reorder must leave the order alone"
        );
    }
}

/// A name the set does not hold positions a rule that does not exist, so the
/// request is refused with the modelled `RuleDoesNotExist` naming it, and the
/// set keeps the order it had.
#[tokio::test]
async fn positioning_a_rule_the_set_does_not_hold_is_refused() {
    let router = emulator();
    let seeded = names(2);
    seed(&router, "unknown-names", &seeded).await;
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
        let case = format!("absent rule {name:?}");
        // The request still positions every rule the set holds, so the absent
        // name is the only defect in it.
        let mut request = seeded.clone();
        request.push(name.clone());
        let (status, body) = reorder(&router, "unknown-names", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
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
        assert_eq!(
            held_order(&router, "unknown-names").await,
            seeded,
            "case {case}: a refused reorder must leave the order alone"
        );
    }
}

/// A rule of the set the request leaves out is left unpositioned, so the request
/// is refused with `InvalidParameterValue` naming every such rule, and the set
/// keeps the order it had.
#[tokio::test]
async fn leaving_a_rule_of_the_set_unpositioned_is_refused() {
    let router = emulator();
    let seeded = names(4);
    seed(&router, "incomplete", &seeded).await;

    // The rows differ in how many rules are left out and where they sit in the
    // set: one from the end, one from the middle, two, and all but one. The
    // expected message names them in the order the SET holds them, not the order
    // the request names the rest.
    let rows: [(Vec<&str>, Vec<&str>); 5] = [
        (
            vec!["rule-1", "rule-2", "rule-3"],
            vec!["rule-4"],
        ),
        (
            vec!["rule-1", "rule-3", "rule-4"],
            vec!["rule-2"],
        ),
        (vec!["rule-4", "rule-1"], vec!["rule-2", "rule-3"]),
        (vec!["rule-2"], vec!["rule-1", "rule-3", "rule-4"]),
        (
            vec!["rule-3", "rule-2", "rule-4"],
            vec!["rule-1"],
        ),
    ];
    for (request, unpositioned) in rows {
        let case = format!("{request:?}");
        let request: Vec<String> = request.iter().map(|name| name.to_string()).collect();
        let (status, body) = reorder(&router, "incomplete", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidParameterValue"),
            "case {case}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(
                format!("Positions for rules not found: {}", unpositioned.join(", ")).as_str()
            ),
            "case {case}: expected the unpositioned rules named in the message, got {body}"
        );
        assert_eq!(
            held_order(&router, "incomplete").await,
            seeded,
            "case {case}: a refused reorder must leave the order alone"
        );
    }
}

/// A rule set the account does not hold is refused with the modelled
/// `RuleSetDoesNotExist` naming it, and no set is created for it.
#[tokio::test]
async fn reordering_a_set_that_does_not_exist_is_refused() {
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
        let (status, body) = reorder(&router, rule_set_name, &seeded).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
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

        // The refused reorder made no set under that name, and left the present
        // set's order alone.
        let (status, body) = describe_rule_set(&router, rule_set_name).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused reorder must make no rule set, got {status} with {body}"
        );
        assert_eq!(
            held_order(&router, "present-set").await,
            seeded,
            "case {case}: a refused reorder must leave the present set's order alone"
        );
    }
}
