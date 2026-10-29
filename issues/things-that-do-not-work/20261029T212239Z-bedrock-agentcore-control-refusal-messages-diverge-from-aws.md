Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control.agent-runtime-lifecycle-transitions bedrock-agentcore-control.create-gateway-duplicate-name-conflict bedrock-agentcore-control.gateway-duplicate-name-conflict-rejected bedrock-agentcore-control.update-gateway-conflicting-name-rejected bedrock-agentcore-control.batch-put-gateway-rate-limits-negative-value-rejected bedrock-agentcore-control.create-gateway-rate-limit-exceeding-max-value-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control.agent-runtime-lifecycle-transitions|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agentcore-control.create-gateway-duplicate-name-conflict|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agentcore-control.gateway-duplicate-name-conflict-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: bedrock-agentcore-control|UpdateGateway|ValidationException
Issue-Key: bedrock-agentcore-control.batch-put-gateway-rate-limits-negative-value-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agentcore-control.create-gateway-rate-limit-exceeding-max-value-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock-agentcore-control refusal messages diverge from AWS wording on 4 paths (runtime not-found, gateway duplicate-name, gateway invalid-state, Double rendering in @range validation messages)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAgentRuntime

## Description
The emulator answers these refusals with the correct HTTP status and error code but with message text of its own authorship. An error message is part of the operation's contract, so each path needs AWS's sentence restored. 4 causes must all be fixed before this issue is closed.

Related open issue: 20261029T170911Z-bedrock-agentcore-control-not-found-and-invalid-state-error-messages-are.md describes the same house-style message problem for the WIRE corpus (its causes cover GetAgentRuntime/GetHarness/GetMemory/UpdateGateway-not-found and DeleteGateway-while-CREATING). It does not carry these behavioral check keys, and causes 2-4 below are paths it does not name at all. Cause 1 overlaps its cause 4 and can be closed by the same fix.

Cause 1 of 4: the agent-runtime not-found message loses AWS's whole template.
AWS: Smithy GetAgentRuntime errors include ResourceNotFoundException (httpError 404, member `message` -> NonBlankString). Recorded behavior: the 404 message is the template "Agent with agentId: <agentRuntimeId>, accountId: <accountId>, and version: <version> not found!" — fixed text "Agent with agentId: ", ", accountId: ", ", and version: ", " not found!"; the variable parts are the runtime id taken from the request path, the caller's account id, and the requested version, rendered as the literal "null" when no version was requested.
Emulator: 404 with 'no agent runtime <id> in this account' — none of the template, and the account and version context dropped.
Why: tooling that parses the id/account/version out of the not-found message, or asserts the trailing "not found!", gets nothing to parse, and an operator sees an error sentence that does not exist in AWS.
Fix: the agent-runtime not-found error construction shared by the runtime read/update/delete handlers in the bedrock-agentcore-control service crate.
Traces: bedrock-agentcore-control.agent-runtime-lifecycle-transitions (the same emulator sentence also surfaces as the terminal read in bedrock-agentcore-control.deploy-agent-runtime-cfn-behaves).

Cause 2 of 4: CreateGateway's duplicate-name refusal is reworded and leaks the emulator's generated id.
AWS: Smithy CreateGateway errors include ConflictException (httpError 409); CreateGatewayRequest.name documents "The name must be unique within your account." Recorded behavior: the message is "A gateway with name '<name>' already exists in this account" — fixed text "A gateway with name '" and "' already exists in this account", with the single variable part being the `name` member from the request, in single quotes.
Emulator: returns the same code with 'a gateway named <name> already exists (gatewayId <gatewayId>)' — lower-cased, unquoted name, and an extra parenthetical exposing the existing gateway's generated id, which AWS never includes.
Why: a client that matches the duplicate-name sentence to decide between retrying with a new name and surfacing the error does not recognise the text, and the leaked gatewayId is an identifier the caller was not authorized to learn from a failed create.
Fix: the duplicate-name conflict branch of the CreateGateway handler's validation.
Traces: bedrock-agentcore-control.create-gateway-duplicate-name-conflict, bedrock-agentcore-control.gateway-duplicate-name-conflict-rejected.

Cause 3 of 4: UpdateGateway's invalid-state refusal is the emulator's own phrasing with a raw enum status and no retry guidance.
AWS: Smithy UpdateGateway errors include ValidationException (httpError 400). Recorded behavior: the message is "<Operation> operation can't be performed on gateway when it is in <State> state. Retry the request when the gateway is in a valid state." — fixed text as shown; the variable parts are the operation name and the gateway's current status rendered capitalised ("Updating"), not the raw enum token.
Emulator: 400 with 'Cannot perform operation UpdateGateway when gateway is in UPDATING status' — a different sentence, the raw enum token, and the retry guidance missing.
Why: retry logic keyed on "Retry the request when the gateway is in a valid state" never fires, and a client cannot tell a transient state conflict from a permanent validation failure by text.
Fix: the gateway state-transition guard in the UpdateGateway handler's validation, which formats the invalid-state message (the same formatter the related wire issue flags for DeleteGateway).
Traces: bedrock-agentcore-control.update-gateway-conflicting-name-rejected.

