// SPDX-License-Identifier: CC0-1.0

//! Every regular expression a WAFv2 save would store is parsed first, end to end
//! over the wire.
//!
//! The rule asserted here: a `RegexString` a save carries has to BE a regular
//! expression, and one the service cannot parse is refused with HTTP 400
//! `WAFInvalidParameterException` naming it, before anything is written. The
//! modelled constraints on the member bound its length only — `RegexPatternString`
//! is `@length(1..512)` with `@pattern(".*")`
//! (`smithy_query(query="shape",shape="Regex")`) — so every byte sequence of legal
//! length reaches the service and whether the text parses is the service's own
//! judgement. The refusal carries the three members the model gives that one
//! exception (`Field`, `Parameter`, `Reason`) and the message WAF composes from
//! them, with `Parameter` the offending expression as the request wrote it, and the
//! save leaves no trace: no set is listed, no rule is stored and no optimistic-lock
//! token rotates.
//!
//! The rule is over every member that carries an expression, not over the one
//! operation the defect was found on: `RegularExpressionList[].RegexString` on
//! `CreateRegexPatternSet`/`UpdateRegexPatternSet`, and
//! `RegexMatchStatement$RegexString` at any nesting depth on the rule-carrying
//! operations (`CreateWebACL`, `UpdateWebACL`, `CreateRuleGroup`,
//! `UpdateRuleGroup`, `CheckCapacity`).
//!
//! Every call here goes through the same front door a real client uses (the `Waf`
//! harness in [`crate::web_acl_lifecycle`]), so what these tests observe is what an
//! SDK observes.

use serde_json::{json, Value};

use crate::rule_group_lifecycle::{byte_match_statement, create_body, create_rule_group};
use crate::web_acl_lifecycle::{create_web_acl, rule, visibility, Waf};

/// The `Reason` WAF carries for a parameter whose text it cannot read in the form
/// that parameter has to take, as the recorded refusals of that class word it.
const FORMAT_REASON: &str = "The parameter contains formatting that is not valid.";

/// The `ParameterExceptionField` value naming a regex pattern set
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `REGEX_PATTERN_SET` among its values).
const FIELD_REGEX_PATTERN_SET: &str = "REGEX_PATTERN_SET";

/// The `ParameterExceptionField` value naming a rule statement. See
/// [`FIELD_REGEX_PATTERN_SET`].
const FIELD_STATEMENT: &str = "STATEMENT";

/// An expression every engine parses, for a positive control and for the valid
/// neighbours of an offending element.
const PARSEABLE: &str = "^[a-z]+$";

/// One expression, and whether it IS a regular expression.
struct Expression {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The expression the request submits.
    pattern: String,
    /// Whether the service must accept it.
    parseable: bool,
}

/// The table every test here drives: expressions that parse and expressions that do
/// not.
///
/// The rows differ along what the rule generalises over — which construct is left
/// open (a group, a character class, an escape), a quantifier or class range whose
/// bounds are in the wrong order, an operator with nothing to apply to, a property
/// name that names nothing, and, on the accepted side, plain text, metacharacters,
/// escapes, a POSIX class, an inline flag, non-ASCII text, and each published length
/// bound (`RegexPatternString` is `@length(1..512)`, per
/// `smithy_query(query="shape",shape="Regex")`). Which of these parse is fixed by
/// the expression syntax this service documents for a `RegexString`, which is the
/// syntax of the maintained engine this emulator matches with: an expression it
/// cannot compile is one WAF cannot run.
fn expressions() -> Vec<Expression> {
    vec![
        Expression {
            case: "an unclosed group and an unclosed character class",
            pattern: "(unterminated[".to_string(),
            parseable: false,
        },
        Expression {
            case: "an unclosed group",
            pattern: "(".to_string(),
            parseable: false,
        },
        Expression {
            case: "an unclosed character class",
            pattern: "[".to_string(),
            parseable: false,
        },
        Expression {
            case: "a group that was never opened",
            pattern: "a)".to_string(),
            parseable: false,
        },
        Expression {
            case: "a repetition operator with nothing to repeat",
            pattern: "*".to_string(),
            parseable: false,
        },
        Expression {
            case: "a counted repetition whose bounds are in the wrong order",
            pattern: "a{3,1}".to_string(),
            parseable: false,
        },
        Expression {
            case: "a character-class range whose bounds are in the wrong order",
            pattern: "[z-a]".to_string(),
            parseable: false,
        },
        Expression {
            case: "an escape with nothing to escape",
            pattern: "\\".to_string(),
            parseable: false,
        },
        Expression {
            case: "a unicode property that names nothing",
            pattern: r"\p{NotAScriptAtAll}".to_string(),
            parseable: false,
        },
        Expression {
            case: "a named group left unclosed",
            pattern: "(?P<path>/admin".to_string(),
            parseable: false,
        },
        Expression {
            case: "plain anchored text",
            pattern: PARSEABLE.to_string(),
            parseable: true,
        },
        Expression {
            case: "the unparseable row's metacharacters, escaped",
            pattern: r"\(unterminated\[".to_string(),
            parseable: true,
        },
        Expression {
            case: "a counted repetition",
            pattern: "a{1,3}".to_string(),
            parseable: true,
        },
        Expression {
            case: "a POSIX character class",
            pattern: "[[:alpha:]]+".to_string(),
            parseable: true,
        },
        Expression {
            case: "an inline flag and an alternation",
            pattern: r"(?i)admin|\d{3}-\d{4}".to_string(),
            parseable: true,
        },
        Expression {
            case: "non-ASCII text",
            pattern: "привет+".to_string(),
            parseable: true,
        },
        Expression {
            case: "a one-character expression, the modelled length floor",
            pattern: "a".to_string(),
            parseable: true,
        },
        Expression {
            case: "a 512-character expression, the modelled length ceiling",
            pattern: "z".repeat(512),
            parseable: true,
        },
    ]
}

