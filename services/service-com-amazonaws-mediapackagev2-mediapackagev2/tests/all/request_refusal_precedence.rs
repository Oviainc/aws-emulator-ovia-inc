// SPDX-License-Identifier: CC0-1.0

//! Where a refusal the request earns on its own face sits relative to resolving the names
//! the request addresses, over every operation of this service that can earn both at once.
//!
//! RULE: a refusal decided from the REQUEST alone — judged on the members the request
//! carried, with nothing read from the store — is decided BEFORE the store is consulted,
//! so a request that is BOTH malformed and addresses a name that does not resolve is
//! answered with that operation's `ValidationException` (400) rather than with its
//! not-found class (404), and every operation of the service reports one malformed request
//! as one error class.
//!
//! The rows span the class rather than one recorded request:
//!
//! * the FAULT ([`Fault::ALL`]) varies which request-level rule is broken and what kind of
//!   thing is wrong with the member — a tag key in the namespace AWS reserves, a tag map
//!   past the per-resource ceiling, an over-long tag key and an over-long tag value, each
//!   built from the bound the handlers share (`state::MAX_TAGS_PER_RESOURCE`,
//!   `state::MAX_TAG_KEY_CHARS`, `state::MAX_TAG_VALUE_CHARS`,
//!   `state::RESERVED_TAG_KEY_PREFIX`), so a row cannot pass because the table disagrees
//!   with the rule;
//! * the SITE ([`Site::ALL`]) varies the operation the member arrives at: tag-on-create for
//!   a channel, an origin endpoint and a harvest job, plus `TagResource` and
//!   `UntagResource`, which take the same keys through an ARN;
//! * the DEPTH ([`Site::breaks`]) varies how deep the addressed chain breaks — a missing
//!   channel group, a missing channel, a missing origin endpoint — because the deeper
//!   lookups are exactly what the refusal must not wait for.
//!
//! Each row carries both controls, so neither verdict can be a constant: the same faulty
//! request with every name RESOLVING is the same 400, and a CLEAN request at the same
//! broken chain is still the 404 (and, with nothing broken, a success).
//!
//! The second test is the same rule at the other request-level refusal this service makes
//! that an addressing operation can earn — a multiview list that repeats an entry — on the
//! create and the update that both write that member.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, RESERVED_TAG_KEY_PREFIX,
};

// The crate's shared test harness: the control-plane client with the error-envelope reader
// and the smallest acceptable `CreateOriginEndpoint` body (`channel_lifecycle`), the one
// `MultiviewConfiguration` builder (`multiview_configuration_validation`), the
// `CreateHarvestJob` body with its schedule clock (`rejection_rule_and_negative_paths`),
// and the tag-map builder (`resource_tags`, whose impl also gives `Emulator` the two
// tagging calls used below). This suite adds only what is specific to refusal ordering.
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::multiview_configuration_validation::multiview;
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use crate::resource_tags::map_of;

/// The Region the shared client signs every request for, and so the Region in the ARNs
/// this service mints for the resources it creates. [`live_hierarchy`] asserts the live
/// group's own reported ARN against [`group_arn`], so neither this nor the account id can
/// drift from what the service builds.
const REGION: &str = "us-west-2";

/// The hierarchy every row addresses when a name is meant to resolve.
const LIVE_GROUP: &str = "rlp-group";
const LIVE_CHANNEL: &str = "rlp-channel";
const LIVE_ENDPOINT: &str = "rlp-endpoint";

/// The names nothing is ever created under, one per level, all matching the model's
/// `ResourceName` pattern `^[a-zA-Z0-9_-]+$` so the boundary validator admits them and the
/// handler is what answers.
const ABSENT_GROUP: &str = "rlp-absent-group";
const ABSENT_CHANNEL: &str = "rlp-absent-channel";
const ABSENT_ENDPOINT: &str = "rlp-absent-endpoint";

/// An ordinary caller tag, sent BESIDE the fault so a refusal is shown to store neither
/// half, and sent alone by the clean control rows.
const ORDINARY: (&str, &str) = ("ordinary", "1");

/// The manifest the live endpoint publishes, and so the one a harvest job may harvest:
/// [`minimal_endpoint`] publishes exactly `index`.
const PUBLISHED_MANIFEST: &str = "index";

