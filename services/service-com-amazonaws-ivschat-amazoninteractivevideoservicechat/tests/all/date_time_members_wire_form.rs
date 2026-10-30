// SPDX-License-Identifier: CC0-1.0

//! The wire FORM of every ivschat output member that targets a timestamp shape the model
//! marks `@timestampFormat("date-time")`.
//!
//! THE RULE UNDER TEST: on this service's restJson1 wire, such a member is serialized as an
//! ISO 8601 / RFC 3339 date-time STRING, never as a JSON number. Every timestamp member of
//! this service targets `com.amazonaws.ivschat#Time`, which
//! `smithy_query(query="shape", shape="com.amazonaws.ivschat#Time")` reports as
//! `"timestampFormat": "date-time"`, and each member's own doc says the value "is returned
//! as a string".
//!
//! Two halves, so the rule is pinned both at the schema the codec consults and on the bytes
//! a client reads:
//!
//! * the SCHEMA half walks every output shape of this service that declares a timestamp
//!   member and asserts the member schema carries the `date-time` format — the thing the
//!   JSON codec reads to choose the rendering
//!   (`vendor/smithy-schema-server/src/json.rs::write_timestamp`);
//! * the WIRE half drives each operation that reports one end-to-end and asserts the raw
//!   JSON value is a string that parses as a date-time, over a table of inputs that differ
//!   along what the rule generalises over (members present, value, count, the union member
//!   a destination carries, the requested session duration, single-resource vs list
//!   projection).
//!
//! The instants themselves are per-run clock values, so nothing here asserts a literal:
//! what is asserted is the ENCODING, and the invariants between the instants (a fresh
//! resource reports one instant twice; an update keeps `createTime` and advances
//! `updateTime`; a read answers the same string the write did; a token expiry sits the
//! requested span after the mint).

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::room_lifecycle::{str_field, Emulator};

/// The raw JSON at `pointer`, as the date-time string the rule requires: a JSON string that
/// parses in RFC 3339 `date-time` form. Answers the text and the instant it names.
///
/// This is the assertion the issue is about: a JSON NUMBER (restJson1's default
/// epoch-seconds rendering) fails on `as_str`.
fn date_time_member(case: &str, body: &Value, pointer: &str) -> (String, DateTime) {
    let raw = body
        .pointer(pointer)
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} missing from {body}"));
    let text = raw.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {pointer} must be an ISO 8601 date-time STRING, got {raw} \
             (a JSON number is restJson1's epoch-seconds default, not this member's \
             modelled form)"
        )
    });
    assert!(
        text.ends_with('Z'),
        "case {case:?}: {pointer} must name an instant in UTC with a trailing Z, got {text:?}"
    );
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: {pointer} = {text:?} must parse as an ISO 8601 date-time: {e}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {pointer} must re-render: {e}"));
    assert_eq!(
        rendered, text,
        "case {case:?}: {pointer} must round-trip through the date-time form, got \
         {rendered:?} from {text:?}"
    );
    (text.to_string(), parsed)
}

