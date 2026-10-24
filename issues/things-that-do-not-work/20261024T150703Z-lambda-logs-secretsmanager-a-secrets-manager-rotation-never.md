Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential
Issue-Key: lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# lambda__logs__secretsmanager: a Secrets Manager rotation never advances because the rotation Lambda is never executed (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: RotateSecret does not drive the configured rotation Lambda through its four steps ===
AWS: RotateSecret invokes the secret's RotationLambdaARN once per step with {SecretId, ClientRequestToken, Step} for Step in createSecret, setSecret, testSecret, finishSecret. The function stages the new value under AWSPENDING, and only on finishSecret does AWSCURRENT move to the new version. Two consequences are gradable only from the data plane: at every instant before the promotion the new token is staged AWSPENDING (never AWSCURRENT), and a concurrent reader calling GetSecretValue throughout a rotation is never served a broken/absent current credential.
Emulator: the rotation Lambda is never run — the emulator has no in-process runtime for a deployment package (open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records that lambda Invoke answers NotImplemented for a real package), and RotateSecret does not advance the staging labels itself. The rotation therefore never progresses: neither check ever observes AWSPENDING appearing or AWSCURRENT moving, and both run to the suite's 60 s cap.
Why: rotation is the entire point of a rotating secret. A client sees RotateSecret accepted and a secret that never rotates, and the two safety invariants it most needs to trust (no reader ever sees a broken current credential; the candidate is only ever AWSPENDING until promotion) are unobservable.
Fix: the RotateSecret handler's step driver (invoke the rotation function per step and move AWSPENDING/AWSCURRENT on finishSecret) and the Lambda invocation runtime it depends on.
Traces: lambda__logs__secretsmanager.deep-secrets-and-params-rotation-token-is-staged-awspending-at-every-step-before-awscurrent-moves, lambda__logs__secretsmanager.concurrent-reader-during-rotation-never-observes-a-broken-current-credential.

## Evidence
Both checks report 'Check timed out after 60.0s'. The rotation function (which calls PutSecretValue with VersionStages=['AWSPENDING'] on createSecret and UpdateSecretVersionStage on finishSecret) is never invoked, so neither the AWSPENDING staging nor the AWSCURRENT promotion is ever visible to DescribeSecret / GetSecretValue. The concurrent-reader check asserts its invariants at every instant and converges only over a 15 s tail past the promotion (_CONVERGENCE_LAG_S=15.0, _CONVERGENCE_WINDOW_S=15.0), so a rotation that completed would finish well inside the window; the cap was reached because no promotion ever occurred.

## Steps to Reproduce
secretsmanager CreateSecret; create a rotation Lambda implementing the four-step protocol; secretsmanager RotateSecret(SecretId, RotationLambdaARN=<function ARN>, RotationRules=...); poll DescribeSecret for a VersionIdsToStages entry carrying AWSPENDING and then for AWSCURRENT moving to the new version id — neither ever happens, and GetSecretValue keeps returning the original version.
