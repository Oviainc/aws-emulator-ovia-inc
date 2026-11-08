Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.daemon-deployment-new-revision-ordering ecs.daemon-deployments-lifecycle-progresses ecs.daemon-managed-rollout
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|ListDaemonDeployments|NotImplemented
Issue-Commit: 85304fbaaec1e137da409d02b027804fcd09632b

# Implement ecs ListDaemonDeployments: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `85304fbaaec1e137da409d02b027804fcd09632b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDaemonDeployments

## Description
`ListDaemonDeployments` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_daemon_deployments.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListDaemonDeployments` call, so each one unblocks only when `ListDaemonDeployments` works: `ecs.daemon-deployment-new-revision-ordering`, `ecs.daemon-deployments-lifecycle-progresses`, `ecs.daemon-managed-rollout`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_daemon_deployments.rs`. Replaying the corpus, the emulator refused the `ListDaemonDeployments` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ecs_daemon_deployment_new_revision_ordering.py", line 283, in run
    raise AssertionError(
AssertionError: ECS Managed Daemon deployment ordering not exercisable in this account/region: An error occurred (NotImplemented) when calling the ListDaemonDeployments operation: operation ListDaemonDeployments is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecs emulator
Call ListDaemonDeployments and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
