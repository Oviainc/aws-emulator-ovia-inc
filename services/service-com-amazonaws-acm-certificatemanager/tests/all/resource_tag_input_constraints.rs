// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the INPUT SHAPES of ACM's generic resource-tagging operations
//! (`TagResource`, `UntagResource`, `ListTagsForResource`) over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: a request whose members fall outside the shapes the model declares for them is
//! refused with HTTP 400 `ValidationException` — the only input error all three
//! operations model — and persists nothing, while a request at the published bound of
//! each of those shapes is served. The shapes, from
//! `smithy_query(query="operation", operation="TagResource,UntagResource,ListTagsForResource")`
//! and `smithy_query(query="shape", shape="com.amazonaws.acm#TagKey,com.amazonaws.acm#TagValue,com.amazonaws.acm#TagList")`:
//!
//! * `ResourceArn` (`Arn`): `len 20..2048`, pattern
//!   `^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`;
//! * `Tags` (`TagList`) and `TagKeys` (`TagKeyList`): `len 1..50`;
//! * `TagKey`: `len 1..128`, pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`;
//! * `TagValue`: `len ..256`, same pattern.
//!
//! This suite is the NEGATIVE and BOUNDARY half of the tagging rule whose positive half
//! lives in [`crate::resource_tagging`]: that suite pins what the stored map does with
//! pairs the shapes admit, and this one pins that a pair they do not admit never reaches
//! the store at all. The two share [`crate::resource_tagging`]'s wire harness, and every
//! "nothing was persisted" assertion reads the resource back through
//! `ListTagsForResource`, a different operation than the refused write.
//!
//! The tables below span the classes each shape generalises over — every punctuation and
//! control character class the tag `@pattern` excludes (and a separator and punctuation
//! set it admits), each length bound at the bound itself and one past it, and an
//! `ResourceArn` that is too short, too long, not an `acm` ARN at all, or carries a
//! character the ARN pattern excludes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{call, error_code, error_message, router, ACCOUNT, REGION};
use crate::resource_tagging::{create_endpoint, expect, stored_tags, tag_resource, TagCase};

/// `TagKey`'s modelled length bounds
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagKey")`: `len 1..128`).
const TAG_KEY_MIN: usize = 1;
const TAG_KEY_MAX: usize = 128;

/// `TagValue`'s modelled maximum length
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#TagValue")`: `len ..256`).
const TAG_VALUE_MAX: usize = 256;

/// `TagList`/`TagKeyList`'s modelled maximum length (`len 1..50`).
const TAG_LIST_MAX: usize = 50;

/// `Arn`'s modelled length bounds (`len 20..2048`).
const ARN_MIN: usize = 20;
const ARN_MAX: usize = 2048;

/// The shortest prefix an `acm` ARN matching the modelled `@pattern` can have: the
/// Region is optional (`[\w+=/,.@-]*`) and the account is any digit run (`[0-9]+`), so
/// what is left to vary is the resource segment's length.
const SHORT_ARN_PREFIX: &str = "arn:aws:acm::1:";

/// A pattern-matching ACM ARN of exactly `len` characters, used to walk the `Arn`
/// `@length` bound. The padding goes in the resource segment, whose own character class
/// (`[\w+=,.@-]+`) admits `x`.
fn arn_of_length(len: usize) -> String {
    let padding = len.saturating_sub(SHORT_ARN_PREFIX.len());
    format!("{SHORT_ARN_PREFIX}{}", "x".repeat(padding))
}

/// One refused request: what it exercises, the request members beside `ResourceArn`, and
/// the lowerCamel member path the refusal must name.
type RefusalCase = (String, Value, &'static str);

/// A `Tags` list of `count` distinct, legal pairs.
fn pairs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("key{index:02}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// A `TagKeys` list of `count` distinct, legal keys.
fn keys(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| Value::String(format!("key{index:02}")))
            .collect(),
    )
}

