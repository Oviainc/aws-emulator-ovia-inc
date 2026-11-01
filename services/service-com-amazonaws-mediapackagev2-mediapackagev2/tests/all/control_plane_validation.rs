// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the semantic rules the MediaPackageV2 control plane enforces on
//! a create, an update and a list — the server-side checks a client cannot do for itself
//! — driven over HTTP through the same router the emulator boots.
//!
//! Each test asserts ONE rule over a SET of inputs; the request a bug report quotes is at
//! most one row of each table.
//!
//! 1. An `@idempotent` create binding `ClientToken` with `@idempotencyToken` replays its
//!    first result for an unchanged repeat under the same token, and refuses a changed
//!    one with `ConflictException` / `IDEMPOTENT_PARAMETER_MISMATCH`, leaving the stored
//!    resource alone. A name taken WITHOUT that token stays `RESOURCE_ALREADY_EXISTS`.
//! 2. A channel member the model restricts to particular `InputType`s is refused with 400
//!    `ValidationException` and that member's own `ValidationExceptionType` on any other
//!    input type, and nothing is created.
//! 3. An origin endpoint's `ContainerType` decides which manifest families it may carry;
//!    a family it cannot serve is a 400 `ValidationException` carrying the
//!    `<container>_CONTAINER_TYPE_WITH_<family>_MANIFEST` discriminator, on create and on
//!    update alike.
//! 4. An origin endpoint needs at least one manifest (`NUM_MANIFESTS_LOW`).
//! 5. A pagination token the service did not issue is a 400 `ValidationException` /
//!    `INVALID_PAGINATION_TOKEN` on every paged list — never a server-fault class — while
//!    a token it did issue pages the rest of the collection.
//! 6. Every refusal this service makes from the model's own declared constraints is
//!    reported in mediapackagev2's form, `Validation error encountered at
//!    request.<member>: <the violated rule>`.
//! 7. A member the model documents as immutable after creation is refused when an update
//!    changes it, leaving the resource and its ETag untouched; an update that changes
//!    something else succeeds and rotates the ETag.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// The `Message` of an error envelope.
///
/// A refusal the request boundary made renders the member as lowercase `message` (the
/// framework's synthetic shape), while a handler's modeled `ValidationException` renders
/// the model's own `Message`; both carry the same sentence, so both are read here.
fn message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `ConflictExceptionType` discriminator of an error envelope.
fn conflict_type(body: &Value) -> &str {
    body.get("ConflictExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Assert that `body` is this service's own `ValidationException`: 400, the modeled code,
/// the expected discriminator, and the message form the whole surface uses.
fn assert_validation(case: &str, status: u16, body: &Value, expected_type: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        validation_type(body),
        expected_type,
        "case {case:?}: expected ValidationExceptionType {expected_type}, got {body}"
    );
    assert!(
        message(body).starts_with("Validation error encountered at request"),
        "case {case:?}: expected this service's validation message form, got {:?}",
        message(body)
    );
}

// =============================================================================
// RULE 1 — ClientToken idempotency on every create
// =============================================================================

/// The three levels a create exists at. Every one of them binds `ClientToken` with
/// `@idempotencyToken` and is `@idempotent`, so every one of them is a row of the table.
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

/// The parent chain every channel/endpoint row in this suite hangs off.
const PARENT_GROUP: &str = "idem-grp";
const PARENT_CHANNEL: &str = "idem-chan";

/// Create a resource at `level`, optionally under a `ClientToken` and with a
/// `Description` — the member a "changed repeat" row varies.
async fn create_at(
    emulator: &Emulator,
    level: Level,
    name: &str,
    description: Option<&str>,
    token: Option<&str>,
) -> (u16, Value) {
    let headers: Vec<(&str, &str)> = token
        .map(|token| vec![("x-amzn-client-token", token)])
        .unwrap_or_default();
    let (path, mut body, name_member) = match level {
        Level::Group => ("/channelGroup".to_string(), json!({}), "ChannelGroupName"),
        Level::Channel => (
            format!("/channelGroup/{PARENT_GROUP}/channel"),
            json!({"InputType": "HLS"}),
            "ChannelName",
        ),
        Level::Endpoint => (
            format!("/channelGroup/{PARENT_GROUP}/channel/{PARENT_CHANNEL}/originEndpoint"),
            minimal_endpoint("TS"),
            "OriginEndpointName",
        ),
    };
    body[name_member] = json!(name);
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    emulator
        .call_with_headers("POST", &path, Some(body), &headers)
        .await
}

/// Read a resource back at `level`.
async fn read_at(emulator: &Emulator, level: Level, name: &str) -> (u16, Value) {
    match level {
        Level::Group => emulator.get_group(name).await,
        Level::Channel => emulator.get_channel(PARENT_GROUP, name).await,
        Level::Endpoint => emulator.get_endpoint(PARENT_GROUP, PARENT_CHANNEL, name).await,
    }
}

/// RULE 1 — a retry under the same token replays; a changed request under it conflicts.
#[tokio::test]
async fn a_create_retried_under_its_client_token_replays_and_a_changed_one_conflicts() {
    for level in [Level::Group, Level::Channel, Level::Endpoint] {
        let label = level.label();
        let emulator = Emulator::new();
        assert_eq!(
            emulator.create_group(PARENT_GROUP, json!({})).await.0,
            200,
            "case {label:?}: the parent group must be created"
        );
        assert_eq!(
            emulator
                .create_channel(PARENT_GROUP, PARENT_CHANNEL, json!({}))
                .await
                .0,
            200,
            "case {label:?}: the parent channel must be created"
        );

        let token = "fixed-client-token-1";
        let (status, first) = create_at(&emulator, level, "res-a", Some("first"), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first create expected 200, got {status} {first}"
        );

        // An SDK retrying the same create on a timeout sends the same token again.
        let (status, replay) =
            create_at(&emulator, level, "res-a", Some("first"), Some(token)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an identical retry expected a 200 replay, got {status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {label:?}: an identical retry must replay the first response verbatim"
        );

        // Same token, a changed member: the model's dedicated conflict type.
        let (_, before) = read_at(&emulator, level, "res-a").await;
        let (status, changed) =
            create_at(&emulator, level, "res-a", Some("second"), Some(token)).await;
        assert_eq!(
            status, 409,
            "case {label:?}: a changed retry expected 409, got {status} {changed}"
        );
        assert_eq!(
            error_type(&changed),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {changed}"
        );
        assert_eq!(
            conflict_type(&changed),
            "IDEMPOTENT_PARAMETER_MISMATCH",
            "case {label:?}: expected IDEMPOTENT_PARAMETER_MISMATCH, got {changed}"
        );

        // Same token, a different name: the name is part of the request, so this is the
        // same mismatch — and it creates nothing.
        let (status, renamed) =
            create_at(&emulator, level, "res-b", Some("first"), Some(token)).await;
        assert_eq!(
            conflict_type(&renamed),
            "IDEMPOTENT_PARAMETER_MISMATCH",
            "case {label:?}: a reused token under another name expected \
             IDEMPOTENT_PARAMETER_MISMATCH, got {status} {renamed}"
        );
        assert_eq!(
            read_at(&emulator, level, "res-b").await.0,
            404,
            "case {label:?}: a refused create must store nothing"
        );

        // The taken name under ANOTHER token, and under none at all, is still the
        // already-exists conflict: idempotency is about the token, not about the name.
        for (row, other_token) in [
            ("another token", Some("fixed-client-token-2")),
            ("no token", None),
        ] {
            let (status, taken) = create_at(&emulator, level, "res-a", Some("first"), other_token).await;
            assert_eq!(
                status, 409,
                "case {label:?}/{row}: expected 409, got {status} {taken}"
            );
            assert_eq!(
                conflict_type(&taken),
                "RESOURCE_ALREADY_EXISTS",
                "case {label:?}/{row}: expected RESOURCE_ALREADY_EXISTS, got {taken}"
            );
        }

        assert_eq!(
            read_at(&emulator, level, "res-a").await.1,
            before,
            "case {label:?}: a refused repeat must leave the stored resource untouched"
        );
    }
}

// =============================================================================
// RULE 2 — a channel member the model restricts to particular input types
// =============================================================================

/// One row: the channel members under test, the `InputType` they are sent with, and what
/// the service must answer. `InputType` values are the model's `InputType` enum (HLS,
/// CMAF, MULTIVIEW) plus the absent case, which the member's doc defaults to HLS.
struct InputTypeCase {
    label: &'static str,
    input_type: Option<&'static str>,
    members: fn() -> Value,
    /// The expected `ValidationExceptionType`, or `None` when the combination is legal.
    refused_as: Option<&'static str>,
}

fn input_type_cases() -> Vec<InputTypeCase> {
    vec![
        InputTypeCase {
            label: "OutputLockingMode EPOCH_LOCKED on HLS input",
            input_type: Some("HLS"),
            members: || json!({"OutputLockingMode": "EPOCH_LOCKED"}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE"),
        },
        InputTypeCase {
            label: "OutputLockingMode NON_EPOCH_LOCKED on HLS input",
            input_type: Some("HLS"),
            members: || json!({"OutputLockingMode": "NON_EPOCH_LOCKED"}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE"),
        },
        InputTypeCase {
            label: "OutputLockingMode with no InputType, which defaults to HLS",
            input_type: None,
            members: || json!({"OutputLockingMode": "EPOCH_LOCKED"}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE"),
        },
        InputTypeCase {
            label: "OutputLockingMode on CMAF input",
            input_type: Some("CMAF"),
            members: || json!({"OutputLockingMode": "EPOCH_LOCKED"}),
            refused_as: None,
        },
        InputTypeCase {
            label: "OutputLockingMode NON_EPOCH_LOCKED on MULTIVIEW input",
            input_type: Some("MULTIVIEW"),
            // A MULTIVIEW channel carries the configuration its own doc calls required on
            // that input type, over the CMAF source this suite seeds — so the only thing
            // the create can be refused for is the locking mode it also names, which a
            // multiview channel carries at its other value alone.
            members: || {
                json!({
                    "OutputLockingMode": "NON_EPOCH_LOCKED",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["source-a"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            refused_as: Some("MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED"),
        },
        InputTypeCase {
            label: "OutputLockingMode EPOCH_LOCKED on MULTIVIEW input",
            input_type: Some("MULTIVIEW"),
            members: || {
                json!({
                    "OutputLockingMode": "EPOCH_LOCKED",
                    "MultiviewConfiguration": {
                        "AvailableSources": ["source-a"],
                        "AvailableLayouts": ["LAYOUT_2EH"],
                    },
                })
            },
            refused_as: None,
        },
        InputTypeCase {
            label: "MQCS input switching on HLS input",
            input_type: Some("HLS"),
            members: || json!({"InputSwitchConfiguration": {"MQCSInputSwitching": true}}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING"),
        },
        InputTypeCase {
            label: "MQCS input switching on CMAF input",
            input_type: Some("CMAF"),
            members: || json!({"InputSwitchConfiguration": {"MQCSInputSwitching": true}}),
            refused_as: None,
        },
        InputTypeCase {
            label: "PreferredInput on MULTIVIEW input",
            input_type: Some("MULTIVIEW"),
            members: || json!({"InputSwitchConfiguration": {"PreferredInput": 2}}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_PREFERRED_INPUT_CONFIGURATION"),
        },
        InputTypeCase {
            label: "PublishMQCS on HLS input",
            input_type: Some("HLS"),
            members: || json!({"OutputHeaderConfiguration": {"PublishMQCS": true}}),
            refused_as: Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION"),
        },
        InputTypeCase {
            label: "PublishMQCS on CMAF input",
            input_type: Some("CMAF"),
            members: || json!({"OutputHeaderConfiguration": {"PublishMQCS": false}}),
            refused_as: None,
        },
        InputTypeCase {
            label: "MultiviewConfiguration on CMAF input",
            input_type: Some("CMAF"),
            members: || {
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": ["source-a"],
                        "AvailableLayouts": ["LAYOUT_4E"],
                    }
                })
            },
            refused_as: Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        },
        InputTypeCase {
            label: "MultiviewConfiguration on MULTIVIEW input",
            input_type: Some("MULTIVIEW"),
            members: || {
                json!({
                    "MultiviewConfiguration": {
                        "AvailableSources": ["source-a"],
                        "AvailableLayouts": ["LAYOUT_4E"],
                    }
                })
            },
            refused_as: None,
        },
    ]
}

/// RULE 2 — a member only some input types may carry is refused on the others, and the
/// refused channel is not created.
#[tokio::test]
async fn a_channel_member_restricted_to_an_input_type_is_refused_on_the_others() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    // The source a multiview row names must be a CMAF channel of the same group, which is
    // what `MultiviewConfiguration$AvailableSources` requires of a tile source.
    assert_eq!(
        emulator
            .create_channel("grp", "source-a", json!({"InputType": "CMAF"}))
            .await
            .0,
        200,
        "seeding the CMAF source channel must succeed"
    );

    for (index, case) in input_type_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("chan-{index}");
        let mut body = (case.members)();
        if let Some(input_type) = case.input_type {
            body["InputType"] = json!(input_type);
        }
        let (status, answer) = emulator.create_channel("grp", &name, body).await;

        match case.refused_as {
            Some(expected_type) => {
                assert_validation(label, status, &answer, expected_type);
                assert_eq!(
                    emulator.get_channel("grp", &name).await.0,
                    404,
                    "case {label:?}: a refused create must store nothing"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected 200, got {status} {answer}"
                );
                assert_eq!(
                    emulator.get_channel("grp", &name).await.0,
                    200,
                    "case {label:?}: an accepted create must be readable"
                );
            }
        }
    }
}

// =============================================================================
// RULES 3 and 4 — which manifests a container type serves, and how few it may have
// =============================================================================

/// Every container type the model's `ContainerType` enum declares.
const CONTAINER_TYPES: [&str; 3] = ["TS", "CMAF", "ISM"];

/// Every manifest family an origin endpoint's request members carry: the member, one
/// manifest for it, and the `<container>_CONTAINER_TYPE_WITH_<family>_MANIFEST`
/// discriminator per container that refuses it (read off the model's
/// `ValidationExceptionType`).
const MANIFEST_FAMILIES: [(&str, &str); 4] = [
    ("HlsManifests", "hls"),
    ("LowLatencyHlsManifests", "llhls"),
    ("DashManifests", "dash"),
    ("MssManifests", "mss"),
];

/// The `ValidationExceptionType` the model names for `container` + `member`, or `None`
/// when that container serves that family.
fn expected_conflict(container: &str, member: &str) -> Option<&'static str> {
    match (container, member) {
        ("TS", "DashManifests") => Some("TS_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        ("TS", "MssManifests") => Some("TS_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        ("CMAF", "MssManifests") => Some("CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        ("ISM", "HlsManifests") => Some("ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST"),
        ("ISM", "LowLatencyHlsManifests") => Some("ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST"),
        ("ISM", "DashManifests") => Some("ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        _ => None,
    }
}

/// RULE 3 — the whole container × manifest-family matrix, on create and on update.
#[tokio::test]
async fn a_manifest_family_its_container_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );

    for container in CONTAINER_TYPES {
        for (member, manifest_name) in MANIFEST_FAMILIES {
            let label = format!("create {container} + {member}");
            let name = format!("{container}-{manifest_name}").to_lowercase();
            let body = json!({
                "ContainerType": container,
                member: [{"ManifestName": manifest_name}],
            });
            let (status, answer) = emulator
                .create_endpoint("grp", "chan", &name, body.clone())
                .await;

            match expected_conflict(container, member) {
                Some(expected_type) => {
                    assert_validation(&label, status, &answer, expected_type);
                    assert_eq!(
                        emulator.get_endpoint("grp", "chan", &name).await.0,
                        404,
                        "case {label:?}: a refused create must store nothing"
                    );
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {label:?}: expected 200, got {status} {answer}"
                    );
                    assert_eq!(
                        answer[member].as_array().map(Vec::len),
                        Some(1),
                        "case {label:?}: the accepted manifest must be reported, got {answer}"
                    );
                }
            }
        }
    }

    // The same rule on the update path: the endpoint exists with a legal manifest set,
    // and an update that swaps in a family its container cannot serve is refused.
    for container in CONTAINER_TYPES {
        let legal = if container == "ISM" {
            "MssManifests"
        } else {
            "HlsManifests"
        };
        let name = format!("update-{container}").to_lowercase();
        let (status, created) = emulator
            .create_endpoint(
                "grp",
                "chan",
                &name,
                json!({"ContainerType": container, legal: [{"ManifestName": "index"}]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"update {container} setup\": expected 200, got {status} {created}"
        );

        for (member, manifest_name) in MANIFEST_FAMILIES {
            let Some(expected_type) = expected_conflict(container, member) else {
                continue;
            };
            let label = format!("update {container} + {member}");
            let (status, answer) = emulator
                .update_endpoint(
                    "grp",
                    "chan",
                    &name,
                    json!({
                        "ContainerType": container,
                        member: [{"ManifestName": manifest_name}],
                    }),
                    &[],
                )
                .await;
            assert_validation(&label, status, &answer, expected_type);
            assert_eq!(
                emulator.get_endpoint("grp", "chan", &name).await.1["ETag"],
                created["ETag"],
                "case {label:?}: a refused update must leave the endpoint's ETag alone"
            );
        }
    }
}

/// RULE 4 — an endpoint with no manifest at all is refused, whatever its container type,
/// on create and on update.
#[tokio::test]
async fn an_origin_endpoint_with_no_manifest_is_refused() {
    // The recorded, request-derived form of this refusal: it names no member, because the
    // rule is about the request as a whole.
    const EXPECTED: &str = "Validation error encountered at request: You've requested an \
                            origin endpoint without a manifest. Your origin endpoint needs \
                            to contain at least one manifest. Add a manifest and try to \
                            create your origin endpoint again";

    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );

    for container in CONTAINER_TYPES {
        // Both the absent members and explicitly empty lists are "no manifest".
        for (row, body) in [
            ("no manifest members", json!({"ContainerType": container})),
            (
                "empty manifest lists",
                json!({
                    "ContainerType": container,
                    "HlsManifests": [],
                    "LowLatencyHlsManifests": [],
                    "DashManifests": [],
                    "MssManifests": [],
                }),
            ),
        ] {
            let label = format!("create {container}: {row}");
            let name = format!("empty-{container}-{row}").replace(' ', "-").to_lowercase();
            let (status, answer) = emulator.create_endpoint("grp", "chan", &name, body).await;
            assert_validation(&label, status, &answer, "NUM_MANIFESTS_LOW");
            assert_eq!(
                message(&answer),
                EXPECTED,
                "case {label:?}: the message must state the rule that produced it"
            );
            assert_eq!(
                emulator.get_endpoint("grp", "chan", &name).await.0,
                404,
                "case {label:?}: a refused create must store nothing"
            );
        }

        // And an update may not empty an endpoint's manifest set either.
        let legal = if container == "ISM" {
            "MssManifests"
        } else {
            "HlsManifests"
        };
        let name = format!("kept-{container}").to_lowercase();
        let (status, created) = emulator
            .create_endpoint(
                "grp",
                "chan",
                &name,
                json!({"ContainerType": container, legal: [{"ManifestName": "index"}]}),
            )
            .await;
        assert_eq!(status, 200, "case \"{container} setup\": got {status} {created}");
        let label = format!("update {container} to no manifest");
        let (status, answer) = emulator
            .update_endpoint(
                "grp",
                "chan",
                &name,
                json!({"ContainerType": container}),
                &[],
            )
            .await;
        assert_validation(&label, status, &answer, "NUM_MANIFESTS_LOW");
        assert_eq!(
            emulator.get_endpoint("grp", "chan", &name).await.1[legal],
            created[legal],
            "case {label:?}: a refused update must leave the manifest set alone"
        );
    }
}

// =============================================================================
// RULE 5 — pagination tokens, on every paged list
// =============================================================================

/// RULE 5 — a token this service did not issue is a modeled 400 on every paged list, and
/// a token it did issue walks the rest of the collection.
#[tokio::test]
async fn a_pagination_token_the_service_did_not_issue_is_a_validation_exception() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );

    // Tokens of several shapes, none of them one this service issued: prose, odd-length
    // hex, and a base64-looking string.
    for token in [
        "not-a-real-pagination-token",
        "zzzz",
        "abc",
        "eyJrZXkiOiAidmFsdWUifQ==",
    ] {
        for (list, (status, body)) in [
            (
                "ListChannelGroups",
                emulator.list_groups(&format!("?nextToken={token}")).await,
            ),
            (
                "ListOriginEndpoints",
                emulator
                    .list_endpoints("grp", "chan", &format!("?nextToken={token}"))
                    .await,
            ),
        ] {
            let label = format!("{list} with token {token:?}");
            assert_validation(&label, status, &body, "INVALID_PAGINATION_TOKEN");
        }
    }

    // A token the service DID issue resumes the list: every group appears exactly once,
    // in name order, no page longer than the requested size.
    let expected: Vec<String> = (0..12).map(|index| format!("g-{index:02}")).collect();
    for name in &expected {
        assert_eq!(
            emulator.create_group(name, json!({})).await.0,
            200,
            "case \"paging setup\": creating {name} must succeed"
        );
    }
    for page_size in [1usize, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let (status, body) = emulator.list_groups(&query).await;
            assert_eq!(
                status, 200,
                "case \"maxResults={page_size}\": expected 200, got {status} {body}"
            );
            let items = body["Items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "case \"maxResults={page_size}\": a page of {} is longer than the page size",
                items.len()
            );
            for item in items {
                seen.push(
                    item["ChannelGroupName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match body["NextToken"].as_str() {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        // "grp" from the setup above sorts after the generated names.
        let mut all = expected.clone();
        all.push("grp".to_string());
        assert_eq!(
            seen, all,
            "case \"maxResults={page_size}\": paging must report every group once, in name order"
        );
    }
}

// =============================================================================
// RULE 6 — every declared-constraint refusal in this service's own form
// =============================================================================

/// RULE 6 — a violation of a constraint the model declares is reported in
/// mediapackagev2's `Validation error encountered at request.<member>: ...` form, with
/// the bound the member actually declares, and the published bounds themselves are
/// accepted.
#[tokio::test]
async fn a_declared_constraint_violation_is_reported_in_this_services_own_form() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );

    // `MaxResults` -> `ListResourceMaxResults`, @range min 1 max 100 (from the model), on
    // both paged lists: each bound, one past each bound, and far past the top.
    for (value, expected_message) in [
        (
            "0",
            Some(
                "Validation error encountered at request.maxResults: Member must have value \
                 greater than or equal to 1. Check the value you specified in your request \
                 and then try again",
            ),
        ),
        ("1", None),
        ("100", None),
        (
            "101",
            Some(
                "Validation error encountered at request.maxResults: Member must have value \
                 less than or equal to 100. Check the value you specified in your request \
                 and then try again",
            ),
        ),
        (
            "1001",
            Some(
                "Validation error encountered at request.maxResults: Member must have value \
                 less than or equal to 100. Check the value you specified in your request \
                 and then try again",
            ),
        ),
    ] {
        for (list, (status, body)) in [
            (
                "ListChannelGroups",
                emulator.list_groups(&format!("?maxResults={value}")).await,
            ),
            (
                "ListOriginEndpoints",
                emulator
                    .list_endpoints("grp", "chan", &format!("?maxResults={value}"))
                    .await,
            ),
        ] {
            let label = format!("{list} with maxResults={value}");
            match expected_message {
                Some(expected) => {
                    assert_eq!(
                        status, 400,
                        "case {label:?}: expected 400, got {status} {body}"
                    );
                    assert_eq!(
                        error_type(&body),
                        "ValidationException",
                        "case {label:?}: expected ValidationException, got {body}"
                    );
                    assert_eq!(
                        message(&body),
                        expected,
                        "case {label:?}: the message must name the member and the bound"
                    );
                }
                None => assert_eq!(
                    status, 200,
                    "case {label:?}: a value on the published bound expected 200, got \
                     {status} {body}"
                ),
            }
        }
    }

    // The same form for the other constraint classes the model declares, so the whole
    // surface reports a refusal one way: a pattern violation, a length violation and an
    // absent required member.
    let long_name = "n".repeat(257);
    let long_description = "d".repeat(1025);
    for (label, (status, body), expected) in [
        (
            "a channel group name outside its @pattern",
            emulator.create_group("has a space", json!({})).await,
            "Validation error encountered at request.channelGroupName: Member must \
             contain only supported characters. Supported characters are A-Z, a-z, 0-9, \
             _ (underscore), and - (hyphen). Check the value you specified in your \
             request and then try again",
        ),
        (
            "a channel group name past its @length maximum",
            emulator.create_group(&long_name, json!({})).await,
            "Validation error encountered at request.channelGroupName: Member must have \
             length less than or equal to 256. Check the value you specified in your \
             request and then try again",
        ),
        (
            "a description past its @length maximum",
            emulator
                .create_group("described", json!({"Description": long_description}))
                .await,
            "Validation error encountered at request.description: Member must have length \
             less than or equal to 1024. Check the value you specified in your request and \
             then try again",
        ),
        (
            "an origin endpoint with no ContainerType",
            emulator
                .create_endpoint("grp", "chan", "no-container", json!({}))
                .await,
            "Validation error encountered at request.containerType: Member must not be \
             null. Check the value you specified in your request and then try again",
        ),
    ] {
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {label:?}: the message must be this service's own form"
        );
    }
}

// =============================================================================
// RULE 7 — immutable members, and what a successful update does
// =============================================================================

/// RULE 7 — a member the model fixes at creation is refused when an update changes it,
/// and the endpoint (ETag included) is left alone.
#[tokio::test]
async fn an_immutable_origin_endpoint_member_cannot_be_changed_by_an_update() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );
    let (status, created) = emulator
        .create_endpoint(
            "grp",
            "chan",
            "ep",
            json!({
                "ContainerType": "TS",
                "Description": "as created",
                "HlsManifests": [{"ManifestName": "index"}],
                "StreamNameOutputMode": "INDEX",
            }),
        )
        .await;
    assert_eq!(status, 200, "case \"setup\": got {status} {created}");

    // One row per immutable member, each tried with every OTHER value its enum declares.
    let rows = [
        (
            "containerType",
            "CONTAINER_TYPE_IMMUTABLE",
            vec![
                json!({"ContainerType": "CMAF", "HlsManifests": [{"ManifestName": "index"}]}),
                json!({"ContainerType": "ISM", "MssManifests": [{"ManifestName": "index"}]}),
            ],
        ),
        (
            "streamNameOutputMode",
            "STREAM_NAME_OUTPUT_MODE_IMMUTABLE",
            vec![json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "StreamNameOutputMode": "PASSTHROUGH_NAME",
            })],
        ),
    ];
    for (member, expected_type, bodies) in rows {
        for body in bodies {
            let label = format!("update changing {member}: {body}");
            let (status, answer) = emulator
                .update_endpoint("grp", "chan", "ep", body, &[])
                .await;
            assert_validation(&label, status, &answer, expected_type);
            let (_, read) = emulator.get_endpoint("grp", "chan", "ep").await;
            for kept in ["ContainerType", "StreamNameOutputMode", "ETag", "Description"] {
                assert_eq!(
                    read[kept], created[kept],
                    "case {label:?}: a refused update must leave {kept} as it was"
                );
            }
        }
    }

    // An update that keeps every immutable member and changes something else succeeds,
    // reports the change, and rotates the ETag — and the read agrees with the update.
    let (status, updated) = emulator
        .update_endpoint(
            "grp",
            "chan",
            "ep",
            json!({
                "ContainerType": "TS",
                "Description": "after the update",
                "HlsManifests": [{"ManifestName": "index"}, {"ManifestName": "extra"}],
                "StreamNameOutputMode": "INDEX",
            }),
            &[],
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an update that changes only the packaging\": expected 200, got {status} {updated}"
    );
    assert_eq!(
        updated["Description"], json!("after the update"),
        "case \"an update that changes only the packaging\": the new description must be reported"
    );
    assert_eq!(
        updated["ContainerType"], created["ContainerType"],
        "case \"an update that changes only the packaging\": the container type is unchanged"
    );
    assert_eq!(
        updated["CreatedAt"], created["CreatedAt"],
        "case \"an update that changes only the packaging\": the creation instant is unchanged"
    );
    assert_ne!(
        updated["ETag"], created["ETag"],
        "case \"an update that changes only the packaging\": the ETag must rotate"
    );
    assert_eq!(
        updated["HlsManifests"].as_array().map(Vec::len),
        Some(2),
        "case \"an update that changes only the packaging\": the new manifest set must be \
         reported, got {updated}"
    );
    let (_, read) = emulator.get_endpoint("grp", "chan", "ep").await;
    for member in [
        "Arn",
        "ContainerType",
        "Description",
        "HlsManifests",
        "CreatedAt",
        "ModifiedAt",
        "ETag",
    ] {
        assert_eq!(
            read[member], updated[member],
            "case \"an update that changes only the packaging\": GetOriginEndpoint {member} \
             must equal what the update stored"
        );
    }

    // The `x-amzn-update-if-match` precondition: the endpoint's current ETag is accepted,
    // a stale one is refused and changes nothing.
    let current = read["ETag"].as_str().unwrap_or_default().to_string();
    let body = json!({
        "ContainerType": "TS",
        "Description": "conditional",
        "HlsManifests": [{"ManifestName": "index"}],
        "StreamNameOutputMode": "INDEX",
    });
    let (status, refused) = emulator
        .update_endpoint(
            "grp",
            "chan",
            "ep",
            body.clone(),
            &[("x-amzn-update-if-match", "not-the-current-etag")],
        )
        .await;
    assert_eq!(
        status, 409,
        "case \"a stale ETag precondition\": expected 409, got {status} {refused}"
    );
    assert_eq!(
        emulator.get_endpoint("grp", "chan", "ep").await.1["ETag"],
        json!(current),
        "case \"a stale ETag precondition\": a refused update must leave the ETag alone"
    );
    let (status, accepted) = emulator
        .update_endpoint("grp", "chan", "ep", body, &[("x-amzn-update-if-match", &current)])
        .await;
    assert_eq!(
        status, 200,
        "case \"the current ETag precondition\": expected 200, got {status} {accepted}"
    );
}

