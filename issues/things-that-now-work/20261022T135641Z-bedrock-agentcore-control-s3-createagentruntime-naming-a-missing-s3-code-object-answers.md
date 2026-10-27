Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object
Issue-Key: bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object|run#12|AssertionError|assert refusal["Code"] == "ValidationException", (
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# CreateAgentRuntime naming a missing S3 code object answers 500 AmbiguousService instead of the modeled ValidationException

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentRuntime

## Description
Cause 1 of 1: the AgentCore control plane is unreachable through the emulator's router, so the S3-artifact validation path of CreateAgentRuntime is never exercised and the wrong error class is returned.
AWS: Smithy — AmazonBedrockAgentCoreControl (2023-06-05, restJson1) declares awsService.endpointPrefix "bedrock-agentcore-control" and sigv4Name "bedrock-agentcore"; AmazonBedrockAgentCore (2024-02-28) declares endpointPrefix "bedrock-agentcore" with the same sigv4Name. AWS routes by endpoint host + restJson1 route, so the control plane is reachable, and (recorded behavior) CreateAgentRuntime whose codeConfiguration names an S3 object that does not exist is refused synchronously with a 400-class `ValidationException` (recorded message example: "S3 object not found in bucket"), with no runtime of that name ever listed.
Emulator: the call is answered HTTP 500 with error code `AmbiguousService` ("2 emulated services all claim requests for 'bedrock-agentcore': ..."), so the check sees code 'AmbiguousService' where ValidationException is required.
Why: a client that mistypes or has not yet uploaded its deployment package gets a retryable server fault with a code absent from the model instead of the modeled client-side ValidationException, so it retries instead of correcting the request; it also cannot tell this case apart from a genuine service fault.
Fix: the request-routing / service-registration component (dispatch on endpointPrefix/host + restJson1 route, not the sigv4 signing name); once reachable, the CreateAgentRuntime handler's codeConfiguration validation must verify the S3 object exists and raise ValidationException. Same router root cause as the bedrock-agentcore-control issue filed in this sweep; filed separately because this check belongs to a different service grouping.
Traces: bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object.

## Evidence
Expected: refusal code "ValidationException" (HTTP 400) for CreateAgentRuntime pointing at a not-yet-uploaded s3://.../deployment_package.zip. Actual: AssertionError — expected CreateAgentRuntime to refuse the missing code object ... with ValidationException, got 'AmbiguousService' (HTTP 500, message "2 emulated services all claim requests for 'bedrock-agentcore': bedrock-agentcore, bedrock-agentcore; the request is rejected rather than served by an arbitrary one").

## Steps to Reproduce
1. Boot the emulator.
2. Create an S3 bucket but do NOT upload the deployment package.
3. Call `bedrock-agentcore-control.create_agent_runtime(...)` with a codeConfiguration pointing at that missing key -> 500 AmbiguousService instead of 400 ValidationException.
4. Replay bedrock-agentcore-control__s3.agent-runtime-code-artifact-requires-existing-s3-object.
