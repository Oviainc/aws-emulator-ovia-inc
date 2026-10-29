// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the modelled `EmailContactName` constraint, wherever this
//! service accepts an email-contact name.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: a name this service accepts must satisfy the modelled
//! `EmailContactName` pattern `[\w-.~]+`
//! (`smithy_query(query="shape",shape="EmailContactName")`) over its WHOLE value — every
//! character one that class admits — and a request naming a value outside it is the
//! modelled 400 `ValidationException` that persists nothing. `CreateEmailContact$name` is
//! the one member of this service whose target carries that expression
//! (OPERATIONS.md's input lines; the other pattern-bearing members are `arn`,
//! `emailAddress` and `code`), so it is the one place the rule applies.
//!
//! The class is read as ECMA-262 reads it, which is the language a Smithy `@pattern` is
//! written in: a `-` between two class items that are not both single characters is a
//! LITERAL, so `[\w-.~]` is the union of `\w` (there an ASCII letter, an ASCII digit or
//! an underscore — never a letter of another script) and the three literals `-`, `.` and
//! `~`. The suites below therefore span:
//!
//! * a table of names differing along what the class generalises over — which admitted
//!   item a name is built from, its length at each bound the member's `@length` admits,
//!   which excluded character it carries, WHERE that character sits (start, middle, end,
//!   alone), and how wide it is in UTF-8;
//! * every ASCII character, swept one per request, so the admitted set is pinned as a SET
//!   rather than at the handful of characters a table can name;
//! * the round trip: a name the class admits is reported back verbatim by
//!   `GetEmailContact` and `ListEmailContacts`, which report it through a member
//!   declaring the same expression;
//! * what a refused create leaves behind, which is nothing — not an untagged contact,
//!   and not one holding the tags the refused request named.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_notificationscontacts_notificationscontacts::ops::types::CREATE_EMAIL_CONTACT_INPUT_SCHEMA;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::EMAIL_CONTACT_NAME_PATTERN;

use crate::email_contacts::{create, emulator, error_code, get_ok, list, Emulator};

/// The fewest characters `EmailContactName`'s `@length` admits
/// (`smithy_query(query="shape",shape="EmailContactName")`).
const MIN_NAME_LEN: usize = 1;

/// The most characters it admits, from the same shape.
const MAX_NAME_LEN: usize = 64;

/// The three literals `[\w-.~]` admits beside `\w`, as the test reads the class — kept
/// separate from the crate's own statement of them so a row is not merely re-asserting
/// the implementation's list back at itself.
const LITERALS: &str = "-.~";

/// One name and whether the modelled `EmailContactName` pattern admits it.
struct Case {
    /// What this row varies, named in every assertion.
    what: String,
    name: String,
    /// What the modelled `name` member does with this value.
    outcome: Outcome,
}

/// What a row's `name` earns.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Every character is one `[\w-.~]` admits, and there is at least one of them (which
    /// the `+` quantifier wants), so the contact is created.
    Created,
    /// Some character is outside that class: the 400 this service's handler words, which
    /// blames the member through `ValidationException$fieldList`.
    OutsideThePattern,
    /// Outside the member's `@length` 1..64 — the bound the centralized constraint walk
    /// applies at the request boundary, which refuses the request before any handler
    /// runs and so states no field list. Here as a control: these rows must be 400 too,
    /// and they are what shows the length bound is not what refuses the pattern rows.
    OutsideTheLengthBound,
}

fn case(what: impl Into<String>, name: impl Into<String>, outcome: Outcome) -> Case {
    Case {
        what: what.into(),
        name: name.into(),
        outcome,
    }
}

