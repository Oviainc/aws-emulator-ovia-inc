// SPDX-License-Identifier: CC0-1.0

//! ONE rule, over every member of this service whose model carries a `@pattern` a
//! request can violate: a refusal quotes the model's expression MINUS the whole-value
//! `^` / `$` pair the model writes around it to make the match total — and quotes the
//! model's expression as declared where those two characters are anchors of the
//! expression's own branches rather than a pair wrapped around it, since then there is no
//! wrapping to take off and taking the two characters off states a requirement neither
//! branch ever had.
//!
//! The rows span the `@pattern` expressions this service's request members target
//! (`smithy_query query=closure` over this crate's `smithy-model/`): the plain
//! whole-value forms `^[a-zA-Z0-9][a-zA-Z0-9_-]*$` (`RescoreExecutionPlanName`),
//! `^[a-zA-Z0-9][a-zA-Z0-9-]*$` (`RescoreExecutionPlanId`) and `^\P{C}*$` (`Description`,
//! `NextToken`, `DocumentBody`), and the ALTERNATION `^$|[\x00-\x7F]+$`
//! (`ClientTokenName`), whose `^` belongs to its first branch and whose final `$` to its
//! second — over five operations, a member nested in a list element included.
//!
//! Two tests: the text each refusal states, and the invariant that decides which of the
//! two renderings a pattern gets — the pair comes off exactly where taking it off leaves
//! an expression that, applied to a whole value, admits what the model's own text admits.

use regex::Regex;
use serde_json::{json, Value};

use crate::harness::{call, emulator, error_code, error_message};

/// The text a clause puts between the constraint it names and the expression it
/// advertises.
const PATTERN_MARKER: &str = "Member must satisfy regular expression pattern: ";

/// A 36-character `RescoreExecutionPlanId` inside everything `RescoreExecutionPlanId`
/// models, so a row violating ANOTHER member's pattern reports that one member only.
const WELL_FORMED_PLAN_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `@pattern` of each shape the rows below reach, as the model declares it
/// (`smithy_query query=shape` on this crate's `smithy-model/`).
const PLAN_NAME_PATTERN: &str = r"^[a-zA-Z0-9][a-zA-Z0-9_-]*$";
const PLAN_ID_PATTERN: &str = r"^[a-zA-Z0-9][a-zA-Z0-9-]*$";
const NON_CONTROL_PATTERN: &str = r"^\P{C}*$";
const CLIENT_TOKEN_PATTERN: &str = r"^$|[\x00-\x7F]+$";

/// One row: a member, the `@pattern` its target shape carries, the expression a refusal
/// of it must quote, a request outside that pattern and a request inside it.
struct PatternCase {
    /// What this row varies.
    case: &'static str,
    operation: &'static str,
    /// The validation path the report names the member by.
    member: &'static str,
    /// The expression the model declares on the member's target shape.
    modelled: &'static str,
    /// The expression the clause must quote.
    quoted: &'static str,
    /// A request carrying [`PatternCase::refused`] in `member`.
    body: Value,
    /// The value `body` carries in `member`, which the pattern does NOT admit.
    refused: String,
    /// The same request with a value of `member` the pattern DOES admit.
    admitted_body: Value,
}

/// A `Rescore` request against a well-formed plan id carrying one document whose `Body`
/// is `body`.
fn rescore_with_body(body: &str) -> Value {
    json!({
        "RescoreExecutionPlanId": WELL_FORMED_PLAN_ID,
        "SearchQuery": "cats",
        "Documents": [{ "Id": "d1", "OriginalScore": 0.5, "Body": body }],
    })
}

