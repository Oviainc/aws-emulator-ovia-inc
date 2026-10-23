Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.create-invocation-nonexistent-session-not-found bedrock-agent-runtime.delete-session-after-end-succeeds bedrock-agent-runtime.delete-session-twice-rejected bedrock-agent-runtime.get-session-nonexistent-not-found bedrock-agent-runtime.get-session-nonexistent-resource-not-found bedrock-agent-runtime.session-delete-then-404 bedrock-agent-runtime.session-lifecycle-active-ended-deleted bedrock-agent-runtime.create-invocation-on-ended-session-rejected bedrock-agent-runtime.update-session-after-end-rejected bedrock-agent-runtime.delete-active-session-rejected bedrock-agent-runtime.get-session-not-found bedrock-agent-runtime.malformed-session-id-input-rejected bedrock-agent-runtime.list-invocation-steps-malformed-invocation-identifier-rejected
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime.create-invocation-nonexistent-session-not-found|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agent-runtime|GetSession|ResourceNotFoundException
Issue-Key: bedrock-agent-runtime.delete-session-twice-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agent-runtime.get-session-nonexistent-not-found|run#12|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.get-session-nonexistent-resource-not-found|run#12|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.session-delete-then-404|run#8|AssertionError|assert get_message == expected_message, (
Issue-Key: bedrock-agent-runtime.session-lifecycle-active-ended-deleted|run#17|AssertionError|assert end_message == expected_message, (
Issue-Key: bedrock-agent-runtime.create-invocation-on-ended-session-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agent-runtime.update-session-after-end-rejected|run#12|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.delete-active-session-rejected|run#8|AssertionError|assert message == "Cannot delete session in ACTIVE state", (
Issue-Key: bedrock-agent-runtime.get-session-not-found|run#6|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.malformed-session-id-input-rejected|run#11|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.list-invocation-steps-malformed-invocation-identifier-rejected|run#8|AssertionError|assert message == expected_message, (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# bedrock-agent-runtime session error messages are rewritten: not-found, terminal-state and delete-refusal text differ from AWS, and ValidationException echoes the modeled pattern with its ^/$ anchors (3 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSession

## Description
Three message-rendering root causes. In each one the HTTP status and the error CODE already match AWS; what diverges is the message text, and in each case the rule that produces the message is known — it is a fixed template interpolating a value taken from the request, or the modeled constraint echoed back. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json.

=== Cause 1 of 3: ResourceNotFoundException for an unknown session uses the wrong template ===
AWS: Smithy models ResourceNotFoundException (404) on GetSession, EndSession, DeleteSession, UpdateSession and CreateInvocation; the message text is recorded behavior only. The recorded rule is a fixed template interpolating the request's session identifier: 'Session with ID <sessionIdentifier> not found'. It is observed identically across GetSession, EndSession, DeleteSession and CreateInvocation, so the template — not the one recorded id — is the requirement.
Emulator: emits 'Session with identifier <sessionIdentifier> was not found.' (different wording, trailing period) from the same five paths. Status 404 and code ResourceNotFoundException are correct.
Why: clients and operators that match on the message (log scrapers, user-facing error surfacing, and recorded-response fixtures) see text AWS never produces, and a suite asserting the AWS message fails against the emulator.
Fix: the shared session-lookup error constructor used by the get_session / end_session / delete_session / create_invocation handlers in services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/ops/ must render the AWS template.
Traces: 7 checks, see trace_ids (create-invocation-nonexistent-session-not-found, delete-session-after-end-succeeds, delete-session-twice-rejected, get-session-nonexistent-not-found, get-session-nonexistent-resource-not-found, session-delete-then-404, session-lifecycle-active-ended-deleted).

=== Cause 2 of 3: terminal-state and ACTIVE-delete refusals use the wrong templates ===
AWS: Smithy models ConflictException/ValidationException on CreateInvocation, UpdateSession and DeleteSession; the text is recorded behavior only. Recorded rules, each a fixed template interpolating the request's session id: writing to or updating an ENDED session -> 'Session with id <id> is in a terminal state and cannot be updated anymore.'; deleting a session that is still ACTIVE -> the constant 'Cannot delete session in ACTIVE state'.
Emulator: produces per-operation prose instead — 'Session <id> has status ENDED and can no longer be added to.' (CreateInvocation), 'Session <id> has status ENDED and can no longer be modified.' (UpdateSession), 'Session <id> has status ACTIVE and cannot be deleted. End the session first.' (DeleteSession). The codes/status match.
Why: same client impact as Cause 1; additionally the emulator's wording varies per operation where AWS uses one terminal-state sentence, so a client cannot match one rule.
Fix: the state-machine guard shared by create_invocation / update_session / delete_session must emit the two AWS templates.
Traces: 3 checks, see trace_ids.

