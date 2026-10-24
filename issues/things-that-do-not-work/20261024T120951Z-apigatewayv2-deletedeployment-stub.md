Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.delete-deployment-in-use-by-stage-rejected apigatewayv2.get-deployments-pagination-chains-every-deployment-once
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.delete-deployment-in-use-by-stage-rejected|run#12|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigatewayv2|DeleteDeployment|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement apigatewayv2 DeleteDeployment: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDeployment

## Description
`DeleteDeployment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_deployment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteDeployment` call, so each one unblocks only when `DeleteDeployment` works: `apigatewayv2.delete-deployment-in-use-by-stage-rejected`, `apigatewayv2.get-deployments-pagination-chains-every-deployment-once`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/delete_deployment.rs`. Replaying the corpus, the emulator refused the `DeleteDeployment` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_apigatewayv2_delete_deployment_in_use_by_stage_rejected.py", line 70, in run
    assert code == "BadRequestException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: deleting the deployment stage 'live' is bound to: expected BadRequestException, got NotImplemented: 'operation DeleteDeployment is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call DeleteDeployment and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
