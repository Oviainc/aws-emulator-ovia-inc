Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses apigateway.create-model-duplicate-name-rejected apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare apigateway.delete-model-referenced-by-method-conflict apigateway.e2e-request-validator-model-enforced-on-invoke apigateway.get-model-template-and-models apigateway.get-model-template-generates-example-json apigateway.get-model-template-generates-example-payload apigateway.get-models-list-creation-order-preserved apigateway.get-models-listing-order-matches-creation apigateway.get-models-lists-created apigateway.get-models-ordering-reflects-creation apigateway.model-lifecycle apigateway.model-schema-numeric-precision-round-trip apigateway.model-schema-unicode-and-nested-json-round-trip apigateway.model-schema-unicode-round-trip apigateway.op-contract-createModel apigateway.op-contract-delete-model apigateway.op-contract-getModelTemplate apigateway.request-validator-rejects-body-and-params-at-stage apigateway.test-invoke-method-bad-request apigateway.update-method-and-method-response apigateway.update-method-response-model-and-validator apigateway.update-method-response-patch-model apigateway.update-model-and-validator-in-place apigateway.update-model-invalid-patch-path-rejected apigateway.update-model-invalid-schema-bad-request apigateway.update-model-patch-schema apigateway.update-model-schema
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: api-gateway|CreateModel|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement apigateway CreateModel: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModel

## Description
`CreateModel` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_model.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

29 recorded unit(s) fail on their `CreateModel` call, so each one unblocks only when `CreateModel` works: `apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses`, `apigateway.create-model-duplicate-name-rejected`, `apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare`, `apigateway.delete-model-referenced-by-method-conflict`, `apigateway.e2e-request-validator-model-enforced-on-invoke`, `apigateway.get-model-template-and-models`, `apigateway.get-model-template-generates-example-json`, `apigateway.get-model-template-generates-example-payload`, `apigateway.get-models-list-creation-order-preserved`, `apigateway.get-models-listing-order-matches-creation`, `apigateway.get-models-lists-created`, `apigateway.get-models-ordering-reflects-creation`, `apigateway.model-lifecycle`, `apigateway.model-schema-numeric-precision-round-trip`, `apigateway.model-schema-unicode-and-nested-json-round-trip`, `apigateway.model-schema-unicode-round-trip`, `apigateway.op-contract-createModel`, `apigateway.op-contract-delete-model`, `apigateway.op-contract-getModelTemplate`, `apigateway.request-validator-rejects-body-and-params-at-stage` (+9 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_model.rs`. Replaying the corpus, the emulator refused the `CreateModel` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateModel operation: operation CreateModel is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the apigateway emulator
Call CreateModel and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
