// SPDX-License-Identifier: CC0-1.0

//! The MediaLive channel lifecycle, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::TRANSIENT_STATE_SPAN;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, medialive, ACCOUNT};

/// Long enough for a transient channel state to have come due, with margin.
pub(crate) fn wait_for_the_clock() {
    std::thread::sleep(TRANSIENT_STATE_SPAN + Duration::from_millis(200));
}

async fn create(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {response}");
    response["channel"].clone()
}

pub(crate) async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/channels/{id}"), None).await
}

/// Rule: `CreateChannel` mints a numeric `Id`, builds the `Arn` from the request's
/// region and that id, starts the channel in `CREATING` and settles it to `IDLE`;
/// `PipelinesRunningCount` is the channel class's pipeline count exactly while the
/// channel is `RUNNING`, and 0 in every other state.
///
/// The table spans the `ChannelClass` enum
/// (`smithy_query(query="shape", shape="ChannelClass")`: STANDARD |
/// SINGLE_PIPELINE) plus the absent-class row, which the service defaults to the
/// two-pipeline STANDARD class.
#[tokio::test]
async fn the_channel_lifecycle_runs_the_pipelines_the_class_gives_it() {
    // (case, requested ChannelClass, effective class, pipelines when RUNNING)
    let table: [(&str, Option<&str>, &str, usize); 3] = [
        ("single pipeline", Some("SINGLE_PIPELINE"), "SINGLE_PIPELINE", 1),
        ("standard", Some("STANDARD"), "STANDARD", 2),
        ("class omitted", None, "STANDARD", 2),
    ];

    for (case, requested, effective, pipelines) in table {
        let router = medialive();
        let mut request = json!({ "name": format!("channel-{case}") });
        if let Some(class) = requested {
            request["channelClass"] = json!(class);
        }

        let created = create(&router, request).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric channel id, got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:channel:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["channelClass"],
            json!(effective),
            "case {case:?}: expected channelClass {effective}, got {:?}",
            created["channelClass"]
        );
        assert_eq!(
            created["state"],
            json!("CREATING"),
            "case {case:?}: expected a new channel to be CREATING, got {:?}",
            created["state"]
        );
        assert_eq!(
            created["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipelines running on a CREATING channel, got {:?}",
            created["pipelinesRunningCount"]
        );
        assert_eq!(
            created["pipelineDetails"],
            json!([]),
            "case {case:?}: expected no pipeline details on a CREATING channel, got {:?}",
            created["pipelineDetails"]
        );

        // A channel that is still creating cannot be started.
        let (status, body) = call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected starting a CREATING channel to conflict, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );

        wait_for_the_clock();
        let (status, idle) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {idle}");
        assert_eq!(
            idle["state"],
            json!("IDLE"),
            "case {case:?}: expected a created channel to settle to IDLE, got {:?}",
            idle["state"]
        );

        // start -> STARTING -> RUNNING, with the class's pipelines running.
        let (status, starting) =
            call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
        assert_eq!(status, 200, "case {case:?}: StartChannel: got {status}: {starting}");
        assert_eq!(
            starting["state"],
            json!("STARTING"),
            "case {case:?}: expected StartChannel to answer STARTING, got {:?}",
            starting["state"]
        );
        assert_eq!(
            starting["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipelines running while STARTING, got {:?}",
            starting["pipelinesRunningCount"]
        );

        wait_for_the_clock();
        let (status, running) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {running}");
        assert_eq!(
            running["state"],
            json!("RUNNING"),
            "case {case:?}: expected a started channel to settle to RUNNING, got {:?}",
            running["state"]
        );
        assert_eq!(
            running["pipelinesRunningCount"],
            json!(pipelines),
            "case {case:?}: expected {pipelines} pipeline(s) running, got {:?}",
            running["pipelinesRunningCount"]
        );
        assert_eq!(
            running["pipelineDetails"].as_array().map(Vec::len),
            Some(pipelines),
            "case {case:?}: expected {pipelines} pipeline detail(s), got {:?}",
            running["pipelineDetails"]
        );

        // A running channel cannot be deleted.
        let (status, body) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected deleting a RUNNING channel to conflict, got {status}: {body}"
        );

        // stop -> STOPPING -> IDLE, with no pipeline running again.
        let (status, stopping) =
            call(&router, "POST", &format!("/prod/channels/{id}/stop"), None).await;
        assert_eq!(status, 200, "case {case:?}: StopChannel: got {status}: {stopping}");
        assert_eq!(
            stopping["state"],
            json!("STOPPING"),
            "case {case:?}: expected StopChannel to answer STOPPING, got {:?}",
            stopping["state"]
        );
        assert_eq!(
            stopping["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipelines running once stopping, got {:?}",
            stopping["pipelinesRunningCount"]
        );

        wait_for_the_clock();
        let (status, stopped) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {stopped}");
        assert_eq!(
            stopped["state"],
            json!("IDLE"),
            "case {case:?}: expected a stopped channel to settle to IDLE, got {:?}",
            stopped["state"]
        );

        // delete -> DELETING -> the terminal DELETED the channel keeps. The
        // record stays in the account, which is what lets the SDK's
        // `ChannelDeleted` waiter poll `DescribeChannel` for that state.
        let (status, deleting) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(status, 200, "case {case:?}: DeleteChannel: got {status}: {deleting}");
        assert_eq!(
            deleting["state"],
            json!("DELETING"),
            "case {case:?}: expected DeleteChannel to answer DELETING, got {:?}",
            deleting["state"]
        );

        wait_for_the_clock();
        let (status, deleted) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a deleted channel to still be readable, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETED"),
            "case {case:?}: expected a deleted channel to settle to DELETED, got {:?}",
            deleted["state"]
        );
        assert_eq!(
            deleted["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipelines running on a DELETED channel, got {:?}",
            deleted["pipelinesRunningCount"]
        );

        // `DELETED` is terminal: a second delete is refused rather than
        // starting the deletion over.
        let (status, again) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected deleting a DELETED channel to conflict, got {status}: {again}"
        );
        assert_eq!(
            error_code(status, &again),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {again}"
        );

        // The deleted channel is still this account's record of it, so the list
        // goes on carrying it in that state.
        let (status, listed) = call(&router, "GET", "/prod/channels", None).await;
        assert_eq!(status, 200, "case {case:?}: ListChannels: got {status}: {listed}");
        let channels = listed["channels"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            channels.len(),
            1,
            "case {case:?}: expected the deleted channel to still be listed, got {:?}",
            listed["channels"]
        );
        assert_eq!(
            (&channels[0]["id"], &channels[0]["state"]),
            (&json!(id), &json!("DELETED")),
            "case {case:?}: expected the deleted channel listed as DELETED, got {:?}",
            channels[0]
        );
    }
}

