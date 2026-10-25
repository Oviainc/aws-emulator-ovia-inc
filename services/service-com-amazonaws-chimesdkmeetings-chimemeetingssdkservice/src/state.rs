// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkmeetings-chimemeetingssdkservice service.
//!
//! The meeting is this service's primary resource and its attendees live INSIDE it, so
//! `CreateMeeting`, `GetMeeting`, `DeleteMeeting`, `CreateAttendee`, `GetAttendee`,
//! `ListAttendees` and `DeleteAttendee` all read and write the one [`StoredMeeting`] map
//! held here. Nesting the attendees under their meeting is what makes
//! `DeleteMeeting` remove them transitively and makes it impossible for
//! `ListAttendees`/`GetAttendee` to reach another meeting's attendees.
//!
//! The derivations beside the map — the GUID minting, the `MediaPlacement` endpoints, the
//! ARN, the documented capability defaults and the documented attendee ceiling — live
//! here rather than in a handler because several operation files share them, and a second
//! copy of any of them would let two operations disagree about one meeting.
//!
//! # Every store here is bounded
//!
//! This process authenticates nobody, so a caller that reaches it must not be able to
//! grow its memory without limit:
//!
//! | store | ceiling | release |
//! |---|---|---|
//! | [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::meetings`] | [`MAX_MEETINGS`] | `DeleteMeeting`, or `CreateMeeting` is refused |
//! | [`StoredMeeting::attendees`] | [`attendee_ceiling`] per meeting | `DeleteAttendee`/`DeleteMeeting`, or `CreateAttendee` is refused |
//! | [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::meeting_by_client_request_token`] | one entry per live meeting | the entry is dropped with its meeting |
//! | the record of ended meetings | [`MAX_ENDED_MEETINGS`] | the oldest id is forgotten when the next meeting ends |

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::ops::types::{
    Attendee, AttendeeCapabilities, BadRequestException, LimitExceededException, MediaPlacement,
    Meeting, MeetingFeaturesConfiguration, NotFoundException, NotificationsConfiguration,
    ServiceFailureException, Tag,
};

/// The `arnNamespace` this service's ARNs carry (`OPERATIONS.md`, `## Wire protocol`).
pub const ARN_NAMESPACE: &str = "chime";

// The values of the model's `MediaCapabilities` enum
// (`smithy_query(query="shape",shape="MediaCapabilities")` -> `SendReceive | Send |
// Receive | None`). Named once here so the capability rules below read as the
// documentation states them.
pub const CAPABILITY_SEND_RECEIVE: &str = "SendReceive";
pub const CAPABILITY_SEND: &str = "Send";
pub const CAPABILITY_RECEIVE: &str = "Receive";
pub const CAPABILITY_NONE: &str = "None";

/// The `None` member of both `VideoResolution` and `ContentResolution`.
pub const RESOLUTION_NONE: &str = "None";

/// The resolution a meeting's video features carry when the request names none.
///
/// From the model: `VideoFeatures$MaxResolution` documents "Defaults to `HD`."
pub const DEFAULT_VIDEO_MAX_RESOLUTION: &str = "HD";

/// The resolution a meeting's content features carry when the request names none.
///
/// From the model: `ContentFeatures$MaxResolution` documents "Defaults to `FHD`."
pub const DEFAULT_CONTENT_MAX_RESOLUTION: &str = "FHD";

/// The `VideoResolution` value that reduces a meeting's attendee ceiling.
pub const VIDEO_RESOLUTION_FHD: &str = "FHD";

/// The `ContentResolution` value that reduces a meeting's attendee ceiling.
pub const CONTENT_RESOLUTION_UHD: &str = "UHD";

/// How many attendees a meeting admits when it names no `MeetingFeatures.Attendee.MaxCount`
/// and asks for neither `Video.MaxResolution: FHD` nor `Content.MaxResolution: UHD`.
///
/// From the model: `AttendeeFeatures` documents "the maximum number of attendees changes
/// from the default of `250` to `25`" in those two cases, and `AttendeeMax` is
/// `@range(1..250)`.
pub const DEFAULT_MAX_ATTENDEES: usize = 250;

