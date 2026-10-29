Issue-Source: trace-sweep-divergence
Issue-Trace: states-definition-validation-and-test-state
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: states-definition-validation-and-test-state states-definition-validation-workflow sfn-validate-definition-warning-diagnostics stepfunctions-validate-definition-lint
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*]
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*].location
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*].message
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.truncated
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Step Functions ValidateStateMachineDefinition emits the wrong diagnostic set, locations and wording, and never truncates on maxResults (5 causes)

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateStateMachineDefinition

## Description
This issue bundles FIVE net-new wire divergence clusters on sfn ValidateStateMachineDefinition. Smithy models it with input {definition (required, @length 1..1048576), type, severity, maxResults (0..100)} and output {result (required), diagnostics (required), truncated} where each ValidateStateMachineDefinitionDiagnostic carries required severity, code and message plus an optional location, documented as "/States/<StateName>/<FieldName>". All five causes must be closed before submitting; re-replay every fixture#step listed in trace_ids.

=== Cause 1 of 5: a definition missing BOTH StartAt and States yields two per-field diagnostics with emulator wording instead of one grouped root-level diagnostic ===
AWS: recorded behavior — a definition missing the top-level required fields produces a SINGLE diagnostic with `location` = `/` (the root, because the fields are missing from the document rather than from a named state) and `message` = `These fields are required: [States, StartAt]`, i.e. fixed text `These fields are required: [` plus the bracketed list of the missing field names.
Emulator: it emits one diagnostic per missing field — the first at `location` `/StartAt` with `a state machine definition must carry a string 'StartAt' member`, and a second, extra one at `/States` with `a state machine definition must carry an object 'States' member`. Both the per-field split and both messages are emulator-authored.
Why: a client that counts diagnostics, or keys them by location to annotate an editor, gets two annotations on fields that do not exist in the document and none at the root; the requirement list AWS hands back in one message is unavailable.
Fix: ValidateStateMachineDefinition's definition-schema validation — collect the missing top-level required fields and emit one root-located SCHEMA_VALIDATION_FAILED diagnostic with AWS's grouped message.
Traces: states-definition-validation-and-test-state#task#4.

=== Cause 2 of 5: the same grouped root diagnostic is wrong when only StartAt is missing ===
AWS: recorded behavior — with only StartAt absent the single diagnostic is `location` `/` and `message` `These fields are required: [StartAt]`.
Emulator: `location` `/StartAt` with `a state machine definition must carry a string 'StartAt' member`.
Why: as Cause 1 — the location and the message both differ, so neither an editor annotation nor an assertion on AWS's wording works.
Fix: as Cause 1 (same construction, exercised with one missing field).
Traces: states-definition-validation-workflow#task#4 (also #task#5).

