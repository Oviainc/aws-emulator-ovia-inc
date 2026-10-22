Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.concurrent-start-job-same-token-idempotent-dedup comprehend.describe-job-not-found-rejected comprehend.start-job-duplicate-token-mismatched-params-creates-second-job comprehend.start-key-phrases-detection-job-reaches-completed comprehend.tag-job-resource-roundtrip comprehend.untag-resource-nonexistent-resource-rejected
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|StartKeyPhrasesDetectionJob|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement comprehend StartKeyPhrasesDetectionJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartKeyPhrasesDetectionJob

## Description
`StartKeyPhrasesDetectionJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/start_key_phrases_detection_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `StartKeyPhrasesDetectionJob` call, so each one unblocks only when `StartKeyPhrasesDetectionJob` works: `comprehend.concurrent-start-job-same-token-idempotent-dedup`, `comprehend.describe-job-not-found-rejected`, `comprehend.start-job-duplicate-token-mismatched-params-creates-second-job`, `comprehend.start-key-phrases-detection-job-reaches-completed`, `comprehend.tag-job-resource-roundtrip`, `comprehend.untag-resource-nonexistent-resource-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/start_key_phrases_detection_job.rs`. Replaying the corpus, the emulator refused the `StartKeyPhrasesDetectionJob` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartKeyPhrasesDetectionJob operation: operation StartKeyPhrasesDetectionJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the comprehend emulator
Call StartKeyPhrasesDetectionJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