/// How many attendees a meeting admits when it asks for `Video.MaxResolution: FHD` or
/// `Content.MaxResolution: UHD` without naming its own `Attendee.MaxCount`
/// (`AttendeeFeatures`, same sentence as [`DEFAULT_MAX_ATTENDEES`]).
pub const REDUCED_MAX_ATTENDEES: usize = 25;

/// The page size `ListAttendees` serves when the request names no `MaxResults`: the
/// maximum the model allows that member (`ListAttendeesRequest$MaxResults`, range 1..100).
pub const MAX_LIST_PAGE_SIZE: i32 = 100;

/// The most meetings this emulator holds at once.
///
/// `meetings` grows by one record per `CreateMeeting` with a fresh `ClientRequestToken`
/// and shrinks only on `DeleteMeeting`, so without a ceiling its size is set by how many
/// requests a caller makes. AWS bounds the same thing with an adjustable per-account
/// service quota that is not in the model, so this number is the EMULATOR's own bound
/// rather than a claim about AWS's — while the refusal it produces is the error the model
/// declares for exactly this case: `LimitExceededException` (HTTP 400, "The request
/// exceeds the resource limit.").
pub const MAX_MEETINGS: usize = 250;

/// How many ENDED meetings this emulator remembers having ended.
///
/// A meeting that was deleted is reported differently from an id that never named one
/// (see [`ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState::meeting_absent`]), so
/// the ids of deleted meetings have to be remembered — and that record, unlike the
/// meetings themselves, is never released by a client call. It is therefore a bounded
/// FIFO: the oldest id is forgotten when the next one is recorded, after which that id
/// reads as one that never named a meeting again. The bound is the EMULATOR's own, chosen
/// to match [`MAX_MEETINGS`] so a caller that deletes every meeting it created still sees
/// every one of them reported as ended.
pub const MAX_ENDED_MEETINGS: usize = MAX_MEETINGS;

// The failures these operations share. Each handler raises them through its OWN
// operation's error enum (the variants a model declares differ per operation), but the
// payload inside is the same refusal, and a payload built inline in seven places drifts
// in wording. These constructors are the one place each payload's `Code` and message are
// chosen. `RequestId` is deliberately left unset: it identifies one AWS request, and this
// emulator mints no request ids.

/// `BadRequestException`'s payload: the request broke a rule the service enforces but the
/// model cannot express, so the boundary validator passed it.
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: Some("BadRequest".to_string()),
        message: Some(message.into()),
        request_id: None,
    }
}

/// How this service words every `NotFoundException`: the kind of resource the request
/// named, then the id it supplied for that resource in angle brackets, then `not found`.
///
/// The message is built from the REQUEST — the id in it is the one the caller sent, never
/// a value from this store — so a client that logs or surfaces the message can see which
/// of the ids it passed was the absent one. One function so every reader of a missing
/// meeting or attendee (`GetMeeting`, `ListAttendees`, `CreateAttendee`, `GetAttendee`,
/// `DeleteAttendee`) words the refusal identically.
fn not_found(kind: &str, id: &str) -> NotFoundException {
    NotFoundException {
        code: Some("NotFound".to_string()),
        message: Some(format!("{kind} <{id}> not found")),
        request_id: None,
    }
}

/// `NotFoundException`'s payload for a well-formed id that names no stored meeting.
pub fn meeting_not_found(meeting_id: &str) -> NotFoundException {
    not_found("Meeting", meeting_id)
}

/// `NotFoundException`'s payload for a meeting that EXISTED and has since been deleted.
///
/// `DeleteMeeting` is documented as disconnecting every client and preventing new ones
/// from joining — it ENDS the meeting — so a later caller is told that outcome rather
/// than that its id is unknown: the id did name a meeting, and no retry or correction of
/// it will ever succeed. The message names no id because it reports a fact about the
/// meeting's lifecycle, not a value from the request.
pub fn meeting_ended() -> NotFoundException {
    NotFoundException {
        code: Some("NotFound".to_string()),
        message: Some("The meeting has ended".to_string()),
        request_id: None,
    }
}

