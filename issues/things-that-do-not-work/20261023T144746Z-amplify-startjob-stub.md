Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.startjob-retry-missing-jobid-rejected
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.startjob-retry-missing-jobid-rejected|run#7|AssertionError|assert err == (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# Implement amplify StartJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJob

## Description
`StartJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-amplify-amplify/src/ops/start_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StartJob` call, so each one unblocks only when `StartJob` works: `amplify.startjob-retry-missing-jobid-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-amplify-amplify/src/ops/start_job.rs`. Replaying the corpus, the emulator refused the `StartJob` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_amplify_startjob_retry_missing_jobid_rejected.py", line 28, in run
    assert err == (
           ^^^^^^^^
    ...<3 lines>...
    ), err
    ^
AssertionError: ('NotImplemented', 'operation StartJob is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the amplify emulator
Call StartJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
