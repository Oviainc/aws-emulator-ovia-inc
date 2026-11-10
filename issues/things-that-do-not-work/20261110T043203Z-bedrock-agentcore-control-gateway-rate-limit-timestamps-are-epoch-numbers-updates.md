Issue-Source: trace-sweep-divergence
Issue-Trace: acc-gateway-rate-limit-batch-and-update
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-gateway-rate-limit-batch-and-update bedrock-agentcore-control-gateway-rate-limit-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateGatewayRateLimit|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|CreateGatewayRateLimit|2xx->2xx|$.updatedAt
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|2xx->2xx|$.description
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|2xx->2xx|$.entries[*].requests[*].period
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|2xx->2xx|$.entries[*].requests[*].rate
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|4xx->2xx|$
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetGatewayRateLimit|4xx->2xx|status
Issue-Related: 20261108T204036Z-bedrock-agentcore-control-policy-policy-engine-and-gateway-rate-limit-timestamps.md
Issue-Commit: 97e87117743d236dfa23fd76b444a194861aeef0

# Gateway rate limit timestamps are epoch numbers, updates are not applied and deletes are not honored

> _Note: this issue was identified against the code base at commit `97e87117743d236dfa23fd76b444a194861aeef0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayRateLimit

## Description
The gateway rate-limit sub-resource diverges in its timestamp serialization, in not applying UpdateGatewayRateLimit, and in not honoring DeleteGatewayRateLimit. 3 causes, all on service bedrock-agentcore-control.

Cause 1 of 3: createdAt/updatedAt are serialized as epoch floats instead of RFC-3339 date-time strings.
AWS: Smithy CreateGatewayRateLimit.output.createdAt and .updatedAt target DateTimestamp with `timestampFormat: date-time` and are @required, so the wire value is an ISO-8601/RFC-3339 string such as `2026-11-03T09:45:45.701543537Z`. The variable part is the instant itself (service clock at creation); the format is the requirement.
Emulator: emits a bare JSON number (seconds since the epoch, e.g. 1794284882.6078033) for both members.
Why: an SDK that parses a date-time-formatted member rejects or mis-parses a number, so CreateGatewayRateLimit fails client-side deserialization; anything comparing or displaying the timestamp breaks. This is the same serialization defect the already-open issue 20261108T204036Z-bedrock-agentcore-control-policy-policy-engine-and-gateway-rate-limit-timestamps.md reports for BatchPutGatewayRateLimits/GetGatewayRateLimit, which does not cover the CreateGatewayRateLimit and update-read clusters filed here.
Fix: the gateway-rate-limit output serializer (timestamp formatting for DateTimestamp members).
Traces: acc-gateway-rate-limit-batch-and-update#task#5, bedrock-agentcore-control-gateway-rate-limit-workflow#task#5.

Cause 2 of 3: UpdateGatewayRateLimit's new description and entries are not persisted.
AWS: Smithy UpdateGatewayRateLimit (PATCH /gateways/{gatewayIdentifier}/rate-limits/{rateLimitId}) takes an optional `description` and a @required `entries` (LimitEntries) and returns the updated resource; a following GetGatewayRateLimit reflects the new description and the new per-entry `requests[].rate` / `requests[].period`, and `updatedAt` advances past `createdAt`.
Emulator: after the update, GetGatewayRateLimit still reports the description, rate and period supplied at CREATE time, and `updatedAt` equals `createdAt` — the PATCH body is accepted and discarded.
Why: a client that relaxes or tightens a gateway's rate limit sees its change acknowledged but never applied, so the limit it believes is in force is not the one stored, and a read-after-write check on the limit is silently wrong.
Fix: the UpdateGatewayRateLimit handler's write into gateway-rate-limit state (description + entries + updatedAt).
Traces: acc-gateway-rate-limit-batch-and-update#task#8.

Cause 3 of 3: DeleteGatewayRateLimit does not remove the rate limit — GetGatewayRateLimit keeps answering 200.
AWS: Smithy DeleteGatewayRateLimit (DELETE /gateways/{gatewayIdentifier}/rate-limits/{rateLimitId}) removes the limit; GetGatewayRateLimit then answers HTTP 404 ResourceNotFoundException with the recorded message `Rate limit not found: <rateLimitId>` (fixed text `Rate limit not found: `, variable part the rateLimitId from the request path).
Emulator: GetGatewayRateLimit answers HTTP 200 with the full rate-limit body (status ACTIVE) for 60 poll attempts after the delete; no error is ever produced.
Why: deletion is not durable, so a client cannot free the rate-limit id, cleanup loops that poll for the 404 never terminate, and a limit the operator removed is still reported as enforced.
Fix: the DeleteGatewayRateLimit handler's removal from gateway-rate-limit state (and the Get path's existence check).
Traces: bedrock-agentcore-control-gateway-rate-limit-workflow#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (296 steps, 143 passed, score 48.3%).

Cause 1 — CreateGatewayRateLimit, expected 201 / got 201:
  $.createdAt: expected an RFC-3339 string ('2026-11-03T09:45:45.701543537Z' in the recording), got 1794284882.6078033
  $.updatedAt: same defect
Cause 2 — GetGatewayRateLimit after UpdateGatewayRateLimit, expected 200 / got 200:
  $.description: expected the updated description ('acc per-target rate limit, relaxed to a per-minute budget'), got the create-time description ('acc per-target rate limit')
  $.entries[0].requests[0].period: expected 'minute', got 'second'
  $.entries[0].requests[0].rate: expected 250.0, got 100.0
  $.createdAt/$.updatedAt: epoch floats, and updatedAt did not advance
Cause 3 — GetGatewayRateLimit after DeleteGatewayRateLimit: status expected 404, got 200;
  $.message: missing (expected 'Rate limit not found: <rateLimitId>', got None);
  $headers.x-amzn-errortype: missing; poll: status 200 not in [404] after 60 attempts, body still {status: 'ACTIVE', ...}

Model citations: Smithy CreateGatewayRateLimit.output.createdAt/updatedAt -> DateTimestamp, timestampFormat 'date-time', @required; UpdateGatewayRateLimit PATCH /gateways/{gatewayIdentifier}/rate-limits/{rateLimitId} with @required entries; DeleteGatewayRateLimit DELETE on the same URI; GetGatewayRateLimit errors include ResourceNotFoundException (404); GatewayRateLimitStatus enum = [CREATING, ACTIVE, UPDATING, DELETING].

## Steps to Reproduce
1. CreateGateway, then CreateGatewayRateLimit on it with dimensionKeys=['targetName'] and one entry -> 201 whose createdAt/updatedAt are JSON numbers instead of RFC-3339 strings.
2. UpdateGatewayRateLimit on that rateLimitId with a new description and entries[0].requests[0] = {rate: 250, period: 'minute'}, then GetGatewayRateLimit -> the create-time description, rate 100 and period 'second' are still returned and updatedAt has not advanced.
3. DeleteGatewayRateLimit on that rateLimitId, then poll GetGatewayRateLimit -> 200 with status ACTIVE forever instead of 404 ResourceNotFoundException 'Rate limit not found: <rateLimitId>'.
Replay: fixture acc-gateway-rate-limit-batch-and-update step task#5, plus the fixture#step bindings listed in trace_ids.