/// `NotFoundException`'s payload for an attendee that is not in the named meeting.
///
/// The absent resource is the ATTENDEE — the meeting it was looked for in exists, or the
/// caller would have been refused with [`meeting_not_found`] instead — so the id this
/// names is the `AttendeeId`.
pub fn attendee_not_found(attendee_id: &str) -> NotFoundException {
    not_found("Attendee", attendee_id)
}

/// `ServiceFailureException`'s payload for a state store that could not be reached — the
/// one case a handler cannot serve from state at all.
pub fn store_unavailable() -> ServiceFailureException {
    ServiceFailureException {
        code: Some("ServiceFailure".to_string()),
        message: Some("the Chime SDK meeting store is unavailable".to_string()),
        request_id: None,
    }
}

/// `LimitExceededException`'s payload for a store already holding [`MAX_MEETINGS`].
pub fn meeting_limit_reached() -> LimitExceededException {
    LimitExceededException {
        code: Some("LimitExceeded".to_string()),
        message: Some(format!(
            "this account already holds the maximum of {MAX_MEETINGS} Amazon Chime SDK meetings"
        )),
        request_id: None,
    }
}

/// `LimitExceededException`'s payload for a meeting already at its attendee ceiling.
pub fn attendee_limit_reached(meeting_id: &str, ceiling: usize) -> LimitExceededException {
    LimitExceededException {
        code: Some("LimitExceeded".to_string()),
        message: Some(format!(
            "meeting {meeting_id} already holds its maximum of {ceiling} attendees"
        )),
        request_id: None,
    }
}

/// One Amazon Chime SDK meeting as this service persists it: the modelled [`Meeting`] it
/// hands back, plus the request members the `Meeting` shape does not carry.
///
/// The `Meeting` record IS the stored value — not a parallel struct plus a conversion —
/// so a reader clones exactly what the writer stored and `GetMeeting` cannot drift from
/// what `CreateMeeting` returned.
///
/// `attendees` is a `BTreeMap` rather than a `HashMap` because `ListAttendees` is
/// `@paginated` and a page boundary is only meaningful over a stable order: the
/// `NextToken` a page returns names the first attendee it did NOT return, so the next
/// call resumes there.
#[derive(Debug, Clone)]
pub struct StoredMeeting {
    /// The meeting record, exactly as every reader returns it.
    pub meeting: Meeting,
    /// The attendees of this meeting, keyed by `AttendeeId`.
    pub attendees: BTreeMap<String, Attendee>,
    /// `NotificationsConfiguration` as supplied: the targets AWS would publish meeting
    /// and attendee events to. Kept because it belongs to the meeting, and reported by no
    /// operation — the `Meeting` shape has no member for it.
    pub notifications_configuration: Option<NotificationsConfiguration>,
    /// The `MediaPlacementNetworkType` the meeting was created with. Kept for the same
    /// reason: it is part of the meeting, and the `Meeting` shape has no member for it.
    pub media_placement_network_type: Option<String>,
    /// The tags supplied for the meeting, kept as the one place `TagResource` /
    /// `ListTagsForResource` will read them from when they are implemented.
    pub tags: Vec<Tag>,
    /// The `ClientRequestToken` that created this meeting, so deleting the meeting can
    /// drop its idempotency record too.
    pub client_request_token: String,
}

