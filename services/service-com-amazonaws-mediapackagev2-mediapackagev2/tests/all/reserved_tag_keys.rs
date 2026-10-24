// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over every operation of this service that takes tag
//! keys from a request.
//!
//! RULE: AWS owns the `aws:` tag-key namespace — "Do not use the `aws:` prefix in your
//! tag names or values because it is reserved for AWS use. You can't edit or delete tag
//! names or values with this prefix" (AWS "Tagging your AWS resources") — so a request
//! that names a tag key BEGINNING with that prefix, in any ASCII letter case, is refused
//! with the operation's modelled client error (`ValidationException`, HTTP 400) and
//! writes nothing, while a key that merely contains the prefix elsewhere, or shares only
//! part of it, is an ordinary caller tag.
//!
//! The rows below span the whole class rather than the one recorded key:
//!
//! * the KEY table (`KEY_CASES`) varies the spelling — the prefix alone, a system key
//!   AWS itself writes, upper and mixed case, the prefix without its colon, the prefix
//!   not at the start of the key, a key shorter than the prefix and a non-ASCII first
//!   character — and says for each whether it is in the namespace;
//! * the SITE table (`Site::ALL`) varies where the keys arrive: `TagResource`,
//!   `UntagResource` and tag-on-create for a channel group, a channel, an origin
//!   endpoint and a harvest job.
//!
//! Every row also asserts the ATOMICITY the rule implies: a refused request that also
//! carried an allowed key stores neither, and a refused create leaves no resource
//! behind.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    is_reserved_tag_key, RESERVED_TAG_KEY_PREFIX,
};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::encoded as pct;

/// The key a resource under test is created with, so every row can prove that a refused
/// request left the tags that were already there alone.
const KEPT: (&str, &str) = ("keep", "me");

/// An ordinary caller key sent ALONGSIDE the key under test, so a refusal is shown to
/// store neither half of the request rather than just the offending key.
const ALSO_SENT: (&str, &str) = ("also-sent", "1");

/// One row of the key table: a tag key, and whether it is in the reserved namespace.
struct KeyCase {
    label: &'static str,
    key: &'static str,
    reserved: bool,
}

/// The spellings the rule has to separate. The reserved rows differ in case and in what
/// follows the prefix; the permitted rows are the near misses — the prefix without its
/// colon, the prefix away from the start, a key shorter than the prefix, and a key whose
/// first character is multi-byte (so the prefix comparison cannot even be attempted on a
/// character boundary).
const KEY_CASES: &[KeyCase] = &[
    KeyCase {
        label: "the reserved prefix alone",
        key: "aws:",
        reserved: true,
    },
    KeyCase {
        label: "a system key AWS itself writes",
        key: "aws:cloudformation:stack-name",
        reserved: true,
    },
    KeyCase {
        label: "the prefix in upper case",
        key: "AWS:owner",
        reserved: true,
    },
    KeyCase {
        label: "the prefix in mixed case",
        key: "Aws:cost-centre",
        reserved: true,
    },
    KeyCase {
        label: "the prefix followed by a non-ASCII name",
        key: "aws:équipe",
        reserved: true,
    },
    KeyCase {
        label: "the prefix without its colon",
        key: "awsome",
        reserved: false,
    },
    KeyCase {
        label: "the three letters and nothing else",
        key: "aws",
        reserved: false,
    },
    KeyCase {
        label: "the prefix away from the start of the key",
        key: "team:aws:owner",
        reserved: false,
    },
    KeyCase {
        label: "a key shorter than the prefix",
        key: "aw",
        reserved: false,
    },
    KeyCase {
        label: "a non-ASCII first character",
        key: "équipe:aws",
        reserved: false,
    },
];

/// `POST /tags/{ResourceArn}` with `tags` — the wire name of `TagResourceRequest$Tags`.
async fn tag(emulator: &Emulator, arn: &str, tags: &BTreeMap<String, String>) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/tags/{}", pct(arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=..`: `TagKeys` is an `@httpQuery` list, so one
/// `tagKeys=` parameter per key.
async fn untag(emulator: &Emulator, arn: &str, keys: &[&str]) -> (u16, Value) {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .call("DELETE", &format!("/tags/{}?{query}", pct(arn)), None)
        .await
}

