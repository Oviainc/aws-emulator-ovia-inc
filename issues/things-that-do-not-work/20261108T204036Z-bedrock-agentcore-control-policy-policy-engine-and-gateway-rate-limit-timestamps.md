Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-policy-engine-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-policy-engine-lifecycle bedrock-agentcore-control-policy-error-paths bedrock-agentcore-control-policy-lifecycle acc-gateway-rate-limit-batch-and-update
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreatePolicyEngine|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|CreatePolicyEngine|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agentcore-control|GetPolicyEngine|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|GetPolicyEngine|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agentcore-control|CreatePolicy|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|CreatePolicy|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agentcore-control|GetPolicy|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|GetPolicy|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agentcore-control|BatchPutGatewayRateLimits|2xx->2xx|$.rateLimits[*].createdAt
Issue-Key: bedrock-agentcore-control|BatchPutGatewayRateLimits|2xx->2xx|$.rateLimits[*].status
Issue-Key: bedrock-agentcore-control|BatchPutGatewayRateLimits|2xx->2xx|$.rateLimits[*].updatedAt
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|2xx->2xx|$.status
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|2xx->2xx|$.updatedAt
Issue-Commit: f392f82803b1854878505550560cb55ee4e9c61d

# Policy, policy-engine and gateway rate-limit timestamps are epoch numbers and rate-limit status never settles

