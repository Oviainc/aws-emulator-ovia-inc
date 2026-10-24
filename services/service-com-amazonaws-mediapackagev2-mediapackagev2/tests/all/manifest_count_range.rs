// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for ONE rule: the number of manifest configurations an origin
//! endpoint may carry, counted across the HLS, low-latency HLS, DASH and MSS families
//! together, is a permitted RANGE, and a request outside it is an input failure — HTTP
//! 400 `ValidationException` carrying the discriminator for the end of the range that was
//! crossed (`NUM_MANIFESTS_LOW` below the floor, `NUM_MANIFESTS_HIGH` above the ceiling,
//! both from the model's `ValidationExceptionType` enum) — never a quota class, and
//! nothing is created or changed.
//!
//! The rule holds for both operations that write a whole manifest set,
//! `CreateOriginEndpoint` and `UpdateOriginEndpoint`, so every row below is driven through
//! each of them. The floor and the ceiling VALUES are the emulator's own, from
//! `src/state.rs` (`MIN_MANIFESTS_PER_ORIGIN_ENDPOINT` /
//! `MAX_MANIFESTS_PER_ORIGIN_ENDPOINT`): the rows sit on each bound, inside it and past
//! it, and vary how the total is spread over the families and which container type
//! carries them.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    MAX_MANIFESTS_PER_ORIGIN_ENDPOINT, MIN_MANIFESTS_PER_ORIGIN_ENDPOINT,
};

use crate::channel_lifecycle::{error_type, Emulator};

/// What the service is expected to answer for one manifest set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The set is inside the range: the write is accepted.
    Accepted,
    /// The set is outside the range: 400 `ValidationException` with this discriminator.
    Refused(&'static str),
}

/// One row: a container type and how many manifests of each family the request carries.
///
/// The per-family members are the model's own (`HlsManifests`, `LowLatencyHlsManifests`,
/// `DashManifests` and `MssManifests` on `CreateOriginEndpointRequest`), and every row
/// only ever asks a container type for families it serves — a CMAF container with an MSS
/// manifest is a DIFFERENT rule, covered in `tests/all/control_plane_validation.rs` — so
/// what the rows vary is the COUNT and its spread, not family/container compatibility.
#[derive(Debug, Clone, Copy)]
struct Case {
    label: &'static str,
    container: &'static str,
    /// `(member, count)` pairs; a member not named here is not sent at all.
    families: &'static [(&'static str, usize)],
    expected: Expected,
}

/// The total a row asks for: the number the rule is about.
fn total(case: &Case) -> usize {
    case.families.iter().map(|(_, count)| count).sum()
}

/// The request body for a row: its container type plus each named manifest member, every
/// manifest with a distinct `ManifestName` so no row is refused on the name-collision rule
/// instead of the count rule.
fn request_body(case: &Case) -> Value {
    let mut body = json!({"ContainerType": case.container});
    for (member, count) in case.families {
        let manifests: Value = (0..*count)
            .map(|index| json!({"ManifestName": format!("{}-{index}", member.to_lowercase())}))
            .collect();
        body[*member] = manifests;
    }
    body
}

/// The rows. `MAX` below is [`MAX_MANIFESTS_PER_ORIGIN_ENDPOINT`] and `MIN` is
/// [`MIN_MANIFESTS_PER_ORIGIN_ENDPOINT`]; `ContainerType` values are the model's
/// (`CMAF | ISM | TS`).
fn rows() -> Vec<Case> {
    const MAX: usize = MAX_MANIFESTS_PER_ORIGIN_ENDPOINT;
    const MIN: usize = MIN_MANIFESTS_PER_ORIGIN_ENDPOINT;
    vec![
        Case {
            label: "below the floor: no manifest member at all",
            container: "TS",
            families: &[],
            expected: Expected::Refused("NUM_MANIFESTS_LOW"),
        },
        Case {
            label: "below the floor: three manifest members present but empty",
            container: "CMAF",
            families: &[
                ("HlsManifests", 0),
                ("LowLatencyHlsManifests", 0),
                ("DashManifests", 0),
            ],
            expected: Expected::Refused("NUM_MANIFESTS_LOW"),
        },
        Case {
            label: "on the floor: one HLS manifest",
            container: "TS",
            families: &[("HlsManifests", MIN)],
            expected: Expected::Accepted,
        },
        Case {
            label: "on the floor: one MSS manifest on an ISM container",
            container: "ISM",
            families: &[("MssManifests", MIN)],
            expected: Expected::Accepted,
        },
        Case {
            label: "inside the range: spread over three families",
            container: "CMAF",
            families: &[
                ("HlsManifests", 7),
                ("LowLatencyHlsManifests", 11),
                ("DashManifests", 3),
            ],
            expected: Expected::Accepted,
        },
        Case {
            label: "on the ceiling: one family",
            container: "TS",
            families: &[("HlsManifests", MAX)],
            expected: Expected::Accepted,
        },
        Case {
            label: "on the ceiling: spread over three families",
            container: "CMAF",
            families: &[
                ("HlsManifests", MAX / 2),
                ("LowLatencyHlsManifests", MAX / 4),
                ("DashManifests", MAX - MAX / 2 - MAX / 4),
            ],
            expected: Expected::Accepted,
        },
        Case {
            label: "past the ceiling: one too many in one family",
            container: "TS",
            families: &[("HlsManifests", MAX + 1)],
            expected: Expected::Refused("NUM_MANIFESTS_HIGH"),
        },
        Case {
            label: "past the ceiling: the one too many is in another family",
            container: "CMAF",
            families: &[("HlsManifests", MAX), ("DashManifests", 1)],
            expected: Expected::Refused("NUM_MANIFESTS_HIGH"),
        },
        Case {
            label: "far past the ceiling: MSS manifests on an ISM container",
            container: "ISM",
            families: &[("MssManifests", MAX * 2)],
            expected: Expected::Refused("NUM_MANIFESTS_HIGH"),
        },
    ]
}