/// Every row of the table, built fresh so each test drives its own requests.
fn cases() -> Vec<PatternCase> {
    // A plan id whose first character is outside `RescoreExecutionPlanId`'s pattern while
    // the value stays at the 36 characters the shape pins, so the pattern is the only
    // constraint it offends.
    let underscore_led_id = format!("_{}", "0".repeat(35));
    vec![
        PatternCase {
            case: "a Name carrying a character outside its pattern",
            operation: "CreateRescoreExecutionPlan",
            member: "name",
            modelled: PLAN_NAME_PATTERN,
            quoted: "[a-zA-Z0-9][a-zA-Z0-9_-]*",
            body: json!({ "Name": "bad name!" }),
            refused: "bad name!".to_string(),
            admitted_body: json!({ "Name": "plan-1" }),
        },
        PatternCase {
            case: "a Description carrying a control character",
            operation: "CreateRescoreExecutionPlan",
            member: "description",
            modelled: NON_CONTROL_PATTERN,
            quoted: r"\P{C}*",
            body: json!({ "Name": "fine", "Description": "a\u{7}bell" }),
            refused: "a\u{7}bell".to_string(),
            admitted_body: json!({ "Name": "fine", "Description": "a bell" }),
        },
        // The alternation, in the two forms a value can fall outside it: its ASCII branch
        // has to reach the END of the value, so a value whose last character is outside
        // ASCII satisfies neither branch.
        PatternCase {
            case: "a ClientToken whose last character is outside its ASCII branch",
            operation: "CreateRescoreExecutionPlan",
            member: "clientToken",
            modelled: CLIENT_TOKEN_PATTERN,
            quoted: CLIENT_TOKEN_PATTERN,
            body: json!({ "Name": "fine", "ClientToken": "tokenø" }),
            refused: "tokenø".to_string(),
            admitted_body: json!({ "Name": "fine", "ClientToken": "token" }),
        },
        PatternCase {
            case: "a ClientToken of one non-ASCII character, at its length minimum",
            operation: "CreateRescoreExecutionPlan",
            member: "clientToken",
            modelled: CLIENT_TOKEN_PATTERN,
            quoted: CLIENT_TOKEN_PATTERN,
            body: json!({ "Name": "fine", "ClientToken": "ø" }),
            refused: "ø".to_string(),
            admitted_body: json!({ "Name": "fine", "ClientToken": "t" }),
        },
        PatternCase {
            case: "an Update's Name carrying a character outside its pattern",
            operation: "UpdateRescoreExecutionPlan",
            member: "name",
            modelled: PLAN_NAME_PATTERN,
            quoted: "[a-zA-Z0-9][a-zA-Z0-9_-]*",
            body: json!({ "Id": WELL_FORMED_PLAN_ID, "Name": "spaced name" }),
            refused: "spaced name".to_string(),
            admitted_body: json!({ "Id": WELL_FORMED_PLAN_ID, "Name": "spaced_name" }),
        },
        PatternCase {
            case: "a plan id at its pinned length whose first character is outside its \
                   pattern",
            operation: "DescribeRescoreExecutionPlan",
            member: "id",
            modelled: PLAN_ID_PATTERN,
            quoted: "[a-zA-Z0-9][a-zA-Z0-9-]*",
            body: json!({ "Id": underscore_led_id }),
            refused: underscore_led_id.clone(),
            admitted_body: json!({ "Id": "0".repeat(36) }),
        },
        PatternCase {
            case: "a NextToken carrying a control character",
            operation: "ListRescoreExecutionPlans",
            member: "nextToken",
            modelled: NON_CONTROL_PATTERN,
            quoted: r"\P{C}*",
            body: json!({ "NextToken": "tab\u{1}control" }),
            refused: "tab\u{1}control".to_string(),
            admitted_body: json!({ "NextToken": "page-2" }),
        },
        PatternCase {
            case: "a document Body carrying a control character, nested in a list element",
            operation: "Rescore",
            member: "documents.1.member.body",
            modelled: NON_CONTROL_PATTERN,
            quoted: r"\P{C}*",
            body: rescore_with_body("a\u{2}note"),
            refused: "a\u{2}note".to_string(),
            admitted_body: rescore_with_body("a note"),
        },
    ]
}

/// The clause a refusal of `member`'s pattern opens with — the text an answer that
/// refused a value for THAT pattern carries and no other answer does.
fn pattern_clause_of(member: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {PATTERN_MARKER}")
}

/// The expression the clause of `message` quotes.
fn quoted_expression(case: &str, message: &str) -> String {
    message
        .rsplit_once(PATTERN_MARKER)
        .unwrap_or_else(|| panic!("case {case:?}: expected a pattern requirement, got {message:?}"))
        .1
        .to_string()
}

/// `expression` compiled as the requirement a WHOLE value has to satisfy, which is how a
/// caller applies the expression a refusal quotes.
fn whole_value(case: &str, expression: &str) -> Regex {
    Regex::new(&format!("^(?:{expression})$"))
        .unwrap_or_else(|error| panic!("case {case:?}: {expression:?} is no expression: {error}"))
}

