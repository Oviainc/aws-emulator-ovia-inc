Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__rds__secretsmanager.xsvc-rds-2-disabled-secret-cmk-blocks-credential-fetch-not-sql-login
Issue-Key: kms__rds__secretsmanager.xsvc-rds-2-disabled-secret-cmk-blocks-credential-fetch-not-sql-login|_poll_until#17|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# kms__rds__secretsmanager: CreateDBInstance with ManageMasterUserPassword=true mints no managed secret, so MasterUserSecret.SecretStatus never settles to 'active' and the CMK-disabled credential-fetch path is unreachable

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
One cause.

Cause 1 of 1: a DB instance created with ManageMasterUserPassword=true under a customer CMK never reports a MasterUserSecret whose SecretStatus becomes 'active', so the check that proves a disabled CMK blocks the credential fetch (but not an already-open SQL login) never reaches its subject and spends the whole run budget polling.

AWS: Smithy AmazonRDSv19 models CreateDBInstance$ManageMasterUserPassword (BooleanOptional) and CreateDBInstance$MasterUserSecretKmsKeyId, and DBInstance$MasterUserSecret: MasterUserSecret {SecretArn, SecretStatus (creating|active|rotating|impaired), KmsKeyId}. With ManageMasterUserPassword=true RDS generates the master password into a Secrets Manager secret it owns, encrypts it under the named (or default) KMS key, and reports it on the instance; SecretStatus moves creating -> active in bounded time, after which GetSecretValue on that ARN serves the full credential document. Disabling the CMK afterwards makes that fetch fail with a KMS-sourced refusal while an already-authenticated database session keeps working.

Emulator: DescribeDBInstances never reports a MasterUserSecret with SecretStatus 'active' — the check's poll for "MasterUserSecret.SecretStatus to be active" exhausts the budget and raises TimeoutError. This is the same defect already filed for the sibling check as 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md (ManageMasterUserPassword=true returns a DBInstance with no MasterUserSecret member and creates nothing in Secrets Manager); that issue does not carry this check, and the status-never-settles consequence is what blocks it.

Why: a client that opts into RDS-managed master credentials can never obtain the credential — there is no SecretArn to read and no SecretStatus to wait on — so every managed-master-password flow, and every KMS condition layered on the managed secret, is unobservable and any wait-for-active loop hangs forever.

Fix: the CreateDBInstance handler (and the matching ModifyDBInstance path) must mint the RDS-owned secret through the Secrets Manager service state under the resolved MasterUserSecretKmsKeyId and the RDS state module's DBInstance projection must emit MasterUserSecret {SecretArn, SecretStatus, KmsKeyId}, advancing SecretStatus to 'active'.

Traces: kms__rds__secretsmanager.xsvc-rds-2-disabled-secret-cmk-blocks-credential-fetch-not-sql-login.

## Evidence
[error] kms__rds__secretsmanager.xsvc-rds-2-disabled-secret-cmk-blocks-credential-fetch-not-sql-login
Error: File "check_kms__rds__secretsmanager_xsvc_rds_2_disabled_secret_cmk_blocks_credential_fetch_not_sql_login.py", line 77, in run
  inst = _poll_until(
  ...
  raise TimeoutError(f"{description}: the run budget ended first; not graded")
TimeoutError: MasterUserSecret.SecretStatus to be active: the run budget ended first; not graded

Expected: DescribeDBInstances reports MasterUserSecret {SecretArn: <arn of the RDS-owned secret>, SecretStatus: 'active', KmsKeyId: <the CMK>} within the polling window.
Actual: the member never appears at all (see the identical response capture in 20261024T030130Z-…-returns-no-masterusersecret-so-no.md).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. kms: CreateKey (to use as MasterUserSecretKmsKeyId).
3. rds: CreateDBSubnetGroup, then CreateDBInstance with ManageMasterUserPassword=true, MasterUserSecretKmsKeyId=<key>, and no MasterUserPassword.
4. Poll DescribeDBInstances for MasterUserSecret.SecretStatus == 'active' -> expect it within the polling window; observed: no MasterUserSecret member ever appears and the poll times out.
5. Replay kms__rds__secretsmanager.xsvc-rds-2-disabled-secret-cmk-blocks-credential-fetch-not-sql-login.
