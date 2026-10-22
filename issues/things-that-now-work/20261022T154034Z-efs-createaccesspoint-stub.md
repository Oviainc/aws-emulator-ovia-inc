Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs.create-access-point-lifecycle-transition efs.create-access-point-nonexistent-file-system-not-found efs.create-access-point-token-reuse-conflict efs.create-access-point-token-reuse-mismatched-params-rejected efs.delete-access-point-concurrent-delete-idempotent efs.delete-access-point-lifecycle-transition efs.tag-resource-and-list-tags-round-trip
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: efs|CreateAccessPoint|NotImplemented
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement efs CreateAccessPoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
`CreateAccessPoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_access_point.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateAccessPoint` call, so each one unblocks only when `CreateAccessPoint` works: `efs.create-access-point-lifecycle-transition`, `efs.create-access-point-nonexistent-file-system-not-found`, `efs.create-access-point-token-reuse-conflict`, `efs.create-access-point-token-reuse-mismatched-params-rejected`, `efs.delete-access-point-concurrent-delete-idempotent`, `efs.delete-access-point-lifecycle-transition`, `efs.tag-resource-and-list-tags-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_access_point.rs`. Replaying the corpus, the emulator refused the `CreateAccessPoint` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAccessPoint operation: operation CreateAccessPoint is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the efs emulator
Call CreateAccessPoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
