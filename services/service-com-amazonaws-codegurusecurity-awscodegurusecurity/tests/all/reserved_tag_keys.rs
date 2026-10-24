// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE tag-key namespace a caller does not own.
//!
//! RULE: the `aws:` tag-key prefix (`state::RESERVED_TAG_KEY_PREFIX`, matched however it
//! is cased) is reserved for the tags Amazon Web Services applies itself, so a request
//! that would create, edit or delete a tag whose key carries it is refused with the
//! operation's modelled `ValidationException` (HTTP 400) and leaves the resource's tag
//! map exactly as it was — on EVERY operation that writes this service's tag store
//! (`CreateScan`'s `tags`, `TagResource`, `UntagResource`). A key that merely resembles
//! the prefix is an ordinary tag and round-trips.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: where
//! the prefix sits in the key, how it is cased, what follows it (nothing, ASCII,
//! non-ASCII, up to the key's 128-character bound), whether it is alone in the request
//! or beside ordinary keys, and whether it appears in a key or only in a value. The
//! prefix itself is read from the crate rather than re-spelled here, so the rows and the
//! handlers can never disagree about what is reserved.
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is asserted is the wire behaviour a
//! `codeguru-security` SDK client sees.

use serde_json::{json, Value};

use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    is_reserved_tag_key, RESERVED_TAG_KEY_PREFIX,
};

use crate::resource_tagging::{list_tags, scan_with_tags, tag, tag_object, untag};
use crate::scan_lifecycle::{artifact_for, call, create_scan, harness, Answer};

/// One tag key, and what about its form makes it the case it is.
#[derive(Debug)]
struct KeyCase {
    case: String,
    key: String,
}

/// The reserved rows: a key is reserved exactly when it BEGINS with
/// [`RESERVED_TAG_KEY_PREFIX`], so the rows vary what follows the prefix and how the
/// prefix is cased. `aws:cloudformation:stack-name` is one real example of such a key;
/// the rest of the class is covered by varying form, not by naming more examples.
fn reserved_keys() -> Vec<KeyCase> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    // `TagKey` is len 1..128 (`smithy_query(query="shape",shape="TagKey")`), so the
    // longest reserved key is the prefix plus 128 - prefix.len() characters.
    let at_length_bound = format!("{prefix}{}", "k".repeat(128 - prefix.len()));
    vec![
        KeyCase {
            case: "the prefix with nothing after it".to_string(),
            key: prefix.to_string(),
        },
        KeyCase {
            case: "a key Amazon Web Services itself applies".to_string(),
            key: format!("{prefix}cloudformation:stack-name"),
        },
        KeyCase {
            case: "one character after the prefix".to_string(),
            key: format!("{prefix}x"),
        },
        KeyCase {
            case: "the prefix upper-cased".to_string(),
            key: format!("{}created-by", prefix.to_uppercase()),
        },
        KeyCase {
            case: "the prefix with mixed case".to_string(),
            key: "Aws:Owner".to_string(),
        },
        KeyCase {
            case: "non-ASCII text after the prefix".to_string(),
            key: format!("{prefix}проект"),
        },
        KeyCase {
            case: "a reserved key at the 128-character key bound".to_string(),
            key: at_length_bound,
        },
    ]
}

/// The rows that only RESEMBLE the reserved prefix, and so are ordinary tags: the prefix
/// is reserved as a PREFIX, spelled exactly, and only in a key.
fn ordinary_keys() -> Vec<KeyCase> {
    let bare = RESERVED_TAG_KEY_PREFIX.trim_end_matches(':').to_string();
    vec![
        KeyCase {
            case: "the prefix without its separator".to_string(),
            key: bare.clone(),
        },
        KeyCase {
            case: "a word that merely starts with the prefix letters".to_string(),
            key: format!("{bare}ome"),
        },
        KeyCase {
            case: "the prefix letters with another separator".to_string(),
            key: format!("{bare}.team"),
        },
        KeyCase {
            case: "the prefix with a space before its separator".to_string(),
            key: format!("{bare} :team"),
        },
        KeyCase {
            case: "the prefix somewhere other than the start".to_string(),
            key: format!("my-{RESERVED_TAG_KEY_PREFIX}thing"),
        },
    ]
}

