Issue-Source: trace-sweep-divergence
Issue-Trace: acc-gateway-update-and-target-errors
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: acc-gateway-update-and-target-errors bedrock-agentcore-control-gateway-error-paths bedrock-agentcore-control-gateway-lifecycle-basic bedrock-agentcore-control-agent-runtime-not-found-errors bedrock-agentcore-control-harness-not-found-errors bedrock-agentcore-control-memory-validation-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|UpdateGateway|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|DeleteGateway|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetAgentRuntime|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetHarness|4xx->4xx|$.message
Issue-Key: bedrock-agentcore-control|GetMemory|4xx->4xx|$.message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Not-found and invalid-state error messages are emulator-invented text instead of AWS wording

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGateway

## Description
Across gateways, agent runtimes, harnesses and memories the emulator returns the right HTTP status and error shape but a message of its own invention, in its own house style ('no <resource> <id> in this account', 'Cannot perform operation X when ...'). The message is part of each error's contract, so each resource family needs its AWS wording restored. 6 causes must all be fixed before this issue is closed.

Cause 1 of 6: UpdateGateway on a missing gateway uses the operation's own verb.
AWS: UpdateGateway models ResourceNotFoundException (Smithy: UpdateGateway errors include ResourceNotFoundException, httpError 404, member `message` -> NonBlankString). Recorded behavior: the 404 body message is the fixed text "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier." — all fixed text, no variable parts, and notably the verb is "retrieve" for every gateway operation because the lookup that fails is the gateway read, not the update.
Emulator: returns 404 with the same sentence but the operation's verb substituted: "Failed to update gateway because it doesn't exist. ...".
Why: clients and tests that match on the documented not-found sentence (or diff gateway error text across operations) see a message that varies per operation, which AWS's does not.
Fix: the gateway not-found error constructor shared by the gateway handlers — one fixed sentence, not a per-operation verb template.
Traces: acc-gateway-update-and-target-errors#task#11.

Cause 2 of 6: DeleteGateway on a missing gateway uses the operation's own verb.
AWS: DeleteGateway models ResourceNotFoundException (httpError 404). Recorded behavior: the identical fixed sentence "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier."
Emulator: returns 404 with "Failed to delete gateway because it doesn't exist. Retry the request with a different resource identifier."
Why: same contract break as cause 1, confirming the verb substitution is systematic across the gateway handlers rather than a single typo.
Fix: same shared gateway not-found error constructor, reached from the DeleteGateway handler.
Traces: bedrock-agentcore-control-gateway-error-paths#task#4.

Cause 3 of 6: DeleteGateway during CREATING reports an invalid-state message in the emulator's own phrasing.
AWS: deleting a gateway that is not in a terminal state fails with HTTP 400 (Smithy: DeleteGateway errors include ValidationException, httpError 400; ConflictException is 409 and is NOT what this path returns). Recorded behavior: message "DeleteGateway operation can't be performed on gateway when it is in Creating state. Retry the request when the gateway is in a valid state." Fixed text: "<Operation> operation can't be performed on gateway when it is in <State> state. Retry the request when the gateway is in a valid state." Variable parts taken from the request and gateway state: the operation name and the gateway's current status rendered in capitalised form ("Creating"), not the raw enum token.
Emulator: returns 400 with 'Cannot perform operation DeleteGateway when gateway is in CREATING status' — different sentence, raw enum status, and no retry guidance.
Why: a client that distinguishes a transient state conflict from a permanent validation failure by the message text cannot, and retry logic keyed on "Retry the request when the gateway is in a valid state" never triggers.
Fix: the gateway state-transition guard in the DeleteGateway handler's validation, which formats the invalid-state message.
Traces: bedrock-agentcore-control-gateway-lifecycle-basic#task#4.

Cause 4 of 6: GetAgentRuntime on a missing runtime loses the whole AWS message template.
AWS: GetAgentRuntime models ResourceNotFoundException (httpError 404). Recorded behavior: message "Agent with agentId: <agentRuntimeId>, accountId: <accountId>, and version: <agentRuntimeVersion> not found!" Fixed text: "Agent with agentId: ", ", accountId: ", ", and version: ", " not found!" — variable parts are the runtime id from the request path, the caller's account id, and the requested version, with the literal "null" when the `version` query parameter is absent.
Emulator: returns 404 with 'no agent runtime <id> in this account' — none of the template, and the account and version context dropped entirely.
Why: tooling that parses the id/account/version out of the not-found message (or asserts the trailing "not found!") gets nothing to parse.
Fix: the GetAgentRuntime handler's not-found error construction.
Traces: bedrock-agentcore-control-agent-runtime-not-found-errors#task#1.

