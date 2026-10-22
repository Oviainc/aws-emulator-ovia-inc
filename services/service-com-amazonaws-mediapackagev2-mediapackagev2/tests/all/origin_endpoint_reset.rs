// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ResetOriginEndpointState`, driven over HTTP through the same
//! router the emulator boots.
//!
//! The rules under test, each asserted over a SET of endpoints rather than one recorded
//! request:
//!
//! 1. A reset records the instant it happened on the ADDRESSED origin endpoint and
//!    reports that instant as `ResetAt`, alongside the endpoint's persisted identity
//!    (`Arn` and the three names); a later `GetOriginEndpoint` reads back the same value.
//! 2. A reset clears content, it does not reconfigure the resource: every other member
//!    the read reports (`CreatedAt`, `ModifiedAt`, `ETag`, container type, segment,
//!    manifests, tags, ...) is unchanged, as is every endpoint and the channel the reset
//!    did not address.
//! 3. Resetting again moves `ResetAt` forward (never backward) and still changes nothing
//!    else, so the operation is repeatable.
//! 4. A reset whose containment chain does not resolve answers `ResourceNotFoundException`
//!    (404) naming the level that is missing, and persists nothing.
//!
//! The emulator and its restJson1 client are the ones `channel_lifecycle` already builds,
//! so both suites exercise the service through one client.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};

/// `ResetOriginEndpointState`: POST to the endpoint's `/reset` sub-resource, per the
/// model's http binding for the operation
/// (`POST /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint/{OriginEndpointName}/reset`).
async fn reset_endpoint(
    emulator: &Emulator,
    group: &str,
    channel: &str,
    endpoint: &str,
) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/reset"),
            None,
        )
        .await
}

/// A timestamp member as epoch seconds — restJson1 renders `Timestamp` as a JSON number.
fn epoch_seconds(body: &Value, member: &str, case: &str) -> f64 {
    body[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member} to be an epoch-seconds number, got {body}")
    })
}

