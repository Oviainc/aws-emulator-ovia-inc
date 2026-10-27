Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.scheduled-query-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|DeleteScheduledQuery|NotImplemented
Issue-Commit: 1f1c94accd1d7961aa84a7b9995986ca278396b6

# Implement logs DeleteScheduledQuery: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1f1c94accd1d7961aa84a7b9995986ca278396b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScheduledQuery

## Description
`DeleteScheduledQuery` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_scheduled_query.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteScheduledQuery` call, so each one unblocks only when `DeleteScheduledQuery` works: `logs.scheduled-query-lifecycle-transitions`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_scheduled_query.rs`. Replaying the corpus, the emulator refused the `DeleteScheduledQuery` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteScheduledQuery operation: operation DeleteScheduledQuery is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call DeleteScheduledQuery and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
