// SPDX-License-Identifier: CC0-1.0

//! A session policy that is not a JSON policy document is refused with
//! `MalformedPolicyDocument` (HTTP 400), and the refusal DESCRIBES the document it was
//! given WHERE the document gives something to describe: "The error message describes
//! the specific error" (`MalformedPolicyDocumentException`, this service's model).
//!
//! The rules under test, over every operation that takes an inline session policy
//! (`AssumeRole` and `GetFederationToken`) — the wording follows the document's defect,
//! not the operation it was passed to:
//!
//! * a document whose parse stopped ON a character is reported with that character and
//!   its character code, what the parser expected in its place, and the line and column
//!   the failure sits at — so two documents that differ only in that character get two
//!   different messages;
//! * the reported position names the reported character: the column is that
//!   character's position on the reported line of the submitted document;
//! * a document that merely RAN OUT of input — it ends while a value, object, array or
//!   string is still open, so there is no offending character and nothing specific to
//!   describe — is refused with one fixed sentence, every part of it fixed text, with
//!   no character, no position and nothing else of the request in it;
//! * the refusal never echoes the submitted document back;
//! * a document that parses but is not a JSON object is refused naming the JSON type
//!   it carries instead;
//! * a document that IS a JSON object is accepted.
//!
//! Every call goes over the wire as the `application/x-www-form-urlencoded` awsQuery
//! POST a client makes, through the same router the emulator serves with.

use crate::sts_sessions::{call, element, emulator, member, LONG_TERM_KEY};

/// The role every `AssumeRole` row here assumes.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/demo";

/// The sentence a document that ran out of input is refused with, spelled out here
/// rather than read off the service.
const RAN_OUT_SENTENCE: &str = "Syntax errors in policy.";

/// The two operations that take an inline session policy, with the other parameters
/// each one requires.
fn policy_operations() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    vec![
        (
            "AssumeRole",
            vec![
                ("RoleArn", ROLE_ARN.to_string()),
                ("RoleSessionName", "probe".to_string()),
            ],
        ),
        ("GetFederationToken", vec![("Name", "bob".to_string())]),
    ]
}

/// An XML text node, decoded: a message carrying a control character arrives as the
/// numeric reference the envelope writes for it, and one carrying `&` or `<` arrives
/// escaped as XML requires of a text node. A quote character is not markup there and
/// arrives as itself (`refusal_message_bytes.rs` owns that rule), so resolving it here is
/// a no-op kept for a message this decoder is handed from anywhere else.
pub(crate) fn decode_xml_text(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#x9;", "\t")
        .replace("&#xA;", "\n")
        .replace("&#xD;", "\r")
        .replace("&amp;", "&")
}

/// The `line: <l>, column: <c>` the refusal reports, or a panic naming the case.
fn reported_position(case: &str, message: &str) -> (usize, usize) {
    let marker = "line: ";
    let start = message
        .find(marker)
        .unwrap_or_else(|| panic!("case {case:?}: no reported line in {message:?}"))
        + marker.len();
    let rest = &message[start..];
    let (line, rest) = rest
        .split_once(", column: ")
        .unwrap_or_else(|| panic!("case {case:?}: no reported column in {message:?}"));
    let column: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (
        line.parse()
            .unwrap_or_else(|e| panic!("case {case:?}: line {line:?} in {message:?}: {e}")),
        column
            .parse()
            .unwrap_or_else(|e| panic!("case {case:?}: column {column:?} in {message:?}: {e}")),
    )
}

/// The character a reported position names: the `column`th byte of the `line`th line
/// of the document, both counted from one. Computed here from the submitted document
/// alone, so the test checks the position against the document rather than against
/// the service's own bookkeeping.
fn character_at(document: &str, line: usize, column: usize) -> Option<char> {
    let target = document.split('\n').nth(line.checked_sub(1)?)?;
    target.get(column.checked_sub(1)?..)?.chars().next()
}

/// The way a message must name a character: itself when it is printable, its escape
/// when it is a control character that would otherwise be invisible.
fn named_character(offender: char) -> String {
    let rendered = if offender.is_control() {
        offender.escape_default().to_string()
    } else {
        offender.to_string()
    };
    format!("('{rendered}' (code {}))", u32::from(offender))
}

