Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3
Issue-Key: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3|_mcp#30|AssertionError|assert status == 200, (
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# bedrock-agentcore__lambda__s3: a gateway's published MCP endpoint is unrouted — POST <gatewayUrl>/mcp answers 404 UnknownService for 'bedrock-agentcore' (1 cause)

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
Cause 1 of 1: the host CreateGateway publishes in gatewayUrl is not served. A SigV4-signed MCP JSON-RPC POST to <gatewayUrl>/mcp is rejected by the emulator's router with HTTP 404 and an UnknownService body claiming no emulated service is registered for 'bedrock-agentcore', even though bedrock-agentcore is mounted and answering its own API operations in the same run.

Failing check ids for this cause: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3

AWS: Smithy (bedrockagentcorecontrol) CreateGateway POST /gateways/, HTTP 202, output gatewayUrl (GatewayUrl, len 1..1024), status (GatewayStatus), protocolType default 'MCP', authorizerType @required; the gateway target is created with CreateGatewayTarget and reaches status READY. The MCP wire surface itself is not modelled (recorded behavior only): the blessed recording shows gatewayUrl is a bare host with no path, and the JSON-RPC surface lives at the fixed sub-path /mcp on that host. A SigV4-signed POST (signing name 'bedrock-agentcore', MCP-Protocol-Version 2025-03-26) of {"jsonrpc":"2.0","id":...,"method":"tools/list"} answers HTTP 200 with a JSON-RPC result listing the aggregated tools as <targetName>___<toolName>; a tools/call then really invokes the target Lambda, whose side effect is observable in S3; and the identical POST sent unsigned is refused with 401/403 because the gateway's inbound authorization is AWS_IAM.

Emulator: every call to that endpoint fails at the router before any MCP handling. Replay detail: AssertionError: MCP tools/list to https://<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp returned HTTP 404: '{"__type":"UnknownService","message":"no emulated service is registered for \'bedrock-agentcore\'"}'. Everything before it succeeded — S3 bucket, IAM roles, the writer Lambda, CreateGateway (gatewayUrl present and matching the emulator's published URL, status READY) and CreateGatewayTarget (status READY) — so the gateway is advertising an endpoint it then does not serve. Because the positive control (tools/list) never answers, the tools/call -> Lambda -> S3 join and the unsigned-POST refusal are unreachable too.

Why: a client that follows the documented flow — create a gateway, attach an mcp.lambda target, then drive the gateway's own gatewayUrl as an MCP server — gets a 404 UnknownService from the URL the service itself handed back. No MCP client can list or call tools against the emulator, the gateway's Lambda-target translation is unobservable, and the AWS_IAM enforcement on that endpoint cannot be verified; an UnknownService 404 is also indistinguishable from a wrong-region/endpoint misconfiguration, so callers debug their own code instead.

Fix: the gateway data-plane routing and handler — the emulator's request router must dispatch the gateway host it published in CreateGateway's gatewayUrl (and its /mcp sub-path) to the bedrock-agentcore gateway surface instead of failing service resolution, and that surface must serve MCP JSON-RPC: tools/list aggregated from the gateway's READY targets as <targetName>___<toolName>, tools/call translated into an invocation of the target's lambdaArn, and unsigned requests refused with 401/403 when authorizerType is AWS_IAM. The gateway record that CreateGateway/CreateGatewayTarget persist in services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/state.rs is the state this surface reads.

Traces: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3 (mismatch at the tools/list positive control). No open issue covers this endpoint: 20261023T113357Z-bedrock-agentcore-invokeagentruntime-stub.md is the InvokeAgentRuntime runtime-invoke stub, and 20261023T004131Z-bedrock-agentcore-control-agent-runtime-and-gateway-target-control-plane-rejections.md is about control-plane gateway-target rejections, not the gateway's MCP wire surface.

## Evidence
Expected (blessed recording, real AWS us-west-2): POST https://<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp, SigV4-signed with signing name 'bedrock-agentcore', headers content-type: application/json, accept: application/json, text/event-stream, MCP-Protocol-Version: 2025-03-26, body {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}} -> HTTP 200 with a JSON-RPC 'result' whose tools include '<targetName>___<toolName>'; a subsequent tools/call with that qualified name invokes the target Lambda (result.isError not true) and the object it writes is readable in S3; the same POST unsigned -> HTTP 401 or 403.

Actual (emulator): HTTP 404 with body {"__type":"UnknownService","message":"no emulated service is registered for 'bedrock-agentcore'"} — raised as AssertionError 'MCP tools/list to .../mcp returned HTTP 404' — despite CreateGateway having returned that gatewayUrl with status READY, the published-URL check passing, and the gateway target reporting READY.

## Steps to Reproduce
1. Boot the emulator and point boto3 at it.
2. s3 CreateBucket; iam CreateRole for the Lambda and for the gateway; lambda CreateFunction for a handler that writes {key, body} to that bucket.
3. bedrock-agentcore-control CreateGateway(name=..., roleArn=<gateway role>, protocolType='MCP', authorizerType='AWS_IAM'); keep gatewayId and gatewayUrl; poll GetGateway until status == 'READY'.
4. CreateGatewayTarget(gatewayIdentifier=gatewayId, name=<targetName>, targetConfiguration={mcp:{lambda:{lambdaArn:<fn arn>, toolSchema:{inlinePayload:[...]}}}}); poll GetGatewayTarget until status == 'READY'.
5. POST to gatewayUrl.rstrip('/') + '/mcp', SigV4-signed with service name 'bedrock-agentcore', body {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}.
   -> AWS: HTTP 200 JSON-RPC result listing '<targetName>___<toolName>'. Emulator: HTTP 404 {"__type":"UnknownService","message":"no emulated service is registered for 'bedrock-agentcore'"}.
6. (blocked by step 5) repeat with method 'tools/call', params {name:'<targetName>___<toolName>', arguments:{key, body}} and GetObject the key from the bucket; and repeat the same POST unsigned, expecting 401/403.
