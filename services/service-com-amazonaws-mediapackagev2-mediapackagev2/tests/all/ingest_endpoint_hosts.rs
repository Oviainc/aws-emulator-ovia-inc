// SPDX-License-Identifier: CC0-1.0

//! Where a channel's ingest endpoints are PUBLISHED.
//!
//! RULE under test: each ingest endpoint of a channel is a separate ingest target, so each
//! is published on its own hostname — the channel group's ingest DNS with a first label of
//! that endpoint's own, carrying its ordinal — rather than on one channel-level host that
//! the endpoints differ from each other only by URL path under.
//! `CreateChannel.output.IngestEndpoints` is a list of `IngestEndpoint{Id, Url}` whose
//! `Url` is "The ingest domain URL where the source stream should be sent"
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackagev2#IngestEndpoint",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")`),
//! so a client that pins or load-balances per ingest endpoint by DOMAIN — two publishers
//! sending to the redundant pair — has two domains to distinguish.
//!
//! Asserted together with what a published URL is FOR, so the hostnames are pinned on URLs
//! that still work: every host is one the ingest data plane serves, every reader of the
//! channel reports the same set, and the hosts share the group's own DNS (the suffix its
//! `EgressDomain` is published under), so one group's data plane stays one DNS name.
//!
//! The rows differ along what the rule generalises over: the channel's input type (the
//! values the model admits for a channel that receives ingest of its own, and the absent
//! case its doc defaults to HLS), the channel's name, and which channel group it hangs
//! under — a host is a function of the resource, so two channels, two groups and the two
//! endpoints of one channel all have their own.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::rejection_rule_and_negative_paths::{split_url, Answer, Emulator};

/// The channel groups the rows hang under: a host is derived from the group as well as the
/// channel, so the same channel name in two groups is two hosts.
const GROUPS: [&str; 2] = ["ingest-hosts-one", "ingest-hosts-two"];

/// The label every one of this service's ingest hostnames carries.
const INGEST_LABEL: &str = ".ingest.";

/// The label a channel group's published egress domain carries.
const EGRESS_LABEL: &str = ".egress.";

/// One row: a channel to create and the body it is created with.
struct Row {
    case: &'static str,
    channel: &'static str,
    body: fn() -> Value,
}

/// Rows spanning the input types a channel that receives ingest of its own is created with
/// — the model's `InputType` enum less `MULTIVIEW`, which receives none — plus the absent
/// case the member's doc defaults to HLS.
const ROWS: &[Row] = &[
    Row {
        case: "input type absent (defaults to HLS)",
        channel: "plain",
        body: || json!({}),
    },
    Row {
        case: "HLS input, described",
        channel: "hls-channel",
        body: || json!({"InputType": "HLS", "Description": "ingests HLS"}),
    },
    Row {
        case: "CMAF input",
        channel: "cmaf-channel",
        body: || json!({"InputType": "CMAF"}),
    },
];

/// The `(Id, Url)` pairs a channel response publishes, in the order it reports them.
fn published(case: &str, body: &Value) -> Vec<(String, String)> {
    body.get("IngestEndpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a channel that receives ingest publishes endpoints, got {body}")
        })
        .iter()
        .map(|endpoint| {
            let member = |name: &str| {
                endpoint
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: an ingest endpoint reports {name}, got {endpoint}")
                    })
                    .to_string()
            };
            (member("Id"), member("Url"))
        })
        .collect()
}

/// The DNS a group's data plane is published under: everything past the label that names
/// which half of it a hostname addresses. Both halves of a group's data plane — the egress
/// domain it reports and every ingest host beneath it — share it, so a group stays one DNS
/// name however many endpoints it publishes.
fn group_dns(case: &str, egress_domain: &str) -> String {
    egress_domain
        .split_once(EGRESS_LABEL)
        .unwrap_or_else(|| {
            panic!("case {case:?}: an egress domain carries the egress label, got {egress_domain}")
        })
        .1
        .to_string()
}

