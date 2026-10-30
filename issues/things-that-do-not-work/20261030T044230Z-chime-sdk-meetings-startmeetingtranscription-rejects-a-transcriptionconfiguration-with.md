Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-meetings.stop-meeting-transcription-not-active-is-noop
Issue-Shape-Id: com.amazonaws.chimesdkmeetings#ChimeMeetingsSDKService
Issue-Key: chime-sdk-meetings.stop-meeting-transcription-not-active-is-noop|run#14|AssertionError|assert bad_message == want_message, f"expected message {want_message!r}, got {bad_message!r}"
Issue-Commit: 77df9ab14707543ffeca12b439613a255d8cc0c7

# chime-sdk-meetings: StartMeetingTranscription rejects a TranscriptionConfiguration with neither/both engine settings using the emulator's own wording instead of AWS's BadRequestException message

> _Note: this issue was identified against the code base at commit `77df9ab14707543ffeca12b439613a255d8cc0c7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMeetingTranscription

## Description
One net-new root cause.

Cause 1 of 1: StartMeetingTranscription's cross-member validation of TranscriptionConfiguration refuses with the right error class but the wrong message text.

AWS: Smithy (services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json) — StartMeetingTranscription input members MeetingId (GuidString, httpLabel, required) and TranscriptionConfiguration (required); modeled errors include BadRequestException (HTTP 400, "The input parameters don't match the service's restrictions"). The model documents on TranscriptionConfiguration only that it "Must contain EngineTranscribeSettings or EngineTranscribeMedicalSettings"; it does NOT state the message text. For the text this is recorded behavior only: the replayed recording shows AWS answering HTTP 400 BadRequestException with the message "Exactly one out of EngineTranscribeSettings and EngineTranscribeMedicalSettings must be specified in the TranscriptionConfiguration" when TranscriptionConfiguration carries neither engine settings block. That whole sentence is FIXED text for this error class — it contains no value taken from the request or from state; the only variable parts AWS could substitute are the member names EngineTranscribeSettings / EngineTranscribeMedicalSettings / TranscriptionConfiguration, which are modeled member names and so are fixed for this operation. The same rule class covers the symmetric violation (BOTH engine settings specified), which is why AWS words it as "Exactly one out of ...".

Emulator: answers BadRequestException (code matches, status matches) but with the message "TranscriptionConfiguration must contain either EngineTranscribeSettings or EngineTranscribeMedicalSettings". The replay assertion reads: expected message 'Exactly one out of EngineTranscribeSettings and EngineTranscribeMedicalSettings must be specified in the TranscriptionConfiguration', got 'TranscriptionConfiguration must contain either EngineTranscribeSettings or EngineTranscribeMedicalSettings'. The rest of the check passed: the repeated no-op StopMeetingTranscription calls returned 200 and Start-then-Stop succeeded, so only the refusal's wording diverges — not its code or status.

Why: the error message is part of the contract. Clients, SDK retry/branching helpers and test suites that match on the BadRequestException message text (or surface it to operators/users) see a sentence real AWS never emits, so message-based classification of "wrong transcription configuration" silently fails and user-visible diagnostics differ from production. Because the emulator's wording also omits AWS's "exactly one" framing, a caller cannot distinguish the neither-specified from the both-specified violation the way AWS's single sentence does.

Fix: the operation's validation — the configuration_problem helper in the StartMeetingTranscription handler, which composes the BadRequestException message for the (None, None) and (Some, Some) arms of the EngineTranscribeSettings/EngineTranscribeMedicalSettings pair. Reword that arm to AWS's fixed sentence; the neighbouring EngineTranscribeSettings cross-member messages are not exercised by this check and should be confirmed separately against recordings.

Traces: chime-sdk-meetings.stop-meeting-transcription-not-active-is-noop (the StartMeetingTranscription(TranscriptionConfiguration={}) leg at the end of the check).

## Evidence
Check: chime-sdk-meetings.stop-meeting-transcription-not-active-is-noop [mismatch]

Request: StartMeetingTranscription on a live meeting with TranscriptionConfiguration={} (neither EngineTranscribeSettings nor EngineTranscribeMedicalSettings).

Expected (AWS, as recorded): HTTP 400, Error.Code = "BadRequestException", Error.Message = "Exactly one out of EngineTranscribeSettings and EngineTranscribeMedicalSettings must be specified in the TranscriptionConfiguration".

Actual (emulator): HTTP 400, Error.Code = "BadRequestException" (correct), Error.Message = "TranscriptionConfiguration must contain either EngineTranscribeSettings or EngineTranscribeMedicalSettings".

Replay assertion: AssertionError: expected message 'Exactly one out of EngineTranscribeSettings and EngineTranscribeMedicalSettings must be specified in the TranscriptionConfiguration', got 'TranscriptionConfiguration must contain either EngineTranscribeSettings or EngineTranscribeMedicalSettings'

Source of the diverging text: services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/src/ops/start_meeting_transcription.rs, fn configuration_problem, the (None, None) | (Some(_), Some(_)) arm at lines 257-261, whose string is returned through crate::state::bad_request at lines 215-220.

Message-part breakdown: the expected sentence is entirely fixed text for this error class; no request value, id, timestamp or limit appears in it. The member names it cites (EngineTranscribeSettings, EngineTranscribeMedicalSettings, TranscriptionConfiguration) come from the Smithy model, not from the recording.

## Steps to Reproduce
1. CreateMeeting(ClientRequestToken=<unique>, MediaRegion="us-east-1", ExternalMeetingId=<unique>) and take Meeting.MeetingId.
2. Call StartMeetingTranscription(MeetingId=<that id>, TranscriptionConfiguration={}) — an empty configuration object, so neither EngineTranscribeSettings nor EngineTranscribeMedicalSettings is present.
3. Observe the ClientError: the code is BadRequestException (HTTP 400) as AWS gives, but Error.Message is "TranscriptionConfiguration must contain either EngineTranscribeSettings or EngineTranscribeMedicalSettings" instead of AWS's "Exactly one out of EngineTranscribeSettings and EngineTranscribeMedicalSettings must be specified in the TranscriptionConfiguration".
4. Same divergence for the symmetric violation: StartMeetingTranscription with BOTH EngineTranscribeSettings and EngineTranscribeMedicalSettings set takes the same emulator arm and therefore the same non-AWS message.
5. Re-replay chime-sdk-meetings.stop-meeting-transcription-not-active-is-noop to confirm a fix.
