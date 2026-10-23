Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame
Issue-Key: apigatewayv2|CreateRouteResponse|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement apigatewayv2 CreateRouteResponse: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRouteResponse

## Description
`CreateRouteResponse` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_route_response.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateRouteResponse` call, so each one unblocks only when `CreateRouteResponse` works: `apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route`, `apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes`, `apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-body-action-selects-custom-route-else-default`, `apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch`, `apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered`, `apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route`, `apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas`, `apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-route-selection-dispatches-each-action-to-its-own-lambda`, `apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-unmatched-action-without-default-route-rejected-with-error-frame`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_route_response.rs`. Replaying the corpus, the emulator refused the `CreateRouteResponse` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRouteResponse operation: operation CreateRouteResponse is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call CreateRouteResponse and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
