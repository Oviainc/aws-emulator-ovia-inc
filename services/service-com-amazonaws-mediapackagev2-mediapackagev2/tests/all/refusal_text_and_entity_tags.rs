// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for two rules of the MediaPackageV2 control plane, driven over HTTP
//! through the same router the emulator boots. Each one is asserted over a SET of inputs;
//! the request a bug report quotes is at most one row.
//!
//! 1. A REFUSAL'S TEXT IS REQUEST-INDEPENDENT. Whatever a caller asked for, a refusal
//!    carries the fixed sentence for the kind of resource and the rejection class, and
//!    none of the caller's own values — no channel group, channel or origin endpoint
//!    name, no `ClientToken`. The machine-readable signal is the status, the error code
//!    and the `ResourceTypeNotFound` / `ConflictExceptionType` discriminator. AWS
//!    publishes the channel and origin-endpoint not-found sentences and the
//!    idempotent-parameter-mismatch sentence, so those three are pinned verbatim; for the
//!    other classes the requirement asserted is the status, the code, the discriminator
//!    and that the text echoes nothing from the request.
//!    (The one request-derived message form this service has is the constraint violation,
//!    `Validation error encountered at request.<member>: <rule>`, which
//!    `control_plane_validation.rs` covers — it is built from the member and the bound,
//!    not from the caller's identifiers.) 22 not-found paths are covered, across every
//!    scope of the channel group -> channel -> origin endpoint hierarchy.
//! 2. AN ENTITY TAG IS A FUNCTION OF THE RESOURCE AND OF THE COMMIT COUNT THE STORE
//!    HOLDS IT AT, and a commit happens exactly when a write changes the resource. A
//!    committed change to `UpdateOriginEndpoint`'s endpoint answers a tag that differs
//!    from every tag the endpoint has had, whatever member changed — including members no
//!    manifest name mentions; a following `GetOriginEndpoint` reports exactly that tag;
//!    the tag the caller held before the change stops matching, so re-sent in
//!    `x-amzn-update-if-match` it is refused with the conflict class while the current one
//!    is accepted; and an update that stores what was already there commits nothing and
//!    leaves the tag alone. Asserted over updates that differ in WHICH part of the
//!    configuration they change.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

// The sentences AWS answers with, quoted from the recorded MediaPackage responses the
// issue reports (channel, origin endpoint, idempotent parameter mismatch). They are fixed
// per resource type and rejection class and contain none of the request's values.
const CHANNEL_GROUP_NOT_FOUND: &str = "MediaPackage can't process your request because \
    we can't find your channel group. Verify your channel group name or add a channel \
    group and then try again";
const CHANNEL_NOT_FOUND: &str = "MediaPackage can't process your request because we \
    can't find your channel. Verify your channel name or add a channel and then try \
    again";
const ORIGIN_ENDPOINT_NOT_FOUND: &str = "MediaPackage can't process your request \
    because we can't find your origin endpoint. Verify your origin endpoint name or add \
    an origin endpoint and then try again";
const IDEMPOTENT_PARAMETER_MISMATCH: &str = "You've specified a client token with your \
    request using parameter values that differ from a previous request that used the \
    same client token. Check your parameters and then try again";

/// The addresses that exist in every fixture below.
const GROUP: &str = "live-set-one";
const CHANNEL: &str = "contribution-feed";
const ENDPOINT: &str = "player-facing";

/// The names a request asks for that are NOT there, differing in length, case, digits,
/// underscores and dashes — the `ResourceName` pattern is `^[a-zA-Z0-9_-]+$`. None of
/// them is a substring of any of this service's sentences, so "the message does not echo
/// it" is a real assertion.
const ABSENT_NAMES: &[&str] = &[
    "nope",
    "A_Missing_Thing-0123456789",
    "q7",
    "zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz-zzzz",
];

