// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tagging surface: `TagResource`, `GetTags` and
//! `UntagResource`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `CreateApiKey` helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `stage_lifecycle` and
//! `api_key_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the tag round trip over EVERY taggable resource kind this service stores (a REST
//!   API, a stage, an API key) crossed with a table of tag shapes — ASCII, non-ASCII,
//!   an empty value, the documented character set, and a key and a value at their
//!   published length bounds;
//! * the merge rule (`TagResource` "adds or updates") over a table of before/after
//!   collections;
//! * the `resourceArn` reading over a table spanning every way a value fails to name a
//!   taggable resource — not an ARN, another vendor's ARN, a resource path API Gateway
//!   does not tag, and a well-formed path whose record does not exist (including every
//!   taggable kind this emulator keeps no records of) — applied to all three
//!   operations;
//! * the tag bounds over each published bound and one past it, plus the reserved `aws:`
//!   prefix in several spellings;
//! * `UntagResource` over tables of named and unnamed keys.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    RESERVED_TAG_KEY_PREFIX, MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::api_key_lifecycle::create_key;
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::create_deployed_stage;
// The ARN builder, the `@httpLabel` percent-encoding and the `GetTags` call are the
// sibling `tag_reads_by_arn` suite's: one spelling of each for the whole tagging
// surface, so a write suite and a read suite cannot address a resource differently.
use crate::tag_reads_by_arn::{apigateway_arn, encode_label, get_tags};

/// The stage every suite below deploys, so a stage ARN has a record behind it.
const STAGE: &str = "prod";

/// The taggable resource kinds this emulator keeps no records of — the first path
/// segment of each one's read URI. Source: `UNSTORED_TAGGABLE_KINDS` in
/// `src/state.rs`, which lists them because the operations that would create a usage
/// plan, a domain name, a client certificate, a VPC link or a domain name access
/// association are still scaffolds. An ARN of one of these is well formed, so it
/// resolves to a missing resource rather than to a bad request.
const UNSTORED_KINDS: &[&str] = &[
    "usageplans",
    "domainnames",
    "clientcertificates",
    "vpclinks",
    "domainnameaccessassociations",
];

/// `/tags/{resourceArn}`: the ARN travels inside ONE path segment, so it is encoded the
/// way an SDK encodes a non-greedy `@httpLabel`.
fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", encode_label(resource_arn))
}

/// One row of the merge table: the case name, the tags the request carries, and the
/// collection the resource must be left with.
type MergeRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, Vec<(&'a str, &'a str)>);

/// One row of the bounds table: the case name, the tags the request carries, and
/// whether this service accepts them.
type BoundRow<'a> = (&'a str, Vec<(&'a str, &'a str)>, bool);

/// A tag collection as a request body or a response member renders it.
fn as_value<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> Value {
    Value::Object(
        tags.into_iter()
            .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
            .collect::<Map<String, Value>>(),
    )
}

/// The same, for the accumulated expectation a test keeps.
fn map_value(tags: &BTreeMap<String, String>) -> Value {
    as_value(tags.iter().map(|(key, value)| (key.as_str(), value.as_str())))
}

/// `TagResource` for `resource_arn` with `tags`.
async fn tag<'a>(
    gateway: &Gateway,
    resource_arn: &str,
    tags: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Answer {
    gateway
        .call(
            "PUT",
            &tags_uri(resource_arn),
            Some(json!({ "tags": as_value(tags) })),
        )
        .await
}

/// `UntagResource` for `resource_arn`, naming `keys` in the `tagKeys` query.
async fn untag(gateway: &Gateway, resource_arn: &str, keys: &[&str]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect();
    gateway
        .call(
            "DELETE",
            &format!("{}?{}", tags_uri(resource_arn), query.join("&")),
            None,
        )
        .await
}

/// Assert `GetTags` reports exactly `expected`.
fn assert_tags(answer: &Answer, expected: &BTreeMap<String, String>, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetTags, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body,
        json!({ "tags": map_value(expected) }),
        "case {case}: GetTags must report the stored collection {:?}, got {:?}",
        map_value(expected),
        answer.body
    );
}