/// Rule: every member `CreateChannel` accepts is persisted verbatim and read back
/// by `DescribeChannel`, whatever subset of them the request carries.
///
/// The rows differ along which members are supplied and along their type (string,
/// enum, map, list, nested structure), because that is what the rule generalises
/// over. Member names and enum values come from
/// `smithy_query(query="operation", operation="CreateChannel")` and the
/// `InputCodec` / `InputResolution` / `InputMaximumBitrate` / `LogLevel` /
/// `MaintenanceDay` enums.
#[tokio::test]
async fn every_member_a_create_supplies_is_read_back_by_describe() {
    let rows: [(&str, Value); 6] = [
        ("name only", json!({ "name": "demo" })),
        (
            "tags and role",
            json!({
                "name": "tagged",
                "roleArn": "arn:aws:iam::000000000000:role/MediaLiveAccessRole",
                "tags": { "env": "test", "team": "video" },
            }),
        ),
        (
            "input specification and log level",
            json!({
                "name": "specified",
                "logLevel": "DEBUG",
                "inputSpecification": {
                    "codec": "AVC",
                    "resolution": "HD",
                    "maximumBitrate": "MAX_10_MBPS",
                },
                "cdiInputSpecification": { "resolution": "HD" },
            }),
        ),
        (
            "destinations and input attachments",
            json!({
                "name": "wired",
                "destinations": [
                    { "id": "destination-1" },
                    { "id": "destination-2", "logicalInterfaceNames": ["eth0"] },
                ],
                "inputAttachments": [
                    { "inputAttachmentName": "primary", "inputId": "9999999" },
                ],
            }),
        ),
        (
            "maintenance window",
            json!({
                "name": "maintained",
                "maintenance": {
                    "maintenanceDay": "THURSDAY",
                    "maintenanceStartTime": "02:00",
                },
            }),
        ),
        (
            "security groups and vpc",
            json!({
                "name": "contained",
                "channelSecurityGroups": ["1111111", "2222222"],
                "vpc": {
                    "subnetIds": ["subnet-a", "subnet-b"],
                    "securityGroupIds": ["sg-1"],
                },
            }),
        ),
    ];

    for (case, request) in rows {
        let router = medialive();
        let created = create(&router, request.clone()).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let (status, described) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {described}");

        let submitted = request.as_object().expect("the request row is an object");
        for (member, value) in submitted {
            let stored = match member.as_str() {
                // The request's `maintenance` is `MaintenanceCreateSettings`; the
                // response's is `MaintenanceStatus`, which adds the scheduler's own
                // deadline and scheduled date. Compare the members the request set.
                "maintenance" => json!({
                    "maintenanceDay": described["maintenance"]["maintenanceDay"],
                    "maintenanceStartTime": described["maintenance"]["maintenanceStartTime"],
                }),
                // Likewise `VpcOutputSettings` -> `VpcOutputSettingsDescription`.
                "vpc" => json!({
                    "subnetIds": described["vpc"]["subnetIds"],
                    "securityGroupIds": described["vpc"]["securityGroupIds"],
                }),
                other => described[other].clone(),
            };
            assert_eq!(
                &stored, value,
                "case {case:?}: member {member:?} was not persisted: expected {value}, got {stored}"
            );
        }

        // The id and the ARN are the channel's own identity, stable across reads.
        assert_eq!(
            described["id"], created["id"],
            "case {case:?}: the id changed between create and describe: {:?} then {:?}",
            created["id"], described["id"]
        );
        assert_eq!(
            described["arn"], created["arn"],
            "case {case:?}: the ARN changed between create and describe: {:?} then {:?}",
            created["arn"], described["arn"]
        );
    }
}

