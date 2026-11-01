// SPDX-License-Identifier: CC0-1.0

//! The tag-key character bound, over every operation of this service that takes tag keys
//! from a request.
//!
//! RULE: a tag key carries at most `state::MAX_TAG_KEY_CHARS` CHARACTERS — AWS's published
//! tag-key limit, which this service's model expresses on no tag member — and a request
//! naming a longer key is refused with that operation's modelled `ValidationException`
//! (HTTP 400) and changes nothing, while a key at or under the bound is an ordinary caller
//! key. How long a key is is a property of the request alone and does not depend on what
//! the request asks to be done with the key, so the path that REMOVES keys judges the bound
//! exactly as the paths that write them do, and every path states one sentence for it.
//!
//! The rows span the class rather than one recorded key:
//!
//! * the LENGTH table ([`LENGTH_CASES`]) varies how long the key is and how it is encoded —
//!   a short key, one character under the bound, exactly the bound, one character past it,
//!   far past it, and a multi-byte key at and one past the bound, which is where counting
//!   BYTES instead of characters would disagree — each built from the bound the handlers
//!   judge against, so no row can pass because the table disagrees with the rule;
//! * the SITE table ([`Site::ALL`]) varies where the key arrives: the two tagging doors
//!   that take the same key through an ARN (`TagResource` writing it, `UntagResource`
//!   removing it) and all four tag-on-create paths.
//!
//! Every refused row also asserts that nothing moved: the resource still carries exactly
//! the tags it had, and a refused create leaves no resource behind.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_TAG_KEY_CHARS;

// The crate's shared test harness: the client with the error-envelope reader and the
// smallest acceptable `CreateOriginEndpoint` body (`channel_lifecycle`), the tagging calls
// and tag-map builder (`resource_tags`), and the one `CreateHarvestJob` body with its
// schedule clock (`rejection_rule_and_negative_paths`). This suite adds only what is
// specific to the key bound.
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use crate::resource_tags::{map_of, tag_map};

/// The Region the shared client signs every request for, and so the Region in the ARNs
/// this service mints. The hierarchy builder below checks the group's reported ARN against
/// [`group_arn`], so neither this nor the account id can drift from what the service builds.
const REGION: &str = "us-west-2";

/// The tag a resource under test is created with, so every row can prove that a refused
/// request left the tags that were already there alone.
const SEEDED: (&str, &str) = ("seeded", "1");

/// An ordinary caller key sent BESIDE the key under test, so a refusal is shown to move
/// neither half of the request rather than just the offending key.
const ORDINARY: (&str, &str) = ("ordinary", "1");

/// The manifest [`minimal_endpoint`] publishes, and so the one a harvest job may harvest.
const PUBLISHED_MANIFEST: &str = "index";

/// One row of the length table: a tag key and whether it is past the bound.
struct LengthCase {
    label: &'static str,
    /// The character the key repeats, so a row states its own encoding.
    unit: &'static str,
    /// How many of those characters the key carries.
    characters: usize,
    refused: bool,
}

impl LengthCase {
    fn key(&self) -> String {
        self.unit.repeat(self.characters)
    }
}

/// The lengths the rule has to separate, every one of them built from
/// `state::MAX_TAG_KEY_CHARS`: the accepted rows walk up to the bound itself, the refused
/// rows start one character past it, and the two multi-byte rows sit either side of the
/// bound in CHARACTERS while being two bytes per character — a byte-counting bound would
/// refuse the accepted one.
const LENGTH_CASES: &[LengthCase] = &[
    LengthCase {
        label: "a one-character key",
        unit: "k",
        characters: 1,
        refused: false,
    },
    LengthCase {
        label: "a key one character short of the bound",
        unit: "k",
        characters: MAX_TAG_KEY_CHARS - 1,
        refused: false,
    },
    LengthCase {
        label: "a key of exactly the bound",
        unit: "k",
        characters: MAX_TAG_KEY_CHARS,
        refused: false,
    },
    LengthCase {
        label: "a multi-byte key of exactly the bound in characters",
        unit: "é",
        characters: MAX_TAG_KEY_CHARS,
        refused: false,
    },
    LengthCase {
        label: "a key one character past the bound",
        unit: "k",
        characters: MAX_TAG_KEY_CHARS + 1,
        refused: true,
    },
    LengthCase {
        label: "a key far past the bound",
        unit: "k",
        characters: MAX_TAG_KEY_CHARS * 3,
        refused: true,
    },
    LengthCase {
        label: "a multi-byte key one character past the bound",
        unit: "é",
        characters: MAX_TAG_KEY_CHARS + 1,
        refused: true,
    },
];