/// RULE 7 (same rule, the case the create left unnamed) — the mode is immutable from
/// creation, because the service applies one whether or not the create named it: the
/// model states "If you don't specify a value, the default is INDEX" and "You can't
/// change the stream name output mode after you create the endpoint". So an update
/// repeating the applied mode is accepted and one naming the other mode is refused.
#[tokio::test]
async fn the_immutable_mode_the_create_omitted_is_the_service_applied_one() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );
    let (status, created) = emulator
        .create_endpoint("grp", "chan", "ep", minimal_endpoint("TS"))
        .await;
    assert_eq!(status, 200, "case \"setup\": got {status} {created}");
    assert_eq!(
        created["StreamNameOutputMode"],
        json!("INDEX"),
        "case \"setup\": the create must report the mode the service applied, got {created}"
    );

    // Naming the applied mode is no change, so the update is accepted.
    let body = json!({
        "ContainerType": "TS",
        "HlsManifests": [{"ManifestName": "index"}],
        "StreamNameOutputMode": "INDEX",
    });
    let (status, updated) = emulator
        .update_endpoint("grp", "chan", "ep", body.clone(), &[])
        .await;
    assert_eq!(
        status, 200,
        "case \"an update repeating the applied mode\": expected 200, got {status} {updated}"
    );
    assert_eq!(
        updated["StreamNameOutputMode"],
        json!("INDEX"),
        "case \"an update repeating the applied mode\": the mode must be kept and reported"
    );

    // Naming the other mode is a change to an immutable member.
    let (status, refused) = emulator
        .update_endpoint(
            "grp",
            "chan",
            "ep",
            json!({
                "ContainerType": "TS",
                "HlsManifests": [{"ManifestName": "index"}],
                "StreamNameOutputMode": "PASSTHROUGH_NAME",
            }),
            &[],
        )
        .await;
    assert_validation(
        "an update changing the applied mode",
        status,
        &refused,
        "STREAM_NAME_OUTPUT_MODE_IMMUTABLE",
    );
    assert_eq!(
        emulator.get_endpoint("grp", "chan", "ep").await.1["StreamNameOutputMode"],
        json!("INDEX"),
        "case \"an update changing the applied mode\": the stored mode must be kept"
    );
}

