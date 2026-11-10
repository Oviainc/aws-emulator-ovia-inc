// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `BatchCreateAttendee`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! `POST /meetings/{MeetingId}/attendees?operation=batch-create` signed for the `chime`
//! signing name — so what is asserted is what an AWS SDK client would see, not an internal
//! return value.
//!
//! The rule under test: ONE call adds every attendee the request lists to the meeting it
//! names and PERSISTS each of them, minting that attendee's own `AttendeeId` and
//! `JoinToken`, so each record the call returns reads back through `GetAttendee` and
//! `ListAttendees`; an item the meeting cannot admit is reported in the modelled `Errors`
//! list naming the `ExternalUserId` the caller sent, instead of failing the call or the
//! other items; and a `MeetingId` naming no meeting fails the whole call with
//! `NotFoundException`, creating nothing.
//!
//! The table spans the class the rule generalises over: how many items the request carries
//! (1, 3, and the modelled maximum of 100), whether each item names its own capabilities
//! or takes the meeting's documented defaults, how many attendees the meeting already
//! holds when the batch arrives, and which items the meeting's own `MeetingFeatures`
//! admit (all of them, none of them, or only the ones inside its attendee ceiling).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes (`OPERATIONS.md`, `## Wire protocol`). The scope is how the
/// router picks the claiming service; the signature itself is never checked here, since
/// the harness turns request authentication off.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The modelled maximum length of `BatchCreateAttendeeRequest$Attendees`
/// (`smithy_query(query="operation",operation="BatchCreateAttendee",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`
/// -> `Attendees` `@length(1..100)`).
const MAX_ITEMS_PER_REQUEST: usize = 100;

/// The modelled bounds of `JoinTokenString` (`smithy_query(query="shape",
/// shape="JoinTokenString")` -> `@length(min: 2, max: 2048)`).
const JOIN_TOKEN_LENGTH: (usize, usize) = (2, 2048);

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A named member of a success body.
    fn member(&self, name: &str) -> Value {
        self.body
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("response has no `{name}` member: {self:?}"))
    }

    /// A named member read as a list.
    fn list(&self, name: &str) -> Vec<Value> {
        match self.member(name) {
            Value::Array(items) => items,
            other => panic!("member `{name}` is not a list: {other:?}"),
        }
    }

    /// The `message` of an error body, whatever casing the wire member carries.
    fn message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("error body carries no message: {self:?}"))
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with this service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://meetings-chime.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGNED_FOR_CHIME)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        // restJson1 carries the wire error code in `x-amzn-errortype`; fall back to the
        // body's `__type` so a missing header never reads as "no error code".
        let error_code = header_code.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        });
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// The operation under test: `POST /meetings/{MeetingId}/attendees?operation=batch-create`,
    /// the URI the model binds it to.
    async fn batch_create_attendee(&self, meeting_id: &str, items: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/meetings/{meeting_id}/attendees?operation=batch-create"),
            Some(&json!({ "Attendees": items })),
        )
        .await
    }

    async fn create_meeting(&self, request: &Value) -> Reply {
        self.call("POST", "/meetings", Some(request)).await
    }

    async fn create_attendee(&self, meeting_id: &str, external_user_id: &str) -> Reply {
        self.call(
            "POST",
            &format!("/meetings/{meeting_id}/attendees"),
            Some(&json!({ "ExternalUserId": external_user_id })),
        )
        .await
    }

    async fn delete_meeting(&self, meeting_id: &str) -> Reply {
        self.call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    async fn list_attendees(&self, meeting_id: &str) -> Reply {
        self.call(
            "GET",
            &format!("/meetings/{meeting_id}/attendees?MaxResults=100"),
            None,
        )
        .await
    }

    async fn get_attendee(&self, meeting_id: &str, attendee_id: &str) -> Reply {
        self.call(
            "GET",
            &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
            None,
        )
        .await
    }

    /// A meeting to batch into, with the `MeetingFeatures` the case asks for, answering
    /// its minted `MeetingId`.
    async fn meeting(&self, token: &str, region: &str, features: Option<Value>) -> String {
        let mut request = json!({
            "ClientRequestToken": token,
            "MediaRegion": region,
            "ExternalMeetingId": format!("ext-{token}"),
        });
        if let Some(features) = features {
            request["MeetingFeatures"] = features;
        }
        let reply = self.create_meeting(&request).await;
        assert_eq!(
            reply.status, 200,
            "CreateMeeting for {token:?} expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        string_member(&reply.member("Meeting"), "MeetingId")
    }
}

/// Does `candidate` satisfy the modelled `GuidString` pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`?
/// (`smithy_query(query="shape",shape="Attendee")` -> `AttendeeId`.)
fn is_modelled_guid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit())
        })
}

