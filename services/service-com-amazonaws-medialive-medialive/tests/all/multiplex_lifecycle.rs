// SPDX-License-Identifier: CC0-1.0

//! The MediaLive multiplex lifecycle, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MULTIPLEX_AVAILABILITY_ZONES;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, medialive, ACCOUNT};

/// The collection URI `CreateMultiplex` is bound to
/// (`smithy_query(query="operation", operation="CreateMultiplex")`:
/// `POST /prod/multiplexes`). Shared with the multiplex PROGRAM suite, which
/// builds its own URIs under one multiplex of this collection.
pub(crate) const MULTIPLEXES: &str = "/prod/multiplexes";

/// The published bounds of `MultiplexSettings$TransportStreamBitrate`, from its
/// target shape `__integerMin1000000Max100000000`
/// (`smithy_query(query="shape", shape="MultiplexSettings")`).
const TRANSPORT_STREAM_BITRATE: (i64, i64) = (1_000_000, 100_000_000);

/// The published bounds of `MultiplexSettings$TransportStreamId`, from its
/// target shape `__integerMin0Max65535`.
const TRANSPORT_STREAM_ID: (i64, i64) = (0, 65_535);

/// The published bounds of
/// `MultiplexSettings$MaximumVideoBufferDelayMilliseconds`, from its target
/// shape `__integerMin800Max3000`.
const VIDEO_BUFFER_DELAY: (i64, i64) = (800, 3_000);

/// The published bounds of `MultiplexSettings$TransportStreamReservedBitrate`,
/// from its target shape `__integerMin0Max100000000`.
const TRANSPORT_STREAM_RESERVED_BITRATE: (i64, i64) = (0, 100_000_000);

/// Every ranged member of `MultiplexSettings`, by wire name, with the inclusive
/// bounds the model publishes for it — the whole class the range rule covers.
const RANGED_SETTINGS: [(&str, (i64, i64)); 4] = [
    ("transportStreamBitrate", TRANSPORT_STREAM_BITRATE),
    ("transportStreamId", TRANSPORT_STREAM_ID),
    ("maximumVideoBufferDelayMilliseconds", VIDEO_BUFFER_DELAY),
    (
        "transportStreamReservedBitrate",
        TRANSPORT_STREAM_RESERVED_BITRATE,
    ),
];

/// The two members `MultiplexSettings` requires, at the low end of their
/// published ranges: the row's own values are what each test varies.
pub(crate) fn settings(bitrate: i64, transport_stream_id: i64) -> Value {
    json!({
        "transportStreamBitrate": bitrate,
        "transportStreamId": transport_stream_id,
    })
}

/// A `CreateMultiplex` body carrying the two availability zones the member
/// documents, so a test that is not about the zone count does not have to spell
/// them.
pub(crate) fn create_body(name: &str, request_id: &str, multiplex_settings: Value) -> Value {
    json!({
        "name": name,
        "requestId": request_id,
        "availabilityZones": ["us-west-2a", "us-west-2c"],
        "multiplexSettings": multiplex_settings,
    })
}

/// One row of the create-and-read-back table: the case, the multiplex's name,
/// the two availability zones, the `MultiplexSettings` and the `Tags` the
/// request carries, if any.
type CreateRow = (
    &'static str,
    &'static str,
    [&'static str; 2],
    Value,
    Option<Value>,
);

/// `CreateMultiplex`: the status and, on success, the multiplex it answers with.
async fn create_multiplex(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "POST", MULTIPLEXES, Some(body)).await;
    if status == 201 {
        (status, response["multiplex"].clone())
    } else {
        (status, response)
    }
}

/// A created multiplex, with the create itself asserted to have succeeded.
pub(crate) async fn created(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, multiplex) = create_multiplex(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateMultiplex: expected 201, got {status}: {multiplex}"
    );
    multiplex
}

/// `DescribeMultiplex`: its status and body. The description is the response
/// itself, not a wrapper member.
pub(crate) async fn describe_multiplex(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{MULTIPLEXES}/{id}"), None).await
}

