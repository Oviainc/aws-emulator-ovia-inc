// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the MediaPackageV2 UPDATE operations, driven over HTTP through
//! the same router the emulator boots, on the shared harness in
//! [`crate::channel_lifecycle`].
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A successful update COMMITS the resource's mutable content to the store, restamps
//!    `ModifiedAt` and re-derives the stored entity tag, and answers from that record —
//!    so the matching read reports exactly the content and the `ETag` the update
//!    returned, while the values the update does not own (identity, ARN, egress domain,
//!    creation stamp, ingest endpoints, input type) are unchanged.
//! 2. `ETag` in `x-amzn-update-if-match` is a precondition: the resource's current tag
//!    lets the update through, any other tag is refused with `ConflictException` (409)
//!    and leaves the stored resource exactly as it was.
//! 3. An update addressing a resource whose parent chain does not resolve answers
//!    `ResourceNotFoundException` (404) and stores nothing.
//! 4. An update replaces the endpoint's list-valued and numeric content too: the
//!    manifests it carries become the endpoint's manifests (each with the URL the
//!    service computes for it), and `StartoverWindowSeconds` is persisted across its
//!    published range, a value past the bound being refused.
//! 5. The members the model documents as PRESERVED keep their stored value when the
//!    update omits them, rather than being cleared by the replacement rule; the ones it
//!    documents as fixed once the resource has them refuse a change with the modelled
//!    `ValidationException` (400) and commit nothing.
//!
//! The immutability of `ContainerType` and `StreamNameOutputMode`, and the manifest-set
//! rules, are covered by [`crate::control_plane_validation`]; this suite adds the
//! segment's output-timestamp mode and the channel's multiview configuration.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// The conditional-update header `ETag` is bound to on every `Update*` operation
/// (`smithy_query(query="operation", operation="UpdateChannelGroup")` reports
/// `ETag.httpHeader = x-amzn-update-if-match`).
const IF_MATCH: &str = "x-amzn-update-if-match";

/// The value at a path of members, or `Value::Null` when the path is absent.
fn at(body: &Value, path: &[&str]) -> Value {
    let mut cursor = body;
    for member in path {
        cursor = &cursor[*member];
    }
    cursor.clone()
}

/// `base` with every member of `overlay` set on it — the request bodies below are
/// written as a shared base plus the one fragment a row varies.
fn merged(base: &Value, overlay: &Value) -> Value {
    let mut body = base.clone();
    for (member, value) in overlay.as_object().expect("an object overlay") {
        body[member] = value.clone();
    }
    body
}

/// The three levels an update exists at. Every one of them takes a `Description`, hands
/// back an `ETag` and matches `x-amzn-update-if-match` against the stored one, so every
/// one of them is a row of the tables below.
#[derive(Debug, Clone, Copy)]
enum Level {
    Group,
    Channel,
    Endpoint,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Group => "channel group",
            Self::Channel => "channel",
            Self::Endpoint => "origin endpoint",
        }
    }
}

/// The parent chain every channel/endpoint row hangs off.
const GROUP: &str = "upd-grp";
const CHANNEL: &str = "upd-chan";
const ENDPOINT: &str = "upd-ep";

/// Create the resource at `level` with `description`, and return the create's response.
async fn create_at(emulator: &Emulator, level: Level, description: Option<&str>) -> Value {
    let described = |body: Value| match description {
        Some(text) => merged(&body, &json!({"Description": text})),
        None => body,
    };
    let (status, body) = match level {
        Level::Group => emulator.create_group(GROUP, described(json!({}))).await,
        Level::Channel => {
            emulator
                .create_channel(GROUP, CHANNEL, described(json!({"InputType": "HLS"})))
                .await
        }
        Level::Endpoint => {
            emulator
                .create_endpoint(
                    GROUP,
                    CHANNEL,
                    ENDPOINT,
                    described(merged(
                        &minimal_endpoint("TS"),
                        &json!({"StartoverWindowSeconds": 300}),
                    )),
                )
                .await
        }
    };
    assert_eq!(
        status,
        200,
        "case {:?}: the create expected 200, got {status} {body}",
        level.label()
    );
    body
}

