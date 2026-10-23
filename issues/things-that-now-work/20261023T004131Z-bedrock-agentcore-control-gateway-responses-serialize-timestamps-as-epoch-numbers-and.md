Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-gateway-lifecycle-basic
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-gateway-lifecycle-basic acc-gateway-rate-limit-batch-and-update acc-gateway-update-and-target-errors bedrock-agentcore-control-gateway-get-returns-metadata bedrock-agentcore-control-gateway-rate-limit-workflow bedrock-agentcore-control-gateway-target-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.gatewayUrl
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agentcore-control|CreateGateway|2xx->2xx|$.workloadIdentityDetails.workloadIdentityArn
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.gatewayUrl
Issue-Related: 20261023T004131Z-bedrock-agentcore-control-gateway-state-mutations-are-not-persisted-deletegateway-leaves.md
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Gateway responses serialize timestamps as epoch numbers and return a /mcp-suffixed gatewayUrl

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGateway

## Description
Every gateway-shaped response the emulator builds diverges from the modelled/recorded shape in the same two ways: createdAt/updatedAt are emitted as epoch floats instead of date-time strings, and gatewayUrl carries a '/mcp' path suffix that AWS does not return. One recording additionally shows workloadIdentityDetails being populated where AWS returned none. 5 causes, all the same serializer.

Cause 1 of 5: CreateGateway (basic lifecycle) - timestamps, gatewayUrl and workloadIdentityDetails.
AWS: Smithy CreateGatewayResponse.createdAt / .updatedAt target DateTimestamp and carry @timestampFormat date-time, so they must serialize as ISO-8601 strings, not JSON numbers. gatewayUrl is modelled as a plain string (length 1..1024) and the recording shows it as 'https://<gatewayId>.gateway.bedrock-agentcore.<region>.amazonaws.com' with no path - recorded behavior only. workloadIdentityDetails is modelled but the recorded CreateGateway response for this fixture contains no workloadIdentityDetails member - recorded behavior only.
Emulator: createdAt/updatedAt are epoch floats (example 1792715623.9447937); gatewayUrl is '<...>.amazonaws.com/mcp'; workloadIdentityDetails.workloadIdentityArn is present where the recording has nothing.
Why: SDKs that parse a @timestampFormat date-time member reject or mis-parse a bare number, so every gateway create/read breaks at deserialization; an MCP client that uses gatewayUrl verbatim appends its own path and ends up at '/mcp/mcp'.
Fix: the gateway response serializer (timestamp formatting for DateTimestamp members, gatewayUrl construction) in the CreateGateway/GetGateway/UpdateGateway output path.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#setup#1 (representative).

Cause 2 of 5: CreateGateway (acc rate-limit/update fixtures) - createdAt/updatedAt shape only.
AWS: as cause 1, both members are date-time timestamps.
Emulator: epoch floats (example 1792715405.6525264) - the fixture asserts the shape, not the value, and the shape check fails. Cluster covers acc-gateway-rate-limit-batch-and-update#setup#2 and acc-gateway-update-and-target-errors#setup#2.
Why/Fix: as cause 1.
Traces: acc-gateway-rate-limit-batch-and-update#setup#2.

Cause 3 of 5: GetGateway (metadata fixture) - createdAt/updatedAt shape.
AWS: GetGatewayResponse.createdAt/.updatedAt are DateTimestamp @timestampFormat date-time.
Emulator: epoch floats (example 1792715558.1995022).
Why/Fix: as cause 1, on the GetGateway output path.
Traces: bedrock-agentcore-control-gateway-get-returns-metadata#task#3.

Cause 4 of 5: GetGateway (basic lifecycle) - gatewayUrl '/mcp' suffix plus the timestamp shape.
AWS: the recorded GetGateway response returns the same gatewayUrl as CreateGateway, with no '/mcp' path - recorded behavior only.
Emulator: '<gatewayId>.gateway.bedrock-agentcore.us-west-2.amazonaws.com/mcp', and epoch-float timestamps.
Why/Fix: as cause 1.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#task#5.

Cause 5 of 5: CreateGateway (rate-limit / target workflow fixtures) - gatewayUrl and timestamp shapes.
AWS: as causes 1 and 4.
Emulator: '/mcp'-suffixed gatewayUrl and epoch-float createdAt/updatedAt. Cluster covers bedrock-agentcore-control-gateway-rate-limit-workflow#setup#2 and bedrock-agentcore-control-gateway-target-workflow#setup#1.
Why/Fix: as cause 1.
Traces: bedrock-agentcore-control-gateway-rate-limit-workflow#setup#2.

## Evidence
Cause 1 (bedrock-agentcore-control-gateway-lifecycle-basic#setup#1, status 202 both sides): $.createdAt / $.updatedAt expected a date-time-shaped value, actual 1792715623.9447937; $.gatewayUrl expected 'https://<gatewayId>.gateway.bedrock-agentcore.us-west-2.amazonaws.com', actual the same host with '/mcp' appended; $.workloadIdentityDetails.workloadIdentityArn reported 'extra' (expected absent).
Cause 2 (acc-gateway-rate-limit-batch-and-update#setup#2, +acc-gateway-update-and-target-errors#setup#2, status 202): $.createdAt/$.updatedAt actual 1792715405.6525264 against a shaped date-time expectation.
Cause 3 (bedrock-agentcore-control-gateway-get-returns-metadata#task#3, status 200): $.createdAt/$.updatedAt actual 1792715558.1995022.
Cause 4 (bedrock-agentcore-control-gateway-lifecycle-basic#task#5, status 200): $.gatewayUrl expected '...amazonaws.com', actual '...amazonaws.com/mcp'; timestamps as above.
Cause 5 (bedrock-agentcore-control-gateway-rate-limit-workflow#setup#2, +bedrock-agentcore-control-gateway-target-workflow#setup#1, status 202): $.gatewayUrl actual '...amazonaws.com/mcp' and epoch-float timestamps against shaped expectations.
Model: Smithy CreateGateway/GetGateway/UpdateGateway output members createdAt and updatedAt -> DateTimestamp with timestampFormat date-time.

## Steps to Reproduce
1. CreateGateway (POST /gateways/) with a valid name/roleArn/authorizerType as in bedrock-agentcore-control-gateway-lifecycle-basic setup#1 and read the 202 body: createdAt/updatedAt come back as JSON numbers rather than ISO-8601 date-time strings, gatewayUrl ends in '/mcp', and workloadIdentityDetails is populated.
2. GET /gateways/{gatewayIdentifier}/ for the same gateway (bedrock-agentcore-control-gateway-lifecycle-basic task#5): same timestamp shape and the same '/mcp' suffix.
3. Repeat with fixtures acc-gateway-rate-limit-batch-and-update#setup#2, bedrock-agentcore-control-gateway-get-returns-metadata#task#3 and bedrock-agentcore-control-gateway-rate-limit-workflow#setup#2 to see the same serializer defects on every gateway-shaped response.
