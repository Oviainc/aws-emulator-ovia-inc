// SPDX-License-Identifier: CC0-1.0

//! The namespace an origin endpoint's manifest names are taken from, driven end-to-end
//! over HTTP through the same router the emulator boots and reusing the restJson1 client
//! in `channel_lifecycle`.
//!
//! The rule under test, asserted over a TABLE of inputs rather than over one recorded
//! request: a manifest name is what makes the egress URL the service computes for a
//! manifest identify THAT manifest, so no two manifests of one endpoint may be published
//! at the same URL — which is to say that a name may be used at most once among the
//! manifests published in the same container format. The HLS and low-latency HLS families
//! share that format (both publish an `.m3u8` playlist), which is why the model documents
//! that you "can't use the same manifest name if you use HLS manifest and low-latency HLS
//! manifest"
//! (`smithy_query(query="doc", shape="CreateHlsManifestConfiguration", member="ManifestName")`)
//! and names `MANIFEST_NAME_COLLISION` in its `ValidationExceptionType` enum. A set that
//! breaks the rule is a 400 `ValidationException` with that discriminator and nothing
//! stored; one that keeps it is stored, and every manifest reads back with a URL of its
//! own.
//!
//! The rows differ along what the rule generalises over: which family carries the repeat,
//! whether the two manifests are in one family or in two that share a format, the POSITION
//! of the repeat in the set, the endpoint's container type, and the count of manifests
//! around it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The parents every endpoint in this suite hangs under. A CMAF-input channel, so a row
/// may carry any container type and any manifest family its container serves.
const GROUP: &str = "mn-group";
const CHANNEL: &str = "mn-channel";

/// Every manifest family an origin endpoint can carry, as the request member that carries
/// it (`CreateOriginEndpointRequest$HlsManifests` and its siblings).
const FAMILIES: [&str; 4] = [
    "HlsManifests",
    "LowLatencyHlsManifests",
    "DashManifests",
    "MssManifests",
];

/// What a create carrying the row's manifest set must answer.
enum Expected {
    /// 200, with every manifest stored under its own URL.
    Stored,
    /// 400 `ValidationException` / `MANIFEST_NAME_COLLISION`, naming this member path, and
    /// with no endpoint created.
    Refused(&'static str),
}

/// One row: the endpoint's container type, the names each family carries, and the answer.
struct Case {
    label: &'static str,
    container: &'static str,
    /// `(family member, the names that family's manifests carry, in request order)`.
    manifests: &'static [(&'static str, &'static [&'static str])],
    expected: Expected,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one name per manifest, across three families",
            container: "CMAF",
            manifests: &[
                ("HlsManifests", &["index", "backup"]),
                ("LowLatencyHlsManifests", &["low-latency"]),
                ("DashManifests", &["dash"]),
            ],
            expected: Expected::Stored,
        },
        Case {
            label: "one name reused by manifests of two different formats",
            container: "CMAF",
            manifests: &[
                ("HlsManifests", &["index"]),
                ("DashManifests", &["index"]),
            ],
            expected: Expected::Stored,
        },
        Case {
            label: "the same name twice in the HLS family, side by side",
            container: "CMAF",
            manifests: &[("HlsManifests", &["index", "index"])],
            expected: Expected::Refused("hlsManifests.2.member.manifestName"),
        },
        Case {
            label: "the same name twice in the HLS family, with another between them",
            container: "TS",
            manifests: &[("HlsManifests", &["index", "other", "index"])],
            expected: Expected::Refused("hlsManifests.3.member.manifestName"),
        },
        Case {
            label: "an HLS and a low-latency HLS manifest sharing one name",
            container: "CMAF",
            manifests: &[
                ("HlsManifests", &["index"]),
                ("LowLatencyHlsManifests", &["index"]),
            ],
            expected: Expected::Refused("lowLatencyHlsManifests.1.member.manifestName"),
        },
        Case {
            label: "a low-latency HLS manifest repeating a name of its own family",
            container: "TS",
            manifests: &[
                ("HlsManifests", &["index"]),
                ("LowLatencyHlsManifests", &["low", "low"]),
            ],
            expected: Expected::Refused("lowLatencyHlsManifests.2.member.manifestName"),
        },
        Case {
            label: "two DASH manifests sharing a name",
            container: "CMAF",
            manifests: &[
                ("HlsManifests", &["index"]),
                ("DashManifests", &["dash", "dash"]),
            ],
            expected: Expected::Refused("dashManifests.2.member.manifestName"),
        },
        Case {
            label: "two MSS manifests sharing a name",
            container: "ISM",
            manifests: &[("MssManifests", &["smooth", "smooth"])],
            expected: Expected::Refused("mssManifests.2.member.manifestName"),
        },
    ]
}

impl Case {
    /// The `CreateOriginEndpoint` body this row submits.
    fn body(&self) -> Value {
        let mut body = json!({"ContainerType": self.container});
        for (family, names) in self.manifests {
            body[family] = Value::Array(
                names
                    .iter()
                    .map(|name| json!({"ManifestName": name}))
                    .collect(),
            );
        }
        body
    }
}

