Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.update-memory-invalid-state-transition-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.update-memory-invalid-state-transition-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 3bf1cda674b4c141ca585fcc6237b428546bb56b

# UpdateMemory's "namespaceKeys still referenced by a strategy" ValidationException message is emulator-authored, not AWS's sentence

> _Note: this issue was identified against the code base at commit `3bf1cda674b4c141ca585fcc6237b428546bb56b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMemory

## Description
1 cause must be fixed before this issue is closed.

Cause 1 of 1: UpdateMemory refuses the correct request with the correct status and error code, but replaces AWS's refusal sentence with the emulator's own wording (and leaks the strategy's generated id).

AWS: Smithy UpdateMemory (PUT /memories/{memoryId}/update, success code 202) models ValidationException with httpError 400 among its errors; UpdateMemoryInput.namespaceKeys targets NamespaceKeysList (@length max 5) and REPLACES the full namespaceKeys set rather than merging it. Recorded behavior: when the replacement set omits a namespace key that is still referenced by an existing memory strategy's namespaceTemplate, AWS answers HTTP 400 ValidationException with the message template "Validation failed during UpdateMemory: [<keys>] namespaceKeys are still in reference with existing strategies, please update the strategies before proceeding with this update". The fixed text is "Validation failed during UpdateMemory: " and " namespaceKeys are still in reference with existing strategies, please update the strategies before proceeding with this update"; the only variable part is the bracketed list of the still-referenced namespace keys, taken from the memory's current namespaceKeys intersected with the keys the live strategies' namespaceTemplates mention (in the recorded run that list was the single key the request dropped). AWS never names the referencing strategy or its id in this message.

Emulator: returns the same 400/ValidationException but with a sentence of its own authorship — "namespaceKeys: the namespace variable <key> is referenced by a namespaceTemplate of memory strategy <strategyName>-<generatedStrategyId> and cannot be omitted". None of AWS's fixed text is present: the "Validation failed during UpdateMemory: " prefix is gone, the bracketed key list is gone, the "please update the strategies before proceeding with this update" remediation clause is gone, and an extra variable — the referencing strategy's name plus the emulator's generated strategy-id suffix — is added. (The strategy name/id and key name in any one run are per-run values; the requirement is the template.)

Why: an error message is part of the operation's contract. A client that keys remediation on AWS's sentence — detecting the "Validation failed during UpdateMemory:" prefix to classify the refusal as a strategy-retarget prerequisite, or parsing the bracketed list to learn exactly which namespaceKeys it must keep or which strategies to update first — matches nothing and falls through to generic failure handling, even though the emulator refused for the right reason. The missing bracketed list also means a caller dropping several keys at once cannot learn the full set that is still referenced from one response, and the added strategy id discloses a generated identifier AWS does not put in a failed-update response.

Fix: the namespaceKeys-reference guard in the UpdateMemory handler's validation for the bedrock-agentcore-control service crate — the branch that detects a dropped namespaceKey still named by a live strategy's namespaceTemplate must compose AWS's message (prefix + bracketed list of still-referenced keys + remediation clause) instead of its own per-strategy sentence. Status (400) and code (ValidationException) are already correct and must not change; the surrounding behavior the check asserts (the rejected update leaves the memory's original namespaceKeys untouched, and the identical call succeeds once the strategy is retargeted) is already correct too.

Related open issue: 20261029T212239Z-bedrock-agentcore-control-refusal-messages-diverge-from-aws.md reports the same house-style-message problem on other bedrock-agentcore-control paths (runtime not-found, gateway duplicate-name, gateway invalid-state, Double rendering in @range messages). It does not name UpdateMemory or carry this check's key, so this path is not covered by it. 20261029T170911Z-bedrock-agentcore-control-memory-reads-ignore-updatememory-and-drop-extractionhealth-and.md concerns memory READ projection, a different cause.

Traces: bedrock-agentcore-control.update-memory-invalid-state-transition-rejected.

## Evidence
Check bedrock-agentcore-control.update-memory-invalid-state-transition-rejected [mismatch].
The check asserted code == "ValidationException" and that passed; only the message diverged:

  AssertionError: expected message 'Validation failed during UpdateMemory: [customattr] namespaceKeys are still in reference with existing strategies, please update the strategies before proceeding with this update', got 'namespaceKeys: the namespace variable customattr is referenced by a namespaceTemplate of memory strategy sim2realStrategy-5gxlkk0i1l and cannot be omitted'

(check_bedrock_agentcore_control_update_memory_invalid_state_transition_rejected.py line 62: assert message == expected)

Fixed text (requirement): "Validation failed during UpdateMemory: " ... " namespaceKeys are still in reference with existing strategies, please update the strategies before proceeding with this update".
Variable text (from request/state): the bracketed namespaceKeys list — here "[customattr]", the key present on the memory and referenced by the strategy's namespaceTemplate "/custom/{customattr}/facts" but omitted from the replacement set.
Per-run values that are NOT part of the requirement: the memory name/id, and the strategy identifier "sim2realStrategy-5gxlkk0i1l" the emulator adds (AWS's message contains no strategy identifier at all).

Model grounding: Smithy UpdateMemory errors include ValidationException (httpError 400); UpdateMemoryInput.namespaceKeys -> NamespaceKeysList @length max 5; http { method: PUT, uri: /memories/{memoryId}/update, code: 202 }.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (check bedrock-agentcore-control.update-memory-invalid-state-transition-rejected).
2. CreateMemory with eventExpiryDuration=7, namespaceKeys=[{"key": "customattr"}] and one semanticMemoryStrategy whose namespaceTemplates = ["/custom/{customattr}/facts"]; poll GetMemory until status == ACTIVE.
3. Call UpdateMemory on that memoryId with namespaceKeys=[{"key": "unrelatedattr"}] — i.e. a full replacement that drops "customattr" while the live strategy's namespaceTemplate still references it.
4. Read the refusal: HTTP status is 400 and $.__type/Error.Code is ValidationException (both correct). Read $.message — the emulator returns 'namespaceKeys: the namespace variable customattr is referenced by a namespaceTemplate of memory strategy <strategyName>-<strategyId> and cannot be omitted' instead of 'Validation failed during UpdateMemory: [customattr] namespaceKeys are still in reference with existing strategies, please update the strategies before proceeding with this update'.
5. Positive controls that already pass and must keep passing: GetMemory after the refusal still lists "customattr"; after UpdateMemory retargets the strategy to namespaceTemplates=["/generic/facts"], the identical namespaceKeys=[{"key": "unrelatedattr"}] call succeeds and GetMemory eventually shows only "unrelatedattr".