> _Note: this issue was identified against the code base at commit `f392f82803b1854878505550560cb55ee4e9c61d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyEngine

## Description
This bundle has 6 causes. Across the policy, policy-engine and gateway rate-limit surfaces the emulator serializes createdAt/updatedAt as a bare JSON number (fractional epoch seconds) where AWS serializes an ISO-8601 date-time string, and on the rate-limit surface it additionally leaves the resource in the non-terminal status CREATING that never advances.

Shared AWS fact (Smithy): createdAt and updatedAt on these shapes target DateTimestamp, which carries @timestampFormat("date-time"). On the wire that is an RFC-3339/ISO-8601 string (e.g. "2026-11-08T12:34:56.789Z"), NOT the protocol-default epoch number. The replay reports these as a shape divergence ('<shaped-not-asserted>' means the recorded value's shape, a date-time string, was asserted and a number arrived).

Cause 1 of 6 - CreatePolicyEngine (fixture bedrock-agentcore-control-policy-engine-lifecycle, step setup#1).
AWS: POST /policy-engines -> 202 with createdAt/updatedAt as date-time strings (Smithy: CreatePolicyEngineResponse.createdAt/updatedAt -> DateTimestamp @timestampFormat date-time, both @required).
Emulator: emits numbers for both members.
Why: an SDK/parser that expects a date-time string for this member fails to deserialize or yields a wrong instant, so every client that reads the creation time of an engine is broken regardless of environment.
Fix: the CreatePolicyEngine response serializer (the timestamp encoding for DateTimestamp members).

Cause 2 of 6 - GetPolicyEngine (fixture bedrock-agentcore-control-policy-engine-lifecycle, step task#2).
AWS: GET /policy-engines/{policyEngineId} -> 200 with createdAt/updatedAt as date-time strings.
Emulator: numbers for both.
Why: same as cause 1 on the read path.
Fix: the GetPolicyEngine response serializer.

Cause 3 of 6 - CreatePolicy (fixture bedrock-agentcore-control-policy-error-paths, step task#3).
AWS: POST /policy-engines/{policyEngineId}/policies -> 202 with createdAt/updatedAt as date-time strings (Smithy: CreatePolicyResponse.createdAt/updatedAt -> DateTimestamp @timestampFormat date-time).
Emulator: numbers for both.
Why: same as cause 1.
Fix: the CreatePolicy response serializer.

Cause 4 of 6 - GetPolicy (fixture bedrock-agentcore-control-policy-error-paths, step task#4).
AWS: GET .../policies/{policyId} -> 200 with createdAt/updatedAt as date-time strings.
Emulator: numbers for both.
Why: same as cause 1 on the read path.
Fix: the GetPolicy response serializer.

Cause 5 of 6 - BatchPutGatewayRateLimits (fixture acc-gateway-rate-limit-batch-and-update, step task#9).
AWS: PUT /gateways/{gatewayIdentifier}/rate-limits/batch -> 200 and each returned rate limit carries createdAt/updatedAt as date-time strings and status 'ACTIVE' (Smithy: GatewayRateLimitStatus values CREATING, ACTIVE, UPDATING, DELETING; the recorded response for a batch put of limits on a live gateway reports ACTIVE).
Emulator: rateLimits[0].createdAt/updatedAt are numbers and rateLimits[0].status is 'CREATING'.
Why: a client that waits for a rate limit to reach ACTIVE before sending traffic waits forever, and the timestamp cannot be parsed as modeled.
Fix: BatchPutGatewayRateLimits' handler/state (settle the limit to ACTIVE as AWS does) and its response serializer (timestamp encoding).

Cause 6 of 6 - GetGatewayRateLimit (fixture acc-gateway-rate-limit-batch-and-update, step task#10).
AWS: GET /gateways/{gatewayIdentifier}/rate-limits/{rateLimitId} -> 200 with status 'ACTIVE' and date-time string timestamps.
Emulator: status 'CREATING' and numeric createdAt/updatedAt - i.e. the limit written by cause 5 is still reported as not yet created.
Why: the rate limit never becomes usable from the client's point of view; polling GetGatewayRateLimit for ACTIVE never terminates.
Fix: the gateway rate-limit state transition plus GetGatewayRateLimit's response serializer.

Traces: bedrock-agentcore-control-policy-engine-lifecycle#setup#1, bedrock-agentcore-control-policy-engine-lifecycle#task#2, bedrock-agentcore-control-policy-error-paths#task#3, bedrock-agentcore-control-policy-error-paths#task#4, acc-gateway-rate-limit-batch-and-update#task#9, acc-gateway-rate-limit-batch-and-update#task#10. The same numeric-timestamp cluster also recurs on bedrock-agentcore-control-policy-lifecycle#task#3 and #task#4.

## Evidence
Cause 1 (policy-engine-lifecycle#setup#1) CreatePolicyEngine 202 - '$.createdAt: value_mismatch (expected <shaped-not-asserted>, got 1794169421.6921656)', same for $.updatedAt; AWS shape is a date-time string per DateTimestamp @timestampFormat("date-time").
Cause 2 (policy-engine-lifecycle#task#2) GetPolicyEngine 200 - identical numeric createdAt/updatedAt.
Cause 3 (policy-error-paths#task#3) CreatePolicy 202 - '$.createdAt / $.updatedAt: value_mismatch (expected <shaped-not-asserted>, got 1794168958.7879467)'.
Cause 4 (policy-error-paths#task#4) GetPolicy 200 - same two members numeric.
Cause 5 (acc-gateway-rate-limit-batch-and-update#task#9) BatchPutGatewayRateLimits 200 - '$.rateLimits[0].status: value_mismatch (expected ACTIVE, got CREATING)' plus numeric $.rateLimits[0].createdAt/updatedAt.
Cause 6 (acc-gateway-rate-limit-batch-and-update#task#10) GetGatewayRateLimit 200 - '$.status: value_mismatch (expected ACTIVE, got CREATING)' plus numeric $.createdAt/$.updatedAt.

## Steps to Reproduce
1. Replay bedrock-agentcore-control-policy-engine-lifecycle steps setup#1 and task#2: inspect createdAt/updatedAt in the CreatePolicyEngine and GetPolicyEngine bodies - they are JSON numbers, not ISO-8601 strings.
2. Replay bedrock-agentcore-control-policy-error-paths steps task#3 and task#4 for CreatePolicy/GetPolicy - same numeric encoding.
3. Replay acc-gateway-rate-limit-batch-and-update steps task#9 and task#10: BatchPutGatewayRateLimits returns status CREATING, and the following GetGatewayRateLimit still reports CREATING instead of ACTIVE, with numeric timestamps.
