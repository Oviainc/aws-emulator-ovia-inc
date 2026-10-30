// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the ivschat room lifecycle: `CreateRoom`, `GetRoom`,
//! `ListRooms`, `UpdateRoom`, `CreateChatToken` and `DeleteRoom`, driven over the wire
//! through the router the production entrypoint serves.
//!
//! Every table row is a different input along what the rule under test generalises over
//! (members present, value, size, count, ordering, page size, region), not a rename of
//! one case.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::state::{
    Room, ROOMS_PER_REGION,
};
use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::ComAmazonawsIvschatAmazoninteractivevideoservicechatState as ChatState;

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";
/// The region the test signs for unless a case names another.
pub(crate) const REGION: &str = "us-east-1";

/// The capability values the model's `ChatTokenCapability` enum carries, as the service
/// crate's generated catalog reports them.
const CAPABILITIES: &[&str] =
    service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::ops::types::CHAT_TOKEN_CAPABILITY_VALUES;

/// One emulator, one shared engine: exactly what the router hands every handler.
pub(crate) struct Emulator {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    pub(crate) async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVSCHAT/20260101/{region}/ivschat/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://localhost/{operation}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a room and return its response body, failing the test on any other status.
    async fn create_room(&self, body: &Value) -> Value {
        let (status, created) = self.call("CreateRoom", body).await;
        assert_eq!(
            status, 200,
            "case {body}: CreateRoom must answer 200, got {status} {created}"
        );
        created
    }
}

pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

pub(crate) fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member {member} missing from {body}"))
        .to_string()
}

pub(crate) fn seconds(body: &Value, member: &str) -> i64 {
    let raw = body
        .get(member)
        .unwrap_or_else(|| panic!("member {member} missing from {body}"));
    // The model gives `Time` the `date-time` timestamp format, so the member is on the wire
    // as an ISO 8601 string, never as restJson1's default epoch-seconds number. What this
    // suite asserts is the VALUE; `tests/all/date_time_members_wire_form.rs` owns the form.
    let text = raw
        .as_str()
        .unwrap_or_else(|| panic!("member {member} must be an ISO 8601 timestamp, got {raw}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("member {member} = {text:?} must parse as ISO 8601: {e}"))
        .secs()
}

pub(crate) fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
    )
    .expect("the clock fits in an i64")
}

/// A room ARN for a room that was never created, in `region`.
fn absent_room_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ivschat:{region}:{ACCOUNT}:room/{id}")
}