/// `[{ "RegexString": … }, …]` — a `RegularExpressionList` over `patterns`.
fn regular_expression_list<S: AsRef<str>>(patterns: &[S]) -> Value {
    Value::Array(
        patterns
            .iter()
            .map(|pattern| json!({ "RegexString": pattern.as_ref() }))
            .collect(),
    )
}

/// The refusal must carry the three members the model gives
/// `WAFInvalidParameterException` and the message WAF composes from exactly those
/// three.
fn assert_regex_refusal(body: &Value, field: &str, pattern: &str, case: &str) {
    assert_eq!(
        (
            body["Field"].as_str(),
            body["Parameter"].as_str(),
            body["Reason"].as_str()
        ),
        (Some(field), Some(pattern), Some(FORMAT_REASON)),
        "case {case:?}: the refusal must name the settings, the offending expression \
         and the reason, got {body}"
    );
    assert_eq!(
        body["message"].as_str().unwrap_or_default(),
        format!("Error reason: {FORMAT_REASON}, field: {field}, parameter: {pattern}"),
        "case {case:?}: the refusal must read the way WAF words it, got {body}"
    );
}

/// The names one `ListRegexPatternSets` page reports.
fn listed_set_names(page: &Value) -> Vec<&str> {
    page["RegexPatternSets"]
        .as_array()
        .map(|sets| {
            sets.iter()
                .filter_map(|summary| summary["Name"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// The names one `ListWebACLs` or `ListRuleGroups` page reports under `member`.
fn listed_names<'page>(page: &'page Value, member: &str) -> Vec<&'page str> {
    page[member]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|summary| summary["Name"].as_str())
                .collect()
        })
        .unwrap_or_default()
}

