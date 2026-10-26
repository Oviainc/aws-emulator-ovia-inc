// SPDX-License-Identifier: CC0-1.0

//! The modelled bound of a refusal message, over the awsJson1_1 wire.
//!
//! RULE: `ValidationException$Message` is `@length(0, 1024)`
//! (`smithy_query(query="shape",shape="ValidationException")`), so a refusal this service
//! builds from a LIST the request carried states as many of that list's values as the
//! bound admits and reports the rest as a count — the message never grows with the size of
//! the request. The lists are caller-sized: `ResourceTagKeyList` and `ResourceTagList` are
//! `@length(0, 200)` over values of `@length(1, 128)` characters
//! (`smithy_query(query="shape",shape="ResourceTagKeyList,ResourceTagList,ResourceTagKey")`),
//! so quoting every value of one would run to tens of thousands of characters, and a
//! refused request is likewise stated clause by clause, one clause per offending value.
//!
//! Both paths that build a message from such a list are asserted here:
//!
//! * the handler-side refusal of the reserved `aws:` tag-key namespace, which every
//!   tag-accepting operation reaches — `UntagResource`'s `ResourceTagKeys`,
//!   `TagResource`'s and `CreateExport`'s `ResourceTags`;
//! * the boundary refusal of a request whose values miss their modelled constraints, which
//!   states one clause per violation.
//!
//! The rows differ along what the rule generalises over: how many values the list carries
//! (one, a handful, the published ceiling), how long each is (a few characters, the
//! published key ceiling, past it), and how wide its characters are (ASCII against
//! multi-byte text, where the characters the model counts and the bytes the body carries
//! come apart) — never along the identity of a value.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    MAX_RESOURCE_TAGS, RESERVED_TAG_KEY_PREFIX,
};

use crate::export_lifecycle::{cases, error_code, Client};
use crate::tagging::{listed_tags, untagged_export};

/// The most characters `ValidationException$Message` admits (smithy_query shape
/// `ValidationException`, member `Message`: `@length(min: 0, max: 1024)`).
const MESSAGE_MAX_CHARS: usize = 1024;

/// The modelled `@length` ceiling of a tag key (smithy_query shape `ResourceTagKey`:
/// `@length(min: 1, max: 128)`).
const MAX_TAG_KEY_CHARS: usize = 128;

/// A multi-byte character, so a row can hold the characters the model counts while
/// quadrupling the bytes the message carries: U+1D11E is four bytes of UTF-8.
const WIDE_CHAR: char = '𝄞';

/// A reserved tag key `chars` characters long, distinguished from every other key of the
/// same row by `index` and padded with `filler`.
///
/// Every key of a row has the same length and a distinct three-digit index, so no key is a
/// substring of another and a test can ask which keys a message quotes.
fn reserved_key(index: usize, chars: usize, filler: char) -> String {
    let head = format!("{RESERVED_TAG_KEY_PREFIX}{index:03}");
    let tail = chars.saturating_sub(head.chars().count());
    format!("{head}{}", filler.to_string().repeat(tail))
}

/// One row of the reserved-namespace table: how many keys, how long, and of what
/// characters.
struct Row {
    label: &'static str,
    count: usize,
    chars: usize,
    filler: char,
}

/// The reserved-namespace rows: the list size and the value size the rule generalises
/// over, from one short key to the published ceiling of both.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "one short key",
            count: 1,
            chars: 8,
            filler: 'k',
        },
        Row {
            label: "two short keys",
            count: 2,
            chars: 8,
            filler: 'k',
        },
        Row {
            // Nine keys at the key ceiling is where the quoted list first passes 1024
            // characters.
            label: "nine keys at the key-length ceiling",
            count: 9,
            chars: MAX_TAG_KEY_CHARS,
            filler: 'k',
        },
        Row {
            label: "the whole list of short keys",
            count: MAX_RESOURCE_TAGS,
            chars: 8,
            filler: 'k',
        },
        Row {
            label: "the whole list at the key-length ceiling",
            count: MAX_RESOURCE_TAGS,
            chars: MAX_TAG_KEY_CHARS,
            filler: 'k',
        },
        Row {
            label: "one multi-byte key at the key-length ceiling",
            count: 1,
            chars: MAX_TAG_KEY_CHARS,
            filler: WIDE_CHAR,
        },
        Row {
            label: "the whole list of multi-byte keys at the key-length ceiling",
            count: MAX_RESOURCE_TAGS,
            chars: MAX_TAG_KEY_CHARS,
            filler: WIDE_CHAR,
        },
    ]
}

