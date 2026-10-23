// SPDX-License-Identifier: CC0-1.0

//! `TagResource`, end to end over HTTP through the router this service mounts in
//! production.
//!
//! The rule every case here asserts: `TagResource` resolves its `resourceArn` to the
//! ONE stored profiling group that ARN names and MERGES the request's entries into that
//! group's single tag map — a key the resource already carries takes the request's
//! value, every key the request does not name keeps the value it had — within AWS's
//! published tagging limits, and answers 204 with no content. The write is then visible
//! through `ListTagsForResource` and through `DescribeProfilingGroup`, because there is
//! one stored map and not a second copy. A write carrying no tag at all is refused by
//! the REST front door (400 `BadRequestException`) ahead of every other check; a
//! `resourceArn` that is not a profiling-group ARN of this service is a bad input (400
//! `ValidationException`); a well-formed ARN this store holds no resource for is 404
//! `ResourceNotFoundException`; and a request outside the tagging limits is refused
//! before anything is stored (400 `ValidationException`).
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds and the
//! `@httpLabel` encoding is the one [`crate::untag_resource`] already applies; the
//! reader each case reads back through is the `ListTagsForResource` request
//! [`crate::resource_tags`] covers on its own. Rows vary what the rule generalises
//! over: how many tags one write carries, how many writes are made, whether a key
//! already has a value, the length and character class of keys and values (at each
//! published bound and one past it), how many tags the resource was created with, and
//! which resource an ARN selects.

use std::collections::BTreeMap;

use crate::profiling_group_lifecycle::{group_arn, Answer, Harness, ACCOUNT, REGION};
use crate::untag_resource::encode;

/// The sentence the shared REST front door refuses a request body with
/// (`service_common::front_door::INVALID_REQUEST_BODY`), restated here so this suite
/// pins the wire text rather than importing the value it compares against.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// AWS's published tagging limits, restated here for the same reason: the most tags one
/// resource holds, and the longest a key and a value may be. They are
/// `state::MAX_TAGS_PER_RESOURCE`, `state::MAX_TAG_KEY_CHARS` and
/// `state::MAX_TAG_VALUE_CHARS`.
const MAX_TAGS_PER_RESOURCE: usize = 50;
const MAX_TAG_KEY_CHARS: usize = 128;
const MAX_TAG_VALUE_CHARS: usize = 256;

/// `count` distinct tags, `tag-00`..`tag-NN`, for a case about how MANY tags a resource
/// holds rather than about what they say.
fn distinct_tags(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("tag-{index:02}"), format!("value-{index:02}")))
        .collect()
}

fn tag_map<const N: usize>(entries: [(&str, &str); N]) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn tag_object(tags: &BTreeMap<String, String>) -> serde_json::Value {
    serde_json::Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::from(value.clone())))
            .collect(),
    )
}

/// `CreateProfilingGroup` with the tags the group is born with, answering the response
/// as it came back — so a case can assert a create this service must REFUSE.
async fn create_answer(
    harness: &Harness,
    name: &str,
    tags: &BTreeMap<String, String>,
    token: &str,
) -> Answer {
    let mut body = serde_json::Map::new();
    body.insert(
        "profilingGroupName".to_string(),
        serde_json::Value::from(name),
    );
    if !tags.is_empty() {
        body.insert("tags".to_string(), tag_object(tags));
    }
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken={token}"),
            "application/json",
            serde_json::to_vec(&serde_json::Value::Object(body)).expect("a serializable body"),
        )
        .await
}

/// `CreateProfilingGroup` that must succeed, answering the ARN the service minted for
/// the group — the value every tag call below addresses it by.
async fn create(
    harness: &Harness,
    name: &str,
    tags: &BTreeMap<String, String>,
    token: &str,
) -> String {
    let created = create_answer(harness, name, tags, token).await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateProfilingGroup must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    // `profilingGroup` is the response's `@httpPayload`, so the description IS the body.
    created
        .json()
        .pointer("/arn")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: no arn in {}", created.json()))
        .to_string()
}

