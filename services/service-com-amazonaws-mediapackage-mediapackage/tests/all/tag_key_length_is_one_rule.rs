// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag-key LENGTH bound, driven over real HTTP through the same
//! `MultiServiceRouter` the emulator boots.
//!
//! The rule under test: how long a tag key may be is part of the one rule that decides
//! whether a tag key is well-formed at all, so EVERY operation naming a tag key judges it
//! the same way — a key of at most `MAX_TAG_KEY_CHARS` characters is the caller's to
//! store and to remove, and a longer one is refused with `UnprocessableEntityException`
//! (HTTP 422) keyed by the member that carried it (`tags` for a tag map, `tagKeys` for
//! `UntagResource`), leaving the stored tag set exactly as it was. The bound counts
//! CHARACTERS, not bytes. A door that would DELETE such a key refuses it for the same
//! reason a door that would store it does: no write path could ever have stored it.
//!
//! The bound is not a modelled constraint — `smithy_query(query="operation",
//! operation="TagResource,UntagResource",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`
//! types `Tags` as a plain `map<string,string>` and `TagKeys` as a plain list of
//! `__string`, with no length constraint on either — it is AWS's universal tagging
//! restriction, which the service enforces itself.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! length of the key (one character, one short of the published maximum, exactly at it,
//! one past it, many times past it), what the key is BUILT from (ASCII letters, the
//! published punctuation set, multi-byte letters whose byte length is twice their
//! character count), which resource type carries the tag, and which operation names the
//! key.

use serde_json::{json, Value};
// AWS's published per-key length limit, taken from the service's own constant rather than
// restated here, so the boundary rows cannot drift from the bound the handlers enforce.
use service_com_amazonaws_mediapackage_mediapackage::state::MAX_TAG_KEY_CHARS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code};
use crate::resource_tags::{
    create_with_tags, described_tags, list_tags, one_tag, tags_path, untag_query, Kind,
    PARENT_CHANNEL,
};

/// The tag set every stored fixture below carries, and which a refused request must leave
/// exactly as it was.
fn baseline_tags() -> Value {
    json!({ "team": "video", "env": "staging" })
}

/// A key of `characters` characters built by repeating `alphabet` — so a row states the
/// LENGTH it is about and nothing else, and a multi-byte alphabet makes the key's byte
/// length differ from its character count.
fn key_of(alphabet: &str, characters: usize) -> String {
    alphabet.chars().cycle().take(characters).collect()
}

/// One tag key, and whether a caller may name it.
struct KeyLengthRow {
    /// What the row is about, for the assertion messages.
    case: String,
    key: String,
    /// `true` when the key is within the published bound, so every door accepts it.
    accepted: bool,
}

/// The tag keys the length rule decides. The rows differ along the key's length — the
/// shortest key, one character short of the published maximum, exactly at it, one past it
/// and many times past it — and along the alphabet it is built from: ASCII letters, the
/// punctuation set AWS publishes for tags (`_ . : / = + - @`, as
/// `state::validate_tag_key`'s own refusal spells it), and multi-byte letters, which the
/// bound counts as one character each however many bytes they take.
fn key_length_cases() -> Vec<KeyLengthRow> {
    let rows: Vec<(String, &str, usize, bool)> = vec![
        ("a one-character key".to_string(), "k", 1, true),
        (
            "a key one character short of the published maximum".to_string(),
            "k",
            MAX_TAG_KEY_CHARS - 1,
            true,
        ),
        (
            "a key exactly at the published maximum".to_string(),
            "k",
            MAX_TAG_KEY_CHARS,
            true,
        ),
        (
            "a key at the published maximum drawn from the published punctuation set".to_string(),
            "_.:/=+-@",
            MAX_TAG_KEY_CHARS,
            true,
        ),
        (
            "a key at the published maximum in multi-byte letters".to_string(),
            "é",
            MAX_TAG_KEY_CHARS,
            true,
        ),
        (
            "a key one character past the published maximum".to_string(),
            "k",
            MAX_TAG_KEY_CHARS + 1,
            false,
        ),
        (
            "a key one multi-byte character past the published maximum".to_string(),
            "é",
            MAX_TAG_KEY_CHARS + 1,
            false,
        ),
        (
            "a key many times longer than the published maximum".to_string(),
            "key-",
            MAX_TAG_KEY_CHARS * 10,
            false,
        ),
    ];
    rows.into_iter()
        .map(|(case, alphabet, characters, accepted)| KeyLengthRow {
            case,
            key: key_of(alphabet, characters),
            accepted,
        })
        .collect()
}

