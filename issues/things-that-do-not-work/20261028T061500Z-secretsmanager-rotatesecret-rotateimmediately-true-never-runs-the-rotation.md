Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.e2e-rotation-produces-new-current-version
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.e2e-rotation-produces-new-current-version|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: bd9d06f33b8d9537de0c5b2087b0e7a1f1d0ee12

# secretsmanager: RotateSecret(RotateImmediately=true) never runs the rotation function, so AWSCURRENT never moves to a new version and the old version is never demoted to AWSPREVIOUS (1 cause)

> _Note: this issue was identified against the code base at commit `bd9d06f33b8d9537de0c5b2087b0e7a1f1d0ee12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1 — RotateSecret with RotateImmediately=true is accepted, but the secret's rotation function is never executed, so the end-to-end rotation state machine never completes: no new version is ever staged, AWSCURRENT stays on the version CreateSecret minted, and no version ever carries AWSPREVIOUS. The check that observes this end state (ListSecretVersionIds polled for a moved AWSCURRENT plus a demoted AWSPREVIOUS, then the two values read back by VersionStage) never sees any change and the check is killed at the harness timeout.

AWS: RotateSecret is modeled with input SecretId (required, @length 1..2048), ClientRequestToken (@idempotencyToken, @length 32..64), RotationLambdaARN (@length 0..2048), RotationRules and RotateImmediately, and output ARN/Name/VersionId (Smithy: services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json; errors InvalidParameterException, InvalidRequestException, ResourceNotFoundException, all HTTP 400, and InternalServiceError 500). With RotateImmediately=true Secrets Manager immediately invokes the configured RotationLambdaARN once per documented Step — createSecret, setSecret, testSecret, finishSecret — passing SecretId and the call's ClientRequestToken, and every secretsmanager call that function makes (GetSecretValue, PutSecretValue, DescribeSecret, UpdateSecretVersionStage) takes effect on the secret. The consequence a client observes is a completed stage move: the candidate version the function staged under AWSPENDING at createSecret becomes AWSCURRENT when the function calls UpdateSecretVersionStage(VersionStage=AWSCURRENT, MoveToVersionId=<token>, RemoveFromVersionId=<old current>) at finishSecret, and the version that held AWSCURRENT is automatically relabelled AWSPREVIOUS — so ListSecretVersionIds.Versions then contains one version with VersionStages containing AWSCURRENT whose VersionId is the rotation token (not the original version id) and one with AWSPREVIOUS whose VersionId is the original version id, AWSPENDING is gone, GetSecretValue(VersionStage=AWSCURRENT) returns the value the function wrote and GetSecretValue(VersionStage=AWSPREVIOUS) returns the pre-rotation value, and DescribeSecret reports RotationEnabled=true. The version ids and values themselves are per-run; the requirement is the relationship (new AWSCURRENT != pre-rotation version id, AWSPREVIOUS == pre-rotation version id, each stage serving its own version's value).

Emulator: the rotation never advances at all. After RotateSecret(RotateImmediately=true) against a deployed rotation function that was granted lambda:InvokeFunction for principal secretsmanager.amazonaws.com with SourceArn=<secret ARN>, ListSecretVersionIds keeps reporting exactly the single version created by CreateSecret, still carrying AWSCURRENT; no version ever appears with AWSPENDING or AWSPREVIOUS, so the check's poll predicate (AWSCURRENT moved off the original version id AND an AWSPREVIOUS version exists) is never satisfied and the check dies with 'Error: Check timed out after 60.0s' without ever reaching its value and RotationEnabled assertions. Nothing in the response signals the failure — RotateSecret answers 200.

Why: RotateSecret's only completion signal is the staging-label move, so a client that rotates a credential and waits for AWSCURRENT to change hangs forever (or times out) while the emulator reports success; worse, a client that does not wait keeps serving the stale secret value believing it was rotated, and AWSPREVIOUS — the documented rollback path to the pre-rotation credential — never exists, so rollback logic finds nothing to roll back to.

Fix: the RotateSecret handler and the rotation driver in the secretsmanager service — it must invoke the configured RotationLambdaARN for the four documented Steps with the request's ClientRequestToken and let the function's own secretsmanager calls mutate the secret, and the UpdateSecretVersionStage/state layer must, when AWSCURRENT is moved to another version, relabel the version it left as AWSPREVIOUS and drop AWSPENDING from the promoted version, so ListSecretVersionIds/DescribeSecret report the post-rotation label set and GetSecretValue resolves AWSCURRENT/AWSPREVIOUS to the right versions.

Related open issues that do NOT carry this check's replay key: 20261028T011946Z-secretsmanager-rotatesecret-with-rotateimmediately-true-never-executes-the.md (the in-flight-rotation refusal contract), 20261027T052624Z-dynamodb-lambda-s3-secretsmanager-secretsmanager-rotatesecret-never-drives-a-rotation-so-awspending.md, 20261025T022656Z-lambda-logs-secretsmanager-a-rotation-never-replaces-the.md, 20261024T150703Z-lambda-logs-secretsmanager-a-secrets-manager-rotation-never.md and 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md; 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md is likely the blocker underneath the missing invocation, but the AWSCURRENT->AWSPREVIOUS demotion asserted here is secretsmanager-side state and must be fixed here.

Traces: secretsmanager.e2e-rotation-produces-new-current-version.

## Evidence
Replay of secretsmanager.e2e-rotation-produces-new-current-version:
  Error: Check timed out after 60.0s (the check polls _versions_by_stage() for up to 120s via _poll_until and is cut off by the harness first; it never reaches its assertions).

Expected (AWS): after RotateSecret(SecretId=<secret ARN>, RotationLambdaARN=<fn>, RotationRules={AutomaticallyAfterDays:30}, RotateImmediately=true), within seconds ListSecretVersionIds.Versions contains a version whose VersionStages include AWSCURRENT and whose VersionId differs from the pre-rotation version id, plus a version whose VersionStages include AWSPREVIOUS and whose VersionId equals the pre-rotation version id; GetSecretValue(VersionStage=AWSPREVIOUS).SecretString is the pre-rotation value and GetSecretValue(VersionStage=AWSCURRENT).SecretString is the value the rotation function wrote at createSecret (here the pre-rotation value with the function's own '-rotated' suffix); DescribeSecret.RotationEnabled is true.

Actual (emulator): RotateSecret returns 200, but the rotation function is never invoked for any Step. ListSecretVersionIds keeps returning only the single CreateSecret version, still labelled AWSCURRENT; no AWSPENDING version is ever staged and no version is ever labelled AWSPREVIOUS, so the poll predicate never becomes true and the check times out.

## Steps to Reproduce
1. CreateSecret(Name=<unique>, SecretString="initial-value") and record the VersionId that ListSecretVersionIds reports with VersionStages=[AWSCURRENT].
2. Create an IAM role trusted by lambda.amazonaws.com with secretsmanager:DescribeSecret/GetSecretValue/PutSecretValue/UpdateSecretVersionStage on that secret, and CreateFunction a standard 4-step rotation handler: createSecret -> PutSecretValue(ClientRequestToken=<event token>, VersionStages=[AWSPENDING]) with the AWSCURRENT value plus a suffix; setSecret -> no-op; testSecret -> GetSecretValue(VersionId=<token>, VersionStage=AWSPENDING); finishSecret -> UpdateSecretVersionStage(VersionStage=AWSCURRENT, MoveToVersionId=<token>, RemoveFromVersionId=<current version>).
3. AddPermission on the function: Action=lambda:InvokeFunction, Principal=secretsmanager.amazonaws.com, SourceArn=<secret ARN>.
4. RotateSecret(SecretId=<secret ARN>, RotationLambdaARN=<function ARN>, RotationRules={AutomaticallyAfterDays:30}, RotateImmediately=true).
5. Poll ListSecretVersionIds(SecretId=<secret ARN>): on AWS a new AWSCURRENT (the rotation token) and an AWSPREVIOUS (the original version id) appear within seconds; on the emulator only the original AWSCURRENT version is ever returned and the poll never terminates.
6. GetSecretValue(VersionStage=AWSCURRENT) and GetSecretValue(VersionStage=AWSPREVIOUS), and DescribeSecret: AWS serves the rotated and pre-rotation values respectively and reports RotationEnabled=true; the emulator has no AWSPREVIOUS version to serve.
