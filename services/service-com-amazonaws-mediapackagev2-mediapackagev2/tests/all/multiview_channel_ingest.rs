// SPDX-License-Identifier: CC0-1.0

//! Which channels get an ingest stream of their own — asserted over the SET of input
//! types a channel can be created with, rather than the one recorded request.
//!
//! The rules under test:
//!
//! 1. A channel whose input type receives no ingest of its own publishes NO ingest
//!    endpoint, while a channel of every other input type publishes its redundant pair of
//!    them; `CreateChannel`, `GetChannel` and `UpdateChannel` all report the same set,
//!    because each reads the one the create stored.
//! 2. The ingest data plane serves exactly the URLs the control plane published: a
//!    credentialed push to a published URL is accepted, and a push to a path under a
//!    channel that published none is answered as a path this service does not publish —
//!    the same answer a path naming no channel at all gets, so nothing is recorded
//!    against a channel that can never have received it.
//!
//! The rows differ along the channel's input type (the model's whole `InputType` enum
//! plus the absent case), the source count and layout count of a multiview configuration,
//! and — on the data plane — the index within a channel's redundant pair and the write
//! method. They do not vary the resource names, which neither rule generalises over.
//!
//! The client and the URL splitter are the ones `rejection_rule_and_negative_paths`
//! already owns, reached through `pub(crate)`, so there is no second copy of them here.

use serde_json::{json, Value};

use crate::rejection_rule_and_negative_paths::{split_url, Answer, Emulator};

/// The channel group every row hangs under. Nothing in either rule depends on it.
const GROUP: &str = "mv-ingest";

/// The CMAF channels the multiview rows list as tile sources:
/// `MultiviewConfiguration$AvailableSources`' doc requires each source to be in the same
/// channel group and to have an `InputType` of `CMAF`.
const SOURCES: [&str; 2] = ["tile-one", "tile-two"];

/// Layout values, from the model's `MultiviewLayoutType` enum (LAYOUT_2EH | LAYOUT_2PL |
/// LAYOUT_3EL | LAYOUT_3PL | LAYOUT_4E | LAYOUT_4PL). The rows use entries the reported
/// case did not cite as well as the whole catalogue, which `AvailableLayouts`' `@length`
/// of 1..6 also makes the longest legal list.
const LAYOUTS: [&str; 6] = [
    "LAYOUT_2EH",
    "LAYOUT_2PL",
    "LAYOUT_3EL",
    "LAYOUT_3PL",
    "LAYOUT_4E",
    "LAYOUT_4PL",
];

/// The methods the ingest data plane serves: a contributing encoder writes a segment with
/// either of them.
const INGEST_METHODS: &[&str] = &["PUT", "POST"];

/// The indices a channel's redundant pair of ingest endpoints is published under, which
/// are also the only indices a published ingest path can carry.
const INGEST_INDICES: &[u8] = &[1, 2];

/// One row: a channel to create, and whether its input type gives it an ingest stream of
/// its own.
struct Row {
    label: &'static str,
    channel: &'static str,
    body: fn() -> Value,
    receives_ingest: bool,
}

/// The rows span the model's whole `InputType` enum (CMAF | HLS | MULTIVIEW) plus the
/// absent case the member's doc defaults to HLS, and vary the multiview channels along
/// source count, layout count and the members ingest is indifferent to.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "input type absent (defaults to HLS)",
            channel: "plain",
            body: || json!({}),
            receives_ingest: true,
        },
        Row {
            label: "HLS input, described and tagged",
            channel: "hls-channel",
            body: || json!({"InputType": "HLS", "Description": "ingests HLS", "tags": {"k": "v"}}),
            receives_ingest: true,
        },
        Row {
            label: "CMAF input",
            channel: "cmaf-channel",
            body: || json!({"InputType": "CMAF"}),
            receives_ingest: true,
        },
        Row {
            label: "MULTIVIEW: one source, one layout the report did not cite",
            channel: "mv-one",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": [SOURCES[1]],
                        "AvailableLayouts": [LAYOUTS[4]],
                    },
                })
            },
            receives_ingest: false,
        },
        Row {
            label: "MULTIVIEW: every source, the whole layout catalogue",
            channel: "mv-every",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "MultiviewConfiguration": {
                        "AvailableSources": SOURCES,
                        "AvailableLayouts": LAYOUTS,
                    },
                })
            },
            receives_ingest: false,
        },
        Row {
            label: "MULTIVIEW: described and tagged",
            channel: "mv-described",
            body: || {
                json!({
                    "InputType": "MULTIVIEW",
                    "Description": "composites two sources",
                    "tags": {"a": "1", "b": "2"},
                    "MultiviewConfiguration": {
                        "AvailableSources": SOURCES,
                        "AvailableLayouts": [LAYOUTS[1], LAYOUTS[5]],
                    },
                })
            },
            receives_ingest: false,
        },
    ]
}

