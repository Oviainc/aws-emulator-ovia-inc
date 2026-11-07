Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.concurrent-data-api-and-native-increments-lose-no-update rds__rds-data__secretsmanager.data-api-and-native-mysql-sessions-read-each-others-writes rds__rds-data__secretsmanager.data-api-insert-is-read-by-a-native-postgres-session rds__rds-data__secretsmanager.data-api-open-transaction-is-invisible-to-a-native-session-until-commit rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit rds__rds-data__secretsmanager.execute-statement-with-another-clusters-transaction-id-rejected rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds rds__rds-data__secretsmanager.mysql-repeatable-read-snapshot-ignores-a-later-commit-until-it-ends rds__rds-data__secretsmanager.native-insert-is-read-by-the-data-api-postgres rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed rds__rds-data__secretsmanager.typed-values-agree-between-the-data-api-and-a-native-session
Issue-Key: rds__rds-data__secretsmanager|CreateDBCluster|NotImplemented
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# rds__rds-data__secretsmanager: CreateDBCluster refuses ManageMasterUserPassword with 501 NotImplemented, so no Aurora cluster can be created and all eleven Data-API/native-session consistency checks are blocked

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
One cause.

Cause 1 of 1: CreateDBCluster refuses the whole request when ManageMasterUserPassword is present, with HTTP 501 and code NotImplemented, so every check in this group — all of which stand up an Aurora cluster with an RDS-managed master secret and then compare RDS Data API behaviour against a native database session — fails on its very first call.

AWS: Smithy AmazonRDSv19 models CreateDBCluster$ManageMasterUserPassword (BooleanOptional) and CreateDBCluster$MasterUserSecretKmsKeyId, and DBCluster$MasterUserSecret: MasterUserSecret {SecretArn, SecretStatus (creating|active|rotating|impaired), KmsKeyId}. A CreateDBCluster carrying ManageMasterUserPassword=true and no MasterUserPassword is accepted: RDS generates the master password into a Secrets Manager secret it owns and reports it as DBCluster$MasterUserSecret. The operation's modelled errors contain no NotImplemented, and there is no AWS condition under which this member makes the request unsupported.

Emulator: HTTP 501, code 'NotImplemented', message "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead." — emulator wording for a request AWS accepts.

Why: the modelled managed-master-password contract is unavailable, so a client cannot create a cluster the documented way, and every downstream capability that needs such a cluster (Data API ExecuteStatement/BeginTransaction against a live cluster, transaction visibility, locking and typed-value fidelity between the Data API and a native session) is unreachable. Eleven behaviours in this group reach no verdict for this one reason.

Fix: the CreateDBCluster handler plus the RDS-to-Secrets-Manager managed-secret minting it needs, and the DBCluster projection that must then emit MasterUserSecret. The same 501 is filed for a different service group's checks as 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md (cause 1); that issue does not carry any of the checks below.

Traces: all eleven checks listed in trace_ids.

## Evidence
All eleven failures in service group rds__rds-data__secretsmanager (0/11 passed) carry the identical first-call refusal:

botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBCluster operation: CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead.

Expected: HTTP 200 with a DBCluster whose MasterUserSecret.SecretArn names the RDS-owned secret (SecretStatus settling to 'active').
Actual: HTTP 501 NotImplemented, no cluster created.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. rds: CreateDBSubnetGroup, then Action=CreateDBCluster with Engine=aurora-postgresql (or aurora-mysql), MasterUsername set, ManageMasterUserPassword=true and no MasterUserPassword.
3. Expect HTTP 200 and a DBCluster carrying MasterUserSecret.SecretArn; observed HTTP 501 NotImplemented with the message quoted in the evidence.
4. Replay any of the checks in trace_ids.
