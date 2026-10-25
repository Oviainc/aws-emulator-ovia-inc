// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CreateMeetingWithAttendees`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! `POST /meetings?operation=create-attendees` signed for the `chime` signing name — so
//! what is asserted is what an AWS SDK client would see, not an internal return value.
//!
//! The rule under test: ONE call mints a meeting and the attendees the request lists, and
//! PERSISTS both, so every member of the answer is readable back through `GetMeeting`,
//! `ListAttendees` and `GetAttendee`. A request item the meeting cannot admit is reported
//! in the modelled `Errors` list — naming the `ExternalUserId` the caller sent — instead
//! of failing the call, and a replayed `ClientRequestToken` returns the first call's
//! meeting rather than minting a second one.
//!
//! The table spans the class the rule generalises over: how many attendees the request
//! carries (1, 3, and the modelled maximum of 20), whether each item names its own
//! capabilities or takes the meeting's documented defaults, and which items the meeting's
//! own `MeetingFeatures` admit (all of them, none of them, or only the first).

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

/// The modelled maximum length of `CreateMeetingWithAttendeesRequest$Attendees`
/// (`smithy_query(query="operation",operation="CreateMeetingWithAttendees",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`
/// -> `Attendees` `@length(1..20)`).
const MAX_ATTENDEES_PER_REQUEST: usize = 20;

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

    /// The operation under test: `POST /meetings?operation=create-attendees`, the URI the
    /// model binds it to.
    async fn create_meeting_with_attendees(&self, request: &Value) -> Reply {
        self.call("POST", "/meetings?operation=create-attendees", Some(request))
            .await
    }

    async fn get_meeting(&self, meeting_id: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    async fn list_attendees(&self, meeting_id: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}/attendees"), None)
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
}

