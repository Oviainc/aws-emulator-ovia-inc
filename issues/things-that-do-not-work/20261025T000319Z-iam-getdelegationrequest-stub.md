Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-getDelegationRequest
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-getDelegationRequest|run#6|AssertionError|assert err["Code"] == "NoSuchEntity", f"expected NoSuchEntity, got {err!r}"
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Implement iam GetDelegationRequest: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDelegationRequest

## Description
`GetDelegationRequest` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_delegation_request.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetDelegationRequest` call, so each one unblocks only when `GetDelegationRequest` works: `iam.op-contract-getDelegationRequest`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_delegation_request.rs`. Replaying the corpus, the emulator refused the `GetDelegationRequest` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_iam_op_contract_getDelegationRequest.py", line 32, in run
    assert err["Code"] == "NoSuchEntity", f"expected NoSuchEntity, got {err!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected NoSuchEntity, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation GetDelegationRequest is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the iam emulator
Call GetDelegationRequest and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
