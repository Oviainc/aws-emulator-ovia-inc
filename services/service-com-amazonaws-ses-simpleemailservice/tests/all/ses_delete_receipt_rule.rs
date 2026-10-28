// SPDX-License-Identifier: CC0-1.0

//! `DeleteReceiptRule`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. Nothing is seeded into the store — each set is made with
//! `CreateReceiptRuleSet`, each rule with `CreateReceiptRule`, and every result
//! read back through `DescribeReceiptRule` and `DescribeReceiptRuleSet` — so what
//! is asserted is the RULE the operation follows over a SET of inputs rather than
//! one recorded request:
//!
//! * the rule the request names leaves the set's ordered rule list and the rules
//!   the set keeps stay in their relative order, each still reported whole;
//! * a rule name the set does not hold is already in the state the caller asked
//!   for, so the call succeeds and changes nothing — however often it is
//!   repeated;
//! * a rule set the account does not hold is refused with the modelled
//!   `RuleSetDoesNotExist`, naming the set, and nothing is removed anywhere;
//! * the set email receiving is currently served by is not special: a rule may be
//!   taken out of it, and the active-set reader stops reporting that rule.

use crate::harness::{call, element, elements, emulator, field, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    MAX_RECEIPT_RULE_NAME, MAX_RECEIPT_RULE_SET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

/// What one `Rule` member of a `CreateReceiptRule` request carries, and so what
/// the stored rule must keep reporting until it is deleted.
#[derive(Debug, Clone)]
struct RuleSpec {
    /// `ReceiptRule$Name` — the rule's name, which is what a delete names.
    name: String,
    /// `ReceiptRule$Enabled`.
    enabled: bool,
    /// `ReceiptRule$ScanEnabled`.
    scan_enabled: bool,
    /// `ReceiptRule$TlsPolicy`, one of the modelled enum's values.
    tls_policy: String,
    /// `ReceiptRule$Recipients`, the domains and addresses the rule applies to.
    recipients: Vec<String>,
}

impl RuleSpec {
    /// The `Rule.*` form fields of an awsQuery request carrying this rule.
    fn fields(&self) -> Vec<Field> {
        let mut params = vec![
            field("Rule.Name", self.name.clone()),
            field("Rule.Enabled", self.enabled.to_string()),
            field("Rule.ScanEnabled", self.scan_enabled.to_string()),
            field("Rule.TlsPolicy", self.tls_policy.clone()),
        ];
        for (index, recipient) in self.recipients.iter().enumerate() {
            params.push(field(
                format!("Rule.Recipients.member.{}", index + 1),
                recipient.clone(),
            ));
        }
        params
    }
}

/// The rule names a case builds its set from, taken in order from a catalog that
/// spans the form `ReceiptRule$Name` documents: ASCII letters, numbers,
/// underscores, dashes or periods, starting and ending with a letter or number,
/// 64 characters or fewer. The longest name is the published bound itself.
fn rule_catalog() -> Vec<RuleSpec> {
    let names = vec![
        "a".to_string(),
        "9".to_string(),
        "inbound-rule_2024".to_string(),
        "spam.filter.v2".to_string(),
        format!("{}z", "r".repeat(MAX_RECEIPT_RULE_NAME - 1)),
        "Mixed-Case.Rule_7".to_string(),
    ];
    names
        .into_iter()
        .enumerate()
        .map(|(index, name)| RuleSpec {
            name,
            enabled: index % 2 == 0,
            scan_enabled: index % 3 == 0,
            tls_policy: if index % 2 == 0 { "Optional" } else { "Require" }.to_string(),
            recipients: ["sales@example.com", "example.net", "support@example.org"]
                [..index.min(3)]
                .iter()
                .map(|at| (*at).to_string())
                .collect(),
        })
        .collect()
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "CreateReceiptRuleSet", &[field("RuleSetName", name)]).await
}

/// `CreateReceiptRule`, placing `rule` straight after `after` (and at the
/// beginning of the rule list when `after` is `None`).
async fn create_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    after: Option<&str>,
    rule: &RuleSpec,
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", rule_set_name)];
    if let Some(after) = after {
        params.push(field("After", after));
    }
    params.extend(rule.fields());
    call(router, "CreateReceiptRule", &params).await
}

