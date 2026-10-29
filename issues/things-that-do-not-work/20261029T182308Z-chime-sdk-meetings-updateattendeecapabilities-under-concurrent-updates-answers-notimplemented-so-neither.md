Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-meetings.concurrent-capability-updates-final-state-consistent
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings.concurrent-capability-updates-final-state-consistent|run#9|AssertionError|assert status in ("ok", "ConflictException"), f"unexpected contention error: {status}"
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# UpdateAttendeeCapabilities under concurrent updates answers NotImplemented, so neither the ConflictException contention contract nor a consistent final capability state is observable

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAttendeeCapabilities

## Description
Cause 1 of 1: Concurrent UpdateAttendeeCapabilities calls against one attendee are refused with `NotImplemented` instead of each racer either succeeding or being refused with `ConflictException`, and the attendee's final capability set is therefore never reconciled.

AWS: Smithy: `UpdateAttendeeCapabilities` (PUT /meetings/{MeetingId}/attendees/{AttendeeId}/capabilities, success code 200) takes required `MeetingId`, `AttendeeId` (both GuidString, pattern ^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$) and required `Capabilities` (AttendeeCapabilities), and returns `Attendee`. Its modeled client errors are BadRequestException (400), ConflictException (409), ForbiddenException (403), NotFoundException (404), ThrottlingException (429), UnauthorizedException (401). `ConflictException` (409) is the modeled outcome for a request that loses a contention race on the attendee; a request that wins returns 200 with the `Attendee` whose `Capabilities` equal the set that was applied. Consequently, when several individually-valid capability sets are applied concurrently to the same attendee, every call terminates as either 200 or 409/ConflictException — never any other code — and a subsequent GetAttendee returns `Attendee.Capabilities` exactly equal to one of the sets that returned 200, never a mix of members drawn from different sets.

Emulator: every concurrent call is refused with error code `NotImplemented` and the message `operation UpdateAttendeeCapabilities is scaffolded but not implemented in this emulator` (HTTP 501). In that message the fixed text is the scaffold wording and `UpdateAttendeeCapabilities` is the operation name from the request; none of it is an AWS error class. No call returns 200, no call returns 409 with code `ConflictException`, nothing is persisted, and the attendee's capabilities never change.

Why: a client that fans out capability changes (a moderator demoting several attendees, an SDK retry storm) cannot distinguish "lost the race, retry" from "operation unavailable": it sees an unmodeled code its error handling has no branch for, so the ConflictException retry path is never exercised and the write-after-write ordering the client depends on is untestable. The attendee also silently keeps its creation-time capabilities, so downstream media-permission logic reads stale state.

Fix: belongs in the `UpdateAttendeeCapabilities` handler and the attendee capability state it mutates — the handler must apply the requested `Capabilities` to the stored attendee under a per-attendee serialization so that concurrent writers are either applied whole or refused with ConflictException (409), and must never leave a partially-applied capability set. The already-open issue `20261029T175426Z-chime-sdk-meetings-updateattendeecapabilities-stub.md` asks for that handler to be implemented, but its `Issue-Trace-Ids` list only `chime-sdk-meetings.update-attendee-capabilities-success-roundtrip` and `chime-sdk-meetings.update-attendee-capabilities-video-maxresolution-none-rejected`; the concurrency contract below is not in its replay list and will not be re-replayed by that issue's fix, which is why it is filed here.

Traces: chime-sdk-meetings.concurrent-capability-updates-final-state-consistent — four distinct, individually-valid capability sets applied concurrently to one attendee; the check asserts each outcome is `ok` or `ConflictException` and fails with `AssertionError: unexpected contention error: NotImplemented`.

## Evidence
Expected (AWS / Smithy model): each concurrent UpdateAttendeeCapabilities returns HTTP 200 with the updated `Attendee`, or HTTP 409 with error code `ConflictException`; at least one returns 200; a following GetAttendee returns `Attendee.Capabilities` exactly equal to one of the sets that returned 200.

Actual (emulator): every concurrent call fails with HTTP 501, error code `NotImplemented`, message `operation UpdateAttendeeCapabilities is scaffolded but not implemented in this emulator`. Replay output: `AssertionError: unexpected contention error: NotImplemented` at check_chime_sdk_meetings_concurrent_capability_updates_final_state_consistent.py line 53 (`assert status in ("ok", "ConflictException")`). No capability change is persisted, so the final GetAttendee comparison against the winning set is never reached.

## Steps to Reproduce
1. Boot the chime-sdk-meetings emulator.
2. CreateMeeting with a fresh ClientRequestToken, MediaRegion=us-east-1 and a fresh ExternalMeetingId; keep Meeting.MeetingId.
3. CreateAttendee on that meeting with a fresh ExternalUserId; keep Attendee.AttendeeId.
4. From 4 threads concurrently call UpdateAttendeeCapabilities(MeetingId, AttendeeId, Capabilities=...) with the four distinct sets {Audio:SendReceive,Video:SendReceive,Content:SendReceive}, {Audio:Send,Video:Send,Content:None}, {Audio:Receive,Video:None,Content:None}, {Audio:None,Video:Receive,Content:None}.
5. Observe every call fails with code NotImplemented (HTTP 501) instead of returning 200 or ConflictException (409).
6. Call GetAttendee(MeetingId, AttendeeId) and observe Attendee.Capabilities is unchanged from creation rather than equal to one of the sets that succeeded.