/// The `Message` member of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// The ARN of a channel group, in the form this service mints.
fn group_arn(group: &str) -> String {
    format!(
        "arn:aws:mediapackagev2:{REGION}:{}:channelGroup/{group}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The part of a refusal message that follows the member path: the requirement itself,
/// which is what every path that judges this bound has to state identically. Asserts the
/// envelope and the member path on the way, so a message in another form fails here rather
/// than silently comparing as equal.
fn requirement(case: &str, member: &str, body: &Value) -> String {
    let prefix = format!("Validation error encountered at request.{member}: ");
    message(body)
        .strip_prefix(&prefix)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must name the member the key arrived in \
                 ({member:?}), got {:?}",
                message(body)
            )
        })
        .to_string()
}

/// Assert `body` is this service's over-long-key refusal: the modelled client error, with a
/// sentence stating the bound and how many characters the key the request sent carries.
fn assert_key_bound_refusal(
    case: &str,
    member: &str,
    characters: usize,
    status: u16,
    body: &Value,
) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: a tag key past the bound must be refused with HTTP 400, got \
         {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    let stated = requirement(case, member, body);
    assert!(
        stated.contains(&MAX_TAG_KEY_CHARS.to_string()),
        "case {case:?}: the refusal must state the bound ({MAX_TAG_KEY_CHARS}), got \
         {stated:?}"
    );
    assert!(
        stated.contains(&characters.to_string()),
        "case {case:?}: the refusal must state what the request sent ({characters} \
         characters), got {stated:?}"
    );
    stated
}

/// A channel group carrying [`SEEDED`] and nothing else, plus its ARN.
async fn seeded_group(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .create_group(name, json!({ "tags": map_of(&[SEEDED]) }))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup {name} expected 200, got {status} {body}"
    );
    assert_eq!(
        body.get("Arn").and_then(Value::as_str),
        Some(group_arn(name).as_str()),
        "the ARN builder must match the ARN the service mints, got {body}"
    );
    group_arn(name)
}

/// The tag map `ListTagsForResource` reports for `arn`, after checking the read itself
/// succeeded: an error body carries no tag member, so reading one without checking the
/// status would report a refusal as "this resource has no tags".
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator.list_tags(arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {body}"
    );
    tag_map(&body, "tags")
}