/// A member of an error envelope, as a string (`""` when absent).
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .or_else(|| body.get(name.to_lowercase().as_str()))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert that nothing the caller sent made it into the refusal's text.
fn assert_echoes_nothing(case: &str, body: &Value, sent: &[&str]) {
    let message = member(body, "Message");
    assert!(
        !message.is_empty(),
        "case {case:?}: a refusal must carry a Message, got {body}"
    );
    for value in sent {
        assert!(
            !message.contains(value),
            "case {case:?}: the message must not echo {value:?}, got {message:?}"
        );
    }
}

/// Assert one not-found refusal: the status and code the model declares, the fixed
/// sentence for that resource type, its `ResourceTypeNotFound` discriminator, and no
/// value from the request.
fn assert_not_found(
    case: &str,
    answer: &(u16, Value),
    expected_message: &str,
    expected_kind: &str,
    sent: &[&str],
) {
    let (status, body) = answer;
    assert_eq!(
        *status, 404,
        "case {case:?}: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        member(body, "Message"),
        expected_message,
        "case {case:?}: expected the fixed sentence for this resource type, got {body}"
    );
    assert_eq!(
        member(body, "ResourceTypeNotFound"),
        expected_kind,
        "case {case:?}: expected ResourceTypeNotFound {expected_kind}, got {body}"
    );
    assert_echoes_nothing(case, body, sent);
}

/// Assert one conflict refusal: 409, the code, the discriminator, and no value from the
/// request.
fn assert_conflict(case: &str, answer: &(u16, Value), expected_kind: &str, sent: &[&str]) {
    let (status, body) = answer;
    assert_eq!(
        *status, 409,
        "case {case:?}: expected 409, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {body}"
    );
    assert_eq!(
        member(body, "ConflictExceptionType"),
        expected_kind,
        "case {case:?}: expected ConflictExceptionType {expected_kind}, got {body}"
    );
    assert_echoes_nothing(case, body, sent);
}

/// A group, a channel and an origin endpoint that all resolve.
async fn given_hierarchy(emulator: &Emulator) -> Value {
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "setup: creating the group answered {body}");
    let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(status, 200, "setup: creating the channel answered {body}");
    let (status, body) = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index", "ManifestWindowSeconds": 60}],
            }),
        )
        .await;
    assert_eq!(status, 200, "setup: creating the endpoint answered {body}");
    body
}

/// A complete `UpdateOriginEndpoint` body. Every update replaces the whole packaging
/// configuration, so each row carries one.
fn update_body(extra: Value) -> Value {
    let mut body = json!({
        "ContainerType": "TS",
        "HlsManifests": [{"ManifestName": "index", "ManifestWindowSeconds": 60}],
    });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    body
}

// =============================================================================
// RULE 1 — a refusal's text is request-independent
// =============================================================================