/// Rule: `CreateMultiplex` mints a numeric `Id`, builds the `Arn` from the
/// region the request was signed for and that id, stores the `Name`, the
/// `AvailabilityZones` in the order the request listed them, the
/// `MultiplexSettings` and the `Tags` verbatim, and answers with the multiplex
/// `CREATING` — no program in it and no pipeline running — which settles to
/// `IDLE`; `DescribeMultiplex` reads that same stored multiplex back.
///
/// The rows differ along the settings members the request names and their values
/// at each published bound, along the zone pair and its order, and along whether
/// tags are supplied — which is what the rule generalises over.
#[tokio::test]
async fn a_created_multiplex_is_read_back_by_describe() {
    // (case, name, availability zones, multiplex settings, tags)
    let rows: [CreateRow; 6] = [
        (
            "required settings only",
            "sports-feed",
            ["us-west-2a", "us-west-2c"],
            settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0),
            None,
        ),
        (
            "settings at their upper bounds",
            "high-bitrate",
            ["us-west-2b", "us-west-2d"],
            settings(TRANSPORT_STREAM_BITRATE.1, TRANSPORT_STREAM_ID.1),
            None,
        ),
        (
            "zones in the other order",
            "reversed-zones",
            ["us-west-2c", "us-west-2a"],
            settings(20_000_000, 1),
            None,
        ),
        (
            "every optional settings member, at its lower bound",
            "buffered",
            ["us-west-2a", "us-west-2b"],
            json!({
                "transportStreamBitrate": 5_000_000,
                "transportStreamId": 42,
                "maximumVideoBufferDelayMilliseconds": VIDEO_BUFFER_DELAY.0,
                "transportStreamReservedBitrate": TRANSPORT_STREAM_RESERVED_BITRATE.0,
            }),
            None,
        ),
        (
            "every optional settings member, at its upper bound",
            "deeply-buffered",
            ["us-west-2a", "us-west-2b"],
            json!({
                "transportStreamBitrate": TRANSPORT_STREAM_BITRATE.1,
                "transportStreamId": 7,
                "maximumVideoBufferDelayMilliseconds": VIDEO_BUFFER_DELAY.1,
                "transportStreamReservedBitrate": TRANSPORT_STREAM_RESERVED_BITRATE.1,
            }),
            None,
        ),
        (
            "tagged",
            "tagged-multiplex",
            ["us-west-2a", "us-west-2c"],
            settings(3_000_000, 300),
            Some(json!({ "env": "test", "team": "playout" })),
        ),
    ];

    for (case, name, zones, multiplex_settings, tags) in rows {
        let router = medialive();
        let mut request = json!({
            "name": name,
            "requestId": format!("token-{name}"),
            "availabilityZones": zones,
            "multiplexSettings": multiplex_settings.clone(),
        });
        if let Some(tags) = tags.clone() {
            request["tags"] = tags;
        }

        let multiplex = created(&router, case, request).await;
        let id = multiplex["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric multiplex id, got {:?}",
            multiplex["id"]
        );
        assert_eq!(
            multiplex["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:multiplex:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            multiplex["arn"]
        );
        assert_eq!(
            multiplex["name"],
            json!(name),
            "case {case:?}: expected the requested name, got {:?}",
            multiplex["name"]
        );
        assert_eq!(
            multiplex["availabilityZones"],
            json!(zones),
            "case {case:?}: expected the zones in the order the request listed them, got {:?}",
            multiplex["availabilityZones"]
        );
        assert_eq!(
            multiplex["multiplexSettings"], multiplex_settings,
            "case {case:?}: expected the settings the request supplied, got {:?}",
            multiplex["multiplexSettings"]
        );
        assert_eq!(
            multiplex["tags"],
            tags.clone().unwrap_or_else(|| json!({})),
            "case {case:?}: expected the tags the request supplied, got {:?}",
            multiplex["tags"]
        );
        assert_eq!(
            multiplex["state"],
            json!("CREATING"),
            "case {case:?}: expected a new multiplex to be CREATING, got {:?}",
            multiplex["state"]
        );
        assert_eq!(
            multiplex["programCount"],
            json!(0),
            "case {case:?}: expected no programs in a new multiplex, got {:?}",
            multiplex["programCount"]
        );
        assert_eq!(
            multiplex["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipeline running on a CREATING multiplex, got {:?}",
            multiplex["pipelinesRunningCount"]
        );

        // The round trip: the reader answers with the stored multiplex, member
        // for member, not with anything derived from its own request. `State` is
        // the one member the clock may have moved on by now.
        let (status, described) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
        );
        let mut expected = multiplex.clone();
        expected["state"] = described["state"].clone();
        assert_eq!(
            described, expected,
            "case {case:?}: expected DescribeMultiplex to report the created multiplex, got {described}"
        );
        assert!(
            matches!(described["state"].as_str(), Some("CREATING" | "IDLE")),
            "case {case:?}: expected a multiplex creating or settled, got {:?}",
            described["state"]
        );

        // The transition the clock owes: `CREATING` settles to `IDLE`, and
        // nothing else about the multiplex changes with it.
        wait_for_the_clock();
        let (status, settled) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the clock: expected 200, got {status}: {settled}"
        );
        assert_eq!(
            settled["state"],
            json!("IDLE"),
            "case {case:?}: expected a created multiplex to settle to IDLE, got {:?}",
            settled["state"]
        );
        let mut expected = multiplex.clone();
        expected["state"] = json!("IDLE");
        assert_eq!(
            settled, expected,
            "case {case:?}: expected only the state to have moved, got {settled}"
        );
    }
}

