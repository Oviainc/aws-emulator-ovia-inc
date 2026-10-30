// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateAttendeeCapabilities`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `chime` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: the call REPLACES the whole capability set of the stored attendee
//! the two path ids name, and the replacement is all-or-nothing. An admissible set is
//! applied whole (HTTP 200, the answer carrying the attendee with exactly that set, which
//! a following `GetAttendee` reads back); a set the meeting's `MeetingFeatures` do not
//! admit is refused with HTTP 400 and leaves the attendee's current set untouched; ids
//! that name no live meeting or no attendee of it are refused with HTTP 404 and write
//! nothing. Under simultaneous callers each one therefore ends as a success or as the
//! modelled contention refusal (HTTP 409 `ConflictException`), and the attendee is left
//! holding one applied set member-for-member rather than a mix of two.
//!
//! The tables span the class the rule generalises over: the whole cartesian product of the
//! modelled `MediaCapabilities` enum over the three capability members, three meeting
//! feature configurations that change which of those sets are admissible, which of the two
//! path ids is absent, and the NUMBER of simultaneous callers.

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

/// The values of the model's `MediaCapabilities` enum
/// (`smithy_query(query="shape",shape="MediaCapabilities",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`
/// -> `SendReceive | Send | Receive | None`).
const MEDIA_CAPABILITIES: &[&str] = &["SendReceive", "Send", "Receive", "None"];

/// A well-formed `GuidString` (modelled pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`) that names nothing here.
const ABSENT_GUID: &str = "0c1d2e3f-4a5b-4c6d-8e9f-0a1b2c3d4e5f";

/// A second absent `GuidString`, in the pattern's upper-case form.
const OTHER_ABSENT_GUID: &str = "0C1D2E3F-4A5B-4C6D-8E9F-0A1B2C3D4E5F";

