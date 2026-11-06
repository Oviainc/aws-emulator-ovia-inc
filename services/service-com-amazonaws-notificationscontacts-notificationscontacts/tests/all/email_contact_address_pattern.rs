// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the modelled `EmailContactAddress` constraint, wherever
//! this service accepts an email address.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rule under test: an email address this service accepts must satisfy the modelled
//! `EmailContactAddress` pattern `^(.+)@(.+)$`
//! (`smithy_query(query="shape",shape="EmailContactAddress")`), read as the ECMA-262
//! expression a Smithy `@pattern` is — one or more characters, an `@`, one or more
//! characters, where a character is every scalar value EXCEPT a line terminator (line
//! feed, carriage return, line separator U+2028 and paragraph separator U+2029, the four
//! `.` does not match) — and a request carrying a value outside it is the modelled 400
//! `ValidationException` that persists nothing. It holds at the one member whose target
//! is `EmailContactAddress`: `CreateEmailContact$emailAddress`, whose stored value is
//! what `EmailContact$address` reports back through that same modelled expression.

use serde_json::{json, Value};
use service_com_amazonaws_notificationscontacts_notificationscontacts::ops::types::CREATE_EMAIL_CONTACT_INPUT_SCHEMA;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::EMAIL_CONTACT_ADDRESS_PATTERN;

use crate::email_contacts::{create, emulator, error_code, get_ok, list, Emulator};

/// The character the modelled pattern requires between its two `.+` runs.
const SEPARATOR: char = '@';

/// The expression a refusal of this member ADVERTISES: the modelled pattern as this
/// service publishes it, which is [`EMAIL_CONTACT_ADDRESS_PATTERN`] without the
/// whole-value anchors it is written with. Transcribed rather than computed, so the suite
/// states the published expression independently of the code that strips the anchors.
const PUBLISHED_ADDRESS_PATTERN: &str = r"(.+)@(.+)";

/// The fewest characters the member's own `@length` admits
/// (`smithy_query(query="shape",shape="EmailContactAddress")` reports `len 6..254`), so a
/// row refused by the PATTERN stays clear of the length bound.
const MIN_ADDRESS_LEN: usize = 6;

/// The most characters that same `@length` admits.
const MAX_ADDRESS_LEN: usize = 254;

/// The characters the pattern's `.` does not match. A Smithy `@pattern` is an ECMA-262
/// regular expression, whose `.` matches every character except a line terminator: line
/// feed, carriage return, line separator (U+2028) and paragraph separator (U+2029).
const LINE_TERMINATORS: [(&str, char); 4] = [
    ("a line feed", '\n'),
    ("a carriage return", '\r'),
    ("the line separator U+2028", '\u{2028}'),
    ("the paragraph separator U+2029", '\u{2029}'),
];

/// Where in an otherwise legal address a line terminator is planted. The pattern excludes
/// it wherever it sits, so position cannot be what refuses the value.
const TERMINATOR_PLACEMENTS: [(&str, &str, &str); 4] = [
    ("inside the local part", "on", "call@example.com"),
    ("inside the domain", "oncall@exa", "mple.com"),
    ("at the very start", "", "oncall@example.com"),
    ("at the very end", "oncall@example.com", ""),
];

/// One email address and whether the modelled `EmailContactAddress` pattern admits it.
struct Case {
    /// What this row varies, named in every assertion.
    what: String,
    address: String,
    /// `true` when the modelled `EmailContactAddress` constraints admit `address`: its
    /// `@pattern` `^(.+)@(.+)$` read as ECMA-262, and its 6..254 `@length`.
    admitted: bool,
}

fn case(what: impl Into<String>, address: impl Into<String>, admitted: bool) -> Case {
    Case {
        what: what.into(),
        address: address.into(),
        admitted,
    }
}

/// An address of exactly `characters` characters whose shape the pattern admits: a local
/// part of ASCII letters, the separator, and a domain. Used for the rows at the LENGTH
/// bounds, so only rows long enough to carry that domain.
fn address_of_length(characters: usize) -> String {
    let domain = "@example.com";
    let local = characters
        .checked_sub(domain.chars().count())
        .unwrap_or_else(|| panic!("a {characters}-character row must have room for {domain:?}"));
    format!("{}{domain}", "a".repeat(local))
}

