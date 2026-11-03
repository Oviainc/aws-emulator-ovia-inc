// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees — status codes,
//! error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over TABLES of inputs rather than one case: a tag key
//! that OPENS with the prefix Amazon Web Services reserves for the tags it generates
//! itself — `state::RESERVED_TAG_KEY_PREFIX`, in any letter case — names a tag no caller
//! may create, so EVERY request of this service that writes tags is refused with the
//! modelled `ValidationException` (400) and writes nothing at all. That covers
//! `TagResource` and the `tags` member of `CreateLandingZone`, `EnableControl` and
//! `EnableBaseline`, which all land in the one tag store `ListTagsForResource` reads.
//!
//! The other half of the rule is that it is a PREFIX test: a key that merely contains the
//! prefix, or opens with its letters but not its separator, is an ordinary key, so the
//! rows below drive those too and assert they round-trip through
//! `ListTagsForResource`.
//!
//! Every bound written out by hand here names its source. The prefix itself is read from
//! the crate rather than respelled, so a row and the rule it tests cannot drift. `TagKey`
//! is `@length(1..128)` and `TagMap` is `@length(0..200)`, and `TagResource` answers 204
//! on success — from `smithy_query(query="shape", shape="TagMap,TagKey")` and
//! `smithy_query(query="operation", operation="TagResource")` on this crate's
//! `smithy-model/`.

use std::collections::BTreeMap;

use service_com_amazonaws_controltower_awscontroltowerapis::state::RESERVED_TAG_KEY_PREFIX;
use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{call, emulator, set_up_account, simple_manifest, Answer};
use crate::resource_tags::{apply_tags, read_tags, reported_tags, resources, tags};

/// Tag keys in the reserved namespace: the prefix ALONE, the AWS-generated keys a client
/// is most likely to copy from another resource, the prefix in other letter cases (the AWS
/// tag restrictions reserve it "in any upper or lowercase combination"), a key at
/// `TagKey`'s published length maximum of 128, and one whose remainder is not ASCII.
///
/// Every row satisfies `TagKey`'s `@length(1..128)`, so the front door passes it and the
/// handler is what refuses it.
pub(crate) fn reserved_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        ("the reserved prefix alone", prefix.to_string()),
        (
            "an AWS-generated CloudFormation tag key",
            format!("{prefix}cloudformation:stack-name"),
        ),
        ("an AWS-generated provenance key", format!("{prefix}created-by")),
        (
            "the prefix in upper case",
            prefix.to_uppercase() + "cloudformation:logical-id",
        ),
        (
            "the prefix in mixed case",
            format!("{}ws:Foo", prefix[..1].to_uppercase()),
        ),
        (
            "a reserved key at TagKey's length maximum of 128",
            format!("{prefix}{}", "k".repeat(128 - prefix.chars().count())),
        ),
        (
            "a reserved key whose remainder is not ASCII",
            format!("{prefix}Проект"),
        ),
    ]
}

/// Tag keys that only RESEMBLE the reserved namespace, and so are ordinary keys a caller
/// may create: the letters without the separator, the letters as the start of a longer
/// word, the prefix CONTAINED rather than opening the key, another separator in its place,
/// and the shortest key `TagKey` admits.
fn ordinary_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    let letters = prefix.trim_end_matches(':');
    vec![
        ("the prefix's letters without its separator", letters.to_string()),
        ("the prefix's letters starting a longer word", format!("{letters}ome")),
        (
            "the prefix's letters, upper case, starting a longer word",
            format!("{}omething", letters.to_uppercase()),
        ),
        ("the prefix contained but not opening the key", format!("team:{prefix}cost")),
        ("the prefix's letters with another separator", format!("{letters}-internal")),
        ("the prefix's letters with an underscore", format!("{letters}_cost")),
        ("a key at TagKey's length minimum of 1", "a".to_string()),
    ]
}

/// Assert `refused` is the 400 `ValidationException` the rule names and that it states the
/// key it refused, so a client with several tags learns which one is reserved.
fn assert_reserved_refusal(case: &str, key: &str, refused: &Answer) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?} (key {key:?}): expected 400 ValidationException, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    let message = refused.string("message");
    assert!(
        message.contains(key),
        "case {case:?}: expected the refusal to name the offending key {key:?}, got \
         {message:?}"
    );
}

