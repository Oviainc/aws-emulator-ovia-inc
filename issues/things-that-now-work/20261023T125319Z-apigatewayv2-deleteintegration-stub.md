Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.delete-integration-in-use-by-a-route-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.delete-integration-in-use-by-a-route-rejected|run#7|AssertionError|assert code == "ConflictException", (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigatewayv2 DeleteIntegration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIntegration

## Description
`DeleteIntegration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_integration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteIntegration` call, so each one unblocks only when `DeleteIntegration` works: `apigatewayv2.delete-integration-in-use-by-a-route-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_integration.rs`. Replaying the corpus, the emulator refused the `DeleteIntegration` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_apigatewayv2_delete_integration_in_use_by_a_route_rejected.py", line 46, in run
    assert code == "ConflictException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: DeleteIntegration while a route targets it: expected ConflictException, got NotImplemented: 'operation DeleteIntegration is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call DeleteIntegration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