/// Every row is one `name`, the rows differing along what the pattern generalises over:
/// which item of the class a name is built from, its length at each bound of the member's
/// `@length` (and one past the maximum, which the length bound itself refuses), which
/// excluded character it carries, where that character sits, and how many bytes it takes.
///
/// The three position rows are the ones a FIND of the expression would admit — each
/// carries a run of admitted characters — and are what pins the rule as holding over the
/// whole value.
fn name_table() -> Vec<Case> {
    [
        case("a word of ASCII letters", "oncall", Outcome::Created),
        case(
            "upper case, which the class admits too",
            "ONCALL",
            Outcome::Created,
        ),
        case("digits and an underscore", "team_42", Outcome::Created),
        case(
            "every literal the class admits beside the word class",
            LITERALS,
            Outcome::Created,
        ),
        case(
            "one of each item of the class",
            "On-call.Primary~v2_3",
            Outcome::Created,
        ),
        case(
            "the shortest name the length bound admits",
            "n".repeat(MIN_NAME_LEN),
            Outcome::Created,
        ),
        case(
            "a name at the length maximum",
            "n".repeat(MAX_NAME_LEN),
            Outcome::Created,
        ),
        case(
            "one character past the length maximum",
            "n".repeat(MAX_NAME_LEN + 1),
            Outcome::OutsideTheLengthBound,
        ),
        case(
            "the empty name, one below the length minimum",
            "",
            Outcome::OutsideTheLengthBound,
        ),
        case("a space in the middle", "team alpha", Outcome::OutsideThePattern),
        case(
            "an exclamation mark at the end",
            "team alpha!",
            Outcome::OutsideThePattern,
        ),
        case("a space at the start", " oncall", Outcome::OutsideThePattern),
        case("a space at the end", "oncall ", Outcome::OutsideThePattern),
        case("a solidus", "a/b", Outcome::OutsideThePattern),
        case("a line feed", "line\nbreak", Outcome::OutsideThePattern),
        case("a carriage return", "line\rbreak", Outcome::OutsideThePattern),
        case("a tab", "team\talpha", Outcome::OutsideThePattern),
        case("an at sign", "team@example", Outcome::OutsideThePattern),
        case("a plus sign", "team+42", Outcome::OutsideThePattern),
        case("a colon", "team:infra", Outcome::OutsideThePattern),
        case("a percent sign", "100%oncall", Outcome::OutsideThePattern),
        case("a comma", "oncall,primary", Outcome::OutsideThePattern),
        case("an asterisk", "oncall*", Outcome::OutsideThePattern),
        case(
            "nothing but excluded characters",
            "!!!",
            Outcome::OutsideThePattern,
        ),
        case(
            "two-byte letters of another script, which the word class excludes",
            "équipe",
            Outcome::OutsideThePattern,
        ),
        case(
            "a non-ASCII digit",
            "team\u{0664}",
            Outcome::OutsideThePattern,
        ),
        case(
            "a four-byte character",
            "oncall\u{1F514}",
            Outcome::OutsideThePattern,
        ),
        case(
            "a name at the length maximum whose last character is excluded",
            format!("{} ", "n".repeat(MAX_NAME_LEN - 1)),
            Outcome::OutsideThePattern,
        ),
    ]
    .into_iter()
    .collect()
}

/// A name short enough to name in an assertion message, with its excluded characters
/// escaped, so a 65-character row does not print a wall of text and a line feed does not
/// break the message across two lines.
fn shown(name: &str) -> String {
    let head: String = name.chars().take(24).collect();
    if head.chars().count() == name.chars().count() {
        format!("{head:?}")
    } else {
        format!("{head:?}… [{} characters]", name.chars().count())
    }
}

/// `POST /2022-09-19/emailcontacts` with `name`, and an `emailAddress` the model admits
/// whatever the name is — so the name is the only thing a row varies.
async fn create_named(em: &Emulator, index: usize, name: &str) -> (u16, Value) {
    create(
        em,
        &json!({
            "name": name,
            "emailAddress": format!("contact-{index}@example.com"),
        }),
    )
    .await
}

/// Every contact the store currently holds, by ARN, read through `ListEmailContacts`.
async fn stored_arns(em: &Emulator) -> BTreeSet<String> {
    listed(em).await.into_iter().map(|(arn, _)| arn).collect()
}

