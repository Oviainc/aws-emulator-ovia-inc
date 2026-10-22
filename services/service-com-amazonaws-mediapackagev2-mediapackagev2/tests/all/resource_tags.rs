// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for MediaPackageV2 resource tagging — `ListTagsForResource`,
//! `TagResource` and `UntagResource` — driven over HTTP through the same router the
//! emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A tag map belongs to the ONE resource its ARN names. `ListTagsForResource` reports
//!    exactly that resource's tag map, at every taggable level of the hierarchy (channel
//!    group, channel, origin endpoint), and reports the same map that level's own `Get*`
//!    does.
//! 2. `TagResource` MERGES into that map: a key the resource does not carry is added, a
//!    key it already carries takes the submitted value, and a key the request does not
//!    name keeps the value it had.
//! 3. `UntagResource` removes exactly the named keys; a key the resource does not carry
//!    is removed silently, so the call is idempotent and never fails for it.
//! 4. The two ARN failure classes, over all three operations (one row per class; the
//!    sibling `rejection_rule_and_negative_paths` suite owns the full tables, for
//!    `ListTagsForResource` and `TagResource`): a string that is not an ARN for one of
//!    this service's resources is refused `BadRequestException` (400), a well-formed ARN
//!    naming a resource this emulator does not hold is refused `NotFoundException` (404),
//!    and a refused call changes no resource's tags.
//! 5. Listing tags is a read: it changes nothing a later caller sees.
//! 6. The tag ceilings are AWS's published limits, and a character limit counts
//!    characters rather than bytes — tested at the bound and one past it, with a refused
//!    request storing nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

// The emulator client, the error-envelope reader and the smallest acceptable
// `CreateOriginEndpoint` body are the crate's shared test harness, declared `pub(crate)`
// in `channel_lifecycle` for exactly this reuse, and the ARN label escaper is the one
// `rejection_rule_and_negative_paths` already shares (`pct` below is an alias for it, so
// this suite adds no second encoder). Only the tagging-specific calls are this suite's own.
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::encoded as pct;

/// The tagging-specific calls this suite adds to the shared client: the three tagging
/// operations, plus building and reading back a whole hierarchy. An inherent impl on the
/// shared type, so every suite still drives the emulator through one client.
impl Emulator {
    /// `GET /tags/{ResourceArn}`.
    async fn list_tags(&self, arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags/{}", pct(arn)), None).await
    }

    /// `POST /tags/{ResourceArn}` with the tag map in the body member the model names
    /// `tags` (`@jsonName("tags")` on `TagResourceRequest$Tags`).
    async fn tag(&self, arn: &str, tags: &BTreeMap<String, String>) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/tags/{}", pct(arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    /// `DELETE /tags/{ResourceArn}?tagKeys=..`: `TagKeys` is an `@httpQuery` list, so one
    /// `tagKeys=` parameter per key. The keys the rows below remove are drawn from the
    /// query-safe part of the tag alphabet (a key carrying a space or a `/` is exercised
    /// on the `TagResource` body path, where it needs no escaping), so the shared label
    /// escaper covers every URI this suite builds.
    async fn untag(&self, arn: &str, keys: &[&str]) -> (u16, Value) {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", pct(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.call("DELETE", &format!("/tags/{}?{query}", pct(arn)), None)
            .await
    }

