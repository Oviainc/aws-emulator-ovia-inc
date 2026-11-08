Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes
Issue-Key: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes|run#31|AssertionError|assert code == _PUT_CODE, (
Issue-Commit: 6b512def31f7223c9d34f1f96480dd1fa7227928

# kms__rds__secretsmanager: PutSecretValue on a service-owned (RDS-managed) master user secret is accepted instead of refused InvalidRequestException (1 cause)

> _Note: this issue was identified against the code base at commit `6b512def31f7223c9d34f1f96480dd1fa7227928`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSecretValue

## Description
Cause 1 of 1: Secrets Manager does not enforce service ownership on direct writes to a secret owned by another service. The emulator already marks an RDS-managed master user secret as service-owned (the check's DescribeSecret assertions on OwningService == "rds" and on MasterUserSecret.KmsKeyId both passed), but PutSecretValue against that secret is executed and mints a new version instead of being refused. Failing check: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes.

AWS: a secret whose DescribeSecret reports OwningService (here "rds", created by CreateDBInstance with ManageMasterUserPassword=true and MasterUserSecretKmsKeyId) may only be modified through its owning service. A direct PutSecretValue is refused with HTTP 400 and error code InvalidRequestException. Smithy: PutSecretValue binds error InvalidRequestException (errorKind client, httpError 400); the modeled documentation of InvalidRequestException names exactly this cause — "The secret is managed by another service, and you must use that service to update it. For more information, see Secrets managed by other Amazon Web Services services". The recorded message for this error class is fixed text with one variable part, the owning service id taken from the secret's state: "This secret is managed by Amazon RDS (service ID: rds), and you must use that service to update it. For more information, see https://docs.aws.amazon.com/secretsmanager/latest/userguide/service-linked-secrets.html." — everything there is fixed text except the service name/id pair ("Amazon RDS" / "rds"), which comes from the secret's OwningService. The refusal must also be inert: AWSCURRENT must not move and no new version id may appear.

Emulator: PutSecretValue(SecretId=<rds!db-... ARN>, SecretString=...) returns HTTP 200 with a fresh VersionId staged AWSCURRENT (observed response carried ARN/Name of the rds!db-… managed secret, a new VersionId and VersionStages ["AWSCURRENT"]). No error code, no 400 — the check reports code='<call succeeded>' status=200.

Why: a client that relies on AWS refusing direct writes to service-linked secrets sees the opposite. The RDS master credential document can be silently overwritten through Secrets Manager, so the password Secrets Manager hands out no longer matches the one RDS provisioned on the instance, and code that treats the InvalidRequestException as the signal to route the change through ModifyDBInstance instead takes the success path. Negative-path tests of service-owned secrets pass against the emulator and fail against AWS.

Fix: the PutSecretValue handler's pre-write validation in the secretsmanager service — before staging a version, reject when the target secret carries an owning service, emitting InvalidRequestException with HTTP 400 and the service-linked-secret message built from the secret's OwningService, and leave the version table and staging labels untouched. The same ownership gate belongs on the other caller-facing mutations of an owned secret (DeleteSecret with ForceDeleteWithoutRecovery=true, UpdateSecret); the check never reached its DeleteSecret step because it aborted at PutSecretValue, so only PutSecretValue is evidenced here — Hypothesis: that gate is missing for those operations too.

Related but not covering this: open issue 20261107T023319Z-kms-rds-secretsmanager-an-rds-managed-master-user.md reports DescribeSecret omitting OwningService; here OwningService is reported correctly and the gap is that nothing consults it on write.

Traces: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes (assertion at line 134 of the check).

## Evidence
Check: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes [mismatch]

Setup that succeeded against the emulator: CreateDBInstance(ManageMasterUserPassword=True, MasterUserSecretKmsKeyId=<CMK arn>) returned a MasterUserSecret.SecretArn; DescribeDBInstances reported MasterUserSecret.SecretStatus=active and KmsKeyId=<CMK arn>; DescribeSecret reported the same KmsKeyId and OwningService='rds' with DeletedDate=None; GetSecretValue returned the master username and a password.

Expected (AWS): PutSecretValue(SecretId=<managed secret ARN>, SecretString={"username":...,"password":"x"}) -> HTTP 400, Error.Code=InvalidRequestException, Error.Message="This secret is managed by Amazon RDS (service ID: rds), and you must use that service to update it. For more information, see https://docs.aws.amazon.com/secretsmanager/latest/userguide/service-linked-secrets.html." and AWSCURRENT unchanged / no new version minted.

Actual (emulator): HTTP 200. Response: {'ARN': 'arn:aws:secretsmanager:<region>:<account>:secret:rds!db-<id>-<suffix>', 'Name': 'rds!db-<id>', 'VersionId': '<new uuid>', 'VersionStages': ['AWSCURRENT']}. Harness report: "PutSecretValue on an RDS-managed secret must be refused with InvalidRequestException; got code='<call succeeded>' status=200 message=''".

Model citation: Smithy PutSecretValue errors include InvalidRequestException (client, httpError 400); InvalidRequestException documentation lists "The secret is managed by another service, and you must use that service to update it."

## Steps to Reproduce
1. kms.CreateKey -> symmetric ENCRYPT_DECRYPT CMK; note its Arn.
2. Create a VPC with two subnets in two AZs and an RDS DB subnet group.
3. rds.CreateDBInstance(Engine=postgres, MasterUsername=sim2real, ManageMasterUserPassword=True, MasterUserSecretKmsKeyId=<CMK arn>, DBSubnetGroupName=<group>) and wait for DBInstanceStatus=available and MasterUserSecret.SecretStatus=active; capture MasterUserSecret.SecretArn.
4. secretsmanager.DescribeSecret(SecretId=<SecretArn>) -> confirm OwningService='rds'.
5. secretsmanager.PutSecretValue(SecretId=<SecretArn>, SecretString='{"username":"sim2real","password":"x"}').
   Expected: ClientError InvalidRequestException with HTTP 400 and the service-linked-secret message; GetSecretValue still reports the pre-call VersionId and ListSecretVersionIds is unchanged.
   Actual: HTTP 200 with a new VersionId staged AWSCURRENT.