/// Every contact the store currently holds as `(arn, name)`, read through
/// `ListEmailContacts`.
async fn listed(em: &Emulator) -> BTreeSet<(String, String)> {
    let (status, body) = list(em, "").await;
    assert_eq!(
        status, 200,
        "ListEmailContacts must answer 200, got {status} / {body:?}"
    );
    body["emailContacts"]
        .as_array()
        .unwrap_or_else(|| panic!("emailContacts is @required, got {body:?}"))
        .iter()
        .map(|contact| {
            (
                contact["arn"].as_str().unwrap_or_default().to_string(),
                contact["name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The modelled 400 a name outside the pattern earns: the status, the error shape, the
/// member it blames, and the expression it states.
fn assert_refused(case: &str, name: &str, status: u16, body: &Value) {
    assert_eq!(
        status,
        400,
        "case {case:?} ({}): a name the modelled EmailContactName shape refuses must be \
         refused, got {status} / {body:?}",
        shown(name)
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?} ({}): expected ValidationException, got {body:?}",
        shown(name)
    );
}

/// The same, plus what the handler's own refusal adds: the member it blames through
/// `ValidationException$fieldList`.
fn assert_refused_by_the_pattern(case: &str, name: &str, status: u16, body: &Value) {
    assert_refused(case, name, status, body);
    let blamed: Vec<&str> = body
        .get("fieldList")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        blamed,
        vec!["name"],
        "case {case:?} ({}): the 400 must blame the member the request wrote, got {body:?}",
        shown(name)
    );
}

#[tokio::test]
async fn create_email_contact_applies_the_modelled_email_contact_name_pattern() {
    let em = emulator();
    let table = name_table();
    assert!(
        table.len() >= 5,
        "the name table must span the class, got {} rows",
        table.len()
    );

    let mut expected: BTreeSet<String> = BTreeSet::new();
    let mut expected_pairs: BTreeSet<(String, String)> = BTreeSet::new();
    for (index, case) in table.iter().enumerate() {
        let (status, body) = create_named(&em, index, &case.name).await;

        if case.outcome == Outcome::Created {
            assert_eq!(
                status,
                201,
                "case {:?} ({}): a name the pattern admits must be created, got {status} / \
                 {body:?}",
                case.what,
                shown(&case.name)
            );
            let arn = body["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateEmailContact must return an arn, got {body:?}"))
                .to_string();
            let contact = get_ok(&em, &arn).await;
            assert_eq!(
                contact.get("name").and_then(Value::as_str),
                Some(case.name.as_str()),
                "case {:?} ({}): the stored name must be reported verbatim, got {contact:?}",
                case.what,
                shown(&case.name)
            );
            expected_pairs.insert((arn.clone(), case.name.clone()));
            expected.insert(arn);
        } else if case.outcome == Outcome::OutsideThePattern {
            assert_refused_by_the_pattern(&case.what, &case.name, status, &body);
        } else {
            // The length bound is applied at the request boundary, which refuses the
            // request before any handler runs, so this 400 states no field list.
            assert_refused(&case.what, &case.name, status, &body);
        }

        assert_eq!(
            stored_arns(&em).await,
            expected,
            "case {:?} ({}): only the creates the pattern admitted may have persisted a \
             contact",
            case.what,
            shown(&case.name)
        );
    }

    assert!(
        !expected.is_empty(),
        "the table must admit at least one name, or the refusals prove nothing"
    );

    // And every name the class admitted is reported by the LIST too, verbatim: the member
    // `ListEmailContacts` reports it through declares the same expression.
    assert_eq!(
        listed(&em).await,
        expected_pairs,
        "the list must report exactly the stored contacts, each under the name it was \
         created with"
    );
}

/// The refusal states the requirement the value broke: the expression the MODEL declares,
/// at the member path the request wrote it under.
#[tokio::test]
async fn the_refusal_states_the_modelled_expression_at_the_member_it_blames() {
    let em = emulator();

    for (index, name) in ["team alpha", "a/b", "line\nbreak", "équipe"]
        .into_iter()
        .enumerate()
    {
        let (status, body) = create_named(&em, index, name).await;
        assert_refused_by_the_pattern("a name outside the pattern", name, status, &body);

        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a ValidationException carries a message, got {body:?}"));
        assert!(
            message.contains(EMAIL_CONTACT_NAME_PATTERN),
            "case {:?}: the refusal must state the modelled expression \
             {EMAIL_CONTACT_NAME_PATTERN:?}, got {message:?}",
            shown(name)
        );
        assert!(
            message.contains("at 'name'"),
            "case {:?}: the refusal must name the member path, got {message:?}",
            shown(name)
        );
        let requirement: Vec<&str> = body["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("message").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            requirement
                .iter()
                .all(|stated| stated.contains(EMAIL_CONTACT_NAME_PATTERN)),
            "case {:?}: the blamed field must state the same expression, got {requirement:?}",
            shown(name)
        );
    }
}

/// The expression the crate applies is the one the MODEL declares for this member, read
/// off the generated input schema rather than transcribed into the assertion.
#[test]
fn the_applied_expression_is_the_models_own() {
    let member = CREATE_EMAIL_CONTACT_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| member.member_name() == Some("name"))
        .expect("CreateEmailContactRequest declares a `name` member");
    let declared = member
        .traits()
        .and_then(|traits| {
            traits.get(&aws_smithy_schema_server::constraints::PatternTrait::TRAIT_ID)
        })
        .and_then(|found| {
            found
                .as_any()
                .downcast_ref::<aws_smithy_schema_server::constraints::PatternTrait>()
        })
        .map(|pattern| pattern.regex);
    assert_eq!(
        declared,
        Some(EMAIL_CONTACT_NAME_PATTERN),
        "the expression this service applies must be the one the model declares for \
         CreateEmailContactRequest$name, got {declared:?}"
    );
}

/// The admitted set, as a SET: every ASCII character is submitted inside an otherwise
/// legal name, and exactly the characters `[\w-.~]` admits are accepted.
///
/// The expectation is read off the class itself — an ASCII letter, an ASCII digit, an
/// underscore, or one of [`LITERALS`] — so the sweep is not a restatement of the
/// implementation's own list.
#[tokio::test]
async fn every_ascii_character_is_admitted_exactly_when_the_class_admits_it() {
    // One account per row: the published per-account email-contact quota bounds how
    // many contacts one account may HOLD at a time, and this sweep is about the name a
    // create carries rather than about that quota.
    for codepoint in 0u8..128 {
        let em = emulator();
        let character = char::from(codepoint);
        let admitted =
            character.is_ascii_alphanumeric() || character == '_' || LITERALS.contains(character);
        let name = format!("x{character}");
        let (status, body) = create_named(&em, usize::from(codepoint), &name).await;
        let expected = if admitted { 201 } else { 400 };
        assert_eq!(
            status,
            expected,
            "case {:?} (U+{codepoint:04X}): the class {} this character, so expected HTTP \
             {expected}, got {status} / {body:?}",
            shown(&name),
            if admitted { "admits" } else { "excludes" }
        );
        if !admitted {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {:?} (U+{codepoint:04X}): expected ValidationException, got {body:?}",
                shown(&name)
            );
        }
    }
}