/// Update the resource at `level` with `body`, optionally under an `ETag` precondition.
async fn update_at(
    emulator: &Emulator,
    level: Level,
    body: Value,
    if_match: Option<&str>,
) -> (u16, Value) {
    let headers: Vec<(&str, &str)> = if_match.map(|tag| vec![(IF_MATCH, tag)]).unwrap_or_default();
    match level {
        Level::Group => emulator.update_group(GROUP, body, &headers).await,
        Level::Channel => emulator.update_channel(GROUP, CHANNEL, body, &headers).await,
        Level::Endpoint => {
            emulator
                .update_endpoint(GROUP, CHANNEL, ENDPOINT, body, &headers)
                .await
        }
    }
}

/// Read the resource at `level` back.
async fn read_at(emulator: &Emulator, level: Level) -> (u16, Value) {
    match level {
        Level::Group => emulator.get_group(GROUP).await,
        Level::Channel => emulator.get_channel(GROUP, CHANNEL).await,
        Level::Endpoint => emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await,
    }
}

/// The body an update at `level` sends when it carries nothing but what the operation
/// requires — the members an update may not omit.
fn required_body(level: Level) -> Value {
    match level {
        Level::Group | Level::Channel => json!({}),
        // An origin endpoint's update carries its container type and (at least one)
        // manifest on every call.
        Level::Endpoint => minimal_endpoint("TS"),
    }
}

/// One row of the update table: what an update sends for `Description`.
///
/// The rows differ along the VALUE and the SIZE of the submitted content, and along
/// whether the member is sent at all — not along which resource is renamed.
struct DescriptionCase {
    label: &'static str,
    /// What the update sends; `None` omits the member, which clears the description.
    sent: Option<String>,
}

impl DescriptionCase {
    /// What every response must report for `Description` afterwards: the submitted text,
    /// or no member at all when the update omitted it.
    fn expected(&self) -> Value {
        self.sent
            .as_ref()
            .map_or(Value::Null, |text| json!(text.clone()))
    }

    /// This row's update body at `level`.
    fn body(&self, level: Level) -> Value {
        match &self.sent {
            Some(text) => merged(&required_body(level), &json!({"Description": text})),
            None => required_body(level),
        }
    }
}

/// The rows. The length bounds come from the model: `Description` targets
/// `ResourceDescription`, `@length 0..1024`
/// (`smithy_query(query="operation", operation="UpdateChannelGroup")`).
fn description_cases() -> Vec<DescriptionCase> {
    vec![
        DescriptionCase {
            label: "plain replacement text",
            sent: Some("updated description".to_string()),
        },
        DescriptionCase {
            label: "the empty string, the lower length bound",
            sent: Some(String::new()),
        },
        DescriptionCase {
            label: "1024 characters, the upper length bound",
            sent: Some("d".repeat(1024)),
        },
        DescriptionCase {
            label: "punctuation and non-ASCII text",
            sent: Some("flux 1/2 — \"quoted\", prüfung ✓".to_string()),
        },
        DescriptionCase {
            label: "the member omitted, which clears the description",
            sent: None,
        },
    ]
}

/// RULE 1 — a successful update is committed, and the tag it returns is the tag the next
/// read reports, at all three levels of the hierarchy.
#[tokio::test]
async fn an_update_is_committed_and_the_tag_it_returns_is_what_the_next_read_reports() {
    for level in [Level::Group, Level::Channel, Level::Endpoint] {
        for case in description_cases() {
            let label = format!("{} / {}", level.label(), case.label);
            let emulator = Emulator::new();
            // The parent chain of the level under test.
            if !matches!(level, Level::Group) {
                create_at(&emulator, Level::Group, None).await;
            }
            if matches!(level, Level::Endpoint) {
                create_at(&emulator, Level::Channel, None).await;
            }
            let created = create_at(&emulator, level, Some("initial description")).await;

            let (status, updated) = update_at(&emulator, level, case.body(level), None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the update expected 200, got {status} {updated}"
            );
            assert_eq!(
                updated["Description"],
                case.expected(),
                "case {label:?}: the update must report the submitted description, got {updated}"
            );
            assert_ne!(
                updated["ETag"], created["ETag"],
                "case {label:?}: the entity tag must move when the content does, got {updated}"
            );

            let (status, read) = read_at(&emulator, level).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the read expected 200, got {status} {read}"
            );
            for member in ["Description", "ETag", "ModifiedAt"] {
                assert_eq!(
                    read[member], updated[member],
                    "case {label:?}: the read's {member} must be what the update returned, got {read}"
                );
            }
            // What the update does not own is untouched by it.
            let untouched: &[&str] = match level {
                Level::Group => &["Arn", "EgressDomain", "CreatedAt", "ChannelGroupName"],
                Level::Channel => &[
                    "Arn",
                    "CreatedAt",
                    "InputType",
                    "IngestEndpoints",
                    "ChannelName",
                ],
                Level::Endpoint => &["Arn", "CreatedAt", "ContainerType", "OriginEndpointName"],
            };
            for member in untouched {
                assert_eq!(
                    read[*member], created[*member],
                    "case {label:?}: an update must not change {member}, got {read}"
                );
            }
        }
    }
}

