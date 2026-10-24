// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag CONTENT rules, driven over real HTTP through the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rule under test: a caller may not create, modify or delete a tag whose key carries
//! the prefix AWS reserves for its own tags, nor store a key or value holding a character
//! outside AWS's published tag character set (letters, digits, spaces and
//! `_ . : / = + - @`); every operation that names a tag — `CreateChannel`,
//! `CreateOriginEndpoint`, `TagResource` and `UntagResource` — refuses such a request with
//! `UnprocessableEntityException` (HTTP 422) keyed by the member the request carried, and
//! leaves the stored tag set exactly as it was.
//!
//! The rule is not a modelled constraint — `smithy_query(query="operation",
//! operation="TagResource",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`
//! types `Tags` as a plain `map<string,string>` with no key constraint — it is AWS's
//! universal tagging restriction, which the service itself enforces.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: which
//! half of the tag is at fault (the key or the value), the WAY it is at fault (the
//! reserved prefix, or one of several characters outside the set), where in the string the
//! offending character sits, which resource type carries the tag, and which operation
//! names it. The accepted table is the other side of the same rule: keys that merely look
//! like the refused ones, and the whole published set, round-trip untouched.

use serde_json::{json, Value};
// The reserved prefix comes from the service's own constant rather than being restated
// here, so the rows below cannot drift from the rule the handlers enforce.
use service_com_amazonaws_mediapackage_mediapackage::state::RESERVED_TAG_KEY_PREFIX;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code};
use crate::resource_tags::{
    create_with_tags, described_tags, list_tags, tags_path, urlencode, Kind, PARENT_CHANNEL,
};

/// A one-entry tag map. Built rather than written with a literal, because every key below
/// is a value computed from the rule's own constant or from a table row.
fn one_tag(key: &str, value: &str) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        key.to_string(),
        json!(value),
    )]))
}

/// The tag set every resource in the mutation suites below is created with, and which a
/// refused request must leave untouched.
fn baseline_tags() -> Value {
    json!({ "team": "video", "env": "staging" })
}

/// One tag a caller may not store.
struct RefusedTag {
    /// What is wrong with it, for the assertion messages.
    case: &'static str,
    key: String,
    value: String,
    /// `true` when it is the KEY that is at fault — so `UntagResource`, which names keys
    /// and no values, must refuse this row too.
    key_at_fault: bool,
    /// The character outside the published set the refusal must name, for a row that
    /// breaks the CHARACTER SET; `None` for one refused by the reserved prefix.
    offending_character: Option<char>,
}

impl RefusedTag {
    /// The substrings this row's refusal must state: the key it is about (a value's
    /// refusal names the key it belongs to, which is the only member path a map entry
    /// has) and, for a character-set row, the character at fault.
    fn must_name(&self) -> Vec<String> {
        let mut names = vec![self.key.clone()];
        if let Some(character) = self.offending_character {
            names.push(format!("{character:?}"));
        }
        names
    }
}

/// A row whose KEY carries the reserved prefix.
fn reserved_key(case: &'static str, key: String) -> RefusedTag {
    RefusedTag {
        case,
        key,
        value: "v".to_string(),
        key_at_fault: true,
        offending_character: None,
    }
}

/// A row whose KEY carries `character`, which is outside the published set.
fn refused_key(case: &'static str, key: &str, character: char) -> RefusedTag {
    RefusedTag {
        case,
        key: key.to_string(),
        value: "v".to_string(),
        key_at_fault: true,
        offending_character: Some(character),
    }
}

/// A row whose VALUE carries `character`; its key is well-formed.
fn refused_value(case: &'static str, key: &str, value: &str, character: char) -> RefusedTag {
    RefusedTag {
        case,
        key: key.to_string(),
        value: value.to_string(),
        key_at_fault: false,
        offending_character: Some(character),
    }
}

/// The tags no operation may store. The rows differ along which half of the tag is at
/// fault, why (the reserved prefix, or a character outside the published set), and where
/// in the string the fault sits.
fn refused_tag_cases() -> Vec<RefusedTag> {
    vec![
        reserved_key(
            "a key AWS itself mints, under the reserved prefix",
            format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name"),
        ),
        reserved_key(
            "the reserved prefix with nothing after it",
            RESERVED_TAG_KEY_PREFIX.to_string(),
        ),
        reserved_key(
            "the reserved prefix followed by one character",
            format!("{RESERVED_TAG_KEY_PREFIX}x"),
        ),
        reserved_key(
            "the reserved prefix on another AWS service's tag",
            format!("{RESERVED_TAG_KEY_PREFIX}autoscaling:groupName"),
        ),
        // The character set: AWS publishes letters, digits, spaces and `_ . : / = + - @`,
        // so a currency sign, a hash, a bracket, an exclamation mark and a pictograph are
        // each outside it — taken at the start, in the middle and at the end of the key.
        refused_key("a key opening on a currency sign", "€uro-cost", '€'),
        refused_key("a key carrying a hash", "tag#1", '#'),
        refused_key("a key carrying brackets", "key(one)", '('),
        refused_key("a key ending in an exclamation mark", "owner!", '!'),
        refused_key("a key carrying a pictograph", "team-🎬", '🎬'),
        refused_value("a value carrying a currency sign", "cost", "20€", '€'),
        refused_value("a value carrying a star", "glob", "a*b", '*'),
        refused_value("a value carrying a percent sign", "ratio", "100%", '%'),
        refused_value("a value carrying a comma", "owners", "ops,sre", ','),
    ]
}

