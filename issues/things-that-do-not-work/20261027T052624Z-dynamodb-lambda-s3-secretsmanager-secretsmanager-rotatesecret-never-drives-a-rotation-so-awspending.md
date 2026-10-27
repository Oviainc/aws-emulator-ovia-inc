Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.cli-rotate-secret-and-verify-version-stages
Issue-Key: dynamodb__lambda__s3__secretsmanager.cli-rotate-secret-and-verify-version-stages|run#21|Timeout|time.sleep(3)
Issue-Commit: fbdf9d3baa643423c5a7eac120dec57cfd2abcbc

# secretsmanager: RotateSecret never drives a rotation, so AWSPENDING is never promoted to AWSCURRENT and the call never completes

> _Note: this issue was identified against the code base at commit `fbdf9d3baa643423c5a7eac120dec57cfd2abcbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1 — a rotation requested through RotateSecret is accepted but never executed, and the version stages never move.
AWS: RotateSecret invokes the secret's configured rotation Lambda four times, with Step values createSecret, setSecret, testSecret and finishSecret, passing the SecretId and a ClientRequestToken. The handler's finishSecret step calls UpdateSecretVersionStage, after which DescribeSecret/ListSecretVersionIds report the new version carrying stage `AWSCURRENT` and the previously current version demoted to `AWSPREVIOUS`; the rotation completes within seconds for a trivial handler. Recorded behavior only for the end-to-end timing — the Smithy (secretsmanager) model describes the request/response but not the backend's invocation sequence.
Emulator: the check never gets past its rotation wait — it exhausts the harness budget with `Check timed out after 60.0s`. No stage transition is observed, i.e. the rotation backend never drives the createSecret/setSecret/testSecret/finishSecret sequence against the registered Lambda, so AWSPENDING is never created or promoted.
Why: secret rotation is the whole point of a rotation-enabled secret. A client (or `aws secretsmanager rotate-secret`) that waits for AWSCURRENT to move — which is the only way to know a rotation finished — hangs indefinitely, and a credential that the application believes was rotated is still the old one, with no error anywhere to say so.
Fix: the RotateSecret handler's rotation driver in the secretsmanager service — it must invoke the configured RotationLambdaARN for the four documented Steps and let the handler's UpdateSecretVersionStage take effect. Note that this driver depends on Lambda actually executing a deployment package; the open issue `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` reports that Lambda Invoke refuses every real invocation, which is likely the blocker underneath this one, but it does not carry this check and does not cover the secretsmanager-side rotation sequence or the version-stage promotion, which must be fixed here.
Traces: dynamodb__lambda__s3__secretsmanager.cli-rotate-secret-and-verify-version-stages.

## Evidence
Error: Check timed out after 60.0s. The check creates a secret, creates a rotation-permitted role, deploys a rotation Lambda implementing createSecret/setSecret/testSecret/finishSecret, grants secretsmanager.amazonaws.com lambda:InvokeFunction on it, then runs `aws secretsmanager rotate-secret` and waits for the AWSPENDING version to become AWSCURRENT with the old version demoted to AWSPREVIOUS. No stage transition is ever observed within the budget.

## Steps to Reproduce
1. CreateSecret with a SecretString.
2. Deploy a rotation Lambda that implements the createSecret/setSecret/testSecret/finishSecret steps and AddPermission for Principal secretsmanager.amazonaws.com.
3. Call RotateSecret (or `aws secretsmanager rotate-secret`) naming that function as RotationLambdaARN.
4. Poll DescribeSecret / ListSecretVersionIds for a new version carrying AWSCURRENT and the original version carrying AWSPREVIOUS — it never arrives.
