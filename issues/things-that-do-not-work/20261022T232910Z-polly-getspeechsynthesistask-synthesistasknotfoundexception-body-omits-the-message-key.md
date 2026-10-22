Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.get-speech-synthesis-task-not-found-rejected polly.get-speech-synthesis-task-invalid-id-format-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.get-speech-synthesis-task-not-found-rejected|run#11|AssertionError|assert message_missing is None, f"expected no error message, got {message_missing!r}"
Issue-Key: polly.get-speech-synthesis-task-invalid-id-format-rejected|run#10|AssertionError|assert unknown_message is None, f"expected no error message, got {unknown_message!r}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Polly GetSpeechSynthesisTask: SynthesisTaskNotFoundException body omits the `message` key instead of carrying it as JSON null, so SDK clients read an empty string where AWS gives none

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSpeechSynthesisTask

## Description
1 cause.

Cause 1 of 1: the serialized error document for SynthesisTaskNotFoundException differs in the presence of its `message` member.
AWS: Smithy: GetSpeechSynthesisTask declares SynthesisTaskNotFoundException, errorKind client, httpError 400, with a single optional member `message` (target ErrorMessage). Recorded behavior only: for a well-formed TaskId (matching ^[a-zA-Z0-9_-]{1,100}$) that names no stored task, Polly answers HTTP 400 with code SynthesisTaskNotFoundException and a null message — the recordings surface Error.Message as Python None through botocore, i.e. the restJson1 error body carries the `message` key with a JSON null value rather than leaving it out.
Emulator: the same request yields the correct HTTP 400 and the correct code SynthesisTaskNotFoundException, but Error.Message is the empty string '' — the value botocore substitutes when the error body contains no `message` key at all. The handler (services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs) correctly builds crate::service::errors::synthesis_task_not_found() with message: None; the divergence is in how an absent optional error member is rendered into the restJson1 error document (dropped entirely instead of emitted as null).
Why: a client that distinguishes 'no message supplied' from 'an empty message' — logging, error-reporting, or any `if message is None` branch — behaves differently against the emulator than against AWS, and a conformance assertion on the absent message cannot pass. The status and the error code are unaffected.
Fix: the error serializer for this service's restJson1 error envelope (how a None optional `message` on a modeled exception is written), not the handler, which already models the absence correctly.
Traces: polly.get-speech-synthesis-task-not-found-rejected, polly.get-speech-synthesis-task-invalid-id-format-rejected (both reach this leg only after the preceding ValidationException leg passed, so the pattern-level rejection and its message are already faithful).

## Evidence
polly.get-speech-synthesis-task-not-found-rejected:
  assert message_missing is None, f"expected no error message, got {message_missing!r}"
  AssertionError: expected no error message, got ''

polly.get-speech-synthesis-task-invalid-id-format-rejected:
  assert unknown_message is None, f"expected no error message, got {unknown_message!r}"
  AssertionError: expected no error message, got ''

In both checks the preceding assertions pass: GetSpeechSynthesisTask with TaskId='bad id! not/valid' is rejected with ValidationException and exactly the expected framework message, and a well-formed unknown TaskId ('a'*40) is rejected with code SynthesisTaskNotFoundException. Only the message member's presence diverges: expected None (key present, null), observed '' (key absent).

Source context: services/service-com-amazonaws-polly-parrot-v1/src/service.rs lines 324-329 build SynthesisTaskNotFoundException { message: None } with the comment that Polly answers this error with a null message body; services/service-com-amazonaws-polly-parrot-v1/src/ops/get_speech_synthesis_task.rs line 163 returns it.

## Steps to Reproduce
1. Boot the polly emulator.
2. Call GetSpeechSynthesisTask with TaskId='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' (well-formed, never created).
3. Observe HTTP 400 with code SynthesisTaskNotFoundException, and inspect the raw JSON error body: the `message` key is absent, so botocore reports Error.Message == ''. AWS returns the key with a null value, which botocore reports as None.
4. Replay polly.get-speech-synthesis-task-not-found-rejected and polly.get-speech-synthesis-task-invalid-id-format-rejected.
