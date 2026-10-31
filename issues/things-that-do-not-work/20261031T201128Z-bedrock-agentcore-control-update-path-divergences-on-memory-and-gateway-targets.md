Issue-Source: trace-sweep-divergence
Issue-Trace: acc-memory-update-and-strategy-round-trip
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-memory-update-and-strategy-round-trip acc-gateway-update-and-target-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|UpdateMemory|2xx->2xx|$.memory.strategies[*].status
Issue-Key: bedrock-agentcore-control|GetMemory|2xx->2xx|$.memory.strategies[*].extractionHealth
Issue-Key: bedrock-agentcore-control|UpdateMemory|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agentcore-control|UpdateGatewayTarget|4xx->4xx|status
Issue-Related: 20261029T170911Z-bedrock-agentcore-control-memory-reads-ignore-updatememory-and-drop-extractionhealth-and.md
Issue-Commit: c58e269113894f48dd3e032b4127d0ccc7e88163

# Update-path divergences on memory and gateway targets: wrong strategy status, missing extractionHealth, and invented refusals

> _Note: this issue was identified against the code base at commit `c58e269113894f48dd3e032b4127d0ccc7e88163`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMemory

## Description
The memory and gateway-target update paths diverge on what they return and on how they refuse: the strategy added by UpdateMemory is reported in a transitional state, memory reads omit the extraction-health object at both levels, and the refusals are the wrong error class or carry emulator-authored text. 5 causes must all be fixed before this issue is closed. Related open issues that do NOT cover these steps: 20261029T170911Z-bedrock-agentcore-control-memory-reads-ignore-updatememory-and-drop-extractionhealth-and.md (its replay pointer names fixtures without steps and its cause 2 is about `strategies` being absent, not about extractionHealth inside a present strategy) and 20261029T170911Z-bedrock-agentcore-control-not-found-and-invalid-state-error-messages-are.md (covers the GetMemory/GetGateway/DeleteGateway/GetHarness/GetAgentRuntime not-found texts, not the UpdateMemory or UpdateGatewayTarget ones below).

Cause 1 of 5: UpdateMemory returns a newly added memory strategy as CREATING where AWS returns it ACTIVE.
AWS: UpdateMemory (Smithy: PUT /memories/{memoryId}/update, HTTP 202; input `memoryStrategies` -> ModifyMemoryStrategies; output `memory` -> Memory, whose `strategies` -> MemoryStrategyList holds MemoryStrategy items with `status` -> MemoryStrategyStatus, values CREATING|ACTIVE|DELETING|FAILED). The recording shows the 202 response to the strategy-adding update already reporting the strategy's `status` as ACTIVE — the added strategy is usable when the update returns.
Emulator: answers 202 with $.memory.strategies[0].status 'CREATING'.
Why: a client that adds a strategy and gates on ACTIVE in the update response never gets the go-ahead, so the strategy round-trip stalls on a state AWS does not expose here.
Fix: the UpdateMemory handler's strategy state transition and the MemoryStrategy status projection in its response serializer.
Traces: acc-memory-update-and-strategy-round-trip#task#8.

Cause 2 of 5: GetMemory omits the extraction-health object on the memory AND on each strategy.
AWS: recorded behavior only — `extractionHealth` is not present in the on-disk Memory/MemoryStrategy shapes, and the recording shows GetMemory (Smithy: GET /memories/{memoryId}/details, HTTP 200, output `memory` -> Memory required) returning `memory.extractionHealth` with `failedMemoryExtractionJobCount` 0 and, for every entry of `memory.strategies`, that strategy's own `extractionHealth` with `failedMemoryExtractionJobCount` 0 on a healthy memory.
Emulator: answers 200 with both $.memory.extractionHealth and $.memory.strategies[0].extractionHealth absent (the strategy entry itself is now present, so this is purely the health object being dropped).
Why: a client that reads extraction health to decide whether long-term extraction is succeeding finds the field missing at both levels and cannot make the decision at all; code that dereferences failedMemoryExtractionJobCount faults.
Fix: the Memory/MemoryStrategy serializer shared by the memory read handlers, which must always emit extractionHealth at the memory level and inside each strategy.
Traces: acc-memory-update-and-strategy-round-trip#task#9.

Cause 3 of 5: UpdateMemory's not-found message is emulator-authored instead of AWS's sentence.
AWS: UpdateMemory errors include ResourceNotFoundException (Smithy: errorKind client, httpError 404, member `message` -> NonBlankString). Updating a memoryId that does not exist is refused 404 ResourceNotFoundException with the message "Resource not found during UpdateMemory: Failed to retrieve memory item for ID: <memoryId>" — the fixed text is "Resource not found during " <operation name> ": Failed to retrieve memory item for ID: " and the only variable parts are the operation name and the `memoryId` taken from the request path.
Emulator: answers 404 with "Failed to retrieve memory because it doesn't exist. Retry the request with a different resource identifier." — a different sentence that names neither the operation nor the requested id.
Why: the client cannot tell which identifier was rejected, and message-matching or log-scraping on AWS's wording fails.
Fix: the UpdateMemory handler's not-found error construction (message template).
Traces: acc-memory-update-and-strategy-round-trip#task#4.