/// The `Message` member of an error envelope.
fn message(body: &Value) -> &str {
    body.get("Message").and_then(Value::as_str).unwrap_or("")
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The ARN of a channel group, in the form this service mints.
fn group_arn(group: &str) -> String {
    format!(
        "arn:aws:mediapackagev2:{REGION}:{}:channelGroup/{group}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One request-level fault: something wrong with the members the request carried, judged
/// without reading anything from the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// A tag key in the namespace AWS reserves for itself.
    ReservedTagKey,
    /// More tags than one resource may carry.
    PastTagCeiling,
    /// A tag key longer than a tag key may be.
    OverLongTagKey,
    /// A tag value longer than a tag value may be.
    OverLongTagValue,
}

impl Fault {
    const ALL: [Fault; 4] = [
        Fault::ReservedTagKey,
        Fault::PastTagCeiling,
        Fault::OverLongTagKey,
        Fault::OverLongTagValue,
    ];

    /// The tag map carrying this fault, always beside [`ORDINARY`]: every bound comes from
    /// the constant the handlers judge against, and every map is one step past it.
    fn tags(self) -> BTreeMap<String, String> {
        let mut tags = map_of(&[ORDINARY]);
        match self {
            Self::ReservedTagKey => {
                tags.insert(format!("{RESERVED_TAG_KEY_PREFIX}owner"), "x".to_string());
            }
            Self::PastTagCeiling => {
                for index in 0..=MAX_TAGS_PER_RESOURCE {
                    tags.insert(format!("bulk-{index}"), "x".to_string());
                }
            }
            Self::OverLongTagKey => {
                tags.insert("k".repeat(MAX_TAG_KEY_CHARS + 1), "x".to_string());
            }
            Self::OverLongTagValue => {
                tags.insert(
                    "over-long-value".to_string(),
                    "v".repeat(MAX_TAG_VALUE_CHARS + 1),
                );
            }
        }
        tags
    }

    /// The text the refusal must state: the rule the caller broke, which for a bound is
    /// the bound's own value.
    fn must_state(self) -> String {
        match self {
            Self::ReservedTagKey => RESERVED_TAG_KEY_PREFIX.to_string(),
            Self::PastTagCeiling => MAX_TAGS_PER_RESOURCE.to_string(),
            Self::OverLongTagKey => MAX_TAG_KEY_CHARS.to_string(),
            Self::OverLongTagValue => MAX_TAG_VALUE_CHARS.to_string(),
        }
    }

    /// Whether `UntagResource` can carry this fault: it names tag KEYS and stores none of
    /// them, so only the rule about a key's namespace reaches it.
    fn reaches_a_remover(self) -> bool {
        matches!(self, Self::ReservedTagKey)
    }
}

/// Which level of the addressed chain is named as something that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Break {
    /// Every name resolves — the control that shows the 400 is the fault's.
    Nothing,
    Group,
    Channel,
    Endpoint,
}

impl Break {
    fn slug(self) -> &'static str {
        match self {
            Self::Nothing => "n",
            Self::Group => "g",
            Self::Channel => "c",
            Self::Endpoint => "e",
        }
    }
}

/// An operation that takes tag keys from its request AND resolves names in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    CreateChannel,
    CreateOriginEndpoint,
    CreateHarvestJob,
    TagResource,
    UntagResource,
}

impl Site {
    const ALL: [Site; 5] = [
        Site::CreateChannel,
        Site::CreateOriginEndpoint,
        Site::CreateHarvestJob,
        Site::TagResource,
        Site::UntagResource,
    ];

    /// The levels this site addresses, and so the depths at which its lookups can fail.
    fn breaks(self) -> &'static [Break] {
        match self {
            // The tagging pair addresses one resource through an ARN; the rows below give
            // them a channel-group ARN.
            Self::CreateChannel | Self::TagResource | Self::UntagResource => &[Break::Group],
            Self::CreateOriginEndpoint => &[Break::Group, Break::Channel],
            Self::CreateHarvestJob => &[Break::Group, Break::Channel, Break::Endpoint],
        }
    }

    /// A short label keeping the resource names of the rows apart.
    fn slug(self) -> &'static str {
        match self {
            Self::CreateChannel => "chn",
            Self::CreateOriginEndpoint => "oep",
            Self::CreateHarvestJob => "hjb",
            Self::TagResource => "tag",
            Self::UntagResource => "untag",
        }
    }

    /// The wire code of this site's not-found class: the modelled
    /// `ResourceNotFoundException` for the creates, and the tagging API's own
    /// `NotFoundException` (`state::RESOURCE_NOT_FOUND_CODE`) for the two operations the
    /// model gives no 404 of their own.
    fn not_found_code(self) -> &'static str {
        match self {
            Self::TagResource | Self::UntagResource => "NotFoundException",
            _ => "ResourceNotFoundException",
        }
    }

    /// What an accepted request answers: the creates report the resource (200), the two
    /// tagging writers carry no payload (204).
    fn accepted_status(self) -> u16 {
        match self {
            Self::TagResource | Self::UntagResource => 204,
            _ => 200,
        }
    }

    /// Whether this site mints a resource, and so whether a refused row has a read-back to
    /// prove it stored nothing.
    fn mints_a_resource(self) -> bool {
        matches!(
            self,
            Self::CreateChannel | Self::CreateOriginEndpoint | Self::CreateHarvestJob
        )
    }
}