/// `DeleteReceiptRule`.
async fn delete_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    rule_name: &str,
) -> (u16, String) {
    call(
        router,
        "DeleteReceiptRule",
        &[
            field("RuleSetName", rule_set_name),
            field("RuleName", rule_name),
        ],
    )
    .await
}

/// `DescribeReceiptRule`.
async fn describe_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    rule_name: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeReceiptRule",
        &[
            field("RuleSetName", rule_set_name),
            field("RuleName", rule_name),
        ],
    )
    .await
}

/// `DescribeReceiptRuleSet`.
async fn describe_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "DescribeReceiptRuleSet", &[field("RuleSetName", name)]).await
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// The names of the rules a `DescribeReceiptRuleSet` (or
/// `DescribeActiveReceiptRuleSet`) body reports, in the order the set holds them.
///
/// `<Name>` is the rule's own member, and no recipient of a rule carries an
/// element of that name, so reading them in document order reads the rule list's
/// order.
fn reported_names(body: &str) -> Vec<String> {
    elements(&element(body, "Rules").unwrap_or_default(), "Name")
}

/// Assert that `rule`, the XML of one reported receipt rule, still carries every
/// member `spec` asked for.
fn assert_reports(case: &str, rule: &str, spec: &RuleSpec) {
    assert_eq!(
        element(rule, "Name").as_deref(),
        Some(spec.name.as_str()),
        "case {case}: expected the rule named {:?}, got {rule}",
        spec.name
    );
    assert_eq!(
        element(rule, "Enabled").as_deref(),
        Some(spec.enabled.to_string().as_str()),
        "case {case}: expected Enabled={}, got {rule}",
        spec.enabled
    );
    assert_eq!(
        element(rule, "ScanEnabled").as_deref(),
        Some(spec.scan_enabled.to_string().as_str()),
        "case {case}: expected ScanEnabled={}, got {rule}",
        spec.scan_enabled
    );
    assert_eq!(
        element(rule, "TlsPolicy").as_deref(),
        Some(spec.tls_policy.as_str()),
        "case {case}: expected TlsPolicy={:?}, got {rule}",
        spec.tls_policy
    );
    let recipients = element(rule, "Recipients")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    assert_eq!(
        recipients, spec.recipients,
        "case {case}: expected the recipients {:?}, got {rule}",
        spec.recipients
    );
}

/// Build the set `rule_set_name` holding the first `count` rules of the catalog,
/// in catalog order.
async fn seed_set(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    count: usize,
) -> Vec<RuleSpec> {
    let (status, body) = create_rule_set(router, rule_set_name).await;
    assert_eq!(
        status, 200,
        "expected 200 creating {rule_set_name:?}, got {status} with {body}"
    );
    let rules: Vec<RuleSpec> = rule_catalog().into_iter().take(count).collect();
    let mut previous: Option<String> = None;
    for rule in &rules {
        let (status, body) = create_rule(router, rule_set_name, previous.as_deref(), rule).await;
        assert_eq!(
            status, 200,
            "expected 200 creating the rule {:?}, got {status} with {body}",
            rule.name
        );
        previous = Some(rule.name.clone());
    }
    let held = reported_names(&describe_rule_set(router, rule_set_name).await.1);
    let expected: Vec<String> = rules.iter().map(|rule| rule.name.clone()).collect();
    assert_eq!(
        held, expected,
        "seeding {rule_set_name:?}: expected the rules {expected:?} in order, got {held:?}"
    );
    rules
}