/// Assert one answer against the rule: inside the range a 200, outside it a 400
/// `ValidationException` carrying the expected discriminator.
fn assert_answer(label: &str, expected: Expected, status: u16, body: &Value) {
    match expected {
        Expected::Accepted => assert_eq!(
            status, 200,
            "case {label:?}: a manifest count inside the range expected 200, \
             got {status} {body}"
        ),
        Expected::Refused(exception_type) => {
            assert_eq!(
                status, 400,
                "case {label:?}: a manifest count outside the range expected 400, \
                 got {status} {body}"
            );
            assert_eq!(
                error_type(body),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body.get("ValidationExceptionType").and_then(Value::as_str),
                Some(exception_type),
                "case {label:?}: expected ValidationExceptionType {exception_type}, got {body}"
            );
        }
    }
}

/// How many manifests an endpoint reports, counted across every family it reports.
fn reported_total(endpoint: &Value) -> usize {
    [
        "HlsManifests",
        "LowLatencyHlsManifests",
        "DashManifests",
        "MssManifests",
    ]
    .into_iter()
    .map(|member| {
        endpoint
            .get(member)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    })
    .sum()
}

/// A channel group and a channel for the endpoints each test writes.
async fn hierarchy() -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group("grp", json!({})).await;
    assert_eq!(status, 200, "setup: CreateChannelGroup got {status} {body}");
    let (status, body) = emulator.create_channel("grp", "chan", json!({})).await;
    assert_eq!(status, 200, "setup: CreateChannel got {status} {body}");
    emulator
}

#[tokio::test]
async fn create_holds_the_manifest_count_to_the_permitted_range() {
    let emulator = hierarchy().await;
    for (index, case) in rows().iter().enumerate() {
        let name = format!("ep-{index}");
        let (status, body) = emulator
            .create_endpoint("grp", "chan", &name, request_body(case))
            .await;
        assert_answer(case.label, case.expected, status, &body);

        // Round trip: an accepted set is stored and read back whole, a refused one stores
        // nothing at all.
        let (read_status, read) = emulator.get_endpoint("grp", "chan", &name).await;
        match case.expected {
            Expected::Accepted => {
                assert_eq!(
                    read_status, 200,
                    "case {:?}: the accepted endpoint must read back, got {read_status} {read}",
                    case.label
                );
                assert_eq!(
                    reported_total(&read),
                    total(case),
                    "case {:?}: expected {} manifests stored, got {read}",
                    case.label,
                    total(case)
                );
            }
            Expected::Refused(_) => assert_eq!(
                read_status, 404,
                "case {:?}: a refused create must store nothing, got {read_status} {read}",
                case.label
            ),
        }
    }
}

#[tokio::test]
async fn update_holds_the_manifest_count_to_the_permitted_range() {
    let emulator = hierarchy().await;
    // An endpoint's container type is fixed once it exists (`CONTAINER_TYPE_IMMUTABLE`),
    // so each container type gets an endpoint of its own, seeded with a manifest set the
    // rule accepts, and is updated by the rows that carry that container type.
    for (container, seed_families) in [
        ("TS", &[("HlsManifests", 1usize)][..]),
        ("CMAF", &[("DashManifests", 1usize)][..]),
        ("ISM", &[("MssManifests", 1usize)][..]),
    ] {
        let seed = Case {
            label: "update seed",
            container,
            families: seed_families,
            expected: Expected::Accepted,
        };
        let name = format!("ep-{container}");
        let (status, body) = emulator
            .create_endpoint("grp", "chan", &name, request_body(&seed))
            .await;
        assert_eq!(
            status, 200,
            "setup: CreateOriginEndpoint for {container} got {status} {body}"
        );

        // What the endpoint holds: an accepted update replaces the whole set, a refused
        // one must leave it exactly as it was.
        let mut stored = total(&seed);
        for case in rows().iter().filter(|case| case.container == container) {
            let (status, body) = emulator
                .update_endpoint("grp", "chan", &name, request_body(case), &[])
                .await;
            assert_answer(case.label, case.expected, status, &body);
            if case.expected == Expected::Accepted {
                stored = total(case);
            }
            let (read_status, read) = emulator.get_endpoint("grp", "chan", &name).await;
            assert_eq!(
                read_status, 200,
                "case {:?}: the endpoint must still read back, got {read_status} {read}",
                case.label
            );
            assert_eq!(
                reported_total(&read),
                stored,
                "case {:?}: expected {stored} manifests stored, got {read}",
                case.label
            );
        }
    }
}
