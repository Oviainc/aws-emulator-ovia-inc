// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListTagsForResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `ListTagsForResource` reports exactly the tag set persisted
//! on the email contact its `arn` names, whatever that set is — a contact carrying no
//! tags reports an EMPTY map rather than an absent one — a well-formed ARN naming no
//! stored contact is the modelled 404, and the read leaves the record exactly as it
//! was.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::email_contacts::{
    as_millis, call, create, delete, emulator, error_code, get_ok, label, unknown_arn, Emulator,
};

/// The most pairs a `TagMap` may carry, from
/// `smithy_query(query="shape",shape="TagMap")` (`len 0..200`).
const MAX_TAGS: usize = 200;
/// The most characters a `TagKey` may carry, from its modelled pattern
/// `^(?!aws:).{1,128}$` (`smithy_query(query="shape",shape="TagKey")`).
const MAX_TAG_KEY_LEN: usize = 128;
/// The most characters a `TagValue` may carry, from
/// `smithy_query(query="shape",shape="TagValue")` (`len 0..256`).
const MAX_TAG_VALUE_LEN: usize = 256;

async fn list_tags(em: &Emulator, arn: &str) -> (u16, Value) {
    call(em, "GET", &format!("/tags/{}", label(arn)), None).await
}

/// The tag set a successful `ListTagsForResource` reported, as an ordered map so two
/// of them compare regardless of the order the wire happened to carry.
async fn tags_ok(em: &Emulator, arn: &str) -> BTreeMap<String, String> {
    let (status, body) = list_tags(em, arn).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource on {arn} must answer 200, got {status} / {body:?}"
    );
    let tags = body.get("tags").unwrap_or_else(|| {
        panic!("ListTagsForResource must report a tags member, got {body:?}")
    });
    let object = tags
        .as_object()
        .unwrap_or_else(|| panic!("tags must be a map, got {tags:?}"));
    object
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("tag {key:?} must carry a string, got {value:?}"));
            (key.clone(), value.to_string())
        })
        .collect()
}

/// Create a contact carrying `tags` (absent when `None`) and return its ARN.
async fn create_tagged(
    em: &Emulator,
    name: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("emailAddress".to_string(), json!(format!("{name}@example.com")));
    if let Some(tags) = tags {
        body.insert(
            "tags".to_string(),
            Value::Object(
                tags.iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            ),
        );
    }
    let (status, answer) = create(em, &Value::Object(body)).await;
    assert_eq!(
        status, 201,
        "creating {name:?} must answer 201, got {status} / {answer:?}"
    );
    answer
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateEmailContact must return an arn, got {answer:?}"))
        .to_string()
}

fn tag_map<I: IntoIterator<Item = (String, String)>>(pairs: I) -> BTreeMap<String, String> {
    pairs.into_iter().collect()
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Every row is a tag set a create may persist, differing along what the rule
/// generalises over: how many pairs, how long a key or a value is, and which
/// characters they carry. `None` means the create named no `tags` member at all.
fn tag_set_table() -> Vec<(&'static str, Option<BTreeMap<String, String>>)> {
    vec![
        ("no tags member at all", None),
        ("an explicitly empty map", Some(BTreeMap::new())),
        ("one pair", Some(tag_map([pair("team", "infra")]))),
        (
            "several pairs",
            Some(tag_map([
                pair("team", "infra"),
                pair("env", "prod"),
                pair("owner", "on-call"),
            ])),
        ),
        (
            "an empty value, the TagValue minimum",
            Some(tag_map([pair("retired", "")])),
        ),
        (
            "a key at the TagKey maximum length",
            Some(tag_map([pair(&"k".repeat(MAX_TAG_KEY_LEN), "at-the-bound")])),
        ),
        (
            "a value at the TagValue maximum length",
            Some(tag_map([pair("long", &"v".repeat(MAX_TAG_VALUE_LEN))])),
        ),
        (
            "keys and values carrying punctuation and non-ascii text",
            Some(tag_map([
                pair("cost:center", "eu-west/42"),
                pair("équipe", "naïve value"),
                pair("spaced key", "spaced value"),
            ])),
        ),
        (
            "the TagMap maximum of 200 pairs",
            Some(tag_map(
                (0..MAX_TAGS).map(|n| pair(&format!("key-{n:03}"), &format!("value-{n:03}"))),
            )),
        ),
    ]
}

#[tokio::test]
async fn list_tags_reports_exactly_the_tag_set_the_contact_persists() {
    let em = emulator();
    let table = tag_set_table();
    assert!(
        table.len() >= 5,
        "the tag-set table must span the class, got {} rows",
        table.len()
    );

    for (index, (case, tags)) in table.iter().enumerate() {
        let arn = create_tagged(&em, &format!("contact-{index}"), tags.as_ref()).await;
        // A create that named no tags, and one that named an empty map, both leave the
        // contact with the EMPTY tag set.
        let expected = tags.clone().unwrap_or_default();

        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: ListTagsForResource must report the persisted tag set, got {reported:?}"
        );

        // The read is a report, so repeating it answers the same thing and leaves the
        // record — including the instant it was last updated — untouched.
        let before = get_ok(&em, &arn).await;
        let again = tags_ok(&em, &arn).await;
        let after = get_ok(&em, &arn).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second read must report the same tag set, got {again:?}"
        );
        assert_eq!(
            as_millis(&after, "updateTime"),
            as_millis(&before, "updateTime"),
            "case {case:?}: listing tags must not touch updateTime, got {after:?}"
        );
    }
}

