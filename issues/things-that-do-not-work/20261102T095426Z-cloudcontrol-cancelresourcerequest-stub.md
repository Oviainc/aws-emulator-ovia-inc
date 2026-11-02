Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.cancel-in-flight-request-reaches-cancel-complete cloudcontrol.cloudcontrol-cancel-in-flight-create-leaves-no-native-resource cloudcontrol.concurrent-cancel-vs-natural-completion-race cloudcontrol.get-resource-request-status-cancelled-status-exact
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol|CancelResourceRequest|NotImplemented
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# Implement cloudcontrol CancelResourceRequest: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelResourceRequest

## Description
`CancelResourceRequest` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/cancel_resource_request.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CancelResourceRequest` call, so each one unblocks only when `CancelResourceRequest` works: `cloudcontrol.cancel-in-flight-request-reaches-cancel-complete`, `cloudcontrol.cloudcontrol-cancel-in-flight-create-leaves-no-native-resource`, `cloudcontrol.concurrent-cancel-vs-natural-completion-race`, `cloudcontrol.get-resource-request-status-cancelled-status-exact`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/cancel_resource_request.rs`. Replaying the corpus, the emulator refused the `CancelResourceRequest` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CancelResourceRequest operation: operation CancelResourceRequest is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudcontrol emulator
Call CancelResourceRequest and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
