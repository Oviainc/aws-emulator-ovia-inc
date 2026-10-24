Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.delete-portal-product-access-denied-without-permission apigatewayv2.e2e-portal-product-publish-flow apigatewayv2.e2e-portal-publish-consume-flow apigatewayv2.portal-product-description-unicode-round-trip apigatewayv2.portal-product-lifecycle apigatewayv2.portal-product-lifecycle-crud apigatewayv2.portal-product-sharing-policy-lifecycle apigatewayv2.portal-product-sharing-policy-resets-to-empty-after-delete apigatewayv2.product-page-lifecycle apigatewayv2.product-page-lifecycle-crud apigatewayv2.put-portal-product-sharing-policy-access-denied
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreatePortalProduct|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigatewayv2 CreatePortalProduct: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePortalProduct

## Description
`CreatePortalProduct` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_portal_product.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `CreatePortalProduct` call, so each one unblocks only when `CreatePortalProduct` works: `apigatewayv2.delete-portal-product-access-denied-without-permission`, `apigatewayv2.e2e-portal-product-publish-flow`, `apigatewayv2.e2e-portal-publish-consume-flow`, `apigatewayv2.portal-product-description-unicode-round-trip`, `apigatewayv2.portal-product-lifecycle`, `apigatewayv2.portal-product-lifecycle-crud`, `apigatewayv2.portal-product-sharing-policy-lifecycle`, `apigatewayv2.portal-product-sharing-policy-resets-to-empty-after-delete`, `apigatewayv2.product-page-lifecycle`, `apigatewayv2.product-page-lifecycle-crud`, `apigatewayv2.put-portal-product-sharing-policy-access-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_portal_product.rs`. Replaying the corpus, the emulator refused the `CreatePortalProduct` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePortalProduct operation: operation CreatePortalProduct is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call CreatePortalProduct and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
