Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret rds__secretsmanager.refarch-aur-manual-rotate-succeeds-on-rds-managed-secret rds__secretsmanager.refarch-aur-master-secret-appears-only-when-management-is-turned-on
Issue-Key: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret|run#18|AssertionError|assert secret["Name"].startswith("rds!cluster-"), (
Issue-Key: rds__secretsmanager.refarch-aur-manual-rotate-succeeds-on-rds-managed-secret|run#11|AssertionError|assert before.get("OwningService") == "rds", (
Issue-Key: rds__secretsmanager.refarch-aur-master-secret-appears-only-when-management-is-turned-on|run#18|AssertionError|assert resolved.get("OwningService") == "rds", (
Issue-Commit: d2ccc1a1188e008fb4f624824feca16be793f9fa

# rds__secretsmanager: 2 net-new causes — the RDS-managed master user secret is given a caller-mintable name instead of AWS's 'rds!cluster-…', and DescribeSecret reports no OwningService/owning-service tag

> _Note: this issue was identified against the code base at commit `d2ccc1a1188e008fb4f624824feca16be793f9fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
This issue bundles 2 net-new root causes in the rds__secretsmanager service, both grounded in failing behavioural checks the suite marked neither ALREADY FILED nor OWNED. A fixer must close BOTH causes below before submitting.

These are NOT the replay keys of the open issue 20261024T030130Z-kms-rds-secretsmanager-rds-managemasteruserpassword-true-returns-no-masterusersecret-so-no.md (ManageMasterUserPassword=true returning NO MasterUserSecret at all): a MasterUserSecret IS now returned, the cluster reaches SecretStatus 'active', and DescribeSecret resolves the ARN — what diverges is the secret's NAME and its service-ownership metadata. They are also not covered by 20261104T044754Z-rds-secretsmanager-7-net-new-root-causes-create.md, which carries the rotation-field and service-default keys for this service.

=== Cause 1 of 2: the managed master user secret is named with a caller-mintable prefix instead of AWS's reserved 'rds!' form ===
AWS: Smithy: CreateDBCluster.ManageMasterUserPassword — "Specifies whether to manage the master user password with Amazon Web Services Secrets Manager"; the resulting secret is described by rds#MasterUserSecret ("Contains the secret managed by RDS in Amazon Web Services Secrets Manager for the master user password"), and secretsmanager#DescribeSecretResponse$OwningService is "The ID of the service that created this secret" (service-linked secrets). AWS names a service-linked RDS secret with the reserved prefix 'rds!' — 'rds!cluster-<cluster resource id>' for a DB cluster (and 'rds!db-<dbi resource id>' for a DB instance). The '!' is precisely what a caller cannot put in a CreateSecret name, which is how the name proves the secret was minted by RDS and not created independently. The fixed text of the name is the literal 'rds!cluster-' prefix; the variable part is the cluster's own resource id.
Emulator: the secret is named 'rds-cluster-cluster-<cluster resource id>' — a plain hyphen where AWS uses '!', plus a doubled 'cluster-' segment. Nothing about the name distinguishes it from a secret a caller could create, and the recorded name in this run was 'rds-cluster-cluster-00000000000000000000000001' (the digits are this run's generated resource id and are not part of the requirement; the prefix shape is).
Why: a client that identifies RDS-managed credentials by the documented 'rds!' naming convention — audits, tag/name-based IAM policies scoped to 'rds!*', dashboards that separate service-linked secrets from user secrets — classifies the managed master secret as a user-created secret, and a caller can mint a colliding name because the emulator's name contains no reserved character.
Fix: the CreateDBCluster/ModifyDBCluster managed-master-secret minting path in the rds handler — the name it composes when it creates the Secrets Manager secret.
Traces: rds__secretsmanager.refarch-aur-managed-master-password-mints-rotating-rds-owned-secret.

=== Cause 2 of 2: DescribeSecret on the RDS-managed master secret reports no OwningService and no aws:secretsmanager:owningService tag ===
AWS: Smithy: secretsmanager#DescribeSecretResponse$OwningService — "The ID of the service that created this secret" (see 'Secrets managed by other Amazon Web Services services'). For a secret RDS minted for ManageMasterUserPassword, DescribeSecret returns OwningService = "rds", and the secret carries the AWS-applied tags aws:secretsmanager:owningService = "rds" and aws:rds:primaryDBClusterArn = the owning cluster's DBClusterArn. Ownership is what makes Secrets Manager refuse direct writes to the secret (InvalidRequestException), so the field is the gate for that whole behaviour.
Emulator: DescribeSecret on the ARN RDS reported returns OwningService absent (None) and no owning-service/primary-cluster tags; the secret looks like an ordinary, independently created user secret. This is observed both on a cluster created with ManageMasterUserPassword=true and on a cluster where management was turned on later with ModifyDBCluster.
Why: a client cannot tell an RDS-managed credential from a hand-made one; code that branches on OwningService ("only rotate secrets I own", "never write to a service-linked secret") takes the wrong branch, and the service-owned-write protection that depends on ownership cannot be enforced at all.
Fix: the rds managed-master-secret minting path must record the owning service on the secret it creates, and the secretsmanager DescribeSecret serializer/state must carry and emit OwningService plus the AWS-applied aws:secretsmanager:owningService and aws:rds:primaryDBClusterArn tags.
Traces: rds__secretsmanager.refarch-aur-manual-rotate-succeeds-on-rds-managed-secret, rds__secretsmanager.refarch-aur-master-secret-appears-only-when-management-is-turned-on.

## Evidence
Cause 1: AssertionError: a managed MasterUserSecret is named by RDS with an 'rds!cluster-' prefix a caller cannot mint; got 'rds-cluster-cluster-00000000000000000000000001' (check refarch-aur-managed-master-password-mints-rotating-rds-owned-secret, after CreateDBCluster(Engine=aurora-postgresql, ManageMasterUserPassword=True) reached Status=available with MasterUserSecret.SecretStatus=active). Expected: DescribeSecret.Name starts with 'rds!cluster-'.
Cause 2: AssertionError: the managed master secret must be owned by rds, got None (refarch-aur-manual-rotate-succeeds-on-rds-managed-secret) and AssertionError: the secret RDS just minted must be owned by rds, got None (refarch-aur-master-secret-appears-only-when-management-is-turned-on). Expected: DescribeSecret(SecretId=<MasterUserSecret.SecretArn>) returns OwningService="rds", Tags containing aws:secretsmanager:owningService="rds" and aws:rds:primaryDBClusterArn=<DBClusterArn>; actual: OwningService absent and those tags absent.

## Steps to Reproduce
1. CreateDBSubnetGroup over the shared network.
2. CreateDBCluster(DBClusterIdentifier=<name>, Engine=aurora-postgresql, EngineVersion=<newest 16.x>, MasterUsername=dbadmin, ManageMasterUserPassword=true, DatabaseName=appdb, ServerlessV2ScalingConfiguration={MinCapacity:0.5, MaxCapacity:1.0}).
3. Poll DescribeDBClusters until Status=available and MasterUserSecret.SecretStatus=active; take MasterUserSecret.SecretArn and DBClusterArn.
4. Call secretsmanager DescribeSecret(SecretId=<SecretArn>). Observe Name='rds-cluster-cluster-…' (expected 'rds!cluster-<cluster resource id>'), OwningService absent (expected "rds"), and no aws:secretsmanager:owningService / aws:rds:primaryDBClusterArn tags.
5. Repeat with a cluster created with an explicit MasterUserPassword and then ModifyDBCluster(ManageMasterUserPassword=true, ApplyImmediately=true): the newly minted secret has the same missing OwningService.