impl Row {
    /// The keys this row sends.
    fn keys(&self) -> Vec<String> {
        (0..self.count)
            .map(|index| reserved_key(index, self.chars, self.filler))
            .collect()
    }
}

/// The count a refusal states for the values it quoted none of — `and <n> more` — or 0
/// when it quoted them all.
fn unquoted_count(message: &str) -> usize {
    message
        .rsplit_once("and ")
        .and_then(|(_, tail)| tail.trim_end_matches('.').strip_suffix(" more"))
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

/// The `Message` of a refusal, asserted to satisfy the modelled bound.
fn bounded_message(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message: {body}"))
        .to_string();
    assert!(
        !message.is_empty() && message.chars().count() <= MESSAGE_MAX_CHARS,
        "case {case:?}: Message must satisfy the modelled @length(0, {MESSAGE_MAX_CHARS}), got {} characters: {message:?}",
        message.chars().count()
    );
    // Every other string of the refusal carries the same modelled bound.
    if let Some(fields) = body["Fields"].as_array() {
        for field in fields {
            for member in ["Name", "Message"] {
                let text = field[member].as_str().unwrap_or_default();
                assert!(
                    text.chars().count() <= MESSAGE_MAX_CHARS,
                    "case {case:?}: Fields[].{member} must satisfy the modelled @length(0, {MESSAGE_MAX_CHARS}), got {} characters",
                    text.chars().count()
                );
            }
        }
    }
    message
}

/// Assert the refusal of `keys` names the member at fault, stays inside the modelled
/// bound, quotes the leading keys of the list and accounts for every key it did not quote.
fn assert_bounded_refusal(case: &str, member: &str, keys: &[String], status: u16, body: &Value) {
    let message = bounded_message(case, status, body);
    let names_member = body["Fields"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| field["Name"] == json!(member)));
    assert!(
        names_member,
        "case {case:?}: the refusal must name {member} among its Fields, got {body}"
    );

    let quoted = keys.iter().filter(|key| message.contains(*key)).count();
    assert!(
        quoted >= 1,
        "case {case:?}: the refusal must quote at least one offending key, got {message:?}"
    );
    assert_eq!(
        quoted + unquoted_count(&message),
        keys.len(),
        "case {case:?}: the refusal must account for all {} keys, quoting {quoted} and stating {} unquoted: {message:?}",
        keys.len(),
        unquoted_count(&message)
    );
}

/// The `CreateExport` request of the first lifecycle row, named `name` and carrying `tags`.
fn create_request(name: &str, tags: Value) -> Value {
    let mut case = cases().remove(0);
    case.set_name(name);
    let mut request = case.create_request();
    request["ResourceTags"] = tags;
    request
}

/// `keys` as a `ResourceTags` list, every pair carrying the same ordinary value.
fn tag_list(keys: &[String]) -> Value {
    Value::Array(
        keys.iter()
            .map(|key| json!({ "Key": key, "Value": "x" }))
            .collect(),
    )
}