/// Every taggable resource kind this service stores, built on one gateway: the case
/// name, the resource's ARN, and the URI its OWN reader answers on (so a tag written
/// through `TagResource` can be read back through the resource's body too).
async fn taggables(gateway: &Gateway, label: &str) -> Vec<(&'static str, String, String)> {
    let (api, _root) = create_api(gateway, label).await;
    create_deployed_stage(gateway, &api, STAGE).await;
    let key = create_key(gateway, label, json!({ "name": format!("{label}-key") })).await;
    let key_id = string_member(&key, "id", label);
    vec![
        (
            "a REST API",
            apigateway_arn(&format!("/restapis/{api}")),
            format!("/restapis/{api}"),
        ),
        (
            "a stage",
            apigateway_arn(&format!("/restapis/{api}/stages/{STAGE}")),
            format!("/restapis/{api}/stages/{STAGE}"),
        ),
        (
            "an API key",
            apigateway_arn(&format!("/apikeys/{key_id}")),
            format!("/apikeys/{key_id}"),
        ),
    ]
}

#[tokio::test]
async fn a_tag_written_on_any_taggable_resource_reads_back_through_get_tags_and_the_resource() {
    // Rows differ along what the rule generalises over: the character set of the key and
    // of the value, the entry count, an empty value, and the length of each at its
    // published bound. The lengths come from the `tags` member's own documentation —
    // "The tag key can be up to 128 characters … The tag value can be up to 256
    // characters" — and the character rows include non-ASCII, which AWS accepts.
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("one ASCII tag", vec![("team", "api")]),
        (
            "three tags at once",
            vec![("env", "test"), ("owner", "platform"), ("cost", "42")],
        ),
        (
            "the documented character set",
            vec![("key+-=._:/", "value+-=._:/")],
        ),
        ("an empty value", vec![("unset", "")]),
        ("a non-ASCII key and value", vec![("クラウド", "東京")]),
        ("a non-ASCII value only", vec![("city", "münchen")]),
        ("an emoji value", vec![("mood", "🚀")]),
        (
            "a key and a value at their published length bound",
            vec![(long_key.as_str(), long_value.as_str())],
        ),
    ];

    let gateway = Gateway::new();
    for (kind, resource_arn, reader) in taggables(&gateway, "tag-round-trip").await {
        // A resource created with no tags carries the EMPTY collection, not a missing
        // one.
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        assert_tags(
            &get_tags(&gateway, &resource_arn).await,
            &expected,
            &format!("{kind} before any tag"),
        );

        for (row, tags) in &rows {
            let case = format!("{kind} / {row}");
            let written = tag(&gateway, &resource_arn, tags.iter().copied()).await;
            assert_eq!(
                written.status, 204,
                "case {case}: expected the modelled 204 from TagResource, got {} with {:?}",
                written.status, written.body
            );
            assert!(
                written.body.is_null(),
                "case {case}: a 204 carries no body, got {:?}",
                written.body
            );
            for (key, value) in tags {
                expected.insert((*key).to_owned(), (*value).to_owned());
            }

            // The reporting read answers the stored collection, byte for byte.
            assert_tags(&get_tags(&gateway, &resource_arn).await, &expected, &case);

            // And so does the resource's own body: one record, not two copies.
            let read = gateway.call("GET", &reader, None).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 reading {reader}, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["tags"],
                map_value(&expected),
                "case {case}: the resource's own body must report the same tags, got {:?}",
                read.body["tags"]
            );
        }
    }
}

#[tokio::test]
async fn tagging_merges_into_the_collection_rather_than_replacing_it() {
    // Rows differ along how the request's keys relate to the stored ones: disjoint,
    // overlapping, identical, and none at all.
    let rows: Vec<MergeRow<'_>> = vec![
        (
            "a disjoint key is added",
            vec![("team", "api")],
            vec![("team", "api"), ("env", "test")],
        ),
        (
            "a named key takes the new value and the others keep theirs",
            vec![("env", "prod")],
            vec![("team", "api"), ("env", "prod")],
        ),
        (
            "re-setting a key to the value it has changes nothing",
            vec![("env", "prod")],
            vec![("team", "api"), ("env", "prod")],
        ),
        (
            "two keys at once, one new and one overwritten",
            vec![("env", "stage"), ("owner", "platform")],
            vec![("team", "api"), ("env", "stage"), ("owner", "platform")],
        ),
        (
            "a request naming no tag leaves the collection alone",
            vec![],
            vec![("team", "api"), ("env", "stage"), ("owner", "platform")],
        ),
    ];

    let gateway = Gateway::new();
    for (kind, resource_arn, _) in taggables(&gateway, "tag-merge").await {
        // Every row below merges into this starting collection in turn.
        let first = tag(&gateway, &resource_arn, [("team", "api"), ("env", "test")]).await;
        assert_eq!(
            first.status, 204,
            "case {kind} / the first tags: expected 204, got {} with {:?}",
            first.status, first.body
        );

        for (row, request, result) in &rows {
            let case = format!("{kind} / {row}");
            let answer = tag(&gateway, &resource_arn, request.iter().copied()).await;
            assert_eq!(
                answer.status, 204,
                "case {case}: expected 204 from TagResource, got {} with {:?}",
                answer.status, answer.body
            );
            let expected: BTreeMap<String, String> = result
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect();
            assert_tags(&get_tags(&gateway, &resource_arn).await, &expected, &case);
        }
    }
}