/// The ingest URLs a channel response publishes, in the order it reports them. An absent
/// member and an empty list both mean the channel publishes none.
fn published_urls(body: &Value) -> Vec<String> {
    body.get("IngestEndpoints")
        .and_then(Value::as_array)
        .map(|endpoints| {
            endpoints
                .iter()
                .map(|endpoint| {
                    endpoint
                        .get("Url")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The DNS every ingest hostname of a group shares: everything from the `.ingest.` label
/// onwards of a hostname the service published. Each ingest ENDPOINT carries its own first
/// label in front of it.
fn ingest_dns(host: &str) -> &str {
    let at = host
        .find(".ingest.")
        .unwrap_or_else(|| panic!("a published ingest hostname carries the ingest label, got {host}"));
    &host[at..]
}

/// [`GROUP`] with its [`SOURCES`] in place, so the multiview rows resolve, reporting one
/// ingest hostname the group's channels publish their URLs on.
///
/// The hostname is read off a published URL rather than built here: a path this suite
/// pushes to must be one the service itself handed out.
async fn seeded(emulator: &Emulator) -> String {
    let group = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        group.status, 200,
        "setup: creating group {GROUP} must answer 200, got {} with body {}",
        group.status, group.body
    );
    let mut host = String::new();
    for source in SOURCES {
        let created = emulator
            .create_channel(GROUP, source, json!({"InputType": "CMAF"}))
            .await;
        assert_eq!(
            created.status, 200,
            "setup: creating source {source} must answer 200, got {} with body {}",
            created.status, created.body
        );
        let urls = published_urls(&created.json);
        let first = urls.first().unwrap_or_else(|| {
            panic!("setup: a CMAF source publishes ingest URLs, got {}", created.body)
        });
        host = split_url(first).0;
    }
    host
}

/// Read one channel back.
async fn get_channel(emulator: &Emulator, channel: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("/channelGroup/{GROUP}/channel/{channel}/"),
            None,
        )
        .await
}

/// Update one channel's description, which no rule here is about: it is the cheapest
/// mutation that makes `UpdateChannel` report the channel again.
async fn update_channel(emulator: &Emulator, channel: &str, description: &str) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("/channelGroup/{GROUP}/channel/{channel}/"),
            Some(json!({"Description": description})),
        )
        .await
}

/// The path a push addresses under one channel's `index`-th ingest endpoint, in the form
/// [`published_urls`] reports: `/in/v1/<group>/<index>/<channel>/index/<object>`.
fn ingest_path(channel: &str, index: u8, object: &str) -> String {
    format!("/in/v1/{GROUP}/{index}/{channel}/index/{object}")
}

/// Send one credentialed push of a small body.
async fn push(emulator: &Emulator, method: &str, path: &str, host: &str) -> Answer {
    emulator
        .send(
            method,
            path,
            Some(json!({"segment": "bytes"})),
            host,
            true,
            &[],
        )
        .await
}

#[tokio::test]
async fn only_a_channel_that_receives_ingest_of_its_own_publishes_ingest_endpoints() {
    let emulator = Emulator::new();
    let host = seeded(&emulator).await;

    for row in rows() {
        let label = row.label;
        let created = emulator
            .create_channel(GROUP, row.channel, (row.body)())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: creating {} must answer 200, got {} with body {}",
            row.channel, created.status, created.body
        );
        let published = published_urls(&created.json);
        if row.receives_ingest {
            assert!(
                published.len() >= 2,
                "case {label:?}: a channel that receives ingest publishes a redundant pair of \
                 ingest endpoints, got {published:?}"
            );
            let dns = ingest_dns(&host);
            let mut hosts: Vec<String> = Vec::new();
            for (position, url) in published.iter().enumerate() {
                let (url_host, path) = split_url(url);
                assert!(
                    url_host.ends_with(dns),
                    "case {label:?}: published URL {position} must be on the group's ingest DNS \
                     {dns}, got {url}"
                );
                assert!(
                    path.contains(&format!("/{}/", row.channel)),
                    "case {label:?}: published URL {position} must name the channel, got {url}"
                );
                hosts.push(url_host);
            }
            // Each endpoint of the pair is a separate ingest TARGET, so each is published
            // on a host of its own: two publishers sending to the redundant pair address
            // different hostnames rather than one host under two paths.
            let mut distinct = hosts.clone();
            distinct.sort();
            distinct.dedup();
            assert_eq!(
                distinct.len(),
                hosts.len(),
                "case {label:?}: each published ingest endpoint must have its own hostname, got \
                 {hosts:?}"
            );
        } else {
            assert!(
                published.is_empty(),
                "case {label:?}: a channel that receives no ingest of its own publishes no \
                 ingest endpoint, got {published:?}"
            );
        }

        // Every reader of the channel reports the set the create stored, so none of them
        // can hand out a URL another one does not.
        let read = get_channel(&emulator, row.channel).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: reading {} must answer 200, got {} with body {}",
            row.channel, read.status, read.body
        );
        assert_eq!(
            published_urls(&read.json),
            published,
            "case {label:?}: GetChannel must report the ingest endpoints the create stored, got \
             {}",
            read.body
        );
        let updated = update_channel(&emulator, row.channel, "an updated description").await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: updating {} must answer 200, got {} with body {}",
            row.channel, updated.status, updated.body
        );
        assert_eq!(
            published_urls(&updated.json),
            published,
            "case {label:?}: UpdateChannel must report the same ingest endpoints, got {}",
            updated.body
        );
    }
}

