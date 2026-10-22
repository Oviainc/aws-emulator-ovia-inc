Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3
Issue-Key: bedrock-agentcore__lambda__s3|CreateFunction|AmbiguousService
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Cross-service AgentCore Gateway + Lambda + S3 check never runs: CreateFunction is refused with AmbiguousService because four emulated services claim the 'lambda' signing name

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Cause 1 of 1: the AgentCore-Gateway -> Lambda -> S3 scenario cannot start because the router refuses the Lambda call that creates its MCP tool target.
AWS: CreateFunction is a bound Lambda operation — Smithy: restJson1, POST /2015-03-31/functions, success code 201, output FunctionConfiguration (FunctionName, FunctionArn, State, RevisionId, ...), modeled errors InvalidParameterValueException 400 / ResourceConflictException 409 / ResourceNotFoundException 404 / CodeStorageExceededException 400 / ServiceException 500 / TooManyRequestsException 429. Four modeled services in this repo genuinely share awsService.endpointPrefix "lambda" and sigv4Name "lambda" (the Lambda control plane AWSGirApiService, plus LambdaCoreApiService / lambdamicrovms / lambdaweb); on AWS they are one endpoint whose requests are disambiguated by HTTP method + URI path, not by signing name. "AmbiguousService" is not an AWS error code and no Lambda operation can answer it.
Emulator: the first CreateFunction call is refused before dispatch with code AmbiguousService and message "4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one", retried to exhaustion ("reached max retries: 3"), i.e. served with a retryable 5xx-class status. The router resolves a request to a service by sigv4 signing name alone, so all four claimants tie and the request is dropped; they all print under the same token, so the message cannot distinguish them.
Why: Lambda is unusable through the emulator, and with it every scenario that composes AgentCore Gateway MCP tool targets over a Lambda function and the S3 side effects that function writes. The caller receives a retried server error rather than any modeled client error, so it burns its full retry budget on a request that can never succeed, and the cross-service contract (a gateway tools/call reaching Lambda and the invocation's object landing in S3) is never exercised.
Fix: the request router's service-resolution step — disambiguate a signing name shared by several modeled services by matching the request's method+URI against each candidate service's bound restJson1 operations (or by endpoint prefix / host) instead of refusing the request. Two open issues already describe the same 'lambda' four-way collision — "Lambda is unreachable: four emulated services all claim the 'lambda' signing name, so every Lambda request is refused with AmbiguousService", "Lambda is unreachable (router AmbiguousService across four 'lambda'-signing services), and a CloudFormation-deployed Lambda stack exposes no Outputs (2 causes)" and "Wire replay: every Lambda request answered 500 AmbiguousService instead of the modeled response" — but none of them carries this check's replay key, so this cross-service instance would otherwise be closed unreplayed; a shared router fix may close all of them.
Traces: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3 (dies on its first CreateFunction call, before any gateway, MCP tools/call or S3 assertion).

## Evidence
Expected: CreateFunction returns HTTP 201 with a FunctionConfiguration envelope (Smithy: CreateFunction -> FunctionConfiguration, http {method: POST, uri: /2015-03-31/functions, code: 201}), or one of its modeled errors (InvalidParameterValueException 400, ResourceConflictException 409, TooManyRequestsException 429, ServiceException 500).
Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateFunction operation (reached max retries: 3): 4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one.
Retry exhaustion shows a retryable 5xx-class status; AmbiguousService appears in no Lambda operation's modeled error list. Smithy confirms the collision is real in the model: AWSGirApiService (sdkId "Lambda") and LambdaCoreApiService (sdkId "Lambda Core") both declare awsService.endpointPrefix "lambda" and sigv4Name "lambda", as do lambdamicrovms and lambdaweb.

## Steps to Reproduce
Boot the emulator, point a botocore 'lambda' client at it and call create_function(FunctionName=..., Role=..., Runtime="python3.12", Handler=..., Code={"ZipFile": ...}). Observe the AmbiguousService refusal after three retries instead of a 201 FunctionConfiguration. The rest of the scenario — CreateGateway / CreateGatewayTarget for an mcp.lambda target, the MCP tools/call through the gateway, and the object the Lambda writes to S3 — is never reached because the tool function cannot be created.
