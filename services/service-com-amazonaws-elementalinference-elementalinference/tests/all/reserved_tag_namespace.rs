// SPDX-License-Identifier: CC0-1.0

//! Which tag keys claim the namespace AWS reserves for its own tags, end to end over the
//! wire, on every operation of this service that NAMES a tag key.
//!
//! The rule: a key claims the reserved namespace when its leading characters are the
//! reserved prefix ([`RESERVED_TAG_KEY_PREFIX`]) WITHOUT regard to letter case, and such a
//! key is refused with the modelled `ValidationException` (HTTP 400) while nothing is
//! written. A key that merely carries the prefix further along, that begins with the
//! prefix's letters without its separator, or that begins with other characters
//! altogether, is a key of the caller's own namespace and is stored, read back and
//! deleted like any other — letter case decides WHICH namespace a key claims, and
//! nothing else, so a tag key remains case-sensitive as an identifier.
//!
//! The rows below span what that rule generalises over: the letter case the prefix
//! arrives in (the model's own lower case, upper case, alternating case), where in the
//! key it sits, whether its separator is there, and keys shorter than the prefix or
//! beginning with multi-byte characters. Each row is asserted on every operation a tag
//! key reaches — `CreateFeed`, `CreateDictionary`, `TagResource` and `UntagResource` —
//! and on both kinds of resource this service tags, since the rule belongs to the TAG and
//! not to the operation or the resource that carried it.

use crate::harness::{
    call, create_tagged_resource, expected_tags, list_member, list_path, pair, read_resource,
    router, tag_object, tagged_create_body, tags_of, tags_path, untag_resource, TAGGABLE_KINDS,
};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    reserved_tag_key_message, RESERVED_TAG_KEY_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

/// One row: a tag key a request may name, and whether it claims the reserved namespace.
struct KeyCase {
    label: String,
    key: String,
    reserved: bool,
}

/// `text` with every other character upper-cased, for the rows that carry the reserved
/// prefix in a case that is neither the model's own nor all upper.
fn alternating_case(text: &str) -> String {
    text.chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect()
}

/// The keys under test, every one of them built from [`RESERVED_TAG_KEY_PREFIX`] rather
/// than from a literal, so the table states the rule and not one spelling of it.
fn key_cases() -> Vec<KeyCase> {
    let reserved = RESERVED_TAG_KEY_PREFIX;
    // The prefix's letters without the separator it ends with: a key beginning with just
    // those letters claims nothing.
    let letters = reserved.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
    let row = |label: String, key: String, reserved: bool| KeyCase {
        label,
        key,
        reserved,
    };
    vec![
        row(
            "the prefix in the case the model spells it".to_string(),
            format!("{reserved}cloudformation:stack-name"),
            true,
        ),
        row(
            "the prefix in upper case".to_string(),
            format!("{}cloudformation:stack-name", reserved.to_uppercase()),
            true,
        ),
        row(
            "the prefix in alternating case".to_string(),
            format!("{}secretsmanager:owning-service", alternating_case(reserved)),
            true,
        ),
        row(
            "the bare prefix in upper case as the whole key".to_string(),
            reserved.to_uppercase(),
            true,
        ),
        row(
            "the bare prefix in alternating case as the whole key".to_string(),
            alternating_case(reserved),
            true,
        ),
        row(
            "the prefix in upper case with a non-ASCII remainder".to_string(),
            format!("{}項目", reserved.to_uppercase()),
            true,
        ),
        row(
            "the prefix's letters in upper case without its separator".to_string(),
            letters.to_uppercase(),
            false,
        ),
        row(
            "an ordinary key beginning with the prefix's letters in upper case".to_string(),
            format!("{}omeKey", letters.to_uppercase()),
            false,
        ),
        row(
            "the prefix in upper case further along the key, not at its start".to_string(),
            format!("team:{}thing", reserved.to_uppercase()),
            false,
        ),
        row(
            "a key beginning with multi-byte characters".to_string(),
            "項目:team".to_string(),
            false,
        ),
        row(
            "a key shorter than the prefix".to_string(),
            letters[..1].to_string(),
            false,
        ),
        row("an ordinary key".to_string(), "team".to_string(), false),
    ]
}

/// Assert the refusal a reserved key earns, whichever operation answered it.
fn assert_reserved_refusal(label: &str, key: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {label:?}: a key in the reserved namespace is refused 400, got {status} with \
         {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ValidationException"),
        "case {label:?}: that 400 is the modelled ValidationException, got {body}"
    );
    assert_eq!(
        body["message"],
        json!(reserved_tag_key_message(key)),
        "case {label:?}: expected {:?}, got {body}",
        reserved_tag_key_message(key)
    );
}

/// How many resources of `kind` this store holds at all — the check that a refused create
/// minted nothing.
async fn stored_count(router: &MultiServiceRouter, kind: &str) -> usize {
    let (status, listed) = call(router, "GET", list_path(kind), None).await;
    assert_eq!(
        status, 200,
        "listing {kind}s answers 200, got {status} with {listed}"
    );
    listed[list_member(kind)]
        .as_array()
        .unwrap_or_else(|| panic!("a {kind} list reports an array, got {listed}"))
        .len()
}