/// Rule: `ListChannels` pages the channels that exist in a stable order, returns
/// at most `MaxResults` of them per page, and carries a `NextToken` on every page
/// but the last.
///
/// The table spans page sizes below, at and above the collection size, which is
/// what the rule generalises over.
#[tokio::test]
async fn list_channels_pages_every_persisted_channel_exactly_once() {
    let router = medialive();
    let mut expected = Vec::new();
    for index in 0..5 {
        let created = create(&router, json!({ "name": format!("channel-{index}") })).await;
        expected.push(created["id"].as_str().unwrap_or_default().to_string());
        // Asserted here, not at the end of the test: by then the clock may
        // legitimately have settled these channels to IDLE.
        assert_eq!(
            created["state"],
            json!("CREATING"),
            "case channel-{index}: expected a new channel to be CREATING, got {:?}",
            created["state"]
        );
    }

    for page_size in [1_usize, 2, 4, 5, 9] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let uri = match &token {
                Some(token) => {
                    format!("/prod/channels?maxResults={page_size}&nextToken={token}")
                }
                None => format!("/prod/channels?maxResults={page_size}"),
            };
            let (status, page) = call(&router, "GET", &uri, None).await;
            assert_eq!(status, 200, "case {case:?}: ListChannels: got {status}: {page}");

            let channels = page["channels"].as_array().cloned().unwrap_or_default();
            assert!(
                channels.len() <= page_size,
                "case {case:?}: expected at most {page_size} channels on a page, got {}",
                channels.len()
            );
            for channel in &channels {
                seen.push(channel["id"].as_str().unwrap_or_default().to_string());
            }

            pages += 1;
            assert!(pages <= 10, "case {case:?}: pagination did not terminate");

            token = page["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every channel once in id order, got {seen:?}"
        );
        let last_page_count = expected.len() % page_size;
        let expected_pages = expected.len() / page_size + usize::from(last_page_count > 0);
        assert_eq!(
            pages,
            expected_pages.max(1),
            "case {case:?}: expected {expected_pages} page(s), walked {pages}"
        );
    }

    // A page size below the modelled minimum of one asks for nothing.
    for page_size in ["0", "-1"] {
        let (status, body) =
            call(&router, "GET", &format!("/prod/channels?maxResults={page_size}"), None).await;
        assert_eq!(
            status, 400,
            "case maxResults={page_size:?}: expected a refusal, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case maxResults={page_size:?}: expected the operation's own BadRequestException, got {body}"
        );
    }

    // Every channel is a summary of the channel that was created, not a canned row.
    let (_, listed) = call(&router, "GET", "/prod/channels", None).await;
    let channels = listed["channels"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        channels.len(),
        expected.len(),
        "expected every created channel to be listed, got {channels:?}"
    );
    for (index, channel) in channels.iter().enumerate() {
        assert_eq!(
            channel["name"],
            json!(format!("channel-{index}")),
            "expected the summary to carry the created name, got {:?}",
            channel["name"]
        );
        assert_eq!(
            channel["id"],
            json!(expected[index]),
            "expected the summary to carry the created id, got {:?}",
            channel["id"]
        );
    }
}

