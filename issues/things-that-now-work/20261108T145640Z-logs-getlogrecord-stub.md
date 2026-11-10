Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.delete-transformer-then-log-events-no-longer-transformed logs.get-log-record-field-values-match-query-results-pointer logs.get-log-record-from-query-result logs.get-log-record-returns-full-event logs.log-fields-and-record-pointer-errors-rejected logs.transformer-rewrites-ingested-events-preserving-the-raw-message
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|GetLogRecord|NotImplemented
Issue-Commit: 544316519d780db35004270636d68199c524c41e

# Implement logs GetLogRecord: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `544316519d780db35004270636d68199c524c41e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLogRecord

## Description
`GetLogRecord` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_log_record.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `GetLogRecord` call, so each one unblocks only when `GetLogRecord` works: `logs.delete-transformer-then-log-events-no-longer-transformed`, `logs.get-log-record-field-values-match-query-results-pointer`, `logs.get-log-record-from-query-result`, `logs.get-log-record-returns-full-event`, `logs.log-fields-and-record-pointer-errors-rejected`, `logs.transformer-rewrites-ingested-events-preserving-the-raw-message`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_log_record.rs`. Replaying the corpus, the emulator refused the `GetLogRecord` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetLogRecord operation: operation GetLogRecord is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call GetLogRecord and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
