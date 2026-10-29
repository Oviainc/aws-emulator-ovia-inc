Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: databrew.schedule-rejects-non-cron-expression-and-unknown-job
Issue-Shape-Id: com.amazonaws.databrew#AWSGlueDataBrew
Issue-Key: databrew.schedule-rejects-non-cron-expression-and-unknown-job|run#14|AssertionError|raise AssertionError(f"UpdateSchedule with a bare cron raised {code}: {message!r}")
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# Implement databrew UpdateSchedule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSchedule

## Description
`UpdateSchedule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/update_schedule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateSchedule` call, so each one unblocks only when `UpdateSchedule` works: `databrew.schedule-rejects-non-cron-expression-and-unknown-job`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-databrew-awsgluedatabrew/src/ops/update_schedule.rs`. Replaying the corpus, the emulator refused the `UpdateSchedule` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_databrew_schedule_rejects_non_cron_expression_and_unknown_job.py", line 90, in run
    raise AssertionError(f"UpdateSchedule with a bare cron raised {code}: {message!r}")
AssertionError: UpdateSchedule with a bare cron raised NotImplemented: 'operation UpdateSchedule is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the databrew emulator
Call UpdateSchedule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
