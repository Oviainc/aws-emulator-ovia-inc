Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative apigateway__dynamodb__lambda__logs.apigw-dynamodb-batch-write-integration-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-getitem-integration-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-missing-execution-role-permission-negative apigateway__dynamodb__lambda__logs.apigw-dynamodb-putitem-integration-positive apigateway__dynamodb__lambda__logs.apigw-dynamodb-query-integration-response-mapping-positive apigateway__dynamodb__lambda__logs.iam-role-trusts-apigateway-service-positive apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative apigateway__dynamodb__lambda__logs.lambda-apigateway-create-deployment-positive apigateway__dynamodb__lambda__logs.lambda-apigateway-cross-account-negative apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-rest-api-id-negative apigateway__dynamodb__lambda__logs.lambda-get-deployment-details-positive apigateway__dynamodb__lambda__logs.lambda-update-apigateway-stage-variables-positive apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected
Issue-Key: api-gateway|PutIntegrationResponse|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement api-gateway PutIntegrationResponse: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegrationResponse

## Description
`PutIntegrationResponse` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/put_integration_response.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `PutIntegrationResponse` call, so each one unblocks only when `PutIntegrationResponse` works: `apigateway__dynamodb__lambda__logs.apigateway-access-log-wrong-destination-arn-negative`, `apigateway__dynamodb__lambda__logs.apigateway-log-group-resource-policy-missing-negative`, `apigateway__dynamodb__lambda__logs.apigateway-missing-cloudwatch-role-negative`, `apigateway__dynamodb__lambda__logs.apigw-dynamodb-batch-write-integration-positive`, `apigateway__dynamodb__lambda__logs.apigw-dynamodb-getitem-integration-positive`, `apigateway__dynamodb__lambda__logs.apigw-dynamodb-missing-execution-role-permission-negative`, `apigateway__dynamodb__lambda__logs.apigw-dynamodb-putitem-integration-positive`, `apigateway__dynamodb__lambda__logs.apigw-dynamodb-query-integration-response-mapping-positive`, `apigateway__dynamodb__lambda__logs.iam-role-trusts-apigateway-service-positive`, `apigateway__dynamodb__lambda__logs.iam-role-wrong-trust-principal-apigateway-negative`, `apigateway__dynamodb__lambda__logs.lambda-apigateway-create-deployment-positive`, `apigateway__dynamodb__lambda__logs.lambda-apigateway-cross-account-negative`, `apigateway__dynamodb__lambda__logs.lambda-apigateway-wrong-rest-api-id-negative`, `apigateway__dynamodb__lambda__logs.lambda-get-deployment-details-positive`, `apigateway__dynamodb__lambda__logs.lambda-update-apigateway-stage-variables-positive`, `apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs`, `apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields`, `apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/put_integration_response.rs`. Replaying the corpus, the emulator refused the `PutIntegrationResponse` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutIntegrationResponse operation: operation PutIntegrationResponse is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the api-gateway emulator
Call PutIntegrationResponse and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