/// The tags every operation must accept: the other side of the same rule. The rows differ
/// along how close they come to a refused one — the reserved letters without the colon,
/// the reserved prefix somewhere other than the start — and along the breadth of the
/// published set: every punctuation character it names, spaces, digits, and letters
/// outside ASCII.
fn accepted_tag_cases() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "the reserved letters without the colon that makes them a prefix",
            "awsome-platform".to_string(),
            "video".to_string(),
        ),
        (
            "the reserved prefix after the key's first character",
            format!("team:{RESERVED_TAG_KEY_PREFIX}owner"),
            "video".to_string(),
        ),
        (
            "every punctuation character the published set names",
            "_.:/=+-@".to_string(),
            "_.:/=+-@".to_string(),
        ),
        (
            "spaces in both halves",
            "cost centre".to_string(),
            "the video team".to_string(),
        ),
        ("digits only", "123".to_string(), "0".to_string()),
        (
            "letters outside ASCII, which the published set admits as letters",
            "Ägare".to_string(),
            "vidéo".to_string(),
        ),
    ]
}

/// `UntagResource`'s request line for `keys`: one `tagKeys=` parameter each, with every
/// reserved character percent-encoded.
fn untag_query(arn: &str, keys: &[&str]) -> String {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", urlencode(key)))
        .collect::<Vec<String>>()
        .join("&");
    format!("{}?{query}", tags_path(arn))
}

/// Assert `error` is this service's 422 refusal keyed by `member`, stating a requirement
/// that names each of `names` — the refusal is built from the request, so it says which
/// value failed and why.
fn assert_refusal(case: &str, status: u16, error: &Value, member: &str, names: &[String]) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {error:?}"
    );
    assert_eq!(
        error_code(error),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {error:?}"
    );
    assert!(
        error.get("message").and_then(Value::as_str).is_some(),
        "case {case:?}: expected a message, got {error:?}"
    );
    let reported = error
        .pointer(&format!("/errors/{member}"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    for name in names {
        assert!(
            reported.contains(name.as_str()),
            "case {case:?}: expected the {member:?} requirement to name {name:?}, got {error:?}"
        );
    }
}

#[tokio::test]
async fn a_create_refuses_a_tag_the_caller_may_not_store_and_stores_nothing() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, row) in refused_tag_cases().into_iter().enumerate() {
            let case = format!("{} created with {}", kind.label(), row.case);
            let id = format!("refused-create-{}-{index}", kind.label().replace(' ', "-"));
            let tags = one_tag(&row.key, &row.value);
            let (path, body) = kind.create_request(&id, &tags);

            let (status, error) = call(&router, "POST", path, Some(&body)).await;
            assert_refusal(&case, status, &error, "tags", &row.must_name());

            // A refused create stores nothing: the resource it named is not readable.
            let (status, read) = call(&router, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: the refused create stored something, got {status} {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn tag_resource_refuses_a_tag_the_caller_may_not_store_and_leaves_the_set_as_it_was() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        let id = format!("refused-tag-{}", kind.label().replace(' ', "-"));
        create_with_tags(&router, &kind, &id, &baseline_tags()).await;
        let arn = kind.arn(&id);

        for row in refused_tag_cases() {
            let case = format!("tagging a {} with {}", kind.label(), row.case);
            let body = json!({ "tags": one_tag(&row.key, &row.value) });
            let (status, error) = call(&router, "POST", &tags_path(&arn), Some(&body)).await;
            assert_refusal(&case, status, &error, "tags", &row.must_name());

            // Nothing was written: neither the refused tag nor any change to the others.
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed,
                baseline_tags(),
                "case {case:?}: expected the tag set {:?}, got {listed:?}",
                baseline_tags()
            );
            let (_, described) = call(&router, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                described_tags(&described),
                baseline_tags(),
                "case {case:?}: the stored record's tags changed, got {described:?}"
            );
        }

        // One request carrying a well-formed tag BESIDE a refused one is refused whole:
        // the acceptable half is not written either.
        let reserved = format!("{RESERVED_TAG_KEY_PREFIX}sneaked-in");
        let case = format!(
            "tagging a {} with one acceptable tag and one reserved key",
            kind.label()
        );
        let mut batch = serde_json::Map::new();
        batch.insert("owner".to_string(), json!("ops"));
        batch.insert(reserved.clone(), json!("mine"));
        let body = json!({ "tags": Value::Object(batch) });
        let (status, error) = call(&router, "POST", &tags_path(&arn), Some(&body)).await;
        assert_refusal(&case, status, &error, "tags", &[reserved]);
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            listed,
            baseline_tags(),
            "case {case:?}: expected the tag set {:?}, got {listed:?}",
            baseline_tags()
        );
    }
}