#[tokio::test]
async fn each_contact_reports_only_its_own_tags() {
    let em = emulator();
    // Distinct tag sets, including one empty, so a handler answering from the wrong
    // record — or from a single shared map — cannot pass.
    let wanted = [
        ("alpha", tag_map([pair("team", "alpha"), pair("tier", "1")])),
        ("beta", tag_map([pair("team", "beta")])),
        ("gamma", BTreeMap::new()),
        (
            "delta",
            tag_map([pair("team", "delta"), pair("tier", "3"), pair("env", "dev")]),
        ),
    ];

    let mut arns = Vec::new();
    for (name, tags) in &wanted {
        arns.push(create_tagged(&em, name, Some(tags)).await);
    }

    for (arn, (name, tags)) in arns.iter().zip(wanted.iter()) {
        let reported = tags_ok(&em, arn).await;
        assert_eq!(
            &reported, tags,
            "case {name:?}: each contact must report only its own tags, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_stored_contact_is_a_modelled_404() {
    let em = emulator();
    let tagged = create_tagged(&em, "survivor", Some(&tag_map([pair("keep", "me")]))).await;

    // A well-formed ARN the store never minted, and one whose contact has since been
    // deleted: both are the modelled 404, never an empty 200.
    let deleted = create_tagged(&em, "doomed", Some(&tag_map([pair("gone", "soon")]))).await;
    assert_eq!(
        delete(&em, &deleted).await.0,
        200,
        "the delete must succeed"
    );
    for (case, arn) in [
        ("an arn this store never minted", unknown_arn('q')),
        ("another well-formed unknown arn", unknown_arn('7')),
        ("a deleted contact's arn", deleted.clone()),
    ] {
        let (status, body) = list_tags(&em, &arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an arn naming nothing must answer 404, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // `arn`'s own modelled pattern is refused at the boundary, before the handler.
    let account = engine::EMULATOR_ACCOUNT_ID;
    for (case, arn) in [
        (
            "an id that is too short",
            format!("arn:aws:notifications-contacts::{account}:emailcontact/abc"),
        ),
        (
            "an arn carrying a region",
            format!(
                "arn:aws:notifications-contacts:us-west-2:{account}:emailcontact/{}",
                "a".repeat(27)
            ),
        ),
        ("not an arn at all", "just-a-name".to_string()),
    ] {
        let (status, body) = list_tags(&em, &arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a malformed arn must be refused, got {status} / {body:?}"
        );
    }

    // None of it disturbed the contact that does exist.
    let reported = tags_ok(&em, &tagged).await;
    assert_eq!(
        reported,
        tag_map([pair("keep", "me")]),
        "the surviving contact must still report its own tags, got {reported:?}"
    );
}
