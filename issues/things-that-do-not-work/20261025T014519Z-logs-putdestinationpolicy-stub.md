Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.destination-lifecycle logs.e2e-log-group-subscription-destination-delivery-flow logs.put-destination-then-get
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|PutDestinationPolicy|NotImplemented
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement logs PutDestinationPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDestinationPolicy

## Description
`PutDestinationPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_destination_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `PutDestinationPolicy` call, so each one unblocks only when `PutDestinationPolicy` works: `logs.destination-lifecycle`, `logs.e2e-log-group-subscription-destination-delivery-flow`, `logs.put-destination-then-get`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_destination_policy.rs`. Replaying the corpus, the emulator refused the `PutDestinationPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutDestinationPolicy operation: operation PutDestinationPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call PutDestinationPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