/// RULE, on the two tagging doors: the same key is judged the same way whether the request
/// WRITES it or REMOVES it — refused past the bound, accepted at or under it, with the
/// write and the removal both visible through the resource's own tag read.
#[tokio::test]
async fn both_tagging_doors_judge_a_tag_key_by_its_character_count() {
    let emulator = Emulator::new();

    for (index, case) in LENGTH_CASES.iter().enumerate() {
        let label = case.label;
        let key = case.key();
        // A fresh resource per row, so every row starts from the same tag map.
        let arn = seeded_group(&emulator, &format!("kb-door-{index}")).await;

        // `TagResource`, which would CREATE the key, beside an ordinary one.
        let mut submitted = map_of(&[ORDINARY]);
        submitted.insert(key.clone(), "v".to_string());
        let (status, body) = emulator.tag(&arn, &submitted).await;
        let after_write = if case.refused {
            assert_key_bound_refusal(label, "tags", case.characters, status, &body);
            map_of(&[SEEDED])
        } else {
            assert_eq!(
                status, 204,
                "case {label:?}: a key at or under the bound must be accepted, got \
                 {status} {body}"
            );
            let mut expected = map_of(&[SEEDED, ORDINARY]);
            expected.insert(key.clone(), "v".to_string());
            expected
        };
        let reported = listed_tags(&emulator, &arn, label).await;
        assert_eq!(
            reported, after_write,
            "case {label:?}: after TagResource expected {after_write:?}, got {reported:?}"
        );

        // `UntagResource`, which would REMOVE it, named beside a key the resource
        // definitely holds: a refusal must remove neither.
        let (status, body) = emulator.untag(&arn, &[SEEDED.0, key.as_str()]).await;
        let after_removal = if case.refused {
            let removal_requirement =
                assert_key_bound_refusal(label, "tagKeys", case.characters, status, &body);
            // The two doors share the rule, so they cannot word it two ways.
            let (_, written) = emulator.tag(&arn, &submitted).await;
            assert_eq!(
                removal_requirement,
                requirement(label, "tags", &written),
                "case {label:?}: UntagResource stated {removal_requirement:?} and \
                 TagResource {:?} for the same over-long key",
                requirement(label, "tags", &written)
            );
            after_write
        } else {
            assert_eq!(
                status, 204,
                "case {label:?}: removing a key at or under the bound must be accepted, \
                 got {status} {body}"
            );
            // Both named keys are gone; the one sent alongside stays.
            map_of(&[ORDINARY])
        };
        let reported = listed_tags(&emulator, &arn, label).await;
        assert_eq!(
            reported, after_removal,
            "case {label:?}: after UntagResource expected {after_removal:?}, got \
             {reported:?}"
        );
    }
}

/// Where a key can arrive, beyond the two tagging doors.
#[derive(Debug, Clone, Copy)]
enum Site {
    ChannelGroup,
    Channel,
    OriginEndpoint,
    HarvestJob,
    TagResource,
    UntagResource,
}

impl Site {
    const ALL: [Site; 6] = [
        Site::ChannelGroup,
        Site::Channel,
        Site::OriginEndpoint,
        Site::HarvestJob,
        Site::TagResource,
        Site::UntagResource,
    ];

    /// The wire name of the member the keys arrive in: `@jsonName("tags")` on
    /// `CreateChannelGroupRequest$Tags`, `CreateChannelRequest$Tags` and
    /// `TagResourceRequest$Tags`, the member's own name on
    /// `CreateOriginEndpointRequest$Tags` and `CreateHarvestJobRequest$Tags`, and
    /// `@httpQuery("tagKeys")` on `UntagResourceRequest$TagKeys` (source:
    /// `smithy_query(query="operation", operation="CreateChannelGroup,CreateChannel,
    /// CreateOriginEndpoint,CreateHarvestJob,TagResource,UntagResource")`).
    fn member(self) -> &'static str {
        match self {
            Self::ChannelGroup | Self::Channel | Self::TagResource => "tags",
            Self::OriginEndpoint | Self::HarvestJob => "Tags",
            Self::UntagResource => "tagKeys",
        }
    }

    /// A short label keeping the resource names of the rows apart.
    fn slug(self) -> &'static str {
        match self {
            Self::ChannelGroup => "grp",
            Self::Channel => "chn",
            Self::OriginEndpoint => "oep",
            Self::HarvestJob => "hjb",
            Self::TagResource => "tag",
            Self::UntagResource => "untag",
        }
    }

    /// Whether this site mints a resource, and so whether a refused row has a read-back to
    /// prove it stored nothing.
    fn mints_a_resource(self) -> bool {
        !matches!(self, Self::TagResource | Self::UntagResource)
    }
}

