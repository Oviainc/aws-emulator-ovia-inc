Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.acknowledge-third-party-job-invalid-nonce-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.acknowledge-third-party-job-invalid-nonce-rejected|run#4|AssertionError|assert (code, message) == ("JobNotFoundException", f"Job with id '{unknown_job_id}' does not exist."), (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codepipeline AcknowledgeThirdPartyJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcknowledgeThirdPartyJob

## Description
`AcknowledgeThirdPartyJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/acknowledge_third_party_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AcknowledgeThirdPartyJob` call, so each one unblocks only when `AcknowledgeThirdPartyJob` works: `codepipeline.acknowledge-third-party-job-invalid-nonce-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/acknowledge_third_party_job.rs`. Replaying the corpus, the emulator refused the `AcknowledgeThirdPartyJob` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codepipeline_acknowledge_third_party_job_invalid_nonce_rejected.py", line 46, in run
    assert (code, message) == ("JobNotFoundException", f"Job with id '{unknown_job_id}' does not exist."), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected JobNotFoundException for a nonexistent job id, got ('NotImplemented', 'operation AcknowledgeThirdPartyJob is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the codepipeline emulator
Call AcknowledgeThirdPartyJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
