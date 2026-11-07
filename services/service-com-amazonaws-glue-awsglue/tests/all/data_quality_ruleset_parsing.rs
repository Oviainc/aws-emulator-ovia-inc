// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DQDL a data quality ruleset write may carry:
//! `CreateDataQualityRuleset` and the `UpdateDataQualityRuleset` that replaces the stored
//! text.
//!
//! The rule under test: "a ruleset is a set of rules in DQDL", and the rules of one are the
//! entries of the `Rules = [ … ]` list its text declares — so a `Ruleset` that declares no
//! rule is refused with `InvalidInputException` (HTTP 400) and the fixed sentence the service
//! words that refusal with, storing nothing and leaving a stored ruleset exactly as it was.
//! Text declares no rule when no terminated rule list can be read out of it — no list at
//! all, a list left unterminated, a list swallowed by an unterminated string or comment, a
//! list nested past what a rule's own operands use — or when the list it declares holds no
//! entry.
//!
//! A ruleset that DOES declare rules is stored and reports that many rules, which is what
//! makes the refusals above about the rule rather than about the create having stopped
//! working.
//!
//! The rows differ along what the rule generalises over: the form of the DQDL (how the list
//! is opened, terminated, commented, quoted and nested), how many rules it declares, and
//! whether it arrives on the create or on the update. Every call goes over the wire through
//! the shared [`crate::wire`] harness, so what is asserted is HTTP behaviour and not a
//! direct call into a handler.

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The whole `Message` a ruleset write refuses DQDL declaring no rule with — written out here
/// rather than read from the crate's own constant so this suite pins the wording. No trailing
/// period, as the service words it.
const RULES_NOT_PARSED: &str = "DataQuality rules cannot be parsed";

/// The longest `Ruleset` the model accepts: `DataQualityRulesetString`, "len 1..65536"
/// (`smithy_query operation=CreateDataQualityRuleset`).
const MAX_RULESET_LENGTH: usize = 65_536;

/// One row: the case, the DQDL the write carries, and the number of rules it declares —
/// `None` for text that declares none and so is refused.
type Row<'a> = (&'a str, String, Option<i64>);

/// Every form of DQDL, with the rules it declares.
fn rows() -> Vec<Row<'static>> {
    // A rule list nested far past anything a rule's own operands need.
    let overly_nested = format!("Rules = [ {} ]", "[".repeat(64));
    // A rule list that fills the modelled length bound exactly, so the text is read whole.
    let rule = "ColumnExists \"c\",";
    let head = "Rules = [";
    let filled = (MAX_RULESET_LENGTH - head.len() - 1) / rule.len();
    let at_the_bound = format!(
        "{head}{}{}]",
        rule.repeat(filled),
        " ".repeat(MAX_RULESET_LENGTH - head.len() - 1 - filled * rule.len())
    );
    assert_eq!(
        at_the_bound.len(),
        MAX_RULESET_LENGTH,
        "the padded ruleset must sit exactly on the modelled bound, got {}",
        at_the_bound.len()
    );
    let filled = i64::try_from(filled).expect("the rule count fits an i64");

    vec![
        (
            "one rule",
            "Rules = [ ColumnExists \"order_id\" ]".to_string(),
            Some(1),
        ),
        (
            "two rules on one line",
            "Rules = [ IsComplete \"customer_id\", RowCount > 0 ]".to_string(),
            Some(2),
        ),
        (
            "rules spread over lines",
            "Rules = [\n    ColumnExists \"a\",\n    Completeness \"a\" > 0.95,\n    Uniqueness \"a\" > 0.9\n]"
                .to_string(),
            Some(3),
        ),
        (
            "a rule whose operands nest",
            "Rules = [ ColumnValues \"grade\" in [ \"a\", \"b\" ], RowCount > 0 ]".to_string(),
            Some(2),
        ),
        (
            "a rule list a comment describes",
            "# the orders table\nRules = [ ColumnExists \"a\" ]".to_string(),
            Some(1),
        ),
        ("a rule list at the modelled length bound", at_the_bound, Some(filled)),
        // Text no terminated rule list can be read out of declares nothing.
        (
            "an unterminated rule list",
            "Rules = [ ColumnExists \"a\"".to_string(),
            None,
        ),
        ("truncated after the keyword", "Rules".to_string(), None),
        ("truncated after the assignment", "Rules =".to_string(), None),
        (
            "an unterminated string swallowing the closing bracket",
            "Rules = [ ColumnExists \"a ]".to_string(),
            None,
        ),
        (
            "a comment swallowing the closing bracket",
            "Rules = [ ColumnExists \"a\" # ]".to_string(),
            None,
        ),
        ("a single blank", " ".to_string(), None),
        ("punctuation alone", "][,,,".to_string(), None),
        (
            "the keyword without a list",
            "Rules = ColumnExists \"a\"".to_string(),
            None,
        ),
        (
            "the keyword as the tail of a longer word",
            "MyRules = [ ColumnExists \"a\" ]".to_string(),
            None,
        ),
        (
            "rules nested past what a rule uses",
            overly_nested,
            None,
        ),
        // A list that declares no entry declares no rule either.
        ("an empty rule list", "Rules = []".to_string(), None),
        (
            "a rule list holding only blanks",
            "Rules = [\n\n   \n]".to_string(),
            None,
        ),
    ]
}

