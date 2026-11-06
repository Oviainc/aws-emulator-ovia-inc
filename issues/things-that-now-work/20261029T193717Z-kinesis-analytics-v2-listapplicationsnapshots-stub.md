Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesisanalyticsv2.snapshot-of-ready-application-rejected
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesis-analytics-v2|ListApplicationSnapshots|NotImplemented
Issue-Commit: 954781cef49baf7406aef764139782df619c6255

# Implement kinesis-analytics-v2 ListApplicationSnapshots: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `954781cef49baf7406aef764139782df619c6255`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApplicationSnapshots

## Description
`ListApplicationSnapshots` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/list_application_snapshots.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListApplicationSnapshots` call, so each one unblocks only when `ListApplicationSnapshots` works: `kinesis-analytics-v2.snapshot-of-ready-application-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/list_application_snapshots.rs`. Replaying the corpus, the emulator refused the `ListApplicationSnapshots` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListApplicationSnapshots operation: operation ListApplicationSnapshots is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis-analytics-v2 emulator
Call ListApplicationSnapshots and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
