// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Amazon Chime SDK meeting + attendee lifecycle.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `chime` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: each operation of the meeting/attendee loop computes its answer
//! from PERSISTED state. A created record is readable field-for-field afterwards, the
//! server-owned members are derived from the minted id (so repeated reads agree), a
//! replayed idempotency token returns the first meeting, `ListAttendees` pages over
//! exactly what exists in a stable order, an attendee is reachable only through its own
//! meeting, and a deleted meeting (with its attendees) is gone.
//!
//! Each table spans the class the rule generalises over — media Region, which optional
//! members a request carries, which meeting features it asks for, which capabilities it
//! requests, attendee count and page size — rather than one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes (`OPERATIONS.md`, `## Wire protocol`). The scope is how the
/// router picks the claiming service. The signature itself is never checked here — the
/// harness turns request authentication off, exactly as an unauthenticated emulator runs.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT_ID: &str = "000000000000";

/// The values of the model's `MediaCapabilities` enum
/// (`smithy_query(query="shape",shape="MediaCapabilities",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`).
const MEDIA_CAPABILITIES: &[&str] = &["SendReceive", "Send", "Receive", "None"];

/// Every member of the modelled `MediaPlacement` shape, which `CreateMeeting` must fill
/// in (`smithy_query(query="shape",shape="MediaPlacement")`).
const MEDIA_PLACEMENT_MEMBERS: &[&str] = &[
    "AudioHostUrl",
    "AudioFallbackUrl",
    "SignalingUrl",
    "TurnControlUrl",
    "ScreenDataUrl",
    "ScreenSharingUrl",
    "ScreenViewingUrl",
    "EventIngestionUrl",
];