#[tokio::test]
async fn create_room_persists_every_member_and_get_room_reads_it_back() {
    let emulator = Emulator::new();

    // Rows differ in which members are present, and in the value, size and count of the
    // ones that are: the rule is that a create persists exactly what a later read answers.
    let cases = vec![
        ("no members at all", json!({})),
        ("name only", json!({ "name": "demo-room" })),
        ("empty name", json!({ "name": "" })),
        (
            "name using every character the pattern admits",
            json!({ "name": "Ab-9_z" }),
        ),
        (
            "both knobs at their lower bound",
            json!({ "maximumMessageRatePerSecond": 1, "maximumMessageLength": 1 }),
        ),
        (
            "both knobs at their upper bound",
            json!({ "maximumMessageRatePerSecond": 100, "maximumMessageLength": 500 }),
        ),
        (
            "one knob set, the other defaulted",
            json!({ "maximumMessageLength": 200 }),
        ),
        (
            "a message review handler",
            json!({
                "messageReviewHandler": {
                    "uri": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:review"),
                    "fallbackResult": "DENY"
                }
            }),
        ),
        (
            "tags",
            json!({ "name": "tagged", "tags": { "team": "chat", "env": "test" } }),
        ),
        (
            "the maximum three logging configurations",
            json!({
                "loggingConfigurationIdentifiers": [
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/abc123ABC456"),
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/def456DEF789"),
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/ghi789GHI012")
                ]
            }),
        ),
        (
            "every member at once",
            json!({
                "name": "everything",
                "maximumMessageRatePerSecond": 7,
                "maximumMessageLength": 321,
                "messageReviewHandler": {
                    "uri": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:all"),
                    "fallbackResult": "ALLOW"
                },
                "tags": { "only": "one" },
                "loggingConfigurationIdentifiers": [
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/abc123ABC456")
                ]
            }),
        ),
    ];

    for (case, request) in cases {
        let created = emulator.create_room(&request).await;

        let id = str_field(&created, "id");
        assert_eq!(
            id.chars().count(),
            12,
            "case {case:?}: a RoomID is 12 characters, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: a RoomID matches ^[a-zA-Z0-9]+$, got {id:?}"
        );
        assert_eq!(
            str_field(&created, "arn"),
            format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/{id}"),
            "case {case:?}: the ARN must name the region, account and generated id"
        );

        // Defaults for the knobs the request omitted, from the model's own documentation.
        let expected_rate = request
            .get("maximumMessageRatePerSecond")
            .and_then(Value::as_i64)
            .unwrap_or(10);
        let expected_length = request
            .get("maximumMessageLength")
            .and_then(Value::as_i64)
            .unwrap_or(500);
        assert_eq!(
            created.get("maximumMessageRatePerSecond").and_then(Value::as_i64),
            Some(expected_rate),
            "case {case:?}: expected maximumMessageRatePerSecond {expected_rate}, got {created}"
        );
        assert_eq!(
            created.get("maximumMessageLength").and_then(Value::as_i64),
            Some(expected_length),
            "case {case:?}: expected maximumMessageLength {expected_length}, got {created}"
        );

        // The members the request carried are persisted as given.
        for member in ["name", "messageReviewHandler"] {
            if let Some(sent) = request.get(member) {
                assert_eq!(
                    created.get(member),
                    Some(sent),
                    "case {case:?}: expected {member} {sent}, got {created}"
                );
            }
        }
        let expected_tags = request.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            created.get("tags"),
            Some(&expected_tags),
            "case {case:?}: expected tags {expected_tags}, got {created}"
        );
        let expected_logging = request
            .get("loggingConfigurationIdentifiers")
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            created.get("loggingConfigurationIdentifiers"),
            Some(&expected_logging),
            "case {case:?}: expected loggingConfigurationIdentifiers {expected_logging}, got {created}"
        );

        let create_time = seconds(&created, "createTime");
        assert_eq!(
            create_time,
            seconds(&created, "updateTime"),
            "case {case:?}: a fresh room was never updated after it was created"
        );
        assert!(
            (create_time - now_secs()).abs() < 300,
            "case {case:?}: createTime {create_time} must be the time of the call"
        );

        // The round trip: a get answers exactly what the create answered.
        let (status, fetched) = emulator
            .call("GetRoom", &json!({ "identifier": str_field(&created, "arn") }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRoom on a created room must answer 200, got {status} {fetched}"
        );
        assert_eq!(
            fetched, created,
            "case {case:?}: expected GetRoom to answer {created}, got {fetched}"
        );
    }
}

