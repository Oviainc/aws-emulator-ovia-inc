Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.cancel-trace-retrieval-already-completed-fails xray.cancel-trace-retrieval-unknown-token-fails xray.concurrent-start-trace-retrieval-same-window-dedupes xray.e2e-trace-retrieval-full-flow xray.get-retrieved-traces-graph-unknown-token-fails xray.list-retrieved-traces-unknown-token-fails xray.put-resource-policy-lockout-prevented xray.resource-policy-lockout-prevention-rejected xray.start-trace-retrieval-consistent-token-data-plane xray.start-trace-retrieval-results-not-immediately-available xray.start-trace-retrieval-unknown-destination-not-found xray.start-trace-retrieval-unknown-trace-ids-rejected xray.state-machine-trace-retrieval-token-lifecycle xray.trace-retrieval-lifecycle-status-transitions xray.visibility-trace-retrieval-processing-window
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|PutResourcePolicy|NotImplemented
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Implement xray PutResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
`PutResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-xray-awsxray/src/ops/put_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `PutResourcePolicy` call, so each one unblocks only when `PutResourcePolicy` works: `xray.cancel-trace-retrieval-already-completed-fails`, `xray.cancel-trace-retrieval-unknown-token-fails`, `xray.concurrent-start-trace-retrieval-same-window-dedupes`, `xray.e2e-trace-retrieval-full-flow`, `xray.get-retrieved-traces-graph-unknown-token-fails`, `xray.list-retrieved-traces-unknown-token-fails`, `xray.put-resource-policy-lockout-prevented`, `xray.resource-policy-lockout-prevention-rejected`, `xray.start-trace-retrieval-consistent-token-data-plane`, `xray.start-trace-retrieval-results-not-immediately-available`, `xray.start-trace-retrieval-unknown-destination-not-found`, `xray.start-trace-retrieval-unknown-trace-ids-rejected`, `xray.state-machine-trace-retrieval-token-lifecycle`, `xray.trace-retrieval-lifecycle-status-transitions`, `xray.visibility-trace-retrieval-processing-window`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-xray-awsxray/src/ops/put_resource_policy.rs`. Replaying the corpus, the emulator refused the `PutResourcePolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutResourcePolicy operation: operation PutResourcePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the xray emulator
Call PutResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
