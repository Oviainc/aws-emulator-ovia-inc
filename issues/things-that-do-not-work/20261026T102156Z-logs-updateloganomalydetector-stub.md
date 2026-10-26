Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.update-log-anomaly-detector-and-list-anomalies
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|UpdateLogAnomalyDetector|NotImplemented
Issue-Commit: b37186c89a6243bd753c08865f727ca2e1551d74

# Implement logs UpdateLogAnomalyDetector: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b37186c89a6243bd753c08865f727ca2e1551d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateLogAnomalyDetector

## Description
`UpdateLogAnomalyDetector` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/update_log_anomaly_detector.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateLogAnomalyDetector` call, so each one unblocks only when `UpdateLogAnomalyDetector` works: `logs.update-log-anomaly-detector-and-list-anomalies`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/update_log_anomaly_detector.rs`. Replaying the corpus, the emulator refused the `UpdateLogAnomalyDetector` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateLogAnomalyDetector operation: operation UpdateLogAnomalyDetector is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call UpdateLogAnomalyDetector and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
