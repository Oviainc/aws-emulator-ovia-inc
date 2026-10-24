Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.express-gateway-ops-on-missing-cluster-not-found ecs.express-gateway-service-delete-then-describe-404
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.express-gateway-ops-on-missing-cluster-not-found|run#15|AssertionError|assert error == cluster_missing, f"CreateExpressGatewayService: got {error!r}"
Issue-Key: ecs|CreateExpressGatewayService|NotImplemented
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Implement ecs CreateExpressGatewayService: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExpressGatewayService

## Description
`CreateExpressGatewayService` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_express_gateway_service.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateExpressGatewayService` call, so each one unblocks only when `CreateExpressGatewayService` works: `ecs.express-gateway-ops-on-missing-cluster-not-found`, `ecs.express-gateway-service-delete-then-describe-404`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_express_gateway_service.rs`. Replaying the corpus, the emulator refused the `CreateExpressGatewayService` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ecs_express_gateway_ops_on_missing_cluster_not_found.py", line 99, in run
    assert error == cluster_missing, f"CreateExpressGatewayService: got {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: CreateExpressGatewayService: got ('NotImplemented', 'operation CreateExpressGatewayService is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ecs emulator
Call CreateExpressGatewayService and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