/// The tag map `ListTagsForResource` reports for `arn`, after asserting the read itself
/// succeeded: an error body carries no `tags` member, so reading one without checking the
/// status would report a refusal as "this resource has no tags".
async fn listed_tags(emulator: &Emulator, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = emulator.call("GET", &format!("/tags/{}", pct(arn)), None).await;
    assert_eq!(
        status, 200,
        "case {case}: ListTagsForResource expected 200, got {status} {body}"
    );
    let Some(Value::Object(map)) = body.get("tags") else {
        panic!("case {case}: ListTagsForResource must report a tag map, got {body}");
    };
    map.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: tag {key} must be a string"))
                    .to_string(),
            )
        })
        .collect()
}

/// The `Message` member of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// `pairs` as a tag map.
fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Assert `body` is this service's reserved-key refusal: the modelled client error, with
/// a sentence that states the rule the caller broke.
fn assert_reserved_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: a reserved tag key must be refused with HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case}: expected the modelled ValidationException, got {body}"
    );
    assert!(
        message(body).contains(RESERVED_TAG_KEY_PREFIX),
        "case {case}: the refusal must state the reserved prefix \
         {RESERVED_TAG_KEY_PREFIX:?}, got {:?}",
        message(body)
    );
}

/// The ARN a create reported.
fn arn_of(case: &str, created: &Value) -> String {
    created["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a create must report an Arn, got {created}"))
        .to_string()
}

/// A channel group created with [`KEPT`] and nothing else, plus its ARN.
async fn group_with_kept_tag(emulator: &Emulator, name: &str) -> String {
    let (status, body) = emulator
        .create_group(name, json!({ "tags": map_of(&[KEPT]) }))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup {name} expected 200, got {status} {body}"
    );
    arn_of(name, &body)
}

/// The predicate the handlers share agrees with the table above — so a row that passes
/// over HTTP cannot be passing because the table disagrees with the rule.
#[tokio::test]
async fn the_reserved_namespace_predicate_matches_the_key_table() {
    assert_eq!(
        RESERVED_TAG_KEY_PREFIX, "aws:",
        "the reserved prefix is AWS's published one, got {RESERVED_TAG_KEY_PREFIX:?}"
    );
    for case in KEY_CASES {
        let label = case.label;
        assert_eq!(
            is_reserved_tag_key(case.key),
            case.reserved,
            "case {label:?}: is_reserved_tag_key({:?}) expected {}, got {}",
            case.key,
            case.reserved,
            is_reserved_tag_key(case.key)
        );
    }
}

/// RULE, on `TagResource`: a reserved key is refused and stores nothing (not even the
/// allowed key sent beside it); a key outside the namespace is stored and read back.
#[tokio::test]
async fn tag_resource_refuses_a_reserved_key_and_accepts_every_other_one() {
    let emulator = Emulator::new();

    for (index, case) in KEY_CASES.iter().enumerate() {
        let label = case.label;
        // A fresh resource per row, so every row starts from the same tag map.
        let name = format!("tr-{index}");
        let arn = group_with_kept_tag(&emulator, &name).await;

        let mut submitted = map_of(&[ALSO_SENT]);
        submitted.insert(case.key.to_string(), "v".to_string());
        let (status, body) = tag(&emulator, &arn, &submitted).await;

        let expected = if case.reserved {
            assert_reserved_refusal(label, status, &body);
            map_of(&[KEPT])
        } else {
            assert_eq!(
                status, 204,
                "case {label:?}: a key outside the reserved namespace must be accepted, \
                 got {status} {body}"
            );
            let mut expected = map_of(&[KEPT, ALSO_SENT]);
            expected.insert(case.key.to_string(), "v".to_string());
            expected
        };

        let reported = listed_tags(&emulator, &arn, label).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: after TagResource expected {expected:?}, got {reported:?}"
        );
        // The same map through the resource's own read, so the refusal (or the write) is
        // a property of the stored resource and not of the tagging projection.
        let (status, read) = emulator.get_group(&name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannelGroup expected 200, got {status} {read}"
        );
        assert_eq!(
            read.get("tags"),
            Some(&json!(expected)),
            "case {label:?}: GetChannelGroup expected {expected:?}, got {read}"
        );
    }
}