/// `CreateRegexPatternSet` stores a set whose expressions parse, and refuses one
/// whose expressions do not — leaving nothing behind.
///
/// Every row runs on its own emulator: each is a create under its own name, and a
/// refused row then creates the SAME name again as a positive control, which is the
/// comparison that pins the difference to the expression and not to the request's
/// shape, name or scope. On an unparseable row the offending expression is the
/// SECOND element of the list on every other row, so the refusal is shown to report
/// the element at fault rather than the list's first element.
#[tokio::test]
async fn create_regex_pattern_set_refuses_an_expression_that_does_not_parse() {
    for (index, expression) in expressions().iter().enumerate() {
        let Expression {
            case,
            pattern,
            parseable,
        } = expression;
        let waf = Waf::new();
        let name = format!("regex-syntax-{index}");
        let scope = if index % 2 == 0 {
            "REGIONAL"
        } else {
            "CLOUDFRONT"
        };
        let submitted: Vec<String> = if *parseable || index % 3 == 0 {
            vec![pattern.clone()]
        } else {
            vec![PARSEABLE.to_string(), pattern.clone()]
        };
        let request = json!({
            "Name": &name,
            "Scope": scope,
            "RegularExpressionList": regular_expression_list(&submitted),
        });

        if *parseable {
            let created = waf.ok("CreateRegexPatternSet", request).await;
            let id = created["Summary"]["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create answers Summary.Id"))
                .to_string();
            let got = waf
                .ok(
                    "GetRegexPatternSet",
                    json!({ "Name": &name, "Scope": scope, "Id": &id }),
                )
                .await;
            assert_eq!(
                got["RegexPatternSet"]["RegularExpressionList"],
                regular_expression_list(&submitted),
                "case {case:?}: an accepted expression must read back byte for byte, \
                 got {got}"
            );
            continue;
        }

        let body = waf
            .refused(
                "CreateRegexPatternSet",
                request,
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_regex_refusal(&body, FIELD_REGEX_PATTERN_SET, pattern, case);
        assert!(
            body["Summary"].is_null(),
            "case {case:?}: a refusal must carry no Summary, got {body}"
        );

        let page = waf
            .ok("ListRegexPatternSets", json!({ "Scope": scope }))
            .await;
        assert!(
            !listed_set_names(&page).contains(&name.as_str()),
            "case {case:?}: a refused create must leave no set named {name:?}, got {page}"
        );

        // The positive control: the SAME name and scope, with an expression that
        // parses, is accepted and round-trips.
        let created = waf
            .ok(
                "CreateRegexPatternSet",
                json!({
                    "Name": &name,
                    "Scope": scope,
                    "RegularExpressionList": regular_expression_list(&[PARSEABLE]),
                }),
            )
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the control create answers Summary.Id"))
            .to_string();
        let got = waf
            .ok(
                "GetRegexPatternSet",
                json!({ "Name": &name, "Scope": scope, "Id": &id }),
            )
            .await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&[PARSEABLE]),
            "case {case:?}: the control must store the expression that parses, got {got}"
        );
    }
}

/// `UpdateRegexPatternSet` makes the same judgement, and a refused update changes
/// nothing: the stored list stands and the token the caller holds still works.
#[tokio::test]
async fn update_regex_pattern_set_refuses_an_expression_that_does_not_parse() {
    let waf = Waf::new();
    let scope = "REGIONAL";
    let name = "regex-syntax-update";
    let stored = vec![PARSEABLE.to_string(), r"\.js$".to_string()];
    let created = waf
        .ok(
            "CreateRegexPatternSet",
            json!({
                "Name": name,
                "Scope": scope,
                "RegularExpressionList": regular_expression_list(&stored),
                "Description": "before the refusals",
            }),
        )
        .await;
    let id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let lock_token = created["Summary"]["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    for expression in expressions().iter().filter(|row| !row.parseable) {
        let Expression { case, pattern, .. } = expression;
        let submitted = vec![PARSEABLE.to_string(), pattern.clone()];
        let body = waf
            .refused(
                "UpdateRegexPatternSet",
                json!({
                    "Name": name,
                    "Scope": scope,
                    "Id": &id,
                    "LockToken": &lock_token,
                    "RegularExpressionList": regular_expression_list(&submitted),
                }),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_regex_refusal(&body, FIELD_REGEX_PATTERN_SET, pattern, case);
        assert!(
            body["NextLockToken"].is_null(),
            "case {case:?}: a refusal must carry no NextLockToken, got {body}"
        );

        let got = waf
            .ok(
                "GetRegexPatternSet",
                json!({ "Name": name, "Scope": scope, "Id": &id }),
            )
            .await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&stored),
            "case {case:?}: a refused update must leave the stored list as it was, \
             got {got}"
        );
        assert_eq!(
            got["LockToken"].as_str(),
            Some(lock_token.as_str()),
            "case {case:?}: a refused update must not rotate the token, got {got}"
        );
    }

    // The token no refusal spent still works, and the expressions that parse are
    // stored: the refusals above changed nothing at all.
    let replacement = vec!["^/only$".to_string()];
    let updated = waf
        .ok(
            "UpdateRegexPatternSet",
            json!({
                "Name": name,
                "Scope": scope,
                "Id": &id,
                "LockToken": &lock_token,
                "RegularExpressionList": regular_expression_list(&replacement),
            }),
        )
        .await;
    assert!(
        updated["NextLockToken"].as_str().is_some_and(|next| next != lock_token),
        "the accepted update must rotate the token, got {updated}"
    );
    let got = waf
        .ok(
            "GetRegexPatternSet",
            json!({ "Name": name, "Scope": scope, "Id": &id }),
        )
        .await;
    assert_eq!(
        got["RegexPatternSet"]["RegularExpressionList"],
        regular_expression_list(&replacement),
        "the accepted update must replace the stored list, got {got}"
    );
}

