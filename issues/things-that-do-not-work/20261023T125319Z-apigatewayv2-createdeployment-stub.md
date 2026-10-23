Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.client-initiated-close-invokes-disconnect-route apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-selects-lambda-backend apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected apigatewayv2.delete-deployment-in-use-by-stage-rejected apigatewayv2.get-deployments-pagination-chains-every-deployment-once apigatewayv2.stage-variables-and-route-settings-round-trip apigatewayv2.websocket-inbound-binary-frame-rejected apigatewayv2.websocket-inbound-message-32kb-boundary
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateDeployment|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigatewayv2 CreateDeployment: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
`CreateDeployment` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_deployment.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `CreateDeployment` call, so each one unblocks only when `CreateDeployment` works: `apigatewayv2.client-initiated-close-invokes-disconnect-route`, `apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-selects-lambda-backend`, `apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected`, `apigatewayv2.delete-deployment-in-use-by-stage-rejected`, `apigatewayv2.get-deployments-pagination-chains-every-deployment-once`, `apigatewayv2.stage-variables-and-route-settings-round-trip`, `apigatewayv2.websocket-inbound-binary-frame-rejected`, `apigatewayv2.websocket-inbound-message-32kb-boundary`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_deployment.rs`. Replaying the corpus, the emulator refused the `CreateDeployment` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeployment operation: operation CreateDeployment is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call CreateDeployment and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