fn string_member(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member `{member}` is not a string in {record:?}"))
        .to_string()
}

/// `n` request items, each with its own `ExternalUserId` and no capabilities.
fn items(prefix: &str, n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "ExternalUserId": format!("{prefix}-{index}") }))
            .collect(),
    )
}

/// The `ExternalUserId`s of a list of `Attendee` / `CreateAttendeeError` records, in the
/// order the response carries them.
fn external_user_ids(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| string_member(record, "ExternalUserId"))
        .collect()
}

/// One row: the meeting's features, how many attendees it already holds, the request
/// items, and the `ExternalUserId`s the meeting must admit (in request order) versus the
/// ones it must refuse into `Errors`.
struct Case {
    name: &'static str,
    token: &'static str,
    region: &'static str,
    features: Option<Value>,
    /// `ExternalUserId`s created one at a time through `CreateAttendee` BEFORE the batch,
    /// so the batch is seen to extend a meeting that is already populated.
    existing: Vec<&'static str>,
    items: Value,
    created: Vec<String>,
    refused: Vec<String>,
}

/// `BatchCreateAttendee` persists every attendee the meeting admits, reports the rest in
/// `Errors`, and every record it returns reads back through `GetAttendee` / `ListAttendees`.
#[tokio::test]
async fn batch_create_attendee_persists_every_admitted_attendee() {
    let hundred: Vec<String> = (0..MAX_ITEMS_PER_REQUEST)
        .map(|index| format!("user-max-{index}"))
        .collect();

    let cases: Vec<Case> = vec![
        Case {
            name: "a single item, required members only",
            token: "tok-one",
            region: "us-east-1",
            features: None,
            existing: Vec::new(),
            items: items("user-one", 1),
            created: vec!["user-one-0".to_string()],
            refused: Vec::new(),
        },
        Case {
            name: "three items in another Region",
            token: "tok-three",
            region: "eu-west-2",
            features: None,
            existing: Vec::new(),
            items: items("user-three", 3),
            created: vec![
                "user-three-0".to_string(),
                "user-three-1".to_string(),
                "user-three-2".to_string(),
            ],
            refused: Vec::new(),
        },
        Case {
            name: "the modelled maximum of one hundred items",
            token: "tok-max",
            region: "ap-southeast-2",
            features: None,
            existing: Vec::new(),
            items: items("user-max", MAX_ITEMS_PER_REQUEST),
            created: hundred.clone(),
            refused: Vec::new(),
        },
        Case {
            name: "items that name their own capabilities",
            token: "tok-caps",
            region: "us-west-2",
            features: None,
            existing: Vec::new(),
            items: json!([
                {
                    "ExternalUserId": "user-caps-0",
                    "Capabilities": {
                        "Audio": "SendReceive",
                        "Video": "Receive",
                        "Content": "Receive",
                    },
                },
                {
                    "ExternalUserId": "user-caps-1",
                    "Capabilities": { "Audio": "Send", "Video": "None", "Content": "None" },
                },
                { "ExternalUserId": "user-caps-2" },
            ]),
            created: vec![
                "user-caps-0".to_string(),
                "user-caps-1".to_string(),
                "user-caps-2".to_string(),
            ],
            refused: Vec::new(),
        },
        Case {
            name: "a batch into a meeting that already holds attendees",
            token: "tok-existing",
            region: "us-east-2",
            features: None,
            existing: vec!["user-already-a", "user-already-b"],
            items: items("user-added", 3),
            created: vec![
                "user-added-0".to_string(),
                "user-added-1".to_string(),
                "user-added-2".to_string(),
            ],
            refused: Vec::new(),
        },
        Case {
            // `AttendeeCapabilities`: with `Video:MaxResolution:None`, "all API requests
            // that include SendReceive, Send, or Receive for AttendeeCapabilities:Video
            // will be rejected" — per item, not per call, so the item that names no
            // capabilities at all is still created with the meeting's defaults.
            name: "one item whose capabilities the meeting's features forbid",
            token: "tok-forbidden",
            region: "ca-central-1",
            features: Some(json!({ "Video": { "MaxResolution": "None" } })),
            existing: Vec::new(),
            items: json!([
                {
                    "ExternalUserId": "user-forbidden-0",
                    "Capabilities": {
                        "Audio": "SendReceive",
                        "Video": "SendReceive",
                        "Content": "None",
                    },
                },
                { "ExternalUserId": "user-forbidden-1" },
            ]),
            created: vec!["user-forbidden-1".to_string()],
            refused: vec!["user-forbidden-0".to_string()],
        },
        Case {
            // Every item's capabilities are forbidden, so the call still succeeds and
            // creates nothing: a per-item refusal never fails the call.
            name: "every item's capabilities forbidden by the meeting's features",
            token: "tok-all-forbidden",
            region: "eu-central-1",
            features: Some(json!({ "Content": { "MaxResolution": "None" } })),
            existing: Vec::new(),
            items: json!([
                {
                    "ExternalUserId": "user-none-0",
                    "Capabilities": {
                        "Audio": "SendReceive",
                        "Video": "SendReceive",
                        "Content": "Receive",
                    },
                },
                {
                    "ExternalUserId": "user-none-1",
                    "Capabilities": {
                        "Audio": "Send",
                        "Video": "SendReceive",
                        "Content": "Send",
                    },
                },
            ]),
            created: Vec::new(),
            refused: vec!["user-none-0".to_string(), "user-none-1".to_string()],
        },
        Case {
            // `AttendeeFeatures$MaxCount` is the meeting's attendee ceiling and the
            // attendees it ALREADY holds count against it, so only the items that fit in
            // the places left are created.
            name: "more items than the places left under Attendee.MaxCount",
            token: "tok-ceiling",
            region: "ap-northeast-1",
            features: Some(json!({ "Attendee": { "MaxCount": 3 } })),
            existing: vec!["user-seat-a"],
            items: items("user-ceiling", 4),
            created: vec![
                "user-ceiling-0".to_string(),
                "user-ceiling-1".to_string(),
            ],
            refused: vec![
                "user-ceiling-2".to_string(),
                "user-ceiling-3".to_string(),
            ],
        },
    ];

    let emulator = Emulator::new();
    let mut minted_attendee_ids = BTreeSet::new();
    let mut minted_join_tokens = BTreeSet::new();

    for case in cases {
        let Case {
            name,
            token,
            region,
            features,
            existing,
            items,
            created,
            refused,
        } = case;

        let meeting_id = emulator.meeting(token, region, features).await;
        let mut already: Vec<String> = Vec::new();
        for external_user_id in existing {
            let reply = emulator.create_attendee(&meeting_id, external_user_id).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: CreateAttendee for {external_user_id:?} expected 200, got {} \
                 with {:?}",
                reply.status, reply.body
            );
            already.push(external_user_id.to_string());
        }

        let reply = emulator.batch_create_attendee(&meeting_id, &items).await;
        assert_eq!(
            (reply.status, reply.error_code.as_deref()),
            (200, None),
            "case {name:?}: expected HTTP 200 with no error code, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );

        // The attendees: exactly the items the meeting admits, in request order, each with
        // its own minted id and join token.
        let returned = reply.list("Attendees");
        assert_eq!(
            external_user_ids(&returned),
            created,
            "case {name:?}: expected the admitted items in request order, got {returned:?}"
        );
        for (index, attendee) in returned.iter().enumerate() {
            let attendee_id = string_member(attendee, "AttendeeId");
            assert!(
                is_modelled_guid(&attendee_id),
                "case {name:?} item {index}: expected a GuidString AttendeeId, got \
                 {attendee_id:?}"
            );
            assert!(
                minted_attendee_ids.insert(attendee_id.clone()),
                "case {name:?} item {index}: AttendeeId {attendee_id:?} is not distinct"
            );
            let join_token = string_member(attendee, "JoinToken");
            assert!(
                (JOIN_TOKEN_LENGTH.0..=JOIN_TOKEN_LENGTH.1).contains(&join_token.chars().count()),
                "case {name:?} item {index}: expected a JoinToken inside the modelled \
                 length {JOIN_TOKEN_LENGTH:?}, got one of {} characters",
                join_token.chars().count()
            );
            assert!(
                minted_join_tokens.insert(join_token.clone()),
                "case {name:?} item {index}: JoinToken {join_token:?} is not distinct"
            );
            assert!(
                attendee.get("Capabilities").is_some(),
                "case {name:?} item {index}: an attendee must report its capabilities, got \
                 {attendee:?}"
            );

            // Round trip: the record this call returned is the record a reader reads.
            let read_back = emulator.get_attendee(&meeting_id, &attendee_id).await;
            assert_eq!(
                read_back.status, 200,
                "case {name:?} item {index}: GetAttendee expected 200, got {} with {:?}",
                read_back.status, read_back.body
            );
            assert_eq!(
                &read_back.member("Attendee"),
                attendee,
                "case {name:?} item {index}: GetAttendee must read back the created attendee"
            );
        }

        // The refused items: reported in `Errors`, naming the ExternalUserId sent, with a
        // code and a message — and nothing else is refused.
        let errors = reply.list("Errors");
        assert_eq!(
            external_user_ids(&errors),
            refused,
            "case {name:?}: expected exactly the refused items in Errors, got {errors:?}"
        );
        for (index, error) in errors.iter().enumerate() {
            assert!(
                !string_member(error, "ErrorCode").is_empty(),
                "case {name:?} error {index}: expected an ErrorCode, got {error:?}"
            );
            assert!(
                !string_member(error, "ErrorMessage").is_empty(),
                "case {name:?} error {index}: expected an ErrorMessage, got {error:?}"
            );
        }

        // Round trip: the meeting holds exactly what it held before plus what was created,
        // and no refused item left a trace.
        let listed = emulator.list_attendees(&meeting_id).await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: ListAttendees expected 200, got {} with {:?}",
            listed.status, listed.body
        );
        let listed_ids: BTreeSet<String> = external_user_ids(&listed.list("Attendees"))
            .into_iter()
            .collect();
        let expected: BTreeSet<String> = already.iter().chain(created.iter()).cloned().collect();
        assert_eq!(
            listed_ids, expected,
            "case {name:?}: ListAttendees must hold exactly the attendees that exist"
        );
    }
}