/// A create body for `name` carrying `ruleset`.
fn create_body(name: &str, ruleset: &str) -> Value {
    json!({ "Name": name, "Ruleset": ruleset })
}

/// The number of rules the stored ruleset `name` is listed with.
async fn listed_rule_count(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> i64 {
    let page = ok(
        router,
        "ListDataQualityRulesets",
        &json!({ "Filter": { "Name": name } }),
    )
    .await;
    let entries = page
        .get("Rulesets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report a Rulesets list, got {page}"));
    assert_eq!(
        entries.len(),
        1,
        "expected exactly the ruleset named {name}, got {page}"
    );
    entries[0]
        .get("RuleCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("every entry carries a RuleCount, got {}", entries[0]))
}

/// A create is admitted exactly for DQDL that declares at least one rule, and the stored
/// ruleset is the text the create sent, reporting the rules it declares.
#[tokio::test]
async fn a_ruleset_is_stored_only_when_its_dqdl_declares_a_rule() {
    let router = emulator();

    for (index, (case, ruleset, rules)) in rows().iter().enumerate() {
        let name = format!("parsed-{index:02}");
        let (status, body) = call(
            &router,
            "CreateDataQualityRuleset",
            &create_body(&name, ruleset),
        )
        .await;
        let Some(expected) = rules else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the create to be refused, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("InvalidInputException"),
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                RULES_NOT_PARSED,
                "case {case:?}: expected Message {RULES_NOT_PARSED:?}, got {:?}",
                error_message(&body)
            );
            // Nothing was stored: the name is still free, and a read of it is a miss.
            let (status, read) = call(
                &router,
                "GetDataQualityRuleset",
                &json!({ "Name": name.clone() }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a refused create must store no ruleset, but the read answered \
                 {status} with {read}"
            );
            assert!(
                error_code(&read).ends_with("EntityNotFoundException"),
                "case {case:?}: expected the ruleset to be absent, got {read}"
            );
            continue;
        };

        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to be admitted, got {status} with {body}"
        );
        // Round trip: the stored ruleset carries the text the create sent, and is listed
        // with the rules that text declares.
        let read = ok(
            &router,
            "GetDataQualityRuleset",
            &json!({ "Name": name.clone() }),
        )
        .await;
        assert_eq!(
            read.get("Ruleset").and_then(Value::as_str),
            Some(ruleset.as_str()),
            "case {case:?}: expected the stored ruleset to carry the DQDL the create sent, got \
             {read}"
        );
        assert_eq!(
            listed_rule_count(&router, &name).await,
            *expected,
            "case {case:?}: expected the listing to report {expected} rules"
        );
    }
}

/// An update is held to the same requirement: DQDL declaring no rule is refused and the
/// stored ruleset keeps the text it had, while DQDL declaring rules replaces it — and an
/// update naming no `Ruleset` replaces none.
#[tokio::test]
async fn an_update_replaces_the_stored_dqdl_only_when_the_new_text_declares_a_rule() {
    let router = emulator();
    let stored = "Rules = [ ColumnExists \"a\" ]";

    for (index, (case, ruleset, rules)) in rows().iter().enumerate() {
        let name = format!("updated-{index:02}");
        ok(
            &router,
            "CreateDataQualityRuleset",
            &create_body(&name, stored),
        )
        .await;

        let (status, body) = call(
            &router,
            "UpdateDataQualityRuleset",
            &json!({ "Name": name.clone(), "Ruleset": ruleset }),
        )
        .await;
        let expected_stored = if let Some(expected) = rules {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to be admitted, got {status} with {body}"
            );
            assert_eq!(
                listed_rule_count(&router, &name).await,
                *expected,
                "case {case:?}: expected the listing to report {expected} rules after the update"
            );
            ruleset.as_str()
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the update to be refused, got {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                RULES_NOT_PARSED,
                "case {case:?}: expected Message {RULES_NOT_PARSED:?}, got {:?}",
                error_message(&body)
            );
            stored
        };

        let read = ok(
            &router,
            "GetDataQualityRuleset",
            &json!({ "Name": name.clone() }),
        )
        .await;
        assert_eq!(
            read.get("Ruleset").and_then(Value::as_str),
            Some(expected_stored),
            "case {case:?}: expected the stored DQDL to be {expected_stored:?}, got {read}"
        );
    }

    // An update naming no `Ruleset` leaves the stored text alone.
    ok(
        &router,
        "CreateDataQualityRuleset",
        &create_body("untouched", stored),
    )
    .await;
    ok(
        &router,
        "UpdateDataQualityRuleset",
        &json!({ "Name": "untouched", "Description": "changed" }),
    )
    .await;
    let read = ok(
        &router,
        "GetDataQualityRuleset",
        &json!({ "Name": "untouched" }),
    )
    .await;
    assert_eq!(
        read.get("Ruleset").and_then(Value::as_str),
        Some(stored),
        "case an update naming no Ruleset: expected the stored DQDL to be left alone, got {read}"
    );
}