/// One endpoint of a row: its name and the body it is created with.
type EndpointSpec = (&'static str, fn() -> Value);

/// One row of the reset table: a channel whose endpoints differ in name shape, container
/// type and which manifest families they carry. The first endpoint is the one that gets
/// reset; the rest are the untouched siblings.
struct ResetCase {
    label: &'static str,
    group: &'static str,
    channel: &'static str,
    /// The row's endpoints, the first entry being the reset target.
    endpoints: &'static [EndpointSpec],
}

/// The rows. `ContainerType` values come from the model's `ContainerType` enum
/// (CMAF, ISM, TS); every endpoint carries at least one manifest, which
/// `CreateOriginEndpoint` requires (`NUM_MANIFESTS_LOW`).
fn reset_cases() -> Vec<ResetCase> {
    vec![
        ResetCase {
            label: "a lone minimal TS endpoint",
            group: "g1",
            channel: "c1",
            endpoints: &[("e1", || minimal_endpoint("TS"))],
        },
        ResetCase {
            label: "a configured CMAF endpoint beside two siblings",
            group: "A_group_with_9_and-dashes",
            channel: "A_channel_0",
            endpoints: &[
                ("An_endpoint_0", || {
                    json!({
                        "ContainerType": "CMAF",
                        "Description": "an endpoint",
                        "StartoverWindowSeconds": 300,
                        "Segment": {"SegmentDurationSeconds": 6, "SegmentName": "seg"},
                        "HlsManifests": [{"ManifestName": "hls-a"}, {"ManifestName": "hls-b"}],
                        "LowLatencyHlsManifests": [{"ManifestName": "ll-hls"}],
                        "DashManifests": [{"ManifestName": "dash"}],
                        "Tags": {"team": "video"},
                    })
                }),
                ("sibling-1", || minimal_endpoint("TS")),
                ("sibling_2", || {
                    json!({
                        "ContainerType": "CMAF",
                        "Description": "another",
                        "HlsManifests": [{"ManifestName": "index"}],
                    })
                }),
            ],
        },
        ResetCase {
            label: "an ISM endpoint carrying an MSS manifest",
            group: "g-ism",
            channel: "c-ism",
            endpoints: &[
                ("e-ism", || {
                    json!({
                        "ContainerType": "ISM",
                        "MssManifests": [{"ManifestName": "smooth"}],
                    })
                }),
                ("e-ism-other", || minimal_endpoint("ISM")),
            ],
        },
    ]
}

/// Build one case's hierarchy and return the pre-reset `GetOriginEndpoint` body of every
/// endpoint, in the case's own order.
async fn build_case(emulator: &Emulator, case: &ResetCase) -> Vec<Value> {
    let label = case.label;
    let (status, body) = emulator.create_group(case.group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannelGroup expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_channel(case.group, case.channel, json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannel expected 200, got {status} {body}"
    );

    let mut reads = Vec::new();
    for (name, body_of) in case.endpoints {
        let (status, body) = emulator
            .create_endpoint(case.group, case.channel, name, body_of())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateOriginEndpoint {name} expected 200, got {status} {body}"
        );
        let (status, read) = emulator.get_endpoint(case.group, case.channel, name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint {name} expected 200, got {status} {read}"
        );
        // RULE 1, the other side of it: an endpoint nobody reset carries no ResetAt, so
        // the member cannot be a value the create invented.
        assert!(
            read["ResetAt"].is_null(),
            "case {label:?}: endpoint {name} was never reset, so ResetAt must be absent, got {read}"
        );
        reads.push(read);
    }
    reads
}

/// RULES 1 and 2 — a reset records its instant on the addressed endpoint, reports it, and
/// leaves everything else (including the siblings and the channel) exactly as it was.
#[tokio::test]
async fn a_reset_records_its_instant_on_the_addressed_endpoint_and_changes_nothing_else() {
    let emulator = Emulator::new();

    for case in reset_cases() {
        let label = case.label;
        let before = build_case(&emulator, &case).await;
        let (target, _) = case.endpoints[0];
        let before_target = &before[0];

        let (status, reset) = reset_endpoint(&emulator, case.group, case.channel, target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ResetOriginEndpointState expected 200, got {status} {reset}"
        );

        // The identity members are answered from the persisted resource, so they must be
        // the very values the create stored.
        for member in [
            "Arn",
            "ChannelGroupName",
            "ChannelName",
            "OriginEndpointName",
        ] {
            assert_eq!(
                reset[member], before_target[member],
                "case {label:?}: reset {member} must equal the stored endpoint's, got {reset}"
            );
        }

        let reset_at = epoch_seconds(&reset, "ResetAt", label);
        let created_at = epoch_seconds(before_target, "CreatedAt", label);
        assert!(
            reset_at >= created_at,
            "case {label:?}: ResetAt {reset_at} must not precede CreatedAt {created_at}"
        );

        // RULE 1 — the read reports the instant the reset answered with.
        let (status, after) = emulator
            .get_endpoint(case.group, case.channel, target)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetOriginEndpoint after reset expected 200, got {status} {after}"
        );
        assert_eq!(
            after["ResetAt"], reset["ResetAt"],
            "case {label:?}: the read must report the ResetAt the reset answered with, got {after}"
        );

        // RULE 2 — ResetAt is the ONLY member that moved.
        let members = before_target
            .as_object()
            .expect("GetOriginEndpoint answers an object");
        for member in members.keys() {
            if member == "ResetAt" {
                continue;
            }
            assert_eq!(
                after[member], before_target[member],
                "case {label:?}: a reset must not change {member}, got {} after {}",
                after[member], before_target[member]
            );
        }
        // The member SET the read reports gains exactly `ResetAt` — stated as a set
        // comparison so an explicitly-null `ResetAt` pre-reset would still be caught.
        let mut expected_members: std::collections::BTreeSet<&str> =
            members.keys().map(String::as_str).collect();
        expected_members.insert("ResetAt");
        let actual_members: std::collections::BTreeSet<&str> = after
            .as_object()
            .expect("GetOriginEndpoint answers an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            actual_members, expected_members,
            "case {label:?}: a reset must add only ResetAt to the read, got {after}"
        );

        // RULE 2 — the siblings and the parent channel are untouched.
        for (name, _) in &case.endpoints[1..] {
            let (status, sibling) = emulator.get_endpoint(case.group, case.channel, name).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetOriginEndpoint {name} expected 200, got {status} {sibling}"
            );
            assert!(
                sibling["ResetAt"].is_null(),
                "case {label:?}: sibling {name} was not addressed, so its ResetAt must stay absent, got {sibling}"
            );
        }
        let (status, channel) = emulator.get_channel(case.group, case.channel).await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetChannel expected 200, got {status} {channel}"
        );
        assert!(
            channel["ResetAt"].is_null(),
            "case {label:?}: resetting an endpoint must not reset its channel, got {channel}"
        );
    }
}

