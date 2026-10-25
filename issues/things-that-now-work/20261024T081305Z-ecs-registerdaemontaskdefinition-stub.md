Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.concurrent-daemon-update ecs.daemon-deployment-timeout-marks-failed ecs.daemon-revision-numbers-strictly-increase ecs.daemon-task-definition-delete-then-describe-inactive ecs.daemon-task-definition-register-describe-delete ecs.daemon-task-definition-register-describe-list ecs.daemon-task-definition-register-lifecycle ecs.invalid-parameter-daemon-taskdef-and-namespace ecs.list-daemon-task-definitions-filters ecs.list-daemon-task-definitions-reflects-registration
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RegisterDaemonTaskDefinition|NotImplemented
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Implement ecs RegisterDaemonTaskDefinition: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterDaemonTaskDefinition

## Description
`RegisterDaemonTaskDefinition` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_daemon_task_definition.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `RegisterDaemonTaskDefinition` call, so each one unblocks only when `RegisterDaemonTaskDefinition` works: `ecs.concurrent-daemon-update`, `ecs.daemon-deployment-timeout-marks-failed`, `ecs.daemon-revision-numbers-strictly-increase`, `ecs.daemon-task-definition-delete-then-describe-inactive`, `ecs.daemon-task-definition-register-describe-delete`, `ecs.daemon-task-definition-register-describe-list`, `ecs.daemon-task-definition-register-lifecycle`, `ecs.invalid-parameter-daemon-taskdef-and-namespace`, `ecs.list-daemon-task-definitions-filters`, `ecs.list-daemon-task-definitions-reflects-registration`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_daemon_task_definition.rs`. Replaying the corpus, the emulator refused the `RegisterDaemonTaskDefinition` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterDaemonTaskDefinition operation: operation RegisterDaemonTaskDefinition is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecs emulator
Call RegisterDaemonTaskDefinition and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