/// The named rule leaves the set and the rules the set keeps stay in their
/// relative order, each still reported whole. The rows differ along how many rules
/// the set held and which position in the evaluation order is deleted — the first,
/// a middle one, the last, and the only one — and along the form and length of the
/// deleted rule's name.
#[tokio::test]
async fn a_deleted_rule_leaves_the_set_and_the_rest_keep_their_order() {
    let rows: Vec<(usize, usize)> = vec![
        (1, 0),
        (2, 0),
        (2, 1),
        (3, 1),
        (4, 0),
        (4, 2),
        (6, 3),
        (6, 5),
    ];
    for (count, at) in rows {
        let router = emulator();
        let rules = seed_set(&router, "inbound-rules", count).await;
        let deleted = rules[at].clone();
        let case = format!("{count} rule(s), deleting {:?} at position {at}", deleted.name);

        let (status, body) = delete_rule(&router, "inbound-rules", &deleted.name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DeleteReceiptRule, got {status} with {body}"
        );

        // The rule is gone: its own reader refuses the name, with the modelled
        // refusal naming it.
        let (status, body) = describe_rule(&router, "inbound-rules", &deleted.name).await;
        assert_eq!(
            status, 400,
            "case {case}: a deleted rule must not be readable, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleDoesNotExist"),
            "case {case}: expected the modelled RuleDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule does not exist: {}", deleted.name).as_str()),
            "case {case}: expected the deleted rule named in the message, got {body}"
        );

        // And the rules the set keeps are still there, in the same relative
        // order, each reporting every member it was created with.
        let kept: Vec<RuleSpec> = rules
            .iter()
            .filter(|rule| rule.name != deleted.name)
            .cloned()
            .collect();
        let expected: Vec<String> = kept.iter().map(|rule| rule.name.clone()).collect();
        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        let held = reported_names(&body);
        assert_eq!(
            held, expected,
            "case {case}: expected the rules {expected:?} left in order, got {held:?}"
        );
        for rule in &kept {
            let (status, body) = describe_rule(&router, "inbound-rules", &rule.name).await;
            assert_eq!(
                status, 200,
                "case {case}: the kept rule {:?} must still be readable, got {status} \
                 with {body}",
                rule.name
            );
            assert_reports(&case, &element(&body, "Rule").unwrap_or_default(), rule);
        }

        // The delete is the state the caller asked for, so repeating it is the
        // same answer and changes nothing further.
        let (status, body) = delete_rule(&router, "inbound-rules", &deleted.name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 repeating the delete, got {status} with {body}"
        );
        let held = reported_names(&describe_rule_set(&router, "inbound-rules").await.1);
        assert_eq!(
            held, expected,
            "case {case}: the repeated delete must leave {expected:?} alone, got {held:?}"
        );

        // The deleted name is free again: re-creating it puts the rule back at
        // the position the request asks for rather than the one it used to hold.
        let (status, body) = create_rule(&router, "inbound-rules", None, &deleted).await;
        assert_eq!(
            status, 200,
            "case {case}: the deleted name must be free again, got {status} with {body}"
        );
        let held = reported_names(&describe_rule_set(&router, "inbound-rules").await.1);
        let mut recreated = vec![deleted.name.clone()];
        recreated.extend(expected.iter().cloned());
        assert_eq!(
            held, recreated,
            "case {case}: expected {recreated:?} after re-creating the rule, got {held:?}"
        );
    }
}

/// A rule name the set does not hold is already in the state the caller asked
/// for: the call succeeds, removes nothing and leaves the set exactly as it was —
/// whatever the form of the absent name, and however often it is repeated. The
/// rows include names one character either side of a name the set does hold.
#[tokio::test]
async fn deleting_a_rule_the_set_does_not_hold_changes_nothing() {
    let router = emulator();
    let rules = seed_set(&router, "inbound-rules", 4).await;
    let before: Vec<String> = rules.iter().map(|rule| rule.name.clone()).collect();
    let held = rules[2].name.clone();

    let absent: Vec<String> = vec![
        "absent".to_string(),
        "b".to_string(),
        "0".to_string(),
        "Absent.Rule_2024-x".to_string(),
        held[..held.len() - 1].to_string(),
        format!("{held}s"),
        "n".repeat(MAX_RECEIPT_RULE_NAME),
    ];
    for name in &absent {
        for attempt in 1..=2 {
            let (status, body) = delete_rule(&router, "inbound-rules", name).await;
            assert_eq!(
                status, 200,
                "case {name:?} attempt {attempt}: expected 200 deleting a rule the set does \
                 not hold, got {status} with {body}"
            );
        }

        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        let reported = reported_names(&body);
        assert_eq!(
            reported, before,
            "case {name:?}: the delete must leave the rules {before:?} alone, got {reported:?}"
        );
    }
    for rule in &rules {
        let (status, body) = describe_rule(&router, "inbound-rules", &rule.name).await;
        assert_eq!(
            status, 200,
            "case every absent name: the rule {:?} must still be readable, got {status} \
             with {body}",
            rule.name
        );
        assert_reports(
            "every absent name",
            &element(&body, "Rule").unwrap_or_default(),
            rule,
        );
    }
}