#[tokio::test]
async fn each_resource_carries_its_own_collection() {
    let gateway = Gateway::new();
    let resources = taggables(&gateway, "tag-isolation").await;

    // One distinct tag per resource, so a collection reported for the wrong record is
    // visible.
    for (kind, resource_arn, _) in &resources {
        let answer = tag(&gateway, resource_arn, [("owner", *kind)]).await;
        assert_eq!(
            answer.status, 204,
            "case {kind}: expected 204 from TagResource, got {} with {:?}",
            answer.status, answer.body
        );
    }
    for (kind, resource_arn, _) in &resources {
        let expected: BTreeMap<String, String> =
            [("owner".to_owned(), (*kind).to_owned())].into_iter().collect();
        assert_tags(
            &get_tags(&gateway, resource_arn).await,
            &expected,
            &format!("{kind} after every resource was tagged"),
        );
    }
}

#[tokio::test]
async fn a_resource_arn_is_read_as_the_arn_form_a_taggable_resource_takes() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "tag-arn-forms").await;
    create_deployed_stage(&gateway, &api, STAGE).await;
    let missing = "zzzzzzzzzz";

    // (case, resourceArn, status, error code). A value that names no taggable resource
    // at all is a BAD REQUEST — it can never resolve — while a well-formed ARN whose
    // record does not exist is a NOT FOUND.
    let mut rows: Vec<(String, String, u16, &str)> = vec![
        (
            "not an ARN at all".to_string(),
            "petstore".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "an ARN of another vendor".to_string(),
            "arn:aws:lambda:us-east-1:000000000000:function:tags".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "an ARN that stops before its resource".to_string(),
            apigateway_arn(""),
            400,
            "BadRequestException",
        ),
        (
            "a resource field that is not a path".to_string(),
            apigateway_arn("restapis/abc123"),
            400,
            "BadRequestException",
        ),
        (
            "a resource path API Gateway does not tag".to_string(),
            apigateway_arn(&format!("/restapis/{api}/deployments/abc123")),
            400,
            "BadRequestException",
        ),
        (
            "a resource path naming no id".to_string(),
            apigateway_arn("/restapis/"),
            400,
            "BadRequestException",
        ),
        (
            "a stage path naming no stage".to_string(),
            apigateway_arn(&format!("/restapis/{api}/stages/")),
            400,
            "BadRequestException",
        ),
        (
            "an unknown REST API".to_string(),
            apigateway_arn(&format!("/restapis/{missing}")),
            404,
            "NotFoundException",
        ),
        (
            "an unknown stage of a stored API".to_string(),
            apigateway_arn(&format!("/restapis/{api}/stages/{missing}")),
            404,
            "NotFoundException",
        ),
        (
            "a stage of an unknown API".to_string(),
            apigateway_arn(&format!("/restapis/{missing}/stages/{STAGE}")),
            404,
            "NotFoundException",
        ),
        (
            "an unknown API key".to_string(),
            apigateway_arn(&format!("/apikeys/{missing}")),
            404,
            "NotFoundException",
        ),
    ];
    // Every taggable kind whose records this emulator stores none of: the ARN is well
    // formed and names a kind AWS tags, so resolving it is a not-found.
    for kind in UNSTORED_KINDS {
        rows.push((
            format!("a {kind} entry, a kind this emulator stores none of"),
            apigateway_arn(&format!("/{kind}/whatever-1")),
            404,
            "NotFoundException",
        ));
    }

    for (row, resource_arn, status, code) in &rows {
        // The SAME reading, on all three operations that take a `resourceArn`.
        assert_failure(
            &tag(&gateway, resource_arn, [("team", "api")]).await,
            *status,
            code,
            &format!("TagResource / {row}"),
        );
        assert_failure(
            &get_tags(&gateway, resource_arn).await,
            *status,
            code,
            &format!("GetTags / {row}"),
        );
        assert_failure(
            &untag(&gateway, resource_arn, &["team"]).await,
            *status,
            code,
            &format!("UntagResource / {row}"),
        );
    }

    // The stored API and stage, by contrast, are tagged and read back — the refusals
    // above are about the ARN, not about the operations.
    for resource_arn in [
        apigateway_arn(&format!("/restapis/{api}")),
        apigateway_arn(&format!("/restapis/{api}/stages/{STAGE}")),
    ] {
        let answer = tag(&gateway, &resource_arn, [("team", "api")]).await;
        assert_eq!(
            answer.status, 204,
            "case TagResource({resource_arn}): expected 204, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_tag_outside_the_published_rules_is_refused_and_changes_nothing() {
    // Source: the `tags` member's own documentation — "The tag key can be up to 128
    // characters and must not start with aws:. The tag value can be up to 256
    // characters." Each bound is tested AT the bound (accepted, above) and one past it,
    // and the reserved prefix in several spellings beside keys that merely contain it.
    let over_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let over_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");
    let reserved_upper = format!(
        "{}region",
        RESERVED_TAG_KEY_PREFIX.to_uppercase()
    );
    let rows: Vec<BoundRow<'_>> = vec![
        (
            "a key one character past its bound",
            vec![(over_key.as_str(), "x")],
            false,
        ),
        (
            "a value one character past its bound",
            vec![("size", over_value.as_str())],
            false,
        ),
        ("an empty key", vec![("", "x")], false),
        (
            "the reserved prefix",
            vec![(reserved.as_str(), "x")],
            false,
        ),
        (
            "the reserved prefix in upper case",
            vec![(reserved_upper.as_str(), "x")],
            false,
        ),
        (
            "one accepted tag beside a refused one",
            vec![("fine", "yes"), (reserved.as_str(), "x")],
            false,
        ),
        (
            "the reserved token without its colon",
            vec![("aws", "not-reserved")],
            true,
        ),
        (
            "the reserved prefix away from the start",
            vec![("team-aws:api", "yes")],
            true,
        ),
    ];

    let gateway = Gateway::new();
    for (kind, resource_arn, _) in taggables(&gateway, "tag-bounds").await {
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for (row, tags, accepted) in &rows {
            let case = format!("{kind} / {row}");
            let answer = tag(&gateway, &resource_arn, tags.iter().copied()).await;
            if *accepted {
                assert_eq!(
                    answer.status, 204,
                    "case {case}: expected 204 inside the published tag rules, got {} with {:?}",
                    answer.status, answer.body
                );
                for (key, value) in tags {
                    expected.insert((*key).to_owned(), (*value).to_owned());
                }
            } else {
                assert_failure(&answer, 400, "BadRequestException", &case);
            }
            // A refused request stores NOTHING — not even the acceptable tags it
            // carried beside the refused one.
            assert_tags(&get_tags(&gateway, &resource_arn).await, &expected, &case);
        }
    }
}

