// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `BatchUpdateAttendeeCapabilitiesExcept`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `chime` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: the call replaces the whole capability set of EVERY attendee of the
//! named meeting whose `AttendeeId` the request's exclusion list does not carry, leaves the
//! excluded attendees holding the set they already had, and reports success with the
//! modelled empty 200. The exclusion list only SELECTS attendees, so an id in it that
//! belongs to no attendee of the meeting excludes nothing and refuses nothing. The set is
//! judged against the stored meeting's `MeetingFeatures` exactly as a single-attendee
//! update judges it, so a set the meeting does not admit is refused with the modelled 400
//! and leaves every attendee's set untouched; a `MeetingId` that names no live meeting is
//! refused with the modelled 404 and writes nothing.
//!
//! The tables span the class the rule generalises over: the NUMBER of attendees in the
//! meeting, WHICH of them (and how many) the exclusion list names, exclusion lists holding
//! ids of no attendee and repeated ids, the whole cartesian product of the modelled
//! `MediaCapabilities` enum over the three capability members, and the meeting feature
//! configurations that move the admissibility boundary.

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

    /// A named member of the body, as a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// Send one request through the router and reduce the answer to a [`Reply`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Reply {
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
/// way the production router reaches it.
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

/// Create `count` attendees of `meeting_id` and answer their minted `AttendeeId`s.
async fn new_attendees(
    router: &MultiServiceRouter,
    meeting_id: &str,
    prefix: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let user = format!("{prefix}-{index}");
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
        ids.push(
            reply
                .body
                .get("Attendee")
                .and_then(|attendee| attendee.get("AttendeeId"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("CreateAttendee {user:?} returned no AttendeeId"))
                .to_string(),
        );
    }
    ids
}

