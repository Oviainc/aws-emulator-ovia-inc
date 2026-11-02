Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3
Issue-Key: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3|_mcp#30|AssertionError|assert status == 200, (
Issue-Commit: 307ae70e1176afe8148743d9dbd874f3de53a94d

# REOPENS the gateway MCP endpoint report: a gateway's published MCP endpoint is still refused — the signed tools/list POST answers 404 ResourceNotFoundException "This gateway serves no resource at this path"

> _Note: this issue was identified against the code base at commit `307ae70e1176afe8148743d9dbd874f3de53a94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway (gateway MCP data plane: JSON-RPC tools/list, tools/call)

## Description
REOPENS 20261024T085126Z-bedrock-agentcore-lambda-s3-a-gateway-s-published.md: its fix left the call refused. The router no longer answers UnknownService, but the gateway endpoint the service itself published still serves no MCP surface, so the same check fails at the same positive control.

Cause 1 of 1: the AgentCore gateway data plane serves no MCP JSON-RPC surface at the path reached from the gatewayUrl the control plane vends, so a SigV4-signed tools/list is refused and the gateway's Lambda target can never be reached.

AWS: Smithy (bedrockagentcorecontrol) CreateGateway POST /gateways/ -> HTTP 202, output `gatewayUrl` (GatewayUrl, @length 1..1024, no pattern), `status` (GatewayStatus), `protocolType` enum GatewayProtocolType with default "MCP", `authorizerType` @required; GetGateway GET /gateways/{gatewayIdentifier}/ -> 200 returns the same `gatewayUrl`. The MCP wire surface itself is not modeled, so recorded behavior only: the behavioral recording shows the gateway host serving the MCP JSON-RPC transport at the fixed sub-path `/mcp` — a POST signed with SigV4 (signing name `bedrock-agentcore`, headers content-type: application/json, accept: application/json, text/event-stream, MCP-Protocol-Version: 2025-03-26) carrying {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}} answers HTTP 200 with a JSON-RPC `result` whose `tools` include the aggregated name `<targetName>___<toolName>` for every READY target of the gateway; a following tools/call with that qualified name invokes the target's lambdaArn and the side effect is observable; the same POST unsigned is refused 401/403 because authorizerType is AWS_IAM. In that recording `gatewayUrl` is the bare gateway host and the client appends `/mcp`.

Emulator: the gateway host answers that POST with HTTP 404 and body {"__type":"ResourceNotFoundException","message":"This gateway serves no resource at this path"} — fixed text, with no variable part; AWS does not word a gateway-path refusal this way and, for a gateway whose target is READY, does not refuse the MCP path at all. The requested path is the published `gatewayUrl` plus `/mcp`: because the control plane now vends `gatewayUrl` already carrying the `/mcp` transport path (per the resolved wire issue 20261029T170911Z-bedrock-agentcore-control-gateway-responses-omit-the-mcp-transport-path-in.md), the request lands on `<host>/mcp/mcp`, which the gateway surface does not route. Everything before the POST succeeded: S3 bucket, both IAM roles, the writer Lambda (State Active), CreateGateway (gatewayUrl present, published-URL check clean, GetGateway status READY) and CreateGatewayTarget with an mcp.lambda target (GetGatewayTarget status READY). So the gateway advertises an endpoint it then refuses, and the two interlocking facts — a gatewayUrl that already contains the transport path, and a data plane that routes only one exact path — must be reconciled for either corpus to work.

Why: a client that follows the documented flow (create a gateway, attach an mcp.lambda target, drive the gateway's own gatewayUrl as an MCP server) gets 404 ResourceNotFoundException from the URL the service handed it, with a message that reads as "your path is wrong" rather than "this emulator serves no MCP"; no MCP client can list or call tools, the gateway-to-Lambda translation is unobservable, and the AWS_IAM enforcement on that endpoint cannot be verified. Because tools/list is the positive control, the tools/call -> Lambda -> S3 join and the unsigned-refusal assertion are unreachable as well.

Fix: the gateway data-plane router/handler for the published gateway host (the surface that serves MCP JSON-RPC from the gateway and target records persisted by CreateGateway/CreateGatewayTarget in the bedrock-agentcore-control crate's state). It must serve the MCP transport endpoint at the path reachable from the vended gatewayUrl — i.e. accept the transport sub-path idempotently (treat a duplicated trailing `/mcp` segment as the same transport endpoint) or stop embedding it in gatewayUrl, consistently with the wire fix cited above — and answer tools/list with the gateway's READY targets aggregated as `<targetName>___<toolName>`, tools/call by invoking the target's lambdaArn, and an unsigned request with 401/403 when authorizerType is AWS_IAM. Related but not covering: 20261025T022656Z-bedrock-agentcore-lambda-s3-an-agentcore-gateway-mcp.md (open) is about the tool error the Lambda target returns once the call is routed, which is downstream of this refusal.

Traces: bedrock-agentcore__lambda__s3.gateway-mcp-tools-call-reaches-lambda-and-writes-to-s3 (mismatch at the tools/list positive control).

## Evidence
Expected (behavioral recording, real AWS): POST <gateway MCP endpoint>, SigV4-signed with signing name 'bedrock-agentcore', body {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}} -> HTTP 200, JSON-RPC result listing '<targetName>___<toolName>'; then tools/call invokes the Lambda target (result.isError not true) and the object it writes is readable in S3; the identical POST unsigned -> 401/403.

Actual (emulator): AssertionError: MCP tools/list to https://<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com/mcp/mcp returned HTTP 404: '{"__type":"ResourceNotFoundException","message":"This gateway serves no resource at this path"}'. Message breakdown: 'This gateway serves no resource at this path' is entirely emulator-authored fixed text with no request- or state-derived values; the host, gatewayId and region in the URL are per-run values, not part of the requirement. The endpoint is the vended gatewayUrl (which the emulator now returns with a trailing '/mcp') plus the check's '/mcp' transport sub-path. Preceding steps all passed: GetGateway status READY, GetGatewayTarget status READY, lambda State Active, ctx.published_url_mismatch(gatewayUrl) clean.

## Steps to Reproduce
1. Boot the emulator and point boto3 at it.
2. s3 CreateBucket; iam CreateRole for the Lambda execution role and for the gateway role (trust bedrock-agentcore.amazonaws.com, policy lambda:InvokeFunction on the function); lambda CreateFunction (python3.12) for a handler that writes {key, body} to that bucket; poll GetFunction until State == 'Active'.
3. bedrock-agentcore-control CreateGateway(name=..., roleArn=<gateway role>, authorizerType='AWS_IAM') (protocolType defaults to MCP); keep gatewayId and gatewayUrl; poll GetGateway until status == 'READY'.
4. CreateGatewayTarget(gatewayIdentifier=gatewayId, name=<targetName>, targetConfiguration={mcp:{lambda:{lambdaArn:<fn arn>, toolSchema:{inlinePayload:[{name:'simparityPutObject', ...}]}}}}, credentialProviderConfigurations=[{credentialProviderType:'GATEWAY_IAM_ROLE'}]); poll GetGatewayTarget until status == 'READY'.
5. POST to gatewayUrl.rstrip('/') + '/mcp', SigV4-signed with service name 'bedrock-agentcore' and header MCP-Protocol-Version: 2025-03-26, body {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}.
   -> AWS: HTTP 200 JSON-RPC result listing '<targetName>___simparityPutObject'. Emulator: HTTP 404 {"__type":"ResourceNotFoundException","message":"This gateway serves no resource at this path"}.
6. (blocked by step 5) repeat with method 'tools/call', params {name:'<targetName>___simparityPutObject', arguments:{key, body}}, then GetObject the key; and repeat the same POST unsigned, expecting 401/403.
