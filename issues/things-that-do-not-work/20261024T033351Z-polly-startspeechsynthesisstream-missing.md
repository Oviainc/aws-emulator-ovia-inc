Issue-Severity: HIGH
Issue-Gap-Class: missing
Issue-Commit: b7068a3aaca27128a9512527e96d46285251fa2e

# Implement StartSpeechSynthesisStream: the operation has no handler at all and is not routable

_Filed: 2026-10-24T03:33:51Z_
_Operation: StartSpeechSynthesisStream | Class: missing_

> _Note: this issue was identified against the code base at commit `b7068a3aaca27128a9512527e96d46285251fa2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model binds StartSpeechSynthesisStream (POST /v1/synthesisStream -> 200, bidirectional event stream: StartSpeechSynthesisStreamActionStream in, StartSpeechSynthesisStreamEventStream out). The emulator has NO handler file for it — src/ops/ contains one file per operation for the other nine, and start_speech_synthesis_stream.rs does not exist. The operation is also not in the service's generated operations(...) allow-list, so the request is answered by the framework's unroutable-operation path (a rest-protocol 501 that is not one of the operation's modeled errors) rather than by any handler.

Expected: a POST to /v1/synthesisStream with x-amzn-Engine, x-amzn-OutputFormat and x-amzn-VoiceId headers and an event-stream payload opens a synthesis stream: input TextEvent/CloseStreamEvent action events are consumed and AudioEvent (plus the stream's other modeled events) are framed back as an event stream with HTTP 200, and the modeled refusals (ValidationException 400, ThrottlingException 400, ServiceQuotaExceededException 402, ServiceFailureException 500) are used for failures.
Actual: every request answers 501 NotImplemented from the routing path; no part of the operation exists (no header binding, no event-stream framing, no voice/engine/lexicon resolution).

## Evidence

Smithy model: smithy_query(query="operation", operation="StartSpeechSynthesisStream") -> http POST /v1/synthesisStream code 200, required Engine/OutputFormat/VoiceId @httpHeader members, @streaming @httpPayload ActionStream input and EventStream output, errors ServiceFailureException(500)/ServiceQuotaExceededException(402)/ThrottlingException(400)/ValidationException(400). Repository: glob of services/service-com-amazonaws-polly-parrot-v1/src/ops/*.rs lists nine operation files and no start_speech_synthesis_stream.rs; src/ops/SKIPPED_STUBS.md records "// SKIPPED: StartSpeechSynthesisStream — unsupported shape construct: operation 'StartSpeechSynthesisStream' has a @streaming event-stream input member, which needs wire framing the generator does not emit" and explains such an op is "left modeled-but-unwired" and "answered by the framework's unroutable-operation path". No test under tests/all/ exercises /v1/synthesisStream (grep for synthesisStream/StartSpeechSynthesisStream finds only OPERATIONS.md and SKIPPED_STUBS.md). Not covered by any existing issue (list_existing_issues(query="polly") shows issues for SynthesizeSpeech, StartSpeechSynthesisTask, GetSpeechSynthesisTask, ListSpeechSynthesisTasks, PutLexicon only).

## Steps to Reproduce

1. Boot the emulator and POST to /v1/synthesisStream with headers x-amzn-Engine: standard, x-amzn-OutputFormat: pcm, x-amzn-VoiceId: Joanna and an event-stream body carrying a TextEvent followed by a CloseStreamEvent
2. Observe HTTP 501 with error code NotImplemented from the framework's unroutable-operation path instead of a 200 event-stream response carrying AudioEvent frames
3. Note that no handler file exists: services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_stream.rs is absent and the operation is listed in src/ops/SKIPPED_STUBS.md