/// RULE, on `UntagResource`: a caller can no more DELETE a tag in the reserved namespace
/// than create one, and a refused removal leaves every key in place — while a key outside
/// the namespace is removed exactly as before.
#[tokio::test]
async fn untag_resource_refuses_a_reserved_key_and_removes_every_other_one() {
    let emulator = Emulator::new();

    for (index, case) in KEY_CASES.iter().enumerate() {
        let label = case.label;
        let name = format!("ur-{index}");
        let arn = group_with_kept_tag(&emulator, &name).await;
        // The key under test is given to the resource when it is one a caller may hold,
        // so the permitted rows have something to remove.
        if !case.reserved {
            let (status, body) = tag(
                &emulator,
                &arn,
                &BTreeMap::from([(case.key.to_string(), "v".to_string())]),
            )
            .await;
            assert_eq!(
                status, 204,
                "case {label:?}: seeding the key expected 204, got {status} {body}"
            );
        }

        // The key under test beside a key the resource definitely holds: a refusal must
        // remove neither.
        let (status, body) = untag(&emulator, &arn, &[KEPT.0, case.key]).await;
        let expected = if case.reserved {
            assert_reserved_refusal(label, status, &body);
            map_of(&[KEPT])
        } else {
            assert_eq!(
                status, 204,
                "case {label:?}: removing a key outside the reserved namespace must be \
                 accepted, got {status} {body}"
            );
            BTreeMap::new()
        };
        let reported = listed_tags(&emulator, &arn, label).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: after UntagResource expected {expected:?}, got {reported:?}"
        );
    }
}

/// Where a create takes its tag map, and how a created resource is read back.
#[derive(Debug, Clone, Copy)]
enum Site {
    ChannelGroup,
    Channel,
    OriginEndpoint,
    HarvestJob,
}

impl Site {
    const ALL: [Site; 4] = [
        Site::ChannelGroup,
        Site::Channel,
        Site::OriginEndpoint,
        Site::HarvestJob,
    ];

    /// The wire name of the create's tag member: `@jsonName("tags")` on
    /// `CreateChannelGroupRequest$Tags` and `CreateChannelRequest$Tags`, and the member's
    /// own name on `CreateOriginEndpointRequest$Tags` and `CreateHarvestJobRequest$Tags`
    /// (source: `smithy_query(query="operation", operation="CreateChannelGroup,
    /// CreateChannel,CreateOriginEndpoint,CreateHarvestJob")`).
    fn tags_member(self) -> &'static str {
        match self {
            Self::ChannelGroup | Self::Channel => "tags",
            Self::OriginEndpoint | Self::HarvestJob => "Tags",
        }
    }

    /// A short label keeping the resource names of the rows apart.
    fn slug(self) -> &'static str {
        match self {
            Self::ChannelGroup => "grp",
            Self::Channel => "chn",
            Self::OriginEndpoint => "oep",
            Self::HarvestJob => "hjb",
        }
    }

    /// The wire name of the tag member the site's own read reports: `@jsonName("tags")`
    /// on `GetChannelGroupResponse$Tags`, the member's own name on the other three
    /// (source: `smithy_query(query="operation", operation="GetChannelGroup,GetChannel,
    /// GetOriginEndpoint,GetHarvestJob")`).
    fn read_tags_member(self) -> &'static str {
        match self {
            Self::ChannelGroup => "tags",
            Self::Channel | Self::OriginEndpoint | Self::HarvestJob => "Tags",
        }
    }
}

/// Epoch seconds, far enough ahead that a harvest window does not end mid-test.
fn epoch_seconds_from_now(offset: f64) -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
        + offset
}