/// Does `candidate` satisfy the modelled `GuidString` pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`?
/// (`smithy_query(query="shape",shape="Attendee")` / `shape="Meeting"`.)
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
fn attendees(prefix: &str, n: usize) -> Value {
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

/// One row: the request, the `ExternalUserId`s whose attendees the meeting must admit (in
/// request order), and the ones it must refuse into `Errors`.
struct Case {
    name: &'static str,
    request: Value,
    created: Vec<String>,
    refused: Vec<String>,
}

/// `CreateMeetingWithAttendees` persists the meeting AND every attendee the meeting
/// admits, reports the rest in `Errors`, and every record it returns reads back through
/// `GetMeeting` / `ListAttendees` / `GetAttendee`.
#[tokio::test]
async fn create_meeting_with_attendees_persists_the_meeting_and_its_attendees() {
    let twenty: Vec<String> = (0..MAX_ATTENDEES_PER_REQUEST)
        .map(|index| format!("user-max-{index}"))
        .collect();

    let cases: Vec<Case> = vec![
        Case {
            name: "a single attendee, required members only",
            request: json!({
                "ClientRequestToken": "tok-one",
                "MediaRegion": "us-east-1",
                "ExternalMeetingId": "ext-one",
                "Attendees": attendees("user-one", 1),
            }),
            created: vec!["user-one-0".to_string()],
            refused: Vec::new(),
        },
        Case {
            name: "three attendees in another Region, with meeting-level members",
            request: json!({
                "ClientRequestToken": "tok-three",
                "MediaRegion": "eu-west-2",
                "ExternalMeetingId": "ext-three",
                "MeetingHostId": "host-three",
                "TenantIds": ["tenant-a", "tenant-b"],
                "Tags": [{ "Key": "team", "Value": "collab" }],
                "MediaPlacementNetworkType": "DualStack",
                "Attendees": attendees("user-three", 3),
            }),
            created: vec![
                "user-three-0".to_string(),
                "user-three-1".to_string(),
                "user-three-2".to_string(),
            ],
            refused: Vec::new(),
        },
        Case {
            name: "the modelled maximum of twenty attendees",
            request: json!({
                "ClientRequestToken": "tok-max",
                "MediaRegion": "ap-southeast-2",
                "ExternalMeetingId": "ext-max",
                "Attendees": attendees("user-max", MAX_ATTENDEES_PER_REQUEST),
            }),
            created: twenty.clone(),
            refused: Vec::new(),
        },
        Case {
            name: "items that name their own capabilities",
            request: json!({
                "ClientRequestToken": "tok-caps",
                "MediaRegion": "us-west-2",
                "ExternalMeetingId": "ext-caps",
                "Attendees": [
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
                        "Capabilities": {
                            "Audio": "Send",
                            "Video": "None",
                            "Content": "None",
                        },
                    },
                    { "ExternalUserId": "user-caps-2" },
                ],
            }),
            created: vec![
                "user-caps-0".to_string(),
                "user-caps-1".to_string(),
                "user-caps-2".to_string(),
            ],
            refused: Vec::new(),
        },
        Case {
            // `AttendeeCapabilities`: with `Video:MaxResolution:None`, "all API requests
            // that include SendReceive, Send, or Receive for AttendeeCapabilities:Video
            // will be rejected" — per item, not per call, so the item that asks for no
            // capabilities at all is still created with the meeting's defaults.
            name: "one item whose capabilities the meeting's features forbid",
            request: json!({
                "ClientRequestToken": "tok-forbidden",
                "MediaRegion": "us-east-2",
                "ExternalMeetingId": "ext-forbidden",
                "MeetingFeatures": { "Video": { "MaxResolution": "None" } },
                "Attendees": [
                    {
                        "ExternalUserId": "user-forbidden-0",
                        "Capabilities": {
                            "Audio": "SendReceive",
                            "Video": "SendReceive",
                            "Content": "None",
                        },
                    },
                    { "ExternalUserId": "user-forbidden-1" },
                ],
            }),
            created: vec!["user-forbidden-1".to_string()],
            refused: vec!["user-forbidden-0".to_string()],
        },
        Case {
            // `AttendeeFeatures$MaxCount` is the meeting's attendee ceiling, so the items
            // past it are refused while the ones inside it are created.
            name: "more items than the meeting's Attendee.MaxCount admits",
            request: json!({
                "ClientRequestToken": "tok-ceiling",
                "MediaRegion": "ca-central-1",
                "ExternalMeetingId": "ext-ceiling",
                "MeetingFeatures": { "Attendee": { "MaxCount": 2 } },
                "Attendees": attendees("user-ceiling", 4),
            }),
            created: vec!["user-ceiling-0".to_string(), "user-ceiling-1".to_string()],
            refused: vec!["user-ceiling-2".to_string(), "user-ceiling-3".to_string()],
        },
    ];

    let emulator = Emulator::new();
    let mut minted_meeting_ids = BTreeSet::new();
    let mut minted_attendee_ids = BTreeSet::new();
    let mut minted_join_tokens = BTreeSet::new();

    for case in cases {
        let Case {
            name,
            request,
            created,
            refused,
        } = case;
        let reply = emulator.create_meeting_with_attendees(&request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_deref()),
            (200, None),
            "case {name:?}: expected HTTP 200 with no error code, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );

        // The meeting: minted, unique, and derived from its own id.
        let meeting = reply.member("Meeting");
        let meeting_id = string_member(&meeting, "MeetingId");
        assert!(
            is_modelled_guid(&meeting_id),
            "case {name:?}: expected a GuidString MeetingId, got {meeting_id:?}"
        );
        assert!(
            minted_meeting_ids.insert(meeting_id.clone()),
            "case {name:?}: MeetingId {meeting_id:?} was already minted for another meeting"
        );
        assert_eq!(
            string_member(&meeting, "ExternalMeetingId"),
            string_member(&request, "ExternalMeetingId"),
            "case {name:?}: the meeting must report the ExternalMeetingId it was asked for"
        );
        assert!(
            string_member(&meeting, "MeetingArn").ends_with(&format!("meeting/{meeting_id}")),
            "case {name:?}: expected a MeetingArn derived from {meeting_id:?}, got {:?}",
            meeting.get("MeetingArn")
        );

        // The attendees: exactly the items the meeting admits, in request order, each
        // with its own minted id and join token.
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

        // Round trip: the meeting and its attendee set are what the readers report.
        let read_meeting = emulator.get_meeting(&meeting_id).await;
        assert_eq!(
            read_meeting.status, 200,
            "case {name:?}: GetMeeting expected 200, got {} with {:?}",
            read_meeting.status, read_meeting.body
        );
        assert_eq!(
            read_meeting.member("Meeting"),
            meeting,
            "case {name:?}: GetMeeting must read back the meeting this call returned"
        );
        let listed = emulator.list_attendees(&meeting_id).await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: ListAttendees expected 200, got {} with {:?}",
            listed.status, listed.body
        );
        let listed_ids: BTreeSet<String> = external_user_ids(&listed.list("Attendees"))
            .into_iter()
            .collect();
        assert_eq!(
            listed_ids,
            created.iter().cloned().collect::<BTreeSet<String>>(),
            "case {name:?}: ListAttendees must hold exactly the attendees that were created"
        );
    }
}