/// One malformed-document row: the document, the character the parser must stop on, its
/// position, and a fragment of what the message must say was expected there.
struct Malformed {
    case: &'static str,
    policy: &'static str,
    offender: char,
    line: usize,
    column: usize,
    expected: &'static str,
}

/// The rows span what the rule generalises over: which character offends, where in
/// the line it sits, which line it sits on, and what the parser wanted there.
fn documents_with_an_offending_character() -> Vec<Malformed> {
    vec![
        Malformed {
            case: "a field name that is not quoted",
            policy: "{this is not valid json",
            offender: 't',
            line: 1,
            column: 2,
            expected: "double-quote to start field name",
        },
        Malformed {
            case: "the same, offending on another character",
            policy: "{not valid json",
            offender: 'n',
            line: 1,
            column: 2,
            expected: "double-quote to start field name",
        },
        Malformed {
            case: "nothing that can start a value",
            policy: "xyz",
            offender: 'x',
            line: 1,
            column: 1,
            expected: "valid value",
        },
        Malformed {
            case: "a trailing comma before the close",
            policy: r#"{"Version":"2012-10-17",}"#,
            offender: '}',
            line: 1,
            column: 25,
            expected: "double-quote to start field name",
        },
        Malformed {
            case: "a missing colon, on the second line",
            policy: "{\n\"Version\" \"2012-10-17\"\n}",
            offender: '"',
            line: 2,
            column: 11,
            expected: "colon",
        },
        Malformed {
            case: "two array elements with no comma",
            policy: r#"{"Statement":[1 2]}"#,
            offender: '2',
            line: 1,
            column: 17,
            expected: "comma to separate Array entries",
        },
        Malformed {
            case: "a trailing comma in an array",
            policy: r#"{"Statement":[1,]}"#,
            offender: ']',
            line: 1,
            column: 17,
            expected: "valid value",
        },
        Malformed {
            case: "a raw control character inside a string",
            policy: "{\"Version\":\"a\tb\"}",
            offender: '\t',
            line: 1,
            column: 14,
            expected: "control character",
        },
        Malformed {
            case: "a non-ASCII character where a field name belongs",
            // The member admits \u0020-\u00FF (unrestrictedSessionPolicyDocumentType's
            // pattern, this service's model), so a Latin-1 character can offend.
            policy: "{\u{e9}tat:1}",
            offender: '\u{e9}',
            line: 1,
            column: 2,
            expected: "double-quote to start field name",
        },
    ]
}

