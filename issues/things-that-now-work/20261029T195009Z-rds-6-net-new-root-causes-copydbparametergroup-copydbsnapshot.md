Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.copy-parameter-group rds.copy-snapshot-contract rds.create-read-replica-cross-engine-mismatch-rejected rds.read-replica-requires-source-backups rds.delete-integration-boundary rds.delete-default-parameter-group-rejected rds.describe-parameter-groups-list-all rds.describe-option-groups-list-all rds.describe-parameters-pagination rds.create-custom-engine-version-contract rds.custom-engine-version-lifecycle rds.custom-engine-version-manifest-round-trip
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.copy-parameter-group|run#8|AssertionError|assert code == ERR_DB_PARAMETER_GROUP_NOT_FOUND, f"unexpected error code: {code!r}"
Issue-Key: rds.copy-snapshot-contract|run#4|AssertionError|assert code == ERR_DB_SNAPSHOT_NOT_FOUND, f"expected {ERR_DB_SNAPSHOT_NOT_FOUND}, got {code!r}"
Issue-Key: rds.create-read-replica-cross-engine-mismatch-rejected|run#13|AssertionError|assert code == "InvalidParameterCombination", f"unexpected error code: {code!r}"
Issue-Key: rds.read-replica-requires-source-backups|run#10|AssertionError|assert code == "InvalidDBInstanceState", f"expected InvalidDBInstanceState, got {code!r}"
Issue-Key: rds.delete-integration-boundary|run#4|AssertionError|assert code == ERR_INTEGRATION_NOT_FOUND, (
Issue-Key: rds|DescribeDBParameterGroups|DBParameterGroupNotFound
Issue-Key: rds.describe-parameter-groups-list-all|run#13|AssertionError|assert any(g != name for g in by_name), f"expected pre-existing default groups too, saw only {sorted(by_name)}"
Issue-Key: rds.describe-option-groups-list-all|run#4|AssertionError|assert len(groups) > 1, f"expected multiple catalog option groups, got {groups}"
Issue-Key: rds.describe-parameters-pagination|run#6|AssertionError|assert len(first["Parameters"]) == 20, (
Issue-Key: rds.create-custom-engine-version-contract|run#4|AssertionError|assert control_error.get("Code") != ERR_INVALID_PARAMETER_VALUE or (
Issue-Key: rds.custom-engine-version-lifecycle|_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: rds.custom-engine-version-manifest-round-trip|run#5|AssertionError|assert code == "InvalidParameterValue", (
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rds: 6 net-new root causes — CopyDBParameterGroup/CopyDBSnapshot/CreateDBInstanceReadReplica/DeleteIntegration answer 501 NotImplemented, the account has no default parameter-group/option-group catalog, and CreateCustomDBEngineVersion validates in the wrong order

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstanceReadReplica

## Description
Six distinct root causes in the rds crate: four operations still answer the scaffold's NotImplemented, the AWS-managed catalog every account ships is missing, and the custom-engine-version path refuses for the wrong reason. None of these failures is marked ALREADY FILED or OWNED in the replay report (the auto-filed stub issues in this run cover DeleteCustomDBEngineVersion, DescribeDBRecommendations, DescribeEvents, ResetDBParameterGroup and neighbouring ec2/logs stubs — not these).

Cause 1 of 6: CopyDBParameterGroup is an unimplemented scaffold stub.
AWS: Smithy: CopyDBParameterGroup takes SourceDBParameterGroupIdentifier / TargetDBParameterGroupIdentifier / TargetDBParameterGroupDescription (all required) and errors with DBParameterGroupNotFoundFault (awsQueryError DBParameterGroupNotFound, HTTP 404) for a source that does not exist, DBParameterGroupAlreadyExistsFault / DBParameterGroupQuotaExceededFault otherwise; a valid copy returns the new DBParameterGroup.
Emulator: every call is refused with code 'NotImplemented' ("operation ... is scaffolded but not implemented in this emulator"), so neither the not-found refusal nor the copy itself is reachable.
Why: the whole copy-a-parameter-group flow is unavailable, and the not-found class is unobservable.
Fix: implement the CopyDBParameterGroup handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/) over parameter-group state.
Traces: rds.copy-parameter-group