/// RULE 3 — the operation is repeatable: each reset moves ResetAt forward, never back,
/// and still changes nothing else.
#[tokio::test]
async fn resetting_again_moves_reset_at_forward_and_nothing_else() {
    let emulator = Emulator::new();

    for case in reset_cases() {
        let label = case.label;
        let before = build_case(&emulator, &case).await;
        let (target, _) = case.endpoints[0];
        let before_target = &before[0];

        let mut previous = f64::NEG_INFINITY;
        // Three resets: one to establish ResetAt, two more to pin that a later reset
        // reports a later-or-equal instant rather than re-reporting the first.
        for attempt in 1..=3 {
            let (status, reset) = reset_endpoint(&emulator, case.group, case.channel, target).await;
            assert_eq!(
                status, 200,
                "case {label:?}: reset #{attempt} expected 200, got {status} {reset}"
            );
            let reset_at = epoch_seconds(&reset, "ResetAt", label);
            assert!(
                reset_at >= previous,
                "case {label:?}: reset #{attempt} reported ResetAt {reset_at}, which precedes the previous {previous}"
            );
            previous = reset_at;

            let (status, after) = emulator
                .get_endpoint(case.group, case.channel, target)
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetOriginEndpoint after reset #{attempt} expected 200, got {status} {after}"
            );
            assert_eq!(
                after["ResetAt"], reset["ResetAt"],
                "case {label:?}: after reset #{attempt} the read must report that reset's instant, got {after}"
            );
            for member in ["CreatedAt", "ModifiedAt", "ETag", "Arn", "ContainerType"] {
                assert_eq!(
                    after[member], before_target[member],
                    "case {label:?}: reset #{attempt} must not change {member}, got {}",
                    after[member]
                );
            }
        }
    }
}

/// RULE 4 — an unresolved containment chain is refused with the level that is missing.
/// `ResourceTypeNotFound` values come from the model's `ResourceTypeNotFound` enum
/// (CHANNEL_GROUP, CHANNEL, ORIGIN_ENDPOINT, HARVEST_JOB).
#[tokio::test]
async fn a_reset_through_an_unresolved_chain_answers_404_naming_the_missing_level() {
    let emulator = Emulator::new();
    let case = ResetCase {
        label: "the 404 fixture",
        group: "g404",
        channel: "c404",
        endpoints: &[("e404", || minimal_endpoint("TS"))],
    };
    build_case(&emulator, &case).await;

    // Each row addresses exactly one level that does not exist, with every other level
    // present: `(row, group, channel, endpoint, expected ResourceTypeNotFound)`.
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        ("absent group", "no-such-group", "c404", "e404", "CHANNEL_GROUP"),
        ("absent channel", "g404", "no-such-channel", "e404", "CHANNEL"),
        ("absent endpoint", "g404", "c404", "no-such-endpoint", "ORIGIN_ENDPOINT"),
    ];

    for (row, group, channel, endpoint, expected) in rows {
        let (status, body) = reset_endpoint(&emulator, group, channel, endpoint).await;
        assert_eq!(status, 404, "case {row:?}: expected 404, got {status} {body}");
        assert!(
            error_type(&body).contains("ResourceNotFoundException"),
            "case {row:?}: expected a ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceTypeNotFound"].as_str(),
            Some(*expected),
            "case {row:?}: expected ResourceTypeNotFound {expected}, got {body}"
        );
    }

    // A refused reset persists nothing: the endpoint that does exist was never reset.
    let (status, read) = emulator.get_endpoint(case.group, case.channel, "e404").await;
    assert_eq!(
        status, 200,
        "GetOriginEndpoint expected 200, got {status} {read}"
    );
    assert!(
        read["ResetAt"].is_null(),
        "a refused reset must persist nothing, got {read}"
    );
}
