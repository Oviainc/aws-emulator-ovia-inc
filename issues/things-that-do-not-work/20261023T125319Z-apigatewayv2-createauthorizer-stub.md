Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.authorizer-lifecycle apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl apigatewayv2.delete-authorizer-in-use-by-a-route-rejected apigatewayv2.lambda-authorizer-array-context-value-passes-through-unchanged apigatewayv2.lambda-authorizer-context-propagates-to-integration-event apigatewayv2.lambda-authorizer-payload-format-1-0-iam-policy-response apigatewayv2.lambda-authorizer-simple-response-explicit-deny-forbidden apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value apigatewayv2.request-authorizer-missing-one-of-two-identity-sources-unauthorized apigatewayv2.reset-authorizers-cache-affects-invoke apigatewayv2.reset-authorizers-cache-server-side apigatewayv2.simple-response-authorizer-under-payload-format-1-0-fails-500 apigatewayv2.websocket-connect-request-authorizer-token-gate
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateAuthorizer|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigatewayv2 CreateAuthorizer: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
`CreateAuthorizer` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_authorizer.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `CreateAuthorizer` call, so each one unblocks only when `CreateAuthorizer` works: `apigatewayv2.authorizer-lifecycle`, `apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl`, `apigatewayv2.delete-authorizer-in-use-by-a-route-rejected`, `apigatewayv2.lambda-authorizer-array-context-value-passes-through-unchanged`, `apigatewayv2.lambda-authorizer-context-propagates-to-integration-event`, `apigatewayv2.lambda-authorizer-payload-format-1-0-iam-policy-response`, `apigatewayv2.lambda-authorizer-simple-response-explicit-deny-forbidden`, `apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value`, `apigatewayv2.request-authorizer-missing-one-of-two-identity-sources-unauthorized`, `apigatewayv2.reset-authorizers-cache-affects-invoke`, `apigatewayv2.reset-authorizers-cache-server-side`, `apigatewayv2.simple-response-authorizer-under-payload-format-1-0-fails-500`, `apigatewayv2.websocket-connect-request-authorizer-token-gate`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_authorizer.rs`. Replaying the corpus, the emulator refused the `CreateAuthorizer` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call CreateAuthorizer and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
