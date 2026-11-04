Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2.event-source-lifecycle-against-active-bus
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|CreateEventSource|NotImplemented
Issue-Commit: e5b2e36fc94a41937ab22fe33196324420972c76

# Implement eventbridgev2 CreateEventSource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e5b2e36fc94a41937ab22fe33196324420972c76`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSource

## Description
`CreateEventSource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_source.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateEventSource` call, so each one unblocks only when `CreateEventSource` works: `eventbridgev2.event-source-lifecycle-against-active-bus`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/create_event_source.rs`. Replaying the corpus, the emulator refused the `CreateEventSource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateEventSource operation: operation CreateEventSource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the eventbridgev2 emulator
Call CreateEventSource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