    /// Create a channel group, a channel inside it and an origin endpoint inside that,
    /// all named after `suffix`, giving `tagged` the tag map `tags` and the other two
    /// levels no tags at all. Returns every level's ARN, so each can be addressed by the
    /// tagging operations.
    async fn create_hierarchy(
        &self,
        suffix: &str,
        tagged: Level,
        tags: &BTreeMap<String, String>,
    ) -> Vec<(Level, String)> {
        let (group, channel, endpoint) = names(suffix);
        let tags_for = |level: Level, mut body: Value| -> Value {
            if level == tagged {
                body[level.create_tags_member()] = json!(tags);
            }
            body
        };

        let (status, created_group) = self
            .create_group(&group, tags_for(Level::Group, json!({})))
            .await;
        assert_eq!(
            status, 200,
            "CreateChannelGroup {group} expected 200, got {status} {created_group}"
        );

        let (status, created_channel) = self
            .create_channel(&group, &channel, tags_for(Level::Channel, json!({})))
            .await;
        assert_eq!(
            status, 200,
            "CreateChannel {channel} expected 200, got {status} {created_channel}"
        );

        // `minimal_endpoint` is the shared harness's smallest acceptable create body: the
        // required `ContainerType` plus the one manifest an endpoint must carry. Neither
        // is relevant to tagging.
        let (status, created_endpoint) = self
            .create_endpoint(
                &group,
                &channel,
                &endpoint,
                tags_for(Level::Endpoint, minimal_endpoint("TS")),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateOriginEndpoint {endpoint} expected 200, got {status} {created_endpoint}"
        );

        vec![
            (Level::Group, arn_of(&created_group)),
            (Level::Channel, arn_of(&created_channel)),
            (Level::Endpoint, arn_of(&created_endpoint)),
        ]
    }

    /// The tag map `ListTagsForResource` reports for `arn`, after asserting that the call
    /// SUCCEEDED and answered a tag map.
    ///
    /// An error body carries no `tags` member, so reading the body without checking the
    /// status would read a refusal as "this resource has no tags" — and several rows
    /// below expect exactly an empty map.
    async fn listed_tags(&self, arn: &str, case: &str) -> BTreeMap<String, String> {
        let (status, body) = self.list_tags(arn).await;
        assert_eq!(
            status, 200,
            "case {case}: ListTagsForResource expected 200, got {status} {body}"
        );
        assert!(
            body.get("tags").is_some_and(Value::is_object),
            "case {case}: ListTagsForResource must report a tag map, got {body}"
        );
        tag_map(&body, "tags")
    }

    /// Read one level back through its own `Get*`, and return its reported tag map.
    async fn read_tags(&self, level: Level, suffix: &str) -> BTreeMap<String, String> {
        let (group, channel, endpoint) = names(suffix);
        let (status, body) = match level {
            Level::Group => self.get_group(&group).await,
            Level::Channel => self.get_channel(&group, &channel).await,
            Level::Endpoint => self.get_endpoint(&group, &channel, &endpoint).await,
        };
        assert_eq!(
            status, 200,
            "the {level:?} Get* expected 200, got {status} {body}"
        );
        // The modelled member is a map, present even when empty — so "no tags" is told
        // apart from "no tag map", exactly as `listed_tags` does for the list path.
        assert!(
            body.get(level.get_tags_member())
                .is_some_and(Value::is_object),
            "the {level:?} Get* must report its {} member as a map, got {body}",
            level.get_tags_member()
        );
        tag_map(&body, level.get_tags_member())
    }
}

/// One taggable level of the MediaPackage hierarchy. Every level carries its own tag map
/// and its own ARN, so every level is a row of each table below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Group,
    Channel,
    Endpoint,
}

impl Level {
    const ALL: [Level; 3] = [Level::Group, Level::Channel, Level::Endpoint];

    /// The member a level's `Create*` takes its tag map in, and the member its `Get*`
    /// reports it in. The two differ per level because the model puts
    /// `@jsonName("tags")` on some of those members and not others — source:
    /// `smithy_query(query="operation", operation="CreateChannelGroup,CreateChannel,
    /// CreateOriginEndpoint,GetChannelGroup,GetChannel,GetOriginEndpoint")`.
    fn create_tags_member(self) -> &'static str {
        match self {
            Self::Group | Self::Channel => "tags",
            Self::Endpoint => "Tags",
        }
    }

    fn get_tags_member(self) -> &'static str {
        match self {
            Self::Group => "tags",
            Self::Channel | Self::Endpoint => "Tags",
        }
    }

    /// A short label that keeps resource names unique per level.
    fn tag(self) -> &'static str {
        match self {
            Self::Group => "g",
            Self::Channel => "c",
            Self::Endpoint => "e",
        }
    }
}

