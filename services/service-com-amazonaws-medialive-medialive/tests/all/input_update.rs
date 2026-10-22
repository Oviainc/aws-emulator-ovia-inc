// SPDX-License-Identifier: CC0-1.0

//! `UpdateInput`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded `URL_PULL` rename is ONE row of each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, medialive};

/// One row of a pull-source list, as `InputSourceRequest` shapes it.
fn source(url: &str) -> Value {
    json!({ "url": url, "username": "u", "passwordParam": "p" })
}

/// The input every row of the update table starts from: a two-source `URL_PULL`
/// input carrying a name, a role, one security group and a tag, so a patch that
/// replaces one member can be seen to leave every other one alone.
fn baseline_body() -> Value {
    json!({
        "name": "baseline",
        "type": "URL_PULL",
        "sources": [source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
        "inputSecurityGroups": ["3456789"],
        "roleArn": "arn:aws:iam::111122223333:role/Baseline",
        "tags": { "team": "video" },
    })
}

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

async fn update_input(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("/prod/inputs/{id}"), Some(body)).await
}

/// One row of the member table: the case, the request body, and the members the
/// stored input must report differently once that request has been applied.
type MemberRow = (&'static str, Value, Vec<(&'static str, Value)>);

/// One row of the derived-member table: the case, the type and sources the input
/// was created with, the sources the update names, and the `InputClass` and
/// `InputSourceType` the updated input must then report.
type DerivedRow = (
    &'static str,
    &'static str,
    Vec<Value>,
    Vec<Value>,
    &'static str,
    &'static str,
);

