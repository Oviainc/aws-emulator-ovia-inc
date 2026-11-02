Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.agent-runtime-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.agent-runtime-lifecycle-transitions|run#24|AssertionError|assert update_message == expected_update_message, (
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# UpdateAgentRuntime's ResourceNotFoundException reuses GetAgentRuntime's not-found message template instead of AWS's UpdateAgentRuntime sentence

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAgentRuntime

## Description
The emulator answers an UpdateAgentRuntime against a runtime id that no longer exists with the correct HTTP status (404) and the correct error code (ResourceNotFoundException), but with the message template AWS uses for the *read* path (GetAgentRuntime) rather than the sentence AWS uses for the *mutation* path. 1 cause must be fixed before this issue is closed.

Cause 1 of 1: the agent-runtime not-found message is identical on GetAgentRuntime and UpdateAgentRuntime; AWS words them differently.

AWS: Smithy UpdateAgentRuntime (http PUT /runtimes/{agentRuntimeId}/, success code 202) models ResourceNotFoundException with httpError 404, alongside ConflictException (409) and ValidationException (400); its `agentRuntimeId` is an @httpLabel @required member. Smithy GetAgentRuntime (GET /runtimes/{agentRuntimeId}/) models the same ResourceNotFoundException (404). The model does not carry message text, so the wording is recorded behavior only: on the READ path the 404 message is the template "Agent with agentId: <agentRuntimeId>, accountId: <accountId>, and version: <version> not found!" (fixed text "Agent with agentId: ", ", accountId: ", ", and version: ", " not found!"; variable parts are the runtime id from the request path, the caller's account id, and the requested version rendered as the literal "null" when the version query parameter is absent), whereas on the UPDATE path the 404 message is the different, shorter template "Agent '<agentRuntimeId>' was not found. Please check the agent ID and try again." — fixed text "Agent '" and "' was not found. Please check the agent ID and try again.", with the single variable part being the runtime id taken from the request path, in single quotes, and no account id and no version in the sentence at all.

Emulator: UpdateAgentRuntime on a deleted runtime returns 404 ResourceNotFoundException whose message is the GetAgentRuntime template — e.g. "Agent with agentId: <id>, accountId: <account>, and version: null not found!" — so a mutation reports a read-shaped error, quotes an account id and a version the update request never named, and omits AWS's "Please check the agent ID and try again." guidance. The read path on the same id returns the correct template, so the two paths are indistinguishable by text where AWS distinguishes them.

Why: a client that keys its error handling on the not-found sentence (matching "Please check the agent ID and try again." to surface a user-facing "check the id" hint, or distinguishing a failed write from a failed read in logs) never matches, and a caller that parses the message sees an account id and a `version: null` field that its UpdateAgentRuntime request did not contain, implying a versioned lookup the update never performed.

Fix: the UpdateAgentRuntime handler's not-found error construction in the bedrock-agentcore-control service crate. The runtime not-found error is currently built by one constructor shared by the runtime read/update/delete handlers; the update path needs its own sentence rather than the read template, so the per-operation message must be chosen at the handler rather than in the shared constructor.

Related already-open/already-fixed issues, neither of which covers this path: 20261029T212239Z-bedrock-agentcore-control-refusal-messages-diverge-from-aws.md carries this same check id but its Cause 1 is the GetAgentRuntime read message (that assertion now passes — the check proceeds past it and fails on the later UpdateAgentRuntime assertion), and the fixed issue 20261029T170911Z-bedrock-agentcore-control-not-found-and-invalid-state-error-messages-are.md named only GetAgentRuntime/GetHarness/GetMemory/UpdateGateway-not-found in its causes. Neither names the UpdateAgentRuntime message.

Traces: bedrock-agentcore-control.agent-runtime-lifecycle-transitions.

## Evidence
Check bedrock-agentcore-control.agent-runtime-lifecycle-transitions, verdict [mismatch]. The check creates a runtime, waits for READY, updates it, deletes it, asserts the GetAgentRuntime 404 message (this assertion PASSES — the emulator emits the correct "Agent with agentId: ..., accountId: ..., and version: null not found!" template), then issues UpdateAgentRuntime on the same dead id. Both the status and the error code are right (404 / ResourceNotFoundException — the check got past its `update_code == "ResourceNotFoundException"` assertion); only the message diverged, at check_bedrock_agentcore_control_agent_runtime_lifecycle_transitions.py line 90:

  AssertionError: expected message "Agent '<agentRuntimeId>' was not found. Please check the agent ID and try again.", got 'Agent with agentId: <agentRuntimeId>, accountId: <accountId>, and version: null not found!'

In the recorded run the variable slots were filled with agentRuntimeId 'sim2realartc8878fd89dd7-d7k5mt9j09' and accountId '000000000000'; those are per-run generated values filling the templates, not part of the requirement. The requirement is the fixed text of the UpdateAgentRuntime sentence plus the runtime id from the request path as its one variable part.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (check bedrock-agentcore-control.agent-runtime-lifecycle-transitions).
2. CreateAgentRuntime with an agentRuntimeName, a code artifact, a role ARN and networkConfiguration {networkMode: PUBLIC}; poll GetAgentRuntime until status leaves CREATING (expect READY, agentRuntimeVersion '1').
3. UpdateAgentRuntime with a new artifact; poll until status leaves UPDATING.
4. DeleteAgentRuntime on that agentRuntimeId, and poll GetAgentRuntime until it answers 404 ResourceNotFoundException — read $.message and observe the correct "Agent with agentId: <id>, accountId: <account>, and version: null not found!" template.
5. Call UpdateAgentRuntime on the same (now deleted) agentRuntimeId with the same artifact, roleArn and networkConfiguration, and read $.message of the 404: the emulator repeats the read template from step 4 instead of "Agent '<id>' was not found. Please check the agent ID and try again.".