/// `TagResource`: `POST /tags/{resourceArn}`, the ARN in one path segment and the tag
/// map in the body, exactly as the model binds them.
async fn tag(harness: &Harness, resource_arn: &str, tags: &BTreeMap<String, String>) -> Answer {
    harness
        .send(
            "POST",
            &format!("/tags/{}", encode(resource_arn)),
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "tags": tag_object(tags) }))
                .expect("a serializable body"),
        )
        .await
}

/// `ListTagsForResource`: `GET /tags/{resourceArn}` — the reader each write is checked
/// against.
async fn list_tags(harness: &Harness, resource_arn: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/tags/{}", encode(resource_arn)),
            "application/json",
            Vec::new(),
        )
        .await
}

async fn delete_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "DELETE",
            &format!("/profilingGroups/{name}"),
            "application/json",
            Vec::new(),
        )
        .await
}

/// The `tags` member of a `ListTagsForResource` or `DescribeProfilingGroup` answer, as
/// a comparable map. An absent member counts as empty, which is what "carries no tag"
/// means.
fn tags_of(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .json()
        .pointer("/tags")
        .and_then(serde_json::Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn error_message(answer: &Answer) -> String {
    answer
        .json()
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// One tagging case: a group, the tags it is created with, and the successive
/// `TagResource` calls made against it.
#[derive(Debug)]
struct TagCase {
    name: String,
    at_creation: BTreeMap<String, String>,
    writes: Vec<BTreeMap<String, String>>,
}

impl TagCase {
    /// What the group's tag map must hold once every write has been applied: the
    /// creation tags, with each write's entries merged over them in order.
    fn expected(&self) -> BTreeMap<String, String> {
        let mut expected = self.at_creation.clone();
        for write in &self.writes {
            for (key, value) in write {
                expected.insert(key.clone(), value.clone());
            }
        }
        expected
    }
}

/// The table: rows differ in how many tags the group starts with, how many entries one
/// write carries, how many writes are made, whether a write re-tags a key that already
/// has a value, and the size and alphabet of keys and values.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            name: "tagged-demo-app".to_string(),
            at_creation: tag_map([("env", "dev")]),
            writes: vec![tag_map([("owner", "profiling")])],
        },
        TagCase {
            // Born with no tag at all, then given three in one call.
            name: "tagged_handler-9".to_string(),
            at_creation: BTreeMap::new(),
            writes: vec![tag_map([("a", "1"), ("b", "2"), ("c", "3")])],
        },
        TagCase {
            // A write that RE-TAGS an existing key, beside one it leaves alone.
            name: "Tagged_Mixed-Case".to_string(),
            at_creation: tag_map([("team", "payments"), ("tier", "prod")]),
            writes: vec![tag_map([("team", "platform")])],
        },
        TagCase {
            // Several writes in a row, one of them carrying an empty VALUE.
            name: "t".to_string(),
            at_creation: BTreeMap::new(),
            writes: vec![
                tag_map([("first", "1")]),
                tag_map([("second", "")]),
                tag_map([("first", "1-again"), ("third", "3")]),
            ],
        },
        TagCase {
            // The longest group name the model allows (ProfilingGroupName length
            // 1..255), with a key and a value at the published tagging bounds.
            name: "g".repeat(255),
            at_creation: tag_map([("only", "one")]),
            writes: vec![BTreeMap::from([(
                "k".repeat(MAX_TAG_KEY_CHARS),
                "v".repeat(MAX_TAG_VALUE_CHARS),
            )])],
        },
        TagCase {
            // Non-ASCII and punctuation in both key and value.
            name: "tagged-unicode".to_string(),
            at_creation: tag_map([("équipe", "paiements")]),
            writes: vec![tag_map([("cost:centre", "ñ-prod"), ("path/like", "a b\tc")])],
        },
    ]
}