=== Cause 3 of 3: ValidationException echoes the modeled @pattern including its ^ and $ anchors ===
AWS: Smithy: GetSession/DeleteSession/EndSession/UpdateSession sessionIdentifier -> SessionIdentifier @pattern ^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]+:[0-9]{12}:session/<uuid>)|(<uuid>)$ and ListInvocationSteps invocationIdentifier -> InvocationIdentifier @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$. The message is a validation error naming the member and its constraint, so the rule is known; the recorded rule is that AWS renders the constraint with the ^ and $ ANCHORS STRIPPED — 'Member must satisfy regular expression pattern: ((arn:...session/<uuid>)|(<uuid>))' and '... pattern: [a-f0-9]{8}-...-[a-f0-9]{12}'. Observed consistently across three independent recordings on two different members.
Emulator: echoes the model's pattern verbatim, anchors included ('... pattern: ^(arn:...)|(...)$', '... pattern: ^[a-f0-9]{8}-...$'). Status 400 and code ValidationException are correct, and the accept/reject decision is correct.
Why: the message is a contract clients parse to report which constraint failed; the anchored form is not what AWS emits, so fixture-based clients and any assertion on the rendered constraint diverge.
Fix: the validation-message renderer (the shared constraint-violation formatter behind these handlers) should strip leading ^ / trailing $ from the pattern it echoes.
Traces: 3 checks, see trace_ids (get-session-not-found, malformed-session-id-input-rejected, list-invocation-steps-malformed-invocation-identifier-rejected).

## Evidence
Cause 1 — expected message 'Session with ID 1a400015-c2b4-45b9-996e-1953e0e39268 not found', got 'Session with identifier 1a400015-c2b4-45b9-996e-1953e0e39268 was not found.' (same divergence on EndSession, DeleteSession and CreateInvocation paths; code ResourceNotFoundException / 404 matches in all of them).

Cause 2 — expected 'Session with id 7732b827-4789-48b9-88d3-7b77039f52e7 is in a terminal state and cannot be updated anymore.', got 'Session 7732b827-4789-48b9-88d3-7b77039f52e7 has status ENDED and can no longer be added to.'; expected the same terminal-state sentence on UpdateSession, got '... can no longer be modified.'; expected 'Cannot delete session in ACTIVE state', got 'Session 276db80d-bfca-4de4-b7bd-39dc943b4ecb has status ACTIVE and cannot be deleted. End the session first.'

Cause 3 — expected "1 validation error detected: Value 'not-a-valid-session-id' at 'sessionIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]+:[0-9]{12}:session/[a-f0-9]{8}-...)|([a-f0-9]{8}-...))", got the same sentence with the pattern rendered as "^(arn:aws(-[^:]+)?:bedrock:...)|(...)$"; and expected "... at 'invocationIdentifier' ... pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}", got "... pattern: ^[a-f0-9]{8}-...-[a-f0-9]{12}$".

## Steps to Reproduce
Cause 1: get_session(sessionIdentifier=<random uuid never created>) -> 404 ResourceNotFoundException with message 'Session with identifier <uuid> was not found.' instead of 'Session with ID <uuid> not found'. Repeat with end_session / delete_session / create_invocation on the same unknown id.
Cause 2: create_session(); end_session(<id>); then create_invocation(sessionIdentifier=<id>) and update_session(sessionIdentifier=<id>, sessionMetadata={...}) -> per-operation ENDED prose instead of the terminal-state template. Separately: create_session(); delete_session(sessionIdentifier=<id>) while still ACTIVE -> 'Session <id> has status ACTIVE and cannot be deleted. End the session first.' instead of 'Cannot delete session in ACTIVE state'.
Cause 3: get_session(sessionIdentifier='not-a-uuid-and-not-an-arn!!!') and list_invocation_steps(sessionIdentifier=<valid>, invocationIdentifier='not-a-valid-uuid') -> 400 ValidationException whose echoed pattern carries ^ and $.
