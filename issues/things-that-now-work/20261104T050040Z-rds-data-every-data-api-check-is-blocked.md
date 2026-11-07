Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.aggregate-join-and-window-results-are-computed-by-the-engine rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation rds-data.batch-execute-statement-generated-fields-per-row rds-data.batch-execute-statement-multi-row-insert rds-data.batch-execute-statement-rejects-whole-batch-on-one-failure rds-data.begin-commit-transaction-persists-data rds-data.begin-rollback-transaction-discards-data rds-data.commit-transaction-twice-rejected rds-data.execute-statement-basic-crud rds-data.execute-statement-database-parameter-switches-schema rds-data.execute-statement-fabricated-transaction-id-rejected rds-data.execute-statement-failure-aborts-transaction-and-rollback-discards-writes rds-data.execute-statement-format-records-as-json rds-data.execute-statement-generated-fields-autoincrement rds-data.execute-statement-http-endpoint-disabled-rejected rds-data.execute-statement-result-metadata-nullable-flags rds-data.execute-statement-typed-values-roundtrip rds-data.execute-statement-unassociated-secret-rejected rds-data.formatted-json-records-match-column-records rds-data.missing-iam-permission-access-denied-rejected rds-data.named-parameters-bind-literally-and-reject-unbound-placeholders rds-data.parallel-concurrent-updates-no-lost-updates rds-data.rds-data-batch-execute-statement-in-transaction-discarded-by-rollback-and-persisted-by-commit rds-data.rds-data-batch-update-delete-and-returning-report-empty-update-results rds-data.rds-data-column-metadata-members-and-result-set-options-under-json rds-data.rds-data-concurrent-transactions-independent-and-dead-transaction-ids-refuse-apart rds-data.rds-data-own-cluster-execute-statement-lifecycle rds-data.rds-data-rollback-transaction-on-terminal-transaction-rejected-and-state-kept rds-data.rds-data-type-hint-is-required-for-engine-typed-parameters rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes rds-data.transaction-multiple-statements-same-transaction rds-data.transaction-uncommitted-write-isolation rds-data.typed-value-round-trip
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|CreateDBCluster|NotImplemented
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# rds-data: every Data API check is blocked because CreateDBCluster refuses ManageMasterUserPassword with 501 NotImplemented, so no Aurora cluster with a managed master secret can be provisioned

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
One root cause behind all 34 unowned rds-data failures in this suite.

Cause 1 of 1: CreateDBCluster refuses the ManageMasterUserPassword member with HTTP 501 NotImplemented, so the Aurora Serverless v2 cluster every rds-data check provisions (cluster with the HTTP endpoint enabled + an RDS-managed master secret) can never be created, and no RDS Data API operation of this service is ever reached.