/// A rule running the anti-DDoS managed rule group, whose configuration carries
/// `ClientSideAction$ExemptUriRegularExpressions` — "the regular expression to match
/// against the web request URI, used to identify requests that can't handle a silent
/// browser challenge" (that member's own doc), the other member of this service that
/// carries caller-supplied expressions.
///
/// A rule that runs a rule group carries `OverrideAction` rather than `Action`, and a
/// `ManagedRuleGroupStatement` is well placed only as a rule's own top-level
/// statement in a web ACL (its own doc), so this is the one placement the member has.
fn anti_ddos_rule(name: &str, patterns: &[&str]) -> Value {
    json!({
        "Name": name,
        "Priority": 0,
        "Statement": {
            "ManagedRuleGroupStatement": {
                "VendorName": "AWS",
                "Name": "AWSManagedRulesAntiDDoSRuleSet",
                "ManagedRuleGroupConfigs": [{
                    "AWSManagedRulesAntiDDoSRuleSet": {
                        "ClientSideActionConfig": {
                            "Challenge": {
                                "UsageOfAction": "ENABLED",
                                "Sensitivity": "HIGH",
                                "ExemptUriRegularExpressions": regular_expression_list(patterns),
                            }
                        },
                        "SensitivityToBlock": "LOW",
                    }
                }],
            }
        },
        "OverrideAction": { "None": {} },
        "VisibilityConfig": visibility(name),
    })
}

/// The expressions a managed rule group's own configuration carries are judged the
/// same way: the rule is over the member that carries an expression, not over the
/// statement that happens to carry the member.
#[tokio::test]
async fn a_managed_rule_group_configs_expressions_are_judged_too() {
    let waf = Waf::new();
    let scope = "REGIONAL";

    for expression in expressions().iter().filter(|row| !row.parseable) {
        let Expression { case, pattern, .. } = expression;
        let name = "anti-ddos-acl";
        let body = waf
            .refused(
                "CreateWebACL",
                json!({
                    "Name": name,
                    "Scope": scope,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(name),
                    "Rules": [anti_ddos_rule("anti-ddos", &[PARSEABLE, pattern])],
                }),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_regex_refusal(&body, FIELD_STATEMENT, pattern, case);
        let page = waf.ok("ListWebACLs", json!({ "Scope": scope })).await;
        assert!(
            !listed_names(&page, "WebACLs").contains(&name),
            "case {case:?}: a refused create must leave no web ACL named {name:?}, \
             got {page}"
        );
    }

    // The same configuration with expressions that parse is accepted and reads back
    // verbatim: the syntax is the only difference.
    let accepted = anti_ddos_rule("anti-ddos", &[PARSEABLE, r"\/api\/|\.js$"]);
    let (acl_id, _, _) =
        create_web_acl(&waf, scope, "anti-ddos-acl", json!([accepted.clone()])).await;
    let got = waf
        .ok(
            "GetWebACL",
            json!({ "Name": "anti-ddos-acl", "Scope": scope, "Id": &acl_id }),
        )
        .await;
    assert_eq!(
        got["WebACL"]["Rules"][0], accepted,
        "the accepted configuration must read back verbatim, got {got}"
    );
}

/// A `RegexMatchStatement` over the URI path, carrying `pattern`.
fn regex_match_statement(pattern: &str) -> Value {
    json!({
        "RegexMatchStatement": {
            "RegexString": pattern,
            "FieldToMatch": { "UriPath": {} },
            "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
        }
    })
}

/// One placement of a statement inside a rule's statement tree.
fn placements(pattern: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "the rule's own top-level statement",
            regex_match_statement(pattern),
        ),
        (
            "nested inside a NotStatement",
            json!({ "NotStatement": { "Statement": regex_match_statement(pattern) } }),
        ),
        (
            "nested two deep, inside an AndStatement's NotStatement",
            json!({
                "AndStatement": {
                    "Statements": [
                        byte_match_statement(),
                        { "NotStatement": { "Statement": regex_match_statement(pattern) } },
                    ]
                }
            }),
        ),
    ]
}