/// A `MeetingId` that names no meeting fails the WHOLE call with the modelled
/// `NotFoundException`, and creates nothing — whatever the items say.
///
/// The rows differ along WHY the id names no meeting: it never named one here, and it
/// named one that has since been deleted (whose attendees went with it).
#[tokio::test]
async fn a_meeting_id_that_names_no_meeting_is_not_found() {
    let emulator = Emulator::new();

    let unknown = "00000000-0000-4000-8000-000000000000";
    let deleted = emulator.meeting("tok-deleted", "us-east-1", None).await;
    let seeded = emulator.create_attendee(&deleted, "user-doomed").await;
    assert_eq!(
        seeded.status, 200,
        "CreateAttendee before the delete expected 200, got {} with {:?}",
        seeded.status, seeded.body
    );
    let dropped = emulator.delete_meeting(&deleted).await;
    assert_eq!(
        dropped.status, 204,
        "DeleteMeeting expected the modelled 204, got {} with {:?}",
        dropped.status, dropped.body
    );

    // The message of each refusal is the one this service words for that case: an id that
    // named no meeting names itself, an ended meeting reports its outcome.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "an id that never named a meeting",
            unknown,
            format!("Meeting <{unknown}> not found"),
        ),
        (
            "an id whose meeting has been deleted",
            &deleted,
            "The meeting has ended".to_string(),
        ),
    ];

    for (case, meeting_id, message) in rows {
        let reply = emulator
            .batch_create_attendee(meeting_id, &items("user-absent", 2))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            message,
            "case {case:?}: unexpected refusal message"
        );

        // Nothing was created: the meeting is still unreadable.
        let listed = emulator.list_attendees(meeting_id).await;
        assert_eq!(
            listed.status, 404,
            "case {case:?}: ListAttendees expected 404 after the refusal, got {} with {:?}",
            listed.status, listed.body
        );
    }
}
