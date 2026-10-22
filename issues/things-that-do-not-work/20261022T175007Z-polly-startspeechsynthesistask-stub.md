Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.list-speech-synthesis-tasks-ordered-by-creation-time-descending polly.list-speech-synthesis-tasks-status-filter polly.speech-synthesis-task-completes-and-uploads-to-s3 polly.speech-synthesis-task-sns-notification-on-completion polly.start-speech-synthesis-task-invalid-sns-topic-arn-error polly.start-speech-synthesis-task-json-speech-marks-to-s3 polly.start-speech-synthesis-task-output-s3-key-prefix-applied
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly|StartSpeechSynthesisTask|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Implement polly StartSpeechSynthesisTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisTask

## Description
`StartSpeechSynthesisTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `StartSpeechSynthesisTask` call, so each one unblocks only when `StartSpeechSynthesisTask` works: `polly.list-speech-synthesis-tasks-ordered-by-creation-time-descending`, `polly.list-speech-synthesis-tasks-status-filter`, `polly.speech-synthesis-task-completes-and-uploads-to-s3`, `polly.speech-synthesis-task-sns-notification-on-completion`, `polly.start-speech-synthesis-task-invalid-sns-topic-arn-error`, `polly.start-speech-synthesis-task-json-speech-marks-to-s3`, `polly.start-speech-synthesis-task-output-s3-key-prefix-applied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs`. Replaying the corpus, the emulator refused the `StartSpeechSynthesisTask` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartSpeechSynthesisTask operation: operation StartSpeechSynthesisTask is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the polly emulator
Call StartSpeechSynthesisTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