/// A create carrying a refused name leaves nothing behind — not an untagged contact, and
/// not one holding the tags the request named — and the same request with its name
/// replaced by one the pattern admits is accepted, so the refusal was the name alone.
#[tokio::test]
async fn a_refused_name_persists_no_contact_and_no_tags() {
    let em = emulator();
    let tags = json!({ "team": "infra", "env": "prod" });

    for (index, name) in ["team alpha", "a b c", "oncall/primary"]
        .into_iter()
        .enumerate()
    {
        let (status, body) = create(
            &em,
            &json!({
                "name": name,
                "emailAddress": format!("mixed-{index}@example.com"),
                "tags": tags.clone(),
            }),
        )
        .await;
        assert_refused_by_the_pattern("a refused name beside legal tags", name, status, &body);
        assert!(
            stored_arns(&em).await.is_empty(),
            "case {:?}: a refused create must leave no contact behind",
            shown(name)
        );
    }

    // A name carrying a refused character AND a tag key the other hand-applied pattern
    // refuses is still one 400: the input is judged before the store is touched.
    let (status, body) = create(
        &em,
        &json!({
            "name": "team alpha",
            "emailAddress": "both@example.com",
            "tags": { "aws:owner": "x" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a refused name and a refused tag key\": expected 400, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a refused name and a refused tag key\": expected ValidationException, got \
         {body:?}"
    );
    assert!(
        stored_arns(&em).await.is_empty(),
        "case \"a refused name and a refused tag key\": nothing may persist"
    );

    // The same request with a name the pattern admits is created, and keeps its tags.
    let (status, body) = create(
        &em,
        &json!({
            "name": "team-alpha",
            "emailAddress": "team-alpha@example.com",
            "tags": tags.clone(),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"the same request with an admitted name\": expected 201, got {status} / {body:?}"
    );
    let arn = body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateEmailContact must return an arn, got {body:?}"))
        .to_string();
    let contact = get_ok(&em, &arn).await;
    assert_eq!(
        contact.get("name").and_then(Value::as_str),
        Some("team-alpha"),
        "case \"the same request with an admitted name\": the name must round-trip, got \
         {contact:?}"
    );
    assert_eq!(
        stored_arns(&em).await,
        BTreeSet::from([arn]),
        "case \"the same request with an admitted name\": exactly the accepted create persists"
    );
}