/// The group, channel and endpoint names of the hierarchy built for `suffix`. Every name
/// matches the model's `ResourceName` pattern `^[a-zA-Z0-9_-]+$`.
fn names(suffix: &str) -> (String, String, String) {
    (
        format!("group-{suffix}"),
        format!("channel-{suffix}"),
        format!("endpoint-{suffix}"),
    )
}

/// The `Arn` a create reported.
fn arn_of(created: &Value) -> String {
    created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a create must report an Arn, got {created}"))
        .to_string()
}

/// A response's tag map as an ordered map, so two maps compare regardless of the order
/// they were serialized in.
///
/// Panics when the member is absent or is not an object: the modelled member is a map,
/// present even when empty, so reading an absent member as "no tags" would hide exactly
/// the difference several rows below exist to pin.
fn tag_map(body: &Value, member: &str) -> BTreeMap<String, String> {
    let Some(Value::Object(map)) = body.get(member) else {
        panic!("expected a {member} map in the response, got {body}");
    };
    map.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("tag {key} must be a string, got {value}"))
                    .to_string(),
            )
        })
        .collect()
}

/// `pairs` as a tag map.
fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One row of the reporting table: a tag map a resource is created with.
struct TagMapCase {
    label: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The rows differ in tag COUNT, in the characters their keys and values use and in
/// whether a value is empty. AWS tag keys and values accept Unicode letters and digits,
/// whitespace and `_ . : / = + - @` (AWS "Tagging your AWS resources" guide); the model
/// puts no `@length` or `@pattern` on `TagKey`/`TagValue`
/// (`smithy_query(query="shape", shape="TagKey,TagValue")`), so none of these is refused.
const TAG_MAP_CASES: &[TagMapCase] = &[
    TagMapCase {
        label: "no tags at all",
        tags: &[],
    },
    TagMapCase {
        label: "a single tag",
        tags: &[("Key1", "Value1")],
    },
    TagMapCase {
        label: "several tags",
        tags: &[("a", "1"), ("b", "2"), ("c", "3")],
    },
    TagMapCase {
        label: "punctuation and spaces AWS permits in keys and values",
        tags: &[
            ("cost center", "video / prod"),
            ("owner@example.com", "a+b=c"),
            ("aws:like:colons", "path/to:thing-1.0_x"),
        ],
    },
    TagMapCase {
        label: "non-ASCII key and value",
        tags: &[("équipe", "vidéo"), ("チーム", "映像")],
    },
    TagMapCase {
        label: "an empty value",
        tags: &[("present-but-empty", "")],
    },
];

/// RULE 1 — a tag map belongs to the resource its ARN names: `ListTagsForResource`
/// reports that resource's map and nothing else, at every taggable level, and agrees
/// with the level's own `Get*`.
#[tokio::test]
async fn list_tags_reports_the_tag_map_of_the_resource_its_arn_names() {
    let emulator = Emulator::new();

    for (index, case) in TAG_MAP_CASES.iter().enumerate() {
        for tagged in Level::ALL {
            let label = case.label;
            let expected = map_of(case.tags);
            let suffix = format!("{index}{}", tagged.tag());
            let arns = emulator.create_hierarchy(&suffix, tagged, &expected).await;

            for (level, arn) in &arns {
                // Only the level the tags were created on carries them: a tag map is
                // neither inherited from a parent nor visible through a child.
                let want = if *level == tagged {
                    expected.clone()
                } else {
                    BTreeMap::new()
                };

                let (status, body) = emulator.list_tags(arn).await;
                assert_eq!(
                    status, 200,
                    "case {label:?} tagged at {tagged:?}: ListTagsForResource on the \
                     {level:?} ARN expected 200, got {status} {body}"
                );
                assert!(
                    body.get("tags").is_some_and(Value::is_object),
                    "case {label:?} tagged at {tagged:?}: ListTagsForResource on the \
                     {level:?} ARN must report a tag map, got {body}"
                );
                let reported = tag_map(&body, "tags");
                assert_eq!(
                    reported, want,
                    "case {label:?} tagged at {tagged:?}: ListTagsForResource on the \
                     {level:?} ARN expected {want:?}, got {reported:?}"
                );

                let read = emulator.read_tags(*level, &suffix).await;
                assert_eq!(
                    read, reported,
                    "case {label:?} tagged at {tagged:?}: the {level:?} Get* reports \
                     {read:?} but ListTagsForResource reports {reported:?}"
                );
            }
        }
    }
}

/// One row of the mutation table: what the resource is created with, what is tagged onto
/// it, what is untagged from it, and the map each step must leave behind.
struct MutationCase {
    label: &'static str,
    initial: &'static [(&'static str, &'static str)],
    tag: &'static [(&'static str, &'static str)],
    after_tag: &'static [(&'static str, &'static str)],
    untag: &'static [&'static str],
    after_untag: &'static [(&'static str, &'static str)],
}