#[tokio::test]
async fn untag_resource_refuses_a_key_the_caller_may_not_name_and_removes_nothing() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        let id = format!("refused-untag-{}", kind.label().replace(' ', "-"));
        create_with_tags(&router, &kind, &id, &baseline_tags()).await;
        let arn = kind.arn(&id);

        for row in refused_tag_cases().into_iter().filter(|row| row.key_at_fault) {
            let case = format!("untagging a {} by {}", kind.label(), row.case);
            let (status, error) =
                call(&router, "DELETE", &untag_query(&arn, &[&row.key]), None).await;
            assert_refusal(&case, status, &error, "tagKeys", &row.must_name());

            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed,
                baseline_tags(),
                "case {case:?}: expected the tag set {:?}, got {listed:?}",
                baseline_tags()
            );
        }

        // A request naming a key it MAY remove beside one it may not is refused whole, so
        // the removable key survives.
        let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");
        let case = format!(
            "untagging a {} by one removable key and one reserved key",
            kind.label()
        );
        let uri = untag_query(&arn, &["team", &reserved]);
        let (status, error) = call(&router, "DELETE", &uri, None).await;
        assert_refusal(&case, status, &error, "tagKeys", &[reserved]);
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            listed,
            baseline_tags(),
            "case {case:?}: expected the tag set {:?}, got {listed:?}",
            baseline_tags()
        );

        // The refusal is about a key a caller may not NAME, not about one the resource
        // happens not to carry: a well-formed absent key is still removed without error.
        let case = format!("untagging a {} by a well-formed absent key", kind.label());
        let uri = untag_query(&arn, &["absent-key"]);
        let (status, body) = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204, got {status} {body:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            listed,
            baseline_tags(),
            "case {case:?}: expected the tag set {:?}, got {listed:?}",
            baseline_tags()
        );
    }
}

#[tokio::test]
async fn a_tag_within_the_published_set_is_stored_by_a_create_and_by_tag_resource() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, (name, key, value)) in accepted_tag_cases().into_iter().enumerate() {
            let label = kind.label().replace(' ', "-");
            let tags = one_tag(&key, &value);

            // Tag-on-create: the create stores it and a `Describe*` reads it back.
            let case = format!("{} created with {name}", kind.label());
            let id = format!("accepted-create-{label}-{index}");
            let created = create_with_tags(&router, &kind, &id, &tags).await;
            assert_eq!(
                described_tags(&created),
                tags,
                "case {case:?}: expected the create to store {tags:?}, got {created:?}"
            );
            let (status, described) = call(&router, "GET", &kind.describe_path(&id), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described_tags(&described),
                tags,
                "case {case:?}: the read-back expected {tags:?}, got {described:?}"
            );

            // `TagResource`: the same tag added to an already-stored resource.
            let case = format!("{} tagged with {name}", kind.label());
            let id = format!("accepted-tag-{label}-{index}");
            create_with_tags(&router, &kind, &id, &json!({})).await;
            let arn = kind.arn(&id);
            let body = json!({ "tags": tags });
            let (status, answered) = call(&router, "POST", &tags_path(&arn), Some(&body)).await;
            assert_eq!(
                status, 204,
                "case {case:?}: TagResource expected 204, got {status} {answered:?}"
            );
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the read-back expected 200, got {status} {listed:?}"
            );
            assert_eq!(
                listed, tags,
                "case {case:?}: expected the tag set {tags:?}, got {listed:?}"
            );

            // And `UntagResource` removes the very key it just stored: a key within the
            // published set is the caller's own, so the caller can delete it again.
            let uri = untag_query(&arn, &[&key]);
            let (status, answered) = call(&router, "DELETE", &uri, None).await;
            assert_eq!(
                status, 204,
                "case {case:?}: UntagResource expected 204, got {status} {answered:?}"
            );
            let (_, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                listed,
                json!({}),
                "case {case:?}: expected an empty tag set, got {listed:?}"
            );
        }
    }
}