Cause 2 of 6: CopyDBSnapshot is an unimplemented scaffold stub.
AWS: Smithy: CopyDBSnapshot errors include DBSnapshotNotFoundFault (awsQueryError DBSnapshotNotFound, 404) for an unknown SourceDBSnapshotIdentifier, plus DBSnapshotAlreadyExistsFault, InvalidDBSnapshotStateFault, KMSKeyNotAccessibleFault, SnapshotQuotaExceededFault.
Emulator: code 'NotImplemented' instead of DBSnapshotNotFound.
Why: snapshot copy (and its not-found/malformed-identifier classes) cannot be exercised at all.
Fix: implement the CopyDBSnapshot handler over snapshot state.
Traces: rds.copy-snapshot-contract

Cause 3 of 6: CreateDBInstanceReadReplica is an unimplemented scaffold stub.
AWS: Smithy: CreateDBInstanceReadReplica errors include InvalidDBInstanceStateFault (awsQueryError InvalidDBInstanceState, 400). When the source instance has BackupRetentionPeriod == 0 AWS refuses with InvalidDBInstanceState and the fixed message "Automated backups are not enabled for this database instance. To enable automated backups, use ModifyDBInstance to set the backup retention period to a non-zero value."; after ModifyDBInstance re-enables backups the identical call succeeds and the replica reports ReadReplicaSourceDBInstanceIdentifier = the source id. A cross-engine source/replica mismatch is likewise refused by RDS, not by the scaffold.
Emulator: both the rejection leg and the success leg answer code 'NotImplemented'.
Why: read replicas are entirely unavailable, and two documented refusal classes are unobservable.
Fix: implement the CreateDBInstanceReadReplica handler, including the source-state/backup-retention gate.
Traces: rds.read-replica-requires-source-backups, rds.create-read-replica-cross-engine-mismatch-rejected

Cause 4 of 6: DeleteIntegration is an unimplemented scaffold stub.
AWS: Smithy: DeleteIntegration(IntegrationIdentifier) errors IntegrationNotFoundFault (404), IntegrationConflictOperationFault, InvalidIntegrationStateFault; an unknown identifier gets IntegrationNotFoundFault.
Emulator: code 'NotImplemented' instead of IntegrationNotFoundFault.
Why: zero-ETL integration deletes cannot be modelled and the not-found boundary is unobservable.
Fix: implement the DeleteIntegration handler over integration state.
Traces: rds.delete-integration-boundary

Cause 5 of 6: the account ships no AWS-managed parameter-group/option-group catalog, and a new parameter group carries a stub parameter set.
AWS: every account/region has default.<family> DB parameter groups and default option groups pre-created. Consequences the checks assert: DescribeDBParameterGroups(DBParameterGroupName='default.<family>') describes that group (family from DescribeDBEngineVersions(Engine='postgres', DefaultOnly=True)); DeleteDBParameterGroup on it is refused with InvalidDBParameterGroupState and the message "Default DBParameterGroup cannot be deleted: " + the group name; an unfiltered DescribeDBParameterGroups lists those defaults alongside user-created groups; an unfiltered DescribeOptionGroups returns more than one group spanning more than one EngineName, each with OptionGroupName/EngineName/MajorEngineVersion/OptionGroupArn ending ':og:<name>' and a boolean AllowsVpcAndNonVpcInstanceMemberships; and DescribeDBParameters on a mysql8.0 group returns hundreds of parameters, so a MaxRecords=20 page is exactly 20 records and carries a continuation Marker.
Emulator: DescribeDBParameterGroups(default.postgres17) answers DBParameterGroupNotFound 'DBParameterGroup not found: default.postgres17'; unfiltered DescribeDBParameterGroups returns only the group the check just created; DescribeOptionGroups returns []; DescribeDBParameters(MaxRecords=20) on a new mysql8.0 group returns 14 parameters (no full page, no Marker).
Why: anything that references a default group or enumerates the catalog (CloudFormation templates, console-equivalent flows, parameter-tuning tooling, paginators) behaves differently than on AWS, and the default-group delete refusal is unreachable.
Fix: the rds crate's catalog/state seeding for default DB parameter groups, default option groups and per-family parameter definitions, plus the DeleteDBParameterGroup default-group gate.
Traces: rds.delete-default-parameter-group-rejected, rds.describe-parameter-groups-list-all, rds.describe-option-groups-list-all, rds.describe-parameters-pagination