#[tokio::test]
async fn the_ingest_data_plane_serves_only_the_paths_a_channel_published() {
    let emulator = Emulator::new();
    let host = seeded(&emulator).await;

    // The answer a path naming no channel at all gets: what a path this service does not
    // publish looks like, and the answer every unpublished path has to be
    // indistinguishable from.
    let unknown = push(
        &emulator,
        "PUT",
        &ingest_path("no-such-channel", INGEST_INDICES[0], "0.mp4"),
        &host,
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "a push to a path naming no channel must answer 404, got {} with body {}",
        unknown.status, unknown.body
    );

    for row in rows() {
        let label = row.label;
        let created = emulator
            .create_channel(GROUP, row.channel, (row.body)())
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: creating {} must answer 200, got {} with body {}",
            row.channel, created.status, created.body
        );

        for index in INGEST_INDICES {
            for method in INGEST_METHODS {
                let path = ingest_path(row.channel, *index, &format!("{method}-{index}.mp4"));
                let answer = push(&emulator, method, &path, &host).await;
                if row.receives_ingest {
                    assert_eq!(
                        answer.status, 200,
                        "case {label:?} / endpoint {index} / {method}: a credentialed push to a \
                         published ingest path must be accepted, got {} with body {}",
                        answer.status, answer.body
                    );
                } else {
                    assert_eq!(
                        answer.status, 404,
                        "case {label:?} / endpoint {index} / {method}: a push to a channel that \
                         published no ingest endpoint must answer 404, got {} with body {}",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.body, unknown.body,
                        "case {label:?} / endpoint {index} / {method}: the refusal must be the \
                         one an unpublished path gets, got {}",
                        answer.body
                    );
                }
            }
        }

        // A push that presented no credential is still refused for the credential, on
        // either kind of channel: the data plane does not report which channels exist to a
        // caller that has not authenticated.
        let anonymous = emulator
            .send(
                "PUT",
                &ingest_path(row.channel, INGEST_INDICES[0], "anonymous.mp4"),
                Some(json!({"segment": "bytes"})),
                &host,
                false,
                &[],
            )
            .await;
        assert_eq!(
            anonymous.status, 403,
            "case {label:?}: an unauthenticated push must answer 403, got {} with body {}",
            anonymous.status, anonymous.body
        );

        // And the refused pushes left the channel exactly as the create did.
        let read = get_channel(&emulator, row.channel).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: reading {} must answer 200, got {} with body {}",
            row.channel, read.status, read.body
        );
        for member in ["IngestEndpoints", "ETag", "ModifiedAt", "InputType"] {
            assert_eq!(
                read.json.get(member),
                created.json.get(member),
                "case {label:?}: a push must leave {member} alone: was {:?}, now {:?}",
                created.json.get(member),
                read.json.get(member)
            );
        }
    }
}
