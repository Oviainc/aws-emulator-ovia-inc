Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.api-cache-ttl-boundary-enforcement appsync.flush-api-cache-clears-state appsync.flush-api-cache-concurrent-double-flush-single-winner appsync.flush-api-cache-concurrent-modification-accepted appsync.flush-api-cache-immediate-clear appsync.flush-api-cache-preserves-resource-state
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateApiCache|NotImplemented
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Implement appsync CreateApiCache: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiCache

## Description
`CreateApiCache` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_api_cache.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateApiCache` call, so each one unblocks only when `CreateApiCache` works: `appsync.api-cache-ttl-boundary-enforcement`, `appsync.flush-api-cache-clears-state`, `appsync.flush-api-cache-concurrent-double-flush-single-winner`, `appsync.flush-api-cache-concurrent-modification-accepted`, `appsync.flush-api-cache-immediate-clear`, `appsync.flush-api-cache-preserves-resource-state`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/create_api_cache.rs`. Replaying the corpus, the emulator refused the `CreateApiCache` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateApiCache operation: operation CreateApiCache is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appsync emulator
Call CreateApiCache and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