/// RULE 2 — the conditional `ETag` is a precondition: the current tag lets an update
/// through, and any other tag is refused and changes nothing.
#[tokio::test]
async fn a_conditional_update_needs_the_resources_current_entity_tag() {
    for level in [Level::Group, Level::Channel, Level::Endpoint] {
        let label = level.label();
        let emulator = Emulator::new();
        if !matches!(level, Level::Group) {
            create_at(&emulator, Level::Group, None).await;
        }
        if matches!(level, Level::Endpoint) {
            create_at(&emulator, Level::Channel, None).await;
        }
        let created = create_at(&emulator, level, Some("first")).await;
        let fresh = created["ETag"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the create must report an ETag, got {created}"))
            .to_string();

        // The resource's own tag is accepted, and hands out the next one.
        let second = merged(&required_body(level), &json!({"Description": "second"}));
        let (status, updated) = update_at(&emulator, level, second, Some(&fresh)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an update carrying the current ETag expected 200, got {status} {updated}"
        );
        let committed = updated["ETag"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the update must report an ETag, got {updated}"))
            .to_string();
        assert_ne!(
            committed, fresh,
            "case {label:?}: the tag must move once the update is committed"
        );

        // The tag the caller held BEFORE that update no longer matches, so a second
        // conditional update on it is refused and the record is left alone.
        let third = merged(&required_body(level), &json!({"Description": "third"}));
        let (status, refused) = update_at(&emulator, level, third, Some(&fresh)).await;
        assert_eq!(
            status, 409,
            "case {label:?}: an update carrying a superseded ETag expected 409, got {status} {refused}"
        );
        assert_eq!(
            error_type(&refused),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {refused}"
        );

        let (status, read) = read_at(&emulator, level).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read after a refused update expected 200, got {status} {read}"
        );
        assert_eq!(
            read["Description"],
            json!("second"),
            "case {label:?}: a refused update must leave the stored content alone, got {read}"
        );
        assert_eq!(
            read["ETag"],
            json!(committed),
            "case {label:?}: the read must report the tag the last committed update returned, got {read}"
        );
    }
}

/// RULE 3 — an update whose parent chain does not resolve is a 404, whichever level it
/// addresses, and stores nothing.
#[tokio::test]
async fn an_update_of_an_unresolvable_resource_is_not_found() {
    let emulator = Emulator::new();
    emulator.create_group(GROUP, json!({})).await;
    emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    let endpoint_body = minimal_endpoint("TS");

    let cases: [(&str, (u16, Value)); 5] = [
        (
            "absent channel group",
            emulator
                .update_group("no-such-group", json!({"Description": "x"}), &[])
                .await,
        ),
        (
            "channel in an absent group",
            emulator
                .update_channel("no-such-group", CHANNEL, json!({"Description": "x"}), &[])
                .await,
        ),
        (
            "absent channel in a live group",
            emulator
                .update_channel(GROUP, "no-such-channel", json!({"Description": "x"}), &[])
                .await,
        ),
        (
            "endpoint on an absent channel",
            emulator
                .update_endpoint(GROUP, "no-such-channel", "ep", endpoint_body.clone(), &[])
                .await,
        ),
        (
            "absent endpoint on a live channel",
            emulator
                .update_endpoint(GROUP, CHANNEL, "no-such-endpoint", endpoint_body, &[])
                .await,
        ),
    ];
    for (label, (status, body)) in cases {
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
    // Nothing the refused updates named came into being.
    let (status, body) = emulator.get_group("no-such-group").await;
    assert_eq!(
        status, 404,
        "a refused update must not create the group it named, got {status} {body}"
    );
}

/// RULE 4 — an update replaces the endpoint's manifests and its start-over window.
///
/// `StartoverWindowSeconds` is `@range 0..1209600`
/// (`smithy_query(query="operation", operation="UpdateOriginEndpoint")`), so the rows
/// below carry both published bounds, an interior value and one past the upper bound.
/// Each row's manifest families are ones its container serves, and the container itself
/// is fixed once the endpoint exists, so it is part of the row.
#[tokio::test]
async fn an_update_replaces_the_endpoints_manifests_and_startover_window() {
    // `(json member, manifest name, the URL suffix of that family)`
    type Manifests = &'static [(&'static str, &'static str, &'static str)];
    // (label, container type, StartoverWindowSeconds, the manifests the update carries,
    // expected status)
    let cases: &[(&str, &str, i64, Manifests, u16)] = &[
        (
            "lower bound, one HLS manifest",
            "TS",
            0,
            &[("HlsManifests", "hls-only", ".m3u8")],
            200,
        ),
        (
            "interior value, two HLS manifests",
            "TS",
            3600,
            &[
                ("HlsManifests", "hls-a", ".m3u8"),
                ("HlsManifests", "hls-b", ".m3u8"),
            ],
            200,
        ),
        (
            "upper bound, every family a CMAF endpoint serves",
            "CMAF",
            1_209_600,
            &[
                ("HlsManifests", "hls", ".m3u8"),
                ("LowLatencyHlsManifests", "ll-hls", ".m3u8"),
                ("DashManifests", "dash", ".mpd"),
            ],
            200,
        ),
        (
            "an ISM endpoint's MSS manifest",
            "ISM",
            600,
            &[("MssManifests", "smooth", ".ism/Manifest")],
            200,
        ),
        (
            "one past the upper bound",
            "TS",
            1_209_601,
            &[("HlsManifests", "hls-only", ".m3u8")],
            400,
        ),
    ];

    for (label, container, window, manifests, expected_status) in cases {
        let emulator = Emulator::new();
        let (_, created_group) = emulator.create_group(GROUP, json!({})).await;
        emulator.create_channel(GROUP, CHANNEL, json!({})).await;
        // The endpoint starts with a manifest and a window the update must replace.
        let (status, created_endpoint) = emulator
            .create_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                merged(
                    &minimal_endpoint(container),
                    &json!({"StartoverWindowSeconds": 60}),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created_endpoint}"
        );

        let mut body = json!({"ContainerType": container, "StartoverWindowSeconds": window});
        for (member, name, _) in *manifests {
            body[*member] = match body.get(*member).and_then(Value::as_array) {
                Some(existing) => {
                    let mut list = existing.clone();
                    list.push(json!({"ManifestName": name}));
                    Value::Array(list)
                }
                None => json!([{"ManifestName": name}]),
            };
        }
        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, ENDPOINT, body, &[])
            .await;
        assert_eq!(
            status, *expected_status,
            "case {label:?}: UpdateOriginEndpoint expected {expected_status}, got {status} {updated}"
        );

        let (_, read) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
        if *expected_status != 200 {
            // A refused update commits nothing: the record still reads as it was created.
            for member in ["StartoverWindowSeconds", "HlsManifests", "ETag"] {
                assert_eq!(
                    read[member], created_endpoint[member],
                    "case {label:?}: a refused update must leave {member} alone, got {read}"
                );
            }
            continue;
        }

        assert_eq!(
            read["StartoverWindowSeconds"],
            json!(window),
            "case {label:?}: the submitted start-over window must be stored, got {read}"
        );
        assert_eq!(
            read["ETag"], updated["ETag"],
            "case {label:?}: the read must report the tag the update returned, got {read}"
        );
        // The manifests the update carried are the endpoint's manifests now — the one it
        // was created with is gone, and every submitted manifest has the URL the service
        // computes for it, rooted on the parent group's egress domain.
        let egress = created_group["EgressDomain"]
            .as_str()
            .expect("the create must report an egress domain");
        for member in [
            "HlsManifests",
            "LowLatencyHlsManifests",
            "DashManifests",
            "MssManifests",
        ] {
            let expected: Vec<&(&str, &str, &str)> =
                manifests.iter().filter(|(m, _, _)| *m == member).collect();
            let stored = read[member].as_array().cloned().unwrap_or_default();
            assert_eq!(
                stored.len(),
                expected.len(),
                "case {label:?}: {member} must hold exactly the submitted manifests, got {read}"
            );
            for ((_, name, suffix), got) in expected.iter().zip(stored.iter()) {
                assert_eq!(
                    got["ManifestName"],
                    json!(name),
                    "case {label:?}: {member} must report the submitted manifest name, got {got}"
                );
                let url = got["Url"].as_str().unwrap_or_default();
                assert_eq!(
                    url,
                    format!("https://{egress}/out/v1/{GROUP}/{CHANNEL}/{ENDPOINT}/{name}{suffix}"),
                    "case {label:?}: {member} URL must be computed for the submitted manifest, got {got}"
                );
            }
        }
    }
}

