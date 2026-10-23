Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.authorizer-cache-ttl-window apigateway.authorizer-cache-ttl-zero-disables-caching apigateway.authorizer-deletion-blocked-while-referenced-rejected apigateway.authorizer-lifecycle apigateway.authorizer-protected-method-invoke-flow apigateway.authorizer-ttl-caches-result-within-window apigateway.authorizer-ttl-default-and-update-reflected apigateway.authorizer-ttl-out-of-range-rejected apigateway.authorizer-ttl-zero-disables-caching apigateway.concurrent-create-authorizer-same-config-no-dedup apigateway.delete-authorizer-attached-to-method-conflict apigateway.delete-authorizer-in-use-rejected apigateway.e2e-authorizer-cache-invalidation-flow apigateway.e2e-authorizer-protected-method-consequence apigateway.e2e-custom-authorizer-protected-method-flow apigateway.flush-stage-authorizers-cache-bypasses-ttl apigateway.flush-stage-authorizers-cache-clears-authorizer-cache apigateway.flush-stage-authorizers-cache-forces-reinvoke apigateway.flush-stage-authorizers-cache-invalidates apigateway.flush-stage-authorizers-cache-rejects-missing-stage apigateway.op-contract-delete-authorizer apigateway.op-contract-getAuthorizer apigateway.op-contract-getAuthorizers apigateway.op-contract-updateAuthorizer apigateway.test-invoke-authorizer-enforces-identity-source apigateway.test-invoke-authorizer-executes-configured-authorizer apigateway.test-invoke-authorizer-executes-lambda-policy apigateway.test-invoke-authorizer-missing-identity-source-fails apigateway.test-invoke-authorizer-missing-identity-source-rejected apigateway.test-invoke-authorizer-not-found apigateway.test-invoke-authorizer-returns-policy-and-claims apigateway.update-authorizer-invalid-ttl-error-contract apigateway.update-method-patch-authorization-type
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|CreateAuthorizer|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigateway CreateAuthorizer: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
`CreateAuthorizer` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_authorizer.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

33 recorded unit(s) fail on their `CreateAuthorizer` call, so each one unblocks only when `CreateAuthorizer` works: `apigateway.authorizer-cache-ttl-window`, `apigateway.authorizer-cache-ttl-zero-disables-caching`, `apigateway.authorizer-deletion-blocked-while-referenced-rejected`, `apigateway.authorizer-lifecycle`, `apigateway.authorizer-protected-method-invoke-flow`, `apigateway.authorizer-ttl-caches-result-within-window`, `apigateway.authorizer-ttl-default-and-update-reflected`, `apigateway.authorizer-ttl-out-of-range-rejected`, `apigateway.authorizer-ttl-zero-disables-caching`, `apigateway.concurrent-create-authorizer-same-config-no-dedup`, `apigateway.delete-authorizer-attached-to-method-conflict`, `apigateway.delete-authorizer-in-use-rejected`, `apigateway.e2e-authorizer-cache-invalidation-flow`, `apigateway.e2e-authorizer-protected-method-consequence`, `apigateway.e2e-custom-authorizer-protected-method-flow`, `apigateway.flush-stage-authorizers-cache-bypasses-ttl`, `apigateway.flush-stage-authorizers-cache-clears-authorizer-cache`, `apigateway.flush-stage-authorizers-cache-forces-reinvoke`, `apigateway.flush-stage-authorizers-cache-invalidates`, `apigateway.flush-stage-authorizers-cache-rejects-missing-stage` (+13 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_authorizer.rs`. Replaying the corpus, the emulator refused the `CreateAuthorizer` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call CreateAuthorizer and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
