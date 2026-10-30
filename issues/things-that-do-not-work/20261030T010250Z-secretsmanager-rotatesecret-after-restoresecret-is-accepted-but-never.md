Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.rotate-secret-scheduled-deletion-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.rotate-secret-scheduled-deletion-rejected|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Commit: 716594ad03747236be5f362ba3ba8170b7a2d152

# secretsmanager: RotateSecret after RestoreSecret is accepted but never rotates — AWSCURRENT never moves off the pre-rotation version, so the restore-then-rotate path hangs (1 cause)

> _Note: this issue was identified against the code base at commit `716594ad03747236be5f362ba3ba8170b7a2d152`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1 — RotateSecret is accepted (HTTP 200, ARN/Name/VersionId returned) on a secret whose scheduled deletion was just cancelled by RestoreSecret, but no rotation is ever carried out: the staked AWSPENDING candidate is never created by the rotation function and AWSCURRENT never moves to a new version. A client that waits for the rotation's only completion signal therefore waits forever; the check that exercises this path (secretsmanager.rotate-secret-scheduled-deletion-rejected) is killed by the harness with 'Error: Check timed out after 60.0s' while polling DescribeSecret for a moved AWSCURRENT.

AWS: Smithy (services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json) models RotateSecret with input SecretId (SecretIdType, @length 1..2048, required), ClientRequestToken (@idempotencyToken, @length 32..64), RotationLambdaARN (@length 0..2048), RotationRules, RotateImmediately (default documented as true), output ARN/Name/VersionId, and errors InvalidParameterException / InvalidRequestException / ResourceNotFoundException (HTTP 400) and InternalServiceError (500). The operation doc states it "immediately starts a rotation"; InvalidRequestException is documented for, among others, "The secret is scheduled for deletion." So AWS behaves in two phases here, and only the SECOND one diverges:
  (a) While the secret is inside its recovery window, RotateSecret is refused with InvalidRequestException and the fixed message "You can't perform this operation on the secret because it was marked for deletion." (no request values are interpolated), and nothing is mutated — DescribeSecret still lists exactly the one version CreateSecret minted, still carrying AWSCURRENT, and LastRotatedDate stays unset.
  (b) After RestoreSecret cancels the deletion (DescribeSecret.DeletedDate cleared), the IDENTICAL RotateSecret call succeeds and actually rotates: Secrets Manager invokes the configured RotationLambdaARN once per documented Step (createSecret, setSecret, testSecret, finishSecret) passing SecretId, ClientRequestToken and Step, the function's own secretsmanager calls take effect, and within seconds DescribeSecret.VersionIdsToStages contains a version carrying AWSCURRENT whose VersionId differs from the pre-restore version id (the pre-rotation version is relabelled AWSPREVIOUS, AWSPENDING is gone) and LastRotatedDate is set. The version ids and values are per-run; the requirement is the relationship — new AWSCURRENT != the pre-rotation version id.

Emulator: phase (a) is right (the refusal code, the exact message and the untouched version set all match — see MARKED_FOR_DELETION_MESSAGE in services/service-com-amazonaws-secretsmanager-secretsmanager/src/state.rs and the scheduled_for_deletion() branch in src/ops/rotate_secret.rs). Phase (b) diverges: the post-restore RotateSecret answers 200 and configure_rotation/start_rotation only stake AWSPENDING on a freshly minted version id; the rotation function is never invoked, so no version holding a new value appears, AWSCURRENT stays on the version CreateSecret minted, no version ever carries AWSPREVIOUS and LastRotatedDate stays unset. The handler's own module docs state this ("the rotation this call starts never finishes"). The replay therefore never satisfies the poll predicate 'some version other than v1 carries AWSCURRENT' and dies at the 60s harness timeout without reaching its final assertion.

Why: a client relying on AWS's behavior treats the AWSCURRENT stage move as the completion signal of a rotation — credential-rotation jobs, and in particular the restore-then-rotate recovery path (cancel a scheduled deletion, then immediately rotate the credential that was about to be lost) — so against the emulator it either blocks/times out while the API reports success, or, if it does not wait, keeps serving the stale pre-rotation secret value while believing the credential was replaced. The AWSPREVIOUS rollback target also never exists, so rollback logic finds nothing to roll back to. Because the refusal half of the contract passes, the emulator also makes the restore look like it had no effect on rotation at all.

