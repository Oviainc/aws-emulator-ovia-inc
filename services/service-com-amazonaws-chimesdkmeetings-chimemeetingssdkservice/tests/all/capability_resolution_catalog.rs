// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHEN a capability set is refused for a meeting's maximum
//! resolutions, and with WHICH sentence, over the whole catalog of modelled resolutions.
//!
//! The rule under test: a meeting admits only `None` for a capability axis whose
//! `MeetingFeatures` maximum resolution for that axis is `None`, admits every value of that
//! axis otherwise, and refuses the inadmissible set with the one fixed sentence the service
//! serves for that error class — the sentence naming the axis the meeting feature forbade
//! and nothing of the request, whichever capability value broke the rule and whichever
//! operation supplied the set.
//!
//! The table spans the class the rule generalises over: EVERY entry of the modelled
//! `VideoResolution` and `ContentResolution` enums crossed with each other (so the `None`
//! entry of each axis is exercised beside every other published resolution, including the
//! `HD`/`FHD`/`UHD` entries no recorded case names), crossed with the whole product of the
//! modelled `MediaCapabilities` enum over the video and content members, through the two
//! operations that take a request-supplied set for a single attendee.

use serde_json::{json, Value};

use crate::capability_refusal_message::{
    call, capability_set, create_attendee_with, expected_refusal, get_attendee, new_attendee,
    new_meeting, router, update_capabilities, Reply, CONTENT_MAX_RESOLUTION_NONE_MESSAGE,
    MEDIA_CAPABILITIES, VIDEO_MAX_RESOLUTION_NONE_MESSAGE,
};

/// The values of the model's `VideoResolution` enum
/// (`smithy_query(query="shape",shape="VideoResolution",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`
/// -> `None | HD | FHD`).
const VIDEO_RESOLUTIONS: &[&str] = &["None", "HD", "FHD"];

/// The values of the model's `ContentResolution` enum, from the same query
/// (-> `None | FHD | UHD`).
const CONTENT_RESOLUTIONS: &[&str] = &["None", "FHD", "UHD"];

/// The `MeetingFeatures.Attendee.MaxCount` a meeting must also name to ask for the top
/// resolution of an axis: `VideoFeatures$MaxResolution` ("To use `FHD`, you must also
/// provide a `MeetingFeatures:Attendee:MaxCount` value and override the default size limit
/// of 250 attendees") and `ContentFeatures$MaxResolution` (the same for `UHD`).
const OVERRIDDEN_ATTENDEE_MAX_COUNT: i64 = 250;

