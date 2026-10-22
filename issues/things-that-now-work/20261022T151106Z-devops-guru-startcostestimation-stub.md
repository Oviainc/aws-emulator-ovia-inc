Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-guru.cost-estimation-lifecycle-completes
Issue-Shape-Id: com.amazonaws.devopsguru#CapstoneControlPlaneService
Issue-Key: devops-guru.cost-estimation-lifecycle-completes|run#4|AssertionError|assert error.get("Code") == "ResourceNotFoundException", error
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# Implement devops-guru StartCostEstimation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCostEstimation

## Description
`StartCostEstimation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/start_cost_estimation.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StartCostEstimation` call, so each one unblocks only when `StartCostEstimation` works: `devops-guru.cost-estimation-lifecycle-completes`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/start_cost_estimation.rs`. Replaying the corpus, the emulator refused the `StartCostEstimation` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_devops_guru_cost_estimation_lifecycle_completes.py", line 33, in run
    assert error.get("Code") == "ResourceNotFoundException", error
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: {'Message': 'operation StartCostEstimation is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the devops-guru emulator
Call StartCostEstimation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