/// RULE 5 — a member the model documents as PRESERVED keeps its stored value when the
/// update omits it, and (where it is mutable) takes a submitted one.
///
/// The rows and the model sentence each comes from
/// (`smithy_query(query="doc", shape=..., member=...)`):
/// * `UriSeparator` — "If you don't specify a value in the update request, the current
///   value is preserved."; both published enum values (`HYPHEN`, `UNDERSCORE`) appear.
/// * `StreamNameOutputMode` — "You can't change the stream name output mode after you
///   create the endpoint."
/// * `Segment.OutputTimestampMode` — "This value is immutable after endpoint creation."
#[tokio::test]
async fn a_member_documented_as_preserved_survives_an_update_that_omits_it() {
    // (label, the channel the endpoint hangs off, the fragment the endpoint is created
    // with, where to read the member back). The channel matters for one row:
    // `Segment.OutputTimestampMode` is "only configurable on channels with
    // OutputLockingMode set to NON_EPOCH_LOCKED".
    let rows: &[(&str, Value, Value, &[&str])] = &[
        (
            "UriSeparator HYPHEN",
            json!({}),
            json!({"UriSeparator": "HYPHEN"}),
            &["UriSeparator"],
        ),
        (
            "UriSeparator UNDERSCORE",
            json!({}),
            json!({"UriSeparator": "UNDERSCORE"}),
            &["UriSeparator"],
        ),
        (
            "StreamNameOutputMode PASSTHROUGH_NAME",
            json!({}),
            json!({"StreamNameOutputMode": "PASSTHROUGH_NAME"}),
            &["StreamNameOutputMode"],
        ),
        (
            "Segment.OutputTimestampMode REBASED_TO_CHANNEL_START",
            json!({"InputType": "CMAF", "OutputLockingMode": "NON_EPOCH_LOCKED"}),
            json!({"Segment": {"OutputTimestampMode": "REBASED_TO_CHANNEL_START"}}),
            &["Segment", "OutputTimestampMode"],
        ),
    ];
    let base = minimal_endpoint("TS");

    for (label, channel_body, created_fragment, path) in rows {
        let emulator = Emulator::new();
        emulator.create_group(GROUP, json!({})).await;
        emulator
            .create_channel(GROUP, CHANNEL, channel_body.clone())
            .await;
        let (status, created) = emulator
            .create_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                merged(&base, created_fragment),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );
        let stored = at(&created, path);
        assert!(
            !stored.is_null(),
            "case {label:?}: the create must report the submitted member, got {created}"
        );

        // An update that omits the member must not clear it.
        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, ENDPOINT, base.clone(), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {updated}"
        );
        assert_eq!(
            at(&updated, path),
            stored,
            "case {label:?}: an update that omits the member must preserve it, got {updated}"
        );
        let (_, read) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
        assert_eq!(
            at(&read, path),
            stored,
            "case {label:?}: the read must report the preserved member, got {read}"
        );
        assert_eq!(
            read["ETag"], updated["ETag"],
            "case {label:?}: the read must report the tag the update returned, got {read}"
        );
    }

    // `UriSeparator` is preserved, not frozen: a submitted value replaces it.
    let emulator = Emulator::new();
    emulator.create_group(GROUP, json!({})).await;
    emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            merged(&base, &json!({"UriSeparator": "HYPHEN"})),
        )
        .await;
    let (status, updated) = emulator
        .update_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            merged(&base, &json!({"UriSeparator": "UNDERSCORE"})),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"UriSeparator replaced\": expected 200, got {status} {updated}"
    );
    let (_, read) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
    assert_eq!(
        read["UriSeparator"],
        json!("UNDERSCORE"),
        "case \"UriSeparator replaced\": the submitted separator must be stored, got {read}"
    );
}