Cause 4 of 4: @range validation messages render the offending Double as an integer, not in AWS's Double form.
AWS: Smithy RateConfig.rate targets Double with @range min 0 max 10000000, reached from both CreateGatewayRateLimit (entries -> LimitEntries -> LimitEntry.requests -> RateConfigs -> RateConfig) and BatchPutGatewayRateLimits (rateLimits -> BatchPutLimitEntries). Recorded behavior: the 400 ValidationException message is "1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must have value <greater|less> than or equal to <limit>", where the echoed value is the submitted Double rendered the way AWS renders a double — '-5.0' for -5, '2.0E7' for 20000000 — and the member path and limit come from the request and the model.
Emulator: emits the identical sentence but renders the value as an integer: "Value '-5' at 'rateLimits.1.member.entries.1.member.requests.1.member.rate' ..." and "Value '20000000' at 'entries.1.member.requests.1.member.rate' ...".
Why: the echoed value is the part of the message a caller uses to find the offending field in a 1000-entry batch; a client that compares the echo to what it sent, or parses it as a double, sees a value it did not submit in that form, and the divergence hits every Double-constrained member in the service, not just `rate`.
Fix: the number formatting used by the shared @range validation-message formatter for Double members (render as a double, including exponent form for large magnitudes), not the rate-limit handlers themselves.
Traces: bedrock-agentcore-control.batch-put-gateway-rate-limits-negative-value-rejected, bedrock-agentcore-control.create-gateway-rate-limit-exceeding-max-value-rejected.

## Evidence
All six checks matched status and error code; only the message diverged (cause 4 also only in one echoed token).
Cause 1 — bedrock-agentcore-control.agent-runtime-lifecycle-transitions: expected message 'Agent with agentId: <id>, accountId: <account>, and version: null not found!', got 'no agent runtime <id> in this account'. (Recorded run: id sim2realartac9502d30255-8ozt527ci1, account 000000000000 — those fill the template's variable slots; the fixed text is the requirement.)
Cause 2 — bedrock-agentcore-control.create-gateway-duplicate-name-conflict: expected "A gateway with name 'sim2real-gw-dup-0ea6d88a32b4' already exists in this account", got 'a gateway named sim2real-gw-dup-0ea6d88a32b4 already exists (gatewayId sim2real-gw-dup-0ea6d88a32b4-y2mpkoixt5)'. bedrock-agentcore-control.gateway-duplicate-name-conflict-rejected: same divergence with a different generated name (sim2real-gw-dup-403d5ccb5027). The names and gateway ids are per-run values.
Cause 3 — bedrock-agentcore-control.update-gateway-conflicting-name-rejected: expected "UpdateGateway operation can't be performed on gateway when it is in Updating state. Retry the request when the gateway is in a valid state.", got 'Cannot perform operation UpdateGateway when gateway is in UPDATING status'.
Cause 4 — bedrock-agentcore-control.batch-put-gateway-rate-limits-negative-value-rejected: expected "1 validation error detected: Value '-5.0' at 'rateLimits.1.member.entries.1.member.requests.1.member.rate' failed to satisfy constraint: Member must have value greater than or equal to 0", got the same sentence with Value '-5'. bedrock-agentcore-control.create-gateway-rate-limit-exceeding-max-value-rejected: expected "Value '2.0E7' at 'entries.1.member.requests.1.member.rate' failed to satisfy constraint: Member must have value less than or equal to 10000000", got Value '20000000'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Cause 1: GetAgentRuntime with an agentRuntimeId that does not exist and no version; read $.message of the 404 (check bedrock-agentcore-control.agent-runtime-lifecycle-transitions).
3. Cause 2: CreateGateway with a name, then CreateGateway again with the same name; read $.message of the 409 (checks create-gateway-duplicate-name-conflict, gateway-duplicate-name-conflict-rejected).
4. Cause 3: start an UpdateGateway so the gateway is UPDATING, then issue a second UpdateGateway; read $.message of the 400 (check update-gateway-conflicting-name-rejected).
5. Cause 4: BatchPutGatewayRateLimits with rateLimits[0].entries[0].requests[0].rate = -5, and CreateGatewayRateLimit with entries[0].requests[0].rate = 20000000; read the echoed Value in each 400 message (checks batch-put-gateway-rate-limits-negative-value-rejected, create-gateway-rate-limit-exceeding-max-value-rejected).