/// A replayed `ClientRequestToken` answers with the meeting the first call created and its
/// attendees, and mints nothing new — whatever the second request's other members say.
///
/// The rows differ along what the replay carries: the identical request, the same token
/// with a different `ExternalMeetingId` and Region, and the same token with more
/// attendees.
#[tokio::test]
async fn a_replayed_client_request_token_returns_the_first_meeting() {
    let emulator = Emulator::new();
    let first = json!({
        "ClientRequestToken": "tok-replayed",
        "MediaRegion": "us-east-1",
        "ExternalMeetingId": "ext-replayed",
        "Attendees": attendees("user-replayed", 2),
    });

    let created = emulator.create_meeting_with_attendees(&first).await;
    assert_eq!(
        created.status, 200,
        "the first call expected 200, got {} with {:?}",
        created.status, created.body
    );
    let meeting = created.member("Meeting");
    let meeting_id = string_member(&meeting, "MeetingId");
    let attendee_ids: BTreeSet<String> = created
        .list("Attendees")
        .iter()
        .map(|attendee| string_member(attendee, "AttendeeId"))
        .collect();

    let replays: Vec<(&str, Value)> = vec![
        ("the identical request", first.clone()),
        (
            "the same token, another ExternalMeetingId and Region",
            json!({
                "ClientRequestToken": "tok-replayed",
                "MediaRegion": "eu-west-1",
                "ExternalMeetingId": "ext-replayed-again",
                "Attendees": attendees("user-replayed-other", 1),
            }),
        ),
        (
            "the same token with more attendees",
            json!({
                "ClientRequestToken": "tok-replayed",
                "MediaRegion": "us-east-1",
                "ExternalMeetingId": "ext-replayed",
                "Attendees": attendees("user-replayed-more", 5),
            }),
        ),
    ];

    for (case, request) in replays {
        let reply = emulator.create_meeting_with_attendees(&request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.member("Meeting"),
            meeting,
            "case {case:?}: a replay must answer with the meeting the first call created"
        );
        let replayed_ids: BTreeSet<String> = reply
            .list("Attendees")
            .iter()
            .map(|attendee| string_member(attendee, "AttendeeId"))
            .collect();
        assert_eq!(
            replayed_ids, attendee_ids,
            "case {case:?}: a replay must answer with the attendees the first call created"
        );

        // Nothing was added to the meeting either.
        let listed = emulator.list_attendees(&meeting_id).await;
        let listed_ids: BTreeSet<String> = listed
            .list("Attendees")
            .iter()
            .map(|attendee| string_member(attendee, "AttendeeId"))
            .collect();
        assert_eq!(
            listed_ids, attendee_ids,
            "case {case:?}: a replay must not create another attendee, got {listed_ids:?}"
        );
    }
}