/// Every character class the tag `@pattern` `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` EXCLUDES:
/// it admits letters (`\p{L}`), separators (`\p{Z}`), numbers (`\p{N}`) and the
/// punctuation set `_ . : / = + - @`, so everything below is outside it. Each entry is
/// (what it exercises, the offending text).
fn illegal_tag_texts() -> Vec<(&'static str, String)> {
    vec![
        ("a number sign", "env#prod".to_string()),
        ("a dollar sign", "env$prod".to_string()),
        ("a percent sign", "env%prod".to_string()),
        ("an ampersand", "env&prod".to_string()),
        ("an asterisk", "env*prod".to_string()),
        ("parentheses", "env(prod)".to_string()),
        ("a comma", "env,prod".to_string()),
        ("a semicolon", "env;prod".to_string()),
        ("an exclamation mark", "env!prod".to_string()),
        ("a question mark", "env?prod".to_string()),
        ("a double quote", "env\"prod".to_string()),
        ("a single quote", "env'prod".to_string()),
        ("a backslash", "env\\prod".to_string()),
        ("a vertical bar", "env|prod".to_string()),
        ("angle brackets", "env<prod>".to_string()),
        ("square brackets", "env[prod]".to_string()),
        ("curly braces", "env{prod}".to_string()),
        ("a caret", "env^prod".to_string()),
        ("a tilde", "env~prod".to_string()),
        ("a backtick", "env`prod".to_string()),
        // A control character is not a `\p{Z}` separator, so neither of these is admitted.
        ("a tab", "env\tprod".to_string()),
        ("a newline", "env\nprod".to_string()),
    ]
}

/// RULE: a `Tags` member outside the modelled `TagList`/`TagKey`/`TagValue` shapes is
/// refused with HTTP 400 `ValidationException` naming the member that failed, and the
/// resource's stored tags are left exactly as they were.
#[tokio::test]
async fn tag_resource_refuses_tags_outside_the_modelled_shapes() {
    let router = router();
    let case = "tag input shapes";
    let arn = create_endpoint(&router, case).await;
    // One legal pair first, so "nothing was persisted" is a claim about a NON-empty map:
    // a refusal that wiped the map would be caught too.
    tag_resource(&router, case, &arn, &json!([{ "Key": "env", "Value": "dev" }])).await;
    let before = expect(&[("env", "dev")]);

    let mut cases: Vec<RefusalCase> = vec![
        (
            "an empty Tags list, one short of the modelled minimum".to_string(),
            json!({ "Tags": [] }),
            "tags",
        ),
        (
            "one Tags entry past the modelled maximum".to_string(),
            json!({ "Tags": pairs(TAG_LIST_MAX + 1) }),
            "tags",
        ),
        (
            "an empty key, one short of TagKey's modelled minimum".to_string(),
            json!({ "Tags": [{ "Key": "", "Value": "x" }] }),
            "tags",
        ),
        (
            "a key one character past TagKey's modelled maximum".to_string(),
            json!({ "Tags": [{ "Key": "k".repeat(TAG_KEY_MAX + 1), "Value": "x" }] }),
            "tags",
        ),
        (
            "a value one character past TagValue's modelled maximum".to_string(),
            json!({ "Tags": [{ "Key": "k", "Value": "v".repeat(TAG_VALUE_MAX + 1) }] }),
            "tags",
        ),
    ];
    // Every excluded character class, on the key and on the value.
    for (what, text) in illegal_tag_texts() {
        cases.push((
            format!("{what} in the tag key"),
            json!({ "Tags": [{ "Key": text, "Value": "x" }] }),
            "tags",
        ));
        cases.push((
            format!("{what} in the tag value"),
            json!({ "Tags": [{ "Key": "k", "Value": text }] }),
            "tags",
        ));
    }

    for (row, members, path) in cases {
        let row = row.as_str();
        refusal(&router, row, "TagResource", &arn, &members, path).await;
        let after = stored_tags(&router, row, &arn).await;
        assert_eq!(
            after, before,
            "case {row:?}: a refused TagResource must leave the stored tags {before:?}, \
             got {after:?}"
        );
    }
}