#[tokio::test]
async fn a_reserved_key_refusal_stays_inside_the_modelled_message_bound() {
    let client = Client::new();
    let arn = untagged_export(&client, "bounded_reserved_refusal").await;
    client
        .ok(
            "TagResource",
            &json!({
                "ResourceArn": arn,
                "ResourceTags": [{ "Key": "team", "Value": "billing" }],
            }),
        )
        .await;
    let before = listed_tags(&client, "before any refusal", &arn).await;

    for (index, row) in rows().into_iter().enumerate() {
        let keys = row.keys();

        let case = format!("{} via UntagResource", row.label);
        let (status, body) = client
            .call(
                "UntagResource",
                &json!({ "ResourceArn": arn, "ResourceTagKeys": keys }),
            )
            .await;
        assert_bounded_refusal(&case, "ResourceTagKeys", &keys, status, &body);

        let case = format!("{} via TagResource", row.label);
        let (status, body) = client
            .call(
                "TagResource",
                &json!({ "ResourceArn": arn, "ResourceTags": tag_list(&keys) }),
            )
            .await;
        assert_bounded_refusal(&case, "ResourceTags", &keys, status, &body);

        let case = format!("{} via CreateExport", row.label);
        let (status, body) = client
            .call(
                "CreateExport",
                &create_request(&format!("bounded_create_{index}"), tag_list(&keys)),
            )
            .await;
        assert_bounded_refusal(&case, "ResourceTags", &keys, status, &body);

        // A refusal is still a refusal: nothing of the request was applied.
        let after = listed_tags(&client, row.label, &arn).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused call must leave the tag set as it was, got {after:?}",
            row.label
        );
    }
}

/// How many clauses a boundary envelope spells: every clause states one constraint a value
/// failed, so the phrase occurs once per clause.
fn stated_clauses(message: &str) -> usize {
    message.matches("failed to satisfy constraint").count()
}

/// The violation count a boundary envelope opens with (`<n> validation error[s]
/// detected:`).
fn stated_violations(case: &str, message: &str) -> usize {
    let count = message
        .split_whitespace()
        .next()
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the envelope must open with a violation count: {message:?}")
        });
    let expected = if count == 1 {
        format!("{count} validation error detected: ")
    } else {
        format!("{count} validation errors detected: ")
    };
    assert!(
        message.starts_with(&expected),
        "case {case:?}: expected the envelope to open {expected:?}, got {message:?}"
    );
    count
}

#[tokio::test]
async fn a_boundary_refusal_stays_inside_the_modelled_message_bound() {
    let client = Client::new();
    let arn = untagged_export(&client, "bounded_boundary_refusal").await;

    // Each row misses a modelled constraint in a different way and at a different size:
    // one over-long value, many over-long values, many values below the minimum length, a
    // list past its own length bound, and a single value far past the framework's echo cap.
    let rows: Vec<(String, Value)> = vec![
        (
            "one key past the key-length ceiling".to_string(),
            json!([ "k".repeat(MAX_TAG_KEY_CHARS + 1) ]),
        ),
        (
            "the whole list past the key-length ceiling".to_string(),
            Value::Array(
                (0..MAX_RESOURCE_TAGS)
                    .map(|index| json!(format!("{index:03}{}", "k".repeat(300))))
                    .collect(),
            ),
        ),
        (
            "the whole list below the key-length floor".to_string(),
            Value::Array((0..MAX_RESOURCE_TAGS).map(|_| json!("")).collect()),
        ),
        (
            "one list past the list-length ceiling".to_string(),
            Value::Array(
                (0..MAX_RESOURCE_TAGS + 1)
                    .map(|index| json!(format!("key-{index:03}")))
                    .collect(),
            ),
        ),
        (
            "one key far past the framework's echo cap".to_string(),
            json!([ "k".repeat(20_000) ]),
        ),
    ];

    for (case, keys) in rows {
        let (status, body) = client
            .call(
                "UntagResource",
                &json!({ "ResourceArn": arn, "ResourceTagKeys": keys }),
            )
            .await;
        let message = bounded_message(&case, status, &body);
        let stated = stated_violations(&case, &message);
        assert!(
            stated_clauses(&message) >= 1,
            "case {case:?}: the envelope must spell at least one clause, got {message:?}"
        );
        assert_eq!(
            stated_clauses(&message) + unquoted_count(&message),
            stated,
            "case {case:?}: the envelope must account for all {stated} violations, spelling {} clauses and stating {} unspelled: {message:?}",
            stated_clauses(&message),
            unquoted_count(&message)
        );
    }
}