/// Rule: an operation on a channel id that does not exist answers
/// `NotFoundException` (404), and one on a channel whose state does not allow it
/// answers `ConflictException` (409).
///
/// The table spans every channel operation that takes a `ChannelId` and, for the
/// conflict half, every state this slice can put a channel in.
#[tokio::test]
async fn channel_operations_refuse_unknown_ids_and_wrong_states() {
    // (case, method, uri suffix)
    let operations: [(&str, &str, &str); 4] = [
        ("describe", "GET", ""),
        ("start", "POST", "/start"),
        ("stop", "POST", "/stop"),
        ("delete", "DELETE", ""),
    ];

    for (case, method, suffix) in operations {
        let router = medialive();
        let (status, body) =
            call(&router, method, &format!("/prod/channels/0000000{suffix}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?} on an unknown id: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?} on an unknown id: expected NotFoundException, got {body}"
        );
    }

    // (case, state to reach, method, uri suffix)
    let conflicts: [(&str, &str, &str, &str); 5] = [
        ("start while CREATING", "CREATING", "POST", "/start"),
        ("stop while CREATING", "CREATING", "POST", "/stop"),
        ("stop while IDLE", "IDLE", "POST", "/stop"),
        ("start while RUNNING", "RUNNING", "POST", "/start"),
        ("delete while RUNNING", "RUNNING", "DELETE", ""),
    ];

    for (case, state, method, suffix) in conflicts {
        let router = medialive();
        let created = create(&router, json!({ "name": case })).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        if state != "CREATING" {
            wait_for_the_clock();
        }
        if state == "RUNNING" {
            let (status, started) =
                call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
            assert_eq!(status, 200, "case {case:?}: StartChannel: got {status}: {started}");
            wait_for_the_clock();
        }

        let (status, observed) = describe(&router, &id).await;
        assert_eq!(status, 200, "case {case:?}: DescribeChannel: got {status}: {observed}");
        assert_eq!(
            observed["state"],
            json!(state),
            "case {case:?}: the test did not reach {state}, it reached {:?}",
            observed["state"]
        );

        let (status, body) =
            call(&router, method, &format!("/prod/channels/{id}{suffix}"), None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }
}

/// Rule: `CreateChannel`'s `RequestId` is an `@idempotencyToken`, so two creates
/// carrying the same one yield one channel, and creates carrying different ones
/// (or none) each yield their own.
#[tokio::test]
async fn a_repeated_request_id_creates_one_channel() {
    // (case, first request id, second request id, same channel expected)
    let table: [(&str, Option<&str>, Option<&str>, bool); 3] = [
        ("same token", Some("token-a"), Some("token-a"), true),
        ("different tokens", Some("token-a"), Some("token-b"), false),
        ("no token", None, None, false),
    ];

    for (case, first_token, second_token, same) in table {
        let router = medialive();
        let mut first = json!({ "name": "idempotent" });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let mut second = json!({ "name": "idempotent" });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }

        let one = create(&router, first).await;
        let two = create(&router, second).await;
        assert_eq!(
            one["id"] == two["id"],
            same,
            "case {case:?}: expected same-channel={same}, got {:?} then {:?}",
            one["id"],
            two["id"]
        );

        let (_, listed) = call(&router, "GET", "/prod/channels", None).await;
        let expected_channels = if same { 1 } else { 2 };
        assert_eq!(
            listed["channels"].as_array().map(Vec::len),
            Some(expected_channels),
            "case {case:?}: expected {expected_channels} channel(s) to exist, got {:?}",
            listed["channels"]
        );
    }
}

/// Rule: the channel store is bounded, so a create loop is refused with the
/// operation's modelled throttle rather than growing the emulator's heap, and
/// `ListChannels` never projects more than a finite page even when the request
/// names no `MaxResults`.
#[tokio::test]
async fn the_store_is_bounded_and_the_default_page_is_finite() {
    use service_com_amazonaws_medialive_medialive::state::{MAX_CHANNELS, MAX_LIST_PAGE_SIZE};

    let router = medialive();
    for index in 0..MAX_CHANNELS {
        create(&router, json!({ "name": format!("channel-{index}") })).await;
    }

    let (status, body) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "one too many" })),
    )
    .await;
    assert_eq!(
        status, 429,
        "case {MAX_CHANNELS} stored: expected the create to be throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case {MAX_CHANNELS} stored: expected TooManyRequestsException, got {body}"
    );

    // No maxResults: the page is the service's own finite ceiling, and it hands
    // out a token because channels are left over.
    let (status, page) = call(&router, "GET", "/prod/channels", None).await;
    assert_eq!(status, 200, "ListChannels: got {status}: {page}");
    assert_eq!(
        page["channels"].as_array().map(Vec::len),
        Some(MAX_LIST_PAGE_SIZE),
        "expected the default page to hold {MAX_LIST_PAGE_SIZE} channels, got {:?}",
        page["channels"].as_array().map(Vec::len)
    );
    assert!(
        page["nextToken"].is_string(),
        "expected a token while channels are left over, got {:?}",
        page["nextToken"]
    );

    // A maxResults above that ceiling is clamped to it rather than honoured.
    let (status, page) = call(&router, "GET", "/prod/channels?maxResults=10000", None).await;
    assert_eq!(status, 200, "ListChannels: got {status}: {page}");
    assert_eq!(
        page["channels"].as_array().map(Vec::len),
        Some(MAX_LIST_PAGE_SIZE),
        "expected an over-large maxResults to be clamped to {MAX_LIST_PAGE_SIZE}, got {:?}",
        page["channels"].as_array().map(Vec::len)
    );
}