/// A well-formed `GuidString` that names nothing in the store, for the not-found cases.
const ABSENT_MEETING_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The `Meeting` / `Attendee` member of a success body.
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

    async fn create_meeting(&self, request: &Value) -> Reply {
        self.call("POST", "/meetings", Some(request)).await
    }

    async fn get_meeting(&self, meeting_id: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    async fn delete_meeting(&self, meeting_id: &str) -> Reply {
        self.call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    async fn create_attendee(&self, meeting_id: &str, request: &Value) -> Reply {
        self.call(
            "POST",
            &format!("/meetings/{meeting_id}/attendees"),
            Some(request),
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

    async fn delete_attendee(&self, meeting_id: &str, attendee_id: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
            None,
        )
        .await
    }

    async fn list_attendees(&self, meeting_id: &str, query: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}/attendees{query}"), None)
            .await
    }

    /// Create a meeting from `request` and return its minted `MeetingId`.
    async fn meeting_id_of_new_meeting(&self, request: &Value) -> String {
        let reply = self.create_meeting(request).await;
        assert_eq!(
            reply.status, 200,
            "CreateMeeting {request}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        string_member(&reply.member("Meeting"), "MeetingId")
    }

    /// Create an attendee and return its minted `AttendeeId`.
    async fn attendee_id_of_new_attendee(&self, meeting_id: &str, external_user_id: &str) -> String {
        let reply = self
            .create_attendee(meeting_id, &json!({ "ExternalUserId": external_user_id }))
            .await;
        assert_eq!(
            reply.status, 200,
            "CreateAttendee {external_user_id}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        string_member(&reply.member("Attendee"), "AttendeeId")
    }
}

/// Does `candidate` satisfy the modelled `GuidString` pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`?
/// (`smithy_query(query="shape",shape="Meeting")` / `shape="Attendee"`.)
fn is_modelled_guid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups
            .iter()
            .zip(widths)
            .all(|(group, width)| group.len() == width && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn string_member(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member `{member}` is not a string in {record:?}"))
        .to_string()
}

/// A `CreateMeeting` request body carrying only the three required members.
fn minimal_meeting(token: &str, region: &str, external_id: &str) -> Value {
    json!({
        "ClientRequestToken": token,
        "MediaRegion": region,
        "ExternalMeetingId": external_id,
    })
}

/// A `MeetingFeatures` configuration naming a video and a content maximum resolution.
fn features(video: Option<&str>, content: Option<&str>, max_attendees: Option<i32>) -> Value {
    let mut config = json!({});
    if let Some(video) = video {
        config["Video"] = json!({ "MaxResolution": video });
    }
    if let Some(content) = content {
        config["Content"] = json!({ "MaxResolution": content });
    }
    if let Some(max_attendees) = max_attendees {
        config["Attendee"] = json!({ "MaxCount": max_attendees });
    }
    config
}

/// `CreateMeeting` persists a meeting whose server-owned members are derived from the
/// minted `MeetingId`, and `GetMeeting` reads that record back unchanged.
///
/// The table spans the class the rule generalises over: the media Region (which the ARN
/// and every media endpoint are scoped to), and which optional members the request
/// carries (none, a host id, a primary meeting, 1 and 5 tenant ids, meeting features,
/// tags, a network type).
#[tokio::test]
async fn create_meeting_persists_a_meeting_derived_from_its_minted_id() {
    // `MediaRegion`'s own documentation lists the Regions a meeting may be created in;
    // these rows are drawn from it, including a GovCloud one.
    let cases: Vec<(&str, Value)> = vec![
        (
            "required members only",
            minimal_meeting("tok-minimal", "us-east-1", "ext-meeting-1"),
        ),
        (
            "another Region and a host id",
            json!({
                "ClientRequestToken": "tok-host",
                "MediaRegion": "eu-west-2",
                "ExternalMeetingId": "ext-meeting-2",
                "MeetingHostId": "host-user-2",
            }),
        ),
        (
            "one tenant id and a primary meeting",
            json!({
                "ClientRequestToken": "tok-tenant-1",
                "MediaRegion": "ap-southeast-2",
                "ExternalMeetingId": "ext-meeting-3",
                "TenantIds": ["tenant-a"],
                "PrimaryMeetingId": "primary-meeting-id",
            }),
        ),
        (
            "the modelled maximum of five tenant ids",
            json!({
                "ClientRequestToken": "tok-tenant-5",
                "MediaRegion": "sa-east-1",
                "ExternalMeetingId": "ext-meeting-4",
                "TenantIds": ["tenant-a", "tenant-b", "tenant-c", "tenant-d", "tenant-e"],
            }),
        ),
        (
            "meeting features, tags and a network type",
            json!({
                "ClientRequestToken": "tok-features",
                "MediaRegion": "us-gov-west-1",
                "ExternalMeetingId": "ext-meeting-5",
                "MeetingFeatures": {
                    "Audio": { "EchoReduction": "AVAILABLE" },
                    "Video": { "MaxResolution": "FHD" },
                    "Content": { "MaxResolution": "UHD" },
                    "Attendee": { "MaxCount": 25 },
                },
                "Tags": [{ "Key": "team", "Value": "collab" }],
                "MediaPlacementNetworkType": "DualStack",
            }),
        ),
        (
            "an ExternalMeetingId at the modelled minimum length of 2",
            minimal_meeting("tok-short-ext", "ca-central-1", "ab"),
        ),
    ];

    let emulator = Emulator::new();
    let mut minted_ids = BTreeSet::new();

    for (case, request) in cases {
        let reply = emulator.create_meeting(&request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateMeeting expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let meeting = reply.member("Meeting");
        let meeting_id = string_member(&meeting, "MeetingId");
        assert!(
            is_modelled_guid(&meeting_id),
            "case {case:?}: expected a GuidString MeetingId, got {meeting_id:?}"
        );
        assert!(
            minted_ids.insert(meeting_id.clone()),
            "case {case:?}: MeetingId {meeting_id} was already minted for another meeting"
        );

        let region = string_member(&request, "MediaRegion");
        assert_eq!(
            string_member(&meeting, "MeetingArn"),
            format!("arn:aws:chime:{region}:{ACCOUNT_ID}:meeting/{meeting_id}"),
            "case {case:?}: expected the meeting's ARN to name its id and media Region, got {}",
            meeting["MeetingArn"]
        );

        // Every modelled MediaPlacement member is filled in, and each endpoint is scoped
        // to the meeting's media Region.
        let placement = meeting
            .get("MediaPlacement")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no MediaPlacement in {meeting:?}"));
        for member in MEDIA_PLACEMENT_MEMBERS {
            let url = string_member(&placement, member);
            assert!(
                url.contains(&region),
                "case {case:?}: expected MediaPlacement.{member} to name the media Region \
                 {region}, got {url:?}"
            );
        }
        // The per-meeting endpoints carry the meeting's own id, which is what makes two
        // meetings' media placements differ.
        for member in ["AudioFallbackUrl", "SignalingUrl", "ScreenDataUrl"] {
            let url = string_member(&placement, member);
            assert!(
                url.contains(&meeting_id),
                "case {case:?}: expected MediaPlacement.{member} to name the MeetingId, got {url:?}"
            );
        }

        // The caller-owned members come back exactly as supplied, and no more.
        for member in [
            "ExternalMeetingId",
            "MediaRegion",
            "MeetingHostId",
            "PrimaryMeetingId",
            "MeetingFeatures",
        ] {
            assert_eq!(
                meeting.get(member),
                request.get(member),
                "case {case:?}: expected {member} to be stored as supplied ({:?}), got {:?}",
                request.get(member),
                meeting.get(member)
            );
        }
        // `TenantIds` belongs to the projection itself: the ids supplied, or the empty
        // list when the create named none (see `meeting_projection.rs`).
        let expected_tenant_ids = request
            .get("TenantIds")
            .cloned()
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            meeting.get("TenantIds"),
            Some(&expected_tenant_ids),
            "case {case:?}: expected TenantIds {expected_tenant_ids:?}, got {:?}",
            meeting.get("TenantIds")
        );

        // The round trip: GetMeeting reads back the same record, twice.
        for attempt in 1..=2u8 {
            let read = emulator.get_meeting(&meeting_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetMeeting #{attempt} expected 200, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.member("Meeting"),
                meeting,
                "case {case:?}: GetMeeting #{attempt} must report what CreateMeeting returned, got {:?}",
                read.member("Meeting")
            );
        }
    }
}

/// `ClientRequestToken` is `@idempotencyToken`: replaying a create returns the meeting the
/// first call created, and a different token creates a different meeting.
#[tokio::test]
async fn create_meeting_replays_the_meeting_its_client_request_token_created() {
    // Rows differ along the token itself (its length and alphabet, inside the modelled
    // `length 2..64, pattern ^[-_a-zA-Z0-9]*$`) and along the request it accompanies.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a two-character token, the modelled minimum",
            "ab",
            minimal_meeting("ab", "us-east-1", "ext-replay-1"),
        ),
        (
            "a token with the punctuation the pattern allows",
            "a-b_C9",
            minimal_meeting("a-b_C9", "us-west-2", "ext-replay-2"),
        ),
        (
            "a 64-character token, the modelled maximum",
            "0123456789012345678901234567890123456789012345678901234567890123",
            minimal_meeting(
                "0123456789012345678901234567890123456789012345678901234567890123",
                "eu-central-1",
                "ext-replay-3",
            ),
        ),
        (
            "a token on a request carrying optional members",
            "tok-with-options",
            json!({
                "ClientRequestToken": "tok-with-options",
                "MediaRegion": "ap-northeast-1",
                "ExternalMeetingId": "ext-replay-4",
                "TenantIds": ["tenant-z"],
            }),
        ),
    ];

    let emulator = Emulator::new();
    let mut by_token = Vec::new();

    for (case, token, request) in cases {
        let first = emulator.create_meeting(&request).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first CreateMeeting expected 200, got {} with {:?}",
            first.status, first.body
        );
        let created = first.member("Meeting");

        let replay = emulator.create_meeting(&request).await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: the replay expected 200, got {} with {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.member("Meeting"),
            created,
            "case {case:?}: replaying token {token} must return the first meeting, got {:?}",
            replay.member("Meeting")
        );
        by_token.push((case, token.to_string(), string_member(&created, "MeetingId")));
    }

    // Distinct tokens named distinct meetings.
    let distinct: BTreeSet<&String> = by_token.iter().map(|(_, _, id)| id).collect();
    assert_eq!(
        distinct.len(),
        by_token.len(),
        "each distinct ClientRequestToken must create its own meeting, got {by_token:?}"
    );

    // The idempotency record does not outlive the meeting: once the meeting is deleted,
    // the same token creates a new one.
    let (case, token, meeting_id) = by_token
        .first()
        .cloned()
        .expect("the table has at least one row");
    let deleted = emulator.delete_meeting(&meeting_id).await;
    assert_eq!(
        deleted.status, 204,
        "case {case:?}: DeleteMeeting expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    let after_delete = emulator
        .create_meeting(&minimal_meeting(&token, "us-east-1", "ext-replay-1"))
        .await;
    assert_eq!(
        after_delete.status, 200,
        "case {case:?}: CreateMeeting after the delete expected 200, got {} with {:?}",
        after_delete.status, after_delete.body
    );
    let remade = string_member(&after_delete.member("Meeting"), "MeetingId");
    assert_ne!(
        remade, meeting_id,
        "case {case:?}: reusing token {token} after the delete must create a new meeting, \
         got the deleted id {meeting_id}"
    );
}

/// An attendee created without `Capabilities` takes the defaults the model's
/// `AttendeeCapabilities` documentation states for its meeting's features, and
/// `GetAttendee` reads the stored attendee back field-for-field.
#[tokio::test]
async fn create_attendee_defaults_capabilities_from_the_meetings_features() {
    // (case, MeetingFeatures, expected Audio/Video/Content).
    //
    // The documented rules: all three default to `SendReceive`; but "If meeting features
    // is defined as Video:MaxResolution:None but Content:MaxResolution is defined as
    // something other than None and attendee capabilities are not defined in the API
    // request, then the default attendee video capability is set to Receive and attendee
    // content capability is set to SendReceive."
    let cases: Vec<(&str, Option<Value>, [&str; 3])> = vec![
        (
            "no MeetingFeatures at all",
            None,
            ["SendReceive", "SendReceive", "SendReceive"],
        ),
        (
            "the documented default resolutions, stated explicitly",
            Some(features(Some("HD"), Some("FHD"), None)),
            ["SendReceive", "SendReceive", "SendReceive"],
        ),
        (
            "the resolutions that reduce the attendee ceiling",
            Some(features(Some("FHD"), Some("UHD"), Some(25))),
            ["SendReceive", "SendReceive", "SendReceive"],
        ),
        (
            "Video:MaxResolution:None with a content resolution",
            Some(features(Some("None"), Some("FHD"), None)),
            ["SendReceive", "Receive", "SendReceive"],
        ),
        (
            "Video:MaxResolution:None with the content default left implicit",
            Some(features(Some("None"), None, None)),
            ["SendReceive", "Receive", "SendReceive"],
        ),
        (
            "both resolutions None",
            Some(features(Some("None"), Some("None"), None)),
            ["SendReceive", "None", "None"],
        ),
        (
            "Content:MaxResolution:None only",
            Some(features(None, Some("None"), None)),
            ["SendReceive", "SendReceive", "None"],
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, meeting_features, expected)) in cases.into_iter().enumerate() {
        let mut request = minimal_meeting(
            &format!("tok-defaults-{index}"),
            "us-east-1",
            &format!("ext-defaults-{index}"),
        );
        if let Some(meeting_features) = meeting_features {
            request["MeetingFeatures"] = meeting_features;
        }
        let meeting_id = emulator.meeting_id_of_new_meeting(&request).await;

        let reply = emulator
            .create_attendee(&meeting_id, &json!({ "ExternalUserId": "user-1" }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateAttendee expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let attendee = reply.member("Attendee");

        let attendee_id = string_member(&attendee, "AttendeeId");
        assert!(
            is_modelled_guid(&attendee_id),
            "case {case:?}: expected a GuidString AttendeeId, got {attendee_id:?}"
        );
        let join_token = string_member(&attendee, "JoinToken");
        assert!(
            join_token.len() >= 2 && join_token.len() <= 2048,
            "case {case:?}: expected a JoinToken inside the modelled length 2..2048, got \
             {} characters",
            join_token.len()
        );
        assert!(
            !join_token.contains(&attendee_id) && !join_token.contains(&meeting_id),
            "case {case:?}: the JoinToken must be opaque, not built from the ids: {join_token:?}"
        );
        assert_eq!(
            attendee["ExternalUserId"],
            json!("user-1"),
            "case {case:?}: expected the ExternalUserId as supplied, got {}",
            attendee["ExternalUserId"]
        );
        assert_eq!(
            attendee["Capabilities"],
            json!({ "Audio": expected[0], "Video": expected[1], "Content": expected[2] }),
            "case {case:?}: expected the documented default capabilities {expected:?}, got {}",
            attendee["Capabilities"]
        );

        // The round trip: GetAttendee reports the stored attendee, unchanged.
        let read = emulator.get_attendee(&meeting_id, &attendee_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAttendee expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("Attendee"),
            attendee,
            "case {case:?}: GetAttendee must report what CreateAttendee returned, got {:?}",
            read.member("Attendee")
        );
    }
}

/// Request-supplied `Capabilities` are stored as asked for, except in the three corner
/// cases the model's `AttendeeCapabilities` documentation refuses — each with HTTP 400.
#[tokio::test]
async fn create_attendee_stores_requested_capabilities_and_refuses_the_documented_corner_cases() {
    // (case, MeetingFeatures, requested Audio/Video/Content, expected status).
    let cases: Vec<(&str, Option<Value>, [&str; 3], u16)> = vec![
        (
            "every capability sending and receiving",
            None,
            ["SendReceive", "SendReceive", "SendReceive"],
            200,
        ),
        (
            "audio only",
            None,
            ["SendReceive", "None", "None"],
            200,
        ),
        (
            "send-only video with no content",
            None,
            ["Send", "Send", "None"],
            200,
        ),
        (
            "receiving content with receiving video",
            None,
            ["Receive", "Receive", "Receive"],
            200,
        ),
        (
            "nothing at all",
            None,
            ["None", "None", "None"],
            200,
        ),
        (
            "content SendReceive without any video",
            None,
            ["SendReceive", "None", "SendReceive"],
            400,
        ),
        (
            "content Receive with send-only video",
            None,
            ["SendReceive", "Send", "Receive"],
            400,
        ),
        (
            "content Send needs no video",
            None,
            ["SendReceive", "None", "Send"],
            200,
        ),
        (
            "video asked for on a Video:MaxResolution:None meeting",
            Some(features(Some("None"), Some("FHD"), None)),
            ["SendReceive", "Receive", "None"],
            400,
        ),
        (
            "video None accepted on a Video:MaxResolution:None meeting",
            Some(features(Some("None"), Some("FHD"), None)),
            ["SendReceive", "None", "None"],
            200,
        ),
        (
            "content asked for on a Content:MaxResolution:None meeting",
            Some(features(None, Some("None"), None)),
            ["SendReceive", "SendReceive", "Send"],
            400,
        ),
        (
            "content None accepted on a Content:MaxResolution:None meeting",
            Some(features(None, Some("None"), None)),
            ["SendReceive", "SendReceive", "None"],
            200,
        ),
    ];

    let emulator = Emulator::new();

    for (index, (case, meeting_features, requested, expected_status)) in
        cases.into_iter().enumerate()
    {
        for capability in requested {
            assert!(
                MEDIA_CAPABILITIES.contains(&capability),
                "case {case:?}: {capability:?} is not a modelled MediaCapabilities value"
            );
        }
        let mut request = minimal_meeting(
            &format!("tok-caps-{index}"),
            "us-east-1",
            &format!("ext-caps-{index}"),
        );
        if let Some(meeting_features) = meeting_features {
            request["MeetingFeatures"] = meeting_features;
        }
        let meeting_id = emulator.meeting_id_of_new_meeting(&request).await;

        let capabilities = json!({
            "Audio": requested[0],
            "Video": requested[1],
            "Content": requested[2],
        });
        let reply = emulator
            .create_attendee(
                &meeting_id,
                &json!({ "ExternalUserId": "user-caps", "Capabilities": capabilities }),
            )
            .await;
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {:?}",
            reply.status, reply.body
        );

        if expected_status == 200 {
            let attendee = reply.member("Attendee");
            assert_eq!(
                attendee["Capabilities"], capabilities,
                "case {case:?}: expected the requested capabilities to be stored, got {}",
                attendee["Capabilities"]
            );
            // And the listing agrees with the create, because both read one record.
            let listed = emulator.list_attendees(&meeting_id, "").await;
            assert_eq!(
                listed.member("Attendees"),
                json!([attendee]),
                "case {case:?}: ListAttendees must report the stored attendee, got {:?}",
                listed.member("Attendees")
            );
        } else {
            assert_eq!(
                reply.error_code.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected the modelled BadRequestException, got {:?}",
                reply.error_code
            );
            // A refused create stored nothing.
            let listed = emulator.list_attendees(&meeting_id, "").await;
            assert_eq!(
                listed.member("Attendees"),
                json!([]),
                "case {case:?}: a refused CreateAttendee must store nothing, got {:?}",
                listed.member("Attendees")
            );
        }
    }
}

/// `ListAttendees` pages over exactly the attendees that exist, each one once, whatever
/// the page size — and a page's `NextToken` resumes where the page stopped.
#[tokio::test]
async fn list_attendees_pages_over_every_attendee_exactly_once() {
    // Rows differ along attendee count and page size, including a page size of 1, a page
    // size that does not divide the count, the modelled maximum of 100, and no
    // `max-results` at all.
    let cases: &[(&str, usize, Option<i32>)] = &[
        ("no attendees at all", 0, None),
        ("one attendee, one per page", 1, Some(1)),
        ("three attendees, one per page", 3, Some(1)),
        ("five attendees, two per page", 5, Some(2)),
        ("seven attendees, three per page", 7, Some(3)),
        ("four attendees, the modelled maximum page", 4, Some(100)),
        ("six attendees, no max-results", 6, None),
    ];

    let emulator = Emulator::new();

    for (index, (case, count, max_results)) in cases.iter().enumerate() {
        let meeting_id = emulator
            .meeting_id_of_new_meeting(&minimal_meeting(
                &format!("tok-list-{index}"),
                "us-east-1",
                &format!("ext-list-{index}"),
            ))
            .await;
        let mut created = BTreeSet::new();
        for user in 0..*count {
            created.insert(
                emulator
                    .attendee_id_of_new_attendee(&meeting_id, &format!("user-{index}-{user}"))
                    .await,
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= count + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut query = Vec::new();
            if let Some(max_results) = max_results {
                query.push(format!("max-results={max_results}"));
            }
            if let Some(token) = &token {
                query.push(format!("next-token={token}"));
            }
            let query = if query.is_empty() {
                String::new()
            } else {
                format!("?{}", query.join("&"))
            };

            let reply = emulator.list_attendees(&meeting_id, &query).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: ListAttendees{query} expected 200, got {} with {:?}",
                reply.status, reply.body
            );
            let attendees = reply.member("Attendees");
            let page = attendees
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: Attendees is not a list: {attendees:?}"));
            if let Some(max_results) = max_results {
                let bound = usize::try_from(*max_results).unwrap_or(0);
                assert!(
                    page.len() <= bound,
                    "case {case:?}: expected at most {bound} attendees per page, got {}",
                    page.len()
                );
            }
            for attendee in page {
                seen.push(string_member(attendee, "AttendeeId"));
            }
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: paging returned an attendee twice: {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case:?}: paging must report exactly the attendees that exist, got {unique:?}"
        );
    }
}

/// A `next-token` this service did not issue is refused rather than silently read as the
/// start of the listing.
#[tokio::test]
async fn list_attendees_refuses_a_next_token_it_did_not_issue() {
    // Rows differ along the shape of the bad token: not hex, hex of the wrong payload,
    // and a bare attendee id.
    let cases: &[(&str, &str)] = &[
        ("not hexadecimal at all", "not-a-token"),
        ("hex of an untagged payload", "6162636465"),
        ("a bare AttendeeId", ABSENT_MEETING_ID),
    ];

    let emulator = Emulator::new();
    let meeting_id = emulator
        .meeting_id_of_new_meeting(&minimal_meeting("tok-bad-token", "us-east-1", "ext-bad-token"))
        .await;
    emulator
        .attendee_id_of_new_attendee(&meeting_id, "user-bad-token")
        .await;

    for (case, token) in cases {
        let reply = emulator
            .list_attendees(&meeting_id, &format!("?next-token={token}"))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for token {token:?}, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            reply.error_code
        );
    }

    // The empty token is a legal `NextToken` (modelled length 0..4096) and means "start
    // at the beginning".
    let empty = emulator.list_attendees(&meeting_id, "?next-token=").await;
    assert_eq!(
        empty.status, 200,
        "an empty next-token must start the listing, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.member("Attendees").as_array().map(Vec::len),
        Some(1),
        "an empty next-token must list every attendee, got {:?}",
        empty.member("Attendees")
    );
}

/// Every READER of the loop answers the modelled `NotFoundException` (HTTP 404) for an
/// id that is well formed but names nothing — and an attendee is reachable only through
/// its own meeting.
///
/// `DeleteMeeting` is NOT in this table: it states an outcome rather than reporting a
/// value, so an absent meeting is a no-op for it rather than a refusal — see
/// `tests/all/delete_meeting_idempotence.rs`, which owns that half of the rule.
#[tokio::test]
async fn unknown_meetings_and_attendees_are_not_found() {
    let emulator = Emulator::new();
    let meeting_id = emulator
        .meeting_id_of_new_meeting(&minimal_meeting("tok-404-a", "us-east-1", "ext-404-a"))
        .await;
    let other_meeting_id = emulator
        .meeting_id_of_new_meeting(&minimal_meeting("tok-404-b", "us-east-1", "ext-404-b"))
        .await;
    let attendee_id = emulator
        .attendee_id_of_new_attendee(&meeting_id, "user-404")
        .await;

    // Rows span every reader in the slice, over both kinds of unknown id.
    let cases: Vec<(&str, Reply)> = vec![
        ("GetMeeting on an absent meeting", emulator.get_meeting(ABSENT_MEETING_ID).await),
        (
            "ListAttendees on an absent meeting",
            emulator.list_attendees(ABSENT_MEETING_ID, "").await,
        ),
        (
            "CreateAttendee on an absent meeting",
            emulator
                .create_attendee(ABSENT_MEETING_ID, &json!({ "ExternalUserId": "user-x" }))
                .await,
        ),
        (
            "GetAttendee on an absent meeting",
            emulator.get_attendee(ABSENT_MEETING_ID, &attendee_id).await,
        ),
        (
            "GetAttendee on an absent attendee",
            emulator.get_attendee(&meeting_id, ABSENT_MEETING_ID).await,
        ),
        (
            "DeleteAttendee on an absent attendee",
            emulator
                .delete_attendee(&meeting_id, ABSENT_MEETING_ID)
                .await,
        ),
        (
            "GetAttendee through the wrong meeting",
            emulator.get_attendee(&other_meeting_id, &attendee_id).await,
        ),
        (
            "DeleteAttendee through the wrong meeting",
            emulator
                .delete_attendee(&other_meeting_id, &attendee_id)
                .await,
        ),
    ];

    for (case, reply) in cases {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled NotFoundException, got {:?}",
            reply.error_code
        );
    }

    // The attendee the wrong-meeting calls could not reach is still there.
    let intact = emulator.get_attendee(&meeting_id, &attendee_id).await;
    assert_eq!(
        intact.status, 200,
        "a call through another meeting must not disturb the attendee, got {} with {:?}",
        intact.status, intact.body
    );
}

/// `DeleteAttendee` and `DeleteMeeting` answer 204 with an empty body and remove exactly
/// what they name — the meeting taking its attendees with it.
#[tokio::test]
async fn deleting_removes_exactly_what_it_names() {
    let emulator = Emulator::new();
    let meeting_id = emulator
        .meeting_id_of_new_meeting(&minimal_meeting("tok-delete", "us-east-1", "ext-delete"))
        .await;
    let survivor = emulator
        .attendee_id_of_new_attendee(&meeting_id, "user-survivor")
        .await;
    let doomed = emulator
        .attendee_id_of_new_attendee(&meeting_id, "user-doomed")
        .await;

    let deleted = emulator.delete_attendee(&meeting_id, &doomed).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteAttendee expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body,
        Value::Null,
        "DeleteAttendee must answer an empty body, got {:?}",
        deleted.body
    );

    let gone = emulator.get_attendee(&meeting_id, &doomed).await;
    assert_eq!(
        gone.status, 404,
        "GetAttendee after the delete expected 404, got {} with {:?}",
        gone.status, gone.body
    );
    let listed = emulator.list_attendees(&meeting_id, "").await;
    let remaining: Vec<String> = listed
        .member("Attendees")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|attendee| string_member(attendee, "AttendeeId"))
        .collect();
    assert_eq!(
        remaining,
        vec![survivor.clone()],
        "ListAttendees after the delete must report only the survivor, got {remaining:?}"
    );

    // Deleting the same attendee twice is a 404: the first delete really removed it.
    let again = emulator.delete_attendee(&meeting_id, &doomed).await;
    assert_eq!(
        again.status, 404,
        "a second DeleteAttendee expected 404, got {} with {:?}",
        again.status, again.body
    );

    let dropped = emulator.delete_meeting(&meeting_id).await;
    assert_eq!(
        dropped.status, 204,
        "DeleteMeeting expected 204, got {} with {:?}",
        dropped.status, dropped.body
    );
    assert_eq!(
        dropped.body,
        Value::Null,
        "DeleteMeeting must answer an empty body, got {:?}",
        dropped.body
    );

    // The meeting took its attendees with it, so every READER of it is a 404.
    let after: Vec<(&str, Reply)> = vec![
        ("GetMeeting", emulator.get_meeting(&meeting_id).await),
        ("ListAttendees", emulator.list_attendees(&meeting_id, "").await),
        (
            "GetAttendee on the surviving attendee",
            emulator.get_attendee(&meeting_id, &survivor).await,
        ),
        (
            "CreateAttendee",
            emulator
                .create_attendee(&meeting_id, &json!({ "ExternalUserId": "user-late" }))
                .await,
        ),
    ];
    for (case, reply) in after {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: after DeleteMeeting expected 404, got {} with {:?}",
            reply.status, reply.body
        );
    }

    // Deleting it AGAIN is the no-op the operation's own rule makes it: the outcome it
    // asks for already holds, so it answers the modelled 204 and nothing is refused.
    let again_meeting = emulator.delete_meeting(&meeting_id).await;
    assert_eq!(
        (again_meeting.status, again_meeting.error_code.as_deref()),
        (204, None),
        "a second DeleteMeeting expected the modelled 204 no-op, got {} {:?} with {:?}",
        again_meeting.status,
        again_meeting.error_code,
        again_meeting.body
    );
}