/// Rule: `CreateMultiplex` stores a multiplex only when `AvailabilityZones`
/// names exactly the number of zones its documentation requires ("You must
/// specify exactly two"); any other count is refused as a bad request and mints
/// nothing.
///
/// The rows are every count from none up to one past the required one — the
/// bound itself and each side of it.
#[tokio::test]
async fn a_multiplex_is_created_in_exactly_two_availability_zones() {
    let zone_pool = ["us-west-2a", "us-west-2b", "us-west-2c", "us-west-2d"];

    for count in 0..=zone_pool.len() {
        let case = format!("{count} zone(s)");
        let router = medialive();
        let zones: Vec<&str> = zone_pool.iter().copied().take(count).collect();
        let mut request = create_body(
            "zoned",
            &format!("token-{count}"),
            settings(TRANSPORT_STREAM_BITRATE.0, 1),
        );
        request["availabilityZones"] = json!(zones);

        let (status, body) = create_multiplex(&router, request).await;
        if count == MULTIPLEX_AVAILABILITY_ZONES {
            assert_eq!(
                status, 201,
                "case {case:?}: expected the documented zone count to be accepted, got {status}: {body}"
            );
            assert_eq!(
                body["availabilityZones"],
                json!(zones),
                "case {case:?}: expected the zones the request listed, got {:?}",
                body["availabilityZones"]
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected any other zone count to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        // The refusal says what was wrong with the request: the member, the
        // count it requires and the count the request carried.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("availabilityZones")
                && message.contains(&MULTIPLEX_AVAILABILITY_ZONES.to_string())
                && message.contains(&count.to_string()),
            "case {case:?}: expected the refusal to name the member and both counts, got {body}"
        );
        // And it minted nothing: the first id this emulator would have handed
        // out names no multiplex.
        let (status, described) = describe_multiplex(&router, "6000000").await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused create to mint nothing, got {status}: {described}"
        );
    }
}

/// Rule: a create retried with the `RequestId` (`@idempotencyToken`) of an
/// earlier one answers with the multiplex that one created; two creates that do
/// not share a token create two multiplexes.
#[tokio::test]
async fn a_retried_create_returns_the_multiplex_the_first_one_created() {
    // (case, first RequestId, second RequestId, whether both calls share one multiplex)
    let rows: [(&str, &str, &str, bool); 3] = [
        ("the same token", "token-a", "token-a", true),
        ("two tokens", "token-a", "token-b", false),
        ("the second token is a prefix of the first", "token-ab", "token-a", false),
    ];

    for (case, first_token, second_token, shared) in rows {
        let router = medialive();
        let one = created(
            &router,
            case,
            create_body("first", first_token, settings(2_000_000, 1)),
        )
        .await;
        let two = created(
            &router,
            case,
            create_body("second", second_token, settings(4_000_000, 2)),
        )
        .await;

        if shared {
            assert_eq!(
                two, one,
                "case {case:?}: expected the retry to answer with the first multiplex, got {two}"
            );
            continue;
        }

        assert_ne!(
            two["id"], one["id"],
            "case {case:?}: expected two distinct multiplexes, got {:?} twice",
            one["id"]
        );
        // Each multiplex carries its own request's values, and both are stored.
        for (label, expected) in [("first", &one), ("second", &two)] {
            let id = expected["id"].as_str().unwrap_or_default();
            let (status, described) = describe_multiplex(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{label}: DescribeMultiplex: expected 200, got {status}: {described}"
            );
            assert_eq!(
                described["name"],
                json!(label),
                "case {case:?}/{label}: expected the multiplex to carry its own name, got {:?}",
                described["name"]
            );
        }
    }
}

