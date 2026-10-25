Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.rotation-lambda-actually-replaces-the-current-secret-value-it-generated
Issue-Key: lambda__logs__secretsmanager.rotation-lambda-actually-replaces-the-current-secret-value-it-generated|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__logs__secretsmanager: a rotation never replaces the secret value the rotation Lambda generated (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1: the rotation that must end with AWSCURRENT pointing at the value the rotation Lambda generated never completes; the check is killed at the harness 60 s cap.

AWS: RotateSecret invokes the configured rotation Lambda through the four steps (createSecret, setSecret, testSecret, finishSecret); the function's generated value is staged as AWSPENDING and finishSecret moves AWSCURRENT onto it, so a subsequent GetSecretValue returns the newly generated value. Recorded behavior only for the step timing.

Emulator: the flow makes no progress — 'Error: Check timed out after 60.0s'. AWSCURRENT is never advanced to the generated value, so the rotation's only observable outcome never appears.

Why: a client that relies on rotation to replace a credential keeps serving the old value while the API reports a rotation was requested; readers never see the new secret and a waiter on the rotation never resolves.

Fix: the Secrets Manager rotation driver (the step sequence that must invoke the rotation Lambda and move the version stages) and the lambda invoke path it depends on. The open issue 20261024T150703Z-lambda-logs-secretsmanager-a-secrets-manager-rotation-never.md records this cause for two sibling checks; this check is not listed there.

Traces: lambda__logs__secretsmanager.rotation-lambda-actually-replaces-the-current-secret-value-it-generated.

## Evidence
Expected: after RotateSecret completes, GetSecretValue(VersionStage='AWSCURRENT') returns the value the rotation Lambda generated.
Actual: Error: Check timed out after 60.0s — the rotation never advanced.

## Steps to Reproduce
1. Create a secret and a rotation Lambda implementing the four rotation steps.
2. RotateSecret with that function configured.
3. Poll GetSecretValue(AWSCURRENT) for the generated value.
4. Observe it never changes.
