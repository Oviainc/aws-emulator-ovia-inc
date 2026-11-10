Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.rotate-secret-while-in-progress-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.rotate-secret-while-in-progress-rejected|_poll_until#10|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last value: {last!r}")
Issue-Commit: f5d2818b51255ceb308b62fff342c02a5f2a7669

# secretsmanager: RotateSecret with RotateImmediately=true never executes the rotation function, so no AWSPENDING candidate is staged and the in-flight-rotation guard never refuses a second RotateSecret (1 cause)

> _Note: this issue was identified against the code base at commit `f5d2818b51255ceb308b62fff342c02a5f2a7669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1 — RotateSecret(RotateImmediately=true) is accepted (200, with ARN/Name/VersionId) but the configured rotation function is never executed, so no candidate version is ever created or labelled AWSPENDING; as a consequence the state-dependent refusal that AWS gives a second RotateSecret while a rotation is outstanding is never produced either.

AWS: RotateSecret is modeled with input SecretId (required, @length 1..2048), ClientRequestToken (@idempotencyToken, @length 32..64), RotationLambdaARN (@length 0..2048), RotationRules and RotateImmediately, and output ARN/Name/VersionId (Smithy: services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json). With RotateImmediately=true the Secrets Manager backend immediately invokes the secret's rotation function once per step with Step=createSecret, setSecret, testSecret and finishSecret, passing SecretId and the ClientRequestToken; the handler's createSecret step stages the new version under AWSPENDING, so DescribeSecret.VersionIdsToStages reports {<original version>: [AWSCURRENT], <token>: [AWSPENDING]} while the rotation is in flight, and any secretsmanager call the function makes (TagResource, PutSecretValue, GetSecretValue) takes effect on the secret. While that candidate sits on a version other than AWSCURRENT, a further RotateSecret with a new ClientRequestToken is refused with HTTP 400 InvalidRequestException (modeled error on RotateSecret; the shape documents it as "A parameter value is not valid for the current state of the resource") and the fixed message "A previous rotation isn't complete. That rotation will be reattempted." — that message has no variable parts. The refused call must mint nothing: GetSecretValue for the refused token is then 400 ResourceNotFoundException with the message "Secrets Manager can't find the specified secret value for VersionId: <VersionId>", whose only variable part is the VersionId echoed from the request, and VersionIdsToStages must be unchanged. A secret with no outstanding rotation accepts RotateSecret, and DescribeSecret afterwards reports RotationEnabled=true with RotationLambdaARN set to the function named in the call.

Emulator: nothing of the rotation happens. The check calls RotateSecret with RotateImmediately=true and a fresh token against a deployed rotation function, then polls DescribeSecret for the tag that the function writes when it reaches finishSecret; after ~52s the tag list is still only the tag CreateSecret was given, i.e. the function was never invoked for any Step. No AWSPENDING version appears, so the in-flight guard is never armed and the InvalidRequestException refusal above cannot be observed at all. Replay output: AssertionError: condition not met within 51.7759974359999s; last value: [{'Key': 'sim2real', 'Value': 'rotate-secret-while-in-progress-rejected'}].

Why: a client that calls RotateSecret and then waits for the stage move (the only way to learn a rotation finished) hangs forever while believing the credential was rotated, and continues to use the old secret value with no error anywhere. Worse for correctness-sensitive callers, concurrent rotation is silently permitted: because the emulator never holds an in-flight rotation, a second RotateSecret is accepted instead of being refused with InvalidRequestException, so an orchestrator that relies on that refusal to serialize rotations (and on the guarantee that the refused call mints no version and moves no staging label) will fan out overlapping rotations.

Why this is not already covered: the open issue 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending.md reports the promotion of AWSPENDING to AWSCURRENT for a different check and does not carry this check, and 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md bundles authorization/KMS causes; neither carries the in-flight-rotation refusal contract (InvalidRequestException wording, no version minted, no label moved) asserted here. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md is likely the blocker underneath the missing invocation, but the secretsmanager-side rotation driver and the in-progress guard must be fixed here.

Fix: the RotateSecret handler and its rotation driver in the secretsmanager service — it must invoke the configured RotationLambdaARN for the four documented Steps with the request's ClientRequestToken (letting the function's own secretsmanager calls mutate the secret), and its request validation/state check must refuse a RotateSecret while a version other than AWSCURRENT carries AWSPENDING with 400 InvalidRequestException and the exact message above, without creating a version or moving a staging label. DescribeSecret must report RotationEnabled/RotationLambdaARN after a RotateImmediately=false call.

Traces: secretsmanager.rotate-secret-while-in-progress-rejected.

## Evidence
Replay of secretsmanager.rotate-secret-while-in-progress-rejected:
  File "check_secretsmanager_rotate_secret_while_in_progress_rejected.py", line 67, in run — _poll_until(...)
  AssertionError: condition not met within 51.7759974359999s; last value: [{'Key': 'sim2real', 'Value': 'rotate-secret-while-in-progress-rejected'}]

Expected (AWS): after RotateSecret(RotateImmediately=true) the rotation function runs through createSecret..finishSecret, so DescribeSecret shows VersionIdsToStages {<original>: [AWSCURRENT], <token>: [AWSPENDING]} and the function's TagResource call is visible; a second RotateSecret with a new ClientRequestToken is then refused 400 InvalidRequestException / "A previous rotation isn't complete. That rotation will be reattempted.", GetSecretValue(VersionId=<that token>) is 400 ResourceNotFoundException / "Secrets Manager can't find the specified secret value for VersionId: <VersionId>", and VersionIdsToStages is unchanged.

Actual (emulator): RotateSecret returns success, but the rotation function is never invoked for any Step — the secret's tags never gain the tag the function writes at finishSecret, and no AWSPENDING version is ever staged, so the in-flight refusal is never reachable.

## Steps to Reproduce
1. CreateSecret with a SecretString and a tag.
2. Deploy a rotation function that, at Step=createSecret, PutSecretValue with VersionStages=[AWSPENDING] under the event's ClientRequestToken, and at Step=finishSecret calls TagResource on the secret and then fails; grant secretsmanager.amazonaws.com permission to invoke it.
3. Call RotateSecret(SecretId=<name>, ClientRequestToken=<uuid>, RotationLambdaARN=<function>, RotationRules={AutomaticallyAfterDays:30}, RotateImmediately=true).
4. Poll DescribeSecret: the tag written by the function never appears and VersionIdsToStages never gains an AWSPENDING entry (emulator); on AWS both appear within seconds.
5. With the AWSPENDING candidate outstanding, call RotateSecret again with a new ClientRequestToken: AWS refuses it 400 InvalidRequestException "A previous rotation isn't complete. That rotation will be reattempted.", mints no version for that token (GetSecretValue -> ResourceNotFoundException) and moves no staging label.
6. Control: on a fresh secret with no outstanding rotation, RotateSecret(RotateImmediately=false) is accepted and DescribeSecret reports RotationEnabled=true with RotationLambdaARN set.
