Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 79e019950bd0394d5d050b3458cf0765058555c7

# Bootstrap the Amazon Chime SDK Meetings emulator

_Filed: 2026-10-22T00:15:37Z_
_Operation: service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice | Class: missing_

> _Note: this issue was identified against the code base at commit `79e019950bd0394d5d050b3458cf0765058555c7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 16 handlers under `services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Chime SDK Meetings client can do anything. Implement a FIRST, SMALL, stateful slice: the meeting + attendee lifecycle (restJson1, sigv4 signing name `chime`, endpoint prefix `meetings-chime`).

### Happy path to implement (in order)

1. **CreateMeeting** — `POST /meetings`. Input: `ClientRequestToken` (required, @idempotencyToken, 2..64, `^[-_a-zA-Z0-9]*$`), `MediaRegion` (required), `ExternalMeetingId` (required), optional `MeetingHostId`, `MeetingFeatures`, `PrimaryMeetingId`, `TenantIds`, `Tags`, `MediaPlacementNetworkType`. Must mint a fresh GUID `MeetingId` matching `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, build a `MediaPlacement` (AudioHostUrl, AudioFallbackUrl, SignalingUrl, TurnControlUrl, ScreenDataUrl, ScreenSharingUrl, ScreenViewingUrl, EventIngestionUrl) and a `MeetingArn` (`arn:aws:chime:<MediaRegion>:<account>:meeting/<MeetingId>`), persist the whole record, and return it as `Meeting`. Same `ClientRequestToken` replayed ⇒ return the SAME stored meeting (idempotency), not a new one.
2. **CreateAttendee** — `POST /meetings/{MeetingId}/attendees`. Input: `MeetingId` (@httpLabel), `ExternalUserId` (required, 2..64), optional `Capabilities` (AttendeeCapabilities{Audio,Video,Content}). Must look the meeting up in state (unknown ⇒ `NotFoundException` 404), mint a GUID `AttendeeId` + an opaque `JoinToken`, default `Capabilities` to `SendReceive` for all three when absent, store the attendee under that meeting, and return it as `Attendee`.
3. **GetAttendee** — `GET /meetings/{MeetingId}/attendees/{AttendeeId}`. Must return the EXACT attendee stored in step 2 (same `AttendeeId`, `ExternalUserId`, `JoinToken`, `Capabilities`); unknown meeting or attendee ⇒ `NotFoundException` 404.
4. **ListAttendees** — `GET /meetings/{MeetingId}/attendees`. Must return every attendee created for that meeting, honouring `max-results` (1..100) and round-tripping `next-token` so a paged client sees each attendee exactly once; unknown meeting ⇒ 404.
5. **GetMeeting** — `GET /meetings/{MeetingId}`. Must return the stored meeting byte-for-byte equal to what CreateMeeting returned (same `MeetingId`, `ExternalMeetingId`, `MediaRegion`, `MediaPlacement` URLs, `MeetingArn`, `MeetingFeatures`, `TenantIds`); unknown ⇒ `NotFoundException` 404.
6. **DeleteAttendee** — `DELETE /meetings/{MeetingId}/attendees/{AttendeeId}` ⇒ 204 empty body, attendee removed so GetAttendee afterwards is 404 and ListAttendees no longer shows it.
7. **DeleteMeeting** — `DELETE /meetings/{MeetingId}` ⇒ 204 empty body, meeting AND all its attendees removed so GetMeeting / ListAttendees / CreateAttendee afterwards all return `NotFoundException` 404.

Every response must be computed from persisted state. No canned bodies, no echoing the request back, no minting an ID that is never stored. IDs must satisfy the modeled GUID pattern so SDK output validation accepts them.

## State Model

A single service store behind the usual lock, e.g.:

- `meetings: HashMap<String /*MeetingId (GUID)*/, Meeting>` where
  `Meeting { meeting_id, external_meeting_id, media_region, meeting_host_id: Option<String>, media_placement: MediaPlacement, meeting_features: Option<MeetingFeaturesConfiguration>, primary_meeting_id: Option<String>, tenant_ids: Vec<String>, meeting_arn, notifications_configuration: Option<NotificationsConfiguration>, tags: Vec<Tag>, attendees: HashMap<String /*AttendeeId*/, Attendee>, transcription: Option<TranscriptionConfiguration>, created_at }`
- `MediaPlacement { audio_host_url, audio_fallback_url, signaling_url, turn_control_url, screen_data_url, screen_sharing_url, screen_viewing_url, event_ingestion_url }` — derived deterministically from the MeetingId so repeated GetMeeting calls are stable.
- `Attendee { attendee_id, external_user_id, join_token, capabilities: AttendeeCapabilities { audio, video, content } }`
- `idempotency: HashMap<String /*ClientRequestToken*/, String /*MeetingId*/>` so CreateMeeting replays return the original meeting.
- Tag storage keyed by `MeetingArn` can reuse `Meeting.tags` (used later by TagResource / ListTagsForResource).

## Out of scope for this first slice

BatchCreateAttendee, CreateMeetingWithAttendees, UpdateAttendeeCapabilities, BatchUpdateAttendeeCapabilitiesExcept, Start/StopMeetingTranscription, TagResource, UntagResource, ListTagsForResource stay 501 and will be filed as individual gap issues later.

## Evidence

Spec: `services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/OPERATIONS.md` lists all 16 operations with routes, required inputs and errors — CreateMeeting (`POST /meetings` → `Meeting`), CreateAttendee (`POST /meetings/{MeetingId}/attendees` → `Attendee`), GetAttendee, ListAttendees (@paginated NextToken/max-results 1..100), GetMeeting, DeleteAttendee (204), DeleteMeeting (204), each declaring `NotFoundException (HTTP 404)`. `smithy_query` on the `Meeting`, `Attendee`, `MediaPlacement`, `AttendeeCapabilities` shapes gives the exact output members cited above (MeetingId/MeetingArn GUID + `^arn:` patterns, Attendee.JoinToken, AttendeeCapabilities{Audio,Video,Content} all @required). Implementation: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 16 handler files (create_meeting.rs, create_attendee.rs, get_attendee.rs, list_attendees.rs, get_meeting.rs, delete_attendee.rs, delete_meeting.rs, …), each returning `crate::scaffolded::Scaffolded::not_implemented(..)` ⇒ HTTP 501. No tests exist for any of them.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `chime-sdk-meetings` client at it.
2. CreateMeeting(ClientRequestToken='tok-1', MediaRegion='us-east-1', ExternalMeetingId='ext-meeting-1') — expect 200 with Meeting.MeetingId as a GUID, populated MediaPlacement URLs and a MeetingArn; actual: HTTP 501 NotImplemented.
3. Repeat the identical CreateMeeting call — expect the same MeetingId returned (idempotency token).
4. CreateAttendee(MeetingId=<id>, ExternalUserId='user-1') — expect 200 with an Attendee carrying a GUID AttendeeId, a JoinToken and Capabilities defaulted to SendReceive/SendReceive/SendReceive.
5. GetAttendee(MeetingId=<id>, AttendeeId=<aid>) — expect the identical attendee record created in the previous step.
6. ListAttendees(MeetingId=<id>, MaxResults=1) — expect the created attendee, and a NextToken that pages through the rest without repeats.
7. GetMeeting(MeetingId=<id>) — expect the same Meeting body CreateMeeting returned.
8. DeleteAttendee(MeetingId=<id>, AttendeeId=<aid>) — expect 204; then GetAttendee ⇒ NotFoundException 404 and ListAttendees no longer lists it.
9. DeleteMeeting(MeetingId=<id>) — expect 204; then GetMeeting ⇒ NotFoundException 404 and CreateAttendee on that meeting ⇒ NotFoundException 404.