/// A rule of a rule set the account does not hold is no rule of this account: the
/// delete is refused with the modelled `RuleSetDoesNotExist` naming the set, and
/// the set the account does hold keeps every rule. The rows differ along the form
/// and length of the absent set name — including the name of a set that was
/// deleted — and along whether the rule it names is one another set holds.
#[tokio::test]
async fn deleting_a_rule_of_a_rule_set_the_account_does_not_hold_is_refused() {
    let router = emulator();
    let rules = seed_set(&router, "inbound-rules", 3).await;
    let before: Vec<String> = rules.iter().map(|rule| rule.name.clone()).collect();

    // A set that EXISTED and was deleted is a name the account does not hold, so
    // it is refused the same way as a name that never existed.
    let (status, body) = create_rule_set(&router, "retired-rules").await;
    assert_eq!(
        status, 200,
        "expected 200 creating the set to retire, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "DeleteReceiptRuleSet",
        &[field("RuleSetName", "retired-rules")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 retiring the set, got {status} with {body}"
    );

    let rows: Vec<(String, String)> = vec![
        ("absent-set".to_string(), "some-rule".to_string()),
        // The rule name is one the OTHER set holds: the set is what is resolved
        // first, so the refusal is about the set either way.
        ("absent-set".to_string(), before[0].clone()),
        ("inbound-rule".to_string(), before[1].clone()),
        ("inbound-ruless".to_string(), before[1].clone()),
        ("retired-rules".to_string(), before[2].clone()),
        ("A".to_string(), "a".to_string()),
        (
            "x".repeat(MAX_RECEIPT_RULE_SET_NAME),
            "rule".to_string(),
        ),
    ];
    for (rule_set_name, rule_name) in &rows {
        let case = format!("set {rule_set_name:?}, rule {rule_name:?}");
        let (status, body) = delete_rule(&router, rule_set_name, rule_name).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a set the account does not hold, got {status} \
             with {body}"
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
            "case {case}: expected the absent set named in the message, got {body}"
        );

        let reported = reported_names(&describe_rule_set(&router, "inbound-rules").await.1);
        assert_eq!(
            reported, before,
            "case {case}: the refusal must remove nothing, expected {before:?}, got {reported:?}"
        );
    }
}

/// The set email receiving is currently served by is not special here: a rule may
/// be taken out of the ACTIVE set, and the active-set reader stops reporting it
/// while keeping the rest in order. The rows differ along which position of the
/// active set is deleted.
#[tokio::test]
async fn a_rule_of_the_active_rule_set_is_deletable() {
    for at in [0usize, 1, 3] {
        let router = emulator();
        let rules = seed_set(&router, "inbound-rules", 4).await;
        let (status, body) = call(
            &router,
            "SetActiveReceiptRuleSet",
            &[field("RuleSetName", "inbound-rules")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case position {at}: expected 200 from SetActiveReceiptRuleSet, got {status} \
             with {body}"
        );

        let deleted = rules[at].clone();
        let (status, body) = delete_rule(&router, "inbound-rules", &deleted.name).await;
        assert_eq!(
            status, 200,
            "case position {at}: expected 200 deleting {:?} from the active set, got {status} \
             with {body}",
            deleted.name
        );

        let expected: Vec<String> = rules
            .iter()
            .filter(|rule| rule.name != deleted.name)
            .map(|rule| rule.name.clone())
            .collect();
        let (status, body) = call(&router, "DescribeActiveReceiptRuleSet", &[]).await;
        assert_eq!(
            status, 200,
            "case position {at}: expected 200 from DescribeActiveReceiptRuleSet, got {status} \
             with {body}"
        );
        let reported = reported_names(&body);
        assert_eq!(
            reported, expected,
            "case position {at}: expected the active set left holding {expected:?}, \
             got {reported:?}"
        );
    }
}
