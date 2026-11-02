Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-gateway-lifecycle-basic
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-gateway-lifecycle-basic bedrock-agentcore-control-gateway-rate-limit-workflow bedrock-agentcore-control-gateway-target-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.gatewayUrl
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.workloadIdentityDetails.workloadIdentityArn
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.gatewayUrl
Issue-Commit: 307ae70e1176afe8148743d9dbd874f3de53a94d

# CreateGateway and GetGateway gateway representation diverges: gatewayUrl carries a trailing /mcp path and CreateGateway emits workloadIdentityDetails

> _Note: this issue was identified against the code base at commit `307ae70e1176afe8148743d9dbd874f3de53a94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
The emulator's gateway representation is built with two members that do not match what AWS returns: every gatewayUrl it mints carries an extra trailing path segment, and CreateGateway's 202 body carries a workloadIdentityDetails block the create response does not contain. Both are read straight out of the create/read responses, so every client that stores the gateway endpoint or round-trips the create response sees the divergence. 2 causes must both be fixed before this issue is closed.

Cause 1 of 2: gatewayUrl is rendered with a trailing "/mcp" path segment on both the create and the read path.
AWS: Smithy CreateGateway/GetGateway output member gatewayUrl -> GatewayUrl (string, @length min 1 max 1024); the model constrains only the length, so this is recorded behavior only. The recording shows the gateway endpoint as a bare origin with NO path component: scheme+host only, the host being "<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com". The fixed part of that template is the ".gateway.bedrock-agentcore." / ".amazonaws.com" host suffix and the absence of any path; the variable parts are the gatewayId returned for the gateway and the region the request was made in, both of which are per-environment values. GetGateway returns the identical string CreateGateway returned for the same gateway.
Emulator: appends "/mcp" to the origin, e.g. it answers "https://<gatewayId>.gateway.bedrock-agentcore.us-west-2.amazonaws.com/mcp" where the recording has "https://<gatewayId>.gateway.bedrock-agentcore.us-west-2.amazonaws.com". The same wrong string is served by CreateGateway (202) and by GetGateway (200), and it also fails the fixture's shape-only assertion on gatewayUrl in the rate-limit workflow, so it is the single URL formatter and not one handler.
Why: gatewayUrl is the value a client is told to connect its MCP client to. A client that treats the returned string as an origin and appends its own protocol path builds "...amazonaws.com/mcp/mcp" and cannot reach the gateway; a client that compares the stored endpoint against the one AWS would have issued, or parses it into host/path, sees a path component AWS never sends.
Fix: the gatewayUrl construction in the bedrock-agentcore-control gateway state/serializer shared by the CreateGateway and GetGateway handlers — emit the origin only, with no protocol path appended.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#setup#1 (CreateGateway), bedrock-agentcore-control-gateway-lifecycle-basic#task#5 (GetGateway), bedrock-agentcore-control-gateway-rate-limit-workflow#setup#2 (CreateGateway, shape assertion).

Cause 2 of 2: the CreateGateway 202 response carries a workloadIdentityDetails block AWS does not return at create time.
AWS: Smithy CreateGatewayResponse does model an optional workloadIdentityDetails -> WorkloadIdentityDetails{workloadIdentityArn required}, but it is not @required, and the recording shows the member ABSENT from the CreateGateway response for a gateway created with no workload identity configured (recorded behavior only; the fixture asserts the key is not present).
Emulator: includes workloadIdentityDetails.workloadIdentityArn in the create response, synthesising a workload-identity ARN of the form arn:aws:bedrock-agentcore:<region>:<account>:workload-identity-directory/default/workload-identity/<gatewayId> for a gateway that was created without one (the ARN's region, account and gateway id are per-run values; the defect is that the member is emitted at all).
Why: a client that reads workloadIdentityDetails from the create response concludes a workload identity was provisioned for the gateway and will reference that directory entry; against AWS the member is absent and the client must instead resolve the identity later or treat it as unset. Tests and SDK round-trips that assert the create response shape see an extra member.
Fix: the CreateGateway handler's response serializer in the bedrock-agentcore-control service crate — do not populate workloadIdentityDetails on create when no workload identity was requested.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#setup#1.

## Evidence
Cause 1 — fixture bedrock-agentcore-control-gateway-lifecycle-basic, step setup#1 (CreateGateway, status 202 expected and returned): $.gatewayUrl value_mismatch, expected 'https://{{basic_gateway_id}}.gateway.bedrock-agentcore.us-west-2.amazonaws.com' (the fixture's template for the created gateway's id), got 'https://sim2real-probe-gateway-a8e58e39-bmw3vx6otv.gateway.bedrock-agentcore.us-west-2.amazonaws.com/mcp'. Same fixture, step task#5 (GetGateway, status 200 expected and returned): $.gatewayUrl expected 'https://sim2real-probe-gateway-a8e58e39-bmw3vx6otv.gateway.bedrock-agentcore.us-west-2.amazonaws.com', got the same string with '/mcp' appended. Fixture bedrock-agentcore-control-gateway-rate-limit-workflow, step setup#2 (CreateGateway, 202): $.gatewayUrl failed the shape-only assertion ('<shaped-not-asserted>') with 'https://sim2real-probe-gw-rl-c78d3b2d-tnyztq83pr.gateway.bedrock-agentcore.us-west-2.amazonaws.com/mcp'. The host labels and gateway ids above are per-run generated values; the requirement is the absence of the '/mcp' path.
Cause 2 — fixture bedrock-agentcore-control-gateway-lifecycle-basic, step setup#1: $.workloadIdentityDetails.workloadIdentityArn reported 'extra' (expected None, got 'arn:aws:bedrock-agentcore:us-west-2:000000000000:workload-identity-directory/default/workload-identity/sim2real-probe-gateway-a8e58e39-bmw3vx6otv'). Status and all other asserted members matched.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture bedrock-agentcore-control-gateway-lifecycle-basic. Step setup#1 POSTs /gateways/ (CreateGateway) with a name, roleArn, protocolType MCP and an authorizer config; read $.gatewayUrl of the 202. Then step task#5 GETs /gateways/{gatewayIdentifier}/ (GetGateway) and reads $.gatewayUrl of the 200. Both strings end in '/mcp' instead of at the host. Fixture bedrock-agentcore-control-gateway-rate-limit-workflow step setup#2 shows the same on a second CreateGateway.
3. Cause 2: in the same setup#1 response, check for the workloadIdentityDetails key — the emulator emits it with a synthesised workloadIdentityArn where the recording has no such member.
