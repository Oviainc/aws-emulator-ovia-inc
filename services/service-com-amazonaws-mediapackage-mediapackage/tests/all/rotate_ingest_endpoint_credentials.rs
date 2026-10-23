// SPDX-License-Identifier: CC0-1.0

//! `RotateIngestEndpointCredentials` rotates the credentials of the ONE ingest endpoint
//! the request addresses, and changes nothing else.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//! The client, the region and the account label are the ones
//! `tests/all/channel_lifecycle.rs` defines, so every suite in this target addresses the
//! same emulator in the same way.
//!
//! The rule under test: a rotation on one ingest endpoint (`PUT
//! /channels/{Id}/ingest_endpoints/{IngestEndpointId}/credentials`, documented as "Rotate
//! the IngestEndpoint's username and password, as specified by the IngestEndpoint's id")
//! replaces the `Username` and `Password` of the endpoint whose `Id` the request names —
//! whichever position it occupies — with freshly minted system-generated values, never the
//! pair they replace, and leaves everything else as stored: that endpoint's `Id` and `Url`,
//! every OTHER ingest endpoint of the channel, the channel's own members (id, ARN,
//! `CreatedAt`, description, log subscriptions, tags) and every other channel. The rotation
//! is persisted, so the operation's own response and the next `DescribeChannel` render the
//! same record; a rotation naming a channel, or an ingest endpoint id, the service does not
//! hold writes nothing and is `NotFoundException` (404).
//!
//! The generated shape — 32 lowercase-hex characters for each of `Id`, `Username` and
//! `Password` — is the one `tests/all/ingest_credentials.rs` pins for a freshly created
//! channel, from recorded MediaPackage responses; the Smithy model puts no `length` or
//! `pattern` constraint on those members
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#IngestEndpoint")`). No
//! literal credential value is pinned anywhere below: a credential is system-generated, so
//! only its shape and its freshness are the contract.
//!
//! The rows differ along what the rule generalises over: the channel id's length and
//! character set, whether the channel carries a description, tags or a log subscription,
//! WHICH of the channel's endpoints is targeted (first, last, every one in turn) and how
//! many times in a row the same endpoint is rotated.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, emulator, error_code};

/// Characters in each system-generated ingest value, as `tests/all/ingest_credentials.rs`
/// states it from recorded MediaPackage responses.
const GENERATED_HEX_CHARS: usize = 32;

/// `true` when `text` is exactly [`GENERATED_HEX_CHARS`] lowercase-hex characters.
fn is_generated_token(text: &str) -> bool {
    text.chars().count() == GENERATED_HEX_CHARS
        && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// One case: a name, the `CreateChannel` body, an optional `ConfigureLogs` body applied
/// before the first rotation, and how many rotations in a row to perform on each endpoint.
struct Case {
    name: &'static str,
    create: Value,
    logs: Option<Value>,
    rotations: usize,
}

/// The channel id a case's `CreateChannel` body names.
fn case_id(case: &Case) -> String {
    case.create
        .get("id")
        .and_then(Value::as_str)
        .expect("every case creates a channel by id")
        .to_string()
}

/// The table. Rows differ along the channel id's length (one character up to 64) and form
/// (hyphens, underscores, digits, mixed case), whether the channel carries a description,
/// tags or a log subscription, and the number of consecutive rotations — none of which the
/// rotation's effect may depend on.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "single-character id, nothing else set, one rotation",
            create: json!({ "id": "c" }),
            logs: None,
            rotations: 1,
        },
        Case {
            name: "described channel, each endpoint rotated twice in a row",
            create: json!({ "id": "mp-channel", "description": "two endpoints" }),
            logs: None,
            rotations: 2,
        },
        Case {
            name: "mixed-case id with tags and both log subscriptions",
            create: json!({ "id": "Mixed_Case_99", "tags": { "team": "video" } }),
            logs: Some(json!({
                "egressAccessLogs": { "logGroupName": "/aws/MediaPackage/egress" },
                "ingressAccessLogs": { "logGroupName": "/aws/MediaPackage/ingress" }
            })),
            rotations: 1,
        },
        Case {
            name: "recorded-style probe id, each endpoint rotated three times in a row",
            create: json!({ "id": "sim2real-probe-mediapackage-mp-channel-818202c7" }),
            logs: None,
            rotations: 3,
        },
        Case {
            name: "maximum-length id",
            create: json!({ "id": "x".repeat(64) }),
            logs: None,
            rotations: 1,
        },
    ]
}

