// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the ivschat logging-configuration lifecycle:
//! `CreateLoggingConfiguration`, `GetLoggingConfiguration`, `UpdateLoggingConfiguration`,
//! `ListLoggingConfigurations` and `DeleteLoggingConfiguration`, driven over the wire
//! through the router the production entrypoint serves.
//!
//! Every table row is a different input along what the rule under test generalises over
//! (which destination member the union carries, the destination's value, members present,
//! tag count, stored count, page size, region), not a rename of one case.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::ops::types::{
    DestinationConfiguration, S3DestinationConfiguration,
};
use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::state::{
    LoggingConfiguration, LOGGING_CONFIGURATIONS_PER_REGION, LOGGING_CONFIGURATION_ID_LEN,
};
use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::ComAmazonawsIvschatAmazoninteractivevideoservicechatState as ChatState;

use crate::room_lifecycle::{error_code, now_secs, seconds, str_field, Emulator, ACCOUNT, REGION};

/// The states `CreateLoggingConfiguration` may report, as the service crate's generated
/// catalog of the modelled `CreateLoggingConfigurationState` enum carries them. The enum
/// has exactly one member, so a freshly minted configuration is already ready to log.
const CREATE_STATES: &[&str] =
    service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::ops::types::CREATE_LOGGING_CONFIGURATION_STATE_VALUES;

/// One request body per member of the modelled `DestinationConfiguration` union, as
/// `smithy_query(query="shape", shape="DestinationConfiguration")` reports them: `s3`
/// (`bucketName`), `cloudWatchLogs` (`logGroupName`) and `firehose`
/// (`deliveryStreamName`). The union admits exactly one member per request.
fn destination_catalog() -> Vec<(&'static str, Value)> {
    vec![
        ("s3", json!({ "s3": { "bucketName": "chat-logs-bucket" } })),
        (
            "s3 with dots and digits in the bucket name",
            json!({ "s3": { "bucketName": "chat.logs.2026-archive" } }),
        ),
        (
            "cloudWatchLogs",
            json!({ "cloudWatchLogs": { "logGroupName": "/aws/ivschat/room-logs" } }),
        ),
        (
            "firehose",
            json!({ "firehose": { "deliveryStreamName": "chat-logs-stream" } }),
        ),
    ]
}

/// A logging-configuration ARN for `id` in `region`, in the form the modelled
/// `LoggingConfigurationArn` pattern admits. Used both to predict the ARN a create
/// answers and to build identifiers for configurations that were never created.
fn logging_configuration_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ivschat:{region}:{ACCOUNT}:logging-configuration/{id}")
}

/// Create a logging configuration and return its response body, failing the test on any
/// other status.
async fn create_logging_configuration(emulator: &Emulator, body: &Value) -> Value {
    let (status, created) = emulator.call("CreateLoggingConfiguration", body).await;
    assert_eq!(
        status, 200,
        "case {body}: CreateLoggingConfiguration must answer 200, got {status} {created}"
    );
    created
}