/// An update REPLACES the whole manifest set, so it is bounded by the same range the
/// create is: at the ceiling it is accepted, one past it is refused as an input failure
/// (`NUM_MANIFESTS_HIGH`) and the stored set is untouched.
#[tokio::test]
async fn an_update_cannot_push_an_endpoint_past_the_manifest_ceiling() {
    // The emulator's own ceiling, from crate::state::MAX_MANIFESTS_PER_ORIGIN_ENDPOINT.
    const MAX_MANIFESTS: usize = 100;

    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );
    let (status, created) = emulator
        .create_endpoint("grp", "chan", "ep", minimal_endpoint("TS"))
        .await;
    assert_eq!(status, 200, "case \"setup\": got {status} {created}");

    let manifests = |count: usize| -> Value {
        (0..count)
            .map(|index| json!({"ManifestName": format!("m-{index}")}))
            .collect()
    };
    for (label, count, accepted) in [
        ("at the ceiling", MAX_MANIFESTS, true),
        ("one past the ceiling", MAX_MANIFESTS + 1, false),
    ] {
        let (status, body) = emulator
            .update_endpoint(
                "grp",
                "chan",
                "ep",
                json!({"ContainerType": "TS", "HlsManifests": manifests(count)}),
                &[],
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {body}"
            );
        } else {
            assert_validation(label, status, &body, "NUM_MANIFESTS_HIGH");
            assert_eq!(
                emulator.get_endpoint("grp", "chan", "ep").await.1["HlsManifests"]
                    .as_array()
                    .map(Vec::len),
                Some(MAX_MANIFESTS),
                "case {label:?}: a refused update must leave the stored manifest set alone"
            );
        }
    }
}

/// An operation addressing an origin endpoint whose parent chain does not resolve is a
/// 404 — the update path included, which answered 501 while it was a stub.
#[tokio::test]
async fn an_update_of_a_resource_that_does_not_resolve_is_not_found() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_group("grp", json!({})).await.0, 200);
    assert_eq!(
        emulator.create_channel("grp", "chan", json!({})).await.0,
        200
    );
    assert_eq!(
        emulator
            .create_endpoint("grp", "chan", "ep", minimal_endpoint("TS"))
            .await
            .0,
        200
    );

    for (label, group, channel, endpoint) in [
        ("a missing channel group", "ghost", "chan", "ep"),
        ("a missing channel", "grp", "ghost", "ep"),
        ("a missing origin endpoint", "grp", "chan", "ghost"),
    ] {
        let (status, body) = emulator
            .update_endpoint(group, channel, endpoint, minimal_endpoint("TS"), &[])
            .await;
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
}
