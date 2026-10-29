Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-error-paths-validation-and-conflict
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-error-paths-validation-and-conflict
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|CreateWorkflowDefinition|4xx->4xx|$.message
Issue-Key: nova-act|GetWorkflowRun|4xx->4xx|$.message
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# nova-act ValidationException message text diverges from AWS constraint-violation format

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflowDefinition

## Description
The nova-act emulator rejects pattern-violating input with the right HTTP status (400) and the right error type (ValidationException), but it renders the constraint-violation message differently from AWS in two ways that are shared by every member it validates: it interpolates the rejected VALUE into the message, and it prints the Smithy `@pattern` string with its `^`/`$` anchors still attached. AWS's nova-act ValidationException for a pattern violation uses the fixed text `1 validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>` - the only variable parts are the member name (from the request's rejected member) and the pattern (from that member's modeled `@pattern`, with the leading `^` and trailing `$` stripped). The rejected value itself is NOT echoed. Two concrete causes follow; both are the same formatter and both must be closed.

Cause 1 of 2: CreateWorkflowDefinition, input member `name` (pattern violation).
AWS: Smithy model services/service-com-amazonaws-novaact-amazonnovaagentsdataplane - CreateWorkflowDefinition.name targets WorkflowDefinitionName, required, @length min 1 max 40, @pattern "^[a-zA-Z0-9_-]{1,40}$"; modeled error ValidationException (httpError 400). The recorded AWS response is HTTP 400 with header x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.novaact/ and $.message exactly "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,40}". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member name (`name`) and the anchor-stripped pattern.
Emulator: returns 400 with $.message "1 validation error detected: Value 'invalid name with spaces!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,40}$" - i.e. it inserts "Value '<rejected value>' at" instead of "Value at", and keeps the `^`...`$` anchors on the pattern.
Why: a client (or a test/CLI that asserts on error text, or a log scrubber that relies on nova-act not echoing submitted workflow names) sees a message AWS never emits; value-echoing also leaks request content into error text that AWS deliberately omits, so redaction and golden-message assertions both break.
Fix: the shared ValidationException message formatter used by nova-act input validation (the constraint-violation renderer behind CreateWorkflowDefinition's input validation): drop the rejected-value interpolation and strip the anchors from the modeled pattern before rendering.
Traces: nova-act-error-paths-validation-and-conflict#task#1.

Cause 2 of 2: GetWorkflowRun, URI label `workflowRunId` (pattern violation) - same formatter, second member, confirming it is not a one-off string.
AWS: Smithy model - GetWorkflowRun.workflowRunId is an @httpLabel, required, targets UuidString with @pattern "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"; modeled error ValidationException (httpError 400). Recorded AWS response is HTTP 400 with $.message exactly "1 validation error detected: Value at 'workflowRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}".
Emulator: returns 400 with $.message "1 validation error detected: Value 'not-a-uuid' at 'workflowRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" - again value echoed and pattern anchors retained.
Why: same as cause 1; additionally it shows the defect applies to httpLabel validation, so fixing only the CreateWorkflowDefinition path would leave GetWorkflowRun diverging.
Fix: same formatter/validation layer, applied to httpLabel-bound member validation for GetWorkflowRun.
Traces: nova-act-error-paths-validation-and-conflict#task#4.

Note: the other two divergences in this fixture family (expected 409 ConflictException on duplicate CreateWorkflowDefinition, and expected 400 export-bucket validation) are already owned by the open issue 20261023T043407Z-nova-act-createworkflowdefinition-skips-precondition-checks-and-returns.md and are NOT part of this issue; that issue covers missing precondition checks, not message formatting, so it does not cover the two causes above.

## Evidence
Cause 1 - fixture nova-act-error-paths-validation-and-conflict, step task#1, CreateWorkflowDefinition, status 400 expected and 400 actual (status matches; body diverges).
  Expected $.message: "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]{1,40}"
  Actual   $.message: "1 validation error detected: Value 'invalid name with spaces!' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]{1,40}$"
  Divergence key: service=nova-act|CreateWorkflowDefinition|status=400->400|keys=$.message

Cause 2 - fixture nova-act-error-paths-validation-and-conflict, step task#4, GetWorkflowRun, status 400 expected and 400 actual.
  Expected $.message: "1 validation error detected: Value at 'workflowRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"
  Actual   $.message: "1 validation error detected: Value 'not-a-uuid' at 'workflowRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
  Divergence key: service=nova-act|GetWorkflowRun|status=400->400|keys=$.message

Common shape of the defect: actual = expected with (a) "Value " replaced by "Value '<request value>' " and (b) the pattern rendered with its ^ and $ anchors. Status code, x-amzn-errortype and all other body members match in both steps.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture nova-act-error-paths-validation-and-conflict step task#1 - PUT /workflow-definitions with a `name` that violates the modeled pattern (e.g. containing spaces/punctuation). Observe HTTP 400 and compare $.message against the recorded AWS text: the emulator adds "Value '<value>' at" and keeps the ^...$ anchors.
3. Cause 2: replay the same fixture's step task#4 - GET /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId} with a non-UUID workflowRunId. Observe HTTP 400 and the same two message deviations.
4. Both steps pass only when $.message equals "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <modeled pattern without ^ and $>".