#[tokio::test]
async fn create_logging_configuration_persists_every_destination_and_get_reads_it_back() {
    let emulator = Emulator::new();
    let active = CREATE_STATES
        .first()
        .copied()
        .expect("the modelled CreateLoggingConfigurationState enum has a member");

    // Rows differ in which union member the destination carries, in that member's value,
    // in whether `name` is present and in how many tags the request attaches.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for (case, destination) in destination_catalog() {
        cases.push((
            format!("{case}, no name and no tags"),
            json!({ "destinationConfiguration": destination }),
        ));
    }
    let (_, s3) = destination_catalog().remove(0);
    cases.push((
        "named, one tag".to_string(),
        json!({
            "destinationConfiguration": s3,
            "name": "chat-logs",
            "tags": { "team": "chat" },
        }),
    ));
    let (_, firehose) = destination_catalog().remove(3);
    cases.push((
        "named with underscores and dashes, three tags".to_string(),
        json!({
            "destinationConfiguration": firehose,
            "name": "chat_logs-2026",
            "tags": { "team": "chat", "env": "test", "owner": "ivschat" },
        }),
    ));

    let mut seen_ids = BTreeSet::new();
    for (case, request) in cases {
        let before = now_secs();
        let created = create_logging_configuration(&emulator, &request).await;

        let id = str_field(&created, "id");
        assert_eq!(
            id.len(),
            LOGGING_CONFIGURATION_ID_LEN,
            "case {case:?}: the modelled LoggingConfigurationID is 12 characters, got {id:?}"
        );
        assert!(
            id.bytes().all(|b| b.is_ascii_alphanumeric()),
            "case {case:?}: the modelled LoggingConfigurationID matches ^[a-zA-Z0-9]+$, got {id:?}"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {case:?}: id {id} was minted twice"
        );

        let arn = str_field(&created, "arn");
        assert_eq!(
            arn,
            logging_configuration_arn(REGION, &id),
            "case {case:?}: the ARN must name the signed region, the emulator account and the \
             minted id, got {arn}"
        );

        let state = str_field(&created, "state");
        assert_eq!(
            state, active,
            "case {case:?}: expected the modelled state {active}, got {state}"
        );

        // Every member the request carried comes back; one it omitted is absent rather
        // than invented.
        assert_eq!(
            created.get("destinationConfiguration"),
            request.get("destinationConfiguration"),
            "case {case:?}: the stored destination must be the one the request named, got {created}"
        );
        assert_eq!(
            created.get("name").and_then(Value::as_str),
            request.get("name").and_then(Value::as_str),
            "case {case:?}: expected the requested name, got {created}"
        );
        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            created.get("tags"),
            Some(&expected_tags),
            "case {case:?}: expected tags {expected_tags}, got {created}"
        );

        let create_time = seconds(&created, "createTime");
        let update_time = seconds(&created, "updateTime");
        assert_eq!(
            create_time, update_time,
            "case {case:?}: a fresh configuration was never updated after it was created, \
             got createTime {create_time} and updateTime {update_time}"
        );
        assert!(
            (before - 5..=now_secs() + 5).contains(&create_time),
            "case {case:?}: createTime {create_time} must sit at the time of the call \
             ({before}..{})",
            now_secs()
        );

        // Round trip: the reader answers exactly what the create answered.
        let (status, fetched) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLoggingConfiguration must answer 200, got {status} {fetched}"
        );
        assert_eq!(
            fetched, created,
            "case {case:?}: GetLoggingConfiguration must read back what the create persisted, \
             got {fetched}"
        );
    }
}

