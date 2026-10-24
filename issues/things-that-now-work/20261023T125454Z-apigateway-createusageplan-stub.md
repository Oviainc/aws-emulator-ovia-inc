Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration apigateway.concurrent-create-usage-plan-key-invalid-keytype-rejected apigateway.concurrent-create-usage-plan-key-nonexistent-key-rejected apigateway.concurrent-create-usage-plan-key-same-key-dedup apigateway.concurrent-delete-usage-plan-key-double-delete-rejected apigateway.concurrent-update-usage-plan-throttle-race apigateway.concurrent-usage-plan-key-association-race apigateway.concurrent-usage-plan-key-duplicate-conflict apigateway.concurrent-usage-plan-key-invalid-keytype-rejected apigateway.create-usage-plan-key-duplicate-fails apigateway.create-usage-plan-key-invalid-type-rejected apigateway.create-usage-plan-key-nonexistent-key-not-found apigateway.create-usage-plan-key-wrong-type-fails apigateway.delete-usage-plan-with-active-key-cascades apigateway.delete-usage-plan-with-keys-cascades apigateway.get-usage-not-found apigateway.get-usage-plan-key-not-found-vs-badrequest apigateway.get-usage-plan-keys-lists-associated apigateway.get-usage-plan-keys-pagination-order apigateway.get-usage-plans-lists-created apigateway.import-api-keys-bulk-then-list apigateway.op-contract-createUsagePlan apigateway.op-contract-delete-stage apigateway.op-contract-delete-usage-plan-key apigateway.op-contract-deleteUsagePlanKey apigateway.op-contract-getUsagePlan apigateway.op-contract-getUsagePlanKey apigateway.update-usage-enforced-server-side-throttling apigateway.update-usage-invalid-range-bad-request apigateway.update-usage-plan-and-list-plans apigateway.update-usage-plan-and-usage-plan-list apigateway.update-usage-plan-invalid-quota-period-rejected apigateway.update-usage-plan-quota-reflects-change apigateway.update-usage-throttle-reflects-change apigateway.usage-plan-invalid-quota-offset-rejected apigateway.usage-plan-key-association-lifecycle apigateway.usage-plan-lifecycle apigateway.usage-plan-negative-quota-limit-rejected apigateway.usage-plan-quota-large-integer-precision-preserved apigateway.usage-plan-quota-large-numeric-precision-preserved apigateway.usage-plan-quota-numeric-precision-preserved apigateway.usage-plan-quota-period-and-get-usage apigateway.usage-plan-quota-period-reset-window apigateway.usage-plan-quota-unknown-key-rejected apigateway.usage-plan-update-and-listing
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.usage-plan-negative-quota-limit-rejected|run#5|AssertionError|assert (err.get("Code"), err.get("Message")) == expected, f"expected {expected!r}, got {err!r}"
Issue-Related: 20261023T114649Z-api-gateway-createusageplan-stub.md
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigateway CreateUsagePlan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUsagePlan

## Description
`CreateUsagePlan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_usage_plan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

45 recorded unit(s) fail on their `CreateUsagePlan` call, so each one unblocks only when `CreateUsagePlan` works: `apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration`, `apigateway.concurrent-create-usage-plan-key-invalid-keytype-rejected`, `apigateway.concurrent-create-usage-plan-key-nonexistent-key-rejected`, `apigateway.concurrent-create-usage-plan-key-same-key-dedup`, `apigateway.concurrent-delete-usage-plan-key-double-delete-rejected`, `apigateway.concurrent-update-usage-plan-throttle-race`, `apigateway.concurrent-usage-plan-key-association-race`, `apigateway.concurrent-usage-plan-key-duplicate-conflict`, `apigateway.concurrent-usage-plan-key-invalid-keytype-rejected`, `apigateway.create-usage-plan-key-duplicate-fails`, `apigateway.create-usage-plan-key-invalid-type-rejected`, `apigateway.create-usage-plan-key-nonexistent-key-not-found`, `apigateway.create-usage-plan-key-wrong-type-fails`, `apigateway.delete-usage-plan-with-active-key-cascades`, `apigateway.delete-usage-plan-with-keys-cascades`, `apigateway.get-usage-not-found`, `apigateway.get-usage-plan-key-not-found-vs-badrequest`, `apigateway.get-usage-plan-keys-lists-associated`, `apigateway.get-usage-plan-keys-pagination-order`, `apigateway.get-usage-plans-lists-created` (+25 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_usage_plan.rs`. Replaying the corpus, the emulator refused the `CreateUsagePlan` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateUsagePlan operation: operation CreateUsagePlan is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call CreateUsagePlan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