/// RULE 5, the refusal half for the segment's output-timestamp mode: "This value is
/// immutable after endpoint creation" (`Segment$OutputTimestampMode`), and the service's
/// `ValidationExceptionType` names `OUTPUT_TIMESTAMP_MODE_IMMUTABLE` for it. Every
/// endpoint whose channel carries the member HAS a stored mode — the service applies the
/// documented default at create — so there is no first update that sets one; the whole
/// table of channels that carry it and channels that do not lives in
/// [`crate::segment_output_timestamp_mode`].
#[tokio::test]
async fn the_segments_output_timestamp_mode_is_fixed_once_the_endpoint_has_one() {
    let base = minimal_endpoint("TS");
    // The member is "only configurable on channels with OutputLockingMode set to
    // NON_EPOCH_LOCKED", so the endpoints below hang off such a channel.
    let channel_body = json!({"InputType": "CMAF", "OutputLockingMode": "NON_EPOCH_LOCKED"});
    // Both published values of the model's `OutputTimestampMode` enum, each as the stored
    // mode and as the one an update tries to put in its place.
    for (stored, other) in [
        ("PASSTHROUGH", "REBASED_TO_CHANNEL_START"),
        ("REBASED_TO_CHANNEL_START", "PASSTHROUGH"),
    ] {
        let emulator = Emulator::new();
        emulator.create_group(GROUP, json!({})).await;
        emulator
            .create_channel(GROUP, CHANNEL, channel_body.clone())
            .await;
        let (status, created) = emulator
            .create_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                merged(&base, &json!({"Segment": {"OutputTimestampMode": stored}})),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {stored:?}: CreateOriginEndpoint expected 200, got {status} {created}"
        );

        // Resubmitting the stored mode is no change, so it is accepted.
        let (status, again) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                merged(&base, &json!({"Segment": {"OutputTimestampMode": stored}})),
                &[],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {stored:?}: an update repeating the stored mode expected 200, got {status} {again}"
        );

        // Naming the other one is, and is refused.
        let (status, refused) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                merged(&base, &json!({"Segment": {"OutputTimestampMode": other}})),
                &[],
            )
            .await;
        assert_eq!(
            status, 400,
            "case {stored:?} -> {other:?}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_type(&refused),
            "ValidationException",
            "case {stored:?} -> {other:?}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["ValidationExceptionType"],
            json!("OUTPUT_TIMESTAMP_MODE_IMMUTABLE"),
            "case {stored:?} -> {other:?}: expected OUTPUT_TIMESTAMP_MODE_IMMUTABLE, got {refused}"
        );

        // Nothing was committed.
        let (_, read) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
        assert_eq!(
            at(&read, &["Segment", "OutputTimestampMode"]),
            json!(stored),
            "case {stored:?} -> {other:?}: a refused update must leave the mode alone, got {read}"
        );
    }

    // An endpoint created without the member still HAS one: the service applied the
    // documented default, so the first update naming a different mode is refused like any
    // other change to it.
    let emulator = Emulator::new();
    emulator.create_group(GROUP, json!({})).await;
    emulator
        .create_channel(GROUP, CHANNEL, channel_body.clone())
        .await;
    let (_, created) = emulator
        .create_endpoint(GROUP, CHANNEL, ENDPOINT, base.clone())
        .await;
    assert_eq!(
        at(&created, &["Segment", "OutputTimestampMode"]),
        json!("PASSTHROUGH"),
        "case \"the create omitted the mode\": the documented default must be stored, got {created}"
    );
    let (status, refused) = emulator
        .update_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            merged(
                &base,
                &json!({"Segment": {"OutputTimestampMode": "REBASED_TO_CHANNEL_START"}}),
            ),
            &[],
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"the create omitted the mode\": expected 400, got {status} {refused}"
    );
    assert_eq!(
        refused["ValidationExceptionType"],
        json!("OUTPUT_TIMESTAMP_MODE_IMMUTABLE"),
        "case \"the create omitted the mode\": expected OUTPUT_TIMESTAMP_MODE_IMMUTABLE, \
         got {refused}"
    );
    let (_, read) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
    assert_eq!(
        at(&read, &["Segment", "OutputTimestampMode"]),
        json!("PASSTHROUGH"),
        "case \"the create omitted the mode\": the refused update must leave the default \
         mode alone, got {read}"
    );
}

