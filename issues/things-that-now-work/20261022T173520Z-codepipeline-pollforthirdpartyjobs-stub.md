Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.poll-for-third-party-jobs-access-denied
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.poll-for-third-party-jobs-access-denied|run#5|AssertionError|assert error.get("Code") == "AccessDeniedException", f"expected AccessDeniedException for a non-partner caller, got {error!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codepipeline PollForThirdPartyJobs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PollForThirdPartyJobs

## Description
`PollForThirdPartyJobs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/poll_for_third_party_jobs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PollForThirdPartyJobs` call, so each one unblocks only when `PollForThirdPartyJobs` works: `codepipeline.poll-for-third-party-jobs-access-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/poll_for_third_party_jobs.rs`. Replaying the corpus, the emulator refused the `PollForThirdPartyJobs` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codepipeline_poll_for_third_party_jobs_access_denied.py", line 43, in run
    assert error.get("Code") == "AccessDeniedException", f"expected AccessDeniedException for a non-partner caller, got {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDeniedException for a non-partner caller, got {'Message': 'operation PollForThirdPartyJobs is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the codepipeline emulator
Call PollForThirdPartyJobs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
