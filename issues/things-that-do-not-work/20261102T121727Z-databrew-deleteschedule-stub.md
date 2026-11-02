Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.schedule-rejects-non-cron-expression-and-unknown-job
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.schedule-rejects-non-cron-expression-and-unknown-job|run#31|AssertionError|raise AssertionError(f"{label} on a missing schedule raised {code}: {message!r}")
Issue-Commit: 75d52ea0c257a701244c018d7c3f550d51e1d46f

# Implement databrew DeleteSchedule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `75d52ea0c257a701244c018d7c3f550d51e1d46f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSchedule

## Description
`DeleteSchedule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_schedule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteSchedule` call, so each one unblocks only when `DeleteSchedule` works: `databrew.schedule-rejects-non-cron-expression-and-unknown-job`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/delete_schedule.rs`. Replaying the corpus, the emulator refused the `DeleteSchedule` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_databrew_schedule_rejects_non_cron_expression_and_unknown_job.py", line 114, in run
    raise AssertionError(f"{label} on a missing schedule raised {code}: {message!r}")
AssertionError: DeleteSchedule on a missing schedule raised NotImplemented: 'operation DeleteSchedule is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the databrew emulator
Call DeleteSchedule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