/// Per-service state for com-amazonaws-chimesdkmeetings-chimemeetingssdkservice. A handler reaches it through [`service_state`].
///
/// The meetings map is NOT partitioned by region: a meeting's ARN and media endpoints
/// record the `MediaRegion` it was created in, but any later call reaches every meeting in
/// the store whatever region it is signed for. Partitioning the control plane by region is
/// a separate change; nothing here claims it.
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState {
    /// Every meeting that currently exists, keyed by its minted `MeetingId`.
    pub meetings: HashMap<String, StoredMeeting>,
    /// `ClientRequestToken` -> the `MeetingId` it created, so a replayed `CreateMeeting`
    /// returns the meeting the first call created instead of minting a second one. One
    /// entry per live meeting: `DeleteMeeting` drops the meeting's entry with it, so this
    /// map is bounded by [`MAX_MEETINGS`] too.
    pub meeting_by_client_request_token: HashMap<String, String>,
    /// The `MeetingId`s of meetings that EXISTED and have since been deleted, oldest
    /// first, so a reader can tell a meeting that ended from an id that never named one.
    /// Bounded by [`MAX_ENDED_MEETINGS`]: recording one past the bound drops the oldest.
    ended_meetings: VecDeque<String>,
    /// Counter the minted ids and join tokens are derived from. Never an id itself.
    next_seq: u64,
}

impl ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState {
    /// Mint a `MeetingId` no stored meeting holds.
    ///
    /// The caller must insert the record under the returned id while still holding the
    /// same state lock, or the id it returned names nothing.
    pub fn mint_meeting_id(&mut self) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = guid_from(&format!(
                "chime-sdk-meetings/meeting/{seq}/{}",
                clock_nanos()
            ));
            // Neither a live meeting's id nor one recorded as ended, so the two records
            // stay disjoint and a fresh meeting is never read as an ended one.
            if !self.meetings.contains_key(&candidate) && !self.has_ended(&candidate) {
                return candidate;
            }
        }
    }

    /// Mint an `AttendeeId` no attendee of `meeting_id` holds.
    pub fn mint_attendee_id(&mut self, meeting_id: &str) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = guid_from(&format!(
                "chime-sdk-meetings/attendee/{meeting_id}/{seq}/{}",
                clock_nanos()
            ));
            let taken = self
                .meetings
                .get(meeting_id)
                .is_some_and(|meeting| meeting.attendees.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint the `JoinToken` an attendee authenticates with.
    ///
    /// `Attendee$JoinToken` is `@sensitive` and the shape's own documentation asks that it
    /// be transferred securely, so it must not be derivable from the ids a client already
    /// knows: it is a digest of those ids together with this store's counter and the
    /// wall clock, which a caller holding only the `MeetingId` and `AttendeeId` cannot
    /// reproduce. 64 hex characters, inside `JoinTokenString`'s modelled length 2..2048.
    pub fn mint_join_token(&mut self, meeting_id: &str, attendee_id: &str) -> String {
        let seq = self.next_seq();
        let digest = Sha256::digest(
            format!(
                "chime-sdk-meetings/join-token/{meeting_id}/{attendee_id}/{seq}/{}",
                clock_nanos()
            )
            .as_bytes(),
        );
        hex::encode(digest)
    }

    /// Drop a meeting and everything recorded against it, so nothing outlives the meeting
    /// it belongs to: its attendees (nested in the record) and its idempotency entry.
    ///
    /// Answers the removed record, or `None` when no meeting had that id.
    ///
    /// The id itself is remembered as ENDED, which is what lets every later reader of it
    /// report the meeting's outcome instead of an unknown id.
    pub fn forget_meeting(&mut self, meeting_id: &str) -> Option<StoredMeeting> {
        let removed = self.meetings.remove(meeting_id)?;
        self.meeting_by_client_request_token
            .remove(&removed.client_request_token);
        if !self.has_ended(meeting_id) {
            self.ended_meetings.push_back(meeting_id.to_string());
            while self.ended_meetings.len() > MAX_ENDED_MEETINGS {
                self.ended_meetings.pop_front();
            }
        }
        Some(removed)
    }

    /// Whether this id named a meeting this store has since deleted.
    pub fn has_ended(&self, meeting_id: &str) -> bool {
        self.ended_meetings.iter().any(|ended| ended == meeting_id)
    }

    /// The `NotFoundException` payload for a `MeetingId` this store holds no meeting for.
    ///
    /// Two situations reach it and a caller acts differently on each: a meeting that
    /// EXISTED and has ended (its media is gone, so the caller moves on to a new meeting),
    /// and an id that never named a meeting here (the caller has the wrong id). The
    /// distinction is the store's to make, so it is made once, here, for every operation
    /// that resolves a `MeetingId`.
    ///
    /// The caller must have established that `self.meetings` holds no record under
    /// `meeting_id`.
    pub fn meeting_absent(&self, meeting_id: &str) -> NotFoundException {
        if self.has_ended(meeting_id) {
            meeting_ended()
        } else {
            meeting_not_found(meeting_id)
        }
    }

    /// The next value of this store's monotonic id counter.
    fn next_seq(&mut self) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.next_seq
    }
}