/// Every row is one email address, the rows differing along what the pattern generalises
/// over: length (at each bound of the member's own 6..254 and one past each), which
/// characters the value carries (`.` is every scalar value but a line terminator, so
/// spaces, tabs, punctuation and non-ASCII text are all admitted), how many separators it
/// carries and WHERE they sit (the expression wants at least one character on each side
/// of one of them), and which line terminator sits where.
///
/// The rows deliberately include entries the issue did not cite: a value of nothing but
/// separators is ADMITTED, because `.` matches `@` too, so `(.+)@(.+)` is satisfied by the
/// second one; and a tab is admitted, because a tab is not a line terminator.
fn address_table() -> Vec<Case> {
    [
        case("an ordinary address", "oncall@example.com", true),
        case(
            "the shortest address the modelled length admits",
            // Exactly MIN_ADDRESS_LEN characters: one before the separator, four after.
            "a@b.co",
            true,
        ),
        case(
            "an address at the maximum modelled length",
            address_of_length(MAX_ADDRESS_LEN),
            true,
        ),
        case(
            "an address at the maximum length in multi-byte characters",
            format!(
                "{}@example.com",
                "é".repeat(MAX_ADDRESS_LEN - "@example.com".chars().count())
            ),
            true,
        ),
        case("a domain with no dot in it", "oncall@localhost", true),
        case("several separators", "a@b@example.com", true),
        case(
            "nothing but separators, which the dot also matches",
            SEPARATOR.to_string().repeat(MIN_ADDRESS_LEN),
            true,
        ),
        case("a local part carrying a space", "on call@example.com", true),
        case("a local part carrying a tab", "on\tcall@example.com", true),
        case("non-ascii characters", "équipe@example.com", true),
        case(
            "punctuation the dot admits",
            "on+call_1.2-3~x@example.com",
            true,
        ),
        case("no separator at all", "oncall.example.com", false),
        case(
            "a separator with nothing before it",
            format!("{SEPARATOR}example.com"),
            false,
        ),
        case(
            "a separator with nothing after it",
            format!("oncall.example{SEPARATOR}"),
            false,
        ),
        // The two length rows: the expression itself admits both, and the member's own
        // `@length` is what refuses them, one character outside each published bound.
        case("one character below the minimum length", "a@b.c", false),
        case(
            "one character past the maximum length",
            address_of_length(MAX_ADDRESS_LEN + 1),
            false,
        ),
    ]
    .into_iter()
    // The cross product of every line terminator the pattern's `.` excludes with every
    // position it can sit at: the expression admits none of them anywhere, so neither the
    // character nor its position is what decides.
    .chain(
        LINE_TERMINATORS
            .into_iter()
            .flat_map(|(character, terminator)| {
                TERMINATOR_PLACEMENTS
                    .into_iter()
                    .map(move |(placement, head, tail)| {
                        case(
                            format!("{character} {placement}"),
                            format!("{head}{terminator}{tail}"),
                            false,
                        )
                    })
            }),
    )
    .collect()
}

/// An address short enough to name in an assertion message, with every line terminator
/// escaped so a failure prints one line.
fn shown(address: &str) -> String {
    let head: String = address.chars().take(32).collect();
    if head.chars().count() == address.chars().count() {
        format!("{head:?}")
    } else {
        format!("{head:?}… [{} characters]", address.chars().count())
    }
}

/// Whether `address` satisfies the modelled pattern, decided here INDEPENDENTLY of the
/// service's own predicate: no character is one the ECMA-262 `.` excludes, and some
/// separator has at least one character on each side of it.
fn pattern_admits(address: &str) -> bool {
    if address
        .chars()
        .any(|c| LINE_TERMINATORS.iter().any(|(_, excluded)| c == *excluded))
    {
        return false;
    }
    let characters = address.chars().count();
    address
        .chars()
        .enumerate()
        .any(|(position, c)| c == SEPARATOR && position > 0 && position + 1 < characters)
}

/// How many email contacts the store currently holds, read through `ListEmailContacts`.
async fn contact_count(em: &Emulator) -> usize {
    let (status, body) = list(em, "").await;
    assert_eq!(
        status, 200,
        "ListEmailContacts must answer 200, got {status} / {body:?}"
    );
    body["emailContacts"]
        .as_array()
        .unwrap_or_else(|| panic!("emailContacts is @required, got {body:?}"))
        .len()
}