/// Send one request at `site`, addressing the hierarchy with `brk` broken and carrying
/// `tags` (`None` for a clean request that breaks no request-level rule). `child` names the
/// resource a create would mint, so no row can be refused for a name another row took.
async fn send(
    emulator: &Emulator,
    site: Site,
    brk: Break,
    child: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> (u16, Value) {
    let group = if brk == Break::Group {
        ABSENT_GROUP
    } else {
        LIVE_GROUP
    };
    let channel = if brk == Break::Channel {
        ABSENT_CHANNEL
    } else {
        LIVE_CHANNEL
    };
    let endpoint = if brk == Break::Endpoint {
        ABSENT_ENDPOINT
    } else {
        LIVE_ENDPOINT
    };
    match site {
        Site::CreateChannel => {
            let mut body = json!({});
            if let Some(tags) = tags {
                // `@jsonName("tags")` on `CreateChannelRequest$Tags`.
                body["tags"] = json!(tags);
            }
            emulator.create_channel(group, child, body).await
        }
        Site::CreateOriginEndpoint => {
            let mut body = minimal_endpoint("TS");
            if let Some(tags) = tags {
                // No `@jsonName` on `CreateOriginEndpointRequest$Tags`.
                body["Tags"] = json!(tags);
            }
            emulator.create_endpoint(group, channel, child, body).await
        }
        Site::CreateHarvestJob => {
            let start = epoch_seconds_from_now(60.0);
            let mut body = harvest_body(start, start + 600.0, &[PUBLISHED_MANIFEST]);
            body["HarvestJobName"] = json!(child);
            if let Some(tags) = tags {
                // No `@jsonName` on `CreateHarvestJobRequest$Tags`.
                body["Tags"] = json!(tags);
            }
            emulator
                .call(
                    "POST",
                    &format!(
                        "/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}\
                         /harvestJob"
                    ),
                    Some(body),
                )
                .await
        }
        Site::TagResource => {
            let submitted = tags.cloned().unwrap_or_else(|| map_of(&[ORDINARY]));
            emulator.tag(&group_arn(group), &submitted).await
        }
        Site::UntagResource => {
            let keys: Vec<String> = match tags {
                Some(tags) => tags.keys().cloned().collect(),
                None => vec![ORDINARY.0.to_string()],
            };
            let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
            emulator.untag(&group_arn(group), &borrowed).await
        }
    }
}

/// Read `child` back at the level `site` mints, always through the LIVE chain: a refused
/// create must leave nothing there, whichever name in the request was the one that would
/// not resolve.
async fn read_back(emulator: &Emulator, site: Site, child: &str) -> (u16, Value) {
    match site {
        Site::CreateChannel => emulator.get_channel(LIVE_GROUP, child).await,
        Site::CreateOriginEndpoint => {
            emulator
                .get_endpoint(LIVE_GROUP, LIVE_CHANNEL, child)
                .await
        }
        Site::CreateHarvestJob => {
            emulator
                .call(
                    "GET",
                    &format!(
                        "/channelGroup/{LIVE_GROUP}/channel/{LIVE_CHANNEL}/originEndpoint\
                         /{LIVE_ENDPOINT}/harvestJob/{child}"
                    ),
                    None,
                )
                .await
        }
        _ => panic!("case {child}: only a create has a resource to read back"),
    }
}

/// An emulator holding the live group -> channel -> origin endpoint chain the rows address,
/// with the group's minted ARN checked against [`group_arn`].
async fn live_hierarchy() -> Emulator {
    let emulator = Emulator::new();
    let (status, group) = emulator.create_group(LIVE_GROUP, json!({})).await;
    assert_eq!(
        status, 200,
        "CreateChannelGroup {LIVE_GROUP} expected 200, got {status} {group}"
    );
    assert_eq!(
        group.get("Arn").and_then(Value::as_str),
        Some(group_arn(LIVE_GROUP).as_str()),
        "the ARN builder must match the ARN the service mints, got {group}"
    );
    let (status, body) = emulator
        .create_channel(LIVE_GROUP, LIVE_CHANNEL, json!({}))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel {LIVE_CHANNEL} expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_endpoint(
            LIVE_GROUP,
            LIVE_CHANNEL,
            LIVE_ENDPOINT,
            minimal_endpoint("TS"),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint {LIVE_ENDPOINT} expected 200, got {status} {body}"
    );
    emulator
}

/// Assert one answer is the request-level refusal: 400, the modelled envelope, and a
/// message stating the rule that was broken.
fn assert_request_level_refusal(case: &str, fault: Fault, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: a request-level fault must be refused with HTTP 400, got \
         {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {body}"
    );
    let stated = fault.must_state();
    assert!(
        message(body).contains(&stated),
        "case {case:?}: the refusal must state the rule ({stated:?}), got {:?}",
        message(body)
    );
}

/// RULE: a tag refusal the request earns on its own face is answered before the names it
/// addresses are resolved — at every operation that takes tag keys, at every depth its
/// lookups reach, with both controls on every row.
#[tokio::test]
async fn a_request_level_tag_refusal_outranks_resolving_the_addressed_names() {
    let emulator = live_hierarchy().await;

    for (index, fault) in Fault::ALL.iter().enumerate() {
        for site in Site::ALL {
            if site == Site::UntagResource && !fault.reaches_a_remover() {
                continue;
            }
            let tags = fault.tags();

            for brk in site.breaks() {
                let label = format!("{fault:?} at {site:?}, {brk:?} missing");
                let child = format!("{}-{index}-{}", site.slug(), brk.slug());
                let (status, body) = send(&emulator, site, *brk, &child, Some(&tags)).await;
                assert_request_level_refusal(&label, *fault, status, &body);
                if site.mints_a_resource() {
                    let (read_status, read) = read_back(&emulator, site, &child).await;
                    assert_eq!(
                        read_status, 404,
                        "case {label:?}: a refused create must leave no resource to read, \
                         got {read_status} {read}"
                    );
                }

                // Control: the same chain, a CLEAN request. The lookup failure is still
                // reported, so putting the fault first swallows no 404.
                let clean_child = format!("{child}-clean");
                let (status, body) = send(&emulator, site, *brk, &clean_child, None).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a clean request at the same missing name must be the \
                     not-found class, got {status} {body}"
                );
                assert_eq!(
                    error_type(&body),
                    site.not_found_code(),
                    "case {label:?}: expected {}, got {body}",
                    site.not_found_code()
                );
            }

            // Control: the SAME faulty request with every name resolving earns the same
            // refusal, so the 400 above is the fault's and not an artefact of the chain.
            let resolved_label = format!("{fault:?} at {site:?}, nothing missing");
            let child = format!("{}-{index}-{}", site.slug(), Break::Nothing.slug());
            let (status, body) = send(&emulator, site, Break::Nothing, &child, Some(&tags)).await;
            assert_request_level_refusal(&resolved_label, *fault, status, &body);
            if site.mints_a_resource() {
                let (read_status, read) = read_back(&emulator, site, &child).await;
                assert_eq!(
                    read_status, 404,
                    "case {resolved_label:?}: a refused create must leave no resource to \
                     read, got {read_status} {read}"
                );
            }

            // Control: the same request, clean, with every name resolving IS accepted — so
            // each row's refusals are about the fault and the missing name, and nothing
            // else in the request.
            let accepted_label = format!("a clean request at {site:?}, nothing missing");
            let child = format!("{}-{index}-ok", site.slug());
            let (status, body) = send(&emulator, site, Break::Nothing, &child, None).await;
            assert_eq!(
                status,
                site.accepted_status(),
                "case {accepted_label:?}: expected {}, got {status} {body}",
                site.accepted_status()
            );
        }
    }
}