/// How many callers race, per row of the contention table: the smallest race there is, an
/// odd count, and the four the issue's fan-out describes — the dimension that rule
/// generalises over is the NUMBER of simultaneous callers.
const RACER_COUNTS: &[usize] = &[2, 3, 4, 8];

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The `Capabilities` member of the `Attendee` this reply carries.
    fn capabilities(&self) -> Value {
        self.body
            .get("Attendee")
            .and_then(|attendee| attendee.get("Capabilities"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The whole `Attendee` record this reply carries.
    fn attendee(&self) -> Value {
        self.body.get("Attendee").cloned().unwrap_or(Value::Null)
    }

    /// A named member of the body, as a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body.get(name).and_then(Value::as_str).map(str::to_string)
    }
}

/// Send one request through the router and reduce the answer to a [`Reply`].
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<&Value>) -> Reply {
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

    let response = router.dispatch(request).await;
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
    // restJson1 carries the wire error code in `x-amzn-errortype`; fall back to the body's
    // `__type` so a missing header never reads as "no error code".
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

/// The emulator under test: ONE shared engine with this service mounted on it, reached the
/// way the production router reaches it. The router is CLONEABLE and every clone shares
/// that one engine, which is what lets the contention table fire overlapping calls.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Create a meeting with `features` (when any) and answer its minted `MeetingId`.
async fn new_meeting(router: &MultiServiceRouter, token: &str, features: Option<Value>) -> String {
    let mut request = json!({
        "ClientRequestToken": token,
        "MediaRegion": "us-east-1",
        "ExternalMeetingId": format!("ext-{token}"),
    });
    if let Some(features) = features {
        request["MeetingFeatures"] = features;
    }
    let reply = call(router, "POST", "/meetings", Some(&request)).await;
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

/// Create an attendee of `meeting_id` and answer its minted `AttendeeId`.
async fn new_attendee(router: &MultiServiceRouter, meeting_id: &str, user: &str) -> String {
    let reply = call(
        router,
        "POST",
        &format!("/meetings/{meeting_id}/attendees"),
        Some(&json!({ "ExternalUserId": user })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "CreateAttendee {user:?}: expected 200, got {} with {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("Attendee")
        .and_then(|attendee| attendee.get("AttendeeId"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateAttendee {user:?} returned no AttendeeId"))
        .to_string()
}

async fn update_capabilities(
    router: &MultiServiceRouter,
    meeting_id: &str,
    attendee_id: &str,
    capabilities: &Value,
) -> Reply {
    call(
        router,
        "PUT",
        &format!("/meetings/{meeting_id}/attendees/{attendee_id}/capabilities"),
        Some(&json!({ "Capabilities": capabilities })),
    )
    .await
}

async fn get_attendee(router: &MultiServiceRouter, meeting_id: &str, attendee_id: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/meetings/{meeting_id}/attendees/{attendee_id}"),
        None,
    )
    .await
}

/// A capability set as the wire carries it.
fn capability_set(audio: &str, video: &str, content: &str) -> Value {
    json!({ "Audio": audio, "Video": video, "Content": content })
}

/// Whether a `MediaCapabilities` value asks to RECEIVE media.
fn receives(capability: &str) -> bool {
    capability == "SendReceive" || capability == "Receive"
}

/// Whether a `MediaCapabilities` value asks to send or receive media at all.
fn sends_or_receives(capability: &str) -> bool {
    capability == "SendReceive" || capability == "Send" || capability == "Receive"
}

/// Why a meeting whose effective `MeetingFeatures` maximum resolutions are
/// `video_resolution` / `content_resolution` does not admit this set, or `None` when it
/// does.
///
/// The three rules are the corner cases the model's `AttendeeCapabilities` documentation
/// lists, restated here from that documentation so the table's expectation is independent
/// of the handler: a meeting whose video (content) maximum resolution is `None` admits only
/// `None` for that capability, and content `SendReceive`/`Receive` requires video
/// `SendReceive`/`Receive`. No documented rule constrains `Audio`, so it is not a parameter
/// here — every one of its four values is admissible in every meeting.
fn inadmissible(
    video: &str,
    content: &str,
    video_resolution: &str,
    content_resolution: &str,
) -> Option<&'static str> {
    if video_resolution == "None" && sends_or_receives(video) {
        return Some("video media in a meeting whose video maximum resolution is None");
    }
    if content_resolution == "None" && sends_or_receives(content) {
        return Some("content media in a meeting whose content maximum resolution is None");
    }
    if receives(content) && !receives(video) {
        return Some("content reception without video reception");
    }
    None
}

/// Assert a refusal is the modelled `NotFoundException`, worded as this service words one.
fn assert_not_found(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (404, Some("NotFoundException")),
        "case {case:?}: expected the modelled 404 NotFoundException, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(message),
        "case {case:?}: expected Message {message:?}, got {:?}",
        reply.text("Message")
    );
}

/// Every set the meeting admits is applied WHOLE and read back member-for-member; every set
/// it does not admit is refused with the modelled 400 and leaves the stored set as it was.
///
/// The table is the full cartesian product of the modelled `MediaCapabilities` enum over
/// `Audio`, `Video` and `Content` (64 sets), run against three meeting feature
/// configurations that move the admissibility boundary: the documented defaults, a meeting
/// whose video maximum resolution is `None`, and one whose content maximum resolution is
/// `None`.
#[tokio::test]
async fn an_admitted_capability_set_replaces_the_whole_stored_set() {
    let router = router();

    // (case, MeetingFeatures, the meeting's effective Video/Content MaxResolution). The
    // documented defaults are `HD` for video and `FHD` for content (`VideoFeatures`
    // /`ContentFeatures` `$MaxResolution`, "Defaults to …").
    let meetings: Vec<(&str, Option<Value>, &str, &str)> = vec![
        ("default features", None, "HD", "FHD"),
        (
            "video maximum resolution None",
            Some(json!({ "Video": { "MaxResolution": "None" } })),
            "None",
            "FHD",
        ),
        (
            "content maximum resolution None",
            Some(json!({ "Content": { "MaxResolution": "None" } })),
            "HD",
            "None",
        ),
    ];

    for (index, (meeting_case, features, video_resolution, content_resolution)) in
        meetings.into_iter().enumerate()
    {
        let meeting_id = new_meeting(&router, &format!("tok-caps-{index}"), features).await;
        let attendee_id = new_attendee(&router, &meeting_id, &format!("user-caps-{index}")).await;
        let created = get_attendee(&router, &meeting_id, &attendee_id).await;
        let external_user_id = created.attendee().get("ExternalUserId").cloned();
        let join_token = created.attendee().get("JoinToken").cloned();
        // What the attendee currently holds, so a refusal can be shown to change nothing.
        let mut stored = created.capabilities();
        let mut applied = 0usize;
        let mut refused = 0usize;

        for audio in MEDIA_CAPABILITIES {
            for video in MEDIA_CAPABILITIES {
                for content in MEDIA_CAPABILITIES {
                    let case = format!("{meeting_case}, Audio {audio} Video {video} Content {content}");
                    let requested = capability_set(audio, video, content);
                    let reply =
                        update_capabilities(&router, &meeting_id, &attendee_id, &requested).await;

                    if let Some(reason) =
                        inadmissible(video, content, video_resolution, content_resolution)
                    {
                        refused += 1;
                        assert_eq!(
                            (reply.status, reply.error_code.as_deref()),
                            (400, Some("BadRequestException")),
                            "case {case:?}: {reason} must be the modelled 400 \
                             BadRequestException, got {} {:?} with {:?}",
                            reply.status,
                            reply.error_code,
                            reply.body
                        );
                        let after = get_attendee(&router, &meeting_id, &attendee_id).await;
                        assert_eq!(
                            after.capabilities(),
                            stored,
                            "case {case:?}: a refused update must leave the stored set at \
                             {stored:?}, got {:?}",
                            after.capabilities()
                        );
                        continue;
                    }

                    applied += 1;
                    assert_eq!(
                        (reply.status, reply.error_code.as_deref()),
                        (200, None),
                        "case {case:?}: expected 200, got {} {:?} with {:?}",
                        reply.status,
                        reply.error_code,
                        reply.body
                    );
                    assert_eq!(
                        reply.capabilities(),
                        requested,
                        "case {case:?}: the answer must carry exactly the applied set, got {:?}",
                        reply.capabilities()
                    );
                    // The call updates the capabilities and nothing else: the attendee's
                    // other members are the ones its creation minted.
                    assert_eq!(
                        reply.attendee().get("AttendeeId").and_then(Value::as_str),
                        Some(attendee_id.as_str()),
                        "case {case:?}: the answer must carry the same AttendeeId, got {:?}",
                        reply.attendee()
                    );
                    assert_eq!(
                        reply.attendee().get("ExternalUserId").cloned(),
                        external_user_id,
                        "case {case:?}: ExternalUserId must be unchanged, got {:?}",
                        reply.attendee().get("ExternalUserId")
                    );
                    assert_eq!(
                        reply.attendee().get("JoinToken").cloned(),
                        join_token,
                        "case {case:?}: JoinToken must be unchanged, got {:?}",
                        reply.attendee().get("JoinToken")
                    );
                    // Persisted, not just answered: the reader sees the same set.
                    let read_back = get_attendee(&router, &meeting_id, &attendee_id).await;
                    assert_eq!(
                        read_back.capabilities(),
                        requested,
                        "case {case:?}: GetAttendee must read back the applied set, got {:?}",
                        read_back.capabilities()
                    );
                    // Applying the same set again is admissible too, and idempotent.
                    let again =
                        update_capabilities(&router, &meeting_id, &attendee_id, &requested).await;
                    assert_eq!(
                        (again.status, again.capabilities()),
                        (200, requested.clone()),
                        "case {case:?}: re-applying the same set expected 200 with {requested:?}, \
                         got {} with {:?}",
                        again.status,
                        again.capabilities()
                    );
                    stored = requested;
                }
            }
        }

        // Both outcomes really occur for every meeting in the table, so no row passes by
        // never exercising one branch of the rule.
        assert!(
            applied > 0 && refused > 0,
            "case {meeting_case:?}: the table must both apply and refuse sets, \
             got {applied} applied and {refused} refused"
        );
    }
}

/// Ids that name no live meeting, or no attendee of the meeting they name, are refused with
/// the modelled 404 and write nothing.
///
/// The rows differ in WHICH of the two path ids is the absent one, and in whether it never
/// named a resource, named one in another meeting, or named one that was deleted.
#[tokio::test]
async fn ids_that_name_nothing_are_refused_and_write_nothing() {
    let router = router();
    let meeting_id = new_meeting(&router, "tok-absent", None).await;
    let attendee_id = new_attendee(&router, &meeting_id, "user-absent-row").await;
    let other_meeting = new_meeting(&router, "tok-absent-other", None).await;
    let other_attendee = new_attendee(&router, &other_meeting, "user-other-meeting").await;

    let requested = capability_set("Send", "Send", "None");
    let before = get_attendee(&router, &meeting_id, &attendee_id)
        .await
        .capabilities();

    // A meeting that EXISTED and has since been deleted, which this service reports as
    // ended rather than as an unknown id.
    let ended_meeting = new_meeting(&router, "tok-absent-ended", None).await;
    let ended_attendee = new_attendee(&router, &ended_meeting, "user-ended").await;
    let deleted = call(&router, "DELETE", &format!("/meetings/{ended_meeting}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteMeeting: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );

    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a MeetingId that never named a meeting",
            ABSENT_GUID,
            attendee_id.as_str(),
            format!("Meeting <{ABSENT_GUID}> not found"),
        ),
        (
            "a MeetingId of a meeting that has ended",
            ended_meeting.as_str(),
            ended_attendee.as_str(),
            "The meeting has ended".to_string(),
        ),
        (
            "an AttendeeId that never named an attendee",
            meeting_id.as_str(),
            ABSENT_GUID,
            format!("Attendee <{ABSENT_GUID}> not found"),
        ),
        (
            "an AttendeeId in its upper-case form that names nothing",
            meeting_id.as_str(),
            OTHER_ABSENT_GUID,
            format!("Attendee <{OTHER_ABSENT_GUID}> not found"),
        ),
        (
            "an AttendeeId of ANOTHER meeting's attendee",
            meeting_id.as_str(),
            other_attendee.as_str(),
            format!("Attendee <{other_attendee}> not found"),
        ),
    ];

    for (case, meeting, attendee, message) in rows {
        let reply = update_capabilities(&router, meeting, attendee, &requested).await;
        assert_not_found(case, &reply, &message);
    }

    // Nothing was written: the attendee that exists still holds the set it was created
    // with, and the other meeting's attendee is untouched too.
    for (case, meeting, attendee) in [
        ("the named meeting's attendee", &meeting_id, &attendee_id),
        ("the other meeting's attendee", &other_meeting, &other_attendee),
    ] {
        let after = get_attendee(&router, meeting, attendee).await;
        assert_eq!(
            (after.status, after.capabilities()),
            (200, before.clone()),
            "case {case:?}: a refused update must write nothing, got {} with {:?}",
            after.status,
            after.capabilities()
        );
    }
}

/// Simultaneous callers each end as a success or as the modelled contention refusal, and
/// the attendee is left holding ONE of the applied sets member-for-member.
///
/// The rows differ in how many callers race; each racer applies a DIFFERENT admissible set,
/// so a final set mixing members from two of them is detectable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_updates_leave_one_applied_set_and_never_a_mix() {
    // Eight distinct admissible sets (none of them breaks a documented corner case for a
    // meeting created with the default features), so each racer asks for its own.
    let sets: Vec<Value> = vec![
        capability_set("SendReceive", "SendReceive", "SendReceive"),
        capability_set("Send", "Send", "None"),
        capability_set("Receive", "None", "None"),
        capability_set("None", "Receive", "None"),
        capability_set("SendReceive", "Receive", "Receive"),
        capability_set("Send", "SendReceive", "Send"),
        capability_set("None", "None", "None"),
        capability_set("Receive", "Send", "None"),
    ];

    let router = router();
    for count in RACER_COUNTS {
        let case = format!("{count} simultaneous UpdateAttendeeCapabilities calls");
        let meeting_id = new_meeting(&router, &format!("tok-race-{count}"), None).await;
        let attendee_id = new_attendee(&router, &meeting_id, &format!("user-race-{count}")).await;

        let racing: Vec<Value> = sets.iter().take(*count).cloned().collect();
        assert_eq!(
            racing.len(),
            *count,
            "case {case:?}: the table must supply one distinct set per racer, got {}",
            racing.len()
        );

        let mut racers = tokio::task::JoinSet::new();
        for (index, requested) in racing.iter().cloned().enumerate() {
            // A CLONE of the router per racer: it shares the one engine and the one service
            // instance, so on this multi-thread runtime the calls really do overlap.
            let router = router.clone();
            let meeting_id = meeting_id.clone();
            let attendee_id = attendee_id.clone();
            racers.spawn(async move {
                let reply =
                    update_capabilities(&router, &meeting_id, &attendee_id, &requested).await;
                (index, reply)
            });
        }

        let mut answers: Vec<(usize, Reply)> = Vec::with_capacity(*count);
        while let Some(joined) = racers.join_next().await {
            answers.push(joined.expect("a racer task must not panic"));
        }
        answers.sort_by_key(|(index, _)| *index);

        let mut winning_sets: Vec<Value> = Vec::new();
        for (index, reply) in &answers {
            let requested = &racing[*index];
            match reply.status {
                200 => {
                    assert_eq!(
                        reply.capabilities(),
                        *requested,
                        "case {case:?}: racer {index} won, so its answer must carry exactly \
                         {requested:?}, got {:?}",
                        reply.capabilities()
                    );
                    if !winning_sets.contains(requested) {
                        winning_sets.push(requested.clone());
                    }
                }
                409 => assert_eq!(
                    reply.error_code.as_deref(),
                    Some("ConflictException"),
                    "case {case:?}: racer {index} lost the race, so its 409 must be \
                     ConflictException, got {:?} with {:?}",
                    reply.error_code,
                    reply.body
                ),
                other => panic!(
                    "case {case:?}: racer {index} must end as 200 or 409 ConflictException, \
                     got {other} {:?} with {:?}",
                    reply.error_code, reply.body
                ),
            }
        }

        assert!(
            !winning_sets.is_empty(),
            "case {case:?}: at least one racer must win, got {answers:?}"
        );

        // The attendee holds ONE of the sets that won, member-for-member: no member of it
        // comes from a different racer's set.
        let final_set = get_attendee(&router, &meeting_id, &attendee_id)
            .await
            .capabilities();
        assert!(
            winning_sets.contains(&final_set),
            "case {case:?}: the final set must be one of the applied sets {winning_sets:?}, \
             got {final_set:?}"
        );
    }
}