/// Every write lands in the ONE stored tag map, and both readers report it.
#[tokio::test]
async fn a_tag_write_is_read_back_from_the_one_stored_tag_map() {
    let harness = Harness::new();

    for (index, case) in tag_cases().iter().enumerate() {
        let arn = create(
            &harness,
            &case.name,
            &case.at_creation,
            &format!("tagtok-{index}"),
        )
        .await;
        assert_eq!(
            arn,
            group_arn(&case.name),
            "case {case:?}: the created group's ARN must be the one this service mints"
        );

        // Before any write, the reader reports exactly the creation tags.
        let initial = list_tags(&harness, &arn).await;
        assert_eq!(
            initial.status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {} {}",
            initial.status,
            String::from_utf8_lossy(&initial.body)
        );
        assert_eq!(
            tags_of(&initial),
            case.at_creation,
            "case {case:?}: before any write the tags must be the creation tags, got {:?}",
            tags_of(&initial)
        );

        let mut applied = case.at_creation.clone();
        for write in &case.writes {
            let tagged = tag(&harness, &arn, write).await;
            assert_eq!(
                tagged.status, 204,
                "case {case:?} (write {write:?}): TagResource must answer 204, got {} {}",
                tagged.status,
                String::from_utf8_lossy(&tagged.body)
            );
            // `TagResourceResponse` models no member, so there is nothing for the answer
            // to carry: the framework renders that empty output as an empty body or as
            // `{}`, and either way the write reports nothing back to the caller.
            let returned = String::from_utf8_lossy(&tagged.body).trim().to_string();
            assert!(
                returned.is_empty() || returned == "{}",
                "case {case:?} (write {write:?}): a 204 answers with no content, got {returned:?}"
            );

            for (key, value) in write {
                applied.insert(key.clone(), value.clone());
            }
            let listed = list_tags(&harness, &arn).await;
            assert_eq!(
                tags_of(&listed),
                applied,
                "case {case:?} (write {write:?}): wrong tags after the write, got {:?}",
                tags_of(&listed)
            );
        }

        let listed = list_tags(&harness, &arn).await;
        assert_eq!(
            tags_of(&listed),
            case.expected(),
            "case {case:?}: wrong tags after every write, got {:?}",
            tags_of(&listed)
        );

        // The same map, reached through the profiling-group reader: one stored record,
        // not two copies.
        let described = harness.describe(&case.name).await;
        assert_eq!(
            tags_of(&described),
            case.expected(),
            "case {case:?}: DescribeProfilingGroup must report the same tag map, got {:?}",
            tags_of(&described)
        );

        // A read changes nothing a later caller sees.
        let again = list_tags(&harness, &arn).await;
        assert_eq!(
            tags_of(&again),
            tags_of(&listed),
            "case {case:?}: ListTagsForResource must be stable across reads, got {:?}",
            tags_of(&again)
        );
    }
}

