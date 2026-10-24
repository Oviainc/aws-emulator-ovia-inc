// SPDX-License-Identifier: CC0-1.0

//! A session policy that is not a JSON policy document is refused with
//! `MalformedPolicyDocument` (HTTP 400), and the refusal DESCRIBES the document it
//! was given: "The error message describes the specific error"
//! (`MalformedPolicyDocumentException`, this service's model).
//!
//! The rules under test, over every operation that takes an inline session policy
//! (`AssumeRole` and `GetFederationToken`):
//!
//! * a document that does not parse is reported with the first offending character
//!   and its character code, what the parser expected in its place, and the line and
//!   column the failure sits at — so two documents that differ only in that
//!   character get two different messages;
//! * the reported position names the reported character: the column is that
//!   character's position on the reported line of the submitted document;
//! * a document that ends mid-parse has no offending character, so it is reported as
//!   an unexpected end of input, still with the position;
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

/// An XML text node, decoded: the awsQuery envelope escapes the five XML entities
/// and emits tab/LF/CR as numeric references, so a message carrying a quote or a
/// newline arrives encoded.
fn decode_xml_text(text: &str) -> String {
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

/// One malformed-document row: the document, the character the parser must stop on
/// (`None` when the document ends mid-parse), its position, and a fragment of what
/// the message must say was expected there.
struct Malformed {
    case: &'static str,
    policy: &'static str,
    offender: Option<char>,
    line: usize,
    column: usize,
    expected: &'static str,
}

/// The rows span what the rule generalises over: which character offends, where in
/// the line it sits, which line it sits on, what the parser wanted there, and
/// whether there is a character at all.
fn malformed_documents() -> Vec<Malformed> {
    vec![
        Malformed {
            case: "a field name that is not quoted",
            policy: "{this is not valid json",
            offender: Some('t'),
            line: 1,
            column: 2,
            expected: "double-quote to start field name",
        },
        Malformed {
            case: "the same, offending on another character",
            policy: "{not valid json",
            offender: Some('n'),
            line: 1,
            column: 2,
            expected: "double-quote to start field name",
        },
        Malformed {
            case: "nothing that can start a value",
            policy: "xyz",
            offender: Some('x'),
            line: 1,
            column: 1,
            expected: "valid value",
        },
        Malformed {
            case: "a trailing comma before the close",
            policy: r#"{"Version":"2012-10-17",}"#,
            offender: Some('}'),
            line: 1,
            column: 25,
            expected: "double-quote to start field name",
        },
        Malformed {
            case: "a missing colon, on the second line",
            policy: "{\n\"Version\" \"2012-10-17\"\n}",
            offender: Some('"'),
            line: 2,
            column: 11,
            expected: "colon",
        },
        Malformed {
            case: "two array elements with no comma",
            policy: r#"{"Statement":[1 2]}"#,
            offender: Some('2'),
            line: 1,
            column: 17,
            expected: "comma to separate Array entries",
        },
        Malformed {
            case: "a trailing comma in an array",
            policy: r#"{"Statement":[1,]}"#,
            offender: Some(']'),
            line: 1,
            column: 17,
            expected: "valid value",
        },
        Malformed {
            case: "a raw control character inside a string",
            policy: "{\"Version\":\"a\tb\"}",
            offender: Some('\t'),
            line: 1,
            column: 14,
            expected: "control character",
        },
        Malformed {
            case: "a document that stops mid-object",
            policy: r#"{"Version":"2012-10-17""#,
            offender: None,
            line: 1,
            column: 23,
            expected: "Object",
        },
        Malformed {
            case: "a non-ASCII character where a field name belongs",
            // The member admits \u0020-\u00FF (unrestrictedSessionPolicyDocumentType's
            // pattern, this service's model), so a Latin-1 character can offend.
            policy: "{\u{e9}tat:1}",
            offender: Some('\u{e9}'),
            line: 1,
            column: 2,
            expected: "double-quote to start field name",
        },
    ]
}

/// A document that does not parse is refused with a message built from THAT
/// document: the offending character and its code, what was expected there, and the
/// line and column it sits at.
#[tokio::test]
async fn an_unparseable_session_policy_is_refused_describing_the_parse_failure() {
    let (router, _engine) = emulator();

    for (action, base_params) in policy_operations() {
        let mut seen: Vec<(&str, String)> = Vec::new();
        for row in malformed_documents() {
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
            match row.offender {
                Some(offender) => {
                    let named = named_character(offender);
                    assert!(
                        message.contains(&named),
                        "case {case:?}: expected the message to name {named}, got {message:?}"
                    );
                    assert_eq!(
                        character_at(row.policy, line, column),
                        Some(offender),
                        "case {case:?}: the reported position must name the reported \
                         character, got {message:?}"
                    );
                }
                None => assert!(
                    message.contains("end-of-input"),
                    "case {case:?}: expected an end-of-input diagnosis, got {message:?}"
                ),
            }
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

/// A document that parses but is not a JSON object is refused naming the JSON type
/// it carries; a document that IS a JSON object is accepted.
#[tokio::test]
async fn a_session_policy_that_is_not_a_json_object_is_refused_naming_its_type() {
    let (router, _engine) = emulator();
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
