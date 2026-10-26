// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the reserved tag-key namespace, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rule this suite pins: a tag key a caller supplies must lie outside the
//! namespace Amazon Web Services keeps for its own tags
//! (`state::RESERVED_TAG_KEY_PREFIX`, matched however it is cased), and EVERY
//! operation that takes caller-supplied tag keys applies that rule the same way —
//! the creates that accept a `TagMap`, `TagResource`, and `UntagResource`, which
//! may no more remove such a tag than set one. A refused request is the
//! client-input error the operation's model declares, `InvalidRequestException`
//! with HTTP 400, and stores nothing. It is the PREFIX that is reserved: a key
//! that merely resembles it, or carries it anywhere but at the start, is an
//! ordinary key, is accepted, and reads back through `ListTagsForResource`.
//!
//! The prefix is read from the crate constant rather than respelled here, so a
//! row and the rule it drives cannot say different things.
//!
//! The operation table is the sibling `tag_key_refusals` suite's — the same set
//! of `TagMap`-accepting operations, covered there for the other half of the
//! tag-key rule, `TagKey`'s modelled `@length(min: 1, max: 128)` — and the HTTP
//! harness and tag reader/remover come from the asset and tagging suites; this
//! suite reuses them rather than growing a second copy.

use serde_json::json;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    RESERVED_TAG_KEY_PREFIX, TAG_KEY_MAX_LENGTH,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::resource_tags::{list_tags, untag_resource};
use crate::tag_key_refusals::{body_with_key, fingerprint, targets};

/// Keys in the reserved namespace, one row each.
///
/// Every row is inside `TagKey`'s modelled `@length(min: 1, max: 128)`, so what
/// each one tests is the reservation alone and not a length the sibling suite
/// already covers. The prefix comes from `RESERVED_TAG_KEY_PREFIX`; the cased
/// spellings are there because the reservation is of the NAMESPACE, so a key is
/// in it however the prefix is cased.
fn reserved_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        ("the reserved prefix alone", prefix.to_string()),
        (
            "a key naming a real AWS-applied tag",
            format!("{prefix}cloudformation:stack-name"),
        ),
        (
            "the prefix in upper case",
            format!("{}billing", prefix.to_uppercase()),
        ),
        (
            "the prefix in mixed case",
            format!("{}CreatedBy", capitalised(prefix)),
        ),
        (
            "a reserved key at the maximum admitted length",
            format!(
                "{prefix}{}",
                "k".repeat(TAG_KEY_MAX_LENGTH - prefix.chars().count())
            ),
        ),
    ]
}

/// `text` with its first character upper-cased — a differently cased spelling of
/// whatever it names.
fn capitalised(text: &str) -> String {
    let mut rest = text.chars();
    rest.next()
        .map(|first| first.to_uppercase().chain(rest).collect())
        .unwrap_or_default()
}

/// Keys that merely RESEMBLE the reserved namespace without being in it: the
/// prefix without its colon, the prefix as part of a longer word, the prefix
/// somewhere other than the start, and a key whose leading characters are
/// multi-byte and so cannot spell the ASCII prefix at all.
fn lookalike_keys() -> Vec<(&'static str, String)> {
    let bare = RESERVED_TAG_KEY_PREFIX.trim_end_matches(':').to_string();
    vec![
        ("the prefix without its colon", bare.clone()),
        ("the prefix inside a longer word", format!("{bare}ome")),
        ("the prefix followed by a dash", format!("{bare}-owner")),
        (
            "the prefix somewhere other than the start",
            format!("team:{RESERVED_TAG_KEY_PREFIX}owner"),
        ),
        (
            "a multi-byte key ahead of the prefix",
            format!("é{RESERVED_TAG_KEY_PREFIX}owner"),
        ),
    ]
}

#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_and_stores_nothing() {
    let (router, engine) = router_with_engine();
    let (tagged_arn, targets) = targets(&router).await;

    for target in &targets {
        for (at, (key_case, key)) in reserved_keys().iter().enumerate() {
            let case = format!("{} / {key_case}", target.label);
            let before = fingerprint(&router, &engine, &tagged_arn).await;

            let (status, answer) = call(
                &router,
                "POST",
                &target.uri,
                Some(body_with_key(target, at, key)),
            )
            .await;

            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: the key {key:?} is in the reserved namespace, so the call must \
                 be refused with the modelled client-input error, got {status} {answer}"
            );
            let message = answer["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(key.as_str()) && message.contains(RESERVED_TAG_KEY_PREFIX),
                "case {case:?}: the refusal must name the offending key and the reserved \
                 prefix, got {message:?}"
            );
            let after = fingerprint(&router, &engine, &tagged_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused call must store nothing, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_key_that_only_resembles_the_reserved_prefix_is_accepted_and_reads_back() {
    let router = router();
    let (_, targets) = targets(&router).await;

    for target in &targets {
        for (at, (key_case, key)) in lookalike_keys().iter().enumerate() {
            let case = format!("{} / {key_case}", target.label);

            let (status, answer) = call(
                &router,
                "POST",
                &target.uri,
                Some(body_with_key(target, at, key)),
            )
            .await;
            assert_eq!(
                status, target.success,
                "case {case:?}: the key {key:?} is outside the reserved namespace, so the call \
                 must answer {}, got {status} {answer}",
                target.success
            );

            let arn = (target.arn)(&answer);
            let (status, held) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the tags of {arn} must be readable, got {status} {held}"
            );
            assert_eq!(
                held["tags"][key].as_str(),
                Some("value"),
                "case {case:?}: the accepted key must be persisted under {arn}, got {held}"
            );
        }
    }
}

#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_nothing() {
    let router = router();
    let (tagged_arn, _) = targets(&router).await;

    // An ordinary tag to be left behind: whatever the request also names, a
    // refused removal must leave the tag set exactly as it was.
    let survivor = "cost-centre";
    let (status, answer) = call(
        &router,
        "POST",
        &format!("/tags?resourceArn={tagged_arn}"),
        Some(json!({ "tags": { survivor: "north-line" } })),
    )
    .await;
    assert_eq!(
        status, 200,
        "the ordinary tag must be written, got {status} {answer}"
    );

    for (key_case, key) in reserved_keys() {
        // The reserved key alone, and then beside a key the resource really
        // holds: in both forms the removal is refused and nothing is removed.
        for (form, keys) in [
            ("alone", vec![key.clone()]),
            (
                "beside a key the resource holds",
                vec![survivor.to_string(), key.clone()],
            ),
        ] {
            let case = format!("{key_case} / {form}");
            let (status, answer) = untag_resource(&router, &tagged_arn, &keys).await;
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: removing the reserved key {key:?} must be refused with the \
                 modelled client-input error, got {status} {answer}"
            );
            let message = answer["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(key.as_str()) && message.contains(RESERVED_TAG_KEY_PREFIX),
                "case {case:?}: the refusal must name the offending key and the reserved \
                 prefix, got {message:?}"
            );

            let (status, held) = list_tags(&router, &tagged_arn).await;
            assert_eq!(
                (status, held["tags"][survivor].as_str()),
                (200, Some("north-line")),
                "case {case:?}: a refused removal must leave the tag set untouched, got {held}"
            );
        }
    }
}
