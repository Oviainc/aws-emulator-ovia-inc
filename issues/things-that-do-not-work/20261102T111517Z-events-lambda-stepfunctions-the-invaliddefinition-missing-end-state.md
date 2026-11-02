Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-poller-loop-without-a-terminal-state-rejected
Issue-Key: events__lambda__stepfunctions.refarch-sfn-poller-loop-without-a-terminal-state-rejected|run#15|AssertionError|assert got == expected, f"a poller loop with no terminal state was answered {got!r}, expected {expected!r}"
Issue-Commit: fbef683f21970efb4631f2658975dd3b2c5af139

# events__lambda__stepfunctions: the InvalidDefinition MISSING_END_STATE refusal message drops its trailing " at <path>" segment (1 cause)

> _Note: this issue was identified against the code base at commit `fbef683f21970efb4631f2658975dd3b2c5af139`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
One net-new root cause in this service group.

Cause 1 of 1: CreateStateMachine now correctly refuses a definition in which every path cycles and no state is terminal, with HTTP 400 and `InvalidDefinition`, but its message omits the trailing location segment AWS appends to every MISSING_END_STATE diagnosis.
Failing check: events__lambda__stepfunctions.refarch-sfn-poller-loop-without-a-terminal-state-rejected.

AWS: a definition whose states form a loop with no terminal state (no `"End": true`, no Succeed and no Fail state) is refused by CreateStateMachine with HTTP 400, `Error.Code` `InvalidDefinition` and the message `Invalid State Machine Definition: '<CODE>: <diagnosis> at <location>'`. For this class the fixed text is `Invalid State Machine Definition: '` + `MISSING_END_STATE: Workflow has no terminal state at ` + `<location>` + `'`: the whole-workflow problem has no owning state, so the location is the literal `null`, giving `Invalid State Machine Definition: 'MISSING_END_STATE: Workflow has no terminal state at null'`. The `at <location>` suffix is part of the template, not an artefact of one run — for a state-scoped diagnosis it carries the offending state's path taken from the submitted definition. Recorded behavior only for the exact wording: Smithy (sfn) models `InvalidDefinition` as a 400 client error carrying only `message` ("The provided Amazon States Language definition is not valid.") and does not state the message template, so the recorded refusal is the requirement. No state machine is created either way.
Emulator: the status and error code are right, but the message is `Invalid State Machine Definition: 'MISSING_END_STATE: Workflow has no terminal state'` — the ` at <location>` segment is dropped entirely, so the message ends after `terminal state`.
Why: Step Functions definition diagnostics are consumed as text (CI linting of ASL, CDK/CloudFormation deploy logs, tooling that greps the `at <path>` tail to point a developer at the offending state). A client that matches or parses the documented template sees no location and cannot distinguish a whole-workflow MISSING_END_STATE (`at null`) from a state-scoped one, and exact-match assertions on AWS's wording fail against the emulator.
Fix: the InvalidDefinition message formatting in CreateStateMachine's (and UpdateStateMachine's) definition-validation layer in the sfn crate — every diagnosis must be rendered as `'<CODE>: <diagnosis> at <location>'`, emitting the literal `null` as the location when the finding is not attributable to a single state.
Open issue 20261102T091831Z-events-lambda-stepfunctions-every-task-state-invoking-lambda.md listed this check under its Cause 2 ("a loop-only definition is accepted"); that acceptance no longer reproduces — the definition is now refused — and the check fails on a different assertion (the refusal's message text), so that issue's replay key no longer matches and this message divergence is filed here.
Traces: the failing check id listed above.

## Evidence
Cause 1 — Expected `(code, message)` from CreateStateMachine on a loop-only poller definition: `('InvalidDefinition', "Invalid State Machine Definition: 'MISSING_END_STATE: Workflow has no terminal state at null'")`.
Actual: `('InvalidDefinition', "Invalid State Machine Definition: 'MISSING_END_STATE: Workflow has no terminal state'")`.
Fixed text of the template: `Invalid State Machine Definition: '`, `MISSING_END_STATE: Workflow has no terminal state at `, `'`. Variable part: the location, which is the literal `null` for a workflow-wide finding and otherwise the path of the offending state from the submitted definition. Status 400 and code `InvalidDefinition` match; `list_state_machines` correctly shows no machine created.

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com.
2. Build a job-poller ASL definition: `SubmitJob` (Task) -> `Wait30Seconds` (Wait) -> `CheckStatus` (Task) -> `JobComplete` (Choice with `Default: "Wait30Seconds"`), plus `JobFailed` (Fail) and `GetFinalJobStatus` (terminal Task).
3. Break exactly one thing: reduce `JobComplete.Choices` to a single branch whose `Next` is `Wait30Seconds`, and delete the `JobFailed` and `GetFinalJobStatus` states, so every path cycles and no state is terminal.
4. Call CreateStateMachine with that definition and read `Error.Code` and `Error.Message`.
AWS answers 400 `InvalidDefinition` with `Invalid State Machine Definition: 'MISSING_END_STATE: Workflow has no terminal state at null'`; the emulator answers the same code with the ` at null` tail missing.