/// AWS's published tagging limits hold on every path that writes a resource's tags, at
/// each bound and one past it, and a refused write stores nothing: at most
/// `MAX_TAGS_PER_RESOURCE` tags per resource, a key of 1..`MAX_TAG_KEY_CHARS`
/// characters, a value of at most `MAX_TAG_VALUE_CHARS`.
#[tokio::test]
async fn a_tag_write_is_bounded_by_the_published_tagging_limits() {
    let harness = Harness::new();

    // (case, the tags one request carries, whether the service must accept it)
    let rows: Vec<(&str, BTreeMap<String, String>, bool)> = vec![
        ("one tag", distinct_tags(1), true),
        (
            "at the per-resource cap",
            distinct_tags(MAX_TAGS_PER_RESOURCE),
            true,
        ),
        (
            "one past the per-resource cap",
            distinct_tags(MAX_TAGS_PER_RESOURCE + 1),
            false,
        ),
        (
            "a key of one character",
            BTreeMap::from([("k".to_string(), "v".to_string())]),
            true,
        ),
        (
            "a key at the key-length bound",
            BTreeMap::from([("k".repeat(MAX_TAG_KEY_CHARS), "v".to_string())]),
            true,
        ),
        (
            "a key one character past the key-length bound",
            BTreeMap::from([("k".repeat(MAX_TAG_KEY_CHARS + 1), "v".to_string())]),
            false,
        ),
        (
            "an empty key",
            BTreeMap::from([(String::new(), "v".to_string())]),
            false,
        ),
        (
            "an empty value",
            BTreeMap::from([("k".to_string(), String::new())]),
            true,
        ),
        (
            "a value at the value-length bound",
            BTreeMap::from([("k".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS))]),
            true,
        ),
        (
            "a value one character past the value-length bound",
            BTreeMap::from([("k".to_string(), "v".repeat(MAX_TAG_VALUE_CHARS + 1))]),
            false,
        ),
        (
            // Character counts, not byte counts: 128 three-byte characters is a key at
            // the bound, not past it.
            "a key at the bound in three-byte characters",
            BTreeMap::from(["é".repeat(MAX_TAG_KEY_CHARS)].map(|key| (key, "v".to_string()))),
            true,
        ),
    ];

    for (index, (case, tags, accepted)) in rows.iter().enumerate() {
        // Each row gets its own resource, so a row's refusal is read against a map only
        // that row wrote.
        let name = format!("bounded-{index}");
        let arn = create(&harness, &name, &BTreeMap::new(), &format!("bound-{index}")).await;

        let written = tag(&harness, &arn, tags).await;
        let listed = list_tags(&harness, &arn).await;
        if *accepted {
            assert_eq!(
                written.status, 204,
                "case {case:?}: must be accepted, got {} {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            assert_eq!(
                tags_of(&listed),
                *tags,
                "case {case:?}: the accepted tags must be what is stored, got {:?}",
                tags_of(&listed)
            );
        } else {
            assert_eq!(
                written.status, 400,
                "case {case:?}: must be refused with 400, got {} {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
            assert_eq!(
                written.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                String::from_utf8_lossy(&written.body)
            );
            assert!(
                tags_of(&listed).is_empty(),
                "case {case:?}: a refused write must store nothing, got {:?}",
                tags_of(&listed)
            );
        }

        // The same limits hold on the other path that writes this map: a create.
        let created = create_answer(
            &harness,
            &format!("created-bounded-{index}"),
            tags,
            &format!("create-bound-{index}"),
        )
        .await;
        let expected_status = if *accepted { 201 } else { 400 };
        assert_eq!(
            created.status, expected_status,
            "case {case:?}: CreateProfilingGroup must answer {expected_status}, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }
}

/// A resource already at the cap refuses a NEW key and still accepts a re-tag of a key
/// it holds, which never grows the map.
#[tokio::test]
async fn a_resource_at_the_tag_cap_accepts_a_retag_but_no_new_key() {
    let harness = Harness::new();
    let arn = create(&harness, "at-the-cap", &BTreeMap::new(), "cap-0").await;
    let full = distinct_tags(MAX_TAGS_PER_RESOURCE);
    assert_eq!(
        tag(&harness, &arn, &full).await.status,
        204,
        "filling the resource to the cap must be accepted"
    );

    let one_more = tag(&harness, &arn, &tag_map([("one-more", "v")])).await;
    assert_eq!(
        one_more.status, 400,
        "case \"a new key at the cap\": must be refused with 400, got {} {}",
        one_more.status,
        String::from_utf8_lossy(&one_more.body)
    );
    assert_eq!(
        one_more.error_code(),
        "ValidationException",
        "case \"a new key at the cap\": expected ValidationException, got {}",
        String::from_utf8_lossy(&one_more.body)
    );
    assert_eq!(
        tags_of(&list_tags(&harness, &arn).await),
        full,
        "case \"a new key at the cap\": the refusal must store nothing"
    );

    let retagged = tag(&harness, &arn, &tag_map([("tag-00", "re-tagged")])).await;
    assert_eq!(
        retagged.status, 204,
        "case \"a re-tag at the cap\": must be accepted, got {} {}",
        retagged.status,
        String::from_utf8_lossy(&retagged.body)
    );
    let mut expected = full.clone();
    expected.insert("tag-00".to_string(), "re-tagged".to_string());
    assert_eq!(
        tags_of(&list_tags(&harness, &arn).await),
        expected,
        "case \"a re-tag at the cap\": the stored value must be the re-tagged one"
    );
}

/// Tagging one resource leaves every other resource's tags alone.
#[tokio::test]
async fn a_tag_write_touches_only_the_resource_its_arn_names() {
    let harness = Harness::new();
    let cases = tag_cases();

    let mut arns = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        arns.push(
            create(
                &harness,
                &case.name,
                &case.at_creation,
                &format!("tagtok-{index}"),
            )
            .await,
        );
    }

    for (index, case) in cases.iter().enumerate() {
        let write = tag_map([("written-by", "this-case")]);
        let tagged = tag(&harness, &arns[index], &write).await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: TagResource must answer 204, got {} {}",
            tagged.status,
            String::from_utf8_lossy(&tagged.body)
        );

        for (other, other_case) in cases.iter().enumerate() {
            let expected = if other <= index {
                let mut expected = other_case.at_creation.clone();
                expected.insert("written-by".to_string(), "this-case".to_string());
                expected
            } else {
                other_case.at_creation.clone()
            };
            let listed = list_tags(&harness, &arns[other]).await;
            assert_eq!(
                tags_of(&listed),
                expected,
                "case {case:?}: tagging it must not change {:?}, got {:?}",
                other_case.name,
                tags_of(&listed)
            );
        }
    }
}

