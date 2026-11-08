Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at|run#30|AssertionError|assert first_status == "FAILED", (
Issue-Commit: 05b9742e94081dbaa46c2640ffce6702ba6a2430

# CreateGatewayTarget settles every mcp.mcpServer target READY with no statusReasons — the live tool fetch is never attempted, so a non-MCP endpoint is never FAILED

> _Note: this issue was identified against the code base at commit `05b9742e94081dbaa46c2640ffce6702ba6a2430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGatewayTarget

## Description
One net-new root cause in bedrock-agentcore-control's gateway-target state machine.

Cause 1 of 1: an `mcp.mcpServer` gateway target is settled READY unconditionally, because the emulator never performs (nor simulates) the target's tool-definition fetch against the configured `endpoint`.

AWS: Smithy (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json) — CreateGatewayTarget is a 202-accepted, asynchronous create whose output/`GetGatewayTarget` output carry `status` (@required, enum TargetStatus: CREATING | UPDATING | UPDATE_UNSUCCESSFUL | DELETING | READY | FAILED | SYNCHRONIZING | SYNCHRONIZE_UNSUCCESSFUL | CREATE_PENDING_AUTH | UPDATE_PENDING_AUTH | SYNCHRONIZE_PENDING_AUTH) and `statusReasons` (list StatusReasons, @length max 100, member StatusReason @length max 2048 — documented as "The reasons for the current status of the target"). `TargetConfiguration`'s `mcp` union member `mcpServer` (McpServerTargetConfiguration) is the one MCP target variant that carries no static schema: unlike `mcp.openApiSchema`/`mcp.smithyModel` (inline or S3 schema) and `http.passthrough`, its tool list can only be learned by contacting the endpoint. Recorded behavior: a target created with `targetConfiguration={"mcp":{"mcpServer":{"endpoint":"<a reachable HTTPS URL that is not an MCP server>"}}}` leaves CREATING and settles in terminal status FAILED, with at least one `statusReasons` entry describing the failed dial — the recording's wording varies per run (it quotes how the endpoint answered), and the check accepts any reason containing the fixed fragment "initialize handshake" or "did not respond"; the variable part is the endpoint/transport detail of that particular dial. The same recording confirms the fetch is fired by CreateGatewayTarget itself, before any explicit SynchronizeGatewayTargets. (The other assertions this check makes — SynchronizeGatewayTargets refusing PASSTHROUGH and OPEN_API_SCHEMA targets with ValidationException, the unknown-target ResourceNotFoundException, and `lastSynchronizedAt` staying unset after a refusal — all already match; only the mcpServer status outcome diverges.)

Emulator: every created target is written with `transient_status = CREATING`, `settled_status = STATUS_READY` and `settled_status_reasons = Vec::new()` regardless of which `targetConfiguration` variant was supplied, so after the clock-owed settle delay GetGatewayTarget answers `status="READY"` with empty/absent `statusReasons` for an `mcp.mcpServer` target whose endpoint is not an MCP server. The only configuration-dependent check performed at create time is the Lambda-existence resolution for `mcp.lambda`; `mcp.mcpServer.endpoint` is stored and never exercised. Replay: `AssertionError: expected the bogus mcp.mcpServer target's first live-fetch attempt to leave it FAILED, got 'READY' statusReasons=''`.

Why: a client that polls GetGatewayTarget until the target leaves CREATING uses READY as the signal that the target's tools were successfully discovered and the target is usable. Against the emulator that signal is unconditional, so a misconfigured or unreachable MCP server endpoint is reported healthy: code that branches on FAILED and reads `statusReasons` to surface a diagnostic (or to retry/rollback) is never exercised and silently takes the success path, and no MCP-endpoint misconfiguration is detectable at all. The FAILED/statusReasons half of the modeled TargetStatus enum is unreachable for this target type.

Fix: the CreateGatewayTarget handler and the gateway-target state that backs it (services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_gateway_target.rs, with the status/settle fields and helpers in that crate's src/state.rs) must choose the target's settled status from the `targetConfiguration` variant instead of hard-coding READY: an `mcp.mcpServer` target must have its tool-definition fetch attempted against the configured endpoint and, when that fetch does not complete an MCP initialize handshake, settle FAILED with a non-empty `statusReasons` entry worded as a failed handshake/no-response against that endpoint. The same decision must back GetGatewayTarget's and SynchronizeGatewayTargets' reported status for the target.

Traces: bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at.

## Evidence
Check: bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at [mismatch]

Expected (recorded AWS): CreateGatewayTarget with targetConfiguration={"mcp":{"mcpServer":{"endpoint":"<non-MCP HTTPS URL>"}}} is accepted (202), the target leaves CREATING and reaches terminal status "FAILED", and GetGatewayTarget reports statusReasons containing a dial-failure reason (fixed fragment "initialize handshake" or "did not respond"; the remainder is the per-dial transport detail).

Actual (emulator): the target settles status="READY" and statusReasons is empty.

Replay failure:
  File "check_bedrock_agentcore_control_synchronize_gateway_targets_advances_last_synchronized_at.py", line 141, in run
    assert first_status == "FAILED", (
  AssertionError: expected the bogus mcp.mcpServer target's first live-fetch attempt to leave it FAILED, got 'READY' statusReasons=''

Source of the unconditional status, services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/create_gateway_target.rs:323-351 — GatewayTargetRecord is built with transient_status = STATUS_CREATING, settled_status = STATUS_READY, settled_status_reasons = Vec::new() for every targetConfiguration; the only variant-specific handling is the mcp.lambda existence check at lines 290-300.

Model: TargetStatus includes FAILED; GetGatewayTargetResponse.statusReasons is StatusReasons (max 100 entries, each max 2048 chars). TargetConfiguration.mcp union includes mcpServer (McpServerTargetConfiguration), the only MCP variant with no static schema.

Not filed here (not emulator-caused): the exact text of the dial-failure reason, which varies per run in the recording and is only matched by fragment.

## Steps to Reproduce
1. Create an IAM role trusted by bedrock-agentcore.amazonaws.com and call CreateGateway with authorizerType=AWS_IAM; poll GetGateway until status==READY.
2. Call CreateGatewayTarget on that gateway with:
   targetConfiguration={"mcp": {"mcpServer": {"endpoint": "https://example.com/sim2real-mcp"}}}
   credentialProviderConfigurations=[{"credentialProviderType": "GATEWAY_IAM_ROLE", "credentialProvider": {"iamCredentialProvider": {"service": "execute-api"}}}]
   (the endpoint is a reachable HTTPS URL that is not an MCP server).
3. Poll GetGatewayTarget(gatewayIdentifier, targetId) until status is terminal (one of READY, FAILED, UPDATE_UNSUCCESSFUL, SYNCHRONIZE_UNSUCCESSFUL).
4. Observe status=="READY" with no statusReasons. AWS settles this target FAILED with a statusReasons entry describing the failed MCP initialize handshake / unanswered dial.

Replay: run behavioral check bedrock-agentcore-control.synchronize-gateway-targets-advances-last-synchronized-at.