/// The element of `body[list]` whose `arn` is `arn`, so a list projection is read for the
/// very resource the single-resource calls reported.
fn element_by_arn(case: &str, body: &Value, list: &str, arn: &str) -> Value {
    let elements = body
        .get(list)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: {list} missing from {body}"));
    elements
        .iter()
        .find(|element| element.get("arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| panic!("case {case:?}: {list} carries no element for {arn}: {body}"))
        .clone()
}

/// Every output shape of this service that declares a `Time`-targeting member carries the
/// modelled `date-time` format on that member's schema — the value the JSON codec reads to
/// decide between a string and an epoch number.
#[test]
fn every_date_time_output_member_schema_declares_the_date_time_format() {
    use aws_smithy_schema::traits::TimestampFormat;
    use aws_smithy_schema::{Schema, ShapeType};
    use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::ops::types::{
        CreateChatTokenOutput, CreateLoggingConfigurationOutput, CreateRoomOutput,
        GetLoggingConfigurationOutput, GetRoomOutput, LoggingConfigurationSummary, RoomSummary,
        UpdateLoggingConfigurationOutput, UpdateRoomOutput,
    };

    // One row per shape of this service whose member list carries a `Time` member, as
    // `smithy_query(query="shape", shape=...)` reports them: the five single-resource
    // responses, the two summaries a list projection reports, and the two update responses.
    let rows: [(&str, &Schema); 9] = [
        ("CreateRoomResponse", &CreateRoomOutput::SCHEMA),
        ("GetRoomResponse", &GetRoomOutput::SCHEMA),
        ("UpdateRoomResponse", &UpdateRoomOutput::SCHEMA),
        ("RoomSummary", &RoomSummary::SCHEMA),
        ("CreateChatTokenResponse", &CreateChatTokenOutput::SCHEMA),
        (
            "CreateLoggingConfigurationResponse",
            &CreateLoggingConfigurationOutput::SCHEMA,
        ),
        (
            "GetLoggingConfigurationResponse",
            &GetLoggingConfigurationOutput::SCHEMA,
        ),
        (
            "UpdateLoggingConfigurationResponse",
            &UpdateLoggingConfigurationOutput::SCHEMA,
        ),
        (
            "LoggingConfigurationSummary",
            &LoggingConfigurationSummary::SCHEMA,
        ),
    ];

    for (case, schema) in rows {
        let timestamps: Vec<&&Schema> = schema
            .members()
            .iter()
            .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
            .collect();
        assert!(
            !timestamps.is_empty(),
            "case {case:?}: this shape declares timestamp members, so the walk must find one"
        );
        for member in timestamps {
            let format = member.timestamp_format().map(|carried| carried.format());
            assert_eq!(
                format,
                Some(TimestampFormat::DateTime),
                "case {case:?}: member {:?} must carry the modelled date-time format, got \
                 {format:?}",
                member.member_name()
            );
        }
    }
}

/// `CreateRoom`, `GetRoom`, `ListRooms` and `UpdateRoom` all report `createTime` and
/// `updateTime` as date-time strings, for every shape of create request.
#[tokio::test]
async fn room_responses_render_every_time_member_as_a_date_time_string() {
    // Rows differ along the members the create carries and their values — no name at all,
    // a name, each modelled bound of the two range-constrained knobs, tags, and a message
    // review handler — not a rename of one case.
    let rows: [(&str, Value); 5] = [
        ("no members at all", json!({})),
        ("a named room", json!({ "name": "lobby" })),
        (
            "both range knobs on their modelled minimum",
            json!({
                "name": "floor-1",
                "maximumMessageLength": 1,
                "maximumMessageRatePerSecond": 1
            }),
        ),
        (
            "both range knobs on their modelled maximum",
            json!({
                "name": "floor_2",
                "maximumMessageLength": 500,
                "maximumMessageRatePerSecond": 100
            }),
        ),
        (
            "tags and a message review handler",
            json!({
                "name": "reviewed-room",
                "tags": { "team": "chat", "tier": "gold" },
                "messageReviewHandler": {
                    "uri": "arn:aws:lambda:us-east-1:000000000000:function:review",
                    "fallbackResult": "DENY"
                }
            }),
        ),
    ];

    for (case, body) in rows {
        // A fresh emulator per row, so `ListRooms` reports exactly this row's room.
        let emulator = Emulator::new();

        let (status, created) = emulator.call("CreateRoom", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateRoom must answer 200, got {status} {created}"
        );
        let arn = str_field(&created, "arn");
        let (created_create, created_create_at) = date_time_member(case, &created, "/createTime");
        let (created_update, _) = date_time_member(case, &created, "/updateTime");
        assert_eq!(
            created_create, created_update,
            "case {case:?}: a fresh room reports one instant for both members, got \
             {created_create:?} and {created_update:?}"
        );

        let (status, read) = emulator
            .call("GetRoom", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRoom must answer 200, got {status} {read}"
        );
        let (read_create, _) = date_time_member(case, &read, "/createTime");
        let (read_update, _) = date_time_member(case, &read, "/updateTime");
        assert_eq!(
            (read_create.as_str(), read_update.as_str()),
            (created_create.as_str(), created_update.as_str()),
            "case {case:?}: GetRoom must answer the strings CreateRoom answered"
        );

        let (status, listed) = emulator.call("ListRooms", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRooms must answer 200, got {status} {listed}"
        );
        let summary = element_by_arn(case, &listed, "rooms", &arn);
        let (summary_create, _) = date_time_member(case, &summary, "/createTime");
        let (summary_update, _) = date_time_member(case, &summary, "/updateTime");
        assert_eq!(
            (summary_create.as_str(), summary_update.as_str()),
            (created_create.as_str(), created_update.as_str()),
            "case {case:?}: the ListRooms projection must answer the strings CreateRoom \
             answered"
        );

        let (status, updated) = emulator
            .call(
                "UpdateRoom",
                &json!({ "identifier": arn, "maximumMessageLength": 42 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRoom must answer 200, got {status} {updated}"
        );
        let (updated_create, _) = date_time_member(case, &updated, "/createTime");
        let (_, updated_update_at) = date_time_member(case, &updated, "/updateTime");
        assert_eq!(
            updated_create, created_create,
            "case {case:?}: an update keeps the room's original createTime"
        );
        assert!(
            updated_update_at.as_nanos() >= created_create_at.as_nanos(),
            "case {case:?}: updateTime must not move backwards, got {updated_update_at:?} \
             against {created_create_at:?}"
        );
    }
}

/// `CreateChatToken` reports `tokenExpirationTime` and `sessionExpirationTime` as date-time
/// strings, for every session duration the modelled range admits — and each instant sits
/// the span the request asked for after the mint.
#[tokio::test]
async fn chat_token_expiry_members_are_date_time_strings_at_the_requested_span() {
    /// The token's own validity window, as `crate::state::CHAT_TOKEN_LIFETIME` sets it.
    const TOKEN_LIFETIME_SECS: i64 = 60;
    /// `sessionDurationInMinutes`' default, from the model's own doc of that member
    /// ("Session duration (in minutes), after which the session expires. Default: 60 (1
    /// hour).").
    const DEFAULT_SESSION_MINUTES: i64 = 60;

    let emulator = Emulator::new();
    let (status, room) = emulator.call("CreateRoom", &json!({ "name": "tokens" })).await;
    assert_eq!(
        status, 200,
        "CreateRoom must answer 200 before a token is minted, got {status} {room}"
    );
    let room_arn = str_field(&room, "arn");

    // Rows differ along the requested duration: omitted (the documented default), each
    // published bound of `SessionDurationInMinutes` (`@range(min: 1, max: 180)`), and two
    // values inside it.
    let rows: [(&str, Option<i64>); 5] = [
        ("omitted, so the documented default", None),
        ("the modelled minimum", Some(1)),
        ("a value inside the range", Some(45)),
        ("the documented default, asked for", Some(60)),
        ("the modelled maximum", Some(180)),
    ];

    for (case, minutes) in rows {
        let mut body = json!({ "roomIdentifier": room_arn, "userId": "viewer-1" });
        if let Some(minutes) = minutes {
            body["sessionDurationInMinutes"] = json!(minutes);
        }

        let before = SystemTime::now();
        let (status, minted) = emulator.call("CreateChatToken", &body).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {case:?}: CreateChatToken must answer 200, got {status} {minted}"
        );

        let (_, token_expiry) = date_time_member(case, &minted, "/tokenExpirationTime");
        let (_, session_expiry) = date_time_member(case, &minted, "/sessionExpirationTime");

        let session_secs = minutes.unwrap_or(DEFAULT_SESSION_MINUTES) * 60;
        for (member, instant, span) in [
            ("tokenExpirationTime", token_expiry, TOKEN_LIFETIME_SECS),
            ("sessionExpirationTime", session_expiry, session_secs),
        ] {
            let window = (
                epoch_secs(before + Duration::from_secs(span as u64)) - 1,
                epoch_secs(after + Duration::from_secs(span as u64)) + 1,
            );
            assert!(
                instant.secs() >= window.0 && instant.secs() <= window.1,
                "case {case:?}: {member} must sit {span}s after the mint, got {} outside \
                 {window:?}",
                instant.secs()
            );
        }
    }
}

/// `CreateLoggingConfiguration`, `GetLoggingConfiguration`, `UpdateLoggingConfiguration`
/// and `ListLoggingConfigurations` all report `createTime` and `updateTime` as date-time
/// strings, whichever member of the destination union the configuration carries.
#[tokio::test]
async fn logging_configuration_responses_render_every_time_member_as_a_date_time_string() {
    // Rows differ along the member of the modelled `DestinationConfiguration` union the
    // create carries, and that destination's value, as
    // `smithy_query(query="shape", shape="DestinationConfiguration")` lists them: `s3`
    // (`bucketName`), `cloudWatchLogs` (`logGroupName`) and `firehose`
    // (`deliveryStreamName`). The last row also carries no name at all.
    let rows: [(&str, Value); 4] = [
        (
            "s3",
            json!({
                "name": "s3-logs",
                "destinationConfiguration": { "s3": { "bucketName": "chat-logs" } }
            }),
        ),
        (
            "cloudWatchLogs",
            json!({
                "name": "cwl-logs",
                "destinationConfiguration": {
                    "cloudWatchLogs": { "logGroupName": "/aws/ivschat/rooms" }
                }
            }),
        ),
        (
            "firehose",
            json!({
                "name": "firehose-logs",
                "destinationConfiguration": {
                    "firehose": { "deliveryStreamName": "chat-stream" }
                }
            }),
        ),
        (
            "no name, s3 with dots in the bucket name",
            json!({
                "destinationConfiguration": {
                    "s3": { "bucketName": "chat.logs.2026-archive" }
                }
            }),
        ),
    ];

    for (case, body) in rows {
        // A fresh emulator per row, so `ListLoggingConfigurations` reports exactly this
        // row's configuration.
        let emulator = Emulator::new();

        let (status, created) = emulator.call("CreateLoggingConfiguration", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateLoggingConfiguration must answer 200, got {status} {created}"
        );
        let arn = str_field(&created, "arn");
        let (created_create, _) = date_time_member(case, &created, "/createTime");
        let (created_update, _) = date_time_member(case, &created, "/updateTime");
        assert_eq!(
            created_create, created_update,
            "case {case:?}: a fresh logging configuration reports one instant for both \
             members, got {created_create:?} and {created_update:?}"
        );

        let (status, read) = emulator
            .call("GetLoggingConfiguration", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetLoggingConfiguration must answer 200, got {status} {read}"
        );
        let (read_create, _) = date_time_member(case, &read, "/createTime");
        let (read_update, _) = date_time_member(case, &read, "/updateTime");
        assert_eq!(
            (read_create.as_str(), read_update.as_str()),
            (created_create.as_str(), created_update.as_str()),
            "case {case:?}: GetLoggingConfiguration must answer the strings the create \
             answered"
        );

        let (status, listed) = emulator.call("ListLoggingConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListLoggingConfigurations must answer 200, got {status} {listed}"
        );
        let summary = element_by_arn(case, &listed, "loggingConfigurations", &arn);
        let (summary_create, _) = date_time_member(case, &summary, "/createTime");
        let (summary_update, _) = date_time_member(case, &summary, "/updateTime");
        assert_eq!(
            (summary_create.as_str(), summary_update.as_str()),
            (created_create.as_str(), created_update.as_str()),
            "case {case:?}: the ListLoggingConfigurations projection must answer the \
             strings the create answered"
        );

        let (status, updated) = emulator
            .call(
                "UpdateLoggingConfiguration",
                &json!({ "identifier": arn, "name": "renamed" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateLoggingConfiguration must answer 200, got {status} {updated}"
        );
        let (updated_create, updated_create_at) =
            date_time_member(case, &updated, "/createTime");
        let (_, updated_update_at) = date_time_member(case, &updated, "/updateTime");
        assert_eq!(
            updated_create, created_create,
            "case {case:?}: an update keeps the configuration's original createTime"
        );
        assert!(
            updated_update_at.as_nanos() >= updated_create_at.as_nanos(),
            "case {case:?}: updateTime must not move backwards, got {updated_update_at:?} \
             against {updated_create_at:?}"
        );
    }
}

/// `time` as whole seconds since the epoch, for comparing against a reported instant.
fn epoch_secs(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(SystemTime::UNIX_EPOCH)
            .expect("the test clock sits after the epoch")
            .as_secs(),
    )
    .expect("the test clock fits in an i64 of seconds")
}