#[tokio::test]
async fn the_tag_quota_is_on_the_resulting_collection() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "tag-quota").await;
    let resource_arn = apigateway_arn(&format!("/restapis/{api}"));

    // AWS's tagging limit is 50 tags per resource: the collection fills exactly to it.
    let filled: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|n| (format!("k{n}"), format!("v{n}")))
        .collect();
    let answer = tag(
        &gateway,
        &resource_arn,
        filled.iter().map(|(key, value)| (key.as_str(), value.as_str())),
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "case TagResource({MAX_TAGS} tags): expected 204 at the published limit, got {} with {:?}",
        answer.status, answer.body
    );
    let expected: BTreeMap<String, String> = filled.iter().cloned().collect();
    assert_tags(
        &get_tags(&gateway, &resource_arn).await,
        &expected,
        &format!("after {MAX_TAGS} tags"),
    );

    // A key the resource already carries does not grow the collection, so it is
    // accepted at the limit; a new key would, so it is refused.
    let overwrite = tag(&gateway, &resource_arn, [("k0", "replaced")]).await;
    assert_eq!(
        overwrite.status, 204,
        "case TagResource(an existing key at the limit): expected 204, got {} with {:?}",
        overwrite.status, overwrite.body
    );
    assert_failure(
        &tag(&gateway, &resource_arn, [("one-too-many", "x")]).await,
        400,
        "BadRequestException",
        "TagResource(a new key at the limit)",
    );
    // And a single request carrying more than the limit is refused outright.
    let too_many: Vec<(String, String)> = (0..=MAX_TAGS)
        .map(|n| (format!("over{n}"), format!("v{n}")))
        .collect();
    assert_failure(
        &tag(
            &gateway,
            &resource_arn,
            too_many.iter().map(|(key, value)| (key.as_str(), value.as_str())),
        )
        .await,
        400,
        "BadRequestException",
        "TagResource(more tags than the limit in one request)",
    );

    let mut after = expected.clone();
    after.insert("k0".to_owned(), "replaced".to_owned());
    assert_tags(
        &get_tags(&gateway, &resource_arn).await,
        &after,
        "after the refusals at the limit",
    );
}