/// The meeting's effective `MeetingFeatures.Video.MaxResolution`: what the request named,
/// or the documented default (`VideoFeatures$MaxResolution`, "Defaults to `HD`").
pub fn video_max_resolution(features: Option<&MeetingFeaturesConfiguration>) -> &str {
    features
        .and_then(|config| config.video.as_ref())
        .and_then(|video| video.max_resolution.as_deref())
        .unwrap_or(DEFAULT_VIDEO_MAX_RESOLUTION)
}

/// The meeting's effective `MeetingFeatures.Content.MaxResolution`: what the request
/// named, or the documented default (`ContentFeatures$MaxResolution`, "Defaults to
/// `FHD`").
pub fn content_max_resolution(features: Option<&MeetingFeaturesConfiguration>) -> &str {
    features
        .and_then(|config| config.content.as_ref())
        .and_then(|content| content.max_resolution.as_deref())
        .unwrap_or(DEFAULT_CONTENT_MAX_RESOLUTION)
}

/// How many attendees a meeting with these features admits.
///
/// From `AttendeeFeatures`: the ceiling is the meeting's own
/// `MeetingFeatures.Attendee.MaxCount` when it names one, else "the maximum number of
/// attendees changes from the default of `250` to `25`" if the meeting asked for
/// `Video.MaxResolution: FHD` or `Content.MaxResolution: UHD`, else 250.
pub fn attendee_ceiling(features: Option<&MeetingFeaturesConfiguration>) -> usize {
    let max_count = features
        .and_then(|config| config.attendee.as_ref())
        .and_then(|attendee| attendee.max_count);
    if let Some(max_count) = max_count {
        // `AttendeeMax` is `@range(1..250)`, so the boundary validator has already
        // refused anything outside it by the time a handler reads this.
        return usize::try_from(max_count).unwrap_or(DEFAULT_MAX_ATTENDEES);
    }
    if video_max_resolution(features) == VIDEO_RESOLUTION_FHD
        || content_max_resolution(features) == CONTENT_RESOLUTION_UHD
    {
        REDUCED_MAX_ATTENDEES
    } else {
        DEFAULT_MAX_ATTENDEES
    }
}

/// Whether a `MediaCapabilities` value asks to send or receive media at all.
fn sends_or_receives(capability: &str) -> bool {
    capability == CAPABILITY_SEND_RECEIVE
        || capability == CAPABILITY_SEND
        || capability == CAPABILITY_RECEIVE
}

/// Whether a `MediaCapabilities` value asks to RECEIVE media.
fn receives(capability: &str) -> bool {
    capability == CAPABILITY_SEND_RECEIVE || capability == CAPABILITY_RECEIVE
}

