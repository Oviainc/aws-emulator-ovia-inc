Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.signal-resource-is-lenient
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|SignalResource|NotImplemented
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# Implement cloudformation SignalResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignalResource

## Description
`SignalResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/signal_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SignalResource` call, so each one unblocks only when `SignalResource` works: `cloudformation.signal-resource-is-lenient`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/signal_resource.rs`. Replaying the corpus, the emulator refused the `SignalResource` call: File "check_cloudformation_signal_resource_is_lenient.py", line 32, in run
    _must_succeed(
    ~~~~~~~~~~~~~^
        lambda: cfn.signal_resource(
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ...<5 lines>...
        "SignalResource on a non-signalable resource",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "check_cloudformation_signal_resource_is_lenient.py", line 80, in _must_succeed
    raise AssertionError(
        f"expected {what} to be accepted, got {err.get('Code')}: {err.get('Message')}"
    ) from exc
AssertionError: expected SignalResource on a non-signalable resource to be accepted, got NotImplemented: operation SignalResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudformation emulator
Call SignalResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