Cause 5 of 6: GetHarness on a missing harness loses the AWS message template.
AWS: GetHarness models ResourceNotFoundException (httpError 404). Recorded behavior: message "Agent with name <harnessId> not found." Fixed text: "Agent with name " and " not found." — the variable part is the identifier taken from the request path; AWS words the harness not-found in terms of "Agent with name", which the emulator must reproduce rather than normalise.
Emulator: returns 404 with 'no harness <id> in this account'.
Why: same breakage as cause 4 for the harness family, and the emulator's wording also leaks its own internal resource naming.
Fix: the GetHarness handler's not-found error construction.
Traces: bedrock-agentcore-control-harness-not-found-errors#task#1.

Cause 6 of 6: GetMemory on a missing memory loses the AWS message template.
AWS: GetMemory models ResourceNotFoundException (Smithy: GetMemory errors include ResourceNotFoundException, httpError 404). Recorded behavior: message "Resource not found during GetMemory: Failed to retrieve memory item for ID: <memoryId>" Fixed text: "Resource not found during <Operation>: Failed to retrieve memory item for ID: " — the variable part is the memory id from the request path, and the operation name is the operation that failed.
Emulator: returns 404 with 'no memory with id <id> exists in this account'.
Why: the AWS message names the failing operation and the internal retrieval step; a client (or operator reading logs) that keys on that prefix sees an unrecognised error text.
Fix: the GetMemory handler's not-found error construction.
Traces: bedrock-agentcore-control-memory-validation-errors#task#2.

## Evidence
Every cause below matched status and error shape; only $.message diverged.
Cause 1 — acc-gateway-update-and-target-errors#task#11, UpdateGateway, 404 vs 404: expected "Failed to retrieve gateway because it doesn't exist. Retry the request with a different resource identifier.", got "Failed to update gateway because it doesn't exist. Retry the request with a different resource identifier."
Cause 2 — bedrock-agentcore-control-gateway-error-paths#task#4, DeleteGateway, 404 vs 404: expected the same "Failed to retrieve gateway ..." sentence, got "Failed to delete gateway because it doesn't exist. ..."
Cause 3 — bedrock-agentcore-control-gateway-lifecycle-basic#task#4, DeleteGateway, 400 vs 400: expected "DeleteGateway operation can't be performed on gateway when it is in Creating state. Retry the request when the gateway is in a valid state.", got 'Cannot perform operation DeleteGateway when gateway is in CREATING status'.
Cause 4 — bedrock-agentcore-control-agent-runtime-not-found-errors#task#1, GetAgentRuntime, 404 vs 404: expected 'Agent with agentId: <id>, accountId: <account>, and version: null not found!' (recorded run: id missingruntime-abcdefghij, account 000000000000), got 'no agent runtime missingruntime-abcdefghij in this account'.
Cause 5 — bedrock-agentcore-control-harness-not-found-errors#task#1, GetHarness, 404 vs 404: expected 'Agent with name <harnessId> not found.', got 'no harness sim2realprobemissing-1234567890 in this account'.
Cause 6 — bedrock-agentcore-control-memory-validation-errors#task#2, GetMemory, 404 vs 404: expected 'Resource not found during GetMemory: Failed to retrieve memory item for ID: <memoryId>', got 'no memory with id missingmemory-1234567890 exists in this account'.
The ids and account number shown are the recorded run's values filling the templates' variable slots; the fixed text is the requirement.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Cause 1: fixture acc-gateway-update-and-target-errors, step task#11 — UpdateGateway against a gateway identifier that does not exist; read $.message of the 404.
3. Cause 2: fixture bedrock-agentcore-control-gateway-error-paths, step task#4 — DeleteGateway on a missing gateway identifier.
4. Cause 3: fixture bedrock-agentcore-control-gateway-lifecycle-basic, step task#4 — DeleteGateway while the gateway is still CREATING; read $.message of the 400.
5. Cause 4: fixture bedrock-agentcore-control-agent-runtime-not-found-errors, step task#1 — GetAgentRuntime with an unknown agentRuntimeId and no version query parameter.
6. Cause 5: fixture bedrock-agentcore-control-harness-not-found-errors, step task#1 — GetHarness with an unknown harnessId.
7. Cause 6: fixture bedrock-agentcore-control-memory-validation-errors, step task#2 — GetMemory with an unknown memoryId.