/// Rule: `UpdateInput` replaces exactly the members its request names on the
/// stored input, leaves every member the request omits at the value the input
/// already had, recomputes the members derived from what it wrote, and answers
/// the input read back out of state — so the same input reads back through
/// `DescribeInput`.
///
/// The table spans every member this operation's input carries
/// (`smithy_query(query="operation", operation="UpdateInput")`: Name, RoleArn,
/// InputSecurityGroups, Sources, Destinations, MediaConnectFlows, InputDevices,
/// SdiSources, SrtSettings, MulticastSettings, Smpte2110ReceiverGroupSettings,
/// SpecialRouterSettings), at one and at two list entries and at an empty list,
/// plus the row that names no member at all. The issue's recorded case is the
/// "name only" row.
#[tokio::test]
async fn an_update_replaces_only_the_members_the_request_names() {
    // (case, request body, the members the stored input must now report
    //  differently — every other member must still read back unchanged)
    let table: [MemberRow; 15] = [
        ("no member named", json!({}), vec![]),
        (
            "name only",
            json!({ "name": "renamed" }),
            vec![("name", json!("renamed"))],
        ),
        (
            "role arn only",
            json!({ "roleArn": "arn:aws:iam::111122223333:role/Updated" }),
            vec![(
                "roleArn",
                json!("arn:aws:iam::111122223333:role/Updated"),
            )],
        ),
        (
            "two security groups replace one",
            json!({ "inputSecurityGroups": ["9999999", "8888888"] }),
            vec![("securityGroups", json!(["9999999", "8888888"]))],
        ),
        (
            // A list the request names is replaced by what it names, so an
            // empty list detaches every group.
            "an empty security group list",
            json!({ "inputSecurityGroups": [] }),
            vec![("securityGroups", json!([]))],
        ),
        (
            // One ingest endpoint is `SINGLE_PIPELINE` (`InputClass`'s doc), so
            // replacing two sources with one moves the derived class too.
            "one pull source replaces two",
            json!({ "sources": [source("https://example.com/only.m3u8")] }),
            vec![
                ("sources", json!([source("https://example.com/only.m3u8")])),
                ("inputClass", json!("SINGLE_PIPELINE")),
            ],
        ),
        (
            "two pull sources replace two",
            json!({ "sources": [source("https://example.com/c.m3u8"), source("https://example.com/d.m3u8")] }),
            vec![(
                "sources",
                json!([source("https://example.com/c.m3u8"), source("https://example.com/d.m3u8")]),
            )],
        ),
        (
            // The baseline input is `URL_PULL`, a type with no push ingest
            // protocol, so the destination reports no port or URL; `Ip` is the
            // request's own `StaticIpAddress`, and the route list is reported
            // empty rather than unset. The push rows live in
            // `input_push_endpoints.rs`.
            "one push destination",
            json!({ "destinations": [{ "streamName": "live/x", "staticIpAddress": "10.0.0.5" }] }),
            vec![(
                "destinations",
                json!([{ "ip": "10.0.0.5", "networkRoutes": [] }]),
            )],
        ),
        (
            "two media connect flows",
            json!({ "mediaConnectFlows": [
                { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:2:b" },
            ] }),
            vec![("mediaConnectFlows", json!([
                { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:1:a" },
                { "flowArn": "arn:aws:mediaconnect:us-west-2:111122223333:flow:2:b" },
            ]))],
        ),
        (
            "one input device",
            json!({ "inputDevices": [{ "id": "hd-0000001" }] }),
            // `InputClass` counts the WIDEST per-pipeline list, and the two
            // baseline sources are still the widest, so the class does not move.
            vec![("inputDevices", json!([{ "id": "hd-0000001" }]))],
        ),
        (
            "two sdi sources",
            json!({ "sdiSources": ["sdi-x", "sdi-y"] }),
            vec![("sdiSources", json!(["sdi-x", "sdi-y"]))],
        ),
        (
            "srt caller sources",
            json!({ "srtSettings": { "srtCallerSources": [{
                "srtListenerAddress": "198.51.100.10",
                "srtListenerPort": "9000",
                "streamId": "stream-a",
                "minimumLatency": 2000,
            }] } }),
            vec![("srtSettings", json!({ "srtCallerSources": [{
                "srtListenerAddress": "198.51.100.10",
                "srtListenerPort": "9000",
                "streamId": "stream-a",
                "minimumLatency": 2000,
            }] }))],
        ),
        (
            "multicast sources",
            json!({ "multicastSettings": { "sources": [
                { "url": "udp://239.0.0.1:5000", "sourceIp": "10.0.0.7" },
                { "url": "udp://239.0.0.2:5000" },
            ] } }),
            vec![("multicastSettings", json!({ "sources": [
                { "url": "udp://239.0.0.1:5000", "sourceIp": "10.0.0.7" },
                { "url": "udp://239.0.0.2:5000" },
            ] }))],
        ),
        (
            "a smpte 2110 receiver group",
            json!({ "smpte2110ReceiverGroupSettings": { "smpte2110ReceiverGroups": [{
                "sdpSettings": { "videoSdp": { "sdpUrl": "s3://bucket/video.sdp", "mediaIndex": 1 } },
            }] } }),
            vec![("smpte2110ReceiverGroupSettings", json!({ "smpte2110ReceiverGroups": [{
                "sdpSettings": { "videoSdp": { "sdpUrl": "s3://bucket/video.sdp", "mediaIndex": 1 } },
            }] }))],
        ),
        (
            // `SpecialRouterSettings` carries only the router's ARN, and the
            // `Input` response shape has no member that reports one, so the
            // stored input reads back exactly as before. This row pins the
            // projection, not the store: the modelled response cannot tell a
            // handler that stored the member from one that dropped it.
            "special router settings",
            json!({ "specialRouterSettings": { "routerArn": "arn:aws:mediaconnect:us-west-2:111122223333:router:r1" } }),
            vec![],
        ),
    ];

    for (case, patch, changes) in table {
        let router = medialive();
        let created = create_input(&router, baseline_body()).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let (status, before) = describe_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput before the update: got {status}: {before}"
        );

        // What the stored input must look like afterwards: the one it had, with
        // the members this request names replaced.
        let mut expected = before.clone();
        for (member, value) in &changes {
            expected[*member] = value.clone();
        }

        let (status, response) = update_input(&router, &id, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateInput: expected 200, got {status}: {response}"
        );
        assert_eq!(
            response["input"], expected,
            "case {case:?}: expected UpdateInput to answer the stored input with {changes:?} replaced, got {:?}",
            response["input"]
        );

        // Round trip: the update is what is stored, not what was answered.
        let (status, after) = describe_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput after the update: got {status}: {after}"
        );
        assert_eq!(
            after, expected,
            "case {case:?}: expected the updated input to read back through DescribeInput, got {after:?}"
        );

        // The identifiers and the members no update member can reach are the
        // ones the create minted.
        for member in ["id", "arn", "type", "tags", "inputNetworkLocation", "state"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: expected {member} to survive the update, got {:?}",
                after[member]
            );
        }

        // Idempotence: applying the same patch again lands the same input.
        let (status, repeated) = update_input(&router, &id, patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated UpdateInput: got {status}: {repeated}"
        );
        assert_eq!(
            repeated["input"], expected,
            "case {case:?}: expected a repeated update to land the same input, got {:?}",
            repeated["input"]
        );
    }
}