/// The requirement the refusal of `key` must state: the bound it broke and the length it
/// actually had, so a caller can fix the request from the error alone.
fn length_requirement(key: &str) -> String {
    format!(
        "may be at most {MAX_TAG_KEY_CHARS} characters, got {}",
        key.chars().count()
    )
}

/// Assert `(status, body)` is this service's 422 refusal keyed by `member`, and answer the
/// requirement it stated for that member — so two doors' sentences can be compared.
fn refusal_requirement(case: &str, status: u16, body: &Value, member: &str, key: &str) -> String {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body:?}"
    );
    let reported = body
        .pointer(&format!("/errors/{member}"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let requirement = length_requirement(key);
    assert!(
        reported.contains(&requirement),
        "case {case:?}: expected the {member:?} requirement to state {requirement:?}, got \
         {body:?}"
    );
    assert!(
        reported.contains(&format!("{key:?}")),
        "case {case:?}: expected the {member:?} requirement to name the key it is about, got \
         {body:?}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(format!("{member} {reported}").as_str()),
        "case {case:?}: expected the sentence to be built from the reported reason, got {body:?}"
    );
    reported
}

/// Assert the stored resource of `kind` with id `id` carries exactly `expected`, through
/// both readers that report a tag set.
async fn assert_tags(
    router: &MultiServiceRouter,
    case: &str,
    kind: &Kind,
    id: &str,
    expected: &Value,
) {
    let arn = kind.arn(id);
    let (status, listed) = list_tags(router, &arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {listed:?}"
    );
    assert_eq!(
        &listed, expected,
        "case {case:?}: expected the tag set {expected:?}, got {listed:?}"
    );
    let (_, described) = call(router, "GET", &kind.describe_path(id), None).await;
    assert_eq!(
        &described_tags(&described),
        expected,
        "case {case:?}: the stored record's tags disagree, got {described:?}"
    );
}

#[tokio::test]
async fn tag_and_untag_judge_a_key_by_the_one_length_bound() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        for (index, row) in key_length_cases().into_iter().enumerate() {
            let id = format!("key-length-{}-{index}", kind.label().replace(' ', "-"));
            create_with_tags(&router, &kind, &id, &baseline_tags()).await;
            let arn = kind.arn(&id);

            let tag_case = format!("tagging a {} with {}", kind.label(), row.case);
            let untag_case = format!("untagging a {} by {}", kind.label(), row.case);
            let body = json!({ "tags": one_tag(&row.key, "v") });
            let (tag_status, tagged) =
                call(&router, "POST", &tags_path(&arn), Some(&body)).await;
            let untag_uri = untag_query(&arn, &[&row.key]);

            if row.accepted {
                // Within the bound the key is the caller's own: `TagResource` stores it,
                // both readers report it, and `UntagResource` removes the very key that
                // was stored — the round trip the rule is about.
                assert_eq!(
                    tag_status, 204,
                    "case {tag_case:?}: TagResource expected 204, got {tag_status} {tagged:?}"
                );
                let mut stored = baseline_tags();
                stored[row.key.as_str()] = json!("v");
                assert_tags(&router, &tag_case, &kind, &id, &stored).await;

                let (status, answered) = call(&router, "DELETE", &untag_uri, None).await;
                assert_eq!(
                    status, 204,
                    "case {untag_case:?}: UntagResource expected 204, got {status} {answered:?}"
                );
                assert_tags(&router, &untag_case, &kind, &id, &baseline_tags()).await;
                continue;
            }

            // Past the bound BOTH doors refuse, with the same requirement under the member
            // each request carried, and neither writes: the key the service would never
            // have let a caller store is not one it reports having removed either.
            let stored_requirement =
                refusal_requirement(&tag_case, tag_status, &tagged, "tags", &row.key);
            let (status, answered) = call(&router, "DELETE", &untag_uri, None).await;
            let removed_requirement =
                refusal_requirement(&untag_case, status, &answered, "tagKeys", &row.key);
            assert_eq!(
                removed_requirement, stored_requirement,
                "case {untag_case:?}: the two doors must state one requirement, got \
                 {removed_requirement:?} against {stored_requirement:?}"
            );
            assert_tags(&router, &untag_case, &kind, &id, &baseline_tags()).await;
        }
    }
}