/// RULE, on tag-on-create, at every create that accepts tags: a reserved key is refused
/// and leaves NO resource behind — the parent it was created under keeps the tags it had
/// — while a key outside the namespace is created WITH its tags and reads back with them,
/// so the gate cannot be an over-broad "any key that mentions aws" check.
#[tokio::test]
async fn every_tag_on_create_separates_the_reserved_namespace_from_every_other_key() {
    let emulator = Emulator::new();

    // One live parent hierarchy the channel, endpoint and harvest-job rows are created
    // under, carrying tags of its own so a refused child cannot disturb them.
    let host = "host";
    let host_arn = group_with_kept_tag(&emulator, host).await;
    let (status, body) = emulator
        .create_channel(host, "hostchannel", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel hostchannel expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(host, "hostchannel", "hostendpoint", minimal_endpoint("TS"))
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint hostendpoint expected 200, got {status} {body}"
    );
    let endpoint_path = format!("/channelGroup/{host}/channel/hostchannel/originEndpoint/hostendpoint");

    for (index, case) in KEY_CASES.iter().enumerate() {
        for site in Site::ALL {
            let label = format!("{:?} at {site:?}", case.label);
            // Every create carries the key under test AND an ordinary one, so a refusal is
            // shown to store neither and an accepted create to store both.
            let mut tags = map_of(&[ALSO_SENT]);
            tags.insert(case.key.to_string(), "v".to_string());
            let name = format!("c{index}-{}", site.slug());
            let mut tagged_body = json!({});
            tagged_body[site.tags_member()] = json!(tags);

            let (status, body, read_back) = match site {
                Site::ChannelGroup => {
                    let (status, body) = emulator.create_group(&name, tagged_body).await;
                    (status, body, emulator.get_group(&name).await)
                }
                Site::Channel => {
                    let (status, body) = emulator.create_channel(host, &name, tagged_body).await;
                    (status, body, emulator.get_channel(host, &name).await)
                }
                Site::OriginEndpoint => {
                    let mut create = minimal_endpoint("TS");
                    create[site.tags_member()] = json!(tags);
                    let (status, body) = emulator
                        .create_endpoint(host, "hostchannel", &name, create)
                        .await;
                    (
                        status,
                        body,
                        emulator.get_endpoint(host, "hostchannel", &name).await,
                    )
                }
                Site::HarvestJob => {
                    let start = epoch_seconds_from_now(60.0);
                    let mut create = json!({
                        "HarvestJobName": name,
                        "HarvestedManifests": {"HlsManifests": [{"ManifestName": "index"}]},
                        "ScheduleConfiguration": {"StartTime": start, "EndTime": start + 600.0},
                        "Destination": {
                            "S3Destination": {
                                "BucketName": "harvest-bucket",
                                "DestinationPath": "out/",
                            },
                        },
                    });
                    create[site.tags_member()] = json!(tags);
                    let (status, body) = emulator
                        .call("POST", &format!("{endpoint_path}/harvestJob"), Some(create))
                        .await;
                    (
                        status,
                        body,
                        emulator
                            .call("GET", &format!("{endpoint_path}/harvestJob/{name}"), None)
                            .await,
                    )
                }
            };

            let (read_status, read_body) = read_back;
            if case.reserved {
                assert_reserved_refusal(&label, status, &body);
                assert_eq!(
                    read_status, 404,
                    "case {label:?}: a refused create must leave no resource to read, got \
                     {read_status} {read_body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a key outside the reserved namespace must be accepted \
                     on create, got {status} {body}"
                );
                // Every create RESPONSE reports the map in `Tags` (no `@jsonName` on any
                // of the four response members), while a Get reports it in the member
                // `read_tags_member` names.
                assert_eq!(
                    body.get("Tags"),
                    Some(&json!(tags)),
                    "case {label:?}: the create must report the tags it was given \
                     ({tags:?}), got {body}"
                );
                assert_eq!(
                    read_status, 200,
                    "case {label:?}: the created resource must read back, got \
                     {read_status} {read_body}"
                );
                assert_eq!(
                    read_body.get(site.read_tags_member()),
                    Some(&json!(tags)),
                    "case {label:?}: the read must report the created tags ({tags:?}), got \
                     {read_body}"
                );
            }
        }
    }

    // The parent the creates were addressed under still carries exactly the tags it was
    // created with: neither a refused nor an accepted child touches them.
    let reported = listed_tags(&emulator, &host_arn, "the host group after the creates").await;
    assert_eq!(
        reported,
        map_of(&[KEPT]),
        "a tag-on-create must not change its parent's tags, got {reported:?}"
    );
}
