Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehub.publish-of-empty-app-template-rejected
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub.publish-of-empty-app-template-rejected|run#9|AssertionError|assert code == "ConflictException", (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement resiliencehub PublishAppVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishAppVersion

## Description
`PublishAppVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/publish_app_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PublishAppVersion` call, so each one unblocks only when `PublishAppVersion` works: `resiliencehub.publish-of-empty-app-template-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/publish_app_version.rs`. Replaying the corpus, the emulator refused the `PublishAppVersion` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_resiliencehub_publish_of_empty_app_template_rejected.py", line 121, in run
    assert code == "ConflictException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: publishing an empty draft returned 'NotImplemented', expected ConflictException ('operation PublishAppVersion is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the resiliencehub emulator
Call PublishAppVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