/// RULE 1 — every not-found path on the implemented surface answers the fixed sentence
/// for the resource type that is missing, whatever names the request carried.
#[tokio::test]
async fn a_resource_that_does_not_resolve_is_refused_with_the_fixed_sentence_for_its_type() {
    let emulator = Emulator::new();
    given_hierarchy(&emulator).await;
    let policy = json!({"Statement": [{"Effect": "Allow"}]}).to_string();
    let endpoint_body = update_body(json!({}));

    for absent in ABSENT_NAMES {
        let sent = [*absent, GROUP, CHANNEL, ENDPOINT];

        // The channel-group scope: the parent every other address hangs off.
        let rows: Vec<(String, (u16, Value))> = vec![
            (
                format!("GetChannelGroup {absent}"),
                emulator.get_group(absent).await,
            ),
            (
                format!("GetChannel in the absent group {absent}"),
                emulator.get_channel(absent, CHANNEL).await,
            ),
            (
                format!("ListOriginEndpoints in the absent group {absent}"),
                emulator.list_endpoints(absent, CHANNEL, "").await,
            ),
            (
                format!("CreateChannel in the absent group {absent}"),
                emulator.create_channel(absent, "new-channel", json!({})).await,
            ),
            (
                format!("CreateOriginEndpoint in the absent group {absent}"),
                emulator
                    .create_endpoint(absent, CHANNEL, "new-endpoint", minimal_endpoint("TS"))
                    .await,
            ),
            (
                format!("UpdateOriginEndpoint in the absent group {absent}"),
                emulator
                    .update_endpoint(absent, CHANNEL, ENDPOINT, endpoint_body.clone(), &[])
                    .await,
            ),
            (
                format!("ResetChannelState in the absent group {absent}"),
                emulator
                    .call(
                        "POST",
                        &format!("/channelGroup/{absent}/channel/{CHANNEL}/reset"),
                        None,
                    )
                    .await,
            ),
            (
                format!("ResetOriginEndpointState in the absent group {absent}"),
                emulator
                    .call(
                        "POST",
                        &format!(
                            "/channelGroup/{absent}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}\
                             /reset"
                        ),
                        None,
                    )
                    .await,
            ),
            (
                format!("GetChannelPolicy in the absent group {absent}"),
                emulator
                    .call(
                        "GET",
                        &format!("/channelGroup/{absent}/channel/{CHANNEL}/policy"),
                        None,
                    )
                    .await,
            ),
            (
                format!("PutChannelPolicy in the absent group {absent}"),
                emulator
                    .call(
                        "PUT",
                        &format!("/channelGroup/{absent}/channel/{CHANNEL}/policy"),
                        Some(json!({"Policy": policy})),
                    )
                    .await,
            ),
        ];
        for (case, answer) in &rows {
            assert_not_found(case, answer, CHANNEL_GROUP_NOT_FOUND, "CHANNEL_GROUP", &sent);
        }

        // The channel scope, reached through a group that DOES resolve.
        let rows: Vec<(String, (u16, Value))> = vec![
            (
                format!("GetChannel {absent}"),
                emulator.get_channel(GROUP, absent).await,
            ),
            (
                format!("ListOriginEndpoints on the absent channel {absent}"),
                emulator.list_endpoints(GROUP, absent, "").await,
            ),
            (
                format!("CreateOriginEndpoint on the absent channel {absent}"),
                emulator
                    .create_endpoint(GROUP, absent, "new-endpoint", minimal_endpoint("TS"))
                    .await,
            ),
            (
                format!("UpdateOriginEndpoint on the absent channel {absent}"),
                emulator
                    .update_endpoint(GROUP, absent, ENDPOINT, endpoint_body.clone(), &[])
                    .await,
            ),
            (
                format!("ResetChannelState on the absent channel {absent}"),
                emulator
                    .call(
                        "POST",
                        &format!("/channelGroup/{GROUP}/channel/{absent}/reset"),
                        None,
                    )
                    .await,
            ),
            (
                format!("ResetOriginEndpointState on the absent channel {absent}"),
                emulator
                    .call(
                        "POST",
                        &format!(
                            "/channelGroup/{GROUP}/channel/{absent}/originEndpoint/{ENDPOINT}\
                             /reset"
                        ),
                        None,
                    )
                    .await,
            ),
            (
                format!("GetChannelPolicy on the absent channel {absent}"),
                emulator
                    .call(
                        "GET",
                        &format!("/channelGroup/{GROUP}/channel/{absent}/policy"),
                        None,
                    )
                    .await,
            ),
            (
                format!("PutChannelPolicy on the absent channel {absent}"),
                emulator
                    .call(
                        "PUT",
                        &format!("/channelGroup/{GROUP}/channel/{absent}/policy"),
                        Some(json!({"Policy": policy})),
                    )
                    .await,
            ),
        ];
        for (case, answer) in &rows {
            assert_not_found(case, answer, CHANNEL_NOT_FOUND, "CHANNEL", &sent);
        }

        // The origin-endpoint scope.
        let rows: Vec<(String, (u16, Value))> = vec![
            (
                format!("GetOriginEndpoint {absent}"),
                emulator.get_endpoint(GROUP, CHANNEL, absent).await,
            ),
            (
                format!("UpdateOriginEndpoint {absent}"),
                emulator
                    .update_endpoint(GROUP, CHANNEL, absent, endpoint_body.clone(), &[])
                    .await,
            ),
            (
                format!("ResetOriginEndpointState {absent}"),
                emulator
                    .call(
                        "POST",
                        &format!(
                            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{absent}/reset"
                        ),
                        None,
                    )
                    .await,
            ),
            (
                format!("GetOriginEndpointPolicy {absent}"),
                emulator
                    .call(
                        "GET",
                        &format!(
                            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{absent}/policy"
                        ),
                        None,
                    )
                    .await,
            ),
            (
                format!("PutOriginEndpointPolicy {absent}"),
                emulator
                    .call(
                        "POST",
                        &format!(
                            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{absent}/policy"
                        ),
                        Some(json!({"Policy": policy})),
                    )
                    .await,
            ),
        ];
        for (case, answer) in &rows {
            assert_not_found(case, answer, ORIGIN_ENDPOINT_NOT_FOUND, "ORIGIN_ENDPOINT", &sent);
        }
    }
}