/// RULE, everywhere a tag key arrives: one over-long key earns the same refusal at every
/// path that takes tag keys — the same status, the same error class and the same
/// requirement sentence, differing only in the member the keys were sent in.
#[tokio::test]
async fn every_path_that_takes_a_tag_key_judges_the_same_bound() {
    let emulator = Emulator::new();

    // One live hierarchy the child creates are addressed under, carrying tags of its own
    // so a refused request can be shown not to disturb them.
    let host = "kb-host";
    let host_arn = seeded_group(&emulator, host).await;
    let host_channel = "kb-host-channel";
    let host_endpoint = "kb-host-endpoint";
    let (status, body) = emulator.create_channel(host, host_channel, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannel {host_channel} expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(host, host_channel, host_endpoint, minimal_endpoint("TS"))
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint {host_endpoint} expected 200, got {status} {body}"
    );
    let endpoint_path =
        format!("/channelGroup/{host}/channel/{host_channel}/originEndpoint/{host_endpoint}");

    // One key, one character past the bound, sent beside an ordinary one at every site.
    let characters = MAX_TAG_KEY_CHARS + 1;
    let key = "k".repeat(characters);
    let mut tags = map_of(&[ORDINARY]);
    tags.insert(key.clone(), "v".to_string());

    let mut stated: Vec<(String, String)> = Vec::new();
    for site in Site::ALL {
        let label = format!("an over-long key at {site:?}");
        let child = format!("kb-{}", site.slug());
        let (status, body) = match site {
            Site::ChannelGroup => {
                emulator
                    .create_group(&child, json!({ "tags": tags }))
                    .await
            }
            Site::Channel => {
                emulator
                    .create_channel(host, &child, json!({ "tags": tags }))
                    .await
            }
            Site::OriginEndpoint => {
                let mut create = minimal_endpoint("TS");
                create["Tags"] = json!(tags);
                emulator
                    .create_endpoint(host, host_channel, &child, create)
                    .await
            }
            Site::HarvestJob => {
                let start = epoch_seconds_from_now(60.0);
                let mut create = harvest_body(start, start + 600.0, &[PUBLISHED_MANIFEST]);
                create["HarvestJobName"] = json!(child);
                create["Tags"] = json!(tags);
                emulator
                    .call("POST", &format!("{endpoint_path}/harvestJob"), Some(create))
                    .await
            }
            Site::TagResource => emulator.tag(&host_arn, &tags).await,
            Site::UntagResource => {
                let keys: Vec<&str> = vec![ORDINARY.0, key.as_str()];
                emulator.untag(&host_arn, &keys).await
            }
        };

        stated.push((
            label.clone(),
            assert_key_bound_refusal(&label, site.member(), characters, status, &body),
        ));

        if site.mints_a_resource() {
            let (read_status, read) = match site {
                Site::ChannelGroup => emulator.get_group(&child).await,
                Site::Channel => emulator.get_channel(host, &child).await,
                Site::OriginEndpoint => emulator.get_endpoint(host, host_channel, &child).await,
                Site::HarvestJob => {
                    emulator
                        .call(
                            "GET",
                            &format!("{endpoint_path}/harvestJob/{child}"),
                            None,
                        )
                        .await
                }
                _ => unreachable!("only a create mints a resource"),
            };
            assert_eq!(
                read_status, 404,
                "case {label:?}: a refused create must leave no resource to read, got \
                 {read_status} {read}"
            );
        }
    }

    // One requirement sentence across the whole surface: the member path differs, the rule
    // does not.
    let (first_label, first) = stated
        .first()
        .cloned()
        .expect("the site table is not empty");
    for (label, requirement) in &stated {
        assert_eq!(
            requirement, &first,
            "case {label:?}: stated {requirement:?}, while {first_label:?} stated {first:?}"
        );
    }

    // Nothing the refused requests addressed moved: the host group still carries exactly
    // the tag it was created with, neither written to nor removed from.
    let reported = listed_tags(&emulator, &host_arn, "the host group after the refusals").await;
    assert_eq!(
        reported,
        map_of(&[SEEDED]),
        "a refused request must leave the addressed resource's tags alone, got {reported:?}"
    );
}