/// `BatchUpdateAttendeeCapabilitiesExcept` over the wire: `PUT` on the modelled URI
/// (`/meetings/{MeetingId}/attendees/capabilities?operation=batch-update-except`).
async fn batch_update_except(
    router: &MultiServiceRouter,
    meeting_id: &str,
    excluded: &[&str],
    capabilities: &Value,
) -> Reply {
    let excluded_ids: Vec<Value> = excluded
        .iter()
        .map(|id| json!({ "AttendeeId": id }))
        .collect();
    call(
        router,
        "PUT",
        &format!("/meetings/{meeting_id}/attendees/capabilities?operation=batch-update-except"),
        Some(&json!({
            "ExcludedAttendeeIds": excluded_ids,
            "Capabilities": capabilities,
        })),
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

/// Assert the modelled success: HTTP 200 with the empty body the `Unit` output shape means.
fn assert_empty_success(case: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (200, None),
        "case {case:?}: expected the modelled 200, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.body,
        Value::Null,
        "case {case:?}: the operation models no output members, so the body must be empty, \
         got {:?}",
        reply.body
    );
}

/// The capability set every attendee of `meeting_id` currently holds, in the order `ids`
/// names them, read back through `GetAttendee`.
async fn stored_sets(
    router: &MultiServiceRouter,
    meeting_id: &str,
    ids: &[String],
) -> Vec<Value> {
    let mut sets = Vec::with_capacity(ids.len());
    for id in ids {
        let reply = get_attendee(router, meeting_id, id).await;
        assert_eq!(
            reply.status, 200,
            "GetAttendee {id:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        sets.push(reply.capabilities());
    }
    sets
}

/// Every attendee the exclusion list does not name gets the requested set; every attendee
/// it names keeps the set it already held.
///
/// The rows span the dimensions the rule generalises over: how many attendees the meeting
/// holds, how many of them are excluded and WHICH (none of them, the first, the last, a
/// middle pair, all of them), and exclusion lists that also carry ids belonging to no
/// attendee of the meeting or repeat an id.
#[tokio::test]
async fn the_requested_set_reaches_every_attendee_outside_the_exclusion_list() {
    let router = router();

    // (case, how many attendees, which of them the exclusion list names by index, extra
    // ids in the list that belong to no attendee of this meeting). `ExcludedAttendeeIds`
    // is modelled `@length(min: 1, max: 250)`, so every list carries at least one entry.
    let rows: Vec<(&str, usize, &[usize], &[&str])> = vec![
        ("one attendee, excluded", 1, &[0], &[]),
        ("one attendee, not excluded", 1, &[], &[ABSENT_GUID]),
        ("two attendees, the first excluded", 2, &[0], &[]),
        ("two attendees, the second excluded", 2, &[1], &[]),
        ("two attendees, both excluded", 2, &[0, 1], &[]),
        ("five attendees, a middle pair excluded", 5, &[1, 3], &[]),
        ("five attendees, the last excluded", 5, &[4], &[]),
        ("five attendees, none excluded", 5, &[], &[ABSENT_GUID]),
        (
            "five attendees, one excluded beside two ids of no attendee",
            5,
            &[2],
            &[ABSENT_GUID, OTHER_ABSENT_GUID],
        ),
        ("three attendees, one excluded twice", 3, &[1, 1], &[]),
    ];

    // Two admissible sets, so each row is driven from a known starting set to a DIFFERENT
    // one and "unchanged" is distinguishable from "updated".
    let first = capability_set("Send", "Send", "None");
    let second = capability_set("SendReceive", "SendReceive", "SendReceive");

    for (index, (case, attendee_count, excluded_indexes, extra_ids)) in rows.into_iter().enumerate()
    {
        let meeting_id = new_meeting(&router, &format!("tok-batch-{index}"), None).await;
        let attendees =
            new_attendees(&router, &meeting_id, &format!("user-batch-{index}"), attendee_count)
                .await;

        // Put every attendee on one known set first, so the second batch's effect on the
        // excluded attendees is visible.
        let all_included: Vec<&str> = vec![ABSENT_GUID];
        let seeded = batch_update_except(&router, &meeting_id, &all_included, &first).await;
        assert_empty_success(&format!("{case}: seeding every attendee"), &seeded);
        let seeded_sets = stored_sets(&router, &meeting_id, &attendees).await;
        for (position, set) in seeded_sets.iter().enumerate() {
            assert_eq!(
                set, &first,
                "case {case:?}: seeding must leave attendee {position} holding {first:?}, \
                 got {set:?}"
            );
        }

        let mut excluded: Vec<&str> = excluded_indexes
            .iter()
            .map(|position| attendees[*position].as_str())
            .collect();
        excluded.extend_from_slice(extra_ids);
        assert!(
            !excluded.is_empty(),
            "case {case:?}: ExcludedAttendeeIds is modelled @length(min: 1), so every row \
             must send at least one id, got {excluded:?}"
        );

        let reply = batch_update_except(&router, &meeting_id, &excluded, &second).await;
        assert_empty_success(case, &reply);

        let after = stored_sets(&router, &meeting_id, &attendees).await;
        for (position, set) in after.iter().enumerate() {
            let expected = if excluded_indexes.contains(&position) {
                &first
            } else {
                &second
            };
            assert_eq!(
                set,
                expected,
                "case {case:?}: attendee {position} is {}, so it must hold {expected:?}, \
                 got {set:?}",
                if excluded_indexes.contains(&position) {
                    "excluded"
                } else {
                    "updated"
                }
            );
        }

        // Applying the same batch again changes nothing: the complement already holds it.
        let again = batch_update_except(&router, &meeting_id, &excluded, &second).await;
        assert_empty_success(&format!("{case}: re-applied"), &again);
        let repeated = stored_sets(&router, &meeting_id, &attendees).await;
        assert_eq!(
            repeated, after,
            "case {case:?}: re-applying the same batch must leave {after:?}, got {repeated:?}"
        );
    }
}

/// An attendee created AFTER a batch holds the meeting's documented defaults, not the set
/// the batch applied: the call updates the attendees that existed when it ran, and does not
/// become the meeting's new default.
#[tokio::test]
async fn a_batch_updates_only_the_attendees_that_existed_when_it_ran() {
    let router = router();
    let meeting_id = new_meeting(&router, "tok-batch-later", None).await;
    let existing = new_attendees(&router, &meeting_id, "user-batch-before", 2).await;
    let created_default = stored_sets(&router, &meeting_id, &existing).await[0].clone();

    let applied = capability_set("None", "Receive", "None");
    assert_ne!(
        applied, created_default,
        "the batch's set must differ from the set a creation mints, got {applied:?}"
    );
    let reply = batch_update_except(&router, &meeting_id, &[ABSENT_GUID], &applied).await;
    assert_empty_success("a batch excluding no attendee of the meeting", &reply);

    let later = new_attendees(&router, &meeting_id, "user-batch-after", 1).await;
    let later_set = stored_sets(&router, &meeting_id, &later).await[0].clone();
    assert_eq!(
        later_set, created_default,
        "an attendee created after the batch must hold the created default \
         {created_default:?}, got {later_set:?}"
    );

    let before = stored_sets(&router, &meeting_id, &existing).await;
    for (position, set) in before.iter().enumerate() {
        assert_eq!(
            set, &applied,
            "attendee {position} existed when the batch ran, so it must still hold \
             {applied:?}, got {set:?}"
        );
    }
}

/// A set the meeting's `MeetingFeatures` do not admit is refused with the modelled 400 and
/// leaves EVERY attendee's set untouched; every set they admit is applied to the whole
/// complement of the exclusion list.
///
/// The table is the full cartesian product of the modelled `MediaCapabilities` enum over
/// `Audio`, `Video` and `Content` (64 sets), run against three meeting feature
/// configurations that move the admissibility boundary: the documented defaults, a meeting
/// whose video maximum resolution is `None`, and one whose content maximum resolution is
/// `None`.
#[tokio::test]
async fn a_set_the_meeting_does_not_admit_is_refused_and_writes_nothing() {
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
        let meeting_id = new_meeting(&router, &format!("tok-batch-caps-{index}"), features).await;
        let attendees = new_attendees(
            &router,
            &meeting_id,
            &format!("user-batch-caps-{index}"),
            3,
        )
        .await;
        // The LAST attendee is excluded from every batch in this row, so it witnesses that
        // an applied set never reaches an excluded attendee either.
        let excluded = vec![attendees[2].as_str()];
        let mut stored = stored_sets(&router, &meeting_id, &attendees).await;
        let mut applied = 0usize;
        let mut refused = 0usize;

        for audio in MEDIA_CAPABILITIES {
            for video in MEDIA_CAPABILITIES {
                for content in MEDIA_CAPABILITIES {
                    let case =
                        format!("{meeting_case}, Audio {audio} Video {video} Content {content}");
                    let requested = capability_set(audio, video, content);
                    let reply =
                        batch_update_except(&router, &meeting_id, &excluded, &requested).await;

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
                        let after = stored_sets(&router, &meeting_id, &attendees).await;
                        assert_eq!(
                            after, stored,
                            "case {case:?}: a refused batch must leave every stored set at \
                             {stored:?}, got {after:?}"
                        );
                        continue;
                    }

                    applied += 1;
                    assert_empty_success(&case, &reply);
                    let after = stored_sets(&router, &meeting_id, &attendees).await;
                    for (position, set) in after.iter().enumerate().take(2) {
                        assert_eq!(
                            set, &requested,
                            "case {case:?}: attendee {position} is outside the exclusion \
                             list, so it must hold {requested:?}, got {set:?}"
                        );
                    }
                    assert_eq!(
                        after[2], stored[2],
                        "case {case:?}: the excluded attendee must keep {:?}, got {:?}",
                        stored[2], after[2]
                    );
                    stored = after;
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

/// A `MeetingId` that names no live meeting is refused with the modelled 404, worded as
/// this service words one, and writes nothing.
///
/// The rows differ in whether the id never named a meeting, named one that has since been
/// deleted, or is the upper-case form of an id that names nothing.
#[tokio::test]
async fn a_meeting_id_that_names_nothing_is_refused_and_writes_nothing() {
    let router = router();
    let meeting_id = new_meeting(&router, "tok-batch-absent", None).await;
    let attendees = new_attendees(&router, &meeting_id, "user-batch-absent", 2).await;
    let before = stored_sets(&router, &meeting_id, &attendees).await;

    let ended_meeting = new_meeting(&router, "tok-batch-ended", None).await;
    let ended_attendees = new_attendees(&router, &ended_meeting, "user-batch-ended", 1).await;
    let deleted = call(&router, "DELETE", &format!("/meetings/{ended_meeting}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteMeeting: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );

    let requested = capability_set("Send", "Send", "None");
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a MeetingId that never named a meeting",
            ABSENT_GUID,
            attendees[0].as_str(),
            format!("Meeting <{ABSENT_GUID}> not found"),
        ),
        (
            "a MeetingId in its upper-case form that names nothing",
            OTHER_ABSENT_GUID,
            attendees[0].as_str(),
            format!("Meeting <{OTHER_ABSENT_GUID}> not found"),
        ),
        (
            "a MeetingId of a meeting that has ended",
            ended_meeting.as_str(),
            ended_attendees[0].as_str(),
            "The meeting has ended".to_string(),
        ),
    ];

    for (case, meeting, excluded, message) in rows {
        let reply = batch_update_except(&router, meeting, &[excluded], &requested).await;
        assert_eq!(
            (reply.status, reply.error_code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected the modelled 404 NotFoundException, got {} {:?} \
             with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.text("Message").as_deref(),
            Some(message.as_str()),
            "case {case:?}: expected Message {message:?}, got {:?}",
            reply.text("Message")
        );
    }

    // Nothing was written: the live meeting's attendees still hold the sets they had.
    let after = stored_sets(&router, &meeting_id, &attendees).await;
    assert_eq!(
        after, before,
        "a refused batch must write nothing, expected {before:?}, got {after:?}"
    );
}