/// Every URL the endpoint publishes, read off a projection of it.
fn published_urls(body: &Value) -> Vec<String> {
    FAMILIES
        .iter()
        .flat_map(|family| {
            body[family]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|manifest| manifest["Url"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// An emulator with the group and channel this suite's endpoints live in.
async fn emulator_with_parents() -> Emulator {
    let emulator = Emulator::new();
    let (status, body) = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(status, 200, "CreateChannelGroup expected 200, got {body}");
    let (status, body) = emulator
        .create_channel(GROUP, CHANNEL, json!({"InputType": "CMAF"}))
        .await;
    assert_eq!(status, 200, "CreateChannel expected 200, got {body}");
    emulator
}

/// Assert that `body` is this service's `MANIFEST_NAME_COLLISION` refusal naming `member`.
fn assert_collision(case: &str, status: u16, body: &Value, member: &str) {
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
        body.get("ValidationExceptionType")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "MANIFEST_NAME_COLLISION",
        "case {case:?}: expected ValidationExceptionType MANIFEST_NAME_COLLISION, got {body}"
    );
    let message = body
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.starts_with(&format!("Validation error encountered at request.{member}:")),
        "case {case:?}: the refusal must name {member} in this service's message form, \
         got {body}"
    );
}

/// RULE, on the create — a manifest set that would publish two manifests at one URL is
/// refused with the model's own discriminator and mints nothing; one that would not is
/// stored, and the endpoint reads back with a distinct URL per manifest.
#[tokio::test]
async fn no_two_manifests_of_an_endpoint_are_published_at_one_url() {
    let emulator = emulator_with_parents().await;

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let endpoint = format!("mn-ep-{index}");
        let (status, body) = emulator
            .create_endpoint(GROUP, CHANNEL, &endpoint, case.body())
            .await;

        match case.expected {
            Expected::Refused(member) => {
                assert_collision(label, status, &body, member);
                let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
                assert_eq!(
                    status, 404,
                    "case {label:?}: a refused create must mint nothing, got {status} {read}"
                );
            }
            Expected::Stored => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: CreateOriginEndpoint expected 200, got {status} {body}"
                );
                let (status, read) = emulator.get_endpoint(GROUP, CHANNEL, &endpoint).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetOriginEndpoint expected 200, got {status} {read}"
                );
                let urls = published_urls(&read);
                let expected_count: usize = case
                    .manifests
                    .iter()
                    .map(|(_, names)| names.len())
                    .sum();
                assert_eq!(
                    urls.len(),
                    expected_count,
                    "case {label:?}: expected {expected_count} published manifests, got \
                     {urls:?}"
                );
                let distinct: BTreeSet<&String> = urls.iter().collect();
                assert_eq!(
                    distinct.len(),
                    urls.len(),
                    "case {label:?}: every manifest must have an egress URL of its own, \
                     got {urls:?}"
                );
            }
        }
    }
}

/// RULE, through the update — `UpdateOriginEndpoint` replaces the whole manifest set, so
/// it reads the same rule: a replacement set that collides is refused and the endpoint
/// keeps the manifests it had, while one that does not is stored.
#[tokio::test]
async fn an_update_reads_the_manifest_names_before_replacing_the_set() {
    let emulator = emulator_with_parents().await;
    let endpoint = "mn-update-ep";
    let stored = json!({
        "ContainerType": "CMAF",
        "HlsManifests": [{"ManifestName": "index"}],
    });
    let (status, created) = emulator
        .create_endpoint(GROUP, CHANNEL, endpoint, stored.clone())
        .await;
    assert_eq!(
        status, 200,
        "CreateOriginEndpoint expected 200, got {status} {created}"
    );

    // (label, the replacement set, the member a refusal must name — `None` when the set is
    // one the endpoint may publish)
    let rows: [(&str, Value, Option<&str>); 3] = [
        (
            "a replacement set repeating a name within the HLS family",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "a"}, {"ManifestName": "a"}],
            }),
            Some("hlsManifests.2.member.manifestName"),
        ),
        (
            "a replacement set naming one manifest in each HLS family",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "shared"}],
                "LowLatencyHlsManifests": [{"ManifestName": "shared"}],
            }),
            Some("lowLatencyHlsManifests.1.member.manifestName"),
        ),
        (
            "a replacement set whose names are all its own",
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": "index"}, {"ManifestName": "backup"}],
            }),
            None,
        ),
    ];

    for (label, body, refused) in rows {
        let (status, answer) = emulator
            .update_endpoint(GROUP, CHANNEL, endpoint, body, &[])
            .await;
        let (read_status, read) = emulator.get_endpoint(GROUP, CHANNEL, endpoint).await;
        assert_eq!(
            read_status, 200,
            "case {label:?}: the endpoint must still be readable, got {read_status} {read}"
        );
        match refused {
            Some(member) => {
                assert_collision(label, status, &answer, member);
                assert_eq!(
                    read["HlsManifests"], created["HlsManifests"],
                    "case {label:?}: a refused update must leave the stored manifests as \
                     they were, got {read}"
                );
                assert_eq!(
                    read["ETag"], created["ETag"],
                    "case {label:?}: a refused update must not rotate the entity tag, got \
                     {read}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: UpdateOriginEndpoint expected 200, got {status} {answer}"
                );
                let urls = published_urls(&read);
                let distinct: BTreeSet<&String> = urls.iter().collect();
                assert_eq!(
                    (urls.len(), distinct.len()),
                    (2, 2),
                    "case {label:?}: the replacement set must publish two distinct URLs, \
                     got {urls:?}"
                );
            }
        }
    }
}
