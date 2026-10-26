Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logs-insights-semantics-stop-running-query-reports-cancelled logs.query-execution-polling-states
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|StopQuery|NotImplemented
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# Implement logs StopQuery: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopQuery

## Description
`StopQuery` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/stop_query.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `StopQuery` call, so each one unblocks only when `StopQuery` works: `logs.logs-insights-semantics-stop-running-query-reports-cancelled`, `logs.query-execution-polling-states`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/stop_query.rs`. Replaying the corpus, the emulator refused the `StopQuery` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_logs_logs_insights_semantics_stop_running_query_reports_cancelled.py", line 75, in run
    assert code == "InvalidParameterException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: a stop that lost the race to completion must be InvalidParameterException, got 'NotImplemented'

## Steps to Reproduce
Boot the logs emulator
Call StopQuery and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