/// RULE: the same shapes bound `UntagResource`'s `TagKeys`, whose entries are `TagKey`s
/// too — so a key outside them is refused with HTTP 400 `ValidationException` and
/// removes nothing.
#[tokio::test]
async fn untag_resource_refuses_tag_keys_outside_the_modelled_shapes() {
    let router = router();
    let case = "untag input shapes";
    let arn = create_endpoint(&router, case).await;
    tag_resource(
        &router,
        case,
        &arn,
        &json!([{ "Key": "env", "Value": "dev" }, { "Key": "team", "Value": "sre" }]),
    )
    .await;
    let before = expect(&[("env", "dev"), ("team", "sre")]);

    let mut cases: Vec<RefusalCase> = vec![
        (
            "an empty TagKeys list, one short of the modelled minimum".to_string(),
            json!({ "TagKeys": [] }),
            "tagKeys",
        ),
        (
            "one TagKeys entry past the modelled maximum".to_string(),
            json!({ "TagKeys": keys(TAG_LIST_MAX + 1) }),
            "tagKeys",
        ),
        (
            "an empty key, one short of TagKey's modelled minimum".to_string(),
            json!({ "TagKeys": [""] }),
            "tagKeys",
        ),
        (
            "a key one character past TagKey's modelled maximum".to_string(),
            json!({ "TagKeys": ["k".repeat(TAG_KEY_MAX + 1)] }),
            "tagKeys",
        ),
    ];
    for (what, text) in illegal_tag_texts() {
        cases.push((
            format!("{what} in a key to remove"),
            json!({ "TagKeys": [text] }),
            "tagKeys",
        ));
        cases.push((
            format!("{what} beside a key the resource carries"),
            json!({ "TagKeys": ["env", text] }),
            "tagKeys",
        ));
    }

    for (row, members, path) in cases {
        let row = row.as_str();
        refusal(&router, row, "UntagResource", &arn, &members, path).await;
        let after = stored_tags(&router, row, &arn).await;
        assert_eq!(
            after, before,
            "case {row:?}: a refused UntagResource must leave the stored tags {before:?}, \
             got {after:?}"
        );
    }
}

/// RULE: a `ResourceArn` outside the modelled `Arn` shape is refused by EVERY generic
/// tagging operation with HTTP 400 `ValidationException` — including the reporting one,
/// `ListTagsForResource` — before any resource is looked up; an ARN the shape admits but
/// no resource answers to is the different, `ResourceNotFoundException` case, which is
/// what the `@length` bound is walked with here.
#[tokio::test]
async fn generic_tagging_refuses_a_resource_arn_outside_the_modelled_shape() {
    let router = router();
    let case = "arn shapes";
    let stored = create_endpoint(&router, case).await;
    tag_resource(
        &router,
        case,
        &stored,
        &json!([{ "Key": "env", "Value": "dev" }]),
    )
    .await;
    let before = expect(&[("env", "dev")]);

    // Each generic operation and the request members it needs beside `ResourceArn`.
    let operations: Vec<(&str, Value)> = vec![
        ("TagResource", json!({ "Tags": pairs(1) })),
        ("UntagResource", json!({ "TagKeys": keys(1) })),
        ("ListTagsForResource", json!({})),
    ];

    let malformed: Vec<(&str, String)> = vec![
        (
            "one character short of the modelled minimum length",
            arn_of_length(ARN_MIN - 1),
        ),
        (
            "one character past the modelled maximum length",
            arn_of_length(ARN_MAX + 1),
        ),
        ("not an ARN at all", "this-is-not-an-arn-at-all".to_string()),
        (
            "another service's ARN, which the acm segment excludes",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/example-bucket"),
        ),
        (
            "an alphabetic account id, which the pattern's digit run excludes",
            format!("arn:aws:acm:{REGION}:not-an-account:acme-endpoint/e-1"),
        ),
        (
            "an empty resource segment",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:"),
        ),
        (
            "a character the ARN pattern's resource class excludes",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/e 1"),
        ),
    ];

    for (row, bad_arn) in &malformed {
        for (operation, members) in &operations {
            refusal(&router, row, operation, bad_arn, members, "resourceArn").await;
        }
    }

    // At each published bound the shape ADMITS the ARN, so the answer is the lookup's,
    // not the shape's.
    for (row, good_arn) in [
        ("at the modelled minimum length", arn_of_length(ARN_MIN)),
        ("at the modelled maximum length", arn_of_length(ARN_MAX)),
    ] {
        for (operation, members) in &operations {
            let (status, response) = tagging_call(&router, operation, &good_arn, members).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {row:?} on {operation}: an ARN the shape admits with no resource \
                 behind it is ResourceNotFoundException, got {status} {response}"
            );
        }
    }

    let after = stored_tags(&router, case, &stored).await;
    assert_eq!(
        after, before,
        "case {case:?}: the one stored resource must keep its tags {before:?} through every \
         refusal above, got {after:?}"
    );
}