/// RULE: `TagResource` refuses a map carrying a key in the reserved namespace with 400
/// `ValidationException` and writes NOTHING — for every resource kind this service tags
/// and every form the reserved prefix takes.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_tag_key_and_writes_nothing() {
    let router = emulator();
    let resources = resources(&router).await;

    for resource in &resources {
        // What the resource carried before any refusal: a refused tagging must leave this
        // exactly as it was.
        let before = read_tags(&router, &resource.arn).await;
        assert_eq!(
            before.status, 200,
            "case {:?}: expected the resource's tags to be readable, got {} body {:?}",
            resource.case, before.status, before.body
        );

        for (case, key) in reserved_keys() {
            let refused = apply_tags(&router, &resource.arn, &tags(&[(&key, "example")])).await;
            assert_reserved_refusal(
                &format!("{case} on {}", resource.case),
                &key,
                &refused,
            );

            let after = read_tags(&router, &resource.arn).await;
            assert_eq!(
                reported_tags(&after),
                reported_tags(&before),
                "case {case:?} on {:?}: a refused tagging must write nothing, expected the \
                 tag set {:?}, got {:?}",
                resource.case,
                reported_tags(&before),
                reported_tags(&after)
            );
        }
    }
}

/// RULE (the other half): the namespace is decided by a PREFIX, so a key that only
/// resembles it is an ordinary tag — accepted with 204 and reported back by
/// `ListTagsForResource`.
#[tokio::test]
async fn a_key_that_only_resembles_the_reserved_prefix_is_an_ordinary_tag() {
    let router = emulator();
    let resources = resources(&router).await;

    for resource in &resources {
        let mut expected = reported_tags(&read_tags(&router, &resource.arn).await);

        for (case, key) in ordinary_keys() {
            let applied = apply_tags(&router, &resource.arn, &tags(&[(&key, "example")])).await;
            assert_eq!(
                applied.status, 204,
                "case {case:?} (key {key:?}) on {:?}: expected 204, got {} body {:?}",
                resource.case, applied.status, applied.body
            );

            expected.insert(key.clone(), "example".to_string());
            let read = read_tags(&router, &resource.arn).await;
            assert_eq!(
                reported_tags(&read),
                expected,
                "case {case:?} (key {key:?}) on {:?}: expected the merged tag set {expected:?}, \
                 got {:?}",
                resource.case,
                reported_tags(&read)
            );
        }
    }
}

/// RULE: the refusal is for the MAP, not for one pair — a reserved key beside ordinary
/// ones refuses the whole call, so none of the ordinary keys is written either.
#[tokio::test]
async fn one_reserved_key_refuses_the_whole_tag_map() {
    let router = emulator();
    let resources = resources(&router).await;
    let subject = &resources[1];
    let before = reported_tags(&read_tags(&router, &subject.arn).await);

    // The reserved key sits in a different position in each row, so the refusal cannot
    // depend on which pair the map happens to yield first.
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        (
            "a reserved key before an ordinary one",
            tags(&[(reserved.as_str(), "example"), ("Zone", "prod")]),
        ),
        (
            "a reserved key after an ordinary one",
            tags(&[("Alpha", "example"), (reserved.as_str(), "example")]),
        ),
        (
            "a reserved key among several ordinary ones",
            tags(&[
                ("Alpha", "a"),
                ("Owner", "platform"),
                (reserved.as_str(), "example"),
                ("Zone", "prod"),
            ]),
        ),
        (
            "two reserved keys and an ordinary one",
            tags(&[
                (reserved.as_str(), "example"),
                ("aws:created-by", "example"),
                ("Team", "platform"),
            ]),
        ),
    ];

    for (case, map) in rows {
        let refused = apply_tags(&router, &subject.arn, &map).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        let after = reported_tags(&read_tags(&router, &subject.arn).await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused map must write none of its keys, expected {before:?}, \
             got {after:?}"
        );
    }
}

/// POST `path` with `tags` set to one key, and whatever other members the operation needs.
async fn create_with_tag(
    router: &MultiServiceRouter,
    path: &str,
    mut body: serde_json::Value,
    key: &str,
) -> Answer {
    body["tags"] = serde_json::json!({ key: "example" });
    call(router, path, body).await
}

