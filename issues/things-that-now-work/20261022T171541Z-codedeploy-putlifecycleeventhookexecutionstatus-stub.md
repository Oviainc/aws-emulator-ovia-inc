Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.put-lifecycle-event-hook-invalid-deployment-id-rejected codedeploy.put-lifecycle-event-hook-invalid-deployment-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.put-lifecycle-event-hook-invalid-deployment-id-rejected|run#5|AssertionError|assert error == malformed_error, (
Issue-Key: codedeploy.put-lifecycle-event-hook-invalid-deployment-rejected|run#9|AssertionError|assert error == (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codedeploy PutLifecycleEventHookExecutionStatus: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleEventHookExecutionStatus

## Description
`PutLifecycleEventHookExecutionStatus` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/put_lifecycle_event_hook_execution_status.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `PutLifecycleEventHookExecutionStatus` call, so each one unblocks only when `PutLifecycleEventHookExecutionStatus` works: `codedeploy.put-lifecycle-event-hook-invalid-deployment-id-rejected`, `codedeploy.put-lifecycle-event-hook-invalid-deployment-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/put_lifecycle_event_hook_execution_status.rs`. Replaying the corpus, the emulator refused the `PutLifecycleEventHookExecutionStatus` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codedeploy_put_lifecycle_event_hook_invalid_deployment_id_rejected.py", line 34, in run
    assert error == malformed_error, (
           ^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: malformed deploymentId 'not a valid deployment id !!' should raise ('InvalidDeploymentIdException', 'Specified DeploymentId is not in the valid format: not a valid deployment id !!'), got ('NotImplemented', 'operation PutLifecycleEventHookExecutionStatus is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the codedeploy emulator
Call PutLifecycleEventHookExecutionStatus and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
