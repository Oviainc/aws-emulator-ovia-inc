// SPDX-License-Identifier: CC0-1.0

//! Every member of an OriginEndpoint that is a DURATION IN SECONDS is a non-negative
//! count of seconds.
//!
//! The rule under test: a member whose value is a number of seconds of media — a window
//! of content to retain or publish, a segment or buffer length, a delay, an interval
//! between key rotations or date-time tags — cannot carry a negative value, so a
//! `CreateOriginEndpoint` or `UpdateOriginEndpoint` request carrying one is refused with
//! `UnprocessableEntityException` (HTTP 422) keyed by that member, and persists nothing:
//! the create stores no endpoint and the update leaves the stored one exactly as it was.
//! Zero is a real value for every one of them (`StartoverWindowSeconds` 0 is startover
//! playback disabled), so the shared floor is zero, and a value at or above it is stored
//! and read back. One member of the set, `StartoverWindowSeconds`, has a published minimum
//! of its OWN above that floor, so it is judged against that minimum here and
//! `tests/all/startover_window_bound.rs` is the suite for its whole accepted domain.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` — the same dispatch the
//! served emulator uses — so what is asserted is the wire behaviour, not a handler call.
//! The client, the region and the helpers are the ones `tests/all/channel_lifecycle.rs`
//! defines.
//!
//! The [`MEMBERS`] catalog below is the COMPLETE set of such members on this operation's
//! input, from
//! `smithy_query(query="shape",shape="com.amazonaws.mediapackage#CreateOriginEndpointRequest,com.amazonaws.mediapackage#HlsPackage,com.amazonaws.mediapackage#DashPackage,com.amazonaws.mediapackage#CmafPackageCreateOrUpdateParameters,com.amazonaws.mediapackage#MssPackage,com.amazonaws.mediapackage#HlsEncryption,com.amazonaws.mediapackage#DashEncryption,com.amazonaws.mediapackage#CmafEncryption,com.amazonaws.mediapackage#HlsManifestCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`:
//! every member of the input or of a packaging configuration it carries whose name ends
//! in `Seconds` and whose target is `__integer`. The issue cited only
//! `StartoverWindowSeconds`; the rows below span the rest, so the rule is tested where it
//! holds rather than where it was reported.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the
//! duration (top-level, each of the four packaging formats, a packaging format's
//! encryption, a CMAF child manifest), how deeply it is nested, the magnitude and sign of
//! the value (-1, -100, the smallest `__integer`, 0 and a positive one), and which
//! operation carries it (create or update).

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;

use crate::channel_lifecycle::{call, emulator, error_code};

/// The floor the member at `path` must meet, taken from the service's own constants rather
/// than restated here: every duration-in-seconds member shares
/// `crate::state::MIN_DURATION_SECONDS`, except `startoverWindowSeconds`, whose ENABLED
/// window has a published minimum of its own — `tests/all/startover_window_bound.rs` is the
/// suite for that member's whole accepted domain.
fn floor(path: &str) -> i32 {
    if path == state::STARTOVER_WINDOW_MEMBER {
        state::MIN_ENABLED_STARTOVER_WINDOW_SECONDS
    } else {
        state::MIN_DURATION_SECONDS
    }
}

/// The requirement a refused duration states, as this service words every violated
/// numeric floor: the member's own floor, in one sentence.
fn requirement(path: &str) -> String {
    format!("must be greater than or equal to {}", floor(path))
}

/// A SPEKE key provider, which every packaging format's `Encryption` requires
/// (`SpekeKeyProvider$ResourceId`, `$RoleArn`, `$SystemIds` and `$Url` are all required),
/// so an encryption row reaches the handler instead of being refused for a missing member.
fn speke() -> Value {
    json!({
        "resourceId": "endpoint-durations",
        "roleArn": "arn:aws:iam::000000000000:role/mediapackage-speke",
        "systemIds": ["81376844-f976-481e-a84e-cc25d39b0b33"],
        "url": "https://speke.example.com/v1/copyProtection"
    })
}

/// One duration-in-seconds member: the path the wire names it by, and the request body
/// that carries `seconds` in it.
///
/// The body always names exactly ONE packaging configuration, because an OriginEndpoint
/// packages its channel in exactly one format, and exactly one duration, so the member a
/// refusal is keyed by is never ambiguous.
struct Member {
    path: &'static str,
    body: fn(i32) -> Value,
}