#[tokio::test]
async fn create_email_contact_applies_the_modelled_email_address_pattern() {
    let table = address_table();
    assert!(
        table.len() >= 5,
        "the address table must span the class, got {} rows",
        table.len()
    );

    // One account per row: the published per-account email-contact quota bounds how many
    // contacts one account may HOLD at a time, and these rows are about the address a
    // request carries rather than about that quota.
    for case in &table {
        let em = emulator();
        let (status, body) = create(
            &em,
            &json!({ "name": "oncall", "emailAddress": case.address }),
        )
        .await;

        if case.admitted {
            assert_eq!(
                status,
                201,
                "case {:?} ({}): an address the modelled pattern admits must be created, got \
                 {status} / {body:?}",
                case.what,
                shown(&case.address)
            );
            let arn = body
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {:?}: expected an arn, got {body:?}", case.what));
            // Round trip: what the create stored is what the reader reports.
            let contact = get_ok(&em, arn).await;
            assert_eq!(
                contact.get("address").and_then(Value::as_str),
                Some(case.address.as_str()),
                "case {:?} ({}): GetEmailContact must report the stored address, got {contact:?}",
                case.what,
                shown(&case.address)
            );
        } else {
            assert_eq!(
                status,
                400,
                "case {:?} ({}): an address outside the modelled pattern must be refused, got \
                 {status} / {body:?}",
                case.what,
                shown(&case.address)
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {:?} ({}): expected ValidationException, got {body:?}",
                case.what,
                shown(&case.address)
            );
            let held = contact_count(&em).await;
            assert_eq!(
                held,
                0,
                "case {:?} ({}): a refused create persists nothing, got {held} contacts",
                case.what,
                shown(&case.address)
            );
        }
    }
}

/// The refusal of a value the LENGTH admits and the pattern does not blames the member,
/// advertises the expression this service publishes for it and states no value — the one
/// wording every constraint refusal of this service carries, whether the walk applied the
/// constraint or a handler did. So the three line terminators the walk's own engine admits
/// are refused the same way the one it excludes is.
#[tokio::test]
async fn the_refusal_blames_the_member_and_states_the_modelled_expression() {
    for (character, terminator) in LINE_TERMINATORS {
        let em = emulator();
        let address = format!("on{terminator}call@example.com");
        assert!(
            (MIN_ADDRESS_LEN..=MAX_ADDRESS_LEN).contains(&address.chars().count()),
            "case {character:?}: the row must sit inside the modelled length, so only the \
             pattern can refuse it"
        );

        let (status, body) =
            create(&em, &json!({ "name": "oncall", "emailAddress": address })).await;
        assert_eq!(
            status, 400,
            "case {character:?}: expected 400, got {status} / {body:?}"
        );
        let stated = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {character:?}: expected a message, got {body:?}"));

        // The whole sentence, transcribed: the envelope, the member path and the
        // expression this service publishes for the member — the modelled one stripped of
        // the whole-value anchors it is written with — and no sign of the refused value.
        let expected = format!(
            "1 validation error detected: Value at 'emailAddress' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: {PUBLISHED_ADDRESS_PATTERN}"
        );
        assert_eq!(
            stated, expected,
            "case {character:?}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(&address),
            "case {character:?}: the refusal must not state the value it refused, got {stated:?}"
        );
    }
}

/// Whatever a caller got stored, every address this service REPORTS satisfies the modelled
/// expression `EmailContact$address` declares — which is the consequence the rule exists
/// for.
#[tokio::test]
async fn every_reported_address_satisfies_the_modelled_expression() {
    let em = emulator();
    let mut stored = 0usize;

    for case in address_table() {
        let (status, _) = create(
            &em,
            &json!({ "name": "oncall", "emailAddress": case.address }),
        )
        .await;
        if status == 201 {
            stored += 1;
        }
        if stored == 5 {
            break;
        }
    }
    assert!(
        stored > 0,
        "the table must contain addresses the pattern admits, got {stored} stored"
    );

    let (status, body) = list(&em, "").await;
    assert_eq!(
        status, 200,
        "ListEmailContacts must answer 200, got {status} / {body:?}"
    );
    let reported = body["emailContacts"]
        .as_array()
        .unwrap_or_else(|| panic!("emailContacts is @required, got {body:?}"))
        .clone();
    assert_eq!(
        reported.len(),
        stored,
        "every stored contact must be reported, got {reported:?}"
    );
    for contact in &reported {
        let address = contact
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("address is @required, got {contact:?}"));
        assert!(
            pattern_admits(address),
            "case \"a reported address\": every value reported through EmailContact$address \
             must satisfy {EMAIL_CONTACT_ADDRESS_PATTERN:?}, got {}",
            shown(address)
        );
    }
}

/// The expression these refusals state is the one the MODEL declares for the member they
/// blame, read off the generated input schema rather than transcribed.
#[test]
fn the_applied_address_expression_is_the_models_own() {
    let member = CREATE_EMAIL_CONTACT_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| member.member_name() == Some("emailAddress"))
        .expect("CreateEmailContactRequest declares an `emailAddress` member");
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
        Some(EMAIL_CONTACT_ADDRESS_PATTERN),
        "the expression these refusals state must be the one the model declares for \
         CreateEmailContactRequest$emailAddress, got {declared:?}"
    );
}
