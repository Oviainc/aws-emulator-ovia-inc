Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.disable-portal-revokes-access-server-side apigatewayv2.disabled-portal-preview-accepted apigatewayv2.disabled-portal-publish-accepted apigatewayv2.portal-concurrent-publish-preview-conflict apigatewayv2.portal-publish-server-managed-build apigatewayv2.publish-portal-server-managed-status-transition
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|PublishPortal|NotImplemented
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# Implement apigatewayv2 PublishPortal: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5d61882ef55489b75c810fe19d4c281a0249999c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPortal

## Description
`PublishPortal` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/publish_portal.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `PublishPortal` call, so each one unblocks only when `PublishPortal` works: `apigatewayv2.disable-portal-revokes-access-server-side`, `apigatewayv2.disabled-portal-preview-accepted`, `apigatewayv2.disabled-portal-publish-accepted`, `apigatewayv2.portal-concurrent-publish-preview-conflict`, `apigatewayv2.portal-publish-server-managed-build`, `apigatewayv2.publish-portal-server-managed-status-transition`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/publish_portal.rs`. Replaying the corpus, the emulator refused the `PublishPortal` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PublishPortal operation: operation PublishPortal is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigatewayv2 emulator
Call PublishPortal and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