/// One row of the create/enable table: an operation whose `tags` member writes into the
/// same tag store, the body it needs beside `tags`, and the list operation that reports
/// whether it created anything.
struct Writer {
    case: &'static str,
    path: &'static str,
    body: serde_json::Value,
    list_path: &'static str,
    list_member: &'static str,
    /// Whether that list operation is one of the enablement reads Control Tower serves only
    /// for an account it has been set up in, so this row has to set the account up before it
    /// can read whether the refused call created anything. The `CreateLandingZone` row is
    /// the one that must NOT: it is the call that sets the account up, and an account holds
    /// one landing zone.
    requires_a_set_up_account: bool,
}

/// Every operation of this service that writes tags OTHER than `TagResource`: the `tags`
/// member of `CreateLandingZone`, `EnableControl` and `EnableBaseline`.
fn writers() -> Vec<Writer> {
    vec![
        Writer {
            case: "CreateLandingZone",
            path: "/create-landingzone",
            body: serde_json::json!({ "version": "3.3", "manifest": simple_manifest() }),
            list_path: "/list-landingzones",
            list_member: "landingZones",
            requires_a_set_up_account: false,
        },
        Writer {
            case: "EnableControl",
            path: "/enable-control",
            body: serde_json::json!({
                "controlIdentifier": "arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES",
                "targetIdentifier": "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-1a",
            }),
            list_path: "/list-enabled-controls",
            list_member: "enabledControls",
            requires_a_set_up_account: true,
        },
        Writer {
            case: "EnableBaseline",
            path: "/enable-baseline",
            body: serde_json::json!({
                "baselineIdentifier": "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
                "baselineVersion": "4.0",
                "targetIdentifier": "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-2b",
            }),
            list_path: "/list-enabled-baselines",
            list_member: "enabledBaselines",
            requires_a_set_up_account: true,
        },
    ]
}

/// How many resources a list operation reports, so a case can assert a refusal created
/// nothing.
async fn listed_count(router: &MultiServiceRouter, path: &str, member: &str) -> usize {
    let answer = call(router, path, serde_json::json!({})).await;
    assert_eq!(
        answer.status, 200,
        "{path}: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
        .get(member)
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE, on the other three writers: a `tags` member carrying a reserved key is refused
/// with 400 `ValidationException` and NOTHING is written — not the tag, and not the
/// resource the call would have created. The same call with an ordinary key creates the
/// resource and the tag round-trips through `ListTagsForResource`.
#[tokio::test]
async fn the_create_and_enable_paths_refuse_a_reserved_tag_key_and_write_nothing() {
    for writer in writers() {
        // A fresh emulator per writer: the account holds one landing zone, and a control
        // or baseline is enabled once per target, so each row starts from an empty store.
        let router = emulator();
        if writer.requires_a_set_up_account {
            set_up_account(&router).await;
        }
        let before = listed_count(&router, writer.list_path, writer.list_member).await;

        for (case, key) in reserved_keys() {
            let refused =
                create_with_tag(&router, writer.path, writer.body.clone(), &key).await;
            assert_reserved_refusal(&format!("{case} on {}", writer.case), &key, &refused);
            assert!(
                refused.body.get("arn").is_none(),
                "case {case:?} on {}: a refusal must mint no resource, got body {:?}",
                writer.case,
                refused.body
            );

            let after = listed_count(&router, writer.list_path, writer.list_member).await;
            assert_eq!(
                after, before,
                "case {case:?} on {}: a refused call must create nothing, expected {before} \
                 resources, got {after}",
                writer.case
            );
        }

        // And the same call with a key that only resembles the prefix does create the
        // resource, whose tag a read reports back.
        let key = format!("{}ome", RESERVED_TAG_KEY_PREFIX.trim_end_matches(':'));
        let created = create_with_tag(&router, writer.path, writer.body.clone(), &key).await;
        assert_eq!(
            created.status, 200,
            "{}: expected 200 for an ordinary tag key {key:?}, got {} body {:?}",
            writer.case, created.status, created.body
        );
        let arn = created.string("arn");
        let read = read_tags(&router, &arn).await;
        assert_eq!(
            reported_tags(&read),
            tags(&[(key.as_str(), "example")]),
            "{}: expected the tag the create carried to round-trip, got {:?}",
            writer.case,
            reported_tags(&read)
        );
    }
}
