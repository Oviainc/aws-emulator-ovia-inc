Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-agent-runtime-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-agent-runtime-lifecycle bedrock-agentcore-control-gateway-target-workflow
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|CreateAgentRuntime|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|CreateAgentRuntime|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateAgentRuntime|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|CreateGatewayTarget|4xx->2xx|$.message
Issue-Key: bedrock-agentcore-control|CreateGatewayTarget|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|CreateGatewayTarget|4xx->2xx|status
Issue-Key: bedrock-agentcore-control|ListAgentRuntimeVersionsByCapacityProvider|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|ListAgentRuntimeVersionsByCapacityProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|ListAgentRuntimeVersionsByCapacityProvider|4xx->4xx|status
Issue-Key: bedrock-agentcore-control|ListAgentRuntimes|2xx->2xx|$.agentRuntimes[*]
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Agent-runtime and gateway-target control-plane rejections are missing: unassumable role and CREATING-state target accepted, capacity-provider denial misclassified

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentRuntime

## Description
The agent-runtime and gateway-target control plane skips the pre-conditions AWS enforces: it accepts a CreateAgentRuntime whose execution role cannot be assumed, accepts CreateGatewayTarget against a gateway that is still CREATING, and answers the capacity-provider denial with the wrong status and error code. The accepted runtime then shows up in ListAgentRuntimes, where AWS has nothing to list. 4 causes.

Cause 1 of 4: CreateAgentRuntime accepts a role AWS refuses.
AWS: Smithy CreateAgentRuntime models ValidationException (httpError 400) and the recording shows a 400 with x-amzn-errortype ValidationException when the roleArn names a role that does not exist or whose trust policy does not allow this service (recorded message example: "Role validation failed for 'arn:aws:iam::...:role/sim2real-infra-lambda-exec-role'. Please verify that the role exists and its trust policy allows assumption by this service"). The requirement is the 400/ValidationException refusal; the wording is an example.
Emulator: answers 202 with a created runtime, no $.message and no x-amzn-errortype header.
Why: a client that relies on CreateAgentRuntime to validate its execution role gets a 'successful' runtime that could never run on AWS, so a misconfigured role is never surfaced and the failure is deferred to code that does not exist in the emulator.
Fix: the CreateAgentRuntime handler's input/pre-condition validation - resolve roleArn against IAM state and raise ValidationException (400) when it is absent or not assumable.
Traces: bedrock-agentcore-control-agent-runtime-lifecycle#task#1 (representative).

Cause 2 of 4: CreateGatewayTarget is accepted while the gateway is CREATING.
AWS: Smithy CreateGatewayTarget models ValidationException (httpError 400); the recording shows 400 / x-amzn-errortype ValidationException with the message "Cannot perform operation CreateGatewayTarget when gateway is in CREATING status" - the requirement is the 400 ValidationException for a gateway not yet READY.
Emulator: answers 202 and creates the target, with no $.message and no x-amzn-errortype.
Why: clients lose the signal that they must wait for the gateway to become READY, and targets get attached to gateways in a state AWS forbids - divergent state from then on.
Fix: the CreateGatewayTarget handler must gate on the parent gateway's status before creating the target.
Traces: bedrock-agentcore-control-gateway-target-workflow#task#2.

Cause 3 of 4: ListAgentRuntimeVersionsByCapacityProvider answers 400 ValidationException where AWS answers 403 AccessDeniedException.
AWS: Smithy ListAgentRuntimeVersionsByCapacityProvider models both AccessDeniedException (403) and ValidationException (400); for the recorded capacityProviderId the service answers 403 with x-amzn-errortype AccessDeniedException (recorded message example: 'Request denied: capacityProviderId does not match the required format with an explicit deny in a resource-based policy').
Emulator: 400 with x-amzn-errortype ValidationException and a pattern-constraint message.
Why: status and error code are the contract a client branches on - 403 AccessDenied is a permissions failure an SDK surfaces and may not retry, while 400 ValidationException tells the caller its input was malformed. Error-handling code takes the wrong branch.
Fix: the ListAgentRuntimeVersionsByCapacityProvider handler's rejection classification for capacityProviderId.
Traces: bedrock-agentcore-control-agent-runtime-lifecycle#task#2.

Cause 4 of 4: ListAgentRuntimes returns a runtime that should not exist.
AWS: ListAgentRuntimes (Smithy: POST /runtimes/, output agentRuntimes required) lists the account's runtimes; in the recording the preceding CreateAgentRuntime was refused (cause 1), so the list is empty.
Emulator: returns one agentRuntimes entry (status CREATING) for the runtime it wrongly accepted - reported as '$.agentRuntimes[0]: extra'. This is downstream of cause 1 but is its own replayed step, so it must be re-replayed after the fix.
Why: list output no longer matches the create contract; a client enumerating runtimes sees a resource AWS never created.
Fix: follows from cause 1 (nothing should be stored for a rejected create); confirm ListAgentRuntimes projection after that handler is fixed.
Traces: bedrock-agentcore-control-agent-runtime-lifecycle#task#3.

## Evidence
Cause 1 (bedrock-agentcore-control-agent-runtime-lifecycle#task#1): expected status 400 with $.message "Role validation failed for 'arn:aws:iam::000000000000:role/sim2real-infra-lambda-exec-role'. Please verify that the role exists and its trust policy allows assumption by this service" and $headers.x-amzn-errortype 'ValidationException'; actual 202, no message, no errortype header.
Cause 2 (bedrock-agentcore-control-gateway-target-workflow#task#2): expected 400 / x-amzn-errortype ValidationException / $.message 'Cannot perform operation CreateGatewayTarget when gateway is in CREATING status'; actual 202, no message, no errortype header.
Cause 3 (bedrock-agentcore-control-agent-runtime-lifecycle#task#2): expected 403 / x-amzn-errortype AccessDeniedException; actual 400 / ValidationException with "1 validation error detected: Value 'sim2realnonexistentcp0000000000000000000' at 'capacityProviderId' failed to satisfy constraint ...".
Cause 4 (bedrock-agentcore-control-agent-runtime-lifecycle#task#3): status 200 both sides; $.agentRuntimes[0] extra, actual {agentRuntimeId: sim2realprobeagentrt638e555b-chfltxb5uz, agentRuntimeVersion: '1', status: 'CREATING', ...}, expected no entry.
Model: CreateAgentRuntime.errors includes ValidationException(400); CreateGatewayTarget.errors includes ValidationException(400); ListAgentRuntimeVersionsByCapacityProvider.errors includes AccessDeniedException(403) and ValidationException(400).

## Steps to Reproduce
1. Replay bedrock-agentcore-control-agent-runtime-lifecycle task#1: PUT /runtimes/ with roleArn 'arn:aws:iam::000000000000:role/sim2real-infra-lambda-exec-role' (not an assumable AgentCore role) - the emulator returns 202 instead of 400 ValidationException.
2. Same fixture task#2: POST /capacity-providers/{capacityProviderId}/runtime-versions with a capacityProviderId that AWS denies - the emulator returns 400 ValidationException instead of 403 AccessDeniedException.
3. Same fixture task#3: POST /runtimes/ to list - the emulator lists the runtime accepted in step 1.
4. Replay bedrock-agentcore-control-gateway-target-workflow task#2: CreateGateway, then immediately POST /gateways/{id}/targets/ while the gateway is CREATING - the emulator returns 202 instead of 400 ValidationException.