#[tokio::test]
async fn create_room_refuses_values_outside_the_modelled_bounds() {
    let emulator = Emulator::new();

    // One row per published bound, each just past it. Bounds from the model:
    // RoomMaxMessageRatePerSecond range 1..100, RoomMaxMessageLength range 1..500,
    // RoomName len ..128 pattern ^[a-zA-Z0-9-_]*$, LoggingConfigurationIdentifierList len 0..3.
    let cases = vec![
        (
            "rate below the lower bound",
            json!({ "maximumMessageRatePerSecond": 0 }),
        ),
        (
            "rate above the upper bound",
            json!({ "maximumMessageRatePerSecond": 101 }),
        ),
        (
            "length below the lower bound",
            json!({ "maximumMessageLength": 0 }),
        ),
        (
            "length above the upper bound",
            json!({ "maximumMessageLength": 501 }),
        ),
        (
            "name one character past its length bound",
            json!({ "name": "a".repeat(129) }),
        ),
        (
            "name outside its pattern",
            json!({ "name": "not a room name!" }),
        ),
        (
            "a fourth logging configuration",
            json!({
                "loggingConfigurationIdentifiers": [
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/abc123ABC456"),
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/def456DEF789"),
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/ghi789GHI012"),
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/jkl012JKL345")
                ]
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator.call("CreateRoom", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // And the bounds themselves are accepted, so the refusal is the bound and not the member.
    for (case, request) in [
        ("rate at 1", json!({ "maximumMessageRatePerSecond": 1 })),
        ("rate at 100", json!({ "maximumMessageRatePerSecond": 100 })),
        ("length at 1", json!({ "maximumMessageLength": 1 })),
        ("length at 500", json!({ "maximumMessageLength": 500 })),
        ("name at 128 characters", json!({ "name": "a".repeat(128) })),
    ] {
        let (status, body) = emulator.call("CreateRoom", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value ON the bound is accepted, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn every_created_room_gets_its_own_stable_id_and_arn() {
    let emulator = Emulator::new();

    let mut ids = BTreeSet::new();
    let mut arns = Vec::new();
    // The same request 25 times: the rule is that identity is minted per room, not derived
    // from the request.
    for index in 0..25 {
        let created = emulator.create_room(&json!({ "name": "same-name" })).await;
        let id = str_field(&created, "id");
        assert!(
            ids.insert(id.clone()),
            "room {index}: id {id} was handed out twice"
        );
        arns.push(str_field(&created, "arn"));
    }

    // And each one keeps the identity it was created with across later reads.
    for arn in &arns {
        let (status, first) = emulator.call("GetRoom", &json!({ "identifier": arn })).await;
        assert_eq!(status, 200, "GetRoom {arn} must answer 200, got {first}");
        let (_, second) = emulator.call("GetRoom", &json!({ "identifier": arn })).await;
        assert_eq!(
            first, second,
            "two reads of {arn} must answer the same record"
        );
        assert_eq!(
            str_field(&first, "arn"),
            *arn,
            "a room must report the ARN it was created with"
        );
    }
}

#[tokio::test]
async fn a_room_arn_names_the_region_the_caller_signed_for() {
    let emulator = Emulator::new();

    // Rows differ in the signed region; the ARN's region segment follows it.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let (status, created) = emulator
            .call_in(region, "CreateRoom", &json!({ "name": "regional" }))
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: CreateRoom must answer 200, got {created}"
        );
        let id = str_field(&created, "id");
        assert_eq!(
            str_field(&created, "arn"),
            format!("arn:aws:ivschat:{region}:{ACCOUNT}:room/{id}"),
            "case {region:?}: the ARN must carry the signed region"
        );
    }
}

#[tokio::test]
async fn reading_or_deleting_an_unknown_room_answers_resource_not_found() {
    let emulator = Emulator::new();
    // A real room exists, so the 404 is about the identifier and not about an empty store.
    emulator.create_room(&json!({ "name": "present" })).await;

    // Rows differ in region and in the id the ARN names.
    let unknown = [
        absent_room_arn(REGION, "abc123ABC456"),
        absent_room_arn("eu-west-2", "000000000000"),
        absent_room_arn(REGION, "zzzzzzzzzzzz"),
    ];

    for identifier in unknown {
        for operation in ["GetRoom", "DeleteRoom", "UpdateRoom"] {
            let (status, body) = emulator
                .call(operation, &json!({ "identifier": identifier }))
                .await;
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

        let (status, body) = emulator
            .call(
                "CreateChatToken",
                &json!({ "roomIdentifier": identifier, "userId": "alice" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case CreateChatToken {identifier:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case CreateChatToken {identifier:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn update_room_applies_only_the_members_the_request_carries() {
    let emulator = Emulator::new();

    // Rows differ in which member is updated, and in the value it is updated to.
    let cases = vec![
        ("name", json!({ "name": "renamed" })),
        ("name to the empty string", json!({ "name": "" })),
        (
            "rate at its lower bound",
            json!({ "maximumMessageRatePerSecond": 1 }),
        ),
        (
            "rate at its upper bound",
            json!({ "maximumMessageRatePerSecond": 100 }),
        ),
        ("length", json!({ "maximumMessageLength": 250 })),
        (
            "message review handler",
            json!({
                "messageReviewHandler": {
                    "uri": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:updated"),
                    "fallbackResult": "ALLOW"
                }
            }),
        ),
        (
            "logging configurations",
            json!({
                "loggingConfigurationIdentifiers": [
                    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/upd123UPD456")
                ]
            }),
        ),
        (
            "every updatable member at once",
            json!({
                "name": "all-updated",
                "maximumMessageRatePerSecond": 42,
                "maximumMessageLength": 99,
                "messageReviewHandler": { "uri": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:all"), "fallbackResult": "DENY" },
                "loggingConfigurationIdentifiers": []
            }),
        ),
        ("no updatable member at all", json!({})),
    ];

    for (case, patch) in cases {
        let created = emulator
            .create_room(&json!({
                "name": "before",
                "maximumMessageRatePerSecond": 5,
                "maximumMessageLength": 60,
                "tags": { "keep": "me" }
            }))
            .await;
        let arn = str_field(&created, "arn");

        let mut request = patch.clone();
        request["identifier"] = json!(arn);
        let (status, updated) = emulator.call("UpdateRoom", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRoom must answer 200, got {status} {updated}"
        );

        for member in [
            "name",
            "maximumMessageRatePerSecond",
            "maximumMessageLength",
            "messageReviewHandler",
            "loggingConfigurationIdentifiers",
        ] {
            let expected = patch.get(member).or_else(|| created.get(member));
            assert_eq!(
                updated.get(member),
                expected,
                "case {case:?}: expected {member} {expected:?}, got {updated}"
            );
        }

        // Identity, creation time and tags are not the update's business.
        for member in ["arn", "id", "createTime", "tags"] {
            assert_eq!(
                updated.get(member),
                created.get(member),
                "case {case:?}: {member} must survive an update unchanged, got {updated}"
            );
        }
        assert!(
            seconds(&updated, "updateTime") >= seconds(&created, "createTime"),
            "case {case:?}: updateTime must be bumped to the time of the update, got {updated}"
        );

        // The mutation is in the STORE, not only in the response.
        let (status, fetched) = emulator
            .call("GetRoom", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRoom after an update must answer 200, got {fetched}"
        );
        assert_eq!(
            fetched, updated,
            "case {case:?}: expected GetRoom to observe {updated}, got {fetched}"
        );
    }
}

#[tokio::test]
async fn list_rooms_pages_cover_every_room_exactly_once() {
    let emulator = Emulator::new();

    let mut expected = BTreeSet::new();
    for index in 0..7 {
        let created = emulator
            .create_room(&json!({ "name": format!("room-{index}") }))
            .await;
        expected.insert(str_field(&created, "id"));
    }

    // Exhaustive over the bounded space: every page size from one room per page to more
    // than the whole store, plus the default page size. The invariant is that following
    // nextToken to the end yields each room exactly once.
    let page_sizes: Vec<Option<i64>> = (1..=8).map(Some).chain(std::iter::once(None)).collect();
    for page_size in page_sizes {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = emulator.call("ListRooms", &request).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size:?}: ListRooms must answer 200, got {status} {body}"
            );
            let rooms = body
                .get("rooms")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case maxResults={page_size:?}: rooms missing: {body}"))
                .clone();
            if let Some(size) = page_size {
                assert!(
                    i64::try_from(rooms.len()).unwrap_or(i64::MAX) <= size,
                    "case maxResults={page_size:?}: a page may not exceed maxResults, got {body}"
                );
            }
            for room in &rooms {
                seen.push(str_field(room, "id"));
            }
            next_token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= 20,
                "case maxResults={page_size:?}: pagination did not terminate"
            );
            if next_token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size:?}: a room was returned twice: {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case maxResults={page_size:?}: expected every room exactly once, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_rooms_reports_the_modelled_order_and_an_empty_list_when_nothing_matches() {
    let emulator = Emulator::new();

    let (status, body) = emulator.call("ListRooms", &json!({})).await;
    assert_eq!(status, 200, "ListRooms on an empty store must answer 200");
    assert_eq!(
        body.get("rooms"),
        Some(&json!([])),
        "rooms is required, so an empty store answers an empty list, got {body}"
    );
    assert_eq!(
        body.get("nextToken"),
        None,
        "no token when there is nothing to resume from, got {body}"
    );

    let mut arns = Vec::new();
    for index in 0..4 {
        let created = emulator
            .create_room(&json!({ "name": format!("ordered-{index}") }))
            .await;
        arns.push(str_field(&created, "arn"));
    }
    // Touch the rooms in a known order, so the expected order is not the creation order:
    // the model documents "Results are sorted in descending order of updateTime".
    for arn in [&arns[1], &arns[3], &arns[0], &arns[2]] {
        let (status, _) = emulator
            .call("UpdateRoom", &json!({ "identifier": arn, "name": "touched" }))
            .await;
        assert_eq!(status, 200, "UpdateRoom {arn} must answer 200");
    }

    let (status, body) = emulator.call("ListRooms", &json!({})).await;
    assert_eq!(status, 200, "ListRooms must answer 200, got {body}");
    let rooms = body
        .get("rooms")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("rooms missing from {body}"));
    let times: Vec<i64> = rooms.iter().map(|room| seconds(room, "updateTime")).collect();
    let mut descending = times.clone();
    descending.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        times, descending,
        "expected updateTime descending, got {times:?}"
    );
}

#[tokio::test]
async fn list_rooms_narrows_on_each_filter_the_request_carries() {
    let emulator = Emulator::new();

    let handler_uri = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:reviewer");
    let other_handler_uri = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:other");
    let logging = format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/log123LOG456");
    let other_logging =
        format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/log789LOG012");

    let wanted = emulator
        .create_room(&json!({
            "name": "wanted",
            "messageReviewHandler": { "uri": handler_uri, "fallbackResult": "ALLOW" },
            "loggingConfigurationIdentifiers": [logging]
        }))
        .await;
    let wanted_id = str_field(&wanted, "id");
    emulator
        .create_room(&json!({
            "name": "unwanted",
            "messageReviewHandler": { "uri": other_handler_uri, "fallbackResult": "ALLOW" },
            "loggingConfigurationIdentifiers": [other_logging]
        }))
        .await;
    emulator.create_room(&json!({})).await;

    // One row per filter the model declares, each matching exactly the one room.
    let cases = vec![
        ("name", json!({ "name": "wanted" })),
        (
            "messageReviewHandlerUri",
            json!({ "messageReviewHandlerUri": handler_uri }),
        ),
        (
            "loggingConfigurationIdentifier",
            json!({ "loggingConfigurationIdentifier": logging }),
        ),
        (
            "every filter at once",
            json!({
                "name": "wanted",
                "messageReviewHandlerUri": handler_uri,
                "loggingConfigurationIdentifier": logging
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator.call("ListRooms", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRooms must answer 200, got {status} {body}"
        );
        let ids: Vec<String> = body
            .get("rooms")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: rooms missing from {body}"))
            .iter()
            .map(|room| str_field(room, "id"))
            .collect();
        assert_eq!(
            ids,
            vec![wanted_id.clone()],
            "case {case:?}: expected only the matching room, got {body}"
        );
    }

    // A filter that matches nothing answers the empty list, not every room.
    let (status, body) = emulator
        .call("ListRooms", &json!({ "name": "no-such-room" }))
        .await;
    assert_eq!(status, 200, "a filter matching nothing still answers 200");
    assert_eq!(
        body.get("rooms"),
        Some(&json!([])),
        "a filter matching nothing answers an empty list, got {body}"
    );
}

#[tokio::test]
async fn list_rooms_refuses_a_next_token_it_never_issued() {
    let emulator = Emulator::new();
    emulator.create_room(&json!({ "name": "one" })).await;

    // Rows differ in how the token is wrong: not hex, hex but not a position, and a
    // plausible-looking plaintext.
    for token in ["not-a-token", "zz", "6162", "abc123ABC456"] {
        let (status, body) = emulator
            .call("ListRooms", &json!({ "nextToken": token }))
            .await;
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
async fn create_chat_token_mints_a_unique_token_with_the_modelled_expirations() {
    let emulator = Emulator::new();
    let room = emulator.create_room(&json!({ "name": "tokens" })).await;
    let arn = str_field(&room, "arn");

    // Rows differ in session duration (both published bounds, a middle value, and the
    // documented default when the member is absent) and in the capabilities requested.
    let cases: Vec<(&str, Value, i64)> = vec![
        ("the documented default duration", json!({}), 60),
        ("the lower bound", json!({ "sessionDurationInMinutes": 1 }), 1),
        (
            "a middle value",
            json!({ "sessionDurationInMinutes": 90 }),
            90,
        ),
        (
            "the upper bound",
            json!({ "sessionDurationInMinutes": 180 }),
            180,
        ),
        (
            "every modelled capability",
            json!({ "capabilities": CAPABILITIES }),
            60,
        ),
        (
            "one capability and attributes",
            json!({
                "capabilities": [CAPABILITIES[CAPABILITIES.len() - 1]],
                "attributes": { "displayName": "Alice" }
            }),
            60,
        ),
    ];

    let mut tokens = BTreeSet::new();
    for (case, extra, expected_minutes) in cases {
        let mut request = extra.clone();
        request["roomIdentifier"] = json!(arn);
        request["userId"] = json!("alice");

        let before = now_secs();
        let (status, body) = emulator.call("CreateChatToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateChatToken must answer 200, got {status} {body}"
        );

        let token = str_field(&body, "token");
        assert!(!token.is_empty(), "case {case:?}: the token must not be empty");
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: token {token} was issued twice"
        );

        let token_expiry = seconds(&body, "tokenExpirationTime");
        let session_expiry = seconds(&body, "sessionExpirationTime");
        assert!(
            token_expiry > before,
            "case {case:?}: tokenExpirationTime {token_expiry} must be in the future"
        );
        let session_seconds = session_expiry - before;
        let expected_seconds = expected_minutes * 60;
        assert!(
            (session_seconds - expected_seconds).abs() <= 5,
            "case {case:?}: expected a session of about {expected_seconds}s, got {session_seconds}s"
        );
    }

    // Two identical calls: the token is minted per call, never a constant.
    let request = json!({ "roomIdentifier": arn, "userId": "alice" });
    let (_, first) = emulator.call("CreateChatToken", &request).await;
    let (_, second) = emulator.call("CreateChatToken", &request).await;
    assert_ne!(
        str_field(&first, "token"),
        str_field(&second, "token"),
        "two identical calls must not answer the same token"
    );
}

#[tokio::test]
async fn create_chat_token_accepts_every_modelled_capability_and_refuses_anything_else() {
    let emulator = Emulator::new();
    let room = emulator.create_room(&json!({ "name": "capabilities" })).await;
    let arn = str_field(&room, "arn");

    // The catalog comes from the service crate's generated `CHAT_TOKEN_CAPABILITY_VALUES`,
    // i.e. the model's own `ChatTokenCapability` enum.
    for capability in CAPABILITIES {
        let (status, body) = emulator
            .call(
                "CreateChatToken",
                &json!({
                    "roomIdentifier": arn,
                    "userId": "moderator",
                    "capabilities": [capability]
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {capability:?}: a modelled capability is accepted, got {status} {body}"
        );
    }

    for capability in ["send_message", "BAN_USER", ""] {
        let (status, body) = emulator
            .call(
                "CreateChatToken",
                &json!({
                    "roomIdentifier": arn,
                    "userId": "moderator",
                    "capabilities": [capability]
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {capability:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {capability:?}: expected ValidationException, got {body}"
        );
    }
}

#[tokio::test]
async fn delete_room_removes_the_room_from_every_later_read() {
    // Rows differ in how many rooms share the store, so a delete is shown to remove one
    // room and leave the rest.
    for room_count in 1..=3 {
        let emulator = Emulator::new();
        let mut arns = Vec::new();
        for index in 0..room_count {
            let created = emulator
                .create_room(&json!({ "name": format!("room-{index}") }))
                .await;
            arns.push(str_field(&created, "arn"));
        }
        let doomed = arns.remove(0);

        let (status, body) = emulator
            .call("DeleteRoom", &json!({ "identifier": doomed }))
            .await;
        assert_eq!(
            status, 204,
            "case {room_count} rooms: DeleteRoom answers the modelled 204, got {status} {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {room_count} rooms: a 204 carries no body, got {body}"
        );

        let (status, body) = emulator
            .call("GetRoom", &json!({ "identifier": doomed }))
            .await;
        assert_eq!(
            status, 404,
            "case {room_count} rooms: GetRoom after a delete is 404, got {status} {body}"
        );

        let (status, body) = emulator
            .call("DeleteRoom", &json!({ "identifier": doomed }))
            .await;
        assert_eq!(
            status, 404,
            "case {room_count} rooms: deleting twice is 404, got {status} {body}"
        );

        let (status, body) = emulator
            .call(
                "CreateChatToken",
                &json!({ "roomIdentifier": doomed, "userId": "alice" }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {room_count} rooms: a token cannot be minted for a deleted room, got {status} {body}"
        );

        let (status, listed) = emulator.call("ListRooms", &json!({})).await;
        assert_eq!(status, 200, "case {room_count} rooms: ListRooms must answer 200");
        let remaining: Vec<String> = listed
            .get("rooms")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("rooms missing from {listed}"))
            .iter()
            .map(|room| str_field(room, "arn"))
            .collect();
        assert!(
            !remaining.contains(&doomed),
            "case {room_count} rooms: ListRooms still reports the deleted room: {remaining:?}"
        );
        for survivor in &arns {
            assert!(
                remaining.contains(survivor),
                "case {room_count} rooms: ListRooms dropped {survivor}: {remaining:?}"
            );
        }
    }
}

#[tokio::test]
async fn create_room_is_refused_once_the_store_holds_the_region_quota() {
    let emulator = Emulator::new();

    // Fill the shared store to the quota through the state the handlers read, so the
    // refusal is exercised over the wire without 50,000 HTTP creates.
    {
        let state = emulator.engine.state::<ChatState>();
        let mut store = state.lock();
        let now = SystemTime::now();
        for index in 0..ROOMS_PER_REGION {
            let id = format!("{index:012}");
            store.rooms.insert(
                id.clone(),
                Room {
                    arn: format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/{id}"),
                    id,
                    name: None,
                    create_time: now,
                    update_time: now,
                    maximum_message_rate_per_second: 10,
                    maximum_message_length: 500,
                    message_review_handler: None,
                    logging_configuration_identifiers: Vec::new(),
                    tags: BTreeMap::new(),
                },
            );
        }
    }

    // Rows differ in the request shape: the quota is about the store, not the request.
    for (case, request) in [
        ("an empty request", json!({})),
        ("a named room", json!({ "name": "over-quota" })),
    ] {
        let (status, body) = emulator.call("CreateRoom", &request).await;
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

    // One delete frees exactly one room's worth of quota, and the next create succeeds.
    let (status, _) = emulator
        .call(
            "DeleteRoom",
            &json!({ "identifier": format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/000000000000") }),
        )
        .await;
    assert_eq!(status, 204, "deleting a room under quota must answer 204");
    let (status, body) = emulator.call("CreateRoom", &json!({})).await;
    assert_eq!(
        status, 200,
        "a create under the quota must succeed again, got {status} {body}"
    );
}

#[tokio::test]
async fn reading_a_room_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let created = emulator
        .create_room(&json!({ "name": "read-only", "maximumMessageLength": 123 }))
        .await;
    let arn = str_field(&created, "arn");

    // Rows differ in the reporting call made in between; none of them may write.
    let (_, baseline) = emulator.call("GetRoom", &json!({ "identifier": arn })).await;
    for case in ["GetRoom", "ListRooms"] {
        let request = if case == "GetRoom" {
            json!({ "identifier": arn })
        } else {
            json!({})
        };
        let (status, body) = emulator.call(case, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a reporting call must answer 200, got {status} {body}"
        );
        let (_, after) = emulator.call("GetRoom", &json!({ "identifier": arn })).await;
        assert_eq!(
            after, baseline,
            "case {case:?}: a read changed the stored room: expected {baseline}, got {after}"
        );
    }
}
