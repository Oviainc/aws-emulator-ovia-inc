Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs.obs-ecs-1-stop-finished-alarm-monitored-deployment-rejected ecs.continue-service-deployment-rejects-non-waiting-state ecs.service-deployment-history-ordered-newest-first ecs.stop-service-deployment-already-stopped-is-idempotent
Issue-Key: ecs|ListServiceDeployments|NotImplemented
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Implement ecs ListServiceDeployments: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServiceDeployments

## Description
`ListServiceDeployments` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_service_deployments.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `ListServiceDeployments` call, so each one unblocks only when `ListServiceDeployments` works: `cloudwatch__ecs.obs-ecs-1-stop-finished-alarm-monitored-deployment-rejected`, `ecs.continue-service-deployment-rejects-non-waiting-state`, `ecs.service-deployment-history-ordered-newest-first`, `ecs.stop-service-deployment-already-stopped-is-idempotent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/list_service_deployments.rs`. Replaying the corpus, the emulator refused the `ListServiceDeployments` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListServiceDeployments operation: operation ListServiceDeployments is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecs emulator
Call ListServiceDeployments and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
