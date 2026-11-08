Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret
Issue-Key: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret|run#20|AssertionError|assert secret.get("RotationEnabled") is True, (
Issue-Commit: 1d306771ef4423b2d5a6d659f0109c3b1fe8eed0

# rds__secretsmanager: the RDS-managed master user secret is minted with rotation never configured — DescribeSecret reports RotationEnabled false and no RotationRules/LastRotatedDate/NextRotationDate

> _Note: this issue was identified against the code base at commit `1d306771ef4423b2d5a6d659f0109c3b1fe8eed0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue carries 1 net-new root cause in the rds__secretsmanager service, grounded in the single failing behavioural check the suite report marked neither ALREADY FILED nor OWNED. A fixer must close the one cause below before submitting.

It is NOT the replay key of any open issue on this surface. 20261104T044754Z-rds-secretsmanager-7-net-new-root-causes-create.md Cause 1 (Create/ModifyDBCluster refusing ManageMasterUserPassword with HTTP 501) no longer reproduces — the cluster is now created, reaches Status=available with MasterUserSecret.SecretStatus=active, and DescribeSecret resolves the ARN; and that issue's Cause 5 is the OPPOSITE projection defect (a hand-created, never-rotated secret must OMIT the rotation fields, keyed on check refarch-aur-never-rotated-secret-omits-every-rotation-field). 20261107T030257Z-rds-secretsmanager-2-net-new-causes-the-rds.md carries the secret's NAME ('rds!cluster-…') and its OwningService/owning-service-tag keys, and the check now runs PAST both of those assertions and fails on the rotation metadata instead. 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md is 'no MasterUserSecret at all', which also no longer reproduces for a cluster.

=== Cause 1 of 1: a secret RDS mints for ManageMasterUserPassword has no rotation configured, so DescribeSecret reports it as a non-rotating secret ===
AWS: Smithy AmazonRDSv19 CreateDBCluster$ManageMasterUserPassword — "Specifies whether to manage the master user password with Amazon Web Services Secrets Manager" — and DBCluster$MasterUserSecret ("Contains the secret managed by RDS in Amazon Web Services Secrets Manager for the master user password"). The secret RDS creates for that flag is a service-linked, RDS-ROTATED secret: RDS configures rotation on it at creation and performs one rotation as part of creating it. In secretsmanager terms (Smithy com.amazonaws.secretsmanager#DescribeSecretResponse): RotationEnabled — "Specifies whether automatic rotation is turned on for this secret. If the secret has never been configured for rotation, Secrets Manager returns null" — comes back true; RotationRules (RotationRulesType$AutomaticallyAfterDays, "The number of days between rotations of the secret", @range 1..1000) reports RDS's managed schedule of AutomaticallyAfterDays = 7; LastRotatedDate — "The last date and time that Secrets Manager rotated the secret. If the secret isn't configured for rotation or rotation has been disabled, Secrets Manager returns null" — is set, because RDS rotates the credential once at creation; NextRotationDate — "The next rotation is scheduled to occur on or before this date. If the secret isn't configured for rotation or rotation has been disabled, Secrets Manager returns null" — is set; and RotationLambdaARN is ABSENT, because RDS-managed rotation uses no caller-visible Lambda. The values of the two dates are per-run clock values and are not part of the requirement; what is required is that they are present and that the schedule is the 7-day managed schedule.
Emulator: DescribeSecret on the ARN reported in DBCluster.MasterUserSecret.SecretArn returns RotationEnabled: false, and (consequently) no managed RotationRules, no LastRotatedDate and no NextRotationDate. The check fails at the first of these: "a managed MasterUserSecret must report RotationEnabled True -- this is the field the task's audit reads to decide whether credentials rotate; got False". The secret is therefore minted as an ordinary static secret that merely holds the credential, with the rotation configuration RDS is responsible for installing never applied.
Why: a client cannot tell whether RDS-managed master credentials rotate. Compliance/audit code reads exactly DescribeSecret.RotationEnabled plus RotationRules.AutomaticallyAfterDays to prove credentials rotate on a bounded schedule, and reads LastRotatedDate/NextRotationDate to prove a rotation has happened and when the next is due; against this emulator every RDS-managed master secret is reported as non-rotating with no schedule and no rotation history, so such an audit fails a configuration AWS would pass, and anything that triggers remediation ("enable rotation on this secret") would act on a secret it must not write to. It also masks the companion false-negative of 'RotationEnabled false' being emitted where AWS returns null.
Fix: the RDS managed-master-secret minting path in the CreateDBCluster/ModifyDBCluster handlers (and the equivalent DB-instance path) must configure rotation on the Secrets Manager secret it creates — the managed 7-day schedule, the initial rotation that sets LastRotatedDate, and the derived NextRotationDate — with no RotationLambdaARN; and the secretsmanager secret state/DescribeSecret projection must carry and emit those rotation members for a service-owned secret (while still omitting them entirely, rather than reporting false, for a secret that never had rotation configured).
Traces: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret.

## Evidence
Quoted from the behavioral replay report (suite simparity-behavioral-aws-v1, service group rds__secretsmanager 5/15 passed):

[mismatch] rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret
  check_rds__secretsmanager_refarch_aur_managed_master_password_mints_rotating_rds_owned_secret.py, line 79, in run
  assert secret.get("RotationEnabled") is True, (
  AssertionError: a managed MasterUserSecret must report RotationEnabled True -- this is the field the task's audit reads to decide whether credentials rotate; got False

Expected (real AWS), for DescribeSecret(SecretId=<DBCluster.MasterUserSecret.SecretArn>) on a cluster created with ManageMasterUserPassword=true:
  RotationEnabled = true
  RotationRules.AutomaticallyAfterDays = 7
  LastRotatedDate present (RDS rotates once at creation)
  NextRotationDate present
  RotationLambdaARN absent
Actual (emulator): RotationEnabled = false, and the schedule/date members that follow it are not reported.

The assertions before line 79 now pass (the cluster reaches Status=available with MasterUserSecret {SecretArn, SecretStatus='active', KmsKeyId}, DescribeSecret resolves the ARN, the name and OwningService assertions are passed), which is what distinguishes this from the already-filed name/OwningService and 501-NotImplemented causes.

## Steps to Reproduce
1. Build and boot the emulator, then replay rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret. Manually:
2. rds.create_db_subnet_group over the shared network.
3. rds.create_db_cluster(DBClusterIdentifier=<name>, Engine='aurora-postgresql', EngineVersion=<newest 16.x from DescribeDBEngineVersions>, MasterUsername='dbadmin', ManageMasterUserPassword=True, DatabaseName='appdb', ServerlessV2ScalingConfiguration={'MinCapacity': 0.5, 'MaxCapacity': 1.0}).
4. Poll rds.describe_db_clusters(DBClusterIdentifier=<name>) until Status='available' and MasterUserSecret.SecretStatus='active'; take MasterUserSecret.SecretArn.
5. Call secretsmanager.describe_secret(SecretId=<SecretArn>).
   Expect: RotationEnabled=True, RotationRules={'AutomaticallyAfterDays': 7}, LastRotatedDate set, NextRotationDate set, and no RotationLambdaARN member.
   Observed: RotationEnabled=False with no RotationRules/LastRotatedDate/NextRotationDate.
