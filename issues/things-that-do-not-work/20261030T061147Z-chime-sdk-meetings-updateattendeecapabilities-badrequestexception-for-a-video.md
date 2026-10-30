Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-meetings.update-attendee-capabilities-video-maxresolution-none-rejected
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings.update-attendee-capabilities-video-maxresolution-none-rejected|run#11|AssertionError|assert message == want_message, f"expected message {want_message!r}, got {message!r}"
Issue-Commit: b68a8f9aac6fc751d687dccee08f38615a28e768

# chime-sdk-meetings: UpdateAttendeeCapabilities BadRequestException for a Video capability against MeetingFeatures.Video.MaxResolution=None uses the emulator's own wording, not AWS's message

> _Note: this issue was identified against the code base at commit `b68a8f9aac6fc751d687dccee08f38615a28e768`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAttendeeCapabilities

## Description
Cause 1 of 1: the BadRequestException raised by UpdateAttendeeCapabilities when the requested AttendeeCapabilities.Video is anything other than `None` for a meeting created with MeetingFeatures.Video.MaxResolution=`None` carries a message the emulator invented instead of the message AWS returns for that error class. Failing check: chime-sdk-meetings.update-attendee-capabilities-video-maxresolution-none-rejected.

AWS: Smithy (services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json) models UpdateAttendeeCapabilities with required member Capabilities -> AttendeeCapabilities{Audio,Video,Content: MediaCapabilities} and error BadRequestException (errorKind client, httpError 400, members Code/Message/RequestId). The AttendeeCapabilities shape documentation states the rule directly: "If you specify MeetingFeatures:Video:MaxResolution:None when you create a meeting, all API requests that include SendReceive, Send, or Receive for AttendeeCapabilities:Video will be rejected with ValidationError 400." The recorded AWS response for that rejection is HTTP 400, error code `BadRequestException`, with the message text:

    Attendee's video capability must be None if MeetingFeatures is configured with Video:MaxResolution:None

That message is entirely FIXED text for this error class — it interpolates no request value, no attendee id, no meeting id and no capability value. There is a sibling form for the Content axis (same sentence with the content capability and Content:MaxResolution:None), which is the only part that varies: which capability axis (video/Video, content/Content) the meeting feature forbade.

Emulator: the refusal happens (status 400, code BadRequestException is correct), but the Message member is the emulator's own diagnostic prose: 'Capabilities.Video is SendReceive, but the meeting was created with MeetingFeatures.Video.MaxResolution None, which admits only None'. It also splices the REQUESTED capability value (`SendReceive`) into the text, which AWS's message never contains.

Why: the error message is part of the operation's contract. Clients, SDK error-mapping layers, log-based alerting and test assertions that match AWS's documented message text (or that surface it verbatim to an end user) see text that never appears in a real response, so a capability/meeting-feature conflict is not recognised as such, and the value spliced in makes the text differ per request where AWS's is stable. The status and code being right makes the divergence quieter, not smaller.

Fix: the wording belongs in the shared capability validator that produces this problem string, `crate::state::capability_problem` (and the `crate::state::bad_request` wrapper that lifts it into BadRequestException) in the chime-sdk-meetings service state; UpdateAttendeeCapabilities' handler already routes the problem into the modelled BadRequestException correctly, so only the produced message text needs to change. The same validator is shared with CreateAttendee, so fix it there once rather than per handler. Note the open issue 20261029T175426Z-chime-sdk-meetings-6-net-new-root-causes.md covers a DIFFERENT corner case of the same validator (content-capability-without-video) and does not cover this Video/MaxResolution=None wording; this is a separate cause.

Traces: chime-sdk-meetings.update-attendee-capabilities-video-maxresolution-none-rejected.

## Evidence
Check chime-sdk-meetings.update-attendee-capabilities-video-maxresolution-none-rejected, verdict mismatch:

  AssertionError: expected message "Attendee's video capability must be None if MeetingFeatures is configured with Video:MaxResolution:None", got 'Capabilities.Video is SendReceive, but the meeting was created with MeetingFeatures.Video.MaxResolution None, which admits only None'

Expected (AWS): HTTP 400, Error.Code = BadRequestException, Error.Message = "Attendee's video capability must be None if MeetingFeatures is configured with Video:MaxResolution:None" (all fixed text; only the capability axis named varies between the Video and Content forms).
Actual (emulator): HTTP 400, Error.Code = BadRequestException (correct), Error.Message = "Capabilities.Video is SendReceive, but the meeting was created with MeetingFeatures.Video.MaxResolution None, which admits only None" — different fixed text, plus the requested capability value interpolated.

The check's other assertions held: the positive control (Audio-only update) succeeded, the error code was BadRequestException, and the rejected call left capabilities unchanged. Only the message text diverges.

Source of the wording: services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/ops/update_attendee_capabilities.rs:231-238 calls crate::state::capability_problem(...) and wraps its string via crate::state::bad_request(problem).

## Steps to Reproduce
1. CreateMeeting with ClientRequestToken, MediaRegion=us-east-1, a unique ExternalMeetingId, and MeetingFeatures={"Video":{"MaxResolution":"None"},"Content":{"MaxResolution":"None"}}.
2. CreateAttendee in that meeting with Capabilities={"Audio":"SendReceive","Video":"None","Content":"None"}.
3. (Positive control, succeeds) UpdateAttendeeCapabilities on that attendee with Capabilities={"Audio":"Receive","Video":"None","Content":"None"}.
4. Call UpdateAttendeeCapabilities on the same attendee with Capabilities={"Audio":"Receive","Video":"SendReceive","Content":"None"}.
5. Inspect the error: code is BadRequestException (HTTP 400) as expected, but Error.Message is the emulator's prose rather than "Attendee's video capability must be None if MeetingFeatures is configured with Video:MaxResolution:None".