/// The `hlsIngest.ingestEndpoints` array of a channel projection.
fn endpoints_of(channel: &Value) -> Vec<Value> {
    channel
        .pointer("/hlsIngest/ingestEndpoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A string member of an ingest endpoint.
fn endpoint_member(endpoint: &Value, member: &str) -> String {
    endpoint
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every ingest endpoint id of a channel projection, in order.
fn endpoint_ids(channel: &Value) -> Vec<String> {
    endpoints_of(channel)
        .iter()
        .map(|endpoint| endpoint_member(endpoint, "id"))
        .collect()
}

/// Rotate `endpoint_id` on `channel_id`: the HTTP status and the parsed body.
async fn rotate(
    router: &MultiServiceRouter,
    channel_id: &str,
    endpoint_id: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/channels/{channel_id}/ingest_endpoints/{endpoint_id}/credentials"),
        None,
    )
    .await
}

/// Create the case's channel (and its log subscription, when the row names one), and
/// answer the channel as stored.
async fn provision(router: &MultiServiceRouter, case: &Case) -> Value {
    let id = case_id(case);
    let (status, mut created) = call(router, "POST", "/channels", Some(&case.create)).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateChannel expected 200, got {status} {created:?}",
        case.name
    );
    if let Some(logs) = &case.logs {
        let (status, configured) = call(
            router,
            "PUT",
            &format!("/channels/{id}/configure_logs"),
            Some(logs),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: ConfigureLogs expected 200, got {status} {configured:?}",
            case.name
        );
        created = configured;
    }
    created
}

/// Every channel member a rotation leaves alone, compared between the projection before
/// the rotation and the one after it.
fn assert_unrotated_members_preserved(case: &str, before: &Value, after: &Value) {
    for key in [
        "id",
        "arn",
        "createdAt",
        "description",
        "egressAccessLogs",
        "ingressAccessLogs",
        "tags",
    ] {
        assert_eq!(
            after.get(key),
            before.get(key),
            "case {case:?}: {key} must be the value the channel already carried, \
             got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_rotation_remints_only_the_addressed_endpoints_credentials_and_persists_them() {
    let router = emulator();
    // Every credential value this suite has seen, so no rotation may hand back a value
    // any endpoint of any channel ever carried.
    let mut seen: Vec<(String, String)> = Vec::new();

    for case in cases() {
        let id = case_id(&case);
        let mut before = provision(&router, &case).await;
        let targets = endpoint_ids(&before);
        assert!(
            targets.len() > 1,
            "case {:?}: the channel must carry several ingest endpoints for \"only the \
             addressed one\" to mean anything, got {before:?}",
            case.name
        );

        for endpoint in endpoints_of(&before) {
            for member in ["username", "password"] {
                seen.push((
                    format!("{} provisioned {member}", case.name),
                    endpoint_member(&endpoint, member),
                ));
            }
        }

        // Every endpoint of the channel is targeted in turn, so the rule cannot hold by
        // the target's position: the first, the last, and any in between.
        for (target_index, target) in targets.iter().enumerate() {
            for round in 1..=case.rotations {
                let label = format!(
                    "{} endpoint {target_index} ({target}) rotation {round}",
                    case.name
                );
                let (status, rotated) = rotate(&router, &id, target).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: RotateIngestEndpointCredentials expected 200, got \
                     {status} {rotated:?}"
                );

                let old_endpoints = endpoints_of(&before);
                let new_endpoints = endpoints_of(&rotated);
                assert_eq!(
                    new_endpoints.len(),
                    old_endpoints.len(),
                    "case {label:?}: a rotation must not add or drop an ingest endpoint, \
                     got {rotated:?}"
                );
                assert_eq!(
                    endpoint_ids(&rotated),
                    targets,
                    "case {label:?}: a rotation must not change or reorder the ingest \
                     endpoint ids, got {rotated:?}"
                );

                // The addressed endpoint: new credentials of the generated shape, same
                // identity.
                let old_target = &old_endpoints[target_index];
                let new_target = &new_endpoints[target_index];
                for member in ["id", "url"] {
                    assert_eq!(
                        new_target.get(member),
                        old_target.get(member),
                        "case {label:?}: the rotated endpoint keeps its {member}, \
                         got {new_target:?}"
                    );
                }
                let username = endpoint_member(new_target, "username");
                let password = endpoint_member(new_target, "password");
                for (member, value) in [("username", &username), ("password", &password)] {
                    assert!(
                        is_generated_token(value),
                        "case {label:?}: the rotated {member} must be {GENERATED_HEX_CHARS} \
                         lowercase-hex characters, got {value:?} ({} characters)",
                        value.chars().count()
                    );
                    assert_ne!(
                        value.as_str(),
                        endpoint_member(old_target, member).as_str(),
                        "case {label:?}: the rotated {member} must differ from the one it \
                         replaced, got {value:?}"
                    );
                    for (seen_case, seen_value) in &seen {
                        assert_ne!(
                            seen_value, value,
                            "case {label:?}: the rotated {member} must be freshly minted, \
                             got {value:?} which {seen_case} already carries"
                        );
                    }
                }
                assert_ne!(
                    username, password,
                    "case {label:?}: a rotation mints two distinct values, both are \
                     {username:?}"
                );
                seen.push((format!("{label} username"), username));
                seen.push((format!("{label} password"), password));

                // Every endpoint the request did NOT address, and every other channel
                // member, as stored.
                for (index, (old, new)) in old_endpoints.iter().zip(&new_endpoints).enumerate() {
                    if index == target_index {
                        continue;
                    }
                    assert_eq!(
                        new, old,
                        "case {label:?}: endpoint {index} was not addressed, so it must be \
                         untouched, got {new:?}"
                    );
                }
                assert_unrotated_members_preserved(&label, &before, &rotated);

                // Persisted: the next read renders the record the rotation wrote.
                let (status, described) =
                    call(&router, "GET", &format!("/channels/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: DescribeChannel expected 200, got {status} {described:?}"
                );
                assert_eq!(
                    described, rotated,
                    "case {label:?}: DescribeChannel must render the rotated record, \
                     got {described:?}"
                );

                before = rotated;
            }
        }
    }
}

#[tokio::test]
async fn a_rotation_leaves_every_other_channel_untouched() {
    let router = emulator();
    let mut provisioned: Vec<(String, Value)> = Vec::new();
    for case in cases() {
        let channel = provision(&router, &case).await;
        provisioned.push((case_id(&case), channel));
    }

    // Rotating one endpoint of each channel in turn: every channel but the rotated one
    // keeps the ingest endpoints it was provisioned with.
    for index in 0..provisioned.len() {
        let (id, channel) = provisioned[index].clone();
        // The LAST endpoint, so the row differs from the one `RotateChannelCredentials`
        // (which always rotates the first) exercises.
        let target = endpoint_ids(&channel)
            .pop()
            .expect("a provisioned channel carries ingest endpoints");
        let (status, rotated) = rotate(&router, &id, &target).await;
        assert_eq!(
            status, 200,
            "case {id:?}: RotateIngestEndpointCredentials expected 200, got {status} \
             {rotated:?}"
        );
        provisioned[index].1 = rotated;

        for (other_id, expected) in &provisioned {
            let (status, described) =
                call(&router, "GET", &format!("/channels/{other_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {other_id:?}: DescribeChannel expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described.get("hlsIngest"),
                expected.get("hlsIngest"),
                "case {other_id:?}: rotating {id:?} must not change this channel's ingest \
                 endpoints, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn rotating_a_target_the_service_does_not_hold_is_not_found_and_writes_nothing() {
    let router = emulator();
    let held = "held-channel";
    let (status, created) = call(&router, "POST", "/channels", Some(&json!({ "id": held }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel expected 200, got {status} {created:?}"
    );
    let held_endpoints = endpoint_ids(&created);
    let held_endpoint = held_endpoints
        .first()
        .expect("a created channel carries ingest endpoints")
        .clone();

    // A second channel, so an endpoint id that is real but belongs to ANOTHER channel can
    // be addressed: an ingest endpoint exists only as part of its own channel.
    let (status, other) = call(&router, "POST", "/channels", Some(&json!({ "id": "other" }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel expected 200, got {status} {other:?}"
    );
    let foreign_endpoint = endpoint_ids(&other)
        .pop()
        .expect("a created channel carries ingest endpoints");

    // A deleted channel, so "never existed" and "no longer exists" are both covered.
    let (status, deleted) = call(&router, "POST", "/channels", Some(&json!({ "id": "gone" }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel expected 200, got {status} {deleted:?}"
    );
    let gone_endpoint = endpoint_ids(&deleted)
        .pop()
        .expect("a created channel carries ingest endpoints");
    let (status, body) = call(&router, "DELETE", "/channels/gone", None).await;
    assert_eq!(
        status, 202,
        "DeleteChannel expected its modelled 202, got {status} {body:?}"
    );

    // Rows differ along WHICH label does not resolve, and along the shape of the
    // unresolvable value: an absent channel, a deleted channel, a channel differing only in
    // case, an absent endpoint id, an endpoint id of the wrong form, and a real endpoint id
    // belonging to a different channel.
    let unresolvable: Vec<(String, String)> = vec![
        ("never-created".to_string(), held_endpoint.clone()),
        ("gone".to_string(), gone_endpoint),
        ("HELD-CHANNEL".to_string(), held_endpoint.clone()),
        (held.to_string(), "never-minted".to_string()),
        (held.to_string(), held_endpoint.to_uppercase()),
        (held.to_string(), "0".repeat(GENERATED_HEX_CHARS)),
        (held.to_string(), foreign_endpoint),
    ];

    for (channel_id, endpoint_id) in unresolvable {
        let case = format!("{channel_id}/{endpoint_id}");
        let (status, body) = rotate(&router, &channel_id, &endpoint_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: rotating an absent target expected 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );
    }

    // Nothing was written: the held channel still carries the credentials it was
    // provisioned with, and the absent channel is still absent.
    let (status, described) = call(&router, "GET", &format!("/channels/{held}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeChannel expected 200, got {status} {described:?}"
    );
    assert_eq!(
        described.get("hlsIngest"),
        created.get("hlsIngest"),
        "a refused rotation must not touch any channel's credentials, got {described:?}"
    );
    let (status, described) = call(&router, "GET", "/channels/never-created", None).await;
    assert_eq!(
        status, 404,
        "a refused rotation must not create the channel it named, got {status} {described:?}"
    );
}

#[tokio::test]
async fn rotating_one_endpoint_does_not_rotate_the_channels_other_endpoint() {
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/channels",
        Some(&json!({ "id": "two-endpoints" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateChannel expected 200, got {status} {created:?}"
    );

    // Each endpoint, rotated alone: the credentials of the other one are the provisioned
    // pair both times, and the two rotations between them change every endpoint exactly
    // once.
    let ids = endpoint_ids(&created);
    let mut current = created.clone();
    for (index, target) in ids.iter().enumerate() {
        let (status, rotated) = rotate(&router, "two-endpoints", target).await;
        assert_eq!(
            status, 200,
            "case {target:?}: RotateIngestEndpointCredentials expected 200, got {status} \
             {rotated:?}"
        );
        let before = endpoints_of(&current);
        let after = endpoints_of(&rotated);
        for (position, (old, new)) in before.iter().zip(&after).enumerate() {
            for member in ["username", "password"] {
                let changed = endpoint_member(old, member) != endpoint_member(new, member);
                assert_eq!(
                    changed,
                    position == index,
                    "case {target:?}: endpoint {position} {member} changed={changed}, but \
                     only the addressed endpoint {index} may change, got {new:?}"
                );
            }
        }
        current = rotated;
    }

    // Both endpoints now differ from what they were provisioned with, and both are still
    // generated tokens.
    for (position, (old, new)) in endpoints_of(&created)
        .iter()
        .zip(endpoints_of(&current))
        .enumerate()
    {
        for member in ["username", "password"] {
            let value = endpoint_member(&new, member);
            assert!(
                is_generated_token(&value),
                "case endpoint {position}: {member} must be {GENERATED_HEX_CHARS} \
                 lowercase-hex characters, got {value:?}"
            );
            assert_ne!(
                value,
                endpoint_member(old, member),
                "case endpoint {position}: {member} must have been rotated, got {value:?}"
            );
        }
    }
}