/// The catalog. One row per duration-in-seconds member of `CreateOriginEndpoint`'s input
/// and of the packaging configurations it carries — see this module's docs for the
/// `smithy_query` the list comes from.
const MEMBERS: &[Member] = &[
    Member {
        path: "startoverWindowSeconds",
        body: |seconds| json!({ "hlsPackage": {}, "startoverWindowSeconds": seconds }),
    },
    Member {
        path: "timeDelaySeconds",
        body: |seconds| json!({ "hlsPackage": {}, "timeDelaySeconds": seconds }),
    },
    Member {
        path: "hlsPackage.playlistWindowSeconds",
        body: |seconds| json!({ "hlsPackage": { "playlistWindowSeconds": seconds } }),
    },
    Member {
        path: "hlsPackage.programDateTimeIntervalSeconds",
        body: |seconds| json!({ "hlsPackage": { "programDateTimeIntervalSeconds": seconds } }),
    },
    Member {
        path: "hlsPackage.segmentDurationSeconds",
        body: |seconds| json!({ "hlsPackage": { "segmentDurationSeconds": seconds } }),
    },
    Member {
        path: "hlsPackage.encryption.keyRotationIntervalSeconds",
        body: |seconds| {
            json!({
                "hlsPackage": {
                    "encryption": {
                        "keyRotationIntervalSeconds": seconds,
                        "spekeKeyProvider": speke()
                    }
                }
            })
        },
    },
    Member {
        path: "dashPackage.manifestWindowSeconds",
        body: |seconds| json!({ "dashPackage": { "manifestWindowSeconds": seconds } }),
    },
    Member {
        path: "dashPackage.minBufferTimeSeconds",
        body: |seconds| json!({ "dashPackage": { "minBufferTimeSeconds": seconds } }),
    },
    Member {
        path: "dashPackage.minUpdatePeriodSeconds",
        body: |seconds| json!({ "dashPackage": { "minUpdatePeriodSeconds": seconds } }),
    },
    Member {
        path: "dashPackage.segmentDurationSeconds",
        body: |seconds| json!({ "dashPackage": { "segmentDurationSeconds": seconds } }),
    },
    Member {
        path: "dashPackage.suggestedPresentationDelaySeconds",
        body: |seconds| json!({ "dashPackage": { "suggestedPresentationDelaySeconds": seconds } }),
    },
    Member {
        path: "dashPackage.encryption.keyRotationIntervalSeconds",
        body: |seconds| {
            json!({
                "dashPackage": {
                    "encryption": {
                        "keyRotationIntervalSeconds": seconds,
                        "spekeKeyProvider": speke()
                    }
                }
            })
        },
    },
    Member {
        path: "cmafPackage.segmentDurationSeconds",
        body: |seconds| json!({ "cmafPackage": { "segmentDurationSeconds": seconds } }),
    },
    Member {
        path: "cmafPackage.encryption.keyRotationIntervalSeconds",
        body: |seconds| {
            json!({
                "cmafPackage": {
                    "encryption": {
                        "keyRotationIntervalSeconds": seconds,
                        "spekeKeyProvider": speke()
                    }
                }
            })
        },
    },
    Member {
        path: "cmafPackage.hlsManifests[0].playlistWindowSeconds",
        body: |seconds| {
            json!({
                "cmafPackage": {
                    "hlsManifests": [{ "id": "child", "playlistWindowSeconds": seconds }]
                }
            })
        },
    },
    Member {
        path: "cmafPackage.hlsManifests[0].programDateTimeIntervalSeconds",
        body: |seconds| {
            json!({
                "cmafPackage": {
                    "hlsManifests": [{
                        "id": "child",
                        "programDateTimeIntervalSeconds": seconds
                    }]
                }
            })
        },
    },
    Member {
        path: "mssPackage.manifestWindowSeconds",
        body: |seconds| json!({ "mssPackage": { "manifestWindowSeconds": seconds } }),
    },
    Member {
        path: "mssPackage.segmentDurationSeconds",
        body: |seconds| json!({ "mssPackage": { "segmentDurationSeconds": seconds } }),
    },
];

/// Negative values every member must refuse, whatever its floor: the smallest step below
/// zero, the value the issue reported, and the smallest `__integer` there is.
const REFUSED: &[i32] = &[-1, -100, i32::MIN];

/// Values every member must store: zero (a real value for each of them — for
/// `startoverWindowSeconds` it is startover playback disabled) and a positive one above
/// every member's floor.
const ACCEPTED: &[i32] = &[0, 86_400];

/// The JSON pointer that reads a member back out of an endpoint projection, derived from
/// the member path so the two cannot drift apart.
fn pointer(path: &str) -> String {
    format!("/{}", path.replace("[0]", ".0").replace('.', "/"))
}