/// The capabilities an attendee created without any gets, for a meeting with `features`.
///
/// From `AttendeeCapabilities`: "If meeting features is defined as
/// `Video:MaxResolution:None` but `Content:MaxResolution` is defined as something other
/// than `None` and attendee capabilities are not defined in the API request, then the
/// default attendee video capability is set to `Receive` and attendee content capability
/// is set to `SendReceive`. This is because content `SendReceive` requires video to be at
/// least `Receive`." Every other meeting defaults all three to `SendReceive`.
///
/// A meeting whose CONTENT resolution is `None` is the case the documentation does not
/// name. Its default content capability is `None` here, because a default the service
/// picks has to be a value a request could have supplied — and
/// [`capability_problem`] refuses `Send`/`Receive`/`SendReceive` content for exactly
/// that meeting.
pub fn default_capabilities(features: Option<&MeetingFeaturesConfiguration>) -> AttendeeCapabilities {
    let video_none = video_max_resolution(features) == RESOLUTION_NONE;
    let content_none = content_max_resolution(features) == RESOLUTION_NONE;
    let content = if content_none {
        CAPABILITY_NONE
    } else {
        CAPABILITY_SEND_RECEIVE
    };
    let video = match (video_none, content_none) {
        // Documented: content `SendReceive` requires video to be at least `Receive`.
        (true, false) => CAPABILITY_RECEIVE,
        (true, true) => CAPABILITY_NONE,
        (false, _) => CAPABILITY_SEND_RECEIVE,
    };
    AttendeeCapabilities {
        audio: CAPABILITY_SEND_RECEIVE.to_string(),
        video: video.to_string(),
        content: content.to_string(),
    }
}

/// Why REQUEST-SUPPLIED `capabilities` cannot hold for a meeting with `features`, or
/// `None` when they can.
///
/// The three rules are the "corner cases" the model's `AttendeeCapabilities`
/// documentation lists, and each one is refused with HTTP 400 — the status both of its
/// sentences name:
///
/// 1. `Video:MaxResolution:None` — "all API requests that include `SendReceive`, `Send`,
///    or `Receive` for `AttendeeCapabilities:Video` will be rejected".
/// 2. `Content:MaxResolution:None` — the same for `AttendeeCapabilities:Content`.
/// 3. "You can't set `content` capabilities to `SendReceive` or `Receive` unless you also
///    set `video` capabilities to `SendReceive` or `Receive`."
pub fn capability_problem(
    capabilities: &AttendeeCapabilities,
    features: Option<&MeetingFeaturesConfiguration>,
) -> Option<String> {
    if video_max_resolution(features) == RESOLUTION_NONE
        && sends_or_receives(&capabilities.video)
    {
        return Some(format!(
            "Capabilities.Video is {}, but the meeting was created with \
             MeetingFeatures.Video.MaxResolution None, which admits only {CAPABILITY_NONE}",
            capabilities.video
        ));
    }
    if content_max_resolution(features) == RESOLUTION_NONE
        && sends_or_receives(&capabilities.content)
    {
        return Some(format!(
            "Capabilities.Content is {}, but the meeting was created with \
             MeetingFeatures.Content.MaxResolution None, which admits only {CAPABILITY_NONE}",
            capabilities.content
        ));
    }
    if receives(&capabilities.content) && !receives(&capabilities.video) {
        return Some(format!(
            "Capabilities.Content {} requires Capabilities.Video to be \
             {CAPABILITY_SEND_RECEIVE} or {CAPABILITY_RECEIVE}, not {}",
            capabilities.content, capabilities.video
        ));
    }
    None
}

/// The `MeetingArn` of a meeting: `arn:aws:chime:<MediaRegion>:<account>:meeting/<id>`,
/// which satisfies `AmazonResourceName`'s modelled `^arn:` pattern and length 1..1011.
pub fn meeting_arn(meeting_id: &str, media_region: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        media_region,
        &format!("meeting/{meeting_id}"),
    )
}