/// Rule: a `MultiplexSettings` member outside the range the model publishes for
/// it is refused as a client error and stores nothing, and a value on either
/// published bound is accepted.
///
/// The rows walk EVERY ranged member of the structure to each of its published
/// bounds and one step past each — the ranges in [`RANGED_SETTINGS`].
#[tokio::test]
async fn multiplex_settings_outside_their_published_range_are_refused() {
    for (member, (min, max)) in RANGED_SETTINGS {
        // (what the row does to the member, its value, the expected status)
        for (variant, value, expected) in [
            ("at the minimum", min, 201u16),
            ("below the minimum", min - 1, 400),
            ("at the maximum", max, 201),
            ("past the maximum", max + 1, 400),
        ] {
            let case = format!("{member} {variant} ({value})");
            let router = medialive();
            // Only the member under test leaves its range: the two required
            // members stay on a value well inside theirs.
            let mut multiplex_settings = settings(TRANSPORT_STREAM_BITRATE.0, TRANSPORT_STREAM_ID.0);
            multiplex_settings[member] = json!(value);

            let (status, body) = create_multiplex(
                &router,
                create_body("ranged", "token-ranged", multiplex_settings.clone()),
            )
            .await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected {expected}, got {status}: {body}"
            );

            if expected == 201 {
                assert_eq!(
                    body["multiplexSettings"], multiplex_settings,
                    "case {case:?}: expected the settings the request supplied, got {:?}",
                    body["multiplexSettings"]
                );
                continue;
            }

            let code = error_code(status, &body);
            assert!(
                code == "BadRequestException" || code == "ValidationException",
                "case {case:?}: expected the request to be refused as a client error, got {code} ({body})"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(member) && message.contains(&value.to_string()),
                "case {case:?}: expected the refusal to name the member and the value, got {body}"
            );
            let (status, described) = describe_multiplex(&router, "6000000").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the refused create to mint nothing, got {status}: {described}"
            );
        }
    }
}

/// Rule: a `MultiplexId` that names no stored multiplex is a `NotFoundException`,
/// including the id of another MediaLive resource — a multiplex id is only ever
/// one this service minted for a multiplex.
#[tokio::test]
async fn describing_a_multiplex_that_does_not_exist_is_a_not_found() {
    let router = medialive();
    let live = created(
        &router,
        "neighbour",
        create_body("kept", "token-kept", settings(2_000_000, 1)),
    )
    .await;
    let live_id = live["id"].as_str().unwrap_or_default().to_string();

    // A channel exists too, so a request that passes ITS id to the multiplex
    // reader is refused rather than answered from the wrong store.
    let (status, response) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "not-a-multiplex" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateChannel: expected 201, got {status}: {response}"
    );
    let channel_id = response["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, id) in [
        ("never minted", "9999999".to_string()),
        ("a channel's id", channel_id),
        ("not a number at all", "no-such-multiplex".to_string()),
    ] {
        let (status, body) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!("Multiplex {id} not found")),
            "case {case:?}: expected the refusal to name the id asked for, got {body}"
        );
    }

    // And none of those refusals touched the multiplex that does exist.
    let (status, described) = describe_multiplex(&router, &live_id).await;
    assert_eq!(
        status, 200,
        "DescribeMultiplex: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described["id"], live["id"],
        "expected the stored multiplex to survive, got {described}"
    );
}