/// A request body for `member` carrying `seconds`, plus the ids a create needs.
fn create_body(member: &Member, id: &str, channel_id: &str, seconds: i32) -> Value {
    let mut body = (member.body)(seconds);
    let object = body
        .as_object_mut()
        .expect("every row's body is a JSON object");
    object.insert("id".to_string(), json!(id));
    object.insert("channelId".to_string(), json!(channel_id));
    body
}

/// A channel for the endpoints below to package, created once per suite.
async fn channel(router: &smithy_server_runtime::MultiServiceRouter, id: &str) {
    let (status, body) = call(router, "POST", "/channels", Some(&json!({ "id": id }))).await;
    assert_eq!(
        status, 200,
        "CreateChannel {id:?} expected 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_negative_duration_is_refused_on_create_and_stores_no_endpoint() {
    let router = emulator();
    channel(&router, "durations").await;

    for member in MEMBERS {
        for seconds in REFUSED {
            let case = format!("create {} = {seconds}", member.path);
            let id = format!(
                "refused-{}-{seconds}",
                member.path.replace(['.', '[', ']'], "-")
            );
            let body = create_body(member, &id, "durations", *seconds);
            let (status, refusal) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
            assert_eq!(
                status, 422,
                "case {case:?}: expected 422, got {status} {refusal:?}"
            );
            assert_eq!(
                error_code(&refusal),
                "UnprocessableEntityException",
                "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
            );
            assert_eq!(
                refusal.pointer(&format!("/errors/{}", member.path)),
                Some(&json!(requirement(member.path))),
                "case {case:?}: the refusal must be keyed by {:?} and state the floor, \
                 got {refusal:?}",
                member.path
            );

            // Nothing was persisted: the endpoint the refused create named is unreadable.
            let (status, described) =
                call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: the refused create must store no endpoint, \
                 got {status} {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_duration_at_or_above_zero_is_stored_and_read_back() {
    let router = emulator();
    channel(&router, "durations").await;

    for member in MEMBERS {
        for seconds in ACCEPTED {
            let case = format!("create {} = {seconds}", member.path);
            let id = format!(
                "stored-{}-{seconds}",
                member.path.replace(['.', '[', ']'], "-")
            );
            let body = create_body(member, &id, "durations", *seconds);
            let (status, created) = call(&router, "POST", "/origin_endpoints", Some(&body)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {created:?}"
            );
            let read = pointer(member.path);
            assert_eq!(
                created.pointer(&read),
                Some(&json!(seconds)),
                "case {case:?}: the create response must carry the value at {read:?}, \
                 got {created:?}"
            );

            // Persisted: the later read renders the same record.
            let (status, described) =
                call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: the read must render the stored record, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_negative_duration_is_refused_on_update_and_changes_nothing() {
    let router = emulator();
    channel(&router, "durations").await;

    // One endpoint per member, so a refused update is compared against a record that
    // nothing else touched.
    for member in MEMBERS {
        let id = format!("updated-{}", member.path.replace(['.', '[', ']'], "-"));
        let (status, created) = call(
            &router,
            "POST",
            "/origin_endpoints",
            Some(&create_body(member, &id, "durations", ACCEPTED[1])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {id:?}: CreateOriginEndpoint expected 200, got {status} {created:?}"
        );

        for seconds in REFUSED {
            let case = format!("update {} = {seconds}", member.path);
            let (status, refusal) = call(
                &router,
                "PUT",
                &format!("/origin_endpoints/{id}"),
                Some(&(member.body)(*seconds)),
            )
            .await;
            assert_eq!(
                status, 422,
                "case {case:?}: expected 422, got {status} {refusal:?}"
            );
            assert_eq!(
                error_code(&refusal),
                "UnprocessableEntityException",
                "case {case:?}: expected UnprocessableEntityException, got {refusal:?}"
            );
            assert_eq!(
                refusal.pointer(&format!("/errors/{}", member.path)),
                Some(&json!(requirement(member.path))),
                "case {case:?}: the refusal must be keyed by {:?} and state the floor, \
                 got {refusal:?}",
                member.path
            );

            // The stored endpoint is exactly the one the create stored.
            let (status, described) =
                call(&router, "GET", &format!("/origin_endpoints/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: DescribeOriginEndpoint expected 200, got {status} {described:?}"
            );
            assert_eq!(
                described, created,
                "case {case:?}: a refused update must leave the stored record alone, \
                 got {described:?}"
            );
        }
    }
}