/// RULE 5, at channel scope — a member the model restricts to particular `InputType`s is
/// refused on an UPDATE too, with that member's own `ValidationExceptionType`, and the
/// channel is left alone; on an input type that may carry it the same update is accepted.
///
/// The rows are the restrictions the model states and the service's
/// `ValidationExceptionType` names, checked here against the channel's STORED input type
/// (which is immutable): `MQCSInputSwitching`, `PreferredInput` and `PublishMQCS` are
/// "valid only when InputType is CMAF", and `MultiviewConfiguration` "can't be set for
/// any other input type" than `MULTIVIEW`.
#[tokio::test]
async fn an_update_cannot_carry_a_member_the_channels_input_type_forbids() {
    let multiview = json!({
        "MultiviewConfiguration": {
            "AvailableSources": ["source"],
            "AvailableLayouts": ["LAYOUT_2EH"],
        },
    });
    // (label, the channel's InputType, the update body, the expected
    // ValidationExceptionType or None when the combination is allowed)
    let rows: &[(&str, &str, Value, Option<&str>)] = &[
        (
            "MQCS switching on an HLS channel",
            "HLS",
            json!({"InputSwitchConfiguration": {"MQCSInputSwitching": true}}),
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING"),
        ),
        (
            "a preferred input on an HLS channel",
            "HLS",
            json!({"InputSwitchConfiguration": {"PreferredInput": 1}}),
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_PREFERRED_INPUT_CONFIGURATION"),
        ),
        (
            "CMSD publication on an HLS channel",
            "HLS",
            json!({"OutputHeaderConfiguration": {"PublishMQCS": true}}),
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION"),
        ),
        (
            "a multiview configuration on a CMAF channel",
            "CMAF",
            multiview.clone(),
            Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        ),
        (
            "a multiview configuration on an HLS channel",
            "HLS",
            multiview,
            Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        ),
        (
            "MQCS switching on a CMAF channel",
            "CMAF",
            json!({"InputSwitchConfiguration": {"MQCSInputSwitching": true, "PreferredInput": 2}}),
            None,
        ),
        (
            "CMSD publication on a CMAF channel",
            "CMAF",
            json!({"OutputHeaderConfiguration": {"PublishMQCS": true}}),
            None,
        ),
    ];

    for (label, input_type, body, expected_type) in rows {
        let emulator = Emulator::new();
        emulator.create_group(GROUP, json!({})).await;
        let (status, created) = emulator
            .create_channel(GROUP, CHANNEL, json!({"InputType": input_type}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateChannel expected 200, got {status} {created}"
        );

        let (status, answered) = emulator
            .update_channel(GROUP, CHANNEL, body.clone(), &[])
            .await;
        let (_, read) = emulator.get_channel(GROUP, CHANNEL).await;
        match expected_type {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected 400, got {status} {answered}"
                );
                assert_eq!(
                    error_type(&answered),
                    "ValidationException",
                    "case {label:?}: expected ValidationException, got {answered}"
                );
                assert_eq!(
                    answered["ValidationExceptionType"],
                    json!(expected),
                    "case {label:?}: expected ValidationExceptionType {expected}, got {answered}"
                );
                for member in [
                    "InputSwitchConfiguration",
                    "OutputHeaderConfiguration",
                    "MultiviewConfiguration",
                    "ETag",
                    "ModifiedAt",
                ] {
                    assert_eq!(
                        read[member], created[member],
                        "case {label:?}: a refused update must leave {member} alone, got {read}"
                    );
                }
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected 200, got {status} {answered}"
                );
                for (member, submitted) in body.as_object().expect("an object body") {
                    for (inner, value) in submitted.as_object().expect("an object member") {
                        assert_eq!(
                            read[member][inner],
                            *value,
                            "case {label:?}: {member}.{inner} must be stored, got {read}"
                        );
                    }
                }
                assert_eq!(
                    read["ETag"], answered["ETag"],
                    "case {label:?}: the read must report the tag the update returned, got {read}"
                );
            }
        }
    }
}

