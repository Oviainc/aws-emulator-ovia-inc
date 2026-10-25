// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of how this service WORDS `NotFoundException`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `chime` signing name — so what is asserted is the body an AWS
//! SDK client parses, not an internal return value.
//!
//! The rule under test: a `NotFoundException` reports WHICH situation the request met,
//! with `Code` `NotFound` and HTTP 404. An id that never named a resource here is reported
//! as that id being absent, worded from the REQUEST — the kind of resource asked for and
//! the id supplied for it (`Meeting <MeetingId> not found`, `Attendee <AttendeeId> not
//! found`) — so a caller that passed two ids can tell which one was wrong. A meeting that
//! EXISTED and has since been deleted is reported as having ENDED instead, because the
//! caller's id was right and no retry of it can ever succeed; `DeleteMeeting` is
//! documented as disconnecting every client and preventing new ones from joining, which
//! is exactly that outcome.
//!
//! The table spans the class the rule generalises over: which operation refuses (reader,
//! lister and writer, for both resource kinds), which resource is the absent one (the
//! meeting, or an attendee inside a meeting that exists), whether the id was never created
//! or was deleted, whether the id is caller-supplied or minted by this service, and the
//! form of the `GuidString` itself (lower-case hex, upper-case hex, both numeric
//! extremes). No row uses the id the issue recorded.

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