/// The `EgressDomain` a group reported.
fn egress_domain(case: &str, body: &Value) -> String {
    body.get("EgressDomain")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: a channel group reports its EgressDomain, got {body}"))
        .to_string()
}

/// Push one small object to a published ingest URL, under a credential, exactly as a
/// contributing encoder does.
async fn push(emulator: &Emulator, url: &str) -> Answer {
    let (host, path) = split_url(url);
    emulator
        .send(
            "PUT",
            &format!("{path}/0.mp4"),
            Some(json!({"segment": "bytes"})),
            &host,
            true,
            &[],
        )
        .await
}

#[tokio::test]
async fn each_ingest_endpoint_of_a_channel_is_published_on_its_own_host() {
    let emulator = Emulator::new();
    // Every ingest host any channel of any group published, so no two endpoints anywhere
    // share one.
    let mut every_host: BTreeSet<String> = BTreeSet::new();

    for group in GROUPS {
        let created_group = emulator.create_group(group, json!({})).await;
        assert_eq!(
            created_group.status, 200,
            "setup: creating group {group} got {} {}",
            created_group.status, created_group.body
        );
        let dns = group_dns(group, &egress_domain(group, &created_group.json));

        for row in ROWS {
            let case = format!("{group}/{}", row.case);
            let created = emulator
                .create_channel(group, row.channel, (row.body)())
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateChannel got {} {}",
                created.status, created.body
            );
            let endpoints = published(&case, &created.json);
            assert!(
                endpoints.len() >= 2,
                "case {case:?}: a channel that receives ingest publishes a redundant pair, got \
                 {endpoints:?}"
            );

            let mut hosts: Vec<String> = Vec::new();
            for (position, (id, url)) in endpoints.iter().enumerate() {
                let (host, path) = split_url(url);
                // The group's own DNS, so a group's data plane stays one DNS name...
                assert!(
                    host.ends_with(&dns),
                    "case {case:?}: endpoint {position} must be published under the group's DNS \
                     {dns:?}, got {url}"
                );
                // ...reached under this service's ingest label...
                assert!(
                    host.contains(INGEST_LABEL),
                    "case {case:?}: endpoint {position} must be published on an ingest host, got \
                     {url}"
                );
                // ...with the endpoint's own ordinal in its first label, which is what makes
                // the two hosts of one channel different hosts.
                let label = host.split('.').next().unwrap_or_default();
                assert!(
                    label.ends_with(&format!("-{id}")),
                    "case {case:?}: endpoint {position} must carry its ordinal {id:?} in its own \
                     label, got {url}"
                );
                assert!(
                    path.contains(&format!("/{}/", row.channel)),
                    "case {case:?}: endpoint {position} must name the channel, got {url}"
                );
                hosts.push(host);
            }

            let distinct: BTreeSet<&String> = hosts.iter().collect();
            assert_eq!(
                distinct.len(),
                hosts.len(),
                "case {case:?}: each ingest endpoint must have its own hostname, got {hosts:?}"
            );
            for host in &hosts {
                assert!(
                    every_host.insert(host.clone()),
                    "case {case:?}: no two ingest endpoints anywhere may share a hostname, got \
                     {host:?} twice"
                );
            }

            // Every reader reports the set the create stored, so no reader hands out a URL
            // another one does not.
            let read = emulator
                .call(
                    "GET",
                    &format!("/channelGroup/{group}/channel/{}/", row.channel),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetChannel got {} {}",
                read.status, read.body
            );
            assert_eq!(
                published(&case, &read.json),
                endpoints,
                "case {case:?}: GetChannel must report the endpoints the create stored, got {}",
                read.body
            );

            // And each published URL is one the ingest data plane serves: a hostname a
            // client cannot push to is not an ingest target.
            for (position, (_id, url)) in endpoints.iter().enumerate() {
                let pushed = push(&emulator, url).await;
                assert_eq!(
                    pushed.status, 200,
                    "case {case:?}: a credentialed push to published endpoint {position} \
                     ({url}) got {} {}",
                    pushed.status, pushed.body
                );
            }
        }
    }
}