/// RULE, at the other request-level refusal an addressing operation can earn: a multiview
/// list that repeats an entry is about the request alone, so the create and the update
/// that write that member both answer it before resolving the names they address.
#[tokio::test]
async fn a_request_level_list_refusal_outranks_resolving_the_addressed_names() {
    let emulator = live_hierarchy().await;
    // A tile source must be a CMAF channel of the multiview channel's own group, so the
    // live group gets one, and a MULTIVIEW channel to address the updates at. Layout values
    // come from the model's `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL | LAYOUT_3EL
    // | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL).
    let source = "rlp-source";
    let tiled = "rlp-tiled";
    let (status, body) = emulator
        .create_channel(LIVE_GROUP, source, json!({"InputType": "CMAF"}))
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel {source} expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_channel(
            LIVE_GROUP,
            tiled,
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": multiview(&[source], &["LAYOUT_2EH"]),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateChannel {tiled} expected 200, got {status} {body}"
    );

    // One row per (which list repeats) x (which operation) x (whether the addressed chain
    // resolves): a repeat in the sources, a repeat in the layouts, and a repeat of a name
    // that does not even resolve as a source.
    let repeats: [(&str, Value, &str); 3] = [
        (
            "the same source twice",
            multiview(&[source, source], &["LAYOUT_2EH"]),
            "MULTIVIEW_DUPLICATE_SOURCE",
        ),
        (
            "a repeated source that does not resolve either",
            multiview(&[ABSENT_CHANNEL, ABSENT_CHANNEL], &["LAYOUT_2EH"]),
            "MULTIVIEW_DUPLICATE_SOURCE",
        ),
        (
            "the same layout twice",
            multiview(&[source], &["LAYOUT_3PL", "LAYOUT_3PL"]),
            "MULTIVIEW_DUPLICATE_LAYOUT",
        ),
    ];

    for (index, (label, configuration, expected_type)) in repeats.iter().enumerate() {
        // `CreateChannel` under a group that does not exist, and `UpdateChannel` at a
        // channel (and at a group) that does not exist: every one of them is the repeat's
        // own refusal.
        let addressed: [(&str, (u16, Value)); 3] = [
            (
                "CreateChannel under a missing group",
                emulator
                    .create_channel(
                        ABSENT_GROUP,
                        &format!("rlp-dup-{index}"),
                        json!({
                            "InputType": "MULTIVIEW",
                            "MultiviewConfiguration": configuration.clone(),
                        }),
                    )
                    .await,
            ),
            (
                "UpdateChannel at a missing channel",
                emulator
                    .update_channel(
                        LIVE_GROUP,
                        ABSENT_CHANNEL,
                        json!({"MultiviewConfiguration": configuration.clone()}),
                        &[],
                    )
                    .await,
            ),
            (
                "UpdateChannel under a missing group",
                emulator
                    .update_channel(
                        ABSENT_GROUP,
                        tiled,
                        json!({"MultiviewConfiguration": configuration.clone()}),
                        &[],
                    )
                    .await,
            ),
        ];
        for (where_, (status, body)) in addressed {
            let case = format!("{label:?} on {where_}");
            assert_eq!(
                status, 400,
                "case {case:?}: a repeated entry must be refused with HTTP 400, got \
                 {status} {body}"
            );
            assert_eq!(
                error_type(&body),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {body}"
            );
            assert_eq!(
                validation_type(&body),
                *expected_type,
                "case {case:?}: expected {expected_type}, got {body}"
            );
        }

        // Control: the same repeat with the chain resolving is the same refusal, and the
        // channel it addressed keeps the configuration it had.
        let case = format!("{label:?} on UpdateChannel with nothing missing");
        let (status, body) = emulator
            .update_channel(
                LIVE_GROUP,
                tiled,
                json!({"MultiviewConfiguration": configuration.clone()}),
                &[],
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            validation_type(&body),
            *expected_type,
            "case {case:?}: expected {expected_type}, got {body}"
        );
        let (status, read) = emulator.get_channel(LIVE_GROUP, tiled).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetChannel expected 200, got {status} {read}"
        );
        assert_eq!(
            read["MultiviewConfiguration"],
            multiview(&[source], &["LAYOUT_2EH"]),
            "case {case:?}: a refused update must leave the stored configuration alone, \
             got {read}"
        );
    }

    // Control: a clean update at the same missing names is still the not-found class.
    for (where_, group, channel) in [
        ("a missing channel", LIVE_GROUP, ABSENT_CHANNEL),
        ("a missing group", ABSENT_GROUP, tiled),
    ] {
        let (status, body) = emulator
            .update_channel(
                group,
                channel,
                json!({"MultiviewConfiguration": multiview(&[source], &["LAYOUT_4E"])}),
                &[],
            )
            .await;
        assert_eq!(
            status, 404,
            "case {where_:?}: a clean update at a name that does not resolve must be the \
             not-found class, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {where_:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}