/// Well-formed `GuidString`s (modelled pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, from
/// `smithy_query(query="operation",operation="GetMeeting,GetAttendee",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`)
/// that name nothing in any store: the pattern's whole character class, spanning
/// lower-case hex, upper-case hex and both of its numeric extremes.
const ABSENT_IDS: &[(&str, &str)] = &[
    ("lower-case hex", "0c1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f"),
    ("upper-case hex", "0C1D2E3F-4A5B-4C6D-8E9F-0A1B2C3D4E5F"),
    ("all zeroes", "00000000-0000-0000-0000-000000000000"),
    ("all f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A named member of the body, as a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body.get(name).and_then(Value::as_str).map(str::to_string)
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

    /// Create a meeting and return its minted `MeetingId`.
    async fn new_meeting_id(&self, token: &str, external_meeting_id: &str) -> String {
        let reply = self
            .call(
                "POST",
                "/meetings",
                Some(&json!({
                    "ClientRequestToken": token,
                    "MediaRegion": "us-east-1",
                    "ExternalMeetingId": external_meeting_id,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "CreateMeeting {token:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        reply
            .body
            .get("Meeting")
            .and_then(|meeting| meeting.get("MeetingId"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateMeeting {token:?} returned no MeetingId"))
            .to_string()
    }

    async fn delete_meeting(&self, meeting_id: &str) -> Reply {
        self.call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await
    }
}

/// How an absent resource of `kind` with id `id` is reported.
fn absent_message(kind: &str, id: &str) -> String {
    format!("{kind} <{id}> not found")
}

/// How a meeting that existed and has since been deleted is reported.
const ENDED_MESSAGE: &str = "The meeting has ended";

/// Assert one refusal is the modelled `NotFoundException` carrying `expected`.
fn assert_not_found(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (404, Some("NotFoundException")),
        "case {case:?}: expected the modelled 404 NotFoundException, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.text("Code").as_deref(),
        Some("NotFound"),
        "case {case:?}: expected Code NotFound, got {:?}",
        reply.text("Code")
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(expected),
        "case {case:?}: expected Message {expected:?}, got {:?}",
        reply.text("Message")
    );
}

/// Every operation that resolves a `MeetingId` reports an id that never named a meeting
/// by naming it, and a meeting that was deleted as one that has ended.
#[tokio::test]
async fn an_absent_meeting_is_reported_by_the_situation_the_request_met() {
    let emulator = Emulator::new();

    // A meeting that DOES exist, so no row's refusal can be a store-wide failure.
    let present = emulator.new_meeting_id("tok-present", "ext-present").await;

    // (case, the absent MeetingId, the message that situation is reported with) — ids
    // never created, plus two minted by this service and then deleted.
    let mut rows: Vec<(String, String, String)> = ABSENT_IDS
        .iter()
        .map(|(case, id)| {
            (
                format!("never created, {case}"),
                (*id).to_string(),
                absent_message("Meeting", id),
            )
        })
        .collect();
    for (case, token) in [("with no attendees", "tok-deleted"), ("with one attendee", "tok-deleted-2")] {
        let deleted = emulator.new_meeting_id(token, &format!("ext-{token}")).await;
        if case == "with one attendee" {
            let created = emulator
                .call(
                    "POST",
                    &format!("/meetings/{deleted}/attendees"),
                    Some(&json!({ "ExternalUserId": "user-before-delete" })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "CreateAttendee before the delete: expected 200, got {} with {:?}",
                created.status, created.body
            );
        }
        let gone = emulator.delete_meeting(&deleted).await;
        assert_eq!(
            gone.status, 204,
            "DeleteMeeting on a meeting that exists: expected 204, got {} with {:?}",
            gone.status, gone.body
        );
        rows.push((
            format!("minted by the service {case}, then deleted"),
            deleted,
            ENDED_MESSAGE.to_string(),
        ));
    }

    for (case, meeting_id, expected) in &rows {
        // Every operation whose first lookup is the meeting: a reader, a lister, a
        // writer, and the two attendee operations that must resolve the meeting first.
        let attendee_id = ABSENT_IDS[0].1;
        let refusals: Vec<(&str, Reply)> = vec![
            (
                "GetMeeting",
                emulator
                    .call("GET", &format!("/meetings/{meeting_id}"), None)
                    .await,
            ),
            (
                "ListAttendees",
                emulator
                    .call("GET", &format!("/meetings/{meeting_id}/attendees"), None)
                    .await,
            ),
            (
                "CreateAttendee",
                emulator
                    .call(
                        "POST",
                        &format!("/meetings/{meeting_id}/attendees"),
                        Some(&json!({ "ExternalUserId": "user-absent-meeting" })),
                    )
                    .await,
            ),
            (
                "GetAttendee",
                emulator
                    .call(
                        "GET",
                        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                        None,
                    )
                    .await,
            ),
            (
                "DeleteAttendee",
                emulator
                    .call(
                        "DELETE",
                        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                        None,
                    )
                    .await,
            ),
        ];
        for (operation, reply) in refusals {
            assert_not_found(
                &format!("{operation} on a meeting {case}"),
                &reply,
                expected,
            );
        }
    }

    // The two situations are reported DIFFERENTLY — an id that never named a meeting is
    // not reported as a meeting that ended, and the reverse.
    assert!(
        rows.iter().any(|(_, _, message)| message == ENDED_MESSAGE)
            && rows
                .iter()
                .any(|(_, _, message)| message != ENDED_MESSAGE),
        "the table must cover both situations, got {:?}",
        rows.iter().map(|(case, _, _)| case).collect::<Vec<_>>()
    );

    // The meeting that exists is still readable, so the refusals above are about the ids
    // they named and nothing else.
    let intact = emulator
        .call("GET", &format!("/meetings/{present}"), None)
        .await;
    assert_eq!(
        intact.status, 200,
        "GetMeeting on the meeting that exists: expected 200, got {} with {:?}",
        intact.status, intact.body
    );
}

/// When the MEETING exists but the attendee does not, the refusal names the ATTENDEE and
/// the `AttendeeId` the request carried — so a caller that passed both ids can tell which
/// of the two was the absent one.
#[tokio::test]
async fn an_absent_attendee_is_named_by_its_own_id() {
    let emulator = Emulator::new();
    let meeting_id = emulator.new_meeting_id("tok-attendees", "ext-attendees").await;

    // An attendee that DOES exist in the same meeting, so the refusals below are about
    // the id each one names rather than about an empty meeting.
    let created = emulator
        .call(
            "POST",
            &format!("/meetings/{meeting_id}/attendees"),
            Some(&json!({ "ExternalUserId": "user-present" })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateAttendee: expected 200, got {} with {:?}",
        created.status, created.body
    );
    let present_attendee = created
        .body
        .get("Attendee")
        .and_then(|attendee| attendee.get("AttendeeId"))
        .and_then(Value::as_str)
        .expect("CreateAttendee returns an AttendeeId")
        .to_string();

    // (case, the absent AttendeeId) — every form the modelled pattern admits, plus the id
    // of an attendee this service minted and then deleted.
    let mut rows: Vec<(String, String)> = ABSENT_IDS
        .iter()
        .map(|(case, id)| (format!("never created, {case}"), (*id).to_string()))
        .collect();
    let deleted = emulator
        .call(
            "DELETE",
            &format!("/meetings/{meeting_id}/attendees/{present_attendee}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteAttendee on an attendee that exists: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    rows.push((
        "minted by the service, then deleted".to_string(),
        present_attendee,
    ));

    for (case, attendee_id) in &rows {
        let refusals: Vec<(&str, Reply)> = vec![
            (
                "GetAttendee",
                emulator
                    .call(
                        "GET",
                        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                        None,
                    )
                    .await,
            ),
            (
                "DeleteAttendee",
                emulator
                    .call(
                        "DELETE",
                        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
                        None,
                    )
                    .await,
            ),
        ];
        for (operation, reply) in refusals {
            assert_not_found(
                &format!("{operation} on an attendee {case}"),
                &reply,
                &absent_message("Attendee", attendee_id),
            );
        }
    }

    // The meeting itself is untouched: it is still readable after every refusal.
    let intact = emulator
        .call("GET", &format!("/meetings/{meeting_id}"), None)
        .await;
    assert_eq!(
        intact.status, 200,
        "GetMeeting after the attendee refusals: expected 200, got {} with {:?}",
        intact.status, intact.body
    );
}