=== Cause 3 of 5: POSSIBLY_UNDEFINED_VARIABLE warnings are never produced ===
AWS: recorded behavior — validating a definition that reads a variable no preceding state assigned yields, for EACH such read, a diagnostic with `severity` `WARNING`, `code` `POSSIBLY_UNDEFINED_VARIABLE`, `location` `/States/<StateName>/<FieldName>` (e.g. the state's `Output` field) and `message` `Variable '<name>' is possibly not defined.` — fixed text `Variable '` ... `' is possibly not defined.` with the variable name from the definition.
Emulator: it emits no diagnostics for this definition at all, so the whole WARNING-severity lint class is missing.
Why: this lint is the only pre-deploy signal that a state machine reads a variable that may be unset at run time; a CI gate asserting on WARNING diagnostics passes a definition AWS flags.
Fix: ValidateStateMachineDefinition's lint pass — track variable assignment across the state graph and emit POSSIBLY_UNDEFINED_VARIABLE warnings with the per-field location.
Traces: sfn-validate-definition-warning-diagnostics#task#1.

=== Cause 4 of 5: maxResults never truncates the diagnostic list and `truncated` is always false ===
AWS: recorded behavior, consistent with the model (maxResults range 0..100 and the output member `truncated`) — when more diagnostics exist than maxResults allows, the response carries at most maxResults diagnostics and `truncated` = true.
Emulator: it answers `truncated` = false and the expected first diagnostic is absent entirely (the warning class of Cause 3 is missing), so neither the cap nor the flag is honoured.
Why: a client paging or budgeting large validation reports cannot tell a complete report from a clipped one, and will present a truncated report as exhaustive.
Fix: ValidateStateMachineDefinition's handler — apply maxResults to the assembled diagnostic list and set `truncated` when it drops any.
Traces: sfn-validate-definition-warning-diagnostics#task#4.

=== Cause 5 of 5: the dangling-Next diagnostic is reworded and MISSING_END_STATE is never emitted ===
AWS: recorded behavior — a definition whose state transitions to an undeclared state produces the diagnostic message `Missing 'Next' target: <target state name>` (fixed text `Missing 'Next' target: ` plus the unresolved target from the definition), AND a second diagnostic {severity ERROR, code MISSING_END_STATE, message `Workflow has no terminal state`} because no state in the graph terminates.
Emulator: the first diagnostic's message is the emulator-authored `state '<state>' transitions to '<target>', which 'States' does not declare`, and the MISSING_END_STATE diagnostic is absent.
Why: two independent faults in one definition are reported as one; a developer fixes the Next target, re-validates, and only then learns the workflow has no terminal state — and automation matching AWS's codes never sees MISSING_END_STATE at all.
Fix: ValidateStateMachineDefinition's graph lint — emit AWS's dangling-Next message and add the MISSING_END_STATE check (no state reachable with End:true or a terminal type).
Traces: stepfunctions-validate-definition-lint#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1; all clusters verdict=mismatch, service=states, operation ValidateStateMachineDefinition, status 200 on both sides.

Cause 1 — states-definition-validation-and-test-state#task#4: `$.diagnostics[0].location: value_mismatch (expected '/', got '/StartAt')`; `$.diagnostics[0].message: value_mismatch (expected 'These fields are required: [States, StartAt]', got "a state machine definition must carry a string 'StartAt' member")`; `$.diagnostics[1]: extra (expected None, got {'severity': 'ERROR', 'code': 'SCHEMA_VALIDATION_FAILED', 'message': "a state machine definition must carry an object 'States' member", 'location': '/States'})`.
Cause 2 — states-definition-validation-workflow#task#4: `$.diagnostics[0].location: expected '/', got '/StartAt'`; `$.diagnostics[0].message: expected 'These fields are required: [StartAt]', got "a state machine definition must carry a string 'StartAt' member"` (also #task#5).
Cause 3 — sfn-validate-definition-warning-diagnostics#task#1: `$.diagnostics[0]: missing (expected {'code': 'POSSIBLY_UNDEFINED_VARIABLE', 'location': '/States/A/Output', 'message': "Variable 'alpha' is possibly not defined.", 'severity': 'WARNING'}, got None)` and the same for `/States/B/Output` (beta) and `/States/C/Output` (gamma).
Cause 4 — sfn-validate-definition-warning-diagnostics#task#4: `$.diagnostics[0]: missing (expected the POSSIBLY_UNDEFINED_VARIABLE warning for /States/A/Output, got None)`; `$.truncated: value_mismatch (expected True, got False)`.
Cause 5 — stepfunctions-validate-definition-lint#task#2: `$.diagnostics[0].message: value_mismatch (expected "Missing 'Next' target: DoesNotExist", got "state 'Start' transitions to 'DoesNotExist', which 'States' does not declare")`; `$.diagnostics[1]: missing (expected {'code': 'MISSING_END_STATE', 'message': 'Workflow has no terminal state', 'severity': 'ERROR'}, got None)`.

The state names (A, B, C, Start), variable names (alpha, beta, gamma) and the target name DoesNotExist are the fixtures' own definitions; the requirements are the diagnostic codes, severities, locations and message templates, the grouping of missing top-level fields into one root diagnostic, and the maxResults/truncated contract.

## Steps to Reproduce
Cause 1: sfn:ValidateStateMachineDefinition(definition='{}'). Expect exactly one diagnostic: {severity ERROR, code SCHEMA_VALIDATION_FAILED, location '/', message 'These fields are required: [States, StartAt]'}; observed two per-field diagnostics at '/StartAt' and '/States' with emulator wording.
Cause 2: sfn:ValidateStateMachineDefinition(definition='{"States":{"P":{"Type":"Pass","End":true}}}'). Expect one diagnostic at location '/' with 'These fields are required: [StartAt]'.
Cause 3: sfn:ValidateStateMachineDefinition(definition=<a chain of states whose Output reads variables ($alpha, $beta, $gamma) that no preceding Assign sets>, severity=WARNING). Expect one WARNING/POSSIBLY_UNDEFINED_VARIABLE diagnostic per read, located at '/States/<State>/Output', message "Variable '<name>' is possibly not defined."; observed no diagnostics.
Cause 4: repeat Cause 3 with maxResults=1. Expect at most one diagnostic and truncated=true; observed truncated=false and the diagnostic missing.
Cause 5: sfn:ValidateStateMachineDefinition(definition='{"StartAt":"Start","States":{"Start":{"Type":"Pass","Next":"DoesNotExist"}}}'). Expect a diagnostic "Missing 'Next' target: DoesNotExist" plus a second {ERROR, MISSING_END_STATE, 'Workflow has no terminal state'}; observed the emulator's reworded first message and no MISSING_END_STATE.
Replay: the fixture#step tokens in Issue-Trace/Issue-Step and trace_ids.
