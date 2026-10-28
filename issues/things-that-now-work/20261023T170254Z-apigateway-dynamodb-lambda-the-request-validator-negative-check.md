Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked
Issue-Key: apigateway__dynamodb__lambda|CreateModel|NotImplemented
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__dynamodb__lambda: the request-validator negative check aborts because apigateway CreateModel answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModel

## Description
Cause 1 of 1: CreateModel is a scaffold stub, so a request validator cannot be given a body model and the 'bad body is rejected before the integration runs' behaviour is unreachable.
AWS: Smithy CreateModel (POST /restapis/{restApiId}/models, success 201) returns Model (id, name, description, schema, contentType) with `name` and `contentType` @required; modeled errors are BadRequestException (400), ConflictException (409), NotFoundException (404), LimitExceededException (429), TooManyRequestsException (429), UnauthorizedException (401). Real AWS creates the model, and a method that references it through a request validator then answers a schema-violating request with HTTP 400 without invoking the integration.
Emulator: the call answers HTTP 501 with error code NotImplemented ('operation CreateModel is scaffolded but not implemented in this emulator'), so the check aborts at setup.
Why: a client cannot attach body validation to a method at all. The guarantee this check exists to prove — that a schema-invalid request is refused by API Gateway and the backend Lambda is never invoked — cannot hold, so a backend may be invoked with input real AWS would have rejected.
Fix: the CreateModel handler and its schema/contentType validation, plus the request-validator path that consumes the stored model (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_model.rs). Existing issue 20261023T125454Z-apigateway-createmodel-stub.md names the stub but does not carry this check's replay key.
Traces: apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked.

## Evidence
Expected: HTTP 201 with the created Model, then HTTP 400 for a schema-violating request with no integration invocation.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateModel operation: operation CreateModel is scaffolded but not implemented in this emulator (report line 70907). HTTP 501, code NotImplemented — not the modeled 201 nor any modeled 4xx.

## Steps to Reproduce
1. Create a REST API and a resource/method with a Lambda proxy integration.
2. aws apigateway create-model --rest-api-id <id> --name Body --content-type application/json --schema '<json schema>'.
3. Expect HTTP 201; observe HTTP 501 NotImplemented.
Re-replay apigateway__dynamodb__lambda.apigw-request-validator-rejects-bad-body-integration-not-invoked.
