Issue-Source: trace-sweep-divergence
Issue-Trace: acc-gateway-rate-limit-batch-and-update
Issue-Step: teardown#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-gateway-rate-limit-batch-and-update acc-gateway-update-and-target-errors bedrock-agentcore-control-gateway-get-returns-metadata bedrock-agentcore-control-gateway-rate-limit-workflow bedrock-agentcore-control-gateway-lifecycle-basic
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|GetGateway|4xx->2xx|$
Issue-Key: bedrock-agentcore-control|GetGateway|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|GetGateway|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|GetGateway|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.createdAt
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.description
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.exceptionLevel
Issue-Key: bedrock-agentcore-control|GetGateway|2xx->2xx|$.updatedAt
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Gateway state mutations are not persisted: DeleteGateway leaves the gateway readable and UpdateGateway drops field changes

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGateway

## Description
Bedrock AgentCore Control keeps gateway records immutable: a deleted gateway stays readable forever and an UpdateGateway call does not change the stored gateway. 3 causes.

Cause 1 of 3: GetGateway still answers 200 READY after DeleteGateway (rate-limit/target fixtures).
AWS: GetGateway is modeled GET /gateways/{gatewayIdentifier}/ with ResourceNotFoundException (httpError 404) among its errors (Smithy: GetGateway.errors includes ResourceNotFoundException -> 404). After a successful DeleteGateway the recorded behaviour is that GetGateway polling converges on 404 ResourceNotFoundException, which is how the fixtures' teardown phases terminate.
Emulator: GetGateway keeps returning 200 with the full gateway body (status READY, same gatewayId/gatewayArn) for the whole 60s poll window; the replay reports 'status: expected 404, got 200' and 'poll: status 200 not in [404] after 13 attempt(s)'. This cluster covers 4 fixture steps (acc-gateway-rate-limit-batch-and-update#teardown#15, acc-gateway-update-and-target-errors#teardown#13, bedrock-agentcore-control-gateway-get-returns-metadata#teardown#5, bedrock-agentcore-control-gateway-rate-limit-workflow#teardown#10).
Why: a client that deletes a gateway and waits for it to disappear never finishes; waiters and cleanup loops hang, and a re-create of the same name sees a resource that AWS would have removed.
Fix: the DeleteGateway handler and the gateway state store - deletion must remove (or tombstone) the record so GetGateway resolves to ResourceNotFoundException.
Traces: acc-gateway-rate-limit-batch-and-update#teardown#15 (representative).

Cause 2 of 3: same non-deletion on the basic lifecycle fixture, where the recorded 404 body is also asserted.
AWS: after DeleteGateway, GetGateway answers 404 with the ResourceNotFoundException error type (recorded message example: "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier.").
Emulator: 200 with the live gateway body; $.message missing and no x-amzn-errortype header at all.
Why: the client sees neither the 404 status nor any error type, so the SDK hands back a success object for a deleted resource.
Fix: same as cause 1 (DeleteGateway handler / gateway state), plus the ResourceNotFoundException path of GetGateway.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#teardown#7.

Cause 3 of 3: UpdateGateway does not apply description or exceptionLevel.
AWS: UpdateGateway (Smithy: PUT /gateways/{gatewayIdentifier}/, input members description and exceptionLevel) replaces the gateway configuration, and GetGateway output models both description and exceptionLevel, so a subsequent read returns the updated values (recording: description 'acc gateway after update', exceptionLevel 'DEBUG').
Emulator: the following GetGateway returns the pre-update description ('acc gateway before update') and omits exceptionLevel entirely ($.exceptionLevel missing, expected 'DEBUG'), i.e. the update is a no-op against stored state.
Why: any read-after-write on a gateway returns stale configuration, so clients that update then verify (or that rely on exceptionLevel to control error verbosity) silently keep the old behaviour.
Fix: the UpdateGateway handler must write the mutable members through to the gateway state record read by GetGateway.
Traces: acc-gateway-update-and-target-errors#task#6.

## Evidence
Cause 1 (acc-gateway-rate-limit-batch-and-update#teardown#15, cluster of 4 steps): expected status 404, actual 200 with body {gatewayId: sim2real-probe-acc-gwrl-...-fq8i3of3q1, status: READY, ...}; 'poll capped at 60s', 'poll: status 200 not in [404] after 13 attempt(s)'.
Cause 2 (bedrock-agentcore-control-gateway-lifecycle-basic#teardown#7): expected 404 + $.message "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier." + an x-amzn-errortype header; actual 200 with the live gateway body, $.message missing, x-amzn-errortype missing.
Cause 3 (acc-gateway-update-and-target-errors#task#6): status 200 both sides; $.description expected 'acc gateway after update', actual 'acc gateway before update'; $.exceptionLevel expected 'DEBUG', actual absent.

## Steps to Reproduce
1. Replay fixture acc-gateway-rate-limit-batch-and-update and inspect teardown#15: CreateGateway, DeleteGateway, then poll GetGateway - the emulator answers 200 READY for the whole window instead of converging on 404.
2. Replay bedrock-agentcore-control-gateway-lifecycle-basic teardown#7 for the same non-deletion with the 404 body asserted.
3. Replay acc-gateway-update-and-target-errors task#6: CreateGateway with description 'acc gateway before update', UpdateGateway with description 'acc gateway after update' and exceptionLevel DEBUG, then GetGateway - the response still carries the old description and no exceptionLevel.
