Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pinpoint.push-template-active-version-tracks-separately-from-latest pinpoint.push-template-stale-version-update-rejected
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint|CreatePushTemplate|NotImplemented
Issue-Commit: ef76ab1e831abf4546de7a6143d684d5cb1e8445

# Implement pinpoint CreatePushTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ef76ab1e831abf4546de7a6143d684d5cb1e8445`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePushTemplate

## Description
`CreatePushTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-pinpoint-pinpoint/src/ops/create_push_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreatePushTemplate` call, so each one unblocks only when `CreatePushTemplate` works: `pinpoint.push-template-active-version-tracks-separately-from-latest`, `pinpoint.push-template-stale-version-update-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-pinpoint-pinpoint/src/ops/create_push_template.rs`. Replaying the corpus, the emulator refused the `CreatePushTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePushTemplate operation: operation CreatePushTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the pinpoint emulator
Call CreatePushTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
