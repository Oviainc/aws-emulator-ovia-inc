Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs.obs-ecs-1-stop-finished-alarm-monitored-deployment-rejected ecs.stop-service-deployment-already-stopped-is-idempotent
Issue-Key: cloudwatch__ecs.obs-ecs-1-stop-finished-alarm-monitored-deployment-rejected|run#24|AssertionError|assert got == want, (
Issue-Key: ecs|StopServiceDeployment|NotImplemented
Issue-Commit: 02e2c56a8a1bc6426078af793fc5f7b555907bec

# Implement ecs StopServiceDeployment: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `02e2c56a8a1bc6426078af793fc5f7b555907bec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopServiceDeployment

## Description
`StopServiceDeployment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/stop_service_deployment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `StopServiceDeployment` call, so each one unblocks only when `StopServiceDeployment` works: `cloudwatch__ecs.obs-ecs-1-stop-finished-alarm-monitored-deployment-rejected`, `ecs.stop-service-deployment-already-stopped-is-idempotent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/stop_service_deployment.rs`. Replaying the corpus, the emulator refused the `StopServiceDeployment` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudwatch__ecs_obs_ecs_1_stop_finished_alarm_monitored_deployment_rejected.py", line 81, in run
    assert got == want, (
           ^^^^^^^^^^^
AssertionError: StopServiceDeployment(ROLLBACK) on a SUCCESSFUL deployment returned ('NotImplemented', 'operation StopServiceDeployment is scaffolded but not implemented in this emulator'); expected ('ConflictException', 'ServiceDeployment is stopping or has already finished for arn: arn:aws:ecs:us-west-2:000000000000:service-deployment/sim2real-xsvc-depstop-8a2d3708b8e0/sim2real-xsvc-depstop-svc-76fb52b9c388/0000000000000000001')

## Steps to Reproduce
Boot the ecs emulator
Call StopServiceDeployment and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