#[tokio::test]
async fn every_door_that_names_a_tag_key_refuses_one_past_the_length_bound() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    // The ARNs no create ever stored, so a refusal cannot have come from the lookup: the
    // length of the key is decided from the request alone.
    let absent_channel = Kind::Channel.arn("length-never-created-channel");
    let absent_endpoint = Kind::OriginEndpoint.arn("length-never-created-endpoint");

    for (index, row) in key_length_cases()
        .into_iter()
        .filter(|row| !row.accepted)
        .enumerate()
    {
        let tags = one_tag(&row.key, "v");
        let channel_id = format!("length-door-channel-{index}");
        let endpoint_id = format!("length-door-endpoint-{index}");
        let (channel_path, channel_body) = Kind::Channel.create_request(&channel_id, &tags);
        let (endpoint_path, endpoint_body) =
            Kind::OriginEndpoint.create_request(&endpoint_id, &tags);

        let doors: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
            (
                "CreateChannel",
                "POST",
                channel_path.to_string(),
                Some(channel_body),
                "tags",
            ),
            (
                "CreateOriginEndpoint",
                "POST",
                endpoint_path.to_string(),
                Some(endpoint_body),
                "tags",
            ),
            (
                "TagResource",
                "POST",
                tags_path(&absent_channel),
                Some(json!({ "tags": tags })),
                "tags",
            ),
            (
                "UntagResource",
                "DELETE",
                untag_query(&absent_endpoint, &[&row.key]),
                None,
                "tagKeys",
            ),
        ];

        for (door, method, uri, body, member) in doors {
            let case = format!("{door} carrying {}", row.case);
            let (status, answered) = call(&router, method, &uri, body.as_ref()).await;
            refusal_requirement(&case, status, &answered, member, &row.key);
        }

        // The refused creates stored nothing.
        for (kind, id) in [
            (Kind::Channel, &channel_id),
            (Kind::OriginEndpoint, &endpoint_id),
        ] {
            let case = format!("the refused create of a {} with {}", kind.label(), row.case);
            let (status, read) = call(&router, "GET", &kind.describe_path(id), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: the refused create stored something, got {status} {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_untag_naming_an_over_long_key_beside_a_removable_one_removes_neither() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        let id = format!("length-whole-{}", kind.label().replace(' ', "-"));
        create_with_tags(&router, &kind, &id, &baseline_tags()).await;
        let arn = kind.arn(&id);

        let over_long = key_of("k", MAX_TAG_KEY_CHARS + 1);
        let case = format!(
            "untagging a {} by one removable key and one key past the length bound",
            kind.label()
        );
        let uri = untag_query(&arn, &["team", &over_long]);
        let (status, answered) = call(&router, "DELETE", &uri, None).await;
        refusal_requirement(&case, status, &answered, "tagKeys", &over_long);
        assert_tags(&router, &case, &kind, &id, &baseline_tags()).await;

        // The control: the same removable key on its own is removed, so what the refusal
        // above protected is the whole request and not that key.
        let case = format!("untagging a {} by the removable key alone", kind.label());
        let (status, answered) = call(&router, "DELETE", &untag_query(&arn, &["team"]), None).await;
        assert_eq!(
            status, 204,
            "case {case:?}: UntagResource expected 204, got {status} {answered:?}"
        );
        assert_tags(&router, &case, &kind, &id, &json!({ "env": "staging" })).await;
    }
}