/// The tag map a suite starts each scan from, so "unchanged" is a map with content.
fn baseline() -> Vec<(String, String)> {
    vec![
        ("team".to_string(), "appsec".to_string()),
        ("env".to_string(), "prod".to_string()),
    ]
}

/// Assert `answer` is the modelled refusal of a reserved tag key naming `member`.
fn assert_reserved_refusal(case: &str, member: &str, key: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 for the reserved key {key:?}, got {answer:?}"
    );
    assert_eq!(
        answer.error_type, "ValidationException",
        "case {case:?}: expected error code ValidationException for {key:?}, got {answer:?}"
    );
    assert_eq!(
        answer.get("reason"),
        &json!("fieldValidationFailed"),
        "case {case:?}: expected the member-validation reason for {key:?}, got {answer:?}"
    );
    let message = answer.str_of("message");
    assert!(
        message.contains(key),
        "case {case:?}: expected the message to name the refused key {key:?}, got {message:?}"
    );
    assert_eq!(
        answer.get("fieldList")[0]["name"],
        json!(member),
        "case {case:?}: expected the refusal to name the member {member:?}, got {answer:?}"
    );
}

/// RULE: `TagResource` refuses a request naming a reserved key — alone or beside
/// ordinary keys — and the scan's tag map is left exactly as it was, so none of the
/// request's OTHER keys land either.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_writes_nothing() {
    let (_engine, router) = harness();
    let arn = scan_with_tags(&router, "reserved-tag", tag_object(&baseline())).await;

    for row in reserved_keys() {
        assert!(
            is_reserved_tag_key(&row.key),
            "case {:?}: {:?} must be a reserved key",
            row.case,
            row.key
        );

        let alone = tag(&router, &arn, json!({ row.key.clone(): "mine" })).await;
        assert_reserved_refusal(&row.case, "tags", &row.key, &alone);

        // Beside ordinary keys: the refusal is of the whole request, so the ordinary
        // key the same call carried is not written either.
        let beside = tag(
            &router,
            &arn,
            json!({ "owner": "code-scanning", row.key.clone(): "mine" }),
        )
        .await;
        assert_reserved_refusal(&row.case, "tags", &row.key, &beside);

        // Editing a reserved key is refused the same way as creating one, whatever
        // value it carries — including the empty value `TagValue` allows.
        let empty_value = tag(&router, &arn, json!({ row.key.clone(): "" })).await;
        assert_reserved_refusal(&row.case, "tags", &row.key, &empty_value);

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.get("tags"),
            &tag_object(&baseline()),
            "case {:?}: expected the scan's tag map unchanged by the refusals, got {listed:?}",
            row.case
        );
    }
}

/// RULE: `UntagResource` refuses a request naming a reserved key — a caller may no more
/// delete a tag AWS applies than create one — and removes nothing, not even the ordinary
/// keys the same call named.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_tag_key_and_removes_nothing() {
    let (_engine, router) = harness();
    let arn = scan_with_tags(&router, "reserved-untag", tag_object(&baseline())).await;

    for row in reserved_keys() {
        let alone = untag(&router, &arn, &[row.key.as_str()]).await;
        assert_reserved_refusal(&row.case, "tagKeys", &row.key, &alone);

        // The reserved key beside a key the scan really carries, in either order: the
        // whole request is refused, so the held key survives.
        let first = untag(&router, &arn, &[row.key.as_str(), "team"]).await;
        assert_reserved_refusal(&row.case, "tagKeys", &row.key, &first);
        let last = untag(&router, &arn, &["team", row.key.as_str()]).await;
        assert_reserved_refusal(&row.case, "tagKeys", &row.key, &last);

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.get("tags"),
            &tag_object(&baseline()),
            "case {:?}: expected the scan to keep every tag through the refusals, got {listed:?}",
            row.case
        );
    }
}

