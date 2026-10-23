Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.content-delete-removes-and-not-found wisdom.content-metadata-and-linkouturi-persisted wisdom.content-search-finds-created-content wisdom.content-update-persists-title-and-bumps-revision wisdom.content-update-stale-revision-precondition-failed-rejected wisdom.content-upload-and-download-round-trip wisdom.create-content-duplicate-name-conflict-rejected wisdom.get-content-summary-matches-full-content-and-omits-url wisdom.update-content-removes-override-link-out-uri wisdom.update-content-revisionid-race-concurrency
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|StartContentUpload|NotImplemented
Issue-Commit: 685e94dfb816aa33c7cd8ca632f897bdb6a87341

# Implement wisdom StartContentUpload: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `685e94dfb816aa33c7cd8ca632f897bdb6a87341`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartContentUpload

## Description
`StartContentUpload` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-wisdom-wisdomservice/src/ops/start_content_upload.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `StartContentUpload` call, so each one unblocks only when `StartContentUpload` works: `wisdom.content-delete-removes-and-not-found`, `wisdom.content-metadata-and-linkouturi-persisted`, `wisdom.content-search-finds-created-content`, `wisdom.content-update-persists-title-and-bumps-revision`, `wisdom.content-update-stale-revision-precondition-failed-rejected`, `wisdom.content-upload-and-download-round-trip`, `wisdom.create-content-duplicate-name-conflict-rejected`, `wisdom.get-content-summary-matches-full-content-and-omits-url`, `wisdom.update-content-removes-override-link-out-uri`, `wisdom.update-content-revisionid-race-concurrency`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-wisdom-wisdomservice/src/ops/start_content_upload.rs`. Replaying the corpus, the emulator refused the `StartContentUpload` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartContentUpload operation: operation StartContentUpload is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the wisdom emulator
Call StartContentUpload and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