/// RULE 1, the path the behavioral checks take — the "effect absent" read after a create
/// the service REFUSED. The read is a plain not-found, so it carries the same fixed
/// sentence as any other, never the name the refused create asked for.
#[tokio::test]
async fn the_read_after_a_refused_create_carries_the_fixed_not_found_sentence() {
    let emulator = Emulator::new();
    given_hierarchy(&emulator).await;

    // Channel-scope rows: creates the service refuses for a cross-member rule, each
    // naming a channel that must not come into being.
    let channel_rows = [
        (
            "an HLS channel asking for OutputLockingMode",
            "refused-locking",
            json!({"InputType": "HLS", "OutputLockingMode": "EPOCH_LOCKING"}),
        ),
        (
            "an HLS channel asking for MQCS input switching",
            "refused-mqcs",
            json!({"InputType": "HLS", "InputSwitchConfiguration": {"MQCSInputSwitching": true}}),
        ),
        (
            "a CMAF channel asking for a MultiviewConfiguration",
            "refused-multiview",
            json!({"InputType": "CMAF", "MultiviewConfiguration": {"MaxChannels": 2}}),
        ),
    ];
    for (label, name, body) in channel_rows {
        let (status, refused) = emulator.create_channel(GROUP, name, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the create to be refused with 400, got {status} {refused}"
        );
        let answer = emulator.get_channel(GROUP, name).await;
        assert_not_found(
            label,
            &answer,
            CHANNEL_NOT_FOUND,
            "CHANNEL",
            &[name, GROUP, CHANNEL],
        );
    }

    // Origin-endpoint-scope rows: manifest sets the endpoint's container type refuses.
    let endpoint_rows = [
        (
            "an endpoint with no manifest at all",
            "refused-no-manifest",
            json!({"ContainerType": "TS"}),
        ),
        (
            "a TS endpoint asking for a DASH manifest",
            "refused-ts-dash",
            json!({"ContainerType": "TS", "DashManifests": [{"ManifestName": "dash"}]}),
        ),
        (
            "an ISM endpoint asking for an HLS manifest",
            "refused-ism-hls",
            json!({"ContainerType": "ISM", "HlsManifests": [{"ManifestName": "index"}]}),
        ),
    ];
    for (label, name, body) in endpoint_rows {
        let (status, refused) = emulator.create_endpoint(GROUP, CHANNEL, name, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected the create to be refused with 400, got {status} {refused}"
        );
        let answer = emulator.get_endpoint(GROUP, CHANNEL, name).await;
        assert_not_found(
            label,
            &answer,
            ORIGIN_ENDPOINT_NOT_FOUND,
            "ORIGIN_ENDPOINT",
            &[name, GROUP, CHANNEL],
        );
    }
}

/// The three levels an `@idempotent` create exists at. Each binds `ClientToken` with
/// `@idempotencyToken`, so each is a row of the table below.
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