/// `expression` compiled as the model's own text, which a `@pattern` check applies as
/// written — the expression anchors whatever part of the value it requires itself.
fn as_modelled(case: &str, expression: &str) -> Regex {
    Regex::new(expression)
        .unwrap_or_else(|error| panic!("case {case:?}: {expression:?} is no expression: {error}"))
}

#[tokio::test]
async fn a_pattern_refusal_quotes_the_model_expression_minus_only_a_wrapping_anchor_pair() {
    let router = emulator();

    for case in cases() {
        let (status, answer) = call(&router, case.operation, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?} on {}: expected HTTP 400, got {status} {answer}",
            case.case, case.operation
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {:?}: expected the modelled error code, got {answer}",
            case.case
        );
        let message = error_message(&answer);
        let expected = format!(
            "1 validation error detected: {}{}",
            pattern_clause_of(case.member),
            case.quoted
        );
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            case.case
        );
        assert!(
            !message.contains(case.refused.as_str()),
            "case {:?}: the report must not echo the offending value {:?}, got {message:?}",
            case.case,
            case.refused
        );
        // The rendering takes a pair OFF the model's text; it never rewrites the
        // expression, so what it quotes is a run of that text.
        assert!(
            case.modelled.contains(case.quoted),
            "case {:?}: the quoted {:?} is no part of the modelled {:?}",
            case.case,
            case.quoted,
            case.modelled
        );
        // A value the pattern admits is refused for no pattern of this member, so the
        // quoted requirement is one a caller can act on.
        let (status, answer) = call(&router, case.operation, case.admitted_body.clone()).await;
        let message = error_message(&answer);
        assert!(
            !message.contains(&pattern_clause_of(case.member)),
            "case {:?}: a value the pattern admits must not be refused for it, got \
             {status} {message:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn the_anchor_pair_comes_off_exactly_where_taking_it_off_preserves_the_expression() {
    let router = emulator();

    // The values each row's two expressions are applied to: empty, ASCII, ASCII carrying
    // the characters the patterns separate on, a value whose non-ASCII character is in the
    // MIDDLE (so an ASCII run still reaches its end) and one whose non-ASCII character is
    // last, non-ASCII throughout, a control character, and a regex metacharacter.
    let probes = [
        "",
        "a",
        "plan_1-2",
        "bad name!",
        "_leading",
        "000000000000000000000000000000000000",
        "tok€n-7",
        "tokenø",
        "日本語",
        "a$b",
        "a\u{7}bell",
    ];

    for case in cases() {
        let (_, answer) = call(&router, case.operation, case.body.clone()).await;
        let message = error_message(&answer);
        let quoted = quoted_expression(case.case, &message);
        let modelled = as_modelled(case.case, case.modelled);

        if quoted == case.modelled {
            // The pair stayed. It stayed because it is no wrapping: taking it off states
            // an expression that, applied to a whole value, parts company with the
            // model's own text — which is what the rows that DO drop it never do.
            let mut stripped = case.modelled.to_string();
            stripped.pop();
            stripped.remove(0);
            let dropped = whole_value(case.case, &stripped);
            let disagreement = probes
                .iter()
                .find(|probe| dropped.is_match(probe) != modelled.is_match(probe));
            assert!(
                disagreement.is_some(),
                "case {:?}: the model's {:?} kept its anchors, so dropping them to \
                 {stripped:?} must change what the expression admits, and over the probes \
                 it did not",
                case.case,
                case.modelled
            );
        } else {
            // The pair came off. What is left admits, as a whole-value requirement,
            // exactly the values the model's own text admits.
            let stated = whole_value(case.case, &quoted);
            for probe in probes {
                assert_eq!(
                    stated.is_match(probe),
                    modelled.is_match(probe),
                    "case {:?}: the quoted {quoted:?} and the modelled {:?} disagree on \
                     {probe:?}: quoted says {}, modelled says {}",
                    case.case,
                    case.modelled,
                    stated.is_match(probe),
                    modelled.is_match(probe)
                );
            }
        }

        // Either way: the value the service refused fails the requirement it quoted, so a
        // caller who applies it sees the same verdict the service reached.
        let stated = whole_value(case.case, &quoted);
        assert!(
            !stated.is_match(&case.refused),
            "case {:?}: the quoted {quoted:?} must refuse the value the service refused, \
             but it admits {:?}",
            case.case,
            case.refused
        );
    }
}
