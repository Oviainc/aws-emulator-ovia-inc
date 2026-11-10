Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__stepfunctions.test-state-reveal-secrets-requires-permission
Issue-Key: iam__stepfunctions.test-state-reveal-secrets-requires-permission|_settle#7|Timeout|time.sleep(interval_s)
Issue-Commit: 5598dd881b8e225c34bc5a9814ddc1ff981b448f

# iam__stepfunctions: TestState(revealSecrets=true) never resolves for a caller without states:RevealSecrets — neither the AccessDeniedException nor the default-false success ever comes back

> _Note: this issue was identified against the code base at commit `5598dd881b8e225c34bc5a9814ddc1ff981b448f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestState

## Description
One net-new root cause, grounded in one failing behavioral check the replay report left unmarked (no ALREADY-FILED marker and no OWNED marker).

Cause 1 of 1 — the states:RevealSecrets authorization gate on TestState never produces a settled answer: for a caller whose identity policy omits states:RevealSecrets, the revealSecrets=true call is never refused with AccessDeniedException, and the otherwise identical default (revealSecrets absent/false) call is never confirmed to succeed for that same caller.
Failing check id: iam__stepfunctions.test-state-reveal-secrets-requires-permission.
AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) models TestState (host prefix "sync-") with input {definition (required, @length 1..1048576, @sensitive), roleArn, input, inspectionLevel, revealSecrets (default false), variables, stateName, mock, context, stateConfiguration} and output {output, error, cause, inspectionData, nextState, status}; its modeled errors are InvalidArn, InvalidDefinition, InvalidExecutionInput and ValidationException. AccessDeniedException is the service-common authorization refusal: a caller that does not hold states:RevealSecrets is refused with exactly error code AccessDeniedException when revealSecrets=true, while the same caller's call with revealSecrets at its false default is served normally — the permission gates only the reveal, not TestState itself.
Emulator: the check is killed by the harness with "Error: Check timed out after 60.0s" while polling for a stable outcome from the two minted callers, so neither the expected AccessDeniedException for the no-RevealSecrets caller nor the expected success for the default call is ever observed.
Why: a client relying on this gate cannot verify that secret-revealing inspection is restricted. Either secrets are revealed to a caller that must not see them, or TestState is refused for a caller that is entitled to use it without reveal — and in both cases the caller gets no classifiable error code to branch on.
Fix: the TestState handler's authorization step — evaluate states:TestState and, only when revealSecrets is true, additionally states:RevealSecrets against the caller's identity policy, and return AccessDeniedException when the latter is absent while leaving the default-false path unaffected.
Related but NOT covering this check: open issues 20261029T171911Z-states-step-functions-teststate-does-not-evaluate-jsonata-states.md and 20261109T163411Z-iam-stepfunctions-7-net-new-causes-trust-permission.md (the replay report marked this check owned by neither).
Traces: iam__stepfunctions.test-state-reveal-secrets-requires-permission.

## Evidence
iam__stepfunctions.test-state-reveal-secrets-requires-permission — verdict mismatch.
Expected: for a user whose policy allows states:TestState but NOT states:RevealSecrets, TestState(definition='{"Type": "Pass", "Result": {"ok": true}, "End": true}', input="{}", revealSecrets=True) is refused with error code exactly AccessDeniedException, while the identical call without revealSecrets succeeds; for a user whose policy also allows states:RevealSecrets, both calls succeed.
Actual: "Error: Check timed out after 60.0s" — the check never settled on either outcome, so neither the refusal nor the control success was observed.

## Steps to Reproduce
1. iam:CreateUser + iam:PutUserPolicy twice: one policy allowing states:TestState only, one allowing states:TestState and states:RevealSecrets. Mint access keys for each.
2. As the no-RevealSecrets caller: sfn:TestState(definition='{"Type": "Pass", "Result": {"ok": true}, "End": true}', input="{}", revealSecrets=true) — expected: refused with error code AccessDeniedException.
3. As the same caller: the identical TestState call with revealSecrets omitted — expected: 200 with the Pass state's output.
4. As the RevealSecrets caller: both calls — expected: both succeed.
5. Observed: the check does not settle on these outcomes and is killed at the 60 s harness cap.