/// Send the create at `level` for `name` under `token`, carrying `description` — the
/// member a "changed repeat" row varies.
async fn create_under_token(
    emulator: &Emulator,
    level: Level,
    name: &str,
    token: &str,
    description: &str,
) -> (u16, Value) {
    let headers = [("x-amzn-client-token", token)];
    match level {
        Level::Group => {
            emulator
                .call_with_headers(
                    "POST",
                    "/channelGroup",
                    Some(json!({"ChannelGroupName": name, "Description": description})),
                    &headers,
                )
                .await
        }
        Level::Channel => {
            emulator
                .call_with_headers(
                    "POST",
                    &format!("/channelGroup/{GROUP}/channel"),
                    Some(json!({"ChannelName": name, "Description": description})),
                    &headers,
                )
                .await
        }
        Level::Endpoint => {
            let mut body = minimal_endpoint("TS");
            body["OriginEndpointName"] = json!(name);
            body["Description"] = json!(description);
            emulator
                .call_with_headers(
                    "POST",
                    &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint"),
                    Some(body),
                    &headers,
                )
                .await
        }
    }
}

/// RULE 1 at the idempotency-conflict scope — a `ClientToken` replayed with changed
/// parameters is refused with the service's fixed sentence, which never contains the
/// token, and the stored resource is left as the first request made it.
#[tokio::test]
async fn an_idempotency_token_replayed_with_changed_parameters_never_echoes_the_token() {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "setup: creating the group answered {body}");
    let (status, body) = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(status, 200, "setup: creating the channel answered {body}");

    // One row per level, with tokens of different shapes — `IdempotencyToken` is
    // `length 1..256, pattern ^[\S]+$`.
    let rows = [
        (Level::Group, "t0k", "idem-group"),
        (
            Level::Channel,
            "a-token-with-digits-0123456789-and-dashes",
            "idem-channel",
        ),
        (Level::Endpoint, "T!o@k#e$n%^&*()_+", "idem-endpoint"),
    ];
    for (level, token, name) in rows {
        let label = level.label();
        let (status, created) =
            create_under_token(&emulator, level, name, token, "the original description").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create answered {status} {created}"
        );
        // The positive control: an identical repeat under the same token replays.
        let (status, replayed) =
            create_under_token(&emulator, level, name, token, "the original description").await;
        assert_eq!(
            status, 200,
            "case {label:?}: an identical repeat must replay, got {status} {replayed}"
        );
        assert_eq!(
            replayed["Arn"], created["Arn"],
            "case {label:?}: the replay must answer the resource the first create made"
        );

        // The conflict: the same token, a different parameter.
        let answer =
            create_under_token(&emulator, level, name, token, "a different description").await;
        let case = format!("{label}: the token replayed with a changed Description");
        assert_conflict(
            &case,
            &answer,
            "IDEMPOTENT_PARAMETER_MISMATCH",
            &[token, name, GROUP, CHANNEL],
        );
        assert_eq!(
            member(&answer.1, "Message"),
            IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected the service's fixed sentence, got {}",
            answer.1
        );

        // And the refused retry changed nothing.
        let read = match level {
            Level::Group => emulator.get_group(name).await,
            Level::Channel => emulator.get_channel(GROUP, name).await,
            Level::Endpoint => emulator.get_endpoint(GROUP, CHANNEL, name).await,
        };
        assert_eq!(
            read.1["Description"], created["Description"],
            "case {case:?}: a refused retry must leave the stored resource alone"
        );
    }
}