/// Rule: `DryRun` carries no documented semantics in this model, so a create that
/// asks for one is refused with the operation's modelled client error rather than
/// silently creating a channel.
#[tokio::test]
async fn a_dry_run_create_is_refused_rather_than_creating_a_channel() {
    let router = medialive();
    let (status, body) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "dry", "dryRun": true })),
    )
    .await;
    assert_eq!(status, 400, "expected a refusal, got {status}: {body}");
    assert_eq!(
        error_code(status, &body),
        "BadRequestException",
        "expected BadRequestException, got {body}"
    );

    let (_, listed) = call(&router, "GET", "/prod/channels", None).await;
    assert_eq!(
        listed["channels"],
        json!([]),
        "a refused dry run must not have created a channel, got {:?}",
        listed["channels"]
    );
}

/// Rule: `InputAttachments` is a caller-chosen list every later read walks, so
/// its length is bounded — a create at or below the emulator's ceiling is
/// accepted and stores exactly the attachments it carried, and one past it is
/// refused with the operation's modelled `BadRequestException` (400), creating
/// nothing.
///
/// The rows span the published bound from both sides (none, one, the ceiling,
/// one past it), because that is what a bound generalises over.
#[tokio::test]
async fn the_attachment_list_of_one_channel_is_bounded() {
    use service_com_amazonaws_medialive_medialive::state::MAX_INPUT_ATTACHMENTS;

    // (case, how many attachments the request carries, expected status)
    let table: [(&str, usize, u16); 4] = [
        ("no attachment", 0, 201),
        ("one attachment", 1, 201),
        ("at the ceiling", MAX_INPUT_ATTACHMENTS, 201),
        ("one past the ceiling", MAX_INPUT_ATTACHMENTS + 1, 400),
    ];

    for (case, count, expected_status) in table {
        let router = medialive();
        let attachments: Vec<Value> = (0..count)
            .map(|slot| json!({ "inputId": format!("100000{slot}"), "inputAttachmentName": format!("slot-{slot}") }))
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            "/prod/channels",
            Some(json!({ "name": "attaching", "inputAttachments": attachments.clone() })),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {body}"
        );

        if expected_status == 201 {
            assert_eq!(
                body["channel"]["inputAttachments"],
                json!(attachments),
                "case {case:?}: expected the stored attachments, got {:?}",
                body["channel"]["inputAttachments"]
            );
        } else {
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body}"
            );
            let (_, listed) = call(&router, "GET", "/prod/channels", None).await;
            assert_eq!(
                listed["channels"],
                json!([]),
                "case {case:?}: a refused create must not have created a channel, got {:?}",
                listed["channels"]
            );
        }
    }
}
