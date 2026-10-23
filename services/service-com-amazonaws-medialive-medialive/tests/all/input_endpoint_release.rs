// SPDX-License-Identifier: CC0-1.0

//! What an input reports once it has been torn down: the ingest endpoints the
//! service ALLOCATED for it are released when its delete is accepted, while the
//! configuration the CALLER supplied goes on being reported.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded `RTMP_PUSH` teardown read is ONE row. The
//! wire harness (`medialive`, `call`) and the clock wait are the crate's shared
//! ones, reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_push_endpoints::destinations;
use crate::wire::{call, medialive};

async fn create_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

async fn describe_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(router, "GET", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput: expected 200, got {status}: {described}"
    );
    described
}

async fn delete_input(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, deleted) = call(router, "DELETE", &format!("/prod/inputs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteInput: expected 200, got {status}: {deleted}"
    );
    deleted
}

fn state(input: &Value) -> String {
    input["state"].as_str().unwrap_or_default().to_string()
}

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// One row of the teardown table: an input of some type, created with some
/// destination list.
struct Row {
    case: &'static str,
    /// The `CreateInput` body for the input under test.
    body: fn() -> Value,
    /// How many destinations that body names, i.e. how many ingest endpoints the
    /// live input reports.
    destinations: usize,
}

/// Rule: a destination is an ingest ENDPOINT the service holds for an input, not
/// configuration echoed back, so an input reports its destinations exactly while
/// it holds them — accepting its delete releases them, so every read of the
/// input after the delete (in the transient `DELETING` state as much as in the
/// terminal `DELETED` one) reports none, while the delete's own response reports
/// the input it deleted, endpoints included. The configuration the caller
/// supplied (type, sources, security groups, tags, …) keeps being reported by
/// the deleted record.
///
/// The rows differ along what the rule generalises over: the input TYPE (so the
/// push protocol whose endpoint is allocated — source: the `InputType` enum,
/// `smithy_query(query="shape", shape="InputType")`), the NUMBER of destinations
/// (none, one, two), whether the destinations name a stream name or their own
/// ON-PREMISES `StaticIpAddress`, and whether the input carries pull sources
/// instead. The issue's recorded case is the single-destination `RTMP_PUSH` row.
#[tokio::test]
async fn a_deleted_input_reports_no_ingest_destinations() {
    let table: [Row; 6] = [
        Row {
            case: "rtmp push with one destination",
            body: || {
                json!({
                    "name": "rtmp-one",
                    "type": "RTMP_PUSH",
                    "destinations": [{ "streamName": "channel/live" }],
                })
            },
            destinations: 1,
        },
        Row {
            case: "rtmp push with two destinations",
            body: || {
                json!({
                    "name": "rtmp-two",
                    "type": "RTMP_PUSH",
                    "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
                })
            },
            destinations: 2,
        },
        Row {
            case: "rtp push with two destinations",
            body: || {
                json!({
                    "name": "rtp-two",
                    "type": "RTP_PUSH",
                    "destinations": [{}, {}],
                })
            },
            destinations: 2,
        },
        Row {
            case: "udp push with one destination",
            body: || {
                json!({
                    "name": "udp-one",
                    "type": "UDP_PUSH",
                    "destinations": [{}],
                })
            },
            destinations: 1,
        },
        Row {
            case: "on-premises rtmp push with a static address",
            body: || {
                json!({
                    "name": "onprem",
                    "type": "RTMP_PUSH",
                    "inputNetworkLocation": "ON_PREMISES",
                    "destinations": [{
                        "streamName": "onprem",
                        "staticIpAddress": "10.0.0.5",
                        "network": "net-1",
                        "networkRoutes": [{ "cidr": "10.0.0.0/24", "gateway": "10.0.0.1" }],
                    }],
                })
            },
            destinations: 1,
        },
        Row {
            // A pull input holds no ingest endpoint at all, so its destinations
            // are empty throughout — and its SOURCES, which the caller supplied,
            // survive the delete.
            case: "url pull with two sources and no destination",
            body: || {
                json!({
                    "name": "pull",
                    "type": "URL_PULL",
                    "sources": [
                        source("https://example.com/a.m3u8"),
                        source("https://example.com/b.m3u8"),
                    ],
                    "tags": { "env": "test" },
                })
            },
            destinations: 0,
        },
    ];

    for Row {
        case,
        body,
        destinations: expected,
    } in table
    {
        let router = medialive();
        let created = create_input(&router, case, body()).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        // Live: the endpoints the create allocated are reported, by the create
        // and by every read of the input.
        assert_eq!(
            destinations(&created).len(),
            expected,
            "case {case:?}: CreateInput: expected one destination per requested destination, got {:?}",
            destinations(&created)
        );
        let live = describe_input(&router, case, &id).await;
        assert_eq!(
            state(&live),
            "DETACHED",
            "case {case:?}: expected a live input before the delete, got {live}"
        );
        assert_eq!(
            destinations(&live),
            destinations(&created),
            "case {case:?}: DescribeInput: expected the allocated destinations, got {:?}",
            destinations(&live)
        );

        // The delete starts the teardown: it reports the input it deleted — the
        // endpoints it is taking down included — in the transient `DELETING`
        // state.
        let deleted = delete_input(&router, case, &id).await;
        assert_eq!(
            state(&deleted),
            "DELETING",
            "case {case:?}: expected the delete to report the transient state it starts, got {deleted}"
        );
        assert_eq!(
            destinations(&deleted),
            destinations(&live),
            "case {case:?}: DeleteInput: expected the delete to report the endpoints it released, got {:?}",
            destinations(&deleted)
        );

        // From the delete on, the endpoints are released: a read inside the
        // transient window reports none of them, whichever side of the clock it
        // lands on.
        let in_flight = describe_input(&router, case, &id).await;
        assert!(
            ["DELETING", "DELETED"].contains(&state(&in_flight).as_str()),
            "case {case:?}: expected a deleted input to be DELETING or DELETED, got {in_flight}"
        );
        assert!(
            destinations(&in_flight).is_empty(),
            "case {case:?}: expected an input whose delete was accepted to report no destinations, got {:?}",
            destinations(&in_flight)
        );

        // Terminal: the deletion has come due, the endpoints are released, and
        // the teardown read reports none of them.
        wait_for_the_clock();
        let torn_down = describe_input(&router, case, &id).await;
        assert_eq!(
            state(&torn_down),
            "DELETED",
            "case {case:?}: expected the deletion to have settled into the terminal DELETED, got {torn_down}"
        );
        assert!(
            destinations(&torn_down).is_empty(),
            "case {case:?}: expected a torn-down input to report no destinations, got {:?}",
            destinations(&torn_down)
        );

        // The released endpoints are the ONLY thing the teardown took away: the
        // record the caller configured still reports itself.
        for member in [
            "arn",
            "id",
            "name",
            "type",
            "inputClass",
            "inputNetworkLocation",
            "inputSourceType",
            "sources",
            "securityGroups",
            "mediaConnectFlows",
            "inputDevices",
            "tags",
        ] {
            assert_eq!(
                torn_down[member], live[member],
                "case {case:?}: expected the deleted input to keep reporting {member} as {:?}, got {:?}",
                live[member], torn_down[member]
            );
        }

        // Reading a torn-down input repeatedly answers the same thing: the
        // release was the delete's, not the reader's.
        let again = describe_input(&router, case, &id).await;
        assert_eq!(
            again, torn_down,
            "case {case:?}: expected repeated teardown reads to agree, got {again}"
        );

        // A repeated delete has nothing left to release, so it reports no
        // destinations either — the endpoints went with the first one.
        let repeated = delete_input(&router, case, &id).await;
        assert_eq!(
            state(&repeated),
            "DELETED",
            "case {case:?}: expected a repeated delete to report the terminal DELETED, got {repeated}"
        );
        assert!(
            destinations(&repeated).is_empty(),
            "case {case:?}: expected a repeated delete to have no endpoints left to report, got {:?}",
            destinations(&repeated)
        );
    }
}
