Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codestar-notifications.list-targets-filter-by-target-address
Issue-Shape-Id: com.amazonaws.codestarnotifications#CodeStarNotifications_20191015
Issue-Key: codestar-notifications|ListTargets|NotImplemented
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# Implement codestar-notifications ListTargets: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTargets

## Description
`ListTargets` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/ops/list_targets.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListTargets` call, so each one unblocks only when `ListTargets` works: `codestar-notifications.list-targets-filter-by-target-address`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codestarnotifications-codestarnotifications-20191015/src/ops/list_targets.rs`. Replaying the corpus, the emulator refused the `ListTargets` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTargets operation: operation ListTargets is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codestar-notifications emulator
Call ListTargets and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
