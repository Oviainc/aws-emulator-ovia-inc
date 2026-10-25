// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteMeeting`'s idempotence.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! `DELETE /meetings/{MeetingId}` signed for the `chime` signing name — so what is
//! asserted is what an AWS SDK client would see, not an internal return value.
//!
//! The rule under test: `DeleteMeeting` states an OUTCOME — the meeting it names is gone
//! when it returns — so it answers its modelled success status (204, empty body) whether
//! the meeting was there to remove or was already absent, and it removes nothing but the
//! meeting it names. Its READERS are the other half of that rule and keep their refusal:
//! `GetMeeting`, `ListAttendees` and `CreateAttendee` on a meeting that does not exist
//! answer the modelled `NotFoundException` (HTTP 404).
//!
//! The table spans the class the rule generalises over: whether the id was ever created,
//! how many times the same id is deleted, whether the meeting held attendees, and the
//! form of the `GuidString` itself (lower-case hex, upper-case hex, all-zero, all-f) —
//! every row a well-formed id by the model's pattern, none of them the id the issue
//! recorded.

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
/// `smithy_query(query="operation",operation="DeleteMeeting",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`)
/// that name no meeting in any store: the pattern's whole character class, spanning
/// lower-case hex, upper-case hex and both of its numeric extremes.
const NEVER_CREATED_IDS: &[(&str, &str)] = &[
    ("lower-case hex", "a1b2c3d4-e5f6-4789-8abc-def012345678"),
    ("upper-case hex", "A1B2C3D4-E5F6-4789-8ABC-DEF012345678"),
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
    /// A named member of a success body.
    fn member(&self, name: &str) -> Value {
        self.body
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("response has no `{name}` member: {self:?}"))
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

    async fn create_meeting(&self, token: &str, external_meeting_id: &str) -> Reply {
        self.call(
            "POST",
            "/meetings",
            Some(&json!({
                "ClientRequestToken": token,
                "MediaRegion": "us-east-1",
                "ExternalMeetingId": external_meeting_id,
            })),
        )
        .await
    }

    async fn create_attendee(&self, meeting_id: &str, external_user_id: &str) -> Reply {
        self.call(
            "POST",
            &format!("/meetings/{meeting_id}/attendees"),
            Some(&json!({ "ExternalUserId": external_user_id })),
        )
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

    async fn delete_meeting(&self, meeting_id: &str) -> Reply {
        self.call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    /// Create a meeting and return its minted `MeetingId`.
    async fn new_meeting_id(&self, token: &str, external_meeting_id: &str) -> String {
        let reply = self.create_meeting(token, external_meeting_id).await;
        assert_eq!(
            reply.status, 200,
            "CreateMeeting {token:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        reply
            .member("Meeting")
            .get("MeetingId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("CreateMeeting {token:?} returned no MeetingId"))
            .to_string()
    }
}

/// Assert one `DeleteMeeting` answered the modelled success: 204, no error code, and an
/// empty body (the operation's output shape is `Unit`).
fn assert_modelled_no_content(case: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (204, None),
        "case {case:?}: expected the modelled 204 with no error code, got {} {:?} with \
         {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.body,
        Value::Null,
        "case {case:?}: expected an empty body, got {:?}",
        reply.body
    );
}

/// `DeleteMeeting` on a well-formed id that was never created is a 204 no-op that leaves
/// nothing behind, and the readers of that id still answer the modelled
/// `NotFoundException`.
///
/// One row per form the modelled `GuidString` pattern admits, and every row is deleted
/// twice so that "already absent" is exercised both before and after a delete.
#[tokio::test]
async fn deleting_a_meeting_that_was_never_created_is_a_no_op() {
    let emulator = Emulator::new();

    // A meeting that DOES exist, to prove no row disturbs anything but the id it names.
    let bystander = emulator.new_meeting_id("tok-bystander", "ext-bystander").await;

    for (case, meeting_id) in NEVER_CREATED_IDS {
        for attempt in 1..=2 {
            let reply = emulator.delete_meeting(meeting_id).await;
            assert_modelled_no_content(&format!("{case} (attempt {attempt})"), &reply);
        }

        // Nothing was created by the delete: every reader of the id still refuses it.
        let readers: Vec<(&str, Reply)> = vec![
            ("GetMeeting", emulator.get_meeting(meeting_id).await),
            ("ListAttendees", emulator.list_attendees(meeting_id).await),
            (
                "CreateAttendee",
                emulator.create_attendee(meeting_id, "user-after-no-op").await,
            ),
        ];
        for (reader, reply) in readers {
            assert_eq!(
                (reply.status, reply.error_code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?}: {reader} on the never-created id expected the modelled 404 \
                 NotFoundException, got {} {:?} with {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }

        // And the meeting that does exist is untouched by the no-op.
        let intact = emulator.get_meeting(&bystander).await;
        assert_eq!(
            intact.status, 200,
            "case {case:?}: deleting an absent id must not disturb another meeting, \
             GetMeeting got {} with {:?}",
            intact.status, intact.body
        );
    }
}

/// Deleting a meeting that EXISTS removes it and answers 204, and every further delete of
/// the same id answers the same 204 — the operation reports the outcome it asks for,
/// which already holds.
///
/// The rows differ along what the meeting holds when it is deleted (no attendees, one,
/// several) and how many times the delete is repeated.
#[tokio::test]
async fn repeated_deletes_of_the_same_meeting_all_answer_the_modelled_204() {
    // (case, attendees to create first, how many times to delete)
    let cases: Vec<(&str, usize, usize)> = vec![
        ("a meeting with no attendees, deleted twice", 0, 2),
        ("a meeting with one attendee, deleted twice", 1, 2),
        ("a meeting with three attendees, deleted three times", 3, 3),
    ];

    let emulator = Emulator::new();
    for (case, attendee_count, deletes) in cases {
        let meeting_id = emulator
            .new_meeting_id(&format!("tok-{attendee_count}-{deletes}"), case)
            .await;
        for index in 0..attendee_count {
            let created = emulator
                .create_attendee(&meeting_id, &format!("user-{case}-{index}"))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateAttendee {index} expected 200, got {} with {:?}",
                created.status, created.body
            );
        }

        for attempt in 1..=deletes {
            let reply = emulator.delete_meeting(&meeting_id).await;
            assert_modelled_no_content(&format!("{case} (delete {attempt})"), &reply);

            // After every one of them the meeting is gone, so its readers refuse it.
            let read = emulator.get_meeting(&meeting_id).await;
            assert_eq!(
                (read.status, read.error_code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?}: after delete {attempt}, GetMeeting expected the modelled \
                 404 NotFoundException, got {} {:?} with {:?}",
                read.status,
                read.error_code,
                read.body
            );
            let listed = emulator.list_attendees(&meeting_id).await;
            assert_eq!(
                (listed.status, listed.error_code.as_deref()),
                (404, Some("NotFoundException")),
                "case {case:?}: after delete {attempt}, ListAttendees expected the modelled \
                 404 NotFoundException, got {} {:?} with {:?}",
                listed.status,
                listed.error_code,
                listed.body
            );
        }
    }
}
