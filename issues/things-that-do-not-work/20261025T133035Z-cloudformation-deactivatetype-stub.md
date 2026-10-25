Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-not-found-across-apis-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-not-found-across-apis-rejected|run#11|AssertionError|assert error == expected_not_found, f"{label} returned {error!r}, expected {expected_not_found!r}"
Issue-Commit: eaacfa963fb5bc737a9bad8d07574a87d1e0bb31

# Implement cloudformation DeactivateType: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eaacfa963fb5bc737a9bad8d07574a87d1e0bb31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeactivateType

## Description
`DeactivateType` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/deactivate_type.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeactivateType` call, so each one unblocks only when `DeactivateType` works: `cloudformation.type-not-found-across-apis-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/deactivate_type.rs`. Replaying the corpus, the emulator refused the `DeactivateType` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudformation_type_not_found_across_apis_rejected.py", line 90, in run
    assert error == expected_not_found, f"{label} returned {error!r}, expected {expected_not_found!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: DeactivateType returned ('NotImplemented', 'operation DeactivateType is scaffolded but not implemented in this emulator'), expected ('TypeNotFoundException', "The type 'SimParity::Fidelity::NoSuch4a35e0c97a01' cannot be found.")

## Steps to Reproduce
Boot the cloudformation emulator
Call DeactivateType and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