/// The rows differ in what the merge has to do (add, replace, both, nothing) and in what
/// the removal has to do (remove a present key, ignore an absent one, both, empty the
/// map).
const MUTATION_CASES: &[MutationCase] = &[
    MutationCase {
        label: "tagging an untagged resource adds the key",
        initial: &[],
        tag: &[("team", "video")],
        after_tag: &[("team", "video")],
        untag: &["team"],
        after_untag: &[],
    },
    MutationCase {
        label: "a new key is appended and the existing ones are untouched",
        initial: &[("a", "1"), ("b", "2")],
        tag: &[("c", "3")],
        after_tag: &[("a", "1"), ("b", "2"), ("c", "3")],
        untag: &["b"],
        after_untag: &[("a", "1"), ("c", "3")],
    },
    MutationCase {
        label: "a key that is already there takes the submitted value",
        initial: &[("stage", "dev"), ("keep", "me")],
        tag: &[("stage", "prod")],
        after_tag: &[("stage", "prod"), ("keep", "me")],
        untag: &["stage"],
        after_untag: &[("keep", "me")],
    },
    MutationCase {
        label: "one request both adds and replaces",
        initial: &[("stage", "dev"), ("keep", "me")],
        tag: &[("stage", ""), ("owner", "video / prod"), ("cost-center", "42")],
        after_tag: &[
            ("stage", ""),
            ("keep", "me"),
            ("owner", "video / prod"),
            ("cost-center", "42"),
        ],
        untag: &["cost-center", "absent"],
        after_untag: &[("stage", ""), ("keep", "me"), ("owner", "video / prod")],
    },
    MutationCase {
        label: "an empty tag map changes nothing, and an absent key removes nothing",
        initial: &[("only", "one")],
        tag: &[],
        after_tag: &[("only", "one")],
        untag: &["not-a-key-here"],
        after_untag: &[("only", "one")],
    },
    MutationCase {
        label: "removing every key leaves an empty map, not an absent one",
        initial: &[("a", "1"), ("b", "2")],
        tag: &[("c", "3")],
        after_tag: &[("a", "1"), ("b", "2"), ("c", "3")],
        untag: &["a", "b", "c"],
        after_untag: &[],
    },
];

