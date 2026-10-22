// SPDX-License-Identifier: CC0-1.0

//! The MediaLive input lifecycle, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded `URL_PULL` case is ONE row of each.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_INPUTS;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, medialive, ACCOUNT};

async fn create_input(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

async fn describe_input(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/inputs/{id}"), None).await
}

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// Rule: `CreateInput` mints a numeric `Id`, builds the `Arn` from the region the
/// request was signed for and that id, stores every member the request supplied,
/// answers with the input already `DETACHED` (attached to no channel), and
/// derives `InputClass` from how many ingest endpoints the request named (one
/// endpoint is `SINGLE_PIPELINE`, otherwise the two-source `STANDARD` class) and
/// `InputSourceType` from whether a dynamic-capable type's source URL carries the
/// `$urlPath$` variable.
///
/// The table spans the per-pipeline list each `InputType` family ingests through
/// (`smithy_query(query="operation", operation="CreateInput")`: Sources,
/// Destinations, MediaConnectFlows, InputDevices, SdiSources) at one and at two
/// endpoints, plus the dynamic-URL rows, which the issue's recorded `URL_PULL`
/// case does not cover.
#[tokio::test]
async fn a_created_input_reports_the_class_and_source_type_its_endpoints_give_it() {
    // (case, request body, expected InputClass, expected InputSourceType)
    let table: [(&str, Value, &str, &str); 9] = [
        (
            "url pull with two sources",
            json!({
                "name": "two-source-pull",
                "type": "URL_PULL",
                "sources": [source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
                "inputSecurityGroups": ["3456789"],
            }),
            "STANDARD",
            "STATIC",
        ),
        (
            "url pull with one source",
            json!({
                "name": "one-source-pull",
                "type": "URL_PULL",
                "sources": [source("https://example.com/only.m3u8")],
            }),
            "SINGLE_PIPELINE",
            "STATIC",
        ),
        (
            "mp4 file with a dynamic url",
            json!({
                "name": "dynamic-mp4",
                "type": "MP4_FILE",
                "sources": [source("s3://bucket/$urlPath$")],
            }),
            "SINGLE_PIPELINE",
            "DYNAMIC",
        ),
        (
            "ts file with two dynamic urls",
            json!({
                "name": "dynamic-ts",
                "type": "TS_FILE",
                "sources": [source("s3://bucket/a/$urlPath$"), source("s3://bucket/b/$urlPath$")],
            }),
            "STANDARD",
            "DYNAMIC",
        ),
        (
            // The variable alone is not enough: the type must be one the model's
            // `InputSourceType` doc names (MP4_FILE, TS_FILE).
            "url pull with a dynamic url",
            json!({
                "name": "pull-with-variable",
                "type": "URL_PULL",
                "sources": [source("https://example.com/$urlPath$")],
            }),
            "SINGLE_PIPELINE",
            "STATIC",
        ),
        (
            "rtmp push with two destinations",
            json!({
                "name": "two-destination-push",
                "type": "RTMP_PUSH",
                "destinations": [{ "streamName": "live/a" }, { "streamName": "live/b" }],
            }),
            "STANDARD",
            "STATIC",
        ),
        (
            "mediaconnect with two flows",
            json!({
                "name": "two-flow",
                "type": "MEDIACONNECT",
                "mediaConnectFlows": [
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                    { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:2:b" },
                ],
                "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
            }),
            "STANDARD",
            "STATIC",
        ),
        (
            "one input device",
            json!({
                "name": "one-device",
                "type": "INPUT_DEVICE",
                "inputDevices": [{ "id": "hd-0000000" }],
            }),
            "SINGLE_PIPELINE",
            "STATIC",
        ),
        (
            "two sdi sources",
            json!({
                "name": "two-sdi",
                "type": "SDI",
                "sdiSources": ["sdi-a", "sdi-b"],
                "inputNetworkLocation": "ON_PREMISES",
            }),
            "STANDARD",
            "STATIC",
        ),
    ];

    for (case, request, expected_class, expected_source_type) in table {
        let router = medialive();
        let created = create_input(&router, request.clone()).await;

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric input id, got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:input:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["state"],
            json!("DETACHED"),
            "case {case:?}: expected a new input to be DETACHED, got {:?}",
            created["state"]
        );
        assert_eq!(
            created["inputClass"],
            json!(expected_class),
            "case {case:?}: expected inputClass {expected_class}, got {:?}",
            created["inputClass"]
        );
        assert_eq!(
            created["inputSourceType"],
            json!(expected_source_type),
            "case {case:?}: expected inputSourceType {expected_source_type}, got {:?}",
            created["inputSourceType"]
        );
        assert_eq!(
            created["attachedChannels"],
            json!([]),
            "case {case:?}: expected a new input to be attached to no channel, got {:?}",
            created["attachedChannels"]
        );

        // Round trip: every member the request supplied is read back through
        // DescribeInput, out of state rather than out of the request.
        let (status, described) = describe_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput: got {status}: {described}"
        );
        assert_eq!(
            described["state"],
            json!("DETACHED"),
            "case {case:?}: expected the created input to read back DETACHED, got {:?}",
            described["state"]
        );
        for member in ["id", "arn", "name", "type", "inputClass", "inputSourceType"] {
            assert_eq!(
                described[member], created[member],
                "case {case:?}: expected DescribeInput to report the created {member}, got {:?}",
                described[member]
            );
        }
        assert_eq!(
            described["name"], request["name"],
            "case {case:?}: expected the stored name, got {:?}",
            described["name"]
        );
        assert_eq!(
            described["type"], request["type"],
            "case {case:?}: expected the stored type, got {:?}",
            described["type"]
        );
        assert_eq!(
            described["inputNetworkLocation"],
            // "By default all inputs will continue to be AWS inputs"
            // (`InputNetworkLocation`'s doc).
            request
                .get("inputNetworkLocation")
                .cloned()
                .unwrap_or_else(|| json!("AWS")),
            "case {case:?}: expected the stored network location, got {:?}",
            described["inputNetworkLocation"]
        );
        assert_eq!(
            described["securityGroups"],
            request
                .get("inputSecurityGroups")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: expected the attached input security groups, got {:?}",
            described["securityGroups"]
        );
        assert_eq!(
            described["sdiSources"],
            request.get("sdiSources").cloned().unwrap_or_else(|| json!([])),
            "case {case:?}: expected the stored SDI sources, got {:?}",
            described["sdiSources"]
        );
        assert_eq!(
            described["inputDevices"],
            request
                .get("inputDevices")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: expected the stored input devices, got {:?}",
            described["inputDevices"]
        );
        assert_eq!(
            described["mediaConnectFlows"],
            request
                .get("mediaConnectFlows")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: expected the stored MediaConnect flows, got {:?}",
            described["mediaConnectFlows"]
        );
        // A PULL input reports its sources verbatim; a PUSH input's destinations
        // report the ingest endpoint the service allocated for each of them (the
        // per-protocol table lives in `input_push_endpoints.rs`).
        assert_eq!(
            described["sources"],
            request.get("sources").cloned().unwrap_or_else(|| json!([])),
            "case {case:?}: expected the stored pull sources, got {:?}",
            described["sources"]
        );
        let destinations = described["destinations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            destinations.len(),
            request
                .get("destinations")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            "case {case:?}: expected one destination per requested destination, got {destinations:?}"
        );
        for destination in &destinations {
            assert!(
                destination["ip"]
                    .as_str()
                    .is_some_and(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok()),
                "case {case:?}: expected a system-generated ingest address, got {destination:?}"
            );
            assert!(
                destination["url"].as_str().is_some_and(|url| !url.is_empty()),
                "case {case:?}: expected the allocated push ingest URL, got {destination:?}"
            );
            assert_eq!(
                destination["networkRoutes"],
                json!([]),
                "case {case:?}: expected an empty route list on a destination with no routes, got {destination:?}"
            );
        }
    }
}

/// Rule: reading an input does not change it — repeated describes of the same
/// input answer the same `Input`, state included.
#[tokio::test]
async fn describing_an_input_does_not_change_it() {
    let router = medialive();
    let created = create_input(
        &router,
        json!({
            "name": "stable",
            "type": "URL_PULL",
            "sources": [source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    let mut reads = Vec::new();
    for read in 1..=3 {
        let (status, described) = describe_input(&router, &id).await;
        assert_eq!(status, 200, "read {read}: DescribeInput: got {status}: {described}");
        assert_eq!(
            described["state"],
            json!("DETACHED"),
            "read {read}: expected the input to stay DETACHED, got {:?}",
            described["state"]
        );
        reads.push(described);
    }
    assert!(
        reads.windows(2).all(|pair| pair[0] == pair[1]),
        "expected every read of an unchanged input to answer the same input, got {reads:?}"
    );
}

/// Rule: an input reports the channels that attach it — `ATTACHED` while some
/// channel's `InputAttachments` names it, `DETACHED` while none does — derived
/// from the channels in state, so a channel created after the input counts.
#[tokio::test]
async fn an_input_is_attached_exactly_while_a_channel_attaches_it() {
    let router = medialive();
    let created = create_input(
        &router,
        json!({
            "name": "attachable",
            "type": "URL_PULL",
            "sources": [source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
        }),
    )
    .await;
    let input_id = created["id"].as_str().unwrap_or_default().to_string();

    let (_, detached) = describe_input(&router, &input_id).await;
    assert_eq!(
        detached["state"],
        json!("DETACHED"),
        "before any channel attaches it: expected DETACHED, got {:?}",
        detached["state"]
    );

    let (status, channel) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "attaching",
            "inputAttachments": [{ "inputId": input_id, "inputAttachmentName": "primary" }],
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: got {status}: {channel}");
    let channel_id = channel["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (_, attached) = describe_input(&router, &input_id).await;
    assert_eq!(
        attached["state"],
        json!("ATTACHED"),
        "while a channel attaches it: expected ATTACHED, got {:?}",
        attached["state"]
    );
    assert_eq!(
        attached["attachedChannels"],
        json!([channel_id]),
        "expected the attaching channel's id, got {:?}",
        attached["attachedChannels"]
    );
}

/// Rule: `CreateInput`'s `RequestId` is an `@idempotencyToken`, so two creates
/// carrying the same one yield one input, and creates carrying different ones (or
/// none) each yield their own.
#[tokio::test]
async fn a_repeated_request_id_creates_one_input() {
    // (case, first request id, second request id, same input expected)
    let table: [(&str, Option<&str>, Option<&str>, bool); 3] = [
        ("same token", Some("token-a"), Some("token-a"), true),
        ("different tokens", Some("token-a"), Some("token-b"), false),
        ("no token", None, None, false),
    ];

    for (case, first_token, second_token, same) in table {
        let router = medialive();
        let mut body = json!({
            "name": "idempotent",
            "type": "URL_PULL",
            "sources": [source("https://example.com/a.m3u8")],
        });

        let mut ids = Vec::new();
        for token in [first_token, second_token] {
            if let Some(token) = token {
                body["requestId"] = json!(token);
            }
            let created = create_input(&router, body.clone()).await;
            ids.push(created["id"].as_str().unwrap_or_default().to_string());
        }

        assert_eq!(
            ids[0] == ids[1],
            same,
            "case {case:?}: expected same_input={same}, got ids {ids:?}"
        );
        for id in &ids {
            let (status, described) = describe_input(&router, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: every id a create answered must describe, got {status}: {described}"
            );
        }
    }
}

/// Rule: a `CreateInput` request that breaks what the `Vpc` member's own doc
/// requires — "This property requires setting the roleArn property on Input
/// creation. Not compatible with the inputSecurityGroups property." — is refused
/// with this operation's modelled `BadRequestException` (400) and creates
/// nothing; a request that satisfies it is created.
#[tokio::test]
async fn a_vpc_input_requires_a_role_and_refuses_input_security_groups() {
    let vpc = json!({ "subnetIds": ["subnet-a", "subnet-b"], "securityGroupIds": ["sg-a"] });
    let role = json!("arn:aws:iam::111122223333:role/MediaLiveAccessRole");

    // (case, request body, expected status)
    let table: [(&str, Value, u16); 4] = [
        (
            "vpc with a role",
            json!({ "name": "vpc-ok", "type": "RTP_PUSH", "vpc": vpc, "roleArn": role,
                    "destinations": [{ "streamName": "a" }, { "streamName": "b" }] }),
            201,
        ),
        (
            "vpc without a role",
            json!({ "name": "vpc-no-role", "type": "RTP_PUSH", "vpc": vpc }),
            400,
        ),
        (
            "vpc with input security groups",
            json!({ "name": "vpc-and-groups", "type": "RTP_PUSH", "vpc": vpc, "roleArn": role,
                    "inputSecurityGroups": ["3456789"] }),
            400,
        ),
        (
            "an empty security-group list is not one",
            json!({ "name": "vpc-empty-groups", "type": "RTP_PUSH", "vpc": vpc, "roleArn": role,
                    "inputSecurityGroups": [] }),
            201,
        ),
    ];

    for (case, body, expected_status) in table {
        let router = medialive();
        let (status, response) = call(&router, "POST", "/prod/inputs", Some(body)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(status, &response),
                "BadRequestException",
                "case {case:?}: expected the operation's own BadRequestException, got {response}"
            );
            // A refused create stored nothing: the next minted id is the first one.
            let created = create_input(
                &router,
                json!({ "name": "after-refusal", "type": "URL_PULL",
                        "sources": [source("https://example.com/a.m3u8")] }),
            )
            .await;
            assert_eq!(
                created["id"],
                json!("1000000"),
                "case {case:?}: a refused create must not have minted an input, got {:?}",
                created["id"]
            );
        }
    }
}

/// Rule: `DescribeInput` on an id no input has answers `NotFoundException` (404).
#[tokio::test]
async fn describing_an_unknown_input_is_not_found() {
    let router = medialive();
    for id in ["0000000", "nope", "1000001"] {
        let (status, body) = describe_input(&router, id).await;
        assert_eq!(
            status, 404,
            "case {id:?}: expected 404 for an unknown input, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {id:?}: expected NotFoundException, got {body}"
        );
    }
}

/// Rule: the input store is finite, so once it holds the emulator's ceiling the
/// create answers the operation's modelled `TooManyRequestsException` (429)
/// rather than growing without bound.
#[tokio::test]
async fn the_input_store_is_bounded() {
    let router = medialive();
    for index in 0..MAX_INPUTS {
        let created = create_input(
            &router,
            json!({ "name": format!("input-{index}"), "type": "URL_PULL",
                    "sources": [source("https://example.com/a.m3u8")] }),
        )
        .await;
        assert!(
            created["id"].is_string(),
            "case input {index}: expected a created input, got {created}"
        );
    }

    let (status, body) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({ "name": "one too many", "type": "URL_PULL" })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case {MAX_INPUTS} stored: expected the create to be throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {MAX_INPUTS} stored: expected TooManyRequestsException, got {body}"
    );
}