/// A write carrying no tag at all is refused by the REST front door — ahead of both the
/// input check on the ARN and the lookup, so the SAME refusal answers an ARN that is
/// malformed and one that names nothing — and it persists nothing.
#[tokio::test]
async fn a_tag_write_with_no_tags_is_refused_as_a_bad_request_body() {
    let harness = Harness::new();
    let existing = create(&harness, "tagged-group", &tag_map([("env", "dev")]), "tok-0").await;

    // (case, the ARN the empty write is addressed to)
    let rows: Vec<(&str, String)> = vec![
        ("a group that exists", existing.clone()),
        ("a group that does not exist", group_arn("never-created")),
        (
            "an ARN of another vendor",
            format!("arn:aws:codeguru-reviewer:{REGION}:{ACCOUNT}:profilingGroup/tagged-group"),
        ),
        ("a value that is not an ARN", "tagged-group".to_string()),
    ];

    for (case, resource_arn) in rows {
        let refused = tag(&harness, &resource_arn, &BTreeMap::new()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an empty tags map must be refused with 400, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {}",
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            error_message(&refused),
            INVALID_REQUEST_BODY,
            "case {case:?}: expected the front door's sentence, got {}",
            String::from_utf8_lossy(&refused.body)
        );
    }

    let listed = list_tags(&harness, &existing).await;
    assert_eq!(
        tags_of(&listed),
        tag_map([("env", "dev")]),
        "a refused write must persist nothing, got {:?}",
        tags_of(&listed)
    );
}

