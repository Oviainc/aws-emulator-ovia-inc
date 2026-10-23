Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: oam.create-link-duplicate-already-linked-conflict-rejected oam.create-link-invalid-resource-type-effect-absent oam.create-link-invalid-sink-identifier-effect-absent oam.create-link-same-account-sink-rejected-regardless-of-link-configuration oam.put-sink-policy-round-trip
Issue-Shape-Id: com.amazonaws.oam#oamservice
Issue-Key: oam|PutSinkPolicy|NotImplemented
Issue-Commit: 6288a0cc473439e64f80da5d6e6b2c53557408ab

# Implement oam PutSinkPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6288a0cc473439e64f80da5d6e6b2c53557408ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSinkPolicy

## Description
`PutSinkPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-oam-oamservice/src/ops/put_sink_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `PutSinkPolicy` call, so each one unblocks only when `PutSinkPolicy` works: `oam.create-link-duplicate-already-linked-conflict-rejected`, `oam.create-link-invalid-resource-type-effect-absent`, `oam.create-link-invalid-sink-identifier-effect-absent`, `oam.create-link-same-account-sink-rejected-regardless-of-link-configuration`, `oam.put-sink-policy-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-oam-oamservice/src/ops/put_sink_policy.rs`. Replaying the corpus, the emulator refused the `PutSinkPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutSinkPolicy operation: operation PutSinkPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the oam emulator
Call PutSinkPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
