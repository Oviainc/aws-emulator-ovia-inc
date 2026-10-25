Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object events__kinesis.kinesis-target-partition-key-path-routes-by-event-field
Issue-Key: kinesis|AddTagsToStream|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Implement kinesis AddTagsToStream: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTagsToStream

## Description
`AddTagsToStream` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/add_tags_to_stream.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `AddTagsToStream` call, so each one unblocks only when `AddTagsToStream` works: `events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object`, `events__kinesis.kinesis-target-partition-key-path-routes-by-event-field`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/add_tags_to_stream.rs`. Replaying the corpus, the emulator refused the `AddTagsToStream` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddTagsToStream operation: operation AddTagsToStream is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call AddTagsToStream and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