Cause 6 of 6: CreateCustomDBEngineVersion refuses in the wrong order and with non-AWS messages.
AWS: Smithy: CreateCustomDBEngineVersion(Engine, EngineVersion required; optional DatabaseInstallationFilesS3BucketName/Prefix, ImageId, KMSKeyId, UseAwsProvidedLatestImage, Manifest) errors CreateCustomDBEngineVersionFault, KMSKeyNotAccessibleFault, Ec2ImagePropertiesNotSupportedFault, etc. Recorded AWS behavior: the engine-availability gate fires FIRST — for Engine='custom-oracle-ee' in an account not onboarded for RDS Custom for Oracle, AWS answers InvalidParameterValue with the fixed message "Invalid DB engine" even when a fabricated KMSKeyId and a nonexistent installation-media bucket are supplied; for a recognized custom family (custom-sqlserver-se) with a well-formed EngineVersion the call is NOT refused as if the Engine/EngineVersion pair were invalid (AWS refuses later, for the missing image). DescribeDBEngineVersions(Engine='custom-oracle-ee') is then either an empty list or InvalidParameterValue "Unrecognized engine name: custom-oracle-ee".
Emulator: (a) for custom-sqlserver-se it answers InvalidParameterValue 'KMSKeyId must be specified for engine custom-sqlserver-se: a symmetric encryption KMS key is required for an RDS Custom engine version' — a KMS-key precondition AWS does not apply at that point, worded in emulator-invented text that names the engine version; (b) for custom-oracle-ee with a bogus key ARN and bucket the call SUCCEEDS (returned None/2xx) where AWS answers InvalidParameterValue 'Invalid DB engine'; (c) in the manifest round-trip it answers InvalidParameterCombination 'UseAwsProvidedLatestImage and ImageId cannot both be specified: the service-provided latest AMI and a named AMI are two different images' where AWS answers InvalidParameterValue for the unavailable engine.
Why: callers see a different error code and class than AWS for the same request, and one request AWS refuses outright is accepted, creating CEV state AWS would never create.
Fix: CreateCustomDBEngineVersion's validation order and refusal wording in its handler — engine availability must be gated before KMS-key/image/manifest combination checks.
Traces: rds.create-custom-engine-version-contract, rds.custom-engine-version-lifecycle, rds.custom-engine-version-manifest-round-trip

## Evidence
All quoted from the behavioral replay report (suite simparity-behavioral-aws-v1).

Cause 1 — rds.copy-parameter-group [mismatch]: AssertionError: unexpected error code: 'NotImplemented' (call: rds.copy_db_parameter_group(SourceDBParameterGroupIdentifier=<bogus>, ...)).
Cause 2 — rds.copy-snapshot-contract [mismatch]: AssertionError: expected DBSnapshotNotFound, got 'NotImplemented' (call: rds.copy_db_snapshot(SourceDBSnapshotIdentifier=<bogus>, ...)).
Cause 3 — rds.read-replica-requires-source-backups [mismatch]: AssertionError: expected InvalidDBInstanceState, got 'NotImplemented'; the check then asserts message == "Automated backups are not enabled for this database instance. To enable automated backups, use ModifyDBInstance to set the backup retention period to a non-zero value." and that the identical call succeeds after backups are re-enabled. rds.create-read-replica-cross-engine-mismatch-rejected [mismatch]: AssertionError: unexpected error code: 'NotImplemented' (call: rds.create_db_instance_read_replica(...)).
Cause 4 — rds.delete-integration-boundary [mismatch]: AssertionError: expected IntegrationNotFoundFault, got NotImplemented (call: rds.delete_integration(IntegrationIdentifier=<bogus>)).
Cause 5 — rds.delete-default-parameter-group-rejected [error]: botocore.errorfactory.DBParameterGroupNotFoundFault: An error occurred (DBParameterGroupNotFound) when calling the DescribeDBParameterGroups operation: DBParameterGroup not found: default.postgres17 (the check needs default.<family> to exist, then expects InvalidDBParameterGroupState with 'Default DBParameterGroup cannot be deleted: default.postgres17'). rds.describe-parameter-groups-list-all [mismatch]: AssertionError: expected pre-existing default groups too, saw only ['sim2real-behav-pg-listall-2f4a82645f5a']. rds.describe-option-groups-list-all [mismatch]: AssertionError: expected multiple catalog option groups, got []. rds.describe-parameters-pagination [mismatch]: AssertionError: expected exactly 20 parameters on the first page (MaxRecords=20), got 14.
Cause 6 — rds.create-custom-engine-version-contract [mismatch]: AssertionError: a real custom engine family (custom-sqlserver-se) with a properly formatted EngineVersion was rejected as if the Engine/EngineVersion pair itself were invalid ...: {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'KMSKeyId must be specified for engine custom-sqlserver-se: a symmetric encryption KMS key is required for an RDS Custom engine version'}. rds.custom-engine-version-lifecycle [mismatch]: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned None (expected InvalidParameterValue / 'Invalid DB engine' for Engine='custom-oracle-ee' with a fabricated KMSKeyId and nonexistent media bucket). rds.custom-engine-version-manifest-round-trip [mismatch]: AssertionError: expected InvalidParameterValue for Engine='custom-oracle-ee', got 'InvalidParameterCombination': 'UseAwsProvidedLatestImage and ImageId cannot both be specified: the service-provided latest AMI and a named AMI are two different images'.

