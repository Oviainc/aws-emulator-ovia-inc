Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: nova-act.create-workflow-definition-invalid-name-pattern-rejected
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act.create-workflow-definition-invalid-name-pattern-rejected|run#7|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Commit: bfd8b7223bf6a1ab9b658956ab58262126f30b11

# nova-act CreateWorkflowDefinition ValidationException message echoes the rejected value and keeps the @pattern anchors

> _Note: this issue was identified against the code base at commit `bfd8b7223bf6a1ab9b658956ab58262126f30b11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflowDefinition

## Description
Cause 1 of 1: CreateWorkflowDefinition rejects an out-of-pattern `name` with the correct HTTP status and error code, but renders the constraint-violation message in a format AWS never emits.

Related open issue: 20261029T174545Z-nova-act-validationexception-message-text-diverges-from-aws.md describes the same message formatter from the WIRE corpus (fixture nova-act-error-paths-validation-and-conflict, steps task#1/task#4). It does NOT carry this behavioral check's replay key, so this issue files that key; a fixer closing the formatter should re-replay the check id listed in trace_ids as well.

AWS: Smithy model services/service-com-amazonaws-novaact-amazonnovaagentsdataplane - CreateWorkflowDefinition input member `name` targets WorkflowDefinitionName, required, @length min 1 max 40, @pattern "^[a-zA-Z0-9_-]{1,40}$"; modeled error ValidationException with httpError 400. The recorded AWS behavior for a pattern violation on this member is HTTP 400, error code ValidationException, and the message "1 validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>". Fixed text of that template: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Its only variable parts are (a) the member name, taken from the request's rejected member, and (b) the pattern, taken from that member's modeled @pattern with the leading `^` and trailing `$` stripped. The rejected VALUE is not echoed. For this member the rendered message is therefore "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,40}".

Emulator: returns HTTP 400 / ValidationException (both correct) with the message "1 validation error detected: Value '<rejected value>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,40}$" - two deviations from the template: it inserts "Value '<rejected value>' at" where AWS writes "Value at", and it prints the modeled pattern with its `^` and `$` anchors still attached. (In the replay the rejected value happened to be a name containing spaces and punctuation; that value is request input filling the template, not part of the requirement.)

Why: a client, CLI or test that asserts on nova-act's documented validation text gets a message AWS never produces, so error-text matching and golden-message assertions fail against the emulator while passing against AWS. The value interpolation additionally leaks submitted request content (a caller-chosen workflow definition name) into error text that AWS deliberately omits, so anything relying on nova-act not echoing request input in error bodies - log scrubbing, redaction - behaves differently than it does in production.

Fix: the shared constraint-violation message formatter behind nova-act input validation, as reached from CreateWorkflowDefinition's input validation for the `name` member: remove the rejected-value interpolation and strip the `^`/`$` anchors from the modeled pattern before rendering. The status code and error code paths are already correct and must not change.

Traces: behavioral check nova-act.create-workflow-definition-invalid-name-pattern-rejected.

## Evidence
Check nova-act.create-workflow-definition-invalid-name-pattern-rejected, verdict mismatch.

The check asserts the error code first (ValidationException) - that assertion PASSES, so status/code are right and only the message diverges. It then asserts the message:

  AssertionError: expected message "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,40}", got "1 validation error detected: Value 'bad name with spaces!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,40}$"

Expected (AWS): HTTP 400, Code=ValidationException, Message="1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,40}"
Actual (emulator): HTTP 400, Code=ValidationException, Message="1 validation error detected: Value 'bad name with spaces!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,40}$"

Delta = actual is expected with (a) "Value " replaced by "Value '<request value>' " and (b) the modeled pattern rendered with its ^ and $ anchors. `bad name with spaces!` is the request value used by this run, not a required literal; `name` is the rejected member name from the request and `[a-zA-Z0-9_-]{1,40}` is WorkflowDefinitionName's modeled @pattern with anchors stripped.

The rest of the check passes: the rejected call leaves no new workflow definition behind (ListWorkflowDefinitions name set unchanged), and the positive control with a compliant name creates an ACTIVE definition readable back via GetWorkflowDefinition.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point a nova-act client at it (region us-east-1 - nova-act serves its control plane there; an emulator target ignores the region pin).
2. Call CreateWorkflowDefinition with a `name` that violates WorkflowDefinitionName's modeled @pattern ^[a-zA-Z0-9_-]{1,40}$ - e.g. a name containing spaces and punctuation - and a fresh clientToken.
3. Observe HTTP 400 with Code=ValidationException (correct) and inspect $.message.
4. The emulator emits "1 validation error detected: Value '<the name you sent>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,40}$".
5. AWS emits "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,40}" - no rejected value echoed, pattern without ^/$ anchors. The check passes only when $.message equals that.
6. Re-replay behavioral check nova-act.create-workflow-definition-invalid-name-pattern-rejected to confirm the fix.