/// The documents that merely RAN OUT of input: the rows differ in WHAT was still open
/// when the input ended (an object, an array, a string, a value after its colon), in
/// how far into the document that is, and in which line it ends on.
fn documents_that_ran_out() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a document that stops mid-object", r#"{"Version":"2012-10-17""#),
        ("a document that stops inside an array", r#"{"Statement":[{"Effect":"Allow""#),
        ("a document that stops inside a string", r#"{"Version":"2012-1"#),
        ("a document that stops after a colon", r#"{"Version":"#),
        ("a document that is a single open brace", "{"),
        // The boundary of the class: a document with no value in it at all ran out
        // before anything opened. (The member's own `@length` minimum of 1 is what
        // refuses a document of zero characters, before any handler runs, so the
        // shortest document that reaches this rule is one character of whitespace.)
        ("a document carrying no value at all", " "),
        ("a document that stops on its second line", "{\n\"Statement\":["),
    ]
}

/// A document whose parse stopped on a character is refused with a message built from
/// THAT document: the offending character and its code, what was expected there, and
/// the line and column it sits at.
#[tokio::test]
async fn an_unparseable_session_policy_is_refused_describing_the_parse_failure() {
    let (router, engine) = emulator();
    // `sts:AssumeRole` resolves its role in IAM and evaluates the role's trust policy,
    // so the role the rows below name has to be one this account holds and admits the
    // caller.
    let (_role_arn, _role_id) = super::sts_sessions::seed_demo_role(&engine);

    for (action, base_params) in policy_operations() {
        let mut seen: Vec<(&str, String)> = Vec::new();
        for row in documents_with_an_offending_character() {
            let case = format!("{action}: {}", row.case);
            let mut params = base_params.clone();
            params.push(("Policy", row.policy.to_string()));
            let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("MalformedPolicyDocument"),
                "case {case:?}: expected a MalformedPolicyDocument, got {body}"
            );
            let message = decode_xml_text(&member(&case, &body, "Message"));

            let (line, column) = reported_position(&case, &message);
            assert_eq!(
                (line, column),
                (row.line, row.column),
                "case {case:?}: expected the failure at line {}, column {}, got line {line}, \
                 column {column} in {message:?}",
                row.line,
                row.column
            );
            let named = named_character(row.offender);
            assert!(
                message.contains(&named),
                "case {case:?}: expected the message to name {named}, got {message:?}"
            );
            assert_eq!(
                character_at(row.policy, line, column),
                Some(row.offender),
                "case {case:?}: the reported position must name the reported character, got \
                 {message:?}"
            );
            assert!(
                message.contains(row.expected),
                "case {case:?}: expected the message to say what was expected ({:?}), got \
                 {message:?}",
                row.expected
            );
            assert!(
                !message.contains(row.policy),
                "case {case:?}: the refusal must not echo the submitted document, got \
                 {message:?}"
            );
            if let Some((earlier, _)) = seen
                .iter()
                .find(|(_, earlier_message)| *earlier_message == message)
            {
                panic!(
                    "case {case:?}: two different documents got the same message as \
                     {earlier:?}: {message:?}"
                );
            }
            seen.push((row.case, message));
        }
    }
}

/// A document that ran out of input is refused with exactly one fixed sentence,
/// whatever ran out and wherever it did: no offending character, no position, and
/// nothing else of the request in the message.
#[tokio::test]
async fn a_session_policy_that_ran_out_of_input_is_refused_with_one_fixed_sentence() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = super::sts_sessions::seed_demo_role(&engine);

    for (action, base_params) in policy_operations() {
        for (row, policy) in documents_that_ran_out() {
            let case = format!("{action}: {row}");
            let mut params = base_params.clone();
            params.push(("Policy", policy.to_string()));
            let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;

            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                element(&body, "Code").as_deref(),
                Some("MalformedPolicyDocument"),
                "case {case:?}: expected a MalformedPolicyDocument, got {body}"
            );
            let message = decode_xml_text(&member(&case, &body, "Message"));
            assert_eq!(
                message, RAN_OUT_SENTENCE,
                "case {case:?}: expected exactly {RAN_OUT_SENTENCE:?}, got {message:?}"
            );
        }
    }
}

/// A document that parses but is not a JSON object is refused naming the JSON type
/// it carries; a document that IS a JSON object is accepted.
#[tokio::test]
async fn a_session_policy_that_is_not_a_json_object_is_refused_naming_its_type() {
    let (router, engine) = emulator();
    let (_role_arn, _role_id) = super::sts_sessions::seed_demo_role(&engine);
    // One row per JSON type a document can carry, plus the object AWS requires.
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("[1,2,3]", Some("a JSON array")),
        (r#""a string""#, Some("a JSON string")),
        ("42", Some("a JSON number")),
        ("true", Some("a JSON boolean")),
        ("null", Some("null")),
        (r#"{"Version":"2012-10-17","Statement":[]}"#, None),
    ];

    for (action, base_params) in policy_operations() {
        for (policy, expected_type) in &cases {
            let case = format!("{action}: {policy}");
            let mut params = base_params.clone();
            params.push(("Policy", (*policy).to_string()));
            let (status, body) = call(&router, LONG_TERM_KEY, action, &params).await;

            match expected_type {
                Some(expected_type) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected 400, got {status} {body}"
                    );
                    assert_eq!(
                        element(&body, "Code").as_deref(),
                        Some("MalformedPolicyDocument"),
                        "case {case:?}: expected a MalformedPolicyDocument, got {body}"
                    );
                    let message = decode_xml_text(&member(&case, &body, "Message"));
                    assert!(
                        message.contains(expected_type),
                        "case {case:?}: expected the message to name {expected_type:?}, got \
                         {message:?}"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected a JSON object policy to be accepted, got \
                         {status} {body}"
                    );
                    assert!(
                        element(&body, "Code").is_none(),
                        "case {case:?}: expected no error, got {body}"
                    );
                }
            }
        }
    }
}
