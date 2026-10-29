Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehubv2.create-system-same-client-token-is-idempotent resiliencehubv2.create-system-token-reuse-with-different-params-conflicts
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|CreateSystem|NotImplemented
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Implement resiliencehubv2 CreateSystem: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSystem

## Description
`CreateSystem` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/create_system.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateSystem` call, so each one unblocks only when `CreateSystem` works: `resiliencehubv2.create-system-same-client-token-is-idempotent`, `resiliencehubv2.create-system-token-reuse-with-different-params-conflicts`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/src/ops/create_system.rs`. Replaying the corpus, the emulator refused the `CreateSystem` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSystem operation: operation CreateSystem is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the resiliencehubv2 emulator
Call CreateSystem and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
