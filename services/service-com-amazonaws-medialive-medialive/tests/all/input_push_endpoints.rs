// SPDX-License-Identifier: CC0-1.0

//! A PUSH-type input's ingest endpoint and every input's
//! `Smpte2110ReceiverGroupSettings`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded `RTMP_PUSH` case is ONE row of each.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, medialive};

async fn create_input(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

async fn describe_input(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, response) = call(router, "GET", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeInput: expected 200, got {status}: {response}"
    );
    response
}

async fn update_input(router: &MultiServiceRouter, id: &str, body: Value) -> Value {
    let (status, response) = call(router, "PUT", &format!("/prod/inputs/{id}"), Some(body)).await;
    assert_eq!(
        status, 200,
        "UpdateInput: expected 200, got {status}: {response}"
    );
    response["input"].clone()
}

/// The destinations of one `Input`-shaped body.
fn destinations(input: &Value) -> Vec<Value> {
    input["destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the push-endpoint table: the case, the `InputType`, the
/// destinations the create request names, and — per destination — the `port` and
/// the `url` the input must then report (`None` for an input type the service
/// allocates no push endpoint for), with `{ip}` standing for the address the
/// service allocated for that destination.
type EndpointRow = (
    &'static str,
    &'static str,
    Value,
    Vec<(Option<&'static str>, Option<&'static str>)>,
);

/// Rule: an input whose type is pushed to reports, on each of its destinations,
/// the ingest endpoint the service allocated for it — a system-generated IPv4
/// address, the port that type's push protocol listens on, and the
/// `<scheme>://<address>:<port>` URL the upstream system pushes to (with the
/// destination's stream name as the path for the protocol that names one) —
/// while an input of a type with no push protocol reports neither a port nor a
/// URL; every destination reports its route list, empty rather than unset.
///
/// The table spans the push protocols the service allocates an endpoint for
/// (source: the `InputType` enum, `smithy_query(query="shape",
/// shape="InputType")`, and the push ports MediaLive opens — RTMP's 1935, 5000
/// for RTP and UDP), at one and at two destinations, with and without a stream
/// name, beside a non-push type and an ON-PREMISES destination that names its
/// own `StaticIpAddress`. The issue's recorded case is the single RTMP row.
#[tokio::test]
async fn a_push_input_reports_the_ingest_endpoint_of_its_protocol() {
    let table: [EndpointRow; 7] = [
        (
            "rtmp push, one destination with a stream name",
            "RTMP_PUSH",
            json!([{ "streamName": "channel/live" }]),
            vec![(Some("1935"), Some("rtmp://{ip}:1935/channel/live"))],
        ),
        (
            "rtmp push, two destinations",
            "RTMP_PUSH",
            json!([{ "streamName": "live/a" }, { "streamName": "live/b" }]),
            vec![
                (Some("1935"), Some("rtmp://{ip}:1935/live/a")),
                (Some("1935"), Some("rtmp://{ip}:1935/live/b")),
            ],
        ),
        (
            // No stream name: the URL is the endpoint alone, with no path.
            "rtmp push with no stream name",
            "RTMP_PUSH",
            json!([{}]),
            vec![(Some("1935"), Some("rtmp://{ip}:1935"))],
        ),
        (
            // `StreamName` is documented as the location the RTMP stream is
            // pushed to, so the RTP endpoint carries no path even when the
            // request names one.
            "rtp push, two destinations",
            "RTP_PUSH",
            json!([{ "streamName": "ignored-by-rtp" }, {}]),
            vec![
                (Some("5000"), Some("rtp://{ip}:5000")),
                (Some("5000"), Some("rtp://{ip}:5000")),
            ],
        ),
        (
            "udp push, one destination",
            "UDP_PUSH",
            json!([{}]),
            vec![(Some("5000"), Some("udp://{ip}:5000"))],
        ),
        (
            // An ON-PREM push input's address is the one the request named, and
            // the endpoint URL is built on it.
            "on-premises rtmp push with a static address",
            "RTMP_PUSH",
            json!([{
                "streamName": "onprem",
                "staticIpAddress": "10.0.0.5",
                "network": "net-1",
                "networkRoutes": [{ "cidr": "10.0.0.0/24", "gateway": "10.0.0.1" }],
            }]),
            vec![(Some("1935"), Some("rtmp://10.0.0.5:1935/onprem"))],
        ),
        (
            // A type with no push ingest protocol: nothing is invented for it.
            "url pull with a destination",
            "URL_PULL",
            json!([{ "streamName": "not-pushed" }]),
            vec![(None, None)],
        ),
    ];

    for (case, input_type, requested, expected) in table {
        let router = medialive();
        let mut body = json!({ "name": case, "type": input_type });
        body["destinations"] = requested.clone();
        if input_type == "RTMP_PUSH" && requested[0].get("network").is_some() {
            body["inputNetworkLocation"] = json!("ON_PREMISES");
        }
        let created = create_input(&router, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let described = describe_input(&router, &id).await;
        // The echoed update body is the same projection, so it must agree.
        let updated = update_input(&router, &id, json!({ "name": "renamed" })).await;

        for (reported, source) in [
            (&created, "CreateInput"),
            (&described, "DescribeInput"),
            (&updated, "UpdateInput"),
        ] {
            let reported = destinations(reported);
            assert_eq!(
                reported.len(),
                expected.len(),
                "case {case:?}: {source}: expected one destination per requested destination, got {reported:?}"
            );
            for (index, (destination, (expected_port, expected_url))) in
                reported.iter().zip(expected.iter()).enumerate()
            {
                let requested_destination = &requested[index];
                let ip = destination["ip"].as_str().unwrap_or_default().to_string();
                if let Some(static_address) = requested_destination["staticIpAddress"].as_str() {
                    assert_eq!(
                        ip, static_address,
                        "case {case:?}: {source}: destination {index}: expected the requested static address, got {destination:?}"
                    );
                } else if expected_port.is_some() {
                    assert!(
                        ip.parse::<Ipv4Addr>().is_ok(),
                        "case {case:?}: {source}: destination {index}: expected a system-generated IPv4 ingest address, got {destination:?}"
                    );
                } else {
                    // No push protocol and no requested address: nothing to
                    // report, and nothing is invented.
                    assert_eq!(
                        destination["ip"],
                        Value::Null,
                        "case {case:?}: {source}: destination {index}: expected no ingest address, got {destination:?}"
                    );
                }
                assert_eq!(
                    destination["port"],
                    expected_port.map_or(Value::Null, |port| json!(port)),
                    "case {case:?}: {source}: destination {index}: expected port {expected_port:?}, got {destination:?}"
                );
                assert_eq!(
                    destination["url"],
                    expected_url.map_or(Value::Null, |url| json!(url.replace("{ip}", &ip))),
                    "case {case:?}: {source}: destination {index}: expected url {expected_url:?}, got {destination:?}"
                );
                assert_eq!(
                    destination["network"], requested_destination["network"],
                    "case {case:?}: {source}: destination {index}: expected the requested network, got {destination:?}"
                );
                assert_eq!(
                    destination["networkRoutes"],
                    requested_destination
                        .get("networkRoutes")
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                    "case {case:?}: {source}: destination {index}: expected the requested routes (empty when none), got {destination:?}"
                );
            }
        }
    }
}

/// Rule: every allocated ingest address is distinct and fixed for the lifetime
/// of the input — an update that rewrites the destinations keeps the address of
/// each slot that already had one and allocates one for each slot it adds.
#[tokio::test]
async fn an_ingest_address_is_distinct_and_fixed_for_the_lifetime_of_the_input() {
    let router = medialive();
    let first = create_input(
        &router,
        json!({
            "name": "fixed",
            "type": "RTMP_PUSH",
            "destinations": [{ "streamName": "a" }, { "streamName": "b" }],
        }),
    )
    .await;
    let id = first["id"].as_str().unwrap_or_default().to_string();

    let addresses = |input: &Value| -> Vec<String> {
        destinations(input)
            .iter()
            .map(|destination| destination["ip"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let created = addresses(&first);
    assert_eq!(
        created.len(),
        2,
        "case \"two destinations\": expected an address per destination, got {created:?}"
    );
    assert_eq!(
        created.iter().collect::<BTreeSet<_>>().len(),
        2,
        "case \"two destinations\": expected distinct addresses, got {created:?}"
    );

    // Reading the input does not move its endpoints.
    for read in 1..=2 {
        let described = addresses(&describe_input(&router, &id).await);
        assert_eq!(
            described, created,
            "read {read}: expected the same ingest addresses, got {described:?}"
        );
    }

    // Renaming the streams keeps every address and rebuilds each URL on it.
    let renamed = update_input(
        &router,
        &id,
        json!({ "destinations": [{ "streamName": "c" }, { "streamName": "d" }] }),
    )
    .await;
    assert_eq!(
        addresses(&renamed), created,
        "case \"streams renamed\": expected the addresses to stay fixed, got {:?}",
        destinations(&renamed)
    );
    assert_eq!(
        destinations(&renamed)[0]["url"],
        json!(format!("rtmp://{}:1935/c", created[0])),
        "case \"streams renamed\": expected the URL rebuilt on the fixed address, got {:?}",
        destinations(&renamed)[0]
    );

    // A third destination gets an address of its own; the first two keep theirs.
    let grown = update_input(
        &router,
        &id,
        json!({ "destinations": [{ "streamName": "c" }, { "streamName": "d" }, { "streamName": "e" }] }),
    )
    .await;
    let grown_addresses = addresses(&grown);
    assert_eq!(
        grown_addresses[..2],
        created[..],
        "case \"a destination added\": expected the existing addresses to stay fixed, got {grown_addresses:?}"
    );
    assert_eq!(
        grown_addresses.iter().collect::<BTreeSet<_>>().len(),
        3,
        "case \"a destination added\": expected distinct addresses, got {grown_addresses:?}"
    );

    // A second input's endpoint is its own.
    let second = create_input(
        &router,
        json!({
            "name": "other",
            "type": "UDP_PUSH",
            "destinations": [{}],
        }),
    )
    .await;
    let second_addresses = addresses(&second);
    assert!(
        second_addresses
            .iter()
            .all(|address| !grown_addresses.contains(address)),
        "case \"a second input\": expected addresses the first input does not hold, got {second_addresses:?} beside {grown_addresses:?}"
    );

    // Deleting the input does not take its endpoints away: the deleted input
    // stays readable, endpoints and all.
    let (status, deleted) = call(&router, "DELETE", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case \"deleted\": DeleteInput: expected 200, got {status}: {deleted}"
    );
    let after_delete = addresses(&describe_input(&router, &id).await);
    assert_eq!(
        after_delete, grown_addresses,
        "case \"deleted\": expected the deleted input to keep reporting its ingest addresses, got {after_delete:?}"
    );
}

/// Rule: an ingest address belongs to a destination SLOT of the input, so an
/// update that shrinks the destination list and a later one that grows it again
/// put the same addresses back on the same slots — the endpoint is a property of
/// the input's ingest position, not of the stream name that happens to occupy
/// it, and no new address is spent re-announcing one the input already had.
#[tokio::test]
async fn a_destination_slot_keeps_its_address_across_a_shrink_and_a_regrow() {
    let router = medialive();
    let created = create_input(
        &router,
        json!({
            "name": "slots",
            "type": "RTP_PUSH",
            "destinations": [{}, {}],
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    let addresses = |input: &Value| -> Vec<String> {
        destinations(input)
            .iter()
            .map(|destination| destination["ip"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    let both = addresses(&created);

    let shrunk = addresses(&update_input(&router, &id, json!({ "destinations": [{}] })).await);
    assert_eq!(
        shrunk,
        both[..1],
        "case \"shrunk to one\": expected the first slot's address, got {shrunk:?}"
    );

    let regrown = addresses(&update_input(&router, &id, json!({ "destinations": [{}, {}] })).await);
    assert_eq!(
        regrown, both,
        "case \"regrown to two\": expected each slot to report the address it already had, got {regrown:?}"
    );
}

/// Rule: every `Input` a read or a write answers with carries
/// `Smpte2110ReceiverGroupSettings`, whose receiver-group list holds the groups
/// the input was configured with and is empty — not unset — when it has none.
///
/// The table spans the receiver-group configurations across input types: none at
/// all, an explicitly empty list, one group and two groups (source:
/// `Smpte2110ReceiverGroupSettings`, `smithy_query(query="shape",
/// shape="Smpte2110ReceiverGroupSettings")`). The issue's recorded cases are the
/// two unconfigured rows.
#[tokio::test]
async fn every_input_reports_its_receiver_group_settings() {
    fn group(sdp_url: &str, media_index: i64) -> Value {
        json!({ "sdpSettings": { "videoSdp": { "sdpUrl": sdp_url, "mediaIndex": media_index } } })
    }

    // (case, the create body's type, the requested settings, the groups the
    // input must then report)
    let table: [(&str, &str, Option<Value>, Value); 5] = [
        ("url pull, unconfigured", "URL_PULL", None, json!([])),
        ("rtmp push, unconfigured", "RTMP_PUSH", None, json!([])),
        (
            "an explicitly empty group list",
            "SMPTE_2110_RECEIVER_GROUP",
            Some(json!({ "smpte2110ReceiverGroups": [] })),
            json!([]),
        ),
        (
            "one receiver group",
            "SMPTE_2110_RECEIVER_GROUP",
            Some(json!({ "smpte2110ReceiverGroups": [group("s3://bucket/one.sdp", 1)] })),
            json!([group("s3://bucket/one.sdp", 1)]),
        ),
        (
            "two receiver groups",
            "SMPTE_2110_RECEIVER_GROUP",
            Some(json!({ "smpte2110ReceiverGroups": [
                group("s3://bucket/a.sdp", 2),
                group("s3://bucket/b.sdp", 3),
            ] })),
            json!([group("s3://bucket/a.sdp", 2), group("s3://bucket/b.sdp", 3)]),
        ),
    ];

    for (case, input_type, requested, expected_groups) in table {
        let router = medialive();
        let mut body = json!({ "name": case, "type": input_type });
        if let Some(requested) = requested {
            body["smpte2110ReceiverGroupSettings"] = requested;
        }
        let created = create_input(&router, body).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let described = describe_input(&router, &id).await;
        let updated = update_input(&router, &id, json!({ "name": "renamed" })).await;

        for (reported, source) in [
            (&created, "CreateInput"),
            (&described, "DescribeInput"),
            (&updated, "UpdateInput"),
        ] {
            let settings = &reported["smpte2110ReceiverGroupSettings"];
            assert!(
                settings.is_object(),
                "case {case:?}: {source}: expected the receiver-group settings structure, got {settings:?}"
            );
            assert_eq!(
                settings["smpte2110ReceiverGroups"], expected_groups,
                "case {case:?}: {source}: expected the receiver groups {expected_groups:?}, got {settings:?}"
            );
        }
    }
}