/// The media endpoints reported for a meeting.
///
/// Every URL is a pure function of the `MeetingId` and the meeting's `MediaRegion`, so
/// every read of one meeting reports the same `MediaPlacement` — which is what makes
/// `GetMeeting` equal to what `CreateMeeting` returned. The shapes of the host names
/// mirror the ones the real service hands out, because a client parses these strings;
/// this emulator runs no Chime SDK media service, so nothing answers behind them.
pub fn media_placement(meeting_id: &str, media_region: &str) -> MediaPlacement {
    MediaPlacement {
        audio_host_url: Some(format!(
            "{}.k.m3.{media_region}.app.chime.aws:3478",
            audio_host_token(meeting_id)
        )),
        audio_fallback_url: Some(format!(
            "wss://haxrp.m3.{media_region}.app.chime.aws:443/calls/{meeting_id}"
        )),
        signaling_url: Some(format!(
            "wss://signal.m3.{media_region}.app.chime.aws/control/{meeting_id}"
        )),
        turn_control_url: Some(format!(
            "https://ccp.cp.{media_region}.app.chime.aws/v2/turn_sessions"
        )),
        screen_data_url: Some(format!(
            "wss://bitpw.m3.{media_region}.app.chime.aws:443/v2/screen/{meeting_id}"
        )),
        screen_sharing_url: Some(format!(
            "wss://bitpw.m3.{media_region}.app.chime.aws:443/v2/screen/{meeting_id}"
        )),
        screen_viewing_url: Some(format!(
            "wss://bitpw.m3.{media_region}.app.chime.aws:443/ws/connect?X-BitHub-Call-Id={meeting_id}"
        )),
        event_ingestion_url: Some(format!(
            "https://data.svc.{media_region}.ingest.chime.aws/v1/client-events"
        )),
    }
}

/// The 8-hex-character per-meeting label the audio host name carries.
fn audio_host_token(meeting_id: &str) -> String {
    let digest = Sha256::digest(format!("chime-sdk-meetings/audio-host/{meeting_id}").as_bytes());
    hex::encode(&digest[..4])
}

/// One id in the `GuidString` form this service's ids take:
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, with RFC 4122's version-4
/// and variant bits stamped in, so the value is also a well-formed random UUID — which is
/// what an SDK that parses a `MeetingId` or `AttendeeId` expects.
fn guid_from(label: &str) -> String {
    let digest = Sha256::digest(label.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Nanoseconds since the Unix epoch: the entropy mixed into a minted id, so successive
/// ids do not expose this store's counter.
fn clock_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// The page size `ListAttendees` serves for a request whose `MaxResults` is `max_results`.
///
/// An absent `MaxResults` asks for as many attendees as the modelled range allows, not an
/// empty page. A caller-supplied bound is already inside `@range(1..100)` by the time a
/// handler runs; clamping again costs nothing and keeps this total.
pub fn page_size(max_results: Option<i32>) -> usize {
    let bound = max_results
        .unwrap_or(MAX_LIST_PAGE_SIZE)
        .clamp(1, MAX_LIST_PAGE_SIZE);
    usize::try_from(bound).unwrap_or(1)
}

/// Prefix carried inside a pagination token, so a token from somewhere else is refused
/// rather than read as an `AttendeeId`.
const PAGE_TOKEN_PREFIX: &str = "attendee:";

/// Encode the opaque `NextToken` that resumes a listing AT `next_id`.
///
/// Opaque to the caller (hex of a tagged byte string) because the model gives `NextToken`
/// no structure: a client must pass back what it was handed, not construct one. 2 hex
/// characters per byte keeps it inside the modelled length 0..4096 for any `AttendeeId`.
pub fn encode_page_token(next_id: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{next_id}"))
}

/// The `AttendeeId` a page token resumes at, or `None` when the token was not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// One page of an ordered `(key, record)` sequence, plus the `NextToken` that continues
/// it — `None` when the page reached the end.
///
/// The token names the KEY OF THE FIRST RECORD NOT RETURNED, and a resumed call starts at
/// the first key at or after it. Resuming is therefore well defined even when the record
/// the token names was deleted between the two calls: the scan simply continues at the
/// next key. Together with the stable key order of a `BTreeMap`, that is what makes a
/// paging client see each attendee exactly once.
pub fn page<'a, T: Clone + 'a>(
    ordered: impl IntoIterator<Item = (&'a String, &'a T)>,
    resume_at: Option<&str>,
    page_size: usize,
) -> (Vec<T>, Option<String>) {
    let mut records = Vec::new();
    for (key, record) in ordered {
        if resume_at.is_some_and(|from| key.as_str() < from) {
            continue;
        }
        if records.len() == page_size {
            return (records, Some(encode_page_token(key)));
        }
        records.push(record.clone());
    }
    (records, None)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkmeetingsChimemeetingssdkserviceState>)
}
