Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes
Issue-Key: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes|run#23|AssertionError|assert desc.get("OwningService") == "rds", (
Issue-Commit: a89f2b297f2a633390e709c28e627328b3f2666a

# kms__rds__secretsmanager: an RDS-managed master user secret is not marked as service-owned — DescribeSecret omits OwningService (1 cause)

> _Note: this issue was identified against the code base at commit `a89f2b297f2a633390e709c28e627328b3f2666a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One cause.

Cause 1 of 1: a Secrets Manager secret minted by RDS for ManageMasterUserPassword=true carries no owning service, so DescribeSecret omits `OwningService` instead of reporting `rds`. Failing check: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes.

AWS: Smithy (com.amazonaws.secretsmanager) models DescribeSecretResponse$OwningService (OwningServiceType, @length 1..128) — "The ID of the service that created this secret" (see Secrets managed by other AWS services / service-linked secrets). RDS, when CreateDBInstance is called with ManageMasterUserPassword=true (Smithy AmazonRDSv19: CreateDBInstance$ManageMasterUserPassword, CreateDBInstance$MasterUserSecretKmsKeyId, DBInstance$MasterUserSecret{SecretArn,SecretStatus,KmsKeyId}), creates the master-credential secret itself, so that secret is a service-linked secret whose DescribeSecret response reports OwningService = "rds" — the service's own ID, not a per-run value. Recorded behavior confirms it: the check asserts OwningService == "rds" on the secret named by DBInstance.MasterUserSecret.SecretArn. Because the secret is service-owned, AWS then refuses direct writes to it: PutSecretValue -> HTTP 400 InvalidRequestException with the fixed text "This secret is managed by Amazon RDS (service ID: rds), and you must use that service to update it. For more information, see https://docs.aws.amazon.com/secretsmanager/latest/userguide/service-linked-secrets.html." (the only variable parts are the owning service's display name and service ID), and DeleteSecret(ForceDeleteWithoutRecovery=true) -> HTTP 400 InvalidRequestException "Operation is not allowed on secret owned by rds" (variable part: the owning service id).

Emulator: the instance and the managed secret are now produced — DescribeDBInstances reports MasterUserSecret.SecretArn with SecretStatus 'active' and KmsKeyId equal to the supplied CMK, and secretsmanager DescribeSecret on that ARN reports the same KmsKeyId — but DescribeSecret returns no OwningService member at all. Replay output: `AssertionError: the managed master user secret must be owned by rds; OwningService=None`. The check stops there, so the two service-owned refusals above were never exercised in this run; Hypothesis: they are refused only off the same owning-service marker, so they are expected to be accepted while the marker is unset.

Why: a client cannot distinguish an RDS-managed master-credential secret from a user-created one. Tooling that keys off OwningService (to skip rotation, to avoid direct writes, to render "managed by rds", or to assert the credential is service-linked) treats the secret as customer-owned, and any guard AWS derives from service ownership — the refusal of PutSecretValue and of a force DeleteSecret on an RDS-owned secret — cannot be reproduced against the emulator.

Fix: the RDS CreateDBInstance handler's managed-master-secret minting path (and the matching ModifyDBInstance path) must create the Secrets Manager record with its owning service set to "rds"; the Secrets Manager state record and the DescribeSecret serializer already carry and emit that field, so no change is needed there. The Secrets Manager PutSecretValue and DeleteSecret handlers must then refuse a secret that carries an owning service with the modeled InvalidRequestException and AWS's wording above.

Traces: kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes (fails at the OwningService assertion). Related open issues that do NOT cover this cause: 20261104T044754Z-kms-rds-secretsmanager-createdbinstance-with-managemasteruserpassword-true-mints.md and 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md are about the secret not being minted at all; in this run it IS minted with the right ARN, status and CMK, and only the ownership marker is missing. Code citations: services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/describe_secret.rs:277 (owning_service is read from the stored secret content and serialized, i.e. the value is never populated by the RDS path).

## Evidence
[mismatch] kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes
Error: File "check_kms__rds__secretsmanager_xsvc_rds_2_managed_master_secret_under_cmk_rejects_direct_writes.py", line 110, in run
    assert desc.get("OwningService") == "rds", (
AssertionError: the managed master user secret must be owned by rds; OwningService=None

Expected (AWS): secretsmanager DescribeSecret(SecretId=<DBInstance.MasterUserSecret.SecretArn>) returns OwningService="rds" (HTTP 200), alongside KmsKeyId=<the CMK ARN> and no DeletedDate. Consequently PutSecretValue on that ARN -> HTTP 400, Error.Code=InvalidRequestException, Error.Message="This secret is managed by Amazon RDS (service ID: rds), and you must use that service to update it. For more information, see https://docs.aws.amazon.com/secretsmanager/latest/userguide/service-linked-secrets.html."; DeleteSecret(ForceDeleteWithoutRecovery=true) -> HTTP 400, Error.Code=InvalidRequestException, Error.Message="Operation is not allowed on secret owned by rds". Fixed text in both messages is everything except the owning service's name/ID ("Amazon RDS" / "rds"), which comes from the owning service of the secret in state.
Actual (emulator): DescribeSecret returns HTTP 200 with KmsKeyId correct but the OwningService member absent (botocore surfaces it as missing -> None). The check aborts at that assertion, so the two refusals above are unverified in this run.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. kms: CreateKey (symmetric ENCRYPT_DECRYPT); keep its Arn.
3. ec2/rds: CreateVpc + two subnets in two AZs, then CreateDBSubnetGroup.
4. rds: CreateDBInstance(Engine=postgres, ManageMasterUserPassword=true, MasterUserSecretKmsKeyId=<key Arn>, MasterUsername=sim2real, DBSubnetGroupName=<group>, no MasterUserPassword); wait for DBInstanceStatus=available and MasterUserSecret.SecretStatus=active; note MasterUserSecret.SecretArn.
5. secretsmanager: DescribeSecret(SecretId=<that ARN>) -> expect OwningService="rds"; observed: the member is absent.
6. Then secretsmanager: PutSecretValue(SecretId=<that ARN>, SecretString=...) -> expect HTTP 400 InvalidRequestException ("This secret is managed by Amazon RDS (service ID: rds), ..."); and DeleteSecret(SecretId=<that ARN>, ForceDeleteWithoutRecovery=true) -> expect HTTP 400 InvalidRequestException ("Operation is not allowed on secret owned by rds").
7. Replay kms__rds__secretsmanager.xsvc-rds-2-managed-master-secret-under-cmk-rejects-direct-writes.