Smithy citations (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json): CopyDBParameterGroup errors DBParameterGroupNotFoundFault(404)/DBParameterGroupAlreadyExistsFault/DBParameterGroupQuotaExceededFault; CopyDBSnapshot errors DBSnapshotNotFoundFault(404)/InvalidDBSnapshotStateFault/KMSKeyNotAccessibleFault; CreateDBInstanceReadReplica errors InvalidDBInstanceStateFault(400)/DBInstanceNotFoundFault(404); DeleteIntegration errors IntegrationNotFoundFault(404)/InvalidIntegrationStateFault; CreateCustomDBEngineVersion errors CreateCustomDBEngineVersionFault/KMSKeyNotAccessibleFault/Ec2ImagePropertiesNotSupportedFault.

## Steps to Reproduce
Build and boot the emulator, then replay:
  rds.copy-parameter-group
  rds.copy-snapshot-contract
  rds.create-read-replica-cross-engine-mismatch-rejected
  rds.read-replica-requires-source-backups
  rds.delete-integration-boundary
  rds.delete-default-parameter-group-rejected
  rds.describe-parameter-groups-list-all
  rds.describe-option-groups-list-all
  rds.describe-parameters-pagination
  rds.create-custom-engine-version-contract
  rds.custom-engine-version-lifecycle
  rds.custom-engine-version-manifest-round-trip

Manually, against the emulator's rds endpoint:
1. rds.copy_db_parameter_group(SourceDBParameterGroupIdentifier=<nonexistent>, TargetDBParameterGroupIdentifier=<new>, TargetDBParameterGroupDescription='x') -> expect DBParameterGroupNotFound, not NotImplemented; then copy a real group and expect success.
2. rds.copy_db_snapshot(SourceDBSnapshotIdentifier=<nonexistent>, TargetDBSnapshotIdentifier=<new>) -> expect DBSnapshotNotFound.
3. Create an instance with BackupRetentionPeriod=0, wait until available, then create_db_instance_read_replica(SourceDBInstanceIdentifier=<it>) -> expect InvalidDBInstanceState with AWS's automated-backups message; re-enable backups via modify_db_instance and retry -> expect success with ReadReplicaSourceDBInstanceIdentifier set.
4. rds.delete_integration(IntegrationIdentifier=<nonexistent>) -> expect IntegrationNotFoundFault.
5. rds.describe_db_engine_versions(Engine='postgres', DefaultOnly=True) -> take DBParameterGroupFamily; describe_db_parameter_groups(DBParameterGroupName='default.<family>') -> expect it to exist; delete_db_parameter_group on it -> expect InvalidDBParameterGroupState 'Default DBParameterGroup cannot be deleted: default.<family>'; describe_db_parameter_groups() unfiltered -> expect defaults present; describe_option_groups() -> expect >1 group spanning >1 engine; create a mysql8.0 parameter group and describe_db_parameters(MaxRecords=20) -> expect 20 records plus a Marker.
6. rds.create_custom_db_engine_version(Engine='custom-sqlserver-se', EngineVersion='15.00.4073.23.<suffix>') -> must not be refused as an invalid Engine/EngineVersion pair; create_custom_db_engine_version(Engine='custom-oracle-ee', EngineVersion='19.<suffix>', KMSKeyId=<fabricated arn>, DatabaseInstallationFilesS3BucketName=<nonexistent>) -> expect InvalidParameterValue 'Invalid DB engine'.