/// The same judgement on a rule's own expression: a `RegexMatchStatement` whose
/// `RegexString` does not parse is refused by every operation that saves or prices
/// rules, at any nesting depth, and nothing is stored.
#[tokio::test]
async fn rules_carrying_an_expression_that_does_not_parse_are_refused() {
    let waf = Waf::new();
    let scope = "REGIONAL";
    // One unparseable expression per placement, drawn from the table above, so the
    // placements are covered independently of which construct is at fault.
    let unparseable: Vec<String> = expressions()
        .into_iter()
        .filter(|row| !row.parseable)
        .map(|row| row.pattern)
        .collect();

    for (index, (placement, _)) in placements(PARSEABLE).into_iter().enumerate() {
        let pattern = unparseable[index % unparseable.len()].clone();
        let statement = placements(&pattern)
            .into_iter()
            .nth(index)
            .map(|(_, statement)| statement)
            .expect("the placement exists");
        let rules = json!([rule("regex-rule", statement)]);
        let name = format!("regex-rule-{index}");

        for (operation, request) in [
            (
                "CreateWebACL",
                json!({
                    "Name": &name,
                    "Scope": scope,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&name),
                    "Rules": rules.clone(),
                }),
            ),
            (
                "CreateRuleGroup",
                create_body(&name, scope, 500, None, &rules, None),
            ),
            (
                "CheckCapacity",
                json!({ "Scope": scope, "Rules": rules.clone() }),
            ),
        ] {
            let case = format!("{operation}, {placement}");
            let body = waf
                .refused(operation, request, "WAFInvalidParameterException", &case)
                .await;
            assert_regex_refusal(&body, FIELD_STATEMENT, &pattern, &case);
            assert!(
                body["Summary"].is_null() && body["Capacity"].is_null(),
                "case {case:?}: a refusal must carry neither a Summary nor a Capacity, \
                 got {body}"
            );
        }

        for (operation, member) in [("ListWebACLs", "WebACLs"), ("ListRuleGroups", "RuleGroups")] {
            let page = waf.ok(operation, json!({ "Scope": scope })).await;
            assert!(
                !listed_names(&page, member).contains(&name.as_str()),
                "case {placement:?}: a refused save must leave nothing named {name:?} \
                 for {operation}, got {page}"
            );
        }
    }

    // The same rule tree with an expression that parses is accepted, by the save
    // paths and by the pricing one: the syntax is the only difference.
    let good_rules = json!([rule("regex-rule", regex_match_statement(PARSEABLE))]);
    let priced = waf
        .ok(
            "CheckCapacity",
            json!({ "Scope": scope, "Rules": good_rules.clone() }),
        )
        .await;
    assert!(
        priced["Capacity"].as_i64().is_some_and(|capacity| capacity > 0),
        "a priceable rule list must answer a capacity, got {priced}"
    );
    let (acl_id, _, acl_lock) = create_web_acl(&waf, scope, "regex-acl", good_rules.clone()).await;
    let (group_id, _, group_lock) =
        create_rule_group(&waf, scope, "regex-group", 500, good_rules.clone()).await;

    // And an UPDATE of either is judged the same way, leaving the stored rules and
    // the stored token as they were.
    let pattern = unparseable[0].clone();
    let bad_rules = json!([rule("regex-rule", regex_match_statement(&pattern))]);
    for (operation, request, getter, member, id, lock) in [
        (
            "UpdateWebACL",
            json!({
                "Name": "regex-acl",
                "Scope": scope,
                "Id": &acl_id,
                "LockToken": &acl_lock,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility("regex-acl"),
                "Rules": bad_rules.clone(),
            }),
            "GetWebACL",
            "WebACL",
            acl_id.clone(),
            acl_lock.clone(),
        ),
        (
            "UpdateRuleGroup",
            json!({
                "Name": "regex-group",
                "Scope": scope,
                "Id": &group_id,
                "LockToken": &group_lock,
                "VisibilityConfig": visibility("regex-group"),
                "Rules": bad_rules.clone(),
            }),
            "GetRuleGroup",
            "RuleGroup",
            group_id.clone(),
            group_lock.clone(),
        ),
    ] {
        let case = format!("{operation} with an expression that does not parse");
        let body = waf
            .refused(operation, request, "WAFInvalidParameterException", &case)
            .await;
        assert_regex_refusal(&body, FIELD_STATEMENT, &pattern, &case);
        assert!(
            body["NextLockToken"].is_null(),
            "case {case:?}: a refusal must carry no NextLockToken, got {body}"
        );

        let name = if member == "WebACL" {
            "regex-acl"
        } else {
            "regex-group"
        };
        let got = waf
            .ok(
                getter,
                json!({ "Name": name, "Scope": scope, "Id": &id }),
            )
            .await;
        assert_eq!(
            got[member]["Rules"], good_rules,
            "case {case:?}: a refused update must leave the stored rules as they were, \
             got {got}"
        );
        assert_eq!(
            got["LockToken"].as_str(),
            Some(lock.as_str()),
            "case {case:?}: a refused update must not rotate the token, got {got}"
        );
    }
}