/// Rule: the members an input derives from its ingest endpoints — `InputClass`
/// ("A standard input has two sources and a single pipeline input only has one")
/// and `InputSourceType` ("By default all input sources are static") — are
/// recomputed from what the update stored, never frozen at create time.
#[tokio::test]
async fn an_update_recomputes_the_members_derived_from_the_input() {
    // (case, created type, created sources, update sources, expected class,
    //  expected source type)
    let table: [DerivedRow; 5] = [
        (
            "two static sources become one",
            "URL_PULL",
            vec![source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
            vec![source("https://example.com/only.m3u8")],
            "SINGLE_PIPELINE",
            "STATIC",
        ),
        (
            "one static source becomes two",
            "URL_PULL",
            vec![source("https://example.com/only.m3u8")],
            vec![source("https://example.com/a.m3u8"), source("https://example.com/b.m3u8")],
            "STANDARD",
            "STATIC",
        ),
        (
            "a dynamic-capable type gains a dynamic url",
            "MP4_FILE",
            vec![source("s3://bucket/a.mp4")],
            vec![source("s3://bucket/$urlPath$")],
            "SINGLE_PIPELINE",
            "DYNAMIC",
        ),
        (
            "a dynamic-capable type loses its dynamic url",
            "TS_FILE",
            vec![source("s3://bucket/a/$urlPath$"), source("s3://bucket/b/$urlPath$")],
            vec![source("s3://bucket/a.ts"), source("s3://bucket/b.ts")],
            "STANDARD",
            "STATIC",
        ),
        (
            // The variable alone is not enough: the type must be one the
            // `InputSourceType` doc names (MP4_FILE, TS_FILE).
            "a type that cannot be dynamic gains the variable",
            "URL_PULL",
            vec![source("https://example.com/a.m3u8")],
            vec![source("https://example.com/$urlPath$")],
            "SINGLE_PIPELINE",
            "STATIC",
        ),
    ];

    for (case, input_type, created_sources, updated_sources, class, source_type) in table {
        let router = medialive();
        let created = create_input(
            &router,
            json!({ "name": "derived", "type": input_type, "sources": created_sources }),
        )
        .await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, response) =
            update_input(&router, &id, json!({ "sources": updated_sources })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateInput: expected 200, got {status}: {response}"
        );

        let (status, after) = describe_input(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeInput: got {status}: {after}"
        );
        assert_eq!(
            after["inputClass"],
            json!(class),
            "case {case:?}: expected inputClass {class} after the update, got {:?}",
            after["inputClass"]
        );
        assert_eq!(
            after["inputSourceType"],
            json!(source_type),
            "case {case:?}: expected inputSourceType {source_type} after the update, got {:?}",
            after["inputSourceType"]
        );
    }
}