/// RULES 2 and 3 — `TagResource` merges, `UntagResource` removes exactly the named keys
/// and is idempotent, and neither touches another resource's tags.
#[tokio::test]
async fn tagging_merges_and_untagging_removes_only_the_named_keys() {
    let emulator = Emulator::new();

    for (index, case) in MUTATION_CASES.iter().enumerate() {
        for level in Level::ALL {
            let label = case.label;
            let suffix = format!("m{index}{}", level.tag());
            let arns = emulator
                .create_hierarchy(&suffix, level, &map_of(case.initial))
                .await;
            let arn = arns
                .iter()
                .find(|(at, _)| *at == level)
                .map(|(_, arn)| arn.clone())
                .expect("every level has an ARN");

            let (status, body) = emulator.tag(&arn, &map_of(case.tag)).await;
            assert_eq!(
                status, 204,
                "case {label:?} at {level:?}: TagResource expected 204, got {status} {body}"
            );
            let want = map_of(case.after_tag);
            let reported = emulator
                .listed_tags(&arn, &format!("{label:?} at {level:?}, after TagResource"))
                .await;
            assert_eq!(
                reported, want,
                "case {label:?} at {level:?}: after TagResource expected {want:?}, got \
                 {reported:?}"
            );
            let read = emulator.read_tags(level, &suffix).await;
            assert_eq!(
                read, want,
                "case {label:?} at {level:?}: the Get* read after TagResource expected \
                 {want:?}, got {read:?}"
            );

            let (status, body) = emulator.untag(&arn, case.untag).await;
            assert_eq!(
                status, 204,
                "case {label:?} at {level:?}: UntagResource expected 204, got {status} {body}"
            );
            let want = map_of(case.after_untag);
            // `listed_tags` asserts the call succeeded AND reported a tag map: removing
            // every key leaves an EMPTY map, which is a different answer from reporting
            // no tag map at all (or from a refusal, whose body carries none).
            let reported = emulator
                .listed_tags(&arn, &format!("{label:?} at {level:?}, after UntagResource"))
                .await;
            assert_eq!(
                reported, want,
                "case {label:?} at {level:?}: after UntagResource expected {want:?}, got \
                 {reported:?}"
            );

            // Idempotent: the same removal again succeeds and leaves the same map, since
            // a key the resource does not carry is removed silently.
            let (status, body) = emulator.untag(&arn, case.untag).await;
            assert_eq!(
                status, 204,
                "case {label:?} at {level:?}: a repeat UntagResource expected 204, got \
                 {status} {body}"
            );
            let reported = emulator
                .listed_tags(
                    &arn,
                    &format!("{label:?} at {level:?}, after a repeat UntagResource"),
                )
                .await;
            assert_eq!(
                reported, want,
                "case {label:?} at {level:?}: a repeat UntagResource expected {want:?}, \
                 got {reported:?}"
            );

            // Tagging one resource leaves every other resource's map alone.
            for (other, other_arn) in &arns {
                if *other == level {
                    continue;
                }
                let reported = emulator
                    .listed_tags(
                        other_arn,
                        &format!("{label:?} at {level:?}, the untouched {other:?}"),
                    )
                    .await;
                assert!(
                    reported.is_empty(),
                    "case {label:?} at {level:?}: the {other:?} resource was never \
                     tagged but reports {reported:?}"
                );
            }
        }
    }
}

/// RULE 4 — the two ARN failure classes, driven over all three tagging operations: a
/// string that is not an ARN for one of this service's resources is refused
/// `BadRequestException` (400), a well-formed ARN naming a resource this emulator does
/// not hold is refused `NotFoundException` (404), and neither changes the tags of a
/// resource that does exist.
///
/// The CLASS (a 400 for a malformed ARN, a 404 for an absent resource) is the rule; the
/// two codes asserted below are the ones MediaPackage's tagging front end answers and the
/// ones all three handlers and the sibling suite already use, while this service's Smithy
/// model declares `ValidationException` and `ResourceNotFoundException` and lists only the
/// first on these operations. Reconciling the two is a HANDLER change, outside this
/// suite's reach; these rows pin the status and the code the emulator answers today so
/// that reconciliation has something to move, and must be updated with it.
///
/// One row per class, not the whole class table: `rejection_rule_and_negative_paths` owns
/// the full tables (malformed partition, unknown resource type, empty segment, half-way
/// path; absent group, channel and endpoint) — but it drives them for
/// `ListTagsForResource` and `TagResource` only (its `UntagResource` coverage is the
/// success round trip at every level), and it does not assert that a REFUSED call leaves
/// an existing resource's tags alone. Those two gaps are what this test adds.
#[tokio::test]
async fn the_tagging_operations_refuse_an_arn_by_its_two_failure_classes() {
    let emulator = Emulator::new();

    // A live hierarchy, so the refusals below are about the ARN and not about an empty
    // store — and so the "deleted" row has something to delete.
    let live = emulator
        .create_hierarchy("live", Level::Group, &map_of(&[("keep", "me")]))
        .await;
    let live_group_arn = live[0].1.clone();

    let doomed = emulator
        .create_hierarchy("doomed", Level::Endpoint, &map_of(&[("gone", "soon")]))
        .await;
    let (group, channel, endpoint) = names("doomed");
    let deleted_arn = doomed[2].1.clone();
    let (status, body) = emulator
        .call(
            "DELETE",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "DeleteOriginEndpoint expected its modelled 200 (smithy_query: http.code), got \
         {status} {body}"
    );

    // Each row is built from an ARN this emulator actually minted, so nothing here
    // hard-codes a partition, Region or account.
    assert!(
        live_group_arn.ends_with(&format!(":channelGroup/{}", names("live").0)),
        "the live group's ARN must name it: {live_group_arn}"
    );
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "not an ARN at all",
            "tags-please".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a resource that was deleted",
            deleted_arn,
            404,
            "NotFoundException",
        ),
    ];

    for (label, arn, want_status, want_code) in &cases {
        let attempts: Vec<(&str, (u16, Value))> = vec![
            ("ListTagsForResource", emulator.list_tags(arn).await),
            (
                "TagResource",
                emulator.tag(arn, &map_of(&[("k", "v")])).await,
            ),
            ("UntagResource", emulator.untag(arn, &["k"]).await),
        ];
        for (operation, (status, body)) in attempts {
            assert_eq!(
                status, *want_status,
                "case {label:?}: {operation} expected {want_status}, got {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                *want_code,
                "case {label:?}: {operation} expected {want_code}, got {body}"
            );
        }
    }

    // The refused calls stored nothing: the live resource still carries exactly the tags
    // it was created with.
    let reported = emulator
        .listed_tags(&live_group_arn, "the live resource after the refusals")
        .await;
    assert_eq!(
        reported,
        map_of(&[("keep", "me")]),
        "a refused tagging call must not change a resource that does exist, got {reported:?}"
    );
}