/// RULE 1 over the remaining rejection classes: a name already taken, a parent that still
/// has children, and a collection at its ceiling. AWS publishes no sentence for these, so
/// what is asserted is the status, the code, the discriminator — and that the text carries
/// nothing the caller sent.
#[tokio::test]
async fn every_other_refusal_class_also_echoes_nothing_from_the_request() {
    let emulator = Emulator::new();
    given_hierarchy(&emulator).await;

    // A name already taken, at each of the three levels.
    let rows: Vec<(String, (u16, Value))> = vec![
        (
            "a channel group name already taken".to_string(),
            emulator.create_group(GROUP, json!({})).await,
        ),
        (
            "a channel name already taken".to_string(),
            emulator.create_channel(GROUP, CHANNEL, json!({})).await,
        ),
        (
            "an origin endpoint name already taken".to_string(),
            emulator
                .create_endpoint(GROUP, CHANNEL, ENDPOINT, minimal_endpoint("TS"))
                .await,
        ),
    ];
    for (case, answer) in &rows {
        assert_conflict(
            case,
            answer,
            "RESOURCE_ALREADY_EXISTS",
            &[GROUP, CHANNEL, ENDPOINT],
        );
    }

    // A delete that would orphan children, at each level that has any.
    let rows: Vec<(String, (u16, Value))> = vec![
        (
            "deleting a channel that still has an origin endpoint".to_string(),
            emulator
                .call(
                    "DELETE",
                    &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/"),
                    None,
                )
                .await,
        ),
        (
            "deleting a channel group that still has a channel".to_string(),
            emulator.delete_group(GROUP).await,
        ),
    ];
    for (case, answer) in &rows {
        assert_conflict(case, answer, "RESOURCE_IN_USE", &[GROUP, CHANNEL, ENDPOINT]);
    }

    // A collection at its ceiling: the channels of one group. The emulator's bound is its
    // own (AWS's is an adjustable quota), so the test finds it by creating until the
    // create is refused rather than by naming a number.
    let mut refused = None;
    for index in 0..1_000 {
        let name = format!("filler-{index}");
        let (status, body) = emulator.create_channel(GROUP, &name, json!({})).await;
        if status != 200 {
            refused = Some((name, status, body));
            break;
        }
    }
    let (name, status, body) = refused.expect("the channel ceiling must refuse some create");
    assert_eq!(
        status, 402,
        "case \"a channel group at its ceiling\": expected 402, got {status} {body}"
    );
    assert_eq!(
        error_type(&body),
        "ServiceQuotaExceededException",
        "case \"a channel group at its ceiling\": expected the quota error, got {body}"
    );
    assert_echoes_nothing(
        "a channel group at its ceiling",
        &body,
        &[&name, GROUP, CHANNEL],
    );
}

// =============================================================================
// RULE 2 — the entity tag is a function of the stored resource version
// =============================================================================