#[tokio::test]
async fn a_logging_configuration_is_minted_per_create_even_for_identical_requests() {
    let emulator = Emulator::new();

    // Rows differ in the region the caller signs for: the ARN names that region, and the
    // id is fresh either way, so `name` need not be unique.
    let mut arns = BTreeSet::new();
    for region in [REGION, "eu-west-2", "ap-southeast-1"] {
        for _ in 0..2 {
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateLoggingConfiguration",
                    &json!({
                        "name": "shared-name",
                        "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {region}: expected 200, got {status} {created}"
            );
            let arn = str_field(&created, "arn");
            assert_eq!(
                arn,
                logging_configuration_arn(region, &str_field(&created, "id")),
                "case {region}: the ARN must carry the signed region"
            );
            assert!(
                arns.insert(arn.clone()),
                "case {region}: two creates answered the same ARN {arn}"
            );
        }
    }
    assert_eq!(
        arns.len(),
        6,
        "six creates must mint six configurations, got {arns:?}"
    );
}

#[tokio::test]
async fn get_update_and_delete_refuse_an_identifier_that_names_no_configuration() {
    let emulator = Emulator::new();
    // One stored configuration, so the refusals below are about the identifier and not
    // about an empty store.
    create_logging_configuration(
        &emulator,
        &json!({ "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } } }),
    )
    .await;

    // Rows differ in the region, the id's length and the id's alphabet; every one is a
    // well-formed `LoggingConfigurationIdentifier` that names nothing stored.
    for identifier in [
        logging_configuration_arn(REGION, "aaaaaaaaaaaa"),
        logging_configuration_arn(REGION, "Zz09"),
        logging_configuration_arn("eu-west-2", "aaaaaaaaaaaa"),
        logging_configuration_arn("ap-southeast-1", "absent-config-id"),
    ] {
        for (operation, request) in [
            (
                "GetLoggingConfiguration",
                json!({ "identifier": identifier }),
            ),
            (
                "UpdateLoggingConfiguration",
                json!({ "identifier": identifier, "name": "renamed" }),
            ),
            (
                "DeleteLoggingConfiguration",
                json!({ "identifier": identifier }),
            ),
        ] {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 404,
                "case {operation} {identifier:?}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} {identifier:?}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn update_logging_configuration_applies_only_the_members_the_request_carries() {
    // Rows differ in which members the update carries, and in the destination member the
    // update switches to: an absent member leaves the stored value as it was.
    let cases: Vec<(&str, Value, Option<&str>, Option<Value>)> = vec![
        ("nothing but the identifier", json!({}), None, None),
        (
            "name only",
            json!({ "name": "renamed-config" }),
            Some("renamed-config"),
            None,
        ),
        (
            "destination only, switched to cloudWatchLogs",
            json!({
                "destinationConfiguration": { "cloudWatchLogs": { "logGroupName": "/aws/ivschat/updated" } }
            }),
            None,
            Some(json!({ "cloudWatchLogs": { "logGroupName": "/aws/ivschat/updated" } })),
        ),
        (
            "destination only, switched to firehose",
            json!({
                "destinationConfiguration": { "firehose": { "deliveryStreamName": "updated-stream" } }
            }),
            None,
            Some(json!({ "firehose": { "deliveryStreamName": "updated-stream" } })),
        ),
        (
            "name and destination together",
            json!({
                "name": "both-updated",
                "destinationConfiguration": { "s3": { "bucketName": "other-logs-bucket" } }
            }),
            Some("both-updated"),
            Some(json!({ "s3": { "bucketName": "other-logs-bucket" } })),
        ),
    ];

    for (case, patch, expected_name, expected_destination) in cases {
        let emulator = Emulator::new();
        let created = create_logging_configuration(
            &emulator,
            &json!({
                "name": "original-config",
                "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } },
                "tags": { "team": "chat" },
            }),
        )
        .await;
        let arn = str_field(&created, "arn");

        let mut request = patch.clone();
        request["identifier"] = json!(arn);
        let (status, updated) = emulator.call("UpdateLoggingConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateLoggingConfiguration must answer 200, got {status} {updated}"
        );

        assert_eq!(
            str_field(&updated, "arn"),
            arn,
            "case {case:?}: an update must not move the configuration's ARN"
        );
        assert_eq!(
            str_field(&updated, "id"),
            str_field(&created, "id"),
            "case {case:?}: an update must not re-mint the configuration's id"
        );
        assert_eq!(
            updated.get("name").and_then(Value::as_str),
            Some(expected_name.unwrap_or("original-config")),
            "case {case:?}: expected name {expected_name:?} (unchanged when absent), got {updated}"
        );
        let wanted_destination = expected_destination
            .clone()
            .unwrap_or_else(|| json!({ "s3": { "bucketName": "chat-logs-bucket" } }));
        assert_eq!(
            updated.get("destinationConfiguration"),
            Some(&wanted_destination),
            "case {case:?}: expected destination {wanted_destination} (unchanged when absent), \
             got {updated}"
        );
        assert_eq!(
            updated.get("tags"),
            created.get("tags"),
            "case {case:?}: an update that names no tags must leave the stored tags alone, \
             got {updated}"
        );
        assert_eq!(
            seconds(&updated, "createTime"),
            seconds(&created, "createTime"),
            "case {case:?}: an update must not move createTime"
        );
        assert!(
            seconds(&updated, "updateTime") >= seconds(&created, "updateTime"),
            "case {case:?}: updateTime must not go backwards, got {} after {}",
            seconds(&updated, "updateTime"),
            seconds(&created, "updateTime")
        );

        // Round trip: the reader answers exactly what the update answered.
        let (status, fetched) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLoggingConfiguration must answer 200, got {status} {fetched}"
        );
        assert_eq!(
            fetched, updated,
            "case {case:?}: GetLoggingConfiguration must read back what the update persisted, \
             got {fetched}"
        );
    }
}

#[tokio::test]
async fn delete_logging_configuration_is_refused_while_any_room_still_has_it_attached() {
    // Rows differ in how many rooms hold the attachment: the refusal is about any room
    // holding it, not about one particular room.
    for attached_rooms in [1_usize, 2, 3] {
        let emulator = Emulator::new();
        let created = create_logging_configuration(
            &emulator,
            &json!({ "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } } }),
        )
        .await;
        let arn = str_field(&created, "arn");

        let mut room_arns = Vec::new();
        for index in 0..attached_rooms {
            let (status, room) = emulator
                .call(
                    "CreateRoom",
                    &json!({
                        "name": format!("attached-{index}"),
                        "loggingConfigurationIdentifiers": [arn],
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {attached_rooms} rooms: CreateRoom must answer 200, got {status} {room}"
            );
            room_arns.push(str_field(&room, "arn"));
        }

        let (status, body) = emulator
            .call("DeleteLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 409,
            "case {attached_rooms} rooms: expected the modelled HTTP 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {attached_rooms} rooms: expected ConflictException, got {body}"
        );

        // The refusal left the configuration in place.
        let (status, fetched) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {attached_rooms} rooms: a refused delete must leave the configuration \
             readable, got {status} {fetched}"
        );

        // Detaching every room through UpdateRoom clears the conflict, one room at a time:
        // while ANY room still holds it the delete stays refused.
        for (detached, room_arn) in room_arns.iter().enumerate() {
            let (status, room) = emulator
                .call(
                    "UpdateRoom",
                    &json!({ "identifier": room_arn, "loggingConfigurationIdentifiers": [] }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {attached_rooms} rooms: UpdateRoom must answer 200, got {status} {room}"
            );
            let (status, body) = emulator
                .call("DeleteLoggingConfiguration", &json!({ "identifier": arn }))
                .await;
            let still_attached = room_arns.len() - (detached + 1);
            if still_attached > 0 {
                assert_eq!(
                    status, 409,
                    "case {attached_rooms} rooms: with {still_attached} room(s) still attached \
                     the delete stays refused, got {status} {body}"
                );
            } else {
                assert_eq!(
                    status, 204,
                    "case {attached_rooms} rooms: with nothing attached the delete must answer \
                     the modelled 204, got {status} {body}"
                );
            }
        }

        // And the delete removed it from every later read.
        let (status, body) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {attached_rooms} rooms: a deleted configuration must be gone, got {status} {body}"
        );
        let (status, listed) = emulator.call("ListLoggingConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {attached_rooms} rooms: ListLoggingConfigurations must answer 200, got {status} {listed}"
        );
        let remaining = listed
            .get("loggingConfigurations")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("loggingConfigurations missing from {listed}"));
        assert!(
            remaining.is_empty(),
            "case {attached_rooms} rooms: the deleted configuration must be gone from the list, \
             got {listed}"
        );
    }
}

#[tokio::test]
async fn list_logging_configurations_pages_every_stored_configuration_exactly_once() {
    // Rows differ in how many configurations the store holds and in the page size asked
    // for: following the tokens to the end must report each one exactly once.
    for (stored, max_results) in [
        (0_usize, None),
        (1, None),
        (3, None),
        (5, Some(2)),
        (7, Some(3)),
        (4, Some(1)),
        (4, Some(50)),
    ] {
        let emulator = Emulator::new();
        let mut expected = BTreeSet::new();
        for index in 0..stored {
            let created = create_logging_configuration(
                &emulator,
                &json!({
                    "name": format!("config-{index}"),
                    "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } },
                }),
            )
            .await;
            expected.insert(str_field(&created, "id"));
        }

        let case = format!("{stored} stored, maxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, listed) = emulator.call("ListLoggingConfigurations", &request).await;
            assert_eq!(
                status, 200,
                "case {case}: ListLoggingConfigurations must answer 200, got {status} {listed}"
            );
            let page = listed
                .get("loggingConfigurations")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: loggingConfigurations missing from {listed}"))
                .clone();
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max as usize,
                    "case {case}: a page must not exceed maxResults, got {} entries",
                    page.len()
                );
            }
            for summary in &page {
                // A summary carries the same members the reader does.
                assert_eq!(
                    summary.get("state").and_then(Value::as_str),
                    Some("ACTIVE"),
                    "case {case}: expected the stored state in the summary, got {summary}"
                );
                assert!(
                    summary.get("destinationConfiguration").is_some(),
                    "case {case}: a summary must carry the stored destination, got {summary}"
                );
                seen.push(str_field(summary, "id"));
            }
            pages += 1;
            assert!(
                pages <= stored + 2,
                "case {case}: pagination must terminate, got {pages} pages"
            );
            match listed.get("nextToken").and_then(Value::as_str) {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: no configuration may appear twice across pages, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case}: following every token must report each stored configuration, \
             got {unique:?}"
        );
    }
}

#[tokio::test]
async fn list_logging_configurations_refuses_a_token_it_never_issued() {
    let emulator = Emulator::new();
    create_logging_configuration(
        &emulator,
        &json!({ "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } } }),
    )
    .await;

    // Rows differ in the shape of the bogus token: hand-built, truncated hex, plain text.
    for token in ["not-a-token", "zzzz", "0", "6e6f74616e6964"] {
        let (status, body) = emulator
            .call("ListLoggingConfigurations", &json!({ "nextToken": token }))
            .await;
        if token == "6e6f74616e6964" {
            // Decodes to "notanid": a token this service could have issued in form, naming
            // an id the store does not hold, so it resumes past nothing and answers a page.
            assert_eq!(
                status, 200,
                "case {token:?}: a decodable token must answer a page, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn create_logging_configuration_is_refused_once_the_store_holds_the_region_bound() {
    let emulator = Emulator::new();

    // Fill the shared store to the bound through the state the handlers read, so the
    // refusal is exercised over the wire without one HTTP create per record.
    {
        let state = emulator.engine.state::<ChatState>();
        let mut store = state.lock();
        let now = SystemTime::now();
        for index in 0..LOGGING_CONFIGURATIONS_PER_REGION {
            let id = format!("{index:012}");
            store.logging_configurations.insert(
                id.clone(),
                LoggingConfiguration {
                    arn: logging_configuration_arn(REGION, &id),
                    id,
                    name: None,
                    create_time: now,
                    update_time: now,
                    destination_configuration: DestinationConfiguration::S3(
                        S3DestinationConfiguration {
                            bucket_name: "chat-logs-bucket".to_string(),
                        },
                    ),
                    state: "ACTIVE".to_string(),
                    tags: BTreeMap::new(),
                },
            );
        }
    }

    // Rows differ in the request shape: the bound is about the store, not the request.
    for (case, request) in [
        (
            "an s3 destination",
            json!({ "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } } }),
        ),
        (
            "a named firehose destination",
            json!({
                "name": "over-bound",
                "destinationConfiguration": { "firehose": { "deliveryStreamName": "chat-logs-stream" } }
            }),
        ),
    ] {
        let (status, body) = emulator.call("CreateLoggingConfiguration", &request).await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected the modelled HTTP 402, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {body}"
        );
    }

    // One delete frees exactly one configuration's worth of room, and the next create
    // succeeds.
    let (status, body) = emulator
        .call(
            "DeleteLoggingConfiguration",
            &json!({ "identifier": logging_configuration_arn(REGION, "000000000000") }),
        )
        .await;
    assert_eq!(
        status, 204,
        "deleting a configuration under the bound must answer 204, got {status} {body}"
    );
    let (status, body) = emulator
        .call(
            "CreateLoggingConfiguration",
            &json!({ "destinationConfiguration": { "s3": { "bucketName": "chat-logs-bucket" } } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a create under the bound must succeed again, got {status} {body}"
    );
}

#[tokio::test]
async fn reading_a_logging_configuration_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let created = create_logging_configuration(
        &emulator,
        &json!({
            "name": "read-only",
            "destinationConfiguration": { "cloudWatchLogs": { "logGroupName": "/aws/ivschat/read" } },
            "tags": { "team": "chat" },
        }),
    )
    .await;
    let arn = str_field(&created, "arn");

    // Rows differ in the reporting call made in between; none of them may write.
    let (_, baseline) = emulator
        .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
        .await;
    for (case, request) in [
        (
            "GetLoggingConfiguration",
            json!({ "identifier": arn.clone() }),
        ),
        ("ListLoggingConfigurations", json!({})),
        (
            "ListLoggingConfigurations with a page size",
            json!({ "maxResults": 1 }),
        ),
    ] {
        let operation = case
            .split_whitespace()
            .next()
            .expect("the case names an operation");
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let (status, after) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the later read must still answer 200, got {status} {after}"
        );
        assert_eq!(
            after, baseline,
            "case {case:?}: a reporting call must not change what a later caller sees, \
             got {after}"
        );
    }
}