/// RULE: `CreateScan`'s `tags` writes the same tag store, so a reserved key there is
/// refused too — and the scan the request would have created does not exist afterwards.
#[tokio::test]
async fn create_scan_refuses_reserved_tags_and_records_no_scan() {
    let (_engine, router) = harness();

    for (index, row) in reserved_keys().into_iter().enumerate() {
        let scan_name = format!("reserved-create-{index}");
        let artifact = artifact_for(&router, &scan_name).await;
        let refused = create_scan(
            &router,
            json!({
                "scanName": scan_name,
                "tags": { "team": "appsec", row.key.clone(): "mine" },
            }),
            &artifact,
        )
        .await;
        assert_reserved_refusal(&row.case, "tags", &row.key, &refused);

        // Nothing was recorded: the scan the refused request named is absent.
        let absent = call(&router, "GET", &format!("/scans/{scan_name}"), None).await;
        assert_eq!(
            absent.status, 404,
            "case {:?}: expected the refused CreateScan to have recorded no scan, got {absent:?}",
            row.case
        );

        // The same request without the reserved key is accepted, so the refusal was the
        // key and nothing else about the request.
        let accepted = create_scan(
            &router,
            json!({ "scanName": scan_name, "tags": { "team": "appsec" } }),
            &artifact,
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the same CreateScan without the reserved key to succeed, got \
             {accepted:?}",
            row.case
        );
    }
}

/// RULE, the other side: a key that only resembles the prefix is an ordinary tag, and a
/// reserved spelling inside a VALUE is ordinary too — the reservation is of a key's
/// leading characters. Each row round-trips: written, read back, and removed again.
#[tokio::test]
async fn keys_that_only_resemble_the_reserved_prefix_are_ordinary_tags() {
    let (_engine, router) = harness();

    for (index, row) in ordinary_keys().into_iter().enumerate() {
        assert!(
            !is_reserved_tag_key(&row.key),
            "case {:?}: {:?} must NOT be a reserved key",
            row.case,
            row.key
        );
        let arn = scan_with_tags(&router, &format!("ordinary-{index}"), json!({})).await;

        let written = tag(&router, &arn, json!({ row.key.clone(): "mine" })).await;
        assert_eq!(
            written.status, 204,
            "case {:?}: expected the ordinary key {:?} to be accepted, got {written:?}",
            row.case, row.key
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.get("tags"),
            &json!({ row.key.clone(): "mine" }),
            "case {:?}: expected the written tag read back, got {listed:?}",
            row.case
        );

        let removed = untag(&router, &arn, &[row.key.as_str()]).await;
        assert_eq!(
            removed.status, 204,
            "case {:?}: expected the ordinary key to be removable, got {removed:?}",
            row.case
        );
        let after = list_tags(&router, &arn).await;
        assert_eq!(
            after.get("tags"),
            &json!({}),
            "case {:?}: expected the ordinary key gone after removal, got {after:?}",
            row.case
        );
    }

    // A VALUE spelling the reserved prefix is ordinary: the rule is about keys.
    let arn = scan_with_tags(&router, "reserved-value", json!({})).await;
    let value = format!("{RESERVED_TAG_KEY_PREFIX}kms");
    let written = tag(&router, &arn, json!({ "encryption": value.clone() })).await;
    assert_eq!(
        written.status, 204,
        "a value carrying the reserved prefix: expected 204, got {written:?}"
    );
    let listed: Value = list_tags(&router, &arn).await.get("tags").clone();
    assert_eq!(
        listed,
        json!({ "encryption": value }),
        "a value carrying the reserved prefix: expected it read back, got {listed:?}"
    );
}
