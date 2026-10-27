Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.continue-service-deployment-rejects-non-waiting-state
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|ContinueServiceDeployment|NotImplemented
Issue-Commit: 02e2c56a8a1bc6426078af793fc5f7b555907bec

# Implement ecs ContinueServiceDeployment: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `02e2c56a8a1bc6426078af793fc5f7b555907bec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueServiceDeployment

## Description
`ContinueServiceDeployment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/continue_service_deployment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ContinueServiceDeployment` call, so each one unblocks only when `ContinueServiceDeployment` works: `ecs.continue-service-deployment-rejects-non-waiting-state`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/continue_service_deployment.rs`. Replaying the corpus, the emulator refused the `ContinueServiceDeployment` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ecs_continue_service_deployment_rejects_non_waiting_state.py", line 52, in run
    assert _code(e) == "InvalidParameterException", f"expected InvalidParameterException, got {_code(e)}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterException, got NotImplemented

## Steps to Reproduce
Boot the ecs emulator
Call ContinueServiceDeployment and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
