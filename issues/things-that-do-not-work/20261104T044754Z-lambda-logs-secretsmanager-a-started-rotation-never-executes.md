Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation lambda__logs__secretsmanager.rotate-immediately-false-schedules-a-rotation-without-running-the-full-protocol
Issue-Key: lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs__secretsmanager.rotate-immediately-false-schedules-a-rotation-without-running-the-full-protocol|_poll_until#38|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# lambda__logs__secretsmanager: a started rotation never executes the rotation Lambda — no createSecret step is ever logged and a failed finishSecret never leaves the candidate AWSPENDING, so both rotation-protocol checks hang

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
One cause.

Cause 1 of 1: RotateSecret is accepted but the four-step rotation protocol is never driven — the rotation Lambda is never invoked, so nothing it would log ever appears and none of the protocol's intermediate states (a candidate version left in AWSPENDING by a failed finishSecret, and the refusal of a new rotation while that candidate is stuck) can be observed. Both checks run out the 60 s harness cap with no verdict.

AWS: Smithy secretsmanager models RotateSecret {SecretId, ClientRequestToken, RotationLambdaARN, RotationRules, RotateImmediately (BooleanOptional, default true)}. When a rotation runs, Secrets Manager invokes the rotation function once per step with {SecretId, ClientRequestToken, Step} for Step in createSecret, setSecret, testSecret, finishSecret, so the function's log stream carries one record per step. With RotateImmediately=false the schedule is recorded and the function is NOT invoked until the schedule fires; with RotateImmediately=true the protocol runs immediately and createSecret is the first step logged. If a step fails, the rotation stops there: the candidate version created by createSecret stays labelled AWSPENDING, AWSCURRENT does not move, and a further RotateSecret on that secret is refused with InvalidRequestException whose message is "A previous rotation isn't complete. That rotation will be reattempted."

Emulator: no rotation step is ever executed. The immediate-rotation control's poll for "the immediate rotation to log its createSecret step" exhausts the run budget (TimeoutError), and the failed-finishSecret check hangs to the harness cap because the AWSPENDING candidate it must inspect is never created. Related open issues in this same service group describe neighbouring symptoms of the same missing machinery but do not carry these two checks: 20261024T150703Z-lambda-logs-secretsmanager-a-secrets-manager-rotation-never.md, 20261101T210635Z-lambda-logs-secretsmanager-secret-rotation-never-stages-awspending.md and 20261025T022656Z-lambda-logs-secretsmanager-a-rotation-never-replaces-the.md.

Why: a client that relies on rotation gets a 200 from RotateSecret and nothing else — the credential is never replaced, the rotation function is never run (so its own side effects never happen), the AWSPENDING staging label a reader uses to pick up a candidate never exists, and the "previous rotation isn't complete" guard that protects a stuck secret never fires. Any wait-for-rotation loop hangs forever.

Fix: the RotateSecret handler and the secret's rotation state machine — it must invoke the configured RotationLambdaARN once per protocol step with the rotation token, honour RotateImmediately, stage the candidate version as AWSPENDING, stop on a failed step leaving that label in place, and refuse a subsequent RotateSecret with InvalidRequestException and the fixed "A previous rotation isn't complete. That rotation will be reattempted." message.

Traces: lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation, lambda__logs__secretsmanager.rotate-immediately-false-schedules-a-rotation-without-running-the-full-protocol.

## Evidence
[error] lambda__logs__secretsmanager.rotate-immediately-false-schedules-a-rotation-without-running-the-full-protocol
Error: File "check_…_rotate_immediately_false_schedules_a_rotation_without_running_the_full_protocol.py", line 200, in run
  _poll_until(
  ...
  raise TimeoutError(f"{description}: the run budget ended first; not graded")
TimeoutError: the immediate rotation to log its createSecret step: the run budget ended first; not graded

[mismatch] lambda__logs__secretsmanager.deep-secrets-and-params-failed-finishsecret-leaves-candidate-awspending-and-rejects-new-rotation
Error: Check timed out after 60.0s

Expected: with RotateImmediately=true the rotation function's log group carries a createSecret record (then setSecret/testSecret/finishSecret); with a finishSecret that fails, DescribeSecret's VersionIdsToStages still maps the candidate version to ["AWSPENDING"] and a new RotateSecret is refused InvalidRequestException "A previous rotation isn't complete. That rotation will be reattempted."
Actual: no step record ever appears and no AWSPENDING candidate is ever created.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. lambda: CreateFunction for a rotation handler that logs its Step, and AddPermission for secretsmanager.amazonaws.com.
3. secretsmanager: CreateSecret, then RotateSecret(SecretId=…, RotationLambdaARN=…, RotationRules={'AutomaticallyAfterDays':30}, RotateImmediately=True).
4. Poll logs:FilterLogEvents on the function's log group for a 'createSecret' record -> expect it promptly; observed: nothing is ever logged.
5. With a rotation function whose finishSecret step fails, repeat and read DescribeSecret -> expect the candidate version still labelled AWSPENDING and a second RotateSecret refused InvalidRequestException "A previous rotation isn't complete. That rotation will be reattempted."
6. Replay the two checks in trace_ids.