/// A create judges the key it is handed: a key claiming the reserved namespace in ANY case
/// mints nothing, and any other key is stored and read back.
#[tokio::test]
async fn a_create_refuses_a_key_claiming_the_reserved_namespace_in_any_case() {
    for kind in TAGGABLE_KINDS {
        for (index, case) in key_cases().into_iter().enumerate() {
            let router = router();
            let label = format!("{} on a created {kind}", case.label);
            let tags = vec![pair(&case.key, "mine")];
            let (status, body) = call(
                &router,
                "POST",
                &format!("/v1/{kind}"),
                Some(tagged_create_body(
                    kind,
                    &format!("rsvcreate{index}"),
                    &tags,
                )),
            )
            .await;

            if case.reserved {
                assert_reserved_refusal(&label, &case.key, status, &body);
                assert_eq!(
                    stored_count(&router, kind).await,
                    0,
                    "case {label:?}: a refused create mints no {kind}"
                );
            } else {
                assert_eq!(
                    status, 202,
                    "case {label:?}: a key of the caller's own namespace is accepted, got \
                     {status} with {body}"
                );
                let id = body["id"].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: a created {kind} carries an id, got {body}")
                });
                let (status, stored) = read_resource(&router, kind, id).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the created {kind} reads back, got {status} with {stored}"
                );
                assert_eq!(
                    tags_of(&stored),
                    expected_tags(&tags),
                    "case {label:?}: the stored tag is what the next read reports, got {stored}"
                );
            }
        }
    }
}

/// `TagResource` judges the key it is handed by the same rule: a reserved key in any case
/// leaves the stored tag set alone, and any other key is merged and read back.
#[tokio::test]
async fn a_tag_write_refuses_a_key_claiming_the_reserved_namespace_in_any_case() {
    for kind in TAGGABLE_KINDS {
        for (index, case) in key_cases().into_iter().enumerate() {
            let router = router();
            let label = format!("{} written onto a {kind}", case.label);
            // The resource starts with one ordinary tag, so a refusal is visibly a write
            // of nothing rather than a resource that simply holds no tags.
            let initial = vec![pair("env", "dev")];
            let (id, arn) = create_tagged_resource(
                &router,
                kind,
                &format!("rsvwrite{index}"),
                &initial,
            )
            .await;

            let tags = vec![pair(&case.key, "mine")];
            let (status, body) = call(
                &router,
                "POST",
                &tags_path(&arn),
                Some(json!({"tags": tag_object(&tags)})),
            )
            .await;

            let (_, stored) = read_resource(&router, kind, &id).await;
            if case.reserved {
                assert_reserved_refusal(&label, &case.key, status, &body);
                assert_eq!(
                    tags_of(&stored),
                    expected_tags(&initial),
                    "case {label:?}: the refused write left the stored tags alone, got {stored}"
                );
            } else {
                assert_eq!(
                    status, 204,
                    "case {label:?}: a key of the caller's own namespace is accepted, got \
                     {status} with {body}"
                );
                let mut expected = initial.clone();
                expected.extend(tags.iter().cloned());
                assert_eq!(
                    tags_of(&stored),
                    expected_tags(&expected),
                    "case {label:?}: the write is merged onto the stored set, got {stored}"
                );
            }
        }
    }
}

/// `UntagResource` judges the key it is handed by the same rule: a reserved key in any
/// case removes nothing, and any other key the resource carries is removed.
#[tokio::test]
async fn a_tag_delete_refuses_a_key_claiming_the_reserved_namespace_in_any_case() {
    for kind in TAGGABLE_KINDS {
        for (index, case) in key_cases().into_iter().enumerate() {
            let router = router();
            let label = format!("{} deleted from a {kind}", case.label);
            // A key of the caller's own namespace is one the resource can really carry, so
            // the row proves the delete acted; a reserved key cannot be stored at all, so
            // its row proves the whole set survived.
            let mut initial = vec![pair("env", "dev")];
            if !case.reserved {
                initial.push(pair(&case.key, "mine"));
            }
            let (id, arn) = create_tagged_resource(
                &router,
                kind,
                &format!("rsvdelete{index}"),
                &initial,
            )
            .await;

            let (status, body) =
                untag_resource(&router, &arn, std::slice::from_ref(&case.key)).await;
            let (_, stored) = read_resource(&router, kind, &id).await;

            if case.reserved {
                assert_reserved_refusal(&label, &case.key, status, &body);
                assert_eq!(
                    tags_of(&stored),
                    expected_tags(&initial),
                    "case {label:?}: the refused delete removed nothing, got {stored}"
                );
            } else {
                assert_eq!(
                    status, 204,
                    "case {label:?}: a key of the caller's own namespace is accepted, got \
                     {status} with {body}"
                );
                assert_eq!(
                    tags_of(&stored),
                    expected_tags(&[pair("env", "dev")]),
                    "case {label:?}: exactly the named key was removed, got {stored}"
                );
            }
        }
    }
}
