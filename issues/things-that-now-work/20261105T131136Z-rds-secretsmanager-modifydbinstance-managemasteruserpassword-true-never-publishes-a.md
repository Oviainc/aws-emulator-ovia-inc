Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.switching-to-managed-master-password-rejects-the-previous-password
Issue-Key: rds__secretsmanager.switching-to-managed-master-password-rejects-the-previous-password|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: e32ddfa48df4fe30ed3566089db5dd37ef687284

# rds__secretsmanager: ModifyDBInstance(ManageMasterUserPassword=true) never publishes a MasterUserSecret, so switching a live instance to RDS-managed admin credentials never completes

> _Note: this issue was identified against the code base at commit `e32ddfa48df4fe30ed3566089db5dd37ef687284`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBInstance

## Description
Cause 1 of 1: turning RDS-managed master credentials ON for an EXISTING DB instance produces no managed secret.
AWS: Smithy: ModifyDBInstanceMessage$ManageMasterUserPassword — "Specifies whether to manage the master user password with Amazon Web Services Secrets Manager"; when it is set to true RDS creates a Secrets Manager secret owned by RDS, publishes it on the instance as DBInstance$MasterUserSecret (SecretArn, KmsKeyId, SecretStatus moving creating -> active), stores a NEW password there (username = the instance's MasterUsername), and the previous master password stops being accepted by the engine — a MySQL login with it is refused with error 1045 and the message "Access denied for user '<master>'@'<client address>' (using password: YES)" (fixed text; the user name and the connecting client address come from the request and the connection).
Emulator: after ModifyDBInstance(ManageMasterUserPassword=true, ApplyImmediately=true) on an available instance, DescribeDBInstances never carries a MasterUserSecret, so the check's poll for the secret never settles and the run is cut off at the harness cap ("Check timed out after 60.0s"); no secret is created in Secrets Manager and the previous password therefore keeps working. Hypothesis: the ModifyDBInstance handler accepts and drops the flag, exactly as the create path does.
Why: a client that migrates an existing database to RDS-managed credentials gets a success response, then finds no secret to read, no rotation and an admin password that was never changed — a silent credential-management failure.
Fix: ModifyDBInstance's ManageMasterUserPassword/RotateMasterUserPassword handling in the rds crate: mint the RDS-owned Secrets Manager secret, publish MasterUserSecret on the instance with SecretStatus settling to active, install the new password on the engine, invalidate the previous one, and retire the secret with the instance.
Related open issue (covers the CREATE path and cluster paths, and does not carry this check id): 20261104T044754Z-rds-secretsmanager-7-net-new-root-causes-create.md.
Traces: rds__secretsmanager.switching-to-managed-master-password-rejects-the-previous-password.

## Evidence
ModifyDBInstance(DBInstanceIdentifier=<live mysql instance>, ManageMasterUserPassword=true, ApplyImmediately=true) is accepted, but DescribeDBInstances never reports DBInstance.MasterUserSecret; the check polling for it ends with "Error: Check timed out after 60.0s" before it can read the secret, log in with the managed password (expected CURRENT_USER() = '<master>@%') or observe the previous password being refused with (1045, "Access denied for user '<master>'@'<client address>' (using password: YES)"). Expected: MasterUserSecret.SecretArn present with SecretStatus active, GetSecretValue returning username=<MasterUsername> and a password different from the previous one.

## Steps to Reproduce
(1) rds CreateDBInstance with Engine=mysql, a MasterUsername/MasterUserPassword and no ManageMasterUserPassword; wait for available and log in over the published endpoint with that password. (2) rds ModifyDBInstance(DBInstanceIdentifier=<id>, ManageMasterUserPassword=true, ApplyImmediately=true). (3) poll rds DescribeDBInstances for DBInstance.MasterUserSecret — none is ever reported. (4) with the ARN AWS would publish, secretsmanager GetSecretValue and log in with that password, and confirm the previous password is now refused with 1045.
