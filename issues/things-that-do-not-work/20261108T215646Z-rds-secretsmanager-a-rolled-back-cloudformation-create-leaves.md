Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.cfn-redeploy-over-a-retained-secret-fails-and-its-rollback-leaves-the-secret-intact
Issue-Key: rds__secretsmanager.cfn-redeploy-over-a-retained-secret-fails-and-its-rollback-leaves-the-secret-intact|_secret_error#29|AssertionError|raise AssertionError(f"DescribeSecret({secret_id!r}) succeeded and returned {got!r}")
Issue-Commit: 8a19514b68a1f686dd21900920ede54cd81f71e4

# rds__secretsmanager: a rolled-back CloudFormation create leaves its DeletionPolicy=Delete secret merely scheduled for deletion, so DescribeSecret still returns it instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `8a19514b68a1f686dd21900920ede54cd81f71e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSecret

## Description
Cause 1 of 1: when CloudFormation rolls back a failed CreateStack, the AWS::SecretsManager::Secret resources it had already created and whose DeletionPolicy is `Delete` must be gone from Secrets Manager; the emulator only schedules them for deletion, leaving them describable (and their names taken). Failing check ids: rds__secretsmanager.cfn-redeploy-over-a-retained-secret-fails-and-its-rollback-leaves-the-secret-intact.

AWS: a stack rollback deletes the resources that stack created. For AWS::SecretsManager::Secret with DeletionPolicy/UpdateReplacePolicy = Delete, the resource is removed outright (the handler deletes without a recovery window — DeleteSecret models exactly that switch: `ForceDeleteWithoutRecovery` (BooleanType) alongside `RecoveryWindowInDays`), so after the rollback the secret's name is free and a read of it is refused. Smithy: DescribeSecret errors include ResourceNotFoundException (client error, httpError 400); the message AWS words for this error class is the fixed sentence `Secrets Manager can't find the specified secret.` with no variable parts — the requested SecretId is NOT echoed into it. Recorded behavior (the AWS-grounded expectation this check replays): after the rolled-back stack, DescribeSecret on the Delete-policy secret's name answers 400 ResourceNotFoundException / `Secrets Manager can't find the specified secret.`

Emulator: DescribeSecret on that name answers HTTP 200 with the full secret — ARN, Name, Description, Tags, CreatedDate, VersionIdsToStages still mapping its version to AWSCURRENT — plus a `DeletedDate` equal to `LastChangedDate`. That is the shape of a secret SCHEDULED for deletion (which is correct for a plain DeleteSecret with a recovery window, but not for what a CloudFormation rollback does), so the rollback path is clearly issuing a recovery-window delete rather than an immediate one, and the secret (and its name) survives the rollback.

Why: a client relying on AWS's behavior sees a stack rollback as leaving nothing behind. Against the emulator the Delete-policy secret keeps existing with its name held, so (a) any redeploy of the same template with the same NamePrefix now collides on the name AWS would have freed, (b) code that probes `DescribeSecret` to decide whether a rollback cleaned up reads 200/present where AWS refuses with ResourceNotFoundException, and (c) secret material the rollback should have destroyed stays readable for the whole emulated recovery window.

Fix: the CloudFormation resource handler for AWS::SecretsManager::Secret — its delete/rollback leg must perform an immediate (no-recovery-window) delete so the secret is removed from Secrets Manager state rather than flagged with DeletedDate, and Secrets Manager's DescribeSecret must then refuse the name with ResourceNotFoundException (400) and the fixed message above. If the handler already asks for a force delete, then the defect is in the Secrets Manager state/DeleteSecret handler, which flags `DeletedDate` instead of purging when ForceDeleteWithoutRecovery is set.

Traces: rds__secretsmanager.cfn-redeploy-over-a-retained-secret-fails-and-its-rollback-leaves-the-secret-intact (the check's other legs — ROLLBACK_COMPLETE, the single RetainedSecret CREATE_FAILED event naming the existing name, and the untouched Retain-policy secret and its value — all passed; only the purge of the Delete-policy secret diverges).

## Evidence
Expected (AWS): after the second, rolled-back CreateStack, `secretsmanager:DescribeSecret(SecretId="<prefix>-dropped")` raises ResourceNotFoundException, HTTP 400, message `Secrets Manager can't find the specified secret.` (fixed text; nothing in it comes from the request).

Actual (emulator): the same call returns HTTP 200 with the secret still present and merely scheduled for deletion, e.g. `{'ARN': 'arn:aws:secretsmanager:<region>:<account>:secret:<prefix>-dropped-<suffix>', 'Name': '<prefix>-dropped', 'Description': 'sim2real refarch-aur secret whose DeletionPolicy is Delete', 'RotationEnabled': False, 'LastChangedDate': <t>, 'DeletedDate': <t>, 'Tags': [...], 'VersionIdsToStages': {'<version>': ['AWSCURRENT']}, 'CreatedDate': <t0>}` (ARN suffix, names and timestamps are per-run values, not requirements). The check's helper therefore raised: `AssertionError: DescribeSecret('<prefix>-dropped') succeeded and returned {...}` at the assertion `the rolled-back stack must leave no '<prefix>-dropped' behind`.

## Steps to Reproduce
1. Deploy a stack from a template with two AWS::SecretsManager::Secret resources named `${NamePrefix}-retained` (DeletionPolicy/UpdateReplacePolicy Retain) and `${NamePrefix}-dropped` (DeletionPolicy/UpdateReplacePolicy Delete); wait for CREATE_COMPLETE.
2. DeleteStack and wait until it is gone — the Retain secret survives, the Delete secret goes away.
3. CreateStack the identical template with the same NamePrefix. The create fails on `${NamePrefix}-retained` already existing and the stack reaches ROLLBACK_COMPLETE.
4. Call secretsmanager DescribeSecret(SecretId="${NamePrefix}-dropped").
   AWS: 400 ResourceNotFoundException, `Secrets Manager can't find the specified secret.`
   Emulator: 200 with the secret still described, carrying DeletedDate (scheduled, not deleted).