#[tokio::test]
async fn untagging_removes_the_keys_it_names_and_refuses_a_key_the_resource_does_not_carry() {
    let gateway = Gateway::new();
    for (kind, resource_arn, _) in taggables(&gateway, "untag").await {
        let stored: Vec<(&str, &str)> = vec![
            ("team", "api"),
            ("env", "test"),
            ("owner", "platform"),
            ("クラウド", "東京"),
        ];
        let answer = tag(&gateway, &resource_arn, stored.iter().copied()).await;
        assert_eq!(
            answer.status, 204,
            "case {kind} / the stored tags: expected 204, got {} with {:?}",
            answer.status, answer.body
        );
        let mut expected: BTreeMap<String, String> = stored
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        // A key the resource does not carry is REFUSED, and the removal is
        // all-or-nothing: a request naming one unknown key beside known ones drops
        // nothing. Rows span an unknown key alone, mixed with a known one, an empty key
        // and a key that was already removed.
        let unknown: Vec<(&str, Vec<&str>)> = vec![
            ("one key the resource never carried", vec!["absent"]),
            ("an unknown key beside a known one", vec!["team", "absent"]),
            ("an empty key", vec![""]),
            ("a reserved key, which can never be stored", vec!["aws:region"]),
        ];
        for (row, keys) in &unknown {
            let case = format!("{kind} / {row}");
            assert_failure(
                &untag(&gateway, &resource_arn, keys).await,
                404,
                "NotFoundException",
                &case,
            );
            assert_tags(&get_tags(&gateway, &resource_arn).await, &expected, &case);
        }

        // The keys it names, and only those — including a non-ASCII key, which travels
        // percent-encoded in the query.
        let removals: Vec<(&str, Vec<&str>)> = vec![
            ("one key", vec!["env"]),
            ("two keys at once", vec!["team", "owner"]),
            ("a non-ASCII key", vec!["クラウド"]),
        ];
        for (row, keys) in &removals {
            let case = format!("{kind} / removing {row}");
            let answer = untag(&gateway, &resource_arn, keys).await;
            assert_eq!(
                answer.status, 204,
                "case {case}: expected the modelled 204 from UntagResource, got {} with {:?}",
                answer.status, answer.body
            );
            for key in keys {
                expected.remove(*key);
            }
            assert_tags(&get_tags(&gateway, &resource_arn).await, &expected, &case);
        }
        assert!(
            expected.is_empty(),
            "case {kind}: the rows above remove every stored key, {expected:?} is left"
        );

        // An emptied collection is still the empty collection, and a tag written after
        // it reads back: removal does not take the record with it.
        let again = tag(&gateway, &resource_arn, [("team", "api")]).await;
        assert_eq!(
            again.status, 204,
            "case {kind} / tagging after every tag was removed: expected 204, got {} with {:?}",
            again.status, again.body
        );
        expected.insert("team".to_owned(), "api".to_owned());
        assert_tags(
            &get_tags(&gateway, &resource_arn).await,
            &expected,
            &format!("{kind} / after tagging an emptied collection"),
        );
    }
}

#[tokio::test]
async fn reading_tags_does_not_change_what_a_later_caller_sees() {
    let gateway = Gateway::new();
    for (kind, resource_arn, _) in taggables(&gateway, "tag-stable").await {
        let answer = tag(&gateway, &resource_arn, [("team", "api"), ("env", "test")]).await;
        assert_eq!(
            answer.status, 204,
            "case {kind}: expected 204 from TagResource, got {} with {:?}",
            answer.status, answer.body
        );
        let expected: BTreeMap<String, String> = [
            ("team".to_owned(), "api".to_owned()),
            ("env".to_owned(), "test".to_owned()),
        ]
        .into_iter()
        .collect();

        let mut seen: Vec<Value> = Vec::new();
        for round in 0..3 {
            let read = get_tags(&gateway, &resource_arn).await;
            assert_tags(&read, &expected, &format!("{kind} / round {round}"));
            seen.push(read.body);
        }
        assert_eq!(
            seen[0], seen[2],
            "case {kind}: a read must not change the collection, got {:?} then {:?}",
            seen[0], seen[2]
        );
    }
}