/// RULE (the other side of the bounds above): a request AT each published bound of every
/// tag shape is served, and the pairs it names are what `ListTagsForResource` reports —
/// so the refusals above are the shape's doing and not a handler refusing legal input.
#[tokio::test]
async fn tagging_at_the_modelled_bounds_is_served() {
    let router = router();

    let longest_key = "k".repeat(TAG_KEY_MAX);
    let shortest_key = "k".repeat(TAG_KEY_MIN);
    let longest_value = "v".repeat(TAG_VALUE_MAX);

    // Each row: what it exercises, the `Tags` the request names, the pairs expected back.
    let rows: Vec<TagCase> = vec![
        (
            "one entry, the modelled minimum TagList length",
            json!([{ "Key": shortest_key, "Value": "x" }]),
            vec![(shortest_key.clone(), "x".to_string())],
        ),
        (
            "a key and a value at their modelled maximum lengths",
            json!([{ "Key": longest_key, "Value": longest_value }]),
            vec![(longest_key.clone(), longest_value.clone())],
        ),
        (
            "an empty value, TagValue's modelled minimum length",
            json!([{ "Key": "marker", "Value": "" }]),
            vec![("marker".to_string(), String::new())],
        ),
        (
            // `\p{Z}` is every separator, not the ASCII space alone: U+00A0 NO-BREAK
            // SPACE and U+2003 EM SPACE are separators the pattern admits.
            "the non-ASCII separators \\p{Z} admits",
            json!([{ "Key": "cost\u{00a0}center", "Value": "team\u{2003}sre" }]),
            vec![(
                "cost\u{00a0}center".to_string(),
                "team\u{2003}sre".to_string(),
            )],
        ),
        (
            // `\p{N}` is every number, not the ASCII digits alone: U+0660 ARABIC-INDIC
            // DIGIT ZERO and U+2167 ROMAN NUMERAL EIGHT are numbers.
            "the non-ASCII numbers \\p{N} admits",
            json!([{ "Key": "rev\u{0660}", "Value": "tier\u{2167}" }]),
            vec![("rev\u{0660}".to_string(), "tier\u{2167}".to_string())],
        ),
        (
            "the whole punctuation set the pattern admits",
            json!([{ "Key": "a_b.c:d/e=f+g-h@i", "Value": "j_k.l:m/n=o+p-q@r" }]),
            vec![(
                "a_b.c:d/e=f+g-h@i".to_string(),
                "j_k.l:m/n=o+p-q@r".to_string(),
            )],
        ),
    ];

    for (row, request_tags, expected) in rows {
        let arn = create_endpoint(&router, row).await;
        tag_resource(&router, row, &arn, &request_tags).await;
        let actual = stored_tags(&router, row, &arn).await;
        assert_eq!(
            actual, expected,
            "case {row:?}: expected the stored tags {expected:?}, got {actual:?}"
        );
    }

    // The widest legal request: `TagList`'s modelled maximum, then the same number of
    // keys removed in one `UntagResource`.
    let arn = create_endpoint(&router, "at the TagList maximum").await;
    tag_resource(&router, "at the TagList maximum", &arn, &pairs(TAG_LIST_MAX)).await;
    let actual = stored_tags(&router, "at the TagList maximum", &arn).await;
    assert_eq!(
        actual.len(),
        TAG_LIST_MAX,
        "case at the TagList maximum: expected {TAG_LIST_MAX} stored tags, got {actual:?}"
    );

    let (status, response) = tagging_call(
        &router,
        "UntagResource",
        &arn,
        &json!({ "TagKeys": keys(TAG_LIST_MAX) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case at the TagKeyList maximum: UntagResource must succeed, got {status} {response}"
    );
    let actual = stored_tags(&router, "at the TagKeyList maximum", &arn).await;
    assert!(
        actual.is_empty(),
        "case at the TagKeyList maximum: expected every tag removed, got {actual:?}"
    );
}

/// One tagging call: `members` merged onto the `ResourceArn` every one of these
/// operations addresses its resource by.
async fn tagging_call(
    router: &MultiServiceRouter,
    operation: &str,
    arn: &str,
    members: &Value,
) -> (u16, Value) {
    let mut body = json!({ "ResourceArn": arn });
    for (member, value) in members.as_object().expect("the members are an object") {
        body[member] = value.clone();
    }
    call(router, operation, &body).await
}

/// Assert that `operation` refuses this request with HTTP 400 `ValidationException`
/// naming the lowerCamel member path `path`.
async fn refusal(
    router: &MultiServiceRouter,
    row: &str,
    operation: &str,
    arn: &str,
    members: &Value,
    path: &str,
) {
    let (status, response) = tagging_call(router, operation, arn, members).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {row:?} on {operation}: expected 400 ValidationException, got {status} {response}"
    );
    let message = error_message(&response);
    assert!(
        message.contains(path),
        "case {row:?} on {operation}: expected the refusal to name the member {path:?}, \
         got {message:?}"
    );
}