/// Rule: `UpdateInput` on an id no input has answers this operation's modelled
/// `NotFoundException` (404) and writes nothing.
#[tokio::test]
async fn updating_an_unknown_input_is_not_found() {
    let router = medialive();
    let created = create_input(&router, baseline_body()).await;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    for unknown in ["0000000", "nope", "1000001"] {
        let (status, body) = update_input(
            &router,
            unknown,
            json!({ "name": "should-not-be-stored" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected 404 for an unknown input, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {unknown:?}: expected NotFoundException, got {body}"
        );
    }

    // The input that does exist was not touched by any of those refusals.
    let (status, after) = describe_input(&router, &id).await;
    assert_eq!(status, 200, "DescribeInput: got {status}: {after}");
    assert_eq!(
        after["name"],
        json!("baseline"),
        "expected a refused update to have written nothing, got {:?}",
        after["name"]
    );
}

/// Rule: an input a channel is attached to can only be edited once that channel
/// has stopped — while the attached channel is in any other state the update is
/// refused with this operation's modelled `ConflictException` (409) and writes
/// nothing.
///
/// The table walks one channel through the states a created channel reaches
/// (`smithy_query(query="shape", shape="ChannelState")`): the transient
/// `CREATING`, the settled `IDLE`, `STARTING`, `RUNNING` and back to `IDLE`.
#[tokio::test]
async fn an_input_attached_to_a_channel_that_has_not_stopped_cannot_be_updated() {
    let router = medialive();
    let created = create_input(&router, baseline_body()).await;
    let input_id = created["id"].as_str().unwrap_or_default().to_string();

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

    // Each step renames the input and then reads it back: an accepted update is
    // the stored name, a refused one leaves the name the last accepted update
    // wrote. `stored` carries that name forward through the walk.
    let mut stored = "baseline".to_string();

    // CREATING: the channel has not reached IDLE yet, so the input is in use.
    step(
        &router,
        &input_id,
        &channel_id,
        "while the channel is CREATING",
        "creating-rename",
        409,
        &mut stored,
    )
    .await;

    // IDLE: the channel has settled and ingests nothing, so the edit lands.
    wait_for_the_clock();
    step(
        &router,
        &input_id,
        &channel_id,
        "once the channel is IDLE",
        "idle-rename",
        200,
        &mut stored,
    )
    .await;

    // STARTING, then RUNNING: the channel is using the input again.
    let (status, starting) = call(
        &router,
        "POST",
        &format!("/prod/channels/{channel_id}/start"),
        None,
    )
    .await;
    assert_eq!(status, 200, "StartChannel: got {status}: {starting}");
    step(
        &router,
        &input_id,
        &channel_id,
        "while the channel is STARTING",
        "starting-rename",
        409,
        &mut stored,
    )
    .await;

    wait_for_the_clock();
    step(
        &router,
        &input_id,
        &channel_id,
        "while the channel is RUNNING",
        "running-rename",
        409,
        &mut stored,
    )
    .await;

    // Stopped and settled: editable again.
    let (status, stopping) = call(
        &router,
        "POST",
        &format!("/prod/channels/{channel_id}/stop"),
        None,
    )
    .await;
    assert_eq!(status, 200, "StopChannel: got {status}: {stopping}");
    wait_for_the_clock();
    step(
        &router,
        &input_id,
        &channel_id,
        "once the channel has stopped",
        "stopped-rename",
        200,
        &mut stored,
    )
    .await;
}

/// One step of the walk above: rename the input, check the status the attached
/// channel's state calls for, then read the input back to show what was written.
/// `stored` is the name the input must now report, which an accepted update
/// advances and a refused one leaves alone.
async fn step(
    router: &MultiServiceRouter,
    input_id: &str,
    channel_id: &str,
    case: &str,
    name: &str,
    expected_status: u16,
    stored: &mut String,
) {
    let (status, body) = update_input(router, input_id, json!({ "name": name })).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status}, got {status}: {body}"
    );
    if expected_status == 409 {
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    } else {
        *stored = name.to_string();
        assert_eq!(
            body["input"]["name"],
            json!(name),
            "case {case:?}: expected the update to have been applied, got {:?}",
            body["input"]["name"]
        );
    }

    let (status, after) = describe_input(router, input_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInput: got {status}: {after}"
    );
    assert_eq!(
        after["name"],
        json!(stored.as_str()),
        "case {case:?}: expected the stored name to be {stored:?}, got {:?}",
        after["name"]
    );
    assert_eq!(
        after["attachedChannels"],
        json!([channel_id]),
        "case {case:?}: expected the input to stay attached to the channel, got {:?}",
        after["attachedChannels"]
    );
}
