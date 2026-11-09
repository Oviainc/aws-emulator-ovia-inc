Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3
Issue-Key: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3|run#18|AssertionError|assert not (p := ctx.published_url_mismatch(gateway_url)), p
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# The AgentCore gateway MCP endpoint is published on a bedrock-agentcore.amazonaws.com host the emulator does not serve

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
Cause 1 of 1: the gateway's MCP endpoint URL is announced as a real AWS hostname, so no client can reach the endpoint the emulator itself serves.

AWS: recorded behavior only for the host — the Smithy model types the gateway URL as a string and places no constraint on its host. The behavioral requirement is that the address the service announces is the address the service answers on: in AWS the published `https://<gateway-id>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp` resolves to the service that served CreateGateway. The path shape (`/mcp`) and the scheme are part of the contract; the host must be one the emulator answers on.

Emulator: publishes `https://<gateway-id>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp`, a host it does not serve and which does not resolve to the emulator; the emulator is reachable only on its own host/port. Every MCP tools/call against the published URL therefore never reaches the gateway, and the check fails before exercising any tool behavior.

Why: a client does exactly what the service tells it to — it dials the URL the gateway hands back. Announcing an unreachable third-party host makes the whole MCP data plane unusable and turns every gateway integration into a name-resolution or egress failure rather than a service response.

Fix: the gateway endpoint serializer/state that mints the published MCP URL — it must publish the emulator's own host and port (or a per-resource listener it owns) while keeping the path and scheme AWS uses.

Traces: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3.

## Evidence
Check bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3:
AssertionError: the emulator published https://<gateway-id>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<resource path>
(the gateway id and the emulator's port differ per run; the defect is that the announced host is not the one the emulator is reached on)

## Steps to Reproduce
1. CreateGateway (protocolType MCP) and add an MCP Lambda target.
2. Read the gateway's published MCP endpoint URL from the create/get response.
3. POST an MCP tools/call to that URL — it does not reach the emulator, because the announced host is an amazonaws.com name the emulator does not serve.