/// RULE 5 — `ListTagsForResource` is a read: calling it repeatedly reports the same map
/// and leaves the resource as the writer left it.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let expected = map_of(&[("a", "1"), ("b", "2")]);
    let arns = emulator
        .create_hierarchy("read", Level::Channel, &expected)
        .await;

    for (level, arn) in &arns {
        let want = if *level == Level::Channel {
            expected.clone()
        } else {
            BTreeMap::new()
        };
        for attempt in 1..=3 {
            let reported = emulator
                .listed_tags(arn, &format!("read {attempt} of the {level:?} tags"))
                .await;
            assert_eq!(
                reported, want,
                "read {attempt} of the {level:?} tags expected {want:?}, got {reported:?}"
            );
        }
        let read = emulator.read_tags(*level, "read").await;
        assert_eq!(
            read, want,
            "after three reads the {level:?} Get* expected {want:?}, got {read:?}"
        );
    }
}

/// `count` distinct tags, for exercising the per-resource count bound.
fn numbered_tags(count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// A one-entry tag map, for exercising the key and value length bounds.
fn one_tag(key: String, value: String) -> BTreeMap<String, String> {
    BTreeMap::from([(key, value)])
}

/// RULE 6 — the two bound properties no other suite pins: the ceilings are AWS's
/// PUBLISHED limits, and a character limit counts CHARACTERS rather than bytes, tested at
/// the bound and one past it.
///
/// `rejection_rule_and_negative_paths::the_tag_store_is_bounded` owns the ASCII rows (50
/// tags accepted; 51 tags, a 129-character key and a 257-character value refused), so
/// they are not repeated here.
#[tokio::test]
async fn a_resources_tag_map_is_bounded() {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
        MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
    };

    let emulator = Emulator::new();
    // The bounds every tag-bound row in this crate exercises are the service's PUBLISHED
    // tag limits — 50 tags per resource, a 128-character key, a 256-character value (AWS
    // "Tagging your AWS resources" guide, tag naming limits) — asserted here once so a
    // wrong constant cannot make an at-bound row pass vacuously.
    assert_eq!(
        (
            MAX_TAGS_PER_RESOURCE,
            MAX_TAG_KEY_CHARS,
            MAX_TAG_VALUE_CHARS
        ),
        (50, 128, 256),
        "the crate's tag ceilings must be AWS's published limits, got \
         ({MAX_TAGS_PER_RESOURCE}, {MAX_TAG_KEY_CHARS}, {MAX_TAG_VALUE_CHARS})"
    );
    // (label, the map one TagResource submits, whether it is accepted). Each row is a
    // non-ASCII key or value, where the character count and the byte count differ.
    let cases: Vec<(String, BTreeMap<String, String>, bool)> = vec![
        (
            format!(
                "a non-ASCII key of {MAX_TAG_KEY_CHARS} characters: exactly the key bound \
                 in characters"
            ),
            one_tag("é".repeat(MAX_TAG_KEY_CHARS), "v".to_string()),
            true,
        ),
        (
            format!(
                "a non-ASCII key of {} characters: one past the key bound",
                MAX_TAG_KEY_CHARS + 1
            ),
            one_tag("é".repeat(MAX_TAG_KEY_CHARS + 1), "v".to_string()),
            false,
        ),
        (
            format!(
                "a non-ASCII value of {MAX_TAG_VALUE_CHARS} characters: exactly the value \
                 bound in characters"
            ),
            one_tag("k".to_string(), "é".repeat(MAX_TAG_VALUE_CHARS)),
            true,
        ),
        (
            format!(
                "a non-ASCII value of {} characters: one past the value bound",
                MAX_TAG_VALUE_CHARS + 1
            ),
            one_tag("k".to_string(), "é".repeat(MAX_TAG_VALUE_CHARS + 1)),
            false,
        ),
    ];

    for (index, (label, tags, accepted)) in cases.into_iter().enumerate() {
        // A fresh, untagged resource per row, so each row sees the same start state.
        let suffix = format!("bound{index}");
        let arns = emulator
            .create_hierarchy(&suffix, Level::Group, &BTreeMap::new())
            .await;
        let arn = &arns[0].1;

        let (status, body) = emulator.tag(arn, &tags).await;
        let want_status = if accepted { 204 } else { 400 };
        assert_eq!(
            status, want_status,
            "case {label:?}: TagResource expected {want_status}, got {status} {body}"
        );
        if !accepted {
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
        }

        // A refused request stores NOTHING: the resource is left as it was.
        let want = if accepted { tags } else { BTreeMap::new() };
        let reported = emulator.listed_tags(arn, &label).await;
        assert_eq!(
            reported, want,
            "case {label:?}: after TagResource expected {} tags, got {}",
            want.len(),
            reported.len()
        );
    }

    // At the ceiling the bound is on the RESULTING map: replacing the value of a key the
    // resource already carries is still accepted, while adding one more key is refused
    // and leaves the map alone.
    let full = numbered_tags(MAX_TAGS_PER_RESOURCE);
    let arns = emulator
        .create_hierarchy("ceiling", Level::Endpoint, &BTreeMap::new())
        .await;
    let arn = &arns[2].1;
    let (status, body) = emulator.tag(arn, &full).await;
    assert_eq!(
        status, 204,
        "case \"fill to the ceiling\": expected 204, got {status} {body}"
    );

    let mut replaced = full.clone();
    replaced.insert("key-0".to_string(), "a new value".to_string());
    let (status, body) = emulator
        .tag(arn, &one_tag("key-0".to_string(), "a new value".to_string()))
        .await;
    assert_eq!(
        status, 204,
        "case \"replace a value at the ceiling\": expected 204, got {status} {body}"
    );
    let reported = emulator
        .listed_tags(arn, "replace a value at the ceiling")
        .await;
    assert_eq!(
        reported, replaced,
        "case \"replace a value at the ceiling\": the replacement must be stored without \
         growing the map, got {} tags",
        reported.len()
    );

    let (status, body) = emulator
        .tag(arn, &one_tag("one-too-many".to_string(), "v".to_string()))
        .await;
    assert_eq!(
        status, 400,
        "case \"add a key at the ceiling\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ValidationException",
        "case \"add a key at the ceiling\": expected ValidationException, got {body}"
    );
    let reported = emulator.listed_tags(arn, "add a key at the ceiling").await;
    assert_eq!(
        reported, replaced,
        "case \"add a key at the ceiling\": the refused request must change nothing, got \
         {} tags",
        reported.len()
    );
}