AWS: Smithy (com.amazonaws.rds, CreateDBClusterMessage$ManageMasterUserPassword) documents the member as "Specifies whether to manage the master user password with Amazon Web Services Secrets Manager", valid for Aurora DB clusters and Multi-AZ DB clusters, with the single constraint "Can't manage the master user password with Amazon Web Services Secrets Manager if MasterUserPassword is specified." A request that sets ManageMasterUserPassword=true and omits MasterUserPassword is therefore ACCEPTED: RDS mints the password in Secrets Manager and reports it on the created cluster as DBCluster$MasterUserSecret (shape com.amazonaws.rds#MasterUserSecret, members SecretArn / SecretStatus / KmsKeyId, SecretStatus one of creating|active|rotating|impaired). The response is a 2xx CreateDBClusterResult; no modeled error of CreateDBCluster covers "this member is not supported", and NotImplemented is not a modeled RDS error at all.

Emulator: the whole call is refused. Status is a 501 and the error code is the unmodeled 'NotImplemented', with the message "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead." That message is entirely the emulator's own wording — AWS words no such sentence for this operation; its only variable part is the operation$member prefix, and even the fixed remainder ("is not implemented in this emulator", "Name MasterUserPassword instead") is text AWS never emits.

Why: a client doing exactly what AWS documents (hand RDS the master password management, then read MasterUserSecret.SecretArn and pass it to rds-data as secretArn) gets a 501 with a non-AWS error code. botocore does not model NotImplemented for RDS, so the caller sees a generic ClientError instead of a modeled exception, and the entire managed-master-secret provisioning path — the normal way an Aurora cluster is prepared for the Data API — is unavailable. Every ExecuteStatement / BatchExecuteStatement / BeginTransaction / CommitTransaction / RollbackTransaction behavior in this suite (typed values, generated fields, result metadata, transaction isolation, JSON formatting, and all of the negative classes) is unobservable as a consequence.

Fix: the CreateDBCluster handler's request handling plus the RDS-to-Secrets-Manager managed-secret minting it needs (mint the secret, store it on the cluster, project it as DBCluster$MasterUserSecret), and the removal of the 501 refusal path for this member. Enforce the modeled constraint instead: refuse only the combination ManageMasterUserPassword=true together with MasterUserPassword.

Note on an existing issue: 20261029T195009Z-lambda-rds-secretsmanager-5-net-new-root-causes.md describes the same CreateDBCluster refusal as its Cause 1, but it is filed under the lambda__rds__secretsmanager service group and carries only that group's four replay keys. It does not carry any of the 34 rds-data check ids listed here, so these remain unowned and are filed as this service's instance of the defect.

Traces: all 34 check ids in trace_ids; the error text is byte-identical in every one of them.

## Evidence
From the behavioral replay report (suite simparity-behavioral-aws-v1), service rds-data: 0/34 passed (0%), 34 failed, all with verdict [error].

Identical failure in all 34 checks:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBCluster operation: CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: it has RDS generate the master user password in Amazon Web Services Secrets Manager and report that secret as MasterUserSecret, and this service mints no such secret. Name MasterUserPassword instead.

Expected (AWS): HTTP 200 CreateDBClusterResult whose DBCluster carries MasterUserSecret{SecretArn, SecretStatus, KmsKeyId}; the check then reads SecretArn and uses it as the rds-data secretArn.
Actual (emulator): HTTP 501, Error.Code 'NotImplemented', message as quoted above. The call never creates a cluster, so the check aborts before issuing a single rds-data request.

Smithy citations:
- CreateDBClusterMessage$ManageMasterUserPassword: "Specifies whether to manage the master user password with Amazon Web Services Secrets Manager. ... Valid for Cluster Type: Aurora DB clusters and Multi-AZ DB clusters. Constraints: Can't manage the master user password with Amazon Web Services Secrets Manager if MasterUserPassword is specified."
- com.amazonaws.rds#MasterUserSecret: structure {SecretArn, SecretStatus, KmsKeyId}, "Contains the secret managed by RDS in Amazon Web Services Secrets Manager for the master user password."
- CreateDBCluster declares no error for an unsupported member, and 'NotImplemented' is not an RDS error code.

## Steps to Reproduce
Build and boot the emulator, then replay the 34 checks in trace_ids.

Manually:
1. rds.create_db_subnet_group(...) over the run's network.
2. rds.create_db_cluster(DBClusterIdentifier='c1', Engine='aurora-mysql', MasterUsername='dbadmin', ManageMasterUserPassword=True, EnableHttpEndpoint=True, ServerlessV2ScalingConfiguration={'MinCapacity':0.5,'MaxCapacity':2}, StorageEncrypted=True, DBSubnetGroupName=...) with NO MasterUserPassword.
   Expected: HTTP 200, DBCluster.MasterUserSecret.SecretArn present and SecretStatus in {creating, active}.
   Actual: HTTP 501, (NotImplemented) "CreateDBCluster$ManageMasterUserPassword is not implemented in this emulator: ...".
3. Then rds-data.execute_statement(resourceArn=<DBClusterArn>, secretArn=<MasterUserSecret.SecretArn>, database=..., sql='SELECT 1') — unreachable today because step 2 never returns a cluster.
4. Regression guard: the same create WITH both ManageMasterUserPassword=True and MasterUserPassword set must still be refused, per the member's modeled constraint.