Fix: the RotateSecret handler and the rotation driver of the secretsmanager service (services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/rotate_secret.rs plus the version/staging-label layer in src/state.rs) — a started rotation must drive the four documented Steps against the configured RotationLambdaARN with the request's ClientRequestToken and let the function's secretsmanager calls mutate the secret, and when AWSCURRENT is moved the version it left must be relabelled AWSPREVIOUS, AWSPENDING dropped from the promoted version, and LastRotatedDate stamped. The restore path must leave the secret fully rotatable, not merely accept the call. Related OPEN issues that do NOT carry this check's replay key (so they do not cover it): 20261028T061500Z-secretsmanager-rotatesecret-rotateimmediately-true-never-runs-the-rotation.md, 20261028T011946Z-secretsmanager-rotatesecret-with-rotateimmediately-true-never-executes-the.md, 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending.md and 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md; 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md is likely the blocker underneath the missing invocation, but the stage-move state asserted here is secretsmanager-side.

Traces: secretsmanager.rotate-secret-scheduled-deletion-rejected.

## Evidence
Replay of secretsmanager.rotate-secret-scheduled-deletion-rejected:
  Error: Check timed out after 60.0s
(The check's final step polls DescribeSecret via _poll_until(timeout_s=150.0, interval_s=5.0) for 'any("AWSCURRENT" in s and v != v1)'; the harness 60s budget cuts it off first, so no assertion message is produced. The earlier phase of the same check — the refusal and the 'effect absent' assertions — is passed by the emulator, which locates the divergence after RestoreSecret.)

Expected (AWS), phase (a) — already correct in the emulator:
  RotateSecret(SecretId=<secret ARN>, RotationLambdaARN=<fn ARN>) on a secret inside its recovery window -> HTTP 400, Error.Code "InvalidRequestException", Error.Message exactly "You can't perform this operation on the secret because it was marked for deletion." (entirely fixed text; no request or state value is interpolated). DescribeSecret.VersionIdsToStages still has exactly the single CreateSecret version id, still containing AWSCURRENT; LastRotatedDate absent.

Expected (AWS), phase (b) — diverging:
  RestoreSecret(SecretId=<secret ARN>) -> DescribeSecret.DeletedDate absent. The identical RotateSecret(SecretId=<secret ARN>, RotationLambdaARN=<fn ARN>) -> 200, and within seconds DescribeSecret.VersionIdsToStages contains a version whose stages include AWSCURRENT and whose VersionId differs from the pre-rotation version id (that pre-rotation id now carrying AWSPREVIOUS), i.e. new_current != v1.

Actual (emulator), phase (b):
  RotateSecret returns 200 with ARN/Name/VersionId, and src/ops/rotate_secret.rs only calls secret.configure_rotation(...) and secret.start_rotation(version_id, now) — staking AWSPENDING on a minted id. The rotation function is never invoked for any Step, so DescribeSecret keeps reporting the pre-rotation version id as the sole AWSCURRENT version, no AWSPREVIOUS version ever appears and LastRotatedDate stays unset; the poll predicate is never satisfied and the check is killed at 60s. The handler's own doc comment states the intent: "the rotation this call starts never finishes: no version holding a new secret value appears, AWSCURRENT does not move, LastRotatedDate stays unset".

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com with secretsmanager:* and logs:* , then CreateFunction a standard 4-step rotation handler (createSecret -> PutSecretValue(ClientRequestToken=<event token>, VersionStages=[AWSPENDING]); setSecret -> no-op; testSecret -> GetSecretValue(VersionId=<token>, VersionStage=AWSPENDING); finishSecret -> UpdateSecretVersionStage(VersionStage=AWSCURRENT, MoveToVersionId=<token>, RemoveFromVersionId=<current>)), wait for function_active_v2 and AddPermission(Action=lambda:InvokeFunction, Principal=secretsmanager.amazonaws.com).
2. CreateSecret(Name=<unique>, SecretString="doomed-value"); record VersionId v1.
3. DeleteSecret(SecretId=<ARN>) with the default recovery window (no ForceDeleteWithoutRecovery).
4. RotateSecret(SecretId=<ARN>, RotationLambdaARN=<fn ARN>): expect HTTP 400 InvalidRequestException with message "You can't perform this operation on the secret because it was marked for deletion." (emulator matches), and DescribeSecret still showing only v1 with AWSCURRENT and no LastRotatedDate (emulator matches).
5. RestoreSecret(SecretId=<ARN>); DescribeSecret.DeletedDate must be absent.
6. Issue the IDENTICAL RotateSecret(SecretId=<ARN>, RotationLambdaARN=<fn ARN>) again — it returns 200 on both AWS and the emulator.
7. Poll DescribeSecret(SecretId=<ARN>) for a version other than v1 carrying AWSCURRENT: on AWS it appears within seconds (and v1 becomes AWSPREVIOUS); on the emulator only v1 ever carries AWSCURRENT, so the poll never terminates and the check times out at the 60s harness limit.