/// The `Capabilities` member of the `Attendee` a reply carries.
fn capabilities_of(reply: &Reply) -> Value {
    reply
        .body
        .get("Attendee")
        .and_then(|attendee| attendee.get("Capabilities"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `MeetingFeatures` naming both maximum resolutions explicitly, with the attendee
/// ceiling the documentation requires beside the top resolution of either axis.
fn features(video_resolution: &str, content_resolution: &str) -> Value {
    let mut config = json!({
        "Video": { "MaxResolution": video_resolution },
        "Content": { "MaxResolution": content_resolution },
    });
    if video_resolution == "FHD" || content_resolution == "UHD" {
        config["Attendee"] = json!({ "MaxCount": OVERRIDDEN_ATTENDEE_MAX_COUNT });
    }
    config
}

/// For every pair of published maximum resolutions, a set asking for a medium the meeting
/// caps at `None` is refused with that axis's fixed sentence, every other set is applied,
/// and a refusal leaves the stored set as it was.
#[tokio::test]
async fn a_max_resolution_of_none_is_the_only_resolution_that_refuses_a_capability() {
    let router = router();

    for (index, video_resolution) in VIDEO_RESOLUTIONS.iter().enumerate() {
        for (offset, content_resolution) in CONTENT_RESOLUTIONS.iter().enumerate() {
            let row = format!("Video:MaxResolution:{video_resolution}, Content:MaxResolution:{content_resolution}");
            let token = format!("tok-res-{index}-{offset}");
            let meeting_id = new_meeting(
                &router,
                &token,
                Some(features(video_resolution, content_resolution)),
            )
            .await;
            let attendee_id = new_attendee(&router, &meeting_id, &format!("user-{token}")).await;
            // What the attendee holds now, so a refusal can be shown to change nothing.
            let mut stored = capabilities_of(&get_attendee(&router, &meeting_id, &attendee_id).await);

            let mut video_class_refusals = 0usize;
            let mut content_class_refusals = 0usize;
            let mut admitted = 0usize;

            for video in MEDIA_CAPABILITIES {
                for content in MEDIA_CAPABILITIES {
                    // No documented rule constrains `Audio`, which the sibling suite's
                    // product covers; this table varies the two constrained axes.
                    let requested = capability_set("SendReceive", video, content);
                    let case = format!("{row}, Video {video} Content {content}");
                    let expected =
                        expected_refusal(video, content, video_resolution, content_resolution);

                    let reply =
                        update_capabilities(&router, &meeting_id, &attendee_id, &requested).await;
                    let created = create_attendee_with(
                        &router,
                        &meeting_id,
                        &format!("user-{token}-{video}-{content}"),
                        &requested,
                    )
                    .await;

                    let Some(message) = expected else {
                        admitted += 1;
                        for (operation, reply) in
                            [("UpdateAttendeeCapabilities", &reply), ("CreateAttendee", &created)]
                        {
                            assert_eq!(
                                (reply.status, reply.error_code.as_deref()),
                                (200, None),
                                "case {case:?} via {operation}: a published resolution other \
                                 than None must admit this set, got {} {:?} with {:?}",
                                reply.status,
                                reply.error_code,
                                reply.body
                            );
                            assert_eq!(
                                capabilities_of(reply),
                                requested,
                                "case {case:?} via {operation}: the answer must carry exactly \
                                 the applied set, got {:?}",
                                capabilities_of(reply)
                            );
                        }
                        // Persisted, not just answered.
                        let read_back = get_attendee(&router, &meeting_id, &attendee_id).await;
                        assert_eq!(
                            capabilities_of(&read_back),
                            requested,
                            "case {case:?}: GetAttendee must read back the applied set, got {:?}",
                            capabilities_of(&read_back)
                        );
                        stored = requested;
                        continue;
                    };

                    if message == VIDEO_MAX_RESOLUTION_NONE_MESSAGE {
                        video_class_refusals += 1;
                    } else if message == CONTENT_MAX_RESOLUTION_NONE_MESSAGE {
                        content_class_refusals += 1;
                    }

                    for (operation, reply) in
                        [("UpdateAttendeeCapabilities", &reply), ("CreateAttendee", &created)]
                    {
                        assert_eq!(
                            (reply.status, reply.error_code.as_deref()),
                            (400, Some("BadRequestException")),
                            "case {case:?} via {operation}: expected the modelled 400 \
                             BadRequestException, got {} {:?} with {:?}",
                            reply.status,
                            reply.error_code,
                            reply.body
                        );
                        assert_eq!(
                            reply.message().as_deref(),
                            Some(message),
                            "case {case:?} via {operation}: expected message {message:?}, \
                             got {:?}",
                            reply.message()
                        );
                        // Nothing of the request is in the sentence, not the capability
                        // value that broke the rule.
                        let served = reply.message().unwrap_or_default();
                        for forbidden in [meeting_id.as_str(), attendee_id.as_str(), "user-"] {
                            assert!(
                                !served.contains(forbidden),
                                "case {case:?} via {operation}: the message must not carry the \
                                 request value {forbidden:?}, got {served:?}"
                            );
                        }
                    }

                    let after = get_attendee(&router, &meeting_id, &attendee_id).await;
                    assert_eq!(
                        capabilities_of(&after),
                        stored,
                        "case {case:?}: a refused update must leave the stored set at \
                         {stored:?}, got {:?}",
                        capabilities_of(&after)
                    );
                }
            }

            // The refusal of an axis fires for EXACTLY the catalog's `None` entry of that
            // axis, and every row still admits sets.
            assert_eq!(
                video_class_refusals > 0,
                *video_resolution == "None",
                "case {row:?}: the video refusal must fire only for Video:MaxResolution:None, \
                 got {video_class_refusals} of them"
            );
            assert_eq!(
                content_class_refusals > 0,
                *content_resolution == "None",
                "case {row:?}: the content refusal must fire only for \
                 Content:MaxResolution:None, got {content_class_refusals} of them"
            );
            assert!(
                admitted > 0,
                "case {row:?}: the row must admit at least one set, got {admitted}"
            );
        }
    }
}

/// A meeting that names NO `MeetingFeatures` behaves as the documented defaults
/// (`HD` video, `FHD` content), so neither maximum-resolution refusal can fire on it.
///
/// The rows are the capability values that a `None` maximum resolution would refuse.
#[tokio::test]
async fn the_default_resolutions_refuse_no_capability_value() {
    let router = router();
    let meeting_id = new_meeting(&router, "tok-res-default", None).await;
    let attendee_id = new_attendee(&router, &meeting_id, "user-res-default").await;

    for video in MEDIA_CAPABILITIES {
        for content in MEDIA_CAPABILITIES {
            let requested = capability_set("Receive", video, content);
            let case = format!("default resolutions, Video {video} Content {content}");
            let reply = update_capabilities(&router, &meeting_id, &attendee_id, &requested).await;
            let served = reply.message().unwrap_or_default();
            assert!(
                served != VIDEO_MAX_RESOLUTION_NONE_MESSAGE
                    && served != CONTENT_MAX_RESOLUTION_NONE_MESSAGE,
                "case {case:?}: a meeting with the default resolutions must not refuse a \
                 capability for a maximum resolution of None, got {served:?}"
            );
            // Only the content-needs-video rule can refuse here; everything else applies.
            let expected = expected_refusal(video, content, "HD", "FHD");
            assert_eq!(
                reply.status,
                if expected.is_some() { 400 } else { 200 },
                "case {case:?}: expected status {}, got {} with {:?}",
                if expected.is_some() { 400 } else { 200 },
                reply.status,
                reply.body
            );
        }
    }

    // The meeting really was created without features, so the row above is the default
    // path and not a configuration this test supplied.
    let read_back = call(&router, "GET", &format!("/meetings/{meeting_id}"), None).await;
    assert_eq!(
        read_back
            .body
            .get("Meeting")
            .and_then(|meeting| meeting.get("MeetingFeatures")),
        None,
        "GetMeeting must report no MeetingFeatures for a meeting created without any, got {:?}",
        read_back.body
    );
}