/// RULE 2 — a committed change rotates the tag, the read agrees with the update, the tag
/// the caller held before the change stops matching, and a write that stored what was
/// already there leaves the tag alone.
#[tokio::test]
async fn a_committed_update_rotates_the_origin_endpoints_entity_tag() {
    let emulator = Emulator::new();
    let created = given_hierarchy(&emulator).await;

    // Each row changes a DIFFERENT part of the configuration — one of them is the
    // recorded case (only a manifest's `ManifestWindowSeconds` moves). The last row
    // re-submits the row before it, so it stores what is already there and must NOT
    // rotate the tag: the third column says which a row is.
    let rows = [
        (
            "only ManifestWindowSeconds changes",
            update_body(json!({
                "HlsManifests": [{"ManifestName": "index", "ManifestWindowSeconds": 120}],
            })),
            true,
        ),
        (
            "only Description changes",
            update_body(json!({"Description": "after the update"})),
            true,
        ),
        (
            "a second manifest appears",
            update_body(json!({
                "HlsManifests": [
                    {"ManifestName": "index", "ManifestWindowSeconds": 60},
                    {"ManifestName": "backup", "ManifestWindowSeconds": 90},
                ],
            })),
            true,
        ),
        (
            "only the manifest's FilterConfiguration changes",
            update_body(json!({
                "HlsManifests": [{
                    "ManifestName": "index",
                    "ManifestWindowSeconds": 60,
                    "FilterConfiguration": {"TimeDelaySeconds": 10},
                }],
            })),
            true,
        ),
        (
            "only StartoverWindowSeconds changes",
            update_body(json!({"StartoverWindowSeconds": 300})),
            true,
        ),
        (
            "only the segment settings change",
            update_body(json!({"Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"}})),
            true,
        ),
        (
            "only UriSeparator changes",
            update_body(json!({"UriSeparator": "HYPHEN"})),
            true,
        ),
        (
            "only the manifest's ChildManifestName changes",
            update_body(json!({
                "HlsManifests": [{
                    "ManifestName": "index",
                    "ChildManifestName": "child",
                    "ManifestWindowSeconds": 60,
                }],
            })),
            true,
        ),
        (
            "an identical repeat of the previous body",
            update_body(json!({
                "HlsManifests": [{
                    "ManifestName": "index",
                    "ChildManifestName": "child",
                    "ManifestWindowSeconds": 60,
                }],
            })),
            false,
        ),
    ];

    let mut previous = created["ETag"]
        .as_str()
        .expect("the create must report an ETag")
        .to_string();
    let mut seen = BTreeSet::new();
    seen.insert(previous.clone());

    for (label, body, changes) in rows {
        let (status, updated) = emulator
            .update_endpoint(GROUP, CHANNEL, ENDPOINT, body.clone(), &[])
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the update to be accepted, got {status} {updated}"
        );
        let current = updated["ETag"].as_str().unwrap_or_default().to_string();
        // `EntityTag` is `length 1..256, pattern ^[\S]+$`.
        assert!(
            (1..=256).contains(&current.chars().count())
                && !current.chars().any(char::is_whitespace),
            "case {label:?}: {current:?} is not a well-formed EntityTag"
        );
        if changes {
            assert_ne!(
                current, previous,
                "case {label:?}: a committed change must rotate the ETag, got {current:?} again"
            );
            assert!(
                seen.insert(current.clone()),
                "case {label:?}: {current:?} repeats a tag this endpoint already had"
            );
        } else {
            assert_eq!(
                current, previous,
                "case {label:?}: storing what was already there must leave the ETag alone"
            );
        }

        // The read reports exactly the tag the update answered with, and the rest of what
        // it stored.
        let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await;
        assert_eq!(
            status, 200,
            "case {label:?}: reading the endpoint back answered {status} {read}"
        );
        for reported in ["ETag", "ModifiedAt", "Description", "HlsManifests", "Segment"] {
            assert_eq!(
                read[reported], updated[reported],
                "case {label:?}: GetOriginEndpoint {reported} must equal what the update \
                 stored, got {:?} against {:?}",
                read[reported], updated[reported]
            );
        }

        // The tag the caller held BEFORE this commit no longer matches, so re-sending it
        // as the precondition is the conflict class — and changes nothing. (Only where
        // something was committed: with nothing committed, `previous` IS the current tag.)
        if changes {
            let answer = emulator
                .update_endpoint(
                    GROUP,
                    CHANNEL,
                    ENDPOINT,
                    body.clone(),
                    &[("x-amzn-update-if-match", &previous)],
                )
                .await;
            let stale = format!("{label}: the pre-change ETag re-sent as the precondition");
            assert_conflict(&stale, &answer, "CONFLICTING_OPERATION", &[&previous]);
            assert_eq!(
                emulator.get_endpoint(GROUP, CHANNEL, ENDPOINT).await.1["ETag"],
                json!(current),
                "case {stale:?}: a refused update must leave the stored ETag alone"
            );
        }

        // The CURRENT tag is accepted as the precondition. This repeat stores exactly
        // what is there, so it commits nothing and the tag stays.
        let (status, accepted) = emulator
            .update_endpoint(
                GROUP,
                CHANNEL,
                ENDPOINT,
                body,
                &[("x-amzn-update-if-match", &current)],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the current ETag must be accepted, got {status} {accepted}"
        );
        assert_eq!(
            accepted["ETag"],
            json!(current),
            "case {label:?}: a conditional update that changed nothing must keep the tag"
        );
        previous = current;
    }
}