/// RULE 5, at channel scope — the multiview configuration "can't be added or removed",
/// while a multiview channel's sources and layouts can change
/// (`smithy_query(query="doc", shape="UpdateChannelRequest", member="MultiviewConfiguration")`).
/// The layout values come from the model's `MultiviewLayoutType` enum.
#[tokio::test]
async fn a_multiview_configuration_can_be_changed_but_not_added_or_removed() {
    let emulator = Emulator::new();
    emulator.create_group(GROUP, json!({})).await;
    // A multiview channel's sources must be channels of the same group with InputType
    // CMAF.
    emulator
        .create_channel(GROUP, "source-a", json!({"InputType": "CMAF"}))
        .await;
    emulator
        .create_channel(GROUP, "source-b", json!({"InputType": "CMAF"}))
        .await;
    let (status, created) = emulator
        .create_channel(
            GROUP,
            "mv-channel",
            json!({
                "InputType": "MULTIVIEW",
                "MultiviewConfiguration": {
                    "AvailableSources": ["source-a"],
                    "AvailableLayouts": ["LAYOUT_2EH"],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"multiview create\": expected 200, got {status} {created}"
    );

    // Omitting the member leaves the channel's configuration in place.
    let (status, updated) = emulator
        .update_channel(GROUP, "mv-channel", json!({"Description": "kept"}), &[])
        .await;
    assert_eq!(
        status, 200,
        "case \"multiview omitted\": expected 200, got {status} {updated}"
    );
    let (_, read) = emulator.get_channel(GROUP, "mv-channel").await;
    assert_eq!(
        read["MultiviewConfiguration"], created["MultiviewConfiguration"],
        "case \"multiview omitted\": the stored configuration must survive, got {read}"
    );

    // Its sources and layouts can change.
    let (status, updated) = emulator
        .update_channel(
            GROUP,
            "mv-channel",
            json!({
                "MultiviewConfiguration": {
                    "AvailableSources": ["source-a", "source-b"],
                    "AvailableLayouts": ["LAYOUT_4E", "LAYOUT_2PL"],
                },
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"multiview changed\": expected 200, got {status} {updated}"
    );
    let (_, read) = emulator.get_channel(GROUP, "mv-channel").await;
    assert_eq!(
        read["MultiviewConfiguration"]["AvailableLayouts"],
        json!(["LAYOUT_4E", "LAYOUT_2PL"]),
        "case \"multiview changed\": the submitted layouts must be stored, got {read}"
    );
    assert_eq!(
        read["MultiviewConfiguration"]["AvailableSources"],
        json!(["source-a", "source-b"]),
        "case \"multiview changed\": the submitted sources must be stored, got {read}"
    );

    // A channel created without one cannot be given one.
    let (_, plain) = emulator.get_channel(GROUP, "source-a").await;
    let (status, refused) = emulator
        .update_channel(
            GROUP,
            "source-a",
            json!({
                "MultiviewConfiguration": {
                    "AvailableSources": ["source-b"],
                    "AvailableLayouts": ["LAYOUT_2EH"],
                },
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"multiview added\": expected 400, got {status} {refused}"
    );
    assert_eq!(
        error_type(&refused),
        "ValidationException",
        "case \"multiview added\": expected ValidationException, got {refused}"
    );
    assert_eq!(
        refused["ValidationExceptionType"],
        json!("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        "case \"multiview added\": expected MULTIVIEW_CONFIGURATION_NOT_ALLOWED, got {refused}"
    );
    let (_, read) = emulator.get_channel(GROUP, "source-a").await;
    for member in ["MultiviewConfiguration", "ETag", "ModifiedAt", "Description"] {
        assert_eq!(
            read[member], plain[member],
            "case \"multiview added\": a refused update must leave {member} alone, got {read}"
        );
    }
}