/// A `resourceArn` that names no profiling group of this service is refused on the
/// INPUT (400 `ValidationException`) rather than reported as a missing resource.
#[tokio::test]
async fn an_arn_that_names_no_profiling_group_is_refused_on_the_input() {
    let harness = Harness::new();
    create(&harness, "tagged-group", &tag_map([("env", "dev")]), "tok-0").await;

    // (case, the value sent as resourceArn)
    let rows: Vec<(&str, String)> = vec![
        ("a value that is not an ARN", "tagged-group".to_string()),
        (
            "an ARN of another vendor",
            format!("arn:aws:codeguru-reviewer:{REGION}:{ACCOUNT}:profilingGroup/tagged-group"),
        ),
        (
            "this vendor in another case",
            format!("arn:aws:CodeGuru-Profiler:{REGION}:{ACCOUNT}:profilingGroup/tagged-group"),
        ),
        (
            "another resource type of this vendor",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profile/tagged-group"),
        ),
        (
            "a resource type this one is a prefix of",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroupX/tagged-group"),
        ),
        (
            "a profilingGroup with no name",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/"),
        ),
        (
            "an ARN that stops before its resource",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}"),
        ),
        (
            "an ARN with no partition",
            format!("arn::codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/tagged-group"),
        ),
    ];

    for (case, resource_arn) in rows {
        let tagged = tag(&harness, &resource_arn, &tag_map([("k", "v")])).await;
        assert_eq!(
            tagged.status, 400,
            "case {case:?}: TagResource must refuse the input with 400, got {} {}",
            tagged.status,
            String::from_utf8_lossy(&tagged.body)
        );
        assert_eq!(
            tagged.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException from TagResource, got {}",
            String::from_utf8_lossy(&tagged.body)
        );
        assert!(
            error_message(&tagged).contains("resourceArn"),
            "case {case:?}: the refusal must name the member, got {:?}",
            error_message(&tagged)
        );
    }

    // The resource that does exist still carries exactly what it was created with.
    let listed = list_tags(&harness, &group_arn("tagged-group")).await;
    assert_eq!(
        tags_of(&listed),
        tag_map([("env", "dev")]),
        "a refused write must persist nothing, got {:?}",
        tags_of(&listed)
    );
}

/// A well-formed profiling-group ARN that names no stored group is 404
/// `ResourceNotFoundException`, carrying the service's own sentence for the group it
/// could not find — whether the group was never created, has been deleted, or lives in
/// another account, Region or partition.
#[tokio::test]
async fn a_tag_write_to_an_arn_naming_no_stored_group_is_not_found() {
    let harness = Harness::new();
    create(&harness, "tagged-group", &tag_map([("env", "dev")]), "tok-0").await;
    create(&harness, "doomed-group", &BTreeMap::new(), "tok-1").await;
    let deleted = delete_group(&harness, "doomed-group").await;
    assert_eq!(
        deleted.status,
        204,
        "DeleteProfilingGroup must answer 204, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // (case, the ARN, the group name the refusal must name)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a group that was never created",
            group_arn("never-created"),
            "never-created".to_string(),
        ),
        (
            "a group that was deleted",
            group_arn("doomed-group"),
            "doomed-group".to_string(),
        ),
        (
            "the longest name the model allows, never created",
            group_arn(&"z".repeat(255)),
            "z".repeat(255),
        ),
        (
            "an existing name in another account",
            format!("arn:aws:codeguru-profiler:{REGION}:999999999999:profilingGroup/tagged-group"),
            "tagged-group".to_string(),
        ),
        (
            "an existing name in another Region",
            format!("arn:aws:codeguru-profiler:eu-west-1:{ACCOUNT}:profilingGroup/tagged-group"),
            "tagged-group".to_string(),
        ),
        (
            "an existing name in another partition",
            format!("arn:aws-cn:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/tagged-group"),
            "tagged-group".to_string(),
        ),
    ];

    for (case, resource_arn, named) in rows {
        let answer = tag(&harness, &resource_arn, &tag_map([("k", "v")])).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let message = error_message(&answer);
        assert!(
            message.contains(&named) && message.contains("was not found"),
            "case {case:?}: the refusal must name the group it could not find, got {message:?}"
        );
    }

    // The group that does exist is untouched by every refusal above.
    let listed = list_tags(&harness, &group_arn("tagged-group")).await;
    assert_eq!(
        tags_of(&listed),
        tag_map([("env", "dev")]),
        "the stored group's tags must be unchanged, got {:?}",
        tags_of(&listed)
    );
}