Cause 4 of 5: UpdateGatewayTarget resolves the target before validating the body, so a request missing credentialProviderConfigurations gets 404 ResourceNotFoundException instead of 400 ValidationException.
AWS: UpdateGatewayTarget (Smithy: PUT /gateways/{gatewayIdentifier}/targets/{targetId}/, HTTP 202; errors include ValidationException httpError 400 and ResourceNotFoundException httpError 404; `credentialProviderConfigurations` -> CredentialProviderConfigurations @length 1..1, required on the output shape). Input validation runs first: an update whose body omits the credential-provider configurations is refused 400 with the message "Credential provider configurations is not defined" — fixed text with no variable parts — regardless of whether the addressed target exists.
Emulator: answers 404 with x-amzn-errortype 'ResourceNotFoundException' and the message "Failed to update target because it doesn't exist. Retry the request with a different resource identifier.", i.e. the wrong HTTP status, the wrong error code and the wrong message, because the target lookup is evaluated ahead of body validation.
Why: a client gets a retry-with-a-different-identifier signal for what is actually a malformed request, so it chases a non-existent resource instead of fixing the missing field; 404-vs-400 also changes retry classification.
Fix: ordering and the missing required-member check in the UpdateGatewayTarget handler's request validation, which must precede target resolution.
Traces: acc-gateway-update-and-target-errors#task#7.

Cause 5 of 5: UpdateGatewayTarget's genuine not-found message uses "update" where AWS words it "retrieve".
AWS: for a target that does not exist, UpdateGatewayTarget returns 404 ResourceNotFoundException with the message "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier." — entirely fixed text, with no values taken from the request; AWS words it as a failed retrieve because the lookup is what failed.
Emulator: answers 404 (status and error class correct) but with "Failed to update target because it doesn't exist. Retry the request with a different resource identifier." — the verb is substituted from the operation name instead of using the modeled sentence.
Why: clients and fixtures that match AWS's exact refusal text see an unrecognised message, and the per-operation verb substitution means every update-style operation produces a sentence AWS never emits.
Fix: the gateway-target not-found error construction (shared message template) in the UpdateGatewayTarget handler.
Traces: acc-gateway-update-and-target-errors#task#8.

## Evidence
Cause 1 — acc-memory-update-and-strategy-round-trip#task#8, UpdateMemory, expected 202 / actual 202: $.memory.strategies[0].status value_mismatch (expected 'ACTIVE', got 'CREATING').
Cause 2 — acc-memory-update-and-strategy-round-trip#task#9, GetMemory, 200 vs 200: $.memory.extractionHealth missing (expected {'failedMemoryExtractionJobCount': 0}, got None) and $.memory.strategies[0].extractionHealth missing (expected {'failedMemoryExtractionJobCount': 0}, got None).
Cause 3 — acc-memory-update-and-strategy-round-trip#task#4, UpdateMemory, 404 vs 404: $.message value_mismatch — expected "Resource not found during UpdateMemory: Failed to retrieve memory item for ID: <memoryId>", got "Failed to retrieve memory because it doesn't exist. Retry the request with a different resource identifier." The recorded run's id was sim2realprobeaccnopef4cd2ee7-AAAAAAAAAA; that value fills the template and is not itself the requirement.
Cause 4 — acc-gateway-update-and-target-errors#task#7, UpdateGatewayTarget: status expected 400, got 404; $.message expected 'Credential provider configurations is not defined', got "Failed to update target because it doesn't exist. Retry the request with a different resource identifier."; $headers.x-amzn-errortype got 'ResourceNotFoundException' where a ValidationException was required.
Cause 5 — acc-gateway-update-and-target-errors#task#8, UpdateGatewayTarget, 404 vs 404: $.message value_mismatch (expected "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier.", got "Failed to update target because it doesn't exist. Retry the request with a different resource identifier.") — the only difference is the fixed verb, which is part of AWS's fixed text.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture acc-memory-update-and-strategy-round-trip, step task#8 — UpdateMemory adding a SEMANTIC memory strategy; inspect $.memory.strategies[0].status in the 202 body (must be ACTIVE).
3. Cause 2: same fixture, step task#9 — GetMemory on that memory; check $.memory.extractionHealth and $.memory.strategies[0].extractionHealth are present with failedMemoryExtractionJobCount.
4. Cause 3: same fixture, step task#4 — UpdateMemory against a memoryId that was never created; compare the 404 body's $.message with "Resource not found during UpdateMemory: Failed to retrieve memory item for ID: <memoryId>".
5. Cause 4: fixture acc-gateway-update-and-target-errors, step task#7 — UpdateGatewayTarget with no credentialProviderConfigurations in the body; expect 400 ValidationException with 'Credential provider configurations is not defined'.
6. Cause 5: same fixture, step task#8 — UpdateGatewayTarget on a target id that does not exist; compare the 404 body's $.message with "Failed to retrieve target because it doesn't exist. Retry the request with a different resource identifier.".
