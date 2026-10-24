Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording
Issue-Key: apigateway__dynamodb__lambda__s3__xray|UpdateStage|NotImplemented
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__dynamodb__lambda__s3__xray: both X-Ray trace-shape checks abort because apigateway UpdateStage answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: UpdateStage is a scaffold stub, so tracing cannot be enabled on a stage and no X-Ray behaviour of a deployed REST API is reachable.
AWS: Smithy UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}, success 200) returns the Stage document — which carries `tracingEnabled` (Boolean, default false) — and models BadRequestException (400), ConflictException (409), NotFoundException (404), LimitExceededException (429), TooManyRequestsException (429), UnauthorizedException (401). Real AWS applies a patchOperation on /tracingEnabled and returns the updated Stage with tracingEnabled true.
Emulator: the call answers HTTP 501 with error code NotImplemented ('operation UpdateStage is scaffolded but not implemented in this emulator'), so both checks abort at setup.
Why: a client cannot turn on active tracing (or change any other stage setting) — it gets an unmodeled 501 instead of a 200 Stage or a modeled 4xx. Consequently the trace-tree shape across API Gateway, Lambda, DynamoDB and S3, and the X-Amzn-Trace-Id continuation/Sampled=0 behaviour, are both unverifiable.
Fix: the UpdateStage handler and its patchOperations application over stage state (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_stage.rs). Existing issue 20261023T125454Z-apigateway-updatestage-stub.md names the stub but does not carry these replay keys.
Traces: apigateway__dynamodb__lambda__s3__xray.apigw-lambda-ddb-s3-trace-tree-shape, apigateway__dynamodb__lambda__s3__xray.apigw-trace-header-continuation-and-sampled-zero-rejects-recording.

## Evidence
Expected: HTTP 200 with the updated Stage (tracingEnabled true), or one of the modeled 400/401/404/409/429 errors.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStage operation: operation UpdateStage is scaffolded but not implemented in this emulator (report lines 95307 and 96018). HTTP 501, code NotImplemented.

## Steps to Reproduce
1. Create a REST API, a method with a Lambda integration, and deploy it to a stage.
2. aws apigateway update-stage --rest-api-id <id> --stage-name <stage> --patch-operations op=replace,path=/tracingEnabled,value=true
3. Expect HTTP 200 with the Stage showing tracingEnabled true; observe HTTP 501 NotImplemented.
Re-replay both check ids in trace_ids.
