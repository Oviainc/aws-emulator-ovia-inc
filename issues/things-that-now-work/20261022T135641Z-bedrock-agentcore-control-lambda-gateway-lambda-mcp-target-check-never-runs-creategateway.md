Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function
Issue-Key: bedrock-agentcore-control__lambda|CreateGateway|AmbiguousService
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Gateway + Lambda MCP-target check never runs: CreateGateway refused 500 AmbiguousService by sigv4-name routing

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
Cause 1 of 1: the cross-service AgentCore-Gateway/Lambda check cannot reach the AgentCore control plane at all, because the emulator's router resolves an emulated service by its sigv4 signing name and two services claim 'bedrock-agentcore'.
AWS: Smithy — AmazonBedrockAgentCoreControl (2023-06-05, restJson1) has awsService.endpointPrefix "bedrock-agentcore-control" with sigv4Name "bedrock-agentcore"; AmazonBedrockAgentCore (2024-02-28, restJson1) has endpointPrefix "bedrock-agentcore" with the same sigv4Name "bedrock-agentcore". AWS dispatches on the endpoint host and the restJson1 method+URI, not on the signing name, so CreateGateway answers its modeled success response, and a gateway target naming a non-existent Lambda function is refused with the operation's modeled 400-class error.
Emulator: CreateGateway is answered HTTP 500 with error code `AmbiguousService`; recorded message example: "2 emulated services all claim requests for 'bedrock-agentcore': bedrock-agentcore, bedrock-agentcore; the request is rejected rather than served by an arbitrary one" (retried 3x, since 5xx is retryable).
Why: a client that wires an MCP gateway target to a Lambda function can never create the gateway, and sees a non-modeled retryable 500 instead of either success or the modeled validation error.
Fix: the request-routing / service-registration component — key dispatch on endpointPrefix (host) plus the restJson1 route rather than the sigv4 signing name. Same root cause as the bedrock-agentcore-control issue filed in this sweep (Cause 1 there); filed separately because this check belongs to a different service grouping. Related already-open router issues in other services: 20261022T032542Z-application-autoscaling-lambda-..., 20261022T131021Z-lex-models-v2-lex-runtime-v2-....
Traces: bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function.

## Evidence
Expected: CreateGateway returns its modeled 2xx gateway envelope (and a gateway target naming a missing Lambda function is then refused with the operation's modeled 400-class error). Actual: botocore.exceptions.ClientError — An error occurred (AmbiguousService) when calling the CreateGateway operation (reached max retries: 3): 2 emulated services all claim requests for 'bedrock-agentcore': bedrock-agentcore, bedrock-agentcore; the request is rejected rather than served by an arbitrary one.

## Steps to Reproduce
1. Boot the emulator.
2. Call `boto3.client('bedrock-agentcore-control').create_gateway(...)` -> 500 AmbiguousService.
3. Replay bedrock-agentcore-control__lambda.gateway-mcp-lambda-target-requires-existing-function.
